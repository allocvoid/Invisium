//! Milestone 3b, E2 (initiators), E3 (framing rules), E4 (local page memory) and E9 leftovers
//! 1-3, against local listeners.

use crate::tests_cors::{hdr, reply, script};
use crate::tests_service::*;
use crate::*;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PRIVATE: NetError = NetError::Blocked(BlockReason::PrivateNetwork);
const FRAMING: NetError = NetError::Blocked(BlockReason::Framing);
const CORS: NetError = NetError::Blocked(BlockReason::Cors);

fn framed(mut r: Request, initiator: &str) -> Request {
    r.initiator = Some(Url::parse(initiator).unwrap());
    r
}

#[test]
fn e2_initiator_is_the_requesting_origin() {
    // Echoes the Origin it received, and allows only the frame's origin.
    let api = serve(|h| {
        let origin = hdr(h, "Origin").unwrap_or("");
        let allow = if origin.starts_with("http://site-b.test:") { origin } else { "http://nobody.test" };
        reply("200 OK", &[("Access-Control-Allow-Origin", allow)], "data")
    });
    let p = api.port;
    let svc = service(local_cfg());
    let top = "http://www.site-a.test:8080/app";
    let frame = format!("http://site-b.test:{p}/frame");
    // Cors from the frame to the tab's site: Origin is the frame's origin, checked against it.
    let r = fetch(&svc, framed(script(&api.url("site-a.test", "/x"), top, FetchMode::Cors, Credentials::Omit), &frame));
    assert_eq!(r.unwrap().body, b"data");
    assert_eq!(hdr(&api.heads()[0], "Origin"), Some(format!("http://site-b.test:{p}").as_str()));
    // Without the initiator the tab's origin is sent, and this server does not allow it.
    let r = fetch(&svc, script(&api.url("site-a.test", "/x"), top, FetchMode::Cors, Credentials::Omit));
    assert_eq!(r.unwrap_err(), CORS);
    assert_eq!(hdr(&api.heads()[1], "Origin"), Some("http://www.site-a.test:8080"));
    // Same origin as the frame: no Origin header, no CORS check.
    let r = fetch(&svc, framed(script(&api.url("site-b.test", "/own"), top, FetchMode::Cors, Credentials::Omit), &frame));
    assert!(r.is_ok());
    assert_eq!(hdr(&api.heads()[2], "Origin"), None);
    // SameOrigin mode compares with the frame, not the tab.
    let same = |url: &str| framed(script(url, top, FetchMode::SameOrigin, Credentials::Include), &frame);
    assert!(fetch(&svc, same(&api.url("site-b.test", "/own"))).is_ok());
    assert_eq!(fetch(&svc, same("http://www.site-a.test:8080/app/api")).unwrap_err(), CORS);
    assert_eq!(api.heads().len(), 4);
}

#[test]
fn e2_cookies_stay_partitioned_by_the_tab() {
    let srv = serve(|h| match path_of(h) {
        "/set" => reply("200 OK", &[("Set-Cookie", "f=1; Path=/")], ""),
        _ => reply("200 OK", &[], hdr(h, "Cookie").unwrap_or("-")),
    });
    let svc = service(local_cfg());
    let (a, b) = (srv.url("site-a.test", "/top"), srv.url("site-b.test", "/frame"));
    // A site-b frame inside site-a's tab talks to site-a: first-party for the tab (third-party
    // decisions stay with the tab), and stored in site-a's partition, not the frame's.
    let in_frame = |path: &str| framed(sub(&srv.url("site-a.test", path), &a, ResourceKind::Image), &b);
    fetch(&svc, in_frame("/set")).unwrap();
    assert_eq!(fetch(&svc, in_frame("/echo")).unwrap().body, b"f=1");
    let tab_a = sub(&srv.url("site-a.test", "/echo"), &a, ResourceKind::Image);
    assert_eq!(fetch(&svc, tab_a).unwrap().body, b"f=1");
    // A tab of the frame's own site has a partition of its own.
    let tab_b = sub(&srv.url("site-a.test", "/echo"), &b, ResourceKind::Image);
    assert_eq!(fetch(&svc, tab_b).unwrap().body, b"-");
    assert_eq!(svc.script_cookies(&Url::parse(&srv.url("site-a.test", "/")).unwrap(), &Url::parse(&b).unwrap()), "");
}

#[test]
fn e2_both_documents_must_be_local_to_reach_a_local_target() {
    let srv = serve(|_| ok("local"));
    let p = srv.port;
    let svc = service(local_cfg());
    let target = format!("http://127.0.0.1:{p}/secret");
    let (local, public) = (format!("http://127.0.0.1:{p}/"), "http://site-a.test/".to_string());
    for (top, initiator) in [(&local, &public), (&public, &local)] {
        let r = fetch(&svc, framed(sub(&target, top, ResourceKind::Image), initiator));
        assert_eq!(r.unwrap_err(), PRIVATE, "top {top}, frame {initiator}");
    }
    assert!(srv.heads().is_empty());
    assert!(fetch(&svc, framed(sub(&target, &local, ResourceKind::Image), &local)).is_ok());
}

