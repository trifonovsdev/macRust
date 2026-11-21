//! Liquid Glass, the material of macOS 26 controls: a clear slab that bends the
//! wallpaper behind its rim, catches a thin specular highlight on the edges
//! that face the light and is tinted only slightly.
//!
//! Rendering is analytic: a rounded-rectangle signed distance field gives
//! coverage, the edge normal and the depth into the bevel. Only the bevel band
//! is computed per pixel; the flat interior is copied from a cached material,
//! so a dock that changes width every frame stays cheap.

use tiny_skia::{Color, Pixmap};

use crate::wallpaper::Vibrancy;

#[derive(Clone, Copy, Debug)]
pub struct GlassStyle {
    /// Premultiplied-over tint laid on the backdrop.
    pub tint: Color,
    /// Width of the curved rim, px.
    pub bevel: f32,
    /// How far the rim pulls the backdrop from outside the shape, px.
    pub refraction: f32,
    /// Strength of the specular rim, 0..1.
    pub specular: f32,
    /// Brightening of the bevel itself (light catching the curved edge), 0..1.
    pub glow: f32,
    /// Strongly blurred backdrop (controls over app windows) instead of the
    /// nearly clear one (the dock over the wallpaper).
    pub frost: bool,
}

impl GlassStyle {
    /// The dock as measured on macOS 26: the backdrop darkened to about 0.78
    /// with its hue kept (dark) or lifted by about 13% white (light), no
    /// bevel, highlight or lens; the edge is a hairline drawn by `rim`.
    pub fn dock(dark: bool, scale: f32) -> Self {
        Self {
            tint: if dark { Color::from_rgba8(0, 0, 0, 56) } else { Color::from_rgba8(255, 255, 255, 33) },
            bevel: scale.max(1.0),
            refraction: 0.0,
            specular: 0.0,
            glow: 0.0,
            frost: false,
        }
    }

    /// Notification Center widgets, notification cards and banners. Dark:
    /// the blurred backdrop at about x0.5 with its hue kept (a black desktop
    /// reads (17,17,17)); light: near-opaque white cards. Rim: `hairline`.
    pub fn card(dark: bool, _scale: f32) -> Self {
        Self {
            tint: if dark {
                Color::from_rgba8(34, 34, 36, 128)
            } else {
                Color::from_rgba8(255, 255, 255, 230)
            },
            bevel: 1.0,
            refraction: 0.0,
            specular: 0.0,
            glow: 0.0,
            frost: true,
        }
    }

    /// Menus: almost a solid sheet. Tahoe menus let at most ~8-10% of the
    /// blurred backdrop through (dark ~#0F0F0F-#292929, light ~80-85% white);
    /// the rim is a separate hairline (`hairline`), not a lit bevel.
    pub fn menu(dark: bool, _scale: f32) -> Self {
        Self {
            tint: if dark {
                Color::from_rgba8(12, 12, 12, 243)
            } else {
                Color::from_rgba8(246, 246, 248, 212)
            },
            bevel: 1.0,
            refraction: 0.0,
            specular: 0.0,
            glow: 0.0,
            frost: true,
        }
    }

    /// Control Center modules: frosted glass tinted by what is behind.
    pub fn panel(dark: bool, scale: f32) -> Self {
        Self {
            tint: if dark {
                Color::from_rgba8(30, 30, 34, 120)
            } else {
                Color::from_rgba8(255, 255, 255, 70)
            },
            bevel: 9.0 * scale,
            refraction: 6.0 * scale,
            specular: if dark { 0.5 } else { 0.8 },
            glow: 0.06,
            frost: true,
        }
    }
}

/// A rounded rectangle in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

impl Shape {
    /// Signed distance (negative inside) and the outward unit normal.
    #[inline]
    fn sdf(&self, px: f32, py: f32) -> (f32, f32, f32) {
        let (hw, hh) = (self.w * 0.5, self.h * 0.5);
        let r = self.r.min(hw).min(hh).max(0.0);
        let (dx, dy) = (px - (self.x + hw), py - (self.y + hh));
        let (sx, sy) = (if dx < 0.0 { -1.0 } else { 1.0 }, if dy < 0.0 { -1.0 } else { 1.0 });
        let qx = dx.abs() - (hw - r);
        let qy = dy.abs() - (hh - r);
        if qx > 0.0 && qy > 0.0 {
            let len = (qx * qx + qy * qy).sqrt().max(1e-4);
            (len - r, sx * qx / len, sy * qy / len)
        } else if qx > qy {
            (qx - r, sx, 0.0)
        } else {
            (qy - r, 0.0, sy)
        }
    }

