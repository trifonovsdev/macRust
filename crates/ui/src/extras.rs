//! Windows notification-area icons as macOS menu-bar extras.
//!
//! Tahoe draws third-party extras as templates: one colour (the bar's text
//! colour), shape from alpha, 12-15 pt tall at symbol weight (TAHOE-SPEC.md
//! section 2). Windows tray icons are 16 px colour pictures: a coloured plate
//! with a light mark, or a glyph with a status overlay, and their plain alpha
//! silhouette is a blob. `Extra::new` rebuilds them at 4x:
//!
//! - known apps get a drawn symbol ([`identify`]);
//! - a plate carrying a mark becomes the plate's outline with the mark inside,
//!   like SF `paperplane.circle`;
//! - a coloured overlay at the edge (a warning triangle, a check, an unread
//!   dot) stays a separate shape behind a clear gap, like SF badge symbols;
//! - anything else keeps its silhouette, with dark inner detail cut out.

use tiny_skia::Pixmap;

use crate::gfx::{self, rgba};
use crate::glyphs::{self, ShieldMark};

/// Output side: the 16 pt extra box at 4x, drawn through the image's mips.
pub const RES: u32 = 64;
const N: usize = RES as usize;
/// Working pixels per point.
const PX: f32 = RES as f32 / 16.0;
/// Symbol stroke: Tahoe's search ring and Control Center pills are 1.25-1.5 pt.
const STROKE: f32 = 1.4 * PX;
/// Clear space between a shape and its badge or inner mark.
const GAP: f32 = 1.0 * PX;
/// Windows art fills its 16 px canvas; Tahoe's extras are 12-15 pt tall.
const FIT: f32 = 14.0 / 16.0;
const MAX_H: f32 = 15.0 * PX;
/// Thin 1 px Windows strokes are thickened by this much per side (≈0.15 pt).
const GROW: f32 = 0.6;
/// tdesktop's unread dot colour, also drawn by its macOS menu-bar icon.
const UNREAD: [u8; 3] = [242, 60, 52];

/// Apps whose tray icon is replaced by a drawn symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    /// The paper plane; Telegram's macOS icon keeps a red unread counter.
    Telegram { unread: bool },
    /// Windows Security: the shield with its status.
    Security(ShieldMark),
    /// Safely Remove Hardware: the macOS Eject extra.
    Eject,
    Bluetooth,
    /// Task Manager's CPU meter: SF `cpu`.
    Cpu,
}

/// Recognises an icon by its owner's file name and tooltip, reading status
/// from the picture's colours.
pub fn identify(exe: &str, tip: &str, src: &Pixmap) -> Option<Known> {
    let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_lowercase();
    let tip = tip.to_lowercase();
    let hues = Hues::of(&sample(src));
    match exe.as_str() {
        "telegram.exe" => Some(Known::Telegram { unread: hues.red > 0 }),
        "securityhealthsystray.exe" => Some(Known::Security(if hues.red > hues.yellow.max(hues.green) {
            ShieldMark::Cross
        } else if hues.yellow > hues.green {
            ShieldMark::Exclamation
        } else {
            ShieldMark::Check
        })),
        // Explorer hosts several system icons; only the tooltip tells them apart.
        "taskmgr.exe" => Some(Known::Cpu),
        "explorer.exe" if tip.contains("bluetooth") => Some(Known::Bluetooth),
        "explorer.exe" if EJECT_TIPS.iter().any(|w| tip.contains(w)) => Some(Known::Eject),
        _ => None,
    }
}

/// Safely Remove Hardware tooltips (en, ru, de, fr, es, it, pt, uk).
const EJECT_TIPS: [&str; 9] = ["eject", "remove hardware", "извлеч", "auswerfen", "éjecter", "expulsar", "rimozione sicura", "remover hardware", "вилуч"];

/// A converted extra: glyph coverage, plus a red unread dot kept in colour.
pub struct Extra {
    cover: Vec<f32>,
    red_dot: Option<(f32, f32, f32)>,
}

