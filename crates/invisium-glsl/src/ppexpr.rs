//! Integer expressions of `#if` / `#elif`.

use crate::lex::{Tok, T};
use crate::{err, Error};

const MAX_DEPTH: u32 = 64;

struct E<'a> {
    t: &'a [Tok],
    i: usize,
    line: u32,
    depth: u32,
}

pub fn eval(t: &[Tok], line: u32) -> Result<i64, Error> {
    if t.is_empty() {
        return err(line, "'#if' : expected an expression");
    }
    let mut e = E { t, i: 0, line, depth: 0 };
    let v = e.binary(0)?;
    if e.i != t.len() {
        return err(line, format!("'#if' : unexpected '{}'", t[e.i].t));
    }
    Ok(v)
}

fn prec(p: &str) -> Option<u8> {
    Some(match p {
        "||" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        _ => return None,
    })
}

impl E<'_> {
    fn binary(&mut self, min: u8) -> Result<i64, Error> {
        let mut l = self.unary()?;
        loop {
            let Some(T::P(op)) = self.t.get(self.i).map(|t| &t.t) else { break };
            let Some(p) = prec(op) else { break };
            if p < min {
                break;
            }
            self.i += 1;
            let r = self.binary(p + 1)?;
            l = match *op {
                "||" => (l != 0 || r != 0) as i64,
                "&&" => (l != 0 && r != 0) as i64,
                "|" => l | r,
                "^" => l ^ r,
                "&" => l & r,
                "==" => (l == r) as i64,
                "!=" => (l != r) as i64,
                "<" => (l < r) as i64,
                ">" => (l > r) as i64,
                "<=" => (l <= r) as i64,
                ">=" => (l >= r) as i64,
                "<<" => l.wrapping_shl((r & 63) as u32),
                ">>" => l.wrapping_shr((r & 63) as u32),
                "+" => l.wrapping_add(r),
                "-" => l.wrapping_sub(r),
                "*" => l.wrapping_mul(r),
                "/" | "%" => {
                    if r == 0 {
                        return err(self.line, "'#if' : division by zero");
                    }
                    if *op == "/" {
                        l.wrapping_div(r)
                    } else {
                        l.wrapping_rem(r)
                    }
                }
                _ => break,
            };
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<i64, Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err(self.line, "'#if' : expression nested too deeply");
        }
        let Some(t) = self.t.get(self.i) else { return err(self.line, "'#if' : unexpected end of expression") };
        self.i += 1;
        let v = match &t.t {
            T::Int(v, _) => *v as i64,
            T::P("+") => self.unary()?,
            T::P("-") => self.unary()?.wrapping_neg(),
            T::P("~") => !self.unary()?,
            T::P("!") => (self.unary()? == 0) as i64,
            T::P("(") => {
                let v = self.binary(0)?;
                if !self.t.get(self.i).is_some_and(|t| t.is(")")) {
                    return err(self.line, "'#if' : expected ')'");
                }
                self.i += 1;
                v
            }
            T::Id(s) => return err(self.line, format!("'#if' : undefined identifier '{s}'")),
            other => return err(self.line, format!("'#if' : unexpected '{other}'")),
        };
        self.depth -= 1;
        Ok(v)
    }
}
