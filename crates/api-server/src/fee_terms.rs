//! The taker fee terms the exchange charges per market, read on demand and
//! cached, for pricing entries and booking the fees their fills pay.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use polymarket_core::api::ClobClient;
use polymarket_core::types::FeeSchedule;
use tokio::sync::Mutex;
use tracing::warn;

/// How long a market's terms are reused.
const TTL: Duration = Duration::from_secs(600);
/// How long a failed read waits before the next attempt.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// The longest a read may hold up an entry.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long the last terms read stand in while reads keep failing.
const LAST_READ_MAX_AGE: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy)]
struct Entry {
    /// The last terms read successfully, if any, and when.
    terms: Option<FeeSchedule>,
    read_at: Option<Instant>,
    /// When the entry is due to be read again.
    refresh_at: Instant,
}

/// Fee terms by market, read from the exchange at most every [`TTL`].
#[derive(Default)]
pub struct FeeTermsCache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl FeeTermsCache {
    /// The terms the exchange charges in `market_id`. A failed read (or one
    /// slower than [`TIMEOUT`]) keeps the last terms read for up to
    /// [`LAST_READ_MAX_AGE`], is retried after [`RETRY_AFTER`], and is
    /// `None` when there are none that recent.
    pub async fn get(&self, clob: &ClobClient, market_id: &str) -> Option<FeeSchedule> {
        let cached = self.entries.lock().await.get(market_id).copied();
        let now = Instant::now();
        if let Some(entry) = cached.filter(|entry| now < entry.refresh_at) {
            return entry.terms;
        }
        let entry = match tokio::time::timeout(TIMEOUT, clob.get_fee_schedule(market_id)).await {
            Ok(Ok(terms)) => Entry {
                terms: Some(terms),
                read_at: Some(now),
                refresh_at: now + TTL,
            },
            failed => {
                let error = match failed {
                    Ok(Err(error)) => error.to_string(),
                    _ => "timed out".to_string(),
                };
                let last = cached.filter(|entry| {
                    entry
                        .read_at
                        .is_some_and(|at| now.duration_since(at) < LAST_READ_MAX_AGE)
                });
                warn!(
                    %market_id,
                    %error,
                    reusing_last_read = last.is_some_and(|entry| entry.terms.is_some()),
                    "Could not read the market's fee terms"
                );
                Entry {
                    terms: last.and_then(|entry| entry.terms),
                    read_at: last.and_then(|entry| entry.read_at),
                    refresh_at: now + RETRY_AFTER,
                }
            }
        };
        self.entries
            .lock()
            .await
            .insert(market_id.to_string(), entry);
        entry.terms
    }
}
