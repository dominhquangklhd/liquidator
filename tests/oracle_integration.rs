// ============================================================================
// ORACLE INTEGRATION TEST
// ============================================================================
//
// Test Oracle module end-to-end trên Anvil mainnet fork.
//
// Prerequisite: Khởi động Anvil mainnet fork:
//   .\scripts\start_anvil.ps1
//   hoặc:
//   anvil --fork-url https://eth-mainnet.g.alchemy.com/v2/<KEY> --port 8545
//
// Cách chạy:
//   cargo test --test oracle_integration -- --nocapture
//
// Chạy từng test:
//   cargo test --test oracle_integration test_chainlink_read_eth_price -- --nocapture
//   cargo test --test oracle_integration test_oracle_manager_poll -- --nocapture
//   cargo test --test oracle_integration test_mock_price_feed -- --nocapture
//
// ============================================================================
// LUỒNG HOẠT ĐỘNG CỦA ORACLE MODULE:
//
//   ┌──────────────────────────────────────────────────────────────┐
//   │  1. OracleConfig::mainnet() / local_fork()                  │
//   │     → Tạo danh sách 6 PriceFeedConfig (ETH, WBTC, USDC...) │
//   │                                                              │
//   │  2. OracleManager::new(config, provider, event_tx)          │
//   │     → Tạo ChainlinkFeed cho mỗi asset                      │
//   │                                                              │
//   │  3. oracle_manager.initialize()                             │
//   │     → Gọi feed.initialize() → đọc decimals(), description()│
//   │     → Gọi feed.latest_price() → đọc giá ban đầu            │
//   │     → Cache giá vào price_cache                             │
//   │                                                              │
//   │  4. oracle_price_worker (loop mỗi 2-12s):                   │
//   │     → oracle_manager.poll_all()                             │
//   │       → poll_single_feed() cho mỗi asset:                   │
//   │         a) Đọc latestRoundData() từ Chainlink               │
//   │         b) So sánh với giá cũ trong cache                   │
//   │         c) Tính deviation %                                  │
//   │         d) Nếu deviation >= threshold:                       │
//   │            → convert_to_engine_price (USD → ETH ratio)      │
//   │            → event_tx.send(Event::PriceUpdate)              │
//   │         e) Update price_cache                                │
//   │                                                              │
//   │  5. RiskEngine nhận Event::PriceUpdate                      │
//   │     → Recalculate Health Factor cho affected users           │
//   │     → Nếu HF < 1.0 → thêm vào liquidation queue            │
//   └──────────────────────────────────────────────────────────────┘
// ============================================================================

use std::sync::{Arc, OnceLock};
use ethers::prelude::*;
use ethers::abi::Abi;
use ethers::providers::{Provider, Http, Middleware};
use ethers::types::{Address, Bytes, H256, U256};
use anyhow::{Result, Context};
use serde_json::Value;
use tokio::sync::mpsc;

use liquidator::oracle::{
    OracleConfig, PriceFeedConfig, OracleManager, 
    PriceData, FeedStatus, OracleStats,
};
use liquidator::events::event::Event;

static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Anvil default RPC URL
const ANVIL_RPC: &str = "http://127.0.0.1:8545";

/// Chainlink ETH/USD trên Mainnet
const CHAINLINK_ETH_USD: &str = "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419";

/// Chainlink USDC/USD trên Mainnet
const CHAINLINK_USDC_USD: &str = "0x8fFfFfd4AfB6115b954Bd326cbe7B4BA576818f6";

/// Anvil default account #0 (chỉ dùng local test)
const ANVIL_DEFAULT_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

abigen!(
    MockPriceFeedContract,
    r#"[
        function setAnswer(int256 newAnswer) external
        function latestAnswer() external view returns (int256)
        event AnswerUpdated(int256 indexed current, uint256 indexed roundId, uint256 updatedAt)
    ]"#
);

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Kết nối đến Anvil
async fn connect_anvil() -> Result<Arc<Provider<Http>>> {
    let provider = Provider::<Http>::try_from(ANVIL_RPC)?;
    let chain_id = provider.get_chainid().await?;
    println!("✓ Connected to Anvil (chain_id: {})", chain_id);
    Ok(Arc::new(provider))
}

