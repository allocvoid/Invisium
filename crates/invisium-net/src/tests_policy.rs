//! A4 policy tests (inspecting what the local server received) and the A5 live test.

use crate::tests_service::*;
use crate::*;
use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn cookie_line(head: &str) -> Option<&str> {
    head.lines().find_map(|l| l.strip_prefix("Cookie: "))
}

fn cookie_server() -> Server {
    serve(|h| match path_of(h) {
        // SameSite=None so that only partitioning / policy (not SameSite) decides what is sent.
        "/set" => b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=abc; Path=/; HttpOnly; Secure; SameSite=None\r\nContent-Length: 0\r\n\r\n".to_vec(),
        "/set2" => b"HTTP/1.1 200 OK\r\nSet-Cookie: other=1; Path=/; Secure; SameSite=None\r\nContent-Length: 0\r\n\r\n".to_vec(),
        _ => ok("x"),
    })
}

#[test]
fn tracker_host_is_blocked_but_not_as_navigation() {
    let srv = serve(|_| ok("x"));
    let svc = service(local_cfg());
    let top = srv.url("site-a.test", "/");
    for host in ["doubleclick.net", "ad.g.doubleclick.net"] {
        let r = fetch(&svc, sub(&srv.url(host, "/px"), &top, ResourceKind::Image));
        assert_eq!(r.unwrap_err(), NetError::Blocked(BlockReason::Tracker));
    }
    assert!(srv.heads().is_empty());
    assert_eq!(svc.stats().blocked, 2);
    assert_eq!(fetch(&svc, get(&srv.url("doubleclick.net", "/"))).unwrap().status, 200);
}

#[test]
fn first_party_cookie_round_trips_and_is_isolated_per_top_level_site() {
    let srv = cookie_server();
    let svc = service(local_cfg()); // FirstPartyOnly
    fetch(&svc, get(&srv.url("a.localhost", "/set"))).unwrap();
    fetch(&svc, get(&srv.url("a.localhost", "/check"))).unwrap();
    // Same server as a subresource of another top-level site, with partitioning allowed.
    svc.set_config(NetConfig {
        cookies: CookiePolicy::AllowPartitioned,
        third_party: ThirdPartyPolicy::Allow,
        ..local_cfg()
    });
    let top_b = srv.url("b.localhost", "/");
    fetch(&svc, sub(&srv.url("a.localhost", "/check"), &top_b, ResourceKind::Image)).unwrap();
    fetch(&svc, get(&srv.url("b.localhost", "/check"))).unwrap();
    let heads = srv.heads();
    assert_eq!(cookie_line(&heads[0]), None);
    assert_eq!(cookie_line(&heads[1]), Some("sid=abc"));
    assert_eq!(cookie_line(&heads[2]), None, "cookie leaked across top-level sites");
    assert_eq!(cookie_line(&heads[3]), None);
    assert_eq!(svc.stats().cookies, 1);
    svc.clear_site_data();
    assert_eq!(svc.stats().cookies, 0);
}

#[test]
fn third_party_request_carries_no_cookie_under_first_party_only() {
    let srv = cookie_server();
    let partitioned = NetConfig {
        cookies: CookiePolicy::AllowPartitioned,
        third_party: ThirdPartyPolicy::Allow,
        ..local_cfg()
    };
    let svc = service(partitioned.clone());
    let top_b = srv.url("b.localhost", "/");
    let third = |p: &str| sub(&srv.url("a.localhost", p), &top_b, ResourceKind::Image);
    fetch(&svc, third("/set")).unwrap(); // stored in site-b's partition
    fetch(&svc, third("/check")).unwrap(); // sanity: sent when allowed
    svc.set_config(NetConfig { third_party: ThirdPartyPolicy::Allow, ..local_cfg() });
    fetch(&svc, third("/check")).unwrap(); // FirstPartyOnly: not sent
    fetch(&svc, third("/set2")).unwrap(); // FirstPartyOnly: not stored
    svc.set_config(partitioned);
    fetch(&svc, third("/check")).unwrap();
    let heads = srv.heads();
    assert_eq!(cookie_line(&heads[1]), Some("sid=abc"));
    assert_eq!(cookie_line(&heads[2]), None);
    assert_eq!(cookie_line(&heads[3]), None);
    assert_eq!(cookie_line(&heads[4]), Some("sid=abc"));
}

