mod events;
mod risk;
mod data;
mod executor;
mod oracle;
mod profit;
mod provider;
mod storage;
mod strategy;
mod bootstrap;

use tokio::sync::mpsc;
use std::sync::Arc;
use std::path::Path;
use crate::risk::engine::{RiskEngine, RiskEngineConfig};
use crate::events::event::Event;
use crate::provider::AaveProvider;
use crate::oracle::{OracleManager, OracleConfig, OracleWorkerConfig};
use crate::oracle::worker::{
    oracle_price_worker,
    oracle_stats_worker,
    oracle_health_worker,
    oracle_chainlink_event_worker,
};
use crate::profit::{ProfitCalculator, ProfitConfig, GasEstimator};
use crate::strategy::{StrategyDecider, StrategyConfig};
use crate::storage::HybridStorage;
use crate::storage::sync::{stats_logger_worker, memory_monitor_worker};
use crate::executor::{LiquidationExecutor, ExecutorConfig, WorkerConfig};
use crate::executor::worker::{executor_worker, stats_worker, nonce_sync_worker};
use crate::bootstrap::onchain::bootstrap_onchain_state;

/// # Liquidator System - Main Entry Point
/// 
/// Hệ thống giám sát và thanh lý các vị thế có rủi ro trên Aave Protocol
/// 
/// ## Kiến trúc:
/// 1. RPC Provider: Kết nối với blockchain (RPC)
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
    // Load .env if present so local runs can use file-based env vars.
    let _ = dotenvy::dotenv();

    // ============================================================================
    // PHASE 0: SYSTEM INITIALIZATION
    // ============================================================================
    
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Liquidator System...");

    // ============================================================================
    // PHASE 1: CONNECT TO BLOCKCHAIN
    // ============================================================================
    
    // Kết nối đến Aave fork (local testnet hoặc mainnet fork)
    let rpc_url = env_string("RPC_URL", "http://127.0.0.1:8545");
    let provider = match AaveProvider::new(&rpc_url).await {
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
    // Buffer size: configurable via EVENT_CHANNEL_CAPACITY
    let event_channel_capacity = env_usize("EVENT_CHANNEL_CAPACITY", 100);
    let (tx, rx) = mpsc::channel(event_channel_capacity);

    // ============================================================================
    // PHASE 2.5: INITIALIZE HYBRID STORAGE
    // ============================================================================

    let reset_storage_on_start = env_bool("STORAGE_RESET_ON_START", false);
    let storage_db_path = env_string("STORAGE_DB_PATH", "liquidator.db");
    if reset_storage_on_start {
        if Path::new(&storage_db_path).exists() {
            match std::fs::remove_file(&storage_db_path) {
                Ok(_) => tracing::warn!(
                    "Removed storage DB on startup (STORAGE_RESET_ON_START=true): {}",
                    storage_db_path
                ),
                Err(e) => tracing::error!(
                    "Failed to remove storage DB {}: {:?}",
                    storage_db_path,
                    e
                ),
            }
        }
    }

    let storage = match HybridStorage::with_config(crate::storage::StorageConfig {
        db_path: storage_db_path,
        ..Default::default()
    }).await {
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
    let storage_stats_interval_secs = env_u64("STORAGE_STATS_INTERVAL_SECS", 30);
    let storage_for_stats = Arc::clone(&storage);
    tokio::spawn(async move {
        stats_logger_worker(storage_for_stats, storage_stats_interval_secs).await;
    });

    let memory_monitor_interval_secs = env_u64("MEMORY_MONITOR_INTERVAL_SECS", 30);
    let storage_for_memory = Arc::clone(&storage);
    tokio::spawn(async move {
        memory_monitor_worker(storage_for_memory, memory_monitor_interval_secs).await;
    });

    // ============================================================================
    // PHASE 3: INITIALIZE RISK ENGINE
    // ============================================================================
    
    let risk_config = RiskEngineConfig {
        reference_eth_price_usd: env_f64("REFERENCE_ETH_PRICE_USD", 2000.0),
        default_liquidation_threshold: env_f64("DEFAULT_LIQUIDATION_THRESHOLD", 0.85),
        risk_score_hf_baseline: env_f64("RISK_SCORE_HF_BASELINE", 1.5),
        risk_score_hf_span: env_f64("RISK_SCORE_HF_SPAN", 0.5),
        risk_score_min: env_f64("RISK_SCORE_MIN", 1.0),
        risk_score_max: env_f64("RISK_SCORE_MAX", 10.0),
    };

    let mut engine = RiskEngine::with_config(
        rx,
        Arc::clone(&storage),
        risk_config.clone(),
    );

    let aave_pool_address = env_string(
        "AAVE_POOL_ADDRESS",
        "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2",
    )
    .parse()
    .expect("Invalid Aave pool address");

    let aave_oracle_address = env_string(
        "AAVE_ORACLE_ADDRESS",
        "0x54586bE62E3c3580375aE3723C145253060Ca0C2",
    )
    .parse()
    .expect("Invalid Aave oracle address");

    if let Err(e) = bootstrap_onchain_state(
        &mut engine,
        Arc::clone(&storage),
        provider.provider(),
        provider.chain_id(),
        aave_pool_address,
        aave_oracle_address,
        &risk_config,
    )
    .await
    {
        tracing::warn!("On-chain bootstrap failed: {:?}", e);
    }

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
    let provider_for_events = Arc::clone(&provider);
    let tx_for_events = tx.clone();
    let aave_ws_url = std::env::var("AAVE_WS_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let aave_ws_reconnect_delay_secs = env_u64("AAVE_WS_RECONNECT_DELAY_SECS", 3);
    tokio::spawn(async move {
        if let Some(ws_url) = aave_ws_url {
            tracing::info!("Aave event watcher started in WS mode");

            loop {
                match provider_for_events
                    .watch_aave_events_ws(&ws_url, aave_pool_address, tx_for_events.clone())
                    .await
                {
                    Ok(_) => tracing::warn!("Aave WS watcher exited unexpectedly"),
                    Err(e) => tracing::error!("Aave WS watcher error: {:?}", e),
                }

                tracing::warn!(
                    "Reconnecting Aave WS in {}s...",
                    aave_ws_reconnect_delay_secs
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(
                    aave_ws_reconnect_delay_secs,
                ))
                .await;
            }
        } else if let Err(e) = provider_for_events
            .watch_aave_events(aave_pool_address, tx_for_events)
            .await
        {
            tracing::error!("Aave polling event watcher error: {:?}", e);
        }
    });

    // ============================================================================
    // PHASE 6: ORACLE PRICE FEEDS
    // ============================================================================
    
    // Khởi tạo Oracle module — theo dõi giá realtime từ Chainlink
    let mut oracle_config = OracleConfig::local_fork(); // Dùng local_fork() cho Anvil
    oracle_config.apply_env_overrides();
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
                stats_interval_secs: env_u64("ORACLE_STATS_INTERVAL_SECS", 60),
                health_check_interval_secs: env_u64("ORACLE_HEALTH_INTERVAL_SECS", 300),
                chainlink_event_poll_interval_secs: env_u64("ORACLE_EVENT_POLL_INTERVAL_SECS", 3),
                chainlink_ws_url: std::env::var("ORACLE_WS_URL")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                chainlink_ws_reconnect_delay_secs: env_u64("ORACLE_WS_RECONNECT_DELAY_SECS", 3),
            };
            
            if worker_config.chainlink_ws_url.is_none() {
                let price_worker_config = worker_config.clone();
                tokio::spawn(async move {
                    oracle_price_worker(oracle_for_price, price_worker_config).await;
                });
            } else {
                tracing::info!(
                    "Oracle WS-first mode enabled: periodic latestRoundData polling disabled"
                );
            }
            
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

            // 6.4 Oracle Chainlink Event Worker — WS primary, polling fallback
            let oracle_for_events = Arc::clone(&oracle);
            let event_worker_config = worker_config.clone();
            tokio::spawn(async move {
                oracle_chainlink_event_worker(oracle_for_events, event_worker_config).await;
            });
            
            tracing::info!("✓ Oracle workers spawned ({} feeds)", oracle.feed_count());
            
            // ── Khởi tạo Profit Calculator (sử dụng oracle price cache) ──
            let mut profit_config = ProfitConfig::local_fork(); // Dùng local_fork() cho Anvil
            profit_config.min_profit_usd = env_f64("PROFIT_MIN_USD", profit_config.min_profit_usd);
            profit_config.min_roi_pct = env_f64("PROFIT_MIN_ROI_PCT", profit_config.min_roi_pct);
            // Direct/Skip-only strategy: keep flash-loan fee disabled to avoid misleading estimates.
            profit_config.include_flash_loan_fee = false;
            profit_config.fallback_gas_price_gwei = env_f64(
                "PROFIT_FALLBACK_GAS_PRICE_GWEI",
                profit_config.fallback_gas_price_gwei,
            );
            profit_config.fallback_eth_price_usd = env_f64(
                "PROFIT_FALLBACK_ETH_PRICE_USD",
                profit_config.fallback_eth_price_usd,
            );
            profit_config.verbose = env_bool("PROFIT_VERBOSE", profit_config.verbose);
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
            
            // ── Khởi tạo Strategy Decider (quyết định direct/skip + ưu tiên targets) ──
            let strategy_config = StrategyConfig::local_fork(); // Dùng local_fork() cho Anvil
            let strategy_decider = Arc::new(StrategyDecider::new(strategy_config.clone()));
            
            tracing::info!("✓ Strategy Decider initialized (max_concurrent={})",
                strategy_config.max_concurrent_liquidations,
            );

            // ── Khởi tạo LiquidationExecutor và spawn executor workers ──
            // Đọc private key từ env-var; fallback sang Anvil account #0 cho local testing
            let private_key = std::env::var("PRIVATE_KEY").unwrap_or_else(|_| {
                tracing::warn!(
                    "PRIVATE_KEY not set — using Anvil default account (local testing only)"
                );
                "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string()
            });

            let mut executor_config = ExecutorConfig::testnet(aave_pool_address);
            executor_config.simulate_before_execute = env_bool("EXECUTOR_SIMULATE_BEFORE_EXECUTE", executor_config.simulate_before_execute);
            executor_config.dry_run = env_bool("EXECUTOR_DRY_RUN", executor_config.dry_run);

            match LiquidationExecutor::new(
                executor_config,
                provider.provider(),
                &private_key,
            ).await {
                Ok(executor) => {
                    let executor = Arc::new(executor);

                    // Main liquidation loop: polls hot cache every 500ms by default
                    let executor_for_worker = Arc::clone(&executor);
                    let storage_for_worker = Arc::clone(&storage);
                    let profit_for_worker  = Arc::clone(&profit_calculator);
                    let strategy_for_worker = Arc::clone(&strategy_decider);
                    let worker_config = WorkerConfig {
                        check_interval_ms: env_u64("EXECUTOR_CHECK_INTERVAL_MS", 500),
                        batch_size: env_usize("EXECUTOR_BATCH_SIZE", 10),
                        liquidation_threshold: env_f64("EXECUTOR_LIQUIDATION_THRESHOLD", 1.0),
                        parallel_execution: env_bool("EXECUTOR_PARALLEL_EXECUTION", false),
                        max_concurrent: env_usize("EXECUTOR_MAX_CONCURRENT", 3),
                    };
                    tokio::spawn(async move {
                        executor_worker(
                            executor_for_worker,
                            storage_for_worker,
                            worker_config,
                            Some(profit_for_worker),
                            Some(strategy_for_worker),
                        ).await;
                    });

                    // Stats logging worker: prints metrics every 60s
                    let executor_stats_interval_secs = env_u64("EXECUTOR_STATS_INTERVAL_SECS", 60);
                    let executor_for_stats = Arc::clone(&executor);
                    tokio::spawn(async move {
                        stats_worker(executor_for_stats, executor_stats_interval_secs).await;
                    });

                    // Nonce sync worker: re-syncs on-chain nonce every 30s
                    let nonce_sync_interval_secs = env_u64("EXECUTOR_NONCE_SYNC_INTERVAL_SECS", 30);
                    let executor_for_nonce = Arc::clone(&executor);
                    tokio::spawn(async move {
                        nonce_sync_worker(executor_for_nonce, nonce_sync_interval_secs).await;
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

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
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
