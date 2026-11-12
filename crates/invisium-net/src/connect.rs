//! Opens transport streams (direct, HTTP CONNECT, SOCKS5 or Tor) and layers TLS on top
//! (rustls, webpki roots, ALPN http/1.1, no resumption). Fails closed: a proxy or Tor failure is
//! never retried as a direct connection, and with any proxy the target is never resolved locally.

use crate::doh::Resolver;
use crate::http::Reader;
use crate::pool::{PoolKey, ProxyAuth};
use crate::privnet;
use crate::proxy::{http_connect, socks5_connect};
use crate::tor::Tor;
use crate::types::{NetConfig, NetError, ProxyConfig, Route};
use crate::url::Url;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub(crate) fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the ring provider supports the default TLS versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    // Session tickets / IDs let servers link visits together.
    cfg.resumption = rustls::client::Resumption::disabled();
    Arc::new(cfg)
}

/// Decrements the open-connection counter when the connection is dropped.
pub(crate) struct ConnGuard(Arc<AtomicUsize>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) struct Conn {
    pub reader: Reader<Box<dyn Io>>,
    pub route: Route,
    /// Pool generation of the request that dialled this connection (see `Pool`).
    pub generation: u64,
    /// Dialled directly to a host that resolved to a local address: a request guarded by
    /// private-network protection must never be given this connection from the pool.
    pub local_peer: bool,
    _guard: ConnGuard,
}

/// Test-only stand-in for name resolution: the address the "resolver" reports for a host and
/// the address the socket is really opened to (a local listener playing a public server).
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct TestRoute {
    pub resolved: std::net::IpAddr,
    pub dial: std::net::IpAddr,
}

/// Stable identifier of the route and its credentials, used in pool keys.
fn proxy_id(p: &ProxyConfig) -> (String, ProxyAuth) {
    match p {
        ProxyConfig::Direct => ("direct".into(), ProxyAuth::default()),
        ProxyConfig::Http { host, port, auth } => (format!("http:{host}:{port}"), ProxyAuth::new(auth.as_ref())),
        ProxyConfig::Socks5 { host, port, auth } => {
            (format!("socks5:{host}:{port}"), ProxyAuth::new(auth.as_ref()))
        }
        ProxyConfig::Tor => ("tor".into(), ProxyAuth::default()),
    }
}

pub(crate) fn pool_key(url: &Url, cfg: &NetConfig, isolation: &str) -> PoolKey {
    let (proxy, auth) = proxy_id(&cfg.proxy);
    PoolKey {
        scheme: url.scheme().to_string(),
        host: url.host().to_string(),
        port: url.port_or_default(),
        proxy,
        auth,
        isolation: if cfg.proxy == ProxyConfig::Tor { isolation.to_string() } else { String::new() },
    }
}

pub(crate) struct Dialer<'a> {
    pub cfg: &'a NetConfig,
    pub tls: &'a Arc<rustls::ClientConfig>,
    pub open: &'a Arc<AtomicUsize>,
    pub resolver: &'a Resolver,
    pub tor: &'a Tor,
    /// Top-level site, used for Tor stream isolation.
    pub isolation: &'a str,
    /// Pool generation the request's configuration snapshot belongs to. Read before dialling
    /// starts, so a route or identity change during the dial leaves the connection stale.
    pub generation: u64,
    /// The request comes from a public page and is not a navigation: refuse to connect when
    /// the name resolves to a local address (see `privnet`).
    pub guard_local: bool,
    #[cfg(test)]
    pub host_map: Option<TestRoute>,
}

async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, NetError> {
    let mut last = String::from("no addresses");
    for a in addrs {
        match TcpStream::connect(a).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = format!("{a}: {e}"),
        }
    }
    Err(NetError::Connect(last))
}

