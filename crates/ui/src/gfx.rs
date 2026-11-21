//! Drawing primitives on top of tiny-skia: continuous-corner shapes, cached
//! nine-slice shadows, box blur and mipmapped images.

use std::cell::OnceCell;

use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Shader, Stroke,
    Transform,
};

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a.clamp(0.0, 1.0))
        .unwrap_or(Color::BLACK)
}

pub fn fade(c: Color, mul: f32) -> Color {
    let mut c = c;
    c.apply_opacity(mul.clamp(0.0, 1.0));
    c
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        a.red() + (b.red() - a.red()) * t,
        a.green() + (b.green() - a.green()) * t,
        a.blue() + (b.blue() - a.blue()) * t,
        a.alpha() + (b.alpha() - a.alpha()) * t,
    )
    .unwrap_or(a)
}

// Bezier constants of the iOS 7+ continuous-curvature rounded rect: the corner
// starts 1.528 r before the vertex instead of r, which removes the visible
// "kink" a circular arc makes where it meets the straight edge.
const C0: f32 = 1.528_664_8;
const C1: f32 = 1.088_493_2;
const C2: f32 = 0.868_406_9;
const C3: f32 = 0.631_494;
const C4: f32 = 0.074_911_39;
const C5: f32 = 0.372_823_9;
const C6: f32 = 0.169_060_13;

/// Rounded rectangle with continuous ("squircle") corners.
pub fn squircle(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w.min(h) / 2.0 / C0).max(0.0);
    if r < 0.05 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    // Each corner: vertex, direction of travel on the incoming edge, on the outgoing edge.
    let corners = [
        ((x + w, y), (1.0, 0.0), (0.0, 1.0)),
        ((x + w, y + h), (0.0, 1.0), (-1.0, 0.0)),
        ((x, y + h), (-1.0, 0.0), (0.0, -1.0)),
        ((x, y), (0.0, -1.0), (1.0, 0.0)),
    ];
    let pt = |c: usize, a: f32, b: f32| -> (f32, f32) {
        let ((vx, vy), (ix, iy), (ox, oy)) = corners[c];
        (vx - ix * a * r + ox * b * r, vy - iy * a * r + oy * b * r)
    };
    let mut pb = PathBuilder::new();
    let (sx, sy) = pt(3, 0.0, C0);
    pb.move_to(sx, sy);
    for c in 0..4 {
        let p = pt(c, C0, 0.0);
        pb.line_to(p.0, p.1);
        for [a, b, d] in [
            [(C1, 0.0), (C2, 0.0), (C3, C4)],
            [(C5, C6), (C6, C5), (C4, C3)],
            [(0.0, C2), (0.0, C1), (0.0, C0)],
        ] {
            let (p1, p2, p3) = (pt(c, a.0, a.1), pt(c, b.0, b.1), pt(c, d.0, d.1));
            pb.cubic_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
        }
    }
    pb.close();
    pb.finish()
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r)
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

pub fn fill(pm: &mut Pixmap, path: &Path, color: Color) {
    pm.fill_path(path, &paint(color), FillRule::Winding, Transform::identity(), None);
}

pub fn fill_shader(pm: &mut Pixmap, path: &Path, shader: Shader) {
    let p = Paint { shader, anti_alias: true, ..Paint::default() };
    pm.fill_path(path, &p, FillRule::Winding, Transform::identity(), None);
}

pub fn stroke(pm: &mut Pixmap, path: &Path, color: Color, width: f32) {
    let s = Stroke { width, ..Stroke::default() };
    pm.stroke_path(path, &paint(color), &s, Transform::identity(), None);
}

pub fn stroke_round(pm: &mut Pixmap, path: &Path, color: Color, width: f32) {
    let s = Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Stroke::default()
    };
    pm.stroke_path(path, &paint(color), &s, Transform::identity(), None);
}

/// Fills an axis-aligned rectangle without anti-aliasing (hit areas, separators).
pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        let mut p = paint(color);
        p.anti_alias = false;
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
}

// ---------------------------------------------------------------------------
// Blur

