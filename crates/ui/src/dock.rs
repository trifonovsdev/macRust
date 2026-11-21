//! The dock: magnification that keeps the icon under the cursor in place,
//! launch bounce, running indicators, labels and insert/remove animation.

use std::cell::RefCell;
use std::f32::consts::PI;
use std::rc::Rc;

use tiny_skia::Pixmap;

use crate::anim::{BOUNCE_PERIOD, Spring, launch_bounce};
use crate::gfx::{self, Image, rgba};
use crate::glass::{self, GlassStyle};
use crate::text::Weight;
use crate::{Ctx, WinRect, glyphs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    /// Pinned and running apps.
    Apps,
    /// Folders and the recycle bin, right of the separator.
    Stacks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    App,
    Folder,
    Trash,
}

#[derive(Clone)]
pub struct DockEntry {
    pub key: String,
    pub label: String,
    pub icon: Option<Rc<Image>>,
    pub running: bool,
    pub section: Section,
    pub fallback: Fallback,
}

#[derive(Debug, PartialEq)]
pub enum DockEvent {
    Click(String),
    /// Context menu for an item; `x, y` is the top-center of its icon in window px.
    Menu { key: String, x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct DockConfig {
    /// Icon size in points.
    pub icon: f32,
    /// Size of the icon under the cursor, in points.
    pub magnified: f32,
    pub magnify: bool,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self { icon: 52.0, magnified: 92.0, magnify: false }
    }
}

struct Slot {
    e: DockEntry,
    appear: Spring,
    removing: bool,
    bounce: Option<f32>,
    /// Keep bouncing until the app shows a window.
    bounce_hold: bool,
}

#[derive(Clone, Copy)]
enum Cell {
    Slot(usize),
    Separator,
}

struct Geo {
    cells: Vec<(Cell, f32, f32)>,
    bg: (f32, f32, f32, f32),
    icon_bottom: f32,
    icon0: f32,
}

/// Wallpaper id, dark theme, window rect.
type MaterialKey = (u64, bool, WinRect);

// Geometry in points.
// Proportions measured from macOS 26 screenshots. Icons carry Apple's
// canvas margin (the visible tile is ART of the icon box), so the gaps
// between visible tiles come out at 0.31 of a tile, as in Tahoe.
const GAP_BOTTOM: f32 = 4.0;
const PAD: f32 = 8.5;
const SIDE: f32 = 3.0;
const TILE: f32 = 1.056;
/// Visible part of an icon in its box (Apple's 824/1024 grid).
const ART: f32 = 0.805;
const SEP_W: f32 = 16.0;
/// Half-width of the magnification bell, in tiles.
const RANGE: f32 = 2.7;
const TIP_SPACE: f32 = 46.0;
const EDGE_MARGIN: f32 = 130.0;

pub struct Dock {
    pub cfg: DockConfig,
    scale: f32,
    screen_w: f32,
    screen_h: f32,
    slots: Vec<Slot>,
    mouse: Option<(f32, f32)>,
    last_mx: f32,
    mag: Spring,
    tip: Spring,
    tip_slot: Option<usize>,
    hover: Option<usize>,
    pressed: Option<usize>,
    menu_open: bool,
    initialized: bool,
    /// Blurred wallpaper + tint for this window: (vibrancy id, dark, rect) -> pixels.
    material: RefCell<Option<(MaterialKey, Pixmap)>>,
    /// Live picture of what is behind the window; replaces the wallpaper.
    backdrop: Option<glass::Backdrop>,
    pub events: Vec<DockEvent>,
}

fn bell(t: f32) -> f32 {
    if t.abs() >= 1.0 { 0.0 } else { (1.0 + (PI * t).cos()) * 0.5 }
}

impl Dock {
    pub fn new(cfg: DockConfig) -> Self {
        Self {
            cfg,
            scale: 1.0,
            screen_w: 1920.0,
            screen_h: 1080.0,
            slots: Vec::new(),
            mouse: None,
            last_mx: 0.0,
            mag: Spring::new(0.0, 0.32, 0.92),
            tip: Spring::new(0.0, 0.18, 1.0),
            tip_slot: None,
            hover: None,
            pressed: None,
            menu_open: false,
            initialized: false,
            material: RefCell::new(None),
            backdrop: None,
            events: Vec::new(),
        }
    }

    pub fn set_screen(&mut self, w: f32, h: f32, scale: f32) {
        self.screen_w = w;
        self.screen_h = h;
        self.scale = scale;
    }

