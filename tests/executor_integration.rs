// ============================================================================
// EXECUTOR INTEGRATION TEST
// ============================================================================
//
// Test end-to-end liquidation executor trên Anvil mainnet fork.
//
// Cách chạy:
//   1. Khởi động Anvil: .\scripts\start_anvil.ps1
//   2. Setup scenario:  .\scripts\setup_liquidation_scenario.ps1
//   3. Crash giá:       .\scripts\crash_price.ps1
//   4. Chạy test:       cargo test --test executor_integration -- --nocapture
//
// Hoặc chạy từng test riêng:
//   cargo test --test executor_integration test_connect_anvil -- --nocapture
//   cargo test --test executor_integration test_read_account_data -- --nocapture
//   cargo test --test executor_integration test_dry_run_liquidation -- --nocapture
// ============================================================================

use std::sync::Arc;
use std::collections::HashMap;
use ethers::prelude::*;
use ethers::providers::{Provider, Http, Middleware};
use ethers::types::{Address, U256, H160};
use anyhow::Result;

// Import from our crate
use liquidator::executor::{ExecutorConfig, LiquidationExecutor, WorkerConfig};
use liquidator::storage::{LiquidationTarget, HybridStorage};

// ============================================================================
// CONSTANTS
// ============================================================================

/// Anvil default RPC URL
const ANVIL_RPC: &str = "http://127.0.0.1:8545";

// ============================================================================
// MAINNET ADDRESSES (Chain ID: 1)
// ============================================================================
mod mainnet {
    pub const AAVE_POOL: &str = "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2";
    pub const AAVE_ORACLE: &str = "0x54586bE62E3c3580375aE3723C145253060Ca0C2";
    pub const WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
    pub const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
}

// ============================================================================
// SEPOLIA ADDRESSES (Chain ID: 11155111)
// ============================================================================
mod sepolia {
    pub const AAVE_POOL: &str = "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951";
    pub const AAVE_ORACLE: &str = "0x2da88497588bf89281816106C7259e31AF45a663";
    pub const WETH: &str = "0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c";
    pub const USDC: &str = "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8";
}

/// Anvil Account #2 (Borrower)
const BORROWER: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

/// Anvil Account #3 (Liquidator)
const LIQUIDATOR: &str = "0x90F79bf6EB2c4f870365E785982E1f101E93b906";
const LIQUIDATOR_KEY: &str = "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

/// Network config struct
struct NetworkConfig {
    pub aave_pool: &'static str,
    pub aave_oracle: &'static str,
    pub weth: &'static str,
    pub usdc: &'static str,
    pub name: &'static str,
}

/// Get network config based on chain ID
fn get_network_config(chain_id: u64) -> NetworkConfig {
    match chain_id {
        1 => NetworkConfig {
            aave_pool: mainnet::AAVE_POOL,
            aave_oracle: mainnet::AAVE_ORACLE,
            weth: mainnet::WETH,
            usdc: mainnet::USDC,
            name: "Ethereum Mainnet",
        },
        11155111 => NetworkConfig {
            aave_pool: sepolia::AAVE_POOL,
            aave_oracle: sepolia::AAVE_ORACLE,
            weth: sepolia::WETH,
            usdc: sepolia::USDC,
            name: "Sepolia Testnet",
        },
        _ => {
            // Default to mainnet for unknown chain IDs (e.g., Anvil's 31337)
            println!("⚠️  Unknown chain ID {}, using Mainnet config", chain_id);
            NetworkConfig {
                aave_pool: mainnet::AAVE_POOL,
                aave_oracle: mainnet::AAVE_ORACLE,
                weth: mainnet::WETH,
                usdc: mainnet::USDC,
                name: "Ethereum Mainnet (default)",
            }
        }
    }
}

// ============================================================================
// ABI BINDINGS
// ============================================================================

abigen!(
    AavePool,
    r#"[
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
        function liquidationCall(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken) external
    ]"#
);

