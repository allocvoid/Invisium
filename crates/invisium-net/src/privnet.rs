//! Private-network protection: a page on the public internet may not reach loopback, private
//! or link-local targets. Pure classification of hosts and addresses; `service.rs` applies it
//! to every hop and `connect.rs` to the addresses a name resolved to (DNS rebinding).

use crate::types::{BlockReason, NetError};
use crate::url::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub(crate) fn v4_is_local(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 0 // "this network", including 0.0.0.0
        || o[0] == 127 // loopback
        || o[0] == 10 // private
        || (o[0] == 172 && o[1] & 0xf0 == 16)
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254) // link-local
        || (o[0] == 100 && o[1] & 0xc0 == 64) // carrier-grade NAT shared space
        || o == [255; 4]
}

/// The IPv4 address an IPv6 address stands for, if any: IPv4-mapped, the deprecated
/// IPv4-compatible form, NAT64 (64:ff9b::/96) and 6to4 (2002::/16).
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[..5] == [0; 5] && s[5] == 0xffff {
        return Some(v4(s[6], s[7]));
    }
    if s[..6] == [0; 6] && (s[6] != 0 || s[7] > 1) {
        return Some(v4(s[6], s[7]));
    }
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return Some(v4(s[6], s[7]));
    }
    (s[0] == 0x2002).then(|| v4(s[1], s[2]))
}

pub(crate) fn v6_is_local(ip: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(ip) {
        return v4_is_local(v4);
    }
    let first = ip.segments()[0];
    ip.is_unspecified()
        || ip.is_loopback()
        || first & 0xfe00 == 0xfc00 // unique local fc00::/7
        || first & 0xffc0 == 0xfe80 // link-local fe80::/10
        || first & 0xffc0 == 0xfec0 // site-local (deprecated)
}

pub(crate) fn ip_is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_local(v4),
        IpAddr::V6(v6) => v6_is_local(v6),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Loose {
    Addr(Ipv4Addr),
    /// Ends in a number but is not a valid IPv4 address in any notation.
    Invalid,
    Name,
}

/// One dotted part: decimal, octal (leading 0) or hexadecimal (0x), as resolvers accept them.
fn parse_part(p: &str) -> Option<u64> {
    let (digits, radix) = match p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None if p.len() > 1 && p.starts_with('0') => (&p[1..], 8),
        None => (p, 10),
    };
    if p.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.len() > 11 {
        return None;
    }
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, radix).ok()
}

/// IPv4 in every notation a resolver may accept: `127.0.0.1`, `127.1`, `0x7f.1`, `2130706433`,
/// `017700000001` (the WHATWG IPv4 parser).
fn loose_ipv4(host: &str) -> Loose {
    let parts: Vec<&str> = host.split('.').collect();
    let last = parts.last().copied().unwrap_or("").as_bytes();
    let numeric = (!last.is_empty() && last.iter().all(u8::is_ascii_digit))
        || (last.len() >= 2
            && last[0] == b'0'
            && last[1] | 0x20 == b'x'
            && last[2..].iter().all(u8::is_ascii_hexdigit));
    if !numeric {
        return Loose::Name;
    }
    if parts.len() > 4 {
        return Loose::Invalid;
    }
    let mut nums = Vec::with_capacity(4);
    for p in &parts {
        match parse_part(p) {
            Some(n) => nums.push(n),
            None => return Loose::Invalid,
        }
    }
    let Some((&tail, rest)) = nums.split_last() else { return Loose::Invalid };
    if rest.iter().any(|&n| n > 255) || tail >= 256u64.pow(4 - rest.len() as u32) {
        return Loose::Invalid;
    }
    let value = rest.iter().enumerate().fold(tail, |v, (i, n)| v + (n << (8 * (3 - i))));
    Loose::Addr(Ipv4Addr::from(value as u32))
}

