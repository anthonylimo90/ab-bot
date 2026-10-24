//! Trade indexer for continuous wallet monitoring.

use alloy_primitives::keccak256;
use anyhow::Result;
use polymarket_core::api::PolygonClient;
use polymarket_core::config::Config;
use polymarket_core::db;
use polymarket_core::db::wallets::WalletRepository;
use polymarket_core::types::BotScore;
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Polymarket contract addresses for trade event monitoring.
pub mod contracts {
    pub use polymarket_core::signing::{CTF_EXCHANGE_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS};

    /// The exchanges whose trades are indexed: the CTF Exchange V2 for
    /// standard markets and its neg-risk counterpart (trading moved to them
    /// at the CLOB V2 cutover of 2026-04-28).
    pub const EXCHANGES: [&str; 2] = [CTF_EXCHANGE_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS];
}

/// CTF Exchange V2 event signatures that identify trading wallets.
///
/// ```solidity
/// event OrderFilled(
///     bytes32 indexed orderHash, address indexed maker, address indexed taker,
///     Side side, uint256 tokenId, uint256 makerAmountFilled,
///     uint256 takerAmountFilled, uint256 fee, bytes32 builder, bytes32 metadata);
/// event OrdersMatched(
///     bytes32 indexed takerOrderHash, address indexed takerOrderMaker,
///     Side side, uint256 tokenId, uint256 makerAmountFilled,
///     uint256 takerAmountFilled);
/// ```
///
/// In both events the first indexed parameter is an **order hash**, not an
/// address; only the `address`-typed indexed parameters are wallets.
pub mod events {
    pub const ORDER_FILLED: &str =
        "OrderFilled(bytes32,address,address,uint8,uint256,uint256,uint256,uint256,bytes32,bytes32)";
    pub const ORDERS_MATCHED: &str = "OrdersMatched(bytes32,address,uint8,uint256,uint256,uint256)";
}

/// Largest block range requested from `eth_getLogs` in one poll.
///
/// After downtime or an RPC error the indexer may be far behind the chain head;
/// an unbounded range would exceed provider result/size caps and fail forever.
/// 250 Polygon blocks (~8 minutes) keeps a response to a few thousand logs, and
/// catching up at 250 blocks per poll is much faster than the chain advances.
pub const MAX_BLOCKS_PER_POLL: u64 = 250;

/// How far behind the head the indexer reads. Load-balanced RPC providers
/// serve some requests from nodes a block or two behind, which refuse a range
/// ending at the head or answer it without the newest blocks' logs; once
/// passed, those blocks would never be read again.
const HEAD_LAG_BLOCKS: u64 = 5;

/// Maximum number of distinct wallets remembered as already analyzed.
///
/// Bounds memory for a process that runs indefinitely; the least recently
/// seen wallet is forgotten first (and would simply be re-analyzed, which also
/// refreshes its score, if it trades again).
pub const MAX_TRACKED_WALLETS: usize = 50_000;

/// Topic hashes of the monitored CTF Exchange events and the decoder for their
/// address-typed indexed parameters.
#[derive(Debug, Clone)]
pub struct TradeEventTopics {
    order_filled: String,
    orders_matched: String,
}

impl TradeEventTopics {
    pub fn new() -> Self {
        Self {
            order_filled: topic0(events::ORDER_FILLED),
            orders_matched: topic0(events::ORDERS_MATCHED),
        }
    }

    /// `topics[0]` alternatives for an `eth_getLogs` filter selecting only the
    /// monitored events (the OR form: `[[a, b]]`).
    pub fn log_filter(&self) -> serde_json::Value {
        serde_json::json!([[self.order_filled, self.orders_matched]])
    }