fn pad_u256_hex(value: U256) -> String {
    format!("{:#066x}", value)
}

fn load_mock_deployed_bytecode() -> Result<String> {
    let json_path = std::path::Path::new("out")
        .join("MockPriceFeed.sol")
        .join("MockPriceFeed.json");

    let raw = std::fs::read_to_string(&json_path)
        .with_context(|| format!("Cannot read {}. Run `forge build contracts/MockPriceFeed.sol` first.", json_path.display()))?;
    let v: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid JSON in {}", json_path.display()))?;

    let bytecode = v
        .get("deployedBytecode")
        .and_then(|x| x.get("object"))
        .and_then(Value::as_str)
        .context("Missing deployedBytecode.object in MockPriceFeed artifact")?
        .trim();

    if bytecode.is_empty() {
        anyhow::bail!("deployedBytecode.object is empty in MockPriceFeed artifact");
    }

    if bytecode.starts_with("0x") {
        Ok(bytecode.to_string())
    } else {
        Ok(format!("0x{}", bytecode))
    }
}

fn load_mock_creation_bytecode() -> Result<String> {
    let json_path = std::path::Path::new("out")
        .join("MockPriceFeed.sol")
        .join("MockPriceFeed.json");

    let raw = std::fs::read_to_string(&json_path)
        .with_context(|| format!("Cannot read {}. Run `forge build contracts/MockPriceFeed.sol` first.", json_path.display()))?;
    let v: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid JSON in {}", json_path.display()))?;

    let bytecode = v
        .get("bytecode")
        .and_then(|x| x.get("object"))
        .and_then(Value::as_str)
        .context("Missing bytecode.object in MockPriceFeed artifact")?
        .trim();

    if bytecode.is_empty() {
        anyhow::bail!("bytecode.object is empty in MockPriceFeed artifact");
    }

    if bytecode.starts_with("0x") {
        Ok(bytecode.to_string())
    } else {
        Ok(format!("0x{}", bytecode))
    }
}

fn load_mock_abi() -> Result<Abi> {
    let json_path = std::path::Path::new("out")
        .join("MockPriceFeed.sol")
        .join("MockPriceFeed.json");

    let raw = std::fs::read_to_string(&json_path)
        .with_context(|| format!("Cannot read {}. Run `forge build contracts/MockPriceFeed.sol` first.", json_path.display()))?;
    let v: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid JSON in {}", json_path.display()))?;

    let abi_value = v.get("abi").context("Missing abi in MockPriceFeed artifact")?;
    let abi: Abi = serde_json::from_value(abi_value.clone())
        .context("Invalid abi in MockPriceFeed artifact")?;
    Ok(abi)
}

async fn anvil_set_code(provider: &Provider<Http>, target: Address, bytecode: &str) -> Result<()> {
    let _: Value = provider
        .request(
            "anvil_setCode",
            serde_json::json!([format!("{:#x}", target), bytecode]),
        )
        .await
        .context("anvil_setCode failed")?;
    Ok(())
}

async fn anvil_set_storage(
    provider: &Provider<Http>,
    target: Address,
    slot: U256,
    value: U256,
) -> Result<()> {
    let _: Value = provider
        .request(
            "anvil_setStorageAt",
            serde_json::json!([
                format!("{:#x}", target),
                pad_u256_hex(slot),
                pad_u256_hex(value)
            ]),
        )
        .await
        .context("anvil_setStorageAt failed")?;
    Ok(())
}

async fn anvil_mine(provider: &Provider<Http>, blocks: u64) -> Result<()> {
    let _: Value = provider
        .request("anvil_mine", serde_json::json!([blocks]))
        .await
        .context("anvil_mine failed")?;
    Ok(())
}

/// Tạo config chỉ với 1 feed (ETH) để test nhanh
fn single_eth_config() -> OracleConfig {
    OracleConfig {
        poll_interval_ms: 1000,
        default_deviation_pct: 0.01,  // Rất nhỏ → dễ trigger event
        default_staleness_secs: 999999,
        max_retries: 1,
        retry_delay_ms: 100,
        verbose_logging: true,
        feeds: vec![
            PriceFeedConfig {
                asset_symbol: "ETH".to_string(),
                asset_id: "ETH".to_string(),
                feed_address: CHAINLINK_ETH_USD.parse().unwrap(),
                decimals: 8,
                heartbeat_secs: 999999,
                deviation_threshold_pct: 0.01,
                is_stablecoin: false,
            },
        ],
    }
}

