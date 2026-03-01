// Executor Module
//
// Handles liquidation transaction execution including:
// - Transaction building and signing
// - Nonce management for parallel TXs
// - Simulation before execution
// - Result tracking and statistics

pub mod config;
pub mod nonce;
pub mod executor;
pub mod worker;

// Re-exports
pub use config::ExecutorConfig;
pub use nonce::NonceManager;
pub use executor::{LiquidationExecutor, LiquidationResult, ExecutorStats};
pub use worker::{executor_worker, stats_worker, nonce_sync_worker, WorkerConfig};