/// Three box blurs of radius `r` approximate a Gaussian with sigma ~ sqrt(r(r+1)).
pub fn box_radius_for_sigma(sigma: f32) -> usize {
    ((sigma * sigma + 0.25).sqrt() - 0.5).round().max(1.0) as usize
}

/// In-place separable box blur of an interleaved u8 buffer with `ch` channels.
pub fn box_blur(buf: &mut [u8], w: usize, h: usize, ch: usize, r: usize, passes: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let n = w.max(h) * ch;
    let mut src = vec![0u8; n];
    for _ in 0..passes {
        for y in 0..h {
            let row = &mut buf[y * w * ch..(y + 1) * w * ch];
            src[..w * ch].copy_from_slice(row);
            blur_line(&src[..w * ch], row, w, ch, r);
        }
        blur_columns(buf, w, h, ch, r);
    }
}

/// The vertical pass, row by row: one running sum per column keeps memory
/// access sequential (a column-at-a-time walk was most of the blur's cost).
fn blur_columns(buf: &mut [u8], w: usize, h: usize, ch: usize, r: usize) {
    let stride = w * ch;
    let win = (2 * r + 1) as u32;
    let last = h as isize - 1;
    let orig = buf.to_vec();
    let row = |y: isize| &orig[(y.clamp(0, last) as usize) * stride..][..stride];
    let mut sum = vec![0u32; stride];
    for i in -(r as isize)..=(r as isize) {
        for (s, &v) in sum.iter_mut().zip(row(i)) {
            *s += v as u32;
        }
    }
    for y in 0..h {
        let out = &mut buf[y * stride..][..stride];
        for (o, &s) in out.iter_mut().zip(&sum) {
            *o = ((s + win / 2) / win) as u8;
        }
        let (add, sub) = (row(y as isize + r as isize + 1), row(y as isize - r as isize));
        for ((s, &a), &b) in sum.iter_mut().zip(add).zip(sub) {
            *s = *s + a as u32 - b as u32;
        }
    }
}

fn blur_line(src: &[u8], dst: &mut [u8], n: usize, ch: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let last = n as isize - 1;
    for c in 0..ch {
        // Edges are clamped, so a blurred edge does not fade toward transparent.
        let at = |i: isize| -> u32 { src[(i.clamp(0, last) as usize) * ch + c] as u32 };
        let mut sum: u32 = 0;
        for i in -(r as isize)..=(r as isize) {
            sum += at(i);
        }
        for i in 0..n {
            dst[i * ch + c] = ((sum + win / 2) / win) as u8;
            sum += at(i as isize + r as isize + 1);
            sum -= at(i as isize - r as isize);
        }
    }
}

// ---------------------------------------------------------------------------
// Shadows

/// A blurred alpha mask of one rounded rect, drawn nine-slice style so a shadow
/// of any size costs one pass over its pixels and no blur at draw time.
pub struct ShadowTex {
    alpha: Vec<u8>,
    n: usize,
    pad: usize,
    edge: usize,
}

impl ShadowTex {
    pub fn new(radius: f32, sigma: f32) -> Self {
        let edge = (radius * C0).ceil() as usize + 2;
        let br = box_radius_for_sigma(sigma);
        let pad = br * 3 + 1;
        let core = 2 * edge + 1;
        let n = core + 2 * pad;
        let mut pm = Pixmap::new(n as u32, n as u32).expect("shadow size");
        if let Some(p) = squircle(pad as f32, pad as f32, core as f32, core as f32, radius) {
            fill(&mut pm, &p, Color::BLACK);
        }
        let mut alpha: Vec<u8> = pm.data().chunks_exact(4).map(|p| p[3]).collect();
        box_blur(&mut alpha, n, n, 1, br, 3);
        Self { alpha, n, pad, edge }
    }

