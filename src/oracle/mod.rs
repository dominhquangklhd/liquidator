// Oracle Module
//
// Theo dõi giá từ Chainlink Price Feed contracts realtime:
// - Kết nối Chainlink AggregatorV3Interface contracts
// - Polling giá định kỳ (mỗi block hoặc configurable interval)
// - Phát Event::PriceUpdate khi giá thay đổi đáng kể (deviation detection)
// - Hỗ trợ multiple asset pairs (ETH/USD, WBTC/USD, USDC/USD, etc.)
// - Fallback mechanism khi feed lỗi (dùng cached price)
// - Staleness detection (cảnh báo khi giá quá cũ)

pub mod config;
pub mod types;
pub mod chainlink;
pub mod manager;
pub mod worker;

// Re-exports
pub use config::{OracleConfig, PriceFeedConfig};
pub use types::{PriceData, PriceFeedInfo, FeedStatus, OracleStats};
pub use chainlink::ChainlinkFeed;
pub use manager::OracleManager;
pub use worker::{OracleWorkerConfig, oracle_price_worker, oracle_stats_worker, oracle_health_worker};
