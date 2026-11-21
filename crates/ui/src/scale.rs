//! The macOS "Scale effect" minimize and restore, measured from 60 and 30 fps
//! recordings (`out/ref/TAHOE-SCALE.md`): the window shrinks uniformly, aspect
//! kept, along a straight line into the Dock, opaque all the way, in about
//! 0.2 s; the restore is the same motion reversed. Every edge moves with one
//! eased progress `e`, so the rect is a plain interpolation between the window
//! and its end rect inside the icon.
//!
//! Pure and platform independent: the Windows host moves a live DWM thumbnail
//! of the window along `frame`.

pub use crate::genie::Dir;

/// Minimize and restore length (TAHOE-SCALE.md: 0.18-0.23 s measured on
/// Ventura/Sequoia; no Tahoe recording of Scale was found).
pub const DURATION_S: f32 = 0.20;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }

    fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }

    /// Edges rounded one by one (left, top, right, bottom), so a rect that
    /// moves by a fraction of a pixel never changes size by two.
    pub fn edges(&self) -> (i32, i32, i32, i32) {
        (self.x.round() as i32, self.y.round() as i32, (self.x + self.w).round() as i32, (self.y + self.h).round() as i32)
    }
}

/// Where a window goes, in screen px.
#[derive(Clone, Copy, Debug)]
pub struct Path {
    /// The window's visible frame at rest.
    pub win: Rect,
    /// The Dock icon it goes into.
    pub icon: Rect,
}

/// smoothstep: the recordings cannot tell it from linear or cosine, and rule
/// out ease-in or ease-out alone (TAHOE-SCALE.md 2.1).
pub fn ease(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// How far into the icon the window is `t` s into the animation: 0 at the
/// window, 1 in the icon.
pub fn progress(dir: Dir, t: f32) -> f32 {
    let e = ease(t / DURATION_S);
    match dir {
        Dir::Minimize => e,
        Dir::Restore => 1.0 - e,
    }
}

pub fn finished(t: f32) -> bool {
    t >= DURATION_S
}

/// The window scaled uniformly to fit inside the icon, centred on it.
pub fn end_rect(p: &Path) -> Rect {
    let (w, i) = (p.win, p.icon);
    let s = if w.w > 0.0 && w.h > 0.0 { (i.w / w.w).min(i.h / w.h).max(0.0) } else { 0.0 };
    let (ew, eh) = (w.w * s, w.h * s);
    Rect { x: i.cx() - ew / 2.0, y: i.cy() - eh / 2.0, w: ew, h: eh }
}

/// The window's rect at progress `e`.
pub fn frame(p: &Path, e: f32) -> Rect {
    let (a, b) = (p.win, end_rect(p));
    // Exact at both ends.
    let l = |x: f32, y: f32| x * (1.0 - e) + y * e;
    Rect { x: l(a.x, b.x), y: l(a.y, b.y), w: l(a.w, b.w), h: l(a.h, b.h) }
}

/// The part of a window's thumbnail that is its visible frame, as insets
/// from the thumbnail's edges (left, top, right, bottom).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Crop {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
    /// From the window's measured resize borders; else derived from sizes.
    pub measured: bool,
}

/// Crops a thumbnail of `src` px to a visible frame of `frame` px.
/// `borders` (left, top, right, bottom) are the window's invisible resize
/// borders when known; they are used if they add up to the source. Otherwise
/// the difference is split: evenly across, and evenly down too when it is
/// the same both ways (a maximized window), else all of it at the bottom
/// (the top border of a normal frame is 0 on Windows 10 and 11).
pub fn crop(src: (i32, i32), frame: (i32, i32), borders: Option<(i32, i32, i32, i32)>) -> Crop {
    let (dx, dy) = (src.0 - frame.0, src.1 - frame.1);
    if let Some((l, t, r, b)) = borders
        && l + r == dx
        && t + b == dy
        && l >= 0
        && t >= 0
        && r >= 0
        && b >= 0
    {
        return Crop { l, t, r, b, measured: true };
    }
    let (dx, dy) = (dx.max(0), dy.max(0));
    let l = dx / 2;
    let t = if dx == dy { dy / 2 } else { 0 };
    Crop { l, t, r: dx - l, b: dy - t, measured: false }
}

/// The same frame's insets in a source that changed size from `from` to
/// `to` (DWM keeps a minimized window's picture at another size, e.g. with
/// its shadow). The frame keeps its size, so the thumbnail's scale does not
/// jump; each axis's change is split between its two insets in proportion
/// to them (evenly when both are 0), which keeps a symmetric layout
/// symmetric and a top-anchored one (top inset 0) anchored. Insets never go
/// below 0: a source too small for the frame crops nothing on that side.
pub fn remap(c: Crop, from: (i32, i32), to: (i32, i32)) -> Crop {
    fn axis(a: i32, b: i32, d: i32) -> (i32, i32) {
        let da = if a + b > 0 { (d as f64 * a as f64 / (a + b) as f64).round() as i32 } else { d / 2 };
        let (mut a, mut b) = (a + da, b + d - da);
        if a < 0 {
            b += a;
            a = 0;
        }
        if b < 0 {
            a += b;
            b = 0;
        }
        (a.max(0), b.max(0))
    }
    let (l, r) = axis(c.l, c.r, to.0 - from.0);
    let (t, b) = axis(c.t, c.b, to.1 - from.1);
    Crop { l, t, r, b, measured: false }
}