/// Whether a URL host is local by itself: `localhost`, `*.localhost`, or an IP literal (in any
/// notation) that is loopback, private, link-local or unspecified. `None` when the host looks
/// like an address but cannot be understood.
fn classify(host: &str) -> Option<bool> {
    let h = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if h.is_empty() {
        return Some(false);
    }
    if let Some(inner) = h.strip_prefix('[') {
        return inner.strip_suffix(']')?.parse::<Ipv6Addr>().ok().map(v6_is_local);
    }
    if h == "localhost" || h.ends_with(".localhost") {
        return Some(true);
    }
    match loose_ipv4(&h) {
        Loose::Addr(ip) => Some(v4_is_local(ip)),
        Loose::Invalid => None,
        Loose::Name => Some(false),
    }
}

/// Whether a request's host is a local target. A host that cannot be understood counts as
/// local: refused rather than guessed.
pub(crate) fn host_is_local(host: &str) -> bool {
    classify(host).unwrap_or(true)
}

/// Whether requests made for this page must be kept away from local targets: every request
/// that is not itself a top-level navigation, unless the page's own URL is certainly local.
pub(crate) fn guards(top_level: Option<&Url>) -> bool {
    top_level.is_some_and(|page| classify(page.host()) != Some(true))
}

/// `guards` for a request made by a document (`initiator`, a frame) inside a tab (`top_level`):
/// a local target is reachable only if both are local. A document is local if its host is a
/// local address literal or its flag says it was itself served from a local address
/// (`Request::top_local` / `initiator_local`). Nothing about pages is remembered here.
pub(crate) fn guards_for(
    top_level: Option<&Url>,
    top_local: bool,
    initiator: Option<&Url>,
    initiator_local: bool,
) -> bool {
    let guarded = |page: &Url, local: bool| !local && guards(Some(page));
    top_level.is_some_and(|top| guarded(top, top_local) || initiator.is_some_and(|i| guarded(i, initiator_local)))
}

fn refused() -> NetError {
    NetError::Blocked(BlockReason::PrivateNetwork)
}

/// The test on the URL host, before anything is sent.
pub(crate) fn check_host(guard: bool, host: &str) -> Result<(), NetError> {
    if guard && host_is_local(host) {
        return Err(refused());
    }
    Ok(())
}

