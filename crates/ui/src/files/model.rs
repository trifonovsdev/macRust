//! Folder contents: entries, natural sort, filters, the selection over the
//! visible order and the navigation history. No drawing, no platform calls.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

pub const ATTR_READONLY: u32 = 0x1;
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_DIRECTORY: u32 = 0x10;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// Lower-cased name: sort, filter and type-to-select compare on it.
    pub lower: String,
    pub dir: bool,
    pub size: u64,
    /// FILETIME: 100 ns ticks since 1601-01-01 UTC; 0 when unknown.
    pub modified: u64,
    pub attrs: u32,
    /// Lower-case extension without the dot; empty for folders.
    pub ext: String,
}

impl Entry {
    pub fn new(name: impl Into<String>, dir: bool, size: u64, modified: u64, attrs: u32) -> Self {
        let name = name.into();
        let lower = name.to_lowercase();
        let ext = if dir { String::new() } else { extension(&lower).to_string() };
        Self { name, lower, dir, size, modified, attrs, ext }
    }

    pub fn hidden(&self) -> bool {
        self.attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }
}

/// "archive.tar.gz" -> "gz"; ".gitignore" (a dot file) has none.
pub fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

/// Characters before the extension, for the rename editor's initial selection.
pub fn stem_len(name: &str) -> usize {
    let ext = extension(name);
    let n = name.chars().count();
    if ext.is_empty() { n } else { n - ext.chars().count() - 1 }
}

/// Compares like Finder and Explorer: case-insensitive, digit runs by value
/// ("file2" < "file10"). Both arguments must already be lower-case.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ia, mut ib) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut da = String::new();
                while let Some(c) = ia.peek().copied().filter(char::is_ascii_digit) {
                    da.push(c);
                    ia.next();
                }
                let mut db = String::new();
                while let Some(c) = ib.peek().copied().filter(char::is_ascii_digit) {
                    db.push(c);
                    ib.next();
                }
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| db.len().cmp(&da.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                if ca != cb {
                    return ca.cmp(&cb);
                }
                ia.next();
                ib.next();
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort {
    pub column: Column,
    pub ascending: bool,
    pub folders_first: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self { column: Column::Name, ascending: true, folders_first: true }
    }
}

impl Sort {
    /// A click on a column header: the same column flips the order; another
    /// one starts in its natural order (newest and largest first, like Finder).
    pub fn clicked(self, column: Column) -> Self {
        if self.column == column {
            Self { ascending: !self.ascending, ..self }
        } else {
            Self { column, ascending: matches!(column, Column::Name | Column::Kind), ..self }
        }
    }
}

/// Kind names per extension, supplied by the host (localized by the system).
#[derive(Default)]
pub struct Kinds {
    pub map: HashMap<String, String>,
    pub folder: String,
    pub file: String,
}

impl Kinds {
    pub fn of<'a>(&'a self, e: &'a Entry) -> &'a str {
        if e.dir {
            return &self.folder;
        }
        self.map.get(&e.ext).map(String::as_str).unwrap_or(&self.file)
    }
}

pub fn compare(a: &Entry, b: &Entry, sort: Sort, kinds: &Kinds) -> Ordering {
    if sort.folders_first && a.dir != b.dir {
        return if a.dir { Ordering::Less } else { Ordering::Greater };
    }
    let by = match sort.column {
        Column::Name => Ordering::Equal,
        Column::Modified => a.modified.cmp(&b.modified),
        // Folders have no size; they sort among themselves by name.
        Column::Size => if a.dir && b.dir { Ordering::Equal } else { a.size.cmp(&b.size) },
        Column::Kind => kinds.of(a).to_lowercase().cmp(&kinds.of(b).to_lowercase()),
    };
    let by = if sort.ascending { by } else { by.reverse() };
    by.then_with(|| {
        let n = natural_cmp(&a.lower, &b.lower);
        if sort.column == Column::Name && !sort.ascending { n.reverse() } else { n }
    })
}

/// The contents of one folder and the view's state over it.
#[derive(Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub kinds: Kinds,
    /// Indices into `entries`, filtered and sorted: what the view shows.
    pub visible: Vec<u32>,
    /// One flag per entry.
    selected: Vec<bool>,
    selected_n: usize,
    /// Positions in `visible`.
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
    pub sort: Sort,
    pub show_hidden: bool,
    filter: String,
}

