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
}

impl GlassStyle {
    pub fn dock(dark: bool, scale: f32) -> Self {
        Self {
            tint: if dark {
                Color::from_rgba8(18, 18, 22, 92)
            } else {
                Color::from_rgba8(255, 255, 255, 56)
            },
            bevel: 13.0 * scale,
            refraction: 11.0 * scale,
            specular: if dark { 0.55 } else { 0.85 },
            glow: if dark { 0.05 } else { 0.10 },
        }
    }

    pub fn panel(dark: bool, scale: f32) -> Self {
        Self {
            tint: if dark {
                Color::from_rgba8(24, 24, 28, 150)
            } else {
                Color::from_rgba8(250, 250, 252, 120)
            },
            bevel: 10.0 * scale,
            refraction: 7.0 * scale,
            specular: if dark { 0.45 } else { 0.7 },
            glow: 0.06,
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

/// Backdrop + tint for a whole window, sampled once per wallpaper/theme/position.
pub fn material(v: &Vibrancy, ox: f32, oy: f32, w: u32, h: u32, tint: Color) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w.max(1), h.max(1))?;
    let t = premul(tint);
    let data = pm.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let c = v.sample_clear(ox + x as f32 + 0.5, oy + y as f32 + 0.5);
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
                let c = v.sample_clear(ox + fx + nx * bend, oy + fy + ny * bend);
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
