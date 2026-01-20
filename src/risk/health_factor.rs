use std::collections::HashMap;
use crate::data::user::User;
use crate::data::asset::{Asset, AssetId};

pub struct HealthFactorCalculator;

impl HealthFactorCalculator {
    pub fn calculate(user: &User, assets: &HashMap<AssetId, Asset>) -> f64 {
        let mut total_collateral_eth = 0.0;
        let mut total_debt_eth = 0.0;
        let mut current_liquidation_threshold = 0.0;

        for (asset_id, amount) in &user.collateral {
            if let Some(asset) = assets.get(asset_id) {
                let value_eth = amount * asset.price_in_eth;
                total_collateral_eth += value_eth;
                current_liquidation_threshold += value_eth * asset.liquidation_threshold;
            }
        }

        for (asset_id, amount) in &user.debt {
            if let Some(asset) = assets.get(asset_id) {
                let value_eth = amount * asset.price_in_eth;
                total_debt_eth += value_eth;
            }
        }

        if total_debt_eth == 0.0 {
            return f64::MAX;
        }

        // Weighted Average Liquidation Threshold
        // Actually Aave formula: sum(Collateral * Threshold) / Total Debt
        // My code above sums (Collateral * Threshold) into `current_liquidation_threshold` variable name (slightly misleading name, but correct logic)
        
        current_liquidation_threshold / total_debt_eth
    }
}
