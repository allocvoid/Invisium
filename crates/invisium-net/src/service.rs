//! `NetService`: owns the tokio runtime, cookie jar, cache, pool, resolver and Tor client, and
//! drives each request through the privacy pipeline hop by hop.

use crate::cache::{freshness, Cache, Cached};
use crate::connect::{pool_key, tls_config, Conn, Dialer};
use crate::cookies::{unix_now, CookieJar, Ctx};
use crate::cors;
use crate::decode::decode_body;
use crate::doh::Resolver;
use crate::http::{parse_content_type, read_response, RawResponse};
use crate::policy::{self, Outgoing};
use crate::pool::{Pool, PoolKey};
use crate::privnet;
use crate::psl::{registrable_domain, same_site};
use crate::tor::Tor;
use crate::types::*;
use crate::url::{parse_data_url, Url};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

const MAX_REDIRECTS: u8 = 10;

struct Inner {
    config: RwLock<NetConfig>,
    jar: Mutex<CookieJar>,
    cache: Mutex<Cache>,
    pool: Pool,
    resolver: Resolver,
    tor: Tor,
    tls: Arc<rustls::ClientConfig>,
    requests: AtomicU64,
    blocked: AtomicU64,
    bytes: AtomicU64,
    open: Arc<AtomicUsize>,
    #[cfg(test)]
    host_map: Mutex<std::collections::HashMap<String, crate::connect::TestRoute>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn config(&self) -> NetConfig {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The configuration for a new request and the pool generation it belongs to, read
    /// atomically: `set_config` starts a new generation while holding the write lock.
    fn snapshot(&self) -> (NetConfig, u64) {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        (cfg.clone(), self.pool.generation())
    }
}

pub struct NetService {
    inner: Arc<Inner>,
    handle: tokio::runtime::Handle,
    rt: Option<tokio::runtime::Runtime>,
}

pub struct RequestHandle {
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl RequestHandle {
    /// Aborts the request; `done` then receives `Err(Cancelled)` unless it already ran.
    pub fn cancel(&self) {
        if let Some(tx) = lock(&self.cancel).take() {
            let _ = tx.send(());
        }
    }
}

impl NetService {
    pub fn new(config: NetConfig) -> NetService {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("invisium-net")
            .enable_all()
            .build()
            .expect("failed to start the network runtime");
        NetService {
            inner: Arc::new(Inner {
                config: RwLock::new(config),
                jar: Mutex::new(CookieJar::default()),
                cache: Mutex::new(Cache::default()),
                pool: Pool::default(),
                resolver: Resolver::default(),
                tor: Tor::default(),
                tls: tls_config(),
                requests: AtomicU64::new(0),
                blocked: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
                open: Arc::new(AtomicUsize::new(0)),
                #[cfg(test)]
                host_map: Mutex::new(std::collections::HashMap::new()),
            }),
            handle: rt.handle().clone(),
            rt: Some(rt),
        }
    }

    pub fn config(&self) -> NetConfig {
        self.inner.config()
    }

    /// Takes effect for new requests; drops pooled connections when proxy/DNS settings change,
    /// and connections in flight at that moment are closed when their exchange completes.
    pub fn set_config(&self, config: NetConfig) {
        let mut cur = self.inner.config.write().unwrap_or_else(|e| e.into_inner());
        if cur.proxy != config.proxy || cur.dns != config.dns {
            self.inner.pool.reset();
        }
        if cur.dns != config.dns {
            self.inner.resolver.clear();
        }
        lock(&self.inner.cache).trim(config.cache_bytes);
        *cur = config;
    }

    pub fn fetch(
        &self,
        req: Request,
        done: impl FnOnce(Result<Response, NetError>) + Send + 'static,
    ) -> RequestHandle {
        let (tx, rx) = oneshot::channel::<()>();
        let inner = self.inner.clone();
        inner.requests.fetch_add(1, Ordering::Relaxed);
        self.handle.spawn(async move {
            let cancelled = async move {
                if rx.await.is_err() {
                    // Handle dropped without cancelling: keep running.
                    std::future::pending::<()>().await;
                }
            };
            let res = tokio::select! {
                r = run(&inner, req) => r,
                _ = cancelled => Err(NetError::Cancelled),
            };
            if matches!(res, Err(NetError::Blocked(_))) {
                inner.blocked.fetch_add(1, Ordering::Relaxed);
            }
            done(res);
        });
        RequestHandle { cancel: Mutex::new(Some(tx)) }
    }

    pub fn clear_site_data(&self) {
        lock(&self.inner.jar).clear();
        lock(&self.inner.cache).clear();
        self.inner.pool.reset();
    }