    /// Horizontal half-span of the shape shrunk by `inset`, on row `py`.
    fn inner_half_span(&self, py: f32, inset: f32) -> Option<f32> {
        let (hw, hh) = (self.w * 0.5 - inset, self.h * 0.5 - inset);
        if hw <= 0.0 || hh <= 0.0 {
            return None;
        }
        let r = (self.r - inset).min(hw).min(hh).max(0.0);
        let dy = (py - (self.y + self.h * 0.5)).abs();
        if dy > hh {
            return None;
        }
        let qy = dy - (hh - r);
        if qy <= 0.0 { Some(hw) } else { Some(hw - r + (r * r - qy * qy).max(0.0).sqrt()) }
    }
}

/// Paints the band of `shape` between depths `d0` and `d1` px (negative is
/// outside the edge), antialiased, blending from `top` to `bottom` colour
/// down the shape: the 0.5 pt rims of menus and widgets.
pub fn hairline(pm: &mut Pixmap, shape: Shape, d0: f32, d1: f32, top: Color, bottom: Color) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let pad = d0.min(0.0).abs().ceil() as i32 + 1;
    let x0 = (shape.x.floor() as i32 - pad).clamp(0, pw);
    let x1 = ((shape.x + shape.w).ceil() as i32 + pad).clamp(0, pw);
    let y0 = (shape.y.floor() as i32 - pad).clamp(0, ph);
    let y1 = ((shape.y + shape.h).ceil() as i32 + pad).clamp(0, ph);
    let reach = d1.max(0.0) + 1.0;
    let data = pm.data_mut();
    for y in y0..y1 {
        let fy = y as f32 + 0.5;
        let t = ((fy - shape.y) / shape.h.max(1.0)).clamp(0.0, 1.0);
        let c = [
            top.red() + (bottom.red() - top.red()) * t,
            top.green() + (bottom.green() - top.green()) * t,
            top.blue() + (bottom.blue() - top.blue()) * t,
            top.alpha() + (bottom.alpha() - top.alpha()) * t,
        ];
        // Rows well inside only touch the left and right edges.
        let skip = shape.inner_half_span(fy, reach);
        let cx = shape.x + shape.w * 0.5;
        for x in x0..x1 {
            let fx = x as f32 + 0.5;
            if let Some(hs) = skip
                && (fx - cx).abs() < hs - 1.0
            {
                continue;
            }
            let depth = -shape.sdf(fx, fy).0;
            let cov = ((depth + 0.5).min(d1) - (depth - 0.5).max(d0)).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let a = c[3] * cov;
            let i = ((y * pw + x) * 4) as usize;
            crate::gfx::blend_premul(&mut data[i..i + 4], c[0] * a, c[1] * a, c[2] * a, a);
        }
    }
}

/// Backdrop + tint for a whole window, sampled once per wallpaper/theme/position.
pub fn material(v: &Vibrancy, ox: f32, oy: f32, w: u32, h: u32, tint: Color) -> Option<Pixmap> {
    material_with(v, ox, oy, w, h, tint, false)
}

fn material_with(v: &Vibrancy, ox: f32, oy: f32, w: u32, h: u32, tint: Color, frost: bool) -> Option<Pixmap> {
    material_in(v, ox, oy, w, h, tint, frost, (0, 0, w as i32, h as i32))
}

/// The material over a `w` x `h` window, filled only inside `area`
/// (x0, y0, x1, y1): `draw` reads nothing outside the shape's bounds, and
/// live glass rebuilds this on every change behind it.
#[allow(clippy::too_many_arguments)]
fn material_in(v: &Vibrancy, ox: f32, oy: f32, w: u32, h: u32, tint: Color, frost: bool, area: (i32, i32, i32, i32)) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w.max(1), h.max(1))?;
    let t = premul(tint);
    let data = pm.data_mut();
    let (x0, y0) = (area.0.clamp(0, w as i32) as usize, area.1.clamp(0, h as i32) as usize);
    let (x1, y1) = (area.2.clamp(0, w as i32) as usize, area.3.clamp(0, h as i32) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let (sx, sy) = (ox + x as f32 + 0.5, oy + y as f32 + 0.5);
            let c = if frost { v.sample_frost(sx, sy) } else { v.sample_clear(sx, sy) };
            let o = (y * w as usize + x) * 4;
            for k in 0..3 {
                data[o + k] = (t[k] * 255.0 + c[k] * (1.0 - t[3]) + 0.5).min(255.0) as u8;
            }
            data[o + 3] = 255;
        }
    }
    Some(pm)
}

