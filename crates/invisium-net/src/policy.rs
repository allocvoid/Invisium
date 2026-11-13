//! The privacy pipeline applied to every request and every redirect hop.

use crate::psl::{is_tracker, same_site};
use crate::types::*;
use crate::url::Url;

pub(crate) const GENERIC_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";
pub(crate) const ACCEPT_LANGUAGE: &str = "en-US,en;q=0.5";
pub(crate) const ACCEPT_ENCODING: &str = "gzip, deflate, br";

/// The `User-Agent` value for this configuration.
pub(crate) fn user_agent(cfg: &NetConfig) -> &str {
    match &cfg.user_agent {
        UaProfile::Custom(s) if valid_value(s) && !s.is_empty() => s.as_str(),
        _ => GENERIC_UA,
    }
}

const TRACKING_PARAMS: &[&str] = &[
    "__s", "_hsenc", "_hsmi", "_openstat", "dclid", "epik", "fbclid", "gbraid", "gclid", "gclsrc",
    "igshid", "li_fat_id", "mc_cid", "mc_eid", "mkt_tok", "msclkid", "oly_anon_id", "oly_enc_id",
    "rb_clickid", "s_cid", "sc_cid", "srsltid", "ttclid", "twclid", "vero_id", "wbraid", "yclid",
    "zanpid",
];

/// Engine-supplied headers that the pipeline owns and never lets the caller set.
const FORBIDDEN: &[&str] = &[
    "accept-charset", "accept-encoding", "accept-language", "connection", "content-length", "cookie",
    "cookie2", "date", "dnt", "expect", "host", "keep-alive", "referer", "te", "trailer",
    "transfer-encoding", "upgrade", "user-agent", "via", "forwarded", "x-forwarded-for",
];

fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str())
}

pub(crate) fn strip_tracking_params(url: &mut Url) {
    let Some(q) = url.query() else { return };
    let parts: Vec<&str> = q.split('&').filter(|p| !p.is_empty()).collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|p| !is_tracking_param(p.split('=').next().unwrap_or("")))
        .collect();
    if kept.len() != parts.len() {
        let joined = kept.join("&");
        url.set_query(if kept.is_empty() { None } else { Some(&joined) });
    }
}

pub(crate) fn is_third_party(top: Option<&Url>, url: &Url) -> bool {
    top.is_some_and(|t| !same_site(t, url))
}

/// Applies rules 1–5 and 10 to one hop. Mutates `url` (param stripping, https upgrade) and
/// returns whether the scheme was upgraded.
pub(crate) fn check(
    cfg: &NetConfig,
    url: &mut Url,
    top: Option<&Url>,
    kind: ResourceKind,
) -> Result<bool, NetError> {
    match url.scheme() {
        "http" | "https" => {}
        "data" => return Ok(false),
        _ => return Err(NetError::Blocked(BlockReason::Scheme)),
    }
    if cfg.strip_tracking_params {
        strip_tracking_params(url);
    }
    let mut upgraded = false;
    if cfg.https_only && url.scheme() == "http" {
        url.set_scheme("https");
        upgraded = true;
    }
    if top.is_some_and(Url::is_secure) && !url.is_secure() {
        return Err(NetError::Blocked(BlockReason::MixedContent));
    }
    let host = url.host().trim_end_matches('.');
    if (host == "onion" || host.ends_with(".onion")) && cfg.proxy != ProxyConfig::Tor {
        return Err(NetError::Tor(".onion hosts are only reachable through Tor".into()));
    }
    if cfg.block_trackers && top.is_some() && is_tracker(url.host()) {
        return Err(NetError::Blocked(BlockReason::Tracker));
    }
    if is_third_party(top, url) {
        let blocked = match cfg.third_party {
            ThirdPartyPolicy::BlockAll => true,
            ThirdPartyPolicy::BlockScriptsAndFrames => {
                matches!(kind, ResourceKind::Script | ResourceKind::Frame)
            }
            _ => false,
        };
        if blocked {
            return Err(NetError::Blocked(BlockReason::ThirdParty));
        }
    }
    Ok(upgraded)
}

