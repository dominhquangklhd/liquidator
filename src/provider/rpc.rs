use ethers::{
    prelude::*,
    providers::{Provider, Http, Middleware},
    types::{Filter, Log, H160},
};
use anyhow::{Result, Context};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Provider để kết nối với Aave Fork
pub struct AaveProvider {
    provider: Arc<Provider<Http>>,
    chain_id: u64,
}

impl AaveProvider {
    /// Tạo provider mới kết nối đến RPC endpoint
    pub async fn new(rpc_url: &str) -> Result<Self> {
        tracing::info!("Connecting to Aave fork at: {}", rpc_url);
        
        let provider = Provider::<Http>::try_from(rpc_url)
            .context("Failed to create provider")?;
        
        let provider = Arc::new(provider);
        
        // Get chain ID
        let chain_id = provider
            .get_chainid()
            .await
            .context("Failed to get chain ID")?
            .as_u64();
        
        tracing::info!("Connected to chain ID: {}", chain_id);
        
        // Get latest block để verify connection
        let block = provider
            .get_block_number()
            .await
            .context("Failed to get block number")?;
        
        tracing::info!("Current block number: {}", block);
        
        Ok(Self {
            provider,
            chain_id,
        })
    }

    /// Get provider reference
    pub fn provider(&self) -> Arc<Provider<Http>> {
        Arc::clone(&self.provider)
    }

    /// Get chain ID
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Get latest block number
    pub async fn get_block_number(&self) -> Result<U64> {
        self.provider
            .get_block_number()
            .await
            .context("Failed to get block number")
    }

    /// Subscribe to new blocks (polling)
    pub async fn watch_blocks(&self) -> Result<()> {
        tracing::info!("Starting block watcher...");
        
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(12));
        let mut last_block = self.get_block_number().await?;
        
        loop {
            interval.tick().await;
            
            match self.get_block_number().await {
                Ok(current_block) => {
                    if current_block > last_block {
                        tracing::info!("New block: {} (previous: {})", current_block, last_block);
                        last_block = current_block;
                        
                        // TODO: Fetch block details and emit events
                        // Có thể fetch transactions trong block để phân tích
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get block number: {:?}", e);
                }
            }
        }
    }

    /// Watch for Aave events (Borrow, Withdraw, Repay, Liquidation)
    /// Gửi events qua channel để RiskEngine xử lý
    pub async fn watch_aave_events(
        &self,
        aave_pool_address: H160,
        tx: mpsc::Sender<crate::events::event::Event>,
    ) -> Result<()> {
        tracing::info!("Starting Aave event watcher for pool: {:?}", aave_pool_address);

        // Aave V3 Event Signatures
        // Borrow(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint256 borrowRate, uint16 indexed referralCode)
        let borrow_signature = "Borrow(address,address,address,uint256,uint256,uint16)";
        
        // Withdraw(address indexed reserve, address indexed user, address indexed to, uint256 amount)
        let withdraw_signature = "Withdraw(address,address,address,uint256)";
        
        // Repay(address indexed reserve, address indexed user, address indexed repayer, uint256 amount, bool useATokens)
        let repay_signature = "Repay(address,address,address,uint256,bool)";
        
        // LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken)
        let liquidation_signature = "LiquidationCall(address,address,address,uint256,uint256,address,bool)";

        let mut last_block = self.get_block_number().await?.as_u64();
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3));

        loop {
            interval.tick().await;
            
            let current_block = match self.get_block_number().await {
                Ok(b) => b.as_u64(),
                Err(e) => {
                    tracing::error!("Failed to get block: {:?}", e);
                    continue;
                }
            };

            if current_block <= last_block {
                continue;
            }

            // Query logs từ last_block+1 đến current_block
            let filter = Filter::new()
                .address(aave_pool_address)
                .from_block(last_block + 1)
                .to_block(current_block)
                .topic0(vec![
                    ethers::utils::keccak256(borrow_signature.as_bytes()),
                    ethers::utils::keccak256(withdraw_signature.as_bytes()),
                    ethers::utils::keccak256(repay_signature.as_bytes()),
                    ethers::utils::keccak256(liquidation_signature.as_bytes()),
                ]);

            match self.provider.get_logs(&filter).await {
                Ok(logs) => {
                    tracing::info!("Found {} Aave events in blocks {}-{}", logs.len(), last_block + 1, current_block);
                    
                    for log in logs {
                        self.process_aave_log(log, &tx).await;
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch logs: {:?}", e);
                }
            }

            last_block = current_block;
        }
    }

    /// Xử lý log từ Aave contract và emit Event
    async fn process_aave_log(&self, log: Log, tx: &mpsc::Sender<crate::events::event::Event>) {
        // Parse topics để xác định event type
        if log.topics.is_empty() {
            return;
        }

        let event_sig = log.topics[0];
        
        // So sánh với event signatures
        let borrow_sig = ethers::utils::keccak256("Borrow(address,address,address,uint256,uint256,uint16)".as_bytes());
        let withdraw_sig = ethers::utils::keccak256("Withdraw(address,address,address,uint256)".as_bytes());
        let repay_sig = ethers::utils::keccak256("Repay(address,address,address,uint256,bool)".as_bytes());
        let liquidation_sig = ethers::utils::keccak256("LiquidationCall(address,address,address,uint256,uint256,address,bool)".as_bytes());

        let event_name = if event_sig == borrow_sig.into() {
            "Borrow"
        } else if event_sig == withdraw_sig.into() {
            "Withdraw"
        } else if event_sig == repay_sig.into() {
            "Repay"
        } else if event_sig == liquidation_sig.into() {
            "Liquidation"
        } else {
            "Unknown"
        };

        tracing::info!("📢 Detected {} event at block {:?}", event_name, log.block_number);

        // Extract user address từ topics (thường là indexed parameter)
        // Topics[1] thường là reserve/asset, Topics[2] là user
        if log.topics.len() > 2 {
            let user_address = format!("{:?}", H160::from(log.topics[2]));
            
            // Emit event để RiskEngine kiểm tra health factor
            // TODO: Parse asset ID từ log.topics[1]
            let affected_assets = vec!["ETH".to_string(), "USDC".to_string()]; // Simplified
            
            if let Err(e) = tx.send(crate::events::event::Event::MempoolTx {
                user_id: user_address,
                affected_assets,
            }).await {
                tracing::error!("Failed to send event: {:?}", e);
            }
        }
    }

    /// Watch mempool for pending transactions (requires special RPC support)
    /// Note: Anvil/Hardhat thường không support mempool subscription như mainnet
    pub async fn watch_mempool(&self, tx: mpsc::Sender<crate::events::event::Event>) -> Result<()> {
        tracing::warn!("Mempool watching is not fully supported on local forks");
        tracing::info!("For mempool detection, consider using:");
        tracing::info!("  - Flashbots RPC on mainnet");
        tracing::info!("  - eth_subscribe('newPendingTransactions') on supported nodes");
        
        // Placeholder - local forks usually don't have real mempool
        // On mainnet: use provider.subscribe_pending_txs().await
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Chỉ chạy khi có local fork
    async fn test_connection() {
        let provider = AaveProvider::new("http://127.0.0.1:8545")
            .await
            .expect("Failed to connect");
        
        assert!(provider.chain_id() > 0);
    }
}
