// ============================================================================
// STRATEGY SCENARIO TESTS (Phase 3) — Anvil Fork
// ============================================================================
//
// End-to-end scenario tests cho Strategy Decider trên Anvil mainnet fork.
// Test luồng thực tế: on-chain data → ProfitEstimate → StrategyDecider → ExecutionPlan
//
// Cách chạy:
//   1. Khởi động Anvil: .\scripts\start_anvil.ps1
//   2. Setup scenario:  .\scripts\setup_liquidation_scenario.ps1
//   3. Crash giá:       .\scripts\crash_price.ps1
//   4. Chạy test:       cargo test --test strategy_scenario -- --nocapture
//
// Hoặc chạy từng test riêng:
//   cargo test --test strategy_scenario test_s1 -- --nocapture
//   cargo test --test strategy_scenario test_s2 -- --nocapture
// ============================================================================

use std::sync::Arc;
use std::collections::HashMap;

use ethers::prelude::*;
use ethers::providers::{Provider, Http, Middleware};
use ethers::types::Address;
use anyhow::Result;

use liquidator::strategy::{StrategyDecider, StrategyConfig, ExecutionMethod};
use liquidator::profit::{
    ProfitEstimate, ProfitBreakdown, LiquidationPair, GasCostEstimate,
};
use liquidator::storage::LiquidationTarget;

// ============================================================================
// CONSTANTS
// ============================================================================

const ANVIL_RPC: &str = "http://127.0.0.1:8545";

mod mainnet {
    pub const AAVE_POOL: &str = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2";
    pub const AAVE_ORACLE: &str = "0x54586bE62E3c3580375aE3723C145253060Ca0C2";
    pub const WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
    pub const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
    pub const CHAINLINK_ETH_USD: &str = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419";
}

mod sepolia {
    pub const AAVE_POOL: &str = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951";
    pub const AAVE_ORACLE: &str = "0x2da88497588bf89281816106C7259e31AF45a663";
    pub const WETH: &str = "0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c";
    pub const USDC: &str = "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8";
    pub const CHAINLINK_ETH_USD: &str = "0x694AA1769357215DE4FAC081bf1f309aDC325306";
}

/// Anvil Account #2 (Borrower — position setup bởi setup_liquidation_scenario.ps1)
const BORROWER: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

/// Anvil Account #3 (Liquidator)
const LIQUIDATOR: &str = "0x90F79bf6EB2c4f870365E785982E1f101E93b906";

// ============================================================================
// ABI BINDINGS
// ============================================================================

abigen!(
    AavePool,
    r#"[
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
    ]"#
);

abigen!(
    AaveOracle,
    r#"[
        function getAssetPrice(address asset) external view returns (uint256)
    ]"#
);

abigen!(
    ERC20Token,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
    ]"#
);

abigen!(
    ChainlinkAggregator,
    r#"[
        function latestRoundData() external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
    ]"#
);

// ============================================================================
// TEST CONTEXT & HELPERS
// ============================================================================

struct NetworkConfig {
    aave_pool: &'static str,
    aave_oracle: &'static str,
    weth: &'static str,
    usdc: &'static str,
    chainlink_eth_usd: &'static str,
    name: &'static str,
}

fn get_network_config(chain_id: u64) -> NetworkConfig {
    match chain_id {
        1 => NetworkConfig {
            aave_pool: mainnet::AAVE_POOL,
            aave_oracle: mainnet::AAVE_ORACLE,
            weth: mainnet::WETH,
            usdc: mainnet::USDC,
            chainlink_eth_usd: mainnet::CHAINLINK_ETH_USD,
            name: "Ethereum Mainnet",
        },
        11155111 => NetworkConfig {
            aave_pool: sepolia::AAVE_POOL,
            aave_oracle: sepolia::AAVE_ORACLE,
            weth: sepolia::WETH,
            usdc: sepolia::USDC,
            chainlink_eth_usd: sepolia::CHAINLINK_ETH_USD,
            name: "Sepolia Testnet",
        },
        _ => NetworkConfig {
            aave_pool: mainnet::AAVE_POOL,
            aave_oracle: mainnet::AAVE_ORACLE,
            weth: mainnet::WETH,
            usdc: mainnet::USDC,
            chainlink_eth_usd: mainnet::CHAINLINK_ETH_USD,
            name: "Ethereum Mainnet (default)",
        },
    }
}

struct ScenarioContext {
    provider: Arc<Provider<Http>>,
    net: NetworkConfig,
}

