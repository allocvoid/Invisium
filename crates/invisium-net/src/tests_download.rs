//! Milestone 3b, E5: `fetch_with` and streaming downloads, against local listeners.

use crate::tests_cors::reply;
use crate::tests_service::*;
use crate::*;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const SIZE: usize = 5 * 1024 * 1024;

/// 5 MiB of pseudo-random letters: compressible, but not trivially.
fn payload() -> Vec<u8> {
    let mut x: u32 = 12345;
    (0..SIZE)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            b'a' + ((x >> 16) % 16) as u8
        })
        .collect()
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn head_bytes(lines: &[String]) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\n{}\r\n\r\n", lines.join("\r\n")).into_bytes()
}

fn download_server() -> Server {
    let body = Arc::new(payload());
    let gz = Arc::new(gzip(&body));
    serve(move |h| {
        let mut out;
        match path_of(h) {
            "/files/plain.bin" => {
                out = head_bytes(&[format!("Content-Length: {SIZE}"), "Connection: close".into()]);
                out.extend_from_slice(&body);
            }
            "/gzip" => {
                out = head_bytes(&[
                    format!("Content-Length: {}", gz.len()),
                    "Content-Encoding: gzip".into(),
                    "Content-Disposition: attachment; filename=\"data.txt\"".into(),
                    "Connection: close".into(),
                ]);
                out.extend_from_slice(&gz);
            }
            "/chunked" => {
                out = head_bytes(&["Transfer-Encoding: chunked".into(), "Connection: close".into()]);
                for c in body.chunks(100_003) {
                    out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
                    out.extend_from_slice(c);
                    out.extend_from_slice(b"\r\n");
                }
                out.extend_from_slice(b"0\r\n\r\n");
            }
            "/cut" => {
                // Promises 5 MiB, delivers 1 MiB, closes.
                out = head_bytes(&[format!("Content-Length: {SIZE}"), "Connection: close".into()]);
                out.extend_from_slice(&body[..1024 * 1024]);
            }
            _ => out = reply("200 OK", &[], "small body"),
        }
        out
    })
}

fn temp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("invisium-net-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

struct Outcome {
    result: Result<Response, NetError>,
    head: Option<ResponseHead>,
    progress: Vec<u64>,
}

/// `fetch_with`, choosing the target with `pick` once the head is known.
fn fetch_with(
    svc: &NetService,
    req: Request,
    pick: impl FnOnce(Box<dyn FnMut(u64) + Send>) -> BodyTarget + Send + 'static,
) -> Outcome {
    let (tx, rx) = mpsc::channel();
    let head = Arc::new(Mutex::new(None));
    let progress = Arc::new(Mutex::new(Vec::new()));
    let (h2, p2) = (head.clone(), progress.clone());
    svc.fetch_with(
        req,
        move |h| {
            *h2.lock().unwrap() = Some(h.clone());
            pick(Box::new(move |n| p2.lock().unwrap().push(n)))
        },
        move |r| tx.send(r).unwrap(),
    );
    let result = rx.recv_timeout(Duration::from_secs(60)).expect("done was not called");
    let head = head.lock().unwrap().take();
    let progress = progress.lock().unwrap().clone();
    Outcome { result, head, progress }
}

fn to_file(path: PathBuf) -> impl FnOnce(Box<dyn FnMut(u64) + Send>) -> BodyTarget + Send + 'static {
    move |progress| BodyTarget::File { path, progress }
}

#[test]
fn e5_bodies_land_in_the_file_byte_for_byte() {
    let srv = download_server();
    let svc = service(local_cfg());
    let want = payload();
    for (path, name) in [("/files/plain.bin", "plain.bin"), ("/gzip", "data.txt"), ("/chunked", "chunked")] {
        let file = temp_path(&format!("e5-{name}"));
        let o = fetch_with(&svc, get(&srv.url("127.0.0.1", path)), to_file(file.clone()));
        let r = o.result.unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(r.body.is_empty() && r.status == 200, "{path}");
        assert!(std::fs::read(&file).unwrap() == want, "{path}: file differs");
        let head = o.head.unwrap();
        assert_eq!(head.filename, name, "{path}");
        assert_eq!(head.attachment, path == "/gzip");
        if path == "/files/plain.bin" {
            assert_eq!(head.length, Some(SIZE as u64));
        }
        // Progress: monotonic, ends at the total.
        assert!(o.progress.windows(2).all(|w| w[0] <= w[1]), "{path}: {:?}", o.progress);
        assert_eq!(o.progress.last(), Some(&(SIZE as u64)), "{path}");
        std::fs::remove_file(&file).unwrap();
    }
    // Never held whole in memory: no piece the download path handled exceeded the reader's
    // small buffer.
    let largest = crate::download::MAX_PIECE.load(Ordering::Relaxed);
    assert!(largest > 0 && largest <= 128 * 1024, "largest piece {largest}");
    // A file download is never cached.
    assert_eq!(svc.stats().cache_bytes, 0);
}

