// Strategy Decider Types
//
// Kiểu dữ liệu cho module quyết định chiến lược:
// - ExecutionMethod: Direct vs Skip
// - StrategyDecision: Quyết định cho một target
// - PrioritizedTarget: Target đã được ranked
// - ExecutionPlan: Kế hoạch execute batch

use crate::profit::ProfitEstimate;
use crate::storage::LiquidationTarget;

/// Phương thức thực thi liquidation
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionMethod {
    /// Direct liquidation — bot tự trả debt từ ví
    /// Gas thấp hơn, nhanh hơn, nhưng cần sẵn token
    Direct {
        /// Gas limit cho tx direct
        gas_limit: u64,
    },
    
    /// Bỏ qua — không nên execute
    Skip {
        /// Lý do bỏ qua
        reason: String,
    },
}

impl ExecutionMethod {
    pub fn is_skip(&self) -> bool {
        matches!(self, ExecutionMethod::Skip { .. })
    }
    
    pub fn gas_limit(&self) -> u64 {
        match self {
            ExecutionMethod::Direct { gas_limit } => *gas_limit,
            ExecutionMethod::Skip { .. } => 0,
        }
    }
    
    pub fn label(&self) -> &str {
        match self {
            ExecutionMethod::Direct { .. } => "DIRECT",
            ExecutionMethod::Skip { .. } => "SKIP",
        }
    }
}

impl std::fmt::Display for ExecutionMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMethod::Direct { gas_limit } => {
                write!(f, "Direct (gas: {})", gas_limit)
            }
            ExecutionMethod::Skip { reason } => {
                write!(f, "Skip: {}", reason)
            }
        }
    }
}

/// Quyết định chiến lược cho một target
#[derive(Debug, Clone)]
pub struct StrategyDecision {
    /// Địa chỉ user
    pub user_address: String,
    
    /// Phương thức thực thi được chọn
    pub method: ExecutionMethod,
    
    /// Priority score (cao = execute trước)
    pub priority_score: f64,
    
    /// Net profit ước tính sau khi tính strategy costs
    pub adjusted_profit_usd: f64,
    
    /// Lý do chọn phương thức này
    pub reasoning: String,
    
    /// Profit estimate gốc (từ ProfitCalculator)
    pub profit_estimate: ProfitEstimate,
}

impl StrategyDecision {
    /// Tóm tắt decision
    pub fn summary(&self) -> String {
        format!(
            "[{}] {} — priority: {:.2}, profit: ${:.2} ({})",
            self.method.label(),
            self.user_address,
            self.priority_score,
            self.adjusted_profit_usd,
            self.reasoning,
        )
    }
    
    /// Có nên execute không
    pub fn should_execute(&self) -> bool {
        !self.method.is_skip() && self.adjusted_profit_usd > 0.0
    }
}

/// Target đã được prioritize với score
#[derive(Debug, Clone)]
pub struct PrioritizedTarget {
    /// Target gốc
    pub target: LiquidationTarget,
    
    /// Profit estimate
    pub estimate: ProfitEstimate,
    
    /// Strategy decision
    pub decision: StrategyDecision,
    
    /// Rank trong batch (1 = cao nhất)
    pub rank: usize,
}

/// Kế hoạch thực thi cho một batch targets
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// Danh sách targets đã prioritize + decide (sorted by priority desc)
    pub targets: Vec<PrioritizedTarget>,
    
    /// Tổng số targets đầu vào
    pub total_input: usize,
    
    /// Số targets được chọn execute
    pub execute_count: usize,
    
    /// Số targets bị skip
    pub skip_count: usize,
    
    /// Số targets dùng direct
    pub direct_count: usize,
    
    /// Tổng estimated profit (USD)
    pub total_estimated_profit: f64,
    
    /// Tổng exposure (USD)
    pub total_exposure_usd: f64,
    
    /// Timestamp tạo plan
    pub created_at: i64,
}

impl ExecutionPlan {
    /// Tạo plan từ danh sách PrioritizedTarget
    pub fn from_targets(targets: Vec<PrioritizedTarget>, total_input: usize) -> Self {
        let execute_count = targets.iter()
            .filter(|t| t.decision.should_execute())
            .count();
        let skip_count = total_input - execute_count;
        
        let direct_count = targets.iter()
            .filter(|t| matches!(t.decision.method, ExecutionMethod::Direct { .. }))
            .count();
        
        let total_estimated_profit: f64 = targets.iter()
            .filter(|t| t.decision.should_execute())
            .map(|t| t.decision.adjusted_profit_usd)
            .sum();
        
        let total_exposure_usd: f64 = targets.iter()
            .filter(|t| t.decision.should_execute())
            .map(|t| t.estimate.debt_to_cover_usd)
            .sum();
        
        Self {
            targets,
            total_input,
            execute_count,
            skip_count,
            direct_count,
            total_estimated_profit,
            total_exposure_usd,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Lấy danh sách targets nên execute (đã sorted by priority)
    pub fn executable_targets(&self) -> Vec<&PrioritizedTarget> {
        self.targets.iter()
            .filter(|t| t.decision.should_execute())
            .collect()
    }
    
    /// In plan summary
    pub fn summary(&self) -> String {
        format!(
            "ExecutionPlan: {}/{} targets (direct: {}, skip: {}) | profit: ${:.2} | exposure: ${:.2}",
            self.execute_count, self.total_input,
            self.direct_count, self.skip_count,
            self.total_estimated_profit, self.total_exposure_usd,
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
    fn test_execution_method_direct() {
        let method = ExecutionMethod::Direct { gas_limit: 500_000 };
        assert!(!method.is_skip());
        assert_eq!(method.gas_limit(), 500_000);
        assert_eq!(method.label(), "DIRECT");
        assert!(format!("{}", method).contains("Direct"));
    }
    
    #[test]
    fn test_execution_method_skip() {
        let method = ExecutionMethod::Skip { reason: "газ дорогой".to_string() };
        assert!(method.is_skip());
        assert_eq!(method.gas_limit(), 0);
        assert_eq!(method.label(), "SKIP");
    }
    
    #[test]
    fn test_strategy_decision_should_execute() {
        let est = ProfitEstimate::unprofitable("0x1".to_string(), "test".to_string());
        
        let decision = StrategyDecision {
            user_address: "0x1".to_string(),
            method: ExecutionMethod::Direct { gas_limit: 500_000 },
            priority_score: 8.5,
            adjusted_profit_usd: 100.0,
            reasoning: "Direct vì đủ vốn".to_string(),
            profit_estimate: est,
        };
        assert!(decision.should_execute());
        assert!(decision.summary().contains("DIRECT"));
    }
    
    #[test]
    fn test_strategy_decision_skip() {
        let est = ProfitEstimate::unprofitable("0x2".to_string(), "test".to_string());
        
        let decision = StrategyDecision {
            user_address: "0x2".to_string(),
            method: ExecutionMethod::Skip { reason: "gas too high".to_string() },
            priority_score: 0.0,
            adjusted_profit_usd: 0.0,
            reasoning: "gas too high".to_string(),
            profit_estimate: est,
        };
        assert!(!decision.should_execute());
    }
    
    #[test]
    fn test_execution_plan_summary() {
        let plan = ExecutionPlan {
            targets: vec![],
            total_input: 5,
            execute_count: 3,
            skip_count: 2,
            direct_count: 3,
            total_estimated_profit: 500.0,
            total_exposure_usd: 25_000.0,
            created_at: 0,
        };
        let summary = plan.summary();
        assert!(summary.contains("3/5"));
        assert!(summary.contains("$500.00"));
    }
}