    pub fn new_identity(&self) {
        // Rotate the Tor circuits before the pool starts its new generation: a connection
        // stamped with the new generation must never have been dialled on an old circuit.
        self.inner.tor.new_identity();
        self.inner.resolver.clear();
        self.clear_site_data();
    }

    pub fn tor_status(&self) -> TorStatus {
        self.inner.tor.status()
    }

    pub fn stats(&self) -> NetStats {
        let i = &self.inner;
        NetStats {
            requests: i.requests.load(Ordering::Relaxed),
            blocked: i.blocked.load(Ordering::Relaxed),
            bytes_received: i.bytes.load(Ordering::Relaxed),
            cache_bytes: lock(&i.cache).bytes(),
            cookies: lock(&i.jar).count(),
            open_connections: i.open.load(Ordering::Relaxed),
        }
    }

    /// What `document.cookie` returns for a document at `url` whose tab shows `top_level`.
    /// Never includes `HttpOnly` cookies; obeys the cookie policy and partitioning.
    pub fn script_cookies(&self, url: &Url, top_level: &Url) -> String {
        let cfg = self.inner.config();
        let Some(access) = ScriptCookieAccess::new(&cfg, url, top_level) else { return String::new() };
        lock(&self.inner.jar).script_string(&access.ctx(url))
    }

    /// `document.cookie = "..."`: stored like a `Set-Cookie` header from `url`, except that
    /// `HttpOnly` cookies cannot be created or overwritten from script.
    pub fn set_script_cookie(&self, url: &Url, top_level: &Url, cookie: &str) {
        let cfg = self.inner.config();
        let Some(access) = ScriptCookieAccess::new(&cfg, url, top_level) else { return };
        lock(&self.inner.jar).store_from_script(&access.ctx(url), cookie);
    }

    /// Test resolver seam: `host` resolves to `ip`, and that is where the socket goes.
    #[cfg(test)]
    pub(crate) fn map_host(&self, host: &str, ip: std::net::IpAddr) {
        let route = crate::connect::TestRoute { resolved: ip, dial: ip };
        lock(&self.inner.host_map).insert(host.to_string(), route);
    }

    /// Test resolver seam: `host` resolves to a public address (TEST-NET-3), so every
    /// private-network check sees a public server, while the socket goes to `listener`.
    #[cfg(test)]
    pub(crate) fn map_public_host(&self, host: &str, listener: std::net::IpAddr) {
        let resolved = std::net::IpAddr::from([203, 0, 113, 10]);
        let route = crate::connect::TestRoute { resolved, dial: listener };
        lock(&self.inner.host_map).insert(host.to_string(), route);
    }
}

/// Whether, and in which partition, a document's script may touch cookies.
struct ScriptCookieAccess {
    partition: String,
    cross_site: bool,
}

impl ScriptCookieAccess {
    fn new(cfg: &NetConfig, url: &Url, top_level: &Url) -> Option<ScriptCookieAccess> {
        let cross_site = policy::is_third_party(Some(top_level), url);
        (url.is_http() && policy::cookies_allowed(cfg, cross_site)).then(|| ScriptCookieAccess {
            partition: registrable_domain(top_level.host()).to_string(),
            cross_site,
        })
    }

