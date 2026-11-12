//! The escape and character-class half of the parser.

use crate::ast::{Node, NodeId};
use crate::charset::{CharSet, SetBuilder};
use crate::parser::Parser;
use crate::unicode::{self, combine_surrogates, is_high_surrogate, is_low_surrogate};
use crate::{props, SyntaxError};
use std::collections::HashSet;

/// What an escape (or a class atom) stands for.
pub(crate) enum Esc {
    Char(u32),
    Set(CharSet),
}

/// Add a class atom to the class being built. A class escape (`\d`, `\p{…}`) whose source
/// text was already seen is skipped: within one pattern the text identifies the set.
fn add_item(set: &mut SetBuilder, seen: &mut HashSet<Vec<u16>>, item: &Esc, source: &[u16]) {
    match item {
        Esc::Char(c) => set.add(*c),
        Esc::Set(s) => {
            if seen.insert(source.to_vec()) {
                set.add_set(s);
            }
        }
    }
}

fn is_ascii(c: u16, ch: char) -> bool {
    c == ch as u16
}

fn ascii_char(c: u16) -> char {
    if c < 128 {
        c as u8 as char
    } else {
        '\u{FFFD}'
    }
}

impl Parser<'_> {
    /// Read one pattern character: a code unit, or a whole code point when `combine` is set.
    fn read_char(&mut self, combine: bool) -> Option<u32> {
        let c = self.peek()? as u32;
        self.pos += 1;
        if combine && is_high_surrogate(c) {
            if let Some(lo) = self.peek().map(u32::from).filter(|&lo| is_low_surrogate(lo)) {
                self.pos += 1;
                return Some(combine_surrogates(c, lo));
            }
        }
        Some(c)
    }

    pub(crate) fn next_char(&mut self) -> Option<u32> {
        self.read_char(self.unicode)
    }

    /// Exactly `count` hex digits starting at `from`.
    fn hex(&self, from: usize, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for i in 0..count {
            let unit = *self.pat.get(from.checked_add(i)?)?;
            value = value * 16 + ascii_char(unit).to_digit(16)?;
        }
        Some(value)
    }

    /// With `pos` at the `u` of `\u`: the escaped value, advancing past it. `full` allows
    /// `\u{…}` and joins an escaped surrogate pair. Leaves `pos` alone on failure.
    fn unicode_escape(&mut self, full: bool) -> Option<u32> {
        if full && self.peek_at(1) == Some(b'{' as u16) {
            let (mut i, mut value, mut digits) = (self.pos + 2, 0u32, 0usize);
            while let Some(d) = self.pat.get(i).and_then(|&c| ascii_char(c).to_digit(16)) {
                value = value * 16 + d;
                if value > 0x10FFFF {
                    return None;
                }
                i += 1;
                digits += 1;
            }
            if digits == 0 || self.pat.get(i) != Some(&(b'}' as u16)) {
                return None;
            }
            self.pos = i + 1;
            return Some(value);
        }
        let value = self.hex(self.pos + 1, 4)?;
        self.pos += 5;
        if full && is_high_surrogate(value) && self.peek_is('\\') && self.peek_at(1) == Some(b'u' as u16) {
            if let Some(lo) = self.hex(self.pos + 2, 4).filter(|&lo| is_low_surrogate(lo)) {
                self.pos += 6;
                return Some(combine_surrogates(value, lo));
            }
        }
        Some(value)
    }

    /// Called just after the `<` of `(?<name>` or `\k<name>`; consumes through `>`.
    pub(crate) fn parse_group_name(&mut self) -> Result<String, SyntaxError> {
        let start = self.pos;
        let mut name = String::new();
        while let Some(mut c) = self.read_char(true) {
            if c == '>' as u32 && !name.is_empty() {
                return Ok(name);
            }
            if c == '\\' as u32 {
                match self.peek_is('u').then(|| self.unicode_escape(true)).flatten() {
                    Some(v) => c = v,
                    None => break,
                }
            }
            let Some(ch) = char::from_u32(c) else { break };
            let start_ok = ch == '$' || ch == '_' || ch.is_alphabetic();
            let part_ok = ch.is_alphanumeric() || ch == '\u{200C}' || ch == '\u{200D}';
            if !(start_ok || (part_ok && !name.is_empty())) {
                break;
            }
            name.push(ch);
        }
        self.err("invalid capture group name", start)
    }

    /// An escape in atom position; `pos` is at the backslash.
    pub(crate) fn parse_atom_escape(&mut self) -> Result<(NodeId, bool), SyntaxError> {
        let start = self.pos;
        self.pos += 1;
        let Some(c) = self.peek() else {
            return self.err("\\ at end of pattern", start);
        };
        match ascii_char(c) {
            'b' => {
                self.pos += 1;
                return Ok((self.add(Node::WordBoundary, true), false));
            }
            'B' => {
                self.pos += 1;
                return Ok((self.add(Node::NotWordBoundary, true), false));
            }
            '1'..='9' => {
                if let Some((n, end)) = self.decimal(self.pos) {
                    if n <= self.total_groups {
                        self.pos = end;
                        return Ok((self.add(Node::BackRef(n), true), true));
                    }
                }
                if self.unicode {
                    return self.err("invalid backreference", start);
                }
                // Annex B: no such group, so this is a legacy octal or identity escape.
            }
            'k' if self.named => {
                self.pos += 1;
                if !self.eat('<') {
                    return self.err("invalid named reference", start);
                }
                let name = self.parse_group_name()?;
                let node = self.add(Node::BackRef(0), true);
                self.named_refs.push((node, name, start));
                return Ok((node, true));
            }
            _ => {}
        }
        let node = match self.parse_escape(false)? {
            Esc::Char(c) => self.add(Node::Char(c), false),
            Esc::Set(mut set) => {
                set.normalize();
                self.add_class(set, false)
            }
        };
        Ok((node, true))
    }

    /// The escapes shared by atoms and classes; `pos` is just after the backslash.
    fn parse_escape(&mut self, in_class: bool) -> Result<Esc, SyntaxError> {
        let start = self.pos.saturating_sub(1);
        let Some(unit) = self.peek() else {
            return self.err("\\ at end of pattern", start);
        };
        let c = ascii_char(unit);
        let control = match c {
            't' => Some(0x09),
            'n' => Some(0x0A),
            'v' => Some(0x0B),
            'f' => Some(0x0C),
            'r' => Some(0x0D),
            'b' if in_class => Some(0x08),
            '-' if in_class && self.unicode => Some('-' as u32),
            _ => None,
        };
        if let Some(value) = control {
            self.pos += 1;
            return Ok(Esc::Char(value));
        }
        let class = match c {
            'd' | 'D' => Some(unicode::digit_set()),
            's' | 'S' => Some(unicode::space_set()),
            'w' | 'W' => Some(unicode::word_set(self.unicode && self.icase)),
            _ => None,
        };
        if let Some(set) = class {
            self.pos += 1;
            return Ok(Esc::Set(if c.is_ascii_uppercase() { set.negated() } else { set }));
        }
        let next = self.peek_at(1).map_or('\0', ascii_char);
        match c {
            'p' | 'P' if self.unicode => self.parse_property(c == 'P', start),
            '0' if !next.is_ascii_digit() => {
                self.pos += 1;
                Ok(Esc::Char(0))
            }
            '0'..='9' if self.unicode => self.err("invalid decimal escape", start),
            '0'..='7' => Ok(Esc::Char(self.legacy_octal())),
            'c' if next.is_ascii_alphabetic() || (in_class && !self.unicode && (next.is_ascii_digit() || next == '_')) => {
                self.pos += 2;
                Ok(Esc::Char(next as u32 % 32))
            }
            'c' if self.unicode => self.err("invalid control escape", start),
            // Annex B: `\c` before anything else is a literal backslash; `c` is read next.
            'c' => Ok(Esc::Char('\\' as u32)),
            'x' => match self.hex(self.pos + 1, 2) {
                Some(value) => {
                    self.pos += 3;
                    Ok(Esc::Char(value))
                }
                None if self.unicode => self.err("invalid hexadecimal escape", start),
                None => {
                    self.pos += 1;
                    Ok(Esc::Char('x' as u32))
                }
            },
            'u' => match self.unicode_escape(self.unicode) {
                Some(value) => Ok(Esc::Char(value)),
                None if self.unicode => self.err("invalid unicode escape", start),
                None => {
                    self.pos += 1;
                    Ok(Esc::Char('u' as u32))
                }
            },
            'k' if self.named => self.err("invalid named reference", start),
            _ if self.unicode && !"^$\\.*+?()[]{}|/".contains(c) => self.err("invalid escape", start),
            _ => Ok(Esc::Char(self.read_char(false).unwrap_or(0))),
        }
    }

    /// Annex B legacy octal escape; `pos` is at its first digit (0–7).
    fn legacy_octal(&mut self) -> u32 {
        let digit = |p: &Self| p.peek().and_then(|c| ascii_char(c).to_digit(8));
        let first = digit(self).unwrap_or(0);
        self.pos += 1;
        let mut value = first;
        let max_len = if first <= 3 { 3 } else { 2 };
        for _ in 1..max_len {
            let Some(d) = digit(self) else { break };
            value = value * 8 + d;
            self.pos += 1;
        }
        value
    }

    /// `\p{…}` / `\P{…}`; `pos` is at the `p`.
    fn parse_property(&mut self, negate: bool, start: usize) -> Result<Esc, SyntaxError> {
        self.pos += 1;
        if !self.eat('{') {
            return self.err("invalid property name", start);
        }
        let mut spec = String::new();
        loop {
            let Some(unit) = self.peek() else {
                return self.err("invalid property name", start);
            };
            self.pos += 1;
            let ch = ascii_char(unit);
            if ch == '}' {
                break;
            }
            if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '=') {
                return self.err("invalid property name", start);
            }
            spec.push(ch);
        }
        match props::lookup(&spec) {
            Some(set) => Ok(Esc::Set(if negate { set.negated() } else { set })),
            None => self.err("invalid property name", start),
        }
    }

    /// A character class; `pos` is at the `[`.
    pub(crate) fn parse_class(&mut self) -> Result<NodeId, SyntaxError> {
        let start = self.pos;
        self.pos += 1;
        let negate = self.eat('^');
        let pat = self.pat;
        let mut set = SetBuilder::new();
        let mut seen = HashSet::new();
        loop {
            let Some(c) = self.peek() else {
                return self.err("unterminated character class", start);
            };
            if is_ascii(c, ']') {
                self.pos += 1;
                break;
            }
            let item_start = self.pos;
            let lhs = self.class_atom()?;
            let lhs_text = pat.get(item_start..self.pos).unwrap_or(&[]);
            let dash = self.peek_is('-') && self.peek_at(1).is_some_and(|c| !is_ascii(c, ']'));
            if !dash {
                add_item(&mut set, &mut seen, &lhs, lhs_text);
                continue;
            }
            self.pos += 1;
            if self.vmode && self.peek_is('-') {
                return self.err("unsupported class set syntax", item_start);
            }
            let rhs_start = self.pos;
            let rhs = self.class_atom()?;
            let rhs_text = pat.get(rhs_start..self.pos).unwrap_or(&[]);
            match (&lhs, &rhs) {
                (Esc::Char(a), Esc::Char(b)) if a <= b => set.add_range(*a, *b),
                (Esc::Char(_), Esc::Char(_)) => {
                    return self.err("range out of order in character class", item_start);
                }
                _ if self.unicode => return self.err("invalid character class", item_start),
                _ => {
                    // Annex B: a class escape next to `-` makes the dash a literal.
                    add_item(&mut set, &mut seen, &lhs, lhs_text);
                    set.add('-' as u32);
                    add_item(&mut set, &mut seen, &rhs, rhs_text);
                }
            }
        }
        Ok(self.add_class(set.finish(), negate))
    }

    fn class_atom(&mut self) -> Result<Esc, SyntaxError> {
        let start = self.pos;
        if self.eat('\\') {
            return self.parse_escape(true);
        }
        if self.vmode {
            // Set operations and nested classes are not supported; refuse rather than misread.
            let c = self.peek().map_or('\0', ascii_char);
            let doubled = "&!#$%*+,.:;<=>?@^`~".contains(c) && self.peek_at(1) == self.peek();
            if "()[{}/|".contains(c) || doubled {
                return self.err("unsupported class set syntax", start);
            }
        }
        Ok(Esc::Char(self.next_char().unwrap_or(0)))
    }
}
