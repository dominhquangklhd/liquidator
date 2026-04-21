// Storage Module - Hybrid Architecture
// 
// Provides:
// - Hot Cache: In-memory top N liquidation targets (< 1ms access)
// - Cold Storage: SQLite persistent database (historical + all users)
// - Hybrid Manager: Coordinates both layers with async sync

mod cache;
mod database;
mod models;
pub mod sync;

pub use cache::HotCache;
pub use database::ColdStorage;
pub use models::{
    LiquidationTarget,
    HistoricalSnapshot,
    LiquidationEvent,
    ExecutorSnapshot,
    EventsSnapshot,
    OracleSnapshot,
    ProfitSnapshot,
    ProviderSnapshot,
    WalletBalanceSnapshot,
    RiskSnapshot,
    StrategySnapshot,
    TransactionSnapshots,
};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use anyhow::Result;

/// Hybrid Storage Manager
/// 
/// Combines hot cache (fast, volatile) with cold storage (persistent, slower)
/// for optimal performance in liquidation detection and execution.
pub struct HybridStorage {
    /// Hot cache: Top N targets sorted by health factor
    hot_cache: Arc<RwLock<HotCache>>,
    
    /// Cold storage: SQLite database for all users + history
    cold_storage: Arc<ColdStorage>,
    
    /// Configuration
    config: StorageConfig,

    /// Cache read metrics
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Maximum number of targets in hot cache
    pub hot_cache_size: usize,
    
    /// Health factor threshold for hot cache entry
    pub hot_cache_threshold: f64,
    
    /// Sync interval (seconds)
    pub sync_interval_secs: u64,
    
    /// Database file path
    pub db_path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            hot_cache_size: 100,
            hot_cache_threshold: 1.2,
            sync_interval_secs: 5,
            db_path: "liquidator.db".to_string(),
        }
    }
}

impl HybridStorage {
    /// Create new hybrid storage with default config
    pub async fn new() -> Result<Self> {
        Self::with_config(StorageConfig::default()).await
    }
    
    /// Create new hybrid storage with custom config
    pub async fn with_config(config: StorageConfig) -> Result<Self> {
        tracing::info!("Initializing Hybrid Storage...");
        tracing::info!("  Cache size: {}", config.hot_cache_size);
        tracing::info!("  Cache threshold: HF < {}", config.hot_cache_threshold);
        tracing::info!("  Sync interval: {}s", config.sync_interval_secs);
        tracing::info!("  Database: {}", config.db_path);
        
        // Initialize cold storage (SQLite)
        let cold_storage = Arc::new(ColdStorage::new(&config.db_path).await?);
        
        // Initialize hot cache
        let hot_cache = Arc::new(RwLock::new(HotCache::new(
            config.hot_cache_size,
            config.hot_cache_threshold,
        )));
        
        // Load initial data from DB to cache (cold start recovery)
        let initial_targets = cold_storage.load_risky_users(config.hot_cache_threshold).await?;
        {
            let mut cache = hot_cache.write().await;
            for target in initial_targets {
                cache.insert(target);
            }
        }
        
        tracing::info!("✓ Hybrid Storage initialized");
        
        Ok(Self {
            hot_cache,
            cold_storage,
            config,
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        })
    }
    
    // ============================================================================
    // HOT PATH: Fast operations for liquidation detection
    // ============================================================================
    
    /// Get top N liquidation targets (sorted by health factor)
    /// 
    /// This is the HOT PATH - called frequently during price updates
    /// Latency: < 1ms
    pub async fn get_top_targets(&self, limit: usize) -> Vec<LiquidationTarget> {
        let cache = self.hot_cache.read().await;
        let targets = cache.get_top(limit);
        if targets.is_empty() {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        }
        targets
    }
    
    /// Update user health factor (fast path)
    /// 
    /// Updates hot cache immediately, marks for lazy DB sync
    pub async fn update_user_hf(&self, target: LiquidationTarget) -> Result<()> {
        let should_cache = target.health_factor < self.config.hot_cache_threshold;
        
        // Update hot cache
        {
            let mut cache = self.hot_cache.write().await;
            if should_cache {
                cache.insert(target.clone());
            } else {
                cache.remove(&target.user_address);
            }
        }
        
        // Mark for async DB sync (non-blocking)
        // This will be handled by background sync worker
        
        // Critical event: Log immediately for liquidation
        if target.health_factor < 1.0 {
            tracing::warn!(
                "🚨 LIQUIDATION OPPORTUNITY: {} (HF: {:.4})",
                target.user_address,
                target.health_factor
            );
        }
        
        Ok(())
    }
    
