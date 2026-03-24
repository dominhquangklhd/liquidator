// Oracle Configuration
//
// Cấu hình cho Oracle Price Feeds module:
// - Danh sách price feed contracts (Chainlink AggregatorV3)
// - Polling interval
// - Price deviation threshold
// - Staleness timeout

use ethers::types::Address;
use std::collections::HashMap;
use std::env;

/// Cấu hình cho một price feed cụ thể
#[derive(Debug, Clone)]
pub struct PriceFeedConfig {
    /// Tên asset (e.g., "ETH", "WBTC", "USDC")
    pub asset_symbol: String,
    
    /// Asset ID dùng trong hệ thống (trùng với AssetId trong data module)
    pub asset_id: String,
    
    /// Địa chỉ Chainlink AggregatorV3 contract
    pub feed_address: Address,
    
    /// Số decimals của price feed (thường là 8 cho USD pairs)
    pub decimals: u8,
    
    /// Heartbeat timeout (seconds) — nếu price không update sau khoảng này, coi là stale
    /// Chainlink ETH/USD: 3600s, WBTC/USD: 3600s, USDC/USD: 86400s
    pub heartbeat_secs: u64,
    
    /// Price deviation threshold (%) — chỉ emit event khi giá thay đổi >= threshold
    /// Chainlink ETH/USD: 0.5%, WBTC/USD: 0.5%, USDC/USD: 0.1%
    pub deviation_threshold_pct: f64,
    
    /// Có phải là stable asset không (USDC, USDT, DAI)
    pub is_stablecoin: bool,
}

/// Cấu hình tổng thể cho Oracle module
#[derive(Debug, Clone)]
pub struct OracleConfig {
    /// Polling interval (milliseconds) — tần suất kiểm tra giá mới
    /// Khuyến nghị: 1000-3000ms cho local fork, 12000ms cho mainnet (mỗi block)
    pub poll_interval_ms: u64,
    
    /// Price deviation threshold mặc định (%) — emit event khi giá thay đổi >= threshold
    /// Áp dụng cho các feed không có deviation_threshold_pct riêng
    pub default_deviation_pct: f64,
    
    /// Staleness timeout mặc định (seconds) — coi price là stale nếu quá cũ
    pub default_staleness_secs: u64,
    
    /// Số lần retry khi RPC call thất bại
    pub max_retries: u32,
    
    /// Delay giữa các retry (milliseconds)
    pub retry_delay_ms: u64,
    
    /// Có log giá mỗi lần poll không (verbose mode)
    pub verbose_logging: bool,
    
    /// Danh sách price feeds
    pub feeds: Vec<PriceFeedConfig>,
}

impl Default for OracleConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 3000,
            default_deviation_pct: 0.5,
            default_staleness_secs: 3600,
            max_retries: 3,
            retry_delay_ms: 1000,
            verbose_logging: false,
            feeds: Vec::new(),
        }
    }
}

impl OracleConfig {
    /// Tạo config cho Ethereum Mainnet với Chainlink feeds chuẩn
    pub fn mainnet() -> Self {
        let feeds = vec![
            PriceFeedConfig {
                asset_symbol: "ETH".to_string(),
                asset_id: "ETH".to_string(),
                // Chainlink ETH/USD trên mainnet
                feed_address: "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 3600,
                deviation_threshold_pct: 0.5,
                is_stablecoin: false,
            },
            PriceFeedConfig {
                asset_symbol: "WBTC".to_string(),
                asset_id: "WBTC".to_string(),
                // Chainlink BTC/USD trên mainnet
                feed_address: "0xF4030086522a5bEEa4988F8cA5B36dbC97BeE88c"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 3600,
                deviation_threshold_pct: 0.5,
                is_stablecoin: false,
            },
            PriceFeedConfig {
                asset_symbol: "USDC".to_string(),
                asset_id: "USDC".to_string(),
                // Chainlink USDC/USD trên mainnet
                feed_address: "0x8fFfFfd4AfB6115b954Bd326cbe7B4BA576818f6"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 86400,
                deviation_threshold_pct: 0.1,
                is_stablecoin: true,
            },
            PriceFeedConfig {
                asset_symbol: "DAI".to_string(),
                asset_id: "DAI".to_string(),
                // Chainlink DAI/USD trên mainnet
                feed_address: "0xAed0c38402a5d19df6E4c03F4E2DceD6e29c1ee9"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 3600,
                deviation_threshold_pct: 0.1,
                is_stablecoin: true,
            },
            PriceFeedConfig {
                asset_symbol: "LINK".to_string(),
                asset_id: "LINK".to_string(),
                // Chainlink LINK/USD trên mainnet
                feed_address: "0x2c1d072e956AFFC0D435Cb7AC38EF18d24d9127c"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 3600,
                deviation_threshold_pct: 0.5,
                is_stablecoin: false,
            },
            PriceFeedConfig {
                asset_symbol: "AAVE".to_string(),
                asset_id: "AAVE".to_string(),
                // Chainlink AAVE/USD trên mainnet
                feed_address: "0x547a514d5e3769680Ce22B2361c10Ea13619e8a9"
                    .parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 3600,
                deviation_threshold_pct: 1.0,
                is_stablecoin: false,
            },
        ];

        Self {
            poll_interval_ms: 12000, // Mỗi block trên mainnet (~12s)
            default_deviation_pct: 0.5,
            default_staleness_secs: 3600,
            max_retries: 3,
            retry_delay_ms: 2000,
            verbose_logging: false,
            feeds,
        }
    }

