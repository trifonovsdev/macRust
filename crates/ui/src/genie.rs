//! The macOS Genie minimize and restore, measured frame by frame from 60 fps
//! recordings (`out/ref/TAHOE-GENIE.md`): the window keeps its height and
//! slides down rigidly, while every screen row is squeezed horizontally into a
//! funnel that bends from the window's sides to the Dock icon along a cosine
//! S-curve over screen y. Rows are never compressed vertically, so the window
//! pours through the funnel and disappears behind the Dock bar.
//!
//! Pure and platform independent: the host captures the window once and asks
//! for one frame per display refresh. Buffers hold premultiplied pixels of 4
//! bytes in any channel order (pixels are only resampled and blended).

use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Minimize,
    Restore,
}

/// Where a window goes, in screen px.
#[derive(Clone, Copy, Debug)]
pub struct Path {
    /// The window's visible frame at rest.
    pub win: Rect,
    /// The Dock icon's visible art; the funnel ends on its centre and width.
    pub icon: Rect,
    /// Rows from here down are behind the Dock bar (the screen bottom when the Dock is hidden).
    pub clip: f32,
    pub screen_h: f32,
    pub dock_visible: bool,
    /// Display scale, for the shadow.
    pub scale: f32,
}

// Minimize timeline in seconds from the first bend. macOS 26 (YG, 30 fps):
// the sides bend in 0.231 s and the top edge slides from 0.136 s for 0.396 s
// (smoothstep fits, rms 0.012 and 13 px). macOS 13 at 60 fps (MZ) has the same
// shape a little faster (0.209 / 0.067 / 0.422 s). Once bent, the funnel's
// neck widens from 0.39 to 0.95 of the icon width over 0.197 s (MZ).
const BEND_S: f32 = 0.231;
const SLIDE_AT_S: f32 = 0.136;
const SLIDE_S: f32 = 0.396;
const NECK_AT_S: f32 = BEND_S;
const NECK_S: f32 = 0.197;
const NECK_FROM: f32 = 0.39;
const NECK_TO: f32 = 0.95;
/// Restore is not the exact reverse: the bend is still half there when the top
/// edge arrives and relaxes about 3 frames later (MZ restore frames 113-125).
const RELAX_S: f32 = 0.18;
/// The S-curve ends this far above the Dock bar (Tahoe minimize 13 px, Ventura
/// restore 30 px) or this far below a hidden Dock's screen bottom (6 px).
const Y1_MINIMIZE: f32 = 13.0;
const Y1_RESTORE: f32 = 30.0;
const Y1_HIDDEN: f32 = 6.0;
/// The top edge ends 2 px below the screen bottom.
const END_BELOW: f32 = 2.0;
/// Shadow along the warped silhouette: wallpaper darkened by 0.159 / 0.146 /
/// 0.108 at 3 / 8 / 15 px from the edge, gone once the window is ~160 px wide.
const SHADOW_A: f32 = 0.17;
const SHADOW_SIGMA: f32 = 22.0;
const SHADOW_REACH: f32 = 40.0;
const SHADOW_TOP: f32 = 0.5;

pub fn duration(dir: Dir) -> f32 {
    match dir {
        Dir::Minimize => SLIDE_AT_S + SLIDE_S,
        Dir::Restore => SLIDE_S + RELAX_S / 2.0,
    }
}

fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

fn cos_s(u: f32) -> f32 {
    0.5 - 0.5 * (PI * u.clamp(0.0, 1.0)).cos()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The shape parameters at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Bend amount, 0 (rectangle) to 1 (full funnel).
    pub k: f32,
    /// Screen y of the window's top edge.
    pub top: f32,
    /// Width of the funnel below the S-curve.
    pub neck: f32,
    /// Screen y where the S-curve reaches the neck.
    pub y1: f32,
}

