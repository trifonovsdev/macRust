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

/// SF `wifi` as the menu bar draws it: a wedge and two bands, 2.1 pt
/// thick, ink exactly `w` wide and `0.74 w` tall from `x, y` (Tahoe: 16 x 11.9 pt).
pub fn wifi(pm: &mut Pixmap, x: f32, y: f32, w: f32, c: Color, connected: bool) {
    // Radii in units of the outer radius, measured top-down on NL: band 2.1 pt,
    // gap 1.7, band 2.2, gap 1.3, wedge 4.0 of 11.9 pt.
    let half = 42.5f32.to_radians();
    let r = w / 2.0 / half.sin();
    let (cx, cy) = (x + w / 2.0, y + r);
    let round = r * 0.045;
    let dim = gfx::fade(c, 0.3);
    for (i, (r0, r1)) in [(0.0f32, 0.36f32), (0.47, 0.655), (0.80, 1.0)].iter().enumerate() {
        let (a0, a1) = (-FRAC_PI_2 - half, -FRAC_PI_2 + half);
        // Inset by the corner rounding, which the round-joined stroke adds back.
        let (ri, ro) = ((r * r0 + round).min(r * r1 - round), r * r1 - round);
        let ang = round / ro;
        let mut pb = PathBuilder::new();
        arc(&mut pb, cx, cy, ro, a0 + ang, a1 - ang, true);
        if ri > round {
            let ang = round / ri;
            arc(&mut pb, cx, cy, ri, a1 - ang, a0 + ang, false);
        } else {
            pb.line_to(cx, cy - round * 1.6);
        }
        pb.close();
        if let Some(p) = pb.finish() {
            let col = if connected || i == 0 { c } else { dim };
            gfx::fill(pm, &p, col);
            gfx::stroke_round(pm, &p, col, round * 2.0);
        }
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

/// SF `switch.2`, the Control Center glyph: an outlined switch with its knob
/// left over a filled one with a hollow knob right. Ink is `w` x `0.96 w`
/// (Tahoe: 12.5 x 12 pt, pills 5.8 pt tall, 0.9 pt apart, 1.25 pt line).
pub fn switches(pm: &mut Pixmap, x: f32, y: f32, w: f32, c: Color) {
    let h = w * 0.465;
    let lw = w * 0.1;
    let y2 = y + w * 0.96 - h;
    if let Some(p) = capsule(x + lw / 2.0, y + lw / 2.0, w - lw, h - lw) {
        gfx::stroke(pm, &p, c, lw);
    }
    if let Some(p) = gfx::circle(x + h / 2.0, y + h / 2.0, h * 0.3) {
        gfx::fill(pm, &p, c);
    }
    let mut pb = PathBuilder::new();
    if let Some(p) = capsule(x, y2, w, h) {
        pb.push_path(&p);
    }
    pb.push_circle(x + w - h / 2.0, y2 + h / 2.0, h / 2.0 - lw * 0.9);
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &gfx::paint(c), FillRule::EvenOdd, Transform::identity(), None);
    }
}

