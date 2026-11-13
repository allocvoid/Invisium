//! HTTP/1.1 response reader over any `AsyncRead`, with hard limits on everything a hostile
//! server controls.

use crate::types::{Headers, NetError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_LINE: usize = 16 * 1024;
const MAX_HEAD: usize = 64 * 1024;
const MAX_HEADERS: usize = 128;
const MAX_INTERIM: usize = 16;

fn proto(msg: &str) -> NetError {
    NetError::Protocol(msg.to_string())
}

pub(crate) struct Reader<R> {
    pub io: R,
    buf: Vec<u8>,
    pos: usize,
    pub received: u64,
    idle: Duration,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    pub fn new(io: R, idle: Duration) -> Self {
        Reader { io, buf: Vec::new(), pos: 0, received: 0, idle }
    }

    /// True when no unread bytes are buffered (safe to reuse for another request).
    pub fn drained(&self) -> bool {
        self.pos == self.buf.len()
    }

    async fn fill(&mut self) -> Result<usize, NetError> {
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos > 64 * 1024 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let old = self.buf.len();
        self.buf.resize(old + 16 * 1024, 0);
        let res = tokio::time::timeout(self.idle, self.io.read(&mut self.buf[old..])).await;
        let n = match res {
            Err(_) => {
                self.buf.truncate(old);
                return Err(NetError::Timeout);
            }
            Ok(Err(e)) => {
                self.buf.truncate(old);
                return Err(NetError::Protocol(format!("read failed: {e}")));
            }
            Ok(Ok(n)) => n,
        };
        self.buf.truncate(old + n);
        self.received += n as u64;
        Ok(n)
    }

    /// One line without its CR LF; `None` on clean EOF before any byte.
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        let mut scanned = 0;
        loop {
            let avail = &self.buf[self.pos..];
            if let Some(i) = avail[scanned..].iter().position(|&b| b == b'\n') {
                let end = scanned + i;
                let mut line = avail[..end].to_vec();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.pos += end + 1;
                return Ok(Some(line));
            }
            scanned = avail.len();
            if scanned > MAX_LINE {
                return Err(proto("line too long"));
            }
            if self.fill().await? == 0 {
                return if scanned == 0 { Ok(None) } else { Err(proto("connection closed mid-line")) };
            }
        }
    }

    async fn read_exact_into(&mut self, mut n: u64, out: &mut Vec<u8>) -> Result<(), NetError> {
        while n > 0 {
            if self.drained() && self.fill().await? == 0 {
                return Err(proto("connection closed before end of body"));
            }
            let avail = &self.buf[self.pos..];
            let take = (n.min(avail.len() as u64)) as usize;
            out.extend_from_slice(&avail[..take]);
            self.pos += take;
            n -= take as u64;
        }
        Ok(())
    }

    async fn read_to_close(&mut self, out: &mut Vec<u8>, max: usize) -> Result<(), NetError> {
        loop {
            out.extend_from_slice(&self.buf[self.pos..]);
            self.pos = self.buf.len();
            if out.len() > max {
                return Err(NetError::TooLarge);
            }
            if self.fill().await? == 0 {
                return Ok(());
            }
        }
    }
}

pub(crate) struct RawResponse {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
    /// The connection may carry another request.
    pub reusable: bool,
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

async fn read_head<R: AsyncRead + Unpin>(
    r: &mut Reader<R>,
) -> Result<(u16, bool, Headers), NetError> {
    let line = r.read_line().await?.ok_or_else(|| proto("connection closed before response"))?;
    let b = line.as_slice();
    let ok = b.len() >= 12
        && b.starts_with(b"HTTP/1.")
        && b[7].is_ascii_digit()
        && b[8] == b' '
        && b[9..12].iter().all(u8::is_ascii_digit)
        && (b.len() == 12 || b[12] == b' ');
    if !ok {
        return Err(proto("malformed status line"));
    }
    let http11 = b[7] != b'0';
    let status = b[9..12].iter().fold(0u16, |a, d| a * 10 + (d - b'0') as u16);
    if status < 100 {
        return Err(proto("invalid status code"));
    }
    let mut headers = Headers::new();
    let mut total = line.len();
    loop {
        let l = r.read_line().await?.ok_or_else(|| proto("connection closed in headers"))?;
        if l.is_empty() {
            break;
        }
        total += l.len();
        if total > MAX_HEAD || headers.len() >= MAX_HEADERS {
            return Err(proto("header section too large"));
        }
        if l[0] == b' ' || l[0] == b'\t' {
            return Err(proto("obsolete header line folding"));
        }
        let colon = l.iter().position(|&c| c == b':').ok_or_else(|| proto("malformed header line"))?;
        let name = &l[..colon];
        if name.is_empty() || !name.iter().all(|&c| is_tchar(c)) {
            return Err(proto("invalid header name"));
        }
        let value = String::from_utf8_lossy(&l[colon + 1..]);
        headers.push(
            String::from_utf8_lossy(name).into_owned(),
            value.trim_matches(|c| c == ' ' || c == '\t').to_string(),
        );
    }
    Ok((status, http11, headers))
}

fn content_length(headers: &Headers) -> Result<Option<u64>, NetError> {
    let mut found: Option<u64> = None;
    for v in headers.get_all("content-length") {
        for part in v.split(',') {
            let p = part.trim();
            if p.is_empty() || p.len() > 19 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(proto("invalid Content-Length"));
            }
            let n: u64 = p.parse().map_err(|_| proto("invalid Content-Length"))?;
            if found.is_some_and(|f| f != n) {
                return Err(proto("conflicting Content-Length"));
            }
            found = Some(n);
        }
    }
    Ok(found)
}

