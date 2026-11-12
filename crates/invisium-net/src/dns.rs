//! Hand-written DNS wire format (RFC 1035): query encoding and bounds-checked response decoding.

use crate::types::NetError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) const TYPE_A: u16 = 1;
pub(crate) const TYPE_CNAME: u16 = 5;
pub(crate) const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;
const MAX_JUMPS: usize = 16;

fn derr(msg: &str) -> NetError {
    NetError::Dns(msg.to_string())
}

/// Encodes a recursive query with ID 0 (RFC 8484 recommends 0 for DoH cache friendliness).
pub(crate) fn encode_query(name: &str, qtype: u16) -> Result<Vec<u8>, NetError> {
    let name = name.trim_end_matches('.');
    if name.is_empty() || name.len() > 253 {
        return Err(derr("invalid DNS name"));
    }
    let mut q = vec![0, 0, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 || !label.is_ascii() {
            return Err(derr("invalid DNS label"));
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(q)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Answer {
    pub addrs: Vec<IpAddr>,
    pub cnames: Vec<String>,
    pub min_ttl: u32,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, NetError> {
    b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or_else(|| derr("truncated DNS message"))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, NetError> {
    b.get(i..i + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| derr("truncated DNS message"))
}

/// Reads a (possibly compressed) name at `pos`; returns it and the position after it.
fn read_name(b: &[u8], mut pos: usize) -> Result<(String, usize), NetError> {
    let mut name = String::new();
    let mut end: Option<usize> = None;
    let mut jumps = 0;
    loop {
        let len = *b.get(pos).ok_or_else(|| derr("truncated DNS name"))?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    return Ok((name, end.unwrap_or(pos + 1)));
                }
                let label = b.get(pos + 1..pos + 1 + len as usize).ok_or_else(|| derr("truncated DNS label"))?;
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(&String::from_utf8_lossy(label));
                if name.len() > 255 {
                    return Err(derr("DNS name too long"));
                }
                pos += 1 + len as usize;
            }
            0xC0 => {
                let lo = *b.get(pos + 1).ok_or_else(|| derr("truncated DNS pointer"))?;
                jumps += 1;
                if jumps > MAX_JUMPS {
                    return Err(derr("DNS compression loop"));
                }
                end.get_or_insert(pos + 2);
                let target = ((len as usize & 0x3F) << 8) | lo as usize;
                if target >= b.len() {
                    return Err(derr("DNS pointer out of range"));
                }
                pos = target;
            }
            _ => return Err(derr("unsupported DNS label type")),
        }
    }
}

/// True when `b` is a complete (QR set, not truncated) response whose RCODE is NXDOMAIN: the
/// resolver's definitive statement that the name does not exist, for any record type.
pub(crate) fn is_nxdomain(b: &[u8]) -> bool {
    b.len() >= 12 && b[2] & 0x80 != 0 && b[2] & 0x02 == 0 && b[3] & 0x0F == 3
}

/// Decodes a response, collecting addresses of the requested type and any CNAMEs.
pub(crate) fn decode_response(b: &[u8], qtype: u16) -> Result<Answer, NetError> {
    if b.len() < 12 {
        return Err(derr("truncated DNS header"));
    }
    let flags = u16_at(b, 2)?;
    if flags & 0x8000 == 0 {
        return Err(derr("DNS message is not a response"));
    }
    if flags & 0x0200 != 0 {
        return Err(derr("truncated DNS response (TC)"));
    }
    match flags & 0x000F {
        0 => {}
        3 => return Err(derr("no such domain (NXDOMAIN)")),
        2 => return Err(derr("DNS server failure (SERVFAIL)")),
        5 => return Err(derr("DNS query refused")),
        _ => return Err(derr("DNS error response")),
    }
    let qd = u16_at(b, 4)? as usize;
    let an = u16_at(b, 6)? as usize;
    let mut pos = 12;
    for _ in 0..qd {
        pos = read_name(b, pos)?.1 + 4;
    }
    let mut out = Answer { min_ttl: u32::MAX, ..Answer::default() };
    for _ in 0..an {
        let (_, p) = read_name(b, pos)?;
        let rtype = u16_at(b, p)?;
        let class = u16_at(b, p + 2)?;
        let ttl = u32_at(b, p + 4)?;
        let rdlen = u16_at(b, p + 8)? as usize;
        let rd = p + 10;
        let rdata = b.get(rd..rd + rdlen).ok_or_else(|| derr("truncated DNS record"))?;
        if class == CLASS_IN {
            match rtype {
                TYPE_A if qtype == TYPE_A && rdlen == 4 => {
                    out.addrs.push(IpAddr::V4(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3])));
                    out.min_ttl = out.min_ttl.min(ttl);
                }
                TYPE_AAAA if qtype == TYPE_AAAA && rdlen == 16 => {
                    let mut o = [0u8; 16];
                    o.copy_from_slice(rdata);
                    out.addrs.push(IpAddr::V6(Ipv6Addr::from(o)));
                    out.min_ttl = out.min_ttl.min(ttl);
                }
                TYPE_CNAME => out.cnames.push(read_name(b, rd)?.0),
                _ => {}
            }
        }
        pos = rd + rdlen;
    }
    if out.addrs.is_empty() {
        out.min_ttl = 0;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query() {
        let q = encode_query("example.com", TYPE_A).unwrap();
        let want: &[u8] = b"\x00\x00\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01";
        assert_eq!(q, want);
        assert!(encode_query("", TYPE_A).is_err());
        assert!(encode_query(&format!("{}.com", "a".repeat(64)), TYPE_A).is_err());
    }

    /// Response to "www.example.com A": CNAME -> example.com (compressed), then A records.
    fn canned() -> Vec<u8> {
        let mut m = vec![0, 0, 0x81, 0x80, 0, 1, 0, 3, 0, 0, 0, 0];
        m.extend_from_slice(b"\x03www\x07example\x03com\x00\x00\x01\x00\x01"); // question at 12
        // CNAME: name ptr to 12, type 5, class 1, ttl 300, rdata = ptr to "example.com" (offset 16)
        m.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 1, 44, 0, 2, 0xC0, 16]);
        // A: name ptr to 16, ttl 60, 93.184.216.34
        m.extend_from_slice(&[0xC0, 16, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
        // AAAA (ignored for an A query), ttl 30
        m.extend_from_slice(&[0xC0, 16, 0, 28, 0, 1, 0, 0, 0, 30, 0, 16]);
        m.extend_from_slice(&[0x26, 0x06, 0x28, 0, 0x02, 0x20, 0, 1, 0x02, 0x48, 0x18, 0x93, 0x25, 0xc8, 0x19, 0x46]);
        m
    }

    #[test]
    fn decodes_a_aaaa_cname_with_compression() {
        let a = decode_response(&canned(), TYPE_A).unwrap();
        assert_eq!(a.addrs, vec!["93.184.216.34".parse::<IpAddr>().unwrap()]);
        assert_eq!(a.cnames, vec!["example.com".to_string()]);
        assert_eq!(a.min_ttl, 60);
        let aaaa = decode_response(&canned(), TYPE_AAAA).unwrap();
        assert_eq!(aaaa.addrs, vec!["2606:2800:220:1:248:1893:25c8:1946".parse::<IpAddr>().unwrap()]);
        assert_eq!(aaaa.min_ttl, 30);
    }

    #[test]
    fn recognises_nxdomain_only_in_complete_responses() {
        let with = |hi: u8, lo: u8| {
            let mut m = canned();
            (m[2], m[3]) = (hi, lo);
            m
        };
        assert!(is_nxdomain(&with(0x81, 0x83)));
        assert!(is_nxdomain(&with(0x81, 0x83)[..12]));
        assert!(!is_nxdomain(&canned()), "NOERROR");
        assert!(!is_nxdomain(&with(0x81, 0x82)), "SERVFAIL");
        assert!(!is_nxdomain(&with(0x81, 0x85)), "REFUSED");
        assert!(!is_nxdomain(&with(0x83, 0x83)), "truncated (TC)");
        assert!(!is_nxdomain(&with(0x01, 0x83)), "not a response");
        assert!(!is_nxdomain(&with(0x81, 0x83)[..11]), "short header");
        assert!(!is_nxdomain(b""));
    }

    #[test]
    fn malformed_packets_are_errors_not_panics() {
        let good = canned();
        let mut nx = good.clone();
        nx[3] = 0x83;
        let mut looped = vec![0, 0, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0];
        looped.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1]);
        let mut oob = vec![0, 0, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0];
        oob.extend_from_slice(&[0xC0, 200, 0, 1, 0, 1]);
        let mut query = good.clone();
        query[2] = 0x01;
        for bad in [&b""[..], &good[..11], &good[..40], &good[..good.len() - 3], &nx, &looped, &oob, &query] {
            assert!(decode_response(bad, TYPE_A).is_err());
        }
        let mut seed: u64 = 42;
        for _ in 0..5000 {
            let mut m = good.clone();
            for _ in 0..3 {
                if m.is_empty() {
                    break;
                }
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let i = (seed >> 33) as usize % m.len();
                if seed & 1 == 0 {
                    m[i] = (seed >> 9) as u8;
                } else {
                    m.truncate(i);
                }
            }
            let _ = decode_response(&m, TYPE_A);
        }
    }
}