pub fn frame(p: &Path, dir: Dir, t: f32) -> Frame {
    let (k, slide, neck_u) = match dir {
        Dir::Minimize => (smooth(t / BEND_S), smooth((t - SLIDE_AT_S) / SLIDE_S), (t - NECK_AT_S) / NECK_S),
        Dir::Restore => (
            1.0 - smooth((t - (SLIDE_S - RELAX_S / 2.0)) / RELAX_S),
            smooth((SLIDE_S - t) / SLIDE_S),
            // The neck replays the minimize backwards, slide aligned to slide.
            (duration(Dir::Minimize) - t - NECK_AT_S) / NECK_S,
        ),
    };
    let y1 = if p.dock_visible {
        p.clip - if dir == Dir::Minimize { Y1_MINIMIZE } else { Y1_RESTORE }
    } else {
        p.screen_h + Y1_HIDDEN
    };
    Frame {
        k,
        top: lerp(p.win.y, p.screen_h + END_BELOW, slide),
        neck: p.icon.w * lerp(NECK_FROM, NECK_TO, neck_u.clamp(0.0, 1.0)),
        y1,
    }
}

/// Left and right edge of the funnel at screen y.
pub fn span(p: &Path, f: &Frame, y: f32) -> (f32, f32) {
    let g = cos_s((y - p.win.y) / (f.y1 - p.win.y).max(1.0)) * f.k;
    let cx = p.icon.x + p.icon.w / 2.0;
    (lerp(p.win.x, cx - f.neck / 2.0, g), lerp(p.win.x + p.win.w, cx + f.neck / 2.0, g))
}

/// Whether any of the window is still above the Dock bar.
pub fn visible(p: &Path, f: &Frame) -> bool {
    f.top < p.clip
}

/// A captured window with horizontal mip levels (each half the width of the
/// one before), so a row squeezed many times over is sampled from a
/// prefiltered copy at constant cost per output pixel.
pub struct Snapshot {
    pub w: usize,
    pub h: usize,
    levels: Vec<(usize, Vec<u8>)>,
    /// Rows without transparent pixels, copied without blending.
    opaque: Vec<bool>,
}

impl Snapshot {
    /// `px`: `w * h` premultiplied 4-byte pixels. Only the full resolution
    /// exists until `build_levels`, so the first, unsqueezed frame can go on
    /// screen without waiting for the rest.
    pub fn new(px: Vec<u8>, w: usize, h: usize) -> Snapshot {
        assert_eq!(px.len(), w * h * 4, "snapshot size");
        let opaque = px.chunks_exact(w.max(1) * 4).map(|r| r.chunks_exact(4).all(|p| p[3] == 255)).collect();
        Snapshot { w, h, levels: vec![(w, px)], opaque }
    }

    pub fn has_levels(&self) -> bool {
        self.levels.len() > 1 || self.w < 16
    }

    /// Builds the half-width levels (rows in bands on up to four threads).
    /// Until then a squeezed row samples the full resolution and aliases.
    pub fn build_levels(&mut self) {
        let h = self.h;
        while let Some((pw, prev)) = self.levels.last()
            && *pw >= 16
        {
            let (pw, nw) = (*pw, pw.div_ceil(2));
            let mut next = vec![0u8; nw * h * 4];
            let band = h.div_ceil(workers()).max(1);
            std::thread::scope(|sc| {
                for (i, chunk) in next.chunks_mut(band * nw * 4).enumerate() {
                    let prev = &prev[i * band * pw * 4..];
                    sc.spawn(move || {
                        for (y, d) in chunk.chunks_exact_mut(nw * 4).enumerate() {
                            let s = &prev[y * pw * 4..(y + 1) * pw * 4];
                            let px =
                                |i: usize| u32::from_le_bytes([s[i * 4], s[i * 4 + 1], s[i * 4 + 2], s[i * 4 + 3]]);
                            for (x, o) in d.chunks_exact_mut(4).enumerate() {
                                let (a, b) = (px(2 * x), px((2 * x + 1).min(pw - 1)));
                                // Per-byte average without carries between channels.
                                let m = (a & b) + (((a ^ b) & 0xfefe_fefe) >> 1);
                                o.copy_from_slice(&m.to_le_bytes());
                            }
                        }
                    });
                }
            });
            self.levels.push((nw, next));
        }
    }

    /// The full-resolution pixels, for keeping the snapshot without its levels.
    pub fn into_pixels(mut self) -> Vec<u8> {
        self.levels.swap_remove(0).1
    }

    pub fn bytes(&self) -> usize {
        self.levels.iter().map(|l| l.1.len()).sum()
    }
}

