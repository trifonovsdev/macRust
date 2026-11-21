//! Vector glyphs for the menu bar and fallback icons, drawn in a `size` box
//! whose top-left corner is `x, y`.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode, Transform};

use crate::gfx::{self, rgba};

fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, start: bool) {
    let segs = ((a1 - a0).abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = (a1 - a0) / segs as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let (mut x0, mut y0) = (cx + r * a0.cos(), cy + r * a0.sin());
    if start {
        pb.move_to(x0, y0);
    } else {
        pb.line_to(x0, y0);
    }
    for i in 0..segs {
        let a = a0 + step * (i + 1) as f32;
        let (x1, y1) = (cx + r * a.cos(), cy + r * a.sin());
        let ap = a - step;
        pb.cubic_to(
            x0 - k * r * ap.sin(),
            y0 + k * r * ap.cos(),
            x1 + k * r * a.sin(),
            y1 - k * r * a.cos(),
            x1,
            y1,
        );
        x0 = x1;
        y0 = y1;
    }
}

fn line_w(size: f32) -> f32 {
    (size * 0.09).max(1.3)
}

pub fn magnifier(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    if let Some(p) = gfx::circle(x + s * 0.42, y + s * 0.42, s * 0.29) {
        gfx::stroke(pm, &p, c, line_w(s));
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + s * 0.64, y + s * 0.64);
    pb.line_to(x + s * 0.88, y + s * 0.88);
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, line_w(s) * 1.25);
    }
}

pub fn wifi(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color, connected: bool) {
    let (cx, cy) = (x + s * 0.5, y + s * 0.8);
    let dim = gfx::fade(c, 0.3);
    for (i, r) in [0.25f32, 0.45, 0.65].iter().enumerate() {
        let mut pb = PathBuilder::new();
        arc(&mut pb, cx, cy, s * r, -PI * 0.75, -PI * 0.25, true);
        if let Some(p) = pb.finish() {
            let col = if connected || i == 0 { c } else { dim };
            gfx::stroke_round(pm, &p, col, line_w(s) * 1.15);
        }
    }
    if let Some(p) = gfx::circle(cx, cy - s * 0.02, s * 0.075) {
        gfx::fill(pm, &p, c);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn battery(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color, pct: u8, charging: bool) {
    let lw = (h * 0.09).max(1.0);
    let bw = w * 0.88;
    if let Some(p) = gfx::squircle(x + lw / 2.0, y + lw / 2.0, bw - lw, h - lw, h * 0.28) {
        gfx::stroke(pm, &p, gfx::fade(c, 0.45), lw);
    }
    if let Some(p) = gfx::squircle(x + bw + w * 0.02, y + h * 0.33, w * 0.06, h * 0.34, w * 0.03) {
        gfx::fill(pm, &p, gfx::fade(c, 0.45));
    }
    let inset = lw + h * 0.08;
    let full = bw - inset * 2.0;
    let lvl = full * (pct.min(100) as f32 / 100.0).max(0.06);
    let fill_c = if pct <= 15 && !charging { rgba(255, 69, 58, 1.0) } else { c };
    if let Some(p) = gfx::squircle(x + inset, y + inset, lvl, h - inset * 2.0, h * 0.16) {
        gfx::fill(pm, &p, fill_c);
    }
    if charging {
        let (bx, by, bh) = (x + bw * 0.5, y + h * 0.5, h * 0.78);
        let mut pb = PathBuilder::new();
        pb.move_to(bx + bh * 0.08, by - bh * 0.5);
        pb.line_to(bx - bh * 0.22, by + bh * 0.06);
        pb.line_to(bx - bh * 0.01, by + bh * 0.06);
        pb.line_to(bx - bh * 0.08, by + bh * 0.5);
        pb.line_to(bx + bh * 0.22, by - bh * 0.06);
        pb.line_to(bx + bh * 0.01, by - bh * 0.06);
        pb.close();
        if let Some(p) = pb.finish() {
            gfx::stroke(pm, &p, rgba(0, 0, 0, 0.45), lw);
            gfx::fill(pm, &p, rgba(255, 255, 255, 1.0));
        }
    }
}

/// Two stacked switches: the control-center glyph.
pub fn switches(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let lw = line_w(s) * 0.9;
    for (row, knob_left) in [(0.16f32, false), (0.56f32, true)] {
        let (py, ph) = (y + s * row, s * 0.3);
        if let Some(p) = gfx::squircle(x + s * 0.06, py, s * 0.88, ph, ph / 2.0) {
            gfx::stroke(pm, &p, c, lw);
        }
        let kx = if knob_left { x + s * 0.06 + ph / 2.0 } else { x + s * 0.94 - ph / 2.0 };
        if let Some(p) = gfx::circle(kx, py + ph / 2.0, ph * 0.36) {
            gfx::fill(pm, &p, c);
        }
    }
}

/// Our menu-bar mark: a gear (Rust's cog) with a round hole.
pub fn logo(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let (cx, cy) = (x + s * 0.5, y + s * 0.5);
    let teeth = 8;
    let (ro, ri) = (s * 0.5, s * 0.38);
    let mut pb = PathBuilder::new();
    for i in 0..teeth {
        let a = i as f32 / teeth as f32 * TAU;
        let half = TAU / teeth as f32 / 2.0;
        let pts = [
            (a - half * 0.95, ri),
            (a - half * 0.42, ro),
            (a + half * 0.42, ro),
            (a + half * 0.95, ri),
        ];
        for (j, (ang, r)) in pts.iter().enumerate() {
            let (px, py) = (cx + r * ang.cos(), cy + r * ang.sin());
            if i == 0 && j == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
    }
    pb.close();
    pb.push_circle(cx, cy, s * 0.17);
    if let Some(p) = pb.finish() {
        let mut paint = gfx::paint(c);
        paint.anti_alias = true;
        pm.fill_path(&p, &paint, FillRule::EvenOdd, Transform::identity(), None);
    }
}

pub fn chevron_right(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + s * 0.36, y + s * 0.2);
    pb.line_to(x + s * 0.66, y + s * 0.5);
    pb.line_to(x + s * 0.36, y + s * 0.8);
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, line_w(s) * 1.2);
    }
}

