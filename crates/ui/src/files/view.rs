//! The state machine of one file manager window: navigation, scrolling,
//! selection, the search and rename fields, and what the host must do in
//! response (`Action`). The host feeds input in window pixels.

use std::collections::HashMap;
use std::rc::Rc;

use super::art::{Art, PlaceIcon};
use super::field::{Field, valid_name};
use super::format::Civil;
use super::layout::{self, Columns, Frame, Grid, Rect, View};
use super::model::{self, Column, Entry, History, Listing, Visit};
use super::strings::FileStrings;
use crate::Mods;
use crate::gfx::Image;
use crate::text::{Text, Weight};

// Windows virtual-key codes the browser understands.
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const ESCAPE: u32 = 0x1B;
    pub const PRIOR: u32 = 0x21;
    pub const NEXT: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const DELETE: u32 = 0x2E;
    pub const KEY_1: u32 = 0x31;
    pub const KEY_2: u32 = 0x32;
    pub const A: u32 = 0x41;
    pub const C: u32 = 0x43;
    pub const F: u32 = 0x46;
    pub const L: u32 = 0x4C;
    pub const N: u32 = 0x4E;
    pub const V: u32 = 0x56;
    pub const W: u32 = 0x57;
    pub const X: u32 = 0x58;
    pub const F2: u32 = 0x71;
    pub const F5: u32 = 0x74;
    pub const OEM_PERIOD: u32 = 0xBE;
}

#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub path: String,
    pub icon: PlaceIcon,
    pub eject: bool,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub title: &'static str,
    pub places: Vec<Place>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Done,
    /// A Win32 error code.
    Failed(u32),
}

/// What the host has to do. Paths are full paths.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Start enumerating this folder (cancel any older enumeration).
    Navigate(String),
    /// Open files with their default programs.
    OpenFiles(Vec<String>),
    NewWindow(String),
    /// Not a folder we can list (shell: locations): hand it to the shell.
    ShellOpen(String),
    Rename { path: String, to: String },
    Delete { paths: Vec<String>, permanent: bool },
    Clipboard { paths: Vec<String>, cut: bool },
    Paste { into: String },
    NewFolder { path: String },
    /// The shell's context menu at window pixel `x, y`; no paths means the
    /// folder background menu.
    ContextMenu { paths: Vec<String>, x: f32, y: f32 },
    DragOut(Vec<String>),
    Eject(String),
    Close,
    Minimize,
    Zoom,
    Refresh,
    /// View options changed; persist them.
    SaveOptions,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    /// Empty toolbar or sidebar space: the host reports a caption there.
    Caption,
    Light(usize),
    Back,
    Forward,
    ViewIcons,
    ViewList,
    NewFolder,
    Search,
    SearchClear,
    Place(usize, usize),
    Eject(usize, usize),
    Header(Column),
    Item(usize),
    Blank,
    Scrollbar,
    Crumb(usize),
}

pub enum IconSlot {
    Pending,
    Ready(Rc<Image>),
    Missing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconRequest {
    pub key: String,
    pub path: String,
    pub size: u32,
    pub thumbnail: bool,
}

#[derive(Clone, Debug)]
enum Press {
    /// `narrow`: a plain click inside a multi-selection; it selects just this
    /// item on release, so a drag can still take the whole selection along.
    Item { pos: usize, x: f32, y: f32, solo: bool, label: bool, narrow: bool },
    /// Rubber band; corners in content coordinates (scroll included).
    Band { x0: f32, y0: f32, x1: f32, y1: f32, base: Vec<usize> },
    Button(Hit),
    Scrollbar { grab: f32 },
    Text { rename: bool },
}

struct Rename {
    name: String,
    field: Field,
}

/// Extensions whose icon is the file's own (programs, shortcuts, icons).
const OWN_ICON: &[&str] = &["exe", "lnk", "ico", "url", "msi", "scr", "cur", "ani", "appref-ms", "website"];
/// Extensions with picture thumbnails in icon view.
const THUMBS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "tif", "tiff", "mp4", "mov", "mkv", "avi", "wmv", "psd", "pdf"];

const DRAG_START: f32 = 5.0;
const TYPE_RESET: f32 = 1.0;
const SLOW_CLICK: f32 = 0.55;
const SCROLLBAR_LINGER: f32 = 1.2;

pub struct Browser {
    pub strings: &'static FileStrings,
    pub w: f32,
    pub h: f32,
    pub scale: f32,
    pub active: bool,
    pub maximized: bool,
    pub path: String,
    pub list: Listing,
    pub view: View,
    pub show_pathbar: bool,
    pub sections: Vec<Section>,
    /// Display names of roots ("C:\" -> "Локальный диск (C:)") for the path bar.
    pub root_names: Vec<(String, String)>,
    history: History,
    pub load: Load,
    pub free: Option<u64>,
    pub(super) scroll: f32,
    target: f32,
    pub(super) scrollbar_t: f32,
    pub(super) hover: Hit,
    pub(super) lights_hover: bool,
    press: Option<Press>,
    pub(super) search: Field,
    pub(super) search_focus: bool,
    rename: Option<Rename>,
    type_buf: String,
    type_at: f32,
    pub(super) time: f32,
    caret_since: f32,
    slow_rename: Option<(usize, f32)>,
    pub icons: HashMap<String, IconSlot>,
    /// A folder item (position) or the current folder (None) under a drag.
    pub drop_hover: Option<Option<usize>>,
    pub now: Civil,
    pub tz: i64,
    pub(super) art: Rc<Art>,
    pub dirty: bool,
    restore: Option<Visit>,
    select_when_seen: Option<(String, bool)>,
}

impl Browser {
    pub fn new(strings: &'static FileStrings, art: Rc<Art>, w: f32, h: f32, scale: f32) -> Self {
        let mut list = Listing::default();
        list.kinds.folder = strings.folder.into();
        list.kinds.file = strings.file.into();
        Self {
            strings,
            w,
            h,
            scale,
            active: true,
            maximized: false,
            path: String::new(),
            list,
            view: View::Icons,
            show_pathbar: true,
            sections: Vec::new(),
            root_names: Vec::new(),
            history: History::default(),
            load: Load::Done,
            free: None,
            scroll: 0.0,
            target: 0.0,
            scrollbar_t: 0.0,
            hover: Hit::None,
            lights_hover: false,
            press: None,
            search: Field::default(),
            search_focus: false,
            rename: None,
            type_buf: String::new(),
            type_at: -10.0,
            time: 0.0,
            caret_since: 0.0,
            slow_rename: None,
            icons: HashMap::new(),
            drop_hover: None,
            now: Civil { year: 2000, month: 1, day: 1, hour: 0, minute: 0 },
            tz: 0,
            art,
            dirty: true,
            restore: None,
            select_when_seen: None,
        }
    }

    pub fn frame(&self) -> Frame {
        layout::frame(self.w, self.h, self.scale, self.view, self.show_pathbar)
    }

    pub fn resize(&mut self, w: f32, h: f32, scale: f32) {
        if (w, h, scale) != (self.w, self.h, self.scale) {
            let rescale = scale / self.scale;
            self.w = w;
            self.h = h;
            self.scale = scale;
            self.scroll *= rescale;
            self.target *= rescale;
            self.clamp_scroll();
            self.dirty = true;
        }
    }

    /// The window title: a sidebar place's name ("Загрузки" rather than the
    /// folder's on-disk "Downloads"), a drive's label, else the folder name.
    pub fn title(&self) -> &str {
        self.root_name(&self.path).unwrap_or_else(|| model::leaf(&self.path))
    }

    fn root_name(&self, path: &str) -> Option<&str> {
        self.sections
            .iter()
            .flat_map(|s| &s.places)
            .find(|p| model::same_path(&p.path, path))
            .map(|p| p.name.as_str())
            .or_else(|| self.root_names.iter().find(|(p, _)| model::same_path(p, path)).map(|(_, n)| n.as_str()))
    }