#[test]
fn e5_an_existing_file_is_never_touched() {
    let srv = download_server();
    let svc = service(local_cfg());
    let file = temp_path("e5-existing");
    std::fs::write(&file, b"keep me").unwrap();
    let o = fetch_with(&svc, get(&srv.url("127.0.0.1", "/files/plain.bin")), to_file(file.clone()));
    assert_eq!(o.result.unwrap_err(), NetError::Protocol("file exists".into()));
    assert_eq!(std::fs::read(&file).unwrap(), b"keep me");
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn e5_failed_and_cancelled_transfers_leave_no_file() {
    let srv = download_server();
    let svc = service(local_cfg());
    let file = temp_path("e5-cut");
    let o = fetch_with(&svc, get(&srv.url("127.0.0.1", "/cut")), to_file(file.clone()));
    assert!(matches!(o.result, Err(NetError::Protocol(_))), "{:?}", o.result.err());
    assert!(!file.exists());

    // Cancel: a server that sends 1 MiB and then stalls.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            read_head(&mut s);
            let _ = s.write_all(&head_bytes(&[format!("Content-Length: {SIZE}")]));
            let _ = s.write_all(&payload()[..1024 * 1024]);
            thread::sleep(Duration::from_secs(10));
        }
    });
    let file = temp_path("e5-cancel");
    let (tx, rx) = mpsc::channel();
    let target = file.clone();
    let handle = svc.fetch_with(
        get(&format!("http://127.0.0.1:{port}/stall")),
        move |_| BodyTarget::File { path: target, progress: Box::new(|_| {}) },
        move |r| tx.send(r).unwrap(),
    );
    let start = Instant::now();
    while std::fs::metadata(&file).map_or(0, |m| m.len()) < 1024 * 1024 {
        assert!(start.elapsed() < Duration::from_secs(20), "the partial file never filled");
        thread::sleep(Duration::from_millis(20));
    }
    handle.cancel();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap_err(), NetError::Cancelled);
    assert!(!file.exists(), "the partial file was left behind");
}

#[test]
fn e5_memory_and_discard() {
    let srv = download_server();
    let svc = service(local_cfg());
    let url = srv.url("127.0.0.1", "/gzip");
    let plain = fetch(&svc, get(&url)).unwrap();
    let o = fetch_with(&svc, get(&url), |_| BodyTarget::Memory);
    let r = o.result.unwrap();
    assert_eq!((r.status, r.body.len(), &r.mime), (plain.status, SIZE, &plain.mime));
    assert!(r.body == plain.body);
    let o = fetch_with(&svc, get(&url), |_| BodyTarget::Discard);
    assert_eq!(o.result.unwrap_err(), NetError::Cancelled);
    assert!(o.head.is_some());
    // A small body, and a data: URL, through every target.
    let small = fetch_with(&svc, get(&srv.url("127.0.0.1", "/small")), |_| BodyTarget::Memory);
    assert_eq!(small.result.unwrap().body, b"small body");
    let file = temp_path("e5-data");
    let o = fetch_with(&svc, get("data:text/plain,hello%20file"), to_file(file.clone()));
    assert!(o.result.unwrap().body.is_empty());
    assert_eq!(std::fs::read(&file).unwrap(), b"hello file");
    assert_eq!(o.progress, [10]);
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn e5_on_head_is_not_called_for_blocked_or_failed_requests() {
    let srv = download_server();
    let svc = service(local_cfg());
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let page = "http://site-a.test/";
    let requests = [
        sub(&srv.url("doubleclick.net", "/files/plain.bin"), page, ResourceKind::Other), // tracker
        sub(&srv.url("127.0.0.1", "/files/plain.bin"), page, ResourceKind::Other),       // private network
        get(&format!("http://127.0.0.1:{dead}/")),                                       // refused
        get("ftp://example.com/file"),                                                    // scheme
    ];
    for req in requests {
        let url = req.url.to_string();
        let o = fetch_with(&svc, req, |_| panic!("on_head must not run"));
        assert!(o.result.is_err() && o.head.is_none(), "{url}");
    }
    // A refused frame does not reach on_head either.
    let deny = serve(|_| reply("200 OK", &[("X-Frame-Options", "DENY")], "x"));
    let o = fetch_with(&svc, sub(&deny.url("site-a.test", "/"), page, ResourceKind::Frame), |_| BodyTarget::Memory);
    assert_eq!(o.result.unwrap_err(), NetError::Blocked(BlockReason::Framing));
    assert!(o.head.is_none());
}
