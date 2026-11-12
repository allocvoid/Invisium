//! Lazy tokenizer. The parser drives it and asks for regex / template re-scans where the
//! grammar allows them.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Eof,
    Ident(String),
    Num(f64),
    /// A BigInt literal: its digits (with any `0x` / `0o` / `0b` prefix, without separators).
    BigInt(String),
    Str(Vec<u16>),
    /// Template chunk; `tail` means it ended with a backtick rather than `${`.
    Template { cooked: Option<Vec<u16>>, raw: Vec<u16>, tail: bool },
    Regex { pattern: String, flags: String },
    Punct(&'static str),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
    pub nl_before: bool,
    /// Identifier contained a unicode escape (cannot be a keyword).
    pub escaped: bool,
    /// String literal contained a legacy octal escape.
    pub legacy_octal: bool,
}

#[derive(Clone)]
pub struct Lexer<'a> {
    pub src: &'a [char],
    pub pos: usize,
}

const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%",
    "&", "|", "^", "!", "~", "?", ":", "=", ".", "@", "#",
];

pub fn is_line_term(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{feff}') || (c as u32 > 127 && c.is_whitespace() && !is_line_term(c))
}

pub fn is_id_start(c: char) -> bool {
    if c.is_ascii() {
        return c == '$' || c == '_' || c.is_ascii_alphabetic();
    }
    non_ascii_id_start(c)
}

pub fn is_id_part(c: char) -> bool {
    if c.is_ascii() {
        return c == '$' || c == '_' || c.is_ascii_alphanumeric();
    }
    c == '\u{200c}' || c == '\u{200d}' || non_ascii_id_start(c) || in_continue_only(c)
}

/// ID_Start for a non-ASCII character: Alphabetic minus the combining marks and the circled
/// letters that Other_Alphabetic adds, plus Other_ID_Start, minus Pattern_Syntax.
#[cold]
fn non_ascii_id_start(c: char) -> bool {
    let u = c as u32;
    if matches!(u, 0x1885 | 0x1886 | 0x2118 | 0x212E | 0x309B | 0x309C) {
        return true;
    }
    if !c.is_alphabetic() || u == 0x2E2F {
        return false;
    }
    if matches!(u, 0x24B6..=0x24E9 | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189) {
        return false;
    }
    !in_continue_only(c)
}

