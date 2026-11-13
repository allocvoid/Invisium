//! Glue between the network stack and pages: turning address-bar text into URLs, starting
//! fetches, and turning responses (or failures) into documents.

use invis_chrome::pages;
use invisium::page::{decode_text, CookieAccess, FetchResult, Page, PageSocket, ResKind, SocketEvent, UrlJoin};
use invisium_net::{BlockReason, BodyTarget, Credentials, FetchMode, Method, NetError, NetService, Request, RequestHandle, ResourceKind, Response, ResponseHead, Url, WsEvent, WsHandle, WsRequest};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::rc::Rc;
use std::sync::Arc;

/// Native stack page scripts may use. The main thread is linked with 32 MiB (see `build.rs`);
/// half of it is left for layout and painting, which recurse with the depth of the document.
pub const SCRIPT_STACK: usize = 16 << 20;

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Intrinsic size (before any downscaling).
    pub orig_w: u32,
    pub orig_h: u32,
}

pub enum NetEvent {
    Document { tab: u64, nav: u64, result: Result<Response, NetError> },
    // `page` in the events below is `Page::id` of the document that asked: the tab's own
    // or one framed in it.
    Sheet { tab: u64, nav: u64, page: u32, req: u32, result: Result<Response, NetError> },
    Script { tab: u64, nav: u64, page: u32, req: u32, result: Result<Response, NetError> },
    /// The answer to a request made by page script (`fetch`, `XMLHttpRequest`).
    Fetch { tab: u64, nav: u64, page: u32, id: u32, result: Result<Response, NetError> },
    Image { tab: u64, nav: u64, page: u32, url: String, result: Result<DecodedImage, NetError> },
    /// A web font file (`@font-face`).
    Font { tab: u64, nav: u64, page: u32, req: u32, result: Result<Response, NetError> },
    /// The document of the `<iframe>` element `node` of page `page`.
    Frame { tab: u64, nav: u64, page: u32, node: u32, result: Result<Response, NetError> },
    /// Something happened on WebSocket `id` of page `page`.
    Socket { tab: u64, nav: u64, page: u32, id: u32, event: WsEvent },
    /// The navigation `nav` of a tab turned out to be a file to keep: download `id`.
    Download { tab: u64, nav: u64, id: u64, update: DownloadUpdate },
}

/// News about a download.
pub enum DownloadUpdate {
    /// The response is being written to `path`; `total` is its size if the server said.
    Started { name: String, path: PathBuf, total: Option<u64> },
    /// Bytes written so far.
    Progress(u64),
    /// The file is complete, or the transfer failed (the partial file is gone then).
    Finished(Result<(), NetError>),
}

/// Where downloads go: the user's Downloads folder, if there is one. `INVIS_DOWNLOADS` names
/// another folder (for trying things out without touching the real one).
pub fn downloads_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("INVIS_DOWNLOADS").map(PathBuf::from) {
        return dir.is_dir().then_some(dir);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join("Downloads");
    dir.is_dir().then_some(dir)
}

/// A path in `dir` for a file called `name` that does not exist yet: `name`, else
/// `stem (1).ext`, `stem (2).ext`, ... A name handed out once is not handed out again by this
/// process, so two downloads that start together do not pick the same one before either file
/// exists. (The network stack never overwrites; a name another program takes in the meantime
/// fails the download instead.)
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    static GIVEN: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let mut given = GIVEN.lock().unwrap_or_else(|e| e.into_inner());
    let free = |p: &PathBuf| !given.contains(p) && !p.exists();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let first = dir.join(name);
    let path = if free(&first) { first } else { (1..1000).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| free(p)).unwrap_or(first) };
    given.push(path.clone());
    path
}

/// Types a tab shows itself (see `document_html`); anything else is a file to keep.
fn shown_in_tab(mime: &str) -> bool {
    mime.is_empty()
        || mime == "text/html"
        || mime == "application/xhtml+xml"
        || mime.starts_with("image/")
        || mime.starts_with("text/")
        || mime.ends_with("json")
        || mime.ends_with("xml")
        || mime.ends_with("javascript")
}