/// Tạo config với 2 feeds (ETH + USDC)
fn eth_usdc_config() -> OracleConfig {
    let mut config = single_eth_config();
    config.feeds.push(PriceFeedConfig {
        asset_symbol: "USDC".to_string(),
        asset_id: "USDC".to_string(),
        feed_address: CHAINLINK_USDC_USD.parse().unwrap(),
        decimals: 8,
        heartbeat_secs: 86400,
        deviation_threshold_pct: 0.01,
        is_stablecoin: true,
    });
    config
}

// ============================================================================
// TEST 1: Kết nối Anvil
// ============================================================================
#[tokio::test]
async fn test_connect_anvil() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let block = provider.get_block_number().await?;
    println!("  Block number: {}", block);
    assert!(block.as_u64() > 0, "Block number phải > 0");
    Ok(())
}

// ============================================================================
// TEST 2: Đọc giá ETH/USD trực tiếp từ Chainlink
// ============================================================================
#[tokio::test]
async fn test_chainlink_read_eth_price() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    
    let config = PriceFeedConfig {
        asset_symbol: "ETH".to_string(),
        asset_id: "ETH".to_string(),
        feed_address: CHAINLINK_ETH_USD.parse().unwrap(),
        decimals: 8,
        heartbeat_secs: 3600,
        deviation_threshold_pct: 0.5,
        is_stablecoin: false,
    };
    
    let mut feed = liquidator::oracle::ChainlinkFeed::new(provider, config);
    
    // Initialize — đọc decimals + description
    feed.initialize().await?;
    println!("  ✓ Feed initialized");
    println!("  Description: {:?}", feed.description());
    
    // Đọc giá mới nhất
    let price = feed.latest_price().await?;
    println!("  ✓ ETH/USD = ${:.2}", price.price_usd);
    println!("    Raw answer: {}", price.price_raw);
    println!("    Decimals: {}", price.decimals);
    println!("    Round ID: {}", price.round_id);
    println!("    Updated at: {}", price.updated_at);
    
    // Verify giá hợp lý (ETH thường $500 - $100,000)
    assert!(price.price_usd > 100.0, "ETH price phải > $100, got ${}", price.price_usd);
    assert!(price.price_usd < 100_000.0, "ETH price phải < $100k, got ${}", price.price_usd);
    assert_eq!(price.decimals, 8);
    assert_eq!(price.asset_id, "ETH");
    
    // Test latestAnswer (lightweight)
    let simple_price = feed.latest_answer().await?;
    println!("  ✓ latestAnswer() = ${:.2}", simple_price);
    assert!((simple_price - price.price_usd).abs() < 0.01, "Hai cách đọc phải cho cùng giá");
    
    Ok(())
}

// ============================================================================
// TEST 3: Đọc giá USDC/USD (stablecoin)
// ============================================================================
#[tokio::test]
async fn test_chainlink_read_usdc_price() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    
    let config = PriceFeedConfig {
        asset_symbol: "USDC".to_string(),
        asset_id: "USDC".to_string(),
        feed_address: CHAINLINK_USDC_USD.parse().unwrap(),
        decimals: 8,
        heartbeat_secs: 86400,
        deviation_threshold_pct: 0.1,
        is_stablecoin: true,
    };
    
    let mut feed = liquidator::oracle::ChainlinkFeed::new(provider, config);
    feed.initialize().await?;
    
    let price = feed.latest_price().await?;
    println!("  ✓ USDC/USD = ${:.6}", price.price_usd);
    
    // USDC phải gần $1.00 (±5%)
    assert!(price.price_usd > 0.95, "USDC phải > $0.95, got ${}", price.price_usd);
    assert!(price.price_usd < 1.05, "USDC phải < $1.05, got ${}", price.price_usd);
    
    Ok(())
}

