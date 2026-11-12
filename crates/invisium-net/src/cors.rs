//! The CORS protocol (Fetch standard) for requests made by page scripts, as pure decisions:
//! origin comparison, header safelists, forbidden headers, the preflight decision and its
//! verdict, the CORS check and response-header filtering. `service.rs` only wires these in.

use crate::policy;
use crate::types::*;
use crate::url::Url;

fn blocked() -> NetError {
    NetError::Blocked(BlockReason::Cors)
}

/// Same scheme, host and port. Only http(s) URLs have an origin that anything can equal.
pub(crate) fn same_origin(a: &Url, b: &Url) -> bool {
    a.is_http()
        && b.is_http()
        && !a.host().is_empty()
        && a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_default() == b.port_or_default()
}

/// The requesting origin as sent in `Origin`; "null" for a page without a network origin.
pub(crate) fn origin_of(page: &Url) -> String {
    if page.is_http() && !page.host().is_empty() {
        page.origin()
    } else {
        "null".to_string()
    }
}

/// What has happened on the redirect chain so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chain {
    /// Some earlier hop was cross-origin: every later hop is treated as cross-origin too, even
    /// one that returns to the requesting origin.
    pub left_origin: bool,
    /// The chain moved from a foreign origin to a different origin: `Origin` is "null" now.
    pub tainted: bool,
}

impl Chain {
    /// Records a redirect from `from` (the hop just answered) to `to`.
    pub fn redirect(&mut self, page: &Url, from: &Url, to: &Url) {
        if !same_origin(page, from) {
            self.left_origin = true;
            if !same_origin(from, to) {
                self.tainted = true;
            }
        }
    }
}

/// How one hop of a request is sent and how its response is protected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hop {
    /// `Some` exactly on a cross-origin CORS hop: the value of the `Origin` header. The
    /// response must then pass the CORS check against it and its headers are filtered.
    pub origin: Option<String>,
    /// Cookies may be sent and stored on this hop (still subject to the cookie policy).
    pub cookies: bool,
    /// Credentials mode is `Include`: the stricter CORS check applies.
    pub include: bool,
}

impl Hop {
    pub fn cross(&self) -> bool {
        self.origin.is_some()
    }
}

/// Rules 1-3 for one hop (the first URL or a redirect target). `Subresource` requests get a
/// neutral plan. Fails with `Blocked(Cors)` before anything is sent.
pub(crate) fn plan_hop(
    page: Option<&Url>,
    url: &Url,
    mode: FetchMode,
    credentials: Credentials,
    chain: Chain,
) -> Result<Hop, NetError> {
    if mode == FetchMode::Subresource {
        return Ok(Hop { origin: None, cookies: true, include: false });
    }
    let page = page.ok_or_else(blocked)?;
    if url.has_userinfo() {
        return Err(blocked());
    }
    let cross = chain.left_origin || !same_origin(page, url);
    if cross && mode == FetchMode::SameOrigin {
        return Err(blocked());
    }
    let origin = cross.then(|| if chain.tainted { "null".to_string() } else { origin_of(page) });
    let cookies = match credentials {
        Credentials::Omit => false,
        Credentials::SameOrigin => !cross,
        Credentials::Include => true,
    };
    Ok(Hop { origin, cookies, include: credentials == Credentials::Include })
}

const FORBIDDEN_REQUEST: &[&str] = &[
    "accept-charset", "accept-encoding", "connection", "content-length", "cookie", "cookie2",
    "date", "dnt", "expect", "host", "keep-alive", "origin", "referer", "set-cookie", "te",
    "trailer", "transfer-encoding", "upgrade", "user-agent", "via",
];

/// Rule 4: headers a script may not set.
pub(crate) fn forbidden_request_header(name: &str, value: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let overrides_method = || {
        value
            .split(',')
            .any(|m| matches!(m.trim().to_ascii_uppercase().as_str(), "CONNECT" | "TRACE" | "TRACK"))
    };
    FORBIDDEN_REQUEST.contains(&n.as_str())
        || n.starts_with("proxy-")
        || n.starts_with("sec-")
        || n.starts_with("access-control-request-")
        || (matches!(n.as_str(), "x-http-method" | "x-http-method-override" | "x-method-override")
            && overrides_method())
}

