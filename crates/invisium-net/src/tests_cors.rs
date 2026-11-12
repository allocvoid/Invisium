//! Milestone 3a, D2-D6: script requests against local listeners, asserting what the servers
//! received as well as what the caller got. Public-looking host names go through the test
//! resolver seam (`service()`), so private-network protection sees public servers.

use crate::tests_service::*;
use crate::*;
use std::cell::Cell;

pub(crate) const CORS: NetError = NetError::Blocked(BlockReason::Cors);
/// A page that is never fetched itself; `ORIGIN` is its origin.
const PAGE: &str = "http://www.site-a.test:8080/app/index.html";
const ORIGIN: &str = "http://www.site-a.test:8080";

pub(crate) fn hdr<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
}

fn method_of(head: &str) -> &str {
    head.split(' ').next().unwrap_or("")
}

/// A response with extra headers that closes the connection.
pub(crate) fn reply(status: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut s = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (n, v) in headers {
        s.push_str(&format!("{n}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s.into_bytes()
}

pub(crate) fn script(url: &str, page: &str, mode: FetchMode, credentials: Credentials) -> Request {
    let mut r = sub(url, page, ResourceKind::Xhr);
    r.mode = mode;
    r.credentials = credentials;
    r
}

#[test]
fn d2_same_origin_mode_never_leaves_the_origin() {
    let foreign = serve(|_| ok("foreign"));
    let fport = foreign.port;
    let home = serve(move |h| match path_of(h) {
        "/redir" => reply("302 Found", &[("Location", &format!("http://site-b.test:{fport}/x"))], ""),
        _ => ok("home"),
    });
    let svc = service(local_cfg());
    let page = home.url("site-a.test", "/page");
    let same = |u: &str| script(u, &page, FetchMode::SameOrigin, Credentials::Include);
    let r = fetch(&svc, same(&home.url("site-a.test", "/api"))).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"home"[..]));
    assert_eq!(hdr(&home.heads()[0], "Origin"), None);
    // Cross-origin first hop: another site, another host, another port, another scheme.
    let elsewhere = [
        foreign.url("site-b.test", "/x"),
        foreign.url("www.site-a.test", "/x"),
        foreign.url("site-a.test", "/x"),
        format!("https://site-a.test:{}/api", home.port),
    ];
    for u in &elsewhere {
        assert_eq!(fetch(&svc, same(u)).unwrap_err(), CORS, "{u}");
    }
    // Cross-origin redirect: the same-origin hop is made, the foreign one is not.
    assert_eq!(fetch(&svc, same(&home.url("site-a.test", "/redir"))).unwrap_err(), CORS);
    // A script request without a page is refused outright.
    for mode in [FetchMode::SameOrigin, FetchMode::Cors] {
        let mut orphan = get(&home.url("site-a.test", "/api"));
        orphan.mode = mode;
        assert_eq!(fetch(&svc, orphan).unwrap_err(), CORS);
    }
    let paths: Vec<String> = home.heads().iter().map(|h| path_of(h).to_string()).collect();
    assert_eq!(paths, ["/api", "/redir"]);
    assert!(foreign.heads().is_empty(), "the foreign server received {:?}", foreign.heads());
}

fn simple_api() -> Server {
    serve(|h| {
        let origin = hdr(h, "Origin").unwrap_or("");
        let acao = ("Access-Control-Allow-Origin", origin);
        let acac = ("Access-Control-Allow-Credentials", "true");
        match path_of(h) {
            "/set" => reply("200 OK", &[("Set-Cookie", "sid=abc; Path=/")], ""),
            "/match" => reply("200 OK", &[acao], "data"),
            "/star" => reply("200 OK", &[("Access-Control-Allow-Origin", "*")], "data"),
            "/mismatch" => reply("200 OK", &[("Access-Control-Allow-Origin", "http://other.test")], "data"),
            "/dup" => reply("200 OK", &[acao, acao], "data"),
            "/creds" => reply("200 OK", &[acao, acac], "data"),
            "/creds-setx" => reply("200 OK", &[acao, acac, ("Set-Cookie", "x=1; Path=/")], "data"),
            "/fail-sety" => reply("200 OK", &[("Set-Cookie", "y=1; Path=/")], "data"),
            _ => reply("200 OK", &[], "data"),
        }
    })
}

#[test]
fn d3_cors_simple_requests() {
    let api = simple_api();
    let svc = service(local_cfg());
    // Same site as the page, but another origin: cookies are first-party, CORS still applies.
    let cors = |path: &str, c| fetch(&svc, script(&api.url("site-a.test", path), PAGE, FetchMode::Cors, c));
    for path in ["/match", "/star"] {
        let r = cors(path, Credentials::Omit).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"data"[..]), "{path}");
    }
    for path in ["/none", "/mismatch", "/dup"] {
        assert_eq!(cors(path, Credentials::Omit).unwrap_err(), CORS, "{path}");
    }
    // With credentials: `*` fails, and a matching origin needs Allow-Credentials: true.
    assert_eq!(cors("/star", Credentials::Include).unwrap_err(), CORS);
    assert_eq!(cors("/match", Credentials::Include).unwrap_err(), CORS);
    assert_eq!(cors("/creds", Credentials::Include).unwrap().body, b"data");
    // Every one was a simple request: sent once, with Origin, without a preflight.
    let heads = api.heads();
    assert_eq!(heads.len(), 8);
    for h in &heads {
        assert!(h.starts_with("GET /"), "{h}");
        assert_eq!(hdr(h, "Origin"), Some(ORIGIN), "{h}");
        assert_eq!(hdr(h, "Cookie"), None, "{h}");
    }
    // Put a cookie for the API host in the jar, then ask with each credentials mode.
    fetch(&svc, get(&api.url("site-a.test", "/set"))).unwrap();
    assert_eq!(svc.stats().cookies, 1);
    for c in [Credentials::Omit, Credentials::SameOrigin, Credentials::Include] {
        assert_eq!(cors("/creds", c).unwrap().status, 200, "{c:?}");
    }
    let heads = api.heads();
    assert_eq!(hdr(&heads[9], "Cookie"), None, "Omit sent a cookie cross-origin");
    assert_eq!(hdr(&heads[10], "Cookie"), None, "SameOrigin sent a cookie cross-origin");
    assert_eq!(hdr(&heads[11], "Cookie"), Some("sid=abc"));
    // ... and nothing is stored without credentials, or from a response that fails the check.
    cors("/creds-setx", Credentials::Omit).unwrap();
    cors("/creds-setx", Credentials::SameOrigin).unwrap();
    assert_eq!(cors("/fail-sety", Credentials::Include).unwrap_err(), CORS);
    assert_eq!(svc.stats().cookies, 1);
    cors("/creds-setx", Credentials::Include).unwrap();
    assert_eq!(svc.stats().cookies, 2);
}

