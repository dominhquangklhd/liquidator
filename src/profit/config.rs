// Profit Calculator Configuration
//
// Cấu hình cho module tính toán lợi nhuận thanh lý:
// - Liquidation bonus rates cho từng asset
// - Gas cost parameters
// - Slippage estimation
// - Profit thresholds

use std::collections::HashMap;

/// Cấu hình cho Profit Calculator
#[derive(Debug, Clone)]
pub struct ProfitConfig {
    /// Liquidation bonus % cho từng asset (e.g., "ETH" -> 5.0 = 5%)
    /// Aave V3: thường 5-10% tùy asset
    pub liquidation_bonus: HashMap<String, f64>,
    
    /// Default liquidation bonus nếu asset không có trong map (%)
    pub default_bonus_pct: f64,
    
    /// Close factor — tỷ lệ tối đa debt được phép thanh lý (thường 50%)
    pub close_factor: f64,
    
    /// Gas limit cho liquidation transaction
    pub gas_limit: u64,
    
    /// Slippage tolerance mặc định (%)
    pub default_slippage_pct: f64,
    
    /// Slippage tolerance cho stablecoins (%)
    pub stablecoin_slippage_pct: f64,
    
    /// Slippage tăng theo position size (% per $10k)
    /// Ví dụ: 0.1 = thêm 0.1% slippage cho mỗi $10k debt
    pub size_impact_pct_per_10k: f64,
    
    /// Minimum net profit (USD) để coi là đáng execute
    pub min_profit_usd: f64,
    
    /// Minimum ROI (%) — net profit / gas cost
    /// Ví dụ: 200.0 = profit phải >= 2x gas cost
    pub min_roi_pct: f64,
    
    /// Danh sách stablecoins (để áp dụng slippage thấp hơn)
    pub stablecoins: Vec<String>,
    
    /// Verbose logging
    pub verbose: bool,

    /// Fallback gas price (Gwei) used when live estimation fails.
    pub fallback_gas_price_gwei: f64,

    /// Fallback ETH/USD price used when oracle cache misses.
    pub fallback_eth_price_usd: f64,
}

impl Default for ProfitConfig {
    fn default() -> Self {
        let mut bonus = HashMap::new();
        // Aave V3 Mainnet liquidation bonus (typical values)
        bonus.insert("ETH".to_string(), 5.0);
        bonus.insert("WETH".to_string(), 5.0);
        bonus.insert("WBTC".to_string(), 6.5);
        bonus.insert("USDC".to_string(), 4.5);
        bonus.insert("USDT".to_string(), 4.5);
        bonus.insert("DAI".to_string(), 4.0);
        bonus.insert("LINK".to_string(), 7.0);
        bonus.insert("AAVE".to_string(), 7.5);
        bonus.insert("UNI".to_string(), 7.5);
        bonus.insert("WSTETH".to_string(), 7.0);
        
        Self {
            liquidation_bonus: bonus,
            default_bonus_pct: 5.0,
            close_factor: 0.5,        // 50% max close
            gas_limit: 500_000,
            default_slippage_pct: 0.5,
            stablecoin_slippage_pct: 0.1,
            size_impact_pct_per_10k: 0.1,
            min_profit_usd: 10.0,
            min_roi_pct: 100.0,       // Profit >= 1x gas cost
            stablecoins: vec![
                "USDC".to_string(),
                "USDT".to_string(),
                "DAI".to_string(),
                "FRAX".to_string(),
                "LUSD".to_string(),
            ],
            verbose: false,
            fallback_gas_price_gwei: 30.0,
            fallback_eth_price_usd: 2000.0,
        }
    }
}

impl ProfitConfig {
    /// Config cho mainnet production
    pub fn mainnet() -> Self {
        Self {
            min_profit_usd: 50.0,    // Mainnet: threshold cao hơn
            min_roi_pct: 200.0,       // Profit >= 2x gas
            verbose: false,
            ..Default::default()
        }
    }
    
    /// Config cho local fork testing
    pub fn local_fork() -> Self {
        Self {
            min_profit_usd: 1.0,     // Test: threshold thấp
            min_roi_pct: 50.0,
            verbose: true,
            ..Default::default()
        }
    }
    
    /// Lấy liquidation bonus cho asset (%)
    pub fn get_bonus(&self, asset_id: &str) -> f64 {
        self.liquidation_bonus
            .get(asset_id)
            .copied()
            .unwrap_or(self.default_bonus_pct)
    }
    
    /// Kiểm tra asset có phải stablecoin không
    pub fn is_stablecoin(&self, asset_id: &str) -> bool {
        self.stablecoins.iter().any(|s| s == asset_id)
    }
    
    /// Lấy slippage estimate cho asset
    pub fn get_slippage(&self, asset_id: &str) -> f64 {
        if self.is_stablecoin(asset_id) {
            self.stablecoin_slippage_pct
        } else {
            self.default_slippage_pct
        }
    }
}