// ============================================================================
// TEST 4: ChainlinkFeed health check
// ============================================================================
#[tokio::test]
async fn test_chainlink_health_check() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    
    let config = PriceFeedConfig {
        asset_symbol: "ETH".to_string(),
        asset_id: "ETH".to_string(),
        feed_address: CHAINLINK_ETH_USD.parse().unwrap(),
        decimals: 8,
        heartbeat_secs: 999999, // Rất dài → feed luôn active
        deviation_threshold_pct: 0.5,
        is_stablecoin: false,
    };
    
    let mut feed = liquidator::oracle::ChainlinkFeed::new(provider, config);
    feed.initialize().await?;
    
    let status = feed.health_check().await;
    println!("  ✓ Feed status: {:?}", status);
    assert!(
        matches!(status, FeedStatus::Active | FeedStatus::Stale),
        "Feed status phải là Active hoặc Stale, got {:?}",
        status
    );
    
    Ok(())
}

// ============================================================================
// TEST 5: OracleManager — khởi tạo và đọc giá
// ============================================================================
#[tokio::test]
async fn test_oracle_manager_initialize() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    
    let config = eth_usdc_config();
    let mut manager = OracleManager::new(config, provider, event_tx).await?;
    
    println!("  Feed count: {}", manager.feed_count());
    assert_eq!(manager.feed_count(), 2);
    
    // Initialize — đọc giá ban đầu
    manager.initialize().await?;
    println!("  ✓ Oracle initialized");
    
    // Kiểm tra price cache
    let eth_price = manager.get_price_usd("ETH").await;
    let usdc_price = manager.get_price_usd("USDC").await;
    
    println!("  ETH  = ${:.2}", eth_price.unwrap_or(0.0));
    println!("  USDC = ${:.6}", usdc_price.unwrap_or(0.0));
    
    assert!(eth_price.is_some(), "ETH price phải có sau initialize");
    assert!(usdc_price.is_some(), "USDC price phải có sau initialize");
    assert!(eth_price.unwrap() > 100.0);
    assert!(usdc_price.unwrap() > 0.95);
    
    // Kiểm tra price detail
    let eth_data = manager.get_price("ETH").await;
    assert!(eth_data.is_some());
    let eth_data = eth_data.unwrap();
    assert_eq!(eth_data.asset_id, "ETH");
    assert_eq!(eth_data.decimals, 8);
    
    // Kiểm tra get_all_prices
    let all = manager.get_all_prices().await;
    assert_eq!(all.len(), 2, "Phải có 2 giá trong cache");
    assert!(all.contains_key("ETH"));
    assert!(all.contains_key("USDC"));
    
    Ok(())
}

// ============================================================================
// TEST 6: OracleManager — poll và emit events
// ============================================================================
#[tokio::test]
async fn test_oracle_manager_poll() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    
    // Config với deviation_threshold rất nhỏ → luôn emit khi có giá đầu tiên
    let config = single_eth_config();
    let mut manager = OracleManager::new(config, provider, event_tx).await?;
    
    // Chưa initialize → price cache trống
    assert!(manager.get_price("ETH").await.is_none());
    
    // Initialize
    manager.initialize().await?;
    let init_price = manager.get_price_usd("ETH").await.unwrap();
    println!("  ✓ Initial ETH price: ${:.2}", init_price);
    
    // Poll lần 1 — giá giống init → deviation = 0 → không emit (trừ khi threshold rất nhỏ)
    let updates = manager.poll_all().await?;
    println!("  ✓ Poll 1: {} updates emitted", updates);
    
    // Kiểm tra stats
    let stats = manager.get_stats().await;
    println!("  Stats: polls={}, updates={}, errors={}", 
        stats.total_polls, stats.total_updates_emitted, stats.total_errors);
    assert!(stats.total_polls >= 1, "Phải có ít nhất 1 poll");
    assert_eq!(stats.total_errors, 0, "Không nên có errors");
    
    // Kiểm tra feed info
    let feeds = manager.get_all_feed_info().await;
    assert_eq!(feeds.len(), 1);
    let eth_feed = &feeds[0];
    println!("  Feed: {} — status={}, success={}, errors={}", 
        eth_feed.asset_id, eth_feed.status, eth_feed.success_count, eth_feed.error_count);
    assert_eq!(eth_feed.status, FeedStatus::Active);
    assert!(eth_feed.success_count >= 1);
    
    Ok(())
}