fn preflight_api() -> Server {
    serve(|h| {
        let origin = hdr(h, "Origin").unwrap_or("");
        let acao = ("Access-Control-Allow-Origin", origin);
        let acac = ("Access-Control-Allow-Credentials", "true");
        if method_of(h) != "OPTIONS" {
            return match path_of(h) {
                "/set" => reply("200 OK", &[("Set-Cookie", "sid=abc; Path=/")], ""),
                _ => reply("200 OK", &[acao, acac], "actual"),
            };
        }
        let allow = |methods: &'static str, headers: &'static str| {
            [acao, acac, ("Access-Control-Allow-Methods", methods), ("Access-Control-Allow-Headers", headers)]
        };
        match path_of(h) {
            "/ok" => reply("204 No Content", &allow("PUT, DELETE", "X-Custom, X-Another, Content-Type"), ""),
            "/p404" => reply("404 Not Found", &allow("PUT", "x-custom"), ""),
            "/nomethod" => reply("200 OK", &allow("DELETE", "x-custom"), ""),
            "/noheader" => reply("200 OK", &allow("PUT", "x-other"), ""),
            "/star" => {
                let any = [("Access-Control-Allow-Origin", "*"), ("Access-Control-Allow-Methods", "*"), ("Access-Control-Allow-Headers", "*")];
                reply("200 OK", &any, "")
            }
            "/redirect" => {
                let a = allow("PUT", "x-custom");
                reply("302 Found", &[a[0], a[1], a[2], a[3], ("Location", "/ok")], "")
            }
            _ => reply("200 OK", &[], ""),
        }
    })
}

