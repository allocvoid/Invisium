//! Memory-only cookie jar (RFC 6265 subset), partitioned by top-level site.

use crate::psl::is_public_suffix;
use crate::url::Url;
use std::collections::HashMap;

const MAX_PER_PARTITION: usize = 1000;
const MAX_COOKIE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Clone, Debug)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    expires: Option<i64>,
    secure: bool,
    /// Hidden from, and untouchable by, page scripts; the network layer always sends it.
    http_only: bool,
    same_site: SameSite,
    seq: u64,
}

/// Context of the request a cookie is being sent with / received from.
pub(crate) struct Ctx<'a> {
    pub partition: &'a str,
    pub url: &'a Url,
    pub cross_site: bool,
    pub top_level_get: bool,
    pub now: i64,
}

#[derive(Default)]
pub(crate) struct CookieJar {
    parts: HashMap<String, Vec<Cookie>>,
    seq: u64,
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn domain_match(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

fn path_match(req: &str, cp: &str) -> bool {
    req == cp || (req.starts_with(cp) && (cp.ends_with('/') || req[cp.len()..].starts_with('/')))
}

fn default_path(url: &Url) -> String {
    let p = url.path();
    match p.rfind('/') {
        Some(i) if p.starts_with('/') && i > 0 => p[..i].to_string(),
        _ => "/".to_string(),
    }
}

/// https, or a loopback host (a "potentially trustworthy" origin, as in browsers).
fn trustworthy(url: &Url) -> bool {
    let h = url.host();
    url.is_secure()
        || h == "localhost"
        || h.ends_with(".localhost")
        || h == "[::1]"
        || h.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

impl CookieJar {
    pub fn clear(&mut self) {
        self.parts.clear();
    }

    pub fn count(&self) -> usize {
        self.parts.values().map(Vec::len).sum()
    }

    /// Stores a cookie received in a `Set-Cookie` header.
    pub fn store(&mut self, ctx: &Ctx<'_>, header: &str) {
        self.store_as(ctx, header, false);
    }

    /// Stores a cookie written through `document.cookie`: the same rules as `store`, except
    /// that a script can neither create an `HttpOnly` cookie nor replace or delete one.
    pub fn store_from_script(&mut self, ctx: &Ctx<'_>, cookie: &str) {
        self.store_as(ctx, cookie, true);
    }

    fn store_as(&mut self, ctx: &Ctx<'_>, header: &str, from_script: bool) {
        // Control characters would end up in a later `Cookie` request header.
        if header.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return;
        }
        let mut parts = header.split(';');
        let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')) else { return };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || name.len() + value.len() > MAX_COOKIE_BYTES {
            return;
        }
        let host = ctx.url.host();
        let mut c = Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain: host.to_string(),
            host_only: true,
            path: default_path(ctx.url),
            expires: None,
            secure: false,
            http_only: false,
            same_site: SameSite::Lax,
            seq: 0,
        };
        let mut max_age: Option<i64> = None;
        for attr in parts {
            let (k, v) = attr.split_once('=').unwrap_or((attr, ""));
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "expires" => {
                    if let Some(t) = parse_cookie_date(v) {
                        c.expires = Some(t);
                    }
                }
                "max-age" => {
                    if let Ok(n) = v.parse::<i64>() {
                        max_age = Some(n);
                    }
                }
                "domain" if !v.is_empty() => {
                    let d = v.trim_start_matches('.').to_ascii_lowercase();
                    if is_ip(host) && d != host {
                        return;
                    }
                    if is_public_suffix(&d) && d != host {
                        return;
                    }
                    if !domain_match(host, &d) {
                        return;
                    }
                    c.host_only = d == host;
                    c.domain = d;
                }
                "path" if v.starts_with('/') => c.path = v.to_string(),
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "samesite" => {
                    c.same_site = match v.to_ascii_lowercase().as_str() {
                        "strict" => SameSite::Strict,
                        "none" => SameSite::None,
                        _ => SameSite::Lax,
                    }
                }
                _ => {}
            }
        }
        if let Some(n) = max_age {
            c.expires = Some(if n <= 0 { i64::MIN } else { ctx.now.saturating_add(n) });
        }
        let secure_origin = trustworthy(ctx.url);
        if (c.secure && !secure_origin) || (c.same_site == SameSite::None && !c.secure) {
            return;
        }
        if (c.name.starts_with("__Secure-") || c.name.starts_with("__Host-")) && !c.secure {
            return;
        }
        if c.name.starts_with("__Host-") && (!c.host_only || c.path != "/") {
            return;
        }
        let list = self.parts.entry(ctx.partition.to_string()).or_default();
        let existing = list
            .iter()
            .position(|o| o.name == c.name && o.domain == c.domain && o.path == c.path);
        if from_script {
            // No HttpOnly cookie is created, replaced or deleted from script, and a document
            // embedded cross-site cannot plant SameSite cookies.
            let protected = existing.is_some_and(|i| list[i].http_only);
            if c.http_only || protected || (ctx.cross_site && c.same_site != SameSite::None) {
                return;
            }
        }
        if let Some(i) = existing {
            if list[i].secure && !secure_origin {
                return;
            }
            list.remove(i);
        }
        if c.expires.is_some_and(|e| e <= ctx.now) {
            return;
        }
        self.seq += 1;
        c.seq = self.seq;
        if list.len() >= MAX_PER_PARTITION {
            list.remove(0);
        }
        list.push(c);
    }

    /// The `Cookie` header value for a request, if any cookie applies.
    pub fn header_for(&mut self, ctx: &Ctx<'_>) -> Option<String> {
        self.matching(ctx, true)
    }

    /// What `document.cookie` reads: the cookies that apply, without the `HttpOnly` ones.
    pub fn script_string(&mut self, ctx: &Ctx<'_>) -> String {
        self.matching(ctx, false).unwrap_or_default()
    }

    fn matching(&mut self, ctx: &Ctx<'_>, with_http_only: bool) -> Option<String> {
        let list = self.parts.get_mut(ctx.partition)?;
        list.retain(|c| c.expires.is_none_or(|e| e > ctx.now));
        let host = ctx.url.host();
        let mut hits: Vec<&Cookie> = list
            .iter()
            .filter(|c| with_http_only || !c.http_only)
            .filter(|c| if c.host_only { host == c.domain } else { domain_match(host, &c.domain) })
            .filter(|c| path_match(ctx.url.path(), &c.path))
            .filter(|c| !c.secure || trustworthy(ctx.url))
            .filter(|c| match c.same_site {
                SameSite::None => true,
                SameSite::Lax => !ctx.cross_site || ctx.top_level_get,
                SameSite::Strict => !ctx.cross_site,
            })
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.seq.cmp(&b.seq)));
        let parts: Vec<String> = hits.iter().map(|c| format!("{}={}", c.name, c.value)).collect();
        Some(parts.join("; "))
    }
}

