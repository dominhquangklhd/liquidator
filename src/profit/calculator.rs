// Profit Calculator
//
// Core logic tính toán lợi nhuận cho mỗi liquidation opportunity:
//
// Công thức:
//   debt_to_cover = min(total_debt × close_factor, max_liquidatable)
//   collateral_received = debt_to_cover × (1 + bonus%)
//   gross_profit = collateral_received - debt_to_cover = debt_to_cover × bonus%
//   gas_cost = gas_price × gas_limit → USD
//   slippage = collateral_received × (base_slippage% + size_impact%)
//   net_profit = gross_profit - gas_cost - slippage
//
// Chọn cặp collateral/debt tối ưu:
//   Score = gross_profit × (1 - slippage%) - gas_cost
//   Chọn cặp có score cao nhất

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use anyhow::Result;

use super::config::ProfitConfig;
use super::types::{
    ProfitEstimate, ProfitBreakdown, LiquidationPair, GasCostEstimate,
};
use super::gas::GasEstimator;
use crate::storage::LiquidationTarget;
use crate::oracle::types::PriceData;

/// Profit Calculator
///
/// Tính toán lợi nhuận ước tính cho mỗi thanh lý tiềm năng.
/// Thread-safe — có thể chia sẻ giữa nhiều worker.
pub struct ProfitCalculator {
    /// Cấu hình
    config: ProfitConfig,
    
    /// Gas estimator
    gas_estimator: GasEstimator,
    
    /// Oracle price cache (shared read-only từ OracleManager)
    price_cache: Arc<RwLock<HashMap<String, PriceData>>>,
    
    /// Thống kê
    stats: Arc<RwLock<ProfitStats>>,
}

/// Thống kê Profit Calculator
#[derive(Debug, Clone, Default)]
pub struct ProfitStats {
    /// Tổng số đánh giá
    pub total_evaluations: u64,
    
    /// Số cơ hội profitable
    pub profitable_count: u64,
    
    /// Số cơ hội unprofitable
    pub unprofitable_count: u64,
    
    /// Tổng estimated profit (USD)
    pub total_estimated_profit: f64,
    
    /// Estimated profit cao nhất (USD)
    pub max_profit_seen: f64,
    
    /// Gas cost trung bình (USD)
    pub avg_gas_cost_usd: f64,
    
    /// Tổng gas cost (để tính avg)
    total_gas_cost: f64,
}

impl ProfitCalculator {
    /// Tạo ProfitCalculator mới
    pub fn new(
        config: ProfitConfig,
        gas_estimator: GasEstimator,
        price_cache: Arc<RwLock<HashMap<String, PriceData>>>,
    ) -> Self {
        Self {
            config,
            gas_estimator,
            price_cache,
            stats: Arc::new(RwLock::new(ProfitStats::default())),
        }
    }
    
    /// Đánh giá lợi nhuận cho một liquidation target
    ///
    /// Đây là hàm chính — được gọi bởi Executor trước khi execute.
    ///
    /// # Flow:
    /// 1. Kiểm tra HF < 1.0
    /// 2. Tìm tất cả cặp collateral/debt khả thi
    /// 3. Chọn cặp tối ưu (score cao nhất)
    /// 4. Tính gross profit, gas cost, slippage
    /// 5. Tính net profit → quyết định có đáng execute không
    pub async fn evaluate(&self, target: &LiquidationTarget) -> Result<ProfitEstimate> {
        // ── Bước 0: Tăng stats ──
        {
            let mut stats = self.stats.write().await;
            stats.total_evaluations += 1;
        }
        
        // ── Bước 1: Kiểm tra HF ──
        if target.health_factor >= 1.0 {
            let mut stats = self.stats.write().await;
            stats.unprofitable_count += 1;
            
            return Ok(ProfitEstimate::unprofitable(
                target.user_address.clone(),
                format!("HF = {:.4} >= 1.0, không thể thanh lý", target.health_factor),
            ));
        }
        
        // ── Bước 2: Lấy giá hiện tại từ Oracle cache ──
        let prices = self.get_current_prices().await;
        
        // ── Bước 3: Tìm tất cả cặp collateral/debt khả thi ──
        let pairs = self.find_liquidation_pairs(target, &prices);
        
        if pairs.is_empty() {
            return Ok(ProfitEstimate::unprofitable(
                target.user_address.clone(),
                "Không tìm thấy cặp collateral/debt hợp lệ".to_string(),
            ));
        }
        
        // ── Bước 4: Chọn cặp tối ưu ──
        let best_pair = pairs.into_iter()
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
            .unwrap();
        
        // ── Bước 5: Tính chi tiết profit ──
        let estimate = self.calculate_profit(target, best_pair).await?;
        
        // ── Bước 6: Update stats ──
        {
            let mut stats = self.stats.write().await;
            if estimate.is_profitable {
                stats.profitable_count += 1;
                stats.total_estimated_profit += estimate.net_profit_usd;
                if estimate.net_profit_usd > stats.max_profit_seen {
                    stats.max_profit_seen = estimate.net_profit_usd;
                }
            } else {
                stats.unprofitable_count += 1;
            }
            stats.total_gas_cost += estimate.gas_cost_usd;
            if stats.total_evaluations > 0 {
                stats.avg_gas_cost_usd = stats.total_gas_cost / stats.total_evaluations as f64;
            }
        }
        
        // ── Log ──
        if self.config.verbose {
            tracing::info!("{}", estimate.summary());
            if estimate.is_profitable {
                tracing::debug!("{}", estimate.breakdown.display());
            }
        }
        
        Ok(estimate)
    }
    
