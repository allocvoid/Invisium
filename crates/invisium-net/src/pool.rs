//! Idle keep-alive connection pool keyed by (scheme, host, port, proxy + credentials, isolation
//! key), with a generation counter that retires every connection made before a route or
//! identity change, including the ones that were in flight when it happened.

use crate::connect::Conn;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PER_KEY: usize = 6;
const MAX_TOTAL: usize = 64;

/// Proxy credentials as part of a pool key: compared and hashed by value, never printed.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct ProxyAuth(Option<(String, String)>);

impl ProxyAuth {
    pub fn new(auth: Option<&(String, String)>) -> ProxyAuth {
        ProxyAuth(auth.cloned())
    }
}

impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "ProxyAuth(<redacted>)" } else { "ProxyAuth(none)" })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Identifies the proxy route (never shared between Direct / proxies / Tor).
    pub proxy: String,
    /// The proxy's credentials: SOCKS credentials select the stream isolation on a Tor SOCKS
    /// port, so connections made with different ones are never interchangeable.
    pub auth: ProxyAuth,
    /// Tor isolation key (top-level site), empty otherwise.
    pub isolation: String,
}

type Idle = HashMap<PoolKey, Vec<(Conn, Instant)>>;

#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<Idle>,
    /// Only changed and compared while `idle` is locked, so a `put` can never slip a
    /// connection in behind a concurrent `reset`.
    generation: AtomicU64,
}

impl Pool {
    fn lock(&self) -> std::sync::MutexGuard<'_, Idle> {
        self.idle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The current generation. A request reads it together with its configuration and stamps
    /// it on every connection it dials.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Most recently used idle connection for `key` that has not timed out. A request that
    /// started under an older generation gets nothing, and only connections of the current
    /// generation are ever handed out.
    pub fn take(&self, key: &PoolKey, generation: u64) -> Option<Conn> {
        let mut map = self.lock();
        if generation != self.generation() {
            return None;
        }
        let list = map.get_mut(key)?;
        let now = Instant::now();
        list.retain(|(c, t)| c.generation == generation && now.duration_since(*t) < IDLE_TIMEOUT);
        let c = list.pop().map(|(c, _)| c);
        if list.is_empty() {
            map.remove(key);
        }
        c
    }

    /// Keeps `conn` for reuse, unless it was dialled under an older generation: such a
    /// connection is closed instead.
    pub fn put(&self, key: PoolKey, conn: Conn) {
        let mut map = self.lock();
        if conn.generation != self.generation() {
            return;
        }
        let now = Instant::now();
        map.retain(|_, l| {
            l.retain(|(_, t)| now.duration_since(*t) < IDLE_TIMEOUT);
            !l.is_empty()
        });
        if map.values().map(Vec::len).sum::<usize>() >= MAX_TOTAL {
            return;
        }
        let list = map.entry(key).or_default();
        if list.len() >= MAX_PER_KEY {
            list.remove(0);
        }
        list.push((conn, now));
    }

    /// Closes every idle connection and starts a new generation, so that connections still in
    /// flight are closed when their exchange completes instead of coming back to the pool.
    pub fn reset(&self) {
        let mut map = self.lock();
        self.generation.fetch_add(1, Ordering::SeqCst);
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::{pool_key, tls_config, Dialer};
    use crate::types::{NetConfig, ProxyConfig};
    use crate::url::Url;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    /// Dials real connections to a local listener, so the pool has something to hold.
    struct Fixture {
        rt: tokio::runtime::Runtime,
        _listener: std::net::TcpListener,
        url: Url,
        open: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn new() -> Fixture {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            Fixture {
                rt: tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
                _listener: listener,
                url: Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
                open: Arc::default(),
            }
        }

        fn dial(&self, generation: u64) -> Conn {
            let cfg = NetConfig { proxy: ProxyConfig::Direct, ..NetConfig::default() };
            let dialer = Dialer {
                cfg: &cfg,
                tls: &tls_config(),
                open: &self.open,
                resolver: &Default::default(),
                tor: &Default::default(),
                isolation: "",
                generation,
                guard_local: false,
                host_map: None,
            };
            self.rt.block_on(dialer.connect(&self.url)).unwrap()
        }

        /// Connections that are alive (pooled or held by the test).
        fn open(&self) -> usize {
            self.open.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn connections_of_an_older_generation_are_neither_pooled_nor_handed_out() {
        let fx = Fixture::new();
        let pool = Pool::default();
        let key = pool_key(&fx.url, &NetConfig::default(), "");
        let g0 = pool.generation();

        pool.put(key.clone(), fx.dial(g0));
        assert!(pool.take(&key, g0).is_some(), "same generation: pooled and handed out");
        assert_eq!(fx.open(), 0);

        // One connection rests in the pool, one is out on a request, when the generation ends.
        pool.put(key.clone(), fx.dial(g0));
        let in_flight = fx.dial(g0);
        assert_eq!(fx.open(), 2);
        pool.reset();
        let g1 = pool.generation();
        assert_ne!(g0, g1);
        assert_eq!(fx.open(), 1, "reset closes the idle connection");
        pool.put(key.clone(), in_flight);
        assert_eq!(fx.open(), 0, "the in-flight connection is closed instead of pooled");
        assert!(pool.take(&key, g1).is_none());

        // A request that started before the reset is not served; a current one is.
        pool.put(key.clone(), fx.dial(g1));
        assert!(pool.take(&key, g0).is_none());
        assert!(pool.take(&key, g1).is_some());
    }

    #[test]
    fn keys_that_differ_only_in_proxy_credentials_never_share_a_connection() {
        let fx = Fixture::new();
        let pool = Pool::default();
        let g = pool.generation();
        for socks in [true, false] {
            let key = |auth: Option<(&str, &str)>| {
                let auth = auth.map(|(u, p)| (u.to_string(), p.to_string()));
                let (host, port) = ("127.0.0.1".to_string(), 1080);
                let proxy = if socks {
                    ProxyConfig::Socks5 { host, port, auth }
                } else {
                    ProxyConfig::Http { host, port, auth }
                };
                pool_key(&fx.url, &NetConfig { proxy, ..NetConfig::default() }, "")
            };
            let alice = key(Some(("alice-user", "s3cret-pass")));
            assert_eq!(alice, key(Some(("alice-user", "s3cret-pass"))));
            let others = [
                key(Some(("bob-user", "s3cret-pass"))),
                key(Some(("alice-user", "other-pass"))),
                key(Some(("alice-users3cret-pass", ""))),
                key(None),
            ];
            pool.put(alice.clone(), fx.dial(g));
            for other in &others {
                assert_ne!(&alice, other);
                assert!(pool.take(other, g).is_none(), "{other:?} got {alice:?}'s connection");
            }
            assert!(pool.take(&alice, g).is_some());
            let shown = format!("{alice:?}");
            assert!(!shown.contains("alice-user") && !shown.contains("s3cret-pass"), "{shown}");
        }
    }
}
