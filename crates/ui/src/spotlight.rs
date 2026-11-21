//! The search panel: a text field with live results under it.

use std::cell::RefCell;
use std::rc::Rc;

use tiny_skia::{FillRule, FilterQuality, Mask, Paint, Pattern, PathBuilder, Pixmap, Rect, SpreadMode, Transform};

use crate::anim::Spring;
use crate::gfx::{self, Image, rgba};
use crate::text::Weight;
use crate::{Ctx, Key, Mods, WinRect, glyphs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    App,
    Folder,
    Document,
    Calculator,
    Web,
    Settings,
}

#[derive(Clone)]
pub struct Row {
    pub title: String,
    pub subtitle: String,
    pub icon: Option<Rc<Image>>,
    pub glyph: Glyph,
}

#[derive(Debug, PartialEq)]
pub enum SpotEvent {
    Query(String),
    Run(usize),
    /// Ctrl+Enter: show the item in its folder.
    Reveal(usize),
    Close,
}

#[derive(Clone, Copy, PartialEq)]
struct ChromeKey {
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    body_h: i32,
    dark: bool,
}

/// Target size and list rect.
type MaskKey = (u32, u32, i32, i32, i32, i32);

// Points.
const BODY_W: f32 = 680.0;
const FIELD_H: f32 = 54.0;
const ROW_H: f32 = 44.0;
const MAX_ROWS: usize = 8;
const LIST_PAD: f32 = 6.0;
const RADIUS: f32 = 16.0;
const MARGIN_X: f32 = 44.0;
const MARGIN_TOP: f32 = 30.0;
const MARGIN_BOTTOM: f32 = 70.0;
const QUERY_FONT: f32 = 22.0;
const BLINK: f32 = 0.53;

pub struct Spotlight {
    scale: f32,
    screen_w: f32,
    screen_h: f32,
    query: Vec<char>,
    caret: usize,
    all_selected: bool,
    rows: Vec<Row>,
    sel: usize,
    visible: bool,
    open: Spring,
    list: Spring,
    hl: Spring,
    scroll: Spring,
    blink: f32,
    layer: RefCell<Option<Pixmap>>,
    /// Shadow, panel and separator for one (target size, origin, height, theme).
    chrome: RefCell<Option<(ChromeKey, Pixmap)>>,
    /// Clip of the result list for icons and the highlight.
    list_mask: RefCell<Option<(MaskKey, Mask)>>,
    pub events: Vec<SpotEvent>,
}

