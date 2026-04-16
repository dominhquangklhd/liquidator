use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use ethers::contract::abigen;
use ethers::providers::{Http, Provider};
use ethers::types::{Address, H160, U256};
use reqwest::Client;
use serde_json::{json, Value};

use crate::data::asset::Asset;
use crate::data::user::User;
use crate::risk::bucket::RiskBucket;
use crate::risk::engine::{RiskEngine, RiskEngineConfig};
use crate::storage::{HybridStorage, LiquidationTarget};

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
}

abigen!(
    BootstrapAavePool,
    r#"[
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
    ]"#
);

abigen!(
    BootstrapAaveOracle,
    r#"[
        function getAssetPrice(address asset) external view returns (uint256)
    ]"#
);

pub async fn bootstrap_onchain_state(
    engine: &mut RiskEngine,
    storage: Arc<HybridStorage>,
    rpc: Arc<Provider<Http>>,
    chain_id: u64,
    aave_pool_address: H160,
    aave_oracle_address: H160,
    risk_config: &RiskEngineConfig,
) -> Result<()> {
    // Prefer Aave subgraph top users (HF <= 2), then fallback to DB/env.
    let subgraph_users = fetch_top_users_from_subgraph().await.unwrap_or_default();
    let bootstrap_users: Vec<Address> = if !subgraph_users.is_empty() {
        tracing::info!(
            "Bootstrap candidates loaded from subgraph: {} users",
            subgraph_users.len()
        );
        subgraph_users
    } else {
        let db_users = storage.load_all_user_addresses().await.unwrap_or_default();
        let env_users = parse_bootstrap_users_from_env();

        let mut user_set = HashSet::new();
        user_set.extend(db_users);
        user_set.extend(env_users);

        let users: Vec<Address> = user_set.into_iter().collect();
        tracing::warn!(
            "Subgraph returned no users, fallback to DB+env candidates: {} users",
            users.len()
        );
        users
    };
    if bootstrap_users.is_empty() {
        tracing::info!("No bootstrap users available, skip on-chain bootstrap");
        return Ok(());
    }

    let reserve_catalog = reserve_catalog_from_env(chain_id);
    let oracle = BootstrapAaveOracle::new(aave_oracle_address, Arc::clone(&rpc));

    let eth_reserve = reserve_catalog
        .get("WETH")
        .copied()
        .unwrap_or_else(Address::zero);

    let eth_price_usd = if eth_reserve == Address::zero() {
        risk_config.reference_eth_price_usd
    } else {
        let raw = oracle.get_asset_price(eth_reserve).call().await?;
        let usd = u256_to_f64(raw) / 1e8;
        if usd > 0.0 {
            usd
        } else {
            risk_config.reference_eth_price_usd
        }
    };

    let mut assets_loaded = 0usize;
    for (symbol, reserve_addr) in &reserve_catalog {
        let price_usd = oracle
            .get_asset_price(*reserve_addr)
            .call()
            .await
            .map(|v| u256_to_f64(v) / 1e8)
            .unwrap_or_else(|_| fallback_price_usd(symbol.as_str(), eth_price_usd));

        let price_in_eth = if symbol == "ETH" || symbol == "WETH" {
            1.0
        } else if eth_price_usd > 0.0 {
            price_usd / eth_price_usd
        } else {
            price_usd
        };

        let decimals = if symbol == "USDC" || symbol == "USDT" {
            6
        } else {
            18
        };
        engine.assets.insert(
            symbol.to_string(),
            Asset {
                id: symbol.to_string(),
                symbol: symbol.to_string(),
                decimals,
                ltv: 0.80,
                liquidation_threshold: default_liquidation_threshold(symbol, chain_id),
                price_in_eth,
            },
        );
        assets_loaded += 1;
    }

    // Alias ETH for oracle price updates (Oracle emits ETH while Aave reserve is WETH).
    if !engine.assets.contains_key("ETH") {
        engine.assets.insert(
            "ETH".to_string(),
            Asset {
                id: "ETH".to_string(),
                symbol: "ETH".to_string(),
                decimals: 18,
                ltv: 0.80,
                liquidation_threshold: default_liquidation_threshold("ETH", chain_id),
                price_in_eth: 1.0,
            },
        );
        assets_loaded += 1;
    }

    let pool = BootstrapAavePool::new(aave_pool_address, Arc::clone(&rpc));
    let mut candidates: Vec<(String, User, LiquidationTarget)> = Vec::new();

    for user_addr in bootstrap_users {
        let onchain = pool.get_user_account_data(user_addr).call().await;
        let Ok((total_collateral_base, total_debt_base, _available, current_lt_bps, _ltv, hf_raw)) = onchain else {
            tracing::warn!("Skip bootstrap user {:?}: cannot read account data", user_addr);
            continue;
        };

        let collateral_usd = u256_to_f64(total_collateral_base) / 1e8;
        let debt_usd = u256_to_f64(total_debt_base) / 1e8;
        let hf = u256_to_f64(hf_raw) / 1e18;
        let current_lt = (u256_to_f64(current_lt_bps) / 10_000.0).clamp(0.0, 1.0);

        // Keep local risk model aligned with Aave account-level liquidation threshold.
        if current_lt > 0.0 {
            if let Some(mut weth) = engine.assets.get_mut("WETH") {
                weth.liquidation_threshold = current_lt;
            }
            if let Some(mut eth) = engine.assets.get_mut("ETH") {
                eth.liquidation_threshold = current_lt;
            }
        }

        let user_id = format!("{:?}", user_addr);
        let mut user = User::new(user_id.clone());

        if collateral_usd > 0.0 && eth_price_usd > 0.0 {
            user.collateral
                .insert("WETH".to_string(), collateral_usd / eth_price_usd);
        }

        if debt_usd > 0.0 {
            user.debt.insert("USDC".to_string(), debt_usd);
        }

        user.health_factor = hf;
        user.risk_bucket = RiskBucket::from_hf(hf);

        let ltv = if collateral_usd > 0.0 {
            debt_usd / collateral_usd
        } else {
            0.0
        };

        let risk_score = ((risk_config.risk_score_hf_baseline - hf)
            / risk_config.risk_score_hf_span.max(f64::EPSILON)
            * risk_config.risk_score_max)
            .clamp(risk_config.risk_score_min, risk_config.risk_score_max) as u8;

        let mut collateral_map = HashMap::new();
        if collateral_usd > 0.0 && eth_price_usd > 0.0 {
            collateral_map.insert("WETH".to_string(), collateral_usd / eth_price_usd);
        }

        let mut debt_map = HashMap::new();
        if debt_usd > 0.0 {
            debt_map.insert("USDC".to_string(), debt_usd);
        }

        let target = LiquidationTarget {
            user_address: user_id.clone(),
            health_factor: hf,
            total_collateral_usd: collateral_usd,
            total_debt_usd: debt_usd,
            ltv,
            liquidation_threshold: risk_config.default_liquidation_threshold,
            collateral: collateral_map,
            debt: debt_map,
            estimated_profit: 0.0,
            risk_score,
            last_updated: chrono::Utc::now().timestamp(),
        };

        if hf <= 2.0 {
            candidates.push((user_id, user, target));
        }
    }

    candidates.sort_by(|a, b| {
        a.2.health_factor
            .partial_cmp(&b.2.health_factor)
            .unwrap_or(Ordering::Equal)
    });

    if candidates.len() > 100 {
        candidates.truncate(100);
    }

    let mut persisted_targets = Vec::with_capacity(candidates.len());
    for (user_id, user, target) in candidates {
        if user.collateral.contains_key("WETH") {
            engine
                .registry
                .add_user_to_asset("WETH".to_string(), user_id.clone());
            // Also track ETH so oracle ETH updates can trigger recalculation.
            engine
                .registry
                .add_user_to_asset("ETH".to_string(), user_id.clone());
        }
        if user.debt.contains_key("USDC") {
            engine
                .registry
                .add_user_to_asset("USDC".to_string(), user_id.clone());
        }

        engine.users.insert(user_id, user);

        if let Err(e) = storage.update_user_hf(target.clone()).await {
            tracing::warn!("Failed to update bootstrap target in hot cache: {:?}", e);
        }
        persisted_targets.push(target);
    }

    if let Err(e) = storage.persist_targets_to_db(&persisted_targets).await {
        tracing::warn!("Failed to persist bootstrap targets to SQLite: {:?}", e);
    }

    tracing::info!(
        "On-chain bootstrap complete: {} assets, {} users loaded (HF <= 2.0, top 100)",
        assets_loaded,
        persisted_targets.len()
    );

    Ok(())
}

