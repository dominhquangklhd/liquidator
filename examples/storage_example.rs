// Example: Using Hybrid Storage
//
// This example demonstrates how to integrate Hybrid Storage
// into the liquidator system.

use liquidator::storage::{HybridStorage, StorageConfig, LiquidationTarget};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    
    // ============================================================================
    // STEP 1: Initialize Storage
    // ============================================================================
    
    let config = StorageConfig {
        hot_cache_size: 100,
        hot_cache_threshold: 1.2,
        sync_interval_secs: 5,
        db_path: "liquidator.db".to_string(),
    };
    
    let storage = Arc::new(HybridStorage::with_config(config).await?);
    
    // ============================================================================
    // STEP 2: Spawn Background Workers
    // ============================================================================
    
    // Sync worker: Cache -> DB every 5s
    let storage_clone = Arc::clone(&storage);
    let _sync_handle = storage_clone.spawn_sync_worker();
    
    // Snapshot worker: Historical records every 60s
    let storage_clone = Arc::clone(&storage);
    tokio::spawn(async move {
        liquidator::storage::sync::snapshot_worker(storage_clone, 60).await;
    });
    
    // Stats logger: Print stats every 30s
    let storage_clone = Arc::clone(&storage);
    tokio::spawn(async move {
        liquidator::storage::sync::stats_logger_worker(storage_clone, 30).await;
    });
    
    // ============================================================================
    // STEP 3: Simulate Adding Targets
    // ============================================================================
    
    tracing::info!("Simulating liquidation targets...");
    
    // User 1: High risk (HF = 1.05)
    let mut target1 = LiquidationTarget::new("0xABC123...".to_string());
    target1.health_factor = 1.05;
    target1.total_collateral_usd = 10000.0;
    target1.total_debt_usd = 9000.0;
    target1.calculate_risk_score();
    target1.estimate_profit(0.05); // 5% bonus
    
    storage.update_user_hf(target1).await?;
    
    // User 2: Medium risk (HF = 1.15)
    let mut target2 = LiquidationTarget::new("0xDEF456...".to_string());
    target2.health_factor = 1.15;
    target2.total_collateral_usd = 20000.0;
    target2.total_debt_usd = 16000.0;
    target2.calculate_risk_score();
    target2.estimate_profit(0.05);
    
    storage.update_user_hf(target2).await?;
    
    // User 3: Critical (HF = 0.95 - Already liquidatable!)
    let mut target3 = LiquidationTarget::new("0xGHI789...".to_string());
    target3.health_factor = 0.95;
    target3.total_collateral_usd = 50000.0;
    target3.total_debt_usd = 51000.0;
    target3.calculate_risk_score();
    target3.estimate_profit(0.05);
    
    storage.update_user_hf(target3).await?;
    
    // ============================================================================
    // STEP 4: Query Top Targets (HOT PATH)
    // ============================================================================
    
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    tracing::info!("\n{}", "=".repeat(60));
    tracing::info!("TOP LIQUIDATION TARGETS:");
    tracing::info!("{}", "=".repeat(60));
    
    let top_targets = storage.get_top_targets(10).await;
    for (i, target) in top_targets.iter().enumerate() {
        tracing::info!(
            "{}. {} - HF: {:.4} | Risk: {}/10 | Profit: ${:.2}",
            i + 1,
            target.user_address,
            target.health_factor,
            target.risk_score,
            target.estimated_profit
        );
    }
    
    // ============================================================================
    // STEP 5: Simulate Liquidation
    // ============================================================================
    
    if let Some(victim) = top_targets.first() {
        if victim.health_factor < 1.0 {
            tracing::info!("\n🎯 Executing liquidation on {}", victim.user_address);
            
            // Record liquidation event
            let event = liquidator::storage::LiquidationEvent::new(
                victim.user_address.clone(),
                "ETH".to_string(),
                "USDC".to_string(),
                5.0,  // 5 ETH seized
                10000.0,  // $10,000 debt covered
                "0xLiquidatorBot...".to_string(),
                "0xTxHash123...".to_string(),
            );
            
            storage.record_liquidation(event).await?;
            tracing::info!("✓ Liquidation recorded");
        }
    }
    
    // ============================================================================
    // STEP 6: Analytics Query (COLD PATH)
    // ============================================================================
    
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    tracing::info!("\n{}", "=".repeat(60));
    tracing::info!("ANALYTICS:");
    tracing::info!("{}", "=".repeat(60));
    
    let liquidations = storage.get_liquidations(24).await?;
    tracing::info!("Total liquidations (last 24h): {}", liquidations.len());
    
    for liq in liquidations {
        tracing::info!(
            "  - {} liquidated {} {} for {} {}",
            liq.liquidator,
            liq.collateral_seized,
            liq.collateral_asset,
            liq.debt_covered,
            liq.debt_asset
        );
    }
    
    // Keep alive
    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    
    Ok(())
}
