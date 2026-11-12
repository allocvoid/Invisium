//! Name resolution: DNS-over-HTTPS (RFC 8484, POST to the provider's IP with its SNI) with an
//! in-memory TTL cache, or the system resolver. Only used for direct connections.

use crate::dns::{decode_response, encode_query, is_nxdomain, TYPE_A, TYPE_AAAA};
use crate::http::{read_response, Reader};
use crate::types::{DnsMode, DohProvider, NetError};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

const MAX_CACHE_ENTRIES: usize = 1024;
const MIN_TTL: u64 = 30;
const MAX_TTL: u64 = 3600;
const MAX_DNS_MESSAGE: usize = 64 * 1024;

pub(crate) struct ProviderInfo {
    pub ip: IpAddr,
    pub sni: &'static str,
    pub path: &'static str,
}

pub(crate) fn provider_info(p: DohProvider) -> ProviderInfo {
    let (ip, sni) = match p {
        DohProvider::Quad9 => ([9, 9, 9, 9], "dns.quad9.net"),
        DohProvider::Cloudflare => ([1, 1, 1, 1], "cloudflare-dns.com"),
        DohProvider::Mullvad => ([194, 242, 2, 2], "dns.mullvad.net"),
    };
    ProviderInfo { ip: IpAddr::from(ip), sni, path: "/dns-query" }
}

/// One DoH exchange for one record type. Returns the raw DNS message of a 200 response; every
/// failure (connect, TLS, timeout, HTTP status) is a `NetError::Dns`.
pub(crate) async fn doh_query(
    tls: &Arc<rustls::ClientConfig>,
    provider: DohProvider,
    name: &str,
    qtype: u16,
    timeout: Duration,
) -> Result<Vec<u8>, NetError> {
    let info = provider_info(provider);
    let query = encode_query(name, qtype)?;
    let fut = async {
        let tcp = TcpStream::connect(SocketAddr::new(info.ip, 443))
            .await
            .map_err(|e| NetError::Dns(format!("DoH connect failed: {e}")))?;
        let _ = tcp.set_nodelay(true);
        let server = rustls::pki_types::ServerName::try_from(info.sni)
            .map_err(|e| NetError::Dns(format!("DoH server name: {e}")))?;
        let mut tls = tokio_rustls::TlsConnector::from(tls.clone())
            .connect(server, tcp)
            .await
            .map_err(|e| NetError::Dns(format!("DoH TLS failed: {e}")))?;
        let mut req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nAccept: application/dns-message\r\n\
             Content-Type: application/dns-message\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            info.path,
            info.sni,
            query.len()
        )
        .into_bytes();
        req.extend_from_slice(&query);
        tls.write_all(&req).await.map_err(|e| NetError::Dns(format!("DoH write failed: {e}")))?;
        tls.flush().await.map_err(|e| NetError::Dns(format!("DoH write failed: {e}")))?;
        let mut r = Reader::new(tls, timeout);
        let resp = read_response(&mut r, false, MAX_DNS_MESSAGE).await?;
        if resp.status != 200 {
            return Err(NetError::Dns(format!("DoH server returned status {}", resp.status)));
        }
        Ok(resp.body)
    };
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err(NetError::Dns("DoH query timed out".into())),
        Ok(Err(NetError::Dns(m))) => Err(NetError::Dns(m)),
        Ok(Err(e)) => Err(NetError::Dns(format!("DoH: {e}"))),
        Ok(Ok(v)) => Ok(v),
    }
}

/// Outcome of one DoH query for one record type.
enum Reply {
    /// NOERROR: the records of the requested type (possibly none) and their TTL in seconds.
    Records(Vec<IpAddr>, u32),
    /// The resolver says the name does not exist at all.
    NxDomain,
    /// No usable answer: the exchange failed, the message was malformed, or the server reported
    /// SERVFAIL / REFUSED / another error. Says nothing about whether the name exists.
    Failed(String),
}

fn interpret(raw: Result<Vec<u8>, NetError>, qtype: u16) -> Reply {
    let failed = |e: NetError| match e {
        NetError::Dns(m) => Reply::Failed(m),
        other => Reply::Failed(other.to_string()),
    };
    match raw {
        Err(e) => failed(e),
        Ok(msg) if is_nxdomain(&msg) => Reply::NxDomain,
        Ok(msg) => match decode_response(&msg, qtype) {
            Ok(ans) => Reply::Records(ans.addrs, ans.min_ttl),
            Err(e) => failed(e),
        },
    }
}

/// Why a DoH lookup produced no address.
#[derive(Debug, PartialEq, Eq)]
enum DohError {
    /// A definitive answer from the resolver: NXDOMAIN, or NOERROR with neither A nor AAAA
    /// records. Final in every mode: asking another resolver would only leak the name.
    Negative(String),
    /// DoH itself did not work, so nothing is known about the name. Non-strict mode may fall
    /// back to the system resolver.
    Unavailable(String),
}

