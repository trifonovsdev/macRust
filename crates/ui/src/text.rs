//! Text rendering: ab_glyph outlines rasterized at quarter-pixel horizontal
//! offsets and cached, then alpha-blended straight into the pixmap.

use std::cell::RefCell;
use std::collections::HashMap;

pub use ab_glyph::FontArc;
use ab_glyph::{Font, GlyphId, PxScale, ScaleFont, point};
use tiny_skia::{Color, Pixmap};

use crate::gfx::blend_premul;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular = 0,
    Medium = 1,
    Bold = 2,
}

struct GlyphBmp {
    left: i32,
    top: i32,
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

#[derive(Hash, PartialEq, Eq)]
struct Key {
    font: u8,
    glyph: u16,
    size_q: u32,
    frac: u8,
}

pub struct Text {
    /// Index by `Weight`; then fallbacks for scripts the primary font lacks.
    primary: [FontArc; 3],
    fallbacks: Vec<FontArc>,
    cache: RefCell<HashMap<Key, Option<GlyphBmp>>>,
    gamma: [u8; 256],
}

const SUBPIXEL: f32 = 4.0;

impl Text {
    pub fn new(regular: FontArc, medium: FontArc, bold: FontArc) -> Self {
        // Slightly fattened coverage: linear coverage blended in sRGB reads thin
        // compared with the platform text people are used to.
        let mut gamma = [0u8; 256];
        for (i, g) in gamma.iter_mut().enumerate() {
            *g = ((i as f32 / 255.0).powf(0.82) * 255.0).round() as u8;
        }
        Self { primary: [regular, medium, bold], fallbacks: Vec::new(), cache: RefCell::new(HashMap::new()), gamma }
    }

    /// The bundled Inter faces.
    pub fn bundled() -> Self {
        let load = |b: &'static [u8]| FontArc::try_from_slice(b).expect("bundled font parses");
        Self::new(
            load(include_bytes!("../fonts/Inter-Regular.ttf")),
            load(include_bytes!("../fonts/Inter-Medium.ttf")),
            load(include_bytes!("../fonts/Inter-Bold.ttf")),
        )
    }

    pub fn add_fallback(&mut self, font: FontArc) {
        self.fallbacks.push(font);
    }

    fn font(&self, idx: u8) -> &FontArc {
        if (idx as usize) < 3 { &self.primary[idx as usize] } else { &self.fallbacks[idx as usize - 3] }
    }

    /// Picks the font that has the glyph: the requested weight, else a fallback.
    fn resolve(&self, c: char, w: Weight) -> (u8, GlyphId) {
        let f = &self.primary[w as usize];
        let id = f.glyph_id(c);
        if id.0 != 0 || c.is_control() {
            return (w as u8, id);
        }
        for (i, fb) in self.fallbacks.iter().enumerate() {
            let id = fb.glyph_id(c);
            if id.0 != 0 {
                return (3 + i as u8, id);
            }
        }
        (w as u8, id)
    }

    /// `size` is the em size in px, like CSS font-size.
    fn scale(font: &FontArc, size: f32) -> PxScale {
        let upem = font.units_per_em().unwrap_or(2048.0);
        let height = font.ascent_unscaled() - font.descent_unscaled();
        PxScale::from(size * height / upem)
    }

    pub fn ascent(&self, size: f32, w: Weight) -> f32 {
        let f = &self.primary[w as usize];
        f.as_scaled(Self::scale(f, size)).ascent()
    }

    /// Height of capital letters, for optical vertical centering.
    pub fn cap_height(&self, size: f32) -> f32 {
        size * 0.727
    }

    pub fn measure(&self, s: &str, size: f32, w: Weight) -> f32 {
        let mut x = 0.0;
        let mut prev: Option<(u8, GlyphId)> = None;
        for c in s.chars() {
            let (fi, id) = self.resolve(c, w);
            let f = self.font(fi);
            let sf = f.as_scaled(Self::scale(f, size));
            if let Some((pf, pid)) = prev
                && pf == fi
            {
                x += sf.kern(pid, id);
            }
            x += sf.h_advance(id);
            prev = Some((fi, id));
        }
        x
    }

