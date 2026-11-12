//! A3/A4/A5: end-to-end tests against local std listeners (and one ignored live test).

use crate::*;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

pub(crate) struct Server {
    pub port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
    pub fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

pub(crate) fn read_head(s: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") && buf.len() < 65536 {
        match s.read(&mut b) {
            Ok(1) => buf.push(b[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Serves one response per connection, recording every request head.
pub(crate) fn serve(handler: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (h2, handler) = (heads.clone(), Arc::new(handler));
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let (heads, handler) = (h2.clone(), handler.clone());
            thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let head = read_head(&mut s);
                if head.is_empty() {
                    return;
                }
                heads.lock().unwrap().push(head.clone());
                let _ = s.write_all(&handler(&head));
                let _ = s.flush();
            });
        }
    });
    Server { port, heads }
}

pub(crate) fn path_of(head: &str) -> &str {
    head.split(' ').nth(1).unwrap_or("")
}

pub(crate) fn ok(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

pub(crate) fn local_cfg() -> NetConfig {
    NetConfig { https_only: false, ..NetConfig::default() }
}

pub(crate) fn service(cfg: NetConfig) -> NetService {
    let svc = NetService::new(cfg);
    // Public-looking names resolve to a public address in the test resolver seam (so that
    // private-network protection sees public servers) while the sockets go to the listener.
    for h in ["site-a.test", "www.site-a.test", "site-b.test", "doubleclick.net"] {
        svc.map_public_host(h, LOCAL);
    }
    for h in ["a.localhost", "b.localhost"] {
        svc.map_host(h, LOCAL);
    }
    svc
}

pub(crate) fn fetch(svc: &NetService, req: Request) -> Result<Response, NetError> {
    let (tx, rx) = mpsc::channel();
    svc.fetch(req, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(30)).expect("done was not called")
}

pub(crate) fn get(url: &str) -> Request {
    Request::get(Url::parse(url).unwrap(), ResourceKind::Document)
}

pub(crate) fn sub(url: &str, top: &str, kind: ResourceKind) -> Request {
    let mut r = Request::get(Url::parse(url).unwrap(), kind);
    r.top_level = Some(Url::parse(top).unwrap());
    r
}

// ── A3 ──

#[test]
fn content_length_body() {
    let srv = serve(|_| {
        b"HTTP/1.1 200 OK\r\nContent-Type: Text/HTML; charset=\"UTF-8\"\r\nContent-Length: 5\r\n\r\nhello".to_vec()
    });
    let svc = service(local_cfg());
    let r = fetch(&svc, get(&srv.url("127.0.0.1", "/"))).unwrap();
    assert_eq!((r.status, r.body.as_slice(), r.mime.as_str()), (200, &b"hello"[..], "text/html"));
    assert_eq!(r.charset.as_deref(), Some("utf-8"));
    assert_eq!((r.via, r.redirects, r.from_cache), (Route::Direct, 0, false));
    let st = svc.stats();
    assert_eq!(st.requests, 1);
    assert!(st.bytes_received > 5);
}

#[test]
fn chunked_body() {
    let srv = serve(|_| {
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-T: 1\r\n\r\n".to_vec()
    });
    let r = fetch(&service(local_cfg()), get(&srv.url("127.0.0.1", "/"))).unwrap();
    assert_eq!(r.body, b"hello world");
}

#[test]
fn gzip_body() {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(b"compressed hello").unwrap();
    let gz = enc.finish().unwrap();
    let srv = serve(move |_| {
        let mut v = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            gz.len()
        )
        .into_bytes();
        v.extend_from_slice(&gz);
        v
    });
    let r = fetch(&service(local_cfg()), get(&srv.url("127.0.0.1", "/"))).unwrap();
    assert_eq!(r.body, b"compressed hello");
}

#[test]
fn redirect_302_then_200() {
    let srv = serve(|h| {
        if path_of(h) == "/start" {
            b"HTTP/1.1 302 Found\r\nLocation: /end\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            ok("done")
        }
    });
    let r = fetch(&service(local_cfg()), get(&srv.url("127.0.0.1", "/start"))).unwrap();
    assert_eq!((r.status, r.redirects, r.url.path()), (200, 1, "/end"));
    assert_eq!(r.body, b"done");
    assert_eq!(srv.heads().len(), 2);
}

#[test]
fn redirect_loop_is_too_many_redirects() {
    let srv = serve(|_| b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    let r = fetch(&service(local_cfg()), get(&srv.url("127.0.0.1", "/loop")));
    assert_eq!(r.unwrap_err(), NetError::TooManyRedirects);
    assert_eq!(srv.heads().len(), 11);
}

#[test]
fn oversized_body_is_too_large() {
    let srv = serve(|h| {
        if path_of(h) == "/cl" {
            format!("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}", "x".repeat(100)).into_bytes()
        } else {
            format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}", "x".repeat(100)).into_bytes()
        }
    });
    let svc = service(NetConfig { max_body_bytes: 10, ..local_cfg() });
    assert_eq!(fetch(&svc, get(&srv.url("127.0.0.1", "/cl"))).unwrap_err(), NetError::TooLarge);
    assert_eq!(fetch(&svc, get(&srv.url("127.0.0.1", "/eof"))).unwrap_err(), NetError::TooLarge);
}

#[test]
fn connect_refused_is_connect_error() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let svc = service(NetConfig { connect_timeout_ms: 10_000, ..local_cfg() });
    let r = fetch(&svc, get(&format!("http://127.0.0.1:{port}/")));
    assert!(matches!(r, Err(NetError::Connect(_))), "{r:?}");
}

fn stalled_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((s, _)) = l.accept() {
            thread::sleep(Duration::from_secs(4));
            drop(s);
        }
    });
    port
}

#[test]
fn stalled_server_times_out() {
    let port = stalled_server();
    let svc = service(NetConfig { connect_timeout_ms: 300, ..local_cfg() });
    assert_eq!(fetch(&svc, get(&format!("http://127.0.0.1:{port}/"))).unwrap_err(), NetError::Timeout);
}

#[test]
fn cancel_reports_cancelled() {
    let port = stalled_server();
    let svc = service(local_cfg());
    let (tx, rx) = mpsc::channel();
    let h = svc.fetch(get(&format!("http://127.0.0.1:{port}/")), move |r| {
        let _ = tx.send(r);
    });
    thread::sleep(Duration::from_millis(100));
    h.cancel();
    let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(r.unwrap_err(), NetError::Cancelled);
}

#[test]
fn head_and_data_urls() {
    let srv = serve(|_| b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec());
    let svc = service(local_cfg());
    let mut req = get(&srv.url("127.0.0.1", "/"));
    req.method = Method::Head;
    let r = fetch(&svc, req).unwrap();
    assert!(r.status == 200 && r.body.is_empty());
    assert!(srv.heads()[0].starts_with("HEAD / HTTP/1.1\r\n"));
    let d = fetch(&svc, get("data:text/plain;charset=UTF-8;base64,aGk=")).unwrap();
    assert_eq!((d.body.as_slice(), d.mime.as_str(), d.via), (&b"hi"[..], "text/plain", Route::Local));
    assert_eq!(d.charset.as_deref(), Some("utf-8"));
}