pub fn checkmark(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + s * 0.18, y + s * 0.54);
    pb.line_to(x + s * 0.42, y + s * 0.76);
    pb.line_to(x + s * 0.82, y + s * 0.26);
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, line_w(s) * 1.3);
    }
}

fn vgrad(y0: f32, y1: f32, top: Color, bottom: Color) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    if let Some(sh) = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        p.shader = sh;
    } else {
        p.set_color(top);
    }
    p
}

fn fill_grad(pm: &mut Pixmap, path: &tiny_skia::Path, y0: f32, y1: f32, top: Color, bottom: Color) {
    pm.fill_path(path, &vgrad(y0, y1, top, bottom), FillRule::Winding, Transform::identity(), None);
}

/// Placeholder app tile: a soft gray squircle.
pub fn app_tile(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let i = s * 0.08;
    if let Some(p) = gfx::squircle(x + i, y + i, s - 2.0 * i, s - 2.0 * i, s * 0.225) {
        fill_grad(pm, &p, y, y + s, rgba(250, 250, 252, 1.0), rgba(214, 214, 220, 1.0));
        gfx::stroke(pm, &p, rgba(0, 0, 0, 0.08), 1.0);
    }
}

/// A blue folder, used until the real shell icon arrives.
pub fn folder(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let (l, w) = (x + s * 0.06, s * 0.88);
    let mut pb = PathBuilder::new();
    let (top, tab_w) = (y + s * 0.2, w * 0.38);
    pb.move_to(l, top + s * 0.06);
    pb.line_to(l, top);
    pb.line_to(l + tab_w, top);
    pb.line_to(l + tab_w + s * 0.07, top + s * 0.07);
    pb.line_to(l + w, top + s * 0.07);
    pb.line_to(l + w, top + s * 0.2);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, rgba(74, 155, 230, 1.0));
    }
    if let Some(p) = gfx::squircle(l, top + s * 0.12, w, s * 0.56, s * 0.06) {
        fill_grad(pm, &p, top, top + s * 0.68, rgba(118, 194, 255, 1.0), rgba(76, 160, 240, 1.0));
    }
}

/// A white page with a folded corner.
pub fn document(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let (l, t, w, h) = (x + s * 0.2, y + s * 0.08, s * 0.6, s * 0.84);
    let fold = s * 0.18;
    let mut pb = PathBuilder::new();
    pb.move_to(l, t);
    pb.line_to(l + w - fold, t);
    pb.line_to(l + w, t + fold);
    pb.line_to(l + w, t + h);
    pb.line_to(l, t + h);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, rgba(255, 255, 255, 1.0));
        gfx::stroke(pm, &p, rgba(0, 0, 0, 0.18), 1.0);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(l + w - fold, t);
    pb.line_to(l + w - fold, t + fold);
    pb.line_to(l + w, t + fold);
    if let Some(p) = pb.finish() {
        gfx::stroke(pm, &p, rgba(0, 0, 0, 0.18), 1.0);
    }
    for i in 0..4 {
        let ly = t + h * (0.38 + i as f32 * 0.13);
        gfx::fill_rect(pm, l + w * 0.18, ly, w * if i == 3 { 0.4 } else { 0.64 }, (s * 0.025).max(1.0), rgba(0, 0, 0, 0.14));
    }
}