impl Extra {
    pub fn new(src: &Pixmap, exe: &str, tip: &str) -> Option<Self> {
        match identify(exe, tip, src) {
            Some(k) => Some(Self::known(k)),
            None => Self::convert(src),
        }
    }

    /// The drawn symbol of a known app.
    pub fn known(k: Known) -> Self {
        let Some(mut pm) = Pixmap::new(RES, RES) else { return Self { cover: vec![0.0; N * N], red_dot: None } };
        let white = rgba(255, 255, 255, 1.0);
        let at = |pt: f32| ((16.0 - pt) / 2.0 * PX, pt * PX);
        let mut red_dot = None;
        match k {
            Known::Telegram { unread } => {
                let (o, s) = at(16.0);
                glyphs::paper_plane(&mut pm, o, o, s, white);
                if unread {
                    // Bottom left, under the wing, where tdesktop puts it.
                    red_dot = Some((o + s * 0.2, o + s * 0.8, s * 0.13));
                }
            }
            Known::Security(mark) => {
                let (o, s) = at(15.0);
                glyphs::shield(&mut pm, o, o, s, white, mark);
            }
            Known::Eject => {
                let (o, s) = at(16.0);
                glyphs::eject(&mut pm, o, o, s, white);
            }
            Known::Bluetooth => {
                let (o, s) = at(15.0);
                glyphs::bluetooth_rune(&mut pm, o, o, s, white);
            }
            Known::Cpu => {
                let (o, s) = at(15.0);
                glyphs::cpu(&mut pm, o, o, s, white);
            }
        }
        let mut cover: Vec<f32> = pm.data().chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect();
        if let Some((cx, cy, r)) = red_dot {
            cut_disc(&mut cover, cx, cy, r + GAP);
        }
        let (cover, shift) = fit(&cover, 1.0);
        let red_dot = red_dot.map(|(x, y, r)| (x + shift.0, y + shift.1, r));
        Self { cover, red_dot }
    }

    /// The template of an unknown icon, rebuilt from its picture.
    pub fn convert(src: &Pixmap) -> Option<Self> {
        let px = sample(src);
        let peak = px.iter().map(|p| p.a).fold(0.0f32, f32::max);
        if peak < 0.05 {
            return None;
        }
        // Faint icons come up to full strength, like every other extra.
        let alpha: Vec<f32> = px.iter().map(|p| (p.a / peak).min(1.0)).collect();
        let solid: Vec<bool> = alpha.iter().map(|&a| a > 0.5).collect();
        let ink = solid.iter().filter(|&&s| s).count();
        if ink == 0 {
            return None;
        }

        let overlay = overlay(&px, &solid, ink);
        let in_overlay = |i: usize| overlay.as_ref().is_some_and(|o| o.region[i]);
        let base: Vec<f32> = (0..N * N).map(|i| if in_overlay(i) { 0.0 } else { alpha[i] }).collect();
        let base_solid: Vec<bool> = base.iter().map(|&a| a > 0.5).collect();

        let mut out = match plate(&px, &base_solid) {
            Some(marks) => {
                let sd = signed_distance(&base_solid);
                (0..N * N)
                    .map(|i| {
                        let ring = cov(sd[i]) * cov(STROKE - sd[i]);
                        let inner = cov(sd[i] - STROKE - GAP);
                        ring.max(marks[i] * inner)
                    })
                    .collect()
            }
            None => glyph(&px, &base_solid),
        };

        if let Some(o) = overlay {
            // Clear a gap around the overlay, then draw it with its dark
            // inner detail (the "!" of a warning triangle) cut out.
            let d = signed_distance(&o.region);
            for (i, v) in out.iter_mut().enumerate() {
                *v *= cov(-d[i] - GAP);
                if o.region[i] {
                    let hole = !o.comp[i] && (alpha[i] < 0.5 || px[i].luma() < 0.35);
                    *v = if hole { 0.0 } else { 1.0 };
                }
            }
        }
        let (cover, _) = fit(&out, FIT);
        Some(Self { cover, red_dot: None })
    }

