// Library exports for liquidator modules
//
// This allows examples and tests to use internal modules

pub mod aave_v3;
pub mod storage;
pub mod data;
pub mod events;
pub mod risk;
pub mod executor;
pub mod provider;
pub mod oracle;
pub mod profit;
pub mod strategy;
pub mod bootstrap;

// Re-export commonly used types
pub use storage::{HybridStorage, StorageConfig, LiquidationTarget, LiquidationEvent};
pub use executor::{
    LiquidationExecutor, ExecutorConfig, ExecutorStats, 
    LiquidationResult, WorkerConfig,
    executor_worker, stats_worker, nonce_sync_worker,
};
pub use oracle::{
    OracleManager, OracleConfig, PriceFeedConfig,
    OracleWorkerConfig, OracleStats, PriceData,
    oracle_price_worker, oracle_stats_worker, oracle_health_worker,
};
pub use profit::{
    ProfitCalculator, ProfitConfig, ProfitEstimate, ProfitBreakdown,
    LiquidationPair, GasCostEstimate, GasEstimator, ProfitStats,
};
pub use strategy::{
    StrategyDecider, StrategyConfig, StrategyStats,
    ExecutionMethod, StrategyDecision, PrioritizedTarget, ExecutionPlan,
};