impl Listing {
    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn entry(&self, pos: usize) -> Option<&Entry> {
        self.visible.get(pos).map(|&i| &self.entries[i as usize])
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// Replaces the contents; the selection and cursor follow names that
    /// still exist (a live refresh must not lose them).
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let keep: HashSet<String> = self.selected_entries().map(|e| e.name.clone()).collect();
        let cursor_name = self.cursor.and_then(|c| self.entry(c)).map(|e| e.name.clone());
        let anchor_name = self.anchor.and_then(|c| self.entry(c)).map(|e| e.name.clone());
        self.entries = entries;
        self.selected = self.entries.iter().map(|e| keep.contains(&e.name)).collect();
        self.selected_n = self.selected.iter().filter(|s| **s).count();
        self.rebuild();
        self.cursor = cursor_name.and_then(|n| self.position_of(&n));
        self.anchor = anchor_name.and_then(|n| self.position_of(&n)).or(self.cursor);
    }

    /// Adds a batch while a folder is still loading.
    pub fn extend(&mut self, more: Vec<Entry>) {
        let cursor_name = self.cursor.and_then(|c| self.entry(c)).map(|e| e.name.clone());
        self.selected.extend(std::iter::repeat_n(false, more.len()));
        self.entries.extend(more);
        self.rebuild();
        self.cursor = cursor_name.and_then(|n| self.position_of(&n));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.visible.clear();
        self.selected.clear();
        self.selected_n = 0;
        self.anchor = None;
        self.cursor = None;
        self.filter.clear();
    }

    pub fn position_of(&self, name: &str) -> Option<usize> {
        self.visible.iter().position(|&i| self.entries[i as usize].name == name)
    }

    pub fn set_sort(&mut self, sort: Sort) {
        if self.sort != sort {
            self.sort = sort;
            self.resort_keep_cursor();
        }
    }

    pub fn set_show_hidden(&mut self, on: bool) {
        if self.show_hidden != on {
            self.show_hidden = on;
            self.resort_keep_cursor();
        }
    }

    pub fn set_filter(&mut self, text: &str) {
        let f = text.trim().to_lowercase();
        if f != self.filter {
            self.filter = f;
            self.resort_keep_cursor();
        }
    }

    fn resort_keep_cursor(&mut self) {
        let cursor = self.cursor.and_then(|c| self.visible.get(c).copied());
        let anchor = self.anchor.and_then(|c| self.visible.get(c).copied());
        self.rebuild();
        self.cursor = cursor.and_then(|i| self.visible.iter().position(|&v| v == i));
        self.anchor = anchor.and_then(|i| self.visible.iter().position(|&v| v == i));
    }

    fn rebuild(&mut self) {
        let f = &self.filter;
        let show_hidden = self.show_hidden;
        let mut vis: Vec<u32> = (0..self.entries.len() as u32)
            .filter(|&i| {
                let e = &self.entries[i as usize];
                (show_hidden || !e.hidden()) && (f.is_empty() || e.lower.contains(f.as_str()))
            })
            .collect();
        let (entries, sort, kinds) = (&self.entries, self.sort, &self.kinds);
        vis.sort_by(|&a, &b| compare(&entries[a as usize], &entries[b as usize], sort, kinds));
        // A selected entry that is filtered out stops being selected: actions
        // must only ever touch what the user sees.
        let shown: HashSet<u32> = vis.iter().copied().collect();
        for (i, s) in self.selected.iter_mut().enumerate() {
            if *s && !shown.contains(&(i as u32)) {
                *s = false;
            }
        }
        self.selected_n = self.selected.iter().filter(|s| **s).count();
        self.visible = vis;
    }

    // -- selection --------------------------------------------------------

    pub fn is_selected(&self, pos: usize) -> bool {
        self.visible.get(pos).is_some_and(|&i| self.selected[i as usize])
    }

    pub fn selected_count(&self) -> usize {
        self.selected_n
    }

    /// Selected entries in view order.
    pub fn selected_entries(&self) -> impl Iterator<Item = &Entry> {
        self.visible.iter().filter(|&&i| self.selected.get(i as usize).copied().unwrap_or(false)).map(|&i| &self.entries[i as usize])
    }

    pub fn selected_positions(&self) -> Vec<usize> {
        (0..self.visible.len()).filter(|&p| self.is_selected(p)).collect()
    }