    pub fn can_back(&self) -> bool {
        self.history.can_back()
    }

    pub fn can_forward(&self) -> bool {
        self.history.can_forward()
    }

    pub fn renaming(&self) -> Option<(&str, &Field)> {
        self.rename.as_ref().map(|r| (r.name.as_str(), &r.field))
    }

    pub fn band(&self) -> Option<Rect> {
        match &self.press {
            Some(Press::Band { x0, y0, x1, y1, .. }) => Some(Rect::span(*x0, *y0, *x1, *y1)),
            _ => None,
        }
    }

    pub fn caret_on(&self) -> bool {
        ((self.time - self.caret_since) / 0.53) as i64 % 2 == 0
    }

    // -- navigation -------------------------------------------------------

    fn visit(&self) -> Visit {
        Visit {
            path: self.path.clone(),
            scroll: self.scroll,
            selected: self.list.cursor.and_then(|c| self.list.entry(c)).map(|e| e.name.clone()),
        }
    }

    /// Opens `path` in this window.
    pub fn navigate(&mut self, path: &str) -> Vec<Action> {
        if path.starts_with("shell:") {
            return vec![Action::ShellOpen(path.into())];
        }
        if !self.path.is_empty() && model::same_path(path, &self.path) {
            return vec![Action::Refresh];
        }
        if !self.path.is_empty() {
            self.history.visit(self.visit());
        }
        self.enter(path, None)
    }

    fn enter(&mut self, path: &str, restore: Option<Visit>) -> Vec<Action> {
        // Coming up out of a folder selects it, as Finder does.
        let from_child = model::parent(&self.path).is_some_and(|p| model::same_path(&p, path));
        let child_name = from_child.then(|| model::leaf(&self.path).to_string());
        self.path = path.to_string();
        self.list.clear();
        self.load = Load::Loading;
        self.free = None;
        self.scroll = 0.0;
        self.target = 0.0;
        self.search = Field::default();
        self.search_focus = false;
        self.rename = None;
        self.press = None;
        self.slow_rename = None;
        self.restore = restore;
        self.select_when_seen = child_name.map(|n| (n, false));
        self.dirty = true;
        vec![Action::Navigate(path.to_string())]
    }

    pub fn back(&mut self) -> Vec<Action> {
        let cur = self.visit();
        match self.history.back(cur) {
            Some(v) => {
                let p = v.path.clone();
                self.enter(&p, Some(v))
            }
            None => Vec::new(),
        }
    }

    pub fn forward(&mut self) -> Vec<Action> {
        let cur = self.visit();
        match self.history.forward(cur) {
            Some(v) => {
                let p = v.path.clone();
                self.enter(&p, Some(v))
            }
            None => Vec::new(),
        }
    }

    pub fn up(&mut self) -> Vec<Action> {
        match model::parent(&self.path) {
            Some(p) => self.navigate(&p),
            None => Vec::new(),
        }
    }

    /// A batch of entries arrived; `done` ends the enumeration.
    pub fn loaded(&mut self, entries: Vec<Entry>, kinds: Vec<(String, String)>, done: bool, free: Option<u64>) {
        self.list.kinds.map.extend(kinds);
        if !entries.is_empty() {
            self.list.extend(entries);
        }
        if free.is_some() {
            self.free = free;
        }
        if done {
            self.load = Load::Done;
            if let Some(v) = self.restore.take() {
                self.target = v.scroll;
                self.scroll = v.scroll;
                self.clamp_scroll();
                if let Some(p) = v.selected.and_then(|n| self.list.position_of(&n)) {
                    self.list.select_only(p);
                }
            }
        }
        self.apply_pending_select();
        self.dirty = true;
    }

    /// A live refresh: the whole folder again, selection kept by name.
    pub fn replaced(&mut self, entries: Vec<Entry>, kinds: Vec<(String, String)>, free: Option<u64>) {
        self.list.kinds.map.extend(kinds);
        self.list.set_entries(entries);
        if free.is_some() {
            self.free = free;
        }
        self.load = Load::Done;
        if let Some(r) = &self.rename
            && self.list.position_of(&r.name).is_none()
        {
            self.rename = None;
        }
        self.apply_pending_select();
        self.clamp_scroll();
        self.dirty = true;
    }

    pub fn failed(&mut self, code: u32) {
        self.load = Load::Failed(code);
        self.dirty = true;
    }

    fn apply_pending_select(&mut self) {
        let Some((name, rename)) = self.select_when_seen.clone() else { return };
        if let Some(p) = self.list.position_of(&name) {
            self.select_when_seen = None;
            self.list.select_only(p);
            self.ensure_visible(p);
            if rename {
                self.begin_rename(p);
            }
        }
    }

    pub fn set_view(&mut self, v: View) -> Vec<Action> {
        if self.view == v {
            return Vec::new();
        }
        self.view = v;
        self.rename = None;
        self.scroll = 0.0;
        self.target = 0.0;
        if let Some(c) = self.list.cursor {
            self.ensure_visible(c);
            self.scroll = self.target;
        }
        self.dirty = true;
        vec![Action::SaveOptions]
    }

    pub fn toggle_hidden(&mut self) -> Vec<Action> {
        let on = !self.list.show_hidden;
        self.list.set_show_hidden(on);
        self.clamp_scroll();
        self.dirty = true;
        vec![Action::SaveOptions]
    }

    pub fn toggle_pathbar(&mut self) -> Vec<Action> {
        self.show_pathbar = !self.show_pathbar;
        self.clamp_scroll();
        self.dirty = true;
        vec![Action::SaveOptions]
    }

    pub fn sort_by(&mut self, c: Column) -> Vec<Action> {
        let s = self.list.sort.clicked(c);
        self.list.set_sort(s);
        self.dirty = true;
        vec![Action::SaveOptions]
    }

    // -- paths ------------------------------------------------------------

    fn path_of(&self, e: &Entry) -> String {
        model::join(&self.path, &e.name)
    }

    pub fn selected_paths(&self) -> Vec<String> {
        self.list.selected_entries().map(|e| self.path_of(e)).collect()
    }

    pub fn crumbs(&self) -> Vec<(String, String)> {
        model::crumbs(&self.path)
            .into_iter()
            .map(|(p, n)| {
                let n = self.root_name(&p).map(str::to_string).unwrap_or(n);
                (p, n)
            })
            .collect()
    }

