// Strategy Decider Module
//
// Quyết định chiến lược tối ưu cho mỗi liquidation opportunity.
//
// ## Các thành phần:
//
// - **StrategyConfig** — Cấu hình (thresholds, weights, risk limits)
// - **ExecutionMethod** — Direct | Skip
// - **StrategyDecision** — Quyết định cho 1 target
// - **ExecutionPlan** — Kế hoạch cho batch targets  
// - **StrategyDecider** — Core logic quyết định
//
// ## Flow:
//
// ```
// Vec<(LiquidationTarget, ProfitEstimate)>
//   → StrategyDecider.create_plan()
//   │
//   ├── Mỗi target:
//   │   ├── Check circuit breaker
//   │   ├── Check exposure limits
//   │   ├── decide_method() → Direct | Skip
//   │   │   ├── wallet có đủ token? → Direct
//   │   │   └── không đủ điều kiện direct? → Skip
//   │   └── calculate priority score (multi-factor)
//   │       = w_profit × profit + w_urgency × (1/HF)
//   │       + w_efficiency × ROI + w_size × (1/debt)
//   │
//   ├── Sort by priority desc
//   ├── Apply concurrent limit
//   └── → ExecutionPlan
// ```

pub mod config;
pub mod types;
pub mod decider;

// Re-exports
pub use config::StrategyConfig;
pub use types::{ExecutionMethod, StrategyDecision, PrioritizedTarget, ExecutionPlan};
pub use decider::{StrategyDecider, StrategyStats};