abigen!(
    AaveOracle,
    r#"[
        function getAssetPrice(address asset) external view returns (uint256)
    ]"#
);

abigen!(
    ERC20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
        function symbol() external view returns (string)
        function approve(address spender, uint256 amount) external returns (bool)
    ]"#
);

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Test context với provider và network config
struct TestContext {
    provider: Arc<Provider<Http>>,
    config: NetworkConfig,
}

/// Kết nối đến Anvil và auto-detect network
async fn connect_anvil() -> Result<TestContext> {
    let provider = Provider::<Http>::try_from(ANVIL_RPC)?;
    
    // Verify connection
    let block = provider.get_block_number().await?;
    let chain_id = provider.get_chainid().await?;
    
    let config = get_network_config(chain_id.as_u64());
    
    println!("✅ Connected to {} at block #{}", config.name, block);
    println!("   Chain ID: {}", chain_id);
    println!("   Aave Pool: {}", config.aave_pool);
    
    Ok(TestContext {
        provider: Arc::new(provider),
        config,
    })
}

/// Đọc account data từ Aave
async fn get_account_data(ctx: &TestContext, user: &str) -> Result<(f64, f64, f64)> {
    let pool_address: Address = ctx.config.aave_pool.parse()?;
    let user_address: Address = user.parse()?;
    let pool = AavePool::new(pool_address, Arc::clone(&ctx.provider));
    
    let data = pool.get_user_account_data(user_address).call().await?;
    
    // Parse values safely - Aave returns max U256 for infinite HF (no debt)
    let total_collateral = u256_to_f64(data.0, 8);
    let total_debt = u256_to_f64(data.1, 8);
    
    // Health factor: if debt = 0, Aave returns max U256 (infinity)
    // We cap it at a large but representable value
    let health_factor = if data.1.is_zero() {
        f64::INFINITY
    } else {
        u256_to_f64(data.5, 18)
    };
    
    Ok((total_collateral, total_debt, health_factor))
}

/// Safe U256 to f64 conversion with decimal handling
fn u256_to_f64(value: U256, decimals: u32) -> f64 {
    // Handle very large values that would overflow u128
    let max_safe = U256::from(u128::MAX);
    
    if value > max_safe {
        // Value is larger than u128::MAX, return a capped value
        return f64::MAX;
    }
    
    let as_u128 = value.low_u128();
    as_u128 as f64 / 10_f64.powi(decimals as i32)
}

/// Lấy giá asset từ Aave Oracle
async fn get_asset_price(ctx: &TestContext, asset: &str) -> Result<f64> {
    let oracle_address: Address = ctx.config.aave_oracle.parse()?;
    let asset_address: Address = asset.parse()?;
    let oracle = AaveOracle::new(oracle_address, Arc::clone(&ctx.provider));
    
    let price = oracle.get_asset_price(asset_address).call().await?;
    Ok(u256_to_f64(price, 8))
}

/// Tạo LiquidationTarget từ on-chain data
fn create_target_from_data(
    ctx: &TestContext,
    user: &str, 
    collateral_usd: f64, 
    debt_usd: f64, 
    health_factor: f64
) -> LiquidationTarget {
    let mut collateral = HashMap::new();
    collateral.insert(ctx.config.weth.to_string(), collateral_usd);
    
    let mut debt = HashMap::new();
    debt.insert(ctx.config.usdc.to_string(), debt_usd);
    
    LiquidationTarget {
        user_address: user.to_string(),
        health_factor,
        total_collateral_usd: collateral_usd,
        total_debt_usd: debt_usd,
        ltv: 0.8,
        liquidation_threshold: 0.85,
        collateral,
        debt,
        estimated_profit: debt_usd * 0.05, // 5% liquidation bonus estimate
        risk_score: if health_factor < 1.0 { 10 } else { 5 },
        last_updated: chrono::Utc::now().timestamp(),
    }
}

// ============================================================================
// TESTS
// ============================================================================