// ---------------------------------------------------------------------------
// Frame pacing on the display's vblank grid. Times are performance-counter
// ticks (DWM reports vblanks in them); the host converts to seconds.

/// The first vblank strictly after `now`, on the grid through `vblank`
/// (a past or a future vblank) with `period` ticks between vblanks.
pub fn vblank_after(now: i64, vblank: i64, period: i64) -> i64 {
    if period <= 0 {
        return now;
    }
    vblank + ((now - vblank).div_euclid(period) + 1) * period
}

/// Frames a run of `DURATION_S` shows at `period_s` per refresh, the first
/// (still) frame included.
pub fn expected_frames(period_s: f32) -> u32 {
    if period_s <= 0.0 { 0 } else { (DURATION_S / period_s).round() as u32 }
}

/// One run's clock: the time of each frame is the vblank it is shown at,
/// not when it was computed, so a late wake-up skips a vblank (counted)
/// instead of stretching or bunching the motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pacing {
    /// The vblank of the first (still) frame.
    pub start: i64,
    last: i64,
    /// Frames placed, the first included (the caller counts them).
    pub shown: u32,
    /// Vblanks inside the run that got no new frame.
    pub missed: u32,
}

impl Pacing {
    pub fn new(start: i64) -> Pacing {
        Pacing { start, last: start, shown: 1, missed: 0 }
    }

