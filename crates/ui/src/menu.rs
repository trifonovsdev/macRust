//! Pull-down and context menus.

use tiny_skia::Pixmap;

use crate::anim::Spring;
use crate::gfx::{self, rgba};
use crate::text::Weight;
use crate::{Ctx, Key, glyphs};

#[derive(Clone, Debug)]
pub struct MenuEntry {
    pub label: String,
    pub shortcut: String,
    pub tag: u32,
    pub enabled: bool,
    pub checked: bool,
    pub separator: bool,
}

impl MenuEntry {
    pub fn item(label: impl Into<String>, tag: u32) -> Self {
        Self { label: label.into(), shortcut: String::new(), tag, enabled: true, checked: false, separator: false }
    }
    pub fn sep() -> Self {
        Self { separator: true, enabled: false, ..Self::item("", 0) }
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        self.shortcut = s.into();
        self
    }
}

#[derive(Debug, PartialEq)]
pub enum MenuEvent {
    Pick(u32),
}

// Points.
const ITEM_H: f32 = 22.0;
const SEP_H: f32 = 11.0;
const VPAD: f32 = 5.0;
const LEFT: f32 = 22.0;
const RIGHT: f32 = 16.0;
const FONT: f32 = 13.0;
const RADIUS: f32 = 8.0;
/// Room for the shadow around the body.
pub const MARGIN_X: f32 = 24.0;
pub const MARGIN_TOP: f32 = 12.0;
pub const MARGIN_BOTTOM: f32 = 34.0;
const FLASH: f32 = 0.13;

pub struct PopupMenu {
    entries: Vec<MenuEntry>,
    scale: f32,
    hover: Option<usize>,
    alpha: Spring,
    closing: bool,
    flash: Option<(usize, f32)>,
    body_w: f32,
    body_h: f32,
    /// Opaque id of the owner, so the host knows which menu this is.
    pub owner: u32,
    pub events: Vec<MenuEvent>,
}

impl PopupMenu {
    pub fn new(entries: Vec<MenuEntry>, owner: u32, scale: f32, ctx: &Ctx) -> Self {
        let s = scale;
        let mut text_w: f32 = 0.0;
        let mut short_w: f32 = 0.0;
        let mut h = VPAD * 2.0 * s;
        for e in &entries {
            if e.separator {
                h += SEP_H * s;
                continue;
            }
            h += ITEM_H * s;
            text_w = text_w.max(ctx.text.measure(&e.label, FONT * s, Weight::Regular));
            if !e.shortcut.is_empty() {
                short_w = short_w.max(ctx.text.measure(&e.shortcut, FONT * s, Weight::Regular));
            }
        }
        let gap = if short_w > 0.0 { 28.0 * s } else { 0.0 };
        let body_w = (LEFT * s + text_w + gap + short_w + RIGHT * s).max(170.0 * s).ceil();
        let mut alpha = Spring::new(0.0, 0.14, 1.0).epsilon(0.002);
        alpha.to(1.0);
        Self { entries, scale, hover: None, alpha, closing: false, flash: None, body_w, body_h: h.ceil(), owner, events: Vec::new() }
    }

    /// Window size including the shadow margins.
    pub fn window_size(&self) -> (i32, i32) {
        let s = self.scale;
        ((self.body_w + 2.0 * MARGIN_X * s).ceil() as i32, (self.body_h + (MARGIN_TOP + MARGIN_BOTTOM) * s).ceil() as i32)
    }

    /// Offset of the menu body's top-left corner inside the window.
    pub fn body_offset(&self) -> (i32, i32) {
        ((MARGIN_X * self.scale).round() as i32, (MARGIN_TOP * self.scale).round() as i32)
    }

    pub fn body_size(&self) -> (f32, f32) {
        (self.body_w, self.body_h)
    }

    pub fn close(&mut self) {
        if !self.closing {
            self.closing = true;
            self.alpha = Spring::new(self.alpha.value, 0.24, 1.0).epsilon(0.002);
            self.alpha.to(0.0);
        }
    }

    pub fn closing(&self) -> bool {
        self.closing
    }

    /// The fade-out finished; the host can hide the window.
    pub fn finished(&self) -> bool {
        self.closing && self.alpha.settled()
    }

    pub fn animating(&self) -> bool {
        !self.alpha.settled() || self.flash.is_some()
    }

    pub fn update(&mut self, dt: f32) {
        self.alpha.step(dt);
        if let Some((i, t)) = self.flash {
            let t = t + dt;
            if t >= FLASH {
                self.flash = None;
                self.events.push(MenuEvent::Pick(self.entries[i].tag));
                self.close();
            } else {
                self.flash = Some((i, t));
            }
        }
    }

    fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        let (ox, oy) = self.body_offset();
        let (bx, by) = (x - ox as f32, y - oy as f32);
        if bx < 0.0 || bx > self.body_w {
            return None;
        }
        let s = self.scale;
        let mut cy = VPAD * s;
        for (i, e) in self.entries.iter().enumerate() {
            let h = if e.separator { SEP_H } else { ITEM_H } * s;
            if by >= cy && by < cy + h {
                return (!e.separator && e.enabled).then_some(i);
            }
            cy += h;
        }
        None
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if self.flash.is_none() && !self.closing {
            self.hover = self.item_at(x, y);
        }
    }

    pub fn mouse_leave(&mut self) {
        if self.flash.is_none() {
            self.hover = None;
        }
    }

    /// Returns true if the click hit an item.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> bool {
        if self.closing || self.flash.is_some() {
            return true;
        }
        match self.item_at(x, y) {
            Some(i) => {
                self.hover = Some(i);
                self.flash = Some((i, 0.0));
                true
            }
            None => false,
        }
    }

    pub fn key(&mut self, key: Key) {
        if self.closing || self.flash.is_some() {
            return;
        }
        let selectable: Vec<usize> =
            self.entries.iter().enumerate().filter(|(_, e)| !e.separator && e.enabled).map(|(i, _)| i).collect();
        if selectable.is_empty() {
            return;
        }
        let pos = self.hover.and_then(|h| selectable.iter().position(|&i| i == h));
        match key {
            Key::Down => {
                self.hover = Some(match pos {
                    Some(p) => selectable[(p + 1).min(selectable.len() - 1)],
                    None => selectable[0],
                })
            }
            Key::Up => {
                self.hover = Some(match pos {
                    Some(p) => selectable[p.saturating_sub(1)],
                    None => selectable[selectable.len() - 1],
                })
            }
            Key::Enter => {
                if let Some(h) = self.hover {
                    self.flash = Some((h, 0.0));
                }
            }
            _ => {}
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let s = self.scale;
        let th = ctx.theme;
        let a = self.alpha.value.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let (ox, oy) = self.body_offset();
        let (bx, by, bw, bh) = (ox as f32, oy as f32, self.body_w, self.body_h);
        let r = RADIUS * s;
        ctx.shadow(r, 14.0 * s).draw(pm, bx, by + 8.0 * s, bw, bh, gfx::fade(th.shadow(), a * 0.9));
        ctx.shadow(r, 1.5 * s).draw(pm, bx, by, bw, bh, gfx::fade(rgba(0, 0, 0, 0.12), a));
        if let Some(p) = gfx::squircle(bx, by, bw, bh, r) {
            gfx::fill(pm, &p, gfx::fade(th.panel(), a));
            gfx::stroke(pm, &p, gfx::fade(th.panel_border(), a), 1.0);
        }
        if th.dark
            && let Some(p) = gfx::squircle(bx + 1.0, by + 1.0, bw - 2.0, bh - 2.0, r - 1.0)
        {
            gfx::stroke(pm, &p, gfx::fade(th.panel_rim(), a), 1.0);
        }
        let fs = FONT * s;
        let mut cy = by + VPAD * s;
        for (i, e) in self.entries.iter().enumerate() {
            if e.separator {
                let ly = (cy + SEP_H * s / 2.0).floor();
                gfx::fill_rect(pm, bx + 10.0 * s, ly, bw - 20.0 * s, s.max(1.0).round(), gfx::fade(th.separator(), a));
                cy += SEP_H * s;
                continue;
            }
            let h = ITEM_H * s;
            let flash_off = matches!(self.flash, Some((fi, t)) if fi == i && t < FLASH / 2.0);
            let lit = self.hover == Some(i) && e.enabled && !flash_off;
            if lit && let Some(p) = gfx::squircle(bx + 5.0 * s, cy, bw - 10.0 * s, h, 4.0 * s) {
                gfx::fill(pm, &p, gfx::fade(th.accent(), a));
            }
            let color = if lit {
                rgba(255, 255, 255, a)
            } else if e.enabled {
                gfx::fade(th.label(), a)
            } else {
                gfx::fade(th.tertiary(), a)
            };
            let base = (cy + h / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
            if e.checked {
                glyphs::checkmark(pm, bx + 7.0 * s, cy + h / 2.0 - 6.0 * s, 12.0 * s, color);
            }
            let max_label = bw - (LEFT + RIGHT) * s;
            let label = ctx.text.ellipsize(&e.label, fs, Weight::Regular, max_label);
            ctx.text.draw(pm, &label, bx + LEFT * s, base, fs, Weight::Regular, color, None);
            if !e.shortcut.is_empty() {
                let sw = ctx.text.measure(&e.shortcut, fs, Weight::Regular);
                let sc = if lit { rgba(255, 255, 255, 0.85 * a) } else { gfx::fade(th.secondary(), a) };
                ctx.text.draw(pm, &e.shortcut, bx + bw - RIGHT * s - sw, base, fs, Weight::Regular, sc, None);
            }
            cy += h;
        }
    }
}
