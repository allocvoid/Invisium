//! Milestone 3a, D7 (private-network protection) and D8 (script access to cookies), against
//! local listeners.

use crate::tests_cors::{hdr, reply, script};
use crate::tests_service::*;
use crate::*;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PRIVATE: NetError = NetError::Blocked(BlockReason::PrivateNetwork);
const MODES: [FetchMode; 3] = [FetchMode::Subresource, FetchMode::SameOrigin, FetchMode::Cors];

fn open_server() -> Server {
    serve(|_| reply("200 OK", &[("Access-Control-Allow-Origin", "*")], "local"))
}

#[test]
fn d7_public_pages_cannot_reach_local_targets() {
    let srv = open_server();
    let svc = service(local_cfg());
    let p = srv.port;
    // A public page; even one that claims the listener's own port.
    let page = format!("http://site-a.test:{p}/");
    let from_public = |url: &str, mode| fetch(&svc, script(url, &page, mode, Credentials::Include));
    let targets = [
        "127.0.0.1", "[::1]", "localhost", "10.0.0.5", "192.168.1.1", "169.254.1.1", "[::ffff:127.0.0.1]",
        // Not in the charter's list: other spellings of 127.0.0.1 and a *.localhost name.
        "127.1", "0x7f000001", "2130706433", "app.localhost",
    ];
    for host in targets {
        for mode in MODES {
            let r = from_public(&format!("http://{host}:{p}/secret"), mode);
            assert_eq!(r.unwrap_err(), PRIVATE, "{host} in {mode:?}");
        }
    }
    // DNS rebinding, through the resolver seam: a public-looking name that resolves to a
    // local address is refused before connecting. Asked for by another public page ...
    svc.map_host("rebind.test", LOCAL);
    let rebind = format!("http://rebind.test:{p}/secret");
    for mode in [FetchMode::Subresource, FetchMode::Cors] {
        assert_eq!(from_public(&rebind, mode).unwrap_err(), PRIVATE, "rebind in {mode:?}");
    }
    // ... and by a page of that very name (same origin, so SameOrigin mode gets this far too).
    let rebind_page = format!("http://rebind.test:{p}/");
    for mode in MODES {
        let r = fetch(&svc, script(&rebind, &rebind_page, mode, Credentials::Include));
        assert_eq!(r.unwrap_err(), PRIVATE, "own name in {mode:?}");
    }
    assert!(srv.heads().is_empty(), "the listener received {:?}", srv.heads());
    assert_eq!(svc.stats().blocked as usize, targets.len() * MODES.len() + 2 + MODES.len());

    // A redirect from a public server to a local target is refused at that hop too.
    let bounce = serve(move |_| reply("302 Found", &[("Location", &format!("http://127.0.0.1:{p}/secret"))], ""));
    let r = fetch(&svc, sub(&bounce.url("site-b.test", "/"), &page, ResourceKind::Image));
    assert_eq!(r.unwrap_err(), PRIVATE);
    assert_eq!(bounce.heads().len(), 1);
    assert!(srv.heads().is_empty());

    // A top-level navigation to a local address still works, by literal and by name.
    assert_eq!(fetch(&svc, get(&format!("http://127.0.0.1:{p}/nav"))).unwrap().body, b"local");
    assert_eq!(fetch(&svc, get(&format!("http://rebind.test:{p}/nav"))).unwrap().body, b"local");
    assert_eq!(srv.heads().len(), 2);
}

#[test]
fn d7_local_pages_may_fetch_local_targets() {
    let srv = open_server();
    let svc = service(local_cfg());
    let p = srv.port;
    svc.map_host("rebind.test", LOCAL);
    for page_host in ["127.0.0.1", "localhost"] {
        let page = format!("http://{page_host}:{p}/");
        let from_local = |url: String, mode| fetch(&svc, script(&url, &page, mode, Credentials::Omit));
        // Its own origin in every mode.
        for mode in MODES {
            let r = from_local(format!("http://{page_host}:{p}/own"), mode).unwrap();
            assert_eq!(r.body, b"local", "{page_host} in {mode:?}");
        }
        // Other local targets, by literal and by a name that resolves locally.
        for target in ["127.0.0.1", "localhost", "rebind.test"] {
            for mode in [FetchMode::Subresource, FetchMode::Cors] {
                let r = from_local(format!("http://{target}:{p}/other"), mode).unwrap();
                assert_eq!(r.body, b"local", "{page_host} -> {target} in {mode:?}");
            }
        }
    }
    assert_eq!(srv.heads().len(), 2 * (3 + 3 * 2));
}

