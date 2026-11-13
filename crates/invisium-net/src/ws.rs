//! WebSocket client (RFC 6455): the handle, and the opening handshake run through the same
//! pipeline as a script request of kind `Xhr`. The session itself lives in `wssession`.

use crate::connect::{ConnGuard, Io};
use crate::cookies::{unix_now, Ctx};
use crate::cors;
use crate::http::read_final_head;
use crate::policy;
use crate::privnet;
use crate::psl::registrable_domain;
use crate::service::Inner;
use crate::types::*;
use crate::wsproto::{accept_key, base64, random};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub(crate) enum Cmd {
    Data(u8, Vec<u8>),
    Close(Option<u16>, String),
}

/// Controls one WebSocket. Cheap to clone; every method returns at once.
#[derive(Clone)]
pub struct WsHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    buffered: Arc<AtomicU64>,
}

impl WsHandle {
    pub fn send_text(&self, text: String) {
        self.queue(crate::wsproto::OP_TEXT, text.into_bytes());
    }

    pub fn send_binary(&self, data: Vec<u8>) {
        self.queue(crate::wsproto::OP_BINARY, data);
    }

    fn queue(&self, opcode: u8, data: Vec<u8>) {
        let n = data.len() as u64;
        self.buffered.fetch_add(n, Ordering::SeqCst);
        if self.tx.send(Cmd::Data(opcode, data)).is_err() {
            self.buffered.fetch_sub(n, Ordering::SeqCst);
        }
    }

    /// Starts the closing handshake (`code` 1000 or 3000..=4999; any other code is sent as a
    /// close without a code; the reason is cut to 123 bytes).
    pub fn close(&self, code: Option<u16>, reason: &str) {
        let _ = self.tx.send(Cmd::Close(code, reason.to_string()));
    }

    /// Bytes accepted by `send_*` and not yet written to the network (`bufferedAmount`).
    pub fn buffered(&self) -> u64 {
        self.buffered.load(Ordering::SeqCst)
    }
}

/// A completed opening handshake.
pub(crate) struct Established {
    pub io: Box<dyn Io>,
    /// Bytes that arrived right after the handshake response (the first frames).
    pub rest: Vec<u8>,
    pub guard: ConnGuard,
    pub protocol: String,
}

pub(crate) fn open(
    rt: &tokio::runtime::Handle,
    inner: Arc<Inner>,
    req: WsRequest,
    on_event: Box<dyn FnMut(WsEvent) + Send>,
) -> WsHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let buffered = Arc::new(AtomicU64::new(0));
    let handle = WsHandle { tx, buffered: buffered.clone() };
    rt.spawn(crate::wssession::run(inner, req, rx, buffered, on_event));
    handle
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// The handshake request with the fixed, machine-independent header set.
fn request_head(cfg: &NetConfig, url: &crate::url::Url, origin: &str, cookie: Option<String>, key: &str, protocols: &[String]) -> Vec<u8> {
    let mut h = format!("GET {} HTTP/1.1\r\n", url.request_target());
    let mut push = |n: &str, v: &str| {
        h.push_str(n);
        h.push_str(": ");
        h.push_str(v);
        h.push_str("\r\n");
    };
    push("Host", &url.host_port());
    push("User-Agent", policy::user_agent(cfg));
    push("Accept", "*/*");
    if cfg.generic_accept_language {
        push("Accept-Language", policy::ACCEPT_LANGUAGE);
    }
    push("Accept-Encoding", policy::ACCEPT_ENCODING);
    push("Sec-WebSocket-Version", "13");
    push("Origin", origin);
    if !protocols.is_empty() {
        push("Sec-WebSocket-Protocol", &protocols.join(", "));
    }
    push("Sec-WebSocket-Key", key);
    if cfg.send_gpc {
        push("DNT", "1");
        push("Sec-GPC", "1");
    }
    push("Connection", "Upgrade");
    if let Some(c) = &cookie {
        push("Cookie", c);
    }
    push("Upgrade", "websocket");
    h.push_str("\r\n");
    h.into_bytes()
}

fn fail(msg: &str) -> NetError {
    NetError::Protocol(msg.to_string())
}