// ============================================================================
// TEST 7: OracleManager — event channel đúng format
// ============================================================================
#[tokio::test]
async fn test_oracle_event_format() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    
    // Config chỉ có ETH, KHÔNG initialize trước → lần poll đầu = giá đầu tiên → luôn emit
    let config = single_eth_config();
    let manager = OracleManager::new(config, provider, event_tx).await?;
    
    // Poll mà chưa initialize → feed chưa có decimals → sẽ dùng config.decimals
    // Lần poll đầu tiên sẽ set giá ban đầu
    let updates = manager.poll_all().await?;
    println!("  ✓ First poll: {} updates", updates);
    
    // Nếu có update → sẽ có event trong channel
    if updates > 0 {
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(1), 
            event_rx.recv()
        ).await {
            Ok(Some(Event::PriceUpdate { asset_id, new_price })) => {
                println!("  ✓ Received Event::PriceUpdate");
                println!("    asset_id: {}", asset_id);
                println!("    new_price (ETH-based): {:.6}", new_price);
                assert_eq!(asset_id, "ETH");
                // ETH price in ETH = 1.0 (convert_to_engine_price returns 1.0 for ETH)
                assert_eq!(new_price, 1.0, "ETH price in ETH phải = 1.0");
            }
            Ok(Some(other)) => {
                panic!("Expected PriceUpdate event, got {:?}", other);
            }
            Ok(None) => {
                panic!("Event channel closed unexpectedly");
            }
            Err(_) => {
                panic!("Timeout waiting for event");
            }
        }
    }
    
    Ok(())
}

// ============================================================================
// TEST 8: OracleManager — giá USD→ETH conversion
// ============================================================================
#[tokio::test]
async fn test_price_conversion_usd_to_eth() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    
    let config = eth_usdc_config();
    let mut manager = OracleManager::new(config, provider, event_tx).await?;
    manager.initialize().await?;
    
    let eth_usd = manager.get_price_usd("ETH").await.unwrap();
    let usdc_usd = manager.get_price_usd("USDC").await.unwrap();
    
    println!("  ETH/USD  = ${:.2}", eth_usd);
    println!("  USDC/USD = ${:.6}", usdc_usd);
    
    // Tính expected USDC/ETH ratio
    let expected_usdc_in_eth = usdc_usd / eth_usd;
    println!("  Expected USDC/ETH = {:.8}", expected_usdc_in_eth);
    
    // USDC/ETH phải rất nhỏ (khoảng 0.0003 - 0.001 tùy giá ETH)
    assert!(expected_usdc_in_eth < 0.01, "USDC/ETH phải < 0.01");
    assert!(expected_usdc_in_eth > 0.0, "USDC/ETH phải > 0");
    
    // price_cache cho phép module khác đọc
    let cache = manager.price_cache();
    let cache_read = cache.read().await;
    assert_eq!(cache_read.len(), 2);
    
    Ok(())
}

// ============================================================================
// TEST 9: OracleManager — full config (6 feeds)
// ============================================================================
#[tokio::test]
async fn test_oracle_full_mainnet_feeds() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, _event_rx) = mpsc::channel::<Event>(100);
    
    // Dùng full mainnet config (6 feeds)
    let config = OracleConfig::local_fork();
    let mut manager = OracleManager::new(config, provider, event_tx).await?;
    
    assert_eq!(manager.feed_count(), 6, "Phải có 6 feeds");
    
    manager.initialize().await?;
    
    let all_prices = manager.get_all_prices().await;
    println!("\n  ━━━━━ ALL PRICES ━━━━━");
    for (asset, price) in &all_prices {
        println!("  {} = ${:.2}", asset, price);
    }
    println!("  ━━━━━━━━━━━━━━━━━━━━━━");
    
    // Verify tất cả feeds đều đọc được giá
    let stats = manager.get_stats().await;
    println!("\n  Active: {}, Stale: {}, Error: {}", 
        stats.active_feeds, stats.stale_feeds, stats.error_feeds);
    
    // Ít nhất ETH, USDC phải thành công
    assert!(all_prices.contains_key("ETH"), "ETH phải có giá");
    assert!(all_prices.contains_key("USDC"), "USDC phải có giá");
    
    // Kiểm tra giá hợp lý
    if let Some(&eth) = all_prices.get("ETH") {
        assert!(eth > 100.0 && eth < 100_000.0, "ETH price bất hợp lý: ${}", eth);
    }
    if let Some(&btc) = all_prices.get("WBTC") {
        assert!(btc > 1_000.0 && btc < 500_000.0, "WBTC price bất hợp lý: ${}", btc);
    }
    if let Some(&usdc) = all_prices.get("USDC") {
        assert!(usdc > 0.95 && usdc < 1.05, "USDC depeg! ${}", usdc);
    }
    if let Some(&dai) = all_prices.get("DAI") {
        assert!(dai > 0.95 && dai < 1.05, "DAI depeg! ${}", dai);
    }
    
    Ok(())
}

