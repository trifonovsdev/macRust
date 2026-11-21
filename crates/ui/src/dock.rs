//! The dock: magnification that keeps the icon under the cursor in place,
//! launch bounce, running indicators, labels and insert/remove animation.

use std::cell::RefCell;
use std::f32::consts::PI;
use std::rc::Rc;

use tiny_skia::Pixmap;

use crate::anim::{BOUNCE_PERIOD, Tween, ease_out, launch_bounce, smoothstep};
use crate::gfx::{self, Image, rgba};
use crate::glass;
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
        Self { icon: 52.0, magnified: 72.0, magnify: false }
    }
}

struct Slot {
    e: DockEntry,
    appear: Tween,
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

/// Dark theme, Downloads folder, Trash.
type Stacks = (bool, Rc<Image>, Rc<Image>);

// Geometry, measured on macOS 26 screenshots in units of the visible tile
// (V). Icons carry Apple's canvas margin: the visible tile is ART of the
// icon box, so a cell of TILE boxes gives a pitch of 1.31 V.
/// Visible part of an icon in its box (Apple's 824/1024 grid).
const ART: f32 = 0.805;
const TILE: f32 = 1.31 * ART;
/// Bar height (1.62-1.72 V measured); the tile is centred in it.
const BAR: f32 = 1.67;
/// From the bar's end to the first tile: one gap (0.31 V), half of which is in the cell.
const SIDE: f32 = 0.155;
/// Extra width of the separator cell: tile to tile across it is 0.94 V.
const SEP_W: f32 = 0.63;
const SEP_H: f32 = 1.22;
/// The bar floats this many points above the bottom of the screen.
const GAP_BOTTOM: f32 = 5.5;
/// Launch hop height and the running dot's centre below the tile.
const HOP: f32 = 0.355;
/// Corner radius / bar height: 20 pt on a 72.5 pt bar.
const RADIUS: f32 = 0.276;
const DOT_BELOW: f32 = 0.2;
/// Measured at 60 fps: the gap for a new icon opens over 0.4 s (ease-out),
/// a removed icon's gap closes over 0.27 s (ease-in-out).
const INSERT_S: f32 = 0.4;
const REMOVE_S: f32 = 0.27;
/// Tahoe shows the label at once; the owner asked for a softer entrance, so it
/// fades in and rises LABEL_RISE pt. Moving to a neighbour keeps it shown.
const LABEL_IN_S: f32 = 0.16;
const LABEL_OUT_S: f32 = 0.12;
const LABEL_RISE: f32 = 4.0;
// Magnification, measured on macOS 26 (out/ref/TAHOE-MAGNIFY.md): a half
// cosine reaching zero RANGE base slots from the cursor, the Dock stays centred,
// the gap between icons and the bar's height do not scale. The zoom snaps on
// in MAG_IN_S and collapses in MAG_OUT_S (ease-in-out, no spring); the bell
// stays where the pointer left.
const RANGE: f32 = 3.0;
const MAG_IN_S: f32 = 0.08;
const MAG_OUT_S: f32 = 0.17;
/// Sum of the bell over the slots plus the separator's share: the widest the
/// magnified Dock gets, in base slots per unit of extra scale.
const MAG_SPREAD: f32 = 3.9 + 0.45;
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
    /// Magnification amount, 0..1.
    mag: Tween,
    tip: Tween,
    tip_slot: Option<usize>,
    hover: Option<usize>,
    pressed: Option<usize>,
    menu_open: bool,
    initialized: bool,
    /// Live picture of what is behind the window; replaces the wallpaper.
    backdrop: Option<glass::Backdrop>,
    /// Drawn Downloads and Trash for this appearance.
    stacks: RefCell<Option<Stacks>>,
    pub events: Vec<DockEvent>,
}

