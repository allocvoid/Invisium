use crate::decode::decode_body;
use crate::http::{read_response, Reader};
use crate::types::{Headers, NetError};
use std::io::Write;
use std::time::Duration;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn parse(bytes: &[u8], max: usize) -> Result<(u16, Vec<u8>), NetError> {
    rt().block_on(async {
        let mut r = Reader::new(bytes, Duration::from_secs(2));
        read_response(&mut r, false, max).await.map(|x| (x.status, x.body))
    })
}

#[test]
fn parses_well_formed_bodies() {
    let ok = parse(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", 100);
    assert_eq!(ok, Ok((200, b"abc".to_vec())));
    let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;x=y\r\nabc\r\n0\r\nT: 1\r\n\r\n";
    assert_eq!(parse(chunked, 100), Ok((200, b"abc".to_vec())));
    assert_eq!(parse(b"HTTP/1.0 200 OK\n\nto close", 100), Ok((200, b"to close".to_vec())));
}

#[test]
fn malformed_responses_are_errors_not_panics() {
    let long_line = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(100_000));
    let many: String = (0..500).map(|i| format!("H{i}: v\r\n")).collect();
    let many = format!("HTTP/1.1 200 OK\r\n{many}\r\n");
    let cases: Vec<&[u8]> = vec![
        b"",
        b"garbage",
        b"garbage\r\n\r\n",
        b"HTTP/1.1\r\n\r\n",
        b"HTTP/1.1 abc OK\r\n\r\n",
        b"HTTP/1.1 20 OK\r\n\r\n",
        b"HTTP/1.1 099 Low\r\n\r\n",
        b"HTTP/2 200 OK\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n",
        b"HTTP/1.1 200 OK\r\nX: y",
        b"HTTP/1.1 200 OK\r\nBad Header\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n: empty\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nX: a\r\n folded\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffffffff\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\nab",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabcd\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
        b"HTTP/1.1 101 Switching\r\n\r\n",
        long_line.as_bytes(),
        many.as_bytes(),
    ];
    for c in &cases {
        assert!(parse(c, 1000).is_err(), "accepted: {:?}", String::from_utf8_lossy(&c[..c.len().min(80)]));
    }
    // Pseudo-random mutations of a valid chunked response must never panic.
    let base = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n4\r\nabcd\r\n0\r\n\r\n".to_vec();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..3000 {
        let mut m = base.clone();
        for _ in 0..4 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = (seed >> 33) as usize % m.len();
            match (seed >> 20) % 3 {
                0 => m[i] = (seed >> 8) as u8,
                1 => m.truncate(i.max(1)),
                _ => m.insert(i, b"\r\n:;0f "[(seed >> 12) as usize % 7]),
            }
        }
        let _ = parse(&m, 1000);
    }
}

#[test]
fn decoding_and_bomb_cap() {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(&vec![0u8; 1_000_000]).unwrap();
    let bomb = enc.finish().unwrap();
    let mut h = Headers::new();
    h.push("Content-Encoding", "gzip");
    assert!(bomb.len() < 4096);
    assert_eq!(decode_body(bomb.clone(), &h, 4096), Err(NetError::TooLarge));
    assert_eq!(decode_body(bomb, &h, 2_000_000).unwrap().len(), 1_000_000);
    assert!(matches!(decode_body(b"not gzip".to_vec(), &h, 100), Err(NetError::Protocol(_))));
    let mut d = Headers::new();
    d.push("Content-Encoding", "deflate");
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(b"deflated").unwrap();
    assert_eq!(decode_body(z.finish().unwrap(), &d, 100).unwrap(), b"deflated");
    let mut b = Headers::new();
    b.push("Content-Encoding", "br");
    assert!(decode_body(b"\xff\xff\xff garbage".to_vec(), &b, 100).is_err());
}