async fn read_chunked<R: AsyncRead + Unpin>(
    r: &mut Reader<R>,
    out: &mut Vec<u8>,
    max: usize,
) -> Result<(), NetError> {
    loop {
        let line = r.read_line().await?.ok_or_else(|| proto("connection closed in chunked body"))?;
        let size_part = line.split(|&b| b == b';').next().unwrap_or(&[]);
        let s = std::str::from_utf8(size_part).map_err(|_| proto("invalid chunk size"))?.trim();
        if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(proto("invalid chunk size"));
        }
        let size = u64::from_str_radix(s, 16).map_err(|_| proto("invalid chunk size"))?;
        if size == 0 {
            for _ in 0..=MAX_HEADERS {
                let t = r.read_line().await?.ok_or_else(|| proto("connection closed in trailers"))?;
                if t.is_empty() {
                    return Ok(());
                }
            }
            return Err(proto("too many trailers"));
        }
        if (out.len() as u64).saturating_add(size) > max as u64 {
            return Err(NetError::TooLarge);
        }
        r.read_exact_into(size, out).await?;
        let end = r.read_line().await?.ok_or_else(|| proto("connection closed after chunk"))?;
        if !end.is_empty() {
            return Err(proto("missing CRLF after chunk"));
        }
    }
}

/// Reads one complete response (skipping 1xx interim responses).
pub(crate) async fn read_response<R: AsyncRead + Unpin>(
    r: &mut Reader<R>,
    head_request: bool,
    max_body: usize,
) -> Result<RawResponse, NetError> {
    for _ in 0..MAX_INTERIM {
        let (status, http11, headers) = read_head(r).await?;
        if (100..200).contains(&status) {
            if status == 101 {
                return Err(proto("unexpected protocol switch"));
            }
            continue;
        }
        let mut body = Vec::new();
        let mut delimited = true;
        if !(head_request || status == 204 || status == 304) {
            let te: Vec<String> = headers
                .get_all("transfer-encoding")
                .flat_map(|v| v.split(','))
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty() && t != "identity")
                .collect();
            if !te.is_empty() {
                if te.iter().any(|t| t != "chunked") {
                    return Err(proto("unsupported Transfer-Encoding"));
                }
                read_chunked(r, &mut body, max_body).await?;
            } else if let Some(len) = content_length(&headers)? {
                if len > max_body as u64 {
                    return Err(NetError::TooLarge);
                }
                r.read_exact_into(len, &mut body).await?;
            } else {
                delimited = false;
                r.read_to_close(&mut body, max_body).await?;
            }
        }
        let close = headers
            .get_all("connection")
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case("close"));
        let reusable = http11 && delimited && !close && r.drained();
        return Ok(RawResponse { status, headers, body, reusable });
    }
    Err(proto("too many interim responses"))
}

/// How a response body is delimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    Empty,
    Length(u64),
    Chunked,
    Close,
}

/// A response head read on its own, the body still on the connection.
pub(crate) struct Head {
    pub status: u16,
    pub http11: bool,
    pub headers: Headers,
}

/// Reads one response head, skipping 1xx interim responses. A `101` is returned only when
/// `upgrade` is set (the WebSocket handshake); otherwise it is an error as in `read_response`.
pub(crate) async fn read_final_head<R: AsyncRead + Unpin>(
    r: &mut Reader<R>,
    upgrade: bool,
) -> Result<Head, NetError> {
    for _ in 0..MAX_INTERIM {
        let (status, http11, headers) = read_head(r).await?;
        if status == 101 {
            if upgrade {
                return Ok(Head { status, http11, headers });
            }
            return Err(proto("unexpected protocol switch"));
        }
        if (100..200).contains(&status) {
            continue;
        }
        return Ok(Head { status, http11, headers });
    }
    Err(proto("too many interim responses"))
}

