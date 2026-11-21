//! Pictures of the file manager: the macOS folder, a blank document, the
//! sidebar's outline symbols and the toolbar glyphs. `x, y` is the top-left
//! of a `s`-sized box unless said otherwise.

use std::f32::consts::PI;

use tiny_skia::{Color, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode, Transform};

use crate::gfx::{self, Image, rgba};

fn grad(pm: &mut Pixmap, path: &tiny_skia::Path, y0: f32, y1: f32, top: Color, bottom: Color) {
    if let Some(shader) = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(path, &p, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
}

/// The macOS 26 folder: a light-blue back with a tab and a lighter front
/// plate, drawn in a square of `s`.
pub fn folder(pm: &mut Pixmap, x: f32, y: f32, s: f32) {
    let (l, w) = (x + s * 0.06, s * 0.88);
    let top = y + s * 0.17;
    let r = s * 0.05;
    // Back plate with the tab on the left.
    let mut pb = PathBuilder::new();
    let tab_w = w * 0.36;
    let tab_h = s * 0.07;
    pb.move_to(l, top + tab_h + r);
    pb.quad_to(l, top, l + r, top);
    pb.line_to(l + tab_w - r * 0.5, top);
    pb.quad_to(l + tab_w + r * 0.2, top, l + tab_w + r, top + tab_h * 0.6);
    pb.quad_to(l + tab_w + r * 1.6, top + tab_h, l + tab_w + r * 2.6, top + tab_h);
    pb.line_to(l + w - r, top + tab_h);
    pb.quad_to(l + w, top + tab_h, l + w, top + tab_h + r);
    pb.line_to(l + w, top + s * 0.62);
    pb.line_to(l, top + s * 0.62);
    pb.close();
    if let Some(p) = pb.finish() {
        grad(pm, &p, top, top + s * 0.4, rgba(76, 162, 236, 1.0), rgba(58, 142, 222, 1.0));
    }
    // Front plate.
    let fy = top + s * 0.15;
    let fh = s * 0.6;
    if let Some(p) = gfx::squircle(l, fy, w, fh, s * 0.07) {
        grad(pm, &p, fy, fy + fh, rgba(128, 200, 252, 1.0), rgba(94, 176, 246, 1.0));
    }
    // The lit top edge of the front plate.
    if let Some(p) = gfx::squircle(l, fy, w, (s * 0.012).max(0.8), s * 0.006) {
        gfx::fill(pm, &p, rgba(255, 255, 255, 0.45));
    }
}

/// A white page with a folded corner: files whose icon has not arrived yet.
pub fn document(pm: &mut Pixmap, x: f32, y: f32, s: f32, dark: bool) {
    let (l, t, w, h) = (x + s * 0.19, y + s * 0.06, s * 0.62, s * 0.86);
    let fold = s * 0.2;
    let mut pb = PathBuilder::new();
    pb.move_to(l, t);
    pb.line_to(l + w - fold, t);
    pb.line_to(l + w, t + fold);
    pb.line_to(l + w, t + h);
    pb.line_to(l, t + h);
    pb.close();
    let edge = if dark { rgba(0, 0, 0, 0.45) } else { rgba(0, 0, 0, 0.16) };
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, rgba(252, 252, 252, 1.0));
        gfx::stroke(pm, &p, edge, (s * 0.012).max(1.0));
    }
    let mut pb = PathBuilder::new();
    pb.move_to(l + w - fold, t);
    pb.line_to(l + w - fold, t + fold);
    pb.line_to(l + w, t + fold);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, rgba(226, 226, 230, 1.0));
        gfx::stroke(pm, &p, edge, (s * 0.012).max(1.0));
    }
}

/// Folder and document art rendered once at 256 px; `Image` mipmaps it, so
/// a 16 px list icon and a 64 pt tile both stay crisp.
pub struct Art {
    pub folder: Image,
    pub document: Image,
    pub document_dark: Image,
}

impl Art {
    pub fn new() -> Self {
        let make = |f: &dyn Fn(&mut Pixmap)| {
            let mut pm = Pixmap::new(256, 256).expect("art size");
            f(&mut pm);
            Image::new(pm)
        };
        Self {
            folder: make(&|pm| folder(pm, 0.0, 0.0, 256.0)),
            document: make(&|pm| document(pm, 0.0, 0.0, 256.0, false)),
            document_dark: make(&|pm| document(pm, 0.0, 0.0, 256.0, true)),
        }
    }
}

impl Default for Art {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Glyphs (strokes in `c`, line width `lw`)

fn stroke(pm: &mut Pixmap, pb: PathBuilder, c: Color, lw: f32) {
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, lw);
    }
}