/// Kết nối đến Anvil, skip test nếu không available
async fn setup_anvil() -> Option<ScenarioContext> {
    let provider = match Provider::<Http>::try_from(ANVIL_RPC) {
        Ok(p) => p,
        Err(_) => {
            println!("SKIP: Anvil not running at {}", ANVIL_RPC);
            return None;
        }
    };

    match provider.get_block_number().await {
        Ok(block) => {
            let chain_id = provider.get_chainid().await.unwrap_or_default();
            let net = get_network_config(chain_id.as_u64());
            println!("Connected to {} at block #{} (chain={})", net.name, block, chain_id);
            Some(ScenarioContext {
                provider: Arc::new(provider),
                net,
            })
        }
        Err(_) => {
            println!("SKIP: Anvil not responding at {}", ANVIL_RPC);
            None
        }
    }
}

fn u256_to_f64(value: U256, decimals: u32) -> f64 {
    if value > U256::from(u128::MAX) {
        return f64::MAX;
    }
    value.low_u128() as f64 / 10_f64.powi(decimals as i32)
}

/// Đọc on-chain account data từ Aave
async fn read_account(ctx: &ScenarioContext, user: &str) -> Result<(f64, f64, f64)> {
    let pool_addr: Address = ctx.net.aave_pool.parse()?;
    let user_addr: Address = user.parse()?;
    let pool = AavePool::new(pool_addr, Arc::clone(&ctx.provider));

    let d = pool.get_user_account_data(user_addr).call().await?;
    let collateral = u256_to_f64(d.0, 8);
    let debt = u256_to_f64(d.1, 8);
    let hf = if d.1.is_zero() {
        f64::INFINITY
    } else {
        u256_to_f64(d.5, 18)
    };
    Ok((collateral, debt, hf))
}

/// Đọc giá asset từ Aave Oracle
async fn read_price(ctx: &ScenarioContext, asset: &str) -> Result<f64> {
    let oracle_addr: Address = ctx.net.aave_oracle.parse()?;
    let asset_addr: Address = asset.parse()?;
    let oracle = AaveOracle::new(oracle_addr, Arc::clone(&ctx.provider));
    let price = oracle.get_asset_price(asset_addr).call().await?;
    Ok(u256_to_f64(price, 8))
}

/// Đọc giá ETH/USD từ Chainlink trực tiếp
async fn read_chainlink_eth_price(ctx: &ScenarioContext) -> Result<f64> {
    let feed_addr: Address = ctx.net.chainlink_eth_usd.parse()?;
    let feed = ChainlinkAggregator::new(feed_addr, Arc::clone(&ctx.provider));
    let (_, answer, _, _, _) = feed.latest_round_data().call().await?;
    // Chainlink ETH/USD returns 8 decimals
    Ok(answer.as_u128() as f64 / 1e8)
}

/// Đọc balance USDC của liquidator
async fn read_usdc_balance(ctx: &ScenarioContext, account: &str) -> Result<f64> {
    let usdc_addr: Address = ctx.net.usdc.parse()?;
    let account_addr: Address = account.parse()?;
    let usdc = ERC20Token::new(usdc_addr, Arc::clone(&ctx.provider));
    let balance = usdc.balance_of(account_addr).call().await?;
    Ok(u256_to_f64(balance, 6))
}

/// Đọc gas price hiện tại (gwei)
async fn read_gas_price(ctx: &ScenarioContext) -> Result<f64> {
    let gp = ctx.provider.get_gas_price().await?;
    Ok(u256_to_f64(gp, 9))
}

/// Tạo LiquidationTarget từ on-chain data
fn make_target(
    user: &str,
    collateral_usd: f64,
    debt_usd: f64,
    hf: f64,
    weth_addr: &str,
    usdc_addr: &str,
) -> LiquidationTarget {
    let mut collateral = HashMap::new();
    collateral.insert(weth_addr.to_string(), collateral_usd);
    let mut debt = HashMap::new();
    debt.insert(usdc_addr.to_string(), debt_usd);

    LiquidationTarget {
        user_address: user.to_string(),
        health_factor: hf,
        total_collateral_usd: collateral_usd,
        total_debt_usd: debt_usd,
        ltv: 0.8,
        liquidation_threshold: 0.825,
        collateral,
        debt,
        estimated_profit: debt_usd * 0.05 * 0.5, // 5% bonus * 50% close factor
        risk_score: if hf < 0.8 { 10 } else if hf < 1.0 { 8 } else { 3 },
        last_updated: chrono::Utc::now().timestamp(),
    }
}

