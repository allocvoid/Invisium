//! A minimal, forgiving XML tag reader: start/end tags and attributes only. Text, comments,
//! CDATA, processing instructions and DOCTYPEs are skipped.

use std::borrow::Cow;

pub enum Event<'a> {
    Start { name: &'a str, attrs: Vec<(&'a str, Cow<'a, str>)>, self_closing: bool },
    End,
}

pub struct Reader<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a str) -> Reader<'a> {
        Reader { src, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_past(&mut self, pat: &str) {
        match self.rest().find(pat) {
            Some(i) => self.pos += i + pat.len(),
            None => self.pos = self.src.len(),
        }
    }

    fn skip_ws(&mut self) {
        let b = self.src.as_bytes();
        while self.pos < b.len() && b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn name(&mut self) -> &'a str {
        let b = self.src.as_bytes();
        let start = self.pos;
        while self.pos < b.len() {
            let c = b[self.pos];
            if c.is_ascii_whitespace() || matches!(c, b'/' | b'>' | b'=' | b'<' | b'"' | b'\'') {
                break;
            }
            self.pos += 1;
        }
        &self.src[start..self.pos]
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        loop {
            match self.rest().find('<') {
                Some(i) => self.pos += i,
                None => {
                    self.pos = self.src.len();
                    return None;
                }
            }
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.skip_past("-->");
            } else if rest.starts_with("<![CDATA[") {
                self.skip_past("]]>");
            } else if rest.starts_with("<!") {
                let gt = rest.find('>').unwrap_or(rest.len());
                if rest[..gt].contains('[') {
                    self.skip_past("]>");
                } else {
                    self.skip_past(">");
                }
            } else if rest.starts_with("<?") {
                self.skip_past("?>");
            } else if rest.starts_with("</") {
                self.skip_past(">");
                return Some(Event::End);
            } else {
                self.pos += 1;
                let name = self.name();
                if name.is_empty() {
                    continue; // a stray '<' in text
                }
                let name = name.rsplit(':').next().unwrap_or(name);
                let mut attrs = Vec::new();
                let b = self.src.as_bytes();
                loop {
                    self.skip_ws();
                    if self.pos >= b.len() {
                        return Some(Event::Start { name, attrs, self_closing: true });
                    }
                    match b[self.pos] {
                        b'>' => {
                            self.pos += 1;
                            return Some(Event::Start { name, attrs, self_closing: false });
                        }
                        b'/' => {
                            self.pos += 1;
                            self.skip_ws();
                            if self.pos < b.len() && b[self.pos] == b'>' {
                                self.pos += 1;
                                return Some(Event::Start { name, attrs, self_closing: true });
                            }
                        }
                        b'<' => return Some(Event::Start { name, attrs, self_closing: false }),
                        _ => {
                            let an = self.name();
                            if an.is_empty() {
                                self.pos += 1; // junk such as a stray quote or '='
                                continue;
                            }
                            self.skip_ws();
                            let mut value = Cow::Borrowed("");
                            if self.pos < b.len() && b[self.pos] == b'=' {
                                self.pos += 1;
                                self.skip_ws();
                                if self.pos < b.len() && (b[self.pos] == b'"' || b[self.pos] == b'\'') {
                                    let q = b[self.pos] as char;
                                    self.pos += 1;
                                    let end = self.rest().find(q).map(|i| self.pos + i).unwrap_or(self.src.len());
                                    value = decode_entities(&self.src[self.pos..end]);
                                    self.pos = (end + 1).min(self.src.len());
                                } else {
                                    value = Cow::Borrowed(self.name());
                                }
                            }
                            attrs.push((an, value));
                        }
                    }
                }
            }
        }
    }
}

fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}
