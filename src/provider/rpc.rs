use ethers::{
    prelude::*,
    providers::{Provider, Http},
};
use anyhow::{Result, Context};
use std::sync::Arc;

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
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get block number: {:?}", e);
                }
            }
        }
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