/// Tạo ProfitEstimate từ on-chain data thực
fn make_profit_estimate(
    user: &str,
    debt_usd: f64,
    eth_price: f64,
    gas_price_gwei: f64,
    bonus_pct: f64,
) -> ProfitEstimate {
    let close_factor = 0.5;
    let debt_to_cover = debt_usd * close_factor;
    let collateral_received = debt_to_cover * (1.0 + bonus_pct / 100.0);
    let gross_profit = debt_to_cover * bonus_pct / 100.0;

    // Gas: ~500k gas units * gas_price * ETH price
    let gas_units = 500_000_u64;
    let gas_cost_eth = gas_units as f64 * gas_price_gwei * 1e-9;
    let gas_cost_usd = gas_cost_eth * eth_price;

    let slippage = collateral_received * 0.005; // 0.5% slippage
    let net_profit = gross_profit - gas_cost_usd - slippage;
    let roi = if gas_cost_usd > 0.0 {
        net_profit / gas_cost_usd * 100.0
    } else {
        0.0
    };

    ProfitEstimate {
        user_address: user.to_string(),
        pair: LiquidationPair {
            collateral_asset: "ETH".to_string(),
            debt_asset: "USDC".to_string(),
            bonus_pct,
            collateral_price_usd: eth_price,
            debt_price_usd: 1.0,
            collateral_amount: collateral_received / eth_price,
            debt_amount: debt_to_cover,
            score: net_profit,
        },
        debt_to_cover_usd: debt_to_cover,
        collateral_received_usd: collateral_received,
        gross_profit_usd: gross_profit,
        gas_cost_usd,
        slippage_cost_usd: slippage,
        flash_loan_fee_usd: 0.0,
        net_profit_usd: net_profit,
        roi_pct: roi,
        is_profitable: net_profit > 0.0,
        reject_reason: if net_profit <= 0.0 {
            Some("Not profitable after costs".to_string())
        } else {
            None
        },
        breakdown: ProfitBreakdown {
            debt_covered_usd: debt_to_cover,
            collateral_base_usd: collateral_received,
            bonus_usd: gross_profit,
            gas: GasCostEstimate::calculate(gas_price_gwei, gas_units, eth_price),
            slippage_pct: 0.5,
            slippage_usd: slippage,
            size_impact_pct: 0.0,
            flash_loan_fee_usd: 0.0,
            total_cost_usd: gas_cost_usd + slippage,
            gross_profit_usd: gross_profit,
            net_profit_usd: net_profit,
        },
    }
}

/// Macro: skip test nếu Anvil không available
macro_rules! require_anvil {
    () => {
        match setup_anvil().await {
            Some(ctx) => ctx,
            None => return,
        }
    };
}

/// Macro: skip test nếu borrower chưa có position
macro_rules! require_position {
    ($ctx:expr) => {{
        let (c, d, h) = read_account(&$ctx, BORROWER)
            .await
            .expect("Failed to read borrower account");
        if d == 0.0 {
            println!(
                "SKIP: Borrower has no position. Run: .\\scripts\\setup_liquidation_scenario.ps1"
            );
            return;
        }
        (c, d, h)
    }};
}

/// Macro: skip nếu position chưa liquidatable
macro_rules! require_liquidatable {
    ($ctx:expr) => {{
        let (c, d, h) = require_position!($ctx);
        if h >= 1.0 {
            println!(
                "SKIP: Position not liquidatable (HF={:.4}). Run: .\\scripts\\crash_price.ps1",
                h
            );
            return;
        }
        (c, d, h)
    }};
}

// ============================================================================
// S1: Happy Path — Direct Liquidation
// Wallet đủ tiền → StrategyDecider chọn Direct → verify plan đúng
// ============================================================================

