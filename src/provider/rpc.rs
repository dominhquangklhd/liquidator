use ethers::{
    prelude::*,
    providers::{Provider, Http, Middleware},
    types::{Filter, Log, H160, H256},
};
use anyhow::{Result, Context};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Provider để kết nối với Aave Fork
pub struct AaveProvider {
    provider: Arc<Provider<Http>>,
    chain_id: u64,
    block_poll_interval_secs: u64,
    event_poll_interval_secs: u64,
    reserve_asset_map: HashMap<H160, String>,
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

        let block_poll_interval_secs = env_u64("BLOCK_POLL_INTERVAL_SECS", 12);
        let event_poll_interval_secs = env_u64("AAVE_EVENT_POLL_INTERVAL_SECS", 3);
        let reserve_asset_map = load_reserve_asset_map();
        
        Ok(Self {
            provider,
            chain_id,
            block_poll_interval_secs,
            event_poll_interval_secs,
            reserve_asset_map,
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
    pub async fn watch_blocks(&self, tx: mpsc::Sender<crate::events::event::Event>) -> Result<()> {
        tracing::info!(
            "Starting block watcher (interval={}s)...",
            self.block_poll_interval_secs
        );
        
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.block_poll_interval_secs));
        let mut last_block = self.get_block_number().await?;
        
        loop {
            interval.tick().await;
            
            match self.get_block_number().await {
                Ok(current_block) => {
                    if current_block > last_block {
                        tracing::info!("New block: {} (previous: {})", current_block, last_block);
                        last_block = current_block;

                        if let Err(e) = tx.send(crate::events::event::Event::Block {
                            block_number: current_block.as_u64(),
                        }).await {
                            tracing::error!("Failed to send Block event: {:?}", e);
                        }
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
        tracing::info!(
            "Starting Aave event watcher for pool: {:?} (interval={}s)",
            aave_pool_address,
            self.event_poll_interval_secs
        );

        // Aave V3 Event Signatures
        // Supply(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint16 indexed referralCode)
        let supply_signature = "Supply(address,address,address,uint256,uint16)";

        // Borrow(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint256 borrowRate, uint16 indexed referralCode)
        let borrow_signature = "Borrow(address,address,address,uint256,uint256,uint16)";
        
        // Withdraw(address indexed reserve, address indexed user, address indexed to, uint256 amount)
        let withdraw_signature = "Withdraw(address,address,address,uint256)";
        
        // Repay(address indexed reserve, address indexed user, address indexed repayer, uint256 amount, bool useATokens)
        let repay_signature = "Repay(address,address,address,uint256,bool)";
        
        // LiquidationCall(address indexed collateralAsset, address indexed debtAsset, address indexed user, uint256 debtToCover, uint256 liquidatedCollateralAmount, address liquidator, bool receiveAToken)
        let liquidation_signature = "LiquidationCall(address,address,address,uint256,uint256,address,bool)";

        let mut last_block = self.get_block_number().await?.as_u64();
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.event_poll_interval_secs));

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
                    ethers::utils::keccak256(supply_signature.as_bytes()),
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
        let supply_sig = ethers::utils::keccak256("Supply(address,address,address,uint256,uint16)".as_bytes());
        let borrow_sig = ethers::utils::keccak256("Borrow(address,address,address,uint256,uint256,uint16)".as_bytes());
        let withdraw_sig = ethers::utils::keccak256("Withdraw(address,address,address,uint256)".as_bytes());
        let repay_sig = ethers::utils::keccak256("Repay(address,address,address,uint256,bool)".as_bytes());
        let liquidation_sig = ethers::utils::keccak256("LiquidationCall(address,address,address,uint256,uint256,address,bool)".as_bytes());

        let event_name = if event_sig == supply_sig.into() {
            "Supply"
        } else if event_sig == borrow_sig.into() {
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

        if let Some(user_address) = extract_user_from_topics(event_sig, &log.topics) {
            let affected_assets = self.extract_affected_assets(event_sig, &log.topics);

            if affected_assets.is_empty() {
                tracing::debug!(
                    "No reserve->asset mapping found for {} event at block {:?}",
                    event_name,
                    log.block_number
                );
            }

            if let Err(e) = tx
                .send(crate::events::event::Event::MempoolTx {
                    user_id: user_address,
                    affected_assets,
                })
                .await
            {
                tracing::error!("Failed to send event: {:?}", e);
            }
        }
    }

    fn extract_affected_assets(&self, event_sig: H256, topics: &[H256]) -> Vec<String> {
        if topics.len() < 2 {
            return Vec::new();
        }

        let liquidation_sig =
            H256::from(ethers::utils::keccak256("LiquidationCall(address,address,address,uint256,uint256,address,bool)".as_bytes()));

        if event_sig == liquidation_sig {
            let mut assets = Vec::new();
            if let Some(asset) = self.lookup_asset_from_topic(topics.get(1)) {
                assets.push(asset);
            }
            if let Some(asset) = self.lookup_asset_from_topic(topics.get(2)) {
                if !assets.iter().any(|a| a == &asset) {
                    assets.push(asset);
                }
            }
            return assets;
        }

        self.lookup_asset_from_topic(topics.get(1))
            .map(|asset| vec![asset])
            .unwrap_or_default()
    }

    fn lookup_asset_from_topic(&self, topic: Option<&H256>) -> Option<String> {
        topic
            .map(|t| H160::from_slice(&t.as_bytes()[12..]))
            .and_then(|reserve| self.reserve_asset_map.get(&reserve).cloned())
    }

    /// Watch mempool for pending transactions (requires special RPC support)
    /// Note: Anvil/Hardhat thường không support mempool subscription như mainnet
    pub async fn watch_mempool(&self, _tx: mpsc::Sender<crate::events::event::Event>) -> Result<()> {
        tracing::warn!("Mempool watching is not fully supported on local forks");
        tracing::info!("For mempool detection, consider using:");
        tracing::info!("  - Flashbots RPC on mainnet");
        tracing::info!("  - eth_subscribe('newPendingTransactions') on supported nodes");
        
        // Placeholder - local forks usually don't have real mempool
        // On mainnet: use provider.subscribe_pending_txs().await
        
        Ok(())
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn extract_user_from_topics(event_sig: H256, topics: &[H256]) -> Option<String> {
    let liquidation_sig =
        H256::from(ethers::utils::keccak256("LiquidationCall(address,address,address,uint256,uint256,address,bool)".as_bytes()));

    let user_topic = if event_sig == liquidation_sig {
        topics.get(3)
    } else {
        topics.get(2)
    };

    user_topic.map(|t| format!("{:?}", H160::from_slice(&t.as_bytes()[12..])))
}

fn load_reserve_asset_map() -> HashMap<H160, String> {
    let mut map = default_reserve_asset_map();

    if let Ok(raw) = std::env::var("AAVE_RESERVE_ASSET_MAP") {
        for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let mut parts = entry.splitn(2, '=').map(str::trim);
            let left = parts.next();
            let right = parts.next();
            let (Some(addr_raw), Some(asset)) = (left, right) else {
                continue;
            };

            if let Ok(addr) = addr_raw.parse::<H160>() {
                map.insert(addr, asset.to_string());
            }
        }
    }

    map
}

fn default_reserve_asset_map() -> HashMap<H160, String> {
    let mut map = HashMap::new();

    let defaults = [
        ("0xC02aaA39b223FE8D0A0E5C4F27eAD9083C756Cc2", "WETH"),
        ("0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "USDC"),
        ("0xdAC17F958D2ee523a2206206994597C13D831ec7", "USDT"),
        ("0x6B175474E89094C44Da98b954EedeAC495271d0F", "DAI"),
        ("0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", "WBTC"),
        ("0x514910771AF9Ca656af840dff83E8264EcF986CA", "LINK"),
        ("0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DdAE9", "AAVE"),
    ];

    for (addr_raw, asset) in defaults {
        if let Ok(addr) = addr_raw.parse::<H160>() {
            map.insert(addr, asset.to_string());
        }
    }

    map
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