    /// Cuts `s` with an ellipsis so it fits in `max_w`.
    pub fn ellipsize(&self, s: &str, size: f32, w: Weight, max_w: f32) -> String {
        if self.measure(s, size, w) <= max_w {
            return s.to_string();
        }
        let ell = self.measure("…", size, w);
        let mut out = String::new();
        let mut x = 0.0;
        for c in s.chars() {
            let cw = self.measure(c.encode_utf8(&mut [0; 4]), size, w);
            if x + cw + ell > max_w {
                break;
            }
            x += cw;
            out.push(c);
        }
        let trimmed = out.trim_end().to_string();
        trimmed + "…"
    }

    /// Greedy word wrap into at most `max_lines`; the last line is ellipsized.
    pub fn wrap(&self, s: &str, size: f32, w: Weight, max_w: f32, max_lines: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if cur.is_empty() || self.measure(&cand, size, w) <= max_w {
                    cur = cand;
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                }
            }
            lines.push(cur);
            if lines.len() > max_lines {
                break;
            }
        }
        lines.retain(|l| !l.is_empty());
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                *last = self.ellipsize(&format!("{last} …"), size, w, max_w);
            }
        }
        for l in &mut lines {
            if self.measure(l, size, w) > max_w {
                *l = self.ellipsize(l, size, w, max_w);
            }
        }
        lines
    }

    /// Draws `s` with its baseline at `baseline`; returns the advance width.
    /// `clip` is an optional pixel box `(x0, y0, x1, y1)`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        pm: &mut Pixmap,
        s: &str,
        x: f32,
        baseline: f32,
        size: f32,
        w: Weight,
        color: Color,
        clip: Option<(i32, i32, i32, i32)>,
    ) -> f32 {
        let by = baseline.round() as i32;
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let (cx0, cy0, cx1, cy1) = clip.unwrap_or((0, 0, pw, ph));
        let (cx0, cy0, cx1, cy1) = (cx0.max(0), cy0.max(0), cx1.min(pw), cy1.min(ph));
        let (cr, cg, cb, ca) = (color.red(), color.green(), color.blue(), color.alpha());
        let mut pen = x;
        let mut prev: Option<(u8, GlyphId)> = None;
        let mut cache = self.cache.borrow_mut();
        let data = pm.data_mut();
        for c in s.chars() {
            let (fi, id) = self.resolve(c, w);
            let f = self.font(fi);
            let scale = Self::scale(f, size);
            let sf = f.as_scaled(scale);
            if let Some((pf, pid)) = prev
                && pf == fi
            {
                pen += sf.kern(pid, id);
            }
            prev = Some((fi, id));
            let mut ix = pen.floor();
            let mut frac = ((pen - ix) * SUBPIXEL).round() as u8;
            if frac as f32 >= SUBPIXEL {
                ix += 1.0;
                frac = 0;
            }
            let key = Key { font: fi, glyph: id.0, size_q: (size * 16.0) as u32, frac };
            let bmp = cache.entry(key).or_insert_with(|| {
                let g = id.with_scale_and_position(scale, point(frac as f32 / SUBPIXEL, 0.0));
                let og = f.outline_glyph(g)?;
                let b = og.px_bounds();
                let (gw, gh) = (b.width() as usize, b.height() as usize);
                if gw == 0 || gh == 0 {
                    return None;
                }
                let mut cov = vec![0u8; gw * gh];
                og.draw(|px, py, v| {
                    let i = py as usize * gw + px as usize;
                    if i < cov.len() {
                        cov[i] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                });
                Some(GlyphBmp { left: b.min.x.floor() as i32, top: b.min.y.floor() as i32, w: gw, h: gh, cov })
            });
            if let Some(b) = bmp {
                let gx = ix as i32 + b.left;
                let gy = by + b.top;
                for row in 0..b.h as i32 {
                    let py = gy + row;
                    if py < cy0 || py >= cy1 {
                        continue;
                    }
                    for col in 0..b.w as i32 {
                        let px = gx + col;
                        if px < cx0 || px >= cx1 {
                            continue;
                        }
                        let v = b.cov[(row * b.w as i32 + col) as usize];
                        if v == 0 {
                            continue;
                        }
                        let a = self.gamma[v as usize] as f32 / 255.0 * ca;
                        let i = ((py * pw + px) * 4) as usize;
                        blend_premul(&mut data[i..i + 4], cr * a, cg * a, cb * a, a);
                    }
                }
            }
            pen += sf.h_advance(id);
        }
        pen - x
    }
}