    /// Trader wallet addresses (lowercase `0x…`, 40 hex chars) carried by one
    /// log's topics.
    ///
    /// * `OrderFilled`: topics = `[sig, orderHash, maker, taker]` → maker, taker.
    /// * `OrdersMatched`: topics = `[sig, takerOrderHash, takerOrderMaker]` →
    ///   takerOrderMaker.
    /// * Any other event, or a log whose topic count does not match the ABI,
    ///   yields nothing.
    ///
    /// The zero address and the exchange contracts themselves (an exchange
    /// appears as `taker` of the order it fills against makers) are not
    /// trader wallets and are dropped.
    pub fn trader_addresses(&self, topics: &[String]) -> Vec<String> {
        let Some(signature) = topics.first() else {
            return Vec::new();
        };
        let address_positions: &[usize] =
            if signature.eq_ignore_ascii_case(&self.order_filled) && topics.len() == 4 {
                &[2, 3]
            } else if signature.eq_ignore_ascii_case(&self.orders_matched) && topics.len() == 3 {
                &[2]
            } else {
                return Vec::new();
            };

        address_positions
            .iter()
            .filter_map(|&position| address_from_topic(&topics[position]))
            .filter(|address| !is_non_trader(address))
            .collect()
    }
}

impl Default for TradeEventTopics {
    fn default() -> Self {
        Self::new()
    }
}

/// `keccak256(signature)` as a `0x`-prefixed lowercase hex string.
fn topic0(signature: &str) -> String {
    format!("{:#x}", keccak256(signature.as_bytes()))
}

/// Decode an `address`-typed indexed parameter: a 32-byte topic whose first
/// 12 bytes are zero padding. Anything else (e.g. a `bytes32` order hash) is
/// not an address.
fn address_from_topic(topic: &str) -> Option<String> {
    let hex = topic
        .strip_prefix("0x")
        .or_else(|| topic.strip_prefix("0X"))?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (padding, address) = hex.split_at(24);
    if padding.bytes().any(|b| b != b'0') {
        return None;
    }
    Some(format!("0x{}", address.to_ascii_lowercase()))
}

fn is_non_trader(address: &str) -> bool {
    address == "0x0000000000000000000000000000000000000000"
        || contracts::EXCHANGES
            .iter()
            .any(|exchange| address.eq_ignore_ascii_case(exchange))
}

/// How soon a scored wallet is analyzed again: its behaviour can change, and
/// a score from its first trades would otherwise stand for good.
const RESCORE_AFTER: Duration = Duration::from_secs(24 * 3600);
/// How soon a wallet whose analysis failed (an Alchemy error, a database
/// write) is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
/// How soon a wallet with too few transfers to score is looked at again: it
/// may have enough later. Each look costs an Alchemy transfer history call.
const RECHECK_AFTER: Duration = Duration::from_secs(6 * 3600);

/// Bounded set of recently seen wallet addresses with least-recently-seen
/// eviction, each with when it is next due for analysis.
#[derive(Debug)]
pub struct RecentWallets {
    capacity: usize,
    tick: u64,
    by_address: HashMap<String, (u64, Instant)>,
    by_tick: BTreeMap<u64, String>,
}

impl RecentWallets {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            tick: 0,
            by_address: HashMap::new(),
            by_tick: BTreeMap::new(),
        }
    }

    /// Record that `address` was seen at `now`. Returns `true` if it was not
    /// tracked yet or is due for analysis again (the caller should analyze
    /// it); it is then next due after `RETRY_AFTER`, unless `scored` or
    /// `too_few_transfers` pushes that back. A known address becomes the most recently seen; when
    /// full, the least recently seen address is evicted first.
    pub fn observe(&mut self, address: &str, now: Instant) -> bool {
        self.tick += 1;
        let tick = self.tick;

        if let Some((previous, due)) = self.by_address.get_mut(address) {
            self.by_tick.remove(previous);
            *previous = tick;
            self.by_tick.insert(tick, address.to_string());
            if now < *due {
                return false;
            }
            *due = now + RETRY_AFTER;
            return true;
        }

        if self.by_address.len() >= self.capacity {
            if let Some((_, oldest)) = self.by_tick.pop_first() {
                self.by_address.remove(&oldest);
            }
        }
        self.by_address
            .insert(address.to_string(), (tick, now + RETRY_AFTER));
        self.by_tick.insert(tick, address.to_string());
        true
    }

    /// Record that `address` was scored at `now`: it is next due after
    /// `RESCORE_AFTER`.
    pub fn scored(&mut self, address: &str, now: Instant) {
        self.due_after(address, now + RESCORE_AFTER);
    }

    /// Record that `address` had too few transfers to score at `now`: it is
    /// next due after `RECHECK_AFTER`.
    pub fn too_few_transfers(&mut self, address: &str, now: Instant) {
        self.due_after(address, now + RECHECK_AFTER);
    }

    fn due_after(&mut self, address: &str, at: Instant) {
        if let Some((_, due)) = self.by_address.get_mut(address) {
            *due = at;
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_address.len()
    }

    #[cfg(test)]
    pub fn contains(&self, address: &str) -> bool {
        self.by_address.contains_key(address)
    }
}