fn parse_bootstrap_users_from_env() -> Vec<Address> {
    let from_env = std::env::var("BOOTSTRAP_USERS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<Address>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if from_env.is_empty() {
        // Fallback to borrower used by local setup scripts.
        vec![
            "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
                .parse()
                .expect("valid fallback bootstrap user"),
        ]
    } else {
        from_env
    }
}

async fn fetch_top_users_from_subgraph() -> Result<Vec<Address>> {
    let endpoint = std::env::var("AAVE_SUBGRAPH_URL")
        .unwrap_or_else(|_| "https://api.thegraph.com/subgraphs/name/messari/aave-v3-ethereum".to_string());
    let client = Client::new();

    // First try rich account positions query and prefilter likely risky users (HF <= 2)
    // to reduce expensive on-chain calls during bootstrap.
    let rich_query = r#"query BootstrapUsersRich {
        accounts(first: 1000) {
            id
            positions(first: 50) {
                balance
                market {
                    liquidationThreshold
                    inputToken {
                        decimals
                        lastPriceUSD
                    }
                }
            }
        }
    }"#;

    let rich_body = json!({ "query": rich_query });
    if let Ok(resp) = client.post(&endpoint).json(&rich_body).send().await {
        if resp.status().is_success() {
            if let Ok(value) = resp.json::<Value>().await {
                if value.get("errors").is_none() {
                    let prefiltered = parse_prefiltered_accounts_from_positions(&value);
                    if !prefiltered.is_empty() {
                        tracing::info!(
                            "Subgraph prefilter selected {} likely risky users (HF <= 2) from positions",
                            prefiltered.len()
                        );
                        return Ok(prefiltered);
                    }
                }
            }
        }
    }

    // Fallback: common schema variants used by Aave/community subgraphs.
    let queries = [
        // Schema 1: Aave official (users with borrows)
        r#"query BootstrapUsers {
            borrows(first: 1000, orderBy: blockNumber, orderDirection: desc) {
                user {
                    id
                }
            }
        }"#,
        // Schema 2: Messari (accounts with positions)
        r#"query BootstrapUsers {
            accounts(first: 1000, where: { positions_: { balance_gt: "0" } }) {
                id
            }
        }"#,
        // Schema 3: Positions with balance
        r#"query BootstrapUsers {
            positions(first: 1000, where: { balance_gt: "0" }) {
                account {
                    id
                }
            }
        }"#,
    ];

    for query in queries {
        let body = json!({ "query": query });
        let resp = client.post(&endpoint).json(&body).send().await?;
        if !resp.status().is_success() {
            continue;
        }

        let value: Value = resp.json().await?;
        if let Some(errors) = value.get("errors") {
            tracing::debug!("Subgraph query returned errors: {:?}", errors);
            continue;
        }

        let mut out = Vec::new();
        
        // Try parsing borrows (user.id nested)
        if let Some(items) = value
            .get("data")
            .and_then(|d| d.get("borrows"))
            .and_then(|v| v.as_array())
        {
            for item in items {
                if let Some(id) = item
                    .get("user")
                    .and_then(|u| u.get("id"))
                    .and_then(|v| v.as_str())
                {
                    if let Ok(addr) = id.parse::<Address>() {
                        out.push(addr);
                    }
                }
            }
        }

        // Try parsing accounts (direct id)
        if out.is_empty() {
            if let Some(items) = value
                .get("data")
                .and_then(|d| d.get("accounts"))
                .and_then(|v| v.as_array())
            {
                for item in items {
                    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        if let Ok(addr) = id.parse::<Address>() {
                            out.push(addr);
                        }
                    }
                }
            }
        }

        // Try parsing positions (account.id nested)
        if out.is_empty() {
            if let Some(items) = value
                .get("data")
                .and_then(|d| d.get("positions"))
                .and_then(|v| v.as_array())
            {
                for item in items {
                    if let Some(id) = item
                        .get("account")
                        .and_then(|a| a.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        if let Ok(addr) = id.parse::<Address>() {
                            out.push(addr);
                        }
                    }
                }
            }
        }

        // Try parsing users (legacy, direct id)
        if out.is_empty() {
            if let Some(items) = value
                .get("data")
                .and_then(|d| d.get("users"))
                .and_then(|v| v.as_array())
            {
                for item in items {
                    if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        if let Ok(addr) = id.parse::<Address>() {
                            out.push(addr);
                        }
                    }
                }
            }
        }

        if !out.is_empty() {
            out.sort_unstable();
            out.dedup();
            if out.len() > 100 {
                out.truncate(100);
            }
            return Ok(out);
        }
    }

    Ok(Vec::new())
}