    fn set(&mut self, pos: usize, on: bool) {
        if let Some(&i) = self.visible.get(pos) {
            let s = &mut self.selected[i as usize];
            if *s != on {
                *s = on;
                if on { self.selected_n += 1 } else { self.selected_n -= 1 }
            }
        }
    }

    pub fn select_none(&mut self) {
        self.selected.iter_mut().for_each(|s| *s = false);
        self.selected_n = 0;
    }

    pub fn select_all(&mut self) {
        for p in 0..self.visible.len() {
            self.set(p, true);
        }
    }

    /// Selects exactly `pos` and makes it the anchor and cursor.
    pub fn select_only(&mut self, pos: usize) {
        self.select_none();
        if pos < self.visible.len() {
            self.set(pos, true);
            self.anchor = Some(pos);
            self.cursor = Some(pos);
        }
    }

    /// A click with modifiers: Ctrl toggles, Shift selects the range from the
    /// anchor (added to the selection when Ctrl is held too).
    pub fn click(&mut self, pos: usize, ctrl: bool, shift: bool) {
        if pos >= self.visible.len() {
            return;
        }
        if shift {
            let a = self.anchor.unwrap_or(pos).min(self.visible.len() - 1);
            if !ctrl {
                self.select_none();
            }
            for p in a.min(pos)..=a.max(pos) {
                self.set(p, true);
            }
            self.anchor = Some(a);
            self.cursor = Some(pos);
        } else if ctrl {
            let on = !self.is_selected(pos);
            self.set(pos, on);
            self.anchor = Some(pos);
            self.cursor = Some(pos);
        } else {
            self.select_only(pos);
        }
    }

    /// Keyboard movement to `pos`; with Shift the selection spans anchor..pos.
    pub fn move_to(&mut self, pos: usize, shift: bool) {
        if self.visible.is_empty() {
            return;
        }
        let pos = pos.min(self.visible.len() - 1);
        if shift {
            let a = self.anchor.unwrap_or(pos);
            self.select_none();
            for p in a.min(pos)..=a.max(pos) {
                self.set(p, true);
            }
            self.anchor = Some(a);
            self.cursor = Some(pos);
        } else {
            self.select_only(pos);
        }
    }

    /// Rubber band: `base` (positions selected when the drag began, kept with
    /// Ctrl or Shift) plus the positions inside the band.
    pub fn band(&mut self, base: &[usize], inside: &[usize]) {
        self.select_none();
        for &p in base.iter().chain(inside) {
            self.set(p, true);
        }
        if let Some(&last) = inside.last() {
            self.cursor = Some(last);
            self.anchor = inside.first().copied();
        }
    }

    /// Type-to-select: the first visible entry whose name starts with
    /// `prefix` (lower-case), else the first one that sorts after it.
    pub fn find_prefix(&self, prefix: &str) -> Option<usize> {
        if prefix.is_empty() {
            return None;
        }
        let names = self.visible.iter().map(|&i| &self.entries[i as usize].lower);
        let mut after = None;
        for (p, n) in names.enumerate() {
            if n.starts_with(prefix) {
                return Some(p);
            }
            if after.is_none() && natural_cmp(n, prefix) == Ordering::Greater && self.sort.column == Column::Name && self.sort.ascending {
                after = Some(p);
            }
        }
        after
    }
}

// ---------------------------------------------------------------------------
// History

/// Where the user was: the folder and what to restore when coming back.
#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub path: String,
    pub scroll: f32,
    pub selected: Option<String>,
}

#[derive(Default, Debug)]
pub struct History {
    back: Vec<Visit>,
    fwd: Vec<Visit>,
}

const HISTORY_MAX: usize = 100;

impl History {
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }

    /// Leaving `from` for a new folder.
    pub fn visit(&mut self, from: Visit) {
        if self.back.last().is_some_and(|v| v.path == from.path) {
            self.back.pop();
        }
        self.back.push(from);
        if self.back.len() > HISTORY_MAX {
            self.back.remove(0);
        }
        self.fwd.clear();
    }

    pub fn back(&mut self, current: Visit) -> Option<Visit> {
        let v = self.back.pop()?;
        self.fwd.push(current);
        Some(v)
    }

    pub fn forward(&mut self, current: Visit) -> Option<Visit> {
        let v = self.fwd.pop()?;
        self.back.push(current);
        Some(v)
    }
}

