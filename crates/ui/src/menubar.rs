//! The menu bar along the top edge of the screen.

use std::cell::RefCell;

use tiny_skia::Pixmap;

use crate::gfx::{self, rgba};
use crate::text::Weight;
use crate::{Ctx, WinRect, glyphs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarItem {
    Logo,
    App,
    Window,
    Lang,
    Battery,
    Wifi,
    Search,
    Clock,
}

impl BarItem {
    pub fn is_menu(self) -> bool {
        matches!(self, BarItem::Logo | BarItem::App | BarItem::Window)
    }
}

#[derive(Debug, PartialEq)]
pub enum BarEvent {
    /// Open the menu of `item`; `x` is its left edge and `bottom` the bar's bottom, window px.
    Open { item: BarItem, x: f32, bottom: f32 },
    CloseMenus,
    Click(BarItem),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub app: String,
    pub lang: String,
    /// Charge percent and whether it is charging; `None` without a battery.
    pub battery: Option<(u8, bool)>,
    /// Connected to a network; `None` when unknown.
    pub wifi: Option<bool>,
    pub clock: String,
}

/// Wallpaper id, dark text, width, height.
type MaterialKey = (u64, bool, u32, u32);

pub const HEIGHT: f32 = 24.0;
const FONT: f32 = 13.0;

pub struct MenuBar {
    scale: f32,
    width: f32,
    status: Status,
    items: Vec<(BarItem, f32, f32)>,
    hover: Option<BarItem>,
    pressed: Option<BarItem>,
    open: Option<BarItem>,
    dark_text: bool,
    material: RefCell<Option<(MaterialKey, Pixmap)>>,
    pub events: Vec<BarEvent>,
}

impl MenuBar {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            width: 1920.0,
            status: Status::default(),
            items: Vec::new(),
            hover: None,
            pressed: None,
            open: None,
            dark_text: true,
            material: RefCell::new(None),
            events: Vec::new(),
        }
    }

    pub fn set_screen(&mut self, w: f32, scale: f32, ctx: &Ctx) {
        self.width = w;
        self.scale = scale;
        self.relayout(ctx);
    }

    pub fn height_px(&self) -> i32 {
        (HEIGHT * self.scale).round() as i32
    }

    pub fn window_rect(&self) -> WinRect {
        WinRect { x: 0, y: 0, w: self.width as i32, h: self.height_px() }
    }

    /// Returns true when something visible changed.
    pub fn set_status(&mut self, st: Status, ctx: &Ctx) -> bool {
        if st == self.status {
            return false;
        }
        self.status = st;
        self.relayout(ctx);
        true
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Picks dark or light text from the wallpaper behind the bar.
    pub fn update_contrast(&mut self, ctx: &Ctx) {
        self.dark_text = match &ctx.vibrancy {
            Some(v) => v.luma(0.0, 0.0, self.width, HEIGHT * self.scale) > 0.52,
            None => !ctx.theme.dark,
        };
    }

    pub fn set_open(&mut self, item: Option<BarItem>) {
        self.open = item;
    }

    pub fn open_item(&self) -> Option<BarItem> {
        self.open
    }

    fn relayout(&mut self, ctx: &Ctx) {
        let s = self.scale;
        let fs = FONT * s;
        let t = &ctx.text;
        let pad = 9.0 * s;
        let mut items = Vec::new();
        let mut x = 6.0 * s;
        items.push((BarItem::Logo, x, x + 34.0 * s));
        x += 34.0 * s;
        if !self.status.app.is_empty() {
            let w = t.measure(&self.status.app, fs, Weight::Bold) + 2.0 * pad;
            items.push((BarItem::App, x, x + w));
            x += w;
        }
        let w = t.measure(ctx.strings.window, fs, Weight::Regular) + 2.0 * pad;
        items.push((BarItem::Window, x, x + w));

        let mut r = self.width - 6.0 * s;
        let mut push_right = |item: BarItem, w: f32, items: &mut Vec<(BarItem, f32, f32)>| {
            items.push((item, r - w, r));
            r -= w;
        };
        push_right(BarItem::Clock, t.measure(&self.status.clock, fs, Weight::Regular) + 2.0 * pad, &mut items);
        push_right(BarItem::Search, 30.0 * s, &mut items);
        if self.status.wifi.is_some() {
            push_right(BarItem::Wifi, 30.0 * s, &mut items);
        }
        if let Some((pct, _)) = self.status.battery {
            let tw = t.measure(&format!("{pct}%"), fs, Weight::Regular);
            push_right(BarItem::Battery, tw + 34.0 * s + 2.0 * pad, &mut items);
        }
        if !self.status.lang.is_empty() {
            push_right(BarItem::Lang, t.measure(&self.status.lang, fs, Weight::Medium) + 2.0 * pad, &mut items);
        }
        self.items = items;
    }

    fn item_at(&self, x: f32) -> Option<(BarItem, f32)> {
        self.items.iter().find(|(_, a, b)| x >= *a && x < *b).map(|(i, a, _)| (*i, *a))
    }

    fn bottom(&self) -> f32 {
        self.height_px() as f32
    }

    pub fn mouse_move(&mut self, x: f32, _y: f32) {
        let hit = self.item_at(x);
        self.hover = hit.map(|h| h.0);
        // While a menu is open, sliding across titles switches menus.
        if let (Some(open), Some((item, ix))) = (self.open, hit)
            && item.is_menu()
            && item != open
        {
            self.open = Some(item);
            self.events.push(BarEvent::Open { item, x: ix, bottom: self.bottom() });
        }
    }

    pub fn mouse_leave(&mut self) {
        self.hover = None;
        self.pressed = None;
    }

    pub fn mouse_down(&mut self, x: f32, _y: f32) {
        let Some((item, ix)) = self.item_at(x) else {
            if self.open.is_some() {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            }
            return;
        };
        if item.is_menu() {
            if self.open == Some(item) {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            } else {
                self.open = Some(item);
                self.events.push(BarEvent::Open { item, x: ix, bottom: self.bottom() });
            }
        } else {
            self.pressed = Some(item);
        }
    }

    pub fn mouse_up(&mut self, x: f32, _y: f32) {
        if let Some(p) = self.pressed.take()
            && self.item_at(x).map(|h| h.0) == Some(p)
        {
            if self.open.is_some() {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            }
            self.events.push(BarEvent::Click(p));
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let s = self.scale;
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) else { return };
        let full = tiny_skia::PathBuilder::from_rect(rect);
        let fallback = if ctx.theme.dark { rgba(30, 30, 32, 0.9) } else { rgba(240, 240, 242, 0.9) };
        match &ctx.vibrancy {
            Some(v) => {
                let key = (v.id, self.dark_text, pm.width(), pm.height());
                let mut cache = self.material.borrow_mut();
                if cache.as_ref().is_none_or(|(k, _)| *k != key) {
                    let tint = if self.dark_text { rgba(255, 255, 255, 0.26) } else { rgba(0, 0, 0, 0.14) };
                    *cache = v.material(0.0, 0.0, pm.width(), pm.height(), tint).map(|m| (key, m));
                }
                match cache.as_ref() {
                    Some((_, m)) => pm.data_mut().copy_from_slice(m.data()),
                    None => gfx::fill(pm, &full, fallback),
                }
            }
            None => gfx::fill(pm, &full, fallback),
        }
        let fg = if self.dark_text { rgba(0, 0, 0, 0.86) } else { rgba(255, 255, 255, 0.96) };
        let lit_bg = if self.dark_text { rgba(0, 0, 0, 0.11) } else { rgba(255, 255, 255, 0.22) };
        let fs = FONT * s;
        let base = (h / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
        let pad = 9.0 * s;
        for &(item, x0, x1) in &self.items {
            let lit = self.open == Some(item) || (self.pressed == Some(item) && self.hover == Some(item));
            if lit && let Some(p) = gfx::squircle(x0, 3.0 * s, x1 - x0, h - 6.0 * s, 5.0 * s) {
                gfx::fill(pm, &p, lit_bg);
            }
            let cx = (x0 + x1) / 2.0;
            match item {
                BarItem::Logo => glyphs::logo(pm, (cx - 7.5 * s).round(), (h / 2.0 - 7.5 * s).round(), 15.0 * s, fg),
                BarItem::App => {
                    ctx.text.draw(pm, &self.status.app, x0 + pad, base, fs, Weight::Bold, fg, None);
                }
                BarItem::Window => {
                    ctx.text.draw(pm, ctx.strings.window, x0 + pad, base, fs, Weight::Regular, fg, None);
                }
                BarItem::Lang => {
                    ctx.text.draw(pm, &self.status.lang, x0 + pad, base, fs, Weight::Medium, fg, None);
                }
                BarItem::Battery => {
                    if let Some((pct, charging)) = self.status.battery {
                        let tw = ctx.text.draw(pm, &format!("{pct}%"), x0 + pad, base, fs, Weight::Regular, fg, None);
                        let (bw, bh) = (26.0 * s, 12.0 * s);
                        glyphs::battery(pm, (x0 + pad + tw + 6.0 * s).round(), (h / 2.0 - bh / 2.0).round(), bw, bh, fg, pct, charging);
                    }
                }
                BarItem::Wifi => {
                    let gs = 17.0 * s;
                    glyphs::wifi(pm, cx - gs / 2.0, h / 2.0 - gs * 0.5, gs, fg, self.status.wifi.unwrap_or(false));
                }
                BarItem::Search => {
                    let gs = 14.0 * s;
                    glyphs::magnifier(pm, cx - gs / 2.0, h / 2.0 - gs / 2.0, gs, fg);
                }
                BarItem::Clock => {
                    ctx.text.draw(pm, &self.status.clock, x0 + pad, base, fs, Weight::Regular, fg, None);
                }
            }
        }
    }
}

impl Default for MenuBar {
    fn default() -> Self {
        Self::new()
    }
}