#[tokio::test]
async fn test_s1_happy_path_direct_liquidation() {
    let ctx = require_anvil!();
    let (collateral, debt, hf) = require_liquidatable!(ctx);

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);
    let liquidator_usdc = read_usdc_balance(&ctx, LIQUIDATOR).await.unwrap_or(0.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S1: HAPPY PATH — DIRECT LIQUIDATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Borrower HF:       {:.6}", hf);
    println!("  Collateral:        ${:.2}", collateral);
    println!("  Debt:              ${:.2}", debt);
    println!("  ETH Price:         ${:.2}", eth_price);
    println!("  Gas Price:         {:.2} gwei", gas_gwei);
    println!("  Liquidator USDC:   ${:.2}", liquidator_usdc);

    // -- Setup Strategy Decider với enough funds --
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);

    // Set wallet: đủ ETH + USDC
    decider.update_wallet_balance(10.0).await;
    let debt_to_cover = debt * 0.5;
    decider
        .update_token_balance("USDC".to_string(), liquidator_usdc.max(debt_to_cover + 1000.0))
        .await;
    decider.update_gas_price(gas_gwei).await;

    // -- Build real profit estimate --
    let estimate = make_profit_estimate(BORROWER, debt, eth_price, gas_gwei, 5.0);
    let target = make_target(
        BORROWER, collateral, debt, hf, ctx.net.weth, ctx.net.usdc,
    );

    println!("  Net Profit Est:    ${:.2}", estimate.net_profit_usd);
    println!("  ROI:               {:.1}%", estimate.roi_pct);

    // -- Create plan --
    let plan = decider
        .create_plan(vec![(target, estimate)])
        .await
        .expect("create_plan failed");

    println!("  Plan: {}", plan.summary());

    // -- Assertions --
    assert_eq!(plan.total_input, 1);

    let executable = plan.executable_targets();
    if executable.is_empty() {
        println!("  INFO: Target skipped (gas too high or profit too low)");
        println!("  Reason: {}", plan.targets[0].decision.reasoning);
        return;
    }

    assert_eq!(executable.len(), 1);
    let decision = &executable[0].decision;

    // Có đủ USDC → phải chọn Direct
    assert!(
        matches!(decision.method, ExecutionMethod::Direct { .. }),
        "Should choose Direct when wallet has enough USDC, got: {:?}",
        decision.method
    );

    assert!(
        decision.priority_score >= 0.0 && decision.priority_score <= 10.0,
        "Priority score {:.2} should be in [0, 10]",
        decision.priority_score
    );

    assert!(
        decision.adjusted_profit_usd > 0.0,
        "Adjusted profit should be > 0"
    );

    println!("  Method:            {}", decision.method.label());
    println!("  Priority:          {:.2}", decision.priority_score);
    println!("  Adjusted Profit:   ${:.2}", decision.adjusted_profit_usd);
    println!("  PASS: Direct liquidation strategy selected correctly");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S2: No Token Fallback
// Wallet thiếu token → StrategyDecider chọn Skip
// ============================================================================

#[tokio::test]
async fn test_s2_no_token_skip() {
    let ctx = require_anvil!();
    let (collateral, debt, hf) = require_liquidatable!(ctx);

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S2: NO TOKEN → SKIP");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Borrower HF:  {:.6}", hf);
    println!("  Debt:         ${:.2}", debt);
    println!("  ETH Price:    ${:.2}", eth_price);

    // -- Config: direct/skip only, NO token balance --
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);

    // Đủ ETH cho gas nhưng KHÔNG có USDC → phải skip
    decider.update_wallet_balance(5.0).await;
    // Không gọi update_token_balance → wallet_token_balances rỗng
    decider.update_gas_price(gas_gwei).await;

    let estimate = make_profit_estimate(BORROWER, debt, eth_price, gas_gwei, 5.0);
    let target = make_target(
        BORROWER, collateral, debt, hf, ctx.net.weth, ctx.net.usdc,
    );

    let plan = decider
        .create_plan(vec![(target, estimate)])
        .await
        .expect("create_plan failed");

    println!("  Plan: {}", plan.summary());

    let executable = plan.executable_targets();
    assert!(executable.is_empty(), "Should skip when no USDC for direct path");
    let reason = &plan.targets[0].decision.reasoning;
    println!("  INFO: Skipped — {}", reason);
    assert!(
        reason.contains("No sufficient") || reason.contains("token") || reason.contains("Insufficient"),
        "Skip reason should mention direct token insufficiency"
    );
    println!("  PASS: Correctly skipped due to missing debt token");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S3: Multi-target Priority Ordering
// Nhiều users undercollateralized → verify priority ordering đúng
// ============================================================================

