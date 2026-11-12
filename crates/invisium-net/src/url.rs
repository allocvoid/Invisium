//! From-scratch URL parser (absolute URLs, WHATWG-flavoured), RFC 3986 reference resolution,
//! percent-encoding helpers and `data:` URL decoding.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::net::Ipv6Addr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    MissingScheme,
    InvalidHost,
    InvalidPort,
    CannotBeABase,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UrlError::Empty => "empty URL",
            UrlError::MissingScheme => "missing or invalid scheme",
            UrlError::InvalidHost => "invalid host",
            UrlError::InvalidPort => "invalid port",
            UrlError::CannotBeABase => "relative reference against a URL that cannot be a base",
        })
    }
}

impl std::error::Error for UrlError {}

#[derive(Clone)]
pub struct Url {
    ser: String,
    scheme: String,
    username: String,
    password: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
    authority: bool,
    frag_start: usize,
}

impl PartialEq for Url {
    fn eq(&self, other: &Self) -> bool {
        self.ser == other.ser
    }
}
impl Eq for Url {}
impl Hash for Url {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ser.hash(state)
    }
}
impl fmt::Debug for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Url({:?})", self.ser)
    }
}
impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.ser)
    }
}

fn is_special(s: &str) -> bool {
    matches!(s, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

fn default_port(s: &str) -> Option<u16> {
    match s {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

/// Schemes that never have a host; everything after ':' is an opaque path.
fn is_opaque_scheme(s: &str) -> bool {
    matches!(s, "data" | "about" | "invis" | "javascript" | "mailto" | "blob")
}

fn clean(input: &str) -> String {
    input
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

fn scheme_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_alphabetic() {
        return None;
    }
    for (i, &c) in b.iter().enumerate().skip(1) {
        if c == b':' {
            return Some(i);
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')) {
            return None;
        }
    }
    None
}

fn push_pct(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 15) as usize] as char);
}

fn encode(s: &str, needs: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if needs(c) {
            let mut buf = [0u8; 4];
            for &b in c.encode_utf8(&mut buf).as_bytes() {
                push_pct(&mut out, b);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn c0_or_nonascii(c: char) -> bool {
    c < ' ' || c > '~'
}
fn path_needs(c: char) -> bool {
    c0_or_nonascii(c) || matches!(c, ' ' | '"' | '#' | '<' | '>' | '?' | '`' | '{' | '}')
}
fn query_needs(c: char, special: bool) -> bool {
    c0_or_nonascii(c) || matches!(c, ' ' | '"' | '#' | '<' | '>') || (special && c == '\'')
}
fn fragment_needs(c: char) -> bool {
    c0_or_nonascii(c) || matches!(c, ' ' | '"' | '<' | '>' | '`')
}
fn userinfo_needs(c: char) -> bool {
    path_needs(c) || matches!(c, '/' | ':' | ';' | '=' | '@' | '[' | '\\' | ']' | '^' | '|')
}

/// Removes dot segments and percent-encodes each segment. `raw` starts with '/'.
fn normalize_path(raw: &str) -> String {
    let segs: Vec<&str> = raw[1..].split('/').collect();
    let mut out: Vec<String> = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        let lower = s.to_ascii_lowercase();
        let is_dot = lower == "." || lower == "%2e";
        let is_dotdot = matches!(lower.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e");
        if is_dotdot {
            out.pop();
            if last {
                out.push(String::new());
            }
        } else if is_dot {
            if last {
                out.push(String::new());
            }
        } else {
            out.push(encode(s, path_needs));
        }
    }
    let mut p = String::from("/");
    p.push_str(&out.join("/"));
    p
}

fn punycode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
        delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE - TMIN + 1) * delta / (delta + 38)
    }
    fn digit(d: u32) -> char {
        if d < 26 {
            (b'a' + d as u8) as char
        } else {
            (b'0' + (d - 26) as u8) as char
        }
    }
    let cps: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut out: String = input.chars().filter(|c| c.is_ascii()).collect();
    let b = out.len() as u32;
    let mut h = b;
    if b > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (128u32, 0u32, 72u32);
    while (h as usize) < cps.len() {
        let m = cps.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, h + 1, h == b);
                delta = 0;
                h += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

fn process_host(raw: &str, special: bool) -> Result<String, UrlError> {
    if raw.starts_with('[') {
        let inner = raw
            .strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .ok_or(UrlError::InvalidHost)?;
        let ip: Ipv6Addr = inner.parse().map_err(|_| UrlError::InvalidHost)?;
        return Ok(format!("[{ip}]"));
    }
    let forbidden = |c: char| {
        c <= ' '
            || c == '\x7f'
            || matches!(c, '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|')
    };
    if !special {
        if raw.chars().any(forbidden) {
            return Err(UrlError::InvalidHost);
        }
        return Ok(encode(&raw.to_ascii_lowercase(), c0_or_nonascii));
    }
    let decoded = String::from_utf8(percent_decode(raw)).map_err(|_| UrlError::InvalidHost)?;
    let lower = decoded.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for (i, label) in lower.split('.').enumerate() {
        if i > 0 {
            out.push('.');
        }
        if label.is_ascii() {
            out.push_str(label);
        } else {
            out.push_str("xn--");
            out.push_str(&punycode(label).ok_or(UrlError::InvalidHost)?);
        }
    }
    if out.len() > 253 || out.chars().any(|c| forbidden(c) || c == '%') {
        return Err(UrlError::InvalidHost);
    }
    Ok(out)
}

impl Url {
    /// Parses an absolute URL.
    pub fn parse(input: &str) -> Result<Url, UrlError> {
        let s = clean(input);
        if s.is_empty() {
            return Err(UrlError::Empty);
        }
        let colon = scheme_end(&s).ok_or(UrlError::MissingScheme)?;
        let scheme = s[..colon].to_ascii_lowercase();
        let rest = &s[colon + 1..];
        let (rest, fragment) = match rest.find('#') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let (hier, query) = match rest.find('?') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let special = is_special(&scheme);
        let mut url = Url {
            ser: String::new(),
            scheme,
            username: String::new(),
            password: String::new(),
            host: String::new(),
            port: None,
            path: String::new(),
            query: None,
            fragment: None,
            authority: false,
            frag_start: 0,
        };
        if url.scheme == "file" {
            url.authority = true;
            let mut p = hier.replace('\\', "/");
            if let Some(after) = p.strip_prefix("//") {
                p = match after.find('/') {
                    Some(i) => after[i..].to_string(),
                    None => "/".to_string(),
                };
            }
            if !p.starts_with('/') {
                p.insert(0, '/');
            }
            url.path = normalize_path(&p);
        } else if special || (!is_opaque_scheme(&url.scheme) && hier.starts_with("//")) {
            url.authority = true;
            let after = if special {
                hier.trim_start_matches(['/', '\\'])
            } else {
                &hier[2..]
            };
            let end = after
                .find(|c| c == '/' || (special && c == '\\'))
                .unwrap_or(after.len());
            url.parse_authority(&after[..end], special)?;
            let p = &after[end..];
            let p = if special { p.replace('\\', "/") } else { p.to_string() };
            url.path = if p.is_empty() {
                if special { "/".to_string() } else { String::new() }
            } else {
                normalize_path(&p)
            };
        } else {
            url.path = encode(hier, c0_or_nonascii);
        }
        url.query = query.map(|q| encode(q, |c| query_needs(c, special)));
        url.fragment = fragment.map(|f| encode(f, fragment_needs));
        url.rebuild();
        Ok(url)
    }

    fn parse_authority(&mut self, auth: &str, special: bool) -> Result<(), UrlError> {
        let (userinfo, hostport) = match auth.rfind('@') {
            Some(i) => (Some(&auth[..i]), &auth[i + 1..]),
            None => (None, auth),
        };
        if let Some(ui) = userinfo {
            let (u, p) = ui.split_once(':').unwrap_or((ui, ""));
            self.username = encode(u, userinfo_needs);
            self.password = encode(p, userinfo_needs);
        }
        let (host_raw, port_raw) = if hostport.starts_with('[') {
            let close = hostport.find(']').ok_or(UrlError::InvalidHost)?;
            let after = &hostport[close + 1..];
            let port = if after.is_empty() {
                None
            } else {
                Some(after.strip_prefix(':').ok_or(UrlError::InvalidHost)?)
            };
            (&hostport[..=close], port)
        } else {
            match hostport.find(':') {
                Some(i) => (&hostport[..i], Some(&hostport[i + 1..])),
                None => (hostport, None),
            }
        };
        self.host = process_host(host_raw, special)?;
        if special && self.host.is_empty() {
            return Err(UrlError::InvalidHost);
        }
        if let Some(p) = port_raw.filter(|p| !p.is_empty()) {
            if p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(UrlError::InvalidPort);
            }
            let n: u32 = p.parse().map_err(|_| UrlError::InvalidPort)?;
            let n = u16::try_from(n).map_err(|_| UrlError::InvalidPort)?;
            if default_port(&self.scheme) != Some(n) {
                self.port = Some(n);
            }
        }
        Ok(())
    }

    /// `scheme://[userinfo@]host[:port]` (authority URLs only).
    fn prefix(&self, with_userinfo: bool) -> String {
        let mut s = String::with_capacity(self.ser.len());
        s.push_str(&self.scheme);
        s.push_str("://");
        if with_userinfo && (!self.username.is_empty() || !self.password.is_empty()) {
            s.push_str(&self.username);
            if !self.password.is_empty() {
                s.push(':');
                s.push_str(&self.password);
            }
            s.push('@');
        }
        s.push_str(&self.host_port());
        s
    }

    fn rebuild(&mut self) {
        let mut s = if self.authority {
            self.prefix(true)
        } else {
            format!("{}:", self.scheme)
        };
        s.push_str(&self.path);
        if let Some(q) = &self.query {
            s.push('?');
            s.push_str(q);
        }
        self.frag_start = s.len();
        if let Some(f) = &self.fragment {
            s.push('#');
            s.push_str(f);
        }
        self.ser = s;
    }

    /// Resolves `reference` against this URL (RFC 3986 §5.2).
    pub fn join(&self, reference: &str) -> Result<Url, UrlError> {
        let mut r = clean(reference);
        if let Some(colon) = scheme_end(&r) {
            let sch = r[..colon].to_ascii_lowercase();
            let after = &r[colon + 1..];
            let same_special = sch == self.scheme
                && is_special(&sch)
                && !after.starts_with(['/', '\\']);
            if !same_special {
                return Url::parse(&r);
            }
            r = after.to_string();
        }
        if !self.authority {
            return if r.is_empty() {
                Url::parse(self.without_fragment())
            } else if r.starts_with('#') {
                Url::parse(&format!("{}{}", self.without_fragment(), r))
            } else {
                Err(UrlError::CannotBeABase)
            };
        }
        let special = is_special(&self.scheme);
        let is_slash = |c: char| c == '/' || (special && c == '\\');
        let mut chars = r.chars();
        let first = chars.next();
        let second = chars.next();
        let target = match first {
            None => self.without_fragment().to_string(),
            Some(a) if is_slash(a) && second.is_some_and(is_slash) => {
                format!("{}:{}", self.scheme, r)
            }
            Some('#') => format!("{}{}", self.without_fragment(), r),
            Some('?') => format!("{}{}{}", self.prefix(true), self.path, r),
            Some(a) if is_slash(a) => format!("{}{}", self.prefix(true), r),
            Some(_) => {
                let dir = match self.path.rfind('/') {
                    Some(i) => &self.path[..=i],
                    None => "/",
                };
                format!("{}{}{}", self.prefix(true), dir, r)
            }
        };
        Url::parse(&target)
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> Option<u16> {
        self.port
    }
    pub fn port_or_default(&self) -> u16 {
        self.port.or_else(|| default_port(&self.scheme)).unwrap_or(0)
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
    pub fn as_str(&self) -> &str {
        &self.ser
    }
    pub fn without_fragment(&self) -> &str {
        &self.ser[..self.frag_start]
    }
    pub fn origin(&self) -> String {
        if self.host.is_empty() {
            "null".to_string()
        } else {
            self.prefix(false)
        }
    }
    pub fn is_http(&self) -> bool {
        matches!(self.scheme.as_str(), "http" | "https")
    }
    pub fn is_secure(&self) -> bool {
        self.scheme == "https"
    }
    pub fn set_scheme(&mut self, scheme: &str) {
        let s = scheme.to_ascii_lowercase();
        if scheme_end(&format!("{s}:")) != Some(s.len()) {
            return;
        }
        self.scheme = s;
        if self.port.is_some() && self.port == default_port(&self.scheme) {
            self.port = None;
        }
        self.rebuild();
    }
    pub fn set_query(&mut self, query: Option<&str>) {
        let special = is_special(&self.scheme);
        self.query = query.map(|q| encode(q, |c| query_needs(c, special)));
        self.rebuild();
    }
    pub fn set_fragment(&mut self, fragment: Option<&str>) {
        self.fragment = fragment.map(|f| encode(f, fragment_needs));
        self.rebuild();
    }

    /// Whether the URL carries `user[:password]@`.
    pub(crate) fn has_userinfo(&self) -> bool {
        !self.username.is_empty() || !self.password.is_empty()
    }
    /// `host[:port]` as sent in the `Host` header.
    pub(crate) fn host_port(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{}", self.host, p),
            None => self.host.clone(),
        }
    }
    /// Path plus query: the HTTP request-target.
    pub(crate) fn request_target(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{}", self.path, q),
            None => self.path.clone(),
        }
    }
    /// Full URL without credentials or fragment, as used for `Referer`.
    pub(crate) fn referrer_string(&self) -> String {
        format!("{}{}", self.prefix(false), self.request_target())
    }
}

/// Decodes `%XX` escapes; malformed escapes are kept literally.
pub fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `application/x-www-form-urlencoded` serialisation.
pub fn form_urlencode(pairs: &[(String, String)]) -> String {
    fn enc(out: &mut String, s: &str) {
        for &b in s.as_bytes() {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => push_pct(out, b),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        enc(&mut out, k);
        out.push('=');
        enc(&mut out, v);
    }
    out
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    if data.len() % 4 == 0 {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    for chunk in data.chunks(4) {
        let mut acc = 0u32;
        for &c in chunk {
            acc = acc << 6 | val(c)?;
        }
        acc <<= 6 * (4 - chunk.len() as u32);
        let bytes = [(acc >> 16) as u8, (acc >> 8) as u8, acc as u8];
        out.extend_from_slice(&bytes[..chunk.len() - 1]);
    }
    Some(out)
}

/// Parses a `data:` URL into (lower-case MIME type, decoded bytes).
pub fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let s = clean(url);
    if s.len() < 5 || !s[..5].eq_ignore_ascii_case("data:") {
        return None;
    }
    let rest = &s[5..];
    let rest = rest.split('#').next().unwrap_or(rest);
    let comma = rest.find(',')?;
    let meta = rest[..comma].trim();
    let (mime, b64) = match meta.rfind(';') {
        Some(i) if meta[i + 1..].trim().eq_ignore_ascii_case("base64") => (meta[..i].trim(), true),
        _ => (meta, false),
    };
    let raw = percent_decode(&rest[comma + 1..]);
    let body = if b64 { base64_decode(&raw)? } else { raw };
    let mime = if mime.is_empty() {
        "text/plain;charset=us-ascii".to_string()
    } else if mime.starts_with(';') {
        format!("text/plain{}", mime.to_ascii_lowercase())
    } else {
        mime.to_ascii_lowercase()
    };
    Some((mime, body))
}