/// Block range `[from, to]` to query next, or `None` when the chain has not
/// advanced past `last_block`. The range never exceeds `max_blocks`.
pub fn next_block_range(
    last_block: u64,
    current_block: u64,
    max_blocks: u64,
) -> Option<(u64, u64)> {
    if current_block <= last_block {
        return None;
    }
    let from = last_block + 1;
    let to = current_block.min(from.saturating_add(max_blocks.max(1) - 1));
    Some((from, to))
}

/// Continuous trade indexer that monitors Polymarket for new trades.
pub struct TradeIndexer {
    polygon: PolygonClient,
    _pool: PgPool,
    wallet_repo: WalletRepository,
    /// Last processed block.
    last_block: u64,
    /// Wallets we've already analyzed (bounded, least recently seen evicted).
    analyzed_wallets: RecentWallets,
    /// Event topics for the monitored exchange events.
    topics: TradeEventTopics,
    /// Polling interval.
    poll_interval: Duration,
}

impl TradeIndexer {
    /// Create a new trade indexer.
    pub async fn new(config: &Config) -> Result<Self> {
        let pool = db::create_pool(&config.database).await?;
        let wallet_repo = WalletRepository::new(pool.clone());

        let polygon = match config.polygon.get_rpc_url() {
            Some(url) => PolygonClient::new(url),
            None => {
                anyhow::bail!("No Polygon RPC URL configured");
            }
        };

        // Get current block as starting point
        let current_block = polygon
            .get_block_number()
            .await?
            .saturating_sub(HEAD_LAG_BLOCKS);

        Ok(Self {
            polygon,
            _pool: pool,
            wallet_repo,
            last_block: current_block,
            analyzed_wallets: RecentWallets::new(MAX_TRACKED_WALLETS),
            topics: TradeEventTopics::new(),
            poll_interval: Duration::from_secs(12), // ~Polygon block time
        })
    }

    /// Run the indexing loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting indexer from block {}", self.last_block);