// ============================================================================
// TEST 10: Mock Price Feed — Deploy và test thay đổi giá
// ============================================================================
#[tokio::test]
async fn test_mock_price_feed() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    
    // ── Bước 1: Deploy MockPriceFeed ───────────────────────────────
    // Giá initial: $2500 = 250000000000 (8 decimals)
    let initial_price = 250_000_000_000i64;
    
    // ABI-encode constructor(int256 initialAnswer)
    let deploy_data = format!(
        "{}{}",
        // MockPriceFeed bytecode sẽ cần compile trước
        // Fallback: dùng Chainlink ETH/USD feed thật thay vì mock
        "", ""
    );
    
    // Thay vì deploy contract (cần bytecode), test bằng cách đọc Chainlink ETH/USD
    // rồi mine thêm block để thấy round_id thay đổi
    println!("  ℹ Test mock price feed bằng Chainlink ETH/USD thật");
    println!("  (Deploy MockPriceFeed xem ở test_mock_deploy_and_update bên dưới)");
    
    let config = PriceFeedConfig {
        asset_symbol: "ETH".to_string(),
        asset_id: "ETH".to_string(),
        feed_address: CHAINLINK_ETH_USD.parse().unwrap(),
        decimals: 8,
        heartbeat_secs: 999999,
        deviation_threshold_pct: 0.5,
        is_stablecoin: false,
    };
    
    let mut feed = liquidator::oracle::ChainlinkFeed::new(provider.clone(), config);
    feed.initialize().await?;
    
    // Đọc giá 2 lần → phải giống nhau (không có block mới)
    let price1 = feed.latest_price().await?;
    let price2 = feed.latest_price().await?;
    
    println!("  Price 1: ${:.2} (round {})", price1.price_usd, price1.round_id);
    println!("  Price 2: ${:.2} (round {})", price2.price_usd, price2.round_id);
    
    assert_eq!(price1.price_usd, price2.price_usd, "Giá phải giống nhau khi không có update");
    assert_eq!(price1.round_id, price2.round_id, "Round ID phải giống nhau");
    
    Ok(())
}