    /// The extra in one colour, premultiplied, `RES` px square.
    pub fn paint(&self, rgb: [u8; 3]) -> Option<Pixmap> {
        let mut pm = Pixmap::new(RES, RES)?;
        for (o, &c) in pm.data_mut().chunks_exact_mut(4).zip(&self.cover) {
            let a = c.clamp(0.0, 1.0);
            o[0] = (rgb[0] as f32 * a + 0.5) as u8;
            o[1] = (rgb[1] as f32 * a + 0.5) as u8;
            o[2] = (rgb[2] as f32 * a + 0.5) as u8;
            o[3] = (a * 255.0 + 0.5) as u8;
        }
        if let Some((x, y, r)) = self.red_dot
            && let Some(p) = gfx::circle(x, y, r)
        {
            gfx::fill(&mut pm, &p, rgba(UNREAD[0], UNREAD[1], UNREAD[2], 1.0));
        }
        Some(pm)
    }
}

/// Dark and light templates of a tray icon, for light and dark wallpapers.
pub fn templates(src: &Pixmap, exe: &str, tip: &str) -> Option<(Pixmap, Pixmap)> {
    let e = Extra::new(src, exe, tip)?;
    Some((e.paint([0, 0, 0])?, e.paint([255, 255, 255])?))
}

// ---------------------------------------------------------------------------
// Pixels

