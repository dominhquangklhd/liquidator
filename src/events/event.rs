use crate::data::asset::AssetId;
use crate::data::user::UserId;

#[derive(Debug, Clone)]
pub enum Event {
    PriceUpdate {
        asset_id: AssetId,
        new_price: f64,
    },
    MempoolTx {
        user_id: UserId,
        // In a real system, this would contain tx details.
        // For simulation, we might pass simulated balance changes.
        // But per requirements: "Pending borrow / withdraw / repay tx"
        // Let's keep it abstract for now or add payload.
        // Adding a simplified payload for HF impact simulation:
        affected_assets: Vec<AssetId>, 
    },
    Block {
        block_number: u64,
    },
}