/// Keep-alive server that records request heads.
fn keep_alive_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let heads: Arc<Mutex<Vec<String>>> = Arc::default();
    let h2 = heads.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let heads = h2.clone();
            thread::spawn(move || loop {
                let head = read_head(&mut s);
                if head.is_empty() {
                    break;
                }
                heads.lock().unwrap().push(head);
                if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").is_err() {
                    break;
                }
            });
        }
    });
    (port, heads)
}

#[test]
fn d7_a_pooled_connection_to_a_local_address_is_not_lent_to_a_public_page() {
    let (port, heads) = keep_alive_server();
    let svc = service(local_cfg());
    svc.map_host("rebind.test", LOCAL);
    let url = format!("http://rebind.test:{port}/");
    // A navigation leaves an idle keep-alive connection to the local address in the pool.
    assert_eq!(fetch(&svc, get(&url)).unwrap().body, b"ok");
    assert_eq!(svc.stats().open_connections, 1);
    // A public page of the same name (same origin, so every mode reaches the pool).
    for mode in MODES {
        let r = fetch(&svc, script(&url, &url, mode, Credentials::Include));
        assert_eq!(r.unwrap_err(), PRIVATE, "{mode:?}");
    }
    let r = fetch(&svc, sub(&url, "http://site-a.test/", ResourceKind::Image));
    assert_eq!(r.unwrap_err(), PRIVATE);
    assert_eq!(heads.lock().unwrap().len(), 1);
    // The connection is still there for those who may use it.
    assert_eq!(svc.stats().open_connections, 1);
    assert_eq!(fetch(&svc, get(&url)).unwrap().body, b"ok");
    assert_eq!((heads.lock().unwrap().len(), svc.stats().open_connections), (2, 1));
}

#[test]
fn d7_the_cache_never_hands_local_content_to_a_public_page() {
    let srv = serve(|_| reply("200 OK", &[("Cache-Control", "max-age=60")], "local"));
    let svc = service(local_cfg());
    svc.map_host("rebind.test", LOCAL);
    let url = format!("http://rebind.test:{}/fresh", srv.port);
    // Navigations reach the local address; what comes from there is never cached ...
    assert!(!fetch(&svc, get(&url)).unwrap().from_cache);
    assert!(!fetch(&svc, get(&url)).unwrap().from_cache);
    assert_eq!(svc.stats().cache_bytes, 0);
    // ... so a public page in the same cache partition cannot get it from the cache.
    assert_eq!(fetch(&svc, sub(&url, &url, ResourceKind::Image)).unwrap_err(), PRIVATE);
    assert_eq!(srv.heads().len(), 2);
}

fn cookie_line(head: &str) -> Option<&str> {
    hdr(head, "Cookie")
}

