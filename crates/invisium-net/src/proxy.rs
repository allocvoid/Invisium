//! Proxy handshakes over an already-connected stream: HTTP CONNECT and SOCKS5 (RFC 1928/1929).
//! The target hostname is always handed to the proxy; it is never resolved locally.

use crate::types::NetError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn perr(msg: impl Into<String>) -> NetError {
    NetError::Proxy(msg.into())
}

pub(crate) fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Host as written in a CONNECT authority / SOCKS request (IPv6 keeps its brackets for HTTP).
fn bare(host: &str) -> &str {
    host.trim_start_matches('[').trim_end_matches(']')
}

/// Reads a CONNECT response head byte by byte so no tunnelled bytes are consumed.
async fn read_connect_head<S: AsyncRead + Unpin>(s: &mut S) -> Result<Vec<u8>, NetError> {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() > 16 * 1024 {
            return Err(perr("proxy response head too large"));
        }
        let n = s.read(&mut b).await.map_err(|e| perr(format!("proxy read failed: {e}")))?;
        if n == 0 {
            return Err(perr("proxy closed the connection during CONNECT"));
        }
        head.push(b[0]);
    }
    Ok(head)
}

pub(crate) async fn http_connect<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    host: &str,
    port: u16,
    auth: Option<&(String, String)>,
) -> Result<(), NetError> {
    let authority = format!("{host}:{port}");
    let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some((u, p)) = auth {
        req.push_str("Proxy-Authorization: Basic ");
        req.push_str(&base64_encode(format!("{u}:{p}").as_bytes()));
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.map_err(|e| perr(format!("proxy write failed: {e}")))?;
    s.flush().await.map_err(|e| perr(format!("proxy write failed: {e}")))?;
    let head = read_connect_head(s).await?;
    let line = head.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let ok_line = line.len() >= 12 && line.starts_with(b"HTTP/1.") && line[8] == b' ';
    if !ok_line || !line[9..12].iter().all(u8::is_ascii_digit) {
        return Err(perr("malformed CONNECT response"));
    }
    match &line[9..12] {
        b"200" => Ok(()),
        b"407" => Err(perr("proxy authentication required")),
        code => Err(perr(format!("proxy refused CONNECT with status {}", String::from_utf8_lossy(code)))),
    }
}

async fn read_n<S: AsyncRead + Unpin>(s: &mut S, n: usize) -> Result<Vec<u8>, NetError> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).await.map_err(|e| perr(format!("SOCKS5 read failed: {e}")))?;
    Ok(buf)
}

fn socks_reply_text(code: u8) -> &'static str {
    match code {
        1 => "general failure",
        2 => "connection not allowed by ruleset",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused",
        6 => "TTL expired",
        7 => "command not supported",
        8 => "address type not supported",
        _ => "unknown error",
    }
}

pub(crate) async fn socks5_connect<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    host: &str,
    port: u16,
    auth: Option<&(String, String)>,
) -> Result<(), NetError> {
    let w = |e: std::io::Error| perr(format!("SOCKS5 write failed: {e}"));
    let greeting: &[u8] = if auth.is_some() { &[5, 2, 0, 2] } else { &[5, 1, 0] };
    s.write_all(greeting).await.map_err(w)?;
    let choice = read_n(s, 2).await?;
    if choice[0] != 5 {
        return Err(perr("not a SOCKS5 proxy"));
    }
    match (choice[1], auth) {
        (0, _) => {}
        (2, Some((u, p))) => {
            if u.len() > 255 || p.len() > 255 {
                return Err(perr("SOCKS5 credentials too long"));
            }
            let mut m = vec![1, u.len() as u8];
            m.extend_from_slice(u.as_bytes());
            m.push(p.len() as u8);
            m.extend_from_slice(p.as_bytes());
            s.write_all(&m).await.map_err(w)?;
            let r = read_n(s, 2).await?;
            if r[1] != 0 {
                return Err(perr("SOCKS5 authentication failed"));
            }
        }
        _ => return Err(perr("SOCKS5 proxy offered no acceptable authentication method")),
    }
    let h = bare(host);
    let mut req = vec![5, 1, 0];
    if let Ok(ip) = h.parse::<std::net::Ipv4Addr>() {
        req.push(1);
        req.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = h.parse::<std::net::Ipv6Addr>() {
        req.push(4);
        req.extend_from_slice(&ip.octets());
    } else {
        if h.is_empty() || h.len() > 255 {
            return Err(perr("hostname too long for SOCKS5"));
        }
        req.push(3);
        req.push(h.len() as u8);
        req.extend_from_slice(h.as_bytes());
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.map_err(w)?;
    let rep = read_n(s, 4).await?;
    if rep[0] != 5 {
        return Err(perr("malformed SOCKS5 reply"));
    }
    if rep[1] != 0 {
        return Err(perr(format!("SOCKS5 connect failed: {}", socks_reply_text(rep[1]))));
    }
    let addr_len = match rep[3] {
        1 => 4,
        4 => 16,
        3 => read_n(s, 1).await?[0] as usize,
        _ => return Err(perr("malformed SOCKS5 reply address")),
    };
    read_n(s, addr_len + 2).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b""), "");
    }
}