    /// A name for a new folder that is not taken: «Новая папка», «Новая папка (2)».
    pub fn new_folder_name(&self) -> String {
        let base = self.strings.new_folder_name;
        let taken = |n: &str| self.list.entries.iter().any(|e| e.lower == n.to_lowercase());
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} ({i})")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
    }

    pub fn new_folder(&mut self) -> Vec<Action> {
        if self.load != Load::Done {
            return Vec::new();
        }
        let name = self.new_folder_name();
        self.select_when_seen = Some((name.clone(), true));
        vec![Action::NewFolder { path: self.path_of(&Entry::new(name, true, 0, 0, 0)) }]
    }

    /// Opens the selection: one folder in place, files with their programs,
    /// several folders in new windows.
    pub fn open_selection(&mut self, new_window: bool) -> Vec<Action> {
        let sel: Vec<Entry> = self.list.selected_entries().cloned().collect();
        if sel.len() == 1 && sel[0].dir && !new_window {
            let p = self.path_of(&sel[0]);
            return self.navigate(&p);
        }
        let mut out = Vec::new();
        let files: Vec<String> = sel.iter().filter(|e| !e.dir).map(|e| self.path_of(e)).collect();
        if !files.is_empty() {
            out.push(Action::OpenFiles(files));
        }
        for e in sel.iter().filter(|e| e.dir).take(8) {
            out.push(Action::NewWindow(self.path_of(e)));
        }
        out
    }

    // -- scrolling --------------------------------------------------------

    pub fn content_height(&self) -> f32 {
        let f = self.frame();
        match self.view {
            View::Icons => Grid::new(f.content.w, self.scale).height(self.list.len()),
            View::List => layout::list_height(self.list.len(), self.scale),
        }
    }

    fn max_scroll(&self) -> f32 {
        (self.content_height() - self.frame().content.h).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        let m = self.max_scroll();
        self.target = self.target.clamp(0.0, m);
        self.scroll = self.scroll.clamp(0.0, m);
    }

    pub fn scroll_y(&self) -> f32 {
        self.scroll.round()
    }

    /// Wheel input in pixels (positive scrolls down). Notches animate;
    /// touchpad deltas apply at once so the content follows the fingers.
    pub fn wheel(&mut self, dy: f32, smooth: bool) {
        self.target = (self.target + dy).clamp(0.0, self.max_scroll());
        if !smooth {
            self.scroll = self.target;
        }
        self.scrollbar_t = SCROLLBAR_LINGER;
        self.slow_rename = None;
        self.dirty = true;
    }

    /// Scrolls the least amount that shows item `pos` whole.
    pub fn ensure_visible(&mut self, pos: usize) {
        let f = self.frame();
        let (top, bottom) = match self.view {
            View::Icons => {
                let g = Grid::new(f.content.w, self.scale);
                let c = g.cell(pos);
                (c.y - g.pad.min(c.y), c.bottom())
            }
            View::List => {
                let y = layout::row_y(pos, self.scale);
                (y - layout::LIST_PAD * self.scale, y + layout::ROW_H * self.scale + layout::LIST_PAD * self.scale)
            }
        };
        if top < self.target {
            self.target = top;
        } else if bottom > self.target + f.content.h {
            self.target = bottom - f.content.h;
        }
        self.clamp_scroll();
        self.dirty = true;
    }

    /// Thumb of the overlay scrollbar in window pixels, when there is one.
    pub fn scrollbar(&self) -> Option<Rect> {
        let f = self.frame();
        let total = self.content_height();
        if total <= f.content.h + 0.5 || f.content.h <= 0.0 {
            return None;
        }
        let s = self.scale;
        let inset = 3.0 * s;
        let track = f.content.h - 2.0 * inset;
        let h = (track * f.content.h / total).max(24.0 * s).min(track);
        let y = f.content.y + inset + (track - h) * (self.scroll / (total - f.content.h)).clamp(0.0, 1.0);
        let wide = matches!(self.hover, Hit::Scrollbar) || matches!(self.press, Some(Press::Scrollbar { .. }));
        let w = if wide { layout::SCROLLBAR_WIDE } else { layout::SCROLLBAR_W } * s;
        Some(Rect::new(f.content.right() - w - 3.0 * s, y, w, h))
    }

    pub fn scrollbar_alpha(&self) -> f32 {
        if matches!(self.hover, Hit::Scrollbar) || matches!(self.press, Some(Press::Scrollbar { .. })) {
            return 1.0;
        }
        (self.scrollbar_t / 0.3).clamp(0.0, 1.0)
    }

    // -- time -------------------------------------------------------------

    pub fn animating(&self) -> bool {
        (self.scroll - self.target).abs() > 0.5
            || self.scrollbar_t > 0.0
            || self.search_focus
            || self.rename.is_some()
            || self.slow_rename.is_some()
            || matches!(self.press, Some(Press::Band { .. }))
    }

    /// Advances animations by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        if (self.scroll - self.target).abs() > 0.5 {
            // Critically damped approach: most of the distance in ~80 ms.
            let k = 1.0 - (-dt * 22.0).exp();
            self.scroll += (self.target - self.scroll) * k;
            self.dirty = true;
        } else if self.scroll != self.target {
            self.scroll = self.target;
            self.dirty = true;
        }
        if self.scrollbar_t > 0.0 {
            self.scrollbar_t = (self.scrollbar_t - dt).max(0.0);
            self.dirty = true;
        }
        if self.search_focus || self.rename.is_some() {
            // Only the caret blinks; redraw on its phase changes.
            let before = ((self.time - dt - self.caret_since) / 0.53) as i64;
            let after = ((self.time - self.caret_since) / 0.53) as i64;
            if before != after {
                self.dirty = true;
            }
        }
        if let Some((pos, at)) = self.slow_rename
            && self.time >= at
        {
            self.slow_rename = None;
            if self.list.selected_count() == 1 && self.list.is_selected(pos) {
                self.begin_rename(pos);
            }
        }
        if let Some(Press::Band { .. }) = &self.press {
            self.autoscroll_band(dt);
        }
    }

    fn autoscroll_band(&mut self, dt: f32) {
        let f = self.frame();
        let Some(Press::Band { y1, .. }) = &self.press else { return };
        let py = *y1 - self.scroll + f.content.y;
        let edge = 24.0 * self.scale;
        let v = if py < f.content.y + edge {
            -(f.content.y + edge - py)
        } else if py > f.content.bottom() - edge {
            py - (f.content.bottom() - edge)
        } else {
            0.0
        };
        if v != 0.0 {
            let d = v * 12.0 * dt;
            let before = self.scroll;
            self.target = (self.target + d).clamp(0.0, self.max_scroll());
            self.scroll = self.target;
            let moved = self.scroll - before;
            if let Some(Press::Band { y1, .. }) = &mut self.press {
                *y1 += moved;
            }
            self.update_band();
        }
    }

    // -- hit testing ------------------------------------------------------

    fn label_size(&self, pos: usize, grid: &Grid, text: &Text) -> (f32, f32) {
        let Some(e) = self.list.entry(pos) else { return (0.0, 0.0) };
        let s = self.scale;
        let max_w = grid.pitch_x - 8.0 * s;
        let full = text.measure(&e.name, layout::LABEL_FONT * s, Weight::Regular);
        let w = full.min(max_w) + 8.0 * s;
        let lines = if full > max_w { 2.0 } else { 1.0 };
        (w, lines * layout::LABEL_LINE * s)
    }

    /// What is under window pixel `x, y`.
    pub fn hit(&self, x: f32, y: f32, text: &Text) -> Hit {
        let f = self.frame();
        let s = self.scale;
        for i in 0..3 {
            let r = f.light(i);
            let pad = 3.0 * s;
            if Rect::new(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad).contains(x, y) {
                return Hit::Light(i);
            }
        }
        if f.sidebar.contains(x, y) {
            return self.hit_sidebar(x, y);
        }
        if x < f.sidebar.right() {
            return Hit::Caption;
        }
        if f.toolbar.contains(x, y) {
            if f.nav.contains(x, y) {
                return if x < f.nav.x + f.nav.w / 2.0 { Hit::Back } else { Hit::Forward };
            }
            if f.view_seg.contains(x, y) {
                return if x < f.view_seg.x + f.view_seg.w / 2.0 { Hit::ViewIcons } else { Hit::ViewList };
            }
            if f.new_folder.contains(x, y) {
                return Hit::NewFolder;
            }
            if f.search.contains(x, y) {
                let clear = !self.search.is_empty() && x > f.search.right() - f.search.h;
                return if clear { Hit::SearchClear } else { Hit::Search };
            }
            return Hit::Caption;
        }
        if f.header.h > 0.0 && f.header.contains(x, y) {
            return Columns::new(f.content.w, s).at(x - f.content.x).map(Hit::Header).unwrap_or(Hit::Blank);
        }
        if f.pathbar.h > 0.0 && f.pathbar.contains(x, y) {
            return self.hit_crumb(x, text).map(Hit::Crumb).unwrap_or(Hit::None);
        }
        if f.status.contains(x, y) {
            return Hit::None;
        }
        if f.content.contains(x, y) {
            if let Some(t) = self.scrollbar()
                && x >= t.x - 4.0 * s
            {
                return Hit::Scrollbar;
            }
            let (cx, cy) = (x - f.content.x, y - f.content.y + self.scroll_y());
            let n = self.list.len();
            let hit = match self.view {
                View::Icons => {
                    let g = Grid::new(f.content.w, s);
                    g.hit(cx, cy, n, |i| self.label_size(i, &g, text))
                }
                View::List => layout::row_at(cy, n, s),
            };
            return hit.map(Hit::Item).unwrap_or(Hit::Blank);
        }
        Hit::None
    }

    /// Sidebar rows: (section, index, row center y).
    pub fn sidebar_rows(&self) -> Vec<(usize, usize, f32)> {
        let s = self.scale;
        let f = self.frame();
        let mut out = Vec::new();
        let mut y = f.sidebar.y + (layout::SB_HEADER_CY - layout::INSET) * s;
        for (si, sec) in self.sections.iter().enumerate() {
            if si > 0 {
                y += layout::SB_ROW_TO_HEADER * s;
            }
            let mut ry = y + layout::SB_HEADER_TO_ROW * s;
            for pi in 0..sec.places.len() {
                out.push((si, pi, ry));
                y = ry;
                ry += layout::SB_ROW_H * s;
            }
        }
        out
    }

    /// Header center y of each sidebar section.
    pub fn section_headers(&self) -> Vec<f32> {
        let s = self.scale;
        let f = self.frame();
        let mut out = Vec::new();
        let mut y = f.sidebar.y + (layout::SB_HEADER_CY - layout::INSET) * s;
        for (si, sec) in self.sections.iter().enumerate() {
            if si > 0 {
                y += layout::SB_ROW_TO_HEADER * s;
            }
            out.push(y);
            if !sec.places.is_empty() {
                y += layout::SB_HEADER_TO_ROW * s + (sec.places.len() - 1) as f32 * layout::SB_ROW_H * s;
            }
        }
        out
    }

    fn hit_sidebar(&self, x: f32, y: f32) -> Hit {
        let s = self.scale;
        let f = self.frame();
        for (si, pi, cy) in self.sidebar_rows() {
            if (y - cy).abs() < layout::SB_ROW_H * s / 2.0 {
                let place = &self.sections[si].places[pi];
                if place.eject && x > f.sidebar.right() - 34.0 * s {
                    return Hit::Eject(si, pi);
                }
                return Hit::Place(si, pi);
            }
        }
        Hit::Caption
    }

    /// Path bar items as (x, width) in window pixels.
    pub fn crumb_boxes(&self, text: &Text) -> Vec<(f32, f32)> {
        let f = self.frame();
        let s = self.scale;
        let mut x = f.pathbar.x + 12.0 * s;
        let mut out = Vec::new();
        for (_, name) in self.crumbs() {
            let w = (layout::STATUS_FONT + 4.0) * s + text.measure(&name, layout::STATUS_FONT * s, Weight::Regular).min(220.0 * s);
            out.push((x, w));
            x += w + 18.0 * s;
        }
        // Too long for the bar: the front of the path slides out on the left.
        let over = x - f.pathbar.right();
        if over > 0.0 {
            for b in &mut out {
                b.0 -= over;
            }
        }
        out
    }

    fn hit_crumb(&self, x: f32, text: &Text) -> Option<usize> {
        self.crumb_boxes(text).iter().position(|(cx, w)| x >= *cx && x < cx + w)
    }

    // -- mouse ------------------------------------------------------------

    pub fn mouse_move(&mut self, x: f32, y: f32, text: &Text) -> Vec<Action> {
        let f = self.frame();
        let lit = f.lights().contains(x, y);
        if lit != self.lights_hover {
            self.lights_hover = lit;
            self.dirty = true;
        }
        let h = self.hit(x, y, text);
        if h != self.hover {
            self.hover = h;
            self.dirty = true;
        }
        let s = self.scale;
        match self.press.clone() {
            Some(Press::Item { pos, x: px, y: py, .. }) => {
                if (x - px).hypot(y - py) > DRAG_START * s {
                    self.press = None;
                    self.slow_rename = None;
                    if !self.list.is_selected(pos) {
                        self.list.select_only(pos);
                    }
                    let paths = self.selected_paths();
                    self.dirty = true;
                    if !paths.is_empty() {
                        return vec![Action::DragOut(paths)];
                    }
                }
            }
            Some(Press::Band { .. }) => {
                if let Some(Press::Band { x1, y1, .. }) = &mut self.press {
                    *x1 = (x - f.content.x).clamp(0.0, f.content.w);
                    *y1 = y - f.content.y + self.scroll;
                }
                self.update_band();
            }
            Some(Press::Scrollbar { grab }) => {
                if let Some(t) = self.scrollbar() {
                    let inset = 3.0 * s;
                    let track = f.content.h - 2.0 * inset - t.h;
                    if track > 0.0 {
                        let k = ((y - grab - f.content.y - inset) / track).clamp(0.0, 1.0);
                        self.target = k * self.max_scroll();
                        self.scroll = self.target;
                        self.scrollbar_t = SCROLLBAR_LINGER;
                        self.dirty = true;
                    }
                }
            }
            Some(Press::Text { rename }) => {
                let fx = self.field_x(rename);
                let fs = if rename { self.rename_font() } else { layout::SEARCH_FONT * s };
                let field = if rename { self.rename.as_mut().map(|r| &mut r.field) } else { Some(&mut self.search) };
                if let (Some(field), Some(fx)) = (field, fx) {
                    let i = field.index_at(x - fx, |t| text.measure(t, fs, Weight::Regular));
                    field.caret = i;
                    self.dirty = true;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn update_band(&mut self) {
        let f = self.frame();
        let Some(Press::Band { x0, y0, x1, y1, base }) = &self.press else { return };
        let band = Rect::span(*x0, *y0, *x1, *y1);
        let n = self.list.len();
        let inside: Vec<usize> = match self.view {
            View::Icons => Grid::new(f.content.w, self.scale).in_band(&band, n),
            View::List => {
                let cols = Columns::new(f.content.w, self.scale);
                let name = cols.cols[0];
                if band.x > name.1 + name.2 {
                    Vec::new()
                } else {
                    layout::row_range(band.y, band.bottom(), n, self.scale).collect()
                }
            }
        };
        let base = base.clone();
        self.list.band(&base, &inside);
        self.dirty = true;
    }

    pub fn mouse_leave(&mut self) {
        if self.hover != Hit::None || self.lights_hover {
            self.hover = Hit::None;
            self.lights_hover = false;
            self.dirty = true;
        }
    }

    /// Left button down. `double` is the second click of a double-click.
    pub fn mouse_down(&mut self, x: f32, y: f32, mods: Mods, double: bool, text: &Text) -> Vec<Action> {
        let mut out = Vec::new();
        let hit = self.hit(x, y, text);
        self.hover = hit;
        self.dirty = true;
        let in_field = |h: Hit| matches!(h, Hit::Search | Hit::SearchClear);
        // A click anywhere else commits the rename (Finder does the same).
        if self.rename.is_some() && !self.rename_box_contains(x, y, text) {
            out.extend(self.commit_rename());
        } else if self.rename.is_some() {
            let fs = self.rename_font();
            if let Some(fx) = self.field_x(true)
                && let Some(r) = self.rename.as_mut()
            {
                let i = r.field.index_at(x - fx, |t| text.measure(t, fs, Weight::Regular));
                if double {
                    r.field.select_all();
                } else {
                    r.field.select(if mods.shift { r.field.anchor } else { i }, i);
                }
                self.press = Some(Press::Text { rename: true });
            }
            return out;
        }
        if !in_field(hit) && self.search_focus {
            self.search_focus = false;
        }
        self.slow_rename = None;
        match hit {
            Hit::Item(pos) => {
                let solo = self.list.selected_count() == 1 && self.list.is_selected(pos);
                if double {
                    if !self.list.is_selected(pos) {
                        self.list.select_only(pos);
                    }
                    out.extend(self.open_selection(mods.ctrl));
                    self.press = None;
                    return out;
                }
                let mut narrow = false;
                if mods.ctrl || mods.shift {
                    self.list.click(pos, mods.ctrl, mods.shift);
                } else if !self.list.is_selected(pos) {
                    self.list.select_only(pos);
                } else {
                    narrow = self.list.selected_count() > 1;
                }
                let label = self.on_label(pos, x, y, text);
                self.press = Some(Press::Item { pos, x, y, solo, label, narrow });
            }
            Hit::Blank => {
                let f = self.frame();
                let (cx, cy) = (x - f.content.x, y - f.content.y + self.scroll);
                let base = if mods.ctrl || mods.shift { self.list.selected_positions() } else { Vec::new() };
                if !(mods.ctrl || mods.shift) {
                    self.list.select_none();
                }
                if double {
                    return out;
                }
                self.press = Some(Press::Band { x0: cx, y0: cy, x1: cx, y1: cy, base });
            }
            Hit::Scrollbar => {
                if let Some(t) = self.scrollbar() {
                    let grab = if t.contains(t.x + 1.0, y) { y - t.y } else { t.h / 2.0 };
                    self.press = Some(Press::Scrollbar { grab });
                    out.extend(self.mouse_move(x, y, text));
                }
            }
            Hit::Search => {
                if !self.search_focus {
                    self.search_focus = true;
                    self.caret_since = self.time;
                    self.search.select_all();
                } else if let Some(fx) = self.field_x(false) {
                    let i = self.search.index_at(x - fx, |t| text.measure(t, layout::SEARCH_FONT * self.scale, Weight::Regular));
                    if double {
                        self.search.select_all();
                    } else {
                        self.search.select(if mods.shift { self.search.anchor } else { i }, i);
                    }
                    self.caret_since = self.time;
                    self.press = Some(Press::Text { rename: false });
                }
            }
            Hit::Caption | Hit::None => {}
            other => self.press = Some(Press::Button(other)),
        }
        out
    }

    pub fn mouse_up(&mut self, x: f32, y: f32, text: &Text) -> Vec<Action> {
        let press = self.press.take();
        let hit = self.hit(x, y, text);
        self.dirty = true;
        match press {
            Some(Press::Item { pos, solo, label, narrow, .. }) => {
                if hit == Hit::Item(pos) {
                    if narrow {
                        self.list.select_only(pos);
                    } else if solo && label {
                        // A second, slow click on the name of the only selected item renames it.
                        self.slow_rename = Some((pos, self.time + SLOW_CLICK));
                    }
                }
                Vec::new()
            }
            Some(Press::Band { .. }) => Vec::new(),
            Some(Press::Button(b)) if b == hit => self.click(b),
            _ => Vec::new(),
        }
    }

    fn click(&mut self, b: Hit) -> Vec<Action> {
        match b {
            Hit::Light(0) => vec![Action::Close],
            Hit::Light(1) => vec![Action::Minimize],
            Hit::Light(_) => vec![Action::Zoom],
            Hit::Back => self.back(),
            Hit::Forward => self.forward(),
            Hit::ViewIcons => self.set_view(View::Icons),
            Hit::ViewList => self.set_view(View::List),
            Hit::NewFolder => self.new_folder(),
            Hit::SearchClear => {
                self.search = Field::default();
                self.list.set_filter("");
                self.search_focus = true;
                self.caret_since = self.time;
                self.clamp_scroll();
                Vec::new()
            }
            Hit::Place(si, pi) => {
                let p = self.sections[si].places[pi].path.clone();
                self.navigate(&p)
            }
            Hit::Eject(si, pi) => vec![Action::Eject(self.sections[si].places[pi].path.clone())],
            Hit::Header(c) => self.sort_by(c),
            Hit::Crumb(i) => {
                let c = self.crumbs();
                match c.get(i) {
                    Some((p, _)) if i + 1 < c.len() => {
                        let p = p.clone();
                        self.navigate(&p)
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    pub fn right_click(&mut self, x: f32, y: f32, text: &Text) -> Vec<Action> {
        let mut out = Vec::new();
        if self.rename.is_some() {
            out.extend(self.commit_rename());
        }
        self.press = None;
        self.slow_rename = None;
        self.dirty = true;
        match self.hit(x, y, text) {
            Hit::Item(pos) => {
                if !self.list.is_selected(pos) {
                    self.list.select_only(pos);
                }
                out.push(Action::ContextMenu { paths: self.selected_paths(), x, y });
            }
            Hit::Blank => {
                self.list.select_none();
                out.push(Action::ContextMenu { paths: Vec::new(), x, y });
            }
            Hit::Place(si, pi) => {
                let p = self.sections[si].places[pi].path.clone();
                if !p.starts_with("shell:") {
                    out.push(Action::ContextMenu { paths: vec![p], x, y });
                }
            }
            _ => {}
        }
        out
    }

    fn on_label(&self, pos: usize, x: f32, y: f32, text: &Text) -> bool {
        let f = self.frame();
        let s = self.scale;
        let (cx, cy) = (x - f.content.x, y - f.content.y + self.scroll_y());
        match self.view {
            View::Icons => {
                let g = Grid::new(f.content.w, s);
                cy > g.tile(pos).bottom() && g.hit(cx, cy, self.list.len(), |i| self.label_size(i, &g, text)) == Some(pos)
            }
            View::List => {
                let x0 = layout::ROW_INSET * s + (layout::COL_PAD + layout::LIST_ICON + 6.0) * s;
                let w = self.list.entry(pos).map(|e| text.measure(&e.name, layout::ROW_FONT * s, Weight::Regular)).unwrap_or(0.0);
                cx >= x0 && cx <= x0 + w
            }
        }
    }

    // -- fields -----------------------------------------------------------

    /// Left edge of the text in the search field (rename = false) or the
    /// rename box, in window pixels.
    pub fn field_x(&self, rename: bool) -> Option<f32> {
        let f = self.frame();
        let s = self.scale;
        if !rename {
            return Some(f.search.x + 30.0 * s);
        }
        self.rename_box(None).map(|r| r.x + 3.0 * s)
    }

    /// The rename box in window pixels; its width follows the text (`text`
    /// None: only its position is wanted).
    pub fn rename_box(&self, text: Option<&Text>) -> Option<Rect> {
        let width = |t: &str| text.map(|tx| tx.measure(t, self.rename_font(), Weight::Regular)).unwrap_or(0.0);
        let r = self.rename.as_ref()?;
        let pos = self.list.position_of(&r.name)?;
        let f = self.frame();
        let s = self.scale;
        let sy = self.scroll_y();
        match self.view {
            View::List => {
                let x = f.content.x + layout::ROW_INSET * s + (layout::COL_PAD + layout::LIST_ICON + 3.0) * s;
                let y = f.content.y + layout::row_y(pos, s) - sy;
                let cols = Columns::new(f.content.w, s);
                let max = cols.cols[0].1 + cols.cols[0].2 - (x - f.content.x) - 4.0 * s;
                let w = (width(&r.field.text()) + 12.0 * s).clamp(60.0 * s, max.max(60.0 * s));
                Some(Rect::new(x, y + 1.0 * s, w, layout::ROW_H * s - 2.0 * s))
            }
            View::Icons => {
                let g = Grid::new(f.content.w, s);
                let c = g.cell(pos);
                let t = g.tile(pos);
                let w = (width(&r.field.text()) + 10.0 * s).clamp(60.0 * s, g.pitch_x + 40.0 * s);
                Some(Rect::new(f.content.x + c.x + (c.w - w) / 2.0, f.content.y + t.bottom() + layout::LABEL_GAP * s - sy, w, (layout::LABEL_LINE + 4.0) * s))
            }
        }
    }

    fn rename_box_contains(&self, x: f32, y: f32, text: &Text) -> bool {
        self.rename_box(Some(text)).is_some_and(|r| r.contains(x, y))
    }

    fn rename_font(&self) -> f32 {
        let pt = if self.view == View::List { layout::ROW_FONT } else { layout::LABEL_FONT };
        pt * self.scale
    }

    pub fn begin_rename(&mut self, pos: usize) {
        let Some(e) = self.list.entry(pos) else { return };
        let name = e.name.clone();
        let mut field = Field::new(&name);
        let stem = if e.dir { field.len() } else { model::stem_len(&name) };
        field.select(0, stem);
        self.rename = Some(Rename { name, field });
        self.search_focus = false;
        self.caret_since = self.time;
        self.ensure_visible(pos);
        self.dirty = true;
    }

    pub fn commit_rename(&mut self) -> Vec<Action> {
        let Some(r) = self.rename.take() else { return Vec::new() };
        self.dirty = true;
        let to = r.field.text().trim().to_string();
        if to == r.name || !valid_name(&to) {
            return Vec::new();
        }
        self.select_when_seen = Some((to.clone(), false));
        vec![Action::Rename { path: model::join(&self.path, &r.name), to }]
    }

    pub fn cancel_rename(&mut self) {
        if self.rename.take().is_some() {
            self.dirty = true;
        }
    }

    fn filter_changed(&mut self) {
        let t = self.search.text();
        self.list.set_filter(&t);
        self.target = 0.0;
        self.scroll = 0.0;
        if !self.list.is_empty() && self.list.selected_count() == 0 && !t.is_empty() {
            self.list.cursor = None;
        }
        self.dirty = true;
    }

    /// Paste of plain text into whichever field has focus.
    pub fn paste_text(&mut self, text: &str) -> bool {
        let line = text.lines().next().unwrap_or("");
        if let Some(r) = self.rename.as_mut() {
            r.field.insert(line);
        } else if self.search_focus {
            self.search.insert(line);
            self.filter_changed();
        } else {
            return false;
        }
        self.caret_since = self.time;
        self.dirty = true;
        true
    }

    /// Whether a text field has keyboard focus (the host then routes Ctrl+C/V
    /// as text, not files).
    pub fn editing(&self) -> bool {
        self.rename.is_some() || self.search_focus
    }

    pub fn field_selection_text(&self) -> Option<String> {
        if let Some(r) = &self.rename {
            return Some(r.field.selected_text());
        }
        self.search_focus.then(|| self.search.selected_text())
    }

    fn field_key(&mut self, key: u32, mods: Mods) -> Option<Vec<Action>> {
        let rename = self.rename.is_some();
        if !rename && !self.search_focus {
            return None;
        }
        let mut out = Vec::new();
        let mut changed = false;
        {
            let field = if rename { &mut self.rename.as_mut()?.field } else { &mut self.search };
            match key {
                vk::LEFT => field.left(mods.shift, mods.ctrl),
                vk::RIGHT => field.right(mods.shift, mods.ctrl),
                vk::HOME => field.home(mods.shift),
                vk::END => field.end(mods.shift),
                vk::BACK => {
                    field.backspace(mods.ctrl);
                    changed = true;
                }
                vk::DELETE => {
                    field.delete(mods.ctrl);
                    changed = true;
                }
                vk::A if mods.ctrl => field.select_all(),
                vk::X if mods.ctrl => {
                    // The host copies the selection first (field_selection_text).
                    field.insert("");
                    changed = true;
                }
                vk::RETURN | vk::ESCAPE | vk::TAB | vk::UP | vk::DOWN | vk::PRIOR | vk::NEXT => {}
                vk::C | vk::V if mods.ctrl => return Some(out),
                _ if mods.ctrl || mods.alt => return None,
                _ => return Some(out),
            }
        }
        self.caret_since = self.time;
        self.dirty = true;
        match key {
            vk::RETURN if rename => out.extend(self.commit_rename()),
            vk::ESCAPE if rename => self.cancel_rename(),
            vk::TAB if rename => out.extend(self.commit_rename()),
            vk::RETURN => {
                // A typed path goes there, like Finder's Go to Folder.
                let t = self.search.text();
                let t = t.trim();
                if looks_like_path(t) {
                    let p = t.to_string();
                    return Some(self.navigate(&p));
                }
                self.search_focus = false;
                if self.list.selected_count() == 0 && !self.list.is_empty() {
                    self.list.select_only(0);
                }
                out.extend(self.open_selection(false));
            }
            vk::ESCAPE => {
                if self.search.is_empty() {
                    self.search_focus = false;
                } else {
                    self.search = Field::default();
                    self.filter_changed();
                }
            }
            vk::TAB | vk::DOWN if !rename => {
                self.search_focus = false;
                if !self.list.is_empty() {
                    self.list.select_only(0);
                    self.ensure_visible(0);
                }
            }
            vk::UP | vk::DOWN | vk::PRIOR | vk::NEXT => {}
            _ if changed && !rename => self.filter_changed(),
            _ => {}
        }
        Some(out)
    }

    // -- keyboard ---------------------------------------------------------

    /// A key press; returns None when the browser does not use the key (the
    /// host then lets Windows have it, e.g. Alt+F4 or Alt+Space).
    pub fn key(&mut self, key: u32, mods: Mods) -> Option<Vec<Action>> {
        if let Some(r) = self.field_key(key, mods) {
            return Some(r);
        }
        self.dirty = true;
        let n = self.list.len();
        let cursor = self.list.cursor;
        let ctrl_only = mods.ctrl && !mods.alt;
        let r = match key {
            vk::LEFT if mods.alt => self.back(),
            vk::RIGHT if mods.alt => self.forward(),
            vk::UP if mods.alt => self.up(),
            vk::UP if ctrl_only => self.up(),
            vk::DOWN if ctrl_only => self.open_selection(false),
            vk::BACK if !mods.alt => self.back(),
            vk::RETURN if mods.alt => return None,
            vk::RETURN => self.open_selection(mods.ctrl),
            vk::F2 => {
                if let Some(c) = cursor.filter(|&c| self.list.is_selected(c)).or_else(|| self.list.selected_positions().first().copied()) {
                    self.begin_rename(c);
                }
                Vec::new()
            }
            vk::F5 => vec![Action::Refresh],
            vk::DELETE if !mods.alt => {
                let paths = self.selected_paths();
                if paths.is_empty() { Vec::new() } else { vec![Action::Delete { paths, permanent: mods.shift }] }
            }
            vk::A if ctrl_only => {
                self.list.select_all();
                Vec::new()
            }
            vk::C | vk::X if ctrl_only => {
                let paths = self.selected_paths();
                if paths.is_empty() { Vec::new() } else { vec![Action::Clipboard { paths, cut: key == vk::X }] }
            }
            vk::V if ctrl_only => vec![Action::Paste { into: self.path.clone() }],
            vk::N if ctrl_only && mods.shift => self.new_folder(),
            vk::N if ctrl_only => vec![Action::NewWindow(self.path.clone())],
            vk::W if ctrl_only => vec![Action::Close],
            vk::F | vk::L if ctrl_only => {
                self.search_focus = true;
                self.caret_since = self.time;
                if key == vk::L && self.search.is_empty() {
                    self.search.set_text(&self.path);
                }
                self.search.select_all();
                Vec::new()
            }
            vk::KEY_1 if ctrl_only => self.set_view(View::Icons),
            vk::KEY_2 if ctrl_only => self.set_view(View::List),
            vk::OEM_PERIOD if ctrl_only && mods.shift => self.toggle_hidden(),
            vk::ESCAPE => {
                self.list.select_none();
                Vec::new()
            }
            vk::HOME | vk::END | vk::PRIOR | vk::NEXT | vk::UP | vk::DOWN | vk::LEFT | vk::RIGHT if n > 0 && !mods.alt => {
                let to = self.key_target(key, cursor, n);
                self.list.move_to(to, mods.shift);
                self.ensure_visible(to);
                Vec::new()
            }
            _ => return None,
        };
        Some(r)
    }

    fn key_target(&self, key: u32, cursor: Option<usize>, n: usize) -> usize {
        let f = self.frame();
        let Some(c) = cursor else {
            return if matches!(key, vk::END | vk::UP | vk::LEFT) { n - 1 } else { 0 };
        };
        let page_rows = match self.view {
            View::List => ((f.content.h / (layout::ROW_H * self.scale)) as usize).saturating_sub(1).max(1),
            View::Icons => ((f.content.h / (layout::CELL_H * self.scale)) as usize).max(1),
        };
        match self.view {
            View::List => match key {
                vk::HOME => 0,
                vk::END => n - 1,
                vk::UP => c.saturating_sub(1),
                vk::DOWN => (c + 1).min(n - 1),
                vk::PRIOR => c.saturating_sub(page_rows),
                vk::NEXT => (c + page_rows).min(n - 1),
                _ => c,
            },
            View::Icons => {
                let g = Grid::new(f.content.w, self.scale);
                match key {
                    vk::HOME => 0,
                    vk::END => n - 1,
                    vk::UP => g.step(c, n, 0, -1),
                    vk::DOWN => g.step(c, n, 0, 1),
                    vk::LEFT => g.step(c, n, -1, 0),
                    vk::RIGHT => g.step(c, n, 1, 0),
                    vk::PRIOR => g.step(c, n, 0, -(page_rows as i32)),
                    vk::NEXT => g.step(c, n, 0, page_rows as i32),
                    _ => c,
                }
            }
        }
    }

    /// Typed text: goes to a focused field, else selects by name.
    pub fn char(&mut self, c: char) -> Vec<Action> {
        if c.is_control() {
            return Vec::new();
        }
        self.dirty = true;
        self.caret_since = self.time;
        if let Some(r) = self.rename.as_mut() {
            let mut b = [0u8; 4];
            r.field.insert(c.encode_utf8(&mut b));
            return Vec::new();
        }
        if self.search_focus {
            let mut b = [0u8; 4];
            self.search.insert(c.encode_utf8(&mut b));
            self.filter_changed();
            return Vec::new();
        }
        if c == ' ' && self.type_buf.is_empty() {
            return Vec::new();
        }
        if self.time - self.type_at > TYPE_RESET {
            self.type_buf.clear();
        }
        self.type_at = self.time;
        self.type_buf.extend(c.to_lowercase());
        if let Some(p) = self.list.find_prefix(&self.type_buf) {
            self.list.select_only(p);
            self.ensure_visible(p);
        }
        Vec::new()
    }

    // -- drag and drop ----------------------------------------------------

    /// A drag from another app (or this window) moves over window pixel
    /// `x, y`: returns the folder a drop would go into.
    pub fn drag_over(&mut self, x: f32, y: f32, text: &Text) -> Option<String> {
        let (target, path) = match self.hit(x, y, text) {
            Hit::Item(pos) if self.list.entry(pos).is_some_and(|e| e.dir) => {
                (Some(Some(pos)), self.list.entry(pos).map(|e| self.path_of(e)))
            }
            Hit::Place(si, pi) if !self.sections[si].places[pi].path.starts_with("shell:") => {
                (None, Some(self.sections[si].places[pi].path.clone()))
            }
            Hit::Item(_) | Hit::Blank | Hit::Scrollbar if self.load == Load::Done => (Some(None), Some(self.path.clone())),
            _ => (None, None),
        };
        if self.drop_hover != target {
            self.drop_hover = target;
            self.dirty = true;
        }
        path
    }

    pub fn drag_leave(&mut self) {
        if self.drop_hover.is_some() {
            self.drop_hover = None;
            self.dirty = true;
        }
    }

    // -- icons ------------------------------------------------------------

    /// The icon cache key of an entry at the current view size, and whether
    /// it wants a thumbnail. Folders use our own art (None).
    pub fn icon_key(&self, e: &Entry) -> Option<(String, u32, bool)> {
        if e.dir {
            return None;
        }
        let px = match self.view {
            View::Icons => layout::ICON,
            View::List => layout::LIST_ICON,
        } * self.scale;
        let sizes: &[u32] = if self.view == View::Icons { &[48, 64, 96, 128, 256] } else { &[16, 20, 24, 32, 48] };
        let size = sizes.iter().copied().find(|&s| s as f32 >= px).unwrap_or(*sizes.last().unwrap_or(&256));
        let ext = e.ext.as_str();
        if self.view == View::Icons && THUMBS.contains(&ext) {
            let key = format!("t{size}:{}", model::join(&self.path, &e.lower));
            if !matches!(self.icons.get(&key), Some(IconSlot::Missing)) {
                return Some((key, size, true));
            }
        }
        if OWN_ICON.contains(&ext) {
            return Some((format!("p{size}:{}", model::join(&self.path, &e.lower)), size, false));
        }
        Some((format!("e{size}:{ext}"), size, false))
    }

    /// Icons the visible items need that nobody has asked for yet; they are
    /// marked pending.
    pub fn icon_requests(&mut self) -> Vec<IconRequest> {
        let f = self.frame();
        let sy = self.scroll_y();
        let n = self.list.len();
        let range = match self.view {
            View::Icons => Grid::new(f.content.w, self.scale).range(sy, sy + f.content.h, n),
            View::List => layout::row_range(sy, sy + f.content.h, n, self.scale),
        };
        let mut out = Vec::new();
        for pos in range {
            let Some(e) = self.list.entry(pos) else { continue };
            let Some((key, size, thumbnail)) = self.icon_key(e) else { continue };
            if !self.icons.contains_key(&key) {
                out.push(IconRequest { key: key.clone(), path: self.path_of(e), size, thumbnail });
                self.icons.insert(key, IconSlot::Pending);
            }
        }
        out
    }

    /// An icon arrived (None: the shell had none).
    pub fn icon_loaded(&mut self, key: String, img: Option<Rc<Image>>) {
        self.icons.insert(key, img.map(IconSlot::Ready).unwrap_or(IconSlot::Missing));
        self.dirty = true;
    }
}

/// "C:\x", "\\server\share", "D:" — text the search field treats as a path.
pub fn looks_like_path(t: &str) -> bool {
    let b = t.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/')) || t.starts_with("\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::model::Entry;

    fn browser() -> Browser {
        let mut b = Browser::new(&FileStrings::EN, Rc::new(Art::new()), 1000.0, 600.0, 1.0);
        let _ = b.navigate("C:\\Users\\me");
        b.loaded((0..50).map(|i| Entry::new(format!("file{i}.txt"), false, i, 0, 0)).collect(), Vec::new(), true, Some(1));
        b
    }

    thread_local! {
        static TEXT: Text = Text::bundled();
    }

    /// Runs `f` with the bundled font, standing in for the host's text.
    fn with_text<R>(f: impl FnOnce(&Text) -> R) -> R {
        TEXT.with(|t| f(t))
    }

    #[test]
    fn navigate_and_history() {
        let mut b = browser();
        assert_eq!(b.navigate("C:\\Users\\me\\Docs"), vec![Action::Navigate("C:\\Users\\me\\Docs".into())]);
        assert!(b.can_back());
        assert_eq!(b.load, Load::Loading);
        assert_eq!(b.back(), vec![Action::Navigate("C:\\Users\\me".into())]);
        assert_eq!(b.up(), vec![Action::Navigate("C:\\Users".into())]);
        // Coming up selects the folder we left once it is listed.
        b.loaded(vec![Entry::new("me", true, 0, 0, 0), Entry::new("you", true, 0, 0, 0)], Vec::new(), true, None);
        assert_eq!(b.selected_paths(), ["C:\\Users\\me"]);
        assert_eq!(b.navigate("C:\\users"), vec![Action::Refresh]);
        assert_eq!(b.navigate("shell:RecycleBinFolder"), vec![Action::ShellOpen("shell:RecycleBinFolder".into())]);
    }

    #[test]
    fn click_select_and_open() {
        with_text(|m| {
            let mut b = browser();
            let _ = b.set_view(View::List);
            let f = b.frame();
            let y = f.content.y + layout::row_y(3, 1.0) + 5.0;
            let x = f.content.x + 300.0;
            assert_eq!(b.hit(x, y, m), Hit::Item(3));
            b.mouse_down(x, y, Mods::default(), false, m);
            b.mouse_up(x, y, m);
            assert_eq!(b.list.selected_positions(), [3]);
            let y2 = f.content.y + layout::row_y(6, 1.0) + 5.0;
            b.mouse_down(x, y2, Mods { shift: true, ..Mods::default() }, false, m);
            b.mouse_up(x, y2, m);
            assert_eq!(b.list.selected_positions(), [3, 4, 5, 6]);
            let acts = b.mouse_down(x, y2, Mods::default(), true, m);
            assert_eq!(acts.len(), 1);
            assert!(matches!(&acts[0], Action::OpenFiles(p) if p.len() == 4));
        })
    }

    #[test]
    fn drag_starts_after_threshold() {
        with_text(|m| {
            let mut b = browser();
            let _ = b.set_view(View::List);
            let f = b.frame();
            let (x, y) = (f.content.x + 300.0, f.content.y + layout::row_y(2, 1.0) + 5.0);
            b.mouse_down(x, y, Mods::default(), false, m);
            assert!(b.mouse_move(x + 2.0, y, m).is_empty());
            let a = b.mouse_move(x + 10.0, y + 4.0, m);
            assert_eq!(a, vec![Action::DragOut(vec!["C:\\Users\\me\\file2.txt".into()])]);
        })
    }

    #[test]
    fn rubber_band_in_icons() {
        with_text(|m| {
            let mut b = browser();
            let f = b.frame();
            let g = Grid::new(f.content.w, 1.0);
            // Start in the gap left of the first tile, drag over two rows.
            let (x0, y0) = (f.content.x + 3.0, f.content.y + 3.0);
            assert_eq!(b.hit(x0, y0, m), Hit::Blank);
            b.mouse_down(x0, y0, Mods::default(), false, m);
            let t = g.tile(g.cols + 1);
            b.mouse_move(f.content.x + t.x + 5.0, f.content.y + t.y + 5.0, m);
            let sel = b.list.selected_positions();
            assert_eq!(sel, [0, 1, g.cols, g.cols + 1]);
            b.mouse_up(f.content.x + t.x + 5.0, f.content.y + t.y + 5.0, m);
            assert!(b.band().is_none());
        })
    }

    #[test]
    fn keyboard() {
        let mut b = browser();
        let _ = b.set_view(View::List);
        b.key(vk::DOWN, Mods::default());
        assert_eq!(b.list.selected_positions(), [0]);
        b.key(vk::DOWN, Mods { shift: true, ..Mods::default() });
        b.key(vk::DOWN, Mods { shift: true, ..Mods::default() });
        assert_eq!(b.list.selected_positions(), [0, 1, 2]);
        b.key(vk::END, Mods::default());
        assert_eq!(b.list.selected_positions(), [49]);
        assert!(b.target > 0.0);
        assert_eq!(b.key(vk::DELETE, Mods { shift: true, ..Mods::default() }), Some(vec![Action::Delete { paths: vec!["C:\\Users\\me\\file49.txt".into()], permanent: true }]));
        assert_eq!(b.key(vk::C, Mods { ctrl: true, ..Mods::default() }), Some(vec![Action::Clipboard { paths: vec!["C:\\Users\\me\\file49.txt".into()], cut: false }]));
        assert_eq!(b.key(vk::UP, Mods { alt: true, ..Mods::default() }), Some(vec![Action::Navigate("C:\\Users".into())]));
        assert_eq!(b.key(0x73, Mods { alt: true, ..Mods::default() }), None);
    }

    #[test]
    fn type_to_select_and_search() {
        let mut b = browser();
        b.char('f');
        b.char('i');
        b.char('l');
        b.char('e');
        b.char('4');
        assert_eq!(b.list.selected_entries().next().unwrap().name, "file4.txt");
        b.key(vk::F, Mods { ctrl: true, ..Mods::default() });
        for c in "file1".chars() {
            b.char(c);
        }
        assert_eq!(b.list.len(), 11);
        b.key(vk::ESCAPE, Mods::default());
        assert_eq!(b.list.len(), 50);
        assert!(b.search_focus);
        b.key(vk::ESCAPE, Mods::default());
        assert!(!b.search_focus);
    }

    #[test]
    fn rename_flow() {
        let mut b = browser();
        b.list.select_only(4);
        b.key(vk::F2, Mods::default());
        let (name, field) = b.renaming().unwrap();
        assert_eq!(name, "file4.txt");
        assert_eq!(field.selected_text(), "file4");
        for c in "report".chars() {
            b.char(c);
        }
        let a = b.key(vk::RETURN, Mods::default()).unwrap();
        assert_eq!(a, vec![Action::Rename { path: "C:\\Users\\me\\file4.txt".into(), to: "report.txt".into() }]);
        // The watcher's refresh brings the new name; it gets selected.
        let mut entries: Vec<Entry> = b.list.entries.clone();
        entries.retain(|e| e.name != "file4.txt");
        entries.push(Entry::new("report.txt", false, 1, 0, 0));
        b.replaced(entries, Vec::new(), None);
        assert_eq!(b.list.selected_entries().next().unwrap().name, "report.txt");
    }

    #[test]
    fn new_folder_names_and_rename() {
        let mut b = browser();
        assert_eq!(b.new_folder_name(), "New Folder");
        let a = b.new_folder();
        assert_eq!(a, vec![Action::NewFolder { path: "C:\\Users\\me\\New Folder".into() }]);
        let mut entries = b.list.entries.clone();
        entries.push(Entry::new("New Folder", true, 0, 0, 0));
        b.replaced(entries, Vec::new(), None);
        assert_eq!(b.renaming().map(|r| r.0), Some("New Folder"));
        assert_eq!(b.new_folder_name(), "New Folder (2)");
    }

    #[test]
    fn slow_second_click_renames() {
        with_text(|m| {
            let mut b = browser();
            let _ = b.set_view(View::List);
            let f = b.frame();
            let (x, y) = (f.content.x + 40.0, f.content.y + layout::row_y(1, 1.0) + 5.0);
            b.mouse_down(x, y, Mods::default(), false, m);
            b.mouse_up(x, y, m);
            for _ in 0..16 {
                b.tick(0.05);
            }
            b.mouse_down(x, y, Mods::default(), false, m);
            b.mouse_up(x, y, m);
            assert!(b.renaming().is_none());
            for _ in 0..12 {
                b.tick(0.05);
            }
            assert_eq!(b.renaming().map(|r| r.0), Some("file1.txt"));
        })
    }

    #[test]
    fn smooth_wheel_settles() {
        let mut b = browser();
        let _ = b.set_view(View::List);
        b.wheel(100.0, true);
        assert!(b.animating());
        for _ in 0..30 {
            b.tick(0.016);
        }
        assert_eq!(b.scroll_y(), 100.0);
        b.wheel(10_000.0, false);
        assert_eq!(b.scroll_y(), b.max_scroll().round());
    }

    #[test]
    fn icons_requested_once_per_key() {
        let mut b = browser();
        let r = b.icon_requests();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].key, "e64:txt");
        assert!(b.icon_requests().is_empty());
        b.icon_loaded("e64:txt".into(), None);
        assert!(b.icon_requests().is_empty());
    }

    #[test]
    fn paths_typed_into_search() {
        assert!(looks_like_path("C:\\Windows"));
        assert!(looks_like_path("d:"));
        assert!(looks_like_path("\\\\nas\\share"));
        assert!(!looks_like_path("report"));
    }
}