    /// Đánh giá nhiều targets cùng lúc, sắp xếp theo profit giảm dần
    pub async fn evaluate_batch(
        &self, 
        targets: &[LiquidationTarget]
    ) -> Result<Vec<ProfitEstimate>> {
        let mut estimates = Vec::new();
        
        for target in targets {
            match self.evaluate(target).await {
                Ok(est) => estimates.push(est),
                Err(e) => {
                    tracing::warn!("Failed to evaluate {}: {:?}", target.user_address, e);
                    estimates.push(ProfitEstimate::unprofitable(
                        target.user_address.clone(),
                        format!("Evaluation error: {}", e),
                    ));
                }
            }
        }
        
        // Sort by net profit giảm dần
        estimates.sort_by(|a, b| b.net_profit_usd.partial_cmp(&a.net_profit_usd).unwrap());
        
        Ok(estimates)
    }
    
    /// Lấy chỉ các estimates profitable
    pub async fn find_profitable(
        &self, 
        targets: &[LiquidationTarget]
    ) -> Result<Vec<ProfitEstimate>> {
        let all = self.evaluate_batch(targets).await?;
        Ok(all.into_iter().filter(|e| e.is_profitable).collect())
    }
    
    // ========================================================================
    // INTERNAL METHODS
    // ========================================================================
    
    /// Lấy giá hiện tại từ Oracle cache
    async fn get_current_prices(&self) -> HashMap<String, f64> {
        let cache = self.price_cache.read().await;
        cache.iter()
            .map(|(k, v)| (k.clone(), v.price_usd))
            .collect()
    }
    
    /// Tìm tất cả cặp collateral/debt khả thi
    fn find_liquidation_pairs(
        &self,
        target: &LiquidationTarget,
        prices: &HashMap<String, f64>,
    ) -> Vec<LiquidationPair> {
        let mut pairs = Vec::new();
        
        for (col_asset, col_amount) in &target.collateral {
            for (debt_asset, debt_amount) in &target.debt {
                // Lấy giá — bỏ qua nếu không có giá
                let col_price = match prices.get(col_asset) {
                    Some(&p) if p > 0.0 => p,
                    _ => {
                        // Fallback: dùng giá từ target nếu có
                        if target.total_collateral_usd > 0.0 && *col_amount > 0.0 {
                            target.total_collateral_usd / *col_amount
                        } else {
                            continue;
                        }
                    }
                };
                
                let debt_price = match prices.get(debt_asset) {
                    Some(&p) if p > 0.0 => p,
                    _ => {
                        if self.config.is_stablecoin(debt_asset) {
                            1.0 // Stablecoin fallback
                        } else if target.total_debt_usd > 0.0 && *debt_amount > 0.0 {
                            target.total_debt_usd / *debt_amount
                        } else {
                            continue;
                        }
                    }
                };
                
                let bonus = self.config.get_bonus(col_asset);
                
                // Tính score: ước lượng nhanh gross profit
                let debt_value = debt_amount * debt_price;
                let debt_to_cover = debt_value * self.config.close_factor;
                let estimated_gross = debt_to_cover * bonus / 100.0;
                
                // Score = gross profit ước lượng (để ranking)
                let score = estimated_gross;
                
                pairs.push(LiquidationPair {
                    collateral_asset: col_asset.clone(),
                    debt_asset: debt_asset.clone(),
                    bonus_pct: bonus,
                    collateral_price_usd: col_price,
                    debt_price_usd: debt_price,
                    collateral_amount: *col_amount,
                    debt_amount: *debt_amount,
                    score,
                });
            }
        }
        
        pairs
    }
    
