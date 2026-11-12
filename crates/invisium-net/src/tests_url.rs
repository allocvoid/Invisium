use crate::lists::{PUBLIC_SUFFIXES, TRACKERS};
use crate::psl::is_tracker;
use crate::*;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn tables_are_sorted_and_unique() {
    for t in [PUBLIC_SUFFIXES, TRACKERS] {
        for w in t.windows(2) {
            assert!(w[0] < w[1], "{} !< {}", w[0], w[1]);
        }
    }
    assert!(TRACKERS.len() >= 200);
}

#[test]
fn parse_full_components() {
    let x = u("HTTP://User:Pa ss@EXAMPLE.com:8080/a/b?x=1 y#frag");
    assert_eq!(x.scheme(), "http");
    assert_eq!(x.host(), "example.com");
    assert_eq!(x.port(), Some(8080));
    assert_eq!(x.port_or_default(), 8080);
    assert_eq!(x.path(), "/a/b");
    assert_eq!(x.query(), Some("x=1%20y"));
    assert_eq!(x.fragment(), Some("frag"));
    assert_eq!(x.as_str(), "http://User:Pa%20ss@example.com:8080/a/b?x=1%20y#frag");
    assert_eq!(x.without_fragment(), "http://User:Pa%20ss@example.com:8080/a/b?x=1%20y");
    assert_eq!(x.origin(), "http://example.com:8080");
    assert_eq!(x.referrer_string(), "http://example.com:8080/a/b?x=1%20y");
    assert_eq!(x.to_string(), x.as_str());
}

#[test]
fn default_port_and_case_folding() {
    let x = u("https://WWW.Example.COM:443");
    assert_eq!(x.port(), None);
    assert_eq!(x.port_or_default(), 443);
    assert_eq!(x.as_str(), "https://www.example.com/");
    assert_eq!(u("http://h:80/").as_str(), "http://h/");
    assert_eq!(u("http://h/").port_or_default(), 80);
    let mut y = u("http://h:443/p");
    y.set_scheme("https");
    assert_eq!(y.as_str(), "https://h/p");
    assert!(y.is_secure() && y.is_http());
    assert_eq!(u("http://[::1]:8080/").host(), "[::1]");
    assert_eq!(u("http://bücher.example/").host(), "xn--bcher-kva.example");
}

#[test]
fn dot_segments() {
    assert_eq!(u("http://a/b/c/./../../g").path(), "/g");
    assert_eq!(u("http://a/b/c/..").path(), "/b/");
    assert_eq!(u("http://a/b/%2e/c/../../../..").path(), "/");
    assert_eq!(u("http://a/b//c/./").path(), "/b//c/");
}

#[test]
fn rfc3986_normal_examples() {
    let base = u("http://a/b/c/d;p?q");
    let cases = [
        ("g:h", "g:h"),
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        // RFC: "http://g"; special-scheme URLs always serialise a "/" path.
        ("//g", "http://g/"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
    ];
    for (r, want) in cases {
        assert_eq!(base.join(r).unwrap().as_str(), want, "reference {r:?}");
    }
}

#[test]
fn percent_encoding() {
    let x = u("http://h/a b/ü?q=ü x#f g");
    assert_eq!(x.as_str(), "http://h/a%20b/%C3%BC?q=%C3%BC%20x#f%20g");
    assert_eq!(percent_decode("%C3%BC%20x%zz%4"), "ü x%zz%4".as_bytes());
    let pairs = vec![("a b".to_string(), "ü&=".to_string()), ("k".into(), "v".into())];
    assert_eq!(form_urlencode(&pairs), "a+b=%C3%BC%26%3D&k=v");
}

#[test]
fn data_urls() {
    assert_eq!(
        parse_data_url("data:,Hello%2C%20World!"),
        Some(("text/plain;charset=us-ascii".to_string(), b"Hello, World!".to_vec()))
    );
    assert_eq!(
        parse_data_url("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ=="),
        Some(("text/plain".to_string(), b"Hello, World!".to_vec()))
    );
    assert_eq!(parse_data_url("data:;base64,!!!"), None);
    assert_eq!(parse_data_url("data:nocomma"), None);
    let d = u("data:text/html,<b>hi</b>");
    assert_eq!((d.scheme(), d.host()), ("data", ""));
    assert_eq!(d.origin(), "null");
}

#[test]
fn invalid_inputs() {
    for bad in ["", "no-scheme", "http://exa mple.com/", "http://h:99999/", "http://", "1http://x"] {
        assert!(Url::parse(bad).is_err(), "{bad:?} should fail");
    }
    assert!(u("data:x").join("other").is_err());
}

#[test]
fn registrable_domains() {
    assert_eq!(registrable_domain("a.b.example.co.uk"), "example.co.uk");
    assert_eq!(registrable_domain("www.example.com"), "example.com");
    assert_eq!(registrable_domain("example.com"), "example.com");
    assert_eq!(registrable_domain("co.uk"), "co.uk");
    assert_eq!(registrable_domain("localhost"), "localhost");
    assert_eq!(registrable_domain("127.0.0.1"), "127.0.0.1");
    assert_eq!(registrable_domain("foo.user.github.io"), "user.github.io");
    assert!(same_site(&u("https://a.example.com/"), &u("http://b.example.com/")));
    assert!(!same_site(&u("https://a.example.co.uk/"), &u("https://b.other.co.uk/")));
    assert!(!same_site(&u("http://127.0.0.1/"), &u("http://127.0.0.2/")));
    assert!(is_tracker("doubleclick.net") && is_tracker("ad.g.doubleclick.net"));
    assert!(!is_tracker("example.com") && !is_tracker("notdoubleclick.net"));
}