// ---------------------------------------------------------------------------
// Paths (Windows syntax; plain string logic so it is testable anywhere)

/// The parent folder, or None at a drive or share root.
pub fn parent(path: &str) -> Option<String> {
    let root = &path[..root_len(path)];
    let rest = path[root.len()..].trim_matches(['\\', '/']);
    if rest.is_empty() {
        return None;
    }
    match rest.rfind(['\\', '/']) {
        Some(i) => Some(format!("{root}{}", &rest[..i])),
        None if !root.is_empty() => Some(root.to_string()),
        None => None,
    }
}

/// Length of the root part including its trailing separator: "C:\" -> 3,
/// "\\server\share\" -> up to and including the slash after the share.
fn root_len(path: &str) -> usize {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return if b.len() >= 3 && (b[2] == b'\\' || b[2] == b'/') { 3 } else { 2 };
    }
    if let Some(rest) = path.strip_prefix("\\\\") {
        let mut parts = rest.splitn(3, ['\\', '/']);
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let n = 2 + server.len() + 1 + share.len();
        return (n + 1).min(path.len());
    }
    0
}

/// The last component's name (the whole path for a root).
pub fn leaf(path: &str) -> &str {
    if parent(path).is_none() {
        return path;
    }
    let t = path.trim_end_matches(['\\', '/']);
    t.rsplit(['\\', '/']).next().unwrap_or(t)
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with(['\\', '/']) { format!("{dir}{name}") } else { format!("{dir}\\{name}") }
}

/// Folders from the root to `path`, as (path, name) pairs.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = Some(path.to_string());
    while let Some(p) = cur {
        out.push((p.clone(), leaf(&p).to_string()));
        cur = parent(&p);
        if out.len() > 256 {
            break;
        }
    }
    out.reverse();
    out
}

