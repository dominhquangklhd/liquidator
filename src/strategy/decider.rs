// Strategy Decider
//
// Core logic quyết định chiến lược thanh lý:
//
// 1. Direct vs Flash Loan:
//    - Kiểm tra wallet balance → đủ token? → Direct
//    - Debt quá lớn hoặc không đủ vốn? → Flash Loan
//    - Flash loan không sẵn sàng? → Direct (nếu đủ) hoặc Skip
//
// 2. Target Prioritization (multi-factor scoring):
//    Score = w_profit × normalize(profit) 
//          + w_urgency × normalize(1/HF)
//          + w_efficiency × normalize(ROI)
//          + w_size × normalize(1/debt_size)
//
// 3. Risk Management:
//    - Circuit breaker: dừng sau N thất bại liên tiếp
//    - Exposure limit: giới hạn tổng debt đang cover
//    - Gas price check: đợi nếu gas quá cao

use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

use super::config::StrategyConfig;
use super::types::{
    ExecutionMethod, StrategyDecision, PrioritizedTarget, ExecutionPlan,
};
use crate::profit::ProfitEstimate;
use crate::storage::LiquidationTarget;

/// Strategy Decider
///
/// Quyết định cách tối ưu nhất để thực hiện mỗi liquidation.
/// Thread-safe — có thể chia sẻ giữa nhiều worker.
pub struct StrategyDecider {
    /// Cấu hình
    config: StrategyConfig,
    
    /// Wallet balance (ETH) — updated bởi external worker
    wallet_balance_eth: Arc<RwLock<f64>>,
    
    /// Số USD debt token có sẵn trong ví (approximate)
    /// Key = asset symbol, Value = amount USD
    wallet_token_balances: Arc<RwLock<std::collections::HashMap<String, f64>>>,
    
    /// Trạng thái circuit breaker
    circuit_breaker: Arc<RwLock<CircuitBreaker>>,
    
    /// Gas price hiện tại (gwei) — updated bởi external worker
    current_gas_price_gwei: Arc<RwLock<f64>>,
    
    /// Thống kê
    stats: Arc<RwLock<StrategyStats>>,
}

/// Circuit breaker state
#[derive(Debug, Clone)]
struct CircuitBreaker {
    /// Số thất bại liên tiếp
    consecutive_failures: u32,
    
    /// Đang trong trạng thái cooldown?
    is_tripped: bool,
    
    /// Thời điểm bắt đầu cooldown (Unix timestamp)
    tripped_at: Option<i64>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            is_tripped: false,
            tripped_at: None,
        }
    }
}

/// Thống kê Strategy Decider
#[derive(Debug, Clone, Default)]
pub struct StrategyStats {
    /// Tổng số decisions
    pub total_decisions: u64,
    
    /// Số lần chọn Direct
    pub direct_count: u64,
    
    /// Số lần chọn Flash Loan
    pub flash_loan_count: u64,
    
    /// Số lần Skip
    pub skip_count: u64,
    
    /// Tổng plans đã tạo
    pub total_plans: u64,
    
    /// Circuit breaker trips
    pub circuit_breaker_trips: u64,
}

impl StrategyDecider {
    /// Tạo StrategyDecider mới
    pub fn new(config: StrategyConfig) -> Self {
        Self {
            config,
            wallet_balance_eth: Arc::new(RwLock::new(10.0)), // Default 10 ETH
            wallet_token_balances: Arc::new(RwLock::new(std::collections::HashMap::new())),
            circuit_breaker: Arc::new(RwLock::new(CircuitBreaker::default())),
            current_gas_price_gwei: Arc::new(RwLock::new(0.0)), // 0 = unknown/ok
            stats: Arc::new(RwLock::new(StrategyStats::default())),
        }
    }
    
    // ========================================================================
    // PUBLIC API
    // ========================================================================
    
