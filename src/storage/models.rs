// Data models for storage layer

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Liquidation target (user with risky position)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationTarget {
    /// User's blockchain address
    pub user_address: String,
    
    /// Current health factor
    pub health_factor: f64,
    
    /// Total collateral value (in USD)
    pub total_collateral_usd: f64,
    
    /// Total debt value (in USD)
    pub total_debt_usd: f64,
    
    /// Loan-to-Value ratio
    pub ltv: f64,
    
    /// Liquidation threshold
    pub liquidation_threshold: f64,
    
    /// Collateral by asset
    pub collateral: HashMap<String, f64>,
    
    /// Debt by asset
    pub debt: HashMap<String, f64>,
    
    /// Estimated profit from liquidation (in USD)
    pub estimated_profit: f64,
    
    /// Risk score (1-10, 10 = most urgent)
    pub risk_score: u8,
    
    /// Last update timestamp (Unix seconds)
    pub last_updated: i64,
}

impl LiquidationTarget {
    /// Create a new liquidation target
    pub fn new(user_address: String) -> Self {
        Self {
            user_address,
            health_factor: 2.0,
            total_collateral_usd: 0.0,
            total_debt_usd: 0.0,
            ltv: 0.0,
            liquidation_threshold: 0.0,
            collateral: HashMap::new(),
            debt: HashMap::new(),
            estimated_profit: 0.0,
            risk_score: 1,
            last_updated: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Calculate risk score based on health factor and volatility
    pub fn calculate_risk_score(&mut self) {
        self.risk_score = match self.health_factor {
            hf if hf < 1.0 => 10,   // Already liquidatable
            hf if hf < 1.05 => 9,   // Critical
            hf if hf < 1.10 => 8,   // Very high risk
            hf if hf < 1.15 => 6,   // High risk
            hf if hf < 1.20 => 4,   // Medium risk
            hf if hf < 1.30 => 2,   // Low risk
            _ => 1,                 // Minimal risk
        };
    }
    
    /// Estimate liquidation profit (5% bonus typical)
    pub fn estimate_profit(&mut self, liquidation_bonus: f64) {
        if self.health_factor >= 1.0 {
            self.estimated_profit = 0.0;
            return;
        }
        
        // Max liquidation: 50% of debt
        let max_debt_to_cover = self.total_debt_usd * 0.5;
        
        // Profit = bonus on liquidated collateral
        self.estimated_profit = max_debt_to_cover * liquidation_bonus;
    }
    
    /// Update timestamp
    pub fn touch(&mut self) {
        self.last_updated = chrono::Utc::now().timestamp();
    }
}

/// Historical snapshot of user's position
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HistoricalSnapshot {
    pub user_address: String,
    pub timestamp: i64,
    pub health_factor: f64,
    pub total_collateral_usd: f64,
    pub total_debt_usd: f64,
}

/// Liquidation event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationEvent {
    /// Unique event ID
    pub id: Option<i64>,
    
    /// User who was liquidated
    pub user_address: String,
    
    /// Timestamp of liquidation
    pub timestamp: i64,
    
    /// Collateral asset seized
    pub collateral_asset: String,
    
    /// Debt asset repaid
    pub debt_asset: String,
    
    /// Amount of collateral seized
    pub collateral_seized: f64,
    
    /// Amount of debt covered
    pub debt_covered: f64,
    
    /// Liquidator address
    pub liquidator: String,
    
    /// Transaction hash
    pub tx_hash: String,
    
    /// Actual profit in USD
    pub profit_usd: f64,
    
    /// Gas cost in USD
    pub gas_cost_usd: f64,
}

impl LiquidationEvent {
    pub fn new(
        user_address: String,
        collateral_asset: String,
        debt_asset: String,
        collateral_seized: f64,
        debt_covered: f64,
        liquidator: String,
        tx_hash: String,
    ) -> Self {
        Self {
            id: None,
            user_address,
            timestamp: chrono::Utc::now().timestamp(),
            collateral_asset,
            debt_asset,
            collateral_seized,
            debt_covered,
            liquidator,
            tx_hash,
            profit_usd: 0.0,
            gas_cost_usd: 0.0,
        }
    }
    
    /// Calculate net profit
    pub fn net_profit(&self) -> f64 {
        self.profit_usd - self.gas_cost_usd
    }
}