/// A page's end of a WebSocket: the network stack's handle.
struct Wire(WsHandle);

impl PageSocket for Wire {
    fn send_text(&self, text: String) {
        self.0.send_text(text);
    }
    fn send_binary(&self, data: Vec<u8>) {
        self.0.send_binary(data);
    }
    fn close(&self, code: Option<u16>, reason: &str) {
        self.0.close(code, reason);
    }
    fn buffered(&self) -> u64 {
        self.0.buffered()
    }
}

/// What a page hears about a WebSocket event of the network stack.
pub fn socket_event(event: WsEvent) -> SocketEvent {
    match event {
        WsEvent::Open { protocol } => SocketEvent::Open { protocol },
        WsEvent::Text(t) => SocketEvent::Text(t),
        WsEvent::Binary(b) => SocketEvent::Binary(b),
        WsEvent::Closed { code, reason, clean } => SocketEvent::Closed { code, reason, clean },
    }
}

/// Where network results are delivered (the UI thread's event queue, or a channel when headless).
pub type Sink = Arc<dyn Fn(NetEvent) + Send + Sync>;

pub fn url_join() -> UrlJoin {
    Rc::new(|base: &str, reference: &str| {
        if reference.starts_with("invis:") {
            return Some(reference.to_string());
        }
        match Url::parse(base) {
            Ok(b) => b.join(reference).ok().map(|u| u.as_str().to_string()),
            Err(_) => Url::parse(reference).ok().map(|u| u.as_str().to_string()),
        }
    })
}

