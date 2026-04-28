use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use ethers::prelude::*;

pub type AddressMap = HashMap<Address, U256>;

// =============================
// CONTRACT BINDINGS
// =============================

abigen!(
    AaveV3Pool,
    r#"[
        function getReservesList() external view returns (address[])
        function getUserAccountData(address user) external view returns (uint256 totalCollateralBase, uint256 totalDebtBase, uint256 availableBorrowsBase, uint256 currentLiquidationThreshold, uint256 ltv, uint256 healthFactor)
    ]"#
);

abigen!(
    AaveV3DataProvider,
    r#"[
        function getReserveTokensAddresses(address asset) external view returns (address aTokenAddress, address stableDebtTokenAddress, address variableDebtTokenAddress)
        function getUserReserveData(address asset, address user) external view returns (uint256 currentATokenBalance, uint256 currentStableDebt, uint256 currentVariableDebt, uint256 principalStableDebt, uint256 scaledVariableDebt, uint256 stableBorrowRate, uint256 liquidityRate, uint40 stableRateLastUpdated, bool usageAsCollateralEnabled)
    ]"#
);

/// =============================
/// DATA STRUCTURES
/// =============================

#[derive(Debug, Clone)]
pub struct ReserveInfo {
    pub asset: Address,
    pub symbol: String,
    pub decimals: u8,
    pub a_token: Address,
    pub stable_debt_token: Address,
    pub variable_debt_token: Address,
}

/// A user's full position across all reserves, keyed by asset **symbol** (e.g. "WSTETH").
#[derive(Debug, Clone, Default)]
pub struct UserPosition {
    /// symbol -> human-readable amount (already divided by 10^decimals)
    pub collateral: HashMap<String, f64>,
    /// symbol -> human-readable amount
    pub debt_variable: HashMap<String, f64>,
    /// symbol -> human-readable amount
    pub debt_stable: HashMap<String, f64>,
    /// Set of symbols where collateral-usage is enabled
    pub collateral_enabled: HashSet<String>,
}

/// On-chain account-level summary returned by `getUserAccountData`.
#[derive(Debug, Clone)]
pub struct AccountSummary {
    pub total_collateral_base: U256,
    pub total_debt_base: U256,
    pub available_borrows_base: U256,
    pub current_liquidation_threshold: U256,
    pub ltv: U256,
    pub health_factor: U256,
}

/// =============================
/// READER CORE
/// =============================

pub struct AaveV3Reader<M> {
    pub pool: AaveV3Pool<M>,
    pub data_provider: AaveV3DataProvider<M>,
    pub reserves: Vec<ReserveInfo>,
    /// Reverse lookup: reserve address -> symbol
    addr_to_symbol: HashMap<Address, String>,
}