fn parse_prefiltered_accounts_from_positions(value: &Value) -> Vec<Address> {
    let Some(accounts) = value
        .get("data")
        .and_then(|d| d.get("accounts"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    let mut ranked: Vec<(Address, f64)> = Vec::new();

    for account in accounts {
        let Some(account_id) = account.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(addr) = account_id.parse::<Address>() else {
            continue;
        };

        let Some(positions) = account.get("positions").and_then(|v| v.as_array()) else {
            continue;
        };

        let mut total_weighted_collateral_usd = 0.0_f64;
        let mut total_debt_usd = 0.0_f64;

        for position in positions {
            let Some(balance_raw) = position.get("balance").and_then(|v| v.as_str()) else {
                continue;
            };

            let decimals = position
                .get("market")
                .and_then(|m| m.get("inputToken"))
                .and_then(|t| t.get("decimals"))
                .and_then(parse_f64_value)
                .unwrap_or(18.0)
                .clamp(0.0, 36.0) as i32;

            let price_usd = position
                .get("market")
                .and_then(|m| m.get("inputToken"))
                .and_then(|t| t.get("lastPriceUSD"))
                .and_then(parse_f64_value)
                .unwrap_or(0.0);

            if price_usd <= 0.0 {
                continue;
            }

            let liquidation_threshold_pct = position
                .get("market")
                .and_then(|m| m.get("liquidationThreshold"))
                .and_then(parse_f64_value)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0);

            let Ok(balance_base_units) = balance_raw.parse::<f64>() else {
                continue;
            };

            let amount = balance_base_units / 10_f64.powi(decimals);
            let value_usd = amount.abs() * price_usd;

            if amount > 0.0 {
                total_weighted_collateral_usd += value_usd * (liquidation_threshold_pct / 100.0);
            } else if amount < 0.0 {
                total_debt_usd += value_usd;
            }
        }

        if total_debt_usd <= 0.0 {
            continue;
        }

        let hf_estimate = total_weighted_collateral_usd / total_debt_usd;
        if hf_estimate.is_finite() && hf_estimate <= 2.0 {
            ranked.push((addr, hf_estimate));
        }
    }

    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (addr, _) in ranked {
        if seen.insert(addr) {
            out.push(addr);
        }
        if out.len() >= 200 {
            break;
        }
    }

    out
}

fn parse_f64_value(value: &Value) -> Option<f64> {
    if let Some(v) = value.as_f64() {
        return Some(v);
    }

    if let Some(s) = value.as_str() {
        return s.parse::<f64>().ok();
    }

    None
}

fn reserve_catalog_from_env(chain_id: u64) -> HashMap<String, Address> {
    let mut out = default_reserve_catalog(chain_id);

    if let Ok(raw) = std::env::var("RESERVE_CATALOG") {
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let Some((symbol_raw, addr_raw)) = entry.split_once('=') else {
                continue;
            };

            let symbol = symbol_raw.trim().to_ascii_uppercase();
            let Ok(addr) = addr_raw.trim().parse::<Address>() else {
                continue;
            };

            out.insert(symbol, addr);
        }
    }

    for (key, value) in std::env::vars() {
        if !key.starts_with("RESERVE_") || key == "RESERVE_CATALOG" {
            continue;
        }

        let symbol = key.trim_start_matches("RESERVE_").trim().to_ascii_uppercase();
        if symbol.is_empty() {
            continue;
        }

        if let Ok(addr) = value.trim().parse::<Address>() {
            out.insert(symbol, addr);
        }
    }

    out
}