/// Test 1: Kiểm tra kết nối đến Anvil
#[tokio::test]
async fn test_connect_anvil() {
    let ctx = connect_anvil().await;
    assert!(ctx.is_ok(), "Không thể kết nối Anvil! Hãy chạy: .\\scripts\\start_anvil.ps1");
    
    let ctx = ctx.unwrap();
    let chain_id = ctx.provider.get_chainid().await.unwrap();
    println!("Chain ID: {}", chain_id);
    println!("Network: {}", ctx.config.name);
    
    // Anvil fork mainnet → chain_id = 1, sepolia → 11155111
    assert!(
        chain_id.as_u64() == 1 || chain_id.as_u64() == 11155111 || chain_id.as_u64() == 31337, 
        "Unexpected chain ID: {}", chain_id
    );
}

/// Test 2: Đọc account data từ Aave
#[tokio::test]
async fn test_read_account_data() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  BORROWER ACCOUNT DATA ({})", ctx.config.name);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Collateral: ${:.2}", collateral);
    println!("  Debt:       ${:.2}", debt);
    println!("  HF:         {:.6}", hf);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    // Nếu đã chạy setup_liquidation_scenario, sẽ có collateral & debt
    if debt > 0.0 {
        if collateral > 0.0 {
            println!("✅ Borrower có vị thế mở trên Aave");
            assert!(hf > 0.0, "Health factor phải > 0");
        } else {
            // Position was fully liquidated - has bad debt but no collateral
            println!("⚠️  Position đã bị liquidate hoàn toàn (bad debt còn lại)");
        }
    } else {
        println!("⚠️  Borrower chưa có vị thế. Chạy: .\\scripts\\setup_liquidation_scenario.ps1");
    }
}

/// Test 3: Kiểm tra giá ETH từ Aave Oracle
#[tokio::test]
async fn test_read_oracle_price() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let eth_price = get_asset_price(&ctx, ctx.config.weth).await
        .expect("Failed to read ETH price");
    
    let usdc_price = get_asset_price(&ctx, ctx.config.usdc).await
        .expect("Failed to read USDC price");
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ORACLE PRICES ({})", ctx.config.name);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ETH/USD:  ${:.2}", eth_price);
    println!("  USDC/USD: ${:.4}", usdc_price);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    assert!(eth_price > 0.0, "ETH price must be > 0");
    assert!(usdc_price > 0.0 && usdc_price < 2.0, "USDC price should be ~1.0");
}