    /// Tạo execution plan cho một batch targets + estimates
    ///
    /// Đây là hàm chính — nhận danh sách (target, estimate) pairs,
    /// quyết định method và priority cho mỗi target, trả về ExecutionPlan.
    pub async fn create_plan(
        &self,
        targets_with_estimates: Vec<(LiquidationTarget, ProfitEstimate)>,
    ) -> Result<ExecutionPlan> {
        let total_input = targets_with_estimates.len();
        
        // ── Check circuit breaker ──
        if self.is_circuit_breaker_active().await {
            tracing::warn!("Circuit breaker active — skipping all targets");
            return Ok(ExecutionPlan::from_targets(vec![], total_input));
        }
        
        // ── Lấy context hiện tại ──
        let wallet_eth = *self.wallet_balance_eth.read().await;
        let token_balances = self.wallet_token_balances.read().await.clone();
        let current_gas = *self.current_gas_price_gwei.read().await;
        let gas_price_ok = current_gas <= 0.0 || current_gas <= self.config.max_gas_price_gwei;
        
        // ── Decide method + calculate priority cho mỗi target ──
        let mut prioritized: Vec<PrioritizedTarget> = Vec::new();
        let mut current_exposure = 0.0_f64;
        
        // Tính min/max cho normalization
        let profits: Vec<f64> = targets_with_estimates.iter()
            .map(|(_, e)| e.net_profit_usd)
            .collect();
        let rois: Vec<f64> = targets_with_estimates.iter()
            .map(|(_, e)| e.roi_pct)
            .collect();
        let hfs: Vec<f64> = targets_with_estimates.iter()
            .map(|(t, _)| t.health_factor)
            .collect();
        let debts: Vec<f64> = targets_with_estimates.iter()
            .map(|(_, e)| e.debt_to_cover_usd)
            .collect();
        
        let norm_profit = Normalizer::from_values(&profits);
        let norm_roi = Normalizer::from_values(&rois);
        let norm_urgency = Normalizer::from_values_inverse(&hfs);
        let norm_size = Normalizer::from_values_inverse(&debts);
        
        for (target, estimate) in targets_with_estimates {
            let mut stats = self.stats.write().await;
            stats.total_decisions += 1;
            drop(stats);
            
            // Skip nếu không profitable
            if !estimate.is_profitable {
                let decision = StrategyDecision {
                    user_address: target.user_address.clone(),
                    method: ExecutionMethod::Skip {
                        reason: estimate.reject_reason.clone()
                            .unwrap_or_else(|| "Not profitable".to_string()),
                    },
                    priority_score: 0.0,
                    adjusted_profit_usd: 0.0,
                    reasoning: "Unprofitable".to_string(),
                    profit_estimate: estimate.clone(),
                };
                
                let mut stats = self.stats.write().await;
                stats.skip_count += 1;
                drop(stats);
                
                prioritized.push(PrioritizedTarget {
                    target,
                    estimate,
                    decision,
                    rank: 0,
                });
                continue;
            }
            
            // Check gas price
            if !gas_price_ok {
                let decision = StrategyDecision {
                    user_address: target.user_address.clone(),
                    method: ExecutionMethod::Skip {
                        reason: "Gas price too high".to_string(),
                    },
                    priority_score: 0.0,
                    adjusted_profit_usd: 0.0,
                    reasoning: "Gas price exceeds limit".to_string(),
                    profit_estimate: estimate.clone(),
                };
                
                let mut stats = self.stats.write().await;
                stats.skip_count += 1;
                drop(stats);
                
                prioritized.push(PrioritizedTarget {
                    target,
                    estimate,
                    decision,
                    rank: 0,
                });
                continue;
            }
            
            // Check exposure limits
            if current_exposure + estimate.debt_to_cover_usd > self.config.max_total_exposure_usd {
                let decision = StrategyDecision {
                    user_address: target.user_address.clone(),
                    method: ExecutionMethod::Skip {
                        reason: format!(
                            "Exposure limit: ${:.0} + ${:.0} > max ${:.0}",
                            current_exposure, estimate.debt_to_cover_usd,
                            self.config.max_total_exposure_usd
                        ),
                    },
                    priority_score: 0.0,
                    adjusted_profit_usd: 0.0,
                    reasoning: "Exposure limit exceeded".to_string(),
                    profit_estimate: estimate.clone(),
                };
                
                let mut stats = self.stats.write().await;
                stats.skip_count += 1;
                drop(stats);
                
                prioritized.push(PrioritizedTarget {
                    target,
                    estimate,
                    decision,
                    rank: 0,
                });
                continue;
            }
            
            if estimate.debt_to_cover_usd > self.config.max_single_exposure_usd {
                let decision = StrategyDecision {
                    user_address: target.user_address.clone(),
                    method: ExecutionMethod::Skip {
                        reason: format!(
                            "Single exposure ${:.0} > max ${:.0}",
                            estimate.debt_to_cover_usd,
                            self.config.max_single_exposure_usd
                        ),
                    },
                    priority_score: 0.0,
                    adjusted_profit_usd: 0.0,
                    reasoning: "Single exposure too large".to_string(),
                    profit_estimate: estimate.clone(),
                };
                
                let mut stats = self.stats.write().await;
                stats.skip_count += 1;
                drop(stats);
                
                prioritized.push(PrioritizedTarget {
                    target,
                    estimate,
                    decision,
                    rank: 0,
                });
                continue;
            }
            
            // ── Chọn execution method ──
            let (method, adjusted_profit, reasoning) = self.decide_method(
                &estimate, wallet_eth, &token_balances,
            );
            
            // ── Tính priority score ──
            let (wp, wu, we, ws) = self.config.normalized_weights();
            let priority_score = 
                wp * norm_profit.normalize(estimate.net_profit_usd)
                + wu * norm_urgency.normalize(target.health_factor) 
                + we * norm_roi.normalize(estimate.roi_pct)
                + ws * norm_size.normalize(estimate.debt_to_cover_usd);
            
            // Scale to 0-10
            let priority_score = priority_score * 10.0;
            
            // Update stats
            {
                let mut stats = self.stats.write().await;
                match &method {
                    ExecutionMethod::Direct { .. } => stats.direct_count += 1,
                    ExecutionMethod::FlashLoan { .. } => stats.flash_loan_count += 1,
                    ExecutionMethod::Skip { .. } => stats.skip_count += 1,
                }
            }
            
            current_exposure += estimate.debt_to_cover_usd;
            
            let decision = StrategyDecision {
                user_address: target.user_address.clone(),
                method,
                priority_score,
                adjusted_profit_usd: adjusted_profit,
                reasoning,
                profit_estimate: estimate.clone(),
            };
            
            prioritized.push(PrioritizedTarget {
                target,
                estimate,
                decision,
                rank: 0,
            });
        }
        
        // ── Sort by priority score desc ──
        prioritized.sort_by(|a, b| {
            b.decision.priority_score
                .partial_cmp(&a.decision.priority_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        
        // ── Assign ranks ──
        for (i, pt) in prioritized.iter_mut().enumerate() {
            pt.rank = i + 1;
        }
        
        // ── Limit concurrent ──
        let max = self.config.max_concurrent_liquidations;
        for pt in prioritized.iter_mut().skip(max) {
            if pt.decision.should_execute() {
                pt.decision.method = ExecutionMethod::Skip {
                    reason: format!("Concurrent limit: rank {} > max {}", pt.rank, max),
                };
                pt.decision.adjusted_profit_usd = 0.0;
            }
        }
        
        // ── Build plan ──
        {
            let mut stats = self.stats.write().await;
            stats.total_plans += 1;
        }
        
        let plan = ExecutionPlan::from_targets(prioritized, total_input);
        
        tracing::info!("{}", plan.summary());
        
        Ok(plan)
    }
    
    /// Quyết định method cho một target đơn lẻ (convenience)
    pub async fn decide_single(
        &self,
        target: &LiquidationTarget,
        estimate: &ProfitEstimate,
    ) -> StrategyDecision {
        let wallet_eth = *self.wallet_balance_eth.read().await;
        let token_balances = self.wallet_token_balances.read().await.clone();
        
        let (method, adjusted_profit, reasoning) = self.decide_method(
            estimate, wallet_eth, &token_balances,
        );
        
        StrategyDecision {
            user_address: target.user_address.clone(),
            method,
            priority_score: 5.0, // Default cho single
            adjusted_profit_usd: adjusted_profit,
            reasoning,
            profit_estimate: estimate.clone(),
        }
    }
    
    // ========================================================================
    // INTERNAL: Method Decision
    // ========================================================================
    
    /// Logic chọn Direct vs Flash Loan
    ///
    /// Decision tree:
    /// 1. wallet_eth < min_balance → Flash Loan (nếu có) | Skip
    /// 2. debt_to_cover > direct_max → Flash Loan (nếu có) | trả trực tiếp nếu đủ token
    /// 3. Có đủ debt token trong ví → Direct
    /// 4. Không đủ token, flash loan available → Flash Loan
    /// 5. Không đủ token, flash loan unavailable → Skip
    fn decide_method(
        &self,
        estimate: &ProfitEstimate,
        wallet_eth: f64,
        token_balances: &std::collections::HashMap<String, f64>,
    ) -> (ExecutionMethod, f64, String) {
        let debt_asset = &estimate.pair.debt_asset;
        let debt_to_cover_usd = estimate.debt_to_cover_usd;
        
        // Check 1: Wallet balance đủ ETH cho gas?
        if wallet_eth < self.config.min_wallet_balance_eth {
            if self.config.flash_loan_available {
                let fee = debt_to_cover_usd * self.config.flash_loan_fee_pct / 100.0;
                let adjusted = estimate.net_profit_usd - fee;
                return (
                    ExecutionMethod::FlashLoan {
                        gas_limit: self.config.flash_loan_gas_limit,
                        fee_usd: fee,
                    },
                    adjusted,
                    format!("Flash loan: ETH balance {:.4} < min {:.4}", 
                        wallet_eth, self.config.min_wallet_balance_eth),
                );
            } else {
                return (
                    ExecutionMethod::Skip {
                        reason: format!("ETH balance {:.4} < min {:.4}, flash loan unavailable",
                            wallet_eth, self.config.min_wallet_balance_eth),
                    },
                    0.0,
                    "Insufficient ETH, no flash loan".to_string(),
                );
            }
        }
        
        // Check 2: Có đủ debt token trong ví?
        let has_enough_token = token_balances
            .get(debt_asset)
            .map(|&balance| balance >= debt_to_cover_usd)
            .unwrap_or(false);
        
        // Check 3: Debt quá lớn cho direct?
        let debt_too_large = debt_to_cover_usd > self.config.direct_max_debt_usd;
        
        if has_enough_token && !debt_too_large {
            // Direct: đủ token và size hợp lý
            return (
                ExecutionMethod::Direct {
                    gas_limit: self.config.direct_gas_limit,
                },
                estimate.net_profit_usd,
                format!("Direct: đủ {} token, debt ${:.0} <= max ${:.0}",
                    debt_asset, debt_to_cover_usd, self.config.direct_max_debt_usd),
            );
        }
        
        if has_enough_token && debt_too_large {
            // Có token nhưng debt lớn → vẫn direct nếu đủ, vì tiết kiệm flash loan fee
            return (
                ExecutionMethod::Direct {
                    gas_limit: self.config.direct_gas_limit,
                },
                estimate.net_profit_usd,
                format!("Direct: đủ {} token (debt lớn ${:.0} nhưng có sẵn token)",
                    debt_asset, debt_to_cover_usd),
            );
        }
        
        // Không đủ token → cần flash loan
        if self.config.flash_loan_available {
            let fee = debt_to_cover_usd * self.config.flash_loan_fee_pct / 100.0;
            let adjusted = estimate.net_profit_usd - fee;
            
            if adjusted <= 0.0 {
                return (
                    ExecutionMethod::Skip {
                        reason: format!(
                            "Flash loan fee ${:.2} > net profit ${:.2}",
                            fee, estimate.net_profit_usd
                        ),
                    },
                    0.0,
                    "Flash loan fee exceeds profit".to_string(),
                );
            }
            
            return (
                ExecutionMethod::FlashLoan {
                    gas_limit: self.config.flash_loan_gas_limit,
                    fee_usd: fee,
                },
                adjusted,
                format!("Flash loan: không đủ {} token, fee ${:.2}", debt_asset, fee),
            );
        }
        
        // Không đủ token và không có flash loan → skip
        (
            ExecutionMethod::Skip {
                reason: format!("Không đủ {} token, flash loan unavailable", debt_asset),
            },
            0.0,
            format!("No {} token, no flash loan", debt_asset),
        )
    }
    
    // ========================================================================
    // CIRCUIT BREAKER
    // ========================================================================
    
    /// Kiểm tra circuit breaker có đang active
    async fn is_circuit_breaker_active(&self) -> bool {
        let cb = self.circuit_breaker.read().await;
        
        if !cb.is_tripped {
            return false;
        }
        
        // Check if cooldown đã hết
        if let Some(tripped_at) = cb.tripped_at {
            let now = chrono::Utc::now().timestamp();
            let elapsed = (now - tripped_at) as u64;
            
            if elapsed >= self.config.circuit_breaker_cooldown_secs {
                // Cooldown xong — sẽ reset ở lần ghi tiếp theo
                drop(cb);
                self.reset_circuit_breaker().await;
                return false;
            }
        }
        
        true
    }
    
    /// Báo cáo liquidation thành công → reset consecutive failures
    pub async fn report_success(&self) {
        let mut cb = self.circuit_breaker.write().await;
        cb.consecutive_failures = 0;
        // Nếu đang tripped, không reset ở đây — đợi cooldown
    }
    
    /// Báo cáo liquidation thất bại → tăng counter, có thể trip breaker
    pub async fn report_failure(&self) {
        let mut cb = self.circuit_breaker.write().await;
        cb.consecutive_failures += 1;
        
        if cb.consecutive_failures >= self.config.circuit_breaker_threshold {
            cb.is_tripped = true;
            cb.tripped_at = Some(chrono::Utc::now().timestamp());
            
            let mut stats = self.stats.write().await;
            stats.circuit_breaker_trips += 1;
            
            tracing::warn!(
                "⚠️ Circuit breaker TRIPPED after {} failures! Cooldown: {}s",
                cb.consecutive_failures,
                self.config.circuit_breaker_cooldown_secs
            );
        }
    }
    
    /// Reset circuit breaker
    async fn reset_circuit_breaker(&self) {
        let mut cb = self.circuit_breaker.write().await;
        cb.consecutive_failures = 0;
        cb.is_tripped = false;
        cb.tripped_at = None;
        tracing::info!("Circuit breaker reset — resuming operations");
    }
    
    // ========================================================================
    // WALLET STATE
    // ========================================================================
    
    /// Cập nhật ETH balance
    pub async fn update_wallet_balance(&self, balance_eth: f64) {
        let mut bal = self.wallet_balance_eth.write().await;
        *bal = balance_eth;
    }
    
    /// Cập nhật token balance
    pub async fn update_token_balance(&self, asset: String, balance_usd: f64) {
        let mut balances = self.wallet_token_balances.write().await;
        balances.insert(asset, balance_usd);
    }
    
    /// Cập nhật gas price hiện tại (gwei)
    pub async fn update_gas_price(&self, gas_price_gwei: f64) {
        let mut gas = self.current_gas_price_gwei.write().await;
        *gas = gas_price_gwei;
    }
    
    /// Lấy stats
    pub async fn get_stats(&self) -> StrategyStats {
        self.stats.read().await.clone()
    }
    
    /// Config reference
    pub fn config(&self) -> &StrategyConfig {
        &self.config
    }
}

// ============================================================================
// NORMALIZER — chuẩn hóa giá trị về [0, 1] cho scoring
// ============================================================================

/// Min-max normalizer cho priority scoring
struct Normalizer {
    min: f64,
    max: f64,
    inverse: bool,
}

impl Normalizer {
    /// Từ danh sách values — normalize thường (cao = tốt)
    fn from_values(values: &[f64]) -> Self {
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Self { min, max, inverse: false }
    }
    
    /// Từ danh sách values — normalize ngược (thấp = tốt)
    /// Dùng cho HF (HF thấp = urgent hơn) và debt size (nhỏ = ít competition)
    fn from_values_inverse(values: &[f64]) -> Self {
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Self { min, max, inverse: true }
    }
    
    /// Normalize value về [0, 1]
    fn normalize(&self, value: f64) -> f64 {
        if (self.max - self.min).abs() < 1e-10 {
            return 0.5; // Tất cả giống nhau
        }
        
        let normalized = (value - self.min) / (self.max - self.min);
        
        if self.inverse {
            1.0 - normalized // Đảo ngược: thấp → 1.0, cao → 0.0
        } else {
            normalized
        }
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::profit::{ProfitEstimate, ProfitBreakdown, LiquidationPair, GasCostEstimate};
    
    /// Helper: tạo ProfitEstimate mẫu
    fn mock_estimate(user: &str, profit: f64, debt: f64, debt_asset: &str) -> ProfitEstimate {
        ProfitEstimate {
            user_address: user.to_string(),
            pair: LiquidationPair {
                collateral_asset: "ETH".to_string(),
                debt_asset: debt_asset.to_string(),
                bonus_pct: 5.0,
                collateral_price_usd: 2000.0,
                debt_price_usd: 1.0,
                collateral_amount: 10.0,
                debt_amount: debt,
                score: profit,
            },
            debt_to_cover_usd: debt * 0.5,
            collateral_received_usd: debt * 0.5 * 1.05,
            gross_profit_usd: profit + 30.0, // gross > net
            gas_cost_usd: 30.0,
            slippage_cost_usd: 0.0,
            flash_loan_fee_usd: 0.0,
            net_profit_usd: profit,
            roi_pct: if profit > 0.0 { profit / 30.0 * 100.0 } else { 0.0 },
            is_profitable: profit > 0.0,
            reject_reason: if profit <= 0.0 { Some("Not profitable".to_string()) } else { None },
            breakdown: ProfitBreakdown::default(),
        }
    }
    
    /// Helper: tạo LiquidationTarget mẫu
    fn mock_target(user: &str, hf: f64, debt_usd: f64) -> LiquidationTarget {
        let mut target = LiquidationTarget::new(user.to_string());
        target.health_factor = hf;
        target.total_debt_usd = debt_usd;
        target.total_collateral_usd = debt_usd * 1.5;
        target.collateral.insert("ETH".to_string(), debt_usd * 1.5 / 2000.0);
        target.debt.insert("USDC".to_string(), debt_usd);
        target
    }
    
    // ── Normalizer tests ──
    
    #[test]
    fn test_normalizer_normal() {
        let norm = Normalizer::from_values(&[10.0, 20.0, 30.0]);
        assert!((norm.normalize(10.0) - 0.0).abs() < 1e-10);
        assert!((norm.normalize(20.0) - 0.5).abs() < 1e-10);
        assert!((norm.normalize(30.0) - 1.0).abs() < 1e-10);
    }
    
    #[test]
    fn test_normalizer_inverse() {
        let norm = Normalizer::from_values_inverse(&[0.8, 0.9, 1.0]);
        // HF 0.8 (thấp nhất, urgent nhất) → 1.0
        assert!((norm.normalize(0.8) - 1.0).abs() < 1e-10);
        // HF 1.0 (cao nhất, ít urgent) → 0.0
        assert!((norm.normalize(1.0) - 0.0).abs() < 1e-10);
    }
    
    #[test]
    fn test_normalizer_single_value() {
        let norm = Normalizer::from_values(&[5.0]);
        // Chỉ 1 giá trị → trả 0.5
        assert!((norm.normalize(5.0) - 0.5).abs() < 1e-10);
    }
    
    // ── Method decision tests ──
    
    #[tokio::test]
    async fn test_decide_direct_enough_tokens() {
        let config = StrategyConfig::local_fork();
        let decider = StrategyDecider::new(config);
        
        // Set wallet có đủ USDC
        decider.update_wallet_balance(5.0).await;
        decider.update_token_balance("USDC".to_string(), 10_000.0).await;
        
        let target = mock_target("0xuser1", 0.95, 16_000.0);
        let estimate = mock_estimate("0xuser1", 400.0, 16_000.0, "USDC");
        
        let decision = decider.decide_single(&target, &estimate).await;
        
        assert!(matches!(decision.method, ExecutionMethod::Direct { .. }));
        assert!(decision.should_execute());
        assert!((decision.adjusted_profit_usd - 400.0).abs() < 0.01);
        println!("Decision: {}", decision.summary());
    }
    
    #[tokio::test]
    async fn test_decide_flash_loan_no_tokens() {
        let mut config = StrategyConfig::local_fork();
        config.flash_loan_available = true; // Enable flash loan
        let decider = StrategyDecider::new(config);
        
        // Wallet KHÔNG có USDC
        decider.update_wallet_balance(5.0).await;
        
        let target = mock_target("0xuser2", 0.90, 20_000.0);
        let estimate = mock_estimate("0xuser2", 500.0, 20_000.0, "USDC");
        
        let decision = decider.decide_single(&target, &estimate).await;
        
        assert!(matches!(decision.method, ExecutionMethod::FlashLoan { .. }));
        assert!(decision.should_execute());
        // adjusted_profit = 500 - flash_loan_fee
        assert!(decision.adjusted_profit_usd < 500.0);
        println!("Decision: {}", decision.summary());
    }
    
    #[tokio::test]
    async fn test_decide_skip_no_tokens_no_flash() {
        let config = StrategyConfig::local_fork(); // flash_loan_available = false
        let decider = StrategyDecider::new(config);
        
        // Wallet KHÔNG có USDC, flash loan unavailable
        decider.update_wallet_balance(5.0).await;
        
        let target = mock_target("0xuser3", 0.92, 20_000.0);
        let estimate = mock_estimate("0xuser3", 300.0, 20_000.0, "USDC");
        
        let decision = decider.decide_single(&target, &estimate).await;
        
        assert!(matches!(decision.method, ExecutionMethod::Skip { .. }));
        assert!(!decision.should_execute());
    }
    
    #[tokio::test]
    async fn test_decide_skip_low_eth_balance() {
        let config = StrategyConfig::local_fork(); // min_wallet_balance = 0.1
        let decider = StrategyDecider::new(config);
        
        // ETH balance quá thấp, flash loan unavailable
        decider.update_wallet_balance(0.01).await;
        
        let target = mock_target("0xuser4", 0.88, 10_000.0);
        let estimate = mock_estimate("0xuser4", 200.0, 10_000.0, "USDC");
        
        let decision = decider.decide_single(&target, &estimate).await;
        
        assert!(matches!(decision.method, ExecutionMethod::Skip { .. }));
    }
    
    // ── Plan creation tests ──
    
    #[tokio::test]
    async fn test_create_plan_priority_ordering() {
        let config = StrategyConfig::local_fork();
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 100_000.0).await;
        
        let inputs = vec![
            // Profit thấp, HF cao (ít urgent)
            (mock_target("0xlow", 0.98, 5_000.0), 
             mock_estimate("0xlow", 50.0, 5_000.0, "USDC")),
            // Profit cao, HF thấp (urgent)
            (mock_target("0xhigh", 0.85, 20_000.0), 
             mock_estimate("0xhigh", 800.0, 20_000.0, "USDC")),
            // Profit trung bình
            (mock_target("0xmid", 0.92, 10_000.0), 
             mock_estimate("0xmid", 300.0, 10_000.0, "USDC")),
        ];
        
        let plan = decider.create_plan(inputs).await.unwrap();
        
        assert_eq!(plan.total_input, 3);
        assert_eq!(plan.execute_count, 3);
        assert_eq!(plan.skip_count, 0);
        
        // 0xhigh nên ranked #1 (profit cao + HF thấp)
        assert_eq!(plan.targets[0].decision.user_address, "0xhigh");
        assert_eq!(plan.targets[0].rank, 1);
        
        // 0xlow nên ranked cuối
        assert_eq!(plan.targets[2].decision.user_address, "0xlow");
        assert_eq!(plan.targets[2].rank, 3);
        
        println!("Plan: {}", plan.summary());
        for pt in &plan.targets {
            println!("  #{}: {}", pt.rank, pt.decision.summary());
        }
    }
    
    #[tokio::test]
    async fn test_create_plan_concurrent_limit() {
        let mut config = StrategyConfig::local_fork();
        config.max_concurrent_liquidations = 2; // Chỉ cho 2 concurrent
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 100_000.0).await;
        
        let inputs = vec![
            (mock_target("0xa", 0.90, 10_000.0), mock_estimate("0xa", 400.0, 10_000.0, "USDC")),
            (mock_target("0xb", 0.85, 15_000.0), mock_estimate("0xb", 600.0, 15_000.0, "USDC")),
            (mock_target("0xc", 0.95, 8_000.0), mock_estimate("0xc", 200.0, 8_000.0, "USDC")),
        ];
        
        let plan = decider.create_plan(inputs).await.unwrap();
        
        // Chỉ 2 được execute, 1 bị skip do concurrent limit
        assert_eq!(plan.execute_count, 2);
        assert_eq!(plan.skip_count, 1);
        
        println!("Plan: {}", plan.summary());
    }
    
    #[tokio::test]
    async fn test_create_plan_exposure_limit() {
        let mut config = StrategyConfig::local_fork();
        config.max_single_exposure_usd = 5_000.0; // Max $5k per target
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 100_000.0).await;
        
        let inputs = vec![
            // debt_to_cover = 20000*0.5 = $10k > max $5k → skip
            (mock_target("0xbig", 0.90, 20_000.0), mock_estimate("0xbig", 500.0, 20_000.0, "USDC")),
            // debt_to_cover = 5000*0.5 = $2.5k < max $5k → execute
            (mock_target("0xsmall", 0.90, 5_000.0), mock_estimate("0xsmall", 100.0, 5_000.0, "USDC")),
        ];
        
        let plan = decider.create_plan(inputs).await.unwrap();
        
        assert_eq!(plan.execute_count, 1);
        assert_eq!(plan.skip_count, 1);
    }
    
    // ── Circuit breaker tests ──
    
    #[tokio::test]
    async fn test_circuit_breaker_trips() {
        let mut config = StrategyConfig::local_fork();
        config.circuit_breaker_threshold = 3;
        config.circuit_breaker_cooldown_secs = 60;
        let decider = StrategyDecider::new(config);
        
        // 3 failures → trip
        decider.report_failure().await;
        decider.report_failure().await;
        assert!(!decider.is_circuit_breaker_active().await);
        
        decider.report_failure().await; // 3rd → tripped!
        assert!(decider.is_circuit_breaker_active().await);
        
        // Plan should return empty
        let inputs = vec![
            (mock_target("0x1", 0.90, 10_000.0), mock_estimate("0x1", 400.0, 10_000.0, "USDC")),
        ];
        let plan = decider.create_plan(inputs).await.unwrap();
        assert_eq!(plan.execute_count, 0);
    }
    
    #[tokio::test]
    async fn test_circuit_breaker_reset_on_success() {
        let mut config = StrategyConfig::local_fork();
        config.circuit_breaker_threshold = 3;
        let decider = StrategyDecider::new(config);
        
        decider.report_failure().await;
        decider.report_failure().await; // 2 failures
        decider.report_success().await; // Reset counter
        decider.report_failure().await; // 1 failure (not 3)
        
        assert!(!decider.is_circuit_breaker_active().await);
    }
    
    // ── Stats tests ──
    
    #[tokio::test]
    async fn test_stats_tracking() {
        let config = StrategyConfig::local_fork();
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 50_000.0).await;
        
        let inputs = vec![
            (mock_target("0x1", 0.90, 10_000.0), mock_estimate("0x1", 400.0, 10_000.0, "USDC")),
            (mock_target("0x2", 0.95, 5_000.0), mock_estimate("0x2", -10.0, 5_000.0, "USDC")), // unprofitable
        ];
        
        let _plan = decider.create_plan(inputs).await.unwrap();
        
        let stats = decider.get_stats().await;
        assert_eq!(stats.total_decisions, 2);
        assert_eq!(stats.total_plans, 1);
        assert!(stats.direct_count >= 1 || stats.skip_count >= 1);
    }
    
