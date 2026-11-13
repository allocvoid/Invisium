//! `NetService`: owns the tokio runtime, cookie jar, cache, pool, resolver and Tor client, and
//! drives each request through the privacy pipeline hop by hop.

use crate::cache::{freshness, Cache, Cached};
use crate::connect::{pool_key, tls_config, Conn, Dialer};
use crate::cookies::{unix_now, CookieJar, Ctx};
use crate::cors;
use crate::decode::decode_body;
use crate::doh::Resolver;
use crate::http::{
    framing as body_framing, keep_alive, parse_content_type, read_final_head, read_response, BodyStream, Framing,
    Head, RawResponse,
};
use crate::policy::{self, Outgoing};
use crate::pool::{Pool, PoolKey};
use crate::privnet;
use crate::psl::{registrable_domain, same_site};
use crate::tor::Tor;
use crate::types::*;
use crate::url::{parse_data_url, Url};
use crate::ws::WsHandle;
use crate::{download, filename, framing};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

const MAX_REDIRECTS: u8 = 10;

pub(crate) struct Inner {
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
    /// Site-data epoch: `clear_site_data` bumps it, and a response that started under an older
    /// epoch stores no cookies or cache entries.
    epoch: AtomicU64,
    /// Tells every open WebSocket to close (1001) when site data is cleared.
    pub(crate) ws_close: tokio::sync::watch::Sender<u64>,
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
    pub(crate) fn snapshot(&self) -> (NetConfig, u64) {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        (cfg.clone(), self.pool.generation())
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }


    /// Stores `Set-Cookie` values unless site data was cleared since `epoch`.
    pub(crate) fn store_cookies<'a>(&self, ctx: &Ctx<'_>, values: impl Iterator<Item = &'a str>, epoch: u64) {
        let mut jar = lock(&self.jar);
        if self.epoch() == epoch {
            for v in values {
                jar.store(ctx, v);
            }
        }
    }

    pub(crate) fn cookie_header(&self, ctx: &Ctx<'_>) -> Option<String> {
        lock(&self.jar).header_for(ctx)
    }

    pub(crate) fn dialer<'a>(&'a self, cfg: &'a NetConfig, isolation: &'a str, generation: u64, guard_local: bool, _host: &str) -> Dialer<'a> {
        Dialer {
            cfg,
            tls: &self.tls,
            open: &self.open,
            resolver: &self.resolver,
            tor: &self.tor,
            isolation,
            generation,
            guard_local,
            #[cfg(test)]
            host_map: lock(&self.host_map).get(_host).cloned(),
        }
    }

    pub(crate) fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
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
                epoch: AtomicU64::new(0),
                ws_close: tokio::sync::watch::channel(0).0,
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
        self.start(req, None, done)
    }

    /// `fetch`, with the choice of where the body goes made once the head is known (see the
    /// NET contract). With `BodyTarget::File`, `done` gets an empty `Response.body`.
    pub fn fetch_with(
        &self,
        req: Request,
        on_head: impl FnOnce(&ResponseHead) -> BodyTarget + Send + 'static,
        done: impl FnOnce(Result<Response, NetError>) + Send + 'static,
    ) -> RequestHandle {
        self.start(req, Some(Box::new(on_head)), done)
    }

    fn start(
        &self,
        req: Request,
        on_head: Option<OnHead>,
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
            // Cancelling drops `run`, and with it any partial download file (see `download`).
            let res = tokio::select! {
                r = run(&inner, req, on_head) => r,
                _ = cancelled => Err(NetError::Cancelled),
            };
            if matches!(res, Err(NetError::Blocked(_))) {
                inner.blocked.fetch_add(1, Ordering::Relaxed);
            }
            done(res);
        });
        RequestHandle { cancel: Mutex::new(Some(tx)) }
    }

    /// Opens a WebSocket (RFC 6455) for a page script; see `ws`.
    pub fn websocket(&self, req: WsRequest, on_event: impl FnMut(WsEvent) + Send + 'static) -> WsHandle {
        self.inner.requests.fetch_add(1, Ordering::Relaxed);
        crate::ws::open(&self.handle, self.inner.clone(), req, Box::new(on_event))
    }

    pub fn clear_site_data(&self) {
        // The epoch moves first, so a response in flight cannot store into the cleared state.
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner.jar).clear();
        lock(&self.inner.cache).clear();
        self.inner.pool.reset();
        self.inner.ws_close.send_modify(|n| *n += 1);
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
        let route = crate::connect::TestRoute { addrs: vec![(ip, Some(ip))] };
        lock(&self.inner.host_map).insert(host.to_string(), route);
    }

    /// Test resolver seam: `host` resolves to a public address (TEST-NET-3), whose stand-in is
    /// `listener`, and then to 127.0.0.1, which is never dialled (the socket lands on the first).
    #[cfg(test)]
    pub(crate) fn map_public_and_local_host(&self, host: &str, listener: std::net::IpAddr) {
        let public = std::net::IpAddr::from([203, 0, 113, 10]);
        let local = std::net::IpAddr::from([127, 0, 0, 1]);
        let route = crate::connect::TestRoute { addrs: vec![(public, Some(listener)), (local, None)] };
        lock(&self.inner.host_map).insert(host.to_string(), route);
    }

    /// Test resolver seam: `host` resolves to a public address (TEST-NET-3), so every
    /// private-network check sees a public server, while the socket goes to `listener`.
    #[cfg(test)]
    pub(crate) fn map_public_host(&self, host: &str, listener: std::net::IpAddr) {
        let resolved = std::net::IpAddr::from([203, 0, 113, 10]);
        let route = crate::connect::TestRoute { addrs: vec![(resolved, Some(listener))] };
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
    let via = Route::Local;
    Ok(Response { url, status: 200, headers, body, mime, charset, from_cache: false, redirects, via, served_local: false })
}

