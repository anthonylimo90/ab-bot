//! Client identification for rate limiting behind a reverse proxy.
//!
//! `tower_governor`'s `SmartIpKeyExtractor` trusts the *leftmost* parseable
//! `X-Forwarded-For` entry, which is whatever the client sent, so rotating that
//! header gives every request a fresh rate-limit bucket. Proxies append the
//! address they observed to the *right* of the header, so only entries counted
//! from the right can be trusted, and only as far as there are trusted proxies.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use tower_governor::{key_extractor::KeyExtractor, GovernorError};

/// Number of trusted reverse proxies in front of the server when unset.
const DEFAULT_TRUSTED_PROXY_HOPS: usize = 1;

/// How often idle per-client rate-limit state is dropped.
const LIMITER_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Rate-limit key extractor that only trusts `X-Forwarded-For` entries appended
/// by our own proxies.
///
/// With `trusted_hops = N` the client address is the Nth entry from the right
/// (`1` is the entry appended by the edge proxy). `0` means no proxy is
/// trusted and the TCP peer address is always used. Without a usable entry the
/// peer address from `ConnectInfo` is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxyKeyExtractor {
    trusted_hops: usize,
}

impl TrustedProxyKeyExtractor {
    pub fn new(trusted_hops: usize) -> Self {
        Self { trusted_hops }
    }

    /// Read `TRUSTED_PROXY_HOPS` (default 1).
    pub fn from_env() -> Self {
        let hops = match std::env::var("TRUSTED_PROXY_HOPS") {
            Ok(raw) => raw.trim().parse::<usize>().unwrap_or_else(|_| {
                tracing::warn!(
                    value = %raw,
                    default = DEFAULT_TRUSTED_PROXY_HOPS,
                    "Invalid TRUSTED_PROXY_HOPS, using default"
                );
                DEFAULT_TRUSTED_PROXY_HOPS
            }),
            Err(_) => {
                tracing::warn!(
                    default = DEFAULT_TRUSTED_PROXY_HOPS,
                    "TRUSTED_PROXY_HOPS is unset: trusting one reverse proxy's X-Forwarded-For. \
                     If clients reach this server directly, set TRUSTED_PROXY_HOPS=0 or they \
                     can choose their own rate-limit key"
                );
                DEFAULT_TRUSTED_PROXY_HOPS
            }
        };
        Self::new(hops)
    }
}

impl KeyExtractor for TrustedProxyKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        Ok(client_ip(req.headers(), peer, self.trusted_hops))
    }
}

/// Resolve the address a request should be rate limited as.
///
/// Never fails: a request that cannot be attributed to anything (no trusted
/// forwarding entry and no peer address, which only happens when the server is
/// not served with `ConnectInfo`) shares one fallback bucket instead of
/// producing a 500.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>, trusted_hops: usize) -> IpAddr {
    rate_limit_key(
        forwarded_client(headers, trusted_hops)
            .or(peer)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
    )
}

/// The bucket an address is limited in. One IPv6 client normally holds a
/// whole /64, so its addresses share a bucket; IPv4-mapped addresses count as
/// the IPv4 address they carry.
fn rate_limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut segments = v6.segments();
                segments[4..].fill(0);
                IpAddr::V6(Ipv6Addr::from(segments))
            }
        },
        v4 => v4,
    }
}

/// The entry `trusted_hops` places from the right of the combined
/// `X-Forwarded-For` header lines, if there is one and it is a valid address.
///
/// Entries to its left were supplied by earlier hops or by the client and are
/// ignored. A header with fewer entries than trusted hops did not pass through
/// all of our proxies, so it is not trusted at all.
fn forwarded_client(headers: &HeaderMap, trusted_hops: usize) -> Option<IpAddr> {
    if trusted_hops == 0 {
        return None;
    }
    let mut entries = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        entries.extend(value.to_str().ok()?.split(',').map(str::trim));
    }
    let index = entries.len().checked_sub(trusted_hops)?;
    parse_forwarded_entry(entries[index])
}

