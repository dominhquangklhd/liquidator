// Cold Storage Layer - SQLite Database
//
// Provides persistent storage for:
// - All user positions
// - Historical snapshots
// - Liquidation events
// - Analytics data

use super::models::{LiquidationTarget, HistoricalSnapshot, LiquidationEvent};
use anyhow::{Context, Result};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::collections::HashMap;

/// Cold storage (SQLite database)
pub struct ColdStorage {
    pool: SqlitePool,
}

impl ColdStorage {
    /// Create new cold storage and initialize schema
    pub async fn new(db_path: &str) -> Result<Self> {
        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite://{}?mode=rwc", db_path))
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
                gas_cost_usd REAL NOT NULL
            )
            "#
        )
        .execute(pool)
        .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_liquidations_user ON liquidations(user_address)")
            .execute(pool)
            .await?;
        
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_liquidations_time ON liquidations(timestamp)")
            .execute(pool)
            .await?;
        
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
    pub async fn insert_liquidation(&self, event: &LiquidationEvent) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO liquidations (
                user_address, timestamp, collateral_asset, debt_asset,
                collateral_seized, debt_covered, liquidator, tx_hash,
                profit_usd, gas_cost_usd
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .execute(&self.pool)
        .await?;
        
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
        }
    }
}
