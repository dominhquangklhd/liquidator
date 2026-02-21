// Background Sync Worker
//
// Periodically syncs hot cache to cold storage (SQLite)
// for persistence without blocking the hot path.

use super::HybridStorage;
use std::sync::Arc;
use tokio::time::{interval, Duration};

/// Periodic sync worker
/// 
/// Runs in background and syncs hot cache to DB every N seconds.
/// Non-blocking: Does not impact liquidation detection performance.
pub async fn periodic_sync_worker(storage: Arc<HybridStorage>) {
    let sync_interval = storage.config.sync_interval_secs;
    let mut ticker = interval(Duration::from_secs(sync_interval));
    
    tracing::info!("Background sync worker started (interval: {}s)", sync_interval);
    
    loop {
        ticker.tick().await;
        
        match storage.sync_to_db().await {
            Ok(()) => {
                // Success - no log needed (already logged in sync_to_db)
            }
            Err(e) => {
                tracing::error!("Database sync failed: {:?}", e);
                tracing::warn!("Hot cache still valid, will retry in {}s", sync_interval);
            }
        }
    }
}

/// Snapshot worker: Create historical records
/// 
/// Periodically snapshots all targets for time-series analysis.
pub async fn snapshot_worker(storage: Arc<HybridStorage>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    
    tracing::info!("Snapshot worker started (interval: {}s)", interval_secs);
    
    loop {
        ticker.tick().await;
        
        let targets = storage.get_top_targets(100).await;
        
        for target in &targets {
            let snapshot = super::models::HistoricalSnapshot {
                user_address: target.user_address.clone(),
                timestamp: chrono::Utc::now().timestamp(),
                health_factor: target.health_factor,
                total_collateral_usd: target.total_collateral_usd,
                total_debt_usd: target.total_debt_usd,
            };
            
            if let Err(e) = storage.cold_storage.insert_snapshot(&snapshot).await {
                tracing::warn!("Failed to insert snapshot: {:?}", e);
            }
        }
        
        tracing::debug!("Snapshot created for {} targets", targets.len());
    }
}

/// Stats logger: Periodic logging of storage metrics
pub async fn stats_logger_worker(storage: Arc<HybridStorage>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    
    loop {
        ticker.tick().await;
        
        let stats = storage.get_stats().await;
        
        tracing::info!(
            "Storage Stats: hot_cache={}/{} users={} cache_hit_rate={:.2}%",
            stats.hot_cache_size,
            storage.config.hot_cache_size,
            stats.total_users_tracked,
            stats.cache_hit_rate * 100.0
        );
        
        // Get top 5 targets
        let top_targets = storage.get_top_targets(5).await;
        if !top_targets.is_empty() {
            tracing::info!("Top 5 liquidation targets:");
            for (i, target) in top_targets.iter().enumerate() {
                tracing::info!(
                    "  {}. {} (HF: {:.4}, Risk: {}/10, Profit: ${:.2})",
                    i + 1,
                    target.user_address,
                    target.health_factor,
                    target.risk_score,
                    target.estimated_profit
                );
            }
        } else {
            tracing::info!("No risky positions detected");
        }
    }
}

/// Memory monitor: Ensure hot cache doesn't grow too large
pub async fn memory_monitor_worker(storage: Arc<HybridStorage>) {
    let mut ticker = interval(Duration::from_secs(30));
    
    loop {
        ticker.tick().await;
        
        let stats = storage.get_stats().await;
        let usage_pct = (stats.hot_cache_size as f64 / storage.config.hot_cache_size as f64) * 100.0;
        
        if usage_pct > 90.0 {
            tracing::warn!(
                "Hot cache nearly full: {}/{} ({:.1}%)",
                stats.hot_cache_size,
                storage.config.hot_cache_size,
                usage_pct
            );
        }
        
        if stats.hot_cache_size > storage.config.hot_cache_size {
            tracing::error!(
                "Hot cache overflow detected! Size: {} > Max: {}",
                stats.hot_cache_size,
                storage.config.hot_cache_size
            );
            
            // Force eviction would happen automatically in cache.rs
            // This is just monitoring/alerting
        }
    }
}
