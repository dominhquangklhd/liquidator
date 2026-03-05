// Profit Calculator Types
//
// Kiểu dữ liệu cho module tính toán lợi nhuận:
// - ProfitEstimate: Kết quả ước tính lợi nhuận
// - GasCostEstimate: Chi phí gas
// - LiquidationPair: Cặp collateral/debt tối ưu
// - ProfitBreakdown: Chi tiết từng khoản profit/cost

/// Kết quả đánh giá lợi nhuận cho một cơ hội thanh lý
#[derive(Debug, Clone)]
pub struct ProfitEstimate {
    /// Địa chỉ user bị thanh lý
    pub user_address: String,
    
    /// Cặp collateral/debt được chọn
    pub pair: LiquidationPair,
    
    /// Số debt sẽ cover (USD)
    pub debt_to_cover_usd: f64,
    
    /// Số collateral sẽ nhận (USD, bao gồm bonus)
    pub collateral_received_usd: f64,
    
    /// Gross profit (USD) = collateral_received - debt_to_cover
    pub gross_profit_usd: f64,
    
    /// Chi phí gas (USD)
    pub gas_cost_usd: f64,
    
    /// Ước lượng slippage (USD) — mất khi swap collateral → stable
    pub slippage_cost_usd: f64,
    
    /// Flash loan fee (USD) — nếu dùng flash loan
    pub flash_loan_fee_usd: f64,
    
    /// Net profit (USD) = gross - gas - slippage - flash_loan_fee
    pub net_profit_usd: f64,
    
    /// ROI (%) = net_profit / gas_cost × 100
    pub roi_pct: f64,
    
    /// Có đáng execute không
    pub is_profitable: bool,
    
    /// Lý do nếu không profitable
    pub reject_reason: Option<String>,
    
    /// Chi tiết breakdown
    pub breakdown: ProfitBreakdown,
}

impl ProfitEstimate {
    /// Tạo estimate KHÔNG profitable
    pub fn unprofitable(user_address: String, reason: String) -> Self {
        Self {
            user_address,
            pair: LiquidationPair::default(),
            debt_to_cover_usd: 0.0,
            collateral_received_usd: 0.0,
            gross_profit_usd: 0.0,
            gas_cost_usd: 0.0,
            slippage_cost_usd: 0.0,
            flash_loan_fee_usd: 0.0,
            net_profit_usd: 0.0,
            roi_pct: 0.0,
            is_profitable: false,
            reject_reason: Some(reason),
            breakdown: ProfitBreakdown::default(),
        }
    }
    
    /// Tóm tắt ngắn gọn
    pub fn summary(&self) -> String {
        if self.is_profitable {
            format!(
                "✓ {} — net: ${:.2} (gross: ${:.2} - gas: ${:.2} - slip: ${:.2}) ROI: {:.0}%",
                self.user_address, self.net_profit_usd, self.gross_profit_usd,
                self.gas_cost_usd, self.slippage_cost_usd, self.roi_pct
            )
        } else {
            format!(
                "✗ {} — {}", 
                self.user_address, 
                self.reject_reason.as_deref().unwrap_or("unprofitable")
            )
        }
    }
}

/// Cặp collateral/debt tối ưu cho liquidation
#[derive(Debug, Clone, Default)]
pub struct LiquidationPair {
    /// Asset dùng làm collateral (sẽ nhận được)
    pub collateral_asset: String,
    
    /// Asset là debt (sẽ phải trả)
    pub debt_asset: String,
    
    /// Liquidation bonus cho collateral asset (%)
    pub bonus_pct: f64,
    
    /// Giá collateral (USD)
    pub collateral_price_usd: f64,
    
    /// Giá debt (USD)
    pub debt_price_usd: f64,
    
    /// Số lượng collateral user có
    pub collateral_amount: f64,
    
    /// Số lượng debt user nợ
    pub debt_amount: f64,
    
    /// Score để rank cặp này (cao = tốt hơn)
    pub score: f64,
}

impl LiquidationPair {
    /// Giá trị collateral (USD)
    pub fn collateral_value_usd(&self) -> f64 {
        self.collateral_amount * self.collateral_price_usd
    }
    
    /// Giá trị debt (USD)
    pub fn debt_value_usd(&self) -> f64 {
        self.debt_amount * self.debt_price_usd
    }
}

/// Ước lượng chi phí gas
#[derive(Debug, Clone, Default)]
pub struct GasCostEstimate {
    /// Gas price hiện tại (Gwei)
    pub gas_price_gwei: f64,
    
    /// Priority fee / tip (Gwei)
    pub priority_fee_gwei: f64,
    
    /// Gas limit (units)
    pub gas_limit: u64,
    
    /// Estimated gas used (units)
    pub gas_used: u64,
    
    /// ETH price (USD) — để chuyển gas cost sang USD
    pub eth_price_usd: f64,
    
    /// Gas cost (ETH)
    pub cost_eth: f64,
    
    /// Gas cost (USD)
    pub cost_usd: f64,
}

impl GasCostEstimate {
    /// Tính gas cost
    pub fn calculate(gas_price_gwei: f64, gas_limit: u64, eth_price_usd: f64) -> Self {
        let cost_eth = gas_price_gwei * gas_limit as f64 / 1e9;
        let cost_usd = cost_eth * eth_price_usd;
        
        Self {
            gas_price_gwei,
            priority_fee_gwei: 0.0,
            gas_limit,
            gas_used: gas_limit,
            eth_price_usd,
            cost_eth,
            cost_usd,
        }
    }
    