    /// Tạo config cho Anvil local fork (polling nhanh hơn)
    pub fn local_fork() -> Self {
        let mut config = Self::mainnet();
        config.poll_interval_ms = 5000;   // Poll nhanh hơn trên local
        config.retry_delay_ms = 500;
        config.verbose_logging = true;
        config
    }

    /// Apply environment overrides for runtime configuration.
    ///
    /// Supported env vars:
    /// - ORACLE_POLL_INTERVAL_MS
    /// - ORACLE_RETRY_DELAY_MS
    /// - ORACLE_MAX_RETRIES
    /// - ORACLE_VERBOSE_LOGGING
    /// - ORACLE_FEEDS: SYMBOL=0xFeedAddr,SYMBOL2=0xFeedAddr2
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = env::var("ORACLE_POLL_INTERVAL_MS") {
            if let Ok(parsed) = v.trim().parse::<u64>() {
                self.poll_interval_ms = parsed;
            }
        }

        if let Ok(v) = env::var("ORACLE_RETRY_DELAY_MS") {
            if let Ok(parsed) = v.trim().parse::<u64>() {
                self.retry_delay_ms = parsed;
            }
        }

        if let Ok(v) = env::var("ORACLE_MAX_RETRIES") {
            if let Ok(parsed) = v.trim().parse::<u32>() {
                self.max_retries = parsed;
            }
        }

        if let Ok(v) = env::var("ORACLE_VERBOSE_LOGGING") {
            let normalized = v.trim().to_ascii_lowercase();
            if matches!(normalized.as_str(), "1" | "true" | "yes" | "on") {
                self.verbose_logging = true;
            } else if matches!(normalized.as_str(), "0" | "false" | "no" | "off") {
                self.verbose_logging = false;
            }
        }