    /// Height of the strip the dock keeps free at the bottom of the work area.
    pub fn reserved_height(&self) -> i32 {
        ((self.cfg.icon + 2.0 * PAD + GAP_BOTTOM) * self.scale).ceil() as i32
    }

    pub fn window_rect(&self) -> WinRect {
        let s = self.scale;
        let tile = self.cfg.icon * TILE * s;
        let n = self.slots.len() as f32 + 1.0;
        let extra = (self.cfg.magnified / self.cfg.icon - 1.0).max(0.0) * RANGE * tile;
        let w = (n * tile + SEP_W * s + 2.0 * extra + 2.0 * (SIDE + EDGE_MARGIN) * s).min(self.screen_w).ceil();
        let h = ((GAP_BOTTOM + 2.0 * PAD + self.cfg.magnified.max(self.cfg.icon * 1.55) + TIP_SPACE) * s).ceil();
        WinRect { x: ((self.screen_w - w) / 2.0).round() as i32, y: (self.screen_h - h) as i32, w: w as i32, h: h as i32 }
    }

    /// Replaces the item list. Known keys keep their state, new ones grow in,
    /// missing ones shrink out where they were.
    pub fn set_entries(&mut self, entries: Vec<DockEntry>) {
        let keys = self.state_keys();
        let mut pool: Vec<Option<Slot>> = std::mem::take(&mut self.slots).into_iter().map(Some).collect();
        let mut next: Vec<Slot> = Vec::with_capacity(entries.len());
        for e in entries {
            let found = pool.iter().position(|s| s.as_ref().is_some_and(|s| s.e.key == e.key));
            if let Some(mut s) = found.and_then(|i| pool[i].take()) {
                if e.running {
                    s.bounce_hold = false;
                }
                s.e = e;
                if s.removing {
                    s.removing = false;
                    s.appear.to(1.0);
                }
                next.push(s);
            } else {
                let mut appear = Spring::new(0.0, 0.42, 0.9);
                if self.initialized {
                    appear.to(1.0);
                } else {
                    appear.snap(1.0);
                }
                next.push(Slot { e, appear, removing: false, bounce: None, bounce_hold: false });
            }
        }
        // Items that went away shrink out roughly where they were.
        for (i, s) in pool.into_iter().enumerate() {
            if let Some(mut s) = s {
                s.removing = true;
                s.appear.to(0.0);
                next.insert(i.min(next.len()), s);
            }
        }
        self.slots = next;
        self.restore_keys(keys);
        self.initialized = true;
        self.refresh_hover();
    }

    fn state_keys(&self) -> [Option<String>; 2] {
        let key = |i: Option<usize>| i.and_then(|i| self.slots.get(i)).map(|s| s.e.key.clone());
        [key(self.tip_slot), key(self.pressed)]
    }

    /// Indices shift when items come and go; the label and the pressed state follow their item.
    fn restore_keys(&mut self, [tip, pressed]: [Option<String>; 2]) {
        let find = |k: Option<String>| k.and_then(|k| self.slots.iter().position(|s| s.e.key == k));
        self.tip_slot = find(tip);
        self.pressed = find(pressed);
        if self.tip_slot.is_none() {
            self.tip.snap(0.0);
        }
        self.hover = None;
    }

