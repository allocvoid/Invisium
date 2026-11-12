//! Single-line text editing model (character indices, no layout).

const MAX_CHARS: usize = 2048;

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub text: String,
    /// Caret position as a character index.
    pub caret: usize,
    /// Selection anchor; equal to `caret` when nothing is selected.
    pub anchor: usize,
    /// Horizontal scroll in px.
    pub scroll: f32,
}

impl TextField {
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn byte(&self, ci: usize) -> usize {
        self.text.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(self.text.len())
    }

    pub fn set_text(&mut self, s: &str) {
        self.text = s.chars().filter(|c| !c.is_control()).take(MAX_CHARS).collect();
        self.caret = self.len();
        self.anchor = self.caret;
        self.scroll = 0.0;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        if self.caret == self.anchor {
            None
        } else {
            Some((self.caret.min(self.anchor), self.caret.max(self.anchor)))
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        Some(self.text[self.byte(a)..self.byte(b)].to_string())
    }

    pub fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                let (ba, bb) = (self.byte(a), self.byte(b));
                self.text.replace_range(ba..bb, "");
                self.caret = a;
                self.anchor = a;
                true
            }
            None => false,
        }
    }

    /// Replace the selection with `s` (control characters dropped, length capped).
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let room = MAX_CHARS.saturating_sub(self.len());
        let clean: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).take(room).collect();
        let at = self.byte(self.caret);
        self.text.insert_str(at, &clean);
        self.caret += clean.chars().count();
        self.anchor = self.caret;
    }

    pub fn backspace(&mut self, word: bool) {
        if self.selection().is_none() {
            if self.caret == 0 {
                return;
            }
            self.anchor = if word { self.prev_word(self.caret) } else { self.caret - 1 };
        }
        self.delete_selection();
    }

    pub fn delete(&mut self, word: bool) {
        if self.selection().is_none() {
            if self.caret >= self.len() {
                return;
            }
            self.anchor = if word { self.next_word(self.caret) } else { self.caret + 1 };
        }
        self.delete_selection();
    }

    pub fn move_to(&mut self, idx: usize, extend: bool) {
        self.caret = idx.min(self.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn left(&mut self, extend: bool, word: bool) {
        if !extend && !word {
            if let Some((a, _)) = self.selection() {
                return self.move_to(a, false);
            }
        }
        let to = if word { self.prev_word(self.caret) } else { self.caret.saturating_sub(1) };
        self.move_to(to, extend);
    }

    pub fn right(&mut self, extend: bool, word: bool) {
        if !extend && !word {
            if let Some((_, b)) = self.selection() {
                return self.move_to(b, false);
            }
        }
        let to = if word { self.next_word(self.caret) } else { self.caret + 1 };
        self.move_to(to, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.len(), extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.len();
    }

    pub fn prev_word(&self, from: usize) -> usize {
        let cs = self.chars();
        let mut i = from.min(cs.len());
        while i > 0 && class(cs[i - 1]) == Class::Space {
            i -= 1;
        }
        if i > 0 {
            let k = class(cs[i - 1]);
            while i > 0 && class(cs[i - 1]) == k {
                i -= 1;
            }
        }
        i
    }

    pub fn next_word(&self, from: usize) -> usize {
        let cs = self.chars();
        let mut i = from.min(cs.len());
        if i < cs.len() {
            let k = class(cs[i]);
            while i < cs.len() && class(cs[i]) == k {
                i += 1;
            }
        }
        while i < cs.len() && class(cs[i]) == Class::Space {
            i += 1;
        }
        i
    }

    /// Run of same-class characters around `idx` (double-click selection).
    pub fn word_range(&self, idx: usize) -> (usize, usize) {
        let cs = self.chars();
        if cs.is_empty() {
            return (0, 0);
        }
        let i = idx.min(cs.len() - 1);
        let k = class(cs[i]);
        let (mut a, mut b) = (i, i + 1);
        while a > 0 && class(cs[a - 1]) == k {
            a -= 1;
        }
        while b < cs.len() && class(cs[b]) == k {
            b += 1;
        }
        (a, b)
    }
}