        if let Ok(raw_feeds) = env::var("ORACLE_FEEDS") {
            for entry in raw_feeds.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                let Some((symbol_raw, addr_raw)) = entry.split_once('=') else {
                    continue;
                };

                let symbol = symbol_raw.trim().to_ascii_uppercase();
                let Ok(feed_address) = addr_raw.trim().parse::<Address>() else {
                    continue;
                };

                if let Some(existing) = self
                    .feeds
                    .iter_mut()
                    .find(|f| f.asset_id.eq_ignore_ascii_case(&symbol))
                {
                    existing.feed_address = feed_address;
                } else {
                    self.feeds.push(PriceFeedConfig {
                        asset_symbol: symbol.clone(),
                        asset_id: symbol,
                        feed_address,
                        decimals: 8,
                        heartbeat_secs: self.default_staleness_secs,
                        deviation_threshold_pct: self.default_deviation_pct,
                        is_stablecoin: false,
                    });
                }
            }
        }
    }

    /// Thêm custom price feed
    pub fn add_feed(&mut self, feed: PriceFeedConfig) {
        self.feeds.push(feed);
    }

    /// Lấy feed config theo asset_id
    pub fn get_feed(&self, asset_id: &str) -> Option<&PriceFeedConfig> {
        self.feeds.iter().find(|f| f.asset_id == asset_id)
    }

    /// Lấy tất cả feed addresses
    pub fn feed_addresses(&self) -> HashMap<String, Address> {
        self.feeds.iter()
            .map(|f| (f.asset_id.clone(), f.feed_address))
            .collect()
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = OracleConfig::default();
        assert_eq!(config.poll_interval_ms, 3000);
        assert_eq!(config.default_deviation_pct, 0.5);
        assert_eq!(config.default_staleness_secs, 3600);
        assert_eq!(config.max_retries, 3);
        assert!(config.feeds.is_empty(), "Default config không nên có feeds");
    }

    #[test]
    fn test_mainnet_config_has_6_feeds() {
        let config = OracleConfig::mainnet();
        assert_eq!(config.feeds.len(), 6, "Mainnet config phải có 6 feeds");
        assert_eq!(config.poll_interval_ms, 12000, "Mainnet poll mỗi 12s (1 block)");
        assert!(!config.verbose_logging);
    }

    #[test]
    fn test_mainnet_feed_assets() {
        let config = OracleConfig::mainnet();
        let asset_ids: Vec<&str> = config.feeds.iter().map(|f| f.asset_id.as_str()).collect();
        assert!(asset_ids.contains(&"ETH"), "Phải có ETH feed");
        assert!(asset_ids.contains(&"WBTC"), "Phải có WBTC feed");
        assert!(asset_ids.contains(&"USDC"), "Phải có USDC feed");
        assert!(asset_ids.contains(&"DAI"), "Phải có DAI feed");
        assert!(asset_ids.contains(&"LINK"), "Phải có LINK feed");
        assert!(asset_ids.contains(&"AAVE"), "Phải có AAVE feed");
    }

    #[test]
    fn test_mainnet_eth_feed_config() {
        let config = OracleConfig::mainnet();
        let eth = config.get_feed("ETH").expect("ETH feed phải tồn tại");
        assert_eq!(eth.decimals, 8);
        assert_eq!(eth.heartbeat_secs, 3600);
        assert_eq!(eth.deviation_threshold_pct, 0.5);
        assert!(!eth.is_stablecoin);
    }

    #[test]
    fn test_mainnet_stablecoin_config() {
        let config = OracleConfig::mainnet();
        let usdc = config.get_feed("USDC").expect("USDC feed phải tồn tại");
        assert!(usdc.is_stablecoin, "USDC phải là stablecoin");
        assert_eq!(usdc.deviation_threshold_pct, 0.1, "Stablecoin threshold thấp hơn");
        assert_eq!(usdc.heartbeat_secs, 86400, "USDC heartbeat dài hơn (24h)");
    }

    #[test]
    fn test_local_fork_overrides() {
        let config = OracleConfig::local_fork();
        assert_eq!(config.poll_interval_ms, 2000, "Local fork poll nhanh hơn");
        assert_eq!(config.retry_delay_ms, 500, "Local fork retry nhanh hơn");
        assert!(config.verbose_logging, "Local fork nên bật verbose");
        // Vẫn có đủ 6 feeds từ mainnet
        assert_eq!(config.feeds.len(), 6);
    }

    #[test]
    fn test_add_feed() {
        let mut config = OracleConfig::default();
        assert_eq!(config.feeds.len(), 0);

        config.add_feed(PriceFeedConfig {
            asset_symbol: "TEST".to_string(),
            asset_id: "TEST".to_string(),
            feed_address: Address::zero(),
            decimals: 8,
            heartbeat_secs: 3600,
            deviation_threshold_pct: 1.0,
            is_stablecoin: false,
        });

        assert_eq!(config.feeds.len(), 1);
        assert_eq!(config.feeds[0].asset_id, "TEST");
    }

    #[test]
    fn test_get_feed_found() {
        let config = OracleConfig::mainnet();
        let eth = config.get_feed("ETH");
        assert!(eth.is_some());
        assert_eq!(eth.unwrap().asset_symbol, "ETH");
    }

    #[test]
    fn test_get_feed_not_found() {
        let config = OracleConfig::mainnet();
        let nonexistent = config.get_feed("SHIB");
        assert!(nonexistent.is_none(), "SHIB không có trong mainnet config");
    }

    #[test]
    fn test_feed_addresses_map() {
        let config = OracleConfig::mainnet();
        let addresses = config.feed_addresses();
        assert_eq!(addresses.len(), 6);
        assert!(addresses.contains_key("ETH"));
        assert!(addresses.contains_key("WBTC"));
        // Verify address not zero
        assert_ne!(*addresses.get("ETH").unwrap(), Address::zero());
    }
}