    /// Draws the shadow of the shape rect `x, y, w, h`. The middle of the
    /// nine-slice is skipped: callers draw an opaque body over it, and every
    /// caller's offset is smaller than the corner extent, so it stays covered.
    pub fn draw(&self, pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let pad = self.pad as i32;
        let tx0 = x.round() as i32 - pad;
        let ty0 = y.round() as i32 - pad;
        let tw = w.round().max(1.0) as i32 + 2 * pad;
        let th = h.round().max(1.0) as i32 + 2 * pad;
        let n = self.n as i32;
        let lead = pad + self.edge as i32;
        let mid = lead;
        let map = |t: i32, total: i32| -> usize {
            if t < lead {
                t.min(n - 1) as usize
            } else if t >= total - lead {
                (t - (total - n)).clamp(0, n - 1) as usize
            } else {
                mid as usize
            }
        };
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let (cr, cg, cb, ca) = (color.red(), color.green(), color.blue(), color.alpha());
        let data = pm.data_mut();
        for ty in 0..th {
            let py = ty0 + ty;
            if py < 0 || py >= ph {
                continue;
            }
            let srow = map(ty, th) * self.n;
            let x_start = (-tx0).max(0);
            let x_end = tw.min(pw - tx0);
            let inner_row = ty >= lead && ty < th - lead;
            for tx in x_start..x_end {
                if inner_row && tx >= lead && tx < tw - lead {
                    continue;
                }
                let a = self.alpha[srow + map(tx, tw)];
                if a == 0 {
                    continue;
                }
                let sa = a as f32 / 255.0 * ca;
                let i = ((py * pw + tx0 + tx) * 4) as usize;
                blend_premul(&mut data[i..i + 4], cr * sa, cg * sa, cb * sa, sa);
            }
        }
    }
}

/// Source-over of a premultiplied float color (0..1) onto a premultiplied RGBA8 pixel.
#[inline]
pub fn blend_premul(px: &mut [u8], r: f32, g: f32, b: f32, a: f32) {
    let inv = 1.0 - a;
    px[0] = (r * 255.0 + px[0] as f32 * inv + 0.5).min(255.0) as u8;
    px[1] = (g * 255.0 + px[1] as f32 * inv + 0.5).min(255.0) as u8;
    px[2] = (b * 255.0 + px[2] as f32 * inv + 0.5).min(255.0) as u8;
    px[3] = (a * 255.0 + px[3] as f32 * inv + 0.5).min(255.0) as u8;
}

// ---------------------------------------------------------------------------
// Images

/// A premultiplied RGBA image with a mip chain, so heavy downscaling (256 px
/// icons drawn at 48 px) stays sharp without aliasing.
pub struct Image {
    levels: Vec<Pixmap>,
    dark: OnceCell<Vec<Pixmap>>,
    /// Transparent border around the content (fraction of the content size)
    /// holding a baked drop shadow.
    inset: f32,
}

impl Image {
    pub fn new(base: Pixmap) -> Self {
        let mut levels = vec![base];
        loop {
            let last = levels.last().expect("at least one level");
            if last.width() <= 24 || last.height() <= 24 {
                break;
            }
            let next = halve(last);
            levels.push(next);
        }
        Self { levels, dark: OnceCell::new(), inset: 0.0 }
    }

    /// An icon with its soft contact shadow baked in, so a frame draws it in one pass.
    pub fn with_shadow(base: Pixmap) -> Self {
        let (w, h) = (base.width() as usize, base.height() as usize);
        let pad = (w.max(h) as f32 * 0.07).ceil() as usize;
        let (nw, nh) = (w + 2 * pad, h + 2 * pad);
        let dy = (h as f32 * 0.03).round() as usize;
        let mut a = vec![0u8; nw * nh];
        for y in 0..h {
            if y + pad + dy >= nh {
                break;
            }
            for x in 0..w {
                a[(y + pad + dy) * nw + x + pad] = (base.data()[(y * w + x) * 4 + 3] as u32 * 72 / 255) as u8;
            }
        }
        box_blur(&mut a, nw, nh, 1, box_radius_for_sigma(w as f32 * 0.022), 3);
        let Some(mut out) = Pixmap::new(nw as u32, nh as u32) else { return Self::new(base) };
        for (px, v) in out.data_mut().chunks_exact_mut(4).zip(&a) {
            px[3] = *v;
        }
        out.draw_pixmap(pad as i32, pad as i32, base.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        let mut img = Self::new(out);
        img.inset = pad as f32 / w.max(h) as f32;
        img
    }

    /// From premultiplied RGBA8 pixels.
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        let size = tiny_skia::IntSize::from_wh(width, height)?;
        Some(Self::new(Pixmap::from_vec(data, size)?))
    }