/// Merges the A and AAAA outcomes into addresses plus TTL, or the reason there are none.
fn combine(host: &str, a: Reply, aaaa: Reply) -> Result<(Vec<IpAddr>, u32), DohError> {
    let mut ips = Vec::new();
    let mut ttl = u32::MAX;
    let mut nxdomain = false;
    let mut failure = None;
    for r in [a, aaaa] {
        match r {
            Reply::Records(v, t) => {
                if !v.is_empty() {
                    ttl = ttl.min(t);
                }
                ips.extend(v);
            }
            Reply::NxDomain => nxdomain = true,
            Reply::Failed(m) => failure = Some(m),
        }
    }
    if !ips.is_empty() {
        return Ok((ips, ttl));
    }
    if nxdomain {
        return Err(DohError::Negative(format!("{host}: no such domain (NXDOMAIN)")));
    }
    // An empty NOERROR answer is only definitive when both record types were answered.
    Err(match failure {
        Some(m) => DohError::Unavailable(m),
        None => DohError::Negative(format!("{host}: no addresses")),
    })
}

async fn system_lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, NetError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| NetError::Dns(format!("{host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(NetError::Dns(format!("{host}: no addresses")));
    }
    Ok(addrs)
}

#[derive(Default)]
pub(crate) struct Resolver {
    cache: Mutex<HashMap<String, (Vec<IpAddr>, Instant)>>,
}

