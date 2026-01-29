mod events;
mod risk;
mod data;
mod executor;
mod mempool;
mod oracle;
mod provider;

use tokio::sync::mpsc;
use std::sync::Arc;
use crate::risk::engine::RiskEngine;
use crate::events::event::Event;
use crate::data::asset::Asset;
use crate::data::user::User;
use crate::provider::AaveProvider;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Liquidator System...");

    // 0. Connect to Aave Fork
    let rpc_url = "http://127.0.0.1:8545";
    let provider = match AaveProvider::new(rpc_url).await {
        Ok(p) => {
            tracing::info!("✓ Connected to Aave fork successfully!");
            Arc::new(p)
        }
        Err(e) => {
            tracing::error!("✗ Failed to connect to Aave fork: {:?}", e);
            tracing::error!("Please ensure Anvil/Hardhat is running at {}", rpc_url);
            return;
        }
    };

    // 1. Setup Channels
    let (tx, rx) = mpsc::channel(100);

    // 2. Initialize Risk Engine
    let mut engine = RiskEngine::new(rx);

    // 3. Populate Initial Data (Simulation)
    {
        // Add Assets
        let eth = Asset {
            id: "ETH".to_string(),
            symbol: "ETH".to_string(),
            decimals: 18,
            ltv: 0.80,
            liquidation_threshold: 0.85,
            price_in_eth: 1.0, 
        };
        let usdc = Asset {
            id: "USDC".to_string(),
            symbol: "USDC".to_string(),
            decimals: 6,
            ltv: 0.80,
            liquidation_threshold: 0.85,
            price_in_eth: 0.0005, // 1 ETH = 2000 USDC
        };
        engine.assets.insert("ETH".to_string(), eth);
        engine.assets.insert("USDC".to_string(), usdc);

        // Add User 1: Safe
        let mut user1 = User::new("user_safe".to_string());
        user1.collateral.insert("ETH".to_string(), 10.0); // 10 ETH Collateral
        user1.debt.insert("USDC".to_string(), 5000.0);    // 5000 USDC Debt (~2.5 ETH)
        // HF approx: (10 * 1.0 * 0.85) / (5000 * 0.0005) = 8.5 / 2.5 = 3.4 (Safe)
        engine.users.insert("user_safe".to_string(), user1);
        engine.registry.add_user_to_asset("ETH".to_string(), "user_safe".to_string());
        engine.registry.add_user_to_asset("USDC".to_string(), "user_safe".to_string());

        // Add User 2: Risky
        let mut user2 = User::new("user_risky".to_string());
        user2.collateral.insert("ETH".to_string(), 10.0); // 10 ETH Collateral
        user2.debt.insert("USDC".to_string(), 16000.0);   // 16000 USDC Debt (~8 ETH)
        // HF approx: (10 * 1.0 * 0.85) / (16000 * 0.0005) = 8.5 / 8.0 = 1.0625 (Danger)
        engine.users.insert("user_risky".to_string(), user2);
        engine.registry.add_user_to_asset("ETH".to_string(), "user_risky".to_string());
        engine.registry.add_user_to_asset("USDC".to_string(), "user_risky".to_string());
    }

    // 4. Spawn Engine in background
    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // 5. Spawn Block Watcher
    let provider_clone = Arc::clone(&provider);
    tokio::spawn(async move {
        if let Err(e) = provider_clone.watch_blocks().await {
            tracing::error!("Block watcher error: {:?}", e);
        }
    });

    // 6. Simulate Events
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        // Wait a bit
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        tracing::info!("--- SIMULATION START: ETH CRASH ---");
        // Simulate ETH Price Crash from 1.0 -> 0.8
        // User 2: (10 * 0.9 * 0.85) / 8 = 7.65 / 8 = 0.95 -> LIQUIDATE
        
        // Step 1: ETH drops to 0.9
        tx_clone.send(Event::PriceUpdate {
            asset_id: "ETH".to_string(),
            new_price: 0.9, 
        }).await.unwrap();
    });

    // Keep main alive for simulation
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
}
