//! User settings: the privacy switches, how they map onto the network stack, and persistence.
//!
//! Only this file is ever written to disk by the browser itself (plus Tor's own state when Tor
//! is enabled). There is no history file, no cookie store and no disk cache.

use invis_chrome::pages::{SettingsRow, SettingsSection};
use invis_chrome::Toggle;
use invisium_net::{CookiePolicy, DnsMode, DohProvider, NetConfig, ProxyConfig, ReferrerPolicy, ThirdPartyPolicy, UaProfile};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    // ── tracking protection ──
    pub block_trackers: bool,
    pub https_only: bool,
    pub strip_tracking_params: bool,
    pub send_gpc: bool,
    /// allow | cookies | scripts | all
    pub third_party: String,
    /// none | first-party | partitioned
    pub cookies: String,
    /// never | origin | full
    pub referrer: String,
    // ── fingerprinting ──
    pub letterbox: bool,
    pub light_scheme_only: bool,
    pub generic_language: bool,
    // ── network route ──
    /// direct | http | socks5 | tor
    pub proxy: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_user: String,
    pub proxy_pass: String,
    pub tor_isolate: bool,
    /// system | quad9 | cloudflare | mullvad
    pub dns: String,
    pub dns_strict: bool,
    // ── resources ──
    pub load_images: bool,
    /// Run page scripts. Off by default while the JavaScript engine is still being built out.
    pub javascript: bool,
    pub low_power_gpu: bool,
    pub hibernate_tabs: bool,
    pub cache_mb: u32,
    pub search: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            block_trackers: true,
            https_only: true,
            strip_tracking_params: true,
            send_gpc: true,
            third_party: "cookies".into(),
            cookies: "first-party".into(),
            referrer: "origin".into(),
            letterbox: true,
            light_scheme_only: true,
            generic_language: true,
            proxy: "direct".into(),
            proxy_host: String::new(),
            proxy_port: 1080,
            proxy_user: String::new(),
            proxy_pass: String::new(),
            tor_isolate: true,
            // Quad9's DoH endpoint only speaks HTTP/2, which the network stack does not yet;
            // Cloudflare answers over HTTP/1.1.
            dns: "cloudflare".into(),
            dns_strict: false,
            load_images: true,
            javascript: false,
            low_power_gpu: true,
            hibernate_tabs: true,
            cache_mb: 32,
            search: "https://html.duckduckgo.com/html/?q=".into(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("InvisBrowser")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("InvisBrowser")
}

macro_rules! fields {
    ($s:ident, $f:ident) => {
        $f!("block_trackers", $s.block_trackers);
        $f!("https_only", $s.https_only);
        $f!("strip_tracking_params", $s.strip_tracking_params);
        $f!("send_gpc", $s.send_gpc);
        $f!("third_party", $s.third_party);
        $f!("cookies", $s.cookies);
        $f!("referrer", $s.referrer);
        $f!("letterbox", $s.letterbox);
        $f!("light_scheme_only", $s.light_scheme_only);
        $f!("generic_language", $s.generic_language);
        $f!("proxy", $s.proxy);
        $f!("proxy_host", $s.proxy_host);
        $f!("proxy_port", $s.proxy_port);
        $f!("proxy_user", $s.proxy_user);
        $f!("proxy_pass", $s.proxy_pass);
        $f!("tor_isolate", $s.tor_isolate);
        $f!("dns", $s.dns);
        $f!("dns_strict", $s.dns_strict);
        $f!("load_images", $s.load_images);
        $f!("javascript", $s.javascript);
        $f!("low_power_gpu", $s.low_power_gpu);
        $f!("hibernate_tabs", $s.hibernate_tabs);
        $f!("cache_mb", $s.cache_mb);
        $f!("search", $s.search);
    };
}