/// RFC 6265 §5.1.1 cookie-date parsing to Unix seconds.
fn parse_cookie_date(s: &str) -> Option<i64> {
    let is_delim = |c: char| {
        c == '\t'
            || (' '..='/').contains(&c)
            || (';'..='@').contains(&c)
            || ('['..='`').contains(&c)
            || ('{'..='~').contains(&c)
    };
    let digits = |t: &str, lo: usize, hi: usize| {
        (lo..=hi).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit())
    };
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for tok in s.split(is_delim).filter(|t| !t.is_empty()) {
        if time.is_none() {
            let p: Vec<&str> = tok.split(':').collect();
            if p.len() == 3 && p.iter().all(|x| digits(x, 1, 2)) {
                let v: Vec<i64> = p.iter().filter_map(|x| x.parse().ok()).collect();
                time = Some((v[0], v[1], v[2]));
                continue;
            }
        }
        if day.is_none() && digits(tok, 1, 2) {
            day = tok.parse::<i64>().ok();
            continue;
        }
        if month.is_none() {
            const M: [&str; 12] =
                ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
            if let Some(m) = tok.get(..3).and_then(|p| {
                M.iter().position(|x| x.eq_ignore_ascii_case(p))
            }) {
                month = Some(m as i64 + 1);
                continue;
            }
        }
        if year.is_none() && digits(tok, 2, 4) {
            year = tok.parse::<i64>().ok();
        }
    }
    let ((h, mi, se), d, m, mut y) = (time?, day?, month?, year?);
    if (70..=99).contains(&y) {
        y += 1900;
    } else if y <= 69 {
        y += 2000;
    }
    if !(1..=31).contains(&d) || y < 1601 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_dates_and_attributes() {
        assert_eq!(parse_cookie_date("Thu, 01 Jan 1970 00:00:10 GMT"), Some(10));
        assert_eq!(parse_cookie_date("Sun, 06-Nov-94 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(parse_cookie_date("garbage"), None);
        let url = Url::parse("http://www.example.com/a/b").unwrap();
        let ctx = Ctx { partition: "example.com", url: &url, cross_site: false, top_level_get: true, now: 100 };
        let mut jar = CookieJar::default();
        jar.store(&ctx, "a=1; Domain=example.com; Path=/");
        jar.store(&ctx, "b=2"); // default path /a
        jar.store(&ctx, "c=3; Domain=com"); // public suffix: rejected
        jar.store(&ctx, "d=4; Secure"); // secure from http: rejected
        jar.store(&ctx, "e=5; Max-Age=0");
        jar.store(&ctx, "f=6; Expires=Thu, 01 Jan 1970 00:00:10 GMT");
        assert_eq!(jar.header_for(&ctx).as_deref(), Some("b=2; a=1"));
        let other = Url::parse("http://api.example.com/").unwrap();
        let ctx2 = Ctx { url: &other, ..ctx };
        assert_eq!(jar.header_for(&ctx2).as_deref(), Some("a=1"));
        assert_eq!(jar.count(), 2);
    }
}
