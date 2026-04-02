// ============================================================================
// PROFIT SCENARIO TEST — Anvil Fork + Scripts
// ============================================================================
//
// Mục tiêu: xác nhận module ProfitCalculator hoạt động đúng trên scenario được
// dựng bằng PowerShell scripts:
//   1) .\scripts\start_anvil.ps1
//   2) .\scripts\setup_liquidation_scenario.ps1
//   3) .\scripts\crash_price.ps1
//   4) cargo test --test profit_scenario -- --nocapture
//
// Test này:
//   - đọc on-chain AccountData từ Aave Pool (total collateral/debt + HF)
//   - đọc ETH/USD từ Chainlink feed (sau crash giá)
//   - tạo LiquidationTarget theo USD-by-asset (đơn giản, ổn định)
//   - gọi ProfitCalculator.evaluate() và assert profit dương

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use ethers::prelude::*;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, I256, U256};
use tokio::sync::RwLock;

use liquidator::{GasEstimator, LiquidationTarget, PriceData, ProfitCalculator, ProfitConfig};

const ANVIL_RPC: &str = "http://127.0.0.1:8545";

// Borrower được setup bởi scripts/setup_liquidation_scenario.ps1 (Account #2)
const BORROWER: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

mod mainnet {
    pub const AAVE_POOL: &str = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2";
    pub const CHAINLINK_ETH_USD: &str = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419";
}

mod sepolia {
    pub const AAVE_POOL: &str = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951";
    pub const CHAINLINK_ETH_USD: &str = "0x694AA1769357215DE4FAC081bf1f309aDC325306";
}

abigen!(
    AavePool,
    r#"[
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
    ]"#
);

abigen!(
    ChainlinkAggregator,
    r#"[
        function latestRoundData() external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
    ]"#
);

fn u256_to_f64(value: U256, decimals: u32) -> f64 {
    if value > U256::from(u128::MAX) {
        return f64::MAX;
    }
    value.low_u128() as f64 / 10_f64.powi(decimals as i32)
}

async fn connect() -> Result<Arc<Provider<Http>>> {
    let provider = Provider::<Http>::try_from(ANVIL_RPC)?;
    // force a quick call so we can skip early if Anvil isn't responding
    provider.get_block_number().await?;
    Ok(Arc::new(provider))
}

fn addresses_for_chain(chain_id: u64) -> (&'static str, &'static str) {
    match chain_id {
        11155111 => (sepolia::AAVE_POOL, sepolia::CHAINLINK_ETH_USD),
        _ => (mainnet::AAVE_POOL, mainnet::CHAINLINK_ETH_USD),
    }
}

#[tokio::test]
async fn test_profit_calculator_on_scripted_scenario() {
    let provider = match connect().await {
        Ok(p) => p,
        Err(e) => {
            println!("SKIP: Anvil not running at {} ({})", ANVIL_RPC, e);
            return;
        }
    };

    let chain_id = provider.get_chainid().await.unwrap_or_else(|_| U256::from(1));
    let chain_id = chain_id.as_u64();
    let (aave_pool_addr, eth_usd_feed_addr) = addresses_for_chain(chain_id);

    let borrower: Address = BORROWER.parse().expect("invalid borrower address");
    let pool: Address = aave_pool_addr.parse().expect("invalid pool address");
    let feed: Address = eth_usd_feed_addr.parse().expect("invalid feed address");

    let aave = AavePool::new(pool, Arc::clone(&provider));
    let data = match aave.get_user_account_data(borrower).call().await {
        Ok(d) => d,
        Err(e) => {
            println!("SKIP: Failed to read Aave account data ({}).", e);
            println!("      Ensure scripts have been run: setup + crash.");
            return;
        }
    };

    let total_collateral_usd = u256_to_f64(data.0, 8);
    let total_debt_usd = u256_to_f64(data.1, 8);
    let health_factor = if data.1.is_zero() {
        f64::INFINITY
    } else {
        u256_to_f64(data.5, 18)
    };

    if total_debt_usd == 0.0 {
        println!("SKIP: Borrower has no debt. Run: .\\scripts\\setup_liquidation_scenario.ps1");
        return;
    }

    if health_factor >= 1.0 {
        println!(
            "SKIP: Position not liquidatable (HF={:.4}). Run: .\\scripts\\crash_price.ps1",
            health_factor
        );
        return;
    }

    let chainlink = ChainlinkAggregator::new(feed, Arc::clone(&provider));
    let (round_id, answer, _started_at, updated_at, _answered_in_round) =
        match chainlink.latest_round_data().call().await {
            Ok(v) => v,
            Err(_) => (0u128, I256::zero(), U256::zero(), U256::zero(), 0u128),
        };

    // Chainlink ETH/USD uses 8 decimals
    // Note: answer should be positive for ETH/USD.
    let eth_price_usd = (answer.as_u128() as f64) / 1e8;

    // Build a minimal price cache: only ETH is needed for gas-cost USD conversion.
    let mut price_map: HashMap<String, PriceData> = HashMap::new();
    price_map.insert(
        "ETH".to_string(),
        PriceData {
            asset_id: "ETH".to_string(),
            price_usd: if eth_price_usd > 0.0 { eth_price_usd } else { 2000.0 },
            price_raw: answer.as_u128() as i128,
            decimals: 8,
            round_id: round_id as u128,
            updated_at: updated_at.as_u64(),
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: feed,
        },
    );
    let price_cache = Arc::new(RwLock::new(price_map));

    // ProfitConfig.local_fork(): verbose=true, min_profit thấp để dễ quan sát.
    let mut profit_cfg = ProfitConfig::local_fork();
    profit_cfg.include_flash_loan_fee = false;

    let gas_est = GasEstimator::new(Arc::clone(&provider));
    let calc = ProfitCalculator::new(profit_cfg, gas_est, price_cache);

    // Target: dùng USD-by-asset để tránh phụ thuộc decimals/token amounts.
    let mut collateral = HashMap::new();
    collateral.insert("WETH".to_string(), total_collateral_usd);
    let mut debt = HashMap::new();
    debt.insert("USDC".to_string(), total_debt_usd);

    let target = LiquidationTarget {
        user_address: BORROWER.to_string(),
        health_factor,
        total_collateral_usd,
        total_debt_usd,
        ltv: (data.4.as_u128() as f64) / 100.0,
        liquidation_threshold: (data.3.as_u128() as f64) / 100.0,
        collateral,
        debt,
        estimated_profit: 0.0,
        risk_score: 10,
        last_updated: chrono::Utc::now().timestamp(),
    };

    let estimate = calc
        .evaluate(&target)
        .await
        .expect("Profit evaluation failed");

    println!("\n=== PROFIT ESTIMATE ===\n{}", estimate.summary());
    println!("{}", estimate.breakdown.display());

    assert!(estimate.gross_profit_usd > 0.0, "gross profit should be > 0");
    assert!(estimate.net_profit_usd.is_finite(), "net profit should be finite");
    assert!(
        estimate.net_profit_usd > 0.0,
        "net profit should be positive for scripted scenario"
    );
    assert!(estimate.is_profitable, "estimate should be marked profitable");
}
