use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::risk::bucket::RiskBucket;
use super::asset::AssetId;

pub type UserId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub collateral: HashMap<AssetId, f64>, // AssetId -> Amount
    pub debt: HashMap<AssetId, f64>,       // AssetId -> Amount
    pub health_factor: f64,
    pub risk_bucket: RiskBucket,
}

impl User {
    pub fn new(id: UserId) -> Self {
        Self {
            id,
            collateral: HashMap::new(),
            debt: HashMap::new(),
            health_factor: f64::MAX,
            risk_bucket: RiskBucket::Safe,
        }
    }
}
