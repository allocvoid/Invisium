//! CSS tokenizer (CSS Syntax Level 3, minus a few obscure corners).

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Function(String),
    AtKeyword(String),
    Hash(String),
    Str(String),
    Url(String),
    Number(f32),
    Percentage(f32),
    Dimension(f32, String),
    Whitespace,
    Colon,
    Semicolon,
    Comma,
    LBracket,
    RBracket,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Delim(char),
}

impl Tok {
    pub fn is_ident(&self, s: &str) -> bool {
        matches!(self, Tok::Ident(i) if i.eq_ignore_ascii_case(s))
    }
    pub fn is_function(&self, s: &str) -> bool {
        matches!(self, Tok::Function(i) if i.eq_ignore_ascii_case(s))
    }
}

pub struct Tokens {
    pub toks: Vec<Tok>,
    /// Byte span of each token in the source.
    pub spans: Vec<(u32, u32)>,
}

struct Lexer<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

#[inline]
fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

#[inline]
fn is_name(c: u8) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == b'-'
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

impl<'a> Lexer<'a> {
    fn peek(&self, off: usize) -> u8 {
        self.b.get(self.i + off).copied().unwrap_or(0)
    }

    fn valid_escape(&self, off: usize) -> bool {
        self.peek(off) == b'\\' && self.peek(off + 1) != b'\n' && self.i + off + 1 < self.b.len()
    }

    fn starts_ident(&self) -> bool {
        let c = self.peek(0);
        if c == b'-' {
            let d = self.peek(1);
            is_name_start(d) || d == b'-' || self.valid_escape(1)
        } else {
            is_name_start(c) || self.valid_escape(0)
        }
    }

    fn starts_number(&self) -> bool {
        let c = self.peek(0);
        if c == b'+' || c == b'-' {
            let d = self.peek(1);
            d.is_ascii_digit() || (d == b'.' && self.peek(2).is_ascii_digit())
        } else if c == b'.' {
            self.peek(1).is_ascii_digit()
        } else {
            c.is_ascii_digit()
        }
    }

    fn escape(&mut self, out: &mut String) {
        // self.i is just past the backslash
        let mut n = 0;
        let mut code: u32 = 0;
        while n < 6 && self.i < self.b.len() && self.b[self.i].is_ascii_hexdigit() {
            code = code * 16 + (self.b[self.i] as char).to_digit(16).unwrap();
            self.i += 1;
            n += 1;
        }
        if n > 0 {
            if self.i < self.b.len() && is_ws(self.b[self.i]) {
                self.i += 1;
            }
            out.push(char::from_u32(code).filter(|&c| c != '\0').unwrap_or('\u{fffd}'));
        } else if self.i < self.b.len() {
            let ch = self.s[self.i..].chars().next().unwrap();
            out.push(ch);
            self.i += ch.len_utf8();
        }
    }