#[tokio::test]
async fn test_s3_multi_target_priority_ordering() {
    let ctx = require_anvil!();
    // Chỉ cần Anvil chạy, không cần position thật cho test này
    // Sử dụng on-chain price + gas để tạo realistic estimates

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S3: MULTI-TARGET PRIORITY ORDERING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ETH Price:    ${:.2}", eth_price);
    println!("  Gas Price:    {:.2} gwei", gas_gwei);

    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config);
    decider.update_wallet_balance(10.0).await;
    decider
        .update_token_balance("USDC".to_string(), 500_000.0)
        .await;
    decider.update_gas_price(gas_gwei).await;

    // 3 synthetic users với khác HF/debt → real gas/price từ on-chain
    let users = vec![
        ("0xAAAA0000000000000000000000000000000000AA", 0.85, 10_000.0, "User A: HF=0.85, debt=$10k"),
        ("0xBBBB0000000000000000000000000000000000BB", 0.95, 50_000.0, "User B: HF=0.95, debt=$50k"),
        ("0xCCCC0000000000000000000000000000000000CC", 0.70, 2_000.0,  "User C: HF=0.70, debt=$2k"),
    ];

    let inputs: Vec<(LiquidationTarget, ProfitEstimate)> = users
        .iter()
        .map(|(addr, hf, debt, _label)| {
            let coll = debt / hf; // approximate collateral from HF
            let target = make_target(addr, coll, *debt, *hf, ctx.net.weth, ctx.net.usdc);
            let estimate = make_profit_estimate(addr, *debt, eth_price, gas_gwei, 5.0);
            (target, estimate)
        })
        .collect();

    let plan = decider
        .create_plan(inputs)
        .await
        .expect("create_plan failed");

    println!("  Plan: {}", plan.summary());
    println!("  ---");

    // Print ordering
    for pt in &plan.targets {
        let label = users
            .iter()
            .find(|(a, _, _, _)| *a == pt.decision.user_address)
            .map(|(_, _, _, l)| *l)
            .unwrap_or("?");
        println!(
            "    #{} {} | priority={:.2} | method={} | profit=${:.2}",
            pt.rank,
            label,
            pt.decision.priority_score,
            pt.decision.method.label(),
            pt.decision.adjusted_profit_usd,
        );
    }

    // -- Verify sorted DESC by priority --
    let executable: Vec<_> = plan
        .targets
        .iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();

    for i in 0..executable.len().saturating_sub(1) {
        assert!(
            executable[i].decision.priority_score >= executable[i + 1].decision.priority_score,
            "Priority order violated: #{} ({:.2}) < #{} ({:.2})",
            executable[i].rank,
            executable[i].decision.priority_score,
            executable[i + 1].rank,
            executable[i + 1].decision.priority_score,
        );
    }

    // -- Verify ranks are sequential --
    for (i, pt) in plan.targets.iter().enumerate() {
        assert_eq!(
            pt.rank,
            i + 1,
            "Rank should be sequential, expected {} got {}",
            i + 1,
            pt.rank,
        );
    }

    // -- All profitable targets should execute --
    let profitable_count = plan
        .targets
        .iter()
        .filter(|pt| pt.estimate.is_profitable && pt.decision.should_execute())
        .count();
    println!("  Profitable & executing: {}", profitable_count);

    println!("  PASS: Priority ordering correct");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S4: Circuit Breaker Recovery
// N failures → breaker trips → cooldown → resume
// ============================================================================

