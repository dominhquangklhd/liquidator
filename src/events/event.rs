use crate::data::asset::AssetId;
use crate::data::user::UserId;

#[derive(Debug, Clone)]
pub enum Event {
    PriceUpdate {
        asset_id: AssetId,
        new_price: f64,
    },
    UserDeposit {
        user_id: UserId,
        asset_id: AssetId,
        amount: f64,
    },
    UserWithdraw {
        user_id: UserId,
        asset_id: AssetId,
        amount: f64,
    },
    UserBorrow {
        user_id: UserId,
        asset_id: AssetId,
        amount: f64,
    },
    UserRepay {
        user_id: UserId,
        asset_id: AssetId,
        amount: f64,
    },
    Block {
        block_number: u64,
    },
    TriggerDailyBootstrap {
        provider: std::sync::Arc<ethers::providers::Provider<ethers::providers::Http>>,
        chain_id: u64,
        aave_pool_address: ethers::types::Address,
        aave_oracle_address: ethers::types::Address,
        aave_addresses_provider: ethers::types::Address,
    },
}
