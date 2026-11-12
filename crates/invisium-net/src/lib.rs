//! Invisium network stack. Owned by the NET track — see `docs/agents/NET.md` for the contract.

mod cache;
mod connect;
mod cookies;
mod cors;
mod decode;
mod dns;
mod doh;
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

pub use psl::{registrable_domain, same_site};
pub use service::{NetService, RequestHandle};
pub use types::*;
pub use url::{form_urlencode, parse_data_url, percent_decode, Url, UrlError};

#[cfg(test)]
mod tests_cors;
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
