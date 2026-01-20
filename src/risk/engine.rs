use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::mpsc;
use crate::data::asset::{Asset, AssetId};
use crate::data::user::{User, UserId};
use crate::data::registry::Registry;
use crate::events::event::Event;
use crate::risk::bucket::RiskBucket;
use crate::risk::health_factor::HealthFactorCalculator;

pub struct RiskEngine {
    pub users: Arc<DashMap<UserId, User>>,
    pub assets: Arc<DashMap<AssetId, Asset>>,
    pub registry: Arc<Registry>,
    receiver: mpsc::Receiver<Event>,
}

impl RiskEngine {
    pub fn new(receiver: mpsc::Receiver<Event>) -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            assets: Arc::new(DashMap::new()),
            registry: Arc::new(Registry::new()),
            receiver,
        }
    }

    pub async fn run(&mut self) {
        tracing::info!("RiskEngine started");
        
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::PriceUpdate { asset_id, new_price } => {
                    self.handle_price_update(asset_id, new_price);
                }
                Event::MempoolTx { user_id, affected_assets } => {
                    self.handle_mempool_tx(user_id, affected_assets);
                }
                Event::Block { block_number } => {
                    self.handle_block(block_number);
                }
            }
        }
    }

    fn handle_price_update(&self, asset_id: AssetId, new_price: f64) {
        // 1. Update Asset Price
        if let Some(mut asset) = self.assets.get_mut(&asset_id) {
            asset.price_in_eth = new_price;
        } else {
            tracing::warn!("Price update for unknown asset: {}", asset_id);
            return;
        }

        // 2. Fetch affected users
        let affected_users = self.registry.get_users_for_asset(&asset_id);
        
        tracing::debug!("Price update for {} ({} users affected)", asset_id, affected_users.len());

        // 3. Re-evaluate Risk
        // Convert dashmap reference to standard HashMap for HF calculator
        // Note: For high performance, we might want to avoid full clone if possible,
        // but HF calculator needs a consistent view of asset parameters.
        // For O(1) mostly, we can just pass the asset map wrapper and look up individually?
        // Let's modify HF calculator to take a function or trait, or just pass the map.
        // Cloning the *Asset structs* (small) into a HashMap is fine, or we can just iterate.
        // Actually, `DashMap` iteration can deadlock if we are not careful with locks.
        // But here we need random access.
        // Let's clone the assets map for safety during calculation? No, that's O(N_assets).
        // Better: Pass `&DashMap` to HF calculator? 
        // Let's just collect necessary assets for the user?
        // Simplest: Collect all assets into a HashMap. If N_assets is small (e.g. < 50), this is cheap.
        // Let's assume N_assets is small.
        
        let asset_snapshot: std::collections::HashMap<_, _> = self.assets.iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();

        for user_id in affected_users {
            if let Some(mut user) = self.users.get_mut(&user_id) {
                let new_hf = HealthFactorCalculator::calculate(&user, &asset_snapshot);
                let old_bucket = user.risk_bucket;
                let new_bucket = RiskBucket::from_hf(new_hf);

                user.health_factor = new_hf;
                user.risk_bucket = new_bucket;

                if new_bucket != old_bucket {
                    tracing::info!(
                        "User {} bucket change: {:?} -> {:?} (HF: {:.4})", 
                        user_id, old_bucket, new_bucket, new_hf
                    );
                }

                if new_bucket == RiskBucket::Liquidate {
                    tracing::warn!("LIQUIDATION ALERT: User {} HF {:.4}", user_id, new_hf);
                    // Emit high risk event or trigger executor
                }
            }
        }
    }

    fn handle_mempool_tx(&self, user_id: UserId, _affected_assets: Vec<AssetId>) {
         // Logic to simulate bucket change
         // For now, just logging
         tracing::debug!("Mempool tx for user {}", user_id);
    }

    fn handle_block(&self, block_number: u64) {
        tracing::info!("New block: {}", block_number);
    }
}