/// Whether cookies may be sent/stored on this request.
pub(crate) fn cookies_allowed(cfg: &NetConfig, third_party: bool) -> bool {
    match cfg.cookies {
        CookiePolicy::BlockAll => false,
        CookiePolicy::FirstPartyOnly => !third_party,
        CookiePolicy::AllowPartitioned => !third_party || cfg.third_party == ThirdPartyPolicy::Allow,
    }
}

pub(crate) fn referer(policy: ReferrerPolicy, referrer: Option<&Url>, target: &Url) -> Option<String> {
    let r = referrer?;
    if !r.is_http() || (r.is_secure() && !target.is_secure()) {
        return None;
    }
    match policy {
        ReferrerPolicy::Never => None,
        ReferrerPolicy::Full => Some(r.referrer_string()),
        ReferrerPolicy::OriginOnCrossSite if same_site(r, target) => Some(r.referrer_string()),
        ReferrerPolicy::OriginOnCrossSite => Some(format!("{}/", r.origin())),
    }
}

fn accept_for(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document | ResourceKind::Frame => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        }
        ResourceKind::Stylesheet => "text/css,*/*;q=0.1",
        ResourceKind::Image => "image/avif,image/webp,image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5",
        ResourceKind::Font => "application/font-woff2;q=1.0,application/font-woff;q=0.9,*/*;q=0.8",
        ResourceKind::Media => "video/webm,video/ogg,video/*;q=0.9,audio/*;q=0.6,*/*;q=0.5",
        ResourceKind::Script | ResourceKind::Xhr | ResourceKind::Other => "*/*",
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
pub(crate) fn valid_value(v: &str) -> bool {
    v.bytes().all(|b| b == b'\t' || (b >= 0x20 && b != 0x7f))
}

/// Whether an engine-supplied header other than `Accept` is written to the wire as given.
pub(crate) fn extra_sendable(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(lower == "accept"
        || FORBIDDEN.contains(&lower.as_str())
        || lower.starts_with("proxy-")
        || lower.starts_with("sec-")
        || !valid_name(name)
        || !valid_value(value))
}

pub(crate) struct Outgoing<'a> {
    pub url: &'a Url,
    pub method: Method,
    pub kind: ResourceKind,
    pub extra: &'a [(String, String)],
    pub referer: Option<String>,
    /// `Origin` of a cross-origin script request (set by the CORS logic, never by the engine).
    pub origin: Option<&'a str>,
    pub cookie: Option<String>,
    pub body: Option<&'a [u8]>,
}

/// Serialises the request head (and body) with the fixed, machine-independent header set.
pub(crate) fn build_request(cfg: &NetConfig, o: &Outgoing<'_>) -> Vec<u8> {
    let mut h = format!("{} {} HTTP/1.1\r\n", o.method.as_str(), o.url.request_target());
    let mut push = |n: &str, v: &str| {
        h.push_str(n);
        h.push_str(": ");
        h.push_str(v);
        h.push_str("\r\n");
    };
    push("Host", &o.url.host_port());
    push("User-Agent", user_agent(cfg));
    let accept = o
        .extra
        .iter()
        .find(|(n, v)| n.eq_ignore_ascii_case("accept") && valid_value(v))
        .map_or(accept_for(o.kind), |(_, v)| v.as_str());
    push("Accept", accept);
    if cfg.generic_accept_language {
        push("Accept-Language", ACCEPT_LANGUAGE);
    }
    push("Accept-Encoding", ACCEPT_ENCODING);
    if let Some(r) = &o.referer {
        push("Referer", r);
    }
    if let Some(origin) = o.origin {
        push("Origin", origin);
    }
    if let Some(c) = &o.cookie {
        push("Cookie", c);
    }
    if cfg.send_gpc {
        push("DNT", "1");
        push("Sec-GPC", "1");
    }
    push("Connection", "keep-alive");
    for (n, v) in o.extra {
        if extra_sendable(n, v) {
            push(n, v);
        }
    }
    match o.body {
        Some(b) => push("Content-Length", &b.len().to_string()),
        None if matches!(o.method, Method::Post | Method::Put | Method::Patch) => {
            push("Content-Length", "0")
        }
        None => {}
    }
    h.push_str("\r\n");
    let mut out = h.into_bytes();
    if let Some(b) = o.body {
        out.extend_from_slice(b);
    }
    out
}
