//! Glue between the network stack and pages: turning address-bar text into URLs, starting
//! fetches, and turning responses (or failures) into documents.

use invis_chrome::pages;
use invisium::page::{decode_text, CookieAccess, FetchResult, Page, ResKind, UrlJoin};
use invisium_net::{BlockReason, Credentials, FetchMode, Method, NetError, NetService, Request, RequestHandle, ResourceKind, Response, Url};
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
    Sheet { tab: u64, nav: u64, req: u32, result: Result<Response, NetError> },
    Script { tab: u64, nav: u64, req: u32, result: Result<Response, NetError> },
    /// The answer to a request made by page script (`fetch`, `XMLHttpRequest`).
    Fetch { tab: u64, nav: u64, id: u32, result: Result<Response, NetError> },
    Image { tab: u64, nav: u64, url: String, result: Result<DecodedImage, NetError> },
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

pub fn fetch_document(net: &NetService, sink: &Sink, tab: u64, nav: u64, url: Url, body: Option<Vec<u8>>, referrer: Option<Url>) -> RequestHandle {
    let mut req = Request::get(url, ResourceKind::Document);
    if let Some(b) = body {
        req.method = Method::Post;
        req.headers.push(("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()));
        req.body = Some(b);
    }
    req.referrer = referrer;
    let sink = sink.clone();
    net.fetch(req, move |result| sink(NetEvent::Document { tab, nav, result }))
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
pub fn fetch_subresources(net: &NetService, sink: &Sink, tab: u64, nav: u64, page: &mut Page) -> Vec<RequestHandle> {
    let mut handles = Vec::new();
    let page_url = Url::parse(&page.url).ok();
    for r in page.take_requests() {
        let Ok(url) = Url::parse(&r.url) else {
            page.resource_failed(r.id);
            continue;
        };
        let kind = match r.kind {
            ResKind::Stylesheet => ResourceKind::Stylesheet,
            ResKind::Image => ResourceKind::Image,
            ResKind::Script => ResourceKind::Script,
        };
        let mut req = Request::get(url, kind);
        // internal pages (invis://) are not a web origin: their requests carry no referrer
        if let Some(p) = page_url.as_ref().filter(|p| p.is_http()) {
            req.referrer = Some(p.clone());
            req.top_level = Some(p.clone());
        }
        let sink = sink.clone();
        let handle = match r.kind {
            ResKind::Stylesheet => {
                let id = r.id;
                net.fetch(req, move |result| sink(NetEvent::Sheet { tab, nav, req: id, result }))
            }
            ResKind::Script => {
                let id = r.id;
                net.fetch(req, move |result| sink(NetEvent::Script { tab, nav, req: id, result }))
            }
            ResKind::Image => {
                let u = r.url.clone();
                // decoding happens here, on the network thread, never on the UI thread
                net.fetch(req, move |result| sink(NetEvent::Image { tab, nav, url: u, result: result.and_then(|resp| decode_image(&resp)) }))
            }
        };
        handles.push(handle);
    }
    // requests made by the page's scripts: always sent as script requests, so the network
    // stack applies the cross-origin rules; a script request never goes out as a plain fetch
    for f in page.take_fetches() {
        let (Ok(url), Some(doc)) = (Url::parse(&f.url), page_url.clone()) else {
            page.fetch_done(f.id, None);
            continue;
        };
        let mut req = Request::get(url, ResourceKind::Xhr);
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
        req.referrer = Some(doc.clone());
        req.top_level = Some(doc);
        req.mode = if f.same_origin { FetchMode::SameOrigin } else { FetchMode::Cors };
        req.credentials = match f.credentials {
            0 => Credentials::Omit,
            2 => Credentials::Include,
            _ => Credentials::SameOrigin,
        };
        let sink = sink.clone();
        let id = f.id;
        handles.push(net.fetch(req, move |result| sink(NetEvent::Fetch { tab, nav, id, result })));
    }
    handles
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
        &format!("The server sent a file of type \"{}\" ({} bytes). InvisBrowser does not download files yet.", mime, resp.body.len()),
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
