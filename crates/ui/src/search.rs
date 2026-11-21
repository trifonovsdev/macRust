//! Search index, ranking and the inline calculator.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    App,
    Setting,
    Folder,
    File,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// What to open: a path, a `shell:AppsFolder\…` id or a URI.
    pub target: String,
    pub kind: Kind,
    lower: String,
}

impl Entry {
    pub fn new(name: impl Into<String>, target: impl Into<String>, kind: Kind) -> Self {
        let name = name.into();
        let lower = name.to_lowercase();
        Self { name, target: target.into(), kind, lower }
    }
}

#[derive(Default)]
pub struct Index {
    entries: Vec<Entry>,
}

fn is_boundary(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '.' | '(' | '[' | '/' | '\\' | ',')
}

fn kind_bonus(k: Kind) -> i32 {
    match k {
        Kind::App => 140,
        Kind::Setting => 70,
        Kind::Folder => 25,
        Kind::File => 0,
    }
}

/// Scores a lowercase `name` against a lowercase query; `None` = no match.
fn score(name: &str, q: &str, words: &[&str], fuzzy: bool) -> Option<i32> {
    if name == q {
        return Some(1000);
    }
    if name.starts_with(q) {
        return Some(900 - (name.chars().count() as i32 - q.chars().count() as i32).min(200));
    }
    if let Some(pos) = name.find(q) {
        let at_word = name[..pos].chars().last().is_some_and(is_boundary);
        return Some(if at_word { 720 } else { 480 - pos.min(200) as i32 });
    }
    if words.len() > 1 {
        let name_words: Vec<&str> = name.split(is_boundary).filter(|w| !w.is_empty()).collect();
        if words.iter().all(|w| name_words.iter().any(|nw| nw.starts_with(w))) {
            return Some(650);
        }
    }
    if fuzzy {
        // Letters in order ("vsc" -> "Visual Studio Code"); word starts score higher.
        let mut it = name.char_indices().peekable();
        let mut prev = ' ';
        let mut s = 260;
        let mut last_hit: Option<usize> = None;
        'outer: for qc in q.chars() {
            for (i, c) in it.by_ref() {
                let boundary = is_boundary(prev) || i == 0;
                prev = c;
                if c == qc {
                    if boundary {
                        s += 12;
                    }
                    if let Some(l) = last_hit
                        && i > l + 1
                    {
                        s -= 4;
                    }
                    last_hit = Some(i);
                    continue 'outer;
                }
            }
            return None;
        }
        return Some(s);
    }
    None
}

impl Index {
    pub fn push(&mut self, e: Entry) {
        self.entries.push(e);
    }

    pub fn extend(&mut self, it: impl IntoIterator<Item = Entry>) {
        self.entries.extend(it);
    }

    pub fn remove_kind(&mut self, kind: Kind) {
        self.entries.retain(|e| e.kind != kind);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, i: usize) -> &Entry {
        &self.entries[i]
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(i32, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let fuzzy = matches!(e.kind, Kind::App | Kind::Setting) && q.chars().count() >= 2;
            if let Some(s) = score(&e.lower, &q, &words, fuzzy) {
                let len_penalty = (e.lower.chars().count() as i32 / 4).min(30);
                hits.push((s + kind_bonus(e.kind) - len_penalty, i));
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| self.entries[a.1].name.len().cmp(&self.entries[b.1].name.len())));
        let mut out: Vec<usize> = Vec::with_capacity(limit);
        let mut seen: Vec<(Kind, &str)> = Vec::new();
        for (_, i) in hits {
            let e = &self.entries[i];
            // The same app is often linked from several Start menu folders.
            if e.kind == Kind::App {
                if seen.iter().any(|(k, n)| *k == Kind::App && n.eq_ignore_ascii_case(&e.name)) {
                    continue;
                }
                seen.push((Kind::App, &e.name));
            }
            out.push(i);
            if out.len() >= limit {
                break;
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Calculator

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if op == b'+' { v + r } else { v - r };
        }
        Some(v)
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        while let Some(op @ (b'*' | b'/' | b'%')) = self.peek() {
            self.i += 1;
            let r = self.unary()?;
            v = match op {
                b'*' => v * r,
                b'/' => v / r,
                _ => v % r,
            };
        }
        Some(v)
    }
    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.i += 1;
                Some(-self.unary()?)
            }
            b'+' => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            let e = self.unary()?;
            return Some(base.powf(e));
        }
        Some(base)
    }
    fn primary(&mut self) -> Option<f64> {
        match self.peek()? {
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                (self.peek() == Some(b')')).then(|| {
                    self.i += 1;
                    v
                })
            }
            b'0'..=b'9' | b'.' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
            }
            _ => None,
        }
    }
}

/// Evaluates an arithmetic expression; plain numbers are not "calculations".
pub fn calc(input: &str) -> Option<f64> {
    let norm: String = input
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            ',' => '.',
            '×' | 'x' | 'х' => '*',
            '÷' | ':' => '/',
            '−' => '-',
            c => c,
        })
        .collect();
    if !norm.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let body = norm.trim_start_matches(['-', '+']);
    if !body.contains(['+', '-', '*', '/', '^', '%', '(']) {
        return None;
    }
    let mut p = Parser { s: norm.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.ws();
    (p.i == norm.len() && v.is_finite()).then_some(v)
}

pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_handles_precedence_and_locale() {
        assert_eq!(calc("2+2*2"), Some(6.0));
        assert_eq!(calc("(1,5 + 0,5) ^ 3"), Some(8.0));
        assert_eq!(calc("10 / 4"), Some(2.5));
        assert_eq!(calc("-3 * -3"), Some(9.0));
        assert_eq!(calc("42"), None);
        assert_eq!(calc("-42"), None);
        assert_eq!(calc("notepad"), None);
        assert_eq!(calc("1/0"), None);
        assert_eq!(format_number(1.0 / 3.0), "0.3333333333");
        assert_eq!(format_number(1e6), "1000000");
    }

    #[test]
    fn apps_rank_above_files_and_duplicates_collapse() {
        let mut ix = Index::default();
        ix.push(Entry::new("notes.txt", "C:\\notes.txt", Kind::File));
        ix.push(Entry::new("Notepad", "a", Kind::App));
        ix.push(Entry::new("Notepad", "b", Kind::App));
        ix.push(Entry::new("Visual Studio Code", "c", Kind::App));
        let r = ix.search("note", 10);
        assert_eq!(ix.get(r[0]).name, "Notepad");
        assert_eq!(r.len(), 2);
        let r = ix.search("vsc", 10);
        assert_eq!(ix.get(r[0]).name, "Visual Studio Code");
        let r = ix.search("studio code", 10);
        assert_eq!(ix.get(r[0]).name, "Visual Studio Code");
    }

    #[test]
    fn cyrillic_matches_case_insensitively() {
        let mut ix = Index::default();
        ix.push(Entry::new("Блокнот", "x", Kind::App));
        assert_eq!(ix.search("блок", 5).len(), 1);
    }
}
