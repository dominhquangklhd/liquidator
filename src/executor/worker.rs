// Executor Worker
//
// Background task that continuously checks for liquidation opportunities
// and executes them via the LiquidationExecutor

use super::executor::{LiquidationExecutor, LiquidationResult};
use crate::storage::{HybridStorage, LiquidationTarget, LiquidationEvent};
use crate::profit::ProfitCalculator;
use crate::strategy::{ExecutionMethod, StrategyDecider};
use ethers::types::{Address, U256};

use std::sync::Arc;
use std::time::Instant;
use tokio::time::{interval, Duration};

#[derive(Debug, Clone)]
struct PlannedExecution {
    target: LiquidationTarget,
    method: Option<ExecutionMethod>,
}

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
            check_interval_ms: 500,        // Check every 500ms
            batch_size: 10,                 // Process up to 10 targets
            liquidation_threshold: 1.0,     // Only HF < 1.0
            parallel_execution: false,      // Sequential by default
            max_concurrent: 3,              // Max 3 concurrent
        }
    }
}

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
}

fn resolve_token_address(asset: &str, chain_id: u64) -> Option<Address> {
    if let Ok(addr) = asset.parse::<Address>() {
        return Some(addr);
    }

    let symbol = asset.trim().to_ascii_uppercase();

    let env_key = format!("RESERVE_{}", symbol);
    if let Ok(v) = std::env::var(&env_key) {
        if let Ok(addr) = v.trim().parse::<Address>() {
            return Some(addr);
        }
    }

    let default = match chain_id {
        11155111 => match symbol.as_str() {
            "WETH" => "0xC558DBdd856501FCd9aaF1E62eae57A9F0629a3c",
            "USDC" => "0x94a9D9AC8a22534E3FaCa9F4e7F2E2cf85d5E4C8",
            "WBTC" => "0x29f2D40B0605204364af54EC677bD022dA425d03",
            "DAI" => "0x68194a729C2450ad26072b3D33ADaCbcef39D574",
            "USDT" => "0xC2C527C0CACF457746Bd31B2a698Fe89de2b6d49",
            "LINK" => "0xf97f4df75117a78c1A5a0DBb814Af92458539FB4",
            "AAVE" => "0x6Ae43d3271ff6888e7Fc43Fd7321a503ff738951",
            _ => return None,
        },
        _ => match symbol.as_str() {
            "WETH" => "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2",
            "USDC" => "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
            "WBTC" => "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599",
            "DAI" => "0x6B175474E89094C44Da98b954EedeAC495271d0F",
            "USDT" => "0xdAC17F958D2ee523a2206206994597C13D831ec7",
            "LINK" => "0x514910771AF9Ca656af840dff83E8264EcF986CA",
            "AAVE" => "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DdAE9",
            _ => return None,
        },
    };

    default.parse::<Address>().ok()
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
    profit_calc: Option<Arc<ProfitCalculator>>,
    strategy_decider: Option<Arc<StrategyDecider>>,
) {
    let mut ticker = interval(Duration::from_millis(config.check_interval_ms));
    let mut last_skip_reason_log = Instant::now() - Duration::from_secs(10);
    
    tracing::info!(
        "Executor worker started (interval: {}ms, batch: {}, threshold: {}, strategy: {})",
        config.check_interval_ms,
        config.batch_size,
        config.liquidation_threshold,
        if strategy_decider.is_some() { "enabled" } else { "disabled" }
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
        
        // Keep strategy context in sync with current wallet balance.
        if let Some(ref decider) = strategy_decider {
            match executor.wallet_balance().await {
                Ok(balance) => {
                    decider.update_wallet_balance(u256_to_f64(balance) / 1e18).await;
                }
                Err(e) => {
                    tracing::debug!("Failed to refresh strategy wallet balance: {:?}", e);
                }
            }
        }

        // Profit evaluation + optional strategy planning.
        let executable_targets = if let Some(ref calc) = profit_calc {
            let estimates = match calc.evaluate_batch(&liquidatable).await {
                Ok(estimates) => estimates,
                Err(e) => {
                    tracing::warn!("Profit evaluation failed: {:?}, proceeding without filter", e);
                    Vec::new()
                }
            };

            if estimates.is_empty() {
                liquidatable
                    .into_iter()
                    .map(|target| PlannedExecution {
                        target,
                        method: None,
                    })
                    .collect::<Vec<_>>()
            } else if let Some(ref decider) = strategy_decider {
                let chain_id = executor.chain_id().await.unwrap_or(1);
                let estimates_by_user: std::collections::HashMap<_, _> = estimates
                    .iter()
                    .map(|e| (e.user_address.clone(), e.clone()))
                    .collect();

                // Refresh strategy debt-token balances from on-chain wallet state.
                // Strategy compares debt_to_cover_usd vs wallet token balance (USD equivalent).
                let mut debt_assets: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
                for e in &estimates {
                    debt_assets
                        .entry(e.pair.debt_asset.clone())
                        .or_insert(e.pair.debt_price_usd.max(0.0));
                }

                for (asset, debt_price_usd) in debt_assets {
                    if let Some(token_addr) = resolve_token_address(&asset, chain_id) {
                        match executor.wallet_token_balance(token_addr).await {
                            Ok((balance_tokens, _decimals)) => {
                                let balance_usd = balance_tokens * debt_price_usd;
                                decider.update_token_balance(asset.clone(), balance_usd).await;
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "Failed to refresh token balance for {} ({:?}): {:?}",
                                    asset,
                                    token_addr,
                                    e
                                );
                            }
                        }
                    }
                }

                let pairs: Vec<_> = liquidatable
                    .iter()
                    .cloned()
                    .filter_map(|target| {
                        estimates_by_user
                            .get(&target.user_address)
                            .cloned()
                            .map(|estimate| (target, estimate))
                    })
                    .collect();

                match decider.create_plan(pairs).await {
                    Ok(plan) => {
                        if plan.execute_count == 0 {
                            if last_skip_reason_log.elapsed() >= Duration::from_secs(5) {
                                tracing::info!("Strategy plan skipped all targets");
                                for pt in plan.targets.iter().take(3) {
                                    let method_reason = match &pt.decision.method {
                                        ExecutionMethod::Skip { reason } => reason.as_str(),
                                        _ => "n/a",
                                    };
                                    let profit_reason = pt
                                        .estimate
                                        .reject_reason
                                        .as_deref()
                                        .unwrap_or("n/a");
                                    tracing::info!(
                                        "Skip detail user={} method_reason={} profit_reason={} net_profit=${:.4} debt_cover=${:.4}",
                                        pt.target.user_address,
                                        method_reason,
                                        profit_reason,
                                        pt.estimate.net_profit_usd,
                                        pt.estimate.debt_to_cover_usd,
                                    );
                                }
                                last_skip_reason_log = Instant::now();
                            }
                            continue;
                        }

                        let mut planned = Vec::with_capacity(plan.execute_count);
                        for pt in plan.executable_targets() {
                            let mut target = pt.target.clone();
                            target.estimated_profit = pt.decision.adjusted_profit_usd;
                            tracing::info!(
                                "Strategy selected {} for {} (rank #{}, est=${:.2})",
                                pt.decision.method.label(),
                                target.user_address,
                                pt.rank,
                                target.estimated_profit
                            );
                            planned.push(PlannedExecution {
                                target,
                                method: Some(pt.decision.method.clone()),
                            });
                        }
                        planned
                    }
                    Err(e) => {
                        tracing::warn!("Strategy planning failed: {:?}, fallback to profitable filter", e);
                        let profitable_by_user: std::collections::HashMap<_, _> = estimates
                            .iter()
                            .filter(|e| e.is_profitable)
                            .map(|e| (e.user_address.clone(), e.net_profit_usd))
                            .collect();

                        liquidatable
                            .into_iter()
                            .filter_map(|mut t| {
                                profitable_by_user.get(&t.user_address).map(|profit| {
                                    t.estimated_profit = *profit;
                                    PlannedExecution {
                                        target: t,
                                        method: None,
                                    }
                                })
                            })
                            .collect::<Vec<_>>()
                    }
                }
            } else {
                let profitable_by_user: std::collections::HashMap<_, _> = estimates
                    .iter()
                    .filter(|e| e.is_profitable)
                    .map(|e| (e.user_address.clone(), e.net_profit_usd))
                    .collect();

                let filtered = liquidatable
                    .into_iter()
                    .filter_map(|mut t| {
                        profitable_by_user.get(&t.user_address).map(|profit| {
                            t.estimated_profit = *profit;
                            PlannedExecution {
                                target: t,
                                method: None,
                            }
                        })
                    })
                    .collect::<Vec<_>>();

                tracing::info!(
                    "Profit filter: {}/{} targets profitable",
                    filtered.len(),
                    profitable_by_user.len()
                );
                filtered
            }
        } else {
            liquidatable
                .into_iter()
                .map(|target| PlannedExecution {
                    target,
                    method: None,
                })
                .collect::<Vec<_>>()
        };
        
        if executable_targets.is_empty() {
            continue;
        }
        
        if config.parallel_execution {
            // Parallel execution
            execute_parallel(
                &executor,
                &storage,
                executable_targets,
                config.max_concurrent,
                strategy_decider.clone(),
            ).await;
        } else {
            // Sequential execution
            for planned in executable_targets {
                let result = execute_target(&executor, &planned).await;
                
                match result {
                    Ok(res) => {
                        handle_result(&storage, &planned.target, res, strategy_decider.as_ref()).await;
                    }
                    Err(e) => {
                        tracing::error!("Liquidation error for {}: {:?}", planned.target.user_address, e);
                        if let Some(ref decider) = strategy_decider {
                            decider.report_failure().await;
                        }
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
    targets: Vec<PlannedExecution>,
    max_concurrent: usize,
    strategy_decider: Option<Arc<StrategyDecider>>,
) {
    use futures::stream::{self, StreamExt};
    
    stream::iter(targets)
        .for_each_concurrent(max_concurrent, |planned| {
            let executor = Arc::clone(executor);
            let storage = Arc::clone(storage);
            let strategy_decider = strategy_decider.clone();
            
            async move {
                let result = execute_target(&executor, &planned).await;
                
                match result {
                    Ok(res) => {
                        handle_result(&storage, &planned.target, res, strategy_decider.as_ref()).await;
                    }
                    Err(e) => {
                        tracing::error!("Liquidation error for {}: {:?}", planned.target.user_address, e);
                        if let Some(ref decider) = strategy_decider {
                            decider.report_failure().await;
                        }
                    }
                }
            }
        })
        .await;
}

async fn execute_target(
    executor: &Arc<LiquidationExecutor>,
    planned: &PlannedExecution,
) -> anyhow::Result<LiquidationResult> {
    match planned.method.as_ref() {
        Some(method) => executor.liquidate_with_method(&planned.target, method).await,
        None => executor.liquidate(&planned.target).await,
    }
}

fn should_trip_circuit_breaker(error: &str) -> bool {
    let err = error.to_ascii_lowercase();

    // Soft failures happen before transaction execution and should not
    // trip circuit breaker aggressively.
    let soft_prefixes = [
        "preflight failed:",
        "approval failed:",
        "simulation failed:",
        "already pending",
        "too many pending transactions",
        "strategy requested skip:",
    ];

    !soft_prefixes.iter().any(|p| err.starts_with(p))
}

fn should_drop_target_after_failure(error: &str) -> bool {
    let err = error.to_ascii_lowercase();
    err.contains("preflight failed:") && err.contains("health factor") && err.contains(">= 1.0")
}

/// Handle liquidation result
async fn handle_result(
    storage: &Arc<HybridStorage>,
    target: &LiquidationTarget,
    result: LiquidationResult,
    strategy_decider: Option<&Arc<StrategyDecider>>,
) {
    if result.success {
        if let Some(decider) = strategy_decider {
            decider.report_success().await;
        }

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
        let error_msg = result.error.unwrap_or_else(|| "unknown error".to_string());

        if should_drop_target_after_failure(&error_msg) {
            // On-chain state says user is no longer liquidatable. Remove stale
            // cache entry and wait for next risk update to re-add if needed.
            storage.remove_target(&target.user_address).await;
        }

        if let Some(decider) = strategy_decider {
            if should_trip_circuit_breaker(&error_msg) {
                decider.report_failure().await;
            } else {
                tracing::info!(
                    "Skipping circuit-breaker increment for pre-execution failure user={}: {}",
                    target.user_address,
                    error_msg
                );
            }
        }

        tracing::warn!(
            "❌ Liquidation failed for {}: {}",
            target.user_address,
            error_msg
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
                let eth_balance = u256_to_f64(balance) / 1e18;
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
