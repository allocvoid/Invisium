//! Invisium network stack. Owned by the NET track — see `docs/agents/NET.md` for the contract.

mod cache;
mod connect;
mod cookies;
mod cors;
mod decode;
mod dns;
mod doh;
mod download;
mod filename;
mod framing;
mod http;
mod lists;
mod policy;
mod pool;
mod privnet;
mod proxy;
mod psl;
mod service;
mod tor;
mod types;
mod url;
mod ws;
mod wsproto;
mod wssession;

pub use psl::{registrable_domain, same_site};
pub use service::{NetService, RequestHandle};
pub use types::*;
pub use url::{form_urlencode, parse_data_url, percent_decode, Url, UrlError};
pub use ws::WsHandle;

#[cfg(test)]
mod tests_cors;
#[cfg(test)]
mod tests_download;
#[cfg(test)]
mod tests_frames;
#[cfg(test)]
mod tests_http;
#[cfg(test)]
mod tests_m2;
#[cfg(test)]
mod tests_policy;
#[cfg(test)]
mod tests_privnet;
#[cfg(test)]
mod tests_service;
#[cfg(test)]
mod tests_url;
#[cfg(test)]
mod tests_ws;
#[cfg(test)]
mod tests_ws_policy;
