//! Tokenizer: source text to preprocessing tokens with (physical) line numbers.

use std::fmt;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    Id(Rc<str>),
    /// Value and whether it had a `u` suffix.
    Int(u64, bool),
    Float(f64),
    P(&'static str),
    /// Something that is not valid GLSL; an error if it survives preprocessing.
    Bad(&'static str),
}

impl fmt::Display for T {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            T::Id(s) => write!(f, "{s}"),
            T::Int(v, u) => write!(f, "{v}{}", if *u { "u" } else { "" }),
            T::Float(v) => write!(f, "{v:?}"),
            T::P(p) => write!(f, "{p}"),
            T::Bad(_) => write!(f, "?"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub t: T,
    pub line: u32,
    /// First token on its line.
    pub bol: bool,
    /// Whitespace or a comment right before it.
    pub sp: bool,
    /// Macros that must not expand this token again (preprocessor hide set).
    pub hs: Option<Rc<Vec<u32>>>,
}

impl Tok {
    pub fn is(&self, p: &str) -> bool {
        matches!(&self.t, T::P(q) if *q == p)
    }
    pub fn id(&self) -> Option<&str> {
        match &self.t {
            T::Id(s) => Some(s),
            _ => None,
        }
    }
}

const PUNCT: [&str; 47] = [
    "<<=", ">>=", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "^^", "+=", "-=", "*=", "/=", "%=",
    "&=", "^=", "|=", "##", "(", ")", "[", "]", "{", "}", ".", ",", "+", "-", "!", "~", "*", "/", "%", "<", ">",
    "&", "^", "|", "?", ":", "=", ";", "#",
];

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Split `src` into tokens. Line continuations are removed and comments become whitespace.
pub fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut cs: Vec<u8> = Vec::with_capacity(b.len());
    let mut ls: Vec<u32> = Vec::with_capacity(b.len());
    let mut line = 1u32;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            let mut j = i + 1;
            if j < b.len() && b[j] == b'\r' {
                j += 1;
            }
            if j < b.len() && b[j] == b'\n' {
                j += 1;
            }
            if j > i + 1 {
                line = line.saturating_add(1);
                i = j;
                continue;
            }
        }
        if c == b'\r' {
            cs.push(b'\n');
            ls.push(line);
            line = line.saturating_add(1);
            i += 1;
            if i < b.len() && b[i] == b'\n' {
                i += 1;
            }
            continue;
        }
        cs.push(c);
        ls.push(line);
        if c == b'\n' {
            line = line.saturating_add(1);
        }
        i += 1;
    }

    let n = cs.len();
    let at = |k: usize| if k < n { cs[k] } else { 0 };
    let mut toks = Vec::new();
    let mut bol = true;
    let mut sp = false;
    let mut i = 0;
    while i < n {
        let c = cs[i];
        let ln = ls[i];
        let start = i;
        let t = match c {
            b'\n' => {
                bol = true;
                sp = true;
                i += 1;
                continue;
            }
            b' ' | b'\t' | 0x0b | 0x0c => {
                sp = true;
                i += 1;
                continue;
            }
            b'/' if at(i + 1) == b'/' => {
                while i < n && cs[i] != b'\n' {
                    i += 1;
                }
                sp = true;
                continue;
            }
            b'/' if at(i + 1) == b'*' => {
                i += 2;
                loop {
                    if i + 1 >= n {
                        i = n;
                        toks.push(Tok { t: T::Bad("unterminated comment"), line: ln, bol, sp: true, hs: None });
                        break;
                    }
                    if cs[i] == b'*' && cs[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                sp = true;
                continue;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < n && ident_char(cs[i]) {
                    i += 1;
                }
                // Only ASCII bytes were consumed, so this is valid UTF-8.
                let s = std::str::from_utf8(&cs[start..i]).unwrap_or("_");
                T::Id(Rc::from(s))
            }
            c if c.is_ascii_digit() || (c == b'.' && at(i + 1).is_ascii_digit()) => {
                let (t, end) = number(&cs, i);
                i = end;
                t
            }
            _ => {
                let mut found = None;
                for p in PUNCT {
                    let pb = p.as_bytes();
                    if i + pb.len() <= n && &cs[i..i + pb.len()] == pb {
                        found = Some(p);
                        break;
                    }
                }
                match found {
                    Some(p) => {
                        i += p.len();
                        T::P(p)
                    }
                    None => {
                        i += 1;
                        T::Bad("invalid character")
                    }
                }
            }
        };
        toks.push(Tok { t, line: ln, bol, sp, hs: None });
        bol = false;
        sp = false;
    }
    toks
}

fn number(cs: &[u8], mut i: usize) -> (T, usize) {
    let n = cs.len();
    let at = |k: usize| if k < n { cs[k] } else { 0 };
    let start = i;
    let t;
    if cs[i] == b'0' && (at(i + 1) == b'x' || at(i + 1) == b'X') {
        i += 2;
        let ds = i;
        let mut v: u64 = 0;
        let mut over = false;
        while at(i).is_ascii_hexdigit() {
            let d = (at(i) as char).to_digit(16).unwrap_or(0) as u64;
            v = v.saturating_mul(16).saturating_add(d);
            over |= v > u32::MAX as u64;
            i += 1;
        }
        let u = matches!(at(i), b'u' | b'U');
        if u {
            i += 1;
        }
        t = if i == ds || (u && i == ds + 1) {
            T::Bad("invalid hexadecimal constant")
        } else if over {
            T::Bad("integer constant overflow")
        } else {
            T::Int(v, u)
        };
    } else {
        while at(i).is_ascii_digit() {
            i += 1;
        }
        let mut float = false;
        if at(i) == b'.' {
            float = true;
            i += 1;
            while at(i).is_ascii_digit() {
                i += 1;
            }
        }
        let mut bad_exp = false;
        if matches!(at(i), b'e' | b'E') {
            float = true;
            i += 1;
            if matches!(at(i), b'+' | b'-') {
                i += 1;
            }
            bad_exp = !at(i).is_ascii_digit();
            while at(i).is_ascii_digit() {
                i += 1;
            }
        }
        let text = std::str::from_utf8(&cs[start..i]).unwrap_or("0");
        if float {
            if matches!(at(i), b'f' | b'F') {
                i += 1;
            }
            t = match text.parse::<f64>() {
                Ok(v) if !bad_exp => T::Float(v),
                _ => T::Bad("invalid floating-point constant"),
            };
        } else {
            let u = matches!(at(i), b'u' | b'U');
            if u {
                i += 1;
            }
            let (digits, radix) = if text.len() > 1 && text.starts_with('0') { (&text[1..], 8) } else { (text, 10) };
            t = match u64::from_str_radix(digits, radix) {
                Ok(v) if v <= u32::MAX as u64 => T::Int(v, u),
                Ok(_) => T::Bad("integer constant overflow"),
                Err(_) => T::Bad("invalid integer constant"),
            };
        }
    }
    if ident_char(at(i)) || at(i) == b'.' {
        while ident_char(at(i)) || at(i) == b'.' {
            i += 1;
        }
        return (T::Bad("invalid number"), i);
    }
    (t, i)
}
