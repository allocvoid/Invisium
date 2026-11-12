//! Content-Encoding removal (gzip / deflate / br) with a decompressed-size cap.

use crate::types::{Headers, NetError};
use std::io::Read;

fn read_capped(mut r: impl Read, max: usize, what: &str) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    (&mut r)
        .take(max as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| NetError::Protocol(format!("{what} decoding failed: {e}")))?;
    if out.len() > max {
        return Err(NetError::TooLarge);
    }
    Ok(out)
}

fn decode_one(body: Vec<u8>, coding: &str, max: usize) -> Result<Vec<u8>, NetError> {
    match coding {
        "identity" => Ok(body),
        "gzip" | "x-gzip" => read_capped(flate2::read::MultiGzDecoder::new(&body[..]), max, "gzip"),
        "deflate" => {
            // RFC 9110 says zlib-wrapped; some servers send raw deflate.
            match read_capped(flate2::read::ZlibDecoder::new(&body[..]), max, "deflate") {
                Err(NetError::Protocol(_)) => {
                    read_capped(flate2::read::DeflateDecoder::new(&body[..]), max, "deflate")
                }
                other => other,
            }
        }
        "br" => read_capped(brotli_decompressor::Decompressor::new(&body[..], 4096), max, "brotli"),
        other => Err(NetError::Protocol(format!("unsupported Content-Encoding: {other}"))),
    }
}

/// Removes every Content-Encoding layer, outermost last-listed first.
pub(crate) fn decode_body(body: Vec<u8>, headers: &Headers, max: usize) -> Result<Vec<u8>, NetError> {
    let codings: Vec<String> = headers
        .get_all("content-encoding")
        .flat_map(|v| v.split(','))
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    if codings.len() > 4 {
        return Err(NetError::Protocol("too many content codings".into()));
    }
    if body.is_empty() {
        return Ok(body);
    }
    let mut body = body;
    for c in codings.iter().rev() {
        body = decode_one(body, c, max)?;
    }
    Ok(body)
}