#[derive(Clone, Copy, Default, Debug)]
struct Px {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Px {
    fn luma(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    fn max(self) -> f32 {
        self.r.max(self.g).max(self.b)
    }

    fn sat(self) -> f32 {
        let mx = self.max();
        if mx > 0.0 { (mx - self.r.min(self.g).min(self.b)) / mx } else { 0.0 }
    }

    fn hue(self) -> f32 {
        let (mx, mn) = (self.max(), self.r.min(self.g).min(self.b));
        let d = mx - mn;
        if d <= 0.0 {
            return 0.0;
        }
        let h = if mx == self.r {
            ((self.g - self.b) / d).rem_euclid(6.0)
        } else if mx == self.g {
            (self.b - self.r) / d + 2.0
        } else {
            (self.r - self.g) / d + 4.0
        };
        h * 60.0
    }

    /// A strongly coloured, clearly visible pixel.
    fn colourful(self) -> bool {
        self.a > 0.5 && self.sat() > 0.45 && self.max() > 0.35
    }

    fn dist(self, o: Px) -> f32 {
        ((self.r - o.r).powi(2) + (self.g - o.g).powi(2) + (self.b - o.b).powi(2)).sqrt() / 3f32.sqrt()
    }
}

/// Counts of status-coloured pixels.
struct Hues {
    red: usize,
    yellow: usize,
    green: usize,
}

impl Hues {
    fn of(px: &[Px]) -> Self {
        let mut h = Hues { red: 0, yellow: 0, green: 0 };
        // A 16 px dot of radius 2 covers 12 source pixels = 192 here; ignore stray edges.
        let min = N * N / 200;
        for p in px.iter().filter(|p| p.colourful() && p.sat() > 0.5) {
            match p.hue() {
                x if !(15.0..=340.0).contains(&x) => h.red += 1,
                x if (25.0..=65.0).contains(&x) => h.yellow += 1,
                x if (80.0..=170.0).contains(&x) => h.green += 1,
                _ => {}
            }
        }
        for v in [&mut h.red, &mut h.yellow, &mut h.green] {
            if *v < min {
                *v = 0;
            }
        }
        h
    }
}

/// The icon resampled to `N` x `N` straight RGBA: halved down to at most
/// 2N first, then bilinear, so 16 px art turns into smooth shapes.
fn sample(src: &Pixmap) -> Vec<Px> {
    let mut cur = src.clone();
    while cur.width().max(cur.height()) > 2 * RES {
        cur = gfx::halve(&cur);
    }
    let (w, h) = (cur.width() as i32, cur.height() as i32);
    let side = w.max(h) as f32;
    let (ox, oy) = ((side - w as f32) / 2.0, (side - h as f32) / 2.0);
    let d = cur.data();
    let at = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= w || y >= h {
            return [0.0; 4];
        }
        let o = ((y * w + x) * 4) as usize;
        [d[o] as f32, d[o + 1] as f32, d[o + 2] as f32, d[o + 3] as f32]
    };
    let k = side / N as f32;
    let mut out = vec![Px::default(); N * N];
    for y in 0..N {
        for x in 0..N {
            let u = (x as f32 + 0.5) * k - 0.5 - ox;
            let v = (y as f32 + 0.5) * k - 0.5 - oy;
            let (x0, y0) = (u.floor() as i32, v.floor() as i32);
            let (fx, fy) = (u - x0 as f32, v - y0 as f32);
            let mut acc = [0.0f32; 4];
            for (dx, dy, wt) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
                let p = at(x0 + dx, y0 + dy);
                for c in 0..4 {
                    acc[c] += p[c] * wt;
                }
            }
            let a = acc[3] / 255.0;
            if a > 0.002 {
                out[y * N + x] = Px { r: (acc[0] / 255.0 / a).min(1.0), g: (acc[1] / 255.0 / a).min(1.0), b: (acc[2] / 255.0 / a).min(1.0), a };
            }
        }
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Pixel coverage of a signed distance (positive inside).
fn cov(sd: f32) -> f32 {
    (sd + 0.5).clamp(0.0, 1.0)
}

/// Signed distance to the mask's edge, positive inside, in working px
/// (two-pass chamfer).
fn signed_distance(mask: &[bool]) -> Vec<f32> {
    let dist_to = |target: bool| {
        let big = 1e4f32;
        let mut d: Vec<f32> = mask.iter().map(|&m| if m == target { 0.0 } else { big }).collect();
        let s2 = std::f32::consts::SQRT_2;
        for y in 0..N {
            for x in 0..N {
                let i = y * N + x;
                let mut v = d[i];
                if x > 0 {
                    v = v.min(d[i - 1] + 1.0);
                }
                if y > 0 {
                    v = v.min(d[i - N] + 1.0);
                    if x > 0 {
                        v = v.min(d[i - N - 1] + s2);
                    }
                    if x + 1 < N {
                        v = v.min(d[i - N + 1] + s2);
                    }
                }
                d[i] = v;
            }
        }
        for y in (0..N).rev() {
            for x in (0..N).rev() {
                let i = y * N + x;
                let mut v = d[i];
                if x + 1 < N {
                    v = v.min(d[i + 1] + 1.0);
                }
                if y + 1 < N {
                    v = v.min(d[i + N] + 1.0);
                    if x + 1 < N {
                        v = v.min(d[i + N + 1] + s2);
                    }
                    if x > 0 {
                        v = v.min(d[i + N - 1] + s2);
                    }
                }
                d[i] = v;
            }
        }
        d
    };
    let to_out = dist_to(false);
    let to_in = dist_to(true);
    mask.iter().enumerate().map(|(i, &m)| if m { to_out[i] - 0.5 } else { 0.5 - to_in[i] }).collect()
}

/// 4-connected components of `mask`, largest first, as pixel index lists.
fn components(mask: &[bool]) -> Vec<Vec<usize>> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut comp = Vec::new();
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % N, i / N);
            let mut visit = |j: usize| {
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < N {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - N);
            }
            if y + 1 < N {
                visit(i + N);
            }
        }
        out.push(comp);
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.len()));
    out
}

