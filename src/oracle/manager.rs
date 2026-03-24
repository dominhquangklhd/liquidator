// Oracle Manager
//
// Quản lý tất cả Chainlink price feeds:
// - Khởi tạo feeds từ config
// - Poll giá định kỳ
// - Phát hiện price deviation
// - Emit Event::PriceUpdate qua channel
// - Fallback khi feed lỗi (dùng cached price)
// - Thống kê hoạt động

use ethers::providers::{Provider, Http};
use anyhow::Result;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

use super::config::{OracleConfig, PriceFeedConfig};
use super::chainlink::ChainlinkFeed;
use super::types::{PriceData, PriceFeedInfo, FeedStatus, OracleStats};
use crate::events::event::Event;

/// Oracle Manager
///
/// Quản lý lifecycle của tất cả price feeds
/// Thread-safe (Arc<RwLock<...>>) để chia sẻ giữa worker threads
pub struct OracleManager {
    /// Cấu hình
    config: OracleConfig,
    
    /// Ethereum provider
    provider: Arc<Provider<Http>>,
    
    /// Chainlink feeds (asset_id -> feed)
    feeds: HashMap<String, ChainlinkFeed>,
    
    /// Trạng thái feeds (thread-safe)
    feed_info: Arc<RwLock<HashMap<String, PriceFeedInfo>>>,
    
    /// Cache giá mới nhất (thread-safe, để các module khác đọc)
    price_cache: Arc<RwLock<HashMap<String, PriceData>>>,
    
    /// Thống kê
    stats: Arc<RwLock<OracleStats>>,
    
    /// Event sender — gửi PriceUpdate events đến RiskEngine
    event_tx: mpsc::Sender<Event>,
}

impl OracleManager {
    /// Tạo OracleManager mới
    pub async fn new(
        config: OracleConfig,
        provider: Arc<Provider<Http>>,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<Self> {
        let mut feeds = HashMap::new();
        let mut feed_info = HashMap::new();
        
        // Tạo feed reader cho mỗi config entry
        for feed_config in &config.feeds {
            let feed = ChainlinkFeed::new(Arc::clone(&provider), feed_config.clone());
            
            let info = PriceFeedInfo::new(
                feed_config.asset_id.clone(),
                feed_config.asset_symbol.clone(),
                feed_config.feed_address,
            );
            
            feed_info.insert(feed_config.asset_id.clone(), info);
            feeds.insert(feed_config.asset_id.clone(), feed);
        }
        
        tracing::info!("OracleManager created with {} feeds", feeds.len());
        
        Ok(Self {
            config,
            provider,
            feeds,
            feed_info: Arc::new(RwLock::new(feed_info)),
            price_cache: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(OracleStats::default())),
            event_tx,
        })
    }
    
