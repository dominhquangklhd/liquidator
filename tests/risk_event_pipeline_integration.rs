use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};

use liquidator::data::{asset::Asset, user::User};
use liquidator::events::event::Event;
use liquidator::risk::engine::{RiskEngine, RiskEngineConfig};
use liquidator::storage::{HybridStorage, StorageConfig};

fn unique_db_path(prefix: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join(format!("{}_{}.db", prefix, ts))
        .to_string_lossy()
        .to_string()
}

async fn wait_until<F, Fut>(timeout: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check().await {
            return true;
        }
        sleep(Duration::from_millis(20)).await;
    }
    check().await
}

#[tokio::test]
async fn test_mempool_then_block_updates_storage_pipeline() {
    // Keep a narrow threshold to clearly see cache add/remove transitions.
    let storage = Arc::new(
        HybridStorage::with_config(StorageConfig {
            hot_cache_size: 100,
            hot_cache_threshold: 1.2,
            sync_interval_secs: 60,
            db_path: unique_db_path("risk_event_pipeline"),
        })
        .await
        .expect("storage init"),
    );

    let (tx, rx) = mpsc::channel(32);
    let mut engine = RiskEngine::with_config(
        rx,
        Arc::clone(&storage),
        RiskEngineConfig {
            mempool_speculative_hf_penalty: 0.05,
            ..Default::default()
        },
    );

    // Asset setup (ETH base + USDC quoted in ETH)
    engine.assets.insert(
        "ETH".to_string(),
        Asset {
            id: "ETH".to_string(),
            symbol: "ETH".to_string(),
            decimals: 18,
            ltv: 0.80,
            liquidation_threshold: 0.85,
            price_in_eth: 1.0,
        },
    );
    engine.assets.insert(
        "USDC".to_string(),
        Asset {
            id: "USDC".to_string(),
            symbol: "USDC".to_string(),
            decimals: 6,
            ltv: 0.80,
            liquidation_threshold: 0.85,
            price_in_eth: 0.0005,
        },
    );

    // This position has HF ~1.214 (>1.2 threshold), so not in hot cache at baseline.
    let mut user = User::new("user_pipeline".to_string());
    user.collateral.insert("ETH".to_string(), 10.0);
    user.debt.insert("USDC".to_string(), 14_000.0);
    engine.users.insert(user.id.clone(), user);
    engine
        .registry
        .add_user_to_asset("ETH".to_string(), "user_pipeline".to_string());
    engine
        .registry
        .add_user_to_asset("USDC".to_string(), "user_pipeline".to_string());

    let users_ref = Arc::clone(&engine.users);
    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // 1) Mempool event should apply speculative penalty and push user into hot cache.
    tx.send(Event::MempoolTx {
        user_id: "user_pipeline".to_string(),
        affected_assets: vec!["ETH".to_string(), "USDC".to_string()],
    })
    .await
    .expect("send mempool event");

    let inserted = wait_until(Duration::from_secs(2), || {
        let storage = Arc::clone(&storage);
        async move {
            storage
                .get_top_targets(10)
                .await
                .iter()
                .any(|t| t.user_address == "user_pipeline")
        }
    })
    .await;
    assert!(inserted, "mempool speculative path should add user into hot cache");

    // Check HF was penalized in-memory.
    let hf_after_mempool = users_ref
        .get("user_pipeline")
        .expect("user exists")
        .health_factor;
    assert!(
        hf_after_mempool < 1.2,
        "speculative HF should drop below threshold, got {}",
        hf_after_mempool
    );

    // 2) New block should reconcile using non-speculative HF and remove from cache.
    tx.send(Event::Block { block_number: 123456 })
        .await
        .expect("send block event");

    let removed = wait_until(Duration::from_secs(2), || {
        let storage = Arc::clone(&storage);
        async move {
            !storage
                .get_top_targets(10)
                .await
                .iter()
                .any(|t| t.user_address == "user_pipeline")
        }
    })
    .await;
    assert!(removed, "block reconciliation should remove user from hot cache");

    let hf_after_block = users_ref
        .get("user_pipeline")
        .expect("user exists")
        .health_factor;
    assert!(
        hf_after_block > 1.2,
        "post-block HF should recover above threshold, got {}",
        hf_after_block
    );

    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), engine_handle).await;
}
