// Profit Calculator Module
//
// Tính toán lợi nhuận ước tính cho mỗi liquidation opportunity
// trước khi quyết định execute.
//
// ## Các thành phần:
//
// - **ProfitConfig** — Cấu hình (bonus%, gas limit, min profit, min ROI)
// - **ProfitEstimate** — Kết quả đánh giá (gross/net profit, gas, slippage)
// - **GasEstimator** — Ước tính gas cost (đọc gas price từ RPC)
// - **ProfitCalculator** — Core logic tính toán
//
// ## Flow:
//
// ```
// LiquidationTarget → ProfitCalculator.evaluate() → ProfitEstimate
//   ├── find_liquidation_pairs()    → Vec<LiquidationPair>
//   ├── select best pair            → LiquidationPair (score cao nhất)
//   ├── calculate_profit()          → ProfitEstimate
//   │     ├── debt_to_cover = debt × close_factor
//   │     ├── collateral_received = debt_to_cover × (1 + bonus%)
//   │     ├── gross_profit = debt_to_cover × bonus%
//   │     ├── gas_cost = gas_price × gas_limit → USD
//   │     ├── slippage = collateral × (base% + size_impact%)
//   │     ├── flash_loan_fee = debt × 0.05%
//   │     └── net_profit = gross - gas - slippage - flash_fee
//   └── check_profitability()       → (is_profitable, reject_reason)
// ```

pub mod config;
pub mod types;
pub mod gas;
pub mod calculator;

// Re-exports
pub use config::ProfitConfig;
pub use types::{ProfitEstimate, ProfitBreakdown, LiquidationPair, GasCostEstimate};
pub use gas::GasEstimator;
pub use calculator::{ProfitCalculator, ProfitStats};