/// `d` in base slots from the cursor.
fn bell(d: f32) -> f32 {
    if d.abs() >= RANGE { 0.0 } else { (PI / 2.0 * d / RANGE).cos() }
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
            mag: Tween::new(0.0),
            tip: Tween::new(0.0),
            tip_slot: None,
            hover: None,
            pressed: None,
            menu_open: false,
            initialized: false,
            backdrop: None,
            stacks: RefCell::new(None),
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
        ((self.cfg.icon * ART * BAR + GAP_BOTTOM) * self.scale).ceil() as i32
    }

    /// Visible tile size at rest, in points.
    fn v(&self) -> f32 {
        self.cfg.icon * ART
    }

    pub fn window_rect(&self) -> WinRect {
        let s = self.scale;
        let tile = self.cfg.icon * TILE * s;
        let n = self.slots.len() as f32 + 1.0;
        let extra = (self.cfg.magnified / self.cfg.icon - 1.0).max(0.0) * MAG_SPREAD * tile / 2.0;
        let v = self.v();
        let w = (n * tile + (SEP_W * v + 2.0 * (SIDE * v + EDGE_MARGIN)) * s + 2.0 * extra).min(self.screen_w).ceil();
        let h = ((GAP_BOTTOM + (v * BAR - self.cfg.icon) / 2.0 + self.cfg.magnified.max(self.cfg.icon * 1.55) + TIP_SPACE) * s).ceil();
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
                    s.appear.to(1.0, INSERT_S, ease_out);
                }
                next.push(s);
            } else {
                // Critically damped: neighbours slide apart in about 0.4 s, no overshoot.
                let mut appear = Tween::new(0.0);
                if self.initialized {
                    appear.to(1.0, INSERT_S, ease_out);
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
                s.appear.to(0.0, REMOVE_S, smoothstep);
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

    /// While a Dock menu is open the zoom holds; it collapses when the menu
    /// closes with the pointer outside.
    pub fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        if open {
            self.tip.to(0.0, LABEL_OUT_S, |u| u);
        } else if self.mouse.is_none() {
            self.mag.to(0.0, MAG_OUT_S, smoothstep);
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
                cells.push((Cell::Separator, SEP_W * self.v() * s));
                sep_done = true;
            }
            cells.push((Cell::Slot(i), tile0 * sl.appear.value.max(0.0)));
        }
        let total0: f32 = cells.iter().map(|c| c.1).sum();
        let start0 = (win_w - total0) / 2.0;
        let amount = if self.cfg.magnify { self.mag.value.clamp(0.0, 1.0) } else { 0.0 };
        let maxf = (self.cfg.magnified / self.cfg.icon).max(1.0);
        let mx = self.last_mx;
        // The gap between icon boxes keeps its width; boxes and the separator scale.
        let gap0 = tile0 - icon0;
        let mut cum0 = start0;
        let mut widths = Vec::with_capacity(cells.len());
        for (c, w0) in &cells {
            let f = 1.0 + (maxf - 1.0) * amount * bell((mx - (cum0 + w0 / 2.0)) / tile0);
            cum0 += w0;
            widths.push(match c {
                Cell::Slot(i) => {
                    let a = self.slots[*i].appear.value.max(0.0);
                    (tile0 - gap0) * a * f + gap0 * a
                }
                Cell::Separator => w0 * f,
            });
        }
        let total: f32 = widths.iter().sum();
        // Centred on the screen whatever the cursor does (measured).
        let start = (win_w - total) / 2.0;
        let mut x = start;
        let mut out = Vec::with_capacity(cells.len());
        for ((c, _), w) in cells.iter().zip(&widths) {
            out.push((*c, x, *w));
            x += w;
        }
        let v = icon0 * ART;
        let bh = v * BAR;
        let side = SIDE * v;
        // Whole pixels, so the hairline rim stays one crisp device pixel.
        let (x0, x1) = ((start - side).round(), (start + total + side).round());
        let bh = bh.round();
        let bg = (x0, (win_h - GAP_BOTTOM * s - bh).round(), x1 - x0, bh);
        // The box bottom: the tile is centred in the bar, the box adds the canvas margin.
        let icon_bottom = bg.1 + bh / 2.0 + icon0 / 2.0;
        Geo { cells: out, bg, icon_bottom, icon0 }
    }

    fn icon_box(&self, g: &Geo, slot: usize, x: f32, w: f32) -> (f32, f32, f32) {
        let sl = &self.slots[slot];
        let a = sl.appear.value.clamp(0.0, 1.0);
        // The cell is the box plus a gap that never scales (see geo).
        let size = (w - g.icon0 * (TILE - 1.0) * a).max(0.0);
        let lift = sl.bounce.map(launch_bounce).unwrap_or(0.0) * g.icon0 * ART * HOP;
        (x + (w - size) / 2.0, g.icon_bottom - size - lift, size)
    }

    fn refresh_hover(&mut self) {
        let wr = self.window_rect();
        let Some((mx, my)) = self.mouse else {
            self.hover = None;
            self.tip.to(0.0, LABEL_OUT_S, |u| u);
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
            self.tip.to(1.0, LABEL_IN_S, ease_out);
        } else {
            self.tip.to(0.0, LABEL_OUT_S, |u| u);
        }
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = Some((x, y));
        self.last_mx = x;
        self.mag.to(1.0, MAG_IN_S, smoothstep);
        self.refresh_hover();
    }

    pub fn mouse_leave(&mut self) {
        self.mouse = None;
        if !self.menu_open {
            self.mag.to(0.0, MAG_OUT_S, smoothstep);
        }
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

    /// The drawn Downloads folder and Trash for this appearance.
    fn stack_images(&self, dark: bool) -> Option<(Rc<Image>, Rc<Image>)> {
        let mut cache = self.stacks.borrow_mut();
        if cache.as_ref().is_none_or(|(d, _, _)| *d != dark) {
            let n = 256.0;
            let mut folder = Pixmap::new(n as u32, n as u32)?;
            glyphs::stack_folder(&mut folder, 0.0, 0.0, n);
            let mut trash = Pixmap::new(n as u32, n as u32)?;
            glyphs::stack_trash(&mut trash, 0.0, 0.0, n, dark);
            *cache = Some((dark, Rc::new(Image::with_shadow(folder)), Rc::new(Image::new(trash))));
        }
        cache.as_ref().map(|(_, f, t)| (f.clone(), t.clone()))
    }

    /// Blurred colour of what is behind window point `x, y`.
    fn behind(&self, ctx: &Ctx, wr: WinRect, x: f32, y: f32) -> Option<[f32; 3]> {
        match (&self.backdrop, &ctx.vibrancy) {
            (Some(b), _) => Some(b.vib.sample_frost(x, y)),
            (None, Some(v)) => Some(v.sample_frost(wr.x as f32 + x, wr.y as f32 + y)),
            (None, None) => None,
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
        let v0 = g.icon0 * ART;

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

        // No shadow: macOS 26 draws none under the dock.
        let shape = glass::Shape { x: bx, y: by, w: bw, h: bh, r: bh * RADIUS };
        match (&self.backdrop, &ctx.vibrancy) {
            (Some(b), _) => b.dock(pm, shape, th.dark, s),
            (None, Some(v)) => glass::dock_slab(pm, v, wr.x as f32, wr.y as f32, shape, th.dark, s),
            (None, None) => {
                if let Some(path) = gfx::squircle(bx, by, bw, bh, shape.r) {
                    gfx::fill(pm, &path, th.dock_fallback());
                }
            }
        }

        for (c, x, w) in &g.cells {
            match c {
                Cell::Separator => {
                    let h = SEP_H * v0;
                    let lw = s.round().max(1.0);
                    // Lighter than the glass in dark mode, darker in light mode,
                    // by a fixed step rather than a tint (measured).
                    add_rect(pm, (x + w / 2.0 - lw / 2.0).round(), (by + (bh - h) / 2.0).round(), lw, h.round(), if th.dark { 66.0 } else { -57.0 });
                }
                Cell::Slot(i) => {
                    let sl = &self.slots[*i];
                    let (ix, iy, size) = self.icon_box(&g, *i, *x, *w);
                    if size < 1.0 {
                        continue;
                    }
                    let alpha = sl.appear.value.clamp(0.0, 1.0);
                    let darken = self.pressed == Some(*i) && self.hover == Some(*i);
                    let stack = match sl.e.fallback {
                        Fallback::Folder | Fallback::Trash if sl.e.icon.is_none() => self.stack_images(th.dark),
                        _ => None,
                    };
                    match (&sl.e.icon, stack, sl.e.fallback) {
                        (Some(img), _, _) => img.draw(pm, ix, iy, size, alpha, darken),
                        (None, Some((folder, _)), Fallback::Folder) => folder.draw(pm, ix, iy, size, alpha, darken),
                        (None, Some((_, trash)), Fallback::Trash) => trash.draw(pm, ix, iy, size, alpha, darken),
                        _ => glyphs::app_tile(pm, ix, iy, size),
                    }
                    if sl.e.running && alpha > 0.5 {
                        let cx = ix + size / 2.0;
                        // Under the visible tile at rest, not under the icon's canvas margin.
                        let art_bottom = iy + size - size * (1.0 - ART) / 2.0;
                        let cy = art_bottom.max(g.icon_bottom - g.icon0 * (1.0 - ART) / 2.0) + DOT_BELOW * v0;
                        if let Some(p) = gfx::circle(cx, cy, 2.0 * s) {
                            let dot = if th.dark { rgba(255, 255, 255, 0.88) } else { rgba(0, 0, 0, 0.88) };
                            gfx::fill(pm, &p, gfx::fade(dot, alpha));
                        }
                    }
                }
            }
        }

        // Label above the hovered icon: a glass capsule with a pointer.
        if let Some(t) = self.tip_slot
            && self.tip.value > 0.01
            && t < self.slots.len()
            && let Some((_, x, w)) = g.cells.iter().find(|c| matches!(c.0, Cell::Slot(i) if i == t))
        {
            let (ix, iy, size) = self.icon_box(&g, t, *x, *w);
            // The label rides the launch hop with its icon (measured).
            let lift = g.icon_bottom - size - iy;
            let a = self.tip.value.clamp(0.0, 1.0);
            let fs = 13.0 * s;
            let label = ctx.text.ellipsize(&self.slots[t].e.label, fs, Weight::Regular, 280.0 * s);
            let tw = ctx.text.measure(&label, fs, Weight::Regular);
            let (pw, ph) = ((tw + 24.0 * s).round(), (23.0 * s).round());
            let (tip_w, tip_h) = (11.0 * s, 5.5 * s);
            // The bar's top, or the tile's when magnification lifts it out of the bar.
            let rest_top = g.icon_bottom - size + size * (1.0 - ART) / 2.0;
            let anchor = by.min(rest_top - v0 * (BAR - 1.0) / 2.0) - lift;
            let cx = ix + size / 2.0;
            let px = (cx - pw / 2.0).clamp(2.0, ww - pw - 2.0).round();
            let py = (anchor - 9.0 * s - tip_h - ph + (1.0 - a) * LABEL_RISE * s).round();
            if let Some(path) = glyphs::bubble(px, py, pw, ph, cx, tip_w, tip_h) {
                // Dark glass: the blurred backdrop at about 0.45; light glass: about 67% white.
                let tone = |c: [f32; 3]| {
                    let f = |v: f32| if th.dark { v * 0.45 } else { v + (255.0 - v) * 0.67 };
                    tiny_skia::Color::from_rgba8(f(c[0]) as u8, f(c[1]) as u8, f(c[2]) as u8, 255)
                };
                let my = py + ph / 2.0;
                let samples: Option<Vec<tiny_skia::Color>> =
                    [px + ph / 2.0, px + pw / 2.0, px + pw - ph / 2.0].iter().map(|&sx| self.behind(ctx, wr, sx, my).map(tone)).collect();
                match samples.and_then(|c| {
                    tiny_skia::LinearGradient::new(
                        tiny_skia::Point::from_xy(px + ph / 2.0, my),
                        tiny_skia::Point::from_xy(px + pw - ph / 2.0, my),
                        c.iter().enumerate().map(|(k, c)| tiny_skia::GradientStop::new(k as f32 * 0.5, gfx::fade(*c, a))).collect(),
                        tiny_skia::SpreadMode::Pad,
                        tiny_skia::Transform::identity(),
                    )
                }) {
                    Some(sh) => gfx::fill_shader(pm, &path, sh),
                    None => gfx::fill(pm, &path, gfx::fade(th.tooltip(), a)),
                }
                let rim = if th.dark { rgba(255, 255, 255, 0.16) } else { rgba(255, 255, 255, 0.5) };
                gfx::stroke(pm, &path, gfx::fade(rim, a), (0.5 * s).max(1.0));
            }
            let base = py + ph / 2.0 + ctx.text.cap_height(fs) / 2.0;
            ctx.text.draw(pm, &label, px + (pw - tw) / 2.0, base, fs, Weight::Regular, gfx::fade(th.label(), a), None);
        }
    }
}

/// Adds `delta` (0..255 steps, negative darkens) to the colour of an opaque
/// region of a premultiplied pixmap, keeping its alpha.
fn add_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, delta: f32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = ((x as i32).clamp(0, pw), ((x + w) as i32).clamp(0, pw));
    let (y0, y1) = ((y as i32).clamp(0, ph), ((y + h) as i32).clamp(0, ph));
    let stride = pw as usize * 4;
    let data = pm.data_mut();
    for yy in y0..y1 {
        for xx in x0..x1 {
            let o = yy as usize * stride + xx as usize * 4;
            let a = data[o + 3] as f32;
            for k in 0..3 {
                data[o + k] = (data[o + k] as f32 + delta * a / 255.0).clamp(0.0, a).round() as u8;
            }
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
        let peak = DockConfig::default().icon * ART * HOP;
        assert!(icon_y(&d, "b") < rest - peak * 0.95, "icon should be at the top of the hop");
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
    fn magnification_follows_the_measured_profile_and_timing() {
        let mut d = Dock::new(DockConfig { magnify: true, icon: 52.0, magnified: 104.0 });
        d.set_entries((0..8).map(|i| entry(&i.to_string())).collect());
        let wr = d.window_rect();
        let (ww, wh) = (wr.w as f32, wr.h as f32);
        let g0 = d.geo(ww, wh);
        let (_, x, w) = g0.cells[3];
        d.mouse_move(x + w / 2.0, wh - 20.0);
        // Fully on after 0.08 s.
        for _ in 0..10 {
            d.update(1.0 / 120.0);
        }
        assert_eq!(d.mag.value, 1.0);
        let g1 = d.geo(ww, wh);
        let box_of = |g: &Geo, i: usize| g.cells[i].2 - (g0.cells[i].2 - d.cfg.icon);
        // Box scale at 0, 1, 2 slots: 2, 1.87, 1.5 (cos(pi d / 6)).
        for (i, want) in [(3, 2.0), (2, 1.866), (4, 1.866), (1, 1.5), (5, 1.5), (7, 1.0)] {
            let got = box_of(&g1, i) / d.cfg.icon;
            assert!((got - want).abs() < 0.01, "slot {i}: {got} vs {want}");
        }
        // Centred on the window whatever the cursor does, and inside it.
        let (first, last) = (g1.cells[0], g1.cells[g1.cells.len() - 1]);
        assert!(((first.1 + last.1 + last.2) / 2.0 - ww / 2.0).abs() < 0.5);
        assert!(g1.bg.0 >= 0.0 && g1.bg.0 + g1.bg.2 <= ww);
        // Collapses in 0.17 s: half way at 85 ms.
        d.mouse_leave();
        let mut t = 0.0;
        while t < 0.085 - 1e-4 {
            d.update(1.0 / 200.0);
            t += 1.0 / 200.0;
        }
        assert!((d.mag.value - 0.5).abs() < 0.03, "{}", d.mag.value);
        for _ in 0..240 {
            d.update(1.0 / 120.0);
        }
        assert!(d.tip.value < 0.01 && !d.animating());
    }

    #[test]
    fn bar_follows_the_measured_proportions() {
        let mut d = Dock::new(DockConfig::default());
        d.set_entries(vec![entry("a"), entry("b"), entry("c")]);
        let wr = d.window_rect();
        let g = d.geo(wr.w as f32, wr.h as f32);
        let v = d.cfg.icon * ART;
        let (bx, by, _, bh) = g.bg;
        // The bar is snapped to whole pixels.
        assert!((bh - v * BAR).abs() <= 0.5);
        assert!((wr.h as f32 - (by + bh) - GAP_BOTTOM).abs() <= 0.5);
        let (_, x0, w0) = g.cells[0];
        let (_, x1, _) = g.cells[1];
        assert!(((x1 - x0) / v - 1.31).abs() < 1e-3, "pitch");
        // End padding equals the gap between tiles.
        let first_tile = x0 + (w0 - v) / 2.0;
        assert!(((first_tile - bx) - 0.31 * v).abs() <= 0.5);
        // The tile is centred in the bar.
        let tile_top = g.icon_bottom - g.icon0 + g.icon0 * (1.0 - ART) / 2.0;
        assert!(((tile_top - by) - (by + bh - (tile_top + v))).abs() < 1e-3);
        assert!((g.bg.0.fract(), by.fract(), bh.fract()) == (0.0, 0.0, 0.0));
    }

    #[test]
    fn new_item_slides_in_over_about_0_4_s_without_overshoot() {
        let mut d = Dock::new(DockConfig::default());
        d.set_entries(vec![entry("a")]);
        d.set_entries(vec![entry("a"), entry("b")]);
        let (mut t, mut peak, mut at95) = (0.0f32, 0.0f32, None);
        while d.animating() && t < 2.0 {
            d.update(1.0 / 120.0);
            t += 1.0 / 120.0;
            let v = d.slots[1].appear.value;
            peak = peak.max(v);
            if v >= 0.95 && at95.is_none() {
                at95 = Some(t);
            }
        }
        assert!(peak <= 1.0 + 1e-4, "overshoot {peak}");
        // Ease-out over 0.4 s: most of the way early, settled at 0.4 s.
        let at95 = at95.unwrap();
        assert!((0.2..0.3).contains(&at95), "95% at {at95} s");
        assert!((t - 0.4).abs() < 0.02, "settled at {t} s");
    }
}