pub struct Dst<'a> {
    pub px: &'a mut [u8],
    pub w: usize,
    pub h: usize,
    /// Screen position of the buffer's top-left pixel.
    pub x: i32,
    pub y: i32,
}

/// Pixels touched by a frame, in buffer coordinates, half-open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Bounds {
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn union(self, o: Bounds) -> Bounds {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Bounds { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    fn add(&mut self, x0: i32, x1: i32, y: i32) {
        if x1 > x0 {
            *self = self.union(Bounds { x0, y0: y, x1, y1: y + 1 });
        }
    }
}

/// Makes `b` transparent again.
pub fn clear(dst: &mut Dst, b: Bounds) {
    let (x0, x1) = (b.x0.clamp(0, dst.w as i32) as usize, b.x1.clamp(0, dst.w as i32) as usize);
    for y in b.y0.clamp(0, dst.h as i32) as usize..b.y1.clamp(0, dst.h as i32) as usize {
        dst.px[(y * dst.w + x0) * 4..(y * dst.w + x1) * 4].fill(0);
    }
}

/// Shadow alpha (0..255) by distance from the edge in quarter px.
struct ShadowLut(Vec<u8>);

impl ShadowLut {
    fn new(scale: f32, fade: f32) -> ShadowLut {
        let n = (SHADOW_REACH * scale * 4.0).ceil() as usize + 1;
        ShadowLut(
            (0..n)
                .map(|i| {
                    let u = i as f32 / 4.0 / (SHADOW_SIGMA * scale);
                    (SHADOW_A * (-u * u).exp() * fade * 255.0 + 0.5) as u8
                })
                .collect(),
        )
    }

    fn at(&self, d: f32) -> u8 {
        self.0.get((d.max(0.0) * 4.0) as usize).copied().unwrap_or(0)
    }
}

/// Darkens a premultiplied pixel by `a`/255 of black.
#[inline]
fn darken(px: &mut [u8], a: u8) {
    if a == 0 {
        return;
    }
    let keep = 255 - a as u32;
    for c in &mut px[..3] {
        *c = ((*c as u32 * keep + 127) / 255) as u8;
    }
    px[3] = (a as u32 + (px[3] as u32 * keep + 127) / 255) as u8;
}

/// Draws the window at frame `f` over what `dst` holds, with its shadow, and
/// returns the pixels it touched. Rows at and below `p.clip` are left out.
/// Large windows are drawn in row bands on up to four threads.
pub fn draw(src: &Snapshot, p: &Path, f: &Frame, dst: &mut Dst) -> Bounds {
    if src.w == 0 || src.h == 0 || p.win.w < 1.0 || p.win.h < 1.0 {
        return Bounds::default();
    }
    let s = p.scale.max(0.5);
    let reach = SHADOW_REACH * s;
    let bottom = f.top + p.win.h;
    // The shadow fades out as the window narrows to a sliver.
    let (tl, tr) = span(p, f, f.top);
    let lut = ShadowLut::new(s, ((tr - tl - 100.0 * s) / (150.0 * s)).clamp(0.0, 1.0));
    let pass = Pass {
        src,
        p,
        f,
        shadow: lut.0.iter().any(|&a| a > 0),
        lut,
        reach,
        bottom,
        dst_x: dst.x,
        dst_y: dst.y,
        dst_w: dst.w,
    };
    let y0 = ((f.top - reach * SHADOW_TOP).floor() as i32 - dst.y).max(0);
    let y1 = ((bottom + reach).min(p.clip).ceil() as i32 - dst.y).min(dst.h as i32);
    if y1 <= y0 {
        return Bounds::default();
    }
    let rows = &mut dst.px[y0 as usize * dst.w * 4..y1 as usize * dst.w * 4];
    let area = (y1 - y0) as f32 * (tr - tl).max(p.win.w * f.k.max(0.3));
    let threads = if area > 250_000.0 { workers() } else { 1 };
    if threads <= 1 {
        return pass.rows(rows, y0);
    }
    let band = ((y1 - y0) as usize).div_ceil(threads);
    std::thread::scope(|sc| {
        let pass = &pass;
        let jobs: Vec<_> = rows
            .chunks_mut(band * dst.w * 4)
            .enumerate()
            .map(|(i, chunk)| sc.spawn(move || pass.rows(chunk, y0 + (i * band) as i32)))
            .collect();
        jobs.into_iter().fold(Bounds::default(), |b, j| b.union(j.join().unwrap_or_default()))
    })
}

fn workers() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(4)))
}