    /// Khởi tạo tất cả feeds (đọc metadata từ on-chain)
    pub async fn initialize(&mut self) -> Result<()> {
        tracing::info!("Initializing {} oracle feeds...", self.feeds.len());
        
        let mut success = 0;
        let mut failed = 0;
        
        for (asset_id, feed) in self.feeds.iter_mut() {
            match feed.initialize().await {
                Ok(_) => {
                    // Đọc giá ban đầu
                    match feed.latest_price().await {
                        Ok(price) => {
                            tracing::info!(
                                "  ✓ {} = ${:.2} (feed: {:?})",
                                asset_id, price.price_usd, feed.feed_address()
                            );
                            
                            // Cache giá ban đầu
                            let mut cache = self.price_cache.write().await;
                            cache.insert(asset_id.clone(), price.clone());
                            
                            // Update feed info
                            let mut info = self.feed_info.write().await;
                            if let Some(fi) = info.get_mut(asset_id) {
                                fi.status = FeedStatus::Active;
                                fi.current_price = Some(price);
                                fi.success_count += 1;
                            }
                            
                            success += 1;
                        }
                        Err(e) => {
                            tracing::warn!("  ✗ {} — initial price read failed: {:?}", asset_id, e);
                            
                            let mut info = self.feed_info.write().await;
                            if let Some(fi) = info.get_mut(asset_id) {
                                fi.status = FeedStatus::Error(e.to_string());
                                fi.error_count += 1;
                            }
                            
                            failed += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("  ✗ {} — initialization failed: {:?}", asset_id, e);
                    
                    let mut info = self.feed_info.write().await;
                    if let Some(fi) = info.get_mut(asset_id) {
                        fi.status = FeedStatus::Error(e.to_string());
                        fi.error_count += 1;
                    }
                    
                    failed += 1;
                }
            }
        }
        
        tracing::info!(
            "Oracle initialization complete: {}/{} feeds active, {} failed",
            success, self.feeds.len(), failed
        );
        
        Ok(())
    }
    
    /// Poll tất cả feeds một lần và emit events cho giá thay đổi đáng kể
    pub async fn poll_all(&self) -> Result<usize> {
        let mut updates_emitted = 0;
        
        {
            let mut stats = self.stats.write().await;
            stats.total_polls += 1;
        }
        
        for (asset_id, feed) in &self.feeds {
            match self.poll_single_feed(asset_id, feed).await {
                Ok(emitted) => {
                    if emitted {
                        updates_emitted += 1;
                    }
                }
                Err(e) => {
                    // Retry logic
                    let mut retried = false;
                    for retry in 1..=self.config.max_retries {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            self.config.retry_delay_ms
                        )).await;
                        
                        match self.poll_single_feed(asset_id, feed).await {
                            Ok(emitted) => {
                                tracing::debug!(
                                    "Feed {} recovered after {} retries", asset_id, retry
                                );
                                if emitted {
                                    updates_emitted += 1;
                                }
                                retried = true;
                                break;
                            }
                            Err(_) => continue,
                        }
                    }
                    
                    if !retried {
                        tracing::error!(
                            "Feed {} failed after {} retries: {:?}",
                            asset_id, self.config.max_retries, e
                        );
                        
                        let mut info = self.feed_info.write().await;
                        if let Some(fi) = info.get_mut(asset_id) {
                            fi.status = FeedStatus::Error(e.to_string());
                            fi.error_count += 1;
                        }
                        
                        let mut stats = self.stats.write().await;
                        stats.total_errors += 1;
                    }
                }
            }
        }
        
        Ok(updates_emitted)
    }
    