fn framing_server() -> Server {
    serve(|h| {
        let path = path_of(h).to_string();
        let port = hdr(h, "Host").and_then(|v| v.rsplit(':').next()).unwrap_or("").to_string();
        let csp = |v: &str| ("Content-Security-Policy", v.to_string());
        let headers: Vec<(&str, String)> = match path.as_str() {
            "/deny" => vec![("X-Frame-Options", "DENY".into())],
            "/sameorigin" => vec![("X-Frame-Options", "SAMEORIGIN".into())],
            "/junk" => vec![("X-Frame-Options", "ALLOW-FROM http://x.test".into())],
            "/none" => vec![csp("frame-ancestors 'none'")],
            "/self" => vec![csp("frame-ancestors 'self'")],
            "/host" => vec![csp(&format!("default-src 'self'; frame-ancestors http://site-a.test:{port}"))],
            "/wild" => vec![csp("frame-ancestors *.site-a.test:*")],
            "/precedence" => vec![("X-Frame-Options", "DENY".into()), csp("frame-ancestors *")],
            "/two" => vec![csp("frame-ancestors *"), csp("frame-ancestors 'self'")],
            "/two-in-one" => vec![csp("frame-ancestors *, frame-ancestors 'self'")],
            _ => vec![],
        };
        let refs: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        reply("200 OK", &refs, "frame body")
    })
}

#[test]
fn e3_framing_rules() {
    let srv = framing_server();
    let svc = service(local_cfg());
    let p = srv.port;
    let top_a = format!("http://site-a.test:{p}/top");
    let top_www = format!("http://www.site-a.test:{p}/top");
    let frame = |host: &str, path: &str, top: &str| fetch(&svc, sub(&srv.url(host, path), top, ResourceKind::Frame));
    let allowed = |host: &str, path: &str, top: &str| frame(host, path, top).map(|r| r.body);
    // (frame host, path, top, allowed)
    let cases: &[(&str, &str, &str, bool)] = &[
        ("site-a.test", "/plain", &top_a, true),
        ("site-a.test", "/deny", &top_a, false),
        ("site-a.test", "/sameorigin", &top_a, true),
        ("site-b.test", "/sameorigin", &top_a, false),
        ("site-b.test", "/junk", &top_a, true),
        ("site-a.test", "/none", &top_a, false),
        ("site-a.test", "/self", &top_a, true),
        ("site-b.test", "/self", &top_a, false),
        ("site-b.test", "/host", &top_a, true),
        ("site-b.test", "/host", &top_www, false),
        ("site-b.test", "/wild", &top_www, true),
        ("site-b.test", "/wild", &top_a, false),
        ("site-a.test", "/precedence", &top_a, true),
        ("site-a.test", "/two", &top_a, true),
        ("site-b.test", "/two", &top_a, false),
        ("site-b.test", "/two-in-one", &top_a, false),
    ];
    for (host, path, top, ok) in cases {
        let r = allowed(host, path, top);
        if *ok {
            assert_eq!(r.unwrap(), b"frame body", "{host}{path} under {top}");
        } else {
            // A refused frame exposes nothing: no status, headers or body.
            assert_eq!(r.unwrap_err(), FRAMING, "{host}{path} under {top}");
        }
    }
    // None of the rules applies to other resource kinds.
    for kind in [ResourceKind::Document, ResourceKind::Image, ResourceKind::Xhr, ResourceKind::Other] {
        for path in ["/deny", "/none", "/two"] {
            let r = fetch(&svc, sub(&srv.url("site-b.test", path), &top_a, kind));
            assert!(r.is_ok(), "{path} as {kind:?}");
        }
    }
    assert!(fetch(&svc, get(&srv.url("site-a.test", "/deny"))).is_ok());
}

#[test]
fn e4_local_pages_by_flag_not_by_memory() {
    let srv = serve(|h| match path_of(h) {
        "/cached" => reply("200 OK", &[("Cache-Control", "max-age=600")], "cached"),
        _ => ok("local"),
    });
    let p = srv.port;
    let svc = service(local_cfg());
    svc.map_host("intranet.test", LOCAL);
    let page = format!("http://intranet.test:{p}/");
    let target = format!("http://127.0.0.1:{p}/data");
    let from = |top: &str, top_local: bool| {
        let mut r = sub(&target, top, ResourceKind::Image);
        r.top_local = top_local;
        fetch(&svc, r)
    };
    // A navigation answered by a local listener through a host name is served_local.
    let nav = fetch(&svc, get(&page)).unwrap();
    assert!(nav.served_local && nav.via == Route::Direct);
    // With the flag that page reaches local subresources; without it, it does not: nothing
    // is remembered between requests.
    assert_eq!(from(&page, true).unwrap().body, b"local");
    assert_eq!(from(&page, false).unwrap_err(), PRIVATE);
    assert_eq!(from(&page, true).unwrap().body, b"local");
    // Both documents must be local: the frame's flag counts for the frame.
    let frame = format!("http://intranet.test:{p}/f");
    let in_frame = |initiator_local: bool| {
        let mut r = framed(sub(&target, &page, ResourceKind::Image), &frame);
        r.top_local = true;
        r.initiator_local = initiator_local;
        fetch(&svc, r)
    };
    assert!(in_frame(true).is_ok());
    assert_eq!(in_frame(false).unwrap_err(), PRIVATE);
    // A name resolving to a public and a local address, where the socket lands on the public
    // one, is not served_local.
    svc.map_public_and_local_host("split.test", LOCAL);
    let split = fetch(&svc, get(&format!("http://split.test:{p}/"))).unwrap();
    assert!(!split.served_local);
    // A public server is not; and a cached response keeps the value it was stored with.
    let public = format!("http://site-a.test:{p}/cached");
    let first = fetch(&svc, get(&public)).unwrap();
    let again = fetch(&svc, get(&public)).unwrap();
    assert!(!first.served_local && again.from_cache && !again.served_local);
    let cached_local = format!("http://intranet.test:{p}/cached");
    let (a, b) = (fetch(&svc, get(&cached_local)).unwrap(), fetch(&svc, get(&cached_local)).unwrap());
    assert!(a.served_local && b.served_local, "local content is never cached, so it stays local");
    assert!(!b.from_cache);
}

