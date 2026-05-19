// Liquidation Executor
//
// Core execution engine for liquidation transactions
// Supports direct liquidation path

use super::config::ExecutorConfig;
use super::nonce::NonceManager;
use crate::strategy::ExecutionMethod;
use crate::storage::LiquidationTarget;

use ethers::prelude::*;
use ethers::providers::{Provider, Http, Middleware};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, U256};
use ethers::contract::abigen;

use anyhow::{Result, Context, bail};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

fn resolve_token_address(asset: &str, _chain_id: u64) -> Option<Address> {
    if let Ok(addr) = asset.parse::<Address>() {
        return Some(addr);
    }

    let normalized = asset.trim().to_ascii_uppercase();
    let symbol = match normalized.as_str() {
        "ETH" => "WETH",
        other => other,
    };

    let env_key = format!("RESERVE_{}", symbol);
    if let Ok(v) = std::env::var(&env_key) {
        if let Ok(addr) = v.trim().parse::<Address>() {
            return Some(addr);
        }
    }

    let default = match symbol {
        "WETH" => "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2",
        "WSTETH" => "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0",
        "USDC" => "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
        "WBTC" => "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599",
        "DAI" => "0x6B175474E89094C44Da98b954EedeAC495271d0F",
        "USDT" => "0xdAC17F958D2ee523a2206206994597C13D831ec7",
        "LINK" => "0x514910771AF9Ca656af840dff83E8264EcF986CA",
        "AAVE" => "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DdAE9",
        _ => return None,
    };

    default.parse::<Address>().ok()
}

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(f64::INFINITY)
}

// Generate Aave Pool contract bindings
abigen!(
    AavePool,
    r#"[
        function liquidationCall(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken) external
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
    ]"#
);

// Generate ERC20 contract bindings (for approve/allowance)
abigen!(
    ERC20Approve,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function allowance(address owner, address spender) external view returns (uint256)
        function balanceOf(address owner) external view returns (uint256)
        function decimals() external view returns (uint8)
    ]"#
);

/// Signer type alias
type SignedClient = SignerMiddleware<Provider<Http>, LocalWallet>;

/// Result of a liquidation attempt
#[derive(Debug, Clone)]
pub struct LiquidationResult {
    pub success: bool,
    pub submitted_onchain: bool,
    pub tx_hash: Option<String>,
    pub gas_used: u64,
    pub gas_price: u64,
    pub collateral_seized: f64,
    pub debt_covered: f64,
    pub profit_usd: f64,
    pub error: Option<String>,
}

impl LiquidationResult {
    pub fn success(tx_hash: String, gas_used: u64, gas_price: u64, collateral: f64, debt: f64, profit: f64) -> Self {
        Self {
            success: true,
            submitted_onchain: true,
            tx_hash: Some(tx_hash),
            gas_used,
            gas_price,
            collateral_seized: collateral,
            debt_covered: debt,
            profit_usd: profit,
            error: None,
        }
    }
    
    pub fn failed(error: String) -> Self {
        Self {
            success: false,
            submitted_onchain: false,
            tx_hash: None,
            gas_used: 0,
            gas_price: 0,
            collateral_seized: 0.0,
            debt_covered: 0.0,
            profit_usd: 0.0,
            error: Some(error),
        }
    }
}

/// Pending liquidation tracking
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PendingLiquidation {
    target: LiquidationTarget,
    nonce: u64,
    tx_hash: String,
    submitted_at: i64,
}

/// Liquidation Executor
/// 
/// Handles the actual execution of liquidation transactions
pub struct LiquidationExecutor {
    /// Configuration
    config: ExecutorConfig,
    
    /// Ethereum provider (read-only)
    provider: Arc<Provider<Http>>,
    
    /// Signing provider (for transactions)
    signer: Arc<SignedClient>,
    
    /// Wallet for signing transactions
    wallet: LocalWallet,
    
    /// Nonce manager
    nonce_manager: Arc<NonceManager>,
    
    /// Aave Pool contract (with signer — sets correct msg.sender)
    aave_pool: AavePool<SignedClient>,
    
