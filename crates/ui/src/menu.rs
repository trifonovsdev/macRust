//! Pull-down and context menus.

use tiny_skia::{Color, Pixmap};

use crate::gfx::{self, rgba};
use crate::glass::{self, Backdrop, GlassStyle, Shape};
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

// Points, measured on Tahoe 26.0 menus (TAHOE-SPEC.md section 3).
const ITEM_H: f32 = 22.0;
const SEP_H: f32 = 10.0;
/// Space above the first and below the last row; the selection sits this far
/// from the panel edges on all sides.
const VPAD: f32 = 5.0;
const INSET: f32 = 5.0;
/// Text start without and with a checkmark column.
const LEFT: f32 = 16.0;
const LEFT_CHECK: f32 = 26.0;
const RIGHT: f32 = 14.0;
const SHORTCUT_GAP: f32 = 24.0;
const SEP_INSET: f32 = 14.0;
const FONT: f32 = 13.0;
const RADIUS: f32 = 12.5;
const SEL_RADIUS: f32 = 5.0;
/// Room for the shadow around the body.
pub const MARGIN_X: f32 = 24.0;
pub const MARGIN_TOP: f32 = 12.0;
pub const MARGIN_BOTTOM: f32 = 34.0;
/// The picked row blinks off for half of this before the menu closes.
const FLASH: f32 = 0.13;
/// macOS menus appear at once and fade out briefly.
const FADE_OUT: f32 = 0.2;

fn selection(dark: bool) -> Color {
    // Tahoe darkens the accent in dark menus: (20,80,178), not systemBlue.
    if dark { rgba(20, 80, 178, 1.0) } else { rgba(0, 122, 255, 1.0) }
}

pub(crate) fn text_color(dark: bool) -> Color {
    if dark { rgba(242, 242, 242, 1.0) } else { rgba(0, 0, 0, 0.85) }
}

pub(crate) fn secondary(dark: bool) -> Color {
    if dark { rgba(255, 255, 255, 0.5) } else { rgba(0, 0, 0, 0.45) }
}

pub(crate) fn disabled(dark: bool) -> Color {
    if dark { rgba(255, 255, 255, 0.25) } else { rgba(0, 0, 0, 0.25) }
}

pub(crate) fn separator(dark: bool) -> Color {
    if dark { rgba(255, 255, 255, 0.14) } else { rgba(0, 0, 0, 0.1) }
}

/// The menu sheet: shadow, near-opaque glass over `backdrop` (a flat fill
/// without one), a dark 1 px ring outside and a 0.5 pt light hairline inside.
pub(crate) fn sheet(pm: &mut Pixmap, ctx: &Ctx, backdrop: Option<&Backdrop>, shape: Shape, s: f32) {
    let dark = ctx.theme.dark;
    let Shape { x, y, w, h, r } = shape;
    ctx.shadow(r, 14.0 * s).draw(pm, x, y + 8.0 * s, w, h, gfx::fade(ctx.theme.shadow(), 0.7));
    match backdrop {
        Some(b) => b.draw(pm, shape, &GlassStyle::menu(dark, s)),
        None => {
            let fill = if dark { rgba(22, 22, 22, 1.0) } else { rgba(240, 240, 242, 1.0) };
            glass::hairline(pm, shape, 0.0, 1e4, fill, fill);
        }
    }
    let ring = if dark { rgba(0, 0, 0, 0.6) } else { rgba(0, 0, 0, 0.14) };
    glass::hairline(pm, shape, -1.0, 0.0, ring, ring);
    let rim = if dark { rgba(255, 255, 255, 0.4) } else { rgba(255, 255, 255, 0.7) };
    glass::hairline(pm, shape, 0.0, 0.5 * s, rim, rim);
}

