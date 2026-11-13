//! Milestone 2: fake CONNECT / SOCKS5 proxies (B2), pooling (B5) including the retirement of
//! connections that were in flight during a route or identity change, service-level caching
//! (B4), Tor fail-closed without the feature, and the ignored live tests (B6).

use crate::tests_service::*;
use crate::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) type Log = Arc<Mutex<Vec<String>>>;

fn relay(client: TcpStream, port: u16) {
    let Ok(upstream) = TcpStream::connect(("127.0.0.1", port)) else { return };
    let (mut c2, mut u2) = (client.try_clone().unwrap(), upstream.try_clone().unwrap());
    let (mut c, mut u) = (client, upstream);
    thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut u2);
        let _ = u2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut u, &mut c);
    let _ = c.shutdown(std::net::Shutdown::Write);
}

/// Fake HTTP CONNECT proxy: logs the CONNECT head, "resolves" every name to 127.0.0.1.
pub(crate) fn fake_connect_proxy(basic: Option<&'static str>) -> (u16, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let log2 = log.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let log = log2.clone();
            thread::spawn(move || {
                let head = read_head(&mut s);
                log.lock().unwrap().push(head.clone());
                if let Some(cred) = basic {
                    if !head.contains(&format!("Proxy-Authorization: Basic {cred}\r\n")) {
                        let _ = s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n");
                        return;
                    }
                }
                let target = head.split(' ').nth(1).unwrap_or("");
                let port: u16 = target.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(0);
                let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                relay(s, port);
            });
        }
    });
    (port, log)
}

#[derive(Clone, Copy)]
enum SocksAuth {
    None,
    Exact(&'static str, &'static str),
    /// Accepts every username/password (like Tor's SOCKS port) and logs it as "auth:user:pass".
    Any,
}

/// Fake SOCKS5 proxy (RFC 1928/1929): logs the requested address.
pub(crate) fn fake_socks5(auth: Option<(&'static str, &'static str)>) -> (u16, Log) {
    fake_socks5_with(auth.map_or(SocksAuth::None, |(u, p)| SocksAuth::Exact(u, p)))
}

fn fake_socks5_with(auth: SocksAuth) -> (u16, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let log2 = log.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let log = log2.clone();
            thread::spawn(move || {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).unwrap();
                let mut methods = vec![0u8; b[1] as usize];
                s.read_exact(&mut methods).unwrap();
                if !matches!(auth, SocksAuth::None) {
                    if !methods.contains(&2) {
                        let _ = s.write_all(&[5, 0xFF]);
                        return;
                    }
                    s.write_all(&[5, 2]).unwrap();
                    let mut v = [0u8; 2];
                    s.read_exact(&mut v).unwrap();
                    let mut u = vec![0u8; v[1] as usize];
                    s.read_exact(&mut u).unwrap();
                    let mut pl = [0u8; 1];
                    s.read_exact(&mut pl).unwrap();
                    let mut p = vec![0u8; pl[0] as usize];
                    s.read_exact(&mut p).unwrap();
                    let ok = match auth {
                        SocksAuth::Exact(user, pass) => u == user.as_bytes() && p == pass.as_bytes(),
                        _ => {
                            let (u, p) = (String::from_utf8_lossy(&u), String::from_utf8_lossy(&p));
                            log.lock().unwrap().push(format!("auth:{u}:{p}"));
                            true
                        }
                    };
                    s.write_all(&[1, if ok { 0 } else { 1 }]).unwrap();
                    if !ok {
                        return;
                    }
                } else {
                    s.write_all(&[5, 0]).unwrap();
                }
                let mut r = [0u8; 4];
                s.read_exact(&mut r).unwrap();
                let addr = match r[3] {
                    3 => {
                        let mut n = [0u8; 1];
                        s.read_exact(&mut n).unwrap();
                        let mut d = vec![0u8; n[0] as usize];
                        s.read_exact(&mut d).unwrap();
                        format!("domain:{}", String::from_utf8_lossy(&d))
                    }
                    1 => {
                        let mut d = [0u8; 4];
                        s.read_exact(&mut d).unwrap();
                        format!("ipv4:{d:?}")
                    }
                    t => format!("atyp:{t}"),
                };
                let mut p = [0u8; 2];
                s.read_exact(&mut p).unwrap();
                log.lock().unwrap().push(addr);
                s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                relay(s, u16::from_be_bytes(p));
            });
        }
    });
    (port, log)
}

fn via(proxy: ProxyConfig) -> NetService {
    service(NetConfig { proxy, connect_timeout_ms: 10_000, ..local_cfg() })
}

fn auth(u: &str, p: &str) -> Option<(String, String)> {
    Some((u.to_string(), p.to_string()))
}

