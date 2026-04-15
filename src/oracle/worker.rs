// Oracle Workers
//
// Background workers cho Oracle module:
// - oracle_price_worker: Poll giá định kỳ từ Chainlink
// - oracle_stats_worker: Log thống kê oracle định kỳ
// - oracle_health_worker: Kiểm tra sức khỏe feeds định kỳ

use std::sync::Arc;
use super::manager::OracleManager;

/// Cấu hình cho oracle workers
#[derive(Debug, Clone)]
pub struct OracleWorkerConfig {
    /// Interval polling giá (ms) — ghi đè config nếu set
    pub poll_interval_ms: u64,
    
    /// Interval log thống kê (seconds)
    pub stats_interval_secs: u64,
    
    /// Interval kiểm tra health (seconds)
    pub health_check_interval_secs: u64,

    /// Interval quét Chainlink AnswerUpdated logs (seconds)
    pub chainlink_event_poll_interval_secs: u64,

    /// Optional WS endpoint cho Chainlink event subscription (push-based)
    pub chainlink_ws_url: Option<String>,

    /// Delay trước khi reconnect WS khi stream lỗi (seconds)
    pub chainlink_ws_reconnect_delay_secs: u64,
}

impl Default for OracleWorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 3000,
            stats_interval_secs: 60,
            health_check_interval_secs: 300, // 5 minutes
            chainlink_event_poll_interval_secs: 3,
            chainlink_ws_url: None,
            chainlink_ws_reconnect_delay_secs: 3,
        }
    }
}

/// Worker Chainlink event: ưu tiên WS subscription, fallback sang polling logs.
pub async fn oracle_chainlink_event_worker(
    oracle: Arc<OracleManager>,
    config: OracleWorkerConfig,
) {
    if let Some(ws_url) = config.chainlink_ws_url.clone() {
        tracing::info!("Oracle Chainlink event worker started in WS mode");
        let mut attempt: u64 = 0;

        loop {
            if attempt > 0 {
                match oracle.poll_all().await {
                    Ok(refreshed) => tracing::warn!(
                        "WS reconnect fallback: refreshed {} feeds via latestRoundData",
                        refreshed
                    ),
                    Err(e) => tracing::error!(
                        "WS reconnect fallback polling failed: {:?}",
                        e
                    ),
                }
            }

            match oracle.watch_chainlink_events_ws(&ws_url).await {
                Ok(_) => tracing::warn!("Oracle Chainlink WS watcher exited unexpectedly"),
                Err(e) => tracing::error!("Oracle Chainlink WS watcher error: {:?}", e),
            }

            attempt += 1;

            tracing::warn!(
                "Reconnecting Chainlink WS in {}s...",
                config.chainlink_ws_reconnect_delay_secs
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(
                config.chainlink_ws_reconnect_delay_secs,
            ))
            .await;
        }
    } else {
        tracing::info!(
            "Oracle Chainlink event worker started in polling fallback mode (interval: {}s)",
            config.chainlink_event_poll_interval_secs
        );

        if let Err(e) = oracle
            .watch_chainlink_events(config.chainlink_event_poll_interval_secs)
            .await
        {
            tracing::error!("Oracle Chainlink polling watcher stopped: {:?}", e);
        }
    }
}

/// Worker chính: Poll giá từ Chainlink định kỳ
///
/// Chạy vô hạn, poll tất cả feeds mỗi `poll_interval_ms`
/// Emit Event::PriceUpdate khi phát hiện deviation đáng kể
pub async fn oracle_price_worker(
    oracle: Arc<OracleManager>,
    config: OracleWorkerConfig,
) {
    tracing::info!(
        "Oracle price worker started (poll interval: {}ms)",
        config.poll_interval_ms
    );
    
    let mut interval = tokio::time::interval(
        tokio::time::Duration::from_millis(config.poll_interval_ms)
    );
    
    loop {
        interval.tick().await;
        
        match oracle.poll_all().await {
            Ok(updates) => {
                if updates > 0 {
                    tracing::debug!("Oracle poll: {} price updates emitted", updates);
                }
            }
            Err(e) => {
                tracing::error!("Oracle poll error: {:?}", e);
            }
        }
    }
}

/// Worker thống kê: In summary giá và trạng thái feeds định kỳ
pub async fn oracle_stats_worker(
    oracle: Arc<OracleManager>,
    config: OracleWorkerConfig,
) {
    tracing::info!(
        "Oracle stats worker started (interval: {}s)",
        config.stats_interval_secs
    );
    
    let mut interval = tokio::time::interval(
        tokio::time::Duration::from_secs(config.stats_interval_secs)
    );
    
    loop {
        interval.tick().await;
        
        let stats = oracle.get_stats().await;
        let prices = oracle.get_all_prices().await;
        
        tracing::info!("━━━━━━━━━ ORACLE STATUS ━━━━━━━━━");
        tracing::info!(
            "  Feeds: {} active, {} stale, {} error",
            stats.active_feeds, stats.stale_feeds, stats.error_feeds
        );
        tracing::info!(
            "  Polls: {}, Updates emitted: {}, Errors: {}",
            stats.total_polls, stats.total_updates_emitted, stats.total_errors
        );
        
        // Log giá hiện tại
        if !prices.is_empty() {
            let mut price_strs: Vec<String> = prices.iter()
                .map(|(asset, price)| format!("{}=${:.2}", asset, price))
                .collect();
            price_strs.sort();
            tracing::info!("  Prices: {}", price_strs.join(", "));
        }
        
        tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
}

/// Worker health check: Kiểm tra sức khỏe feeds và cảnh báo
pub async fn oracle_health_worker(
    oracle: Arc<OracleManager>,
    config: OracleWorkerConfig,
) {
    tracing::info!(
        "Oracle health worker started (interval: {}s)",
        config.health_check_interval_secs
    );
    
    let mut interval = tokio::time::interval(
        tokio::time::Duration::from_secs(config.health_check_interval_secs)
    );
    
    loop {
        interval.tick().await;

        // WS-first mode fallback: refresh stale/missing feeds via latestRoundData.
        match oracle.refresh_stale_feeds().await {
            Ok(refreshed) => {
                if refreshed > 0 {
                    tracing::warn!(
                        "Oracle stale fallback refreshed {} feeds via latestRoundData",
                        refreshed
                    );
                }
            }
            Err(e) => tracing::error!("Oracle stale fallback failed: {:?}", e),
        }
        
        let feeds = oracle.get_all_feed_info().await;
        let mut issues = Vec::new();
        
        for feed in &feeds {
            match &feed.status {
                super::types::FeedStatus::Stale => {
                    issues.push(format!("{} is STALE", feed.asset_id));
                }
                super::types::FeedStatus::Error(e) => {
                    issues.push(format!("{} ERROR: {}", feed.asset_id, e));
                }
                super::types::FeedStatus::Uninitialized => {
                    issues.push(format!("{} not initialized", feed.asset_id));
                }
                super::types::FeedStatus::Active => {
                    // OK
                }
            }
        }
        
        if issues.is_empty() {
            tracing::debug!("Oracle health check: all {} feeds healthy", feeds.len());
        } else {
            tracing::warn!(
                "⚠ Oracle health issues ({}/{}): {}",
                issues.len(),
                feeds.len(),
                issues.join("; ")
            );
        }
    }
}