#[test]
fn d4_preflight() {
    use Credentials::{Include, Omit};
    let api = preflight_api();
    let svc = service(local_cfg());
    let send = |method: Method, path: &str, headers: &[(&str, &str)], c: Credentials| {
        let mut r = script(&api.url("site-a.test", path), PAGE, FetchMode::Cors, c);
        r.method = method;
        r.headers = headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        fetch(&svc, r)
    };
    // The request heads the server received since the previous call.
    let seen = Cell::new(0);
    let new_heads = || {
        let all = api.heads();
        all[seen.replace(all.len())..].to_vec()
    };
    let acrh = |h: &str| hdr(h, "Access-Control-Request-Headers").map(String::from);
    // A cookie for the API host, to prove the preflight never carries credentials.
    fetch(&svc, get(&api.url("site-a.test", "/set"))).unwrap();
    new_heads();

    // PUT: preflight first, then the actual request.
    assert_eq!(send(Method::Put, "/ok", &[], Include).unwrap().body, b"actual");
    let h = new_heads();
    assert_eq!(h.len(), 2);
    assert!(h[0].starts_with("OPTIONS /ok HTTP/1.1\r\n") && h[1].starts_with("PUT /ok HTTP/1.1\r\n"), "{h:?}");
    assert_eq!(hdr(&h[0], "Access-Control-Request-Method"), Some("PUT"));
    assert_eq!((acrh(&h[0]), hdr(&h[0], "Origin")), (None, Some(ORIGIN)));
    assert_eq!((hdr(&h[0], "Cookie"), hdr(&h[0], "Content-Length")), (None, None));
    assert_eq!((hdr(&h[1], "Origin"), hdr(&h[1], "Cookie")), (Some(ORIGIN), Some("sid=abc")));

    // A custom header: named in the preflight (lower-case, sorted), sent on the actual request.
    let custom = [("X-Custom", "1"), ("X-Another", "2"), ("Accept", "text/plain")];
    assert_eq!(send(Method::Get, "/ok", &custom, Omit).unwrap().body, b"actual");
    let h = new_heads();
    assert!(h[0].starts_with("OPTIONS /ok ") && h[1].starts_with("GET /ok ") && h.len() == 2, "{h:?}");
    assert_eq!(hdr(&h[0], "Access-Control-Request-Method"), Some("GET"));
    assert_eq!(acrh(&h[0]).as_deref(), Some("x-another,x-custom"));
    assert_eq!((hdr(&h[0], "X-Custom"), hdr(&h[1], "X-Custom"), hdr(&h[1], "X-Another")), (None, Some("1"), Some("2")));

    // Content-Type: application/json is not safelisted.
    let json = [("Content-Type", "application/json")];
    assert_eq!(send(Method::Post, "/ok", &json, Include).unwrap().body, b"actual");
    let h = new_heads();
    assert!(h[0].starts_with("OPTIONS /ok ") && h[1].starts_with("POST /ok ") && h.len() == 2, "{h:?}");
    assert_eq!(acrh(&h[0]).as_deref(), Some("content-type"));
    assert_eq!(hdr(&h[1], "Content-Type"), Some("application/json"));

    // A safelisted POST goes out without a preflight.
    let form = [("Content-Type", "text/plain;charset=UTF-8"), ("Accept", "text/plain")];
    assert_eq!(send(Method::Post, "/ok", &form, Include).unwrap().body, b"actual");
    let h = new_heads();
    assert!(h.len() == 1 && h[0].starts_with("POST /ok "), "{h:?}");

    // Every failure mode: Blocked(Cors), and the server never sees the actual request.
    let x = [("X-Custom", "1")];
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], Credentials);
    let failures: [Case; 6] = [
        ("/p404", &x, Include),      // non-2xx
        ("/nomethod", &x, Include),  // method not allowed
        ("/noheader", &x, Include),  // header not allowed
        ("/star", &x, Include),      // wildcards with credentials
        ("/redirect", &x, Include),  // preflight redirect
        ("/silent", &[], Omit),      // no CORS headers at all
    ];
    for (path, headers, c) in failures {
        assert_eq!(send(Method::Put, path, headers, c).unwrap_err(), CORS, "{path}");
        let h = new_heads();
        assert!(h.len() == 1 && h[0].starts_with(&format!("OPTIONS {path} ")), "{path}: {h:?}");
    }
    // The same wildcards are fine without credentials.
    assert_eq!(send(Method::Put, "/star", &x, Omit).unwrap().body, b"actual");
    let h = new_heads();
    assert!(h.len() == 2 && h[1].starts_with("PUT /star ") && hdr(&h[1], "Cookie").is_none(), "{h:?}");
}

