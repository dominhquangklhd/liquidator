use ethers::{
    prelude::*,
    providers::{Provider, Http, Middleware, Ws},
    types::{Filter, Log, H160, H256, U256},
};
use anyhow::{Result, Context};
use futures::StreamExt;
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

        // Borrow(address indexed reserve, address user, address indexed onBehalfOf, uint256 amount, uint8 interestRateMode, uint256 borrowRate, uint16 indexed referralCode)
        let borrow_signature = "Borrow(address,address,address,uint256,uint8,uint256,uint16)";
        
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
            // Alchemy free tier giới hạn eth_getLogs chỉ 10 block range, nên split thành multiple queries
            let mut query_from = last_block + 1;
            let max_range = 10u64;

            while query_from <= current_block {
                let query_to = std::cmp::min(query_from + max_range - 1, current_block);
                
                let filter = Filter::new()
                    .from_block(query_from)
                    .to_block(query_to)
                    .topic0(vec![
                        ethers::utils::keccak256(supply_signature.as_bytes()),
                        ethers::utils::keccak256(borrow_signature.as_bytes()),
                        ethers::utils::keccak256(withdraw_signature.as_bytes()),
                        ethers::utils::keccak256(repay_signature.as_bytes()),
                        ethers::utils::keccak256(liquidation_signature.as_bytes()),
                    ]);

                match self.provider.get_logs(&filter).await {
                    Ok(logs) => {
                        if !logs.is_empty() {
                            tracing::info!("Found {} Aave events in blocks {}-{}", logs.len(), query_from, query_to);
                        }
                        
                        for log in logs {
                            self.process_aave_log(log, tx.clone()).await;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to fetch logs (blocks {}-{}): {:?}", query_from, query_to, e);
                    }
                }
                
                query_from = query_to + 1;
            }
            
            last_block = current_block;
        }
    }

    /// Watch Aave events qua WebSocket subscription (push-based)
    pub async fn watch_aave_events_ws(
        &self,
        ws_url: &str,
        aave_pool_address: H160,
        tx: mpsc::Sender<crate::events::event::Event>,
    ) -> Result<()> {
        tracing::info!(
            "Starting Aave event WS watcher for pool: {:?} via {}",
            aave_pool_address,
            ws_url
        );

        let ws = Ws::connect(ws_url)
            .await
            .with_context(|| format!("Failed to connect Aave WS endpoint: {}", ws_url))?;
        let provider_ws = Provider::new(ws);

        // Aave V3 Event Signatures
        let supply_signature = "Supply(address,address,address,uint256,uint16)";
        let borrow_signature = "Borrow(address,address,address,uint256,uint8,uint256,uint16)";
        let withdraw_signature = "Withdraw(address,address,address,uint256)";
        let repay_signature = "Repay(address,address,address,uint256,bool)";
        let liquidation_signature = "LiquidationCall(address,address,address,uint256,uint256,address,bool)";

        let filter = Filter::new()
            .address(aave_pool_address)
            .topic0(vec![
                ethers::utils::keccak256(supply_signature.as_bytes()),
                ethers::utils::keccak256(borrow_signature.as_bytes()),
                ethers::utils::keccak256(withdraw_signature.as_bytes()),
                ethers::utils::keccak256(repay_signature.as_bytes()),
                ethers::utils::keccak256(liquidation_signature.as_bytes()),
            ]);

        let mut stream = provider_ws
            .subscribe_logs(&filter)
            .await
            .context("Failed to subscribe Aave logs via WS")?;

        while let Some(log) = stream.next().await {
            self.process_aave_log(log, tx.clone()).await;
        }

        anyhow::bail!("Aave WS log stream ended unexpectedly");
    }

    /// Xử lý log từ Aave contract để ghi nhận hoạt động on-chain
    async fn process_aave_log(&self, log: Log, tx: mpsc::Sender<crate::events::event::Event>) {
        // Parse topics để xác định event type
        if log.topics.is_empty() {
            return;
        }

        let event_sig = log.topics[0];
        
        // So sánh với event signatures
        let supply_sig = ethers::utils::keccak256("Supply(address,address,address,uint256,uint16)".as_bytes());
        let borrow_sig = ethers::utils::keccak256("Borrow(address,address,address,uint256,uint8,uint256,uint16)".as_bytes());
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
            // Try to get asset ID from mapping, fallback to hex address if not found
            let asset_id_or_fallback = self.lookup_asset_from_topic(log.topics.get(1))
                .or_else(|| {
                    // Fallback: use hex address if not found in mapping
                    log.topics.get(1)
                        .map(|t| {
                            let addr = H160::from_slice(&t.as_bytes()[12..]);
                            tracing::warn!("Asset {} not in mapping, using fallback address", addr);
                            format!("{:?}", addr)
                        })
                });

            if let Some(asset_id) = asset_id_or_fallback {
                let amount = extract_event_amount(event_sig, &log.data.0)
                    .map(|raw| normalize_amount(raw, &asset_id))
                    .unwrap_or(0.0);

                let event_to_send = if event_sig == H256::from(supply_sig) {
                    Some(crate::events::event::Event::UserDeposit {
                        user_id: user_address.clone(),
                        asset_id,
                        amount,
                    })
                } else if event_sig == H256::from(borrow_sig) {
                    tracing::debug!("Preparing UserBorrow event: user={} asset={} amount={}", user_address, &asset_id, amount);
                    Some(crate::events::event::Event::UserBorrow {
                        user_id: user_address.clone(),
                        asset_id: asset_id.clone(),
                        amount,
                    })
                } else if event_sig == H256::from(withdraw_sig) {
                    tracing::debug!("Preparing UserWithdraw event: user={} asset={} amount={}", user_address, &asset_id, amount);
                    Some(crate::events::event::Event::UserWithdraw {
                        user_id: user_address.clone(),
                        asset_id: asset_id.clone(),
                        amount,
                    })
                } else if event_sig == H256::from(repay_sig) {
                    tracing::debug!("Preparing UserRepay event: user={} asset={} amount={}", user_address, &asset_id, amount);
                    Some(crate::events::event::Event::UserRepay {
                        user_id: user_address.clone(),
                        asset_id: asset_id.clone(),
                        amount,
                    })
                } else {
                    None
                };

                if let Some(event) = event_to_send {
                    if let Err(e) = tx.send(event).await {
                        tracing::error!("Failed to send user event from Aave watcher: {:?}", e);
                    }
                }
            } else {
                tracing::warn!("Could not determine asset_id for {} event", event_name);
            }
        } else {
            tracing::warn!("Could not extract user_address for {} event", event_name);
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

fn extract_event_amount(event_sig: H256, data: &[u8]) -> Option<U256> {
    let supply_sig = H256::from(ethers::utils::keccak256("Supply(address,address,address,uint256,uint16)".as_bytes()));
    let borrow_sig = H256::from(ethers::utils::keccak256("Borrow(address,address,address,uint256,uint8,uint256,uint16)".as_bytes()));
    let withdraw_sig = H256::from(ethers::utils::keccak256("Withdraw(address,address,address,uint256)".as_bytes()));
    let repay_sig = H256::from(ethers::utils::keccak256("Repay(address,address,address,uint256,bool)".as_bytes()));

    let word_index = if event_sig == supply_sig || event_sig == borrow_sig {
        // Supply/Borrow have `user` as first non-indexed argument, then `amount`.
        1
    } else if event_sig == withdraw_sig || event_sig == repay_sig {
        // Withdraw/Repay expose `amount` as first non-indexed argument.
        0
    } else {
        return None;
    };

    decode_u256_word(data, word_index)
}

fn decode_u256_word(data: &[u8], word_index: usize) -> Option<U256> {
    let start = word_index.checked_mul(32)?;
    let end = start.checked_add(32)?;
    if end > data.len() {
        return None;
    }

    Some(U256::from_big_endian(&data[start..end]))
}

fn normalize_amount(raw_amount: U256, asset_id: &str) -> f64 {
    let decimals = asset_decimals(asset_id) as i32;
    let raw_f64 = raw_amount.to_string().parse::<f64>().unwrap_or(0.0);
    if raw_f64 <= 0.0 {
        return 0.0;
    }

    raw_f64 / 10f64.powi(decimals)
}

fn asset_decimals(asset_id: &str) -> u32 {
    match asset_id {
        "USDC" | "USDT" => 6,
        _ => 18,
    }
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
        ("0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0", "WSTETH"),
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
mod integration_tests {
    use super::*;
    use ethers::abi::{encode, Token};

    fn sig(signature: &str) -> H256 {
        H256::from(ethers::utils::keccak256(signature.as_bytes()))
    }

    #[test]
    fn extract_amount_from_supply_payload_uses_second_word() {
        let data = encode(&[
            Token::Address(H160::from_low_u64_be(1)),
            Token::Uint(U256::from(123_456_789u64)),
            Token::Uint(U256::from(42u64)),
        ]);

        let amount = extract_event_amount(
            sig("Supply(address,address,address,uint256,uint16)"),
            &data,
        )
        .expect("amount should decode");

        assert_eq!(amount, U256::from(123_456_789u64));
    }

    #[test]
    fn extract_amount_from_withdraw_payload_uses_first_word() {
        let data = encode(&[
            Token::Uint(U256::from(55_000_000u64)),
        ]);

        let amount = extract_event_amount(
            sig("Withdraw(address,address,address,uint256)"),
            &data,
        )
        .expect("amount should decode");

        assert_eq!(amount, U256::from(55_000_000u64));
    }

    #[test]
    fn normalize_amount_respects_asset_decimals() {
        let usdc = normalize_amount(U256::from(1_500_000u64), "USDC");
        let weth = normalize_amount(U256::from_dec_str("1500000000000000000").unwrap(), "WETH");

        assert!((usdc - 1.5).abs() < 1e-12);
        assert!((weth - 1.5).abs() < 1e-12);
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
