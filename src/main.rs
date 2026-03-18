mod events;
mod risk;
mod data;
mod executor;
mod mempool;
mod oracle;
mod profit;
mod provider;
mod storage;
mod strategy;

use tokio::sync::mpsc;
use std::sync::Arc;
use crate::risk::engine::RiskEngine;
use crate::events::event::Event;
use crate::data::asset::Asset;
use crate::data::user::User;
use crate::provider::AaveProvider;
use crate::oracle::{OracleManager, OracleConfig, OracleWorkerConfig};
use crate::oracle::worker::{oracle_price_worker, oracle_stats_worker, oracle_health_worker};
use crate::profit::{ProfitCalculator, ProfitConfig, GasEstimator};
use crate::strategy::{StrategyDecider, StrategyConfig};
use crate::storage::HybridStorage;
use crate::storage::sync::{stats_logger_worker, memory_monitor_worker};
use crate::executor::{LiquidationExecutor, ExecutorConfig, WorkerConfig};
use crate::executor::worker::{executor_worker, stats_worker, nonce_sync_worker};

/// # Liquidator System - Main Entry Point
/// 
/// Hệ thống giám sát và thanh lý các vị thế có rủi ro trên Aave Protocol
/// 
/// ## Kiến trúc:
/// 1. Provider Layer: Kết nối với blockchain (RPC)
/// 2. Event Watchers: Theo dõi blocks và Aave events
/// 3. Risk Engine: Tính toán health factor và phát hiện vị thế rủi ro
/// 4. Event Channel: Truyền tải events giữa các components (MPSC channel)
/// 
/// ## Luồng xử lý:
/// - Event Watchers phát hiện thay đổi (price, deposit, borrow, etc.)
/// - Events được gửi qua channel đến Risk Engine
/// - Risk Engine tính toán lại health factors
/// - Vị thế có HF < 1.0 sẽ được đánh dấu để thanh lý
#[tokio::main]
async fn main() {
    // ============================================================================
    // PHASE 0: SYSTEM INITIALIZATION
    // ============================================================================
    
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Liquidator System...");

    // ============================================================================
    // PHASE 1: CONNECT TO BLOCKCHAIN
    // ============================================================================
    
    // Kết nối đến Aave fork (local testnet hoặc mainnet fork)
    let rpc_url = "http://127.0.0.1:8545";
    let provider = match AaveProvider::new(rpc_url).await {
        Ok(p) => {
            tracing::info!("✓ Connected to Aave fork at {}", rpc_url);
            Arc::new(p)
        }
        Err(e) => {
            tracing::error!("✗ Failed to connect to Aave fork: {:?}", e);
            tracing::error!("Please ensure Anvil/Hardhat is running at {}", rpc_url);
            return;
        }
    };

    // ============================================================================
    // PHASE 2: SETUP EVENT COMMUNICATION CHANNEL
    // ============================================================================
    
    // MPSC channel: Event watchers (producers) -> Risk Engine (consumer)
    // Buffer size: 100 events
    let (tx, rx) = mpsc::channel(100);

    // ============================================================================
    // PHASE 2.5: INITIALIZE HYBRID STORAGE
    // ============================================================================

    let storage = match HybridStorage::new().await {
        Ok(s) => {
            tracing::info!("✓ Hybrid Storage initialized");
            Arc::new(s)
        }
        Err(e) => {
            tracing::error!("✗ Failed to initialize storage: {:?}", e);
            return;
        }
    };

    // Spawn background sync worker: flushes hot cache -> SQLite every 5s
    let _sync_handle = Arc::clone(&storage).spawn_sync_worker();

    // Storage observability workers
    let storage_for_stats = Arc::clone(&storage);
    tokio::spawn(async move {
        stats_logger_worker(storage_for_stats, 30).await;
    });

    let storage_for_memory = Arc::clone(&storage);
    tokio::spawn(async move {
        memory_monitor_worker(storage_for_memory).await;
    });

    // ============================================================================
    // PHASE 3: INITIALIZE RISK ENGINE
    // ============================================================================
    
    let mut engine = RiskEngine::new(rx, Arc::clone(&storage));

    // ============================================================================
    // PHASE 4: POPULATE INITIAL DATA (SIMULATION)
    // ============================================================================
    
    initialize_simulation_data(&mut engine);

    // ============================================================================
    // PHASE 5: SPAWN BACKGROUND WORKERS
    // ============================================================================
    
    // 5.1 Risk Engine Worker
    // Chạy event loop để xử lý tất cả incoming events
    let _engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // 5.2 Block Watcher Worker
    // Theo dõi các blocks mới trên blockchain
    let provider_for_blocks = Arc::clone(&provider);
    let tx_for_blocks = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = provider_for_blocks.watch_blocks(tx_for_blocks).await {
            tracing::error!("Block watcher error: {:?}", e);
        }
    });

    // 5.3 Aave Event Watcher Worker
    // Theo dõi các events từ Aave Pool contract:
    // - Supply (deposit collateral)
    // - Borrow (vay)
    // - Repay (trả nợ)
    // - Withdraw (rút collateral)
    // - Liquidation (thanh lý)
    let aave_pool_address = "0xE7EC1B0015eb2ADEedb1B7f9F1Ce82F9DAD6dF08"
        .parse()
        .expect("Invalid Aave pool address");
    
    let provider_for_events = Arc::clone(&provider);
    let tx_for_events = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = provider_for_events
            .watch_aave_events(aave_pool_address, tx_for_events)
            .await
        {
            tracing::error!("Aave event watcher error: {:?}", e);
        }
    });

    // ============================================================================
    // PHASE 6: ORACLE PRICE FEEDS
    // ============================================================================
    
    // Khởi tạo Oracle module — theo dõi giá realtime từ Chainlink
    let oracle_config = OracleConfig::local_fork(); // Dùng local_fork() cho Anvil
    let tx_for_oracle = tx.clone();
    
    match OracleManager::new(oracle_config.clone(), provider.provider(), tx_for_oracle).await {
        Ok(mut oracle_manager) => {
            // Khởi tạo feeds (đọc metadata + giá ban đầu)
            if let Err(e) = oracle_manager.initialize().await {
                tracing::warn!("Oracle initialization partial failure: {:?}", e);
            }
            
            let oracle = Arc::new(oracle_manager);
            
            // 6.1 Oracle Price Worker — poll giá định kỳ
            let oracle_for_price = Arc::clone(&oracle);
            let worker_config = OracleWorkerConfig {
                poll_interval_ms: oracle_config.poll_interval_ms,
                stats_interval_secs: 60,
                health_check_interval_secs: 300,
            };
            
            let price_worker_config = worker_config.clone();
            tokio::spawn(async move {
                oracle_price_worker(oracle_for_price, price_worker_config).await;
            });
            
            // 6.2 Oracle Stats Worker — log thống kê định kỳ
            let oracle_for_stats = Arc::clone(&oracle);
            let stats_worker_config = worker_config.clone();
            tokio::spawn(async move {
                oracle_stats_worker(oracle_for_stats, stats_worker_config).await;
            });
            
            // 6.3 Oracle Health Worker — kiểm tra sức khỏe feeds
            let oracle_for_health = Arc::clone(&oracle);
            let health_worker_config = worker_config.clone();
            tokio::spawn(async move {
                oracle_health_worker(oracle_for_health, health_worker_config).await;
            });
            
            tracing::info!("✓ Oracle workers spawned ({} feeds)", oracle.feed_count());
            
            // ── Khởi tạo Profit Calculator (sử dụng oracle price cache) ──
            let profit_config = ProfitConfig::local_fork(); // Dùng local_fork() cho Anvil
            let gas_estimator = GasEstimator::new(provider.provider());
            let profit_calculator = Arc::new(ProfitCalculator::new(
                profit_config,
                gas_estimator,
                oracle.price_cache(),
            ));
            
            tracing::info!("✓ Profit Calculator initialized (min_profit=${}, min_roi={}%)",
                profit_calculator.config().min_profit_usd,
                profit_calculator.config().min_roi_pct,
            );
            
            // ── Khởi tạo Strategy Decider (quyết định direct vs flash loan + ưu tiên targets) ──
            let strategy_config = StrategyConfig::local_fork(); // Dùng local_fork() cho Anvil
            let strategy_decider = Arc::new(StrategyDecider::new(strategy_config.clone()));
            
            tracing::info!("✓ Strategy Decider initialized (max_concurrent={}, flash_loan={})",
                strategy_config.max_concurrent_liquidations,
                strategy_config.flash_loan_available,
            );

            // ── Khởi tạo LiquidationExecutor và spawn executor workers ──
            // Đọc private key từ env-var; fallback sang Anvil account #0 cho local testing
            let private_key = std::env::var("PRIVATE_KEY").unwrap_or_else(|_| {
                tracing::warn!(
                    "PRIVATE_KEY not set — using Anvil default account (local testing only)"
                );
                "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed35a24fd173c1cdd3d3".to_string()
            });

            let mut executor_config = ExecutorConfig::testnet(aave_pool_address);
            executor_config.use_flash_loan = strategy_config.flash_loan_available;
            executor_config.liquidator_contract = std::env::var("LIQUIDATOR_CONTRACT")
                .ok()
                .and_then(|addr| addr.parse().ok());

            if strategy_config.flash_loan_available && executor_config.liquidator_contract.is_none() {
                tracing::warn!(
                    "Strategy has flash loan enabled but LIQUIDATOR_CONTRACT is not configured; flash-loan executions will fail"
                );
            }

            match LiquidationExecutor::new(
                executor_config,
                provider.provider(),
                &private_key,
            ).await {
                Ok(executor) => {
                    let executor = Arc::new(executor);

                    // Main liquidation loop: polls hot cache every 100ms
                    let executor_for_worker = Arc::clone(&executor);
                    let storage_for_worker = Arc::clone(&storage);
                    let profit_for_worker  = Arc::clone(&profit_calculator);
                    let strategy_for_worker = Arc::clone(&strategy_decider);
                    tokio::spawn(async move {
                        executor_worker(
                            executor_for_worker,
                            storage_for_worker,
                            WorkerConfig::default(),
                            Some(profit_for_worker),
                            Some(strategy_for_worker),
                        ).await;
                    });

                    // Stats logging worker: prints metrics every 60s
                    let executor_for_stats = Arc::clone(&executor);
                    tokio::spawn(async move {
                        stats_worker(executor_for_stats, 60).await;
                    });

                    // Nonce sync worker: re-syncs on-chain nonce every 30s
                    let executor_for_nonce = Arc::clone(&executor);
                    tokio::spawn(async move {
                        nonce_sync_worker(executor_for_nonce, 30).await;
                    });

                    tracing::info!("✓ Executor workers spawned (dry_run=false)");
                }
                Err(e) => {
                    tracing::error!("✗ Failed to initialize executor: {:?}", e);
                    tracing::warn!("System running without execution capability");
                }
            }
        }
        Err(e) => {
            tracing::error!("✗ Failed to create OracleManager: {:?}", e);
            tracing::warn!("System will run without oracle price feeds");
            
            // Fallback: chạy simulation worker thay thế
            spawn_simulation_worker(tx.clone());
        }
    }

    // ============================================================================
    // PHASE 7: KEEP SYSTEM ALIVE — wait for Ctrl+C
    // ============================================================================

    tracing::info!("✓ All workers running. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");

    tracing::info!("Received Ctrl+C — shutting down...");
}