    /// Pending liquidations
    pending: Arc<RwLock<HashMap<String, PendingLiquidation>>>,
    
    /// Statistics
    stats: Arc<RwLock<ExecutorStats>>,
}

/// Executor statistics
#[derive(Debug, Default, Clone)]
pub struct ExecutorStats {
    pub total_attempts: u64,
    pub successful: u64,
    pub failed: u64,
    pub reverted: u64,
    pub total_profit_usd: f64,
    pub total_gas_spent: u64,
}

impl LiquidationExecutor {
    /// Create new executor
    pub async fn new(
        config: ExecutorConfig,
        provider: Arc<Provider<Http>>,
        private_key: &str,
    ) -> Result<Self> {
        // Parse wallet from private key
        let wallet: LocalWallet = private_key
            .parse::<LocalWallet>()
            .context("Failed to parse private key")?
            .with_chain_id(provider.get_chainid().await?.as_u64());
        
        let wallet_address = wallet.address();
        tracing::info!("Executor wallet: {:?}", wallet_address);
        
        // Create signing middleware (sets msg.sender for eth_call & signs txs)
        let signer = Arc::new(SignerMiddleware::new(
            (*provider).clone(),
            wallet.clone(),
        ));
        
        // Create nonce manager
        let nonce_manager = Arc::new(
            NonceManager::new(Arc::clone(&provider), wallet_address).await?
        );
        
        // Create Aave Pool contract instance (with signer)
        let aave_pool = AavePool::new(config.aave_pool_address, Arc::clone(&signer));
        
        Ok(Self {
            config,
            provider,
            signer,
            wallet,
            nonce_manager,
            aave_pool,
            pending: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(ExecutorStats::default())),
        })
    }
    
    /// Execute liquidation on a target
    pub async fn liquidate(&self, target: &LiquidationTarget) -> Result<LiquidationResult> {
        let default_method = ExecutionMethod::Direct {
            gas_limit: self.config.gas_limit,
        };

        self.liquidate_with_method(target, &default_method).await
    }

    /// Execute liquidation with explicit strategy execution method.
    pub async fn liquidate_with_method(
        &self,
        target: &LiquidationTarget,
        method: &ExecutionMethod,
    ) -> Result<LiquidationResult> {
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_attempts += 1;
        }
        
        // Pre-flight checks
        if let Err(e) = self.preflight_check(target).await {
            return Ok(LiquidationResult::failed(format!("Preflight failed: {}", e)));
        }
        
        // Check if already pending
        if self.is_pending(&target.user_address).await {
            return Ok(LiquidationResult::failed("Already pending".to_string()));
        }
        
        // Check nonce congestion
        if self.nonce_manager.is_congested(self.config.max_pending_txs).await {
            return Ok(LiquidationResult::failed("Too many pending transactions".to_string()));
        }
        
        // Dry run mode
        if self.config.dry_run {
            tracing::info!(
                "[DRY RUN] Would liquidate {} (HF: {:.4}, method: {})",
                target.user_address,
                target.health_factor,
                method.label()
            );
            return Ok(LiquidationResult::success(
                "0x_dry_run".to_string(), 0, 0, 0.0, 0.0, target.estimated_profit
            ));
        }

        match method {
            ExecutionMethod::Direct { gas_limit } => {
                // Ensure debt token is approved to Aave Pool for direct path.
                if let Err(e) = self.ensure_approval(target).await {
                    return Ok(LiquidationResult::failed(format!("Approval failed: {}", e)));
                }

                if self.config.simulate_before_execute {
                    if let Err(e) = self.simulate_liquidation(target).await {
                        return Ok(LiquidationResult::failed(format!("Simulation failed: {}", e)));
                    }
                }

                self.execute_direct_liquidation(target, Some(*gas_limit)).await
            }
            ExecutionMethod::Skip { reason } => Ok(LiquidationResult::failed(format!(
                "Strategy requested skip: {}",
                reason
            ))),
        }
    }
    
    /// Pre-flight checks before liquidation
    async fn preflight_check(&self, target: &LiquidationTarget) -> Result<()> {
        // Check health factor on-chain
        let user_address: Address = target.user_address
            .parse()
            .context("Invalid user address")?;
        
        let account_data = self.aave_pool
            .get_user_account_data(user_address)
            .call()
            .await
            .context("Failed to get user account data")?;
        
        // Health factor from contract (18 decimals).
        // Avoid as_u128() panic when protocol returns very large HF after liquidation.
        let hf_raw = account_data.5; // healthFactor is 6th return value
        let hf = u256_to_f64(hf_raw) / 1e18;
        
        if hf >= 1.0 {
            bail!("User health factor is {} (>= 1.0), not liquidatable", hf);
        }
        
        // Check profit threshold
        if target.estimated_profit < self.config.min_profit_usd {
            bail!("Estimated profit ${:.2} below minimum ${:.2}", 
                target.estimated_profit, self.config.min_profit_usd);
        }
        
        // Check gas price
        let gas_price = self.provider.get_gas_price().await?;
        let gas_price_gwei = gas_price.as_u64() as f64 / 1e9;
        
        if gas_price_gwei > self.config.max_gas_price_gwei {
            bail!("Gas price {:.2} Gwei exceeds max {:.2} Gwei",
                gas_price_gwei, self.config.max_gas_price_gwei);
        }
        
        Ok(())
    }
    
    /// Simulate direct liquidation using eth_call
    async fn simulate_liquidation(&self, target: &LiquidationTarget) -> Result<()> {
        let user_address: Address = target.user_address.parse()?;
        let chain_id = self.chain_id().await.unwrap_or(1);
        
        // Get largest debt position
        let (debt_asset, _debt_amount) = target.debt
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .context("No debt positions")?;
        
        // Get largest collateral position  
        let (collateral_asset, _) = target.collateral
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .context("No collateral positions")?;
        
        let debt_asset_address = resolve_token_address(debt_asset, chain_id)
            .with_context(|| format!("Invalid debt asset address: {}", debt_asset))?;
        let collateral_asset_address = resolve_token_address(collateral_asset, chain_id)
            .with_context(|| format!("Invalid collateral asset address: {}", collateral_asset))?;
        
        // Use U256::MAX — Aave automatically caps at 50% close factor
        let debt_to_cover = U256::MAX;
        
        // Simulate the call (signer sets correct msg.sender)
        let result = self.aave_pool
            .liquidation_call(
                collateral_asset_address,
                debt_asset_address,
                user_address,
                debt_to_cover,
                true, // receiveAToken
            )
            .call()
            .await;
        
        match result {
            Ok(_) => {
                tracing::debug!("Simulation successful for {}", target.user_address);
                Ok(())
            }
            Err(e) => {
                bail!("Simulation reverted: {:?}", e)
            }
        }
    }

    /// Execute direct liquidation transaction via Aave Pool liquidationCall.
    async fn execute_direct_liquidation(
        &self,
        target: &LiquidationTarget,
        gas_limit_override: Option<u64>,
    ) -> Result<LiquidationResult> {
        let user_address: Address = target.user_address.parse()?;
        let chain_id = self.chain_id().await.unwrap_or(1);
        
        // Get positions to liquidate
        let (debt_asset, debt_amount) = target.debt
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .context("No debt positions")?;
        
        let (collateral_asset, _) = target.collateral
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .context("No collateral positions")?;
        
        let debt_asset_address = resolve_token_address(debt_asset, chain_id)
            .with_context(|| format!("Invalid debt asset address: {}", debt_asset))?;
        let collateral_asset_address = resolve_token_address(collateral_asset, chain_id)
            .with_context(|| format!("Invalid collateral asset address: {}", collateral_asset))?;
        
        // Use U256::MAX — Aave automatically caps at 50% close factor
        let debt_to_cover_f64 = *debt_amount * 0.5;
        let debt_to_cover = U256::MAX;
        
        // Get nonce
        let nonce = self.nonce_manager.get_next().await;
        
        // Build transaction
        let call = self.aave_pool.liquidation_call(
            collateral_asset_address,
            debt_asset_address,
            user_address,
            debt_to_cover,
            true, // receiveAToken
        );
        
        // Get gas price
        let gas_price = self.provider.get_gas_price().await?;
        
        // Estimate gas
        let gas_estimate = call
            .estimate_gas()
            .await
            .unwrap_or(U256::from(gas_limit_override.unwrap_or(self.config.gas_limit)));
        let gas_limit = gas_estimate * 120 / 100; // 20% buffer
        
        tracing::info!(
            "Executing DIRECT liquidation: user={}, debt={:.4} {}, nonce={}",
            target.user_address,
            debt_to_cover_f64,
            debt_asset,
            nonce
        );
        
        // Send transaction
        let tx = call
            .gas(gas_limit)
            .gas_price(gas_price)
            .nonce(nonce);
        
        // SignedTransaction
        let pending_tx = match tx.send().await {
            Ok(pending) => pending,
            Err(e) => {
                self.nonce_manager.fail(nonce).await;
                let mut stats = self.stats.write().await;
                stats.failed += 1;
                return Ok(LiquidationResult::failed(format!("Failed to send: {:?}", e)));
            }
        };
        
        let tx_hash = format!("{:?}", pending_tx.tx_hash());
        tracing::info!("Transaction sent: {}", tx_hash);
        
        // Track pending
        {
            let mut pending = self.pending.write().await;
            pending.insert(target.user_address.clone(), PendingLiquidation {
                target: target.clone(),
                nonce,
                tx_hash: tx_hash.clone(),
                submitted_at: chrono::Utc::now().timestamp(),
            });
        }
        
        // Wait for confirmation with timeout
        let receipt = match tokio::time::timeout(
            tokio::time::Duration::from_secs(self.config.tx_timeout_secs),
            pending_tx
        ).await {
            Ok(Ok(Some(receipt))) => receipt,
            Ok(Ok(None)) => {
                self.nonce_manager.fail(nonce).await;
                self.remove_pending(&target.user_address).await;
                let mut stats = self.stats.write().await;
                stats.failed += 1;
                let mut result = LiquidationResult::failed("Receipt not found".to_string());
                result.submitted_onchain = true;
                result.tx_hash = Some(tx_hash.clone());
                return Ok(result);
            }
            Ok(Err(e)) => {
                self.nonce_manager.fail(nonce).await;
                self.remove_pending(&target.user_address).await;
                let mut stats = self.stats.write().await;
                stats.failed += 1;
                let mut result = LiquidationResult::failed(format!("Transaction error: {:?}", e));
                result.submitted_onchain = true;
                result.tx_hash = Some(tx_hash.clone());
                return Ok(result);
            }
            Err(_) => {
                // Timeout - transaction might still be pending
                tracing::warn!("Transaction timeout: {}", tx_hash);
                let mut result = LiquidationResult::failed("Transaction timeout".to_string());
                result.submitted_onchain = true;
                result.tx_hash = Some(tx_hash.clone());
                return Ok(result);
            }
        };
        
        // Remove from pending
        self.remove_pending(&target.user_address).await;
        
        // Check if succeeded
        let success = receipt.status.map(|s| s == U64::from(1)).unwrap_or(false);
        let gas_used = receipt.gas_used.unwrap_or_default().as_u64();
        let gas_price_used = receipt.effective_gas_price.unwrap_or(gas_price).as_u64();
        
        if success {
            self.nonce_manager.confirm(nonce).await;
            
            let mut stats = self.stats.write().await;
            stats.successful += 1;
            stats.total_profit_usd += target.estimated_profit;
            stats.total_gas_spent += gas_used * gas_price_used;
            
            tracing::info!(
                "✅ Liquidation successful: {} (gas: {}, profit: ${:.2})",
                tx_hash, gas_used, target.estimated_profit
            );
            
            Ok(LiquidationResult::success(
                tx_hash,
                gas_used,
                gas_price_used,
                debt_to_cover_f64 * 1.05, // Approximate collateral (5% bonus)
                debt_to_cover_f64,
                target.estimated_profit,
            ))
        } else {
            self.nonce_manager.fail(nonce).await;
            
            let mut stats = self.stats.write().await;
            stats.reverted += 1;
            
            tracing::warn!("❌ Liquidation reverted: {}", tx_hash);

            let mut result = LiquidationResult::failed(format!("Transaction reverted: {}", tx_hash));
            result.submitted_onchain = true;
            result.tx_hash = Some(tx_hash);
            Ok(result)
        }
    }

    /// Check if target is already being liquidated
    async fn is_pending(&self, user_address: &str) -> bool {
        let pending = self.pending.read().await;
        pending.contains_key(user_address)
    }
    
    /// Remove from pending
    async fn remove_pending(&self, user_address: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(user_address);
    }
    
    /// Get executor statistics
    pub async fn stats(&self) -> ExecutorStats {
        self.stats.read().await.clone()
    }
    
    /// Get pending liquidation count
    pub async fn pending_count(&self) -> usize {
        self.pending.read().await.len()
    }
    
    /// Sync nonce with on-chain state
    pub async fn sync_nonce(&self) -> Result<()> {
        self.nonce_manager.sync().await
    }
    
    /// Reset nonce manager (use when stuck)
    pub async fn reset_nonce(&self) -> Result<()> {
        self.nonce_manager.reset().await
    }
    
    /// Ensure debt token is approved for Aave Pool spending
    async fn ensure_approval(&self, target: &LiquidationTarget) -> Result<()> {
        let chain_id = self.chain_id().await.unwrap_or(1);
        let (debt_asset, _) = target.debt
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .context("No debt positions")?;
        
        let debt_asset_address = resolve_token_address(debt_asset, chain_id)
            .with_context(|| format!("Invalid debt asset address: {}", debt_asset))?;
        
        let erc20 = ERC20Approve::new(debt_asset_address, Arc::clone(&self.signer));
        
        // Check current allowance
        let allowance = erc20
            .allowance(self.wallet.address(), self.config.aave_pool_address)
            .call()
            .await
            .context("Failed to check allowance")?;
        
        // If allowance is insufficient, approve max
        if allowance < U256::from(u128::MAX) {
            tracing::info!("Approving debt token {:?} for Aave Pool", debt_asset_address);
            
            // Use impersonation via anvil to approve (avoids lifetime issues with ContractCall)
            let approve_data = ERC20Approve::new(debt_asset_address, Arc::clone(&self.signer))
                .approve(self.config.aave_pool_address, U256::MAX);
            
            approve_data
                .send()
                .await
                .context("Failed to send approve tx")?
                .confirmations(1)
                .await
                .context("Approve tx failed")?;
            
            // Sync nonce manager after external tx
            self.nonce_manager.sync().await?;
            tracing::info!("Debt token approval confirmed");
        }
        
        Ok(())
    }
    
    /// Get wallet address
    pub fn wallet_address(&self) -> Address {
        self.wallet.address()
    }
    
    /// Check wallet balance
    pub async fn wallet_balance(&self) -> Result<U256> {
        self.provider
            .get_balance(self.wallet.address(), None)
            .await
            .context("Failed to get wallet balance")
    }

    /// Read chain id from provider (used by worker when resolving reserve addresses).
    pub async fn chain_id(&self) -> Result<u64> {
        Ok(self.provider.get_chainid().await?.as_u64())
    }

    /// Check ERC20 token balance of executor wallet and convert to token units.
    pub async fn wallet_token_balance(&self, token: Address) -> Result<(f64, u8)> {
        let erc20 = ERC20Approve::new(token, Arc::clone(&self.signer));

        let raw = erc20
            .balance_of(self.wallet.address())
            .call()
            .await
            .with_context(|| format!("Failed to read balanceOf for token {:?}", token))?;

        let decimals = erc20
            .decimals()
            .call()
            .await
            .unwrap_or(18);

        let balance_tokens = raw
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0)
            / 10_f64.powi(decimals as i32);

        Ok((balance_tokens, decimals))
    }
}