/// Test 4: Phát hiện vị thế liquidatable
#[tokio::test]
async fn test_detect_liquidatable() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  LIQUIDATION DETECTION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Health Factor: {:.6}", hf);
    
    if debt == 0.0 {
        println!("  ⚠️  Chưa có position. Chạy setup script trước.");
        return;
    }
    
    if hf < 1.0 {
        println!("  🔴 LIQUIDATABLE! HF = {:.6} < 1.0", hf);
        println!("  → Có thể liquidate position này");
        
        // Calculate profit estimate
        let liquidation_bonus = 0.05; // 5% standard bonus
        let max_liquidatable_debt = debt * 0.5; // 50% close factor
        let estimated_profit = max_liquidatable_debt * liquidation_bonus;
        
        println!("  💰 Max debt to cover: ${:.2}", max_liquidatable_debt);
        println!("  💰 Estimated profit:  ${:.2}", estimated_profit);
    } else if hf < 1.1 {
        println!("  🟡 RISKY! HF = {:.6} - Gần ngưỡng liquidation", hf);
        println!("  → Chạy .\\scripts\\crash_price.ps1 để crash giá");
    } else {
        println!("  🟢 SAFE. HF = {:.6}", hf);
        println!("  → Chạy .\\scripts\\crash_price.ps1 -DropPercent 40 để crash");
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Test 5: DRY RUN liquidation (không gửi TX thật)
#[tokio::test]
async fn test_dry_run_liquidation() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    // Đọc account data
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    if debt == 0.0 {
        println!("⚠️  Chưa có position. Chạy setup script trước.");
        return;
    }
    
    // Config dry run
    let pool_address: H160 = ctx.config.aave_pool.parse().unwrap();
    let config = ExecutorConfig {
        dry_run: true,
        min_profit_usd: 0.001, // Rất thấp để test
        max_gas_price_gwei: 1000.0,
        simulate_before_execute: false, // Skip simulate in dry run
        aave_pool_address: pool_address,
        ..ExecutorConfig::default()
    };
    
    // Tạo executor
    let executor = LiquidationExecutor::new(config, ctx.provider.clone(), LIQUIDATOR_KEY)
        .await
        .expect("Failed to create executor");
    
    // Tạo target
    let target = create_target_from_data(&ctx, BORROWER, collateral, debt, hf);
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DRY RUN LIQUIDATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Target: {}", target.user_address);
    println!("  HF:     {:.6}", target.health_factor);
    println!("  Profit: ${:.2}", target.estimated_profit);
    
    // Execute dry run
    let result = executor.liquidate(&target).await
        .expect("Liquidate call failed");
    
    println!("  Result: success={}, tx={:?}", result.success, result.tx_hash);
    
    if hf < 1.0 {
        // HF < 1.0: dry run should succeed
        assert!(result.success, "Dry run should succeed for liquidatable target");
        println!("  ✅ Dry run thành công!");
    } else {
        // HF >= 1.0: preflight should fail
        println!("  ℹ️  Position not liquidatable (HF >= 1.0): {:?}", result.error);
    }
    
    // Check stats
    let stats = executor.stats().await;
    println!("  Stats: {:?}", stats);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Test 6: SIMULATE liquidation (eth_call, không gửi TX)
#[tokio::test]
async fn test_simulate_liquidation() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    if debt == 0.0 || hf >= 1.0 {
        println!("⚠️  Position không liquidatable (HF={:.4}). Chạy crash_price.ps1 trước.", hf);
        return;
    }
    
    let pool_address: H160 = ctx.config.aave_pool.parse().unwrap();
    let config = ExecutorConfig {
        dry_run: false,
        simulate_before_execute: true,
        min_profit_usd: 0.001,
        max_gas_price_gwei: 1000.0,
        aave_pool_address: pool_address,
        ..ExecutorConfig::default()
    };
    
    let _executor = LiquidationExecutor::new(config, ctx.provider.clone(), LIQUIDATOR_KEY)
        .await
        .expect("Failed to create executor");
    
    // Create signed provider so eth_call has correct msg.sender
    let wallet: LocalWallet = LIQUIDATOR_KEY.parse::<LocalWallet>().unwrap()
        .with_chain_id(ctx.provider.get_chainid().await.unwrap().as_u64());
    let signer = Arc::new(SignerMiddleware::new(
        (*ctx.provider).clone(),
        wallet.clone(),
    ));
    
    // Approve USDC to Pool (required for liquidationCall)
    // May fail with nonce error if another test ran first - that's OK
    let usdc_address: Address = ctx.config.usdc.parse().unwrap();
    let usdc = ERC20::new(usdc_address, Arc::clone(&signer));
    match usdc.approve(pool_address, U256::MAX).send().await {
        Ok(pending) => {
            let _ = pending.await; // Wait for confirmation
            println!("  \u{2705} USDC approved to Pool");
        }
        Err(e) => {
            println!("  \u{26a0}\u{fe0f}  Approve skipped (may already be approved): {:?}", e);
        }
    }
    
    // Simulate using signed pool (sets correct msg.sender)
    let pool = AavePool::new(pool_address, Arc::clone(&signer));
    let user_address: Address = BORROWER.parse().unwrap();
    let weth_address: Address = ctx.config.weth.parse().unwrap();
    
    // Use U256::MAX — Aave caps at 50% close factor automatically
    let debt_to_cover = U256::MAX;
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SIMULATE LIQUIDATION ({})", ctx.config.name);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  User:         {}", BORROWER);
    println!("  HF:           {:.6}", hf);
    println!("  Collateral:   ${:.2} (WETH)", collateral);
    println!("  Debt:         ${:.2} (USDC)", debt);
    println!("  Covering:     50% of debt");
    
    // eth_call simulation
    let result = pool
        .liquidation_call(
            weth_address,     // collateral
            usdc_address,     // debt
            user_address,     // user
            debt_to_cover,
            false,            // receiveAToken
        )
        .call()
        .await;
    
    match result {
        Ok(_) => {
            println!("  ✅ Simulation PASSED - Liquidation sẽ thành công!");
        }
        Err(e) => {
            println!("  ❌ Simulation REVERTED: {:?}", e);
            println!("  Có thể cần approve USDC cho Pool trước");
        }
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Test 7: EXECUTE liquidation thật (gửi TX lên Anvil)
/// ⚠️ Chỉ chạy test này khi đã setup scenario + crash price
#[tokio::test]
async fn test_execute_real_liquidation() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    if debt == 0.0 {
        println!("⚠️  Chưa có position. Chạy setup script trước.");
        return;
    }
    
    if hf >= 1.0 {
        println!("⚠️  Position chưa liquidatable (HF={:.4}). Chạy crash_price.ps1 trước.", hf);
        return;
    }
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  🔴 EXECUTE REAL LIQUIDATION ({})", ctx.config.name);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    // 1. Kiểm tra liquidator có USDC không
    let usdc_address: Address = ctx.config.usdc.parse().unwrap();
    let liquidator_address: Address = LIQUIDATOR.parse().unwrap();
    let usdc = ERC20::new(usdc_address, ctx.provider.clone());
    
    let usdc_balance = usdc.balance_of(liquidator_address).call().await
        .expect("Failed to get USDC balance");
    let usdc_balance_f = u256_to_f64(usdc_balance, 6);
    println!("  Liquidator USDC: ${:.2}", usdc_balance_f);
    
    if usdc_balance_f < debt * 0.5 {
        println!("  ❌ Không đủ USDC! Cần ít nhất ${:.2}", debt * 0.5);
        println!("  → Chạy lại setup_liquidation_scenario.ps1");
        return;
    }
    
    // 2. Config executor (KHÔNG dry run)
    let pool_address: H160 = ctx.config.aave_pool.parse().unwrap();
    let config = ExecutorConfig {
        dry_run: false,
        simulate_before_execute: true,
        min_profit_usd: 0.001,
        max_gas_price_gwei: 1000.0,
        aave_pool_address: pool_address,
        tx_timeout_secs: 30,
        ..ExecutorConfig::default()
    };
    
    let executor = LiquidationExecutor::new(config, ctx.provider.clone(), LIQUIDATOR_KEY)
        .await
        .expect("Failed to create executor");
    
    // 3. Tạo target
    let target = create_target_from_data(&ctx, BORROWER, collateral, debt, hf);
    
    println!("  Target:  {}", target.user_address);
    println!("  HF:      {:.6}", target.health_factor);
    println!("  Profit:  ${:.2}", target.estimated_profit);
    println!("");
    println!("  Executing liquidation...");
    
    // 4. Execute!
    let result = executor.liquidate(&target).await
        .expect("Liquidate call failed");
    
    println!("");
    if result.success {
        println!("  ✅ LIQUIDATION THÀNH CÔNG!");
        println!("  TX Hash:          {:?}", result.tx_hash);
        println!("  Gas Used:         {}", result.gas_used);
        println!("  Collateral Seized: ${:.2}", result.collateral_seized);
        println!("  Debt Covered:     ${:.2}", result.debt_covered);
        println!("  Profit:           ${:.2}", result.profit_usd);
        
        // Verify: kiểm tra HF sau liquidation
        let (new_collateral, new_debt, new_hf) = get_account_data(&ctx, BORROWER).await
            .expect("Failed to read post-liquidation data");
        println!("");
        println!("  HF trước: {:.6}", hf);
        println!("  HF sau:   {:.6}", new_hf);
        println!("  Collateral sau: ${:.2}", new_collateral);
        println!("  Debt sau:       ${:.2}", new_debt);
        
        // After liquidation:
        // - If partial: HF should increase
        // - If full (all collateral seized): HF = 0 or infinity (no debt left)
        if new_collateral == 0.0 {
            println!("  ℹ️  Position fully liquidated (all collateral seized)");
        } else if new_debt == 0.0 {
            println!("  ℹ️  All debt repaid, position closed");
        } else {
            assert!(new_hf > hf, "HF phải tăng sau partial liquidation");
        }
    } else {
        println!("  ❌ LIQUIDATION THẤT BẠI!");
        println!("  Error: {:?}", result.error);
    }
    
    // 5. Stats
    let stats = executor.stats().await;
    println!("");
    println!("  📊 Executor Stats:");
    println!("     Attempts:   {}", stats.total_attempts);
    println!("     Successful: {}", stats.successful);
    println!("     Failed:     {}", stats.failed);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

/// Test 8: Worker integration test (chạy worker loop ngắn)
#[tokio::test]
async fn test_executor_worker_loop() {
    let ctx = connect_anvil().await
        .expect("Cần Anvil đang chạy!");
    
    let (collateral, debt, hf) = get_account_data(&ctx, BORROWER).await
        .expect("Failed to read account data");
    
    if debt == 0.0 {
        println!("⚠️  Chưa có position. Bỏ qua worker test.");
        return;
    }
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  WORKER INTEGRATION TEST ({})", ctx.config.name);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    // 1. Tạo HybridStorage và thêm target
    let storage = HybridStorage::new().await
        .expect("Failed to create storage");
    let storage = Arc::new(storage);
    
    let target = create_target_from_data(&ctx, BORROWER, collateral, debt, hf);
    storage.update_user_hf(target.clone()).await
        .expect("Failed to update target");
    
    println!("  ✅ Added target to HotCache (HF: {:.4})", hf);
    
    // 2. Tạo executor (dry run để an toàn)
    let pool_address: H160 = ctx.config.aave_pool.parse().unwrap();
    let config = ExecutorConfig {
        dry_run: true,
        min_profit_usd: 0.001,
        max_gas_price_gwei: 1000.0,
        aave_pool_address: pool_address,
        ..ExecutorConfig::default()
    };
    
    let executor = Arc::new(
        LiquidationExecutor::new(config, ctx.provider.clone(), LIQUIDATOR_KEY)
            .await
            .expect("Failed to create executor")
    );
    
    // 3. Config worker  
    let worker_config = WorkerConfig {
        check_interval_ms: 100,
        batch_size: 5,
        liquidation_threshold: 1.0,
        parallel_execution: false,
        max_concurrent: 1,
    };
    
    // 4. Chạy worker trong 1 giây
    let executor_clone = Arc::clone(&executor);
    let storage_clone = Arc::clone(&storage);
    
    let worker_handle = tokio::spawn(async move {
        liquidator::executor::executor_worker(executor_clone, storage_clone, worker_config).await;
    });
    
    // Đợi 1 giây rồi cancel
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    worker_handle.abort();
    
    // 5. Kiểm tra stats
    let stats = executor.stats().await;
    println!("  📊 After 1s of worker:");
    println!("     Attempts:   {}", stats.total_attempts);
    println!("     Successful: {}", stats.successful);
    println!("     Failed:     {}", stats.failed);
    
    if hf < 1.0 {
        // Should have attempted liquidation
        assert!(stats.total_attempts > 0, "Worker should have attempted liquidation");
        println!("  ✅ Worker detected and attempted liquidation!");
    } else {
        println!("  ℹ️  HF >= 1.0, worker skipped (correct behavior)");
    }
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