    fn name(&mut self) -> String {
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && is_name(self.b[self.i]) {
                self.i += 1;
            }
            out.push_str(&self.s[start..self.i]);
            if self.valid_escape(0) {
                self.i += 1;
                self.escape(&mut out);
            } else {
                break;
            }
        }
        out
    }

    fn number(&mut self) -> f32 {
        let start = self.i;
        if matches!(self.peek(0), b'+' | b'-') {
            self.i += 1;
        }
        while self.peek(0).is_ascii_digit() {
            self.i += 1;
        }
        if self.peek(0) == b'.' && self.peek(1).is_ascii_digit() {
            self.i += 1;
            while self.peek(0).is_ascii_digit() {
                self.i += 1;
            }
        }
        if matches!(self.peek(0), b'e' | b'E') {
            let d = self.peek(1);
            if d.is_ascii_digit() || (matches!(d, b'+' | b'-') && self.peek(2).is_ascii_digit()) {
                self.i += 2;
                while self.peek(0).is_ascii_digit() {
                    self.i += 1;
                }
            }
        }
        self.s[start..self.i].parse::<f32>().unwrap_or(0.0)
    }

    fn string(&mut self, quote: u8) -> Tok {
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && !matches!(self.b[self.i], b'\\' | b'\n') && self.b[self.i] != quote {
                self.i += 1;
            }
            out.push_str(&self.s[start..self.i]);
            if self.i >= self.b.len() {
                break;
            }
            match self.b[self.i] {
                b'\\' => {
                    self.i += 1;
                    if self.i >= self.b.len() {
                        break;
                    }
                    if self.b[self.i] == b'\n' {
                        self.i += 1; // line continuation
                    } else {
                        self.escape(&mut out);
                    }
                }
                b'\n' => break, // bad string: recover at the newline
                _ => {
                    self.i += 1; // closing quote
                    break;
                }
            }
        }
        Tok::Str(out)
    }

    fn url_or_function(&mut self, name: String) -> Tok {
        // self.i is just past '('
        if name.eq_ignore_ascii_case("url") {
            let mut j = self.i;
            while j < self.b.len() && is_ws(self.b[j]) {
                j += 1;
            }
            let c = self.b.get(j).copied().unwrap_or(0);
            if c == b'"' || c == b'\'' {
                return Tok::Function(name); // url("...") is tokenized as a function + string
            }
            self.i = j;
            let mut out = String::new();
            loop {
                if self.i >= self.b.len() {
                    break;
                }
                let c = self.b[self.i];
                if c == b')' {
                    self.i += 1;
                    break;
                }
                if is_ws(c) {
                    while self.i < self.b.len() && is_ws(self.b[self.i]) {
                        self.i += 1;
                    }
                    if self.peek(0) == b')' {
                        self.i += 1;
                    } else {
                        // bad url: skip to ')'
                        while self.i < self.b.len() && self.b[self.i] != b')' {
                            self.i += 1;
                        }
                        self.i = (self.i + 1).min(self.b.len());
                    }
                    break;
                }
                if c == b'\\' {
                    self.i += 1;
                    self.escape(&mut out);
                    continue;
                }
                let ch = self.s[self.i..].chars().next().unwrap();
                out.push(ch);
                self.i += ch.len_utf8();
            }
            return Tok::Url(out);
        }
        Tok::Function(name)
    }

    fn next(&mut self) -> Option<Tok> {
        // comments
        while self.peek(0) == b'/' && self.peek(1) == b'*' {
            let rest = &self.b[self.i + 2..];
            let end = rest.windows(2).position(|w| w == b"*/");
            self.i = match end {
                Some(p) => self.i + 2 + p + 2,
                None => self.b.len(),
            };
        }
        if self.i >= self.b.len() {
            return None;
        }
        let c = self.b[self.i];
        if is_ws(c) {
            while self.i < self.b.len() && is_ws(self.b[self.i]) {
                self.i += 1;
            }
            return Some(Tok::Whitespace);
        }
        if c == b'"' || c == b'\'' {
            self.i += 1;
            return Some(self.string(c));
        }
        if self.starts_number() {
            let v = self.number();
            if self.starts_ident() {
                let unit = self.name();
                return Some(Tok::Dimension(v, unit.to_ascii_lowercase()));
            }
            if self.peek(0) == b'%' {
                self.i += 1;
                return Some(Tok::Percentage(v));
            }
            return Some(Tok::Number(v));
        }
        if self.starts_ident() {
            let name = self.name();
            if self.peek(0) == b'(' {
                self.i += 1;
                return Some(self.url_or_function(name));
            }
            return Some(Tok::Ident(name));
        }
        self.i += 1;
        Some(match c {
            b'#' => {
                if is_name(self.peek(0)) || self.valid_escape(0) {
                    Tok::Hash(self.name())
                } else {
                    Tok::Delim('#')
                }
            }
            b'@' => {
                if self.starts_ident() {
                    Tok::AtKeyword(self.name())
                } else {
                    Tok::Delim('@')
                }
            }
            b':' => Tok::Colon,
            b';' => Tok::Semicolon,
            b',' => Tok::Comma,
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'<' if self.b[self.i..].starts_with(b"!--") => {
                self.i += 3;
                Tok::Whitespace
            }
            b'-' if self.b[self.i..].starts_with(b"->") => {
                self.i += 2;
                Tok::Whitespace
            }
            _ => {
                if c < 0x80 {
                    Tok::Delim(c as char)
                } else {
                    // unreachable in practice (non-ASCII starts an ident), kept for safety
                    self.i -= 1;
                    let ch = self.s[self.i..].chars().next().unwrap();
                    self.i += ch.len_utf8();
                    Tok::Delim(ch)
                }
            }
        })
    }
}