fn has_scheme(s: &str) -> bool {
    match s.find("://") {
        Some(p) if p > 0 => s[..p].bytes().enumerate().all(|(i, c)| c.is_ascii_alphabetic() || (i > 0 && (c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.')))),
        _ => false,
    }
}

pub fn encode_query(q: &str) -> String {
    let enc = invisium_net::form_urlencode(&[("q".to_string(), q.to_string())]);
    enc.strip_prefix("q=").unwrap_or(&enc).to_string()
}

/// Turn what the user typed into a URL: an address if it looks like one, otherwise a search.
pub fn fix_url(input: &str, search_prefix: &str) -> String {
    let t = input.trim();
    if t.is_empty() || t == "about:blank" {
        return "invis://newtab".into();
    }
    if t.starts_with("invis:") || t.starts_with("data:") || has_scheme(t) {
        return t.to_string();
    }
    let host_part = t.split(['/', '?', '#']).next().unwrap_or("");
    let looks_like_host = !t.contains(' ')
        && (host_part.contains('.') || host_part.starts_with("localhost") || host_part.starts_with('['))
        && !host_part.ends_with('.');
    if looks_like_host {
        // plain-HTTP for local development hosts; everything else starts secure
        let local = host_part.starts_with("localhost") || host_part.starts_with("127.") || host_part.starts_with("[::1]");
        format!("{}://{}", if local { "http" } else { "https" }, t)
    } else {
        format!("{}{}", search_prefix, encode_query(t))
    }
}

/// May content of the page at `from` navigate to `to`? The browser's own `invis://` pages carry
/// actions (changing settings, choosing a proxy), so only those pages themselves may link to
/// them; a web page asking for one is refused. Typed addresses do not pass through here.
pub fn page_may_open(from: &str, to: &str) -> bool {
    let is_internal = |u: &str| {
        let t = u.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
        t.len() >= 6 && t[..6].eq_ignore_ascii_case("invis:")
    };
    !is_internal(to) || is_internal(from)
}

/// Value of a query parameter, form-decoded.
pub fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next()?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            let bytes = invisium_net::percent_decode(&v.replace('+', " "));
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    None
}

/// A form submission's body with its content type (`None` = `application/x-www-form-urlencoded`).
pub type FormBody = (Vec<u8>, Option<String>);

/// Fetch the document of a navigation. With `downloads` on, a response that is not something
/// a tab shows (an attachment, an archive, a PDF, ...) is written to the Downloads folder
/// instead and reported as `NetEvent::Download`; the tab then never gets a `Document`.
pub fn fetch_document(net: &NetService, sink: &Sink, tab: u64, nav: u64, url: Url, body: Option<FormBody>, referrer: Option<Url>, downloads: bool) -> RequestHandle {
    static NEXT_DOWNLOAD: AtomicU64 = AtomicU64::new(1);
    let mut req = Request::get(url, ResourceKind::Document);
    if let Some((b, content_type)) = body {
        req.method = Method::Post;
        req.headers.push(("Content-Type".to_string(), content_type.unwrap_or_else(|| "application/x-www-form-urlencoded".to_string())));
        req.body = Some(b);
    }
    req.referrer = referrer;
    // 0 until the response has become a download, then that download's id
    let became = Arc::new(AtomicU64::new(0));
    let (on_start, on_progress, on_done, became_done) = (sink.clone(), sink.clone(), sink.clone(), became.clone());
    net.fetch_with(
        req,
        move |head: &ResponseHead| {
            let keep = downloads && (200..300).contains(&head.status) && (head.attachment || !shown_in_tab(&head.mime));
            let Some(dir) = downloads_dir().filter(|_| keep) else { return BodyTarget::Memory };
            let path = unique_path(&dir, &head.filename);
            let name = path.file_name().map_or_else(|| head.filename.clone(), |n| n.to_string_lossy().into_owned());
            let id = NEXT_DOWNLOAD.fetch_add(1, Ordering::SeqCst);
            became.store(id, Ordering::SeqCst);
            on_start(NetEvent::Download { tab, nav, id, update: DownloadUpdate::Started { name, path: path.clone(), total: head.length } });
            BodyTarget::File { path, progress: Box::new(move |n| on_progress(NetEvent::Download { tab, nav, id, update: DownloadUpdate::Progress(n) })) }
        },
        move |result| match became_done.load(Ordering::SeqCst) {
            0 => on_done(NetEvent::Document { tab, nav, result }),
            id => on_done(NetEvent::Download { tab, nav, id, update: DownloadUpdate::Finished(result.map(|_| ())) }),
        },
    )
}

#[cfg(feature = "images")]
fn decode_image(resp: &Response) -> Result<DecodedImage, NetError> {
    use invisium_gfx::{draw::Rgba, images, svg};
    let is_svg = resp.mime == "image/svg+xml" || resp.body.iter().take(256).collect::<Vec<_>>().windows(4).any(|w| w == [&b'<', &b's', &b'v', &b'g']);
    if is_svg {
        let src = String::from_utf8_lossy(&resp.body);
        let (w, h) = svg::intrinsic_size(&src).unwrap_or((300.0, 150.0));
        let k = (1024.0 / w.max(h)).min(2.0).max(0.01);
        let (pw, ph) = ((w * k).round().max(1.0) as u32, (h * k).round().max(1.0) as u32);
        let rgba = svg::rasterize(&src, pw, ph, Rgba::BLACK).ok_or_else(|| NetError::Protocol("unsupported SVG".into()))?;
        return Ok(DecodedImage { width: pw, height: ph, rgba, orig_w: w.round().max(1.0) as u32, orig_h: h.round().max(1.0) as u32 });
    }
    let d = images::decode(&resp.body, 2048, 2048, 40_000_000).map_err(NetError::Protocol)?;
    Ok(DecodedImage { width: d.width, height: d.height, rgba: d.rgba, orig_w: d.orig_width, orig_h: d.orig_height })
}

#[cfg(not(feature = "images"))]
fn decode_image(_resp: &Response) -> Result<DecodedImage, NetError> {
    Err(NetError::Protocol("image support is not built in".into()))
}

/// Rasterise the page's inline `<svg>` elements at their laid-out size. Returns true if any
/// image was attached (the page then needs another `update`).
#[cfg(feature = "images")]
pub fn rasterize_inline_svgs(page: &mut Page, scale: f32, upload: &mut dyn FnMut(u32, u32, &[u8]) -> invisium::draw::ImageId) -> bool {
    let mut any = false;
    page.for_each_page(&mut |p| any |= rasterize_svgs_of(p, scale, upload));
    any
}

#[cfg(feature = "images")]
fn rasterize_svgs_of(page: &mut Page, scale: f32, upload: &mut dyn FnMut(u32, u32, &[u8]) -> invisium::draw::ImageId) -> bool {
    let mut any = false;
    for (node, w, h) in page.svg_elements() {
        let src = page.outer_html(node);
        let pw = ((w * scale).round() as u32).clamp(1, 2048);
        let ph = ((h * scale).round() as u32).clamp(1, 2048);
        match invisium_gfx::svg::rasterize(&src, pw, ph, page.color_of(node)) {
            Some(rgba) => {
                let id = upload(pw, ph, &rgba);
                page.set_node_image(node, invisium::paint::PaintImage { id, width: w, height: h });
                any = true;
            }
            None => page.skip_svg(node),
        }
    }
    any
}

#[cfg(not(feature = "images"))]
pub fn rasterize_inline_svgs(_page: &mut Page, _scale: f32, _upload: &mut dyn FnMut(u32, u32, &[u8]) -> invisium::draw::ImageId) -> bool {
    false
}

/// Start every fetch the page has asked for since the last call.
/// Upload the 2D canvases whose bitmap changed and attach the new images. Returns true if any
/// was attached (the page then needs another `update`).
pub fn upload_canvases(page: &mut Page, upload: &mut dyn FnMut(u32, u32, &[u8]) -> invisium::draw::ImageId) -> bool {
    let mut any = false;
    page.for_each_page(&mut |p| {
        for (node, w, h, px) in p.take_canvas_uploads() {
            let id = upload(w, h, &px);
            p.set_node_image(node, invisium::paint::PaintImage { id, width: w as f32, height: h as f32 });
            any = true;
        }
    });
    any
}

/// Start every fetch the page and the pages framed in it have asked for since the last call.
/// `top_local`: the tab's own document was served from a local address (`Response::served_local`
/// of its navigation), so it may reach local targets. Never set it from anything else.
pub fn fetch_subresources(net: &NetService, sink: &Sink, tab: u64, nav: u64, page: &mut Page, top_local: bool) -> Vec<RequestHandle> {
    let mut handles = Vec::new();
    let top = Url::parse(&page.url).ok().filter(|u| u.is_http());
    page.for_each_page(&mut |p| fetch_for(net, sink, tab, nav, p, top.as_ref(), top_local, &mut handles));
    handles
}

/// The fetches of one document: the tab's own, or one framed in it.
#[allow(clippy::too_many_arguments)]
fn fetch_for(net: &NetService, sink: &Sink, tab: u64, nav: u64, page: &mut Page, top: Option<&Url>, top_local: bool, handles: &mut Vec<RequestHandle>) {
    let pid = page.id();
    let doc = Url::parse(&page.url).ok().filter(|d| d.is_http());
    let framed = page.is_frame();
    let doc_local = page.served_local;
    // Who asks: the tab's document decides partitioning and what counts as third party; a
    // framed document is named as the initiator, so origin checks are made against it.
    // Internal pages (invis://) are not a web origin: their requests carry none of this.
    let stamp = |req: &mut Request| -> bool {
        let Some(d) = doc.as_ref() else { return false };
        req.referrer = Some(d.clone());
        req.top_level = Some(top.cloned().unwrap_or_else(|| d.clone()));
        req.top_local = top_local;
        if framed {
            req.initiator = Some(d.clone());
            // only what the network stack said about this very document
            req.initiator_local = doc_local;
        }
        true
    };
    for r in page.take_requests() {
        let Ok(url) = Url::parse(&r.url) else {
            page.resource_failed(r.id);
            continue;
        };
        let kind = match r.kind {
            ResKind::Stylesheet => ResourceKind::Stylesheet,
            ResKind::Image => ResourceKind::Image,
            ResKind::Script => ResourceKind::Script,
            ResKind::Font => ResourceKind::Font,
        };
        let mut req = Request::get(url, kind);
        stamp(&mut req);
        let sink = sink.clone();
        let handle = match r.kind {
            ResKind::Stylesheet => {
                let id = r.id;
                net.fetch(req, move |result| sink(NetEvent::Sheet { tab, nav, page: pid, req: id, result }))
            }
            ResKind::Script => {
                let id = r.id;
                net.fetch(req, move |result| sink(NetEvent::Script { tab, nav, page: pid, req: id, result }))
            }
            ResKind::Font => {
                let id = r.id;
                net.fetch(req, move |result| sink(NetEvent::Font { tab, nav, page: pid, req: id, result }))
            }
            ResKind::Image => {
                let u = r.url.clone();
                // decoding happens here, on the network thread, never on the UI thread
                net.fetch(req, move |result| sink(NetEvent::Image { tab, nav, page: pid, url: u, result: result.and_then(|resp| decode_image(&resp)) }))
            }
        };
        handles.push(handle);
    }
    // the documents of this page's frames
    for fr in page.take_frame_requests() {
        let Ok(url) = Url::parse(&fr.url) else {
            page.frame_failed(fr.node);
            continue;
        };
        let mut req = Request::get(url, ResourceKind::Frame);
        if !stamp(&mut req) {
            page.frame_failed(fr.node);
            continue;
        }
        if let Some((body, content_type)) = fr.body {
            req.method = Method::Post;
            req.headers.push(("Content-Type".to_string(), content_type.unwrap_or_else(|| "application/x-www-form-urlencoded".to_string())));
            req.body = Some(body);
        }
        let sink = sink.clone();
        let node = fr.node;
        handles.push(net.fetch(req, move |result| sink(NetEvent::Frame { tab, nav, page: pid, node, result })));
    }
    // requests made by the page's scripts: always sent as script requests, so the network
    // stack applies the cross-origin rules; a script request never goes out as a plain fetch
    for f in page.take_fetches() {
        let Ok(url) = Url::parse(&f.url) else {
            page.fetch_done(f.id, None);
            continue;
        };
        let mut req = Request::get(url, ResourceKind::Xhr);
        if !stamp(&mut req) {
            page.fetch_done(f.id, None);
            continue;
        }
        req.method = match f.method.as_str() {
            "POST" => Method::Post,
            "HEAD" => Method::Head,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "OPTIONS" => Method::Options,
            _ => Method::Get,
        };
        req.headers = f.headers;
        req.body = f.body;
        req.mode = if f.same_origin { FetchMode::SameOrigin } else { FetchMode::Cors };
        req.credentials = match f.credentials {
            0 => Credentials::Omit,
            2 => Credentials::Include,
            _ => Credentials::SameOrigin,
        };
        let sink = sink.clone();
        let id = f.id;
        handles.push(net.fetch(req, move |result| sink(NetEvent::Fetch { tab, nav, page: pid, id, result })));
    }
    // the WebSockets the page's scripts opened: the network stack decides whether each may
    // connect (it applies the same rules as to requests) and says so through its events
    // (refusing one runs the page's handlers, which may ask for another: a few rounds, so a
    // handler that reconnects at once is served now and one that never stops cannot hold us)
    for _ in 0..3 {
        let asked = page.take_socket_requests();
        if asked.is_empty() {
            break;
        }
        for s in asked {
            let (Ok(url), Some(d)) = (Url::parse(&s.url), doc.as_ref()) else {
                page.socket_event(s.id, SocketEvent::Closed { code: 1006, reason: String::new(), clean: false });
                continue;
            };
            let req = WsRequest {
                url,
                protocols: s.protocols,
                top_level: top.cloned().unwrap_or_else(|| d.clone()),
                initiator: framed.then(|| d.clone()),
                top_local,
                initiator_local: framed && doc_local,
            };
            let sink = sink.clone();
            let id = s.id;
            let handle = net.websocket(req, move |event| sink(NetEvent::Socket { tab, nav, page: pid, id, event }));
            page.socket_opened(id, Box::new(Wire(handle)));
        }
    }
}

/// What script gets to see of a response: nothing at all if the request failed or was refused.
pub fn fetch_result(result: Result<Response, NetError>) -> Option<FetchResult> {
    let resp = result.ok()?;
    Some(FetchResult {
        status: resp.status,
        url: resp.url.as_str().to_string(),
        headers: resp.headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        body: resp.body,
    })
}

/// `document.cookie` backed by the network stack's jar (which never shows `HttpOnly` cookies
/// to script and applies the cookie policy and partitioning).
pub fn cookie_access(net: Arc<NetService>) -> CookieAccess {
    let reader = net.clone();
    (
        Rc::new(move |doc: &str| match Url::parse(doc) {
            Ok(u) if u.is_http() => reader.script_cookies(&u, &u),
            _ => String::new(),
        }),
        Rc::new(move |doc: &str, cookie: &str| {
            if let Ok(u) = Url::parse(doc) {
                if u.is_http() {
                    net.set_script_cookie(&u, &u, cookie);
                }
            }
        }),
    )
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The document to show for a successful response.
pub fn document_html(resp: &Response) -> String {
    let mime = resp.mime.as_str();
    let sniff_html = || {
        let head = String::from_utf8_lossy(&resp.body[..resp.body.len().min(512)]).to_ascii_lowercase();
        head.contains("<html") || head.contains("<!doctype") || head.contains("<body") || head.contains("<head")
    };
    if mime == "text/html" || mime == "application/xhtml+xml" || (mime.is_empty() && sniff_html()) {
        return decode_text(&resp.body, resp.charset.as_deref());
    }
    if mime.starts_with("image/") {
        return format!(
            "<!doctype html><title>{0}</title><body style=\"margin:0;background:#0a0f1a;text-align:center\"><img src=\"{0}\" style=\"max-width:100%\"></body>",
            escape(resp.url.as_str())
        );
    }
    let textual = mime.starts_with("text/") || mime.is_empty() || mime.ends_with("json") || mime.ends_with("xml") || mime.ends_with("javascript");
    if textual {
        let text = decode_text(&resp.body[..resp.body.len().min(2 * 1024 * 1024)], resp.charset.as_deref());
        return format!(
            "<!doctype html><title>{}</title><body style=\"margin:12px;background:#0a0f1a;color:#d1d5db\"><pre style=\"white-space:pre-wrap;word-break:break-all;font-family:monospace;font-size:13px\">{}</pre></body>",
            escape(resp.url.as_str()),
            escape(&text)
        );
    }
    pages::error(
        "This file can't be shown",
        &format!("The server sent a file of type \"{}\" ({} bytes), and there is no Downloads folder to save it in.", mime, resp.body.len()),
        resp.url.as_str(),
        None,
    )
}

/// The document to show when a navigation fails.
pub fn error_html(err: &NetError, url: &str) -> String {
    let (title, detail, retry): (&str, String, bool) = match err {
        NetError::Blocked(BlockReason::Tracker) => ("Blocked: known tracker", "This address is on InvisBrowser's tracker list, so the request was never sent.".into(), false),
        NetError::Blocked(BlockReason::HttpsOnly) => (
            "No secure connection available",
            "HTTPS-only mode is on and this site did not answer over HTTPS. InvisBrowser will not fall back to an unencrypted connection on its own.".into(),
            true,
        ),
        NetError::Blocked(BlockReason::ThirdParty) => ("Blocked by your third-party policy", "Your settings do not allow this request.".into(), false),
        NetError::Blocked(BlockReason::Scheme) => ("Unsupported address", "InvisBrowser only opens http, https and data addresses.".into(), false),
        NetError::Blocked(BlockReason::MixedContent) => ("Blocked: insecure content", "A secure page tried to load this over plain HTTP.".into(), false),
        // reasons the network stack adds later (cross-origin and local-network protection)
        #[allow(unreachable_patterns)]
        NetError::Blocked(_) => ("Blocked by a protection", "InvisBrowser refused this request to protect another site's data or your local network.".into(), false),
        NetError::InvalidUrl(e) => ("That address isn't valid", e.clone(), false),
        NetError::Dns(e) => ("Site not found", format!("The name could not be resolved. {e}"), true),
        NetError::Connect(e) => ("Can't connect", format!("The server did not accept the connection. {e}"), true),
        NetError::Tls(e) => ("Secure connection failed", format!("The site's certificate or encryption could not be verified, so nothing was loaded. {e}"), true),
        NetError::Proxy(e) => ("Proxy problem", format!("The configured proxy could not complete the request. Nothing was sent directly. {e}"), true),
        NetError::Tor(e) => ("Tor is not ready", format!("The request was not sent because it could not go through Tor. {e}"), true),
        NetError::Timeout => ("The site took too long", "No answer arrived in time.".into(), true),
        NetError::TooLarge => ("Too large", "The response was bigger than InvisBrowser's size limit.".into(), false),
        NetError::TooManyRedirects => ("Redirect loop", "The site kept redirecting without arriving anywhere.".into(), true),
        NetError::Protocol(e) => ("The server's answer made no sense", e.clone(), true),
        NetError::Cancelled => ("Cancelled", "The load was stopped.".into(), true),
    };
    pages::error(title, &detail, url, if retry { Some(url) } else { None })
}

/// True if the failure was the privacy policy refusing the request (counts towards the shield).
pub fn is_blocked(err: &NetError) -> bool {
    matches!(err, NetError::Blocked(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_bar_input() {
        let s = "https://html.duckduckgo.com/html/?q=";
        assert_eq!(fix_url("example.com", s), "https://example.com");
        assert_eq!(fix_url("  example.com/a?b=1 ", s), "https://example.com/a?b=1");
        assert_eq!(fix_url("http://x.test/", s), "http://x.test/");
        assert_eq!(fix_url("localhost:8080/x", s), "http://localhost:8080/x");
        assert_eq!(fix_url("rust browser engine", s), format!("{s}rust+browser+engine"));
        assert_eq!(fix_url("what is 1+1?", s), format!("{s}what+is+1%2B1%3F"));
        assert_eq!(fix_url("hello", s), format!("{s}hello"));
        assert_eq!(fix_url("", s), "invis://newtab");
        assert_eq!(fix_url("invis://settings", s), "invis://settings");
    }

    #[test]
    fn web_pages_cannot_open_internal_pages() {
        let web = "https://example.com/page";
        assert!(page_may_open(web, "https://other.example/"));
        assert!(!page_may_open(web, "invis://settings/toggle?id=https_only"));
        assert!(!page_may_open(web, "invis://settings/set?id=proxy_host&value=evil.example"));
        assert!(!page_may_open(web, "INVIS://settings"));
        assert!(!page_may_open(web, "  invis:settings"));
        assert!(!page_may_open(web, "\tinvis://newtab"));
        assert!(!page_may_open("data:text/html,x", "invis://settings"));
        assert!(page_may_open("invis://settings", "invis://settings/toggle?id=https_only"));
        assert!(page_may_open("invis://newtab", "invis://search?q=x"));
        assert!(page_may_open("invis://newtab", "https://example.com/"));
    }

    #[test]
    fn query_params() {
        assert_eq!(query_param("invis://search?q=a+b%26c&x=1", "q").as_deref(), Some("a b&c"));
        assert_eq!(query_param("invis://settings/toggle?id=https_only", "id").as_deref(), Some("https_only"));
        assert_eq!(query_param("invis://settings", "id"), None);
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[test]
    fn a_download_never_takes_the_name_of_a_file_that_is_there() {
        let dir = std::env::temp_dir().join(format!("invis-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "report.pdf"), dir.join("report.pdf"));
        std::fs::write(dir.join("report.pdf"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "report.pdf"), dir.join("report (1).pdf"));
        std::fs::write(dir.join("report (1).pdf"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "report.pdf"), dir.join("report (2).pdf"));
        // no extension, and a name that is only an extension
        std::fs::write(dir.join("LICENSE"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "LICENSE"), dir.join("LICENSE (1)"));
        std::fs::write(dir.join(".profile"), b"x").unwrap();
        assert_eq!(unique_path(&dir, ".profile"), dir.join(".profile (1)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_what_a_tab_cannot_show_is_kept_as_a_file() {
        for shown in ["", "text/html", "application/xhtml+xml", "image/png", "text/plain", "application/json", "image/svg+xml", "text/javascript"] {
            assert!(shown_in_tab(shown), "{shown}");
        }
        for kept in ["application/pdf", "application/zip", "application/octet-stream", "video/mp4", "audio/mpeg", "application/x-msdownload"] {
            assert!(!shown_in_tab(kept), "{kept}");
        }
    }
}