/// Runs the pipeline and the opening handshake. Any error means the socket never opened.
pub(crate) async fn handshake(inner: &Inner, req: &WsRequest) -> Result<Established, NetError> {
    let (cfg, generation) = inner.snapshot();
    let epoch = inner.epoch();
    let mut url = req.url.clone();
    match url.scheme() {
        "ws" => url.set_scheme("http"),
        "wss" => url.set_scheme("https"),
        _ => return Err(NetError::Blocked(BlockReason::Scheme)),
    }
    if url.fragment().is_some() || url.host().is_empty() {
        return Err(NetError::InvalidUrl("WebSocket URLs have no fragment".into()));
    }
    let mut seen = std::collections::HashSet::new();
    if !req.protocols.iter().all(|p| is_token(p) && seen.insert(p.as_str())) {
        return Err(fail("invalid subprotocol list"));
    }
    let top = &req.top_level;
    let guard_local = privnet::guards_for(Some(top), req.top_local, req.initiator.as_ref(), req.initiator_local);
    privnet::check_host(guard_local, url.host())?;
    // Scheme rules (HTTPS-only upgrades ws, mixed content), tracker list, third-party policy.
    let upgraded = policy::check(&cfg, &mut url, Some(top), ResourceKind::Xhr)?;
    let third = policy::is_third_party(Some(top), &url);
    let partition = registrable_domain(top.host()).to_string();
    let ctx = Ctx { partition: &partition, url: &url, cross_site: third, top_level_get: false, now: unix_now() };
    let cookies_ok = policy::cookies_allowed(&cfg, third);
    let cookie = if cookies_ok { inner.cookie_header(&ctx) } else { None };
    let origin = cors::origin_of(req.initiator.as_ref().unwrap_or(top));
    let key = base64(&random::<16>().ok_or_else(|| fail("no randomness"))?);
    let head = request_head(&cfg, &url, &origin, cookie, &key, &req.protocols);
    let isolation = if cfg.tor_isolate_by_site { partition.as_str() } else { "*" };
    let dialer = inner.dialer(&cfg, isolation, generation, guard_local, url.host());
    let mut conn = match dialer.connect(&url).await {
        Err(NetError::Tls(_) | NetError::Connect(_)) if upgraded => return Err(NetError::Blocked(BlockReason::HttpsOnly)),
        other => other?,
    };
    let io = &mut conn.reader.io;
    io.write_all(&head).await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    io.flush().await.map_err(|e| NetError::Connect(format!("write failed: {e}")))?;
    let resp = read_final_head(&mut conn.reader, true).await?;
    inner.add_bytes(conn.reader.received);
    let h = &resp.headers;
    // Redirects are not followed: anything but 101 fails.
    if resp.status != 101 {
        return Err(fail("handshake not answered with 101"));
    }
    fn single<'a>(h: &'a Headers, name: &'a str) -> Option<&'a str> {
        let mut it = h.get_all(name);
        let first = it.next();
        if it.next().is_some() { None } else { first }
    }
    let single = |name: &'static str| single(h, name);
    if !single("upgrade").is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket")) {
        return Err(fail("missing Upgrade: websocket"));
    }
    if !h.get_all("connection").flat_map(|v| v.split(',')).any(|t| t.trim().eq_ignore_ascii_case("upgrade")) {
        return Err(fail("missing Connection: Upgrade"));
    }
    if single("sec-websocket-accept").map(str::trim) != Some(accept_key(&key).as_str()) {
        return Err(fail("wrong Sec-WebSocket-Accept"));
    }
    if h.get("sec-websocket-extensions").is_some() {
        return Err(fail("extensions were not offered"));
    }
    let protocol = match h.get_all("sec-websocket-protocol").count() {
        0 => String::new(),
        1 => {
            let p = h.get("sec-websocket-protocol").unwrap_or("").trim().to_string();
            if !req.protocols.contains(&p) {
                return Err(fail("subprotocol was not offered"));
            }
            p
        }
        _ => return Err(fail("several subprotocols")),
    };
    if cookies_ok {
        inner.store_cookies(&ctx, h.get_all("set-cookie"), epoch);
    }
    let (io, rest, guard) = conn.into_stream();
    Ok(Established { io, rest, guard, protocol })
}