fn default_reserve_catalog(chain_id: u64) -> HashMap<String, Address> {
    let mut out = HashMap::new();

    let pairs: [(&str, &str); 7] = if chain_id == 11155111 {
        [
            ("WETH", "0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c"),
            ("USDC", "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8"),
            ("WBTC", "0x29f2D40B0605204364af54EC677bD022dA425d03"),
            ("DAI", "0x68194a729C2450ad26072b3D33ADaCbcef39D574"),
            ("USDT", "0xC2C527C0CACF457746Bd31B2a698Fe89de2b6d49"),
            ("LINK", "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4"),
            ("AAVE", "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951"),
        ]
    } else {
        [
            ("WETH", "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            ("USDC", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
            ("WBTC", "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"),
            ("DAI", "0x6B175474E89094C44Da98b954EedeAC495271d0F"),
            ("USDT", "0xdAC17F958D2ee523a2206206994597C13D831ec7"),
            ("LINK", "0x514910771AF9Ca656af840dff83E8264EcF986CA"),
            ("AAVE", "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DdAE9"),
        ]
    };

    for (symbol, addr_raw) in pairs {
        if let Ok(addr) = addr_raw.parse::<Address>() {
            out.insert(symbol.to_string(), addr);
        }
    }

    out
}

fn fallback_price_usd(symbol: &str, eth_price_usd: f64) -> f64 {
    match symbol {
        "WETH" | "ETH" => eth_price_usd,
        "USDC" | "USDT" | "DAI" => 1.0,
        "WBTC" => 60000.0,
        "LINK" => 10.0,
        "AAVE" => 100.0,
        _ => 1.0,
    }
}

fn default_liquidation_threshold(symbol: &str, _chain_id: u64) -> f64 {
    match symbol {
        // Aave V3 WETH reserve threshold used in local scenario scripts.
        "WETH" | "ETH" => 0.83,
        _ => 0.85,
    }
}