    fn ctx<'a>(&'a self, url: &'a Url) -> Ctx<'a> {
        Ctx {
            partition: &self.partition,
            url,
            cross_site: self.cross_site,
            top_level_get: false,
            now: unix_now(),
        }
    }
}

impl Drop for NetService {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

fn data_response(url: Url, redirects: u8, max: usize) -> Result<Response, NetError> {
    let (mime_full, body) = parse_data_url(url.without_fragment())
        .ok_or_else(|| NetError::InvalidUrl("malformed data: URL".into()))?;
    if body.len() > max {
        return Err(NetError::TooLarge);
    }
    let (mime, charset) = parse_content_type(&mime_full);
    let mut headers = Headers::new();
    headers.push("Content-Type", mime_full);
    Ok(Response { url, status: 200, headers, body, mime, charset, from_cache: false, redirects, via: Route::Local })
}

fn response(url: Url, status: u16, headers: Headers, body: Vec<u8>, redirects: u8, via: Route) -> Response {
    let (mime, charset) = headers.get("content-type").map(parse_content_type).unwrap_or_default();
    Response { url, status, headers, body, mime, charset, from_cache: via == Route::Cache, redirects, via }
}

/// Everything needed to put one hop's requests (preflight and actual) on the wire.
struct Wire<'a> {
    inner: &'a Inner,
    cfg: &'a NetConfig,
    generation: u64,
    url: &'a Url,
    isolation: &'a str,
    /// Private-network protection applies to this request (public page, not a navigation).
    guard_local: bool,
    /// The scheme was upgraded by HTTPS-only: a failed connection is never downgraded.
    upgraded: bool,
}

/// One answered request.
struct Answer {
    raw: RawResponse,
    route: Route,
    /// The answer came straight from a local address (see `Conn::local_peer`).
    local_peer: bool,
}

impl Wire<'_> {
    async fn send(&self, head: &[u8], head_req: bool) -> Result<Answer, NetError> {
        match exchange(self, head, head_req).await {
            Err(NetError::Tls(_) | NetError::Connect(_)) if self.upgraded => {
                Err(NetError::Blocked(BlockReason::HttpsOnly))
            }
            other => other,
        }
    }
}

async fn run(inner: &Inner, req: Request) -> Result<Response, NetError> {
    let (cfg, generation) = inner.snapshot();
    let top = req.top_level.as_ref();
    let script = req.mode != FetchMode::Subresource;
    if script && top.is_none() {
        return Err(NetError::Blocked(BlockReason::Cors));
    }
    let guard_local = privnet::guards(top);
    let mut url = req.url.clone();
    let mut method = req.method;
    let mut body = req.body.clone();
    let mut extra = if script { cors::script_headers(&req.headers) } else { req.headers.clone() };
    let mut chain = cors::Chain::default();
    let mut redirects: u8 = 0;
    loop {
        // Private-network protection comes first on every hop, in every mode.
        privnet::check_host(guard_local, url.host())?;
        let upgraded = policy::check(&cfg, &mut url, top, req.kind)?;
        if url.scheme() == "data" {
            if script && redirects > 0 {
                return Err(NetError::Blocked(BlockReason::Cors));
            }
            return data_response(url, redirects, cfg.max_body_bytes);
        }
        // Rules 1-3: refuses a hop the script may not make before anything is sent.
        let hop = cors::plan_hop(top, &url, req.mode, req.credentials, chain)?;
        let third = policy::is_third_party(top, &url);
        let partition = registrable_domain(top.unwrap_or(&url).host()).to_string();
        let cache_key = url.without_fragment().to_string();
        // Script requests are never cached: no response crosses (origin, mode, credentials).
        let cacheable = !script && method == Method::Get && cfg.cache_bytes > 0;
        if cacheable {
            if let Some(hit) = lock(&inner.cache).get(&partition, &cache_key, Instant::now()) {
                return Ok(response(url, hit.status, hit.headers, hit.body, redirects, Route::Cache));
            }
        }
        let cookies_ok = hop.cookies && policy::cookies_allowed(&cfg, third);
        let cross_site = third
            || (top.is_none() && req.referrer.as_ref().is_some_and(|r| !same_site(r, &url)));
        let ctx = Ctx {
            partition: &partition,
            url: &url,
            cross_site,
            top_level_get: top.is_none() && method == Method::Get,
            now: unix_now(),
        };
        let isolation = if cfg.tor_isolate_by_site { partition.as_str() } else { "*" };
        let wire = Wire { inner, cfg: &cfg, generation, url: &url, isolation, guard_local, upgraded };
        let referer = policy::referer(cfg.referrer, req.referrer.as_ref(), &url);
        if let Some(origin) = hop.origin.as_deref() {
            let unsafe_names = cors::unsafe_header_names(&extra);
            if cors::needs_preflight(method, &unsafe_names) {
                let asks = cors::preflight_headers(method, &unsafe_names);
                let head = policy::build_request(
                    &cfg,
                    &Outgoing {
                        url: &url,
                        method: Method::Options,
                        kind: ResourceKind::Other,
                        extra: &asks,
                        referer: referer.clone(),
                        origin: Some(origin),
                        cookie: None,
                        body: None,
                    },
                );
                let pre = wire.send(&head, false).await?.raw;
                if !cors::preflight_passes(pre.status, &pre.headers, origin, hop.include, method, &unsafe_names) {
                    return Err(NetError::Blocked(BlockReason::Cors));
                }
            }
        }
        let cookie = if cookies_ok { lock(&inner.jar).header_for(&ctx) } else { None };
        let head = policy::build_request(
            &cfg,
            &Outgoing {
                url: &url,
                method,
                kind: req.kind,
                extra: &extra,
                referer,
                origin: hop.origin.as_deref(),
                cookie,
                body: body.as_deref(),
            },
        );
        let Answer { raw, route, local_peer } = wire.send(&head, method == Method::Head).await?;
        // The CORS check covers every cross-origin response, redirects included, and runs
        // before anything from the response is used or stored.
        if let Some(origin) = hop.origin.as_deref() {
            if !cors::cors_check(&raw.headers, origin, hop.include) {
                return Err(NetError::Blocked(BlockReason::Cors));
            }
        }
        if cookies_ok {
            let mut jar = lock(&inner.jar);
            for v in raw.headers.get_all("set-cookie") {
                jar.store(&ctx, v);
            }
        }
        if matches!(raw.status, 301 | 302 | 303 | 307 | 308) {
            if let Some(loc) = raw.headers.get("location") {
                if redirects >= MAX_REDIRECTS {
                    return Err(NetError::TooManyRedirects);
                }
                let next = url.join(loc).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
                redirects += 1;
                let to_get = (raw.status == 303 && method != Method::Head)
                    || (matches!(raw.status, 301 | 302) && method == Method::Post);
                if to_get {
                    method = Method::Get;
                    body = None;
                    extra.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
                }
                if let Some(page) = top.filter(|_| script) {
                    chain.redirect(page, &url, &next);
                    if !cors::same_origin(&url, &next) {
                        extra.retain(|(n, _)| !n.eq_ignore_ascii_case("authorization"));
                    }
                }
                url = next;
                continue;
            }
        }
        let RawResponse { status, mut headers, body: raw_body, .. } = raw;
        let body = decode_body(raw_body, &headers, cfg.max_body_bytes)?;
        // Nothing that came from a local address is cached, so the cache can never hand local
        // content to a request that private-network protection would have refused.
        if cacheable && !local_peer {
            if let Some(ttl) = freshness(status, &headers) {
                let entry = Cached { status, headers: headers.clone(), body: body.clone() };
                lock(&inner.cache).put(&partition, &cache_key, entry, ttl, cfg.cache_bytes, Instant::now());
            }
        }
        if script {
            cors::filter_response_headers(&mut headers, hop.cross(), hop.include);
        }
        return Ok(response(url, status, headers, body, redirects, route));
    }
}

async fn transact(c: &mut Conn, head: &[u8], head_req: bool, max: usize) -> Result<RawResponse, NetError> {
    let io = &mut c.reader.io;
    io.write_all(head).await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    io.flush().await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    read_response(&mut c.reader, head_req, max).await
}

/// Sends one request on a pooled keep-alive connection if one is idle (retrying once on a
/// fresh connection if the pooled one turns out to be stale), else on a new connection.
/// `generation` is the pool generation the request started under.
async fn exchange(w: &Wire<'_>, head: &[u8], head_req: bool) -> Result<Answer, NetError> {
    let Wire { inner, cfg, generation, url, isolation, guard_local, .. } = *w;
    let key: PoolKey = pool_key(url, cfg, isolation);
    if let Some(mut c) = inner.pool.take(&key, generation) {
        if guard_local && c.local_peer {
            // Dialled for a navigation or a local page: a public page must not ride on it.
            inner.pool.put(key, c);
            return Err(NetError::Blocked(BlockReason::PrivateNetwork));
        }
        let before = c.reader.received;
        let res = transact(&mut c, head, head_req, cfg.max_body_bytes).await;
        let got = c.reader.received - before;
        inner.bytes.fetch_add(got, Ordering::Relaxed);
        match res {
            Ok(raw) => return Ok(finish(inner, key, c, raw)),
            Err(e) if got > 0 => return Err(e),
            Err(_) => {}
        }
    }
    let dialer = Dialer {
        cfg,
        tls: &inner.tls,
        open: &inner.open,
        resolver: &inner.resolver,
        tor: &inner.tor,
        isolation,
        generation,
        guard_local,
        #[cfg(test)]
        host_map: lock(&inner.host_map).get(url.host()).copied(),
    };
    let mut c = dialer.connect(url).await?;
    let res = transact(&mut c, head, head_req, cfg.max_body_bytes).await;
    inner.bytes.fetch_add(c.reader.received, Ordering::Relaxed);
    Ok(finish(inner, key, c, res?))
}

/// Returns a reusable connection to the pool; the pool closes it instead if a route or
/// identity change happened since the request that dialled it started.
fn finish(inner: &Inner, key: PoolKey, c: Conn, raw: RawResponse) -> Answer {
    let (route, local_peer) = (c.route, c.local_peer);
    if raw.reusable {
        inner.pool.put(key, c);
    }
    Answer { raw, route, local_peer }
}