    /// Check if user is in hot cache (quick lookup)
    pub async fn is_hot_target(&self, user_address: &str) -> bool {
        let cache = self.hot_cache.read().await;
        cache.contains(user_address)
    }

    /// Current hot-cache threshold used for target tracking decisions.
    pub fn hot_cache_threshold(&self) -> f64 {
        self.config.hot_cache_threshold
    }
    
    /// Remove target from hot cache (after successful liquidation)
    pub async fn remove_target(&self, user_address: &str) {
        let mut cache = self.hot_cache.write().await;
        cache.remove(user_address);
        tracing::debug!("Removed {} from hot cache", user_address);
    }
    
    // ============================================================================
    // COLD PATH: Database operations (analytics, history)
    // ============================================================================
    
    /// Record liquidation event (persist to DB)
    pub async fn record_liquidation(&self, event: LiquidationEvent) -> Result<i64> {
        self.cold_storage.insert_liquidation(&event).await
    }

    /// Record module snapshots linked to one liquidation row.
    pub async fn record_transaction_snapshots(
        &self,
        liquidation_id: i64,
        snapshots: TransactionSnapshots,
    ) -> Result<()> {
        self.cold_storage
            .insert_transaction_snapshots(liquidation_id, &snapshots)
            .await
    }
    
    /// Get user's health factor history
    pub async fn get_hf_history(
        &self,
        user_address: &str,
        hours: u32,
    ) -> Result<Vec<HistoricalSnapshot>> {
        self.cold_storage.get_hf_history(user_address, hours).await
    }
    
    /// Get all liquidations in time range
    pub async fn get_liquidations(
        &self,
        since_hours: u32,
    ) -> Result<Vec<LiquidationEvent>> {
        self.cold_storage.get_liquidations(since_hours).await
    }

    /// Persist a full batch of targets directly to SQLite.
    ///
    /// This is used by bootstrap flows that need deterministic DB state
    /// without waiting for background sync.
    pub async fn persist_targets_to_db(&self, targets: &[LiquidationTarget]) -> Result<()> {
        self.cold_storage.bulk_upsert_targets(targets).await
    }
    
    /// Load all user addresses from database (for bootstrap)
    pub async fn load_all_user_addresses(&self) -> Result<Vec<ethers::types::Address>> {
        self.cold_storage.load_all_user_addresses().await
    }

    /// Sync admin-managed liquidator wallet list.
    ///
    /// Wallets present in the input are marked active.
    /// Wallets missing from the input are marked inactive.
    pub async fn sync_wallet_registry(&self, wallet_addresses: &[String]) -> Result<()> {
        self.cold_storage.sync_wallet_registry(wallet_addresses).await
    }

    /// Record a batch of wallet balance snapshots.
    pub async fn record_wallet_balances(&self, snapshots: &[WalletBalanceSnapshot]) -> Result<()> {
        self.cold_storage.insert_wallet_balance_snapshots(snapshots).await
    }
    
    // ============================================================================
    // SYNC: Background synchronization
    // ============================================================================
    
    /// Sync hot cache to cold storage (called by background worker)
    pub async fn sync_to_db(&self) -> Result<()> {
        let targets = {
            let cache = self.hot_cache.read().await;
            cache.get_all()
        };
        
        if targets.is_empty() {
            return Ok(());
        }
        
        tracing::debug!("Syncing {} targets to database...", targets.len());
        self.cold_storage.bulk_upsert_targets(&targets).await?;
        tracing::debug!("✓ Sync complete");
        
        Ok(())
    }
    
    /// Spawn background sync worker
    pub fn spawn_sync_worker(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            sync::periodic_sync_worker(self).await;
        })
    }
    
    // ============================================================================
    // ANALYTICS
    // ============================================================================
    
    /// Get storage statistics (for monitoring/debugging)
    pub async fn get_stats(&self) -> StorageStats {
        let cache_size = {
            let cache = self.hot_cache.read().await;
            cache.len()
        };
        
        let total_users = self.cold_storage.count_users().await.unwrap_or(0);
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total_reads = hits + misses;
        let hit_rate = if total_reads == 0 {
            0.0
        } else {
            hits as f64 / total_reads as f64
        };
        
        StorageStats {
            hot_cache_size: cache_size,
            total_users_tracked: total_users,
            cache_hit_rate: hit_rate,
        }
    }
}

#[derive(Debug)]
pub struct StorageStats {
    pub hot_cache_size: usize,
    pub total_users_tracked: i64,
    pub cache_hit_rate: f64,
}