fn premul(c: Color) -> [f32; 4] {
    let a = c.alpha();
    [c.red() * a, c.green() * a, c.blue() * a, a]
}

/// Draws the glass slab. `mat` is `material()` for this window (same size as
/// `pm`); `ox, oy` is the window's screen position.
pub fn draw(pm: &mut Pixmap, v: &Vibrancy, mat: &Pixmap, ox: f32, oy: f32, shape: Shape, st: &GlassStyle) {
    if shape.w < 1.0 || shape.h < 1.0 || mat.width() != pm.width() || mat.height() != pm.height() {
        return;
    }
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let x0 = (shape.x.floor() as i32 - 1).clamp(0, pw);
    let x1 = ((shape.x + shape.w).ceil() as i32 + 1).clamp(0, pw);
    let y0 = (shape.y.floor() as i32 - 1).clamp(0, ph);
    let y1 = ((shape.y + shape.h).ceil() as i32 + 1).clamp(0, ph);
    let tint = premul(st.tint);
    let bevel = st.bevel.max(1.0);
    // Light from above, slightly left; the opposite edge catches a weaker reflection.
    let (lx, ly) = (-0.42f32, -0.91f32);
    let cx = shape.x + shape.w * 0.5;
    let stride = pw as usize * 4;
    let src = mat.data();
    let dst = pm.data_mut();
    for y in y0..y1 {
        let fy = y as f32 + 0.5;
        let row = y as usize * stride;
        // Interior run: a plain copy of the material.
        let (in0, in1) = match shape.inner_half_span(fy, bevel + 1.0) {
            Some(hs) => (((cx - hs).ceil() as i32).clamp(x0, x1), ((cx + hs).floor() as i32).clamp(x0, x1)),
            None => (x1, x1),
        };
        if in1 > in0 {
            let (a, b) = (row + in0 as usize * 4, row + in1 as usize * 4);
            dst[a..b].copy_from_slice(&src[a..b]);
        }
        for x in (x0..in0.min(x1)).chain(in1.max(in0)..x1) {
            let fx = x as f32 + 0.5;
            let (d, nx, ny) = shape.sdf(fx, fy);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let depth = (-d).max(0.0);
            let t = (1.0 - depth / bevel).clamp(0.0, 1.0);
            let o = row + x as usize * 4;
            // A convex rim: the bend grows quickly towards the edge. Below half a
            // pixel it is invisible and the cached material is the same color.
            let bend = st.refraction * t * t;
            let mut rgb = if bend < 0.5 {
                [src[o] as f32, src[o + 1] as f32, src[o + 2] as f32]
            } else {
                let (sx, sy) = (ox + fx + nx * bend, oy + fy + ny * bend);
                let c = if st.frost { v.sample_frost(sx, sy) } else { v.sample_clear(sx, sy) };
                [
                    tint[0] * 255.0 + c[0] * (1.0 - tint[3]),
                    tint[1] * 255.0 + c[1] * (1.0 - tint[3]),
                    tint[2] * 255.0 + c[2] * (1.0 - tint[3]),
                ]
            };
            let mut spec = st.glow * t * t * t;
            if depth < 4.0 {
                let facing = nx * lx + ny * ly;
                let dir = if facing > 0.0 { facing } else { -facing * 0.55 };
                let k = (depth - 0.6).max(0.0) / 1.1;
                spec += st.specular * dir * dir / (1.0 + k * k * (1.0 + k));
            }
            let spec = spec.clamp(0.0, 1.0);
            for v in &mut rgb {
                *v += (255.0 - *v) * spec;
            }
            let inv = 1.0 - cov;
            for k in 0..3 {
                dst[o + k] = (rgb[k] * cov + dst[o + k] as f32 * inv + 0.5).min(255.0) as u8;
            }
            dst[o + 3] = (255.0 * cov + dst[o + 3] as f32 * inv + 0.5).min(255.0) as u8;
        }
    }
}