fn response(url: Url, status: u16, headers: Headers, body: Vec<u8>, redirects: u8, via: Route, served_local: bool) -> Response {
    let (mime, charset) = headers.get("content-type").map(parse_content_type).unwrap_or_default();
    let from_cache = via == Route::Cache;
    Response { url, status, headers, body, mime, charset, from_cache, redirects, via, served_local }
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

pub(crate) type OnHead = Box<dyn FnOnce(&ResponseHead) -> BodyTarget + Send>;

/// A response body still on its connection (`fetch_with`).
pub(crate) struct OpenBody {
    pub conn: Conn,
    key: PoolKey,
    pub stream: BodyStream,
    keep_alive: bool,
    /// `conn.reader.received` already added to the statistics.
    counted: u64,
}

impl OpenBody {
    /// Accounts the bytes read and, if the body was read to its end, returns the connection
    /// to the pool (which refuses it if a route or identity change happened meanwhile).
    pub(crate) fn finish(self, inner: &Inner, complete: bool) {
        inner.add_bytes(self.conn.reader.received - self.counted);
        if complete && self.keep_alive && self.conn.reader.drained() {
            inner.pool.put(self.key, self.conn);
        }
    }

    /// Reads the whole body into memory under the in-memory cap.
    async fn collect(mut self, inner: &Inner, max: usize) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        let res = async {
            while let Some(piece) = self.stream.next(&mut self.conn.reader).await? {
                if out.len() + piece.len() > max {
                    return Err(NetError::TooLarge);
                }
                out.extend_from_slice(piece);
            }
            Ok(())
        }
        .await;
        self.finish(inner, res.is_ok());
        res.map(|_| out)
    }
}

/// A response whose head was read; its body is either read already or still on the wire.
enum Got {
    Full(RawResponse),
    Open(Head, Box<OpenBody>),
}

impl Got {
    fn status(&self) -> u16 {
        match self {
            Got::Full(r) => r.status,
            Got::Open(h, _) => h.status,
        }
    }
    fn headers(&self) -> &Headers {
        match self {
            Got::Full(r) => &r.headers,
            Got::Open(h, _) => &h.headers,
        }
    }
}

/// One answered request.
struct Answer {
    got: Got,
    route: Route,
    /// The answer came straight from a local address (see `Conn::local_peer`).
    local_peer: bool,
}

impl Wire<'_> {
    /// Sends one request. With `open`, only the head is read and the body stays on the wire.
    async fn send(&self, head: &[u8], head_req: bool, open: bool) -> Result<Answer, NetError> {
        match exchange(self, head, head_req, open).await {
            Err(NetError::Tls(_) | NetError::Connect(_)) if self.upgraded => {
                Err(NetError::Blocked(BlockReason::HttpsOnly))
            }
            other => other,
        }
    }
}

