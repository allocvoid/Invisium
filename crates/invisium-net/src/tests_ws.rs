//! Milestone 3b, E7: the WebSocket client against WebSocket servers written here (std threads).

use crate::tests_service::*;
use crate::wsproto::accept_key;
use crate::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const PAGE: &str = "http://site-a.test/app";

/// A server-side WebSocket peer.
pub(crate) struct Peer {
    pub s: TcpStream,
}

/// One frame as the server sees it.
#[derive(Debug)]
pub(crate) struct Got {
    pub opcode: u8,
    pub masked: bool,
    pub payload: Vec<u8>,
}

impl Peer {
    pub fn frame(&mut self) -> Option<Got> {
        let mut h = [0u8; 2];
        self.s.read_exact(&mut h).ok()?;
        let masked = h[1] & 0x80 != 0;
        let len = match h[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                self.s.read_exact(&mut b).ok()?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                self.s.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        if masked {
            self.s.read_exact(&mut mask).ok()?;
        }
        let mut payload = vec![0u8; len];
        self.s.read_exact(&mut payload).ok()?;
        if masked {
            payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
        }
        Some(Got { opcode: h[0] & 0x0f, masked, payload })
    }

    /// Writes a frame with the given first byte (FIN, RSV, opcode); unmasked unless `mask`.
    pub fn send(&mut self, b0: u8, payload: &[u8], mask: Option<[u8; 4]>) {
        let mut out = vec![b0];
        let m = if mask.is_some() { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => out.push(m | n as u8),
            n if n <= 0xFFFF => {
                out.push(m | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(m | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        match mask {
            Some(k) => {
                out.extend_from_slice(&k);
                out.extend(payload.iter().enumerate().map(|(i, b)| b ^ k[i % 4]));
            }
            None => out.extend_from_slice(payload),
        }
        let _ = self.s.write_all(&out);
    }

    /// Reads frames until a Close and returns its code and reason.
    pub fn until_close(&mut self) -> Option<(u16, String)> {
        loop {
            let f = self.frame()?;
            if f.opcode == 8 {
                let code = if f.payload.len() >= 2 { u16::from_be_bytes([f.payload[0], f.payload[1]]) } else { 1005 };
                return Some((code, String::from_utf8_lossy(f.payload.get(2..).unwrap_or(&[])).into_owned()));
            }
        }
    }
}

/// The standard 101 answer for a handshake `head`, with extra header lines.
pub(crate) fn accept(head: &str, extra: &str) -> Vec<u8> {
    let key = crate::tests_cors::hdr(head, "Sec-WebSocket-Key").unwrap_or("");
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n{extra}\r\n",
        accept_key(key)
    )
    .into_bytes()
}

pub(crate) struct WsServer {
    pub port: u16,
    pub heads: Arc<Mutex<Vec<String>>>,
    pub conns: Arc<AtomicUsize>,
    /// What the session handler reported (one line per finding).
    pub log: Arc<Mutex<Vec<String>>>,
}

/// Accepts connections, records each handshake head, and hands the stream to `session`.
pub(crate) fn ws_server(session: impl Fn(Peer, String, &Mutex<Vec<String>>) + Send + Sync + 'static) -> WsServer {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (heads, conns, log) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
    let (h2, c2, l2, session) = (heads.clone(), conns.clone(), log.clone(), Arc::new(session));
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            c2.fetch_add(1, Ordering::SeqCst);
            let (heads, log, session) = (h2.clone(), l2.clone(), session.clone());
            thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                let head = read_head(&mut s);
                heads.lock().unwrap().push(head.clone());
                session(Peer { s }, head, &log);
            });
        }
    });
    WsServer { port, heads, conns, log }
}

/// Echoes data frames back (unmasked), answers a Close, and logs any unmasked client frame.
pub(crate) fn echo_server() -> WsServer {
    ws_server(|mut p, head, log| {
        let _ = p.s.write_all(&accept(&head, ""));
        while let Some(f) = p.frame() {
            if !f.masked {
                log.lock().unwrap().push("unmasked client frame".into());
            }
            match f.opcode {
                8 => {
                    log.lock().unwrap().push(format!("close {:?}", f.payload));
                    p.send(0x88, &f.payload, None);
                    return;
                }
                op => p.send(0x80 | op, &f.payload, None),
            }
        }
    })
}

pub(crate) fn ws_request(url: &str, top: &str) -> WsRequest {
    WsRequest { url: Url::parse(url).unwrap(), protocols: Vec::new(), top_level: Url::parse(top).unwrap(),
        initiator: None,
        top_local: false,
        initiator_local: false,
    }
}

