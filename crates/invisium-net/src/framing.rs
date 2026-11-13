//! Framing rules for responses to `ResourceKind::Frame` requests: `X-Frame-Options` and the
//! `frame-ancestors` directive of `Content-Security-Policy`, checked against the tab's
//! top-level document. Pure decisions; `service.rs` applies them to the final response.

use crate::types::Headers;
use crate::url::Url;

/// Whether `response` (the frame's final URL, with `headers`) may be shown inside a tab whose
/// document is `top`. `None` (no known ancestor) only passes when nothing restricts framing.
pub(crate) fn allowed(headers: &Headers, response: &Url, top: Option<&Url>) -> bool {
    let policies = frame_ancestor_policies(headers);
    if !policies.is_empty() {
        // Every policy that has the directive must allow; it overrides X-Frame-Options.
        return policies.iter().all(|sources| ancestors_allowed(sources, response, top));
    }
    xfo_allows(headers, response, top)
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.is_http()
        && b.is_http()
        && a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_default() == b.port_or_default()
}

/// `X-Frame-Options`: unknown or malformed values are ignored; `DENY` wins over `SAMEORIGIN`.
fn xfo_allows(headers: &Headers, response: &Url, top: Option<&Url>) -> bool {
    let mut deny = false;
    let mut sameorigin = false;
    for v in headers.get_all("x-frame-options").flat_map(|v| v.split(',')) {
        match v.trim().to_ascii_lowercase().as_str() {
            "deny" => deny = true,
            "sameorigin" => sameorigin = true,
            _ => {}
        }
    }
    if deny {
        return false;
    }
    if sameorigin {
        return top.is_some_and(|t| same_origin(t, response));
    }
    true
}

/// The source lists of every `frame-ancestors` directive, one per policy that has one. A
/// header may carry several policies separated by commas.
fn frame_ancestor_policies(headers: &Headers) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for policy in headers.get_all("content-security-policy").flat_map(|v| v.split(',')) {
        for directive in policy.split(';') {
            let mut tokens = directive.split_ascii_whitespace();
            let Some(name) = tokens.next() else { continue };
            if name.eq_ignore_ascii_case("frame-ancestors") {
                out.push(tokens.map(str::to_string).collect());
                break; // only the first occurrence in a policy counts
            }
        }
    }
    out
}

fn ancestors_allowed(sources: &[String], response: &Url, top: Option<&Url>) -> bool {
    let Some(top) = top else { return false };
    if sources.iter().any(|s| s.eq_ignore_ascii_case("'none'")) {
        return false;
    }
    sources.iter().any(|s| source_matches(s, response, top))
}

fn scheme_matches(source_scheme: &str, url_scheme: &str) -> bool {
    source_scheme == url_scheme || (source_scheme == "http" && url_scheme == "https")
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

/// One source expression against the ancestor `top`. `response` is the protected resource.
fn source_matches(source: &str, response: &Url, top: &Url) -> bool {
    let s = source.to_ascii_lowercase();
    if s == "'self'" {
        return same_origin(top, response);
    }
    if s.starts_with('\'') {
        return false; // other keywords, nonces and hashes never match an ancestor
    }
    if s == "*" {
        return top.is_http();
    }
    // Scheme-only source: "https:".
    if let Some(scheme) = s.strip_suffix(':') {
        if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) {
            return scheme_matches(scheme, top.scheme());
        }
        return false;
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, rest)) => (Some(sc), rest),
        None => (None, s.as_str()),
    };
    let hostport = rest.split('/').next().unwrap_or("");
    // IPv6 literals are not valid host sources; one that is written anyway never matches.
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };
    if host.is_empty() {
        return false;
    }
    match scheme {
        Some(sc) if !scheme_matches(sc, top.scheme()) => return false,
        None if !scheme_matches(response.scheme(), top.scheme()) => return false,
        _ => {}
    }
    let top_host = top.host();
    let host_ok = if let Some(base) = host.strip_prefix("*.") {
        top_host.len() > base.len() + 1 && top_host.ends_with(base) && top_host[..top_host.len() - base.len()].ends_with('.')
    } else if host == "*" {
        true
    } else {
        host == top_host
    };
    if !host_ok {
        return false;
    }
    match port {
        Some("*") => true,
        Some(p) => p.parse::<u16>().ok() == Some(top.port_or_default()),
        None => {
            let source_scheme = scheme.unwrap_or(response.scheme());
            let top_port = top.port_or_default();
            default_port(source_scheme) == Some(top_port) || default_port(top.scheme()) == Some(top_port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(list: &[(&str, &str)]) -> Headers {
        let mut out = Headers::new();
        for (n, v) in list {
            out.push(*n, *v);
        }
        out
    }

    fn check(list: &[(&str, &str)], frame: &str, top: &str) -> bool {
        allowed(&h(list), &Url::parse(frame).unwrap(), Some(&Url::parse(top).unwrap()))
    }

    #[test]
    fn source_expressions() {
        let f = "https://frame.example/x";
        let csp = |v: &str| vec![("Content-Security-Policy", v.to_string())];
        let run = |v: &str, top: &str| {
            let list = csp(v);
            let refs: Vec<(&str, &str)> = list.iter().map(|(n, v)| (*n, v.as_str())).collect();
            check(&refs, f, top)
        };
        assert!(run("frame-ancestors https://top.example", "https://top.example/page"));
        assert!(run("frame-ancestors top.example", "https://top.example/"));
        assert!(!run("frame-ancestors top.example", "https://other.example/"));
        assert!(run("frame-ancestors *.example.org", "https://a.example.org/"));
        assert!(!run("frame-ancestors *.example.org", "https://example.org/"));
        assert!(!run("frame-ancestors *.example.org", "https://badexample.org/"));
        assert!(run("frame-ancestors https:", "https://anything.example/"));
        assert!(!run("frame-ancestors https:", "http://anything.example/"));
        assert!(run("frame-ancestors https://top.example:8443", "https://top.example:8443/"));
        assert!(!run("frame-ancestors https://top.example:8443", "https://top.example/"));
        assert!(!run("frame-ancestors https://top.example", "https://top.example:8443/"));
        assert!(run("frame-ancestors https://top.example:*", "https://top.example:8443/"));
        assert!(run("frame-ancestors http://top.example", "https://top.example/"));
        assert!(!run("frame-ancestors https://top.example", "http://top.example/"));
        assert!(run("frame-ancestors *", "https://x.example/"));
        assert!(!run("frame-ancestors 'unsafe-inline'", "https://x.example/"));
        assert!(!run("frame-ancestors", "https://x.example/"));
        assert!(run("default-src 'none'", "https://x.example/")); // no frame-ancestors: no rule
    }

    #[test]
    fn no_ancestor_only_passes_without_restrictions() {
        let u = Url::parse("https://f.example/").unwrap();
        assert!(allowed(&Headers::new(), &u, None));
        assert!(!allowed(&h(&[("X-Frame-Options", "SAMEORIGIN")]), &u, None));
        assert!(!allowed(&h(&[("Content-Security-Policy", "frame-ancestors *")]), &u, None));
    }
}