/// Orange tile with an equals sign: calculator results.
pub fn calculator(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let i = s * 0.08;
    if let Some(p) = gfx::squircle(x + i, y + i, s - 2.0 * i, s - 2.0 * i, s * 0.225) {
        fill_grad(pm, &p, y, y + s, rgba(255, 176, 64, 1.0), rgba(255, 128, 0, 1.0));
    }
    for dy in [-0.09f32, 0.09] {
        if let Some(p) = gfx::squircle(x + s * 0.3, y + s * (0.5 + dy) - s * 0.04, s * 0.4, s * 0.08, s * 0.04) {
            gfx::fill(pm, &p, rgba(255, 255, 255, 1.0));
        }
    }
}

/// Blue globe: web search row.
pub fn globe(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let (cx, cy, r) = (x + s * 0.5, y + s * 0.5, s * 0.4);
    if let Some(p) = gfx::circle(cx, cy, r) {
        fill_grad(pm, &p, cy - r, cy + r, rgba(90, 200, 250, 1.0), rgba(0, 122, 255, 1.0));
    }
    let c = rgba(255, 255, 255, 0.85);
    let lw = (s * 0.035).max(1.0);
    let mut pb = PathBuilder::new();
    pb.push_oval(tiny_skia::Rect::from_xywh(cx - r * 0.45, cy - r, r * 0.9, r * 2.0).unwrap_or(tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit rect")));
    pb.move_to(cx, cy - r);
    pb.line_to(cx, cy + r);
    pb.move_to(cx - r, cy);
    pb.line_to(cx + r, cy);
    if let Some(p) = pb.finish() {
        gfx::stroke(pm, &p, c, lw);
    }
}

/// Gray tile with a cog: settings rows.
pub fn settings_tile(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let i = s * 0.08;
    if let Some(p) = gfx::squircle(x + i, y + i, s - 2.0 * i, s - 2.0 * i, s * 0.225) {
        fill_grad(pm, &p, y, y + s, rgba(160, 160, 170, 1.0), rgba(110, 110, 120, 1.0));
    }
    logo(pm, x + s * 0.22, y + s * 0.22, s * 0.56, rgba(240, 240, 245, 1.0));
}

/// Fallback recycle bin, drawn as a translucent bin with ribs.
pub fn trash(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let (l, r, t, b) = (x + s * 0.2, x + s * 0.8, y + s * 0.2, y + s * 0.92);
    let mut pb = PathBuilder::new();
    pb.move_to(l, t);
    pb.line_to(r, t);
    pb.line_to(r - s * 0.05, b);
    pb.line_to(l + s * 0.05, b);
    pb.close();
    if let Some(p) = pb.finish() {
        fill_grad(pm, &p, t, b, rgba(250, 250, 252, 0.95), rgba(205, 205, 212, 0.95));
        gfx::stroke(pm, &p, rgba(0, 0, 0, 0.18), 1.0);
    }
    for i in 1..5 {
        let fx = l + (r - l) * i as f32 / 5.0;
        let mut pb = PathBuilder::new();
        pb.move_to(fx, t + s * 0.08);
        pb.line_to(fx + (fx - (l + r) / 2.0) * -0.08, b - s * 0.06);
        if let Some(p) = pb.finish() {
            gfx::stroke(pm, &p, rgba(0, 0, 0, 0.12), (s * 0.02).max(1.0));
        }
    }
    if let Some(p) = gfx::squircle(l - s * 0.04, t - s * 0.06, (r - l) + s * 0.08, s * 0.08, s * 0.04) {
        fill_grad(pm, &p, t - s * 0.06, t + s * 0.02, rgba(236, 236, 240, 1.0), rgba(190, 190, 198, 1.0));
    }
}

fn poly(pts: &[(f32, f32)], close: bool) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (&(x0, y0), rest) = pts.split_first()?;
    pb.move_to(x0, y0);
    for &(x, y) in rest {
        pb.line_to(x, y);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

pub fn bluetooth(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    let pts = [p(0.3, 0.3), p(0.7, 0.68), p(0.5, 0.86), p(0.5, 0.14), p(0.7, 0.32), p(0.3, 0.7)];
    if let Some(path) = poly(&pts, false) {
        gfx::stroke_round(pm, &path, c, line_w(s) * 1.1);
    }
}

/// Crescent moon (dark mode).
pub fn moon(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    let (cx, cy, r) = (x + s * 0.5, y + s * 0.5, s * 0.38);
    arc(&mut pb, cx, cy, r, -PI * 0.3, PI * 1.5, true);
    arc(&mut pb, cx + s * 0.16, cy - s * 0.14, r * 0.8, PI * 1.3, PI * 0.1, false);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, c);
    }
}

pub fn sun(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let (cx, cy) = (x + s * 0.5, y + s * 0.5);
    if let Some(p) = gfx::circle(cx, cy, s * 0.2) {
        gfx::fill(pm, &p, c);
    }
    for i in 0..8 {
        let a = i as f32 * TAU / 8.0;
        if let Some(p) = poly(&[(cx + a.cos() * s * 0.32, cy + a.sin() * s * 0.32), (cx + a.cos() * s * 0.44, cy + a.sin() * s * 0.44)], false) {
            gfx::stroke_round(pm, &p, c, line_w(s));
        }
    }
}

pub fn speaker(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color, waves: u8) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    if let Some(path) = poly(&[p(0.1, 0.38), p(0.26, 0.38), p(0.48, 0.18), p(0.48, 0.82), p(0.26, 0.62), p(0.1, 0.62)], true) {
        gfx::fill(pm, &path, c);
    }
    for i in 0..waves.min(2) {
        let mut pb = PathBuilder::new();
        arc(&mut pb, x + s * 0.48, y + s * 0.5, s * (0.18 + 0.16 * i as f32), -PI * 0.28, PI * 0.28, true);
        if let Some(path) = pb.finish() {
            gfx::stroke_round(pm, &path, c, line_w(s));
        }
    }
}