    /// The full-size picture.
    pub fn base(&self) -> &Pixmap {
        &self.levels[0]
    }

    pub fn width(&self) -> u32 {
        self.levels[0].width()
    }

    pub fn has_shadow(&self) -> bool {
        self.inset > 0.0
    }

    fn pick(levels: &[Pixmap], size: f32) -> &Pixmap {
        let mut best = &levels[0];
        for l in levels {
            if l.width() as f32 >= size * 0.98 {
                best = l;
            }
        }
        best
    }

    /// Draws the image scaled to `size` px (its larger side) with the top-left at `x, y`.
    pub fn draw(&self, pm: &mut Pixmap, x: f32, y: f32, size: f32, opacity: f32, darken: bool) {
        if size < 1.0 || opacity <= 0.0 {
            return;
        }
        let full = size * (1.0 + 2.0 * self.inset);
        let (x, y, size) = (x - size * self.inset, y - size * self.inset, full);
        let levels = if darken { self.dark.get_or_init(|| self.levels.iter().map(darkened).collect()) } else { &self.levels };
        let lvl = Self::pick(levels, size);
        let s = size / lvl.width().max(lvl.height()) as f32;
        let paint = PixmapPaint { opacity, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
        pm.draw_pixmap(0, 0, lvl.as_ref(), &paint, Transform::from_row(s, 0.0, 0.0, s, x, y), None);
    }
}

fn halve(src: &Pixmap) -> Pixmap {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Pixmap::new(nw as u32, nh as u32).expect("mip size");
    let s = src.data();
    let d = out.data_mut();
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let at = |xx: usize, yy: usize| s[((yy.min(h - 1)) * w + xx.min(w - 1)) * 4 + c] as u32;
                let v = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                d[(y * nw + x) * 4 + c] = ((v + 2) / 4) as u8;
            }
        }
    }
    out
}

fn darkened(src: &Pixmap) -> Pixmap {
    let mut out = src.clone();
    for px in out.data_mut().chunks_exact_mut(4) {
        for c in &mut px[..3] {
            *c = (*c as u32 * 150 / 255) as u8;
        }
    }
    out
}

/// Writes a constant premultiplied pixel into a rectangle without blending.
/// Used for the invisible alpha-1 hit areas, which are drawn on a cleared surface.
pub fn put_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, px: [u8; 4]) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let x0 = (x.floor() as i32).clamp(0, pw);
    let x1 = ((x + w).ceil() as i32).clamp(0, pw);
    let y0 = (y.floor() as i32).clamp(0, ph);
    let y1 = ((y + h).ceil() as i32).clamp(0, ph);
    if x0 >= x1 {
        return;
    }
    let data = pm.data_mut();
    for yy in y0..y1 {
        let row = &mut data[((yy * pw + x0) * 4) as usize..((yy * pw + x1) * 4) as usize];
        for p in row.chunks_exact_mut(4) {
            p.copy_from_slice(&px);
        }
    }
}

/// Fills `path` with pixels of `src`, which is aligned with the target.
pub fn fill_from(pm: &mut Pixmap, path: &Path, src: &Pixmap) {
    let shader = tiny_skia::Pattern::new(src.as_ref(), tiny_skia::SpreadMode::Pad, FilterQuality::Nearest, 1.0, Transform::identity());
    fill_shader(pm, path, shader);
}

// ---------------------------------------------------------------------------
// macOS 26 icon shape