fn in_continue_only(c: char) -> bool {
    let u = c as u32;
    let t = crate::ident_table::CONTINUE_ONLY;
    t.binary_search_by(|&(lo, hi)| {
        if hi < u {
            std::cmp::Ordering::Less
        } else if lo > u {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

pub fn push_utf16(out: &mut Vec<u16>, c: char) {
    let mut b = [0u16; 2];
    out.extend_from_slice(c.encode_utf16(&mut b));
}

fn push_cp(out: &mut Vec<u16>, cp: u32) {
    if let Some(c) = char::from_u32(cp) {
        push_utf16(out, c);
    } else if cp <= 0xFFFF {
        out.push(cp as u16); // lone surrogate
    }
}

pub type LexResult<T> = Result<T, (String, usize)>;

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [char]) -> Self {
        Lexer { src, pos: 0 }
    }

    fn peek(&self, off: usize) -> char {
        self.src.get(self.pos + off).copied().unwrap_or('\0')
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Skips whitespace and comments; returns whether a line terminator was crossed.
    fn skip_trivia(&mut self) -> LexResult<bool> {
        let mut nl = false;
        while !self.at_end() {
            let c = self.peek(0);
            if is_line_term(c) {
                nl = true;
                self.pos += 1;
            } else if is_ws(c) {
                self.pos += 1;
            } else if c == '/' && self.peek(1) == '/' {
                while !self.at_end() && !is_line_term(self.peek(0)) {
                    self.pos += 1;
                }
            } else if c == '/' && self.peek(1) == '*' {
                let start = self.pos;
                self.pos += 2;
                loop {
                    if self.at_end() {
                        return Err(("unterminated comment".into(), start));
                    }
                    if self.peek(0) == '*' && self.peek(1) == '/' {
                        self.pos += 2;
                        break;
                    }
                    if is_line_term(self.peek(0)) {
                        nl = true;
                    }
                    self.pos += 1;
                }
            } else if c == '<' && self.peek(1) == '!' && self.peek(2) == '-' && self.peek(3) == '-' {
                while !self.at_end() && !is_line_term(self.peek(0)) {
                    self.pos += 1;
                }
            } else if c == '-' && self.peek(1) == '-' && self.peek(2) == '>' && (nl || self.pos == 0) {
                while !self.at_end() && !is_line_term(self.peek(0)) {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        Ok(nl)
    }

    pub fn next_token(&mut self) -> LexResult<Token> {
        let nl = self.skip_trivia()?;
        let start = self.pos;
        let mut t = Token { tok: Tok::Eof, start, end: start, nl_before: nl, escaped: false, legacy_octal: false };
        if self.at_end() {
            return Ok(t);
        }
        let c = self.peek(0);
        t.tok = if is_id_start(c) || c == '\\' {
            let (name, esc) = self.ident()?;
            t.escaped = esc;
            Tok::Ident(name)
        } else if c.is_ascii_digit() || (c == '.' && self.peek(1).is_ascii_digit()) {
            // a legacy octal literal (`010`) or a decimal with a leading zero (`08`)
            t.legacy_octal = c == '0' && self.peek(1).is_ascii_digit();
            self.number()?
        } else if c == '"' || c == '\'' {
            let (s, oct) = self.string(c)?;
            t.legacy_octal = oct;
            Tok::Str(s)
        } else if c == '`' {
            self.pos += 1;
            self.template_chunk()?
        } else {
            let mut found = None;
            for p in PUNCTS {
                if p.chars().enumerate().all(|(i, pc)| self.peek(i) == pc) {
                    if *p == "?." && self.peek(2).is_ascii_digit() {
                        continue;
                    }
                    found = Some(*p);
                    break;
                }
            }
            match found {
                Some(p) => {
                    self.pos += p.len();
                    Tok::Punct(p)
                }
                None => return Err((format!("unexpected character '{}'", c), start)),
            }
        };
        t.end = self.pos;
        Ok(t)
    }

    fn ident(&mut self) -> LexResult<(String, bool)> {
        let mut s = String::new();
        let mut esc = false;
        let mut first = true;
        loop {
            let c = self.peek(0);
            if c == '\\' {
                if self.peek(1) != 'u' {
                    return Err(("invalid escape in identifier".into(), self.pos));
                }
                self.pos += 2;
                let cp = self.unicode_escape_body()?;
                let ch = char::from_u32(cp).ok_or(("invalid identifier escape".to_string(), self.pos))?;
                if !(if first { is_id_start(ch) } else { is_id_part(ch) }) {
                    return Err(("invalid identifier escape".into(), self.pos));
                }
                s.push(ch);
                esc = true;
            } else if !self.at_end() && (if first { is_id_start(c) } else { is_id_part(c) }) {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
            first = false;
        }
        Ok((s, esc))
    }

    /// After `\u`: parses `XXXX` or `{X...}`.
    fn unicode_escape_body(&mut self) -> LexResult<u32> {
        let err = || ("invalid unicode escape".to_string(), 0usize);
        if self.peek(0) == '{' {
            self.pos += 1;
            let mut v: u32 = 0;
            let mut n = 0;
            while self.peek(0) != '}' {
                let d = self.peek(0).to_digit(16).ok_or_else(|| (err().0, self.pos))?;
                v = v.saturating_mul(16).saturating_add(d);
                n += 1;
                self.pos += 1;
            }
            self.pos += 1;
            if n == 0 || v > 0x10FFFF {
                return Err((err().0, self.pos));
            }
            Ok(v)
        } else {
            let mut v = 0;
            for _ in 0..4 {
                let d = self.peek(0).to_digit(16).ok_or_else(|| (err().0, self.pos))?;
                v = v * 16 + d;
                self.pos += 1;
            }
            Ok(v)
        }
    }

    fn digits(&mut self, radix: u32, out: &mut String) {
        while !self.at_end() {
            let c = self.peek(0);
            if c == '_' && self.peek(1).is_digit(radix) && !out.is_empty() {
                self.pos += 1;
            } else if c.is_digit(radix) {
                out.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn number(&mut self) -> LexResult<Tok> {
        let start = self.pos;
        let c0 = self.peek(0);
        let c1 = self.peek(1).to_ascii_lowercase();
        if c0 == '0' && self.peek(1) == '_' {
            return Err(("numeric separator can not be used after leading 0".into(), start));
        }
        // `010` is legacy octal; `08` / `09.5` (a digit above 7) are decimal despite the zero
        let mut end = self.pos;
        while self.src.get(end).is_some_and(|c| c.is_ascii_digit()) {
            end += 1;
        }
        let legacy_octal = c0 == '0' && end > self.pos + 1 && self.src[self.pos..end].iter().all(|c| *c < '8');
        let v = if c0 == '0' && (c1 == 'x' || c1 == 'o' || c1 == 'b') {
            self.pos += 2;
            let radix = match c1 { 'x' => 16, 'o' => 8, _ => 2 };
            let mut s = String::new();
            self.digits(radix, &mut s);
            if s.is_empty() {
                return Err(("invalid number".into(), start));
            }
            s.chars().fold(0.0, |a, d| a * radix as f64 + d.to_digit(radix).unwrap_or(0) as f64)
        } else if legacy_octal {
            let digits: String = self.src[self.pos..end].iter().collect();
            self.pos = end;
            digits.chars().fold(0.0, |a, d| a * 8.0 + d.to_digit(8).unwrap_or(0) as f64)
        } else {
            let mut s = String::new();
            self.digits(10, &mut s);
            if self.peek(0) == '.' {
                s.push('.');
                self.pos += 1;
                let mut frac = String::new();
                self.digits(10, &mut frac);
                s.push_str(&frac);
            }
            if matches!(self.peek(0), 'e' | 'E') {
                let save = self.pos;
                let mut e = String::from("e");
                self.pos += 1;
                if matches!(self.peek(0), '+' | '-') {
                    e.push(self.peek(0));
                    self.pos += 1;
                }
                let mut ed = String::new();
                self.digits(10, &mut ed);
                if ed.is_empty() {
                    self.pos = save;
                } else {
                    e.push_str(&ed);
                    s.push_str(&e);
                }
            }
            if s.starts_with('.') {
                s.insert(0, '0');
            }
            s.parse::<f64>().map_err(|_| ("invalid number".to_string(), start))?
        };
        let mut tok = Tok::Num(v);
        if self.peek(0) == 'n' {
            // a BigInt literal is an integer: no fraction, exponent or leading zero
            let text: String = self.src[start..self.pos].iter().filter(|c| **c != '_').collect();
            let prefixed = c0 == '0' && (c1 == 'x' || c1 == 'o' || c1 == 'b');
            let integral = prefixed || (!text.contains(['.', 'e', 'E']) && (text.len() == 1 || !text.starts_with('0')));
            if !integral {
                return Err(("invalid BigInt literal".into(), start));
            }
            self.pos += 1;
            tok = Tok::BigInt(text);
        }
        if is_id_start(self.peek(0)) || self.peek(0).is_ascii_digit() {
            return Err(("identifier starts immediately after numeric literal".into(), self.pos));
        }
        Ok(tok)
    }

    /// Parses an escape after the backslash. Returns Ok(None) for an invalid escape (templates).
    fn escape(&mut self, out: &mut Vec<u16>, in_template: bool, octal: &mut bool) -> LexResult<bool> {
        let c = self.peek(0);
        self.pos += 1;
        match c {
            'n' => out.push(10),
            't' => out.push(9),
            'r' => out.push(13),
            'b' => out.push(8),
            'f' => out.push(12),
            'v' => out.push(11),
            '\r' => {
                if self.peek(0) == '\n' {
                    self.pos += 1;
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            'x' => {
                let (a, b) = (self.peek(0).to_digit(16), self.peek(1).to_digit(16));
                match (a, b) {
                    (Some(a), Some(b)) => {
                        self.pos += 2;
                        out.push((a * 16 + b) as u16);
                    }
                    _ => return Ok(false),
                }
            }
            'u' => {
                let save = self.pos;
                match self.unicode_escape_body() {
                    Ok(cp) => push_cp(out, cp),
                    Err(e) => {
                        if in_template {
                            self.pos = save;
                            return Ok(false);
                        }
                        return Err(e);
                    }
                }
            }
            '0'..='7' => {
                if c == '0' && !self.peek(0).is_ascii_digit() {
                    out.push(0);
                } else {
                    if in_template {
                        return Ok(false);
                    }
                    *octal = true;
                    let mut v = c.to_digit(8).unwrap_or(0);
                    let max_len = if c <= '3' { 2 } else { 1 };
                    for _ in 0..max_len {
                        match self.peek(0).to_digit(8) {
                            Some(d) => {
                                v = v * 8 + d;
                                self.pos += 1;
                            }
                            None => break,
                        }
                    }
                    out.push(v as u16);
                }
            }
            '8' | '9' => {
                if in_template {
                    return Ok(false);
                }
                *octal = true;
                out.push(c as u16);
            }
            '\0' if self.pos > self.src.len() => return Err(("unterminated string".into(), self.pos)),
            _ => push_utf16(out, c),
        }
        Ok(true)
    }

    fn string(&mut self, q: char) -> LexResult<(Vec<u16>, bool)> {
        let start = self.pos;
        self.pos += 1;
        let mut out = Vec::new();
        let mut octal = false;
        loop {
            if self.at_end() {
                return Err(("unterminated string literal".into(), start));
            }
            let c = self.peek(0);
            if c == q {
                self.pos += 1;
                break;
            }
            if c == '\\' {
                self.pos += 1;
                if self.at_end() {
                    return Err(("unterminated string literal".into(), start));
                }
                if !self.escape(&mut out, false, &mut octal)? {
                    return Err(("invalid escape sequence".into(), self.pos));
                }
            } else if c == '\n' || c == '\r' {
                return Err(("unterminated string literal".into(), start));
            } else {
                push_utf16(&mut out, c);
                self.pos += 1;
            }
        }
        Ok((out, octal))
    }

    /// Scans a template chunk; position must be just after '`' or '}'.
    fn template_chunk(&mut self) -> LexResult<Tok> {
        let start = self.pos;
        let mut cooked = Some(Vec::new());
        let mut raw = Vec::new();
        loop {
            if self.at_end() {
                return Err(("unterminated template literal".into(), start));
            }
            let c = self.peek(0);
            if c == '`' {
                self.pos += 1;
                return Ok(Tok::Template { cooked, raw, tail: true });
            }
            if c == '$' && self.peek(1) == '{' {
                self.pos += 2;
                return Ok(Tok::Template { cooked, raw, tail: false });
            }
            if c == '\\' {
                let esc_start = self.pos;
                self.pos += 1;
                let mut buf = Vec::new();
                let mut oct = false;
                let ok = self.escape(&mut buf, true, &mut oct)?;
                if ok {
                    if let Some(cv) = cooked.as_mut() {
                        cv.extend_from_slice(&buf);
                    }
                } else {
                    cooked = None;
                }
                for &ch in &self.src[esc_start..self.pos] {
                    if ch == '\r' {
                        raw.push(10);
                    } else {
                        push_utf16(&mut raw, ch);
                    }
                }
                if self.src.get(esc_start + 1) == Some(&'\r') && self.src.get(esc_start + 2) == Some(&'\n') {
                    raw.pop(); // \r\n -> \n already pushed for \r; drop the extra \n
                }
                continue;
            }
            self.pos += 1;
            let ch = if c == '\r' {
                if self.peek(0) == '\n' {
                    self.pos += 1;
                }
                '\n'
            } else {
                c
            };
            if let Some(cv) = cooked.as_mut() {
                push_utf16(cv, ch);
            }
            push_utf16(&mut raw, ch);
        }
    }

    /// Re-scan a `}` token at `start` as a template continuation.
    pub fn rescan_template(&mut self, start: usize) -> LexResult<Token> {
        self.pos = start + 1;
        let tok = self.template_chunk()?;
        Ok(Token { tok, start, end: self.pos, nl_before: false, escaped: false, legacy_octal: false })
    }

    /// Re-scan a `/` or `/=` token at `start` as a regular-expression literal.
    pub fn rescan_regex(&mut self, start: usize, nl_before: bool) -> LexResult<Token> {
        self.pos = start + 1;
        let mut pattern = String::new();
        let mut in_class = false;
        loop {
            if self.at_end() || is_line_term(self.peek(0)) {
                return Err(("unterminated regular expression".into(), start));
            }
            let c = self.peek(0);
            self.pos += 1;
            if c == '\\' {
                pattern.push(c);
                let n = self.peek(0);
                if self.at_end() || is_line_term(n) {
                    return Err(("unterminated regular expression".into(), start));
                }
                pattern.push(n);
                self.pos += 1;
                continue;
            }
            if c == '[' {
                in_class = true;
            } else if c == ']' {
                in_class = false;
            } else if c == '/' && !in_class {
                break;
            }
            pattern.push(c);
        }
        let mut flags = String::new();
        while !self.at_end() && is_id_part(self.peek(0)) {
            flags.push(self.peek(0));
            self.pos += 1;
        }
        Ok(Token { tok: Tok::Regex { pattern, flags }, start, end: self.pos, nl_before, escaped: false, legacy_octal: false })
    }
}

/// Maps char offsets to 1-based (line, column).
pub struct LineMap {
    starts: Vec<usize>,
}

impl LineMap {
    pub fn new(src: &[char]) -> Self {
        let mut starts = vec![0];
        let mut i = 0;
        while i < src.len() {
            let c = src[i];
            if c == '\r' && src.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            if is_line_term(c) {
                starts.push(i + 1);
            }
            i += 1;
        }
        LineMap { starts }
    }

    pub fn pos(&self, off: usize) -> (u32, u32) {
        let line = match self.starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        (line as u32 + 1, (off - self.starts[line]) as u32 + 1)
    }
}
