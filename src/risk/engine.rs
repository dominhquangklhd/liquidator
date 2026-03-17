use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::mpsc;
use crate::data::asset::{Asset, AssetId};
use crate::data::user::{User, UserId};
use crate::data::registry::Registry;
use crate::events::event::Event;
use crate::risk::bucket::RiskBucket;
use crate::risk::health_factor::HealthFactorCalculator;
use crate::storage::{HybridStorage, LiquidationTarget};

pub struct RiskEngine {
    pub users: Arc<DashMap<UserId, User>>,
    pub assets: Arc<DashMap<AssetId, Asset>>,
    pub registry: Arc<Registry>,
    receiver: mpsc::Receiver<Event>,
    storage: Arc<HybridStorage>,
}

impl RiskEngine {
    pub fn new(receiver: mpsc::Receiver<Event>, storage: Arc<HybridStorage>) -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            assets: Arc::new(DashMap::new()),
            registry: Arc::new(Registry::new()),
            receiver,
            storage,
        }
    }

    pub async fn run(&mut self) {
        tracing::info!("RiskEngine started");
        
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::PriceUpdate { asset_id, new_price } => {
                    self.handle_price_update(asset_id, new_price).await;
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

    async fn handle_price_update(&self, asset_id: AssetId, new_price: f64) {
        // 1. Update asset price
        if let Some(mut asset) = self.assets.get_mut(&asset_id) {
            asset.price_in_eth = new_price;
        } else {
            tracing::warn!("Price update for unknown asset: {}", asset_id);
            return;
        }

        // 2. Fetch affected users
        let affected_users = self.registry.get_users_for_asset(&asset_id);
        tracing::debug!("Price update for {} ({} users affected)", asset_id, affected_users.len());

        // 3. Snapshot assets into plain HashMap (avoids holding DashMap shard locks across .await)
        let asset_snapshot: std::collections::HashMap<_, _> = self.assets.iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();

        // 4. Re-evaluate each user; collect targets that need storage update.
        //    All DashMap writes complete before the first .await so no lock is held across await.
        let mut targets_to_update: Vec<LiquidationTarget> = Vec::new();

        for user_id in affected_users {
            if let Some(mut user) = self.users.get_mut(&user_id) {
                let old_hf = user.health_factor;
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
                }

                // Build LiquidationTarget for users within (or recently within) the pre-tracking range.
                // This ensures cache entries are also removed when a user recovers and exits the range.
                if new_hf < 1.3 || old_hf < 1.3 {
                    // Rough USD values using price_in_eth * $2000 (good enough for ordering targets)
                    let total_collateral_usd: f64 = user.collateral.iter()
                        .filter_map(|(aid, amount)| {
                            asset_snapshot.get(aid).map(|a| amount * a.price_in_eth * 2000.0)
                        })
                        .sum();
                    let total_debt_usd: f64 = user.debt.iter()
                        .filter_map(|(aid, amount)| {
                            asset_snapshot.get(aid).map(|a| amount * a.price_in_eth * 2000.0)
                        })
                        .sum();
                    let ltv = if total_collateral_usd > 0.0 {
                        total_debt_usd / total_collateral_usd
                    } else {
                        0.0
                    };
                    // risk_score 1-10: lower HF = higher urgency
                    let risk_score = ((1.5 - new_hf) / 0.5 * 10.0).clamp(1.0, 10.0) as u8;

                    targets_to_update.push(LiquidationTarget {
                        user_address: user.id.clone(),
                        health_factor: new_hf,
                        total_collateral_usd,
                        total_debt_usd,
                        ltv,
                        liquidation_threshold: 0.85,
                        collateral: user.collateral.clone(),
                        debt: user.debt.clone(),
                        estimated_profit: 0.0, // filled by ProfitCalculator in executor_worker
                        risk_score,
                        last_updated: chrono::Utc::now().timestamp(),
                    });
                }
            } // DashMap lock released here
        }

        // 5. Push targets to hot cache (safe to await — all DashMap locks released above)
        for target in targets_to_update {
            if let Err(e) = self.storage.update_user_hf(target).await {
                tracing::error!("Failed to update storage: {:?}", e);
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