    /// Poll một feed cụ thể
    async fn poll_single_feed(&self, asset_id: &str, feed: &ChainlinkFeed) -> Result<bool> {
        let price_data = feed.latest_price().await?;
        
        // Lấy giá cũ từ cache
        let previous_price = {
            let cache = self.price_cache.read().await;
            cache.get(asset_id).map(|p| p.price_usd)
        };
        
        // Lấy deviation threshold cho feed này
        let deviation_threshold = self.config.get_feed(asset_id)
            .map(|f| f.deviation_threshold_pct)
            .unwrap_or(self.config.default_deviation_pct);
        
        // Kiểm tra staleness
        let staleness_timeout = self.config.get_feed(asset_id)
            .map(|f| f.heartbeat_secs)
            .unwrap_or(self.config.default_staleness_secs);
        
        let is_stale = price_data.is_stale(staleness_timeout);
        
        // Update trạng thái feed
        {
            let mut info = self.feed_info.write().await;
            if let Some(fi) = info.get_mut(asset_id) {
                fi.status = if is_stale { FeedStatus::Stale } else { FeedStatus::Active };
                fi.previous_price = previous_price;
                fi.current_price = Some(price_data.clone());
                fi.success_count += 1;
            }
        }
        
        if is_stale {
            tracing::warn!(
                "⚠ Feed {} is STALE (updated_at: {}, threshold: {}s)",
                asset_id, price_data.updated_at, staleness_timeout
            );
        }
        
        // Kiểm tra deviation — chỉ emit event khi thay đổi đáng kể
        let should_emit = match previous_price {
            Some(prev) => {
                let deviation = price_data.deviation_pct(prev);
                
                if deviation >= deviation_threshold {
                    tracing::info!(
                        "📊 {} price change: ${:.2} → ${:.2} ({:.2}% deviation)",
                        asset_id, prev, price_data.price_usd, deviation
                    );
                    true
                } else {
                    // Log all polled prices for debugging (to see if RPC reflects state changes)
                    tracing::debug!(
                        "[ORACLE] {} = ${:.2} (deviation: {:.4}% < threshold {:.2}%, no event)",
                        asset_id, price_data.price_usd, deviation, deviation_threshold
                    );
                    false
                }
            }
            None => {
                // Giá đầu tiên — luôn emit
                tracing::info!(
                    "📊 {} initial price: ${:.2}",
                    asset_id, price_data.price_usd
                );
                true
            }
        };
        
        // Update cache
        {
            let mut cache = self.price_cache.write().await;
            cache.insert(asset_id.to_string(), price_data.clone());
        }
        
        // Emit event nếu cần
        if should_emit {
            // Gửi PriceUpdate event đến RiskEngine
            // 
            // Chuyển đổi giá USD sang giá ETH-based cho RiskEngine
            // RiskEngine hiện dùng price_in_eth (tỷ lệ so với ETH)
            // Nên cần convert: price_in_eth = price_usd / eth_price_usd
            let price_for_engine = self.convert_to_engine_price(asset_id, price_data.price_usd).await;
            
            if let Err(e) = self.event_tx.send(Event::PriceUpdate {
                asset_id: asset_id.to_string(),
                new_price: price_for_engine,
            }).await {
                tracing::error!("Failed to send PriceUpdate event for {}: {:?}", asset_id, e);
            } else {
                let mut stats = self.stats.write().await;
                stats.total_updates_emitted += 1;
                
                let mut info = self.feed_info.write().await;
                if let Some(fi) = info.get_mut(asset_id) {
                    fi.deviation_events += 1;
                }
            }

            // ETH/USD thay đổi sẽ làm giá ETH-based của mọi asset khác thay đổi,
            // kể cả khi USD price của chính asset đó không đổi (e.g., USDC/USD ~ 1.0).
            if asset_id == "ETH" {
                self.emit_repriced_assets_after_eth_move(price_data.price_usd).await;
            }
        }
        
        Ok(should_emit)
    }

    /// Khi ETH/USD biến động, phát thêm updates cho các assets non-ETH
    /// để RiskEngine recalculation đúng theo price_in_eth mới.
    async fn emit_repriced_assets_after_eth_move(&self, eth_price_usd: f64) {
        if eth_price_usd <= 0.0 {
            return;
        }

        let cache_snapshot = {
            let cache = self.price_cache.read().await;
            cache.iter()
                .map(|(asset_id, price)| (asset_id.clone(), price.price_usd))
                .collect::<Vec<_>>()
        };

        for (other_asset_id, other_price_usd) in cache_snapshot {
            if other_asset_id == "ETH" || other_asset_id == "WETH" {
                continue;
            }

            let repriced_in_eth = other_price_usd / eth_price_usd;
            if repriced_in_eth <= 0.0 {
                continue;
            }

            if let Err(e) = self.event_tx.send(Event::PriceUpdate {
                asset_id: other_asset_id.clone(),
                new_price: repriced_in_eth,
            }).await {
                tracing::error!(
                    "Failed to send repriced PriceUpdate event for {}: {:?}",
                    other_asset_id,
                    e
                );
                continue;
            }

            let mut stats = self.stats.write().await;
            stats.total_updates_emitted += 1;

            let mut info = self.feed_info.write().await;
            if let Some(fi) = info.get_mut(&other_asset_id) {
                fi.deviation_events += 1;
            }
        }
    }
    