/// One frame's constants, shared by the row bands.
struct Pass<'a> {
    src: &'a Snapshot,
    p: &'a Path,
    f: &'a Frame,
    lut: ShadowLut,
    shadow: bool,
    reach: f32,
    bottom: f32,
    dst_x: i32,
    dst_y: i32,
    dst_w: usize,
}

impl Pass<'_> {
    /// Draws the rows in `buf`, which starts at buffer row `first`.
    fn rows(&self, buf: &mut [u8], first: i32) -> Bounds {
        let (p, f, src) = (self.p, self.f, self.src);
        let mut out = Bounds::default();
        let ox = self.dst_x as f32;
        let dw = self.dst_w as i32;
        let src_step_y = src.h as f32 / p.win.h;
        for (k, row) in buf.chunks_exact_mut(self.dst_w * 4).enumerate() {
            let yy = first + k as i32;
            let yc = (yy + self.dst_y) as f32 + 0.5;
            let (xl, xr) = span(p, f, yc);
            let (xl, xr) = (xl - ox, xr - ox);
            if yc < f.top || yc >= self.bottom {
                // Above or below the window: its shadow only.
                let a = self.lut.at(if yc < f.top { (f.top - yc) / SHADOW_TOP } else { yc - self.bottom });
                if a > 0 {
                    let (x0, x1) = ((xl.floor() as i32).max(0), (xr.ceil() as i32).min(dw));
                    for x in x0..x1 {
                        darken(&mut row[x as usize * 4..x as usize * 4 + 4], a);
                    }
                    out.add(x0, x1, yy);
                }
                continue;
            }
            if self.shadow {
                let (a0, a1) = (((xl - self.reach).floor() as i32).max(0), (xl.floor() as i32).min(dw));
                for x in a0..a1 {
                    darken(&mut row[x as usize * 4..x as usize * 4 + 4], self.lut.at(xl - (x as f32 + 0.5)));
                }
                out.add(a0, a1, yy);
                let (b0, b1) = ((xr.ceil() as i32).max(0), ((xr + self.reach).ceil() as i32).min(dw));
                for x in b0..b1 {
                    darken(&mut row[x as usize * 4..x as usize * 4 + 4], self.lut.at(x as f32 + 0.5 - xr));
                }
                out.add(b0, b1, yy);
            }
            let sr = (((yc - f.top) * src_step_y) as usize).min(src.h - 1);
            let (x0, x1) = ((xl.floor() as i32).max(0), (xr.ceil() as i32).min(dw));
            out.add(x0, x1, yy);
            resample_row(src, sr, xl, xr, row, x0, x1);
        }
        out
    }
}

