// Oracle Types
//
// Kiểu dữ liệu cho Oracle module:
// - PriceData: Giá hiện tại của một asset
// - PriceFeedStatus: Trạng thái của một price feed
// - PriceUpdate: Thông tin thay đổi giá

use ethers::types::Address;

/// Dữ liệu giá từ Chainlink price feed
#[derive(Debug, Clone)]
pub struct PriceData {
    /// Asset symbol (e.g., "ETH", "WBTC")
    pub asset_id: String,
    
    /// Giá hiện tại (USD, đã chia decimals)
    pub price_usd: f64,
    
    /// Giá raw từ contract (chưa chia decimals)
    pub price_raw: i128,
    
    /// Số decimals của price feed
    pub decimals: u8,
    
    /// Round ID từ Chainlink
    pub round_id: u128,
    
    /// Timestamp lần update cuối từ Chainlink (unix seconds)
    pub updated_at: u64,
    
    /// Timestamp khi bot đọc giá này (unix seconds)
    pub fetched_at: i64,
    
    /// Địa chỉ price feed contract
    pub feed_address: Address,
}

impl PriceData {
    /// Kiểm tra giá có stale không (quá cũ)
    pub fn is_stale(&self, max_age_secs: u64) -> bool {
        let now = chrono::Utc::now().timestamp() as u64;
        // So sánh với updated_at từ Chainlink
        now.saturating_sub(self.updated_at) > max_age_secs
    }

    /// Tính % thay đổi so với giá trước đó
    pub fn deviation_pct(&self, previous_price: f64) -> f64 {
        if previous_price == 0.0 {
            return 100.0; // First price = 100% change
        }
        ((self.price_usd - previous_price) / previous_price * 100.0).abs()
    }
}

/// Trạng thái của một price feed
#[derive(Debug, Clone, PartialEq)]
pub enum FeedStatus {
    /// Feed hoạt động bình thường
    Active,
    
    /// Feed stale — giá quá cũ, có thể không chính xác
    Stale,
    
    /// Feed lỗi — không thể đọc giá
    Error(String),
    
    /// Feed chưa được khởi tạo
    Uninitialized,
}

impl std::fmt::Display for FeedStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedStatus::Active => write!(f, "Active"),
            FeedStatus::Stale => write!(f, "Stale"),
            FeedStatus::Error(e) => write!(f, "Error: {}", e),
            FeedStatus::Uninitialized => write!(f, "Uninitialized"),
        }
    }
}

/// Thông tin tổng hợp về một price feed
#[derive(Debug, Clone)]
pub struct PriceFeedInfo {
    /// Asset ID
    pub asset_id: String,
    
    /// Asset symbol
    pub asset_symbol: String,
    
    /// Địa chỉ feed contract
    pub feed_address: Address,
    
    /// Trạng thái hiện tại
    pub status: FeedStatus,
    
    /// Giá hiện tại (nếu có)
    pub current_price: Option<PriceData>,
    
    /// Giá trước đó (để tính deviation)
    pub previous_price: Option<f64>,
    
    /// Số lần đọc thành công
    pub success_count: u64,
    
    /// Số lần đọc thất bại
    pub error_count: u64,
    
    /// Số lần phát hiện deviation đáng kể
    pub deviation_events: u64,
}

impl PriceFeedInfo {
    pub fn new(asset_id: String, asset_symbol: String, feed_address: Address) -> Self {
        Self {
            asset_id,
            asset_symbol,
            feed_address,
            status: FeedStatus::Uninitialized,
            current_price: None,
            previous_price: None,
            success_count: 0,
            error_count: 0,
            deviation_events: 0,
        }
    }
}

/// Summary thống kê cho tất cả oracle feeds
#[derive(Debug, Clone, Default)]
pub struct OracleStats {
    /// Tổng số lần poll
    pub total_polls: u64,
    
    /// Tổng số price updates đã emit (qua Event channel)
    pub total_updates_emitted: u64,
    
    /// Tổng số lỗi RPC
    pub total_errors: u64,
    
    /// Số feeds đang active
    pub active_feeds: usize,
    
    /// Số feeds đang stale
    pub stale_feeds: usize,
    
    /// Số feeds bị lỗi
    pub error_feeds: usize,
}