    /// Ticks since `start` for a frame shown at vblank `target`; None when
    /// that vblank already has its frame.
    pub fn frame(&mut self, target: i64, period: i64) -> Option<i64> {
        if period <= 0 || target <= self.last {
            return None;
        }
        let n = ((target - self.last) as f64 / period as f64).round() as u32;
        if n == 0 {
            return None;
        }
        self.missed += n - 1;
        self.last = target;
        Some(target - self.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> Path {
        Path { win: Rect { x: 300.0, y: 120.0, w: 900.0, h: 600.0 }, icon: Rect { x: 1400.0, y: 1000.0, w: 52.0, h: 52.0 } }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn ease_ends_and_monotonic() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
        assert!(near(ease(0.5), 0.5));
    }

    #[test]
    fn starts_at_window_ends_in_icon() {
        let p = path();
        assert_eq!(frame(&p, progress(Dir::Minimize, 0.0)), p.win);
        let end = end_rect(&p);
        assert_eq!(frame(&p, progress(Dir::Minimize, DURATION_S)), end);
        assert_eq!(frame(&p, progress(Dir::Restore, 0.0)), end);
        assert_eq!(frame(&p, progress(Dir::Restore, DURATION_S)), p.win);
        assert!(!finished(DURATION_S - 0.001) && finished(DURATION_S));
    }

    #[test]
    fn end_fits_icon_centred_with_window_aspect() {
        let p = path();
        let e = end_rect(&p);
        assert!(e.w <= p.icon.w + 1e-3 && e.h <= p.icon.h + 1e-3);
        assert!(near(e.w, 52.0));
        assert!(near(e.cx(), p.icon.cx()) && near(e.cy(), p.icon.cy()));
        assert!(near(e.w / e.h, p.win.w / p.win.h));
        // A tall window fits by height.
        let tall = Path { win: Rect { x: 0.0, y: 0.0, w: 300.0, h: 900.0 }, ..p };
        assert!(near(end_rect(&tall).h, 52.0));
    }

    #[test]
    fn uniform_scale_on_a_straight_line() {
        let p = path();
        let (a, b) = (p.win, end_rect(&p));
        for i in 0..=20 {
            let e = i as f32 / 20.0;
            let f = frame(&p, e);
            assert!((f.w / f.h - a.w / a.h).abs() < 1e-3, "aspect at {e}");
            // The centre stays on the segment between the two centres.
            let cross = (f.cx() - a.cx()) * (b.cy() - a.cy()) - (f.cy() - a.cy()) * (b.cx() - a.cx());
            assert!(cross.abs() < 1.0, "off the line at {e}: {cross}");
        }
    }

    #[test]
    fn restore_mirrors_minimize() {
        let p = path();
        for i in 0..=10 {
            let t = DURATION_S * i as f32 / 10.0;
            let m = frame(&p, progress(Dir::Minimize, t));
            let r = frame(&p, progress(Dir::Restore, DURATION_S - t));
            assert!(near(m.x, r.x) && near(m.y, r.y) && near(m.w, r.w) && near(m.h, r.h));
        }
    }

    #[test]
    fn degenerate_window_does_not_panic() {
        let p = Path { win: Rect { x: 10.0, y: 10.0, w: 0.0, h: 0.0 }, ..path() };
        let e = end_rect(&p);
        assert_eq!((e.w, e.h), (0.0, 0.0));
    }

    #[test]
    fn edges_round_each_side() {
        let r = Rect { x: 10.4, y: 20.6, w: 100.2, h: 50.0 };
        assert_eq!(r.edges(), (10, 21, 111, 71));
    }

    #[test]
    fn crop_uses_borders_that_add_up() {
        let c = crop((914, 607), (900, 600), Some((7, 0, 7, 7)));
        assert_eq!(c, Crop { l: 7, t: 0, r: 7, b: 7, measured: true });
    }

    #[test]
    fn crop_derives_when_borders_do_not_fit() {
        // Normal frame: all the extra height at the bottom.
        let c = crop((914, 607), (900, 600), Some((8, 8, 8, 8)));
        assert_eq!(c, Crop { l: 7, t: 0, r: 7, b: 7, measured: false });
        // Maximized: the same on all four sides.
        let c = crop((1936, 1056), (1920, 1040), None);
        assert_eq!(c, Crop { l: 8, t: 8, r: 8, b: 8, measured: false });
        // A source smaller than the frame crops nothing.
        let c = crop((160, 28), (900, 600), None);
        assert_eq!((c.l, c.t, c.r, c.b), (0, 0, 0, 0));
    }
    #[test]
    fn remap_keeps_the_frame_size() {
        // Shown: measured 7,0,7,7 in a 914x607 window; kept 40 px larger each way.
        let c = Crop { l: 7, t: 0, r: 7, b: 7, measured: true };
        let m = remap(c, (914, 607), (954, 647));
        assert_eq!((m.l, m.t, m.r, m.b), (27, 0, 27, 47));
        assert_eq!((954 - m.l - m.r, 647 - m.t - m.b), (900, 600));
        // Symmetric (maximized) stays symmetric.
        let c = Crop { l: 8, t: 8, r: 8, b: 8, measured: true };
        let m = remap(c, (1936, 1056), (1960, 1080));
        assert_eq!((m.l, m.t, m.r, m.b), (20, 20, 20, 20));
        // No insets: the change is split evenly.
        let m = remap(Crop::default(), (900, 600), (911, 620));
        assert_eq!((m.l, m.t, m.r, m.b), (5, 10, 6, 10));
        // And back: the identity.
        assert_eq!(remap(remap(c, (1936, 1056), (1960, 1080)), (1960, 1080), (1936, 1056)), Crop { measured: false, ..c });
    }

    #[test]
    fn remap_never_goes_negative() {
        let c = Crop { l: 7, t: 0, r: 7, b: 7, measured: true };
        let m = remap(c, (914, 607), (160, 28));
        assert_eq!((m.l, m.t, m.r, m.b), (0, 0, 0, 0));
        // A small shrink is taken from the insets, the frame keeps its size.
        let m = remap(c, (914, 607), (910, 605));
        assert_eq!((910 - m.l - m.r, 605 - m.t - m.b), (900, 600));
        assert!(m.l >= 0 && m.t >= 0 && m.r >= 0 && m.b >= 0);
    }

    #[test]
    fn vblank_after_is_strictly_after_on_the_grid() {
        let p = 1000;
        assert_eq!(vblank_after(5_500, 5_000, p), 6_000);
        // Exactly on a vblank: the next one.
        assert_eq!(vblank_after(6_000, 5_000, p), 7_000);
        // A stale reference several vblanks back.
        assert_eq!(vblank_after(9_250, 5_000, p), 10_000);
        // A reference in the future.
        assert_eq!(vblank_after(5_500, 8_000, p), 6_000);
        assert_eq!(vblank_after(5_500, 5_000, 0), 5_500);
    }

    #[test]
    fn pacing_counts_frames_and_missed_vblanks() {
        let p = 1000;
        let mut c = Pacing::new(10_000);
        assert_eq!(c.frame(11_000, p), Some(1_000));
        // A second wake-up for the same vblank gets no frame.
        assert_eq!(c.frame(11_000, p), None);
        assert_eq!(c.frame(10_500, p), None);
        // A late wake-up: two vblanks without a new frame, time stays on the grid.
        assert_eq!(c.frame(14_000, p), Some(4_000));
        assert_eq!(c.missed, 2);
        // A grid that drifted by less than half a period still counts one.
        assert_eq!(c.frame(15_040, p), Some(5_040));
        assert_eq!(c.missed, 2);
    }

    #[test]
    fn expected_frames_at_common_rates() {
        assert_eq!(expected_frames(1.0 / 60.0), 12);
        assert_eq!(expected_frames(1.0 / 144.0), 29);
        assert_eq!(expected_frames(0.0), 0);
    }
}