/// The port the request was addressed to, from its Host header.
fn port_of(head: &str) -> &str {
    hdr(head, "Host").and_then(|h| h.rsplit(':').next()).unwrap_or("")
}

#[test]
fn d5_redirects_are_checked_hop_by_hop() {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;
    // `home` is the page's own origin (www.site-a.test); `far` plays two foreign origins,
    // site-b.test and site-a.test, on its port.
    let far_port = Arc::new(AtomicU16::new(0));
    let fp = far_port.clone();
    let home = serve(move |h| {
        let far = fp.load(Ordering::SeqCst);
        let to = |loc: String| reply("302 Found", &[("Location", &loc)], "");
        match path_of(h) {
            "/go-ok" => to(format!("http://site-b.test:{far}/allowed")),
            "/go-denied" => to(format!("http://site-b.test:{far}/denied")),
            "/go-userinfo" => to(format!("http://user:pw@site-b.test:{far}/allowed")),
            "/go-data" => to("data:text/plain,hello".to_string()),
            _ => ok("home"),
        }
    });
    let origin = format!("http://www.site-a.test:{}", home.port);
    let real = origin.clone();
    let far = serve(move |h| {
        let acao = ("Access-Control-Allow-Origin", hdr(h, "Origin").unwrap_or(""));
        let next = |path: &str| format!("http://site-a.test:{}{path}", port_of(h));
        match path_of(h) {
            "/allowed" => reply("200 OK", &[acao], "from far"),
            "/hop" => reply("302 Found", &[acao, ("Location", &next("/final"))], ""),
            "/hop-denied" => reply("302 Found", &[("Location", &next("/final"))], "secret"),
            "/final" => reply("200 OK", &[acao], "final"),
            // Answers with the page's real origin, which is not what a tainted chain sends.
            "/hop-real" => reply("302 Found", &[acao, ("Location", &next("/final-real"))], ""),
            "/final-real" => reply("200 OK", &[("Access-Control-Allow-Origin", &real)], "x"),
            _ => reply("200 OK", &[], "denied"),
        }
    });
    far_port.store(far.port, Ordering::SeqCst);
    let svc = service(local_cfg());
    let page = home.url("www.site-a.test", "/app");
    let cors = |u: String| fetch(&svc, script(&u, &page, FetchMode::Cors, Credentials::Omit));
    let far_paths = || far.heads().iter().map(|h| path_of(h).to_string()).collect::<Vec<_>>();

    // Same-origin hop, then a cross-origin one: Origin appears and the check applies there.
    let r = cors(home.url("www.site-a.test", "/go-ok")).unwrap();
    assert_eq!((r.body.as_slice(), r.redirects, r.url.host()), (&b"from far"[..], 1, "site-b.test"));
    assert_eq!(hdr(&home.heads()[0], "Origin"), None);
    assert_eq!(hdr(&far.heads()[0], "Origin"), Some(origin.as_str()));
    assert_eq!(cors(home.url("www.site-a.test", "/go-denied")).unwrap_err(), CORS);
    assert_eq!(far_paths(), ["/allowed", "/denied"]);
    // Redirect targets with userinfo, or off the network, are refused before anything is sent.
    assert_eq!(cors(home.url("www.site-a.test", "/go-userinfo")).unwrap_err(), CORS);
    assert_eq!(cors(home.url("www.site-a.test", "/go-data")).unwrap_err(), CORS);
    assert_eq!(far_paths().len(), 2);

    // Foreign A (site-b.test) redirects to foreign B (site-a.test): Origin is "null" from then on.
    let r = cors(far.url("site-b.test", "/hop")).unwrap();
    assert_eq!((r.body.as_slice(), r.redirects, r.url.host()), (&b"final"[..], 1, "site-a.test"));
    let heads = far.heads();
    assert_eq!((path_of(&heads[2]), hdr(&heads[2], "Origin")), ("/hop", Some(origin.as_str())));
    assert_eq!((path_of(&heads[3]), hdr(&heads[3], "Origin")), ("/final", Some("null")));
    // B allowing the page's real origin does not match the "null" that was sent.
    assert_eq!(cors(far.url("site-b.test", "/hop-real")).unwrap_err(), CORS);
    assert_eq!(far_paths()[4..], ["/hop-real", "/final-real"]);
    // A hop that fails the check exposes nothing: its redirect is not even followed.
    assert_eq!(cors(far.url("site-b.test", "/hop-denied")).unwrap_err(), CORS);
    assert_eq!(far_paths()[6..], ["/hop-denied"]);
}