    /// Tính chi tiết profit cho một cặp
    async fn calculate_profit(
        &self,
        target: &LiquidationTarget,
        pair: LiquidationPair,
    ) -> Result<ProfitEstimate> {
        // ── 1. Tính debt to cover ──
        let debt_value_usd = pair.debt_amount * pair.debt_price_usd;
        let debt_to_cover_usd = debt_value_usd * self.config.close_factor;
        
        // ── 2. Tính collateral received (bao gồm bonus) ──
        let bonus_multiplier = 1.0 + pair.bonus_pct / 100.0;
        let collateral_received_usd = debt_to_cover_usd * bonus_multiplier;
        
        // Verify collateral đủ
        let available_collateral_usd = pair.collateral_amount * pair.collateral_price_usd;
        let actual_collateral_received = collateral_received_usd.min(available_collateral_usd);
        
        // Nếu collateral không đủ → giảm debt_to_cover
        let actual_debt_to_cover = if collateral_received_usd > available_collateral_usd {
            available_collateral_usd / bonus_multiplier
        } else {
            debt_to_cover_usd
        };
        
        // ── 3. Gross profit = bonus phần ──
        let bonus_usd = actual_debt_to_cover * pair.bonus_pct / 100.0;
        let gross_profit = bonus_usd;
        
        // ── 4. Gas cost ──
        let eth_price = self.get_eth_price().await;
        let gas_cost = self.gas_estimator
            .estimate_liquidation_cost(self.config.gas_limit, eth_price)
            .await
            .unwrap_or_else(|_| {
                // Fallback: estimate dựa trên giá mặc định
                GasCostEstimate::calculate(
                    self.config.fallback_gas_price_gwei,
                    self.config.gas_limit,
                    eth_price,
                )
            });
        
        // ── 5. Slippage ──
        let base_slippage = self.config.get_slippage(&pair.collateral_asset);
        let size_impact = (actual_debt_to_cover / 10_000.0) * self.config.size_impact_pct_per_10k;
        let total_slippage_pct = base_slippage + size_impact;
        let slippage_usd = actual_collateral_received * total_slippage_pct / 100.0;
        
        // ── 6. Net profit ──
        let total_cost = gas_cost.cost_usd + slippage_usd;
        let net_profit = gross_profit - total_cost;
        
        // ── 7. ROI ──
        let roi = if gas_cost.cost_usd > 0.0 {
            net_profit / gas_cost.cost_usd * 100.0
        } else {
            if net_profit > 0.0 { f64::INFINITY } else { 0.0 }
        };
        
        // ── 8. Quyết định ──
        let (is_profitable, reject_reason) = self.check_profitability(net_profit, roi, &gas_cost);
        
        // ── 9. Build breakdown ──
        let breakdown = ProfitBreakdown {
            debt_covered_usd: actual_debt_to_cover,
            collateral_base_usd: actual_debt_to_cover, // phần = debt
            bonus_usd,
            gas: gas_cost.clone(),
            slippage_pct: base_slippage,
            slippage_usd,
            size_impact_pct: size_impact,
            total_cost_usd: total_cost,
            gross_profit_usd: gross_profit,
            net_profit_usd: net_profit,
        };
        
        Ok(ProfitEstimate {
            user_address: target.user_address.clone(),
            pair,
            debt_to_cover_usd: actual_debt_to_cover,
            collateral_received_usd: actual_collateral_received,
            gross_profit_usd: gross_profit,
            gas_cost_usd: gas_cost.cost_usd,
            slippage_cost_usd: slippage_usd,
            net_profit_usd: net_profit,
            roi_pct: roi,
            is_profitable,
            reject_reason,
            breakdown,
        })
    }
    