    // ── U22: Circuit breaker cooldown recovery ──
    
    #[tokio::test]
    async fn test_circuit_breaker_cooldown_recovery() {
        let mut config = StrategyConfig::local_fork();
        config.circuit_breaker_threshold = 2;
        config.circuit_breaker_cooldown_secs = 1; // 1 giây cooldown cho test nhanh
        let decider = StrategyDecider::new(config);
        
        // Trip circuit breaker
        decider.report_failure().await;
        decider.report_failure().await;
        assert!(decider.is_circuit_breaker_active().await);
        
        // Plan nên trả empty khi breaker active
        let inputs = vec![
            (mock_target("0x1", 0.90, 10_000.0), mock_estimate("0x1", 400.0, 10_000.0, "USDC")),
        ];
        let plan = decider.create_plan(inputs).await.unwrap();
        assert_eq!(plan.execute_count, 0, "Should skip all while circuit breaker active");
        
        // Đợi cooldown hết (1 giây)
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        
        // Sau cooldown → breaker tự reset → targets được xử lý
        assert!(!decider.is_circuit_breaker_active().await, "Circuit breaker should reset after cooldown");
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 50_000.0).await;
        
        let inputs2 = vec![
            (mock_target("0x2", 0.90, 10_000.0), mock_estimate("0x2", 400.0, 10_000.0, "USDC")),
        ];
        let plan2 = decider.create_plan(inputs2).await.unwrap();
        assert_eq!(plan2.execute_count, 1, "Should execute after cooldown recovery");
    }
    
    // ── U17: Gas price too high → Skip ──
    
    #[tokio::test]
    async fn test_gas_price_too_high_skips() {
        let mut config = StrategyConfig::local_fork();
        config.max_gas_price_gwei = 50.0; // Max 50 gwei
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 50_000.0).await;
        
        // Set gas price vượt limit
        decider.update_gas_price(100.0).await; // 100 gwei > max 50
        
        let inputs = vec![
            (mock_target("0x1", 0.90, 10_000.0), mock_estimate("0x1", 400.0, 10_000.0, "USDC")),
        ];
        let plan = decider.create_plan(inputs).await.unwrap();
        
        // Target phải bị skip do gas quá cao
        assert_eq!(plan.execute_count, 0, "Should skip when gas price > max");
        assert_eq!(plan.skip_count, 1);
        
        // Giảm gas price xuống dưới limit → target được execute
        decider.update_gas_price(30.0).await; // 30 gwei < max 50
        
        let inputs2 = vec![
            (mock_target("0x2", 0.90, 10_000.0), mock_estimate("0x2", 400.0, 10_000.0, "USDC")),
        ];
        let plan2 = decider.create_plan(inputs2).await.unwrap();
        assert_eq!(plan2.execute_count, 1, "Should execute when gas price drops");
    }
    
    // ── U18: Negative profit → Skip ──
    
    #[tokio::test]
    async fn test_negative_profit_skips() {
        let config = StrategyConfig::local_fork();
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 50_000.0).await;
        
        // Profit âm → estimate.is_profitable = false
        let inputs = vec![
            (mock_target("0xloss", 0.90, 10_000.0), mock_estimate("0xloss", -50.0, 10_000.0, "USDC")),
        ];
        let plan = decider.create_plan(inputs).await.unwrap();
        
        assert_eq!(plan.execute_count, 0, "Should not execute unprofitable target");
        assert_eq!(plan.skip_count, 1);
        assert!(matches!(
            plan.targets[0].decision.method,
            ExecutionMethod::Skip { .. }
        ));
    }
    
    // ── U19: Empty input → empty plan ──
    
    #[tokio::test]
    async fn test_empty_input_returns_empty_plan() {
        let config = StrategyConfig::local_fork();
        let decider = StrategyDecider::new(config);
        
        let plan = decider.create_plan(vec![]).await.unwrap();
        
        assert_eq!(plan.total_input, 0);
        assert_eq!(plan.execute_count, 0);
        assert_eq!(plan.skip_count, 0);
        assert!(plan.targets.is_empty());
    }
    
    // ── U20: Wallet balance update affects decisions ──
    
    #[tokio::test]
    async fn test_wallet_balance_update_affects_decisions() {
        let config = StrategyConfig::local_fork(); // flash_loan_available = false
        let decider = StrategyDecider::new(config);
        
        let target = mock_target("0xuser", 0.90, 10_000.0);
        let estimate = mock_estimate("0xuser", 300.0, 10_000.0, "USDC");
        
        // ETH quá thấp (< min 0.1), no flash loan → Skip
        decider.update_wallet_balance(0.01).await;
        let d1 = decider.decide_single(&target, &estimate).await;
        assert!(matches!(d1.method, ExecutionMethod::Skip { .. }), "Too low ETH should Skip");
        
        // Cập nhật ETH đủ → có thể execute (nếu có token)
        decider.update_wallet_balance(5.0).await;
        decider.update_token_balance("USDC".to_string(), 10_000.0).await;
        let d2 = decider.decide_single(&target, &estimate).await;
        assert!(matches!(d2.method, ExecutionMethod::Direct { .. }), "Enough ETH + token should Direct");
    }
    
    // ── U21: Token balance update affects Direct/FlashLoan ──
    
    #[tokio::test]
    async fn test_token_balance_update_affects_method() {
        let mut config = StrategyConfig::local_fork();
        config.flash_loan_available = true; // Enable flash loan
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(5.0).await;
        
        let target = mock_target("0xuser", 0.90, 10_000.0);
        let estimate = mock_estimate("0xuser", 300.0, 10_000.0, "USDC");
        
        // Không có USDC → Flash Loan
        let d1 = decider.decide_single(&target, &estimate).await;
        assert!(matches!(d1.method, ExecutionMethod::FlashLoan { .. }), "No token → FlashLoan");
        
        // Thêm đủ USDC → Direct
        decider.update_token_balance("USDC".to_string(), 10_000.0).await;
        let d2 = decider.decide_single(&target, &estimate).await;
        assert!(matches!(d2.method, ExecutionMethod::Direct { .. }), "Enough token → Direct");
        
        // Direct nên có profit cao hơn (không mất flash loan fee)
        assert!(d2.adjusted_profit_usd > d1.adjusted_profit_usd, 
            "Direct should have higher profit than FlashLoan");
    }
    
    // ── U23: Per-liquidation exposure limit ──
    
    #[tokio::test]
    async fn test_per_liquidation_exposure_limit() {
        let mut config = StrategyConfig::local_fork();
        config.max_single_exposure_usd = 3_000.0; // Max $3k per target
        let decider = StrategyDecider::new(config);
        
        decider.update_wallet_balance(10.0).await;
        decider.update_token_balance("USDC".to_string(), 100_000.0).await;
        
        let inputs = vec![
            // debt_to_cover = 8000*0.5 = $4k > max $3k → skip
            (mock_target("0xbig", 0.85, 8_000.0), mock_estimate("0xbig", 500.0, 8_000.0, "USDC")),
            // debt_to_cover = 4000*0.5 = $2k < max $3k → execute
            (mock_target("0xok", 0.90, 4_000.0), mock_estimate("0xok", 200.0, 4_000.0, "USDC")),
            // debt_to_cover = 2000*0.5 = $1k < max $3k → execute
            (mock_target("0xsmall", 0.92, 2_000.0), mock_estimate("0xsmall", 100.0, 2_000.0, "USDC")),
        ];
        
        let plan = decider.create_plan(inputs).await.unwrap();
        
        assert_eq!(plan.execute_count, 2, "Only 2 below single exposure limit");
        assert_eq!(plan.skip_count, 1, "1 exceeds single exposure limit");
        
        // Verify the skipped one is 0xbig
        let skipped: Vec<_> = plan.targets.iter()
            .filter(|pt| !pt.decision.should_execute())
            .collect();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].decision.user_address, "0xbig");
    }
}