/// The script-supplied headers that will actually be written to the wire: forbidden ones and
/// those the privacy pipeline owns or rejects are dropped, and only one `Accept` survives.
pub(crate) fn script_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut have_accept = false;
    for (n, v) in headers {
        if forbidden_request_header(n, v) {
            continue;
        }
        let keep = if n.eq_ignore_ascii_case("accept") {
            !std::mem::replace(&mut have_accept, true) && policy::valid_value(v)
        } else {
            policy::extra_sendable(n, v)
        };
        if keep {
            out.push((n.clone(), v.clone()));
        }
    }
    out
}

fn cors_unsafe_byte(b: u8) -> bool {
    (b < 0x20 && b != b'\t') || b == 0x7f || b"\"():<>?@[\\]{}".contains(&b)
}

/// CORS-safelisted request header (name and value).
fn safelisted(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(cors_unsafe_byte),
        "accept-language" | "content-language" => {
            value.bytes().all(|b| b.is_ascii_alphanumeric() || b" *,-.;=".contains(&b))
        }
        "content-type" => {
            let essence = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            !value.bytes().any(cors_unsafe_byte)
                && matches!(
                    essence.as_str(),
                    "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
                )
        }
        _ => false,
    }
}

/// Lower-case, sorted, de-duplicated names of the headers that are not CORS-safelisted.
pub(crate) fn unsafe_header_names(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut safe_bytes = 0usize;
    for (n, v) in headers {
        if safelisted(n, v) {
            safe_bytes += v.len();
        } else {
            names.push(n.to_ascii_lowercase());
        }
    }
    if safe_bytes > 1024 {
        names = headers.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
    }
    names.sort();
    names.dedup();
    names
}

/// A cross-origin request needs a preflight unless method and headers are all safelisted.
pub(crate) fn needs_preflight(method: Method, unsafe_names: &[String]) -> bool {
    !matches!(method, Method::Get | Method::Head | Method::Post) || !unsafe_names.is_empty()
}

/// The headers that make an `OPTIONS` request a preflight for `method` and `unsafe_names`.
pub(crate) fn preflight_headers(method: Method, unsafe_names: &[String]) -> Vec<(String, String)> {
    let mut h = vec![("Access-Control-Request-Method".to_string(), method.as_str().to_string())];
    if !unsafe_names.is_empty() {
        h.push(("Access-Control-Request-Headers".to_string(), unsafe_names.join(",")));
    }
    h
}

/// The CORS check: exactly one `Access-Control-Allow-Origin`, equal to the `Origin` that was
/// sent or `*` without credentials; with credentials also `Access-Control-Allow-Credentials: true`.
pub(crate) fn cors_check(headers: &Headers, origin: &str, include: bool) -> bool {
    let mut allow = headers.get_all("access-control-allow-origin");
    let (Some(value), None) = (allow.next(), allow.next()) else { return false };
    if !include {
        return value == "*" || value == origin;
    }
    let mut creds = headers.get_all("access-control-allow-credentials");
    value != "*" && value == origin && matches!((creds.next(), creds.next()), (Some("true"), None))
}