/// Lightness added along the inside of a shape's edge, in 0..255 steps:
/// `top` on the upper edge, `bottom` on the lower one and `side` on the
/// vertical sides, blended by the edge normal. `width` px, at least one.
/// Negative values darken. This is the hairline rim of macOS 26 glass.
pub fn rim(pm: &mut Pixmap, shape: Shape, top: f32, bottom: f32, side: f32, width: f32) {
    let width = width.max(1.0);
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let x0 = (shape.x.floor() as i32 - 1).clamp(0, pw);
    let x1 = ((shape.x + shape.w).ceil() as i32 + 1).clamp(0, pw);
    let y0 = (shape.y.floor() as i32 - 1).clamp(0, ph);
    let y1 = ((shape.y + shape.h).ceil() as i32 + 1).clamp(0, ph);
    let cx = shape.x + shape.w * 0.5;
    let stride = pw as usize * 4;
    let dst = pm.data_mut();
    for y in y0..y1 {
        let fy = y as f32 + 0.5;
        let (in0, in1) = match shape.inner_half_span(fy, width + 1.0) {
            Some(hs) => (((cx - hs).ceil() as i32).clamp(x0, x1), ((cx + hs).floor() as i32).clamp(x0, x1)),
            None => (x1, x1),
        };
        for x in (x0..in0.min(x1)).chain(in1.max(in0)..x1) {
            let (d, _, ny) = shape.sdf(x as f32 + 0.5, fy);
            // Overlap of this pixel with the band [-width, 0] of the distance.
            let band = ((d + 0.5).min(0.0) - (d - 0.5).max(-width)).clamp(0.0, 1.0);
            if band <= 0.0 {
                continue;
            }
            let l = if ny < 0.0 { top * -ny } else { bottom * ny } + side * (1.0 - ny.abs());
            let o = y as usize * stride + x as usize * 4;
            let a = dst[o + 3] as f32;
            let add = l * band * a / 255.0;
            for k in 0..3 {
                dst[o + k] = (dst[o + k] as f32 + add).clamp(0.0, a).round() as u8;
            }
        }
    }
}

/// What lies behind a floating panel (a capture of the screen under it, or
/// the wallpaper), with a material cache per tint. Window coordinates.
#[derive(PartialEq)]
struct MatKey {
    tint: [u8; 4],
    frost: bool,
    size: (u32, u32),
    area: (i32, i32, i32, i32),
}

pub struct Backdrop {
    pub vib: Vibrancy,
    /// Mean luminance 0..1: light backdrops get dark text.
    pub luma: f32,
    mats: std::cell::RefCell<Vec<(MatKey, Pixmap)>>,
}

impl Backdrop {
    pub fn new(behind: &Pixmap) -> Self {
        let vib = Vibrancy::new(behind);
        let luma = vib.luma(0.0, 0.0, behind.width() as f32, behind.height() as f32);
        Self { vib, luma, mats: std::cell::RefCell::new(Vec::new()) }
    }

    /// Draws a glass shape; `pm` must be the window the backdrop was taken for.
    pub fn draw(&self, pm: &mut Pixmap, shape: Shape, st: &GlassStyle) {
        let c = st.tint.to_color_u8();
        // The same bounds `draw` walks.
        let area = (
            shape.x.floor() as i32 - 1,
            shape.y.floor() as i32 - 1,
            (shape.x + shape.w).ceil() as i32 + 1,
            (shape.y + shape.h).ceil() as i32 + 1,
        );
        let key = MatKey { tint: [c.red(), c.green(), c.blue(), c.alpha()], frost: st.frost, size: (pm.width(), pm.height()), area };
        let mut mats = self.mats.borrow_mut();
        let i = match mats.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                let Some(m) = material_in(&self.vib, 0.0, 0.0, pm.width(), pm.height(), st.tint, st.frost, area) else { return };
                mats.push((key, m));
                mats.len() - 1
            }
        };
        draw(pm, &self.vib, &mats[i].1, 0.0, 0.0, shape, st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdf_is_negative_inside_and_normals_point_out() {
        let s = Shape { x: 0.0, y: 0.0, w: 100.0, h: 40.0, r: 12.0 };
        let (d, _, _) = s.sdf(50.0, 20.0);
        assert!(d < -19.0);
        let (d, nx, ny) = s.sdf(50.0, 0.5);
        assert!(d < 0.0 && d > -1.0);
        assert!(nx.abs() < 1e-3 && ny < -0.99);
        let (d, nx, ny) = s.sdf(0.0, 0.0);
        assert!(d > 0.0 && nx < 0.0 && ny < 0.0);
    }

    #[test]
    fn interior_span_stays_inside_the_bevel() {
        let s = Shape { x: 10.0, y: 10.0, w: 200.0, h: 60.0, r: 24.0 };
        let hs = s.inner_half_span(40.0, 14.0).unwrap();
        assert!((hs - 86.0).abs() < 1e-3);
        assert!(s.inner_half_span(12.0, 14.0).is_none());
        for y in 25..55 {
            if let Some(hs) = s.inner_half_span(y as f32, 14.0) {
                let (d, _, _) = s.sdf(110.0 + hs - 0.01, y as f32);
                assert!(d <= -13.9, "row {y}: {d}");
            }
        }
    }
}
