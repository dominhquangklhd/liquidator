use crate::data::asset::AssetId;

#[derive(Debug, Clone)]
pub enum Event {
    PriceUpdate {
        asset_id: AssetId,
        new_price: f64,
    },
    Block {
        block_number: u64,
    },
}
