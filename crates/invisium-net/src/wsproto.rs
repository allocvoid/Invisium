//! WebSocket (RFC 6455) building blocks written from scratch: SHA-1 and base64 for the opening
//! handshake, client frame encoding (always masked) and the checks on server frames.

/// SHA-1 (FIPS 180-4). Only used for `Sec-WebSocket-Accept`, where it is not a security
/// boundary (RFC 6455 section 1.3).
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// Standard base64 with padding (RFC 4648).
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The `Sec-WebSocket-Accept` value a server must answer for `key`.
pub(crate) fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// Random bytes from the TLS crypto provider (the only randomness source of this crate).
/// `None` if the system source fails: the socket then fails rather than send predictable keys.
pub(crate) fn random<const N: usize>() -> Option<[u8; N]> {
    let mut b = [0u8; N];
    rustls::crypto::ring::default_provider().secure_random.fill(&mut b).ok()?;
    Some(b)
}

pub(crate) const OP_CONT: u8 = 0x0;
pub(crate) const OP_TEXT: u8 = 0x1;
pub(crate) const OP_BINARY: u8 = 0x2;
pub(crate) const OP_CLOSE: u8 = 0x8;
pub(crate) const OP_PING: u8 = 0x9;
pub(crate) const OP_PONG: u8 = 0xA;

/// One complete client frame (FIN set), masked with `mask`.
pub(crate) fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// The fixed part of a server frame header (first two bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameStart {
    pub fin: bool,
    pub rsv: u8,
    pub opcode: u8,
    pub masked: bool,
    /// 0..=125, or 126 / 127 for an extended length that follows.
    pub len7: u8,
}

pub(crate) fn frame_start(b0: u8, b1: u8) -> FrameStart {
    FrameStart { fin: b0 & 0x80 != 0, rsv: (b0 >> 4) & 7, opcode: b0 & 0x0f, masked: b1 & 0x80 != 0, len7: b1 & 0x7f }
}

/// Close codes a peer may put on the wire (RFC 6455 section 7.4).
pub(crate) fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014) || (3000..=4999).contains(&code)
}

/// The payload of a Close frame: code and reason (reason cut to 123 bytes on a char boundary).
pub(crate) fn close_payload(code: Option<u16>, reason: &str) -> Vec<u8> {
    let Some(code) = code else { return Vec::new() };
    let mut end = reason.len().min(123);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let mut p = code.to_be_bytes().to_vec();
    p.extend_from_slice(&reason.as_bytes()[..end]);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(hex(&sha1(&vec![b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }

    #[test]
    fn base64_vectors() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")];
        for (i, o) in cases {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn rfc6455_accept_example() {
        // RFC 6455 section 1.3.
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_are_masked() {
        let f = encode_frame(OP_TEXT, b"Hello", [0x37, 0xfa, 0x21, 0x3d]);
        // RFC 6455 section 5.7: a single-frame masked text message containing "Hello".
        assert_eq!(f, [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]);
        let big = encode_frame(OP_BINARY, &[0u8; 70000], [1, 2, 3, 4]);
        assert_eq!(&big[..2], &[0x82, 0xFF]);
        assert_eq!(big.len(), 2 + 8 + 4 + 70000);
        assert_ne!(random::<16>().unwrap(), random::<16>().unwrap());
    }

    #[test]
    fn close_payloads() {
        assert!(close_payload(None, "x").is_empty());
        let p = close_payload(Some(1000), &"é".repeat(100));
        assert_eq!(&p[..2], &[0x03, 0xE8]);
        assert!(p.len() - 2 <= 123 && std::str::from_utf8(&p[2..]).is_ok());
        assert!(valid_close_code(1000) && valid_close_code(4999) && !valid_close_code(1005) && !valid_close_code(999));
    }
}
