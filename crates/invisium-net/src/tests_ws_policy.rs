//! Milestone 3b, E8: WebSocket requests go through the privacy pipeline and the configured
//! route, with no fallback to a direct connection.

use crate::tests_cors::{hdr, reply};
use crate::tests_m2::{fake_connect_proxy, fake_socks5};
use crate::tests_service::*;
use crate::tests_ws::*;
use crate::*;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Opens a socket that must be refused; asserts the single 1006 event.
fn refused(svc: &NetService, req: WsRequest) {
    let url = req.url.to_string();
    let (_ws, rx) = open(svc, req);
    assert_eq!(next(&rx), failed(), "{url}");
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "{url}: more than one event");
}

#[test]
fn e8_pipeline_refusals_make_no_connection() {
    let srv = echo_server();
    let p = srv.port;
    // Mixed content: ws: from an https: page (HTTPS-only off, so nothing upgrades it).
    let svc = service(local_cfg());
    refused(&svc, ws_request(&format!("ws://site-a.test:{p}/"), "https://site-a.test/"));
    // A tracker-listed host.
    refused(&svc, ws_request(&format!("ws://doubleclick.net:{p}/"), PAGE));
    // A public page to local targets, by address and by a name that resolves locally.
    svc.map_host("rebind.test", LOCAL);
    for host in ["127.0.0.1", "localhost", "[::1]", "rebind.test"] {
        refused(&svc, ws_request(&format!("ws://{host}:{p}/"), PAGE));
    }
    // A public frame inside a local tab is not a local page either.
    let mut req = ws_request(&format!("ws://127.0.0.1:{p}/"), &format!("http://127.0.0.1:{p}/"));
    req.initiator = Some(Url::parse("http://site-b.test/frame").unwrap());
    refused(&svc, req);
    // Third-party sockets under BlockAll.
    let strict = service(NetConfig { third_party: ThirdPartyPolicy::BlockAll, ..local_cfg() });
    refused(&strict, ws_request(&format!("ws://site-b.test:{p}/"), PAGE));
    // Other schemes and fragments.
    refused(&svc, ws_request(&format!("http://site-a.test:{p}/"), PAGE));
    refused(&svc, ws_request(&format!("ws://site-a.test:{p}/#frag"), PAGE));
    assert_eq!(srv.conns.load(Ordering::SeqCst), 0, "the server was contacted");
    // What is allowed works: a local page to a local target, a same-site socket under BlockAll.
    let (ws, rx) = open(&svc, ws_request(&format!("ws://127.0.0.1:{p}/"), &format!("http://127.0.0.1:{p}/")));
    opened(&rx);
    ws.close(Some(1000), "");
    let (ws, rx) = open(&strict, ws_request(&format!("ws://site-a.test:{p}/"), PAGE));
    opened(&rx);
    ws.close(Some(1000), "");
}

#[test]
fn e8_https_only_upgrades_ws_and_never_downgrades() {
    // A plain TCP listener: an upgraded socket speaks TLS to it and fails.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    let first: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let f2 = first.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut b = [0u8; 16];
            let n = s.read(&mut b).unwrap_or(0);
            f2.lock().unwrap().push(b[..n].to_vec());
        }
    });
    let svc = service(NetConfig { https_only: true, ..local_cfg() });
    refused(&svc, ws_request(&format!("ws://site-a.test:{p}/"), PAGE));
    let got = first.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].first(), Some(&0x16), "expected a TLS handshake, got {:?}", got[0]);
}

#[test]
fn e8_origin_cookies_and_clear_site_data() {
    let srv = echo_server();
    let p = srv.port;
    let svc = service(local_cfg());
    // A cookie for site-a, set by a first-party response.
    let set = serve(|_| reply("200 OK", &[("Set-Cookie", "sid=42")], ""));
    fetch(&svc, get(&set.url("site-a.test", "/"))).unwrap();
    let mut req = ws_request(&format!("ws://site-a.test:{p}/"), PAGE);
    let (ws, rx) = open(&svc, req.clone());
    opened(&rx);
    ws.close(Some(1000), "");
    // From a frame: Origin is the frame's origin; cookies still follow the tab.
    req.initiator = Some(Url::parse("http://site-b.test:9000/frame").unwrap());
    let (ws, rx) = open(&svc, req);
    opened(&rx);
    // Third-party socket under FirstPartyOnly: no cookie.
    let (ws3, rx3) = open(&svc, ws_request(&format!("ws://site-a.test:{p}/"), "http://site-b.test/"));
    opened(&rx3);
    let heads = srv.heads.lock().unwrap().clone();
    assert_eq!(hdr(&heads[0], "Origin"), Some("http://site-a.test"));
    assert_eq!(hdr(&heads[0], "Cookie"), Some("sid=42"));
    assert_eq!(hdr(&heads[1], "Origin"), Some("http://site-b.test:9000"));
    assert_eq!(hdr(&heads[1], "Cookie"), Some("sid=42"));
    assert_eq!(hdr(&heads[2], "Cookie"), None);
    // The fixed header set, identical for every user.
    for name in ["User-Agent", "Accept-Language", "Sec-WebSocket-Version", "Upgrade", "Connection"] {
        assert!(hdr(&heads[0], name).is_some(), "{name}");
    }
    assert!(!heads[0].contains("Sec-WebSocket-Extensions"));
    // clear_site_data closes every open socket with 1001.
    svc.clear_site_data();
    for rx in [&rx, &rx3] {
        assert_eq!(next(rx), WsEvent::Closed { code: 1001, reason: String::new(), clean: true });
    }
    drop((ws, ws3));
}

#[test]
fn e8_proxies_carry_the_socket_and_never_fall_back() {
    let srv = echo_server();
    let p = srv.port;
    // "proxied.test" resolves nowhere: only the proxy can reach it.
    let url = format!("ws://proxied.test:{p}/chat");
    let (http_port, http_log) = fake_connect_proxy(None);
    let (socks_port, socks_log) = fake_socks5(None);
    let proxies = [
        ProxyConfig::Http { host: "127.0.0.1".into(), port: http_port, auth: None },
        ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: socks_port, auth: None },
    ];
    for proxy in proxies {
        let svc = service(NetConfig { proxy, connect_timeout_ms: 10_000, ..local_cfg() });
        let (ws, rx) = open(&svc, ws_request(&url, PAGE));
        opened(&rx);
        ws.send_text("through the proxy".into());
        assert_eq!(next(&rx), WsEvent::Text("through the proxy".into()));
        ws.close(Some(1000), "");
        assert!(matches!(next(&rx), WsEvent::Closed { code: 1000, clean: true, .. }));
    }
    assert!(http_log.lock().unwrap()[0].starts_with(&format!("CONNECT proxied.test:{p} HTTP/1.1")));
    assert_eq!(*socks_log.lock().unwrap(), ["domain:proxied.test"]);
    assert_eq!(srv.conns.load(Ordering::SeqCst), 2, "every connection came through a proxy");
    // A dead proxy: the socket fails, nothing goes direct.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let svc = service(NetConfig {
        proxy: ProxyConfig::Http { host: "127.0.0.1".into(), port: dead, auth: None },
        ..local_cfg()
    });
    svc.map_host("proxied.test", LOCAL);
    refused(&svc, ws_request(&url, PAGE));
    assert_eq!(srv.conns.load(Ordering::SeqCst), 2);
}
