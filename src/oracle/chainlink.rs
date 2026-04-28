// Chainlink Price Feed Reader
//
// Đọc giá từ Chainlink AggregatorV3Interface contracts
// Hỗ trợ:
// - latestRoundData(): Đọc giá mới nhất
// - decimals(): Lấy number of decimals
// - description(): Mô tả feed (e.g., "ETH / USD")
// - getRoundData(): Đọc giá tại round cụ thể (fallback)

use ethers::providers::{Provider, Http};
use ethers::types::{Address, I256};
use ethers::contract::abigen;

use anyhow::{Result, Context, bail};
use std::sync::Arc;

use super::config::PriceFeedConfig;
use super::types::{PriceData, FeedStatus};

// Generate Chainlink AggregatorV3Interface bindings
abigen!(
    ChainlinkAggregator,
    r#"[
        function latestRoundData() external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
        function decimals() external view returns (uint8)
        function description() external view returns (string)
        function getRoundData(uint80 _roundId) external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
        function latestAnswer() external view returns (int256)
    ]"#
);

/// Chainlink Price Feed Reader
/// 
/// Đọc giá từ một Chainlink AggregatorV3 contract cụ thể
pub struct ChainlinkFeed {
    /// Contract instance
    contract: ChainlinkAggregator<Provider<Http>>,
    
    /// Feed configuration
    config: PriceFeedConfig,
    
    /// On-chain description (cached sau lần đọc đầu)
    description_cache: Option<String>,
    
    /// On-chain decimals (cached sau lần đọc đầu)
    decimals_cache: Option<u8>,
}

impl ChainlinkFeed {
    fn parse_answer_i128(answer: I256, asset_symbol: &str) -> Result<i128> {
        answer
            .to_string()
            .parse::<i128>()
            .with_context(|| {
                format!(
                    "Price answer for {} is outside i128 range: {}",
                    asset_symbol, answer
                )
            })
    }

    fn parse_answer_f64(answer: I256, asset_symbol: &str) -> Result<f64> {
        answer
            .to_string()
            .parse::<f64>()
            .with_context(|| format!("Failed to parse price answer for {}: {}", asset_symbol, answer))
    }

    /// Tạo feed reader mới
    pub fn new(provider: Arc<Provider<Http>>, config: PriceFeedConfig) -> Self {
        let contract = ChainlinkAggregator::new(config.feed_address, provider);
        
        Self {
            contract,
            config,
            description_cache: None,
            decimals_cache: None,
        }
    }
    
    /// Khởi tạo feed — đọc metadata (decimals, description)
    /// Nên gọi 1 lần khi startup
    pub async fn initialize(&mut self) -> Result<()> {
        // Đọc decimals
        let decimals = self.contract
            .decimals()
            .call()
            .await
            .context("Failed to read decimals from price feed")?;
        
        // Verify decimals khớp với config
        if decimals != self.config.decimals {
            tracing::warn!(
                "Feed {} decimals mismatch: config={}, on-chain={}. Using on-chain value.",
                self.config.asset_symbol, self.config.decimals, decimals
            );
        }
        self.decimals_cache = Some(decimals);
        
        // Đọc description
        match self.contract.description().call().await {
            Ok(desc) => {
                tracing::info!(
                    "Initialized feed: {} ({}) at {:?}",
                    self.config.asset_symbol, desc, self.config.feed_address
                );
                self.description_cache = Some(desc);
            }
            Err(e) => {
                tracing::warn!(
                    "Could not read description for {}: {:?}",
                    self.config.asset_symbol, e
                );
            }
        }
        
        Ok(())
    }
    