/// Khởi tạo dữ liệu mô phỏng cho hệ thống
/// 
/// Tạo 2 assets (ETH, USDC) và 2 users với các vị thế khác nhau:
/// - user_safe: Health Factor cao (> 3.0) - An toàn
/// - user_risky: Health Factor thấp (~1.06) - Nguy hiểm, gần ngưỡng thanh lý
fn initialize_simulation_data(engine: &mut RiskEngine) {
    tracing::info!("Initializing simulation data...");
    
    // ------------------------------------------------------------------------
    // ASSETS CONFIGURATION
    // ------------------------------------------------------------------------
    
    let eth = Asset {
        id: "ETH".to_string(),
        symbol: "ETH".to_string(),
        decimals: 18,
        ltv: 0.80,                      // Loan-to-Value: 80%
        liquidation_threshold: 0.85,     // Thanh lý khi dưới 85%
        price_in_eth: 1.0,               // Base price
    };
    
    let usdc = Asset {
        id: "USDC".to_string(),
        symbol: "USDC".to_string(),
        decimals: 6,
        ltv: 0.80,
        liquidation_threshold: 0.85,
        price_in_eth: 0.0005,            // 1 ETH = 2000 USDC
    };
    
    engine.assets.insert("ETH".to_string(), eth);
    engine.assets.insert("USDC".to_string(), usdc);

    // ------------------------------------------------------------------------
    // USER 1: SAFE POSITION
    // ------------------------------------------------------------------------
    
    let mut user_safe = User::new("user_safe".to_string());
    user_safe.collateral.insert("ETH".to_string(), 10.0);   // Collateral: 10 ETH
    user_safe.debt.insert("USDC".to_string(), 5000.0);      // Debt: 5000 USDC (~2.5 ETH)
    
    // Health Factor = (Collateral * Price * LiqThreshold) / (Debt * Price)
    // HF = (10 * 1.0 * 0.85) / (5000 * 0.0005) = 8.5 / 2.5 = 3.4 ✓ SAFE
    
    engine.users.insert("user_safe".to_string(), user_safe);
    engine.registry.add_user_to_asset("ETH".to_string(), "user_safe".to_string());
    engine.registry.add_user_to_asset("USDC".to_string(), "user_safe".to_string());

    // ------------------------------------------------------------------------
    // USER 2: RISKY POSITION
    // ------------------------------------------------------------------------
    
    let mut user_risky = User::new("user_risky".to_string());
    user_risky.collateral.insert("ETH".to_string(), 10.0);  // Collateral: 10 ETH
    user_risky.debt.insert("USDC".to_string(), 16000.0);    // Debt: 16000 USDC (~8 ETH)
    
    // Health Factor = (10 * 1.0 * 0.85) / (16000 * 0.0005) = 8.5 / 8.0 = 1.0625
    // ⚠ DANGER: Rất gần ngưỡng thanh lý (HF < 1.0)
    
    engine.users.insert("user_risky".to_string(), user_risky);
    engine.registry.add_user_to_asset("ETH".to_string(), "user_risky".to_string());
    engine.registry.add_user_to_asset("USDC".to_string(), "user_risky".to_string());
    
    tracing::info!("✓ Initialized 2 assets and 2 users");
}

/// Worker mô phỏng sự kiện giá giảm (chỉ dùng để test)
/// 
/// Kịch bản: ETH giảm từ 1.0 -> 0.9
/// - user_safe: HF vẫn cao (~3.06) - Vẫn an toàn
/// - user_risky: HF giảm xuống ~0.95 - Bị thanh lý (HF < 1.0)
fn spawn_simulation_worker(tx: mpsc::Sender<Event>) {
    tokio::spawn(async move {
        // Đợi hệ thống khởi động xong
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        tracing::info!("{}", "=".repeat(60));
        tracing::info!("SIMULATION: ETH PRICE CRASH");
        tracing::info!("{}", "=".repeat(60));
        
        // Mô phỏng giá ETH giảm 10%
        // Khi giá giảm: collateral value giảm -> health factor giảm -> risk tăng
        if let Err(e) = tx.send(Event::PriceUpdate {
            asset_id: "ETH".to_string(),
            new_price: 0.9,  // ETH: 1.0 -> 0.9 (-10%)
        }).await {
            tracing::error!("Failed to send simulation event: {:?}", e);
        }
    });
}