pub struct PopupMenu {
    entries: Vec<MenuEntry>,
    scale: f32,
    hover: Option<usize>,
    /// Seconds since `close`, while fading out.
    closing: Option<f32>,
    left: f32,
    flash: Option<(usize, f32)>,
    body_w: f32,
    body_h: f32,
    backdrop: Option<Backdrop>,
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
        let left = if entries.iter().any(|e| e.checked) { LEFT_CHECK } else { LEFT };
        let gap = if short_w > 0.0 { SHORTCUT_GAP * s } else { 0.0 };
        let body_w = (left * s + text_w + gap + short_w + RIGHT * s).max(170.0 * s).ceil();
        Self { entries, scale, hover: None, closing: None, left, flash: None, body_w, body_h: h.ceil(), backdrop: None, owner, events: Vec::new() }
    }

    /// What is behind the window, window-sized; the menu is glass over it.
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
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
        if self.closing.is_none() {
            self.closing = Some(0.0);
        }
    }

    pub fn closing(&self) -> bool {
        self.closing.is_some()
    }

    /// The fade-out finished; the host can hide the window.
    pub fn finished(&self) -> bool {
        self.closing.is_some_and(|t| t >= FADE_OUT)
    }

    pub fn animating(&self) -> bool {
        self.closing.is_some_and(|t| t < FADE_OUT) || self.flash.is_some()
    }

    fn alpha(&self) -> f32 {
        self.closing.map_or(1.0, |t| 1.0 - (t / FADE_OUT).clamp(0.0, 1.0))
    }

    pub fn update(&mut self, dt: f32) {
        if let Some(t) = &mut self.closing {
            *t += dt;
        }
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
        if self.flash.is_none() && self.closing.is_none() {
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
        if self.closing.is_some() || self.flash.is_some() {
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
        if self.closing.is_some() || self.flash.is_some() {
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
        let dark = ctx.theme.dark;
        let a = self.alpha();
        if a <= 0.0 {
            return;
        }
        let (ox, oy) = self.body_offset();
        let (bx, by, bw, bh) = (ox as f32, oy as f32, self.body_w, self.body_h);
        let shape = Shape { x: bx, y: by, w: bw, h: bh, r: RADIUS * s };
        sheet(pm, ctx, self.backdrop.as_ref(), shape, s);

        let fs = FONT * s;
        let left = self.left * s;
        let mut cy = by + VPAD * s;
        for (i, e) in self.entries.iter().enumerate() {
            if e.separator {
                let t = s.max(1.0).round();
                let ly = (cy + (SEP_H * s - t) / 2.0).round();
                gfx::fill_rect(pm, bx + SEP_INSET * s, ly, bw - 2.0 * SEP_INSET * s, t, separator(dark));
                cy += SEP_H * s;
                continue;
            }
            let h = ITEM_H * s;
            let flash_off = matches!(self.flash, Some((fi, t)) if fi == i && t < FLASH / 2.0);
            let lit = self.hover == Some(i) && e.enabled && !flash_off;
            if lit && let Some(p) = gfx::squircle(bx + INSET * s, cy.round(), bw - 2.0 * INSET * s, h.round(), SEL_RADIUS * s) {
                gfx::fill(pm, &p, selection(dark));
            }
            let color = if lit {
                rgba(255, 255, 255, 1.0)
            } else if e.enabled {
                text_color(dark)
            } else {
                disabled(dark)
            };
            let base = (cy + h / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
            if e.checked {
                let g = 11.0 * s;
                glyphs::checkmark(pm, (bx + 10.0 * s).round(), cy + (h - g) / 2.0, g, color);
            }
            let max_label = bw - left - RIGHT * s;
            let label = ctx.text.ellipsize(&e.label, fs, Weight::Regular, max_label);
            ctx.text.draw(pm, &label, (bx + left).round(), base, fs, Weight::Regular, color, None);
            if !e.shortcut.is_empty() {
                let sw = ctx.text.measure(&e.shortcut, fs, Weight::Regular);
                let sc = if lit { rgba(255, 255, 255, 0.85) } else { secondary(dark) };
                ctx.text.draw(pm, &e.shortcut, (bx + bw - RIGHT * s - sw).round(), base, fs, Weight::Regular, sc, None);
            }
            cy += h;
        }
        // The glass cannot be drawn translucent, so the finished menu fades as a whole.
        crate::panel::fade_all(pm, a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_at_once_and_fades_out_briefly() {
        let ctx = Ctx::new(crate::text::Text::bundled(), crate::theme::Theme { dark: true }, &crate::strings::Strings::EN);
        let mut m = PopupMenu::new(vec![MenuEntry::item("One", 1), MenuEntry::sep(), MenuEntry::item("Two", 2)], 0, 1.0, &ctx);
        assert!(!m.animating());
        assert_eq!(m.alpha(), 1.0);
        assert_eq!(m.body_size().1, (2.0 * VPAD + 2.0 * ITEM_H + SEP_H).ceil());
        m.close();
        m.update(0.1);
        assert!(m.animating() && !m.finished());
        m.update(0.11);
        assert!(m.finished());
    }
}
