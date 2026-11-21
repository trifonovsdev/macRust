//! The desktop picture and the "vibrancy" material sampled from it.
//!
//! The menu bar and the dock always sit over the desktop (both reserve their
//! strip of the work area), so their translucent background is the blurred,
//! saturated wallpaper itself. That costs one texture lookup per pixel instead
//! of a live backdrop blur, and the edges stay anti-aliased.

use tiny_skia::{
    Color, FilterQuality, GradientStop, LinearGradient, Paint, Pattern, Pixmap, Point, RadialGradient, Rect, Shader,
    SpreadMode, Transform,
};

use crate::gfx::{box_blur, rgba};

/// An abstract gradient desktop picture, so the shell looks right out of the box.
pub fn procedural(w: u32, h: u32, dark: bool) -> Pixmap {
    let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("wallpaper size");
    let (wf, hf) = (w as f32, h as f32);
    let base: [Color; 2] = if dark {
        [rgba(16, 22, 58, 1.0), rgba(58, 18, 64, 1.0)]
    } else {
        [rgba(70, 120, 214, 1.0), rgba(232, 128, 150, 1.0)]
    };
    let mut p = Paint::default();
    if let Some(sh) = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(wf * 0.35, hf),
        vec![GradientStop::new(0.0, base[0]), GradientStop::new(1.0, base[1])],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        p.shader = sh;
    }
    if let Some(r) = Rect::from_xywh(0.0, 0.0, wf, hf) {
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
    let blobs: &[(f32, f32, f32, Color)] = if dark {
        &[
            (0.18, 0.25, 0.55, rgba(40, 92, 200, 0.75)),
            (0.82, 0.3, 0.5, rgba(150, 50, 160, 0.6)),
            (0.6, 0.95, 0.6, rgba(220, 90, 70, 0.45)),
            (0.1, 0.95, 0.45, rgba(20, 140, 160, 0.4)),
        ]
    } else {
        &[
            (0.15, 0.2, 0.6, rgba(110, 200, 250, 0.85)),
            (0.85, 0.25, 0.5, rgba(178, 120, 240, 0.7)),
            (0.65, 0.95, 0.65, rgba(255, 170, 110, 0.75)),
            (0.05, 0.9, 0.45, rgba(80, 210, 200, 0.55)),
            (0.45, 0.5, 0.35, rgba(255, 255, 255, 0.18)),
        ]
    };
    let diag = (wf * wf + hf * hf).sqrt();
    for &(bx, by, br, c) in blobs {
        let center = Point::from_xy(bx * wf, by * hf);
        let mut clear = c;
        clear.set_alpha(0.0);
        if let Some(sh) = RadialGradient::new(
            center,
            0.0,
            center,
            br * diag,
            vec![GradientStop::new(0.0, c), GradientStop::new(1.0, clear)],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let p = Paint { shader: sh, ..Paint::default() };
            if let Some(r) = Rect::from_xywh(0.0, 0.0, wf, hf) {
                pm.fill_rect(r, &p, Transform::identity(), None);
            }
        }
    }
    pm
}

/// Half resolution, a strong blur and a little extra saturation.
fn clear_level(screen: &Pixmap) -> (Vec<u8>, usize, usize) {
    let (w, h) = (screen.width() as usize, screen.height() as usize);
    let (cw, ch) = ((w / 2).max(1), (h / 2).max(1));
    let src = screen.data();
    let mut buf = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let r0 = &src[(y * 2) * w * 4..][..w * 4];
        let r1 = &src[(y * 2 + 1).min(h - 1) * w * 4..][..w * 4];
        let out = &mut buf[y * cw * 4..][..cw * 4];
        for x in 0..cw {
            let (a, b) = (x * 8, (x * 2 + 1).min(w - 1) * 4);
            for c in 0..4 {
                let acc = r0[a + c] as u32 + r0[b + c] as u32 + r1[a + c] as u32 + r1[b + c] as u32;
                out[x * 4 + c] = (acc / 4) as u8;
            }
        }
    }
    // Sigma about 7 screen px: the macOS 26 dock keeps only ~15% of the
    // contrast of 10 pt wallpaper detail at 2x.
    box_blur(&mut buf, cw, ch, 4, 3, 3);
    for px in buf.chunks_exact_mut(4) {
        let a = px[3].max(1) as f32;
        let (r, g, b) = (px[0] as f32 / a, px[1] as f32 / a, px[2] as f32 / a);
        let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let sat = |v: f32| ((l + (v - l) * 1.25).clamp(0.0, 1.0) * a).round() as u8;
        px[0] = sat(r);
        px[1] = sat(g);
        px[2] = sat(b);
        px[3] = 255;
    }
    (buf, cw, ch)
}