trait Field {
    fn put(&self) -> String;
    fn take(&mut self, v: &str);
}
impl Field for bool {
    fn put(&self) -> String {
        if *self { "on".into() } else { "off".into() }
    }
    fn take(&mut self, v: &str) {
        *self = matches!(v, "on" | "true" | "1" | "yes");
    }
}
impl Field for String {
    fn put(&self) -> String {
        self.clone()
    }
    fn take(&mut self, v: &str) {
        *self = v.to_string();
    }
}
impl Field for u16 {
    fn put(&self) -> String {
        self.to_string()
    }
    fn take(&mut self, v: &str) {
        if let Ok(n) = v.trim().parse() {
            *self = n;
        }
    }
}
impl Field for u32 {
    fn put(&self) -> String {
        self.to_string()
    }
    fn take(&mut self, v: &str) {
        if let Ok(n) = v.trim().parse() {
            *self = n;
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        if let Ok(text) = std::fs::read_to_string(config_dir().join("settings.ini")) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    s.set(k.trim(), v.trim());
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let dir = config_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let mut out = String::from("# InvisBrowser settings\n");
        let s = self;
        macro_rules! w {
            ($name:expr, $field:expr) => {
                out.push_str(&format!("{}={}\n", $name, Field::put(&$field)));
            };
        }
        fields!(s, w);
        let _ = std::fs::write(dir.join("settings.ini"), out);
    }

    /// Set a field by name. Unknown names and invalid values are ignored.
    pub fn set(&mut self, key: &str, value: &str) {
        let s = self;
        macro_rules! r {
            ($name:expr, $field:expr) => {
                if key == $name {
                    Field::take(&mut $field, value);
                    return;
                }
            };
        }
        fields!(s, r);
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let s = self;
        macro_rules! g {
            ($name:expr, $field:expr) => {
                if key == $name {
                    return Some(Field::put(&$field));
                }
            };
        }
        fields!(s, g);
        None
    }

    /// Flip a boolean setting.
    pub fn toggle(&mut self, key: &str) {
        match self.get(key).as_deref() {
            Some("on") => self.set(key, "off"),
            Some("off") => self.set(key, "on"),
            _ => {}
        }
        // "tor" in the quick panel is a shortcut for the proxy choice
        if key == "tor" {
            self.proxy = if self.proxy == "tor" { "direct".into() } else { "tor".into() };
        }
    }

    pub fn uses_tor(&self) -> bool {
        self.proxy == "tor"
    }

    pub fn net_config(&self) -> NetConfig {
        let auth = if self.proxy_user.is_empty() { None } else { Some((self.proxy_user.clone(), self.proxy_pass.clone())) };
        let proxy = match self.proxy.as_str() {
            "http" if !self.proxy_host.is_empty() => ProxyConfig::Http { host: self.proxy_host.clone(), port: self.proxy_port, auth },
            "socks5" if !self.proxy_host.is_empty() => ProxyConfig::Socks5 { host: self.proxy_host.clone(), port: self.proxy_port, auth },
            "tor" => ProxyConfig::Tor,
            _ => ProxyConfig::Direct,
        };
        let provider = match self.dns.as_str() {
            "quad9" => Some(DohProvider::Quad9),
            "cloudflare" => Some(DohProvider::Cloudflare),
            "mullvad" => Some(DohProvider::Mullvad),
            _ => None,
        };
        NetConfig {
            proxy,
            dns: match provider {
                Some(provider) => DnsMode::Doh { provider, strict: self.dns_strict },
                None => DnsMode::System,
            },
            https_only: self.https_only,
            block_trackers: self.block_trackers,
            third_party: match self.third_party.as_str() {
                "allow" => ThirdPartyPolicy::Allow,
                "scripts" => ThirdPartyPolicy::BlockScriptsAndFrames,
                "all" => ThirdPartyPolicy::BlockAll,
                _ => ThirdPartyPolicy::BlockCookies,
            },
            cookies: match self.cookies.as_str() {
                "none" => CookiePolicy::BlockAll,
                "partitioned" => CookiePolicy::AllowPartitioned,
                _ => CookiePolicy::FirstPartyOnly,
            },
            referrer: match self.referrer.as_str() {
                "never" => ReferrerPolicy::Never,
                "full" => ReferrerPolicy::Full,
                _ => ReferrerPolicy::OriginOnCrossSite,
            },
            strip_tracking_params: self.strip_tracking_params,
            user_agent: UaProfile::Generic,
            generic_accept_language: self.generic_language,
            send_gpc: self.send_gpc,
            tor_isolate_by_site: self.tor_isolate,
            cache_bytes: self.cache_mb as usize * 1024 * 1024,
            max_body_bytes: 64 * 1024 * 1024,
            connect_timeout_ms: if self.uses_tor() { 60_000 } else { 15_000 },
            data_dir: data_dir(),
        }
    }

    /// The switches shown in the toolbar's privacy panel.
    pub fn toggles(&self) -> Vec<Toggle> {
        let t = |id: &str, label: &str, detail: &str, on: bool, section: &str| Toggle {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
            on,
            section: section.into(),
            enabled: true,
        };
        vec![
            t("block_trackers", "Block trackers", "Refuse known tracking and ad hosts", self.block_trackers, "Protection"),
            t("https_only", "HTTPS only", "Never load pages over plain HTTP", self.https_only, "Protection"),
            t("strip_tracking_params", "Clean links", "Remove utm_, fbclid and similar parameters", self.strip_tracking_params, "Protection"),
            t("letterbox", "Letterboxing", "Round the page size so it reveals less", self.letterbox, "Fingerprinting"),
            t("light_scheme_only", "Hide colour scheme", "Always report a light theme to pages", self.light_scheme_only, "Fingerprinting"),
            t("tor", "Route through Tor", "Use the built-in Tor client for all traffic", self.uses_tor(), "Network"),
            t("load_images", "Load images", "Turn off to save data and memory", self.load_images, "Resources"),
            t("javascript", "JavaScript (experimental)", "Run page scripts; applies to pages loaded from now on", self.javascript, "Resources"),
        ]
    }

    /// Rows for the full settings page.
    pub fn sections(&self) -> Vec<SettingsSection> {
        let toggle = |id: &str, label: &str, detail: &str, on: bool| SettingsRow::Toggle { id: id.into(), label: label.into(), detail: detail.into(), on };
        let choice = |id: &str, label: &str, detail: &str, options: &[(&str, &str)], selected: &str| SettingsRow::Choice {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
            options: options.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
            selected: selected.into(),
        };
        let text = |id: &str, label: &str, detail: &str, value: &str, placeholder: &str| SettingsRow::Text {
            id: id.into(),
            label: label.into(),
            detail: detail.into(),
            value: value.into(),
            placeholder: placeholder.into(),
        };
        vec![
            SettingsSection {
                title: "Tracking protection".into(),
                blurb: "What InvisBrowser refuses to send or load.".into(),
                rows: vec![
                    toggle("block_trackers", "Block trackers and ads", "Requests to known tracking hosts are never made.", self.block_trackers),
                    toggle("https_only", "HTTPS-only mode", "Plain HTTP is upgraded; if the site has no HTTPS the load is refused.", self.https_only),
                    toggle("strip_tracking_params", "Strip tracking parameters", "utm_*, fbclid, gclid and friends are removed from links.", self.strip_tracking_params),
                    toggle("send_gpc", "Send Global Privacy Control", "Adds Sec-GPC: 1 and DNT: 1 to every request.", self.send_gpc),
                    choice(
                        "third_party",
                        "Third-party requests",
                        "What other sites embedded in a page may do.",
                        &[("allow", "Allow"), ("cookies", "No cookies"), ("scripts", "Block scripts and frames"), ("all", "Block all")],
                        &self.third_party,
                    ),
                    choice(
                        "cookies",
                        "Cookies",
                        "Cookies live in memory only and are gone when you close the browser.",
                        &[("none", "Block all"), ("first-party", "First-party only"), ("partitioned", "Allow, partitioned per site")],
                        &self.cookies,
                    ),
                    choice(
                        "referrer",
                        "Referrer",
                        "How much of the current address is revealed to the next site.",
                        &[("never", "Never send"), ("origin", "Site name only"), ("full", "Full address")],
                        &self.referrer,
                    ),
                ],
            },
            SettingsSection {
                title: "Fingerprinting".into(),
                blurb: "Every InvisBrowser sends the same headers and exposes the same fonts. These add more.".into(),
                rows: vec![
                    toggle("letterbox", "Letterboxing", "The page area is rounded to steps of 100 px so its size is shared by many users.", self.letterbox),
                    toggle("light_scheme_only", "Hide colour scheme", "Pages are always told the system uses a light theme.", self.light_scheme_only),
                    toggle("generic_language", "Generic language", "Always request pages in en-US.", self.generic_language),
                ],
            },
            SettingsSection {
                title: "Network route".into(),
                blurb: "Where your traffic goes. With a proxy or Tor, host names are resolved remotely, never on this machine.".into(),
                rows: vec![
                    choice(
                        "proxy",
                        "Connection",
                        "Tor is built in; no separate program is needed.",
                        &[("direct", "Direct"), ("tor", "Tor"), ("socks5", "SOCKS5 proxy"), ("http", "HTTP proxy")],
                        &self.proxy,
                    ),
                    text("proxy_host", "Proxy host", "Used for SOCKS5 and HTTP proxies.", &self.proxy_host, "127.0.0.1"),
                    text("proxy_port", "Proxy port", "", &self.proxy_port.to_string(), "1080"),
                    text("proxy_user", "Proxy user", "Leave empty if the proxy needs no login.", &self.proxy_user, ""),
                    text("proxy_pass", "Proxy password", "", &self.proxy_pass, ""),
                    toggle("tor_isolate", "Separate Tor circuit per site", "Sites cannot link your visits through a shared exit.", self.tor_isolate),
                    choice(
                        "dns",
                        "DNS (direct connections)",
                        "Encrypted DNS hides lookups from your network provider.",
                        &[("quad9", "Quad9 (encrypted)"), ("cloudflare", "Cloudflare (encrypted)"), ("mullvad", "Mullvad (encrypted)"), ("system", "System resolver")],
                        &self.dns,
                    ),
                    toggle("dns_strict", "Never fall back to system DNS", "Fail the lookup rather than use the unencrypted resolver.", self.dns_strict),
                ],
            },
            SettingsSection {
                title: "Memory and power".into(),
                blurb: "InvisBrowser draws only when something changes and keeps no disk cache.".into(),
                rows: vec![
                    toggle("load_images", "Load images", "Images are the largest memory cost of most pages.", self.load_images),
                    toggle("javascript", "JavaScript (experimental)", "Run page scripts. The engine is still young: many script-heavy sites do not work yet. Applies to pages loaded from now on.", self.javascript),
                    toggle("hibernate_tabs", "Hibernate background tabs", "Tabs you are not looking at drop their layout and textures.", self.hibernate_tabs),
                    toggle("low_power_gpu", "Prefer the low-power GPU", "Takes effect on the next start.", self.low_power_gpu),
                    text("cache_mb", "Memory cache (MiB)", "In-memory only; 0 disables it.", &self.cache_mb.to_string(), "32"),
                    text("search", "Search address", "The query is appended to this address.", &self.search, "https://html.duckduckgo.com/html/?q="),
                ],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_toggle() {
        let mut s = Settings::default();
        assert_eq!(s.get("block_trackers").as_deref(), Some("on"));
        s.toggle("block_trackers");
        assert!(!s.block_trackers);
        s.set("proxy_port", "9050");
        s.set("proxy_port", "not a number");
        assert_eq!(s.proxy_port, 9050);
        s.toggle("tor");
        assert!(s.uses_tor());
        assert!(matches!(s.net_config().proxy, ProxyConfig::Tor));
        s.set("nonsense", "x");
        // a proxy with no host fails safe to direct only when the user picked a proxy type
        // without configuring it; Tor never falls back
        s.set("proxy", "socks5");
        assert!(matches!(s.net_config().proxy, ProxyConfig::Direct));
    }
}
