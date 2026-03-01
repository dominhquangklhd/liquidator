// Executor Configuration
//
// Configurable parameters for the liquidation executor

use ethers::types::H160;

/// Configuration for the liquidation executor
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Minimum profit in USD to execute liquidation
    pub min_profit_usd: f64,
    
    /// Maximum gas price willing to pay (in Gwei)
    pub max_gas_price_gwei: f64,
    
    /// Gas limit for liquidation transaction
    pub gas_limit: u64,
    
    /// Priority fee (tip) in Gwei for EIP-1559
    pub priority_fee_gwei: f64,
    
    /// Aave Pool contract address
    pub aave_pool_address: H160,
    
    /// Our liquidator contract address (if using flash loans)
    pub liquidator_contract: Option<H160>,
    
    /// Whether to use flash loans for liquidation
    pub use_flash_loan: bool,
    
    /// Maximum concurrent pending transactions
    pub max_pending_txs: usize,
    
    /// Transaction confirmation timeout (seconds)
    pub tx_timeout_secs: u64,
    
    /// Retry attempts for failed transactions
    pub max_retries: u32,
    
    /// Delay between retries (milliseconds)
    pub retry_delay_ms: u64,
    
    /// Simulation before execution
    pub simulate_before_execute: bool,
    
    /// Dry run mode (log but don't send transactions)
    pub dry_run: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            min_profit_usd: 10.0,           // Minimum $10 profit
            max_gas_price_gwei: 100.0,      // Max 100 Gwei
            gas_limit: 500_000,             // 500k gas limit
            priority_fee_gwei: 2.0,         // 2 Gwei tip
            aave_pool_address: H160::zero(),
            liquidator_contract: None,
            use_flash_loan: false,
            max_pending_txs: 5,
            tx_timeout_secs: 60,
            max_retries: 3,
            retry_delay_ms: 1000,
            simulate_before_execute: true,
            dry_run: false,
        }
    }
}

impl ExecutorConfig {
    /// Create config for mainnet
    pub fn mainnet(aave_pool: H160) -> Self {
        Self {
            aave_pool_address: aave_pool,
            min_profit_usd: 50.0,    // Higher threshold for mainnet
            max_gas_price_gwei: 50.0,
            simulate_before_execute: true,
            dry_run: false,
            ..Default::default()
        }
    }
    
    /// Create config for testnet/local
    pub fn testnet(aave_pool: H160) -> Self {
        Self {
            aave_pool_address: aave_pool,
            min_profit_usd: 1.0,     // Lower threshold for testing
            max_gas_price_gwei: 200.0,
            simulate_before_execute: true,
            dry_run: false,
            ..Default::default()
        }
    }
    
    /// Create dry-run config (no real transactions)
    pub fn dry_run(aave_pool: H160) -> Self {
        Self {
            aave_pool_address: aave_pool,
            dry_run: true,
            ..Default::default()
        }
    }
}
