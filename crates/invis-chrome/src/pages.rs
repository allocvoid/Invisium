//! HTML for the browser's internal pages. No JavaScript: links and GET forms only.

use std::fmt::Write;

/// Stylesheet shared by all internal pages (the site's tokens + the components used).
pub fn stylesheet() -> &'static str {
    include_str!("pages.css")
}

pub struct SettingsSection {
    pub title: String,
    pub blurb: String,
    pub rows: Vec<SettingsRow>,
}

pub enum SettingsRow {
    Toggle { id: String, label: String, detail: String, on: bool },
    Choice { id: String, label: String, detail: String, options: Vec<(String, String)>, selected: String },
    Text { id: String, label: String, detail: String, value: String, placeholder: String },
    Link { label: String, detail: String, href: String },
}

/// HTML-escape text for element content and quoted attribute values.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() && c != '\n' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// Percent-encode a query component.
fn enc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{b:02X}");
        }
    }
    o
}

/// Only `invis://` destinations are allowed; anything else becomes the about page.
fn internal_href(href: &str) -> String {
    if href.starts_with("invis://") && !href.chars().any(|c| c.is_control() || c.is_whitespace()) {
        esc(href)
    } else {
        "invis://about".to_string()
    }
}

fn document(title: &str, active: &str, body: &str) -> String {
    let nav = [("newtab", "New tab"), ("settings", "Settings"), ("about", "About")];
    let mut links = String::new();
    for (k, label) in nav {
        let cls = if k == active { " class=\"active\"" } else { "" };
        let _ = write!(links, "<a href=\"invis://{k}\"{cls}>{label}</a>");
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>{}</title><style>{}</style></head><body>\
<div class=\"header\"><div class=\"header-inner\"><div class=\"logo-text\"><span>Invis</span><span>Browser</span></div>\
<div class=\"nav\">{}</div></div></div><div class=\"main\">{}</div></body></html>",
        esc(title),
        stylesheet(),
        links,
        body
    )
}

pub fn new_tab(search_action: &str, tor: bool, stats: &[(String, String)]) -> String {
    let mut b = String::new();
    let badge = if tor { "<span class=\"badge badge-success\">Routed through Tor</span>" } else { "<span class=\"badge badge-info\">Direct connection</span>" };
    let _ = write!(
        b,
        "<div class=\"hero\"><h1>Browse privately</h1><p>No telemetry. No tracking. Nothing leaves this device unless you ask it to.</p>{badge}</div>\
<form class=\"search\" action=\"{}\" method=\"get\"><input class=\"form-input\" name=\"q\" placeholder=\"Search or enter address\"><button class=\"btn-primary\" type=\"submit\">Search</button></form>",
        esc(search_action)
    );
    if !stats.is_empty() {
        b.push_str("<div class=\"grid\">");
        for (label, value) in stats {
            let _ = write!(b, "<div class=\"stat\"><div class=\"stat-value\">{}</div><div class=\"stat-label\">{}</div></div>", esc(value), esc(label));
        }
        b.push_str("</div>");
    }
    document("New tab", "newtab", &b)
}

fn settings_row(r: &SettingsRow) -> String {
    match r {
        SettingsRow::Toggle { id, label, detail, on } => format!(
            "<a class=\"row\" href=\"invis://settings/toggle?id={}\"><div class=\"row-text\"><div class=\"row-label\">{}</div><div class=\"row-detail\">{}</div></div><span class=\"toggle-track{}\"></span></a>",
            enc(id),
            esc(label),
            esc(detail),
            if *on { " active" } else { "" }
        ),
        SettingsRow::Choice { id, label, detail, options, selected } => {
            let mut opts = String::new();
            for (value, name) in options {
                let cls = if value == selected { "choice active" } else { "choice" };
                let _ = write!(opts, "<a class=\"{cls}\" href=\"invis://settings/set?id={}&amp;value={}\">{}</a>", enc(id), enc(value), esc(name));
            }
            format!("<div class=\"row\"><div class=\"row-text\"><div class=\"row-label\">{}</div><div class=\"row-detail\">{}</div></div><div class=\"choices\">{opts}</div></div>", esc(label), esc(detail))
        }
        SettingsRow::Text { id, label, detail, value, placeholder } => format!(
            "<div class=\"row\"><div class=\"row-text\"><div class=\"row-label\">{}</div><div class=\"row-detail\">{}</div></div>\
<form action=\"invis://settings/set\" method=\"get\"><input type=\"hidden\" name=\"id\" value=\"{}\"><input class=\"form-input\" name=\"value\" value=\"{}\" placeholder=\"{}\"><button class=\"btn-primary-sm\" type=\"submit\">Save</button></form></div>",
            esc(label),
            esc(detail),
            esc(id),
            esc(value),
            esc(placeholder)
        ),
        SettingsRow::Link { label, detail, href } => format!(
            "<a class=\"row\" href=\"{}\"><div class=\"row-text\"><div class=\"row-label\">{}</div><div class=\"row-detail\">{}</div></div></a>",
            internal_href(href),
            esc(label),
            esc(detail)
        ),
    }
}

pub fn settings(sections: &[SettingsSection]) -> String {
    let mut b = String::from("<h1>Settings</h1>");
    for s in sections {
        let _ = write!(b, "<div class=\"card\"><h2>{}</h2><p>{}</p>", esc(&s.title), esc(&s.blurb));
        for r in &s.rows {
            b.push_str(&settings_row(r));
        }
        b.push_str("</div>");
    }
    document("Settings", "settings", &b)
}

pub fn about(rows: &[(String, String)]) -> String {
    let mut b = String::from("<h1>About InvisBrowser</h1><div class=\"card\"><table class=\"about\">");
    for (k, v) in rows {
        let _ = write!(b, "<tr><td>{}</td><td>{}</td></tr>", esc(k), esc(v));
    }
    b.push_str("</table></div>");
    document("About", "about", &b)
}

/// `retry_url` becomes an `invis://retry?url=` link, so the page itself never links off-device.
pub fn error(title: &str, detail: &str, url: &str, retry_url: Option<&str>) -> String {
    let mut b = format!("<div class=\"error\"><h1>{}</h1><p>{}</p><p><span class=\"error-url\">{}</span></p>", esc(title), esc(detail), esc(url));
    if let Some(r) = retry_url {
        let _ = write!(b, "<p><a class=\"btn-primary\" href=\"invis://retry?url={}\">Try again</a></p>", enc(r));
    }
    b.push_str("</div>");
    document(title, "", &b)
}