    /// Chuyển đổi giá USD sang giá ETH-based cho RiskEngine
    /// 
    /// RiskEngine sử dụng `price_in_eth` (e.g., ETH=1.0, USDC=0.0005)
    /// Oracle đọc giá USD trực tiếp (e.g., ETH=$2500, USDC=$1.0)
    /// Convert: price_in_eth = price_usd / eth_price_usd
    async fn convert_to_engine_price(&self, asset_id: &str, price_usd: f64) -> f64 {
        // Nếu asset là ETH, price_in_eth = 1.0 (base)
        if asset_id == "ETH" || asset_id == "WETH" {
            return 1.0;
        }
        
        // Lấy giá ETH từ cache
        let eth_price = {
            let cache = self.price_cache.read().await;
            cache.get("ETH").map(|p| p.price_usd)
        };
        
        match eth_price {
            Some(eth_usd) if eth_usd > 0.0 => {
                price_usd / eth_usd
            }
            _ => {
                // Fallback: ước lượng nếu chưa có giá ETH
                tracing::warn!(
                    "ETH price not available, using USD price directly for {}",
                    asset_id
                );
                price_usd
            }
        }
    }
    
    // ========================================================================
    // Public API — để các module khác đọc giá
    // ========================================================================
    
    /// Lấy giá hiện tại của một asset (từ cache)
    pub async fn get_price(&self, asset_id: &str) -> Option<PriceData> {
        let cache = self.price_cache.read().await;
        cache.get(asset_id).cloned()
    }
    
    /// Lấy giá USD hiện tại
    pub async fn get_price_usd(&self, asset_id: &str) -> Option<f64> {
        let cache = self.price_cache.read().await;
        cache.get(asset_id).map(|p| p.price_usd)
    }
    
    /// Lấy tất cả giá hiện tại
    pub async fn get_all_prices(&self) -> HashMap<String, f64> {
        let cache = self.price_cache.read().await;
        cache.iter()
            .map(|(k, v)| (k.clone(), v.price_usd))
            .collect()
    }
    
    /// Lấy trạng thái tất cả feeds
    pub async fn get_all_feed_info(&self) -> Vec<PriceFeedInfo> {
        let info = self.feed_info.read().await;
        info.values().cloned().collect()
    }
    
    /// Lấy thống kê oracle
    pub async fn get_stats(&self) -> OracleStats {
        let mut stats = self.stats.read().await.clone();
        
        // Cập nhật feed counts
        let info = self.feed_info.read().await;
        stats.active_feeds = info.values().filter(|f| f.status == FeedStatus::Active).count();
        stats.stale_feeds = info.values().filter(|f| f.status == FeedStatus::Stale).count();
        stats.error_feeds = info.values()
            .filter(|f| matches!(f.status, FeedStatus::Error(_)))
            .count();
        
        stats
    }
    
    /// Lấy price_cache Arc để chia sẻ với các module khác (read-only)
    pub fn price_cache(&self) -> Arc<RwLock<HashMap<String, PriceData>>> {
        Arc::clone(&self.price_cache)
    }
    
    /// Thêm feed mới at runtime
    pub async fn add_feed(&mut self, feed_config: PriceFeedConfig) -> Result<()> {
        let asset_id = feed_config.asset_id.clone();
        
        let mut feed = ChainlinkFeed::new(Arc::clone(&self.provider), feed_config.clone());
        feed.initialize().await?;
        
        let info = PriceFeedInfo::new(
            feed_config.asset_id.clone(),
            feed_config.asset_symbol.clone(),
            feed_config.feed_address,
        );
        
        self.feed_info.write().await.insert(asset_id.clone(), info);
        self.feeds.insert(asset_id.clone(), feed);
        
        tracing::info!("Added new oracle feed: {}", asset_id);
        Ok(())
    }
    
    /// Số lượng feeds
    pub fn feed_count(&self) -> usize {
        self.feeds.len()
    }
    
    /// Config reference
    pub fn config(&self) -> &OracleConfig {
        &self.config
    }
}
