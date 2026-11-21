//! A single-line text editor: the search field and the inline rename box.
//! Positions are char indices, not bytes.

#[derive(Clone, Debug, Default)]
pub struct Field {
    chars: Vec<char>,
    pub caret: usize,
    /// The other end of the selection; equal to `caret` when nothing is selected.
    pub anchor: usize,
}

impl Field {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        Self { chars, caret: n, anchor: n }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn select(&mut self, from: usize, to: usize) {
        self.anchor = from.min(self.chars.len());
        self.caret = to.min(self.chars.len());
    }

    pub fn select_all(&mut self) {
        self.select(0, self.chars.len());
    }

    pub fn set_text(&mut self, text: &str) {
        *self = Self::new(text);
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        self.chars[a..b].iter().collect()
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.chars.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        for c in s.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
        self.anchor = self.caret;
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let from = if word { self.word_left(self.caret) } else { self.caret - 1 };
        self.chars.drain(from..self.caret);
        self.caret = from;
        self.anchor = from;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() || self.caret >= self.chars.len() {
            return;
        }
        let to = if word { self.word_right(self.caret) } else { self.caret + 1 };
        self.chars.drain(self.caret..to);
        self.anchor = self.caret;
    }

    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self, mut i: usize) -> usize {
        let n = self.chars.len();
        while i < n && !self.chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < n && self.chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    /// Arrow keys: without Shift a selection collapses to its edge first.
    pub fn left(&mut self, shift: bool, word: bool) {
        let (a, b) = self.selection();
        let to = if !shift && a != b {
            a
        } else if word {
            self.word_left(self.caret)
        } else {
            self.caret.saturating_sub(1)
        };
        self.caret = to;
        if !shift {
            self.anchor = to;
        }
    }

    pub fn right(&mut self, shift: bool, word: bool) {
        let (a, b) = self.selection();
        let to = if !shift && a != b {
            b
        } else if word {
            self.word_right(self.caret)
        } else {
            (self.caret + 1).min(self.chars.len())
        };
        self.caret = to;
        if !shift {
            self.anchor = to;
        }
    }

    pub fn home(&mut self, shift: bool) {
        self.caret = 0;
        if !shift {
            self.anchor = 0;
        }
    }

    pub fn end(&mut self, shift: bool) {
        self.caret = self.chars.len();
        if !shift {
            self.anchor = self.caret;
        }
    }

    /// Text before char `i`, for measuring the caret and selection position.
    pub fn prefix(&self, i: usize) -> String {
        self.chars[..i.min(self.chars.len())].iter().collect()
    }

    /// The char index nearest to `x` px, given a measuring function.
    pub fn index_at(&self, x: f32, measure: impl Fn(&str) -> f32) -> usize {
        let mut best = (0, f32::MAX);
        let mut s = String::new();
        for i in 0..=self.chars.len() {
            let d = (measure(&s) - x).abs();
            if d < best.1 {
                best = (i, d);
            }
            if i < self.chars.len() {
                s.push(self.chars[i]);
            }
        }
        best.0
    }
}

/// Characters Windows does not allow in file names.
pub fn valid_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && t != "." && t != ".." && !t.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut f = Field::new("отчёт.docx");
        f.select(0, 5);
        f.insert("план");
        assert_eq!(f.text(), "план.docx");
        assert_eq!(f.caret, 4);
        f.backspace(false);
        assert_eq!(f.text(), "пла.docx");
        f.end(false);
        f.backspace(true);
        assert_eq!(f.text(), "пла.");
        f.home(false);
        f.right(true, false);
        assert_eq!(f.selected_text(), "п");
        f.right(false, false);
        assert_eq!(f.caret, 1);
        f.delete(false);
        assert_eq!(f.text(), "па.");
        f.select_all();
        f.insert("x\ny");
        assert_eq!(f.text(), "xy");
    }

    #[test]
    fn word_moves() {
        let mut f = Field::new("one two  three");
        f.left(false, true);
        assert_eq!(f.caret, 9);
        f.left(true, true);
        assert_eq!(f.caret, 4);
        assert_eq!(f.selected_text(), "two  ");
        f.left(false, false);
        assert_eq!(f.caret, 4);
    }

    #[test]
    fn hit_index() {
        let f = Field::new("abcd");
        assert_eq!(f.index_at(21.0, |s| s.chars().count() as f32 * 10.0), 2);
        assert_eq!(f.index_at(99.0, |s| s.chars().count() as f32 * 10.0), 4);
    }

    #[test]
    fn names() {
        assert!(valid_name("Новая папка"));
        assert!(!valid_name("a:b"));
        assert!(!valid_name("  "));
        assert!(!valid_name(".."));
    }
}