/// Fits an app icon into the macOS 26 icon shape. Icons that already fill a
/// square tile are cut to the squircle; any other shape (round logos, glyphs
/// with transparency) sits on a neutral plate, as macOS does with icons that
/// do not follow its grid. Returns a square pixmap of `src`'s larger side.
pub fn mac_icon(src: &Pixmap, dark: bool) -> Option<Pixmap> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let n = w.max(h) as f32;
    let data = src.data();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if data[(y * w + x) * 4 + 3] > 24 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let mut opaque = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            if data[(y * w + x) * 4 + 3] > 200 {
                opaque += 1;
            }
        }
    }
    let fill = opaque as f32 / (bw * bh);
    let square = (bw / bh - 1.0).abs() < 0.08;
    let tile = square && bw.min(bh) >= n * 0.6 && fill >= 0.86;

    // Apple's grid: the shape is 824/1024 of the canvas, corner radius 0.225 of it.
    let size = n * 0.805;
    let inset = (n - size) / 2.0;
    let shape = squircle(inset, inset, size, size, size * 0.225)?;
    let mut out = Pixmap::new(n as u32, n as u32)?;
    let quality = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
    if tile {
        let mut mask = tiny_skia::Mask::new(n as u32, n as u32)?;
        mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
        // A hair larger than the shape, so the tile's own corners never show inside ours.
        let k = size * 1.02 / bw.min(bh);
        let o = inset - size * 0.01;
        let t = Transform::from_row(k, 0.0, 0.0, k, o - x0 as f32 * k, o - y0 as f32 * k);
        out.draw_pixmap(0, 0, src.as_ref(), &quality, t, Some(&mask));
        return Some(out);
    }
    let (top, bottom, edge) = if dark {
        (rgba(64, 64, 68, 1.0), rgba(36, 36, 40, 1.0), rgba(255, 255, 255, 0.12))
    } else {
        (rgba(255, 255, 255, 1.0), rgba(228, 228, 234, 1.0), rgba(0, 0, 0, 0.08))
    };
    if let Some(sh) = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(0.0, inset),
        tiny_skia::Point::from_xy(0.0, inset + size),
        vec![tiny_skia::GradientStop::new(0.0, top), tiny_skia::GradientStop::new(1.0, bottom)],
        tiny_skia::SpreadMode::Pad,
        Transform::identity(),
    ) {
        fill_shader(&mut out, &shape, sh);
    }
    stroke(&mut out, &shape, edge, (n / 256.0).max(1.0));
    let k = size * 0.64 / bw.max(bh);
    let t = Transform::from_row(k, 0.0, 0.0, k, n / 2.0 - (x0 as f32 + bw / 2.0) * k, n / 2.0 - (y0 as f32 + bh / 2.0) * k);
    out.draw_pixmap(0, 0, src.as_ref(), &quality, t, None);
    Some(out)
}

/// A menu-bar "template" version of a colour icon, as macOS draws menu extras:
/// one colour, shape from alpha (and, for colourful icons, from darkness, so a
/// white logo on a coloured disc becomes a cut-out). Strong reds stay red:
/// they are status (a recording dot, an unread badge).
pub fn template(src: &Pixmap, rgb: [u8; 3]) -> Option<Pixmap> {
    let mut out = Pixmap::new(src.width(), src.height())?;
    let d = src.data();
    // Colourful icons take their shape from luminance, flat ones from alpha.
    let (mut sat_sum, mut n) = (0.0f32, 0.0f32);
    for px in d.chunks_exact(4) {
        if px[3] > 128 {
            let (mx, mn) = (px[0].max(px[1]).max(px[2]) as f32, px[0].min(px[1]).min(px[2]) as f32);
            sat_sum += if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
            n += 1.0;
        }
    }
    let colourful = n > 0.0 && sat_sum / n > 0.25;
    for (o, px) in out.data_mut().chunks_exact_mut(4).zip(d.chunks_exact(4)) {
        let a = px[3] as f32 / 255.0;
        if a <= 0.0 {
            continue;
        }
        let (r, g, b) = (px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a);
        let red = r > 0.55 && r > g * 1.7 && r > b * 1.7;
        if red {
            o.copy_from_slice(px);
            continue;
        }
        let k = if colourful {
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            // Dark and mid tones are the shape; near-white parts are holes.
            ((0.88 - l) / 0.5).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let oa = a * k;
        o[0] = (rgb[0] as f32 * oa) as u8;
        o[1] = (rgb[1] as f32 * oa) as u8;
        o[2] = (rgb[2] as f32 * oa) as u8;
        o[3] = (oa * 255.0) as u8;
    }
    Some(out)
}