#[test]
fn http_connect_proxy_relays_and_receives_the_hostname() {
    let origin = serve(|_| ok("via proxy"));
    let (pport, log) = fake_connect_proxy(None);
    let svc = via(ProxyConfig::Http { host: "127.0.0.1".into(), port: pport, auth: None });
    // ".invalid" can never resolve locally: success proves the proxy got the name.
    let r = fetch(&svc, get(&origin.url("origin.invalid", "/x"))).unwrap();
    assert_eq!((r.body.as_slice(), r.via), (&b"via proxy"[..], Route::Proxy));
    let head = &log.lock().unwrap()[0];
    assert!(head.starts_with(&format!("CONNECT origin.invalid:{} HTTP/1.1\r\n", origin.port)), "{head}");
    assert!(origin.heads()[0].contains(&format!("Host: origin.invalid:{}\r\n", origin.port)));
}

#[test]
fn http_connect_proxy_auth() {
    let origin = serve(|_| ok("ok"));
    let (pport, _) = fake_connect_proxy(Some("dXNlcjpwYXNz"));
    let good = via(ProxyConfig::Http { host: "127.0.0.1".into(), port: pport, auth: auth("user", "pass") });
    assert_eq!(fetch(&good, get(&origin.url("origin.invalid", "/"))).unwrap().status, 200);
    let bad = via(ProxyConfig::Http { host: "127.0.0.1".into(), port: pport, auth: None });
    let r = fetch(&bad, get(&origin.url("origin.invalid", "/")));
    assert!(matches!(r, Err(NetError::Proxy(_))), "{r:?}");
}

#[test]
fn socks5_proxy_relays_with_remote_resolution_and_auth() {
    let origin = serve(|_| ok("via socks"));
    let (pport, log) = fake_socks5(Some(("user", "pass")));
    let svc = via(ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: pport, auth: auth("user", "pass") });
    let r = fetch(&svc, get(&origin.url("origin.invalid", "/"))).unwrap();
    assert_eq!((r.body.as_slice(), r.via), (&b"via socks"[..], Route::Proxy));
    assert_eq!(log.lock().unwrap()[0], "domain:origin.invalid");
    let bad = via(ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: pport, auth: auth("user", "nope") });
    let r = fetch(&bad, get(&origin.url("origin.invalid", "/")));
    assert!(matches!(r, Err(NetError::Proxy(_))), "{r:?}");
    let none = via(ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: pport, auth: None });
    assert!(matches!(fetch(&none, get(&origin.url("origin.invalid", "/"))), Err(NetError::Proxy(_))));
}

#[test]
fn proxy_failure_is_a_proxy_error_never_a_direct_fallback() {
    let origin = serve(|_| ok("direct"));
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    for p in [
        ProxyConfig::Http { host: "127.0.0.1".into(), port: dead, auth: None },
        ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: dead, auth: None },
    ] {
        let r = fetch(&via(p), get(&origin.url("127.0.0.1", "/")));
        assert!(matches!(r, Err(NetError::Proxy(_))), "{r:?}");
    }
    assert!(origin.heads().is_empty());
}

#[cfg(not(feature = "tor"))]
#[test]
fn tor_without_feature_fails_closed() {
    let origin = serve(|_| ok("direct"));
    let svc = via(ProxyConfig::Tor);
    let r = fetch(&svc, get(&origin.url("127.0.0.1", "/")));
    assert!(matches!(r, Err(NetError::Tor(_))), "{r:?}");
    assert!(matches!(svc.tor_status(), TorStatus::Failed(_)));
    assert!(origin.heads().is_empty());
}

/// Keep-alive server: serves many requests per connection and counts connections.
fn keep_alive_server() -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let conns = Arc::new(AtomicUsize::new(0));
    let c2 = conns.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            c2.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || loop {
                if read_head(&mut s).is_empty() {
                    break;
                }
                if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").is_err() {
                    break;
                }
            });
        }
    });
    (port, conns)
}

#[test]
fn pool_reuses_connections_and_drops_them_on_proxy_change() {
    let (port, conns) = keep_alive_server();
    let svc = service(local_cfg());
    let u = format!("http://127.0.0.1:{port}/");
    fetch(&svc, get(&u)).unwrap();
    fetch(&svc, get(&u)).unwrap();
    assert_eq!(conns.load(Ordering::SeqCst), 1);
    assert_eq!(svc.stats().open_connections, 1);
    svc.set_config(NetConfig { referrer: ReferrerPolicy::Never, ..local_cfg() });
    assert_eq!(svc.stats().open_connections, 1, "unrelated change keeps the pool");
    svc.set_config(NetConfig {
        proxy: ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: 9, auth: None },
        ..local_cfg()
    });
    assert_eq!(svc.stats().open_connections, 0);
}