/// Stretches source row `sr` onto [l, r) of a destination row: linear taps
/// on the mip level that is at most 2x squeezed, anti-aliased edges.
fn resample_row(src: &Snapshot, sr: usize, l: f32, r: f32, row: &mut [u8], x0: i32, x1: i32) {
    let width = r - l;
    if width <= 0.0 || x1 <= x0 {
        return;
    }
    let step = src.w as f32 / width;
    let level = (step.max(1.0).log2().floor() as usize).min(src.levels.len() - 1);
    let (n, ref px) = src.levels[level];
    let line = &px[sr * n * 4..(sr + 1) * n * 4];
    let opaque = src.opaque[sr];
    // Unscaled on whole pixels: a straight copy (the first and last frames).
    if level == 0 && (step - 1.0).abs() < 1e-4 && l.fract() == 0.0 && l >= 0.0 {
        let off = l as usize;
        let (a, b) = (x0 as usize, (x1 as usize).min(off + n));
        if a >= b {
            return;
        }
        let (s, d) = (&line[(a - off) * 4..(b - off) * 4], &mut row[a * 4..b * 4]);
        if opaque {
            d.copy_from_slice(s);
        } else {
            for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                over(d, s, 256);
            }
        }
        return;
    }
    // Source position in level px, 16.16 fixed point, of pixel centres.
    let lstep = step * n as f32 / src.w as f32;
    let dstep = (lstep * 65536.0) as i64;
    let last = (n as i64 - 1) << 16;
    let start = |x: i32| (((x as f32 + 0.5 - l) * lstep - 0.5) * 65536.0) as i64;
    let tap = |u: i64| -> [u8; 4] {
        let uc = u.clamp(0, last);
        let i = (uc >> 16) as usize;
        let fr = ((uc >> 8) & 0xff) as u32;
        let j = (i + 1).min(n - 1);
        let (a, b) = (&line[i * 4..i * 4 + 4], &line[j * 4..j * 4 + 4]);
        std::array::from_fn(|k| ((a[k] as u32 * (256 - fr) + b[k] as u32 * fr + 128) >> 8) as u8)
    };
    // Partial edge pixels blend by coverage; whole ones in between are copied
    // (or blended when the row has transparent corners).
    let (i0, i1) = ((l.ceil() as i32).max(x0), (r.floor() as i32).min(x1));
    for x in (x0..i0.min(x1)).chain(i1.max(i0).max(x0)..x1) {
        let xf = x as f32;
        let cov = (xf + 1.0).min(r) - xf.max(l);
        if cov > 0.0 {
            over(&mut row[x as usize * 4..x as usize * 4 + 4], &tap(start(x)), (cov.min(1.0) * 256.0) as u32);
        }
    }
    if i1 <= i0 {
        return;
    }
    let u0 = start(i0);
    let count = (i1 - i0) as i64;
    // Pixels whose two taps are both inside the row take the fast path.
    let (f0, f1) = if opaque && dstep > 0 {
        let f0 = if u0 >= 0 { 0 } else { (-u0 + dstep - 1) / dstep };
        let f1 = if u0 >= last { 0 } else { ((last - 1 - u0) / dstep + 1).min(count) };
        (f0.min(count), f1.max(f0.min(count)))
    } else {
        (count, count)
    };
    let inner = &mut row[i0 as usize * 4..i1 as usize * 4];
    let (head, rest) = inner.split_at_mut(f0 as usize * 4);
    let (fast, tail) = rest.split_at_mut((f1 - f0) as usize * 4);
    let slow = |d: &mut [u8], u: i64| {
        let c = tap(u);
        if c[3] == 255 {
            d.copy_from_slice(&c);
        } else {
            over(d, &c, 256);
        }
    };
    for (k, d) in head.chunks_exact_mut(4).enumerate() {
        slow(d, u0 + k as i64 * dstep);
    }
    for (k, d) in tail.chunks_exact_mut(4).enumerate() {
        slow(d, u0 + (f1 + k as i64) * dstep);
    }
    // Two taps on all four channels at once: two channels per u32, in
    // 16-bit lanes (255 * 256 still fits a lane).
    let mut u = u0 + f0 * dstep;
    for d in fast.chunks_exact_mut(4) {
        let i = (u >> 16) as usize * 4;
        let fr = ((u >> 8) & 0xff) as u32;
        let ab = &line[i..i + 8];
        let a = u32::from_le_bytes([ab[0], ab[1], ab[2], ab[3]]);
        let b = u32::from_le_bytes([ab[4], ab[5], ab[6], ab[7]]);
        let rb = (((a & 0x00ff_00ff) * (256 - fr) + (b & 0x00ff_00ff) * fr + 0x0080_0080) >> 8) & 0x00ff_00ff;
        let ga = (((a >> 8) & 0x00ff_00ff) * (256 - fr) + ((b >> 8) & 0x00ff_00ff) * fr + 0x0080_0080) & 0xff00_ff00;
        d.copy_from_slice(&(rb | ga).to_le_bytes());
        u += dstep;
    }
}

/// Source-over of a premultiplied pixel with coverage `cov`/256.
#[inline]
fn over(d: &mut [u8], c: &[u8], cov: u32) {
    let ca = (c[3] as u32 * cov) >> 8;
    let keep = 255 - ca;
    for k in 0..4 {
        d[k] = (((c[k] as u32 * cov) >> 8) + (d[k] as u32 * keep + 127) / 255).min(255) as u8;
    }
}

