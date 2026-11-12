//! Embedded Tor (arti) behind the `tor` feature. Without the feature every Tor request fails
//! closed with `NetError::Tor`.

use crate::types::{NetError, TorStatus};
use std::sync::{Arc, Mutex};

fn set(status: &Mutex<TorStatus>, s: TorStatus) {
    *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

#[cfg(feature = "tor")]
mod imp {
    use super::*;
    use arti_client::config::{BoolOrAuto, TorClientConfigBuilder};
    use arti_client::{DataStream, IsolationToken, StreamPrefs, TorClient};
    use futures_util::StreamExt;
    use std::collections::HashMap;
    use std::path::Path;
    use tor_rtcompat::PreferredRuntime;

    pub(crate) type Stream = DataStream;

    #[derive(Default)]
    pub(crate) struct Tor {
        client: tokio::sync::OnceCell<Arc<TorClient<PreferredRuntime>>>,
        status: Arc<Mutex<TorStatus>>,
        tokens: Mutex<HashMap<String, IsolationToken>>,
    }

    fn terr(e: impl std::fmt::Display) -> NetError {
        NetError::Tor(e.to_string())
    }

    impl Tor {
        pub fn status(&self) -> TorStatus {
            self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        /// Forget all isolation tokens so later streams use fresh circuits.
        pub fn new_identity(&self) {
            self.tokens.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }

        /// Lazily creates and bootstraps the client (state under `data_dir/tor`).
        pub async fn client(&self, data_dir: &Path) -> Result<Arc<TorClient<PreferredRuntime>>, NetError> {
            let status = self.status.clone();
            let c = self
                .client
                .get_or_try_init(|| async {
                    set(&status, TorStatus::Bootstrapping { percent: 0, message: "starting".into() });
                    let dir = data_dir.join("tor");
                    let cfg = TorClientConfigBuilder::from_directories(dir.join("state"), dir.join("cache"))
                        .build()
                        .map_err(terr)?;
                    let client = TorClient::builder().config(cfg).create_unbootstrapped().map_err(terr)?;
                    let mut events = client.bootstrap_events();
                    let st = status.clone();
                    tokio::spawn(async move {
                        while let Some(ev) = events.next().await {
                            let s = if ev.ready_for_traffic() {
                                TorStatus::Ready
                            } else {
                                TorStatus::Bootstrapping {
                                    percent: (ev.as_frac().clamp(0.0, 1.0) * 100.0) as u8,
                                    message: ev.to_string(),
                                }
                            };
                            set(&st, s);
                        }
                    });
                    client.bootstrap().await.map_err(terr)?;
                    set(&status, TorStatus::Ready);
                    Ok::<_, NetError>(client)
                })
                .await;
            match c {
                Ok(c) => Ok(c.clone()),
                Err(e) => {
                    set(&status, TorStatus::Failed(e.to_string()));
                    Err(e)
                }
            }
        }

        /// Opens a stream to `host:port` through Tor; the exit resolves the hostname.
        pub async fn connect(&self, data_dir: &Path, host: &str, port: u16, isolation: &str) -> Result<Stream, NetError> {
            let client = self.client(data_dir).await?;
            let token = *self
                .tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(isolation.to_string())
                .or_insert_with(IsolationToken::new);
            let mut prefs = StreamPrefs::new();
            prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));
            prefs.set_isolation(token);
            client.connect_with_prefs((host, port), &prefs).await.map_err(terr)
        }
    }
}

#[cfg(not(feature = "tor"))]
mod imp {
    use super::*;
    use std::path::Path;

    pub(crate) type Stream = tokio::net::TcpStream;

    #[derive(Default)]
    pub(crate) struct Tor {
        status: Arc<Mutex<TorStatus>>,
    }

    impl Tor {
        pub fn status(&self) -> TorStatus {
            self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        pub fn new_identity(&self) {}

        pub async fn connect(&self, _: &Path, _: &str, _: u16, _: &str) -> Result<Stream, NetError> {
            let msg = "Tor support is not compiled in (build with the `tor` feature)";
            set(&self.status, TorStatus::Failed(msg.into()));
            Err(NetError::Tor(msg.into()))
        }
    }
}

pub(crate) use imp::Tor;

impl Default for TorStatus {
    fn default() -> Self {
        TorStatus::Off
    }
}