pub fn tokenize(src: &str) -> Tokens {
    let mut lx = Lexer { s: src, b: src.as_bytes(), i: 0 };
    let mut toks = Vec::with_capacity(src.len() / 4);
    let mut spans = Vec::with_capacity(src.len() / 4);
    loop {
        let start = lx.i;
        match lx.next() {
            Some(t) => {
                toks.push(t);
                spans.push((start as u32, lx.i as u32));
            }
            None => break,
        }
    }
    Tokens { toks, spans }
}

/// Tokenize a value and drop the spans.
pub fn tokenize_value(src: &str) -> Vec<Tok> {
    tokenize(src).toks
}

/// A cursor over a token slice with helpers for nested blocks.
#[derive(Clone)]
pub struct Cur<'a> {
    pub t: &'a [Tok],
    pub i: usize,
}

impl<'a> Cur<'a> {
    pub fn new(t: &'a [Tok]) -> Self {
        Cur { t, i: 0 }
    }
    pub fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    pub fn next(&mut self) -> Option<&'a Tok> {
        let t = self.t.get(self.i);
        if t.is_some() {
            self.i += 1;
        }
        t
    }
    pub fn skip_ws(&mut self) {
        while matches!(self.t.get(self.i), Some(Tok::Whitespace)) {
            self.i += 1;
        }
    }
    pub fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.i >= self.t.len()
    }
    /// After consuming a `Function` or `(` token: return a cursor over its contents and advance
    /// past the matching `)`.
    pub fn block(&mut self) -> Cur<'a> {
        let start = self.i;
        let mut depth = 1;
        while self.i < self.t.len() {
            match &self.t[self.i] {
                Tok::Function(_) | Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        let inner = &self.t[start..self.i.min(self.t.len())];
        if self.i < self.t.len() {
            self.i += 1;
        }
        Cur::new(inner)
    }
    /// Consume a comma (with surrounding whitespace). Returns false if the next token is not one.
    pub fn comma(&mut self) -> bool {
        self.skip_ws();
        if matches!(self.peek(), Some(Tok::Comma)) {
            self.i += 1;
            self.skip_ws();
            true
        } else {
            false
        }
    }
    pub fn ident(&mut self) -> Option<&'a str> {
        self.skip_ws();
        match self.peek() {
            Some(Tok::Ident(s)) => {
                self.i += 1;
                Some(s)
            }
            _ => None,
        }
    }
    pub fn delim(&mut self, c: char) -> bool {
        self.skip_ws();
        if matches!(self.peek(), Some(Tok::Delim(d)) if *d == c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    pub fn rest(&self) -> &'a [Tok] {
        &self.t[self.i.min(self.t.len())..]
    }
}

/// Split a token slice on top-level commas.
pub fn split_commas(t: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, tok) in t.iter().enumerate() {
        match tok {
            Tok::Function(_) | Tok::LParen | Tok::LBracket => depth += 1,
            Tok::RParen | Tok::RBracket => depth -= 1,
            Tok::Comma if depth == 0 => {
                out.push(&t[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&t[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_tokens() {
        let t = tokenize("a.b#c{color:#fff;width:calc(100% - 2.5em)!important;background:url( x.png )}").toks;
        assert_eq!(t[0], Tok::Ident("a".into()));
        assert_eq!(t[1], Tok::Delim('.'));
        assert_eq!(t[3], Tok::Hash("c".into()));
        assert!(t.contains(&Tok::Percentage(100.0)));
        assert!(t.contains(&Tok::Dimension(2.5, "em".into())));
        assert!(t.contains(&Tok::Url("x.png".into())));
        assert!(t.contains(&Tok::Delim('!')));
    }

    #[test]
    fn numbers_and_idents() {
        let t = tokenize("-1.5e2px -webkit-box --x-y +3 .5").toks;
        assert_eq!(t[0], Tok::Dimension(-150.0, "px".into()));
        assert_eq!(t[2], Tok::Ident("-webkit-box".into()));
        assert_eq!(t[4], Tok::Ident("--x-y".into()));
        assert_eq!(t[6], Tok::Number(3.0));
        assert_eq!(t[8], Tok::Number(0.5));
    }
}