/// The body framing of a final response, as `read_response` decides it.
pub(crate) fn framing(head: &Head, head_request: bool) -> Result<Framing, NetError> {
    if head_request || head.status == 204 || head.status == 304 {
        return Ok(Framing::Empty);
    }
    let te: Vec<String> = head
        .headers
        .get_all("transfer-encoding")
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty() && t != "identity")
        .collect();
    if !te.is_empty() {
        if te.iter().any(|t| t != "chunked") {
            return Err(proto("unsupported Transfer-Encoding"));
        }
        return Ok(Framing::Chunked);
    }
    Ok(match content_length(&head.headers)? {
        Some(n) => Framing::Length(n),
        None => Framing::Close,
    })
}

/// Whether the connection may carry another request once the body was read completely.
pub(crate) fn keep_alive(head: &Head, framing: Framing) -> bool {
    let close = head
        .headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case("close"));
    head.http11 && framing != Framing::Close && !close
}

/// Hands out a response body piece by piece without collecting it. Each piece is at most the
/// reader's buffer (a few tens of KiB).
pub(crate) struct BodyStream {
    framing: Framing,
    /// Bytes left in the current chunk (or in the whole body with `Length`).
    left: u64,
    /// Chunked: a chunk's data was consumed and its CR LF is still to be read.
    chunk_open: bool,
    done: bool,
}

impl BodyStream {
    pub fn new(framing: Framing) -> BodyStream {
        let (left, done) = match framing {
            Framing::Empty => (0, true),
            Framing::Length(n) => (n, n == 0),
            _ => (0, false),
        };
        BodyStream { framing, left, chunk_open: false, done }
    }

    /// The next piece of the body, `None` at its end.
    pub async fn next<'a, R: AsyncRead + Unpin>(
        &mut self,
        r: &'a mut Reader<R>,
    ) -> Result<Option<&'a [u8]>, NetError> {
        if self.done {
            return Ok(None);
        }
        match self.framing {
            Framing::Empty => Ok(None),
            Framing::Length(_) => {
                if r.drained() && r.fill().await? == 0 {
                    return Err(proto("connection closed before end of body"));
                }
                let piece = r.take(self.left);
                self.left -= piece.len() as u64;
                self.done = self.left == 0;
                Ok(Some(piece))
            }
            Framing::Close => {
                if r.drained() && r.fill().await? == 0 {
                    self.done = true;
                    return Ok(None);
                }
                Ok(Some(r.take(u64::MAX)))
            }
            Framing::Chunked => {
                if self.left == 0 {
                    if self.chunk_open {
                        let end = r.read_line().await?.ok_or_else(|| proto("connection closed after chunk"))?;
                        if !end.is_empty() {
                            return Err(proto("missing CRLF after chunk"));
                        }
                        self.chunk_open = false;
                    }
                    let size = chunk_size(r).await?;
                    if size == 0 {
                        for _ in 0..=MAX_HEADERS {
                            let t = r.read_line().await?.ok_or_else(|| proto("connection closed in trailers"))?;
                            if t.is_empty() {
                                self.done = true;
                                return Ok(None);
                            }
                        }
                        return Err(proto("too many trailers"));
                    }
                    self.left = size;
                    self.chunk_open = true;
                }
                if r.drained() && r.fill().await? == 0 {
                    return Err(proto("connection closed before end of body"));
                }
                let piece = r.take(self.left);
                self.left -= piece.len() as u64;
                Ok(Some(piece))
            }
        }
    }
}

async fn chunk_size<R: AsyncRead + Unpin>(r: &mut Reader<R>) -> Result<u64, NetError> {
    let line = r.read_line().await?.ok_or_else(|| proto("connection closed in chunked body"))?;
    let size_part = line.split(|&b| b == b';').next().unwrap_or(&[]);
    let s = std::str::from_utf8(size_part).map_err(|_| proto("invalid chunk size"))?.trim();
    if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(proto("invalid chunk size"));
    }
    u64::from_str_radix(s, 16).map_err(|_| proto("invalid chunk size"))
}

impl<R> Reader<R> {
    /// Up to `max` buffered bytes, consumed.
    fn take(&mut self, max: u64) -> &[u8] {
        let avail = self.buf.len() - self.pos;
        let n = (max.min(avail as u64)) as usize;
        let start = self.pos;
        self.pos += n;
        &self.buf[start..start + n]
    }

    /// The stream and the bytes already read from it but not consumed.
    pub fn into_parts(self) -> (R, Vec<u8>) {
        let rest = self.buf[self.pos..].to_vec();
        (self.io, rest)
    }
}

/// Lower-case MIME essence and charset parameter of a Content-Type value.
pub(crate) fn parse_content_type(v: &str) -> (String, Option<String>) {
    let mut parts = v.split(';');
    let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts.find_map(|p| {
        let (k, val) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| val.trim().trim_matches('"').to_ascii_lowercase())
            .filter(|c| !c.is_empty())
    });
    (essence, charset)
}