#[derive(Clone, Copy)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// A chevron centered on `cx, cy`, `h` tall (its arms meet at the tip).
pub fn chevron(pm: &mut Pixmap, cx: f32, cy: f32, h: f32, dir: Dir, c: Color, lw: f32) {
    let d = h / 2.0;
    let k = d * 0.55;
    let mut pb = PathBuilder::new();
    match dir {
        Dir::Left => {
            pb.move_to(cx + k / 2.0, cy - d);
            pb.line_to(cx - k / 2.0, cy);
            pb.line_to(cx + k / 2.0, cy + d);
        }
        Dir::Right => {
            pb.move_to(cx - k / 2.0, cy - d);
            pb.line_to(cx + k / 2.0, cy);
            pb.line_to(cx - k / 2.0, cy + d);
        }
        Dir::Up => {
            pb.move_to(cx - d, cy + k / 2.0);
            pb.line_to(cx, cy - k / 2.0);
            pb.line_to(cx + d, cy + k / 2.0);
        }
        Dir::Down => {
            pb.move_to(cx - d, cy - k / 2.0);
            pb.line_to(cx, cy + k / 2.0);
            pb.line_to(cx + d, cy - k / 2.0);
        }
    }
    stroke(pm, pb, c, lw);
}

/// The glyphs a hovered traffic light shows, in a circle of diameter `d` at `x, y`.
pub fn light_glyph(pm: &mut Pixmap, i: usize, x: f32, y: f32, d: f32, c: Color) {
    let (cx, cy) = (x + d / 2.0, y + d / 2.0);
    let lw = (d * 0.09).max(1.0);
    match i {
        0 => {
            let k = d * 0.2;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - k, cy - k);
            pb.line_to(cx + k, cy + k);
            pb.move_to(cx + k, cy - k);
            pb.line_to(cx - k, cy + k);
            stroke(pm, pb, c, lw);
        }
        1 => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx - d * 0.27, cy);
            pb.line_to(cx + d * 0.27, cy);
            stroke(pm, pb, c, lw * 1.1);
        }
        _ => {
            // Two triangles pointing away from each other (full screen / zoom).
            let k = d * 0.25;
            let g = d * 0.035;
            for sgn in [-1.0f32, 1.0] {
                let mut pb = PathBuilder::new();
                let (tx, ty) = (cx + sgn * (k + g), cy - sgn * (k + g));
                pb.move_to(tx, ty);
                pb.line_to(tx - sgn * k * 1.25, ty);
                pb.line_to(tx, ty + sgn * k * 1.25);
                pb.close();
                if let Some(p) = pb.finish() {
                    gfx::fill(pm, &p, c);
                }
            }
        }
    }
}

/// Four rounded squares: icon view.
pub fn grid_glyph(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, lw: f32) {
    let q = s * 0.4;
    let g = s * 0.14;
    for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let x = cx + if dx < 0.0 { -g / 2.0 - q } else { g / 2.0 };
        let y = cy + if dy < 0.0 { -g / 2.0 - q } else { g / 2.0 };
        if let Some(p) = gfx::squircle(x, y, q, q, q * 0.22) {
            gfx::stroke(pm, &p, c, lw);
        }
    }
}

/// Three rows with bullets: list view.
pub fn list_glyph(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, lw: f32) {
    for i in -1..=1 {
        let y = cy + i as f32 * s * 0.32;
        if let Some(p) = gfx::circle(cx - s * 0.42, y, lw * 0.85) {
            gfx::fill(pm, &p, c);
        }
        let mut pb = PathBuilder::new();
        pb.move_to(cx - s * 0.22, y);
        pb.line_to(cx + s * 0.5, y);
        stroke(pm, pb, c, lw);
    }
}