#[test]
fn d8_script_cookies() {
    let srv = serve(|h| match path_of(h) {
        "/set" => reply("200 OK", &[("Set-Cookie", "sid=secret; Path=/; HttpOnly"), ("Set-Cookie", "vis=1; Path=/")], ""),
        "/set-b" => reply("200 OK", &[("Set-Cookie", "other=9; Path=/")], ""),
        _ => ok("x"),
    });
    let svc = service(local_cfg()); // FirstPartyOnly
    let u = |s: &str| Url::parse(s).unwrap();
    let (a, b) = (u(&srv.url("site-a.test", "/page")), u(&srv.url("site-b.test", "/page")));
    let sent_to_a = || {
        fetch(&svc, get(&srv.url("site-a.test", "/check"))).unwrap();
        cookie_line(srv.heads().last().unwrap()).map(String::from)
    };
    fetch(&svc, get(&srv.url("site-a.test", "/set"))).unwrap();
    fetch(&svc, get(&srv.url("site-b.test", "/set-b"))).unwrap();

    // Reading: no HttpOnly cookies, no other site's cookies.
    assert_eq!(svc.script_cookies(&a, &a), "vis=1");
    assert_eq!(svc.script_cookies(&b, &b), "other=9");
    assert_eq!(svc.script_cookies(&u(&srv.url("www.site-a.test", "/")), &a), "");
    // Embedded under another top-level site: nothing under FirstPartyOnly.
    assert_eq!(svc.script_cookies(&a, &b), "");
    svc.set_script_cookie(&a, &b, "embedded=1");
    assert_eq!(svc.stats().cookies, 3);

    // Writing round-trips through a later request's Cookie header.
    svc.set_script_cookie(&a, &a, "js=2; Path=/");
    assert_eq!(svc.script_cookies(&a, &a), "vis=1; js=2");
    assert_eq!(sent_to_a().as_deref(), Some("sid=secret; vis=1; js=2"));
    svc.set_script_cookie(&a, &a, "js=3; Path=/");
    svc.set_script_cookie(&a, &a, "vis=; Max-Age=0; Path=/");
    assert_eq!(svc.script_cookies(&a, &a), "js=3");
    assert_eq!(sent_to_a().as_deref(), Some("sid=secret; js=3"));

    // A script can neither create an HttpOnly cookie nor overwrite or delete one.
    svc.set_script_cookie(&a, &a, "made=1; Path=/; HttpOnly");
    svc.set_script_cookie(&a, &a, "sid=hijacked; Path=/");
    svc.set_script_cookie(&a, &a, "sid=; Path=/; Max-Age=0");
    svc.set_script_cookie(&a, &a, "sid=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT");
    // Nor smuggle a header into later requests, or store nonsense.
    svc.set_script_cookie(&a, &a, "inj=1\r\nX-Evil: 1");
    svc.set_script_cookie(&a, &a, "no-equals-sign");
    svc.set_script_cookie(&u("data:text/html,x"), &a, "d=1");
    assert_eq!(svc.script_cookies(&a, &a), "js=3");
    assert_eq!(sent_to_a().as_deref(), Some("sid=secret; js=3"));
    assert_eq!(svc.stats().cookies, 3); // sid, js, other

    // Partitioning: the same document under another top-level site has its own jar.
    svc.set_config(NetConfig { cookies: CookiePolicy::AllowPartitioned, third_party: ThirdPartyPolicy::Allow, ..local_cfg() });
    assert_eq!(svc.script_cookies(&a, &b), "");
    svc.set_script_cookie(&a, &b, "part=1; Secure; SameSite=None");
    assert_eq!(svc.stats().cookies, 3, "Secure cookie from an http page must be rejected");
    // (*.localhost is a trustworthy origin, so Secure; SameSite=None cookies can exist there.)
    let (la, lb) = (u(&srv.url("a.localhost", "/page")), u(&srv.url("b.localhost", "/page")));
    svc.set_script_cookie(&la, &lb, "part=1; Secure; SameSite=None");
    svc.set_script_cookie(&la, &lb, "lax=1"); // SameSite cookie from a cross-site context
    svc.set_script_cookie(&la, &la, "own=1");
    assert_eq!(svc.script_cookies(&la, &lb), "part=1");
    assert_eq!(svc.script_cookies(&la, &la), "own=1");
    assert_eq!(svc.script_cookies(&lb, &lb), "");
    assert_eq!(svc.stats().cookies, 5);
    // The request pipeline agrees: each top-level site sends only its own partition.
    let check = srv.url("a.localhost", "/check");
    fetch(&svc, sub(&check, lb.as_str(), ResourceKind::Image)).unwrap();
    assert_eq!(cookie_line(srv.heads().last().unwrap()), Some("part=1"));
    fetch(&svc, get(&check)).unwrap();
    assert_eq!(cookie_line(srv.heads().last().unwrap()), Some("own=1"));
    assert_eq!(svc.script_cookies(&a, &a), "js=3");
    assert_eq!(svc.script_cookies(&b, &b), "other=9");

    // CookiePolicy::BlockAll: nothing is stored and nothing is readable.
    let blocked = service(NetConfig { cookies: CookiePolicy::BlockAll, ..local_cfg() });
    blocked.set_script_cookie(&a, &a, "js=2; Path=/");
    assert_eq!((blocked.stats().cookies, blocked.script_cookies(&a, &a).as_str()), (0, ""));
    svc.set_config(NetConfig { cookies: CookiePolicy::BlockAll, ..local_cfg() });
    assert_eq!(svc.script_cookies(&a, &a), "");
}