async fn run(inner: &Inner, req: Request, mut on_head: Option<OnHead>) -> Result<Response, NetError> {
    let (cfg, generation) = inner.snapshot();
    let epoch = inner.epoch();
    let top = req.top_level.as_ref();
    // The document that asks: a frame's own URL when set, else the tab's document. Third-party
    // decisions, partitioning and the referrer policy stay with `top`.
    let page = req.initiator.as_ref().or(top);
    let script = req.mode != FetchMode::Subresource;
    if script && top.is_none() {
        return Err(NetError::Blocked(BlockReason::Cors));
    }
    let guard_local = privnet::guards_for(top, req.top_local, req.initiator.as_ref(), req.initiator_local);
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
            let resp = data_response(url, redirects, cfg.max_body_bytes)?;
            return deliver_whole(resp, on_head);
        }
        // Rules 1-3: refuses a hop the script may not make before anything is sent.
        let hop = cors::plan_hop(page, &url, req.mode, req.credentials, chain)?;
        let third = policy::is_third_party(top, &url);
        let partition = registrable_domain(top.unwrap_or(&url).host()).to_string();
        let cache_key = url.without_fragment().to_string();
        // Script requests are never cached: no response crosses (origin, mode, credentials).
        let cacheable = !script && method == Method::Get && cfg.cache_bytes > 0;
        if cacheable {
            let hit = lock(&inner.cache).get(&partition, &cache_key, Instant::now());
            if let Some(hit) = hit {
                if req.kind == ResourceKind::Frame && !framing::allowed(&hit.headers, &url, top) {
                    return Err(NetError::Blocked(BlockReason::Framing));
                }
                let resp = response(url, hit.status, hit.headers, hit.body, redirects, Route::Cache, hit.served_local);
                return deliver_whole(resp, on_head);
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
                let pre = wire.send(&head, false, false).await?.got;
                if !cors::preflight_passes(pre.status(), pre.headers(), origin, hop.include, method, &unsafe_names) {
                    return Err(NetError::Blocked(BlockReason::Cors));
                }
            }
        }
        let cookie = if cookies_ok { inner.cookie_header(&ctx) } else { None };
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
        let Answer { got, route, local_peer } = wire.send(&head, method == Method::Head, on_head.is_some()).await?;
        // The CORS check covers every cross-origin response, redirects included, and runs
        // before anything from the response is used or stored.
        if let Some(origin) = hop.origin.as_deref() {
            if !cors::cors_check(got.headers(), origin, hop.include) {
                return Err(NetError::Blocked(BlockReason::Cors));
            }
        }
        if cookies_ok {
            inner.store_cookies(&ctx, got.headers().get_all("set-cookie"), epoch);
        }
        let status = got.status();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(loc) = got.headers().get("location").map(str::to_string) {
                // A redirect's body is never used: an open one is dropped with its connection.
                drop(got);
                if redirects >= MAX_REDIRECTS {
                    return Err(NetError::TooManyRedirects);
                }
                let next = url.join(&loc).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
                redirects += 1;
                let to_get = (status == 303 && method != Method::Head)
                    || (matches!(status, 301 | 302) && method == Method::Post);
                if to_get {
                    method = Method::Get;
                    body = None;
                    extra.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
                }
                if let Some(page) = page.filter(|_| script) {
                    chain.redirect(page, &url, &next);
                    if !cors::same_origin(&url, &next) {
                        extra.retain(|(n, _)| !n.eq_ignore_ascii_case("authorization"));
                    }
                }
                url = next;
                continue;
            }
        }
        // Framing rules: checked on the final response before any of it reaches the caller.
        if req.kind == ResourceKind::Frame && !framing::allowed(got.headers(), &url, top) {
            return Err(NetError::Blocked(BlockReason::Framing));
        }
        // `local_peer` is the address actually connected to, never one the name merely
        // resolves to; proxy and Tor connections are never local.
        let served_local = route == Route::Direct && local_peer;
        let (headers, raw_body, open) = match got {
            Got::Full(raw) => (raw.headers, raw.body, None),
            Got::Open(head, open) => (head.headers, Vec::new(), Some(*open)),
        };
        let mut visible = headers.clone();
        if script {
            cors::filter_response_headers(&mut visible, hop.cross(), hop.include);
        }
        let raw_body = match (open, on_head.take()) {
            (Some(open), Some(on_head)) => match on_head(&response_head(&url, status, &visible)) {
                BodyTarget::Memory => open.collect(inner, cfg.max_body_bytes).await?,
                BodyTarget::Discard => return Err(NetError::Cancelled),
                BodyTarget::File { path, progress } => {
                    // Written as it arrives, never cached, never held whole in memory.
                    download::to_file(inner, open, &headers, &path, progress, cfg.max_body_bytes).await?;
                    return Ok(response(url, status, visible, Vec::new(), redirects, route, served_local));
                }
            },
            _ => raw_body,
        };
        let body = decode_body(raw_body, &headers, cfg.max_body_bytes)?;
        // Nothing that came from a local address is cached, so the cache can never hand local
        // content to a request that private-network protection would have refused.
        if cacheable && !local_peer {
            if let Some(ttl) = freshness(status, &headers) {
                let entry = Cached { status, headers, body: body.clone(), served_local };
                let mut cache = lock(&inner.cache);
                if inner.epoch() == epoch {
                    cache.put(&partition, &cache_key, entry, ttl, cfg.cache_bytes, Instant::now());
                }
            }
        }
        return Ok(response(url, status, visible, body, redirects, route, served_local));
    }
}

