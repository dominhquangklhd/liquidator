use serde::{Deserialize, Serialize};

pub type AssetId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub symbol: String,
    pub decimals: u8,
    pub ltv: f64,               // Loan-to-Value (e.g., 0.80)
    pub liquidation_threshold: f64, // e.g., 0.85
    pub price_in_eth: f64,      // Current price
}