    /// Kiểm tra profitability dựa trên thresholds
    fn check_profitability(
        &self,
        net_profit: f64,
        roi: f64,
        _gas: &GasCostEstimate,
    ) -> (bool, Option<String>) {
        if net_profit <= 0.0 {
            return (false, Some(format!(
                "Net profit ${:.2} <= 0 (negative)", net_profit
            )));
        }
        
        if net_profit < self.config.min_profit_usd {
            return (false, Some(format!(
                "Net profit ${:.2} < min ${:.2}", 
                net_profit, self.config.min_profit_usd
            )));
        }
        
        if roi < self.config.min_roi_pct {
            return (false, Some(format!(
                "ROI {:.0}% < min {:.0}%", roi, self.config.min_roi_pct
            )));
        }
        
        (true, None)
    }
    
    /// Lấy giá ETH/USD từ Oracle cache
    async fn get_eth_price(&self) -> f64 {
        let cache = self.price_cache.read().await;
        cache.get("ETH")
            .map(|p| p.price_usd)
            .unwrap_or(self.config.fallback_eth_price_usd) // Fallback estimate
    }
    
    // ========================================================================
    // PUBLIC API
    // ========================================================================
    
    /// Lấy thống kê
    pub async fn get_stats(&self) -> ProfitStats {
        self.stats.read().await.clone()
    }
    