fn folder_outline(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32) {
    let r = h * 0.12;
    let tab_w = w * 0.4;
    let tab_h = h * 0.16;
    pb.move_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.line_to(x + tab_w - r, y);
    pb.line_to(x + tab_w, y + tab_h);
    pb.line_to(x + w - r, y + tab_h);
    pb.quad_to(x + w, y + tab_h, x + w, y + tab_h + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.close();
}

/// SF `folder.badge.plus`: an outline folder with a plus at its lower right.
pub fn new_folder_glyph(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, lw: f32) {
    let (w, h) = (s * 0.92, s * 0.7);
    let (x, y) = (cx - w / 2.0, cy - h / 2.0);
    let mut pb = PathBuilder::new();
    folder_outline(&mut pb, x, y, w, h);
    stroke(pm, pb, c, lw);
    let (px, py, k) = (x + w * 0.62, y + h * 0.62, s * 0.15);
    let mut pb = PathBuilder::new();
    pb.move_to(px - k, py);
    pb.line_to(px + k, py);
    pb.move_to(px, py - k);
    pb.line_to(px, py + k);
    stroke(pm, pb, c, lw);
}

pub fn magnifier(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, lw: f32) {
    let r = s * 0.32;
    let (ox, oy) = (cx - s * 0.08, cy - s * 0.08);
    if let Some(p) = gfx::circle(ox, oy, r) {
        gfx::stroke(pm, &p, c, lw);
    }
    let a = PI / 4.0;
    let mut pb = PathBuilder::new();
    pb.move_to(ox + r * a.cos(), oy + r * a.sin());
    pb.line_to(cx + s * 0.42, cy + s * 0.42);
    stroke(pm, pb, c, lw * 1.2);
}

/// A small x in a filled circle: the search field's clear button.
pub fn clear_button(pm: &mut Pixmap, cx: f32, cy: f32, d: f32, disc: Color, mark: Color) {
    if let Some(p) = gfx::circle(cx, cy, d / 2.0) {
        gfx::fill(pm, &p, disc);
    }
    let k = d * 0.18;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - k, cy - k);
    pb.line_to(cx + k, cy + k);
    pb.move_to(cx + k, cy - k);
    pb.line_to(cx - k, cy + k);
    stroke(pm, pb, mark, (d * 0.11).max(1.0));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaceIcon {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Home,
    Drive,
    Removable,
    Network,
    Folder,
    Trash,
}

/// Outline symbols of the sidebar (SF Symbols' look: rounded strokes about
/// 1/12 of the box) in a box of `s` at `x, y`.
pub fn place(pm: &mut Pixmap, icon: PlaceIcon, x: f32, y: f32, s: f32, c: Color) {
    let lw = (s * 0.085).max(1.1);
    let (cx, cy) = (x + s / 2.0, y + s / 2.0);
    match icon {
        PlaceIcon::Desktop => {
            if let Some(p) = gfx::squircle(x + s * 0.06, y + s * 0.14, s * 0.88, s * 0.58, s * 0.08) {
                gfx::stroke(pm, &p, c, lw);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx, y + s * 0.72);
            pb.line_to(cx, y + s * 0.86);
            pb.move_to(cx - s * 0.2, y + s * 0.88);
            pb.line_to(cx + s * 0.2, y + s * 0.88);
            stroke(pm, pb, c, lw);
        }
        PlaceIcon::Downloads => {
            if let Some(p) = gfx::circle(cx, cy, s * 0.43) {
                gfx::stroke(pm, &p, c, lw);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - s * 0.22);
            pb.line_to(cx, cy + s * 0.2);
            pb.move_to(cx - s * 0.17, cy + s * 0.04);
            pb.line_to(cx, cy + s * 0.21);
            pb.line_to(cx + s * 0.17, cy + s * 0.04);
            stroke(pm, pb, c, lw);
        }
        PlaceIcon::Documents => {
            let (l, t, w, h) = (x + s * 0.2, y + s * 0.06, s * 0.6, s * 0.88);
            let f = s * 0.2;
            let mut pb = PathBuilder::new();
            pb.move_to(l + w - f, t);
            pb.line_to(l + s * 0.06, t);
            pb.quad_to(l, t, l, t + s * 0.06);
            pb.line_to(l, t + h - s * 0.06);
            pb.quad_to(l, t + h, l + s * 0.06, t + h);
            pb.line_to(l + w - s * 0.06, t + h);
            pb.quad_to(l + w, t + h, l + w, t + h - s * 0.06);
            pb.line_to(l + w, t + f);
            pb.close();
            pb.move_to(l + w - f, t);
            pb.line_to(l + w - f, t + f);
            pb.line_to(l + w, t + f);
            stroke(pm, pb, c, lw);
        }
        PlaceIcon::Pictures => {
            if let Some(p) = gfx::squircle(x + s * 0.06, y + s * 0.16, s * 0.88, s * 0.68, s * 0.1) {
                gfx::stroke(pm, &p, c, lw);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(x + s * 0.14, y + s * 0.74);
            pb.line_to(x + s * 0.4, y + s * 0.46);
            pb.line_to(x + s * 0.58, y + s * 0.64);
            pb.line_to(x + s * 0.68, y + s * 0.55);
            pb.line_to(x + s * 0.86, y + s * 0.74);
            stroke(pm, pb, c, lw);
            if let Some(p) = gfx::circle(x + s * 0.68, y + s * 0.36, s * 0.07) {
                gfx::fill(pm, &p, c);
            }
        }
        PlaceIcon::Music => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + s * 0.4, y + s * 0.72);
            pb.line_to(x + s * 0.4, y + s * 0.16);
            pb.line_to(x + s * 0.82, y + s * 0.08);
            pb.line_to(x + s * 0.82, y + s * 0.64);
            stroke(pm, pb, c, lw);
            for (ex, ey) in [(0.28f32, 0.76f32), (0.7, 0.68)] {
                if let Some(r) = tiny_skia::Rect::from_xywh(x + s * (ex - 0.13), y + s * (ey - 0.09), s * 0.25, s * 0.18)
                    && let Some(p) = PathBuilder::from_oval(r)
                {
                    gfx::fill(pm, &p, c);
                }
            }
        }
        PlaceIcon::Videos => {
            if let Some(p) = gfx::squircle(x + s * 0.06, y + s * 0.14, s * 0.88, s * 0.72, s * 0.08) {
                gfx::stroke(pm, &p, c, lw);
            }
            let mut pb = PathBuilder::new();
            for fx in [0.27f32, 0.73] {
                pb.move_to(x + s * fx, y + s * 0.14);
                pb.line_to(x + s * fx, y + s * 0.86);
            }
            pb.move_to(x + s * 0.27, cy);
            pb.line_to(x + s * 0.73, cy);
            for fy in [0.38f32, 0.62] {
                pb.move_to(x + s * 0.06, y + s * fy);
                pb.line_to(x + s * 0.27, y + s * fy);
                pb.move_to(x + s * 0.73, y + s * fy);
                pb.line_to(x + s * 0.94, y + s * fy);
            }
            stroke(pm, pb, c, lw * 0.8);
        }
        PlaceIcon::Home => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + s * 0.06, y + s * 0.5);
            pb.line_to(cx, y + s * 0.1);
            pb.line_to(x + s * 0.94, y + s * 0.5);
            pb.move_to(x + s * 0.2, y + s * 0.4);
            pb.line_to(x + s * 0.2, y + s * 0.88);
            pb.line_to(x + s * 0.8, y + s * 0.88);
            pb.line_to(x + s * 0.8, y + s * 0.4);
            pb.move_to(x + s * 0.42, y + s * 0.88);
            pb.line_to(x + s * 0.42, y + s * 0.62);
            pb.line_to(x + s * 0.58, y + s * 0.62);
            pb.line_to(x + s * 0.58, y + s * 0.88);
            stroke(pm, pb, c, lw);
        }
        PlaceIcon::Drive | PlaceIcon::Removable | PlaceIcon::Network => {
            if let Some(p) = gfx::squircle(x + s * 0.04, y + s * 0.3, s * 0.92, s * 0.44, s * 0.1) {
                gfx::stroke(pm, &p, c, lw);
            }
            if let Some(p) = gfx::circle(x + s * 0.76, cy + s * 0.02, s * 0.06) {
                gfx::fill(pm, &p, c);
            }
            if icon == PlaceIcon::Network {
                let mut pb = PathBuilder::new();
                pb.move_to(cx, y + s * 0.74);
                pb.line_to(cx, y + s * 0.92);
                pb.move_to(x + s * 0.2, y + s * 0.92);
                pb.line_to(x + s * 0.8, y + s * 0.92);
                stroke(pm, pb, c, lw);
            }
        }
        PlaceIcon::Folder => {
            let mut pb = PathBuilder::new();
            folder_outline(&mut pb, x + s * 0.04, y + s * 0.16, s * 0.92, s * 0.7);
            stroke(pm, pb, c, lw);
        }
        PlaceIcon::Trash => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + s * 0.12, y + s * 0.22);
            pb.line_to(x + s * 0.88, y + s * 0.22);
            pb.move_to(x + s * 0.36, y + s * 0.22);
            pb.line_to(x + s * 0.38, y + s * 0.08);
            pb.line_to(x + s * 0.62, y + s * 0.08);
            pb.line_to(x + s * 0.64, y + s * 0.22);
            pb.move_to(x + s * 0.2, y + s * 0.22);
            pb.line_to(x + s * 0.26, y + s * 0.92);
            pb.line_to(x + s * 0.74, y + s * 0.92);
            pb.line_to(x + s * 0.8, y + s * 0.22);
            pb.move_to(x + s * 0.42, y + s * 0.38);
            pb.line_to(x + s * 0.43, y + s * 0.78);
            pb.move_to(x + s * 0.58, y + s * 0.38);
            pb.line_to(x + s * 0.57, y + s * 0.78);
            stroke(pm, pb, c, lw);
        }
    }
}

/// The eject symbol (triangle over a bar), drawn as outlines to match the sidebar.
pub fn eject(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + s * 0.5, y + s * 0.14);
    pb.line_to(x + s * 0.88, y + s * 0.6);
    pb.line_to(x + s * 0.12, y + s * 0.6);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, c);
    }
    gfx::fill_rect(pm, x + s * 0.12, y + s * 0.72, s * 0.76, (s * 0.12).max(1.0), c);
}