pub fn play(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    if let Some(path) = poly(&[p(0.24, 0.14), p(0.86, 0.5), p(0.24, 0.86)], true) {
        gfx::fill(pm, &path, c);
    }
}

pub fn pause(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    for bx in [0.22f32, 0.58] {
        if let Some(path) = gfx::squircle(x + s * bx, y + s * 0.14, s * 0.2, s * 0.72, s * 0.05) {
            gfx::fill(pm, &path, c);
        }
    }
}

/// Two triangles: skip forward, or back when `back`.
pub fn skip(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color, back: bool) {
    let f = |a: f32| if back { x + s * (1.0 - a) } else { x + s * a };
    for o in [0.0f32, 0.4] {
        if let Some(path) = poly(&[(f(0.1 + o), y + s * 0.22), (f(0.5 + o), y + s * 0.5), (f(0.1 + o), y + s * 0.78)], true) {
            gfx::fill(pm, &path, c);
        }
    }
}

pub fn music_note(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    if let Some(path) = poly(&[p(0.4, 0.72), p(0.4, 0.18), p(0.82, 0.1), p(0.82, 0.62)], false) {
        gfx::stroke(pm, &path, c, line_w(s) * 1.2);
    }
    if let Some(path) = poly(&[p(0.4, 0.18), p(0.82, 0.1), p(0.82, 0.26), p(0.4, 0.34)], true) {
        gfx::fill(pm, &path, c);
    }
    for (cx, cy) in [(0.3, 0.74), (0.72, 0.64)] {
        if let Some(path) = gfx::circle(x + s * cx, y + s * cy, s * 0.12) {
            gfx::fill(pm, &path, c);
        }
    }
}

/// A dashed selection frame: the screenshot tool.
pub fn screenshot(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let lw = line_w(s);
    let (a, b) = (0.14f32, 0.86f32);
    let l = 0.22f32;
    for (cx, cy, dx, dy) in [(a, a, 1.0, 1.0), (b, a, -1.0, 1.0), (a, b, 1.0, -1.0), (b, b, -1.0, -1.0)] {
        let pts = [(x + s * cx, y + s * (cy + dy * l)), (x + s * cx, y + s * cy), (x + s * (cx + dx * l), y + s * cy)];
        if let Some(path) = poly(&pts, false) {
            gfx::stroke_round(pm, &path, c, lw);
        }
    }
    if let Some(path) = gfx::circle(x + s * 0.5, y + s * 0.5, s * 0.12) {
        gfx::fill(pm, &path, c);
    }
}

pub fn close_x(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    for (a, b) in [((0.3, 0.3), (0.7, 0.7)), ((0.7, 0.3), (0.3, 0.7))] {
        if let Some(path) = poly(&[(x + s * a.0, y + s * a.1), (x + s * b.0, y + s * b.1)], false) {
            gfx::stroke_round(pm, &path, c, line_w(s) * 1.2);
        }
    }
}
