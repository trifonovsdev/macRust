//! Shared pieces of the drop-down panels (Control Center, Notification Center,
//! banners): reveal animation, the panel body, tiles, toggles and sliders.
//!
//! Panels float over app windows, which a layered window cannot blur, so the
//! body is an opaque material with the Liquid Glass rim and generous radii.

use tiny_skia::{Color, Pixmap};

use crate::anim::Spring;
use crate::gfx::{self, rgba};
use crate::theme::Theme;
use crate::Ctx;

/// Room for the shadow around a panel body, points.
pub const MARGIN: f32 = 28.0;

pub struct Reveal {
    pub value: Spring,
    closing: bool,
}

impl Default for Reveal {
    fn default() -> Self {
        Self::new()
    }
}

impl Reveal {
    pub fn new() -> Self {
        let mut value = Spring::new(0.0, 0.3, 0.86).epsilon(0.002);
        value.to(1.0);
        Self { value, closing: false }
    }
    pub fn close(&mut self) {
        if !self.closing {
            self.closing = true;
            self.value = Spring::new(self.value.value, 0.2, 1.0).epsilon(0.002);
            self.value.to(0.0);
        }
    }
    pub fn closing(&self) -> bool {
        self.closing
    }
    pub fn finished(&self) -> bool {
        self.closing && self.value.settled()
    }
    pub fn animating(&self) -> bool {
        !self.value.settled()
    }
    pub fn update(&mut self, dt: f32) {
        self.value.step(dt);
    }
    pub fn alpha(&self) -> f32 {
        self.value.value.clamp(0.0, 1.0)
    }
}

/// Multiplies a whole premultiplied pixmap by `a` (the reveal fade).
pub fn fade_all(pm: &mut Pixmap, a: f32) {
    if a >= 0.999 {
        return;
    }
    let k = (a.clamp(0.0, 1.0) * 256.0) as u32;
    for v in pm.data_mut() {
        *v = ((*v as u32 * k) >> 8) as u8;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn body(pm: &mut Pixmap, ctx: &Ctx, x: f32, y: f32, w: f32, h: f32, r: f32, s: f32) {
    let th = ctx.theme;
    ctx.shadow(r, 16.0 * s).draw(pm, x, y + 8.0 * s, w, h, if th.dark { rgba(0, 0, 0, 0.5) } else { rgba(0, 0, 0, 0.2) });
    if let Some(p) = gfx::squircle(x, y, w, h, r) {
        gfx::fill(pm, &p, th.panel());
        gfx::stroke(pm, &p, th.panel_border(), 1.0);
    }
    if let Some(p) = gfx::squircle(x + 1.0, y + 1.0, w - 2.0, h - 2.0, r - 1.0) {
        gfx::stroke(pm, &p, th.panel_rim(), 1.0);
    }
}

pub fn tile_color(th: Theme) -> Color {
    if th.dark { rgba(255, 255, 255, 0.08) } else { rgba(255, 255, 255, 0.78) }
}

#[allow(clippy::too_many_arguments)]
pub fn tile(pm: &mut Pixmap, th: Theme, x: f32, y: f32, w: f32, h: f32, r: f32, lit: bool) {
    if let Some(p) = gfx::squircle(x, y, w, h, r) {
        gfx::fill(pm, &p, tile_color(th));
        if lit {
            gfx::fill(pm, &p, if th.dark { rgba(255, 255, 255, 0.06) } else { rgba(0, 0, 0, 0.04) });
        }
        gfx::stroke(pm, &p, if th.dark { rgba(255, 255, 255, 0.07) } else { rgba(0, 0, 0, 0.05) }, 1.0);
    }
}

/// A round toggle button; returns the glyph color to draw inside it.
pub fn toggle(pm: &mut Pixmap, th: Theme, cx: f32, cy: f32, r: f32, on: bool, pressed: bool) -> Color {
    let bg = if on {
        th.accent()
    } else if th.dark {
        rgba(255, 255, 255, 0.16)
    } else {
        rgba(0, 0, 0, 0.09)
    };
    if let Some(p) = gfx::circle(cx, cy, r) {
        gfx::fill(pm, &p, bg);
        if pressed {
            gfx::fill(pm, &p, rgba(0, 0, 0, 0.15));
        }
    }
    if on { rgba(255, 255, 255, 1.0) } else { th.label() }
}

/// macOS 26 slider: a thick capsule whose filled part ends in a round knob.
pub fn slider(pm: &mut Pixmap, ctx: &Ctx, x: f32, y: f32, w: f32, h: f32, v: f32) {
    let th = ctx.theme;
    let track = gfx::squircle(x, y, w, h, h / 2.0);
    if let Some(p) = &track {
        gfx::fill(pm, p, if th.dark { rgba(255, 255, 255, 0.13) } else { rgba(0, 0, 0, 0.11) });
    }
    let kx = x + h / 2.0 + v.clamp(0.0, 1.0) * (w - h);
    // Ends under the knob centre, so no sliver of it shows past the knob.
    if let Some(p) = gfx::squircle(x, y, (kx - x).max(h), h, h / 2.0) {
        gfx::fill(pm, &p, rgba(255, 255, 255, if th.dark { 0.92 } else { 1.0 }));
    }
    if !th.dark
        && let Some(p) = &track
    {
        gfx::stroke(pm, p, rgba(0, 0, 0, 0.07), 1.0);
    }
    if let Some(p) = gfx::circle(kx, y + h / 2.0, h / 2.0) {
        gfx::fill(pm, &p, rgba(255, 255, 255, 1.0));
        gfx::stroke(pm, &p, rgba(0, 0, 0, if th.dark { 0.25 } else { 0.14 }), 1.0);
    }
}

/// Slider value for a pointer x.
pub fn slider_value(x: f32, w: f32, h: f32, px: f32) -> f32 {
    ((px - x - h / 2.0) / (w - h).max(1.0)).clamp(0.0, 1.0)
}

pub fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}
