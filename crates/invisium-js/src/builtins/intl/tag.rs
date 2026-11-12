//! BCP 47 language tags: structural validation and canonical casing. Tags are parsed so that
//! malformed ones are rejected the way other engines reject them; they never select data.

pub(super) struct Tag {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
    /// Unicode (`-u-`) extension: attributes, then (key, type) keywords sorted by key.
    pub attributes: Vec<String>,
    pub keywords: Vec<(String, String)>,
    /// Other extensions, by singleton.
    pub extensions: Vec<(char, Vec<String>)>,
    pub private: Vec<String>,
}

fn alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

fn digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

fn title(s: &str) -> String {
    let mut out = s.to_ascii_lowercase();
    if let Some(first) = out.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    out
}

fn is_variant(s: &str) -> bool {
    (5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit())
}

/// Parses a tag; `None` when it is not a structurally valid language tag.
pub(super) fn parse(text: &str) -> Option<Tag> {
    // a tag longer than any real one is rejected before any work is done on it
    if text.is_empty() || text.len() > 256 {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    let subs: Vec<&str> = lower.split('-').collect();
    if subs.iter().any(|s| s.is_empty() || s.len() > 8 || !s.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return None;
    }
    let mut i = 0;
    let lang = subs[0];
    if !alpha(lang) || !(matches!(lang.len(), 2 | 3) || (5..=8).contains(&lang.len())) {
        return None;
    }
    i += 1;
    let mut t = Tag {
        language: lang.to_string(),
        script: None,
        region: None,
        variants: Vec::new(),
        attributes: Vec::new(),
        keywords: Vec::new(),
        extensions: Vec::new(),
        private: Vec::new(),
    };
    if let Some(s) = subs.get(i).filter(|s| s.len() == 4 && alpha(s)) {
        t.script = Some(title(s));
        i += 1;
    }
    if let Some(s) = subs.get(i).filter(|s| (s.len() == 2 && alpha(s)) || (s.len() == 3 && digits(s))) {
        t.region = Some(s.to_ascii_uppercase());
        i += 1;
    }
    while let Some(s) = subs.get(i).filter(|s| is_variant(s)) {
        if t.variants.iter().any(|v| v == s) {
            return None;
        }
        t.variants.push(s.to_string());
        i += 1;
    }
    let mut seen_u = false;
    while let Some(s) = subs.get(i) {
        if s.len() != 1 {
            return None;
        }
        let single = s.chars().next()?;
        i += 1;
        if single == 'x' {
            t.private = subs[i..].iter().map(|s| s.to_string()).collect();
            if t.private.is_empty() {
                return None;
            }
            i = subs.len();
            break;
        }
        let start = i;
        while subs.get(i).is_some_and(|s| s.len() >= 2) {
            i += 1;
        }
        let body = &subs[start..i];
        if body.is_empty() {
            return None;
        }
        if single == 'u' {
            if seen_u {
                return None;
            }
            seen_u = true;
            let mut j = 0;
            while j < body.len() && body[j].len() != 2 {
                t.attributes.push(body[j].to_string());
                j += 1;
            }
            while j < body.len() {
                let key = body[j].to_string();
                j += 1;
                let from = j;
                while j < body.len() && body[j].len() != 2 {
                    j += 1;
                }
                let ty = body[from..j].join("-");
                if t.keywords.iter().any(|(k, _)| *k == key) {
                    return None;
                }
                // `true` is the default value of a keyword and is dropped
                t.keywords.push((key, if ty == "true" { String::new() } else { ty }));
            }
        } else {
            if t.extensions.iter().any(|(c, _)| *c == single) {
                return None;
            }
            t.extensions.push((single, body.iter().map(|s| s.to_string()).collect()));
        }
    }
    if i != subs.len() {
        return None;
    }
    t.variants.sort();
    t.keywords.sort();
    t.extensions.sort();
    Some(t)
}

impl Tag {
    /// Language, script, region and variants: the tag without extensions.
    pub fn base_name(&self) -> String {
        let mut out = self.language.clone();
        for part in self.script.iter().chain(self.region.iter()).chain(self.variants.iter()) {
            out.push('-');
            out.push_str(part);
        }
        out
    }

    pub fn canonical(&self) -> String {
        let mut out = self.base_name();
        let mut unicode = String::new();
        for a in &self.attributes {
            unicode.push('-');
            unicode.push_str(a);
        }
        for (k, v) in &self.keywords {
            unicode.push('-');
            unicode.push_str(k);
            if !v.is_empty() {
                unicode.push('-');
                unicode.push_str(v);
            }
        }
        // extensions in singleton order, the unicode one among them
        let mut wrote_u = unicode.is_empty();
        for (c, body) in &self.extensions {
            if !wrote_u && *c > 'u' {
                out.push_str("-u");
                out.push_str(&unicode);
                wrote_u = true;
            }
            out.push('-');
            out.push(*c);
            for b in body {
                out.push('-');
                out.push_str(b);
            }
        }
        if !wrote_u {
            out.push_str("-u");
            out.push_str(&unicode);
        }
        if !self.private.is_empty() {
            out.push_str("-x");
            for p in &self.private {
                out.push('-');
                out.push_str(p);
            }
        }
        out
    }

    /// The type of a unicode keyword (`ca`, `nu`, `hc`, ...); `Some("")` for a bare key.
    pub fn keyword(&self, key: &str) -> Option<&str> {
        self.keywords.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn set_keyword(&mut self, key: &str, value: &str) {
        self.keywords.retain(|(k, _)| k != key);
        self.keywords.push((key.to_string(), value.to_string()));
        self.keywords.sort();
    }
}
