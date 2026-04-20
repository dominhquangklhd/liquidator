// Gas Estimator
//
// Ước lượng gas cost cho liquidation transactions:
// - Đọc gas price hiện tại từ RPC (legacy hoặc EIP-1559)
// - Tính gas cost bằng ETH
// - Chuyển đổi gas cost sang USD (dùng ETH price từ Oracle)
// - Hỗ trợ nhiều mức gas limit cho các execution path

use ethers::providers::{Provider, Http, Middleware};
use anyhow::Result;
use std::sync::Arc;

use super::types::GasCostEstimate;

/// Gas Estimator
///
/// Đọc gas price on-chain và tính chi phí USD cho transaction
pub struct GasEstimator {
    /// RPC provider
    provider: Arc<Provider<Http>>,
}

impl GasEstimator {
    /// Tạo GasEstimator mới
    pub fn new(provider: Arc<Provider<Http>>) -> Self {
        Self { provider }
    }
    
    /// Ước lượng gas cost cho liquidation transaction
    ///
    /// # Arguments
    /// * `gas_limit` - Gas limit (units)
    /// * `eth_price_usd` - Giá ETH hiện tại (USD)
    ///
    /// # Returns
    /// `GasCostEstimate` với giá gas hiện tại chuyển sang USD
    pub async fn estimate_liquidation_cost(
        &self,
        gas_limit: u64,
        eth_price_usd: f64,
    ) -> Result<GasCostEstimate> {
        let gas_price = self.provider.get_gas_price().await?;
        let gas_price_gwei = gas_price.as_u64() as f64 / 1e9;
        
        Ok(GasCostEstimate::calculate(gas_price_gwei, gas_limit, eth_price_usd))
    }
    
    /// Ước lượng gas cost với EIP-1559 fee
    ///
    /// Đọc base_fee từ latest block + priority fee estimate
    pub async fn estimate_eip1559_cost(
        &self,
        gas_limit: u64,
        priority_fee_gwei: f64,
        eth_price_usd: f64,
    ) -> Result<GasCostEstimate> {
        // Đọc base fee từ latest block
        let block = self.provider
            .get_block(ethers::types::BlockNumber::Latest)
            .await?;
        
        let base_fee_gwei = match block {
            Some(b) => {
                b.base_fee_per_gas
                    .map(|f| f.as_u64() as f64 / 1e9)
                    .unwrap_or(30.0) // Fallback: 30 Gwei
            }
            None => 30.0,
        };
        
        Ok(GasCostEstimate::calculate_eip1559(
            base_fee_gwei, priority_fee_gwei, gas_limit, eth_price_usd
        ))
    }
    
    /// Lấy gas price hiện tại (Gwei)
    pub async fn current_gas_price_gwei(&self) -> Result<f64> {
        let gas_price = self.provider.get_gas_price().await?;
        Ok(gas_price.as_u64() as f64 / 1e9)
    }
    
    /// Kiểm tra gas price có vượt max không
    pub async fn is_gas_acceptable(&self, max_gwei: f64) -> Result<bool> {
        let current = self.current_gas_price_gwei().await?;
        Ok(current <= max_gwei)
    }
}

// ============================================================================
// UNIT TESTS  
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gas_cost_calculation_sanity() {
        // Verify GasCostEstimate calculation without RPC
        let est = GasCostEstimate::calculate(30.0, 500_000, 2000.0);
        
        // 30 Gwei × 500,000 gas = 15,000,000 Gwei = 0.015 ETH
        assert!((est.cost_eth - 0.015).abs() < 1e-10);
        
        // 0.015 ETH × $2000 = $30
        assert!((est.cost_usd - 30.0).abs() < 0.001);
    }
    
    #[test]
    fn test_gas_cost_high_gas_price() {
        // 100 Gwei — gas war scenario
        let est = GasCostEstimate::calculate(100.0, 500_000, 2000.0);
        // 0.05 ETH × $2000 = $100
        assert!((est.cost_usd - 100.0).abs() < 0.01);
    }
    
    #[test]
    fn test_gas_cost_low_gas_price() {
        // 5 Gwei — quiet network
        let est = GasCostEstimate::calculate(5.0, 500_000, 2000.0);
        // 0.0025 ETH × $2000 = $5
        assert!((est.cost_usd - 5.0).abs() < 0.01);
    }
    
    #[test]
    fn test_gas_cost_higher_gas_limit() {
        // Higher gas limit should increase cost
        let standard = GasCostEstimate::calculate(30.0, 500_000, 2000.0);
        let higher_limit = GasCostEstimate::calculate(30.0, 800_000, 2000.0);
        
        assert!(higher_limit.cost_usd > standard.cost_usd);
        assert!((higher_limit.cost_usd - 48.0).abs() < 0.01); // 30 * 800k / 1e9 * 2000 = $48
    }
}
