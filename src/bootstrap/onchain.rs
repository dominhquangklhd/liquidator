use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use ethers::contract::abigen;
use ethers::providers::{Http, Provider};
use ethers::types::{Address, H160, U256};

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
    let bootstrap_users = parse_bootstrap_users();
    if bootstrap_users.is_empty() {
        tracing::info!("No BOOTSTRAP_USERS configured, skip on-chain bootstrap");
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
                liquidation_threshold: 0.85,
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
                liquidation_threshold: 0.85,
                price_in_eth: 1.0,
            },
        );
        assets_loaded += 1;
    }

    let pool = BootstrapAavePool::new(aave_pool_address, Arc::clone(&rpc));
    let mut users_loaded = 0usize;

    for user_addr in bootstrap_users {
        let onchain = pool.get_user_account_data(user_addr).call().await;
        let Ok((total_collateral_base, total_debt_base, _available, _lt, _ltv, hf_raw)) = onchain else {
            tracing::warn!("Skip bootstrap user {:?}: cannot read account data", user_addr);
            continue;
        };

        let collateral_usd = u256_to_f64(total_collateral_base) / 1e8;
        let debt_usd = u256_to_f64(total_debt_base) / 1e8;
        let hf = u256_to_f64(hf_raw) / 1e18;

        let user_id = format!("{:?}", user_addr);
        let mut user = User::new(user_id.clone());

        if collateral_usd > 0.0 && eth_price_usd > 0.0 {
            user.collateral
                .insert("WETH".to_string(), collateral_usd / eth_price_usd);
            engine
                .registry
                .add_user_to_asset("WETH".to_string(), user_id.clone());
            // Also track ETH so oracle ETH updates can trigger recalculation.
            engine
                .registry
                .add_user_to_asset("ETH".to_string(), user_id.clone());
        }

        if debt_usd > 0.0 {
            user.debt.insert("USDC".to_string(), debt_usd);
            engine
                .registry
                .add_user_to_asset("USDC".to_string(), user_id.clone());
        }

        user.health_factor = hf;
        user.risk_bucket = RiskBucket::from_hf(hf);
        engine.users.insert(user_id.clone(), user);

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
            user_address: user_id,
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

        if let Err(e) = storage.update_user_hf(target).await {
            tracing::warn!("Failed to write bootstrap target to storage: {:?}", e);
        }

        users_loaded += 1;
    }

    tracing::info!(
        "On-chain bootstrap complete: {} assets, {} users loaded",
        assets_loaded,
        users_loaded
    );

    Ok(())
}

fn parse_bootstrap_users() -> Vec<Address> {
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
