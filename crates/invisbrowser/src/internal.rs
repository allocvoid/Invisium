//! `invis://` addresses: the browser's own pages and the actions their links trigger.

use crate::loader::{encode_query, query_param};
use crate::settings::Settings;
use invis_chrome::pages;

pub enum Internal {
    /// Show this document.
    Html(String),
    /// Go to this address instead. `settings_changed` = a setting was modified on the way.
    Redirect { url: String, settings_changed: bool },
}

/// Facts shown on the about page and the new-tab page.
#[derive(Default, Clone)]
pub struct Info {
    pub rows: Vec<(String, String)>,
    pub stats: Vec<(String, String)>,
}

pub fn handle(url: &str, settings: &mut Settings, info: &Info) -> Internal {
    let path = url.strip_prefix("invis://").unwrap_or("").split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
    match path {
        "" | "newtab" => Internal::Html(pages::new_tab("invis://search", settings.uses_tor(), &info.stats)),
        "search" => {
            let q = query_param(url, "q").unwrap_or_default();
            if q.trim().is_empty() {
                Internal::Redirect { url: "invis://newtab".into(), settings_changed: false }
            } else {
                // the same rule as the address bar: an address is opened, anything else is searched
                Internal::Redirect { url: crate::loader::fix_url(&q, &settings.search), settings_changed: false }
            }
        }
        "settings" => Internal::Html(pages::settings(&settings.sections())),
        "settings/toggle" => {
            if let Some(id) = query_param(url, "id") {
                settings.toggle(&id);
                settings.save();
            }
            Internal::Redirect { url: "invis://settings".into(), settings_changed: true }
        }
        "settings/set" => {
            if let (Some(id), Some(value)) = (query_param(url, "id"), query_param(url, "value")) {
                settings.set(&id, &value);
                settings.save();
            }
            Internal::Redirect { url: "invis://settings".into(), settings_changed: true }
        }
        "about" => Internal::Html(pages::about(&info.rows)),
        "retry" => match query_param(url, "url") {
            Some(u) if !u.starts_with("invis://retry") => Internal::Redirect { url: u, settings_changed: false },
            _ => Internal::Redirect { url: "invis://newtab".into(), settings_changed: false },
        },
        _ => Internal::Html(pages::error("No such page", "InvisBrowser has no internal page with this name.", url, None)),
    }
}

#[allow(dead_code)]
pub fn search_url(settings: &Settings, q: &str) -> String {
    format!("{}{}", settings.search, encode_query(q))
}
