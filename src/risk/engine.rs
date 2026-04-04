use std::sync::Arc;
use std::collections::HashMap;
use dashmap::DashMap;
use tokio::sync::mpsc;
use crate::data::asset::{Asset, AssetId};
use crate::data::user::{User, UserId};
use crate::data::registry::Registry;
use crate::events::event::Event;
use crate::risk::bucket::RiskBucket;
use crate::risk::health_factor::HealthFactorCalculator;
use crate::storage::{HybridStorage, LiquidationTarget};

#[derive(Debug, Clone)]
pub struct RiskEngineConfig {
    /// Reference ETH/USD used to convert asset amounts (priced in ETH) into USD.
    pub reference_eth_price_usd: f64,

    /// Default liquidation threshold stored with synthesized targets.
    pub default_liquidation_threshold: f64,

    /// Risk score parameters.
    pub risk_score_hf_baseline: f64,
    pub risk_score_hf_span: f64,
    pub risk_score_min: f64,
    pub risk_score_max: f64,
}

impl Default for RiskEngineConfig {
    fn default() -> Self {
        Self {
            reference_eth_price_usd: 2000.0,
            default_liquidation_threshold: 0.85,
            risk_score_hf_baseline: 1.5,
            risk_score_hf_span: 0.5,
            risk_score_min: 1.0,
            risk_score_max: 10.0,
        }
    }
}

pub struct RiskEngine {
    pub users: Arc<DashMap<UserId, User>>,
    pub assets: Arc<DashMap<AssetId, Asset>>,
    pub registry: Arc<Registry>,
    receiver: mpsc::Receiver<Event>,
    storage: Arc<HybridStorage>,
    config: RiskEngineConfig,
}

impl RiskEngine {
    pub fn new(receiver: mpsc::Receiver<Event>, storage: Arc<HybridStorage>) -> Self {
        Self::with_config(receiver, storage, RiskEngineConfig::default())
    }