        loop {
            if let Err(e) = self.poll_new_blocks().await {
                error!("Error polling blocks: {}", e);
            }

            crate::touch_health_file();
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Poll for new blocks and process trades.
    async fn poll_new_blocks(&mut self) -> Result<()> {
        let current_block = self
            .polygon
            .get_block_number()
            .await?
            .saturating_sub(HEAD_LAG_BLOCKS);

        let Some((from_block, to_block)) =
            next_block_range(self.last_block, current_block, MAX_BLOCKS_PER_POLL)
        else {
            return Ok(());
        };

        debug!("Processing blocks {} to {}", from_block, to_block);

        // Fetch only the exchanges' trade events; other events (admin changes,
        // token registration, ...) carry no trader wallets.
        let logs = self
            .polygon
            .get_logs_with_topics(
                &contracts::EXCHANGES,
                from_block,
                to_block,
                self.topics.log_filter(),
            )
            .await?;

        if !logs.is_empty() {
            info!(
                "Found {} trade events in blocks {}-{}",
                logs.len(),
                from_block,
                to_block
            );
        }

        // Extract unique trader wallet addresses from the address-typed topics
        let wallets: HashSet<String> = logs
            .iter()
            .flat_map(|log| self.topics.trader_addresses(&log.topics))
            .collect();

        // Analyze new wallets, and known ones due again
        for wallet in wallets {
            if self.analyzed_wallets.observe(&wallet, Instant::now()) {
                match self.analyze_wallet(&wallet).await {
                    Ok(true) => self.analyzed_wallets.scored(&wallet, Instant::now()),
                    Ok(false) => self
                        .analyzed_wallets
                        .too_few_transfers(&wallet, Instant::now()),
                    Err(e) => warn!("Failed to analyze wallet {}: {}", wallet, e),
                }
            }
        }

        // Advance only the range actually processed; a later poll continues.
        self.last_block = to_block;
        Ok(())
    }

    /// Analyze a single wallet; true when a score was stored.
    async fn analyze_wallet(&self, address: &str) -> Result<bool> {
        debug!("Analyzing wallet: {}", address);

        // Fetch wallet transactions (both directions)
        let transfers = self
            .polygon
            .get_asset_transfers(address, None, None)
            .await?;

        if transfers.len() < 10 {
            // Too few trades to score yet
            return Ok(false);
        }

        // Extract features, then add the realized win rate from trade history
        let mut features =
            polymarket_core::feature_extractor::extract_features(address, &transfers)?;
        crate::trade_history::attach_win_rate(self.wallet_repo.pool(), &mut features).await;

        // Calculate bot score
        let score = BotScore::new(address.to_string(), &features);

        // Log interesting findings
        if score.total_score >= 25 {
            info!(
                "Interesting wallet {}: score={}, classification={:?}",
                address, score.total_score, score.classification
            );
        }

        // Save to database
        self.wallet_repo.upsert_features(&features).await?;
        self.wallet_repo.insert_score(&score).await?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAKER: &str = "0x1111111111111111111111111111111111111111";
    const TAKER: &str = "0x2222222222222222222222222222222222222222";
    const ORDER_HASH: &str = "0xa1b2c3d4e5f60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00";

    fn topic_for(address: &str) -> String {
        format!("0x{:0>64}", address.trim_start_matches("0x"))
    }

    fn order_filled(topics: &TradeEventTopics) -> Vec<String> {
        vec![
            topics.order_filled.clone(),
            ORDER_HASH.to_string(),
            topic_for(MAKER),
            topic_for(TAKER),
        ]
    }

    #[test]
    fn event_topic_hashes_are_the_v2_exchanges() {
        let topics = TradeEventTopics::new();
        // Not the V1 exchange's topics, which no longer carry trades.
        assert_ne!(
            topics.order_filled,
            "0xd0a08e8c493f9c94f29311604c9de1b4e8c8d4c06bd0c789af57f2d65bfec0f6"
        );
        assert_ne!(
            topics.orders_matched,
            "0x63bf4d16b7fa898ef4c4b2b6d90fd201e9c56313b65638af6088d149d2ce956c"
        );
        // The signatures the CTF Exchange V2 hashes into its topics.
        assert_eq!(
            topics.order_filled,
            topic0("OrderFilled(bytes32,address,address,uint8,uint256,uint256,uint256,uint256,bytes32,bytes32)")
        );
        assert_eq!(
            topics.orders_matched,
            topic0("OrdersMatched(bytes32,address,uint8,uint256,uint256,uint256)")
        );
    }

    #[test]
    fn log_filter_selects_only_the_trade_events() {
        let topics = TradeEventTopics::new();
        let filter = topics.log_filter();
        let alternatives = filter[0].as_array().expect("OR list at topic0");
        assert_eq!(
            filter.as_array().unwrap().len(),
            1,
            "no constraints on other topics"
        );
        assert_eq!(alternatives.len(), 2);
        assert_eq!(alternatives[0], topics.order_filled.as_str());
        assert_eq!(alternatives[1], topics.orders_matched.as_str());
    }

    #[test]
    fn order_filled_yields_maker_and_taker_but_not_the_order_hash() {
        let topics = TradeEventTopics::new();
        let addresses = topics.trader_addresses(&order_filled(&topics));
        assert_eq!(addresses, vec![MAKER.to_string(), TAKER.to_string()]);

        // The old decoder turned every topic >= 42 chars into an "address",
        // including the order hash.
        let bogus = format!("0x{}", &ORDER_HASH[ORDER_HASH.len() - 40..]);
        assert!(!addresses.contains(&bogus));
    }

    #[test]
    fn orders_matched_yields_only_the_order_maker() {
        let topics = TradeEventTopics::new();
        let log = vec![
            topics.orders_matched.clone(),
            ORDER_HASH.to_string(),
            topic_for(MAKER),
        ];
        assert_eq!(topics.trader_addresses(&log), vec![MAKER.to_string()]);
    }

    #[test]
    fn other_events_and_malformed_logs_yield_nothing() {
        let topics = TradeEventTopics::new();

        // Some other exchange event with an address-looking indexed topic
        // (e.g. NewOperator(address indexed, address indexed)).
        let other = vec![
            topic0("NewOperator(address,address)"),
            topic_for(MAKER),
            topic_for(TAKER),
        ];
        assert!(topics.trader_addresses(&other).is_empty());

        // Right signature, wrong number of topics: refuse to guess.
        let mut short = order_filled(&topics);
        short.pop();
        assert!(topics.trader_addresses(&short).is_empty());
        let mut long = order_filled(&topics);
        long.push(topic_for(MAKER));
        assert!(topics.trader_addresses(&long).is_empty());

        assert!(topics.trader_addresses(&[]).is_empty());
    }

    #[test]
    fn non_address_topics_are_rejected() {
        let topics = TradeEventTopics::new();
        // maker/taker slots holding values with non-zero high bytes are not
        // address-typed and must be dropped.
        let log = vec![
            topics.order_filled.clone(),
            ORDER_HASH.to_string(),
            ORDER_HASH.to_string(),
            topic_for(TAKER),
        ];
        assert_eq!(topics.trader_addresses(&log), vec![TAKER.to_string()]);
    }

    #[test]
    fn zero_address_and_the_exchange_contract_are_not_traders() {
        let topics = TradeEventTopics::new();
        let log = vec![
            topics.order_filled.clone(),
            ORDER_HASH.to_string(),
            topic_for(contracts::CTF_EXCHANGE_ADDRESS),
            topic_for("0x0000000000000000000000000000000000000000"),
        ];
        assert!(topics.trader_addresses(&log).is_empty());
        let log = vec![
            topics.order_filled.clone(),
            ORDER_HASH.to_string(),
            topic_for(MAKER),
            topic_for(contracts::NEG_RISK_CTF_EXCHANGE_ADDRESS),
        ];
        assert_eq!(topics.trader_addresses(&log), vec![MAKER.to_string()]);
    }

    #[test]
    fn addresses_are_normalized_to_lowercase() {
        let topics = TradeEventTopics::new();
        let mixed = "0xAbCdEf0123456789aBcDeF0123456789AbCdEf01";
        let log = vec![
            topics.orders_matched.to_uppercase().replace("0X", "0x"),
            ORDER_HASH.to_string(),
            topic_for(mixed).to_uppercase().replace("0X", "0x"),
        ];
        assert_eq!(
            topics.trader_addresses(&log),
            vec![mixed.to_ascii_lowercase()]
        );
    }

    #[test]
    fn address_from_topic_validates_shape() {
        assert_eq!(
            address_from_topic(&topic_for(MAKER)),
            Some(MAKER.to_string())
        );
        assert_eq!(address_from_topic("0x1234"), None, "too short");
        assert_eq!(address_from_topic(&format!("{:0>64}", "ab")), None, "no 0x");
        let non_hex = format!("0x{}", "z".repeat(64));
        assert_eq!(address_from_topic(&non_hex), None);
        assert_eq!(address_from_topic(ORDER_HASH), None, "bytes32 hash");
    }

    #[test]
    fn block_range_is_bounded_and_resumable() {
        // Nothing new.
        assert_eq!(next_block_range(100, 100, 250), None);
        assert_eq!(next_block_range(100, 99, 250), None);
        // A few new blocks: process them all.
        assert_eq!(next_block_range(100, 105, 250), Some((101, 105)));
        // Exactly the cap.
        assert_eq!(next_block_range(100, 350, 250), Some((101, 350)));
        // Far behind: only one chunk per poll, then continue from its end.
        assert_eq!(next_block_range(100, 100_000, 250), Some((101, 350)));
        assert_eq!(next_block_range(350, 100_000, 250), Some((351, 600)));
        // A zero cap still makes progress.
        assert_eq!(next_block_range(10, 20, 0), Some((11, 11)));
        // No overflow near u64::MAX.
        assert_eq!(
            next_block_range(u64::MAX - 2, u64::MAX, 250),
            Some((u64::MAX - 1, u64::MAX))
        );
    }

    #[test]
    fn recent_wallets_flags_new_addresses_once() {
        let mut set = RecentWallets::new(10);
        let now = Instant::now();
        assert!(set.observe("0xa", now));
        assert!(!set.observe("0xa", now));
        assert!(set.observe("0xb", now));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn wallets_are_analyzed_again_when_due() {
        let mut set = RecentWallets::new(10);
        let start = Instant::now();
        // A failed analysis: tried again after half an hour.
        assert!(set.observe("0xa", start));
        assert!(!set.observe("0xa", start + RETRY_AFTER - Duration::from_secs(1)));
        assert!(set.observe("0xa", start + RETRY_AFTER));
        // Too few transfers: looked at again after six hours.
        let checked_at = start + RETRY_AFTER;
        set.too_few_transfers("0xa", checked_at);
        assert!(!set.observe("0xa", checked_at + RETRY_AFTER));
        assert!(set.observe("0xa", checked_at + RECHECK_AFTER));
        // Scored: not again for a day.
        let scored_at = checked_at + RECHECK_AFTER;
        set.scored("0xa", scored_at);
        assert!(!set.observe("0xa", scored_at + RECHECK_AFTER));
        assert!(set.observe("0xa", scored_at + RESCORE_AFTER));
    }

    #[test]
    fn recent_wallets_is_bounded_and_evicts_least_recently_seen() {
        let now = Instant::now();
        let mut set = RecentWallets::new(3);
        for a in ["0xa", "0xb", "0xc"] {
            assert!(set.observe(a, now));
        }
        // Touch 0xa so 0xb becomes the least recently seen.
        assert!(!set.observe("0xa", now));
        // Inserting a fourth evicts 0xb, not 0xa.
        assert!(set.observe("0xd", now));
        assert_eq!(set.len(), 3);
        assert!(set.contains("0xa") && set.contains("0xc") && set.contains("0xd"));
        assert!(!set.contains("0xb"));
        // The evicted wallet is treated as new again.
        assert!(set.observe("0xb", now));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn recent_wallets_never_grows_past_capacity() {
        let now = Instant::now();
        let mut set = RecentWallets::new(100);
        for i in 0..10_000 {
            set.observe(&format!("0x{i:040x}"), now);
            assert!(set.len() <= 100);
            assert_eq!(set.by_tick.len(), set.by_address.len());
        }
        assert_eq!(set.len(), 100);
        // Zero capacity is clamped to one slot.
        let mut tiny = RecentWallets::new(0);
        assert!(tiny.observe("0xa", now));
        assert!(tiny.observe("0xb", now));
        assert_eq!(tiny.len(), 1);
    }
}