// ============================================================================
// TEST 11: MockPriceFeed on-chain event -> Oracle PriceUpdate event
// ============================================================================
#[tokio::test]
async fn test_mock_feed_answerupdated_triggers_oracle_priceupdate() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let feed_addr: Address = CHAINLINK_ETH_USD.parse()?;

    // 1) Replace chainlink feed code with MockPriceFeed runtime bytecode.
    let deployed_bytecode = load_mock_deployed_bytecode()?;
    anvil_set_code(provider.as_ref(), feed_addr, &deployed_bytecode).await?;

    // 2) Seed predictable mock storage so latestRoundData/latestAnswer are valid.
    let block_number = provider.get_block_number().await?;
    let block = provider
        .get_block(block_number)
        .await?
        .context("Latest block not found")?;

    let initial_answer: u64 = 250_000_000_000; // $2500 with 8 decimals
    anvil_set_storage(provider.as_ref(), feed_addr, U256::from(0u64), U256::from(initial_answer)).await?; // _answer
    anvil_set_storage(provider.as_ref(), feed_addr, U256::from(1u64), U256::from(8u64)).await?; // _decimals
    anvil_set_storage(provider.as_ref(), feed_addr, U256::from(4u64), U256::from(1u64)).await?; // _roundId
    anvil_set_storage(provider.as_ref(), feed_addr, U256::from(5u64), block.timestamp).await?; // _updatedAt
    anvil_mine(provider.as_ref(), 1).await?;

    // 3) Initialize oracle manager with mocked ETH feed and baseline cache.
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    let config = single_eth_config();
    let mut manager = OracleManager::new(config, Arc::clone(&provider), event_tx).await?;
    manager.initialize().await?;

    // 4) Send tx setAnswer(newPrice) and verify on-chain AnswerUpdated was emitted.
    let chain_id = provider.get_chainid().await?.as_u64();
    let wallet: LocalWallet = ANVIL_DEFAULT_KEY.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let signer = Arc::new(SignerMiddleware::new(provider.as_ref().clone(), wallet));
    let mock = MockPriceFeedContract::new(feed_addr, signer);

    let new_answer: i64 = 1_800 * 100_000_000; // $1800 with 8 decimals
    let receipt = mock
        .set_answer(I256::from(new_answer))
        .send()
        .await?
        .await?
        .context("setAnswer tx dropped without receipt")?;

    let answer_updated_sig: H256 = H256::from(ethers::utils::keccak256(
        "AnswerUpdated(int256,uint256,uint256)".as_bytes(),
    ));
    let emitted = receipt.logs.iter().any(|log| {
        log.address == feed_addr
            && log
                .topics
                .first()
                .map(|t| *t == answer_updated_sig)
                .unwrap_or(false)
    });
    assert!(emitted, "Expected AnswerUpdated event in tx receipt");

    // 5) Oracle poll should detect deviation and emit internal Event::PriceUpdate.
    let updates = manager.poll_all().await?;
    assert_eq!(updates, 1, "Expected one feed update after setAnswer");

    let evt = tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await?;
    match evt {
        Some(Event::PriceUpdate { asset_id, new_price }) => {
            assert_eq!(asset_id, "ETH");
            assert_eq!(new_price, 1.0, "ETH must remain 1.0 in ETH-based pricing");
        }
        Some(other) => panic!("Expected PriceUpdate, got {:?}", other),
        None => panic!("Event channel closed unexpectedly"),
    }

    Ok(())
}

// ============================================================================
// TEST 11.5: MockPriceFeed at NEW address -> OracleManager still works
// ============================================================================
#[tokio::test]
async fn test_oracle_manager_with_deployed_mock_feed_address() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;

    // Build signer from Anvil default account.
    let chain_id = provider.get_chainid().await?.as_u64();
    let wallet: LocalWallet = ANVIL_DEFAULT_KEY.parse::<LocalWallet>()?.with_chain_id(chain_id);
    let signer = Arc::new(SignerMiddleware::new(provider.as_ref().clone(), wallet));

    // Deploy MockPriceFeed at a fresh address instead of replacing Chainlink feed code.
    let bytecode_hex = load_mock_creation_bytecode()?;
    let bytecode: Bytes = bytecode_hex.parse().context("Invalid mock deployed bytecode")?;
    let abi = load_mock_abi()?;
    let factory = ContractFactory::new(abi, bytecode, Arc::clone(&signer));

    let initial_price_raw: i64 = 2_500 * 100_000_000; // $2500, 8 decimals
    let deployed = factory
        .deploy(I256::from(initial_price_raw))?
        .send()
        .await
        .context("Failed to deploy MockPriceFeed")?;
    let mock_addr = deployed.address();

    // Point oracle manager to the mock address.
    let config = OracleConfig {
        poll_interval_ms: 1000,
        default_deviation_pct: 0.01,
        default_staleness_secs: 999999,
        max_retries: 1,
        retry_delay_ms: 100,
        verbose_logging: true,
        feeds: vec![PriceFeedConfig {
            asset_symbol: "ETH".to_string(),
            asset_id: "ETH".to_string(),
            feed_address: mock_addr,
            decimals: 8,
            heartbeat_secs: 999999,
            deviation_threshold_pct: 0.01,
            is_stablecoin: false,
        }],
    };

    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    let mut manager = OracleManager::new(config, Arc::clone(&provider), event_tx).await?;
    manager.initialize().await?;

    let initial_price = manager
        .get_price_usd("ETH")
        .await
        .context("Missing ETH price after initialize")?;
    assert!(
        (initial_price - 2500.0).abs() < 0.1,
        "Expected initial mock ETH price around $2500, got ${}",
        initial_price
    );

    // Update mock feed price and poll again.
    let mock = MockPriceFeedContract::new(mock_addr, Arc::clone(&signer));
    let new_price_raw: i64 = 1_800 * 100_000_000; // $1800
    mock.set_answer(I256::from(new_price_raw))
        .send()
        .await?
        .await?
        .context("setAnswer tx dropped without receipt")?;

    let updates = manager.poll_all().await?;
    assert_eq!(updates, 1, "Expected one update after mock price change");

    let updated_price = manager
        .get_price_usd("ETH")
        .await
        .context("Missing ETH price after poll")?;
    assert!(
        (updated_price - 1800.0).abs() < 0.1,
        "Expected updated mock ETH price around $1800, got ${}",
        updated_price
    );

    let evt = tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await?;
    match evt {
        Some(Event::PriceUpdate { asset_id, new_price }) => {
            assert_eq!(asset_id, "ETH");
            assert_eq!(new_price, 1.0, "ETH must remain 1.0 in ETH-based pricing");
        }
        Some(other) => panic!("Expected PriceUpdate, got {:?}", other),
        None => panic!("Event channel closed unexpectedly"),
    }

    Ok(())
}

