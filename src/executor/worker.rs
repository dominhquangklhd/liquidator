// Executor Worker
//
// Background task that continuously checks for liquidation opportunities
// and executes them via the LiquidationExecutor

use super::executor::{LiquidationExecutor, LiquidationResult};
use crate::storage::{HybridStorage, LiquidationTarget, LiquidationEvent};

use std::sync::Arc;
use tokio::time::{interval, Duration};
use anyhow::Result;

/// Executor worker configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Check interval in milliseconds
    pub check_interval_ms: u64,
    
    /// Maximum targets to process per iteration
    pub batch_size: usize,
    
    /// Minimum health factor to consider (targets below this)
    pub liquidation_threshold: f64,
    
    /// Enable parallel liquidation execution
    pub parallel_execution: bool,
    
    /// Maximum concurrent liquidations
    pub max_concurrent: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: 100,        // Check every 100ms
            batch_size: 10,                 // Process up to 10 targets
            liquidation_threshold: 1.0,     // Only HF < 1.0
            parallel_execution: false,      // Sequential by default
            max_concurrent: 3,              // Max 3 concurrent
        }
    }
}

/// Run the executor worker as a background task
/// 
/// This is the main liquidation loop that:
/// 1. Queries HotCache for liquidatable targets
/// 2. Validates targets on-chain
/// 3. Executes liquidations
/// 4. Records results to storage
pub async fn executor_worker(
    executor: Arc<LiquidationExecutor>,
    storage: Arc<HybridStorage>,
    config: WorkerConfig,
) {
    let mut ticker = interval(Duration::from_millis(config.check_interval_ms));
    
    tracing::info!(
        "Executor worker started (interval: {}ms, batch: {}, threshold: {})",
        config.check_interval_ms,
        config.batch_size,
        config.liquidation_threshold
    );
    
    loop {
        ticker.tick().await;
        
        // Get top liquidation targets
        let targets = storage.get_top_targets(config.batch_size).await;
        
        if targets.is_empty() {
            continue;
        }
        
        // Filter liquidatable targets (HF < threshold)
        let liquidatable: Vec<_> = targets
            .into_iter()
            .filter(|t| t.health_factor < config.liquidation_threshold)
            .collect();
        
        if liquidatable.is_empty() {
            continue;
        }
        
        tracing::info!("Found {} liquidatable targets", liquidatable.len());
        
        if config.parallel_execution {
            // Parallel execution
            execute_parallel(
                &executor,
                &storage,
                liquidatable,
                config.max_concurrent,
            ).await;
        } else {
            // Sequential execution
            for target in liquidatable {
                let result = executor.liquidate(&target).await;
                
                match result {
                    Ok(res) => {
                        handle_result(&storage, &target, res).await;
                    }
                    Err(e) => {
                        tracing::error!("Liquidation error for {}: {:?}", target.user_address, e);
                    }
                }
            }
        }
    }
}

/// Execute liquidations in parallel (up to max_concurrent)
async fn execute_parallel(
    executor: &Arc<LiquidationExecutor>,
    storage: &Arc<HybridStorage>,
    targets: Vec<LiquidationTarget>,
    max_concurrent: usize,
) {
    use futures::stream::{self, StreamExt};
    
    stream::iter(targets)
        .for_each_concurrent(max_concurrent, |target| {
            let executor = Arc::clone(executor);
            let storage = Arc::clone(storage);
            
            async move {
                let result = executor.liquidate(&target).await;
                
                match result {
                    Ok(res) => {
                        handle_result(&storage, &target, res).await;
                    }
                    Err(e) => {
                        tracing::error!("Liquidation error for {}: {:?}", target.user_address, e);
                    }
                }
            }
        })
        .await;
}

/// Handle liquidation result
async fn handle_result(
    storage: &Arc<HybridStorage>,
    target: &LiquidationTarget,
    result: LiquidationResult,
) {
    if result.success {
        // Remove from hot cache (no longer liquidatable)
        storage.remove_target(&target.user_address).await;
        
        // Record liquidation event
        let event = LiquidationEvent {
            id: None,
            user_address: target.user_address.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            collateral_asset: target.collateral.keys().next().cloned().unwrap_or_default(),
            debt_asset: target.debt.keys().next().cloned().unwrap_or_default(),
            collateral_seized: result.collateral_seized,
            debt_covered: result.debt_covered,
            liquidator: "self".to_string(), // TODO: Get from executor
            tx_hash: result.tx_hash.unwrap_or_default(),
            profit_usd: result.profit_usd,
            gas_cost_usd: (result.gas_used * result.gas_price) as f64 / 1e18 * 2000.0, // Rough ETH price
        };
        
        if let Err(e) = storage.record_liquidation(event).await {
            tracing::error!("Failed to record liquidation: {:?}", e);
        }
        
        tracing::info!(
            "✅ Liquidated {}: profit ${:.2}, gas {}",
            target.user_address,
            result.profit_usd,
            result.gas_used
        );
    } else {
        tracing::debug!(
            "❌ Liquidation failed for {}: {}",
            target.user_address,
            result.error.unwrap_or_default()
        );
    }
}

/// Stats logging worker
pub async fn stats_worker(executor: Arc<LiquidationExecutor>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    
    loop {
        ticker.tick().await;
        
        let stats = executor.stats().await;
        let pending = executor.pending_count().await;
        
        tracing::info!(
            "Executor Stats: attempts={} success={} failed={} reverted={} profit=${:.2} pending={}",
            stats.total_attempts,
            stats.successful,
            stats.failed,
            stats.reverted,
            stats.total_profit_usd,
            pending
        );
        
        // Check wallet balance
        match executor.wallet_balance().await {
            Ok(balance) => {
                let eth_balance = balance.as_u128() as f64 / 1e18;
                tracing::info!("Wallet balance: {:.4} ETH", eth_balance);
                
                if eth_balance < 0.1 {
                    tracing::warn!("⚠️ Low wallet balance! Consider adding funds.");
                }
            }
            Err(e) => {
                tracing::error!("Failed to check balance: {:?}", e);
            }
        }
    }
}

/// Nonce sync worker - periodically sync nonce with on-chain
pub async fn nonce_sync_worker(executor: Arc<LiquidationExecutor>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    
    loop {
        ticker.tick().await;
        
        if let Err(e) = executor.sync_nonce().await {
            tracing::error!("Nonce sync failed: {:?}", e);
        }
    }
}