/// The test on the addresses a name resolved to, before connecting (DNS rebinding). One local
/// address among them is enough to refuse.
pub(crate) fn check_resolved(guard: bool, addrs: &[SocketAddr]) -> Result<(), NetError> {
    if guard && addrs.iter().any(|a| ip_is_local(a.ip())) {
        return Err(refused());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_public_hosts() {
        let local = [
            "localhost", "LOCALHOST", "localhost.", "app.localhost", "a.b.localhost.", "127.0.0.1", "127.255.0.9",
            "127.0.0.1.", "10.0.0.5", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.1.1", "0.0.0.0",
            "0.1.2.3", "100.64.0.1", "100.127.255.255", "255.255.255.255", "[::1]", "[::]", "[fc00::1]",
            "[fd12:3456::1]", "[fe80::1]", "[febf::1]", "[fec0::1]", "[::ffff:127.0.0.1]", "[::ffff:10.0.0.5]",
            "[::ffff:c0a8:101]", "[::127.0.0.1]", "[64:ff9b::7f00:1]", "[2002:7f00:1::1]", "[2002:c0a8:101::]",
            // Notations resolvers accept for 127.0.0.1 / 10.0.0.5 / 192.168.1.1.
            "127.1", "0x7f.1", "0x7f000001", "2130706433", "017700000001", "0177.0.0.01", "10.5", "0xc0a80101",
            "0X7F.0.0.1", "0", "0x",
            // Looks like an address but is not one: refused rather than guessed.
            "1.2.3.4.5", "256.1.1.1", "1.2.3.256", "09.0.0.1", "4294967296", "1..2", "x127.0.0.1", "[::1", "[zz]",
            "0é.1", "é.0x1", "99999999999999999999999999", "0x99999999999999999999999999", "[", "[]", "..1",
        ];
        for h in local {
            assert!(host_is_local(h), "{h} must be local");
        }
        let public = [
            "example.com", "localhost.example.com", "notlocalhost", "mylocalhost.net", "8.8.8.8", "1.1.1.1",
            "172.15.0.1", "172.32.0.1", "192.167.1.1", "169.253.1.1", "100.63.0.1", "100.128.0.1", "11.0.0.1",
            "203.0.113.10", "0x08080808", "134744072", "[2001:db8::1]", "[2606:4700::1111]", "[::ffff:8.8.8.8]",
            "[64:ff9b::808:808]", "[2002:808:808::1]", "10.example", "127.0.0.1.example.com", "",
            "0xé", "1.é", "٣.٣", ".", "..",
        ];
        for h in public {
            assert!(!host_is_local(h), "{h} must be public");
        }
    }

    #[test]
    fn loose_ipv4_values() {
        let addr = |a, b, c, d| Loose::Addr(Ipv4Addr::new(a, b, c, d));
        assert_eq!(loose_ipv4("1.2.3.4"), addr(1, 2, 3, 4));
        assert_eq!(loose_ipv4("1.2.772"), addr(1, 2, 3, 4));
        assert_eq!(loose_ipv4("1.131844"), addr(1, 2, 3, 4));
        assert_eq!(loose_ipv4("16909060"), addr(1, 2, 3, 4));
        assert_eq!(loose_ipv4("0x1.02.0x304"), addr(1, 2, 3, 4));
        assert_eq!(loose_ipv4("00000000000000000000177.1"), addr(127, 0, 0, 1));
        assert_eq!(loose_ipv4("4294967295"), addr(255, 255, 255, 255));
        assert_eq!(loose_ipv4("1.2.65536"), Loose::Invalid);
        assert_eq!(loose_ipv4("0x100000000"), Loose::Invalid);
        assert_eq!(loose_ipv4("1.0xg"), Loose::Name);
        assert_eq!(loose_ipv4("example.com"), Loose::Name);
        assert_eq!(loose_ipv4("1.2.3.com"), Loose::Name);
    }

    #[test]
    fn guard_and_checks() {
        let u = |s: &str| Url::parse(s).unwrap();
        // Navigations (no top-level page) and local pages are not guarded; everything else is.
        assert!(!guards(None));
        for page in ["http://localhost:8080/", "http://127.0.0.1/", "https://[::1]/", "http://192.168.1.1/app", "http://dev.localhost/"] {
            assert!(!guards(Some(&u(page))), "{page}");
        }
        let public = [
            "https://example.com/", "http://8.8.8.8/", "http://localhost.example.com/", "file:///C:/page.html",
            "data:text/html,x", "about:blank",
            // A page whose host cannot be understood is not trusted to be local either.
            "http://1.2.3.4.5/", "http://256.0.0.1/",
        ];
        for page in public {
            assert!(guards(Some(&u(page))), "{page}");
        }
        let blocked = Err(NetError::Blocked(BlockReason::PrivateNetwork));
        assert_eq!(check_host(true, "192.168.1.1"), blocked);
        assert_eq!(check_host(true, "example.com"), Ok(()));
        assert_eq!(check_host(false, "192.168.1.1"), Ok(()));
        // Resolved addresses: a public page and a name that resolves to a local address.
        let sock = |s: &str| s.parse::<SocketAddr>().unwrap();
        let (public, local, local6) = (sock("93.184.216.34:80"), sock("127.0.0.1:80"), sock("[fd00::1]:80"));
        assert_eq!(check_resolved(true, &[public]), Ok(()));
        assert_eq!(check_resolved(true, &[local]), blocked);
        assert_eq!(check_resolved(true, &[public, local6]), blocked);
        assert_eq!(check_resolved(true, &[sock("[::ffff:10.0.0.1]:443")]), blocked);
        assert_eq!(check_resolved(false, &[local, local6]), Ok(()));
        assert_eq!(check_resolved(true, &[]), Ok(()));
    }
}