#[test]
fn referer_is_trimmed_to_origin_cross_site() {
    let srv = serve(|_| ok("x"));
    let svc = service(local_cfg());
    let page = srv.url("site-a.test", "/page?q=1#frag");
    let with_ref = |u: &str| {
        let mut r = sub(u, &page, ResourceKind::Image);
        r.referrer = Some(Url::parse(&page).unwrap());
        r
    };
    fetch(&svc, with_ref(&srv.url("site-b.test", "/img"))).unwrap();
    fetch(&svc, with_ref(&srv.url("www.site-a.test", "/img"))).unwrap();
    svc.set_config(NetConfig { referrer: ReferrerPolicy::Never, ..local_cfg() });
    fetch(&svc, with_ref(&srv.url("site-b.test", "/img"))).unwrap();
    let heads = srv.heads();
    let referer = |h: &str| h.lines().find_map(|l| l.strip_prefix("Referer: ")).map(String::from);
    assert_eq!(referer(&heads[0]), Some(format!("http://site-a.test:{}/", srv.port)));
    assert_eq!(referer(&heads[1]), Some(format!("http://site-a.test:{}/page?q=1", srv.port)));
    assert_eq!(referer(&heads[2]), None);
}

#[test]
fn tracking_params_are_stripped() {
    let srv = serve(|_| ok("x"));
    let svc = service(local_cfg());
    let u = srv.url("site-a.test", "/p?utm_source=x&id=7&fbclid=y&UTM_Medium=z&gclid=1");
    let r = fetch(&svc, get(&u)).unwrap();
    assert!(srv.heads()[0].starts_with("GET /p?id=7 HTTP/1.1\r\n"));
    assert_eq!(r.url.query(), Some("id=7"));
}

#[test]
fn headers_are_identical_across_instances() {
    let srv = serve(|_| ok("x"));
    let u = srv.url("127.0.0.1", "/");
    for _ in 0..2 {
        let svc = service(local_cfg());
        let mut req = get(&u);
        req.headers = vec![
            ("User-Agent".into(), "leaky/1.0".into()),
            ("Cookie".into(), "x=1".into()),
            ("X-Bad".into(), "a\r\nInjected: 1".into()),
        ];
        fetch(&svc, req).unwrap();
    }
    let heads = srv.heads();
    assert_eq!(heads[0].as_bytes(), heads[1].as_bytes());
    let expected = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUser-Agent: {}\r\n\
         Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n\
         Accept-Language: en-US,en;q=0.5\r\nAccept-Encoding: gzip, deflate, br\r\n\
         DNT: 1\r\nSec-GPC: 1\r\nConnection: keep-alive\r\n\r\n",
        srv.port,
        crate::policy::GENERIC_UA
    );
    assert_eq!(heads[0], expected);
}

#[test]
fn https_only_rewrites_the_scheme() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut b = [0u8; 1];
            let _ = tx.send(s.read(&mut b).map(|_| b[0]).ok());
        }
    });
    let svc = service(NetConfig { connect_timeout_ms: 5000, ..NetConfig::default() });
    let r = fetch(&svc, get(&format!("http://127.0.0.1:{port}/")));
    assert_eq!(r.unwrap_err(), NetError::Blocked(BlockReason::HttpsOnly));
    // 0x16 = TLS handshake record: the client spoke https on the rewritten URL.
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(0x16));
    let mut u = Url::parse("http://example.com/x").unwrap();
    assert!(crate::policy::check(&NetConfig::default(), &mut u, None, ResourceKind::Document).unwrap());
    assert_eq!(u.as_str(), "https://example.com/x");
}

#[test]
fn scheme_and_mixed_content_blocks() {
    let svc = service(local_cfg());
    let r = fetch(&svc, get("ftp://example.com/file"));
    assert_eq!(r.unwrap_err(), NetError::Blocked(BlockReason::Scheme));
    let r = fetch(&svc, sub("http://site-a.test/x.js", "https://site-a.test/", ResourceKind::Script));
    assert_eq!(r.unwrap_err(), NetError::Blocked(BlockReason::MixedContent));
    let svc = service(NetConfig { third_party: ThirdPartyPolicy::BlockAll, ..local_cfg() });
    let r = fetch(&svc, sub("http://site-b.test/x.js", "http://site-a.test/", ResourceKind::Script));
    assert_eq!(r.unwrap_err(), NetError::Blocked(BlockReason::ThirdParty));
}

#[test]
#[ignore = "live internet access"]
fn live_example_com() {
    let svc = NetService::new(NetConfig::default());
    let r = fetch(&svc, get("https://example.com/")).unwrap();
    assert_eq!(r.status, 200);
    assert!(String::from_utf8_lossy(&r.body).contains("Example Domain"));
}
