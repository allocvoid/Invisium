//! HTML tokenizer: turns markup into start tags, end tags and decoded text.
//! Comments are dropped at this level (nothing in the engine reads them), which also saves memory.

use super::entities;
use crate::atom::{a, Atom};

pub enum Token {
    Doctype,
    Start { name: Atom, attrs: Vec<(Atom, String)>, self_closing: bool },
    End { name: Atom },
    Text(String),
    Eof,
}

#[derive(Clone, Copy, PartialEq)]
enum Raw {
    None,
    /// Raw text up to `</name`; no entity decoding (script, style, ...).
    RawText(Atom),
    /// Raw text up to `</name`; entities decoded (title, textarea).
    RcData(Atom),
    Plaintext,
}

pub struct Tokenizer<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    raw: Raw,
    /// When true `<noscript>` content is raw text (scripting enabled); when false it is markup.
    pub scripting: bool,
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a str, scripting: bool) -> Self {
        Tokenizer { src, b: src.as_bytes(), pos: 0, raw: Raw::None, scripting }
    }

    fn find(&self, from: usize, needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || from >= self.b.len() {
            return None;
        }
        let first = needle[0];
        let hay = &self.b[from..];
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            match hay[i..].iter().position(|&c| c == first) {
                None => return None,
                Some(p) => {
                    i += p;
                    if i + needle.len() > hay.len() {
                        return None;
                    }
                    if &hay[i..i + needle.len()] == needle {
                        return Some(from + i);
                    }
                    i += 1;
                }
            }
        }
        None
    }

    /// Find `</name` (ASCII case-insensitive) followed by whitespace, `/` or `>`.
    fn find_end_tag(&self, from: usize, name: &str) -> Option<usize> {
        let nb = name.as_bytes();
        let mut i = from;
        while let Some(p) = self.find(i, b"</") {
            let s = p + 2;
            if s + nb.len() <= self.b.len()
                && self.b[s..s + nb.len()].eq_ignore_ascii_case(nb)
                && self.b.get(s + nb.len()).map_or(true, |&c| is_ws(c) || c == b'/' || c == b'>')
            {
                return Some(p);
            }
            i = p + 2;
        }
        None
    }

    pub fn next_token(&mut self) -> Token {
        loop {
            if self.pos >= self.b.len() {
                return Token::Eof;
            }
            match self.raw {
                Raw::Plaintext => {
                    let t = self.src[self.pos..].to_owned();
                    self.pos = self.b.len();
                    return Token::Text(t);
                }
                Raw::RawText(name) | Raw::RcData(name) => {
                    let rc = matches!(self.raw, Raw::RcData(_));
                    self.raw = Raw::None;
                    let end = self.find_end_tag(self.pos, name.as_str()).unwrap_or(self.b.len());
                    if end > self.pos {
                        let s = &self.src[self.pos..end];
                        self.pos = end;
                        return Token::Text(if rc { entities::decode(s, false) } else { s.to_owned() });
                    }
                    continue;
                }
                Raw::None => {}
            }

            if self.b[self.pos] != b'<' {
                let end = self.b[self.pos..].iter().position(|&c| c == b'<').map_or(self.b.len(), |p| self.pos + p);
                let s = &self.src[self.pos..end];
                self.pos = end;
                return Token::Text(if s.as_bytes().contains(&b'&') { entities::decode(s, false) } else { s.to_owned() });
            }

            // at '<'
            let next = self.b.get(self.pos + 1).copied().unwrap_or(0);
            if next.is_ascii_alphabetic() {
                self.pos += 1;
                return self.tag(false);
            }
            if next == b'/' {
                let n2 = self.b.get(self.pos + 2).copied().unwrap_or(0);
                if n2.is_ascii_alphabetic() {
                    self.pos += 2;
                    return self.tag(true);
                }
                if n2 == b'>' {
                    self.pos += 3;
                    continue;
                }
                // bogus comment
                self.pos = self.find(self.pos, b">").map_or(self.b.len(), |p| p + 1);
                continue;
            }
            if next == b'!' {
                let rest = &self.b[self.pos + 2..];
                if rest.starts_with(b"--") {
                    let body = self.pos + 4;
                    if self.b[body..].starts_with(b">") {
                        self.pos = body + 1;
                    } else if self.b[body..].starts_with(b"->") {
                        self.pos = body + 2;
                    } else {
                        self.pos = self.find(body, b"-->").map_or(self.b.len(), |p| p + 3);
                    }
                    continue;
                }
                if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case(b"doctype") {
                    self.pos = self.find(self.pos, b">").map_or(self.b.len(), |p| p + 1);
                    return Token::Doctype;
                }
                if rest.starts_with(b"[CDATA[") {
                    let start = self.pos + 9;
                    let end = self.find(start, b"]]>").unwrap_or(self.b.len());
                    let s = self.src[start..end].to_owned();
                    self.pos = (end + 3).min(self.b.len());
                    if s.is_empty() {
                        continue;
                    }
                    return Token::Text(s);
                }
                self.pos = self.find(self.pos, b">").map_or(self.b.len(), |p| p + 1);
                continue;
            }
            if next == b'?' {
                self.pos = self.find(self.pos, b">").map_or(self.b.len(), |p| p + 1);
                continue;
            }
            // A lone '<' is text.
            let end = self.b[self.pos + 1..]
                .iter()
                .position(|&c| c == b'<')
                .map_or(self.b.len(), |p| self.pos + 1 + p);
            let s = &self.src[self.pos..end];
            self.pos = end;
            return Token::Text(entities::decode(s, false));
        }
    }

    /// Parse a tag; `self.pos` is at the first character of the tag name.
    fn tag(&mut self, is_end: bool) -> Token {
        let start = self.pos;
        while self.pos < self.b.len() && !is_ws(self.b[self.pos]) && self.b[self.pos] != b'/' && self.b[self.pos] != b'>' {
            self.pos += 1;
        }
        let name = Atom::lower(&self.src[start..self.pos]);
        let mut attrs: Vec<(Atom, String)> = Vec::new();
        let mut self_closing = false;

        loop {
            while self.pos < self.b.len() && is_ws(self.b[self.pos]) {
                self.pos += 1;
            }
            if self.pos >= self.b.len() {
                break;
            }
            let c = self.b[self.pos];
            if c == b'>' {
                self.pos += 1;
                break;
            }
            if c == b'/' {
                if self.b.get(self.pos + 1) == Some(&b'>') {
                    self_closing = true;
                    self.pos += 2;
                    break;
                }
                self.pos += 1;
                continue;
            }
            // attribute name
            let ns = self.pos;
            self.pos += 1; // a leading '=' is part of the name
            while self.pos < self.b.len() {
                let c = self.b[self.pos];
                if is_ws(c) || c == b'/' || c == b'>' || c == b'=' {
                    break;
                }
                self.pos += 1;
            }
            let aname = &self.src[ns..self.pos];
            while self.pos < self.b.len() && is_ws(self.b[self.pos]) {
                self.pos += 1;
            }
            let mut value = String::new();
            if self.b.get(self.pos) == Some(&b'=') {
                self.pos += 1;
                while self.pos < self.b.len() && is_ws(self.b[self.pos]) {
                    self.pos += 1;
                }
                if let Some(&q) = self.b.get(self.pos) {
                    let raw: &str;
                    if q == b'"' || q == b'\'' {
                        let vs = self.pos + 1;
                        let ve = self.b[vs..].iter().position(|&c| c == q).map_or(self.b.len(), |p| vs + p);
                        raw = &self.src[vs..ve];
                        self.pos = (ve + 1).min(self.b.len());
                    } else {
                        let vs = self.pos;
                        while self.pos < self.b.len() && !is_ws(self.b[self.pos]) && self.b[self.pos] != b'>' {
                            self.pos += 1;
                        }
                        raw = &self.src[vs..self.pos];
                    }
                    value = if raw.as_bytes().contains(&b'&') { entities::decode(raw, true) } else { raw.to_owned() };
                }
            }
            if !is_end && attrs.len() < 256 {
                attrs.push((Atom::lower(aname), value));
            }
        }

        if is_end {
            return Token::End { name };
        }

        // Switch to a raw-text mode for elements whose content is not markup.
        self.raw = match name {
            a::SCRIPT | a::STYLE | a::XMP | a::IFRAME | a::NOEMBED | a::NOFRAMES => Raw::RawText(name),
            a::NOSCRIPT if self.scripting => Raw::RawText(name),
            a::TITLE | a::TEXTAREA => Raw::RcData(name),
            a::PLAINTEXT => Raw::Plaintext,
            _ => Raw::None,
        };
        Token::Start { name, attrs, self_closing }
    }

    /// Cancel raw-text mode (used for `<title>`/`<style>` inside SVG, which are ordinary markup).
    pub fn cancel_raw(&mut self) {
        self.raw = Raw::None;
    }
}