/// The head handed to `on_head`, built from the headers the caller may see.
fn response_head(url: &Url, status: u16, visible: &Headers) -> ResponseHead {
    let (mime, charset) = visible.get("content-type").map(parse_content_type).unwrap_or_default();
    ResponseHead {
        url: url.clone(),
        status,
        headers: visible.clone(),
        mime,
        charset,
        length: visible.get("content-length").and_then(|v| v.trim().parse().ok()),
        attachment: filename::is_attachment(visible),
        filename: filename::suggested(visible, url),
    }
}

/// `fetch_with` for a response that is already in memory (data: URL or cache hit).
fn deliver_whole(resp: Response, on_head: Option<OnHead>) -> Result<Response, NetError> {
    let Some(on_head) = on_head else { return Ok(resp) };
    match on_head(&response_head(&resp.url, resp.status, &resp.headers)) {
        BodyTarget::Memory => Ok(resp),
        BodyTarget::Discard => Err(NetError::Cancelled),
        BodyTarget::File { path, progress } => {
            download::write_whole(&path, &resp.body, progress)?;
            Ok(Response { body: Vec::new(), ..resp })
        }
    }
}

enum Transacted {
    Full(RawResponse),
    Open(Head, Framing),
}

async fn transact(c: &mut Conn, head: &[u8], head_req: bool, max: usize, open: bool) -> Result<Transacted, NetError> {
    let io = &mut c.reader.io;
    io.write_all(head).await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    io.flush().await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    if open {
        let h = read_final_head(&mut c.reader, false).await?;
        let f = body_framing(&h, head_req)?;
        return Ok(Transacted::Open(h, f));
    }
    Ok(Transacted::Full(read_response(&mut c.reader, head_req, max).await?))
}

/// Sends one request on a pooled keep-alive connection if one is idle (retrying once on a
/// fresh connection if the pooled one turns out to be stale), else on a new connection.
/// `generation` is the pool generation the request started under.
async fn exchange(w: &Wire<'_>, head: &[u8], head_req: bool, open: bool) -> Result<Answer, NetError> {
    let Wire { inner, cfg, generation, url, isolation, guard_local, .. } = *w;
    let key: PoolKey = pool_key(url, cfg, isolation);
    if let Some(mut c) = inner.pool.take(&key, generation) {
        if guard_local && c.local_peer {
            // Dialled for a navigation or a local page: a public page must not ride on it.
            inner.pool.put(key, c);
            return Err(NetError::Blocked(BlockReason::PrivateNetwork));
        }
        let before = c.reader.received;
        match transact(&mut c, head, head_req, cfg.max_body_bytes, open).await {
            Ok(t) => return Ok(settle(inner, key, c, t, before)),
            Err(e) if c.reader.received > before => {
                inner.add_bytes(c.reader.received - before);
                return Err(e);
            }
            Err(_) => {}
        }
    }
    let mut c = inner.dialer(cfg, isolation, generation, guard_local, url.host()).connect(url).await?;
    match transact(&mut c, head, head_req, cfg.max_body_bytes, open).await {
        Ok(t) => Ok(settle(inner, key, c, t, 0)),
        Err(e) => {
            inner.add_bytes(c.reader.received);
            Err(e)
        }
    }
}

/// Returns a reusable connection to the pool (the pool closes it instead if a route or
/// identity change happened since the request that dialled it started), or keeps it with the
/// open body. `before` is what the connection had received before this request.
fn settle(inner: &Inner, key: PoolKey, c: Conn, t: Transacted, before: u64) -> Answer {
    let (route, local_peer) = (c.route, c.local_peer);
    let got = match t {
        Transacted::Full(raw) => {
            inner.add_bytes(c.reader.received - before);
            if raw.reusable {
                inner.pool.put(key, c);
            }
            Got::Full(raw)
        }
        Transacted::Open(head, f) => {
            let keep_alive = keep_alive(&head, f);
            let open = OpenBody { stream: BodyStream::new(f), keep_alive, counted: before, conn: c, key };
            Got::Open(head, Box::new(open))
        }
    };
    Answer { got, route, local_peer }
}
