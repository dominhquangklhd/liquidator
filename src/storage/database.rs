// Cold Storage Layer - SQLite Database
//
// Provides persistent storage for:
// - All user positions
// - Historical snapshots
// - Liquidation events
// - Analytics data

use super::models::{
    LiquidationTarget,
    HistoricalSnapshot,
    LiquidationEvent,
    TransactionSnapshots,
    WalletBalanceSnapshot,
};
use anyhow::{Context, Result};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::str::FromStr;

/// Cold storage (SQLite database)
pub struct ColdStorage {
    pool: SqlitePool,
}

impl ColdStorage {
    /// Create new cold storage and initialize schema
    pub async fn new(db_path: &str) -> Result<Self> {
        let connect_options = SqliteConnectOptions::from_str(&format!("sqlite://{}?mode=rwc", db_path))
            .context("Failed to parse SQLite connection string")?
            .foreign_keys(true);

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options)
            .await
            .context("Failed to connect to SQLite database")?;
        
        // Initialize schema
        Self::init_schema(&pool).await?;
        
        tracing::info!("✓ Cold storage initialized at {}", db_path);
        
        Ok(Self { pool })
    }
    
    /// Initialize database schema
    async fn init_schema(pool: &SqlitePool) -> Result<()> {
        // Table 1: Current user positions
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                user_address TEXT PRIMARY KEY,
                health_factor REAL NOT NULL,
                total_collateral_usd REAL NOT NULL,
                total_debt_usd REAL NOT NULL,
                ltv REAL NOT NULL,
                liquidation_threshold REAL NOT NULL,
                collateral_json TEXT NOT NULL,
                debt_json TEXT NOT NULL,
                estimated_profit REAL NOT NULL,
                risk_score INTEGER NOT NULL,
                last_updated INTEGER NOT NULL,
                in_hot_cache INTEGER NOT NULL DEFAULT 0
            )
            "#
        )
        .execute(pool)
        .await?;
        
        // Indexes for fast queries
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_hf ON users(health_factor)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_updated ON users(last_updated)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_users_hot ON users(in_hot_cache, health_factor)")
            .execute(pool)
            .await?;
        
        // Table 2: Historical snapshots
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hf_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_address TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                health_factor REAL NOT NULL,
                total_collateral_usd REAL NOT NULL,
                total_debt_usd REAL NOT NULL
            )
            "#
        )
        .execute(pool)
        .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_history_user_time ON hf_history(user_address, timestamp)")
            .execute(pool)
            .await?;
        
        // Table 3: Liquidation events
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS liquidations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_address TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                collateral_asset TEXT NOT NULL,
                debt_asset TEXT NOT NULL,
                collateral_seized REAL NOT NULL,
                debt_covered REAL NOT NULL,
                liquidator TEXT NOT NULL,
                tx_hash TEXT NOT NULL UNIQUE,
                profit_usd REAL NOT NULL,
                gas_cost_usd REAL NOT NULL,
                status TEXT NOT NULL DEFAULT 'success',
                error_message TEXT
            )
            "#
        )
        .execute(pool)
        .await?;

        // Backward-compatible migration for existing DB files.
        // Ignore duplicate-column errors if columns already exist.
        for migration in [
            "ALTER TABLE liquidations ADD COLUMN status TEXT NOT NULL DEFAULT 'success'",
            "ALTER TABLE liquidations ADD COLUMN error_message TEXT",
        ] {
            if let Err(e) = sqlx::query(migration).execute(pool).await {
                let msg = e.to_string().to_ascii_lowercase();
                if !msg.contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_liquidations_user ON liquidations(user_address)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_liquidations_time ON liquidations(timestamp)")
            .execute(pool)
            .await?;

        // Backward-compatible migration for legacy snapshot table names.
        // Ignore missing-table errors and name-collision errors when target tables already exist.
        for (old_table, new_table) in [
            ("executor_snapshots", "executor"),
            ("event_snapshots", "event"),
            ("oracle_snapshots", "oracle"),
            ("profit_snapshots", "profit"),
            ("provider_snapshots", "provider"),
            ("risk_snapshots", "risk"),
            ("strategy_snapshots", "strategy"),
        ] {
            let migration = format!("ALTER TABLE {} RENAME TO {}", old_table, new_table);
            if let Err(e) = sqlx::query(&migration).execute(pool).await {
                let msg = e.to_string().to_ascii_lowercase();
                if !msg.contains("no such table") && !msg.contains("already exists") {
                    return Err(e.into());
                }
            }
        }

        // Table 4: Executor snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS executor (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                status TEXT NOT NULL,
                execution_method TEXT NOT NULL,
                tx_hash TEXT,
                gas_used INTEGER NOT NULL,
                gas_price INTEGER NOT NULL,
                error_message TEXT,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 5: Events snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS event (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                event_name TEXT NOT NULL,
                block_number INTEGER,
                payload_json TEXT NOT NULL,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 6: Oracle snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS oracle (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                primary_source TEXT NOT NULL,
                observed_assets_json TEXT NOT NULL,
                note TEXT,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 7: Profit snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS profit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                estimated_profit_usd REAL NOT NULL,
                realized_profit_usd REAL NOT NULL,
                gas_cost_usd REAL NOT NULL,
                net_profit_usd REAL NOT NULL,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 8: Provider snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS provider (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                chain_id INTEGER NOT NULL,
                wallet_address TEXT NOT NULL,
                wallet_balance_wei TEXT NOT NULL DEFAULT '0',
                wallet_balance_eth REAL NOT NULL DEFAULT 0,
                pending_tx_count INTEGER NOT NULL,
                rpc_latency_ms INTEGER,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Backward-compatible migration for provider table balance columns.
        for migration in [
            "ALTER TABLE provider ADD COLUMN wallet_balance_wei TEXT NOT NULL DEFAULT '0'",
            "ALTER TABLE provider ADD COLUMN wallet_balance_eth REAL NOT NULL DEFAULT 0",
        ] {
            if let Err(e) = sqlx::query(migration).execute(pool).await {
                let msg = e.to_string().to_ascii_lowercase();
                if !msg.contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }

        // Table 9: Risk snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS risk (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                health_factor REAL NOT NULL,
                total_collateral_usd REAL NOT NULL,
                total_debt_usd REAL NOT NULL,
                ltv REAL NOT NULL,
                liquidation_threshold REAL NOT NULL,
                risk_score INTEGER NOT NULL,
                collateral_json TEXT NOT NULL,
                debt_json TEXT NOT NULL,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 10: Strategy snapshots (1 row per liquidation)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS strategy (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                liquidation_id INTEGER NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                execution_method TEXT NOT NULL,
                reasoning TEXT NOT NULL,
                adjusted_profit_usd REAL NOT NULL,
                is_executable INTEGER NOT NULL,
                plan_context_json TEXT NOT NULL,
                FOREIGN KEY(liquidation_id) REFERENCES liquidations(id) ON DELETE CASCADE
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 11: Wallet balances (historical view)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS wallets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                chain_id INTEGER NOT NULL,
                wallet_address TEXT NOT NULL,
                balance_wei TEXT NOT NULL,
                balance_eth REAL NOT NULL
            )
            "#
        )
        .execute(pool)
        .await?;

        // Table 12: Admin-managed wallet registry
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS wallet_registry (
                wallet_address TEXT PRIMARY KEY,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                last_updated INTEGER NOT NULL
            )
            "#
        )
        .execute(pool)
        .await?;

        for index_stmt in [
            "CREATE INDEX IF NOT EXISTS idx_executor_liq ON executor(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_event_liq ON event(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_oracle_liq ON oracle(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_profit_liq ON profit(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_provider_liq ON provider(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_risk_liq ON risk(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_strategy_liq ON strategy(liquidation_id)",
            "CREATE INDEX IF NOT EXISTS idx_wallets_wallet_time ON wallets(wallet_address, timestamp)",
            "CREATE INDEX IF NOT EXISTS idx_wallet_registry_active ON wallet_registry(is_active, last_updated)",
        ] {
            sqlx::query(index_stmt).execute(pool).await?;
        }
        
        Ok(())
    }
    
    // ============================================================================
    // USER OPERATIONS
    // ============================================================================
    
    /// Insert or update user target
    pub async fn upsert_target(&self, target: &LiquidationTarget) -> Result<()> {
        let collateral_json = serde_json::to_string(&target.collateral)?;
        let debt_json = serde_json::to_string(&target.debt)?;
        
        sqlx::query(
            r#"
            INSERT INTO users (
                user_address, health_factor, total_collateral_usd, total_debt_usd,
                ltv, liquidation_threshold, collateral_json, debt_json,
                estimated_profit, risk_score, last_updated, in_hot_cache
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
            ON CONFLICT(user_address) DO UPDATE SET
                health_factor = excluded.health_factor,
                total_collateral_usd = excluded.total_collateral_usd,
                total_debt_usd = excluded.total_debt_usd,
                ltv = excluded.ltv,
                liquidation_threshold = excluded.liquidation_threshold,
                collateral_json = excluded.collateral_json,
                debt_json = excluded.debt_json,
                estimated_profit = excluded.estimated_profit,
                risk_score = excluded.risk_score,
                last_updated = excluded.last_updated
            "#
        )
        .bind(&target.user_address)
        .bind(target.health_factor)
        .bind(target.total_collateral_usd)
        .bind(target.total_debt_usd)
        .bind(target.ltv)
        .bind(target.liquidation_threshold)
        .bind(&collateral_json)
        .bind(&debt_json)
        .bind(target.estimated_profit)
        .bind(target.risk_score as i32)
        .bind(target.last_updated)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Bulk upsert targets (optimized for batch sync)
    pub async fn bulk_upsert_targets(&self, targets: &[LiquidationTarget]) -> Result<()> {
        if targets.is_empty() {
            return Ok(());
        }
        
        let mut tx = self.pool.begin().await?;

        // Reset previous snapshot. Current sync batch will mark active hot targets.
        sqlx::query("UPDATE users SET in_hot_cache = 0")
            .execute(&mut *tx)
            .await?;
        
        for target in targets {
            let collateral_json = serde_json::to_string(&target.collateral)?;
            let debt_json = serde_json::to_string(&target.debt)?;
            
            sqlx::query(
                r#"
                INSERT INTO users (
                    user_address, health_factor, total_collateral_usd, total_debt_usd,
                    ltv, liquidation_threshold, collateral_json, debt_json,
                    estimated_profit, risk_score, last_updated, in_hot_cache
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
                ON CONFLICT(user_address) DO UPDATE SET
                    health_factor = excluded.health_factor,
                    total_collateral_usd = excluded.total_collateral_usd,
                    total_debt_usd = excluded.total_debt_usd,
                    ltv = excluded.ltv,
                    liquidation_threshold = excluded.liquidation_threshold,
                    collateral_json = excluded.collateral_json,
                    debt_json = excluded.debt_json,
                    estimated_profit = excluded.estimated_profit,
                    risk_score = excluded.risk_score,
                    last_updated = excluded.last_updated,
                    in_hot_cache = 1
                "#
            )
            .bind(&target.user_address)
            .bind(target.health_factor)
            .bind(target.total_collateral_usd)
            .bind(target.total_debt_usd)
            .bind(target.ltv)
            .bind(target.liquidation_threshold)
            .bind(&collateral_json)
            .bind(&debt_json)
            .bind(target.estimated_profit)
            .bind(target.risk_score as i32)
            .bind(target.last_updated)
            .execute(&mut *tx)
            .await?;
        }
        
        tx.commit().await?;
        Ok(())
    }
    
    /// Load risky users from DB (for cold start recovery)
    pub async fn load_risky_users(&self, threshold: f64) -> Result<Vec<LiquidationTarget>> {
        let rows = sqlx::query_as::<_, UserRow>(
            r#"
            SELECT * FROM users
            WHERE health_factor < ?
            ORDER BY health_factor ASC
            LIMIT 100
            "#
        )
        .bind(threshold)
        .fetch_all(&self.pool)
        .await?;
        
        rows.into_iter()
            .map(|row| row.into_target())
            .collect::<Result<Vec<_>>>()
    }
    
    /// Count total users tracked
    pub async fn count_users(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }
    
    /// Load all user addresses from DB (for bootstrap)
    pub async fn load_all_user_addresses(&self) -> Result<Vec<ethers::types::Address>> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT user_address FROM users ORDER BY last_updated DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter()
            .filter_map(|(addr_str,)| {
                // Parse address from stored format (either 0x... or other format)
                match addr_str.parse::<ethers::types::Address>() {
                    Ok(addr) => Some(addr),
                    Err(_) => {
                        tracing::warn!("Failed to parse user address from DB: {}", addr_str);
                        None
                    }
                }
            })
            .collect())
    }

    /// Sync wallet registry from admin-defined wallet list.
    pub async fn sync_wallet_registry(&self, wallet_addresses: &[String]) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;

        sqlx::query("UPDATE wallet_registry SET is_active = 0, last_updated = ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;

        for wallet_address in wallet_addresses {
            sqlx::query(
                r#"
                INSERT INTO wallet_registry (wallet_address, is_active, created_at, last_updated)
                VALUES (?, 1, ?, ?)
                ON CONFLICT(wallet_address) DO UPDATE SET
                    is_active = 1,
                    last_updated = excluded.last_updated
                "#
            )
            .bind(wallet_address)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Insert wallet balance snapshots.
    pub async fn insert_wallet_balance_snapshots(&self, snapshots: &[WalletBalanceSnapshot]) -> Result<()> {
        if snapshots.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for snapshot in snapshots {
            sqlx::query(
                r#"
                INSERT INTO wallets (
                    timestamp, chain_id, wallet_address, balance_wei, balance_eth
                ) VALUES (?, ?, ?, ?, ?)
                "#
            )
            .bind(snapshot.timestamp)
            .bind(snapshot.chain_id)
            .bind(&snapshot.wallet_address)
            .bind(&snapshot.balance_wei)
            .bind(snapshot.balance_eth)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
    
    // ============================================================================
    // HISTORICAL DATA
    // ============================================================================
    
    /// Insert historical snapshot
    pub async fn insert_snapshot(&self, snapshot: &HistoricalSnapshot) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hf_history (user_address, timestamp, health_factor, total_collateral_usd, total_debt_usd)
            VALUES (?, ?, ?, ?, ?)
            "#
        )
        .bind(&snapshot.user_address)
        .bind(snapshot.timestamp)
        .bind(snapshot.health_factor)
        .bind(snapshot.total_collateral_usd)
        .bind(snapshot.total_debt_usd)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Get user's health factor history
    pub async fn get_hf_history(&self, user_address: &str, hours: u32) -> Result<Vec<HistoricalSnapshot>> {
        let since = chrono::Utc::now().timestamp() - (hours as i64 * 3600);
        
        let rows = sqlx::query_as::<_, HistoricalSnapshot>(
            r#"
            SELECT user_address, timestamp, health_factor, total_collateral_usd, total_debt_usd
            FROM hf_history
            WHERE user_address = ? AND timestamp > ?
            ORDER BY timestamp ASC
            "#
        )
        .bind(user_address)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows)
    }
    
    // ============================================================================
    // LIQUIDATION EVENTS
    // ============================================================================
    
    /// Insert liquidation event
    pub async fn insert_liquidation(&self, event: &LiquidationEvent) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO liquidations (
                user_address, timestamp, collateral_asset, debt_asset,
                collateral_seized, debt_covered, liquidator, tx_hash,
                profit_usd, gas_cost_usd, status, error_message
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&event.user_address)
        .bind(event.timestamp)
        .bind(&event.collateral_asset)
        .bind(&event.debt_asset)
        .bind(event.collateral_seized)
        .bind(event.debt_covered)
        .bind(&event.liquidator)
        .bind(&event.tx_hash)
        .bind(event.profit_usd)
        .bind(event.gas_cost_usd)
        .bind(&event.status)
        .bind(&event.error_message)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Insert cross-module snapshots linked to a liquidation row.
    pub async fn insert_transaction_snapshots(
        &self,
        liquidation_id: i64,
        snapshots: &TransactionSnapshots,
    ) -> Result<()> {
        let collateral_json = serde_json::to_string(&snapshots.risk.collateral)?;
        let debt_json = serde_json::to_string(&snapshots.risk.debt)?;

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO executor (
                liquidation_id, timestamp, status, execution_method, tx_hash,
                gas_used, gas_price, error_message
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.executor.timestamp)
        .bind(&snapshots.executor.status)
        .bind(&snapshots.executor.execution_method)
        .bind(&snapshots.executor.tx_hash)
        .bind(u64_to_i64(snapshots.executor.gas_used))
        .bind(u64_to_i64(snapshots.executor.gas_price))
        .bind(&snapshots.executor.error_message)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO event (
                liquidation_id, timestamp, event_name, block_number, payload_json
            ) VALUES (?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.events.timestamp)
        .bind(&snapshots.events.event_name)
        .bind(snapshots.events.block_number)
        .bind(&snapshots.events.payload_json)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO oracle (
                liquidation_id, timestamp, primary_source, observed_assets_json, note
            ) VALUES (?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.oracle.timestamp)
        .bind(&snapshots.oracle.primary_source)
        .bind(&snapshots.oracle.observed_assets_json)
        .bind(&snapshots.oracle.note)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO profit (
                liquidation_id, timestamp, estimated_profit_usd, realized_profit_usd,
                gas_cost_usd, net_profit_usd
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.profit.timestamp)
        .bind(snapshots.profit.estimated_profit_usd)
        .bind(snapshots.profit.realized_profit_usd)
        .bind(snapshots.profit.gas_cost_usd)
        .bind(snapshots.profit.net_profit_usd)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO provider (
                liquidation_id, timestamp, chain_id, wallet_address,
                wallet_balance_wei, wallet_balance_eth, pending_tx_count, rpc_latency_ms
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.provider.timestamp)
        .bind(snapshots.provider.chain_id)
        .bind(&snapshots.provider.wallet_address)
        .bind(&snapshots.provider.wallet_balance_wei)
        .bind(snapshots.provider.wallet_balance_eth)
        .bind(snapshots.provider.pending_tx_count)
        .bind(snapshots.provider.rpc_latency_ms)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO wallets (
                timestamp, chain_id, wallet_address, balance_wei, balance_eth
            ) VALUES (?, ?, ?, ?, ?)
            "#
        )
        .bind(snapshots.provider.timestamp)
        .bind(snapshots.provider.chain_id)
        .bind(&snapshots.provider.wallet_address)
        .bind(&snapshots.provider.wallet_balance_wei)
        .bind(snapshots.provider.wallet_balance_eth)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO risk (
                liquidation_id, timestamp, health_factor, total_collateral_usd,
                total_debt_usd, ltv, liquidation_threshold, risk_score,
                collateral_json, debt_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.risk.timestamp)
        .bind(snapshots.risk.health_factor)
        .bind(snapshots.risk.total_collateral_usd)
        .bind(snapshots.risk.total_debt_usd)
        .bind(snapshots.risk.ltv)
        .bind(snapshots.risk.liquidation_threshold)
        .bind(i64::from(snapshots.risk.risk_score))
        .bind(collateral_json)
        .bind(debt_json)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO strategy (
                liquidation_id, timestamp, execution_method, reasoning,
                adjusted_profit_usd, is_executable, plan_context_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(liquidation_id)
        .bind(snapshots.strategy.timestamp)
        .bind(&snapshots.strategy.execution_method)
        .bind(&snapshots.strategy.reasoning)
        .bind(snapshots.strategy.adjusted_profit_usd)
        .bind(if snapshots.strategy.is_executable { 1 } else { 0 })
        .bind(&snapshots.strategy.plan_context_json)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
    
    /// Get liquidations in time range
    pub async fn get_liquidations(&self, since_hours: u32) -> Result<Vec<LiquidationEvent>> {
        let since = chrono::Utc::now().timestamp() - (since_hours as i64 * 3600);
        
        let rows = sqlx::query_as::<_, LiquidationEventRow>(
            r#"
            SELECT * FROM liquidations
            WHERE timestamp > ?
            ORDER BY timestamp DESC
            "#
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(rows.into_iter().map(|r| r.into_event()).collect())
    }
}

// Helper structs for SQLx row mapping
#[derive(sqlx::FromRow)]
struct UserRow {
    user_address: String,
    health_factor: f64,
    total_collateral_usd: f64,
    total_debt_usd: f64,
    ltv: f64,
    liquidation_threshold: f64,
    collateral_json: String,
    debt_json: String,
    estimated_profit: f64,
    risk_score: i32,
    last_updated: i64,
}

impl UserRow {
    fn into_target(self) -> Result<LiquidationTarget> {
        let collateral: HashMap<String, f64> = serde_json::from_str(&self.collateral_json)?;
        let debt: HashMap<String, f64> = serde_json::from_str(&self.debt_json)?;
        
        Ok(LiquidationTarget {
            user_address: self.user_address,
            health_factor: self.health_factor,
            total_collateral_usd: self.total_collateral_usd,
            total_debt_usd: self.total_debt_usd,
            ltv: self.ltv,
            liquidation_threshold: self.liquidation_threshold,
            collateral,
            debt,
            estimated_profit: self.estimated_profit,
            risk_score: self.risk_score as u8,
            last_updated: self.last_updated,
        })
    }
}

#[derive(sqlx::FromRow)]
struct LiquidationEventRow {
    id: i64,
    user_address: String,
    timestamp: i64,
    collateral_asset: String,
    debt_asset: String,
    collateral_seized: f64,
    debt_covered: f64,
    liquidator: String,
    tx_hash: String,
    profit_usd: f64,
    gas_cost_usd: f64,
    status: String,
    error_message: Option<String>,
}

impl LiquidationEventRow {
    fn into_event(self) -> LiquidationEvent {
        LiquidationEvent {
            id: Some(self.id),
            user_address: self.user_address,
            timestamp: self.timestamp,
            collateral_asset: self.collateral_asset,
            debt_asset: self.debt_asset,
            collateral_seized: self.collateral_seized,
            debt_covered: self.debt_covered,
            liquidator: self.liquidator,
            tx_hash: self.tx_hash,
            profit_usd: self.profit_usd,
            gas_cost_usd: self.gas_cost_usd,
            status: self.status,
            error_message: self.error_message,
        }
    }
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