/// Bounding box (x0, y0, x1, y1), inclusive, of the set pixels.
fn bbox(mask: &[bool]) -> Option<(usize, usize, usize, usize)> {
    let mut b: Option<(usize, usize, usize, usize)> = None;
    for (i, _) in mask.iter().enumerate().filter(|(_, m)| **m) {
        let (x, y) = (i % N, i / N);
        b = Some(match b {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    b
}

struct Overlay {
    /// The coloured shape itself.
    comp: Vec<bool>,
    /// The shape with everything it encloses.
    region: Vec<bool>,
}

/// A coloured status overlay: a small coloured shape at the edge of the ink,
/// on an icon that is mostly something else.
fn overlay(px: &[Px], solid: &[bool], ink: usize) -> Option<Overlay> {
    let colourful: Vec<bool> = px.iter().map(|p| p.colourful()).collect();
    let (ix0, iy0, ix1, iy1) = bbox(solid)?;
    let edge = (GAP * 1.5) as usize;
    let comp = components(&colourful).into_iter().find(|c| {
        if c.len() * 100 < ink || c.len() * 100 > ink * 35 {
            return false;
        }
        let mut m = vec![false; N * N];
        for &i in c {
            m[i] = true;
        }
        let Some((x0, y0, x1, y1)) = bbox(&m) else { return false };
        x0 <= ix0 + edge || y0 <= iy0 + edge || x1 + edge >= ix1 || y1 + edge >= iy1
    })?;
    let mut comp_mask = vec![false; N * N];
    for &i in &comp {
        comp_mask[i] = true;
    }
    // Enclosed pixels: those a flood fill from the border cannot reach.
    let mut outside = vec![false; N * N];
    let mut stack: Vec<usize> = (0..N * N).filter(|&i| (i % N == 0 || i % N == N - 1 || i / N == 0 || i / N == N - 1) && !comp_mask[i]).collect();
    for &i in &stack {
        outside[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % N, i / N);
        for j in [(x > 0).then(|| i - 1), (x + 1 < N).then(|| i + 1), (y > 0).then(|| i - N), (y + 1 < N).then(|| i + N)].into_iter().flatten() {
            if !comp_mask[j] && !outside[j] {
                outside[j] = true;
                stack.push(j);
            }
        }
    }
    let region = outside.iter().map(|&o| !o).collect();
    Some(Overlay { comp: comp_mask, region })
}

/// The mark coverage of a plate icon (a solid shape whose most common colour
/// carries a differently coloured mark), or `None` for other icons.
fn plate(px: &[Px], solid: &[bool]) -> Option<Vec<f32>> {
    let (x0, y0, x1, y1) = bbox(solid)?;
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let area = solid.iter().filter(|&&s| s).count();
    if (bw as f32) < 0.55 * N as f32 || (bh as f32) < 0.55 * N as f32 || (area as f32) < 0.72 * (bw * bh) as f32 {
        return None;
    }
    let colour = mode_colour(px, solid);
    let marks: Vec<f32> = px.iter().zip(solid).map(|(p, &s)| if s { smoothstep(0.18, 0.35, p.dist(colour)) } else { 0.0 }).collect();
    let marked: f32 = marks.iter().sum();
    (marked >= 0.02 * area as f32 && marked <= 0.7 * area as f32).then_some(marks)
}

/// The most common colour (3 bits per channel) of the solid pixels, averaged.
fn mode_colour(px: &[Px], solid: &[bool]) -> Px {
    let bin = |p: &Px| ((p.r * 7.99) as usize) << 6 | ((p.g * 7.99) as usize) << 3 | (p.b * 7.99) as usize;
    let mut hist = vec![0usize; 512];
    for (p, _) in px.iter().zip(solid).filter(|(_, s)| **s) {
        hist[bin(p)] += 1;
    }
    let best = (0..512).max_by_key(|&b| hist[b]).unwrap_or(0);
    let mut acc = Px::default();
    let mut n = 0.0;
    for (p, _) in px.iter().zip(solid).filter(|(p, s)| **s && bin(p) == best) {
        acc.r += p.r;
        acc.g += p.g;
        acc.b += p.b;
        n += 1.0;
    }
    if n > 0.0 {
        acc.r /= n;
        acc.g /= n;
        acc.b /= n;
    }
    acc.a = 1.0;
    acc
}

/// A glyph icon: its silhouette, slightly thickened to symbol weight, with
/// dark inner detail of a light glyph cut out.
fn glyph(px: &[Px], solid: &[bool]) -> Vec<f32> {
    let colour = mode_colour(px, solid);
    let sd = signed_distance(solid);
    (0..N * N)
        .map(|i| {
            let hole = if colour.luma() > 0.6 && solid[i] { smoothstep(0.3, 0.45, colour.luma() - px[i].luma()) } else { 0.0 };
            cov(sd[i] + GROW) * (1.0 - hole)
        })
        .collect()
}

/// Clears a disc (and keeps everything else) in a coverage map.
fn cut_disc(cover: &mut [f32], cx: f32, cy: f32, r: f32) {
    for (i, v) in cover.iter_mut().enumerate() {
        let (x, y) = ((i % N) as f32 + 0.5, (i / N) as f32 + 0.5);
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        *v *= (d - r).clamp(0.0, 1.0);
    }
}

/// Scales the ink by `k` (at most `MAX_H` tall) and centres it in the box.
/// Returns the map and the shift applied to an unscaled ink.
fn fit(cover: &[f32], k: f32) -> (Vec<f32>, (f32, f32)) {
    let ink: Vec<bool> = cover.iter().map(|&c| c > 0.02).collect();
    let Some((x0, y0, x1, y1)) = bbox(&ink) else { return (cover.to_vec(), (0.0, 0.0)) };
    let (cx, cy) = ((x0 + x1 + 1) as f32 / 2.0, (y0 + y1 + 1) as f32 / 2.0);
    let k = k.min(MAX_H / (y1 - y0 + 1) as f32);
    let c = N as f32 / 2.0;
    let at = |x: i32, y: i32| if x < 0 || y < 0 || x >= N as i32 || y >= N as i32 { 0.0 } else { cover[y as usize * N + x as usize] };
    let mut out = vec![0.0; N * N];
    for y in 0..N {
        for x in 0..N {
            let u = cx + (x as f32 + 0.5 - c) / k - 0.5;
            let v = cy + (y as f32 + 0.5 - c) / k - 0.5;
            let (ux, vy) = (u.floor() as i32, v.floor() as i32);
            let (fx, fy) = (u - ux as f32, v - vy as f32);
            out[y * N + x] = at(ux, vy) * (1.0 - fx) * (1.0 - fy) + at(ux + 1, vy) * fx * (1.0 - fy) + at(ux, vy + 1) * (1.0 - fx) * fy + at(ux + 1, vy + 1) * fx * fy;
        }
    }
    (out, (c - cx, c - cy))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16 px icon from straight RGBA per pixel.
    fn icon(f: impl Fn(u32, u32) -> [u8; 4]) -> Pixmap {
        let mut pm = Pixmap::new(16, 16).unwrap();
        for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
            let c = f(i as u32 % 16, i as u32 / 16);
            let a = c[3] as u32;
            px.copy_from_slice(&[(c[0] as u32 * a / 255) as u8, (c[1] as u32 * a / 255) as u8, (c[2] as u32 * a / 255) as u8, c[3]]);
        }
        pm
    }

    fn disc(x: u32, y: u32, cx: f32, cy: f32, r: f32) -> bool {
        (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2) <= r * r
    }

    /// Coverage at a point given in 16 pt box units.
    fn at(e: &Extra, x: f32, y: f32) -> f32 {
        e.cover[(y * PX) as usize * N + (x * PX) as usize]
    }

    /// Coverage along a horizontal line through the box centre, at 1 pt steps.
    fn row(e: &Extra) -> Vec<bool> {
        (0..16).map(|x| at(e, x as f32 + 0.5, 8.0) > 0.5).collect()
    }

    #[test]
    fn flat_and_faint_glyphs_keep_their_whole_shape() {
        let inside = |x: u32, y: u32| (4..12).contains(&x) && (2..14).contains(&y);
        let flat = Extra::convert(&icon(|x, y| if inside(x, y) { [240, 190, 40, 255] } else { [0; 4] })).unwrap();
        assert!(at(&flat, 8.0, 8.0) > 0.99);
        // Faint grey comes up to full strength.
        let faint = Extra::convert(&icon(|x, y| if inside(x, y) { [200, 200, 200, 140] } else { [0; 4] })).unwrap();
        assert!(at(&faint, 8.0, 8.0) > 0.99);
        // 12 px of ink at 14/16 scale, centred.
        let rows: Vec<usize> = (0..N).filter(|&y| flat.cover[y * N + N / 2] > 0.5).collect();
        let tall = (rows[rows.len() - 1] - rows[0] + 1) as f32 / PX;
        assert!((tall - 12.0 * FIT).abs() < 0.6, "{tall}");
    }

    #[test]
    fn a_plate_with_a_mark_becomes_an_outline_around_the_mark() {
        // A blue disc with a white square in the middle (Telegram's colour icon).
        let src = icon(|x, y| {
            if (6..10).contains(&x) && (6..10).contains(&y) {
                [255, 255, 255, 255]
            } else if disc(x, y, 8.0, 8.0, 7.5) {
                [40, 160, 230, 255]
            } else {
                [0; 4]
            }
        });
        let e = Extra::convert(&src).unwrap();
        let r = row(&e);
        // Ring, clear space, mark, clear space, ring: not a solid blob
        // (the 15 pt disc is 13 pt after the 14/16 fit).
        assert!(r[2] && r[8] && r[13], "{r:?}");
        assert!(!r[4] && !r[11] && !r[0] && !r[15], "{r:?}");
    }

    #[test]
    fn a_corner_overlay_is_parted_from_its_glyph() {
        // A white block with a yellow square in its bottom-right corner.
        let src = icon(|x, y| {
            if (9..16).contains(&x) && (9..16).contains(&y) {
                [255, 185, 0, 255]
            } else if (1..12).contains(&x) && (1..12).contains(&y) {
                [255, 255, 255, 255]
            } else {
                [0; 4]
            }
        });
        let e = Extra::convert(&src).unwrap();
        let diag: Vec<bool> = (0..32).map(|i| {
            let t = i as f32 / 2.0;
            at(&e, t.min(15.9), t.min(15.9)) > 0.5
        }).collect();
        // Along the diagonal: glyph, a gap, then the overlay.
        let first_gap = diag.iter().position(|&v| !v).unwrap_or(0);
        let on_again = diag.iter().skip(first_gap + 1).position(|&v| v);
        assert!(diag[8] && on_again.is_some(), "{diag:?}");
    }

    #[test]
    fn known_apps_are_recognised() {
        let plane = icon(|x, y| if disc(x, y, 9.0, 7.0, 5.0) { [255, 255, 255, 255] } else { [0; 4] });
        let unread = icon(|x, y| if disc(x, y, 3.9, 12.7, 2.2) { [242, 60, 52, 255] } else if disc(x, y, 9.0, 7.0, 5.0) { [255; 4] } else { [0; 4] });
        let warn = icon(|x, y| if (8..16).contains(&x) && (8..16).contains(&y) { [255, 185, 0, 255] } else { [255; 4] });
        assert_eq!(identify("C:\\Apps\\Telegram Desktop\\Telegram.exe", "Telegram", &plane), Some(Known::Telegram { unread: false }));
        assert_eq!(identify("Telegram.exe", "", &unread), Some(Known::Telegram { unread: true }));
        assert_eq!(identify("SecurityHealthSystray.exe", "", &warn), Some(Known::Security(ShieldMark::Exclamation)));
        assert_eq!(identify("SecurityHealthSystray.exe", "", &plane), Some(Known::Security(ShieldMark::Check)));
        assert_eq!(identify("C:\\Windows\\explorer.exe", "Устройства Bluetooth", &plane), Some(Known::Bluetooth));
        assert_eq!(identify("explorer.exe", "Безопасное извлечение устройств и дисков", &plane), Some(Known::Eject));
        assert_eq!(identify("explorer.exe", "Safely Remove Hardware and Eject Media", &plane), Some(Known::Eject));
        assert_eq!(identify("explorer.exe", "Meet Now", &plane), None);
        assert_eq!(identify("Taskmgr.exe", "CPU: 3%", &plane), Some(Known::Cpu));
    }

    #[test]
    fn telegram_keeps_its_red_unread_dot_apart_from_the_plane() {
        let e = Extra::known(Known::Telegram { unread: true });
        let pm = e.paint([255, 255, 255]).unwrap();
        let (x, y, r) = e.red_dot.unwrap();
        let red = pm.pixel(x as u32, y as u32).unwrap().demultiply();
        assert!(red.red() > 200 && red.green() < 100);
        // Clear space around the dot.
        let rim = pm.pixel((x + r + GAP * 0.5) as u32, y as u32).unwrap();
        assert_eq!(rim.alpha(), 0);
    }
}