    /// Config reference
    pub fn config(&self) -> &ProfitConfig {
        &self.config
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::Address;
    use ethers::providers::Http;
    use crate::oracle::types::PriceData;
    
    /// Helper: tạo mock price cache
    fn mock_price_cache() -> Arc<RwLock<HashMap<String, PriceData>>> {
        let mut cache = HashMap::new();
        
        cache.insert("ETH".to_string(), PriceData {
            asset_id: "ETH".to_string(),
            price_usd: 2000.0,
            price_raw: 200_000_000_000,
            decimals: 8,
            round_id: 1,
            updated_at: chrono::Utc::now().timestamp() as u64,
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: Address::zero(),
        });
        
        cache.insert("USDC".to_string(), PriceData {
            asset_id: "USDC".to_string(),
            price_usd: 1.0,
            price_raw: 100_000_000,
            decimals: 8,
            round_id: 1,
            updated_at: chrono::Utc::now().timestamp() as u64,
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: Address::zero(),
        });
        
        cache.insert("WBTC".to_string(), PriceData {
            asset_id: "WBTC".to_string(),
            price_usd: 60000.0,
            price_raw: 6_000_000_000_000,
            decimals: 8,
            round_id: 1,
            updated_at: chrono::Utc::now().timestamp() as u64,
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: Address::zero(),
        });
        
        Arc::new(RwLock::new(cache))
    }
    
    /// Helper: tạo LiquidationTarget mẫu
    fn mock_target_liquidatable() -> LiquidationTarget {
        let mut target = LiquidationTarget::new("0xRisky".to_string());
        target.health_factor = 0.95;
        target.total_collateral_usd = 20_000.0; // 10 ETH × $2000
        target.total_debt_usd = 16_000.0;       // 16000 USDC
        target.collateral.insert("ETH".to_string(), 10.0);
        target.debt.insert("USDC".to_string(), 16_000.0);
        target
    }
    
    fn mock_target_safe() -> LiquidationTarget {
        let mut target = LiquidationTarget::new("0xSafe".to_string());
        target.health_factor = 2.5;
        target.total_collateral_usd = 20_000.0;
        target.total_debt_usd = 5_000.0;
        target.collateral.insert("ETH".to_string(), 10.0);
        target.debt.insert("USDC".to_string(), 5_000.0);
        target
    }
    
    #[test]
    fn test_find_pairs() {
        let config = ProfitConfig::default();
        let prices: HashMap<String, f64> = vec![
            ("ETH".to_string(), 2000.0),
            ("USDC".to_string(), 1.0),
        ].into_iter().collect();
        
        let target = mock_target_liquidatable();
        
        // Use a temporary runtime for the test
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let pairs = calc.find_liquidation_pairs(&target, &prices);
        
        assert_eq!(pairs.len(), 1, "Phải có 1 cặp ETH/USDC");
        assert_eq!(pairs[0].collateral_asset, "ETH");
        assert_eq!(pairs[0].debt_asset, "USDC");
        assert_eq!(pairs[0].bonus_pct, 5.0);
        assert!(pairs[0].score > 0.0, "Score phải > 0");
    }
    
    #[test]
    fn test_find_pairs_multiple_positions() {
        let config = ProfitConfig::default();
        let prices: HashMap<String, f64> = vec![
            ("ETH".to_string(), 2000.0),
            ("WBTC".to_string(), 60000.0),
            ("USDC".to_string(), 1.0),
        ].into_iter().collect();
        
        let mut target = mock_target_liquidatable();
        target.collateral.insert("WBTC".to_string(), 0.5); // + 0.5 WBTC
        
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let pairs = calc.find_liquidation_pairs(&target, &prices);
        
        // 2 collateral × 1 debt = 2 cặp
        assert_eq!(pairs.len(), 2);
        
        // WBTC có bonus 6.5% > ETH 5.0% → score cao hơn
        let wbtc_pair = pairs.iter().find(|p| p.collateral_asset == "WBTC").unwrap();
        let eth_pair = pairs.iter().find(|p| p.collateral_asset == "ETH").unwrap();
        
        // Score cùng debt_to_cover nhưng WBTC bonus cao hơn
        assert!(wbtc_pair.bonus_pct > eth_pair.bonus_pct);
    }
    
    #[tokio::test]
    async fn test_evaluate_safe_user() {
        let config = ProfitConfig::local_fork();
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let target = mock_target_safe();
        let result = calc.evaluate(&target).await.unwrap();
        
        assert!(!result.is_profitable, "Safe user không nên profitable");
        assert!(result.reject_reason.unwrap().contains("HF"));
    }
    
    #[tokio::test]
    async fn test_evaluate_liquidatable_user() {
        let config = ProfitConfig::local_fork();
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let target = mock_target_liquidatable();
        let result = calc.evaluate(&target).await.unwrap();
        
        println!("Result: {}", result.summary());
        println!("  Gross: ${:.2}", result.gross_profit_usd);
        println!("  Gas:   ${:.2}", result.gas_cost_usd);
        println!("  Slip:  ${:.2}", result.slippage_cost_usd);
        println!("  Net:   ${:.2}", result.net_profit_usd);
        println!("  ROI:   {:.0}%", result.roi_pct);
        
        // debt_to_cover = 16000 × 0.5 = $8000
        assert!((result.debt_to_cover_usd - 8000.0).abs() < 0.01);
        
        // gross = $8000 × 5% = $400
        assert!((result.gross_profit_usd - 400.0).abs() < 0.01);
        
        // Net = $400 - gas - slippage > 0 (trên local fork gas rẻ)
        // Gas cost sẽ fallback vì provider không kết nối
        assert!(result.gross_profit_usd > 0.0);
    }
    
    #[tokio::test]
    async fn test_evaluate_batch() {
        let config = ProfitConfig::local_fork();
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let targets = vec![
            mock_target_safe(),
            mock_target_liquidatable(),
        ];
        
        let results = calc.evaluate_batch(&targets).await.unwrap();
        assert_eq!(results.len(), 2);
        
        // Sorted by net profit desc → liquidatable first (positive), safe second (0)
        assert!(results[0].net_profit_usd >= results[1].net_profit_usd);
    }
    
    #[tokio::test]
    async fn test_stats_tracking() {
        let config = ProfitConfig::local_fork();
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        // Evaluate 2 targets
        let _ = calc.evaluate(&mock_target_safe()).await;
        let _ = calc.evaluate(&mock_target_liquidatable()).await;
        
        let stats = calc.get_stats().await;
        assert_eq!(stats.total_evaluations, 2);
        assert_eq!(stats.unprofitable_count, 1); // safe user
    }
    
    #[test]
    fn test_profitability_check() {
        let config = ProfitConfig::default(); // min_profit=10, min_roi=100%
        let price_cache = mock_price_cache();
        let provider = Arc::new(
            ethers::providers::Provider::<Http>::try_from("http://localhost:8545").unwrap()
        );
        let gas_est = GasEstimator::new(provider);
        let calc = ProfitCalculator::new(config, gas_est, price_cache);
        
        let gas = GasCostEstimate::calculate(30.0, 500_000, 2000.0);
        
        // Net profit $400, ROI = 400/30*100 = 1333% → profitable
        let (ok, _) = calc.check_profitability(400.0, 1333.0, &gas);
        assert!(ok);
        
        // Net profit $5 < min $10 → not profitable
        let (ok, reason) = calc.check_profitability(5.0, 16.6, &gas);
        assert!(!ok);
        assert!(reason.unwrap().contains("min"));
        
        // Net profit negative → not profitable
        let (ok, reason) = calc.check_profitability(-10.0, -33.0, &gas);
        assert!(!ok);
        assert!(reason.unwrap().contains("negative"));
    }
}
