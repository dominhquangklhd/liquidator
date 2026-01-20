use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskBucket {
    Safe,       // HF > 2.0
    Watch,      // 1.5 - 2.0
    Risk,       // 1.2 - 1.5
    Danger,     // 1.0 - 1.2
    Liquidate,  // < 1.0
}

impl RiskBucket {
    pub fn from_hf(hf: f64) -> Self {
        if hf < 1.0 {
            RiskBucket::Liquidate
        } else if hf < 1.2 {
            RiskBucket::Danger
        } else if hf < 1.5 {
            RiskBucket::Risk
        } else if hf <= 2.0 {
            RiskBucket::Watch
        } else {
            RiskBucket::Safe
        }
    }

    pub fn is_risky(&self) -> bool {
        matches!(self, RiskBucket::Danger | RiskBucket::Liquidate)
    }
}