/// Keep-alive server on a tokio listener. Counts accepted connections; the response to a path
/// starting with "/slow" is held back until `release` is notified.
struct GatedServer {
    port: u16,
    conns: Arc<AtomicUsize>,
    /// One message for every delayed request that has reached the server.
    arrived: std::sync::mpsc::Receiver<()>,
    release: Arc<tokio::sync::Notify>,
}

fn gated_keep_alive_server() -> GatedServer {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let (conns, release) = (Arc::new(AtomicUsize::new(0)), Arc::new(tokio::sync::Notify::new()));
    let (tx, arrived) = std::sync::mpsc::channel();
    let (conns2, release2) = (conns.clone(), release.clone());
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(l).unwrap();
            while let Ok((mut s, _)) = l.accept().await {
                conns2.fetch_add(1, Ordering::SeqCst);
                let (tx, release) = (tx.clone(), release2.clone());
                tokio::spawn(async move {
                    loop {
                        let (mut head, mut b) = (Vec::new(), [0u8; 1]);
                        while !head.ends_with(b"\r\n\r\n") {
                            match s.read(&mut b).await {
                                Ok(1) => head.push(b[0]),
                                _ => return,
                            }
                        }
                        if path_of(&String::from_utf8_lossy(&head)).starts_with("/slow") {
                            let _ = tx.send(());
                            release.notified().await;
                        }
                        if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
    });
    GatedServer { port, conns, arrived, release }
}

/// Runs two rounds of "delayed request, then a second request to the same origin", calling
/// `mid_flight` while the server is holding the delayed response back. In round one the delayed
/// request dials its own connection, in round two it takes one from the pool. Returns, per
/// round, (connections still open once the delayed response is in, TCP connections accepted
/// by the server after the second request).
fn two_rounds_with(mid_flight: impl Fn(&NetService)) -> Vec<(usize, usize)> {
    let srv = gated_keep_alive_server();
    let svc = service(local_cfg());
    let url = |kind: &str, round: u8| format!("http://127.0.0.1:{}/{kind}{round}", srv.port);
    let wait = Duration::from_secs(10);
    (0..2)
        .map(|round| {
            let (tx, rx) = std::sync::mpsc::channel();
            svc.fetch(get(&url("slow", round)), move |r| {
                let _ = tx.send(r);
            });
            srv.arrived.recv_timeout(wait).expect("the delayed request did not reach the server");
            mid_flight(&svc);
            srv.release.notify_one();
            let delayed = rx.recv_timeout(wait).expect("done was not called").unwrap();
            assert_eq!(delayed.body, b"ok");
            let open = svc.stats().open_connections;
            assert_eq!(fetch(&svc, get(&url("next", round))).unwrap().body, b"ok");
            (open, srv.conns.load(Ordering::SeqCst))
        })
        .collect()
}

#[test]
fn connection_in_flight_during_a_route_or_identity_change_is_never_reused() {
    // Control: nothing changes, so the delayed request's connection is pooled and reused.
    assert_eq!(two_rounds_with(|_| {}), [(1, 1), (1, 1)]);
    // A DNS change resets the pool but leaves the pool key as it was: without the generation
    // check the next request would get the old connection back.
    let toggle_dns = |svc: &NetService| {
        let mut cfg = svc.config();
        cfg.dns = if cfg.dns == DnsMode::System { NetConfig::default().dns } else { DnsMode::System };
        svc.set_config(cfg);
    };
    assert_eq!(two_rounds_with(toggle_dns), [(0, 2), (0, 3)], "set_config");
    assert_eq!(two_rounds_with(NetService::new_identity), [(0, 2), (0, 3)], "new_identity");
    assert_eq!(two_rounds_with(NetService::clear_site_data), [(0, 2), (0, 3)], "clear_site_data");
    // A change that does not touch the route keeps the connection.
    let unrelated = |svc: &NetService| svc.set_config(NetConfig { referrer: ReferrerPolicy::Never, ..local_cfg() });
    assert_eq!(two_rounds_with(unrelated), [(1, 1), (1, 1)], "unrelated set_config");
}

#[test]
fn socks5_connections_are_not_shared_between_credentials() {
    let (oport, _) = keep_alive_server();
    let (pport, log) = fake_socks5_with(SocksAuth::Any);
    let with = |user: &str| NetConfig {
        proxy: ProxyConfig::Socks5 { host: "127.0.0.1".into(), port: pport, auth: auth(user, "pw") },
        connect_timeout_ms: 10_000,
        ..local_cfg()
    };
    // One "auth:" line per SOCKS connection the proxy accepted.
    let handshakes = || -> Vec<String> {
        log.lock().unwrap().iter().filter(|l| l.starts_with("auth:")).cloned().collect()
    };
    let u = |n: u8| format!("http://origin.invalid:{oport}/{n}");
    let svc = service(with("alice"));
    fetch(&svc, get(&u(1))).unwrap();
    fetch(&svc, get(&u(2))).unwrap();
    assert_eq!(handshakes(), ["auth:alice:pw"], "same credentials reuse the proxied connection");
    // Same proxy host and port, other credentials: a new SOCKS connection with those.
    svc.set_config(with("bob"));
    fetch(&svc, get(&u(3))).unwrap();
    assert_eq!(handshakes(), ["auth:alice:pw", "auth:bob:pw"]);
    fetch(&svc, get(&u(4))).unwrap();
    assert_eq!(handshakes().len(), 2, "and that one is reused in turn");
}

#[test]
fn service_cache_hit_partition_and_no_store() {
    let srv = serve(|h| match path_of(h) {
        "/fresh" => b"HTTP/1.1 200 OK\r\nCache-Control: max-age=60\r\nContent-Length: 1\r\n\r\nf".to_vec(),
        _ => b"HTTP/1.1 200 OK\r\nCache-Control: no-store\r\nContent-Length: 1\r\n\r\nn".to_vec(),
    });
    let svc = service(local_cfg());
    let fresh = srv.url("site-a.test", "/fresh");
    assert!(!fetch(&svc, get(&fresh)).unwrap().from_cache);
    let hit = fetch(&svc, get(&fresh)).unwrap();
    assert!(hit.from_cache && hit.via == Route::Cache && hit.body == b"f");
    assert_eq!(srv.heads().len(), 1);
    assert!(svc.stats().cache_bytes > 0);
    // Same resource under another top-level site: separate partition, goes to the network.
    let r = fetch(&svc, sub(&fresh, &srv.url("site-b.test", "/"), ResourceKind::Image)).unwrap();
    assert!(!r.from_cache);
    let ns = srv.url("site-a.test", "/nostore");
    fetch(&svc, get(&ns)).unwrap();
    assert!(!fetch(&svc, get(&ns)).unwrap().from_cache);
    assert_eq!(srv.heads().len(), 4);
    svc.clear_site_data();
    assert_eq!(svc.stats().cache_bytes, 0);
}

#[test]
#[ignore = "live internet access"]
fn live_doh_default_provider_example_com() {
    use crate::dns::{decode_response, TYPE_A};
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let tls = crate::connect::tls_config();
    let timeout = Duration::from_secs(10);
    let q = |p| {
        rt.block_on(crate::doh::doh_query(&tls, p, "example.com", TYPE_A, timeout))
            .and_then(|msg| decode_response(&msg, TYPE_A))
            .map(|ans| (ans.addrs, ans.min_ttl))
    };
    // Quad9 only speaks HTTP/2: printed for diagnosis, never allowed to fail the test.
    eprintln!("example.com via Quad9 DoH (diagnostic only): {:?}", q(DohProvider::Quad9));
    let DnsMode::Doh { provider, .. } = NetConfig::default().dns else {
        panic!("the Standard preset must resolve over DoH");
    };
    assert_eq!(provider, DohProvider::Cloudflare);
    let answer = q(provider);
    eprintln!("example.com via the default DoH provider ({provider:?}): {answer:?}");
    let (ips, ttl) = answer.unwrap();
    assert!(!ips.is_empty() && ttl > 0, "{ips:?}");
    // The whole resolver path, strict so that it cannot be the system resolver answering.
    let strict = DnsMode::Doh { provider, strict: true };
    let addrs = rt.block_on(crate::doh::Resolver::default().resolve(&tls, strict, "example.com", 443, timeout));
    assert!(addrs.as_ref().is_ok_and(|a| !a.is_empty()), "{addrs:?}");
}

#[cfg(feature = "tor")]
#[test]
#[ignore = "live Tor network access"]
fn live_tor_check() {
    let dir = std::env::temp_dir().join("invisium-net-tor-test");
    let svc = NetService::new(NetConfig { proxy: ProxyConfig::Tor, data_dir: dir, ..NetConfig::default() });
    let (tx, rx) = std::sync::mpsc::channel();
    svc.fetch(get("https://check.torproject.org/api/ip"), move |r| {
        let _ = tx.send(r);
    });
    let r = rx.recv_timeout(Duration::from_secs(240)).expect("Tor fetch did not finish").unwrap();
    let body = String::from_utf8_lossy(&r.body).into_owned();
    eprintln!("Tor check: {body}");
    assert_eq!(r.via, Route::Tor);
    assert!(body.replace(' ', "").contains("\"IsTor\":true"), "{body}");
    assert_eq!(svc.tor_status(), TorStatus::Ready);
}