/// Comma-separated tokens of every field named `name`.
fn tokens(headers: &Headers, name: &str) -> Vec<String> {
    headers
        .get_all(name)
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Whether a preflight response allows the actual request.
pub(crate) fn preflight_passes(
    status: u16,
    headers: &Headers,
    origin: &str,
    include: bool,
    method: Method,
    unsafe_names: &[String],
) -> bool {
    if !(200..=299).contains(&status) || !cors_check(headers, origin, include) {
        return false;
    }
    let methods = tokens(headers, "access-control-allow-methods");
    let method_ok = matches!(method, Method::Get | Method::Head | Method::Post)
        || methods.iter().any(|m| m == method.as_str())
        || (!include && methods.iter().any(|m| m == "*"));
    let allowed: Vec<String> = tokens(headers, "access-control-allow-headers")
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let wildcard = !include && allowed.iter().any(|h| h == "*");
    method_ok
        && unsafe_names.iter().all(|n| allowed.contains(n) || (wildcard && n != "authorization"))
}

const SAFE_RESPONSE: &[&str] = &[
    "cache-control", "content-language", "content-length", "content-type", "expires",
    "last-modified", "pragma",
];

/// Rule 5: removes the response headers a script may not see. `cross` = cross-origin CORS hop.
pub(crate) fn filter_response_headers(headers: &mut Headers, cross: bool, include: bool) {
    let exposed: Vec<String> = tokens(headers, "access-control-expose-headers")
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let all = !include && exposed.iter().any(|h| h == "*");
    headers.retain(|name| {
        let n = name.to_ascii_lowercase();
        n != "set-cookie"
            && n != "set-cookie2"
            && (!cross || all || SAFE_RESPONSE.contains(&n.as_str()) || exposed.contains(&n))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }
    fn hs(pairs: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (n, v) in pairs {
            h.push(*n, *v);
        }
        h
    }
    fn req(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }
    fn names(h: &Headers) -> Vec<String> {
        h.iter().map(|(n, _)| n.to_ascii_lowercase()).collect()
    }

    #[test]
    fn origins() {
        let page = u("https://app.example/dir/page");
        assert!(same_origin(&page, &u("https://app.example:443/other")));
        assert!(!same_origin(&page, &u("http://app.example/")));
        assert!(!same_origin(&page, &u("https://app.example:8443/")));
        assert!(!same_origin(&page, &u("https://www.app.example/")));
        assert!(!same_origin(&u("file:///a.html"), &u("file:///a.html")));
        assert!(!same_origin(&u("data:text/plain,x"), &u("data:text/plain,x")));
        assert_eq!(origin_of(&u("http://App.Example:8080/x?y#z")), "http://app.example:8080");
        assert_eq!(origin_of(&u("file:///a.html")), "null");
        assert_eq!(origin_of(&u("data:text/html,x")), "null");
    }

    #[test]
    fn hop_plans() {
        let page = u("https://app.example/");
        let (same, other) = (u("https://app.example/api"), u("https://api.other/x"));
        let start = Chain::default();
        let plan = |url: &Url, mode, creds, chain| plan_hop(Some(&page), url, mode, creds, chain);
        // Subresource: untouched, even without a page.
        let sub = plan_hop(None, &other, FetchMode::Subresource, Credentials::Omit, start).unwrap();
        assert_eq!(sub, Hop { origin: None, cookies: true, include: false });
        // Script request without a page.
        for mode in [FetchMode::SameOrigin, FetchMode::Cors] {
            assert_eq!(plan_hop(None, &same, mode, Credentials::Include, start), Err(blocked()));
        }
        // SameOrigin mode never leaves the origin.
        assert!(!plan(&same, FetchMode::SameOrigin, Credentials::Include, start).unwrap().cross());
        assert_eq!(plan(&other, FetchMode::SameOrigin, Credentials::Include, start), Err(blocked()));
        // Cors mode: Origin and credentials per hop.
        let h = plan(&other, FetchMode::Cors, Credentials::Include, start).unwrap();
        assert_eq!((h.cross(), h.origin.as_deref(), h.cookies, h.include), (true, Some("https://app.example"), true, true));
        for creds in [Credentials::Omit, Credentials::SameOrigin] {
            let h = plan(&other, FetchMode::Cors, creds, start).unwrap();
            assert_eq!((h.cross(), h.cookies, h.include), (true, false, false));
        }
        let h = plan(&same, FetchMode::Cors, Credentials::SameOrigin, start).unwrap();
        assert_eq!((h.cross(), h.origin.as_deref(), h.cookies), (false, None, true));
        assert!(!plan(&same, FetchMode::Cors, Credentials::Omit, start).unwrap().cookies);
        // Userinfo is refused on every script hop.
        let cred_url = u("https://user:pw@api.other/x");
        assert_eq!(plan(&cred_url, FetchMode::Cors, Credentials::Omit, start), Err(blocked()));
        // Redirect chains: same-origin -> foreign keeps the real origin; foreign A -> foreign B
        // taints it; coming back to the page's origin stays cross-origin.
        let mut chain = Chain::default();
        chain.redirect(&page, &same, &other);
        assert_eq!(chain, Chain::default());
        assert_eq!(plan(&other, FetchMode::Cors, Credentials::Omit, chain).unwrap().origin.as_deref(), Some("https://app.example"));
        chain.redirect(&page, &other, &u("https://api.other/y"));
        assert_eq!(chain, Chain { left_origin: true, tainted: false });
        chain.redirect(&page, &other, &u("https://third.example/"));
        assert_eq!(chain, Chain { left_origin: true, tainted: true });
        let back = plan(&same, FetchMode::Cors, Credentials::SameOrigin, chain).unwrap();
        assert_eq!((back.cross(), back.origin.as_deref(), back.cookies), (true, Some("null"), false));
        assert_eq!(plan(&same, FetchMode::SameOrigin, Credentials::Omit, chain), Err(blocked()));
    }

    #[test]
    fn forbidden_and_sanitised_request_headers() {
        for n in [
            "Accept-Charset", "accept-encoding", "Access-Control-Request-Method", "Access-Control-Request-Headers",
            "Connection", "Content-Length", "Cookie", "Cookie2", "Date", "DNT", "Expect", "Host", "Keep-Alive",
            "ORIGIN", "Referer", "TE", "Trailer", "Transfer-Encoding", "Upgrade", "User-Agent", "Via",
            "Proxy-Authorization", "Sec-Fetch-Site", "Set-Cookie",
        ] {
            assert!(forbidden_request_header(n, "x"), "{n}");
        }
        assert!(forbidden_request_header("X-HTTP-Method-Override", "get, TRACE"));
        assert!(!forbidden_request_header("X-HTTP-Method-Override", "PUT"));
        for n in ["Accept", "Content-Type", "Authorization", "X-Custom", "Secret", "Proxy"] {
            assert!(!forbidden_request_header(n, "x"), "{n}");
        }
        let kept = script_headers(&req(&[
            ("Cookie", "a=1"), ("X-Ok", "1"), ("Origin", "https://evil.example"), ("Accept", "text/plain"),
            ("Accept", "second/ignored"), ("Accept-Language", "sv"), ("Bad Name", "x"), ("X-Inj", "a\r\nB: c"),
            ("Sec-Fetch-Mode", "navigate"), ("Content-Type", "application/json"),
        ]));
        assert_eq!(kept, req(&[("X-Ok", "1"), ("Accept", "text/plain"), ("Content-Type", "application/json")]));
    }

    #[test]
    fn safelist_and_preflight_decision() {
        let simple = req(&[
            ("Accept", "text/html"), ("Accept-Language", "en-US,en;q=0.5"), ("Content-Language", "sv"),
            ("Content-Type", "Text/Plain; charset=UTF-8"),
        ]);
        assert!(unsafe_header_names(&simple).is_empty());
        for ct in ["application/x-www-form-urlencoded", "multipart/form-data; boundary=x1"] {
            assert!(unsafe_header_names(&req(&[("content-type", ct)])).is_empty(), "{ct}");
        }
        for (n, v) in [
            ("Content-Type", "application/json"), ("Content-Type", "text/plain; a=\"b\""), ("Content-Type", ""),
            ("Accept", "a(b)"), ("Accept-Language", "en@x"), ("X-Custom", "1"), ("Authorization", "Bearer t"),
        ] {
            assert_eq!(unsafe_header_names(&req(&[(n, v)])), [n.to_ascii_lowercase()], "{n}: {v}");
        }
        let long = "a".repeat(129);
        assert_eq!(unsafe_header_names(&req(&[("Accept", &long)])), ["accept"]);
        let sorted = unsafe_header_names(&req(&[("X-B", "1"), ("x-a", "1"), ("X-b", "2"), ("Accept", "*/*")]));
        assert_eq!(sorted, ["x-a", "x-b"]);
        // More than 1024 bytes of safelisted values in total: none of them counts as safe.
        let v = "a".repeat(128);
        let many: Vec<(String, String)> = (0..9).map(|_| ("Accept".to_string(), v.clone())).collect();
        assert_eq!(unsafe_header_names(&many), ["accept"]);
        assert!(!needs_preflight(Method::Get, &[]) && !needs_preflight(Method::Head, &[]) && !needs_preflight(Method::Post, &[]));
        for m in [Method::Put, Method::Delete, Method::Patch, Method::Options] {
            assert!(needs_preflight(m, &[]), "{m:?}");
        }
        assert!(needs_preflight(Method::Get, &["x-a".to_string()]));
        assert_eq!(
            preflight_headers(Method::Put, &sorted),
            req(&[("Access-Control-Request-Method", "PUT"), ("Access-Control-Request-Headers", "x-a,x-b")])
        );
        assert_eq!(preflight_headers(Method::Delete, &[]), req(&[("Access-Control-Request-Method", "DELETE")]));
    }

    #[test]
    fn cors_check_cases() {
        let o = "https://app.example";
        let acao = |v: &str| hs(&[("Access-Control-Allow-Origin", v)]);
        assert!(cors_check(&acao(o), o, false) && cors_check(&acao("*"), o, false));
        for bad in ["https://other.example", "https://app.example/", "HTTPS://APP.EXAMPLE", "", "null", "*, *"] {
            assert!(!cors_check(&acao(bad), o, false), "{bad}");
        }
        assert!(!cors_check(&acao(&format!("{o}, {o}")), o, false));
        assert!(!cors_check(&hs(&[]), o, false));
        assert!(!cors_check(&hs(&[("Access-Control-Allow-Origin", o), ("access-control-allow-origin", o)]), o, false));
        assert!(cors_check(&acao("null"), "null", false));
        // With credentials.
        let with = |a: &str, c: &[&str]| {
            let mut h = acao(a);
            for v in c {
                h.push("Access-Control-Allow-Credentials", *v);
            }
            h
        };
        assert!(cors_check(&with(o, &["true"]), o, true));
        assert!(!cors_check(&with("*", &["true"]), o, true));
        assert!(!cors_check(&with(o, &[]), o, true));
        assert!(!cors_check(&with(o, &["True"]), o, true));
        assert!(!cors_check(&with(o, &["true", "true"]), o, true));
        assert!(!cors_check(&with("*", &["true"]), "*", true));
    }

    #[test]
    fn preflight_verdicts() {
        let o = "https://app.example";
        let custom = vec!["authorization".to_string(), "x-a".to_string()];
        let resp = |methods: &str, headers: &str| {
            hs(&[("Access-Control-Allow-Origin", o), ("Access-Control-Allow-Credentials", "true"),
                 ("Access-Control-Allow-Methods", methods), ("Access-Control-Allow-Headers", headers)])
        };
        let good = resp("GET, PUT", "X-A, Authorization");
        assert!(preflight_passes(204, &good, o, true, Method::Put, &custom));
        assert!(!preflight_passes(301, &good, o, true, Method::Put, &custom));
        assert!(!preflight_passes(404, &good, o, true, Method::Put, &custom));
        assert!(!preflight_passes(200, &good, "https://other.example", true, Method::Put, &custom));
        assert!(!preflight_passes(200, &good, o, true, Method::Delete, &custom));
        assert!(!preflight_passes(200, &resp("put", "x-a, authorization"), o, true, Method::Put, &custom));
        assert!(!preflight_passes(200, &resp("PUT", "x-a"), o, true, Method::Put, &custom));
        // Safelisted methods need no Allow-Methods; no unsafe headers need no Allow-Headers.
        assert!(preflight_passes(200, &resp("", "x-a, authorization"), o, true, Method::Post, &custom));
        assert!(preflight_passes(200, &resp("DELETE", ""), o, false, Method::Delete, &[]));
        // Wildcards: only without credentials, and never for Authorization.
        let star = resp("*", "*");
        let only_xa = vec!["x-a".to_string()];
        assert!(preflight_passes(200, &star, o, false, Method::Put, &only_xa));
        assert!(!preflight_passes(200, &star, o, true, Method::Put, &only_xa));
        assert!(!preflight_passes(200, &star, o, true, Method::Get, &only_xa));
        assert!(!preflight_passes(200, &star, o, false, Method::Put, &custom));
        assert!(preflight_passes(200, &resp("*", "*, authorization"), o, false, Method::Put, &custom));
    }

    #[test]
    fn hostile_header_values_fail_closed_without_panicking() {
        let o = "https://app.example";
        let long = "x,".repeat(20_000);
        let nasty = ["", ",", ",,, ,", "\u{0}", "é\u{301}, ü", "*\u{0}", " * ", "*,*", long.as_str(), "\u{feff}*", "true\u{0}"];
        for a in nasty {
            for b in nasty {
                let mut h = hs(&[
                    ("Access-Control-Allow-Origin", a), ("Access-Control-Allow-Credentials", b),
                    ("Access-Control-Allow-Methods", a), ("Access-Control-Allow-Headers", b),
                    ("Access-Control-Expose-Headers", b), ("Set-Cookie", a), ("X-Secret", "s"),
                ]);
                for include in [false, true] {
                    assert!(!cors_check(&h, o, include), "{a:?} {b:?}");
                    assert!(!preflight_passes(200, &h, o, include, Method::Put, &["x-a".to_string()]));
                }
                filter_response_headers(&mut h, true, true);
                assert!(h.is_empty(), "{a:?} {b:?}: {:?}", names(&h));
            }
        }
        let _ = script_headers(&req(&[("\u{0}", "é"), ("", ""), ("X-A", long.as_str()), ("x-http-method", "é,TRACE")]));
        let _ = unsafe_header_names(&req(&[("é", "é"), ("Content-Type", "é;;;"), ("Accept", long.as_str())]));
    }

    #[test]
    fn response_header_filtering() {
        let full = |expose: Option<&str>| {
            let mut h = hs(&[
                ("Content-Type", "text/plain"), ("Set-Cookie", "a=1"), ("set-cookie2", "b=2"), ("X-Secret", "s"),
                ("Cache-Control", "no-store"), ("X-Shown", "1"), ("Content-Length", "3"), ("ETag", "\"e\""),
            ]);
            if let Some(e) = expose {
                h.push("Access-Control-Expose-Headers", e);
            }
            h
        };
        let filtered = |expose, cross, include| {
            let mut h = full(expose);
            filter_response_headers(&mut h, cross, include);
            names(&h)
        };
        // Same-origin: everything except the cookie headers.
        assert_eq!(filtered(None, false, true), ["content-type", "x-secret", "cache-control", "x-shown", "content-length", "etag"]);
        // Cross-origin: safelist only, plus what the server names.
        assert_eq!(filtered(None, true, false), ["content-type", "cache-control", "content-length"]);
        assert_eq!(filtered(Some("X-Shown, Set-Cookie"), true, true), ["content-type", "cache-control", "x-shown", "content-length"]);
        let all = ["content-type", "x-secret", "cache-control", "x-shown", "content-length", "etag", "access-control-expose-headers"];
        assert_eq!(filtered(Some("*"), true, false), all);
        assert_eq!(filtered(Some("*"), true, true), ["content-type", "cache-control", "content-length"]);
    }
}