    /// Tính với EIP-1559 (base + priority)
    pub fn calculate_eip1559(
        base_fee_gwei: f64, 
        priority_fee_gwei: f64, 
        gas_limit: u64, 
        eth_price_usd: f64
    ) -> Self {
        let total_gwei = base_fee_gwei + priority_fee_gwei;
        let cost_eth = total_gwei * gas_limit as f64 / 1e9;
        let cost_usd = cost_eth * eth_price_usd;
        
        Self {
            gas_price_gwei: total_gwei,
            priority_fee_gwei,
            gas_limit,
            gas_used: gas_limit,
            eth_price_usd,
            cost_eth,
            cost_usd,
        }
    }
}

/// Chi tiết breakdown lợi nhuận
#[derive(Debug, Clone, Default)]
pub struct ProfitBreakdown {
    // ── Revenue ──
    /// Debt sẽ cover (USD)
    pub debt_covered_usd: f64,
    
    /// Collateral nhận được (USD, chưa tính bonus)
    pub collateral_base_usd: f64,
    
    /// Bonus nhận thêm (USD) = debt × bonus%
    pub bonus_usd: f64,
    
    // ── Costs ──
    /// Gas cost chi tiết
    pub gas: GasCostEstimate,
    
    /// Slippage estimate (%)
    pub slippage_pct: f64,
    
    /// Slippage cost (USD)
    pub slippage_usd: f64,
    
    /// Size impact (thêm slippage do position lớn)
    pub size_impact_pct: f64,
    
    /// Flash loan fee (USD)
    pub flash_loan_fee_usd: f64,
    
    // ── Summary ──
    /// Total cost (USD)
    pub total_cost_usd: f64,
    
    /// Gross profit (USD)
    pub gross_profit_usd: f64,
    
    /// Net profit (USD)
    pub net_profit_usd: f64,
}

impl ProfitBreakdown {
    /// In breakdown chi tiết
    pub fn display(&self) -> String {
        format!(
            "┌─ PROFIT BREAKDOWN ────────────────────\n\
             │ Debt covered:     ${:.2}\n\
             │ Collateral base:  ${:.2}\n\
             │ Bonus ({:.1}%):      +${:.2}\n\
             │ ─────────────────────────────────────\n\
             │ Gross profit:     ${:.2}\n\
             │ ─────────────────────────────────────\n\
             │ Gas cost:         -${:.2} ({:.1} Gwei × {} gas)\n\
             │ Slippage:         -${:.2} ({:.2}% + {:.2}% size)\n\
             │ Flash loan fee:   -${:.2}\n\
             │ ─────────────────────────────────────\n\
             │ Total cost:       -${:.2}\n\
             │ NET PROFIT:       ${:.2}\n\
             └──────────────────────────────────────",
            self.debt_covered_usd,
            self.collateral_base_usd,
            self.slippage_pct, // reuse for bonus display (we'll use bonus_pct from pair)
            self.bonus_usd,
            self.gross_profit_usd,
            self.gas.cost_usd, self.gas.gas_price_gwei, self.gas.gas_limit,
            self.slippage_usd, self.slippage_pct, self.size_impact_pct,
            self.flash_loan_fee_usd,
            self.total_cost_usd,
            self.net_profit_usd,
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
    fn test_gas_cost_calculate() {
        // 30 Gwei × 500k gas, ETH = $2000
        let gas = GasCostEstimate::calculate(30.0, 500_000, 2000.0);
        // cost_eth = 30 * 500000 / 1e9 = 0.015 ETH
        assert!((gas.cost_eth - 0.015).abs() < 1e-6);
        // cost_usd = 0.015 * 2000 = $30
        assert!((gas.cost_usd - 30.0).abs() < 0.01);
    }
    
    #[test]
    fn test_gas_cost_eip1559() {
        // base=20 + priority=2 = 22 Gwei × 500k gas, ETH = $2000
        let gas = GasCostEstimate::calculate_eip1559(20.0, 2.0, 500_000, 2000.0);
        assert!((gas.gas_price_gwei - 22.0).abs() < 0.01);
        // cost_eth = 22 * 500000 / 1e9 = 0.011 ETH 
        assert!((gas.cost_eth - 0.011).abs() < 1e-6);
        // cost_usd = 0.011 * 2000 = $22
        assert!((gas.cost_usd - 22.0).abs() < 0.01);
    }
    
    #[test]
    fn test_unprofitable_estimate() {
        let est = ProfitEstimate::unprofitable(
            "0xuser".to_string(),
            "HF >= 1.0".to_string(),
        );
        assert!(!est.is_profitable);
        assert_eq!(est.net_profit_usd, 0.0);
        assert!(est.reject_reason.is_some());
    }
    
    #[test]
    fn test_liquidation_pair_values() {
        let pair = LiquidationPair {
            collateral_asset: "ETH".to_string(),
            debt_asset: "USDC".to_string(),
            bonus_pct: 5.0,
            collateral_price_usd: 2000.0,
            debt_price_usd: 1.0,
            collateral_amount: 10.0,
            debt_amount: 16000.0,
            score: 0.0,
        };
        assert_eq!(pair.collateral_value_usd(), 20000.0);
        assert_eq!(pair.debt_value_usd(), 16000.0);
    }
    
    #[test]
    fn test_profit_estimate_summary() {
        let est = ProfitEstimate {
            user_address: "0xabc".to_string(),
            pair: LiquidationPair::default(),
            debt_to_cover_usd: 8000.0,
            collateral_received_usd: 8400.0,
            gross_profit_usd: 400.0,
            gas_cost_usd: 30.0,
            slippage_cost_usd: 40.0,
            flash_loan_fee_usd: 0.0,
            net_profit_usd: 330.0,
            roi_pct: 1100.0,
            is_profitable: true,
            reject_reason: None,
            breakdown: ProfitBreakdown::default(),
        };
        let summary = est.summary();
        assert!(summary.contains("$330.00"));
        assert!(summary.contains("1100%"));
    }
}