#[test]
fn d6_headers_scripts_may_not_set_or_see() {
    let srv = serve(|h| {
        let headers = [
            ("Access-Control-Allow-Origin", hdr(h, "Origin").unwrap_or("")),
            ("Set-Cookie", "s=1; Path=/"),
            ("Set-Cookie2", "t=2"),
            ("Content-Type", "text/plain"),
            ("Cache-Control", "no-store"),
            ("X-Secret", "s"),
            ("X-Shown", "1"),
            ("ETag", "\"e\""),
            ("Access-Control-Expose-Headers", "X-Shown, Set-Cookie"),
        ];
        reply("200 OK", &headers, "body")
    });
    let svc = service(local_cfg());
    let forbidden = [
        ("Accept-Charset", "x"), ("Accept-Encoding", "identity"), ("Access-Control-Request-Method", "PUT"),
        ("Access-Control-Request-Headers", "x"), ("Connection", "close"), ("Content-Length", "99"),
        ("Cookie", "evil=1"), ("Cookie2", "x"), ("Date", "x"), ("DNT", "0"), ("Expect", "100-continue"),
        ("Host", "evil.test"), ("Keep-Alive", "x"), ("Origin", "http://evil.test"), ("Referer", "http://evil.test/"),
        ("TE", "trailers"), ("Trailer", "x"), ("Transfer-Encoding", "chunked"), ("Upgrade", "websocket"),
        ("User-Agent", "leaky/1.0"), ("Via", "x"), ("Proxy-Authorization", "x"), ("Sec-Fetch-Site", "same-origin"),
    ];
    let request = |page: &str, mode, credentials, allowed: &[(&str, &str)]| {
        let mut r = script(&srv.url("site-a.test", "/h"), page, mode, credentials);
        r.headers = forbidden.iter().chain(allowed).map(|(n, v)| (n.to_string(), v.to_string())).collect();
        fetch(&svc, r).unwrap()
    };
    let fixed = |origin: &str, tail: &str| {
        format!(
            "GET /h HTTP/1.1\r\nHost: site-a.test:{}\r\nUser-Agent: {}\r\nAccept: */*\r\n\
             Accept-Language: en-US,en;q=0.5\r\nAccept-Encoding: gzip, deflate, br\r\n{origin}\
             DNT: 1\r\nSec-GPC: 1\r\nConnection: keep-alive\r\n{tail}\r\n",
            srv.port,
            crate::policy::GENERIC_UA
        )
    };
    let names = |r: &Response| r.headers.iter().map(|(n, _)| n.to_ascii_lowercase()).collect::<Vec<_>>();

    // Same origin: none of the forbidden headers is sent; the allowed one is.
    let same_page = srv.url("site-a.test", "/app");
    let same = request(&same_page, FetchMode::SameOrigin, Credentials::Include, &[("X-Ok", "1")]);
    assert_eq!(srv.heads()[0], fixed("", "X-Ok: 1\r\n"));
    // The response keeps its ordinary headers but never Set-Cookie; the jar still got the cookie.
    assert_eq!(
        names(&same),
        ["content-length", "connection", "access-control-allow-origin", "content-type", "cache-control",
         "x-secret", "x-shown", "etag", "access-control-expose-headers"]
    );
    assert_eq!((same.mime.as_str(), same.body.as_slice()), ("text/plain", &b"body"[..]));
    assert_eq!(svc.stats().cookies, 1);

    // Cross-origin: the forbidden headers neither go out nor cause a preflight, the only
    // addition is the pipeline's own Origin, and just safelisted + exposed headers come back.
    let cross_page = format!("http://www.site-a.test:{}/app", srv.port);
    let cross = request(&cross_page, FetchMode::Cors, Credentials::Omit, &[]);
    let heads = srv.heads();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads[1], fixed(&format!("Origin: http://www.site-a.test:{}\r\n", srv.port), ""));
    assert_eq!(names(&cross), ["content-length", "content-type", "cache-control", "x-shown"]);
    assert_eq!((cross.mime.as_str(), cross.body.as_slice()), ("text/plain", &b"body"[..]));
}
