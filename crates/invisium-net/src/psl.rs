//! Registrable-domain (eTLD+1) computation and tracker-host matching over the embedded tables.

use crate::lists::{PUBLIC_SUFFIXES, TRACKERS};
use crate::url::Url;

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.contains(':') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

fn label_starts(h: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(h.bytes().enumerate().filter(|&(_, b)| b == b'.').map(|(i, _)| i + 1));
    starts
}

fn listed(table: &[&str], s: &str) -> bool {
    table.binary_search(&s).is_ok()
}

/// eTLD+1 of `host`. IP addresses, single-label hosts and hosts that are themselves a public
/// suffix are returned unchanged.
pub fn registrable_domain(host: &str) -> &str {
    let h = host.strip_suffix('.').unwrap_or(host);
    if h.is_empty() || is_ip(h) {
        return h;
    }
    let starts = label_starts(h);
    let si = (0..starts.len())
        .find(|&i| listed(PUBLIC_SUFFIXES, &h[starts[i]..]))
        .unwrap_or(starts.len() - 1);
    if si == 0 {
        return h;
    }
    &h[starts[si - 1]..]
}

/// True when `domain` is a public suffix (listed, or a bare single-label TLD).
pub(crate) fn is_public_suffix(domain: &str) -> bool {
    let d = domain.strip_suffix('.').unwrap_or(domain);
    !d.contains('.') || listed(PUBLIC_SUFFIXES, d)
}

/// Same registrable domain; IP hosts must match exactly. Host-less URLs are never same-site.
pub fn same_site(a: &Url, b: &Url) -> bool {
    if a.host().is_empty() || b.host().is_empty() {
        return false;
    }
    if is_ip(a.host()) || is_ip(b.host()) {
        return a.host() == b.host();
    }
    registrable_domain(a.host()) == registrable_domain(b.host())
}

/// True when `host` or any parent domain of it is on the embedded tracker list.
pub(crate) fn is_tracker(host: &str) -> bool {
    let h = host.strip_suffix('.').unwrap_or(host);
    if h.is_empty() || is_ip(h) {
        return false;
    }
    label_starts(h).into_iter().any(|s| listed(TRACKERS, &h[s..]))
}