pub(crate) fn open(svc: &NetService, req: WsRequest) -> (WsHandle, mpsc::Receiver<WsEvent>) {
    let (tx, rx) = mpsc::channel();
    let h = svc.websocket(req, move |e| {
        let _ = tx.send(e);
    });
    (h, rx)
}

pub(crate) fn next(rx: &mpsc::Receiver<WsEvent>) -> WsEvent {
    rx.recv_timeout(Duration::from_secs(15)).expect("no WebSocket event")
}

pub(crate) fn opened(rx: &mpsc::Receiver<WsEvent>) {
    assert_eq!(next(rx), WsEvent::Open { protocol: String::new() });
}

pub(crate) fn failed() -> WsEvent {
    WsEvent::Closed { code: 1006, reason: String::new(), clean: false }
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn e7_echo_both_ways_and_client_close() {
    let srv = echo_server();
    let svc = service(local_cfg());
    let (ws, rx) = open(&svc, ws_request(&format!("ws://site-a.test:{}/echo", srv.port), PAGE));
    opened(&rx);
    ws.send_text("hello ✓".into());
    assert_eq!(next(&rx), WsEvent::Text("hello ✓".into()));
    ws.send_binary(vec![0, 1, 2, 255]);
    assert_eq!(next(&rx), WsEvent::Binary(vec![0, 1, 2, 255]));
    let big: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
    ws.send_binary(big.clone());
    assert_eq!(next(&rx), WsEvent::Binary(big));
    ws.close(Some(4000), "bye now");
    assert_eq!(next(&rx), WsEvent::Closed { code: 4000, reason: "bye now".into(), clean: true });
    let log = srv.log.lock().unwrap().clone();
    let mut close = vec![0x0f, 0xa0];
    close.extend_from_slice(b"bye now");
    assert_eq!(log, [format!("close {close:?}")], "every client frame is masked");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "Closed is the last event");
}

#[test]
fn e7_fragmented_megabyte_ping_pong_and_server_close() {
    let srv = ws_server(|mut p, head, log| {
        let _ = p.s.write_all(&accept(&head, ""));
        let text = "abcdefgh".repeat(128 * 1024); // 1 MiB
        let parts: Vec<&[u8]> = text.as_bytes().chunks(256 * 1024).collect();
        for (i, part) in parts.iter().enumerate() {
            let fin = if i + 1 == parts.len() { 0x80 } else { 0 };
            let op = if i == 0 { 1 } else { 0 };
            p.send(fin | op, part, None);
            if i == 1 {
                p.send(0x89, b"are you there", None); // a ping between fragments
            }
        }
        let pong = p.frame().unwrap();
        log.lock().unwrap().push(format!("{} {} {}", pong.opcode, pong.masked, String::from_utf8_lossy(&pong.payload)));
        p.send(0x88, &[&4001u16.to_be_bytes()[..], b"server bye"].concat(), None);
        let (code, _) = p.until_close().unwrap();
        log.lock().unwrap().push(format!("client answered {code}"));
    });
    let svc = service(local_cfg());
    let (_ws, rx) = open(&svc, ws_request(&format!("ws://site-a.test:{}/", srv.port), PAGE));
    opened(&rx);
    assert_eq!(next(&rx), WsEvent::Text("abcdefgh".repeat(128 * 1024)));
    assert_eq!(next(&rx), WsEvent::Closed { code: 4001, reason: "server bye".into(), clean: true });
    assert_eq!(*srv.log.lock().unwrap(), ["10 true are you there", "client answered 4001"]);
}

#[test]
fn e7_subprotocols() {
    let srv = ws_server(|mut p, head, _| {
        let chosen = if head.contains("/other") { "other" } else { "superchat" };
        let _ = p.s.write_all(&accept(&head, &format!("Sec-WebSocket-Protocol: {chosen}\r\n")));
        let _ = p.until_close();
    });
    let svc = service(local_cfg());
    let mut req = ws_request(&format!("ws://site-a.test:{}/", srv.port), PAGE);
    req.protocols = vec!["chat".into(), "superchat".into()];
    let (ws, rx) = open(&svc, req.clone());
    assert_eq!(next(&rx), WsEvent::Open { protocol: "superchat".into() });
    assert_eq!(crate::tests_cors::hdr(&srv.heads.lock().unwrap()[0], "Sec-WebSocket-Protocol"), Some("chat, superchat"));
    ws.close(None, "");
    req.url = Url::parse(&format!("ws://site-a.test:{}/other", srv.port)).unwrap();
    let (_ws, rx) = open(&svc, req);
    assert_eq!(next(&rx), failed(), "a subprotocol that was not offered");
}