    pub fn with_config(
        receiver: mpsc::Receiver<Event>,
        storage: Arc<HybridStorage>,
        config: RiskEngineConfig,
    ) -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            assets: Arc::new(DashMap::new()),
            registry: Arc::new(Registry::new()),
            receiver,
            storage,
            config,
        }
    }

    pub async fn run(&mut self) {
        tracing::info!("RiskEngine started");
        
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::PriceUpdate { asset_id, new_price } => {
                    self.handle_price_update(asset_id, new_price).await;
                }
                Event::UserDeposit { user_id, asset_id, amount } => {
                    self.handle_user_deposit(user_id, asset_id, amount).await;
                }
                Event::UserWithdraw { user_id, asset_id, amount } => {
                    self.handle_user_withdraw(user_id, asset_id, amount).await;
                }
                Event::UserBorrow { user_id, asset_id, amount } => {
                    self.handle_user_borrow(user_id, asset_id, amount).await;
                }
                Event::UserRepay { user_id, asset_id, amount } => {
                    self.handle_user_repay(user_id, asset_id, amount).await;
                }
                Event::Block { block_number } => {
                    self.handle_block(block_number).await;
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

        // 2. Fetch affected users and run unified reevaluation pipeline.
        let affected_users = self.registry.get_users_for_asset(&asset_id);
        tracing::debug!("Price update for {} ({} users affected)", asset_id, affected_users.len());
        self.recalculate_and_sync_users(affected_users, "price_update").await;
    }

    async fn handle_block(&self, block_number: u64) {
        tracing::debug!("New block observed: {}", block_number);
    }

    async fn handle_user_deposit(&self, user_id: UserId, asset_id: AssetId, amount: f64) {
        if amount <= 0.0 {
            return;
        }

        let mut should_recalculate = false;
        if let Some(mut user) = self.users.get_mut(&user_id) {
            let balance = user.collateral.entry(asset_id.clone()).or_insert(0.0);
            *balance += amount;
            self.registry.add_user_to_asset(asset_id.clone(), user_id.clone());
            should_recalculate = true;
        } else {
            tracing::debug!(
                "[user_deposit] ignored for unknown user={} asset={} amount={:.8}",
                user_id,
                asset_id,
                amount
            );
            return;
        }

        tracing::info!(
            "[user_deposit] user={} asset={} amount={:.8}",
            user_id,
            asset_id,
            amount
        );

        if should_recalculate {
            self.recalculate_and_sync_users(vec![user_id], "user_deposit").await;
        }
    }

    async fn handle_user_withdraw(&self, user_id: UserId, asset_id: AssetId, amount: f64) {
        if amount <= 0.0 {
            return;
        }

        let mut should_recalculate = false;
        if let Some(mut user) = self.users.get_mut(&user_id) {
            let mut remove_from_registry = false;
            if let Some(balance) = user.collateral.get_mut(&asset_id) {
                *balance = (*balance - amount).max(0.0);
                if *balance <= f64::EPSILON {
                    user.collateral.remove(&asset_id);
                    remove_from_registry = true;
                }
            }

            if remove_from_registry {
                self.registry.remove_user_from_asset(&asset_id, &user_id);
            }
            should_recalculate = true;
        } else {
            tracing::debug!(
                "[user_withdraw] ignored for unknown user={} asset={} amount={:.8}",
                user_id,
                asset_id,
                amount
            );
            return;
        }

        tracing::info!(
            "[user_withdraw] user={} asset={} amount={:.8}",
            user_id,
            asset_id,
            amount
        );

        if should_recalculate {
            self.recalculate_and_sync_users(vec![user_id], "user_withdraw").await;
        }
    }

    async fn handle_user_borrow(&self, user_id: UserId, asset_id: AssetId, amount: f64) {
        if amount <= 0.0 {
            return;
        }

        let mut should_recalculate = false;
        if let Some(mut user) = self.users.get_mut(&user_id) {
            let balance = user.debt.entry(asset_id.clone()).or_insert(0.0);
            *balance += amount;
            self.registry.add_user_to_asset(asset_id.clone(), user_id.clone());
            should_recalculate = true;
        } else {
            tracing::debug!(
                "[user_borrow] ignored for unknown user={} asset={} amount={:.8}",
                user_id,
                asset_id,
                amount
            );
            return;
        }

        tracing::info!(
            "[user_borrow] user={} asset={} amount={:.8}",
            user_id,
            asset_id,
            amount
        );

        if should_recalculate {
            self.recalculate_and_sync_users(vec![user_id], "user_borrow").await;
        }
    }

    async fn handle_user_repay(&self, user_id: UserId, asset_id: AssetId, amount: f64) {
        if amount <= 0.0 {
            return;
        }

        let mut should_recalculate = false;
        if let Some(mut user) = self.users.get_mut(&user_id) {
            let mut remove_from_registry = false;
            if let Some(balance) = user.debt.get_mut(&asset_id) {
                *balance = (*balance - amount).max(0.0);
                if *balance <= f64::EPSILON {
                    user.debt.remove(&asset_id);
                    remove_from_registry = !user.collateral.contains_key(&asset_id);
                }
            }

            if remove_from_registry {
                self.registry.remove_user_from_asset(&asset_id, &user_id);
            }
            should_recalculate = true;
        } else {
            tracing::debug!(
                "[user_repay] ignored for unknown user={} asset={} amount={:.8}",
                user_id,
                asset_id,
                amount
            );
            return;
        }

        tracing::info!(
            "[user_repay] user={} asset={} amount={:.8}",
            user_id,
            asset_id,
            amount
        );

        if should_recalculate {
            self.recalculate_and_sync_users(vec![user_id], "user_repay").await;
        }
    }

    async fn recalculate_and_sync_users(
        &self,
        user_ids: Vec<UserId>,
        reason: &str,
    ) {
        if user_ids.is_empty() {
            return;
        }

        // Snapshot assets once for deterministic cross-event recompute.
        let asset_snapshot: HashMap<_, _> = self.assets.iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();

        let tracking_threshold = self.storage.hot_cache_threshold();
        let mut targets_to_update: Vec<LiquidationTarget> = Vec::new();

        for user_id in user_ids {
            if let Some(mut user) = self.users.get_mut(&user_id) {
                let old_hf = user.health_factor;
                let new_hf = HealthFactorCalculator::calculate(&user, &asset_snapshot);

                let old_bucket = user.risk_bucket;
                let new_bucket = RiskBucket::from_hf(new_hf);

                user.health_factor = new_hf;
                user.risk_bucket = new_bucket;

                if new_bucket != old_bucket {
                    tracing::info!(
                        "[{}] User {} bucket change: {:?} -> {:?} (HF: {:.4})",
                        reason,
                        user_id,
                        old_bucket,
                        new_bucket,
                        new_hf
                    );
                }

                if new_bucket == RiskBucket::Liquidate {
                    tracing::warn!("[{}] LIQUIDATION ALERT: User {} HF {:.4}", reason, user_id, new_hf);
                }

                if new_hf < tracking_threshold || old_hf < tracking_threshold {
                    let total_collateral_usd: f64 = user.collateral.iter()
                        .filter_map(|(aid, amount)| {
                            asset_snapshot
                                .get(aid)
                                .map(|a| amount * a.price_in_eth * self.config.reference_eth_price_usd)
                        })
                        .sum();
                    let total_debt_usd: f64 = user.debt.iter()
                        .filter_map(|(aid, amount)| {
                            asset_snapshot
                                .get(aid)
                                .map(|a| amount * a.price_in_eth * self.config.reference_eth_price_usd)
                        })
                        .sum();
                    let ltv = if total_collateral_usd > 0.0 {
                        total_debt_usd / total_collateral_usd
                    } else {
                        0.0
                    };
                    let hf_span = self.config.risk_score_hf_span.max(f64::EPSILON);
                    let risk_score = ((self.config.risk_score_hf_baseline - new_hf) / hf_span
                        * self.config.risk_score_max)
                        .clamp(self.config.risk_score_min, self.config.risk_score_max)
                        as u8;

                    targets_to_update.push(LiquidationTarget {
                        user_address: user.id.clone(),
                        health_factor: new_hf,
                        total_collateral_usd,
                        total_debt_usd,
                        ltv,
                        liquidation_threshold: self.config.default_liquidation_threshold,
                        collateral: user.collateral.clone(),
                        debt: user.debt.clone(),
                        estimated_profit: 0.0,
                        risk_score,
                        last_updated: chrono::Utc::now().timestamp(),
                    });
                }
            }
        }

        for target in targets_to_update {
            if let Err(e) = self.storage.update_user_hf(target).await {
                tracing::error!("[{}] Failed to update storage: {:?}", reason, e);
            }
        }
    }
}
