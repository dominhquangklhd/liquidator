use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use ethers::contract::abigen;
use ethers::providers::{Http, Provider};
use ethers::types::Address;

use crate::aave_v3::reader::AaveV3Reader;

abigen!(
    PoolAddressesProvider,
    r#"[
        function getPoolDataProvider() external view returns (address)
    ]"#
);

pub async fn fetch_liquidation_bonus_map(
    provider: Arc<Provider<Http>>,
    aave_pool_address: Address,
    aave_addresses_provider: Address,
    chain_id: u64,
) -> Result<HashMap<String, f64>> {
    let addresses_provider = PoolAddressesProvider::new(
        aave_addresses_provider,
        Arc::clone(&provider),
    );
    let data_provider_address = addresses_provider
        .get_pool_data_provider()
        .call()
        .await
        .context("PoolAddressesProvider.getPoolDataProvider() failed")?;

    if data_provider_address == Address::zero() {
        bail!("PoolDataProvider address resolved to zero");
    }

    let reserve_catalog = reserve_catalog_from_env(chain_id);
    if reserve_catalog.is_empty() {
        bail!("Reserve catalog is empty");
    }

    let reverse_map: HashMap<Address, String> = reserve_catalog
        .iter()
        .map(|(symbol, addr)| (*addr, symbol.clone()))
        .collect();

    let mut reader = AaveV3Reader::new(
        aave_pool_address,
        data_provider_address,
        Arc::clone(&provider),
    );
    reader.init_reserves(&reverse_map).await?;

    let mut bonus_map = HashMap::new();
    for reserve in reader.reserves() {
        bonus_map.insert(reserve.symbol.clone(), reserve.liquidation_bonus_pct);
        if reserve.symbol == "WETH" {
            bonus_map.insert("ETH".to_string(), reserve.liquidation_bonus_pct);
        }
    }

    Ok(bonus_map)
}

pub fn reserve_catalog_from_env(chain_id: u64) -> HashMap<String, Address> {
    let mut out = default_reserve_catalog(chain_id);

    if let Ok(raw) = std::env::var("RESERVE_CATALOG") {
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let Some((symbol_raw, addr_raw)) = entry.split_once('=') else {
                continue;
            };

            let symbol = symbol_raw.trim().to_ascii_uppercase();
            let Ok(addr) = addr_raw.trim().parse::<Address>() else {
                continue;
            };

            out.insert(symbol, addr);
        }
    }

    for (key, value) in std::env::vars() {
        if !key.starts_with("RESERVE_") || key == "RESERVE_CATALOG" {
            continue;
        }

        let symbol = key.trim_start_matches("RESERVE_").trim().to_ascii_uppercase();
        if symbol.is_empty() {
            continue;
        }

        if let Ok(addr) = value.trim().parse::<Address>() {
            out.insert(symbol, addr);
        }
    }

    out
}

fn default_reserve_catalog(_chain_id: u64) -> HashMap<String, Address> {
    let mut out: HashMap<String, Address> = HashMap::new();

    let pairs: [(&str, &str); 8] = [
        ("WETH", "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
        ("WSTETH", "0x7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"),
        ("USDC", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
        ("WBTC", "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"),
        ("DAI", "0x6B175474E89094C44Da98b954EedeAC495271d0F"),
        ("USDT", "0xdAC17F958D2ee523a2206206994597C13D831ec7"),
        ("LINK", "0x514910771AF9Ca656af840dff83E8264EcF986CA"),
        ("AAVE", "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DdAE9"),
    ];

    for (symbol, addr_raw) in pairs {
        if let Ok(addr) = addr_raw.parse::<Address>() {
            out.insert(symbol.to_string(), addr);
        }
    }

    out
}
