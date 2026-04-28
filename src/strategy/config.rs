// Strategy Decider Configuration
//
// Cấu hình cho module quyết định chiến lược thanh lý:
// - Direct vs Skip thresholds
// - Target prioritization weights
// - Risk management limits

/// Cấu hình cho Strategy Decider
#[derive(Debug, Clone)]
pub struct StrategyConfig {
    // ── Direct vs Skip ──
    
    /// Số dư tối thiểu trong ví để dùng direct liquidation (ETH)
    /// Nếu balance < min → skip
    pub min_wallet_balance_eth: f64,
    
    /// Ngưỡng debt value (USD) cho direct liquidation
    /// Vì direct liquidation cần sẵn token trong ví
    pub direct_max_debt_usd: f64,
    
    /// Gas limit cho direct liquidation
    pub direct_gas_limit: u64,
    
    
    // ── Target Prioritization ──
    
    /// Trọng số cho profit (cao = ưu tiên profit cao)
    pub weight_profit: f64,
    
    /// Trọng số cho urgency (HF thấp = urgent hơn)
    pub weight_urgency: f64,
    
    /// Trọng số cho efficiency (ROI cao = hiệu quả hơn)
    pub weight_efficiency: f64,
    
    /// Trọng số cho size (position nhỏ = ít competition)
    pub weight_size: f64,
    
    // ── Risk Management ──
    
    /// Số liquidation tối đa cùng lúc
    pub max_concurrent_liquidations: usize,
    
    /// Tổng exposure tối đa (USD) — tổng debt đang cover cùng lúc
    pub max_total_exposure_usd: f64,
    
    /// Exposure tối đa cho 1 liquidation (USD)
    pub max_single_exposure_usd: f64,
    
    /// Số lần thất bại liên tiếp trước khi tạm dừng (circuit breaker)
    pub circuit_breaker_threshold: u32,
    
    /// Thời gian tạm dừng sau circuit breaker (giây)
    pub circuit_breaker_cooldown_secs: u64,
    
    /// Gas price tối đa (Gwei) — vượt quá thì đợi
    pub max_gas_price_gwei: f64,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            // Direct vs Skip
            min_wallet_balance_eth: 0.5,
            direct_max_debt_usd: 5_000.0,
            direct_gas_limit: 500_000,
            
            // Prioritization weights (tổng = 1.0)
            weight_profit: 0.4,
            weight_urgency: 0.3,
            weight_efficiency: 0.2,
            weight_size: 0.1,
            
            // Risk management
            max_concurrent_liquidations: 3,
            max_total_exposure_usd: 100_000.0,
            max_single_exposure_usd: 50_000.0,
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_secs: 300,
            max_gas_price_gwei: 100.0,
        }
    }
}

impl StrategyConfig {
    /// Preset cho mainnet — conservative
    pub fn mainnet() -> Self {
        Self {
            min_wallet_balance_eth: 1.0,
            direct_max_debt_usd: 10_000.0,
            direct_gas_limit: 500_000,
            
            weight_profit: 0.4,
            weight_urgency: 0.3,
            weight_efficiency: 0.2,
            weight_size: 0.1,
            
            max_concurrent_liquidations: 3,
            max_total_exposure_usd: 200_000.0,
            max_single_exposure_usd: 100_000.0,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_secs: 600,
            max_gas_price_gwei: 50.0,
        }
    }
    
    /// Preset cho local fork — aggressive (testing)
    pub fn local_fork() -> Self {
        Self {
            min_wallet_balance_eth: 0.1,
            direct_max_debt_usd: 250_000.0,
            direct_gas_limit: 500_000,
            
            weight_profit: 0.5,
            weight_urgency: 0.3,
            weight_efficiency: 0.1,
            weight_size: 0.1,
            
            max_concurrent_liquidations: 5,
            max_total_exposure_usd: 1_000_000.0,
            max_single_exposure_usd: 500_000.0,
            circuit_breaker_threshold: 10,
            circuit_breaker_cooldown_secs: 60,
            max_gas_price_gwei: 500.0,
        }
    }
    
    /// Tổng trọng số (để normalize)
    pub fn total_weight(&self) -> f64 {
        self.weight_profit + self.weight_urgency + self.weight_efficiency + self.weight_size
    }
    
    /// Normalized weights
    pub fn normalized_weights(&self) -> (f64, f64, f64, f64) {
        let total = self.total_weight();
        if total == 0.0 {
            return (0.25, 0.25, 0.25, 0.25);
        }
        (
            self.weight_profit / total,
            self.weight_urgency / total,
            self.weight_efficiency / total,
            self.weight_size / total,
        )
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
        let config = StrategyConfig::default();
        assert_eq!(config.direct_gas_limit, 500_000);
        assert_eq!(config.circuit_breaker_threshold, 5);
    }
    
    #[test]
    fn test_mainnet_preset() {
        let config = StrategyConfig::mainnet();
        assert_eq!(config.max_gas_price_gwei, 50.0);
        assert_eq!(config.circuit_breaker_threshold, 3);
    }
    
    #[test]
    fn test_local_fork_preset() {
        let config = StrategyConfig::local_fork();
        assert_eq!(config.max_gas_price_gwei, 500.0);
        assert_eq!(config.max_concurrent_liquidations, 5);
    }
    
    #[test]
    fn test_normalized_weights() {
        let config = StrategyConfig::default();
        let (wp, wu, we, ws) = config.normalized_weights();
        let sum = wp + wu + we + ws;
        assert!((sum - 1.0).abs() < 1e-10, "Weights must sum to 1.0");
    }
    
    #[test]
    fn test_normalized_weights_zero() {
        let mut config = StrategyConfig::default();
        config.weight_profit = 0.0;
        config.weight_urgency = 0.0;
        config.weight_efficiency = 0.0;
        config.weight_size = 0.0;
        let (wp, wu, we, ws) = config.normalized_weights();
        assert_eq!(wp, 0.25);
        assert_eq!(wu, 0.25);
        assert_eq!(we, 0.25);
        assert_eq!(ws, 0.25);
    }
}