pub struct Vibrancy {
    low: Pixmap,
    k: f32,
    /// Blurred half-resolution copy: the dock's glass.
    clear: Vec<u8>,
    clear_w: usize,
    clear_h: usize,
    /// Changes whenever the wallpaper does; caches built from it compare this.
    pub id: u64,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Vibrancy {
    /// `screen` is the wallpaper exactly as it covers the screen, in screen pixels.
    pub fn new(screen: &Pixmap) -> Self {
        let k = 8usize;
        let (w, h) = (screen.width() as usize, screen.height() as usize);
        let (clear, clear_w, clear_h) = clear_level(screen);
        let (lw, lh) = ((w / k).max(1), (h / k).max(1));
        let src = screen.data();
        let mut buf = vec![0u8; lw * lh * 4];
        // Column sums per block row, then one division per output pixel.
        let mut acc = vec![0u32; lw * 4];
        for ly in 0..lh {
            acc.fill(0);
            let rows = (ly * k..((ly + 1) * k).min(h)).len() as u32;
            for y in ly * k..((ly + 1) * k).min(h) {
                let row = &src[y * w * 4..][..w * 4];
                for (lx, a) in acc.chunks_exact_mut(4).enumerate() {
                    for px in row[lx * k * 4..((lx + 1) * k).min(w) * 4].chunks_exact(4) {
                        a[0] += px[0] as u32;
                        a[1] += px[1] as u32;
                        a[2] += px[2] as u32;
                        a[3] += px[3] as u32;
                    }
                }
            }
            for lx in 0..lw {
                let n = rows * (lx * k..((lx + 1) * k).min(w)).len() as u32;
                let o = (ly * lw + lx) * 4;
                for c in 0..4 {
                    buf[o + c] = (acc[lx * 4 + c] / n.max(1)) as u8;
                }
            }
        }
        box_blur(&mut buf, lw, lh, 4, 3, 3);
        for px in buf.chunks_exact_mut(4) {
            let a = px[3].max(1) as f32;
            let (r, g, b) = (px[0] as f32 / a, px[1] as f32 / a, px[2] as f32 / a);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let sat = |v: f32| ((l + (v - l) * 1.6).clamp(0.0, 1.0) * a).round() as u8;
            px[0] = sat(r);
            px[1] = sat(g);
            px[2] = sat(b);
            px[3] = 255;
        }
        let size = tiny_skia::IntSize::from_wh(lw as u32, lh as u32).expect("low size");
        Self {
            low: Pixmap::from_vec(buf, size).expect("low pixmap"),
            k: k as f32,
            clear,
            clear_w,
            clear_h,
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Shader for a window whose top-left corner is at screen `ox, oy`.
    pub fn shader(&self, ox: f32, oy: f32) -> Shader<'_> {
        Pattern::new(
            self.low.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bicubic,
            1.0,
            Transform::from_row(self.k, 0.0, 0.0, self.k, -ox, -oy),
        )
    }

    /// The material for a fixed window: blurred wallpaper under `tint`, rendered
    /// once so frames only copy it (sampling the blur live costs ~6 ms a frame).
    pub fn material(&self, ox: f32, oy: f32, w: u32, h: u32, tint: Color) -> Option<Pixmap> {
        let mut pm = Pixmap::new(w.max(1), h.max(1))?;
        let rect = Rect::from_xywh(0.0, 0.0, w as f32, h as f32)?;
        let p = Paint { shader: self.shader(ox, oy), ..Paint::default() };
        pm.fill_rect(rect, &p, Transform::identity(), None);
        let mut p = Paint::default();
        p.set_color(tint);
        pm.fill_rect(rect, &p, Transform::identity(), None);
        Some(pm)
    }

    /// Bilinear sample of the lightly blurred, opaque wallpaper at a screen
    /// point: RGB, 0..255.
    #[inline]
    pub fn sample_clear(&self, sx: f32, sy: f32) -> [f32; 3] {
        let (w, h) = (self.clear_w, self.clear_h);
        let fx = (sx * 0.5 - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (sy * 0.5 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let dx = if x0 + 1 < w { 4 } else { 0 };
        let dy = if y0 + 1 < h { w * 4 } else { 0 };
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let i = (y0 * w + x0) * 4;
        let d = &self.clear[i..i + dy + dx + 3];
        let mut out = [0.0; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let (a, b) = (d[c] as f32, d[dx + c] as f32);
            let (e, f) = (d[dy + c] as f32, d[dy + dx + c] as f32);
            let top = a + (b - a) * tx;
            let bot = e + (f - e) * tx;
            *o = top + (bot - top) * ty;
        }
        out
    }

    /// Bilinear sample of the heavily blurred level ("frosted" glass): RGB 0..255.
    #[inline]
    pub fn sample_frost(&self, sx: f32, sy: f32) -> [f32; 3] {
        let (w, h) = (self.low.width() as usize, self.low.height() as usize);
        let fx = (sx / self.k - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (sy / self.k - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let dx = if x0 + 1 < w { 4 } else { 0 };
        let dy = if y0 + 1 < h { w * 4 } else { 0 };
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let i = (y0 * w + x0) * 4;
        let d = &self.low.data()[i..i + dy + dx + 3];
        let mut out = [0.0; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let (a, b) = (d[c] as f32, d[dx + c] as f32);
            let (e, f) = (d[dy + c] as f32, d[dy + dx + c] as f32);
            let top = a + (b - a) * tx;
            let bot = e + (f - e) * tx;
            *o = top + (bot - top) * ty;
        }
        out
    }

    /// Mean luminance (0..1) of a screen rectangle of the blurred wallpaper.
    pub fn luma(&self, x: f32, y: f32, w: f32, h: f32) -> f32 {
        let (lw, lh) = (self.low.width() as i32, self.low.height() as i32);
        let x0 = ((x / self.k) as i32).clamp(0, lw - 1);
        let y0 = ((y / self.k) as i32).clamp(0, lh - 1);
        let x1 = (((x + w) / self.k).ceil() as i32).clamp(x0 + 1, lw);
        let y1 = (((y + h) / self.k).ceil() as i32).clamp(y0 + 1, lh);
        let d = self.low.data();
        let (mut sum, mut n) = (0.0, 0.0);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let i = ((yy * lw + xx) * 4) as usize;
                sum += (0.2126 * d[i] as f32 + 0.7152 * d[i + 1] as f32 + 0.0722 * d[i + 2] as f32) / 255.0;
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { 0.5 }
    }
}