/// Cuts a snapshot's corners to radius `r` (premultiplied, anti-aliased):
/// the capture of a rounded window holds the desktop in its corners.
pub fn round_corners(px: &mut [u8], w: usize, h: usize, r: f32) {
    let ri = r.ceil() as usize;
    if r <= 0.0 || ri * 2 > w || ri * 2 > h {
        return;
    }
    for y in 0..ri {
        for x in 0..ri {
            let (dx, dy) = (r - (x as f32 + 0.5), r - (y as f32 + 0.5));
            let a = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            if a >= 1.0 {
                continue;
            }
            for (cx, cy) in [(x, y), (w - 1 - x, y), (x, h - 1 - y), (w - 1 - x, h - 1 - y)] {
                let i = (cy * w + cx) * 4;
                for c in &mut px[i..i + 4] {
                    *c = (*c as f32 * a + 0.5) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> Path {
        Path {
            win: Rect { x: 579.0, y: 178.0, w: 705.0, h: 683.0 },
            icon: Rect { x: 1397.0, y: 1012.0, w: 58.0, h: 58.0 },
            clip: 1008.0,
            screen_h: 1080.0,
            dock_visible: true,
            scale: 1.0,
        }
    }

    #[test]
    fn minimize_starts_as_the_window_and_ends_behind_the_dock() {
        let p = path();
        let f = frame(&p, Dir::Minimize, 0.0);
        assert_eq!(f.k, 0.0);
        assert_eq!(f.top, p.win.y);
        for y in [200.0, 500.0, 850.0] {
            assert_eq!(span(&p, &f, y), (p.win.x, p.win.x + p.win.w));
        }
        let end = frame(&p, Dir::Minimize, duration(Dir::Minimize));
        assert!(!visible(&p, &end));
        assert!((end.neck - 0.95 * p.icon.w).abs() < 1e-3);
        assert!((duration(Dir::Minimize) - 0.532).abs() < 1e-3);
    }

    #[test]
    fn funnel_follows_the_measured_cosine_curve() {
        // MZ frames 28-41: G at y 454/574/694/814/934/1054 = .31/.48/.67/.83/.94/1.00
        // for wy 83.5 and Y1 1086.4 (hidden Dock).
        let p = Path {
            win: Rect { x: 892.0, y: 83.5, w: 536.0, h: 713.0 },
            icon: Rect { x: 1320.3, y: 994.0, w: 52.0, h: 66.0 },
            clip: 1080.0,
            screen_h: 1080.0,
            dock_visible: false,
            scale: 1.0,
        };
        let f = frame(&p, Dir::Minimize, 0.3);
        assert_eq!(f.k, 1.0);
        assert!((f.y1 - 1086.0).abs() < 1e-3);
        let cx = 1346.3;
        for (y, g) in [(454.0, 0.31), (574.0, 0.48), (694.0, 0.67), (814.0, 0.83), (934.0, 0.94), (1054.0, 1.0)] {
            let (l, _) = span(&p, &f, y);
            let got = (l - p.win.x) / (cx - f.neck / 2.0 - p.win.x);
            assert!((got - g).abs() < 0.08, "y {y}: G {got} vs {g}");
        }
    }

    #[test]
    fn restore_ends_as_the_window_and_relaxes_after_the_slide() {
        let p = path();
        let at_arrival = frame(&p, Dir::Restore, SLIDE_S);
        assert!((at_arrival.top - p.win.y).abs() < 1e-3);
        assert!((at_arrival.k - 0.5).abs() < 1e-3);
        let start = frame(&p, Dir::Restore, 0.0);
        assert!(!visible(&p, &start));
        assert_eq!(start.k, 1.0);
        let end = frame(&p, Dir::Restore, duration(Dir::Restore));
        assert_eq!(end.k, 0.0);
        assert_eq!(span(&p, &end, 500.0), (p.win.x, p.win.x + p.win.w));
    }

    #[test]
    fn edges_move_monotonically_toward_the_icon() {
        let p = path();
        // The icon is right of the window here, so both edges move right.
        let mut prev = (p.win.x, p.win.x + p.win.w);
        for i in 0..=30 {
            let f = frame(&p, Dir::Minimize, i as f32 / 30.0 * BEND_S);
            let s = span(&p, &f, 800.0);
            assert!(s.0 >= prev.0 - 1e-3 && s.1 >= prev.1 - 1e-3 && s.0 < s.1);
            prev = s;
        }
        assert!(prev.1 > p.win.x + p.win.w);
    }

    fn snapshot(w: usize, h: usize) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90, 255]).collect()
    }

    #[test]
    fn first_frame_reproduces_the_snapshot() {
        let p = path();
        let (w, h) = (p.win.w as usize, p.win.h as usize);
        let px = snapshot(w, h);
        let mut buf = vec![0u8; 1920 * 1080 * 4];
        let mut dst = Dst { px: &mut buf, w: 1920, h: 1080, x: 0, y: 0 };
        let snap = Snapshot::new(px.clone(), w, h);
        let b = draw(&snap, &p, &frame(&p, Dir::Minimize, 0.0), &mut dst);
        assert!(b.x0 <= 579 && b.x1 >= 1284 && b.y0 <= 178 && b.y1 >= 861);
        for (x, y) in [(0, 0), (w - 1, 0), (300, 400), (w - 1, h - 1)] {
            let d = ((178 + y) * 1920 + 579 + x) * 4;
            let s = (y * w + x) * 4;
            assert_eq!(&buf[d..d + 4], &px[s..s + 4], "pixel {x},{y}");
        }
    }

    #[test]
    fn nothing_is_drawn_behind_the_dock_and_clear_empties_it() {
        let p = path();
        let (w, h) = (p.win.w as usize, p.win.h as usize);
        let px = snapshot(w, h);
        let mut buf = vec![0u8; 1920 * 1080 * 4];
        let mut dst = Dst { px: &mut buf, w: 1920, h: 1080, x: 0, y: 0 };
        let f = frame(&p, Dir::Minimize, 0.33);
        let b = draw(&Snapshot::new(px, w, h), &p, &f, &mut dst);
        assert!(b.y1 <= 1008, "drew below the Dock top: {b:?}");
        // The neck row lands on the icon centre.
        let (l, r) = span(&p, &f, 1000.0);
        assert!(((l + r) / 2.0 - 1426.0).abs() < 0.5);
        clear(&mut dst, b);
        assert!(buf.iter().all(|&v| v == 0));
    }

    #[test]
    fn levels_halve_the_width_and_average_pairs() {
        let px: Vec<u8> =
            (0..40 * 3).flat_map(|i| if i % 2 == 0 { [200, 100, 0, 255] } else { [100, 50, 0, 255] }).collect();
        let mut s = Snapshot::new(px, 40, 3);
        assert!(!s.has_levels());
        s.build_levels();
        assert!(s.has_levels());
        assert_eq!(s.levels.iter().map(|l| l.0).collect::<Vec<_>>(), vec![40, 20, 10]);
        assert_eq!(&s.levels[1].1[..4], &[150, 75, 0, 255]);
        assert_eq!(s.levels[2].1.len(), 10 * 3 * 4);
    }

    #[test]
    fn corners_are_cut_and_edges_kept() {
        let mut px = vec![255u8; 40 * 30 * 4];
        round_corners(&mut px, 40, 30, 8.0);
        assert_eq!(px[3], 0);
        assert_eq!(px[(29 * 40 + 39) * 4 + 3], 0);
        assert_eq!(px[(15 * 40) * 4 + 3], 255);
        assert_eq!(px[20 * 4 + 3], 255);
    }

    #[test]
    fn squeezed_rows_keep_the_average_colour() {
        let line: Vec<u8> =
            (0..1000).flat_map(|i| if i % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let mut snap = Snapshot::new(line, 1000, 1);
        snap.build_levels();
        let mut row = vec![0u8; 100 * 4];
        resample_row(&snap, 0, 10.0, 60.0, &mut row, 10, 60);
        for x in 10..60 {
            assert!((row[x * 4] as i32 - 128).abs() <= 2, "x {x}: {}", row[x * 4]);
            assert_eq!(row[x * 4 + 3], 255);
        }
        assert_eq!(row[9 * 4 + 3], 0);
    }
}