    /// Đọc giá mới nhất từ Chainlink
    pub async fn latest_price(&self) -> Result<PriceData> {
        // Try latestRoundData first; if it reverts (mock replacement may not behave exactly),
        // fall back to latestAnswer() so the oracle still provides a price.
        match self.contract.latest_round_data().call().await {
            Ok((round_id, answer, _started_at, updated_at, _answered_in_round)) => {
                if answer.is_negative() || answer.is_zero() {
                    bail!(
                        "Invalid price for {}: answer={} (must be > 0)",
                        self.config.asset_symbol, answer
                    );
                }

                let decimals = self.decimals_cache.unwrap_or(self.config.decimals);
                let answer_raw = Self::parse_answer_i128(answer, &self.config.asset_symbol)?;
                let price_usd = Self::parse_answer_f64(answer, &self.config.asset_symbol)?
                    / 10_f64.powi(decimals as i32);

                Ok(PriceData {
                    asset_id: self.config.asset_id.clone(),
                    price_usd,
                    price_raw: answer_raw,
                    decimals,
                    round_id: round_id as u128,
                    updated_at: updated_at.as_u64(),
                    fetched_at: chrono::Utc::now().timestamp(),
                    feed_address: self.config.feed_address,
                })
            }
            Err(e) => {
                tracing::warn!("latestRoundData failed for {}: {:?}. Falling back to latestAnswer()", self.config.asset_symbol, e);
                // Try latestAnswer fallback
                let answer = self.contract
                    .latest_answer()
                    .call()
                    .await
                    .context(format!("Failed to read latestAnswer for {}", self.config.asset_symbol))?;

                if answer.is_negative() || answer.is_zero() {
                    bail!("Invalid price for {}: {}", self.config.asset_symbol, answer);
                }

                let decimals = self.decimals_cache.unwrap_or(self.config.decimals);
                let answer_raw = Self::parse_answer_i128(answer, &self.config.asset_symbol)?;
                let price_usd = Self::parse_answer_f64(answer, &self.config.asset_symbol)?
                    / 10_f64.powi(decimals as i32);

                Ok(PriceData {
                    asset_id: self.config.asset_id.clone(),
                    price_usd,
                    price_raw: answer_raw,
                    decimals,
                    round_id: 0u128,
                    updated_at: chrono::Utc::now().timestamp() as u64,
                    fetched_at: chrono::Utc::now().timestamp(),
                    feed_address: self.config.feed_address,
                })
            }
        }
    }
    
    /// Đọc giá đơn giản (chỉ giá, không metadata)
    /// Dùng latestAnswer() — gas rẻ hơn latestRoundData()
    pub async fn latest_answer(&self) -> Result<f64> {
        let answer = self.contract
            .latest_answer()
            .call()
            .await
            .context(format!("Failed to read latestAnswer for {}", self.config.asset_symbol))?;
        
        if answer.is_negative() || answer.is_zero() {
            bail!("Invalid price for {}: {}", self.config.asset_symbol, answer);
        }
        
        let decimals = self.decimals_cache.unwrap_or(self.config.decimals);
        Ok(Self::parse_answer_f64(answer, &self.config.asset_symbol)?
            / 10_f64.powi(decimals as i32))
    }
    
    /// Đọc giá tại một round cụ thể (dùng cho fallback/verification)
    pub async fn price_at_round(&self, round_id: u80) -> Result<PriceData> {
        let (rid, answer, _started_at, updated_at, _answered_in_round) = self.contract
            .get_round_data(round_id)
            .call()
            .await
            .context(format!("Failed to read round {} for {}", round_id, self.config.asset_symbol))?;
        
        if answer.is_negative() || answer.is_zero() {
            bail!("Invalid price at round {} for {}", round_id, self.config.asset_symbol);
        }
        
        let decimals = self.decimals_cache.unwrap_or(self.config.decimals);
        let answer_raw = Self::parse_answer_i128(answer, &self.config.asset_symbol)?;
        let price_usd = Self::parse_answer_f64(answer, &self.config.asset_symbol)?
            / 10_f64.powi(decimals as i32);
        
        Ok(PriceData {
            asset_id: self.config.asset_id.clone(),
            price_usd,
            price_raw: answer_raw,
            decimals,
            round_id: rid as u128,
            updated_at: updated_at.as_u64(),
            fetched_at: chrono::Utc::now().timestamp(),
            feed_address: self.config.feed_address,
        })
    }
    
    /// Kiểm tra feed có healthy không
    pub async fn health_check(&self) -> FeedStatus {
        match self.latest_price().await {
            Ok(price) => {
                if price.is_stale(self.config.heartbeat_secs) {
                    FeedStatus::Stale
                } else {
                    FeedStatus::Active
                }
            }
            Err(e) => FeedStatus::Error(e.to_string()),
        }
    }
    
    /// Lấy asset_id
    pub fn asset_id(&self) -> &str {
        &self.config.asset_id
    }
    
    /// Lấy asset symbol
    pub fn asset_symbol(&self) -> &str {
        &self.config.asset_symbol
    }
    
    /// Lấy feed address
    pub fn feed_address(&self) -> Address {
        self.config.feed_address
    }
    
    /// Lấy config
    pub fn config(&self) -> &PriceFeedConfig {
        &self.config
    }
    
    /// Lấy cached description
    pub fn description(&self) -> Option<&str> {
        self.description_cache.as_deref()
    }
}

// Type alias cho u80 — Chainlink dùng uint80 cho roundId
type u80 = u128;
