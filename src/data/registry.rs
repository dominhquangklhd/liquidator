use dashmap::DashMap;
use crate::data::asset::AssetId;
use crate::data::user::UserId;

pub struct Registry {
    // Maps AssetId -> List of Users who hold this asset (collateral or debt)
    asset_users: DashMap<AssetId, Vec<UserId>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            asset_users: DashMap::new(),
        }
    }

    pub fn add_user_to_asset(&self, asset_id: AssetId, user_id: UserId) {
        let mut entry = self.asset_users.entry(asset_id).or_insert_with(Vec::new);
        if !entry.contains(&user_id) {
            entry.push(user_id);
        }
    }

    pub fn remove_user_from_asset(&self, asset_id: &AssetId, user_id: &UserId) {
        if let Some(mut users) = self.asset_users.get_mut(asset_id) {
            if let Some(pos) = users.iter().position(|u| u == user_id) {
                users.swap_remove(pos);
            }
        }
    }

    pub fn get_users_for_asset(&self, asset_id: &AssetId) -> Vec<UserId> {
        self.asset_users.get(asset_id)
            .map(|users| users.clone())
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        self.asset_users.clear();
    }
}