// ============================================================================
// UNIT TESTS
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Address;

    fn sample_price_data(price_usd: f64, updated_at: u64) -> PriceData {
        PriceData {
            asset_id: "ETH".to_string(),
            price_usd,
            price_raw: (price_usd * 1e8) as i128,
            decimals: 8,
            round_id: 100,
            updated_at,
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: Address::zero(),
        }
    }

    // ── PriceData tests ──────────────────────────────────────────

    #[test]
    fn test_price_data_is_stale() {
        // Giá cập nhật 2 giờ trước, threshold 1 giờ → stale
        let two_hours_ago = chrono::Utc::now().timestamp() as u64 - 7200;
        let price = sample_price_data(2500.0, two_hours_ago);
        assert!(price.is_stale(3600), "Giá cập nhật 2h trước phải stale với threshold 1h");
    }

    #[test]
    fn test_price_data_not_stale() {
        // Giá cập nhật 10 giây trước, threshold 1 giờ → fresh
        let ten_secs_ago = chrono::Utc::now().timestamp() as u64 - 10;
        let price = sample_price_data(2500.0, ten_secs_ago);
        assert!(!price.is_stale(3600), "Giá cập nhật 10s trước không nên stale");
    }

    #[test]
    fn test_price_data_stale_exact_boundary() {
        // Giá cập nhật đúng threshold → không stale (saturating_sub > threshold, not >=)
        let exactly_threshold = chrono::Utc::now().timestamp() as u64 - 3600;
        let price = sample_price_data(2500.0, exactly_threshold);
        // At exact boundary, now - updated_at == 3600, > 3600 is false
        assert!(!price.is_stale(3600), "Giá ở biên chính xác không nên stale");
    }

    #[test]
    fn test_deviation_pct_price_increase() {
        let price = sample_price_data(2750.0, 0);
        // 2500 → 2750 = +10%
        let deviation = price.deviation_pct(2500.0);
        assert!((deviation - 10.0).abs() < 0.01, "Deviation phải là 10%, got {}", deviation);
    }

    #[test]
    fn test_deviation_pct_price_decrease() {
        let price = sample_price_data(2250.0, 0);
        // 2500 → 2250 = -10% → abs = 10%
        let deviation = price.deviation_pct(2500.0);
        assert!((deviation - 10.0).abs() < 0.01, "Deviation phải là 10%, got {}", deviation);
    }

    #[test]
    fn test_deviation_pct_no_change() {
        let price = sample_price_data(2500.0, 0);
        let deviation = price.deviation_pct(2500.0);
        assert!((deviation - 0.0).abs() < 0.001, "Deviation phải là 0%, got {}", deviation);
    }

    #[test]
    fn test_deviation_pct_zero_previous() {
        let price = sample_price_data(2500.0, 0);
        let deviation = price.deviation_pct(0.0);
        assert_eq!(deviation, 100.0, "Previous price = 0 → deviation phải 100%");
    }

    #[test]
    fn test_deviation_pct_small_change() {
        let price = sample_price_data(2501.25, 0);
        // 2500 → 2501.25 = 0.05%
        let deviation = price.deviation_pct(2500.0);
        assert!((deviation - 0.05).abs() < 0.001, "Deviation phải ~ 0.05%, got {}", deviation);
    }

    // ── FeedStatus tests ─────────────────────────────────────────

    #[test]
    fn test_feed_status_display() {
        assert_eq!(format!("{}", FeedStatus::Active), "Active");
        assert_eq!(format!("{}", FeedStatus::Stale), "Stale");
        assert_eq!(format!("{}", FeedStatus::Uninitialized), "Uninitialized");
        assert_eq!(
            format!("{}", FeedStatus::Error("RPC timeout".to_string())),
            "Error: RPC timeout"
        );
    }

    #[test]
    fn test_feed_status_equality() {
        assert_eq!(FeedStatus::Active, FeedStatus::Active);
        assert_eq!(FeedStatus::Stale, FeedStatus::Stale);
        assert_ne!(FeedStatus::Active, FeedStatus::Stale);
        assert_eq!(
            FeedStatus::Error("test".to_string()),
            FeedStatus::Error("test".to_string())
        );
        assert_ne!(
            FeedStatus::Error("a".to_string()),
            FeedStatus::Error("b".to_string())
        );
    }

    // ── PriceFeedInfo tests ──────────────────────────────────────

    #[test]
    fn test_price_feed_info_new() {
        let info = PriceFeedInfo::new(
            "ETH".to_string(),
            "ETH".to_string(),
            Address::zero(),
        );
        assert_eq!(info.asset_id, "ETH");
        assert_eq!(info.status, FeedStatus::Uninitialized);
        assert!(info.current_price.is_none());
        assert!(info.previous_price.is_none());
        assert_eq!(info.success_count, 0);
        assert_eq!(info.error_count, 0);
        assert_eq!(info.deviation_events, 0);
    }

    // ── OracleStats tests ────────────────────────────────────────

    #[test]
    fn test_oracle_stats_default() {
        let stats = OracleStats::default();
        assert_eq!(stats.total_polls, 0);
        assert_eq!(stats.total_updates_emitted, 0);
        assert_eq!(stats.total_errors, 0);
        assert_eq!(stats.active_feeds, 0);
        assert_eq!(stats.stale_feeds, 0);
        assert_eq!(stats.error_feeds, 0);
    }
}