/// Connects to the proxy itself; any failure is a proxy error.
async fn connect_proxy(host: &str, port: u16) -> Result<TcpStream, NetError> {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((h, port))
        .await
        .map_err(|e| NetError::Proxy(format!("cannot resolve proxy {host}: {e}")))?
        .collect();
    connect_any(&addrs).await.map_err(|e| match e {
        NetError::Connect(m) => NetError::Proxy(format!("cannot reach proxy: {m}")),
        other => other,
    })
}

impl Dialer<'_> {
    fn timeout(&self) -> Duration {
        Duration::from_millis(self.cfg.connect_timeout_ms.max(1))
    }

    /// The addresses `host` resolves to on this machine.
    async fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, NetError> {
        #[cfg(test)]
        if let Some(r) = self.host_map {
            return Ok(vec![SocketAddr::new(r.resolved, port)]);
        }
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        self.resolver.resolve(self.tls, self.cfg.dns, bare, port, self.timeout()).await
    }

    /// Resolves and connects directly. The private-network test runs on the resolved addresses
    /// before any socket is opened, and the socket is opened to exactly those addresses.
    /// Returns the stream and whether the peer is a local address.
    async fn direct(&self, host: &str, port: u16) -> Result<(TcpStream, bool), NetError> {
        let addrs = self.lookup(host, port).await?;
        privnet::check_resolved(self.guard_local, &addrs)?;
        let local = addrs.iter().any(|a| privnet::ip_is_local(a.ip()));
        #[cfg(test)]
        let addrs = match self.host_map {
            Some(r) => vec![SocketAddr::new(r.dial, port)],
            None => addrs,
        };
        Ok((connect_any(&addrs).await?, local))
    }

    async fn transport(&self, host: &str, port: u16) -> Result<(Box<dyn Io>, Route, bool), NetError> {
        Ok(match &self.cfg.proxy {
            ProxyConfig::Direct => {
                let (s, local) = self.direct(host, port).await?;
                (Box::new(s), Route::Direct, local)
            }
            ProxyConfig::Http { host: ph, port: pp, auth } => {
                let mut s = connect_proxy(ph, *pp).await?;
                http_connect(&mut s, host, port, auth.as_ref()).await?;
                (Box::new(s), Route::Proxy, false)
            }
            ProxyConfig::Socks5 { host: ph, port: pp, auth } => {
                let mut s = connect_proxy(ph, *pp).await?;
                socks5_connect(&mut s, host, port, auth.as_ref()).await?;
                (Box::new(s), Route::Proxy, false)
            }
            ProxyConfig::Tor => {
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                let s = self.tor.connect(&self.cfg.data_dir, bare, port, self.isolation).await?;
                (Box::new(s), Route::Tor, false)
            }
        })
    }

    pub async fn connect(&self, url: &Url) -> Result<Conn, NetError> {
        let host = url.host().to_string();
        let port = url.port_or_default();
        let fut = async {
            let (stream, route, local_peer) = self.transport(&host, port).await?;
            let io: Box<dyn Io> = if url.is_secure() {
                let bare = host.trim_start_matches('[').trim_end_matches(']').to_string();
                let name = rustls::pki_types::ServerName::try_from(bare)
                    .map_err(|e| NetError::Tls(format!("invalid server name: {e}")))?;
                let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                    .connect(name, stream)
                    .await
                    .map_err(|e| NetError::Tls(e.to_string()))?;
                Box::new(tls)
            } else {
                stream
            };
            Ok::<_, NetError>((io, route, local_peer))
        };
        // Tor bootstrapping can take minutes; it gets its own generous bound.
        let limit = if self.cfg.proxy == ProxyConfig::Tor {
            self.timeout().max(Duration::from_secs(180))
        } else {
            self.timeout()
        };
        let (io, route, local_peer) =
            tokio::time::timeout(limit, fut).await.map_err(|_| NetError::Timeout)??;
        self.open.fetch_add(1, Ordering::Relaxed);
        Ok(Conn {
            reader: Reader::new(io, self.timeout()),
            route,
            generation: self.generation,
            local_peer,
            _guard: ConnGuard(self.open.clone()),
        })
    }
}