impl Spotlight {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            screen_w: 1920.0,
            screen_h: 1080.0,
            query: Vec::new(),
            caret: 0,
            all_selected: false,
            rows: Vec::new(),
            sel: 0,
            visible: false,
            open: Spring::new(0.0, 0.3, 0.82).epsilon(0.001),
            list: Spring::new(0.0, 0.28, 0.9).epsilon(0.002),
            hl: Spring::new(0.0, 0.16, 1.0).epsilon(0.002),
            scroll: Spring::new(0.0, 0.22, 1.0).epsilon(0.002),
            blink: 0.0,
            layer: RefCell::new(None),
            chrome: RefCell::new(None),
            list_mask: RefCell::new(None),
            events: Vec::new(),
        }
    }

    pub fn set_screen(&mut self, w: f32, h: f32, scale: f32) {
        self.screen_w = w;
        self.screen_h = h;
        self.scale = scale;
    }

    pub fn window_rect(&self) -> WinRect {
        let s = self.scale;
        let w = ((BODY_W + 2.0 * MARGIN_X) * s).ceil();
        let h = ((MARGIN_TOP + FIELD_H + 1.0 + MAX_ROWS as f32 * ROW_H + 2.0 * LIST_PAD + MARGIN_BOTTOM) * s).ceil();
        WinRect {
            x: ((self.screen_w - w) / 2.0).round() as i32,
            y: (self.screen_h * 0.2 - MARGIN_TOP * s).round() as i32,
            w: w as i32,
            h: h as i32,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// True while the window must stay on screen (open, or fading out).
    pub fn shown(&self) -> bool {
        self.visible || self.open.value > 0.001
    }

    pub fn show(&mut self) {
        if self.visible {
            return;
        }
        self.visible = true;
        self.open = Spring::new(self.open.value, 0.3, 0.8).epsilon(0.001);
        self.open.to(1.0);
        self.all_selected = !self.query.is_empty();
        self.caret = self.query.len();
        self.blink = 0.0;
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.open = Spring::new(self.open.value, 0.2, 1.0).epsilon(0.001);
        self.open.to(0.0);
    }

    pub fn query(&self) -> String {
        self.query.iter().collect()
    }

    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.sel = 0;
        self.hl.snap(0.0);
        self.scroll.snap(0.0);
        self.list.to(self.rows.len().min(MAX_ROWS) as f32);
    }

    /// Replaces the icon of a row once it is loaded.
    pub fn set_row_icon(&mut self, row: usize, icon: Rc<Image>) {
        if let Some(r) = self.rows.get_mut(row) {
            r.icon = Some(icon);
        }
    }

    pub fn selected(&self) -> Option<usize> {
        (!self.rows.is_empty()).then_some(self.sel)
    }

    pub fn animating(&self) -> bool {
        !self.open.settled() || !self.list.settled() || !self.hl.settled() || !self.scroll.settled()
    }

    pub fn update(&mut self, dt: f32) {
        self.open.step(dt);
        self.list.step(dt);
        self.hl.step(dt);
        self.scroll.step(dt);
        self.blink += dt;
    }

    pub fn caret_visible(&self) -> bool {
        self.visible && (self.blink < BLINK || ((self.blink / BLINK) as u32).is_multiple_of(2))
    }

    fn select(&mut self, i: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.sel = i.min(self.rows.len() - 1);
        self.hl.to(self.sel as f32);
        let top = self.scroll.target;
        let max = MAX_ROWS as f32;
        if (self.sel as f32) < top {
            self.scroll.to(self.sel as f32);
        } else if self.sel as f32 > top + max - 1.0 {
            self.scroll.to(self.sel as f32 - max + 1.0);
        }
    }

    fn edited(&mut self) {
        self.blink = 0.0;
        self.events.push(SpotEvent::Query(self.query()));
    }

    fn delete_selection(&mut self) -> bool {
        if self.all_selected {
            self.query.clear();
            self.caret = 0;
            self.all_selected = false;
            return true;
        }
        false
    }

    pub fn char_input(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.delete_selection();
        self.query.insert(self.caret, c);
        self.caret += 1;
        self.edited();
    }

    fn word_left(&self) -> usize {
        let mut i = self.caret;
        while i > 0 && self.query[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && self.query[i - 1] != ' ' {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let n = self.query.len();
        let mut i = self.caret;
        while i < n && self.query[i] == ' ' {
            i += 1;
        }
        while i < n && self.query[i] != ' ' {
            i += 1;
        }
        i
    }

    pub fn key(&mut self, key: Key, mods: Mods) {
        self.blink = 0.0;
        match key {
            Key::Escape => {
                if self.query.is_empty() {
                    self.events.push(SpotEvent::Close);
                } else {
                    self.query.clear();
                    self.caret = 0;
                    self.all_selected = false;
                    self.edited();
                }
            }
            Key::Enter => {
                if let Some(i) = self.selected() {
                    self.events.push(if mods.ctrl { SpotEvent::Reveal(i) } else { SpotEvent::Run(i) });
                }
            }
            Key::Up => self.select(self.sel.saturating_sub(1)),
            Key::Down => self.select(self.sel + 1),
            Key::PageUp => self.select(self.sel.saturating_sub(MAX_ROWS)),
            Key::PageDown => self.select(self.sel + MAX_ROWS),
            Key::Tab => self.select(if mods.shift { self.sel.saturating_sub(1) } else { self.sel + 1 }),
            Key::Left => {
                self.all_selected = false;
                self.caret = if mods.ctrl { self.word_left() } else { self.caret.saturating_sub(1) };
            }
            Key::Right => {
                self.all_selected = false;
                self.caret = if mods.ctrl { self.word_right() } else { (self.caret + 1).min(self.query.len()) };
            }
            Key::Home => {
                self.all_selected = false;
                self.caret = 0;
            }
            Key::End => {
                self.all_selected = false;
                self.caret = self.query.len();
            }
            Key::SelectAll => self.all_selected = !self.query.is_empty(),
            Key::Backspace => {
                if !self.delete_selection() {
                    let from = if mods.ctrl { self.word_left() } else { self.caret.saturating_sub(1) };
                    if from == self.caret {
                        return;
                    }
                    self.query.drain(from..self.caret);
                    self.caret = from;
                }
                self.edited();
            }
            Key::Delete => {
                if !self.delete_selection() {
                    let to = if mods.ctrl { self.word_right() } else { (self.caret + 1).min(self.query.len()) };
                    if to == self.caret {
                        return;
                    }
                    self.query.drain(self.caret..to);
                }
                self.edited();
            }
        }
    }

    fn body_origin(&self) -> (f32, f32) {
        ((MARGIN_X * self.scale).round(), (MARGIN_TOP * self.scale).round())
    }

    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let s = self.scale;
        let (bx, by) = self.body_origin();
        if x < bx || x > bx + BODY_W * s {
            return None;
        }
        let list_top = by + (FIELD_H + 1.0 + LIST_PAD) * s;
        let shown = self.list.value.min(MAX_ROWS as f32);
        let rel = (y - list_top) / (ROW_H * s);
        if rel < 0.0 || rel >= shown {
            return None;
        }
        let i = (rel + self.scroll.value).floor() as usize;
        (i < self.rows.len()).then_some(i)
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if let Some(i) = self.row_at(x, y)
            && i != self.sel
        {
            self.select(i);
        }
    }

    /// Returns false when the click landed outside the panel (the host closes it).
    pub fn mouse_down(&mut self, x: f32, y: f32, ctx: &Ctx) -> bool {
        let s = self.scale;
        let (bx, by) = self.body_origin();
        if let Some(i) = self.row_at(x, y) {
            self.select(i);
            self.events.push(SpotEvent::Run(i));
            return true;
        }
        let in_field = x >= bx && x <= bx + BODY_W * s && y >= by && y <= by + FIELD_H * s;
        if in_field {
            let fs = QUERY_FONT * s;
            let tx = bx + 52.0 * s;
            let mut best = (f32::MAX, self.query.len());
            for i in 0..=self.query.len() {
                let w: String = self.query[..i].iter().collect();
                let d = (tx + ctx.text.measure(&w, fs, Weight::Regular) - x).abs();
                if d < best.0 {
                    best = (d, i);
                }
            }
            self.caret = best.1;
            self.all_selected = false;
            self.blink = 0.0;
            return true;
        }
        let body_h = self.body_height();
        x >= bx && x <= bx + BODY_W * s && y >= by && y <= by + body_h
    }

    pub fn wheel(&mut self, delta: f32) {
        let step = if delta > 0.0 { -1i32 } else { 1 };
        let next = (self.sel as i32 + step * 3).clamp(0, self.rows.len().saturating_sub(1) as i32);
        self.select(next as usize);
    }

    fn body_height(&self) -> f32 {
        let s = self.scale;
        let rows = self.list.value.max(0.0);
        let list = if rows > 0.001 { 1.0 + rows * ROW_H + 2.0 * LIST_PAD * rows.min(1.0) } else { 0.0 };
        ((FIELD_H + list) * s).round()
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let t = self.open.value;
        if t <= 0.001 {
            pm.fill(tiny_skia::Color::TRANSPARENT);
            return;
        }
        if t >= 0.999 {
            self.draw_body(pm, ctx);
            return;
        }
        pm.fill(tiny_skia::Color::TRANSPARENT);
        self.draw_opening(pm, ctx, t);
    }

    fn draw_opening(&self, pm: &mut Pixmap, ctx: &Ctx, t: f32) {
        // Opening: the panel scales up from 96% and fades in, as one layer that
        // only covers the panel and its shadow, not the whole window.
        let s = self.scale;
        let (bx, by) = self.body_origin();
        let (px, ptop, pbot) = ((48.0 * s).ceil(), (24.0 * s).ceil(), (96.0 * s).ceil());
        let lw = (BODY_W * s + 2.0 * px).ceil() as u32;
        let lh = (self.body_height() + ptop + pbot).ceil() as u32;
        let mut layer = self.layer.borrow_mut();
        if layer.as_ref().is_none_or(|l| l.width() != lw || l.height() < lh) {
            let max_h = (FIELD_H + 1.0 + MAX_ROWS as f32 * ROW_H + 2.0 * LIST_PAD) * s + ptop + pbot;
            *layer = Pixmap::new(lw, max_h.ceil() as u32);
        }
        let Some(l) = layer.as_mut() else { return };
        self.draw_body_at(l, ctx, px, ptop);
        let k = 0.96 + 0.04 * t.min(1.0) + (t - 1.0).max(0.0) * 0.04;
        let cx = bx + BODY_W * s / 2.0;
        let (lx, ly) = (bx - px, by - ptop);
        // Scale about the panel's top center.
        let tr = Transform::from_row(k, 0.0, 0.0, k, cx + (lx - cx) * k, by + (ly - by) * k);
        let paint = Paint {
            shader: Pattern::new(l.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, t.clamp(0.0, 1.0), tr),
            ..Paint::default()
        };
        if let Some(r) = Rect::from_xywh(0.0, 0.0, lw as f32, lh as f32) {
            pm.fill_rect(r, &paint, tr, None);
        }
    }

    fn draw_body(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let (bx, by) = self.body_origin();
        self.draw_body_at(pm, ctx, bx, by);
    }

    /// Overwrites all of `pm` with the panel whose top-left corner is `bx, by`.
    fn draw_body_at(&self, pm: &mut Pixmap, ctx: &Ctx, bx: f32, by: f32) {
        let s = self.scale;
        let th = ctx.theme;
        let bw = BODY_W * s;
        let bh = self.body_height();
        let fh = FIELD_H * s;

        let key = ChromeKey { w: pm.width(), h: pm.height(), x: bx as i32, y: by as i32, body_h: bh as i32, dark: th.dark };
        {
            let mut chrome = self.chrome.borrow_mut();
            if chrome.as_ref().is_none_or(|(k, _)| *k != key) {
                *chrome = Pixmap::new(pm.width(), pm.height()).map(|mut c| {
                    let r = RADIUS * s;
                    ctx.shadow(r, 26.0 * s).draw(&mut c, bx, by + 16.0 * s, bw, bh, gfx::fade(th.shadow(), 1.1));
                    ctx.shadow(r, 1.5 * s).draw(&mut c, bx, by, bw, bh, rgba(0, 0, 0, 0.1));
                    if let Some(p) = gfx::squircle(bx, by, bw, bh, r) {
                        gfx::fill(&mut c, &p, th.panel());
                        gfx::stroke(&mut c, &p, th.panel_border(), 1.0);
                    }
                    if th.dark
                        && let Some(p) = gfx::squircle(bx + 1.0, by + 1.0, bw - 2.0, bh - 2.0, r - 1.0)
                    {
                        gfx::stroke(&mut c, &p, th.panel_rim(), 1.0);
                    }
                    if bh - fh >= 1.0 {
                        gfx::fill_rect(&mut c, bx, (by + fh).round(), bw, s.max(1.0).round(), th.separator());
                    }
                    let gs = 21.0 * s;
                    glyphs::magnifier(&mut c, bx + 18.0 * s, by + fh / 2.0 - gs / 2.0, gs, th.secondary());
                    (key, c)
                });
            }
            match chrome.as_ref() {
                Some((_, c)) => pm.data_mut().copy_from_slice(c.data()),
                None => pm.fill(tiny_skia::Color::TRANSPARENT),
            }
        }

        // Field.
        let fs = QUERY_FONT * s;
        let tx = bx + 52.0 * s;
        let base = (by + fh / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
        let clip = Some((tx as i32, by as i32, (bx + bw - 16.0 * s) as i32, (by + fh) as i32));
        if self.query.is_empty() {
            ctx.text.draw(pm, ctx.strings.search_placeholder, tx, base, fs, Weight::Regular, th.field_placeholder(), clip);
        } else {
            let q = self.query();
            if self.all_selected {
                let w = ctx.text.measure(&q, fs, Weight::Regular);
                if let Some(p) = gfx::squircle(tx - 1.0 * s, by + fh / 2.0 - 15.0 * s, w + 2.0 * s, 30.0 * s, 3.0 * s) {
                    gfx::fill(pm, &p, gfx::fade(th.accent(), 0.3));
                }
            }
            ctx.text.draw(pm, &q, tx, base, fs, Weight::Regular, th.label(), clip);
        }
        if self.caret_visible() && !self.all_selected {
            let pre: String = self.query[..self.caret].iter().collect();
            let cx = (tx + ctx.text.measure(&pre, fs, Weight::Regular)).round();
            gfx::fill_rect(pm, cx, by + fh / 2.0 - 13.0 * s, (2.0 * s).round().max(2.0), 26.0 * s, th.accent());
        }

        // Results, clipped to the list area while it grows or scrolls.
        if bh - fh < 2.0 || self.rows.is_empty() {
            return;
        }
        let (lx0, ly0) = (bx as i32, (by + fh + 1.0).round() as i32);
        let (lx1, ly1) = ((bx + bw) as i32, (by + bh - 2.0).round() as i32);
        let mkey = (pm.width(), pm.height(), lx0, ly0, lx1, ly1);
        let mut masks = self.list_mask.borrow_mut();
        if masks.as_ref().is_none_or(|(k, _)| *k != mkey) {
            *masks = Mask::new(pm.width(), pm.height()).and_then(|mut m| {
                let r = Rect::from_ltrb(lx0 as f32, ly0 as f32, lx1 as f32, ly1 as f32)?;
                m.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
                Some((mkey, m))
            });
        }
        let mask = masks.as_ref().map(|(_, m)| m);
        let clip = Some((lx0, ly0, lx1, ly1));
        let rh = ROW_H * s;
        let top = ly0 as f32 + LIST_PAD * s;
        let scroll = self.scroll.value;
        let hy = top + (self.hl.value - scroll) * rh;
        if let Some(p) = gfx::squircle(bx + 8.0 * s, hy, bw - 16.0 * s, rh, 9.0 * s) {
            pm.fill_path(&p, &gfx::paint(th.accent()), FillRule::Winding, Transform::identity(), mask);
        }
        let first = scroll.floor().max(0.0) as usize;
        let last = ((scroll + MAX_ROWS as f32 + 1.0).ceil() as usize).min(self.rows.len());
        let mut tile: Option<Pixmap> = None;
        for i in first..last {
            let row = &self.rows[i];
            let y = top + (i as f32 - scroll) * rh;
            if y > ly1 as f32 || y + rh < ly0 as f32 {
                continue;
            }
            let lit = i == self.sel;
            let isz = 30.0 * s;
            let (ix, iy) = (bx + 18.0 * s, (y + rh / 2.0 - isz / 2.0).round());
            let inside = iy >= ly0 as f32 && iy + isz <= ly1 as f32;
            // Glyph icons are drawn straight when fully visible, else via a small
            // tile so the mask can clip them.
            let icon = |target: &mut Pixmap, x: f32, y: f32| match &row.icon {
                Some(img) => img.draw(target, x, y, isz, 1.0, false),
                None => match row.glyph {
                    Glyph::App => glyphs::app_tile(target, x, y, isz),
                    Glyph::Folder => glyphs::folder(target, x, y, isz),
                    Glyph::Document => glyphs::document(target, x, y, isz),
                    Glyph::Calculator => glyphs::calculator(target, x, y, isz),
                    Glyph::Web => glyphs::globe(target, x, y, isz),
                    Glyph::Settings => glyphs::settings_tile(target, x, y, isz),
                },
            };
            if inside {
                icon(pm, ix, iy);
            } else {
                let n = (isz * 1.3).ceil() as u32;
                let t = tile.get_or_insert_with(|| Pixmap::new(n, n).expect("tile"));
                t.fill(tiny_skia::Color::TRANSPARENT);
                let off = isz * 0.15;
                icon(t, off, off);
                pm.draw_pixmap(0, 0, t.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::from_translate((ix - off).round(), (iy - off).round()), mask);
            }
            let title_c = if lit { rgba(255, 255, 255, 1.0) } else { th.label() };
            let sub_c = if lit { rgba(255, 255, 255, 0.78) } else { th.secondary() };
            let text_x = bx + 60.0 * s;
            let max_w = bw - 60.0 * s - 18.0 * s;
            let tfs = 14.5 * s;
            let sfs = 11.5 * s;
            let t = ctx.text.ellipsize(&row.title, tfs, Weight::Medium, max_w);
            if row.subtitle.is_empty() {
                let b = (y + rh / 2.0 + ctx.text.cap_height(tfs) / 2.0).round();
                ctx.text.draw(pm, &t, text_x, b, tfs, Weight::Medium, title_c, clip);
            } else {
                ctx.text.draw(pm, &t, text_x, (y + 19.0 * s).round(), tfs, Weight::Medium, title_c, clip);
                let sub = ctx.text.ellipsize(&row.subtitle, sfs, Weight::Regular, max_w);
                ctx.text.draw(pm, &sub, text_x, (y + 35.0 * s).round(), sfs, Weight::Regular, sub_c, clip);
            }
        }
    }
}

impl Default for Spotlight {
    fn default() -> Self {
        Self::new()
    }
}