/// A stadium with circular ends (a squircle cannot round a capsule fully).
fn capsule(x: f32, y: f32, w: f32, h: f32) -> Option<tiny_skia::Path> {
    let r = h / 2.0;
    let mut pb = PathBuilder::new();
    arc(&mut pb, x + r, y + r, r, FRAC_PI_2, PI + FRAC_PI_2, true);
    arc(&mut pb, x + w - r, y + r, r, -FRAC_PI_2, FRAC_PI_2, false);
    pb.close();
    pb.finish()
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

fn fill_stops(pm: &mut Pixmap, path: &tiny_skia::Path, y0: f32, y1: f32, stops: &[(f32, Color)]) {
    let stops = stops.iter().map(|&(t, c)| GradientStop::new(t, c)).collect();
    if let Some(sh) = LinearGradient::new(Point::from_xy(0.0, y0), Point::from_xy(0.0, y1), stops, SpreadMode::Pad, Transform::identity()) {
        gfx::fill_shader(pm, path, sh);
    }
}

/// Maps the dock icon box (`x, y`, side `s`) to units of the visible tile,
/// which is 0.805 of the box (Apple's icon grid), origin at its top-left.
fn tile_units(x: f32, y: f32, s: f32) -> impl Fn(f32, f32) -> (f32, f32) {
    let (v, m) = (s * 0.805, s * 0.0975);
    move |u, w| (x + m + u * v, y + m + w * v)
}

/// The dock's Downloads stack in the macOS 26 manner: a plain light-blue
/// folder, wider than a tile, with a debossed down arrow. Sizes from Tahoe
/// screenshots in tile units.
pub fn stack_folder(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let p = tile_units(x, y, s);
    let v = s * 0.805;
    // 1.24 tiles wide: as wide as the icon box allows (Tahoe's is 1.2-1.3).
    let (l, r) = (-0.12f32, 1.12f32);
    let (x0, top) = p(l, -0.11);
    let (_, back) = p(l, 0.0);
    let (x1, bottom) = p(r, 0.95);
    let (_, front) = p(l, 0.08);
    let tab_w = 0.46 * (r - l) * v;
    let back_col = [(0.0, rgba(87, 187, 235, 1.0)), (1.0, rgba(70, 176, 227, 1.0))];
    // Tab with a sloped right side, merged into the back panel.
    let mut pb = PathBuilder::new();
    let rt = 0.07 * v;
    pb.move_to(x0, back + rt);
    pb.quad_to(x0, top, x0 + rt, top);
    pb.line_to(x0 + tab_w - 0.1 * v, top);
    pb.cubic_to(x0 + tab_w - 0.05 * v, top, x0 + tab_w - 0.05 * v, back, x0 + tab_w, back);
    pb.line_to(x0 + tab_w, back + rt);
    pb.close();
    if let Some(path) = pb.finish() {
        fill_stops(pm, &path, top, back + rt, &back_col);
    }
    if let Some(path) = gfx::squircle(x0, back, x1 - x0, bottom - back, 0.09 * v) {
        fill_stops(pm, &path, back, bottom, &back_col);
    }
    // The seam between the panels: the front casts a thin shadow upwards.
    if let Some(path) = gfx::squircle(x0, front - 0.02 * v, x1 - x0, bottom - front, 0.1 * v) {
        gfx::fill(pm, &path, rgba(52, 150, 205, 0.55));
    }
    if let Some(path) = gfx::squircle(x0, front, x1 - x0, bottom - front, 0.1 * v) {
        fill_stops(pm, &path, front, bottom, &[(0.0, rgba(114, 208, 251, 1.0)), (1.0, rgba(72, 177, 226, 1.0))]);
    }
    let ink = rgba(38, 138, 204, 0.38);
    let (sx0, sy0) = p(0.5 - 0.035, 0.34);
    let (sx1, sy1) = p(0.5 + 0.035, 0.6);
    if let Some(path) = gfx::squircle(sx0, sy0, sx1 - sx0, sy1 - sy0, 0.02 * v) {
        gfx::fill(pm, &path, ink);
    }
    if let Some(path) = poly(&[p(0.33, 0.52), p(0.67, 0.52), p(0.5, 0.72)], true) {
        gfx::fill(pm, &path, ink);
    }
}

/// The dock's Trash in the macOS 26 manner: a translucent glass bin with a
/// lip, darker in dark mode. Sizes from Tahoe screenshots in tile units.
pub fn stack_trash(pm: &mut Pixmap, x: f32, y: f32, s: f32, dark: bool) {
    let p = tile_units(x, y, s);
    let v = s * 0.805;
    let (_, top) = p(0.0, 0.04);
    let (_, bottom) = p(0.0, 0.99);
    let rb = 0.11;
    let mut pb = PathBuilder::new();
    let (ax, ay) = p(0.0, 0.04);
    pb.move_to(ax, ay);
    let (bx, by) = p(1.0, 0.04);
    pb.line_to(bx, by);
    // Sides taper to 0.77 of the width; the bottom corners are rounded.
    let (cx, cy) = p(0.885 + 0.012, 0.99 - rb);
    pb.line_to(cx, cy);
    let (qx, qy) = p(0.885, 0.99);
    let (dx, dy) = p(0.885 - rb, 0.99);
    pb.quad_to(qx, qy, dx, dy);
    let (ex, ey) = p(0.115 + rb, 0.99);
    pb.line_to(ex, ey);
    let (qx, qy) = p(0.115, 0.99);
    let (fx, fy) = p(0.115 - 0.012, 0.99 - rb);
    pb.quad_to(qx, qy, fx, fy);
    pb.close();
    let body = pb.finish();
    let (fill, rim_col, lip, lip_edge, lip_rim): ([(f32, Color); 3], Color, Color, Color, Color) = if dark {
        (
            [(0.0, rgba(30, 30, 38, 0.68)), (0.85, rgba(42, 40, 46, 0.8)), (1.0, rgba(92, 94, 104, 0.85))],
            rgba(255, 255, 255, 0.2),
            rgba(70, 68, 76, 0.72),
            rgba(255, 255, 255, 0.4),
            rgba(255, 255, 255, 0.32),
        )
    } else {
        (
            [(0.0, rgba(246, 248, 252, 0.5)), (0.85, rgba(236, 239, 244, 0.62)), (1.0, rgba(255, 255, 255, 0.85))],
            rgba(0, 0, 0, 0.12),
            rgba(250, 250, 252, 0.72),
            rgba(255, 255, 255, 0.95),
            rgba(0, 0, 0, 0.12),
        )
    };
    if let Some(path) = &body {
        fill_stops(pm, path, top, bottom, &fill);
        gfx::stroke(pm, path, rim_col, (0.015 * v).max(1.0));
    }
    let (lx, ly) = p(-0.02, -0.12);
    let (rx, ry) = p(1.02, 0.07);
    if let Some(path) = gfx::squircle(lx, ly, rx - lx, ry - ly, 0.09 * v) {
        gfx::fill(pm, &path, lip);
        gfx::stroke(pm, &path, lip_rim, (0.015 * v).max(1.0));
    }
    // The front edge of the lip catches the light.
    let (ex0, ey0) = p(0.03, 0.04);
    let (ex1, _) = p(0.97, 0.04);
    if let Some(path) = gfx::squircle(ex0, ey0 - 0.01 * v, ex1 - ex0, 0.04 * v, 0.02 * v) {
        gfx::fill(pm, &path, lip_edge);
    }
}

/// A label bubble: a capsule with a rounded pointer of `tip_w` x `tip_h`
/// centred under it at `cx`.
pub fn bubble(x: f32, y: f32, w: f32, h: f32, cx: f32, tip_w: f32, tip_h: f32) -> Option<tiny_skia::Path> {
    let r = h / 2.0;
    let b = y + h;
    let cx = cx.clamp(x + r + tip_w / 2.0, x + w - r - tip_w / 2.0);
    let mut pb = PathBuilder::new();
    arc(&mut pb, x + r, y + r, r, FRAC_PI_2, PI + FRAC_PI_2, true);
    arc(&mut pb, x + w - r, y + r, r, -FRAC_PI_2, FRAC_PI_2, false);
    let hw = tip_w / 2.0;
    pb.line_to(cx + hw, b);
    pb.cubic_to(cx + hw * 0.45, b, cx + hw * 0.3, b + tip_h, cx, b + tip_h);
    pb.cubic_to(cx - hw * 0.3, b + tip_h, cx - hw * 0.45, b, cx - hw, b);
    pb.close();
    pb.finish()
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

/// Crescent moon (dark mode): a disc with an offset disc cut out of it.
pub fn moon(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let n = s.ceil().max(1.0) as u32 + 2;
    let Some(mut tmp) = Pixmap::new(n, n) else { return };
    if let Some(p) = gfx::circle(1.0 + s * 0.5, 1.0 + s * 0.5, s * 0.4) {
        gfx::fill(&mut tmp, &p, c);
    }
    if let Some(p) = gfx::circle(1.0 + s * 0.7, 1.0 + s * 0.33, s * 0.32) {
        let paint = tiny_skia::Paint { blend_mode: tiny_skia::BlendMode::Clear, anti_alias: true, ..Default::default() };
        tmp.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    pm.draw_pixmap((x - 1.0).round() as i32, (y - 1.0).round() as i32, tmp.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
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

/// Outline calculator: body with a display and a 3x3 keypad.
pub fn calculator_glyph(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    if let Some(p) = gfx::squircle(x + s * 0.2, y + s * 0.08, s * 0.6, s * 0.84, s * 0.1) {
        gfx::stroke(pm, &p, c, line_w(s));
    }
    gfx::fill_rect(pm, x + s * 0.3, y + s * 0.18, s * 0.4, s * 0.14, c);
    for row in 0..3 {
        for col in 0..3 {
            if let Some(p) = gfx::circle(x + s * (0.34 + col as f32 * 0.16), y + s * (0.46 + row as f32 * 0.14), s * 0.045) {
                gfx::fill(pm, &p, c);
            }
        }
    }
}

/// Plain gear outline (settings).
pub fn gear(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let (cx, cy) = (x + s * 0.5, y + s * 0.5);
    let mut pb = PathBuilder::new();
    let teeth = 8;
    for i in 0..teeth * 2 {
        let a0 = i as f32 * PI / teeth as f32;
        let r = if i % 2 == 0 { s * 0.42 } else { s * 0.32 };
        let (a, b) = (a0 - PI / teeth as f32 * 0.42, a0 + PI / teeth as f32 * 0.42);
        if i == 0 {
            pb.move_to(cx + r * a.cos(), cy + r * a.sin());
        } else {
            pb.line_to(cx + r * a.cos(), cy + r * a.sin());
        }
        pb.line_to(cx + r * b.cos(), cy + r * b.sin());
    }
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::stroke(pm, &p, c, line_w(s));
    }
    if let Some(p) = gfx::circle(cx, cy, s * 0.12) {
        gfx::stroke(pm, &p, c, line_w(s));
    }
}

/// Padlock.
pub fn lock(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    arc(&mut pb, x + s * 0.5, y + s * 0.4, s * 0.2, PI, TAU, true);
    pb.line_to(x + s * 0.7, y + s * 0.46);
    if let Some(p) = pb.finish() {
        gfx::stroke(pm, &p, c, line_w(s) * 1.1);
    }
    let mut pb2 = PathBuilder::new();
    pb2.move_to(x + s * 0.3, y + s * 0.46);
    pb2.line_to(x + s * 0.3, y + s * 0.4);
    if let Some(p) = pb2.finish() {
        gfx::stroke(pm, &p, c, line_w(s) * 1.1);
    }
    if let Some(p) = gfx::squircle(x + s * 0.22, y + s * 0.44, s * 0.56, s * 0.44, s * 0.08) {
        gfx::fill(pm, &p, c);
    }
}

// ---------------------------------------------------------------------------
// Menu-bar extras standing in for Windows notification-area icons. Strokes are
// `EXTRA_LINE` of the box, Tahoe's 1.4 pt in a 16 pt box.

const EXTRA_LINE: f32 = 1.4 / 16.0;

/// Telegram's paper plane, filled, with the fold cut in as a V.
pub fn paper_plane(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    let pts = [p(0.04, 0.46), p(0.96, 0.08), p(0.80, 0.86), p(0.47, 0.66), p(0.50, 0.55), p(0.75, 0.27), p(0.36, 0.52), p(0.25, 0.56)];
    if let Some(path) = poly(&pts, true) {
        gfx::fill(pm, &path, c);
        // Round the sharp tips the way a symbol's outline is.
        gfx::stroke_round(pm, &path, c, s * 0.04);
    }
}

/// A shield outline (SF `shield`) whose shape is `x0..x1` wide, `y0..y1` tall.
fn shield_path(x0: f32, y0: f32, x1: f32, y1: f32) -> Option<tiny_skia::Path> {
    let (w, h) = (x1 - x0, y1 - y0);
    let p = |a: f32, b: f32| (x0 + w * a, y0 + h * b);
    let mut pb = PathBuilder::new();
    let m = p(0.5, 0.0);
    pb.move_to(m.0, m.1);
    let (c1, c2, e) = (p(0.66, 0.1), p(0.84, 0.13), p(1.0, 0.14));
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
    let e = p(1.0, 0.5);
    pb.line_to(e.0, e.1);
    let (c1, c2, e) = (p(1.0, 0.76), p(0.78, 0.92), p(0.5, 1.0));
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
    let (c1, c2, e) = (p(0.22, 0.92), p(0.0, 0.76), p(0.0, 0.5));
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
    let e = p(0.0, 0.14);
    pb.line_to(e.0, e.1);
    let (c1, c2, e) = (p(0.16, 0.13), p(0.34, 0.1), p(0.5, 0.0));
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
    pb.close();
    pb.finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShieldMark {
    Check,
    Exclamation,
    Cross,
}

/// SF `checkmark.shield` / `exclamationmark.shield` / `xmark.shield`.
pub fn shield(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color, mark: ShieldMark) {
    let lw = s * EXTRA_LINE;
    let (x0, x1, y0, y1) = (x + s * 0.13, x + s * 0.87, y + s * 0.04, y + s * 0.96);
    if let Some(p) = shield_path(x0 + lw / 2.0, y0 + lw / 2.0, x1 - lw / 2.0, y1 - lw / 2.0) {
        gfx::stroke(pm, &p, c, lw);
    }
    let (cx, cy) = (x + s * 0.5, y + s * 0.47);
    let k = s * 0.2;
    let path = match mark {
        ShieldMark::Check => poly(&[(cx - k * 0.95, cy + k * 0.05), (cx - k * 0.25, cy + k * 0.75), (cx + k * 0.95, cy - k * 0.7)], false),
        ShieldMark::Exclamation => {
            if let Some(d) = gfx::circle(cx, cy + k * 1.05, lw * 0.62) {
                gfx::fill(pm, &d, c);
            }
            poly(&[(cx, cy - k * 1.15), (cx, cy + k * 0.35)], false)
        }
        ShieldMark::Cross => {
            if let Some(p) = poly(&[(cx - k * 0.7, cy - k * 0.7), (cx + k * 0.7, cy + k * 0.7)], false) {
                gfx::stroke_round(pm, &p, c, lw);
            }
            poly(&[(cx + k * 0.7, cy - k * 0.7), (cx - k * 0.7, cy + k * 0.7)], false)
        }
    };
    if let Some(p) = path {
        gfx::stroke_round(pm, &p, c, lw);
    }
}

/// SF `eject.fill`: a triangle over a bar.
pub fn eject(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    let r = s * 0.05;
    if let Some(path) = poly(&[p(0.5, 0.14 + 0.05), p(0.89 - 0.05, 0.6 - 0.03), p(0.11 + 0.05, 0.6 - 0.03)], true) {
        gfx::fill(pm, &path, c);
        gfx::stroke_round(pm, &path, c, 2.0 * r);
    }
    if let Some(path) = gfx::squircle(x + s * 0.12, y + s * 0.71, s * 0.76, s * 0.13, s * 0.04) {
        gfx::fill(pm, &path, c);
    }
}

/// The Bluetooth rune at symbol weight (the Control Center one is lighter).
pub fn bluetooth_rune(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let p = |a: f32, b: f32| (x + s * a, y + s * b);
    let pts = [p(0.27, 0.3), p(0.72, 0.68), p(0.5, 0.9), p(0.5, 0.1), p(0.72, 0.32), p(0.27, 0.7)];
    if let Some(path) = poly(&pts, false) {
        gfx::stroke_round(pm, &path, c, s * EXTRA_LINE);
    }
}

/// SF `cpu`: a chip outline with three pins a side and a filled core.
pub fn cpu(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let lw = s * EXTRA_LINE;
    let (b0, b1) = (0.17f32, 0.83f32);
    if let Some(p) = gfx::squircle(x + s * b0 + lw / 2.0, y + s * b0 + lw / 2.0, s * (b1 - b0) - lw, s * (b1 - b0) - lw, s * 0.12) {
        gfx::stroke(pm, &p, c, lw);
    }
    if let Some(p) = gfx::squircle(x + s * 0.36, y + s * 0.36, s * 0.28, s * 0.28, s * 0.05) {
        gfx::fill(pm, &p, c);
    }
    for t in [0.33f32, 0.5, 0.67] {
        for (a, b) in [((t, 0.04), (t, b0)), ((t, b1), (t, 0.96)), ((0.04, t), (b0, t)), ((b1, t), (0.96, t))] {
            if let Some(p) = poly(&[(x + s * a.0, y + s * a.1), (x + s * b.0, y + s * b.1)], false) {
                gfx::stroke_round(pm, &p, c, lw * 0.85);
            }
        }
    }
}