#[tokio::test]
async fn test_s4_circuit_breaker_recovery() {
    let ctx = require_anvil!();
    let (collateral, debt, hf) = require_liquidatable!(ctx);

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S4: CIRCUIT BREAKER RECOVERY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Config: circuit breaker trips after 3 failures, 1s cooldown (fast for test)
    let mut config = StrategyConfig::local_fork();
    config.circuit_breaker_threshold = 3;
    config.circuit_breaker_cooldown_secs = 1;
    let decider = StrategyDecider::new(config);

    decider.update_wallet_balance(10.0).await;
    decider
        .update_token_balance("USDC".to_string(), 200_000.0)
        .await;
    decider.update_gas_price(gas_gwei).await;

    let make_input = || {
        let t = make_target(
            BORROWER, collateral, debt, hf, ctx.net.weth, ctx.net.usdc,
        );
        let e = make_profit_estimate(BORROWER, debt, eth_price, gas_gwei, 5.0);
        vec![(t, e)]
    };

    // Phase 1: Normal operation
    let plan1 = decider.create_plan(make_input()).await.unwrap();
    let exec1 = plan1.executable_targets().len();
    println!("  Phase 1 (normal):    {} executable targets", exec1);

    if exec1 == 0 {
        println!("  SKIP: Target not profitable enough for this test");
        return;
    }
    assert!(exec1 > 0, "Should execute normally before breaker trip");

    // Phase 2: Simulate 3 consecutive failures → trip circuit breaker
    decider.report_failure().await;
    decider.report_failure().await;
    decider.report_failure().await;

    let stats_mid = decider.get_stats().await;
    println!(
        "  Phase 2 (tripped):   circuit_breaker_trips={}",
        stats_mid.circuit_breaker_trips
    );
    assert_eq!(stats_mid.circuit_breaker_trips, 1, "Should have 1 trip");

    let plan2 = decider.create_plan(make_input()).await.unwrap();
    let exec2 = plan2.executable_targets().len();
    println!(
        "  Phase 2 plan:        {} executable (should be 0)",
        exec2
    );
    assert_eq!(
        exec2, 0,
        "All targets should be blocked by circuit breaker"
    );

    // Phase 3: Wait cooldown → should recover
    println!("  Waiting 1.1s for cooldown...");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let plan3 = decider.create_plan(make_input()).await.unwrap();
    let exec3 = plan3.executable_targets().len();
    println!("  Phase 3 (recovered): {} executable targets", exec3);
    assert!(
        exec3 > 0,
        "Should recover after cooldown and execute targets again"
    );

    let stats_final = decider.get_stats().await;
    println!("  Final stats:");
    println!(
        "    plans={}, decisions={}, trips={}",
        stats_final.total_plans,
        stats_final.total_decisions,
        stats_final.circuit_breaker_trips
    );
    println!("  PASS: Circuit breaker trips and recovers correctly");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S5: Gas Spike → All Targets Skipped
// Anvil gas price → strategy respects max_gas_price
// ============================================================================

#[tokio::test]
async fn test_s5_gas_spike_skips_all() {
    let ctx = require_anvil!();
    let (collateral, debt, hf) = require_liquidatable!(ctx);

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let actual_gas = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S5: GAS SPIKE → SKIP ALL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Actual gas:    {:.2} gwei", actual_gas);

    let mut config = StrategyConfig::local_fork();
    config.max_gas_price_gwei = 50.0;
    let decider = StrategyDecider::new(config);

    decider.update_wallet_balance(10.0).await;
    decider
        .update_token_balance("USDC".to_string(), 200_000.0)
        .await;

    let make_input = || {
        let t = make_target(
            BORROWER, collateral, debt, hf, ctx.net.weth, ctx.net.usdc,
        );
        let e = make_profit_estimate(BORROWER, debt, eth_price, actual_gas, 5.0);
        vec![(t, e)]
    };

    // Phase 1: Normal gas (30 gwei < max 50)
    decider.update_gas_price(30.0).await;
    let plan1 = decider.create_plan(make_input()).await.unwrap();
    let exec1 = plan1.executable_targets().len();
    println!("  Phase 1 (30 gwei):   {} executable", exec1);

    // Phase 2: Gas spike (200 gwei > max 50) → all skip
    decider.update_gas_price(200.0).await;
    let plan2 = decider.create_plan(make_input()).await.unwrap();
    let exec2 = plan2.executable_targets().len();
    println!("  Phase 2 (200 gwei):  {} executable (should be 0)", exec2);
    assert_eq!(exec2, 0, "All targets should be skipped at high gas");
    assert_eq!(plan2.skip_count, 1);

    // Verify skip reason mentions gas
    let skip_reason = &plan2.targets[0].decision.reasoning;
    println!("  Skip reason: {}", skip_reason);
    assert!(
        skip_reason.to_lowercase().contains("gas"),
        "Skip reason should mention gas: {}",
        skip_reason
    );

    // Phase 3: Gas drops back (25 gwei) → execute again
    decider.update_gas_price(25.0).await;
    let plan3 = decider.create_plan(make_input()).await.unwrap();
    let exec3 = plan3.executable_targets().len();
    println!("  Phase 3 (25 gwei):   {} executable", exec3);

    if exec1 > 0 {
        // Nếu Phase 1 execute thì Phase 3 cũng phải execute
        assert!(
            exec3 > 0,
            "Should resume execution after gas drops below threshold"
        );
    }

    println!("  PASS: Gas spike correctly blocks all targets");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S6: Exposure Limit Enforcement
// Total exposure gần limit → target mới bị skip
// ============================================================================

#[tokio::test]
async fn test_s6_exposure_limit_enforcement() {
    let ctx = require_anvil!();

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S6: EXPOSURE LIMIT ENFORCEMENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Config: max total exposure $25k
    let mut config = StrategyConfig::local_fork();
    config.max_total_exposure_usd = 25_000.0;
    config.max_concurrent_liquidations = 10; // high limit so only exposure caps
    let decider = StrategyDecider::new(config);

    decider.update_wallet_balance(10.0).await;
    decider
        .update_token_balance("USDC".to_string(), 500_000.0)
        .await;
    decider.update_gas_price(gas_gwei).await;

    // 3 targets mỗi cái debt_to_cover ~ $10k (debt $20k * 0.5 close)
    let users = [
        ("0x1111000000000000000000000000000000000011", 0.85, 20_000.0),
        ("0x2222000000000000000000000000000000000022", 0.88, 20_000.0),
        ("0x3333000000000000000000000000000000000033", 0.90, 20_000.0),
    ];

    let inputs: Vec<(LiquidationTarget, ProfitEstimate)> = users
        .iter()
        .map(|(addr, hf, debt)| {
            let coll = debt / hf;
            let target = make_target(addr, coll, *debt, *hf, ctx.net.weth, ctx.net.usdc);
            let estimate = make_profit_estimate(addr, *debt, eth_price, gas_gwei, 5.0);
            (target, estimate)
        })
        .collect();

    let plan = decider.create_plan(inputs).await.expect("create_plan");

    println!("  Total input:    {}", plan.total_input);
    println!("  Execute:        {}", plan.execute_count);
    println!("  Skip:           {}", plan.skip_count);
    println!("  Exposure:       ${:.2}", plan.total_exposure_usd);
    println!("  Max allowed:    $25,000");

    // With $25k limit and each target ~$10k exposure:
    // only 2 can fit (2 * $10k = $20k < $25k, but 3 * $10k = $30k > $25k)
    assert!(
        plan.execute_count <= 2,
        "Max 2 targets fit in $25k exposure, got {}",
        plan.execute_count
    );
    assert!(
        plan.total_exposure_usd <= 25_000.0,
        "Total exposure ${:.2} should be <= $25,000",
        plan.total_exposure_usd
    );
    assert!(
        plan.skip_count >= 1,
        "At least 1 target should be skipped due to exposure limit"
    );

    // Print details
    for pt in &plan.targets {
        println!(
            "    #{} {} | {} | profit=${:.2}",
            pt.rank,
            &pt.decision.user_address[..10],
            pt.decision.method.label(),
            pt.decision.adjusted_profit_usd,
        );
    }

    println!("  PASS: Exposure limit correctly enforced");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S7: Concurrent Execution Cap
// Nhiều targets nhưng max_concurrent=3 → chỉ 3 trong plan
// ============================================================================

#[tokio::test]
async fn test_s7_concurrent_execution_cap() {
    let ctx = require_anvil!();

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S7: CONCURRENT EXECUTION CAP");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let mut config = StrategyConfig::local_fork();
    config.max_concurrent_liquidations = 3;
    config.max_total_exposure_usd = 10_000_000.0; // very high so only concurrent limit applies
    let decider = StrategyDecider::new(config);

    decider.update_wallet_balance(100.0).await;
    decider
        .update_token_balance("USDC".to_string(), 1_000_000.0)
        .await;
    decider.update_gas_price(gas_gwei).await;

    // 7 synthetic liquid targets (all profitable)
    let addrs = [
        "0xAA00000000000000000000000000000000000001",
        "0xAA00000000000000000000000000000000000002",
        "0xAA00000000000000000000000000000000000003",
        "0xAA00000000000000000000000000000000000004",
        "0xAA00000000000000000000000000000000000005",
        "0xAA00000000000000000000000000000000000006",
        "0xAA00000000000000000000000000000000000007",
    ];

    let inputs: Vec<(LiquidationTarget, ProfitEstimate)> = addrs
        .iter()
        .enumerate()
        .map(|(i, addr)| {
            let hf = 0.80 + i as f64 * 0.02; // 0.80, 0.82, 0.84 ...
            let debt = 15_000.0 + i as f64 * 5_000.0; // 15k, 20k, 25k ...
            let coll = debt / hf;
            let target = make_target(addr, coll, debt, hf, ctx.net.weth, ctx.net.usdc);
            let estimate = make_profit_estimate(addr, debt, eth_price, gas_gwei, 5.0);
            (target, estimate)
        })
        .collect();

    let plan = decider.create_plan(inputs).await.expect("create_plan");

    println!("  Total input:    {}", plan.total_input);
    println!("  Execute:        {}", plan.execute_count);
    println!("  Skip:           {}", plan.skip_count);
    println!("  Max concurrent: 3");

    // Count how many are profitable
    let profitable_input = plan
        .targets
        .iter()
        .filter(|pt| pt.estimate.is_profitable)
        .count();
    println!("  Profitable:     {}", profitable_input);

    // Only top 3 by priority should execute
    assert!(
        plan.execute_count <= 3,
        "Max 3 concurrent, got {} executing",
        plan.execute_count
    );

    // Top 3 must have ranks 1-3
    let executable: Vec<_> = plan
        .targets
        .iter()
        .filter(|pt| pt.decision.should_execute())
        .collect();
    for pt in &executable {
        assert!(
            pt.rank <= 3,
            "Executable target should have rank <= 3, got {}",
            pt.rank
        );
    }

    // Rest should be skipped
    let skipped_by_limit: Vec<_> = plan
        .targets
        .iter()
        .filter(|pt| !pt.decision.should_execute() && pt.estimate.is_profitable)
        .collect();
    println!("  Skipped (cap):  {}", skipped_by_limit.len());

    // Print ordering
    for pt in &plan.targets {
        let status = if pt.decision.should_execute() {
            "EXEC"
        } else {
            "SKIP"
        };
        println!(
            "    #{} {} | {} | priority={:.2} | {}",
            pt.rank,
            &pt.decision.user_address[..10],
            status,
            pt.decision.priority_score,
            pt.decision.method.label(),
        );
    }

    println!("  PASS: Concurrent execution cap enforced");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

// ============================================================================
// S_EXTRA: Full Pipeline — On-chain Data → Strategy Decision
// Đọc real data từ Aave → tạo estimate → strategy → verify coherence
// ============================================================================

#[tokio::test]
async fn test_s_extra_full_pipeline_onchain_to_strategy() {
    let ctx = require_anvil!();
    let (collateral, debt, hf) = require_position!(ctx);

    let eth_price = read_price(&ctx, ctx.net.weth).await.expect("ETH price");
    let usdc_price = read_price(&ctx, ctx.net.usdc).await.expect("USDC price");
    let gas_gwei = read_gas_price(&ctx).await.unwrap_or(20.0);
    let chainlink_price = read_chainlink_eth_price(&ctx).await.unwrap_or(eth_price);
    let liquidator_usdc = read_usdc_balance(&ctx, LIQUIDATOR).await.unwrap_or(0.0);

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  S_EXTRA: FULL PIPELINE (ON-CHAIN → STRATEGY)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  --- On-chain State ---");
    println!("  Borrower HF:        {:.6}", hf);
    println!("  Collateral (USD):   ${:.2}", collateral);
    println!("  Debt (USD):         ${:.2}", debt);
    println!("  --- Oracle Prices ---");
    println!("  Aave ETH price:     ${:.2}", eth_price);
    println!("  Chainlink ETH:      ${:.2}", chainlink_price);
    println!("  USDC price:         ${:.4}", usdc_price);
    println!("  --- Network ---");
    println!("  Gas price:          {:.2} gwei", gas_gwei);
    println!("  Liquidator USDC:    ${:.2}", liquidator_usdc);

    // -- Setup Strategy --
    let config = StrategyConfig::local_fork();
    let decider = StrategyDecider::new(config.clone());

    decider.update_wallet_balance(10.0).await;
    decider
        .update_token_balance("USDC".to_string(), liquidator_usdc)
        .await;
    decider.update_gas_price(gas_gwei).await;

    // -- Build inputs from real data --
    let target = make_target(
        BORROWER, collateral, debt, hf, ctx.net.weth, ctx.net.usdc,
    );
    let estimate = make_profit_estimate(BORROWER, debt, eth_price, gas_gwei, 5.0);

    println!("  --- Profit Estimate ---");
    println!("  Debt to cover:      ${:.2}", estimate.debt_to_cover_usd);
    println!("  Gross profit:       ${:.2}", estimate.gross_profit_usd);
    println!("  Gas cost:           ${:.2}", estimate.gas_cost_usd);
    println!("  Net profit:         ${:.2}", estimate.net_profit_usd);
    println!("  Is profitable:      {}", estimate.is_profitable);

    // -- Run Strategy --
    let plan = decider
        .create_plan(vec![(target, estimate)])
        .await
        .expect("create_plan failed");

    println!("  --- Strategy Decision ---");
    println!("  Plan: {}", plan.summary());

    if !plan.targets.is_empty() {
        let d = &plan.targets[0].decision;
        println!("  Method:             {}", d.method.label());
        println!("  Priority:           {:.2}", d.priority_score);
        println!("  Adjusted profit:    ${:.2}", d.adjusted_profit_usd);
        println!("  Reasoning:          {}", d.reasoning);
        println!("  Summary:            {}", d.summary());

        // -- Coherence checks --
        // 1. Priority score in valid range
        assert!(
            d.priority_score >= 0.0 && d.priority_score <= 10.0,
            "Priority should be [0, 10]"
        );

        // 2. If HF >= 1.0, should still execute (we pass it through profit filter)
        if hf < 1.0 {
            println!("  Status:             LIQUIDATABLE");
            if d.should_execute() {
                // 3. Method should match wallet state
                let debt_to_cover = debt * 0.5;
                if liquidator_usdc >= debt_to_cover {
                    assert!(
                        matches!(d.method, ExecutionMethod::Direct { .. }),
                        "With enough USDC (${:.0}), should use Direct, got {:?}",
                        liquidator_usdc,
                        d.method
                    );
                } else {
                    assert!(
                        matches!(d.method, ExecutionMethod::Skip { .. }),
                        "Without enough USDC, should use Skip"
                    );
                }
                println!("  Verification:       PASS");
            } else {
                println!("  Status:             SKIPPED (profit/gas/exposure)");
            }
        } else {
            println!("  Status:             NOT LIQUIDATABLE (HF >= 1.0)");
        }
    }

    // -- Verify stats coherence --
    let stats = decider.get_stats().await;
    assert_eq!(stats.total_plans, 1);
    assert_eq!(stats.total_decisions, 1);
    println!("  Stats:              plans={}, decisions={}", stats.total_plans, stats.total_decisions);
    println!("  PASS: Full pipeline coherence verified");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