impl Resolver {
    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// DoH lookup of `host`; `query` performs one exchange for a record type and returns the
    /// raw DNS message.
    async fn doh<Q, F>(&self, host: &str, query: Q) -> Result<Vec<IpAddr>, DohError>
    where
        Q: Fn(u16) -> F,
        F: Future<Output = Result<Vec<u8>, NetError>>,
    {
        if let Some((ips, exp)) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(host) {
            if *exp > Instant::now() {
                return Ok(ips.clone());
            }
        }
        // A first, then AAAA; either may legitimately be empty.
        let a = interpret(query(TYPE_A).await, TYPE_A);
        let aaaa = interpret(query(TYPE_AAAA).await, TYPE_AAAA);
        let (ips, ttl) = combine(host, a, aaaa)?;
        let ttl = (ttl as u64).clamp(MIN_TTL, MAX_TTL);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_CACHE_ENTRIES {
            let now = Instant::now();
            cache.retain(|_, (_, exp)| *exp > now);
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(host.to_string(), (ips.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(ips)
    }

    /// Resolves `host` (already bracket-free). IP literals are returned as-is.
    pub async fn resolve(
        &self,
        tls: &Arc<rustls::ClientConfig>,
        mode: DnsMode,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<Vec<SocketAddr>, NetError> {
        self.resolve_with(
            mode,
            host,
            port,
            |provider, qtype| doh_query(tls, provider, host, qtype, timeout),
            || system_lookup(host, port),
        )
        .await
    }

    /// `resolve` with both transports injected: `query` performs one DoH exchange and returns
    /// the raw DNS message, `system` asks the system resolver.
    async fn resolve_with<Q, QF, S, SF>(
        &self,
        mode: DnsMode,
        host: &str,
        port: u16,
        query: Q,
        system: S,
    ) -> Result<Vec<SocketAddr>, NetError>
    where
        Q: Fn(DohProvider, u16) -> QF,
        QF: Future<Output = Result<Vec<u8>, NetError>>,
        S: FnOnce() -> SF,
        SF: Future<Output = Result<Vec<SocketAddr>, NetError>>,
    {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        if host == "localhost" || host.ends_with(".localhost") {
            return Ok(vec![SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port)]);
        }
        if let DnsMode::Doh { provider, strict } = mode {
            match self.doh(host, |qtype| query(provider, qtype)).await {
                Ok(ips) => return Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect()),
                // The resolver answered: the name has no addresses. Never ask anyone else.
                Err(DohError::Negative(m)) => return Err(NetError::Dns(m)),
                Err(DohError::Unavailable(m)) if strict => return Err(NetError::Dns(m)),
                Err(DohError::Unavailable(_)) => {} // non-strict: fall back to the system resolver
            }
        }
        system().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::ready;

    type Canned = Result<Vec<u8>, NetError>;

    const HOST: &str = "host.example";
    const DOH_V4: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
    const DOH_V6: IpAddr = IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

    fn system_addr() -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 99], 443))
    }

    /// A DNS response about HOST with the given RCODE and address records (TTL 60).
    fn reply(rcode: u8, addrs: &[IpAddr]) -> Canned {
        let mut m = vec![0, 0, 0x81, 0x80 | rcode, 0, 1, 0, addrs.len() as u8, 0, 0, 0, 0];
        m.extend_from_slice(b"\x04host\x07example\x00\x00\x01\x00\x01");
        for a in addrs {
            m.extend_from_slice(&[0xC0, 12]);
            match a {
                IpAddr::V4(ip) => {
                    m.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
                    m.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    m.extend_from_slice(&[0, 28, 0, 1, 0, 0, 0, 60, 0, 16]);
                    m.extend_from_slice(&ip.octets());
                }
            }
        }
        Ok(m)
    }

    fn transport_error() -> Canned {
        Err(NetError::Dns("DoH connect failed: connection refused".into()))
    }

    struct Outcome {
        result: Result<Vec<SocketAddr>, NetError>,
        doh_queries: usize,
        asked_system: bool,
    }

    /// Resolves HOST through `resolver` with canned DoH replies instead of the network.
    fn resolve_on(resolver: &Resolver, strict: bool, a: &Canned, aaaa: &Canned) -> Outcome {
        let (queries, asked_system) = (Cell::new(0), Cell::new(false));
        let mode = DnsMode::Doh { provider: DohProvider::Cloudflare, strict };
        let fut = resolver.resolve_with(
            mode,
            HOST,
            443,
            |_, qtype| {
                queries.set(queries.get() + 1);
                ready(if qtype == TYPE_A { a.clone() } else { aaaa.clone() })
            },
            || {
                asked_system.set(true);
                ready(Ok(vec![system_addr()]))
            },
        );
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let result = rt.block_on(fut);
        Outcome { result, doh_queries: queries.get(), asked_system: asked_system.get() }
    }

    fn resolve(strict: bool, a: &Canned, aaaa: &Canned) -> Outcome {
        resolve_on(&Resolver::default(), strict, a, aaaa)
    }

    #[test]
    fn definitive_negative_answers_are_final_and_never_reach_the_system_resolver() {
        let (nxdomain, empty) = (reply(3, &[]), reply(0, &[]));
        let cases = [
            ("NXDOMAIN for both types", &nxdomain, &nxdomain),
            ("NOERROR without A and AAAA", &empty, &empty),
            ("NXDOMAIN, then no records", &nxdomain, &empty),
        ];
        for strict in [false, true] {
            for (what, a, aaaa) in cases {
                let o = resolve(strict, a, aaaa);
                assert!(matches!(o.result, Err(NetError::Dns(_))), "{what} strict={strict}: {:?}", o.result);
                assert!(!o.asked_system, "{what} strict={strict}: leaked to the system resolver");
                assert_eq!(o.doh_queries, 2);
            }
            // NXDOMAIN covers the whole name, even when the other exchange failed.
            let o = resolve(strict, &nxdomain, &transport_error());
            assert!(matches!(o.result, Err(NetError::Dns(_))) && !o.asked_system, "strict={strict}");
            let o = resolve(strict, &transport_error(), &nxdomain);
            assert!(matches!(o.result, Err(NetError::Dns(_))) && !o.asked_system, "strict={strict}");
        }
    }

    #[test]
    fn doh_failures_fall_back_to_the_system_resolver_only_when_not_strict() {
        let cut = |m: Canned, n: usize| m.map(|mut b| {
            b.truncate(n);
            b
        });
        let failures = [
            ("connect / TLS / timeout / HTTP status", transport_error()),
            ("SERVFAIL", reply(2, &[])),
            ("REFUSED", reply(5, &[])),
            ("other RCODE", reply(4, &[])),
            ("short header", cut(reply(0, &[DOH_V4]), 7)),
            ("truncated record", cut(reply(0, &[DOH_V4]), 40)),
            ("not a response", Ok(crate::dns::encode_query(HOST, TYPE_A).unwrap())),
        ];
        for (what, f) in &failures {
            let o = resolve(false, f, f);
            assert_eq!(o.result, Ok(vec![system_addr()]), "{what}");
            assert!(o.asked_system, "{what}");
            let o = resolve(true, f, f);
            assert!(matches!(o.result, Err(NetError::Dns(_))), "{what}: {:?}", o.result);
            assert!(!o.asked_system, "{what}: strict mode must never fall back");
        }
    }

    #[test]
    fn an_empty_answer_is_only_definitive_when_both_queries_were_answered() {
        let (empty, failed) = (reply(0, &[]), transport_error());
        for (a, aaaa) in [(&empty, &failed), (&failed, &empty)] {
            let o = resolve(false, a, aaaa);
            assert_eq!(o.result, Ok(vec![system_addr()]));
            assert!(o.asked_system);
            let o = resolve(true, a, aaaa);
            assert!(matches!(o.result, Err(NetError::Dns(_))) && !o.asked_system);
        }
    }

    #[test]
    fn doh_answers_are_used_without_fallback_and_cached() {
        let (v4, v6) = (reply(0, &[DOH_V4]), reply(0, &[DOH_V6]));
        let both = vec![SocketAddr::new(DOH_V4, 443), SocketAddr::new(DOH_V6, 443)];
        let resolver = Resolver::default();
        let o = resolve_on(&resolver, false, &v4, &v6);
        assert_eq!((o.result, o.doh_queries, o.asked_system), (Ok(both.clone()), 2, false));
        // Second lookup: served from the TTL cache, no query of any kind.
        let o = resolve_on(&resolver, false, &transport_error(), &transport_error());
        assert_eq!((o.result, o.doh_queries, o.asked_system), (Ok(both), 0, false));
        // One address family is enough, whatever happened to the other query.
        for other in [reply(0, &[]), transport_error()] {
            let o = resolve(false, &v4, &other);
            assert_eq!(o.result, Ok(vec![SocketAddr::new(DOH_V4, 443)]));
            assert!(!o.asked_system);
        }
    }
}