/// Parse `1.2.3.4`, `1.2.3.4:5678`, `::1` or `[::1]:5678`.
fn parse_forwarded_entry(entry: &str) -> Option<IpAddr> {
    entry
        .parse::<IpAddr>()
        .ok()
        .or_else(|| entry.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

/// Cleanup hooks for the keyed rate limiters, which otherwise keep one entry per
/// client address forever.
#[derive(Default)]
pub struct LimiterJanitor {
    // Each hook prunes one limiter and reports whether it is still alive.
    hooks: Vec<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl LimiterJanitor {
    /// Register a limiter. Only a weak reference is held, so dropping the router
    /// that owns the limiter also ends its cleanup.
    pub fn register<L, F>(&mut self, limiter: &Arc<L>, prune: F)
    where
        L: Send + Sync + 'static,
        F: Fn(&L) + Send + Sync + 'static,
    {
        let weak: Weak<L> = Arc::downgrade(limiter);
        self.hooks.push(Box::new(move || match weak.upgrade() {
            Some(limiter) => {
                prune(&limiter);
                true
            }
            None => false,
        }));
    }

    /// Prune every registered limiter once; returns how many are still alive.
    pub fn run_once(&self) -> usize {
        self.hooks.iter().filter(|hook| hook()).count()
    }

    /// Prune periodically on the current Tokio runtime. A no-op outside one.
    pub fn spawn(self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(LIMITER_CLEANUP_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if self.run_once() == 0 {
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(lines: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for line in lines {
            map.append("x-forwarded-for", line.parse().unwrap());
        }
        map
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn rightmost_entry_is_used_so_spoofed_leftmost_entries_share_one_key() {
        let peer = Some(ip("10.0.0.1"));
        let a = client_ip(&headers(&["1.1.1.1, 203.0.113.9"]), peer, 1);
        let b = client_ip(&headers(&["2.2.2.2, 203.0.113.9"]), peer, 1);
        let c = client_ip(&headers(&["203.0.113.9"]), peer, 1);
        assert_eq!(a, ip("203.0.113.9"));
        assert_eq!(a, b);
        assert_eq!(a, c);
        // A genuinely different client behind the proxy gets a different key.
        assert_ne!(a, client_ip(&headers(&["1.1.1.1, 198.51.100.7"]), peer, 1));
    }

    #[test]
    fn extra_trusted_hops_are_skipped_from_the_right() {
        let h = headers(&["9.9.9.9, 203.0.113.9, 10.1.1.1"]);
        assert_eq!(client_ip(&h, None, 2), ip("203.0.113.9"));
        assert_eq!(client_ip(&h, None, 3), ip("9.9.9.9"));
        // More trusted hops than entries: the header is not trusted at all.
        assert_eq!(client_ip(&h, Some(ip("10.0.0.1")), 4), ip("10.0.0.1"));
    }

    #[test]
    fn repeated_header_lines_are_combined_in_order() {
        let h = headers(&["6.6.6.6", "203.0.113.9"]);
        assert_eq!(client_ip(&h, None, 1), ip("203.0.113.9"));
        assert_eq!(client_ip(&h, None, 2), ip("6.6.6.6"));
    }

    #[test]
    fn requests_without_forwarding_headers_use_the_peer_address() {
        let none = HeaderMap::new();
        assert_eq!(client_ip(&none, Some(ip("192.0.2.5")), 1), ip("192.0.2.5"));
        assert_eq!(
            client_ip(&none, Some(ip("2001:db8::1")), 1),
            ip("2001:db8::")
        );
        // No peer either: a shared fallback bucket rather than an error.
        assert_eq!(client_ip(&none, None, 1), ip("0.0.0.0"));
    }

    #[test]
    fn zero_trusted_hops_ignores_the_header_entirely() {
        let h = headers(&["203.0.113.9"]);
        assert_eq!(client_ip(&h, Some(ip("192.0.2.5")), 0), ip("192.0.2.5"));
    }

    #[test]
    fn unparseable_trusted_entry_falls_back_to_the_peer_never_a_client_chosen_one() {
        let peer = Some(ip("192.0.2.5"));
        // The rightmost entry is ours; if it is garbage do not fall through to
        // the (client-controlled) entries on its left.
        assert_eq!(
            client_ip(&headers(&["1.2.3.4, unknown"]), peer, 1),
            ip("192.0.2.5")
        );
        assert_eq!(
            client_ip(&headers(&["1.2.3.4, "]), peer, 1),
            ip("192.0.2.5")
        );
    }

    #[test]
    fn ports_and_ipv6_forms_are_understood() {
        let peer = None;
        assert_eq!(
            client_ip(&headers(&["203.0.113.9:4711"]), peer, 1),
            ip("203.0.113.9")
        );
        assert_eq!(
            client_ip(&headers(&["[2001:db8::7]:99"]), peer, 1),
            ip("2001:db8::")
        );
        assert_eq!(
            client_ip(&headers(&["2001:db8::7"]), peer, 1),
            ip("2001:db8::")
        );
    }

    #[test]
    fn an_ipv6_client_is_limited_per_slash_64() {
        let peer = None;
        // Rotating addresses inside one /64 does not buy fresh buckets...
        assert_eq!(
            client_ip(&headers(&["2001:db8:1:2:aaaa::1"]), peer, 1),
            client_ip(&headers(&["2001:db8:1:2:ffff:1:2:3"]), peer, 1)
        );
        // ...while different /64s stay apart.
        assert_ne!(
            client_ip(&headers(&["2001:db8:1:2::1"]), peer, 1),
            client_ip(&headers(&["2001:db8:1:3::1"]), peer, 1)
        );
        // IPv4-mapped addresses count as the IPv4 address.
        assert_eq!(
            client_ip(&headers(&["::ffff:203.0.113.9"]), peer, 1),
            ip("203.0.113.9")
        );
        assert_eq!(
            client_ip(&HeaderMap::new(), Some(ip("2001:db8::9")), 0),
            ip("2001:db8::")
        );
    }

    #[test]
    fn extractor_reads_connect_info_and_headers_from_a_request() {
        let extractor = TrustedProxyKeyExtractor::new(1);
        let mut request = Request::builder()
            .header("x-forwarded-for", "7.7.7.7, 203.0.113.9")
            .body(())
            .unwrap();
        assert_eq!(extractor.extract(&request).unwrap(), ip("203.0.113.9"));

        let mut direct = Request::builder().body(()).unwrap();
        assert_eq!(extractor.extract(&direct).unwrap(), ip("0.0.0.0"));
        direct
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 44], 5000))));
        assert_eq!(extractor.extract(&direct).unwrap(), ip("192.0.2.44"));

        // Forwarding headers beat the peer, which is the proxy's own address.
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 5000))));
        assert_eq!(extractor.extract(&request).unwrap(), ip("203.0.113.9"));
    }

    /// A router wired exactly like production (real limiters and key extractor).
    /// The login body is invalid JSON-for-the-handler, so requests are answered
    /// with 400 without touching the database; only the limiter decides 429.
    fn app() -> axum::Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/not-used").unwrap();
        crate::routes::create_router(crate::state::AppState::for_tests(pool))
    }

    async fn login(
        app: &axum::Router,
        forwarded_for: Option<&str>,
        peer: Option<SocketAddr>,
    ) -> axum::http::StatusCode {
        use tower::ServiceExt;
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json");
        if let Some(value) = forwarded_for {
            request = request.header("x-forwarded-for", value);
        }
        let mut request = request.body(axum::body::Body::from("{}")).unwrap();
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn rotating_the_leftmost_forwarded_for_entry_does_not_reset_the_limit() {
        use axum::http::StatusCode;
        let app = app();
        // The auth limiter allows a burst of 5 per client. Each request claims a
        // different client on the left, but the proxy-appended entry is the same.
        for i in 0..5 {
            let status = login(&app, Some(&format!("10.9.{i}.1, 203.0.113.9")), None).await;
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "request {i}");
        }
        assert_eq!(
            login(&app, Some("10.9.99.1, 203.0.113.9"), None).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        // A genuinely different client behind the same proxy has its own budget.
        assert_ne!(
            login(&app, Some("10.9.0.1, 198.51.100.7"), None).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn requests_without_forwarding_headers_are_limited_per_peer() {
        use axum::http::StatusCode;
        let app = app();
        let peer_a = SocketAddr::from(([192, 0, 2, 10], 40000));
        let peer_b = SocketAddr::from(([192, 0, 2, 11], 40000));
        for i in 0..5 {
            let status = login(&app, None, Some(peer_a)).await;
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "request {i}");
        }
        assert_eq!(
            login(&app, None, Some(peer_a)).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        // Another peer is unaffected.
        assert_ne!(
            login(&app, None, Some(peer_b)).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        // No peer information at all must not fail the request (it used to 500).
        let status = login(&app, None, None).await;
        assert!(status.is_client_error(), "{status}");
    }

    #[tokio::test]
    async fn served_with_connect_info_a_direct_client_is_keyed_by_its_peer_address() {
        use axum::http::StatusCode;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/v1/auth/login",
            listener.local_addr().unwrap()
        );
        let router = app();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let client = reqwest::Client::new();
        let mut statuses = Vec::new();
        for _ in 0..6 {
            let response = client
                .post(&url)
                .header("content-type", "application/json")
                .body("{}")
                .send()
                .await
                .unwrap();
            statuses.push(response.status().as_u16());
        }
        server.abort();
        // Never a 500 for lack of a key; the sixth request from this peer is limited.
        assert_eq!(
            statuses,
            vec![
                400,
                400,
                400,
                400,
                400,
                StatusCode::TOO_MANY_REQUESTS.as_u16()
            ]
        );
    }

    #[test]
    fn janitor_prunes_live_limiters_and_forgets_dropped_ones() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let pruned = Arc::new(AtomicUsize::new(0));
        let limiter = Arc::new(());
        let mut janitor = LimiterJanitor::default();
        let counter = pruned.clone();
        janitor.register(&limiter, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(janitor.run_once(), 1);
        assert_eq!(pruned.load(Ordering::SeqCst), 1);
        drop(limiter);
        assert_eq!(janitor.run_once(), 0);
        assert_eq!(pruned.load(Ordering::SeqCst), 1);
    }
}