pub fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\\', '/']).to_lowercase() == b.trim_end_matches(['\\', '/']).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, dir: bool) -> Entry {
        Entry::new(name, dir, 0, 0, 0)
    }

    fn names(l: &Listing) -> Vec<&str> {
        l.visible.iter().map(|&i| l.entries[i as usize].name.as_str()).collect()
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "file02", "a", "Б", "б2"];
        let mut v2: Vec<String> = v.iter().map(|s| s.to_lowercase()).collect();
        v2.sort_by(|a, b| natural_cmp(a, b));
        // Equal numbers: the one with leading zeros first (Explorer does the same).
        assert_eq!(v2, ["a", "file1", "file02", "file2", "file10", "б", "б2"]);
        v.clear();
        assert_eq!(natural_cmp("x9", "x10"), Ordering::Less);
        assert_eq!(natural_cmp("x", "x1"), Ordering::Less);
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("a.tar.gz"), "gz");
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("noext"), "");
        assert_eq!(extension("trailing."), "");
        assert_eq!(stem_len("photo.jpeg"), 5);
        assert_eq!(stem_len("Папка"), 5);
        assert_eq!(stem_len("отчёт.docx"), 5);
    }

    #[test]
    fn folders_first_then_natural() {
        let mut l = Listing::default();
        l.set_entries(vec![e("b10.txt", false), e("b2.txt", false), e("zdir", true), e("adir", true)]);
        assert_eq!(names(&l), ["adir", "zdir", "b2.txt", "b10.txt"]);
        l.set_sort(Sort { column: Column::Name, ascending: false, folders_first: true });
        assert_eq!(names(&l), ["zdir", "adir", "b10.txt", "b2.txt"]);
        l.set_sort(Sort { column: Column::Name, ascending: true, folders_first: false });
        assert_eq!(names(&l), ["adir", "b2.txt", "b10.txt", "zdir"]);
    }

    #[test]
    fn sort_by_size_and_date() {
        let mut l = Listing::default();
        l.set_entries(vec![
            Entry::new("small", false, 10, 300, 0),
            Entry::new("big", false, 1000, 100, 0),
            Entry::new("mid", false, 100, 200, 0),
        ]);
        l.set_sort(Sort::default().clicked(Column::Size));
        assert_eq!(names(&l), ["big", "mid", "small"]);
        l.set_sort(l.sort.clicked(Column::Size));
        assert_eq!(names(&l), ["small", "mid", "big"]);
        l.set_sort(l.sort.clicked(Column::Modified));
        assert_eq!(names(&l), ["small", "mid", "big"]);
    }

    #[test]
    fn hidden_and_filter() {
        let mut l = Listing::default();
        l.set_entries(vec![
            Entry::new("desktop.ini", false, 1, 1, ATTR_HIDDEN | ATTR_SYSTEM),
            e("Отчёт.docx", false),
            e("notes.txt", false),
        ]);
        assert_eq!(names(&l), ["notes.txt", "Отчёт.docx"]);
        l.set_show_hidden(true);
        assert_eq!(l.len(), 3);
        l.set_filter("ОТЧ");
        assert_eq!(names(&l), ["Отчёт.docx"]);
        l.set_filter("");
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn selection_clicks() {
        let mut l = Listing::default();
        l.set_entries((0..10).map(|i| e(&format!("f{i}"), false)).collect());
        l.click(2, false, false);
        l.click(5, false, true);
        assert_eq!(l.selected_positions(), [2, 3, 4, 5]);
        l.click(8, true, false);
        assert_eq!(l.selected_positions(), [2, 3, 4, 5, 8]);
        l.click(3, true, false);
        assert_eq!(l.selected_positions(), [2, 4, 5, 8]);
        l.click(1, false, true);
        assert_eq!(l.selected_positions(), [1, 2, 3]);
        assert_eq!(l.selected_count(), 3);
        l.move_to(5, true);
        assert_eq!(l.selected_positions(), [3, 4, 5]);
        l.select_all();
        assert_eq!(l.selected_count(), 10);
    }

    #[test]
    fn selection_survives_refresh_and_filter_drops_hidden_ones() {
        let mut l = Listing::default();
        l.set_entries(vec![e("a", false), e("b", false), e("c", false)]);
        l.click(1, false, false);
        l.set_entries(vec![e("0", false), e("a", false), e("b", false), e("c", false)]);
        assert_eq!(l.selected_positions(), [2]);
        assert_eq!(l.cursor, Some(2));
        l.set_filter("c");
        assert_eq!(l.selected_count(), 0);
    }

    #[test]
    fn type_to_select() {
        let mut l = Listing::default();
        l.set_entries(vec![e("apple", false), e("banana", false), e("blueberry", false), e("cherry", false)]);
        assert_eq!(l.find_prefix("b"), Some(1));
        assert_eq!(l.find_prefix("bl"), Some(2));
        assert_eq!(l.find_prefix("bz"), Some(3));
        assert_eq!(l.find_prefix("zz"), None);
    }

    #[test]
    fn band_selection() {
        let mut l = Listing::default();
        l.set_entries((0..6).map(|i| e(&format!("f{i}"), false)).collect());
        l.band(&[0], &[3, 4]);
        assert_eq!(l.selected_positions(), [0, 3, 4]);
    }

    #[test]
    fn history_back_forward() {
        let v = |p: &str| Visit { path: p.into(), scroll: 0.0, selected: None };
        let mut h = History::default();
        h.visit(v("a"));
        h.visit(v("b"));
        assert_eq!(h.back(v("c")).unwrap().path, "b");
        assert_eq!(h.back(v("b")).unwrap().path, "a");
        assert!(!h.can_back());
        assert_eq!(h.forward(v("a")).unwrap().path, "b");
        h.visit(v("b"));
        assert!(!h.can_forward());
    }

    #[test]
    fn paths() {
        assert_eq!(parent("C:\\Users\\me"), Some("C:\\Users".into()));
        assert_eq!(parent("C:\\Users"), Some("C:\\".into()));
        assert_eq!(parent("C:\\"), None);
        assert_eq!(parent("C:\\Users\\"), Some("C:\\".into()));
        assert_eq!(parent("\\\\srv\\share\\dir"), Some("\\\\srv\\share\\".into()));
        assert_eq!(parent("\\\\srv\\share\\"), None);
        assert_eq!(parent("\\\\srv\\share"), None);
        assert_eq!(leaf("C:\\Users\\me"), "me");
        assert_eq!(leaf("C:\\"), "C:\\");
        assert_eq!(join("C:\\", "x"), "C:\\x");
        assert_eq!(join("C:\\a", "x"), "C:\\a\\x");
        let c = crumbs("C:\\Users\\me");
        assert_eq!(c.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(), ["C:\\", "Users", "me"]);
        assert!(same_path("C:\\Users\\", "c:\\users"));
        assert!(same_path("C:\\Папка", "c:\\папка\\"));
    }
}