#[test]
fn e4_proxy_routes_are_never_served_local() {
    let srv = serve(|_| ok("via proxy"));
    let (port, _log) = crate::tests_m2::fake_connect_proxy(None);
    let proxy = ProxyConfig::Http { host: "127.0.0.1".into(), port, auth: None };
    let svc = service(NetConfig { proxy, ..local_cfg() });
    let r = fetch(&svc, get(&srv.url("127.0.0.1", "/"))).unwrap();
    assert!(r.via == Route::Proxy && !r.served_local);
}

#[test]
fn e9_proxy_debug_hides_credentials() {
    let secret = Some(("alice-user".to_string(), "hunter2-pass".to_string()));
    for p in [
        ProxyConfig::Http { host: "proxy.test".into(), port: 8080, auth: secret.clone() },
        ProxyConfig::Socks5 { host: "proxy.test".into(), port: 1080, auth: secret.clone() },
    ] {
        let cfg = NetConfig { proxy: p.clone(), ..NetConfig::default() };
        for text in [format!("{p:?}"), format!("{p:#?}"), format!("{cfg:?}")] {
            assert!(!text.contains("alice-user") && !text.contains("hunter2-pass"), "{text}");
            assert!(text.contains("***") && text.contains("proxy.test"), "{text}");
        }
    }
    assert_eq!(format!("{:?}", ProxyConfig::Tor), "Tor");
}

#[test]
fn e9_names_dns_cannot_encode_fail_at_once() {
    let srv = serve(|_| ok("x"));
    let svc = service(local_cfg());
    let label64 = format!("{}.test", "a".repeat(64));
    let label63 = format!("{}.test", "a".repeat(63));
    for host in [&label64, &label63] {
        svc.map_host(host, LOCAL);
    }
    let r = fetch(&svc, get(&srv.url(&label64, "/")));
    assert!(matches!(r, Err(NetError::Dns(_))), "{r:?}");
    assert!(srv.heads().is_empty());
    assert!(fetch(&svc, get(&srv.url(&label63, "/"))).is_ok());
    // A name over 253 bytes cannot even be written in a Url; the dialler refuses it too.
    let long = format!("{}test", "abcdefghi.".repeat(25)); // 254 bytes
    assert!(Url::parse(&format!("http://{long}/")).is_err());
    assert!(matches!(crate::connect::check_dns_name(&long), Err(NetError::Dns(_))));
    assert!(crate::connect::check_dns_name(&long[1..]).is_ok());
    assert!(crate::connect::check_dns_name("[::1]").is_ok());
}

#[test]
fn e9_in_flight_response_stores_nothing_after_clear_site_data() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    let (arrived_tx, arrived) = mpsc::channel();
    let (go, go_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let head = read_head(&mut s);
            let path = path_of(&head).to_string();
            if path.starts_with("/gated") {
                arrived_tx.send(()).unwrap();
                go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            let resp = if path.ends_with("cookie") {
                reply("200 OK", &[("Set-Cookie", "id=1")], "body")
            } else {
                reply("200 OK", &[("Cache-Control", "max-age=600")], "body")
            };
            let _ = s.write_all(&resp);
        }
    });
    let svc = service(local_cfg());
    let url = |path: &str| format!("http://site-a.test:{p}{path}");
    for path in ["/gated-cookie", "/gated-cache"] {
        let (tx, rx) = mpsc::channel();
        svc.fetch(get(&url(path)), move |r| tx.send(r).unwrap());
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        svc.clear_site_data();
        go.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap().is_ok());
        let s = svc.stats();
        assert_eq!((s.cookies, s.cache_bytes), (0, 0), "{path}");
    }
    // Control: the same responses outside that window are stored.
    assert!(fetch(&svc, get(&url("/cookie"))).is_ok());
    assert!(fetch(&svc, get(&url("/cache"))).is_ok());
    let s = svc.stats();
    assert!(s.cookies == 1 && s.cache_bytes > 0, "{s:?}");
}