impl<M: Middleware + 'static> AaveV3Reader<M> {
    pub fn new(
        pool_address: Address,
        data_provider_address: Address,
        client: Arc<M>,
    ) -> Self {
        let pool = AaveV3Pool::new(pool_address, client.clone());
        let data_provider = AaveV3DataProvider::new(data_provider_address, client);

        Self {
            pool,
            data_provider,
            reserves: vec![],
            addr_to_symbol: HashMap::new(),
        }
    }

    /// =============================
    /// INIT RESERVE CACHE
    /// =============================
    ///
    /// Fetches all reserves from the Pool, resolves their token addresses via
    /// ProtocolDataProvider, and populates the internal cache.
    ///
    /// `known_symbols` maps reserve **address** → human-readable symbol so the
    /// reader can convert raw addresses into symbols used by the rest of the bot
    /// (e.g. "WETH", "USDC"). Any reserve whose address is missing from the map
    /// will be stored with a hex-address fallback symbol.
    pub async fn init_reserves(
        &mut self,
        known_symbols: &HashMap<Address, String>,
    ) -> Result<()> {
        let reserve_list = self
            .pool
            .get_reserves_list()
            .call()
            .await
            .context("Pool.getReservesList() failed")?;

        let mut reserves = Vec::with_capacity(reserve_list.len());
        let mut addr_map = HashMap::with_capacity(reserve_list.len());

        for asset in reserve_list {
            let tokens = self
                .data_provider
                .get_reserve_tokens_addresses(asset)
                .call()
                .await
                .context("ProtocolDataProvider.getReserveTokensAddresses() failed")?;

            let symbol = known_symbols
                .get(&asset)
                .cloned()
                .unwrap_or_else(|| format!("{:?}", asset));

            let decimals = symbol_to_decimals(&symbol);

            addr_map.insert(asset, symbol.clone());

            reserves.push(ReserveInfo {
                asset,
                symbol,
                decimals,
                a_token: tokens.0,
                stable_debt_token: tokens.1,
                variable_debt_token: tokens.2,
            });
        }

        tracing::info!(
            "AaveV3Reader: initialized {} reserves from on-chain",
            reserves.len()
        );

        self.reserves = reserves;
        self.addr_to_symbol = addr_map;

        Ok(())
    }

    /// =============================
    /// LOAD FULL USER POSITION
    /// =============================
    ///
    /// Iterates every cached reserve and calls `getUserReserveData` to build
    /// the user's true per-asset position.  Amounts are normalized to
    /// human-readable f64 (divided by 10^decimals).
    pub async fn load_user_position(
        &self,
        user: Address,
    ) -> Result<UserPosition> {
        let mut position = UserPosition::default();

        for reserve in &self.reserves {
            let data = self
                .data_provider
                .get_user_reserve_data(reserve.asset, user)
                .call()
                .await
                .context("ProtocolDataProvider.getUserReserveData() failed")?;

            let current_a_token_balance = data.0;
            let current_stable_debt = data.1;
            let current_variable_debt = data.2;
            let usage_as_collateral = data.8;

            let divisor = 10f64.powi(reserve.decimals as i32);

            // Collateral
            if current_a_token_balance > U256::zero() {
                let amount = u256_to_f64(current_a_token_balance) / divisor;
                position
                    .collateral
                    .insert(reserve.symbol.clone(), amount);
            }

            // Variable debt
            if current_variable_debt > U256::zero() {
                let amount = u256_to_f64(current_variable_debt) / divisor;
                position
                    .debt_variable
                    .insert(reserve.symbol.clone(), amount);
            }

            // Stable debt
            if current_stable_debt > U256::zero() {
                let amount = u256_to_f64(current_stable_debt) / divisor;
                position
                    .debt_stable
                    .insert(reserve.symbol.clone(), amount);
            }

            // Collateral enabled
            if usage_as_collateral {
                position
                    .collateral_enabled
                    .insert(reserve.symbol.clone());
            }
        }

        Ok(position)
    }

    /// =============================
    /// SANITY CHECK
    /// =============================
    ///
    /// Reads the Aave account-level summary.  Use this to cross-check the
    /// per-reserve position reconstruction.
    pub async fn get_account_summary(
        &self,
        user: Address,
    ) -> Result<AccountSummary> {
        let data = self
            .pool
            .get_user_account_data(user)
            .call()
            .await
            .context("Pool.getUserAccountData() failed")?;

        Ok(AccountSummary {
            total_collateral_base: data.0,
            total_debt_base: data.1,
            available_borrows_base: data.2,
            current_liquidation_threshold: data.3,
            ltv: data.4,
            health_factor: data.5,
        })
    }

    /// Lookup symbol from reserve address.
    pub fn symbol_for(&self, addr: &Address) -> Option<&String> {
        self.addr_to_symbol.get(addr)
    }

    /// Return all cached reserves.
    pub fn reserves(&self) -> &[ReserveInfo] {
        &self.reserves
    }
}

/// =============================
/// HELPERS
/// =============================

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}

fn symbol_to_decimals(symbol: &str) -> u8 {
    match symbol {
        "USDC" | "USDT" => 6,
        "WBTC" => 8,
        _ => 18,
    }
}