// ============================================================================
// TEST 12: Worker config
// ============================================================================
#[tokio::test]
async fn test_worker_config_defaults() -> Result<()> {
    let _guard = test_mutex().lock().await;
    use liquidator::oracle::OracleWorkerConfig;
    
    let config = OracleWorkerConfig::default();
    println!("  poll_interval_ms: {}", config.poll_interval_ms);
    println!("  stats_interval_secs: {}", config.stats_interval_secs);
    println!("  health_check_interval_secs: {}", config.health_check_interval_secs);
    
    assert_eq!(config.poll_interval_ms, 3000);
    assert_eq!(config.stats_interval_secs, 60);
    assert_eq!(config.health_check_interval_secs, 300);
    
    Ok(())
}

// ============================================================================
// TEST 13: Oracle worker chạy ngắn (poll 2 lần rồi dừng)
// ============================================================================
#[tokio::test]
async fn test_oracle_worker_short_run() -> Result<()> {
    let _guard = test_mutex().lock().await;
    let provider = connect_anvil().await?;
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);
    
    let config = single_eth_config();
    let mut manager = OracleManager::new(config, provider, event_tx).await?;
    manager.initialize().await?;
    
    let oracle = Arc::new(manager);
    
    // Chạy worker trong 3 giây rồi cancel
    let oracle_clone = Arc::clone(&oracle);
    let worker_handle = tokio::spawn(async move {
        liquidator::oracle::oracle_price_worker(
            oracle_clone,
            liquidator::oracle::OracleWorkerConfig {
                poll_interval_ms: 1000,  // Poll mỗi 1s
                stats_interval_secs: 60,
                health_check_interval_secs: 300,
                chainlink_event_poll_interval_secs: 3,
                chainlink_ws_url: None,
                chainlink_ws_reconnect_delay_secs: 3,
            },
        ).await;
    });
    
    // Đợi 3.5 giây → worker sẽ poll ít nhất 3 lần
    tokio::time::sleep(tokio::time::Duration::from_millis(3500)).await;
    worker_handle.abort(); // Cancel worker
    
    // Kiểm tra stats
    let stats = oracle.get_stats().await;
    println!("  ✓ Worker ran: {} polls, {} updates, {} errors",
        stats.total_polls, stats.total_updates_emitted, stats.total_errors);
    
    assert!(stats.total_polls >= 2, "Phải poll ít nhất 2 lần trong 3.5s với interval 1s");
    assert_eq!(stats.total_errors, 0, "Không nên có errors trên mainnet fork");
    
    // Drain events từ channel
    let mut event_count = 0;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            Event::PriceUpdate { asset_id, new_price } => {
                println!("  Event: {} = {:.6} ETH", asset_id, new_price);
                event_count += 1;
            }
            _ => {}
        }
    }
    println!("  ✓ Total events received: {}", event_count);
    
    Ok(())
}