    /// Starts the launch bounce of `key`; it keeps hopping until the app runs.
    pub fn bounce(&mut self, key: &str) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.key == key) {
            if s.bounce.is_none() {
                s.bounce = Some(0.0);
            }
            s.bounce_hold = !s.e.running;
        }
    }

    /// What is behind the dock window right now, window-sized.
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(glass::Backdrop::new(behind));
    }

    pub fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        if open {
            self.tip.to(0.0);
        }
    }

    pub fn animating(&self) -> bool {
        !self.mag.settled()
            || !self.tip.settled()
            || self.slots.iter().any(|s| !s.appear.settled() || s.bounce.is_some())
    }

    pub fn update(&mut self, dt: f32) {
        self.mag.step(dt);
        self.tip.step(dt);
        for s in &mut self.slots {
            s.appear.step(dt);
            if let Some(t) = s.bounce {
                let before = (t / BOUNCE_PERIOD).floor();
                let t2 = t + dt;
                let after = (t2 / BOUNCE_PERIOD).floor();
                s.bounce = if after > before && (!s.bounce_hold || after >= 8.0) { None } else { Some(t2) };
            }
        }
        if self.slots.iter().any(|s| s.removing && s.appear.settled()) {
            let keys = self.state_keys();
            self.slots.retain(|s| !(s.removing && s.appear.settled()));
            self.restore_keys(keys);
        }
        self.refresh_hover();
    }

    fn geo(&self, win_w: f32, win_h: f32) -> Geo {
        let s = self.scale;
        let icon0 = self.cfg.icon * s;
        let tile0 = icon0 * TILE;
        let mut cells: Vec<(Cell, f32)> = Vec::new();
        let has_apps = self.slots.iter().any(|x| x.e.section == Section::Apps);
        let mut sep_done = false;
        for (i, sl) in self.slots.iter().enumerate() {
            if sl.e.section == Section::Stacks && !sep_done && has_apps {
                cells.push((Cell::Separator, SEP_W * s));
                sep_done = true;
            }
            cells.push((Cell::Slot(i), tile0 * sl.appear.value.max(0.0)));
        }
        let total0: f32 = cells.iter().map(|c| c.1).sum();
        let start0 = (win_w - total0) / 2.0;
        let amount = if self.cfg.magnify { self.mag.value.clamp(0.0, 1.2) } else { 0.0 };
        let maxf = (self.cfg.magnified / self.cfg.icon).max(1.0);
        let range = RANGE * tile0;
        let mx = self.last_mx;
        let mut cum0 = start0;
        let mut factors = Vec::with_capacity(cells.len());
        for (c, w0) in &cells {
            let center = cum0 + w0 / 2.0;
            cum0 += w0;
            let f = match c {
                Cell::Slot(_) => 1.0 + (maxf - 1.0) * amount * bell((mx - center) / range),
                Cell::Separator => 1.0,
            };
            factors.push(f);
        }
        // Anchor: the point under the cursor stays under the cursor, so the dock
        // grows on both sides of it instead of sliding.
        let u = mx.clamp(start0, start0 + total0.max(0.0));
        let mut acc0 = start0;
        let mut acc = 0.0;
        let mut m_u = 0.0;
        for (i, (_, w0)) in cells.iter().enumerate() {
            let w = w0 * factors[i];
            if u <= acc0 + w0 || i == cells.len() - 1 {
                let frac = if *w0 > 0.0 { ((u - acc0) / w0).clamp(0.0, 1.0) } else { 0.0 };
                m_u = acc + frac * w;
                break;
            }
            acc0 += w0;
            acc += w;
        }
        let start = if cells.is_empty() { start0 } else { u - m_u };
        let mut x = start;
        let mut out = Vec::with_capacity(cells.len());
        for (i, (c, w0)) in cells.iter().enumerate() {
            let w = w0 * factors[i];
            out.push((*c, x, w));
            x += w;
        }
        let total = x - start;
        let bh = icon0 + 2.0 * PAD * s;
        let side = SIDE * s;
        let bg = (start - side, win_h - GAP_BOTTOM * s - bh, total + 2.0 * side, bh);
        Geo { cells: out, bg, icon_bottom: win_h - GAP_BOTTOM * s - PAD * s, icon0 }
    }

    fn icon_box(&self, g: &Geo, slot: usize, x: f32, w: f32) -> (f32, f32, f32) {
        let sl = &self.slots[slot];
        let a = sl.appear.value.clamp(0.0, 1.0);
        let tile0 = g.icon0 * TILE * a;
        let factor = if tile0 > 0.0 { w / tile0 } else { 1.0 };
        let size = g.icon0 * factor * a;
        let lift = sl.bounce.map(launch_bounce).unwrap_or(0.0) * g.icon0 * 0.5;
        (x + (w - size) / 2.0, g.icon_bottom - size - lift, size)
    }

    fn refresh_hover(&mut self) {
        let wr = self.window_rect();
        let Some((mx, my)) = self.mouse else {
            self.hover = None;
            self.tip.to(0.0);
            return;
        };
        let g = self.geo(wr.w as f32, wr.h as f32);
        let mut hit = None;
        for (c, x, w) in &g.cells {
            if let Cell::Slot(i) = c
                && mx >= *x
                && mx < x + w
                && !self.slots[*i].removing
            {
                let (_, iy, _) = self.icon_box(&g, *i, *x, *w);
                if my >= iy.min(g.bg.1) - 2.0 * self.scale {
                    hit = Some(*i);
                }
            }
        }
        self.hover = hit;
        if let Some(h) = hit
            && !self.menu_open
        {
            self.tip_slot = Some(h);
            self.tip.to(1.0);
        } else {
            self.tip.to(0.0);
        }
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = Some((x, y));
        self.last_mx = x;
        self.mag.to(1.0);
        self.refresh_hover();
    }

    pub fn mouse_leave(&mut self) {
        self.mouse = None;
        self.mag.to(0.0);
        self.pressed = None;
        self.refresh_hover();
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) {
        self.mouse_move(x, y);
        self.pressed = self.hover;
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) {
        self.mouse_move(x, y);
        if let (Some(p), Some(h)) = (self.pressed, self.hover)
            && p == h
        {
            self.events.push(DockEvent::Click(self.slots[p].e.key.clone()));
        }
        self.pressed = None;
    }

    pub fn right_click(&mut self, x: f32, y: f32) {
        self.mouse_move(x, y);
        if let Some(h) = self.hover {
            let wr = self.window_rect();
            let g = self.geo(wr.w as f32, wr.h as f32);
            if let Some((_, cx, cw)) = g.cells.iter().find(|c| matches!(c.0, Cell::Slot(i) if i == h)) {
                let (ix, iy, size) = self.icon_box(&g, h, *cx, *cw);
                self.events.push(DockEvent::Menu { key: self.slots[h].e.key.clone(), x: ix + size / 2.0, y: iy });
            }
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let wr = self.window_rect();
        let (ww, wh) = (pm.width() as f32, pm.height() as f32);
        let g = self.geo(ww, wh);
        let s = self.scale;
        let th = ctx.theme;
        let (bx, by, bw, bh) = g.bg;
        if bw <= 1.0 {
            return;
        }

        // Nearly transparent pixels still take mouse input on a layered window:
        // they keep the hover alive in the gaps between magnified icons and on
        // the bottom screen edge, where alpha 0 would fall through.
        let mut top = by;
        for (c, x, w) in &g.cells {
            if let Cell::Slot(i) = c {
                top = top.min(self.icon_box(&g, *i, *x, *w).1);
            }
        }
        if self.mouse.is_none() {
            top = by;
        }
        gfx::put_rect(pm, bx, top, bw + 1.0, wh - top.floor(), [0, 0, 0, 1]);

        let radius = bh * 0.26;
        ctx.shadow(radius, 9.0 * s).draw(pm, bx, by + 3.0 * s, bw, bh, if th.dark { rgba(0, 0, 0, 0.30) } else { rgba(0, 0, 0, 0.10) });
        match (&self.backdrop, &ctx.vibrancy) {
            (Some(b), _) => b.draw(pm, glass::Shape { x: bx, y: by, w: bw, h: bh, r: radius }, &GlassStyle::dock(th.dark, s)),
            (None, Some(v)) => {
                let key = (v.id, th.dark, wr);
                let mut cache = self.material.borrow_mut();
                if cache.as_ref().is_none_or(|(k, _)| *k != key) {
                    let st = GlassStyle::dock(th.dark, s);
                    *cache = glass::material(v, wr.x as f32, wr.y as f32, pm.width(), pm.height(), st.tint).map(|m| (key, m));
                }
                if let Some((_, m)) = cache.as_ref() {
                    let shape = glass::Shape { x: bx, y: by, w: bw, h: bh, r: radius };
                    glass::draw(pm, v, m, wr.x as f32, wr.y as f32, shape, &GlassStyle::dock(th.dark, s));
                }
            }
            (None, None) => {
                if let Some(path) = gfx::squircle(bx, by, bw, bh, radius) {
                    gfx::fill(pm, &path, th.dock_fallback());
                    gfx::stroke(pm, &path, th.dock_border(), 1.0);
                }
            }
        }

        for (c, x, w) in &g.cells {
            match c {
                Cell::Separator => {
                    let lx = (x + w / 2.0).round() - 0.5;
                    gfx::fill_rect(pm, lx, by + bh * 0.16, s.max(1.0), bh * 0.68, if th.dark { rgba(255, 255, 255, 0.22) } else { rgba(0, 0, 0, 0.16) });
                }
                Cell::Slot(i) => {
                    let sl = &self.slots[*i];
                    let (ix, iy, size) = self.icon_box(&g, *i, *x, *w);
                    if size < 1.0 {
                        continue;
                    }
                    let alpha = sl.appear.value.clamp(0.0, 1.0);
                    let darken = self.pressed == Some(*i) && self.hover == Some(*i);
                    match &sl.e.icon {
                        Some(img) => img.draw(pm, ix, iy, size, alpha, darken),
                        None => match sl.e.fallback {
                            Fallback::App => glyphs::app_tile(pm, ix, iy, size),
                            Fallback::Folder => glyphs::folder(pm, ix, iy, size),
                            Fallback::Trash => glyphs::trash(pm, ix, iy, size),
                        },
                    }
                    if sl.e.running && alpha > 0.5 {
                        let cx = ix + size / 2.0;
                        // Just under the visible tile, not under the icon's canvas margin.
                        let art_bottom = iy + size - size * (1.0 - ART) / 2.0;
                        let cy = art_bottom.max(g.icon_bottom - g.icon0 * (1.0 - ART) / 2.0) + g.icon0 * ART * 0.1;
                        if let Some(p) = gfx::circle(cx, cy, 1.8 * s) {
                            gfx::fill(pm, &p, gfx::fade(th.indicator(), alpha));
                        }
                    }
                }
            }
        }

        // Label above the hovered icon.
        if let Some(t) = self.tip_slot
            && self.tip.value > 0.01
            && t < self.slots.len()
            && let Some((_, x, w)) = g.cells.iter().find(|c| matches!(c.0, Cell::Slot(i) if i == t))
        {
            let (ix, iy, size) = self.icon_box(&g, t, *x, *w);
            let a = self.tip.value.clamp(0.0, 1.0);
            let fs = 13.0 * s;
            let label = ctx.text.ellipsize(&self.slots[t].e.label, fs, Weight::Regular, 280.0 * s);
            let tw = ctx.text.measure(&label, fs, Weight::Regular);
            let (pw, ph) = (tw + 20.0 * s, 26.0 * s);
            let px = (ix + size / 2.0 - pw / 2.0).clamp(2.0, ww - pw - 2.0).round();
            let py = (iy - 10.0 * s - ph + (1.0 - a) * 4.0 * s).round();
            ctx.shadow(6.0 * s, 4.0 * s).draw(pm, px, py + 1.0 * s, pw, ph, gfx::fade(th.shadow(), a * 0.7));
            if let Some(p) = gfx::squircle(px, py, pw, ph, 7.0 * s) {
                gfx::fill(pm, &p, gfx::fade(th.tooltip(), a));
                gfx::stroke(pm, &p, gfx::fade(th.panel_border(), a * 0.8), 1.0);
            }
            let base = py + ph / 2.0 + ctx.text.cap_height(fs) / 2.0;
            ctx.text.draw(pm, &label, px + 10.0 * s, base, fs, Weight::Regular, gfx::fade(th.label(), a), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str) -> DockEntry {
        DockEntry { key: key.into(), label: key.into(), icon: None, running: false, section: Section::Apps, fallback: Fallback::App }
    }

    fn icon_y(d: &Dock, key: &str) -> f32 {
        let wr = d.window_rect();
        let g = d.geo(wr.w as f32, wr.h as f32);
        let i = d.slots.iter().position(|s| s.e.key == key).unwrap();
        let (_, x, w) = g.cells.iter().find(|c| matches!(c.0, Cell::Slot(j) if j == i)).copied().unwrap();
        d.icon_box(&g, i, x, w).1
    }

    #[test]
    fn launch_bounce_lifts_then_lands_once_running() {
        let mut d = Dock::new(DockConfig::default());
        d.set_entries(vec![entry("a"), entry("b")]);
        let rest = icon_y(&d, "b");
        d.bounce("b");
        for _ in 0..37 {
            d.update(1.0 / 120.0);
        }
        assert!(icon_y(&d, "b") < rest - 20.0, "icon should be in the air");
        let mut e = entry("b");
        e.running = true;
        d.set_entries(vec![entry("a"), e]);
        for _ in 0..120 {
            d.update(1.0 / 120.0);
        }
        assert_eq!(icon_y(&d, "b"), rest);
        assert!(!d.animating());
    }

    #[test]
    fn magnified_icon_stays_under_the_cursor() {
        let mut d = Dock::new(DockConfig { magnify: true, ..DockConfig::default() });
        d.set_entries((0..8).map(|i| entry(&i.to_string())).collect());
        let wr = d.window_rect();
        let g0 = d.geo(wr.w as f32, wr.h as f32);
        let (_, x, w) = g0.cells[3];
        let mx = x + w * 0.3;
        d.mouse_move(mx, wr.h as f32 - 20.0);
        for _ in 0..240 {
            d.update(1.0 / 120.0);
        }
        let g1 = d.geo(wr.w as f32, wr.h as f32);
        let (_, x1, w1) = g1.cells[3];
        assert!(w1 > w * 1.5);
        assert!((x1 + w1 * 0.3 - mx).abs() < 0.5, "anchor drifted");
        d.mouse_leave();
        for _ in 0..240 {
            d.update(1.0 / 120.0);
        }
        assert!(d.tip.value < 0.01 && !d.animating());
    }
}
