//! Streaming a response body into a new file (`fetch_with` with `BodyTarget::File`): decoded
//! as it arrives, never held whole in memory, removed again if the transfer does not finish.

use crate::service::{Inner, OpenBody};
use crate::types::{Headers, NetError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Decompressed bytes may exceed the in-memory body cap only while they stay within this
/// multiple of the encoded bytes received (decompression-bomb guard).
const MAX_RATIO: u64 = 200;
/// Encoded input is fed to the decoders in slices of at most this size.
const FEED: usize = 8 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Largest body piece the download path ever handled at once (tests assert it stays small).
#[cfg(test)]
pub(crate) static MAX_PIECE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn create(path: &Path) -> Result<std::fs::File, NetError> {
    // create_new: never overwrites and never follows a symbolic link at the final component.
    std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            NetError::Protocol("file exists".into())
        } else {
            NetError::Protocol(format!("cannot create file: {e}"))
        }
    })
}

/// Removes the file it guards when dropped, unless the download completed. Declared before
/// the writer chain so the file is closed (chain dropped) before it is removed.
struct Partial {
    path: PathBuf,
    keep: bool,
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// One stage of the decoder chain; `finish` checks the encoded stream ended properly.
trait Stage: Write + Send {
    fn finish(&mut self) -> io::Result<()>;
}

/// The end of the chain: the file, with the decoded-size accounting.
struct Sink {
    file: std::fs::File,
    written: Arc<AtomicU64>,
    encoded: Arc<AtomicU64>,
    floor: u64,
    bomb: Arc<AtomicBool>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let total = self.written.load(Ordering::Relaxed) + buf.len() as u64;
        let limit = self.floor.max(self.encoded.load(Ordering::Relaxed).saturating_mul(MAX_RATIO));
        if total > limit {
            self.bomb.store(true, Ordering::Relaxed);
            return Err(io::Error::other("decompressed size out of proportion"));
        }
        let n = self.file.write(buf)?;
        self.written.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Stage for Sink {
    fn finish(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

type Next = Box<dyn Stage>;

impl Stage for flate2::write::MultiGzDecoder<Next> {
    fn finish(&mut self) -> io::Result<()> {
        self.try_finish()?;
        self.get_mut().finish()
    }
}

impl Stage for brotli_decompressor::DecompressorWriter<Next> {
    fn finish(&mut self) -> io::Result<()> {
        self.close()?;
        self.get_mut().finish()
    }
}

/// `deflate`: zlib-wrapped per RFC 9110, raw deflate from some servers; decided on the first
/// two bytes as the in-memory decoder does by trial.
enum Deflate {
    Pending(Vec<u8>, Option<Next>),
    Zlib(flate2::write::ZlibDecoder<Next>),
    Raw(flate2::write::DeflateDecoder<Next>),
}

impl Deflate {
    fn decide(&mut self) -> io::Result<()> {
        let Deflate::Pending(buf, next) = self else { return Ok(()) };
        let (buf, next) = (std::mem::take(buf), next.take().ok_or_else(|| io::Error::other("decoder state"))?);
        let zlib = buf.len() >= 2 && buf[0] & 0x0f == 8 && (u16::from(buf[0]) << 8 | u16::from(buf[1])) % 31 == 0;
        *self = if zlib {
            Deflate::Zlib(flate2::write::ZlibDecoder::new(next))
        } else {
            Deflate::Raw(flate2::write::DeflateDecoder::new(next))
        };
        self.write_all(&buf)
    }
}

impl Write for Deflate {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Deflate::Pending(buf, _) => {
                buf.extend_from_slice(data);
                if buf.len() >= 2 {
                    self.decide()?;
                }
                Ok(data.len())
            }
            Deflate::Zlib(d) => d.write(data),
            Deflate::Raw(d) => d.write(data),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Deflate::Pending(..) => Ok(()),
            Deflate::Zlib(d) => d.flush(),
            Deflate::Raw(d) => d.flush(),
        }
    }
}

impl Stage for Deflate {
    fn finish(&mut self) -> io::Result<()> {
        self.decide()?;
        match self {
            Deflate::Pending(_, Some(n)) => n.finish(),
            Deflate::Pending(_, None) => Ok(()),
            Deflate::Zlib(d) => {
                d.try_finish()?;
                d.get_mut().finish()
            }
            Deflate::Raw(d) => {
                d.try_finish()?;
                d.get_mut().finish()
            }
        }
    }
}

/// The Content-Encoding layers, validated as `decode_body` does, in the order applied.
fn codings(headers: &Headers) -> Result<Vec<String>, NetError> {
    let list: Vec<String> = headers
        .get_all("content-encoding")
        .flat_map(|v| v.split(','))
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect();
    if list.len() > 4 {
        return Err(NetError::Protocol("too many content codings".into()));
    }
    for c in &list {
        if !matches!(c.as_str(), "gzip" | "x-gzip" | "deflate" | "br") {
            return Err(NetError::Protocol(format!("unsupported Content-Encoding: {c}")));
        }
    }
    Ok(list)
}

/// Wraps `sink` so that the outermost (last-listed) coding is removed first.
fn chain(codings: &[String], sink: Sink) -> Next {
    let mut next: Next = Box::new(sink);
    for c in codings {
        next = match c.as_str() {
            "gzip" | "x-gzip" => Box::new(flate2::write::MultiGzDecoder::new(next)),
            "deflate" => Box::new(Deflate::Pending(Vec::new(), Some(next))),
            _ => Box::new(brotli_decompressor::DecompressorWriter::new(next, 4096)),
        };
    }
    next
}

/// Streams `open` into a new file at `path`.
pub(crate) async fn to_file(
    inner: &Inner,
    mut open: OpenBody,
    headers: &Headers,
    path: &Path,
    mut progress: Box<dyn FnMut(u64) + Send>,
    max_body: usize,
) -> Result<(), NetError> {
    let codings = codings(headers)?;
    let file = create(path)?;
    let mut partial = Partial { path: path.to_path_buf(), keep: false };
    let (written, encoded, bomb) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
    let sink = Sink { file, written: written.clone(), encoded: encoded.clone(), floor: max_body as u64, bomb: bomb.clone() };
    let mut writer = chain(&codings, sink);
    let write_err = |e: io::Error| {
        if bomb.load(Ordering::Relaxed) {
            NetError::TooLarge
        } else if codings.is_empty() {
            NetError::Protocol(format!("cannot write file: {e}"))
        } else {
            NetError::Protocol(format!("decoding failed: {e}"))
        }
    };
    let mut last = Instant::now();
    let res = async {
        while let Some(piece) = open.stream.next(&mut open.conn.reader).await? {
            #[cfg(test)]
            MAX_PIECE.fetch_max(piece.len(), Ordering::Relaxed);
            for part in piece.chunks(FEED) {
                encoded.fetch_add(part.len() as u64, Ordering::Relaxed);
                writer.write_all(part).map_err(write_err)?;
            }
            if last.elapsed() >= PROGRESS_EVERY {
                progress(written.load(Ordering::Relaxed));
                last = Instant::now();
            }
        }
        writer.finish().map_err(write_err)
    }
    .await;
    open.finish(inner, res.is_ok());
    res?;
    drop(writer);
    progress(written.load(Ordering::Relaxed));
    partial.keep = true;
    Ok(())
}

/// `File` for a body that is already in memory (data: URL or cache hit).
pub(crate) fn write_whole(path: &Path, body: &[u8], mut progress: Box<dyn FnMut(u64) + Send>) -> Result<(), NetError> {
    let mut file = create(path)?;
    let mut partial = Partial { path: path.to_path_buf(), keep: false };
    let res = file.write_all(body);
    drop(file); // closed before `partial` may remove it
    res.map_err(|e| NetError::Protocol(format!("cannot write file: {e}")))?;
    progress(body.len() as u64);
    partial.keep = true;
    Ok(())
}
