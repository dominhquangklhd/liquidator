use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};

use liquidator::data::{asset::Asset, user::User};
use liquidator::events::event::Event;
use liquidator::risk::engine::RiskEngine;
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
async fn test_price_update_pipeline_updates_storage_and_block_keeps_state() {
    // Keep a narrow threshold so a single price move can push a user into hot cache.
    let storage = Arc::new(
        HybridStorage::with_config(StorageConfig {
            hot_cache_size: 100,
            hot_cache_threshold: 1.2,
            sync_interval_secs: 60,
            db_path: unique_db_path("risk_event_pipeline_price_only"),
        })
        .await
        .expect("storage init"),
    );

    let (tx, rx) = mpsc::channel(32);
    let mut engine = RiskEngine::with_config(rx, Arc::clone(&storage), Default::default());

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

    // Baseline HF at ETH=1.0: (10 * 1.0 * 0.85) / (6000 * 0.0005) = 2.833 (> 1.2)
    // After price update ETH=0.4: (10 * 0.4 * 0.85) / (6000 * 0.0005) = 1.133 (< 1.2)
    let mut user = User::new("user_pipeline_price_only".to_string());
    user.collateral.insert("ETH".to_string(), 10.0);
    user.debt.insert("USDC".to_string(), 6_000.0);
    engine.users.insert(user.id.clone(), user);
    engine
        .registry
        .add_user_to_asset("ETH".to_string(), "user_pipeline_price_only".to_string());

    let users_ref = Arc::clone(&engine.users);
    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    tx.send(Event::PriceUpdate {
        asset_id: "ETH".to_string(),
        new_price: 0.4,
    })
    .await
    .expect("send price update event");

    let inserted = wait_until(Duration::from_secs(2), || {
        let storage = Arc::clone(&storage);
        async move {
            storage
                .get_top_targets(10)
                .await
                .iter()
                .any(|t| t.user_address == "user_pipeline_price_only")
        }
    })
    .await;
    assert!(inserted, "price-update path should add user into hot cache");

    let hf_after_price = users_ref
        .get("user_pipeline_price_only")
        .expect("user exists")
        .health_factor;
    assert!(
        hf_after_price < 1.2,
        "HF should drop below threshold after price update, got {}",
        hf_after_price
    );

    // Block event is still consumed by the engine and should not undo price-based state.
    tx.send(Event::Block { block_number: 123456 })
        .await
        .expect("send block event");

    sleep(Duration::from_millis(150)).await;

    let still_present = storage
        .get_top_targets(10)
        .await
        .iter()
        .any(|t| t.user_address == "user_pipeline_price_only");
    assert!(
        still_present,
        "block event should not remove price-triggered target from hot cache"
    );

    let hf_after_block = users_ref
        .get("user_pipeline_price_only")
        .expect("user exists")
        .health_factor;
    assert!(
        (hf_after_block - hf_after_price).abs() < 1e-9,
        "block event should keep HF unchanged without new price events"
    );

    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), engine_handle).await;
}
