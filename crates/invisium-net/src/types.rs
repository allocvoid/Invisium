//! Public configuration, request/response and error types (see the NET contract).

use crate::url::Url;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct NetConfig {
    pub proxy: ProxyConfig,
    pub dns: DnsMode,
    pub https_only: bool,
    pub block_trackers: bool,
    pub third_party: ThirdPartyPolicy,
    pub cookies: CookiePolicy,
    pub referrer: ReferrerPolicy,
    pub strip_tracking_params: bool,
    pub user_agent: UaProfile,
    pub generic_accept_language: bool,
    pub send_gpc: bool,
    pub tor_isolate_by_site: bool,
    pub cache_bytes: usize,
    pub max_body_bytes: usize,
    pub connect_timeout_ms: u64,
    pub data_dir: std::path::PathBuf,
}

impl Default for NetConfig {
    /// The "Standard" preset.
    fn default() -> Self {
        NetConfig {
            proxy: ProxyConfig::Direct,
            dns: DnsMode::Doh { provider: DohProvider::Cloudflare, strict: false },
            https_only: true,
            block_trackers: true,
            third_party: ThirdPartyPolicy::BlockCookies,
            cookies: CookiePolicy::FirstPartyOnly,
            referrer: ReferrerPolicy::OriginOnCrossSite,
            strip_tracking_params: true,
            user_agent: UaProfile::Generic,
            generic_accept_language: true,
            send_gpc: true,
            tor_isolate_by_site: true,
            cache_bytes: 32 * 1024 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
            connect_timeout_ms: 15_000,
            data_dir: std::path::PathBuf::from("invisium-data"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyConfig {
    Direct,
    Http { host: String, port: u16, auth: Option<(String, String)> },
    Socks5 { host: String, port: u16, auth: Option<(String, String)> },
    Tor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DnsMode {
    System,
    Doh { provider: DohProvider, strict: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DohProvider {
    Quad9,
    Cloudflare,
    Mullvad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThirdPartyPolicy {
    Allow,
    BlockCookies,
    BlockScriptsAndFrames,
    BlockAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookiePolicy {
    BlockAll,
    FirstPartyOnly,
    AllowPartitioned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferrerPolicy {
    Never,
    OriginOnCrossSite,
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UaProfile {
    Generic,
    Custom(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Document,
    Stylesheet,
    Image,
    Script,
    Font,
    Media,
    Frame,
    Xhr,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Head,
    Put,
    Delete,
    Patch,
    Options,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Head => "HEAD",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
        }
    }
}

/// How the response to a request is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FetchMode {
    /// Navigations and ordinary subresources (the default).
    Subresource,
    /// Script request that must stay on the requesting page's origin.
    SameOrigin,
    /// Script request that may leave the page's origin if the server opts in (CORS).
    Cors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Credentials {
    Omit,
    SameOrigin,
    Include,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    pub method: Method,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub kind: ResourceKind,
    pub referrer: Option<Url>,
    pub top_level: Option<Url>,
    pub mode: FetchMode,
    /// Only read when `mode != Subresource`.
    pub credentials: Credentials,
}

impl Request {
    pub fn get(url: Url, kind: ResourceKind) -> Request {
        Request {
            url,
            method: Method::Get,
            headers: Vec::new(),
            body: None,
            kind,
            referrer: None,
            top_level: None,
            mode: FetchMode::Subresource,
            credentials: Credentials::Include,
        }
    }
}

/// Case-insensitive, ordered multimap of header fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headers {
    list: Vec<(String, String)>,
}

impl Headers {
    pub fn new() -> Headers {
        Headers::default()
    }
    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.list.push((name.into(), value.into()));
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.list.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.list
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.list.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
    pub fn len(&self) -> usize {
        self.list.len()
    }
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.list.retain(|(n, _)| keep(n));
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub url: Url,
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub mime: String,
    pub charset: Option<String>,
    pub from_cache: bool,
    pub redirects: u8,
    pub via: Route,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    Proxy,
    Tor,
    Cache,
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    Blocked(BlockReason),
    InvalidUrl(String),
    Dns(String),
    Connect(String),
    Tls(String),
    Proxy(String),
    Tor(String),
    Timeout,
    TooLarge,
    TooManyRedirects,
    Protocol(String),
    Cancelled,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::Blocked(r) => write!(f, "blocked: {r:?}"),
            NetError::InvalidUrl(s) => write!(f, "invalid URL: {s}"),
            NetError::Dns(s) => write!(f, "DNS error: {s}"),
            NetError::Connect(s) => write!(f, "connection failed: {s}"),
            NetError::Tls(s) => write!(f, "TLS error: {s}"),
            NetError::Proxy(s) => write!(f, "proxy error: {s}"),
            NetError::Tor(s) => write!(f, "Tor error: {s}"),
            NetError::Timeout => f.write_str("timed out"),
            NetError::TooLarge => f.write_str("response too large"),
            NetError::TooManyRedirects => f.write_str("too many redirects"),
            NetError::Protocol(s) => write!(f, "protocol error: {s}"),
            NetError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for NetError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    Tracker,
    ThirdParty,
    HttpsOnly,
    Scheme,
    MixedContent,
    Cors,
    PrivateNetwork,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TorStatus {
    Off,
    Bootstrapping { percent: u8, message: String },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    pub requests: u64,
    pub blocked: u64,
    pub bytes_received: u64,
    pub cache_bytes: usize,
    pub cookies: usize,
    pub open_connections: usize,
}
