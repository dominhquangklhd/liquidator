# DeFi Risk Monitoring System

A high-performance, event-driven risk monitoring system in Rust for a DeFi lending protocol.

## Architecture

The system is designed with a clean, modular architecture:

-   **`src/events`**: Defines the `Event` enum (Price, Block) and the async `Dispatcher`.
-   **`src/risk`**: Contains the core risk logic:
    -   `engine.rs`: The main event loop processing updates.
    -   `health_factor.rs`: Logic to calculate User HF.
    -   `bucket.rs`: Risk bucket classification (Safe, Watch, Risk, Danger, Liquidate).
-   **`src/data`**: Domain models (`User`, `Asset`) and the `Registry` (Asset -> User index).
-   **`src/executor`**: Placeholder for liquidation execution.

## Key Features

-   **Event-Driven**: Reacts to price updates and block ticks immediately.
-   **O(1) Lookup**: Uses an Asset Registry to find affected users without scanning the entire user base.
-   **Incremental Updates**: Only recalculates HF for users affected by specific asset price changes.
-   **Concurrency**: Uses `dashmap` for concurrent handling of state (though currently single-threaded runner for strict ordering, can be parallelized).

## How to Run

1.  Ensure you have Rust installed.
2.  **IMPORTANT**: You must have the MSVC C++ Build Tools installed (specifically `link.exe`) on Windows.
3.  Run the simulation:

```bash
cargo run
```

## Simulation Flow

When you run the project, `main.rs` sets up a scenario:
1.  Populates `ETH` and `USDC` assets.
2.  Creates a **Safe User** (HF ~3.4) and a **Risky User** (HF ~1.06).
3.  Simulates an **ETH Price Crash** (1.0 -> 0.9).
4.  The system detects the price change, identifies users holding ETH, recalculates HF.
5.  **Risky User** drops below HF 1.0 (Liquidation Threshold).
6.  System logs a `LIQUIDATION ALERT`.

## Engineering Decisions

-   **DashMap**: Used for thread-safe access to User and Asset state, allowing future multi-threaded readers (e.g., API).
-   **Tokio Channels**: Used for the event bus to decouple event producers (Network/RPC) from the Consumer (Risk Engine).
-   **Registry**: An inverse index (`AssetId -> Vec<UserId>`) is crucial for avoiding O(N) scans.