#[test]
fn e7_bad_handshakes_fail_with_1006() {
    let srv = ws_server(|mut p, head, _| {
        let resp = if head.contains("/wrong-accept") {
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n".to_vec()
        } else if head.contains("/extension") {
            accept(&head, "Sec-WebSocket-Extensions: permessage-deflate\r\n")
        } else if head.contains("/redirect") {
            b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nno".to_vec()
        };
        let _ = p.s.write_all(&resp);
        thread::sleep(Duration::from_millis(500));
    });
    let svc = service(local_cfg());
    for path in ["/wrong-accept", "/extension", "/redirect", "/ok200"] {
        let (_ws, rx) = open(&svc, ws_request(&format!("ws://site-a.test:{}{path}", srv.port), PAGE));
        assert_eq!(next(&rx), failed(), "{path}");
    }
    assert_eq!(srv.heads.lock().unwrap().len(), 4, "redirects are not followed");
}

/// The server misbehaves after the handshake; returns the close code the client sent.
fn misbehave(frames: impl Fn(&mut Peer) + Send + Sync + 'static) -> Vec<String> {
    let srv = ws_server(move |mut p, head, log| {
        let _ = p.s.write_all(&accept(&head, ""));
        frames(&mut p);
        let got = p.until_close().map_or("no close".into(), |(c, _)| format!("close {c}"));
        log.lock().unwrap().push(got);
    });
    let svc = service(local_cfg());
    let (_ws, rx) = open(&svc, ws_request(&format!("ws://site-a.test:{}/", srv.port), PAGE));
    opened(&rx);
    assert_eq!(next(&rx), failed());
    wait_for("the server's log", || !srv.log.lock().unwrap().is_empty());
    let log = srv.log.lock().unwrap().clone();
    log
}

#[test]
fn e7_protocol_violations_close_with_the_right_code() {
    // A masked server frame.
    assert_eq!(misbehave(|p| p.send(0x81, b"masked", Some([1, 2, 3, 4]))), ["close 1002"]);
    // Reserved bits, an unknown opcode, an oversized or fragmented control frame.
    assert_eq!(misbehave(|p| p.send(0xC1, b"rsv1", None)), ["close 1002"]);
    assert_eq!(misbehave(|p| p.send(0x83, b"op3", None)), ["close 1002"]);
    assert_eq!(misbehave(|p| p.send(0x89, &[0u8; 126], None)), ["close 1002"]);
    assert_eq!(misbehave(|p| p.send(0x09, b"ping", None)), ["close 1002"]);
    // A continuation without a start.
    assert_eq!(misbehave(|p| p.send(0x80, b"cont", None)), ["close 1002"]);
    // Invalid UTF-8 in a text message.
    assert_eq!(misbehave(|p| p.send(0x81, &[0x68, 0xff, 0xfe], None)), ["close 1007"]);
    // A message over 16 MiB: refused from its header, before the payload.
    assert_eq!(
        misbehave(|p| {
            let mut h = vec![0x82, 127];
            h.extend_from_slice(&(17u64 * 1024 * 1024).to_be_bytes());
            let _ = p.s.write_all(&h);
        }),
        ["close 1009"]
    );
    // ... also when it gets there in fragments.
    assert_eq!(
        misbehave(|p| {
            let part = vec![0u8; 4 * 1024 * 1024];
            p.send(0x02, &part, None);
            for _ in 0..4 {
                p.send(0x00, &part, None);
            }
        }),
        ["close 1009"]
    );
    // An unmasked server frame is fine: echoed back by the echo server elsewhere.
}

#[test]
fn e7_buffered_goes_up_and_back_to_zero() {
    let srv = ws_server(|mut p, head, log| {
        let _ = p.s.write_all(&accept(&head, ""));
        thread::sleep(Duration::from_millis(800)); // let the client's sends pile up
        let mut total = 0;
        while total < 8 * 1024 * 1024 {
            let Some(f) = p.frame() else { break };
            total += f.payload.len();
        }
        log.lock().unwrap().push(format!("{total}"));
        let _ = p.until_close();
    });
    let svc = service(local_cfg());
    let (ws, rx) = open(&svc, ws_request(&format!("ws://site-a.test:{}/", srv.port), PAGE));
    opened(&rx);
    for _ in 0..8 {
        ws.send_binary(vec![7u8; 1024 * 1024]);
    }
    assert!(ws.buffered() > 0, "nothing was counted as buffered");
    wait_for("buffered() to drain", || ws.buffered() == 0);
    wait_for("the server to read everything", || !srv.log.lock().unwrap().is_empty());
    assert_eq!(*srv.log.lock().unwrap(), [format!("{}", 8 * 1024 * 1024)]);
    ws.close(Some(1000), "");
}
