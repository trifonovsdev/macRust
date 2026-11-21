//! Minimize into a Dock icon and restore out of it with the macOS Genie
//! (`macrust_ui::genie` draws it). The window is captured, Windows' own
//! minimize animation is kept off, and a click-through layered overlay just
//! below the Dock plays the warp at display refresh.
//!
//! No blink means the overlay's first frame is composed while the window is
//! still on screen, and the window is back on screen before the last frame
//! goes. Minimizes we carry out (Dock, menu) wait for DWM to compose the
//! first frame. A title-bar button press is seen by a low-level mouse hook
//! (`minbutton.rs`): the first frame is staged on the press and shown on the
//! release, before the app gets it (`arm`, `fire`). Windows also asks shell
//! hook windows for the minimize rect before it hides a window, where it
//! does so (`App::min_rect`). Every other minimize is seen only after
//! Windows has hidden the window: that path still uncovers the desktop for
//! the event's latency, and the trace says so. A restore keeps the last
//! frame up until screen capture, which leaves our windows out, shows the
//! real window under it.
//!
//! The Windows animation is one global setting (SPI_SETANIMATION,
//! iMinAnimate). It stays off while the shell runs so that a minimize from a
//! title bar plays the Genie too; a marker file puts it back after a crash
//! (the `--watch` watchdog and the next start call `release_min_animate`). A
//! user who turned the animation off in Windows gets no Genie either.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use macrust_ui::WinRect;
use macrust_ui::genie::{self, Bounds, Dir, Dst, Path, Snapshot};
use macrust_ui::tiny_skia::Pixmap;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::GetUpdateRect;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    ANIMATIONINFO, CreateWindowExW, DestroyWindow, GetWindowRect, PW_RENDERFULLCONTENT, SPI_GETANIMATION,
    SPI_SETANIMATION, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowPos, ShowWindow, SystemParametersInfoW, ULW_ALPHA,
    UPDATELAYEREDWINDOWINFO, UpdateLayeredWindowIndirect, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PCWSTR;

use super::capture::{self, Capture};
use super::util::{data_dir, log, trace, wide};
use super::winops;

/// Snapshots kept for restoring windows minimized with the Genie.
const CACHE_BYTES: usize = 128 << 20;
/// More windows than this at once (Win+M) minimize without it.
const MAX_RUNS: usize = 8;
/// A first frame shown over a window waits this long for Windows to hide it.
const WAIT_S: f32 = 0.4;
/// Restore: samples per side compared between the screen and the snapshot.
const GRID: usize = 12;
/// Mean channel difference (0..255) below which the screen shows the window.
const MATCH: f32 = 10.0;
/// Restore: the last frame stays at most this long over a window not seen.
const UNCOVER_MAX_S: f32 = 0.7;

// ---------------------------------------------------------------------------
// Windows' own minimize animation

pub(super) fn spi_min_animate() -> Option<bool> {
    let mut ai = ANIMATIONINFO { cbSize: size_of::<ANIMATIONINFO>() as u32, iMinAnimate: 0 };
    unsafe {
        SystemParametersInfoW(
            SPI_GETANIMATION,
            ai.cbSize,
            Some(&mut ai as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    Some(ai.iMinAnimate != 0)
}

/// Changes the setting for this session only: not written to the profile and
/// not broadcast, so a crash without the watchdog lasts until sign-out.
fn spi_set_min_animate(on: bool) -> bool {
    let mut ai = ANIMATIONINFO { cbSize: size_of::<ANIMATIONINFO>() as u32, iMinAnimate: on as i32 };
    unsafe {
        SystemParametersInfoW(
            SPI_SETANIMATION,
            ai.cbSize,
            Some(&mut ai as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
    }
}

fn marker() -> std::path::PathBuf {
    data_dir().join("min-animation-off.txt")
}

/// Turns Windows' minimize animation off; the marker is written first, so a
/// crash at any point leaves enough behind to turn it back on.
pub(super) fn suppress() -> bool {
    let _ = std::fs::write(marker(), "1");
    if spi_set_min_animate(false) {
        return true;
    }
    let _ = std::fs::remove_file(marker());
    false
}

/// Puts Windows' minimize animation back if we turned it off. Safe to call
/// from the watchdog, at start-up and from the panic hook.
pub fn release_min_animate() {
    if std::fs::remove_file(marker()).is_ok() {
        spi_set_min_animate(true);
    }
}

// ---------------------------------------------------------------------------
// The overlay: a full-screen, click-through layered window below the Dock

struct Overlay {
    hwnd: HWND,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
    /// Rows above this stay out of the window: a layered window covering the
    /// whole monitor makes Explorer announce a full-screen app, which hid the
    /// menu bar and the Dock for the length of every Genie.
    top: i32,
    shown: bool,
    /// What the last staged frame covered.
    drawn: Bounds,
}

impl Overlay {
    fn new(w: i32, h: i32, top: i32) -> Option<Overlay> {
        let top = top.clamp(0, h - 1);
        let class = wide(super::surface::CLASS);
        let title = wide("macRust genie");
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                0,
                top,
                w,
                h - top,
                None,
                None,
                GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
                None,
            )
            .ok()?;
            // The Dock's glass must not see the Genie, nor a later snapshot.
            capture::exclude_from_capture(hwnd);
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                let _ = DestroyWindow(hwnd);
                return None;
            };
            let old = SelectObject(dc, bmp.into());
            let o = Overlay {
                hwnd,
                dc,
                bmp,
                old,
                bits: bits as *mut u8,
                w,
                h,
                top,
                shown: false,
                drawn: Bounds::default(),
            };
            // The window's surface starts transparent, so the first frame of
            // every Genie uploads only what it draws.
            o.upload(None);
            Some(o)
        }
    }

    /// Copies the bitmap (or its `dirty` part, screen coordinates) to the window.
    fn upload(&self, dirty: Option<Bounds>) {
        // The bitmap keeps screen coordinates; the window shows it from `top` down.
        let pt = POINT { x: 0, y: self.top };
        let size = SIZE { cx: self.w, cy: self.h - self.top };
        let src = POINT { x: 0, y: self.top };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // prcDirty is in window coordinates.
        let rc = dirty.map(|d| RECT {
            left: d.x0.max(0),
            top: (d.y0 - self.top).max(0),
            right: d.x1.min(self.w),
            bottom: (d.y1 - self.top).clamp(0, self.h - self.top),
        });
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &pt,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: rc.as_ref().map_or(std::ptr::null(), |r| r as *const RECT),
        };
        unsafe {
            let _ = UpdateLayeredWindowIndirect(self.hwnd, &info);
        }
    }

    /// The bitmap: premultiplied BGRA, top-down, zeroed when created.
    fn buf(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }

    /// Puts the frame covering `drawn` into the window, shown or not, and
    /// erases what the previous frame left outside it.
    fn stage(&mut self, drawn: Bounds) {
        let dirty = drawn.union(self.drawn);
        self.drawn = drawn;
        if !dirty.is_empty() {
            self.upload(Some(dirty));
        }
    }

    /// Shows `drawn` and erases what the previous frame left outside it.
    fn present(&mut self, drawn: Bounds, below: HWND) {
        self.stage(drawn);
        if !self.drawn.is_empty() {
            self.show(below);
        }
    }

    /// Shows the staged frame.
    fn show(&mut self, below: HWND) {
        if !self.shown {
            unsafe {
                // Right below the Dock, so the window slides in behind the bar;
                // placed and shown in one call.
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(below),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            self.shown = true;
        }
    }

    /// Hides the window as it is (never through an empty frame), then
    /// clears the bitmap and the hidden window's surface for the next run.
    fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
        let (b, w, h) = (self.drawn, self.w as usize, self.h as usize);
        if !b.is_empty() {
            genie::clear(&mut Dst { px: self.buf(), w, h, x: 0, y: 0 }, b);
            self.upload(Some(b));
        }
        self.drawn = Bounds::default();
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshots

/// The window's pixels under `r`, premultiplied BGRA with rounded corners,
/// from Desktop Duplication. A window still on screen (`before` None) gets
/// a fresh frame, or PrintWindow without capture. A window Windows has just
/// minimized (`before` = GetTickCount of the event) is read from the newest
/// frame taken before that tick, never from one that may already miss it.
pub fn grab(cap: Option<&mut Capture>, h: HWND, r: WinRect, before: Option<u32>, radius: f32) -> Option<Snapshot> {
    let from_dda = cap.and_then(|c| {
        if before.is_none() {
            c.poll();
        }
        c.region_bgra(r, before)
    });
    let mut px = match from_dda {
        Some(px) => px,
        None if before.is_none() => print_window(h, r)?,
        None => return None,
    };
    let (w, hh) = (r.w as usize, r.h as usize);
    genie::round_corners(&mut px, w, hh, radius);
    Some(Snapshot::new(px, w, hh))
}

/// PrintWindow with full content (gets DirectComposition windows right on
/// Windows 8.1+), cropped to the visible frame `r`. Opaque BGRA.
fn print_window(h: HWND, r: WinRect) -> Option<Vec<u8>> {
    let mut wr = RECT::default();
    unsafe { GetWindowRect(h, &mut wr).ok()? };
    let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
    if ww <= 0 || wh <= 0 || r.w <= 0 || r.h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: ww,
                biHeight: -wh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let out = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok().and_then(|bmp| {
            let old = SelectObject(mem, bmp.into());
            let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
            let src = std::slice::from_raw_parts(bits as *const u8, (ww * wh * 4) as usize);
            let (ox, oy) = (r.x - wr.left, r.y - wr.top);
            let mut px = vec![0u8; (r.w * r.h * 4) as usize];
            for y in 0..r.h {
                let sy = y + oy;
                if !(0..wh).contains(&sy) {
                    continue;
                }
                for x in 0..r.w {
                    let sx = x + ox;
                    if (0..ww).contains(&sx) {
                        let s = ((sy * ww + sx) * 4) as usize;
                        let d = ((y * r.w + x) * 4) as usize;
                        px[d..d + 3].copy_from_slice(&src[s..s + 3]);
                        px[d + 3] = 255;
                    }
                }
            }
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            ok.then_some(px)
        });
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// The window still has an invalid region to paint (it validates it when
/// it draws); readable for other processes' windows.
fn needs_paint(h: HWND) -> bool {
    unsafe { GetUpdateRect(h, None, false).as_bool() }
}

/// Restore: why the last frame can come off the window `e` s after it was
/// shown under it, or None to keep it. With screen capture (which leaves
/// our windows out) the window must be seen on screen: like its snapshot,
/// or holding still for a few frames once it had time to draw (its content
/// changed while minimized). An update region validated three frames in a
/// row is the only sign without capture, and says nothing about windows
/// drawn by DirectX (browsers, Electron, UWP).
fn uncovered(r: &mut Run, cap: Option<&mut Capture>, e: f32) -> Option<String> {
    let gdi = !winops::is_minimized(r.hwnd) && !needs_paint(r.hwnd);
    let seen = cap.and_then(|c| c.sample(r.rect(), GRID));
    let Some(seen) = seen.filter(|_| !r.expect.is_empty()) else {
        r.painted = if gdi { r.painted + 1 } else { 0 };
        return if r.painted >= 3 {
            Some("painted (no screen capture)".into())
        } else if e > 0.5 {
            Some("not seen painted, gave up".into())
        } else {
            None
        };
    };
    let diff = genie::sample_diff(&seen, &r.expect);
    r.still = if genie::sample_diff(&seen, &r.seen) < 2.0 { r.still + 1 } else { 0 };
    r.seen = seen;
    if gdi && diff < MATCH {
        Some(format!("on screen as in the snapshot (diff {diff:.1})"))
    } else if gdi && r.still >= 4 && e > 0.2 {
        Some(format!("on screen and still, unlike the snapshot (diff {diff:.1})"))
    } else if e > UNCOVER_MAX_S {
        Some(format!("not seen on screen (diff {diff:.1}), gave up"))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Runs

/// One window to animate.
pub struct Item {
    pub hwnd: HWND,
    pub path: Path,
    pub snap: Snapshot,
}

struct Run {
    hwnd: HWND,
    dir: Dir,
    path: Path,
    snap: Snapshot,
    start: Instant,
    /// Minimize: the first frame is over the window, which Windows has not
    /// hidden yet; the clock starts when it has.
    waiting: Option<Instant>,
    /// What started it, for the trace.
    via: &'static str,
    /// Restore: bring this window to the front when it is back.
    activate: bool,
    /// Restore: when the real window was shown under the last frame.
    shown_at: Option<Instant>,
    /// Restore: frames in a row the real window looked painted.
    painted: u32,
    /// Restore: the snapshot's grid samples, and the screen's last ones.
    expect: Vec<[u8; 3]>,
    seen: Vec<[u8; 3]>,
    /// Restore: captured frames in a row the screen under the window held still.
    still: u32,
    /// The moment the overlay last showed (s into the run).
    drawn_t: f32,
}

impl Run {
    fn new(hwnd: HWND, dir: Dir, path: Path, snap: Snapshot, via: &'static str) -> Run {
        Run {
            hwnd,
            dir,
            path,
            snap,
            start: Instant::now(),
            waiting: None,
            via,
            activate: false,
            shown_at: None,
            painted: 0,
            expect: Vec::new(),
            seen: Vec::new(),
            still: 0,
            drawn_t: 0.0,
        }
    }

    fn rect(&self) -> WinRect {
        let w = self.path.win;
        WinRect { x: w.x as i32, y: w.y as i32, w: w.w as i32, h: w.h as i32 }
    }
}

struct Cached {
    /// The window's frame when it was minimized; it comes back there.
    rect: WinRect,
    px: Vec<u8>,
    w: usize,
    h: usize,
    stamp: u64,
}

pub struct Genie {
    /// The user has Windows' minimize animation on.
    pub enabled: bool,
    /// The menu bar's height in px: the overlay starts below it.
    top: i32,
    /// Windows' animation is off for the whole session (title-bar minimizes
    /// play the Genie too); otherwise only around our own calls.
    session: bool,
    /// Off around our own calls when not `session`.
    op_suppressed: bool,
    pub win11: bool,
    dock: HWND,
    overlay: Option<Overlay>,
    runs: Vec<Run>,
    /// A minimize staged in the hidden overlay, waiting for `fire`.
    armed: Option<Run>,
    cache: HashMap<isize, Cached>,
    stamp: u64,
    /// Invisible resize borders of windows seen on screen.
    borders: HashMap<isize, (i32, i32, i32, i32)>,
    cost: (f64, f64, u32),
    probed: bool,
    /// Scratch for `draw_under`.
    glass_buf: Vec<u8>,
}

/// Logs whether DWM lets this process set an attribute of another app's
/// window: DWMWA_TRANSITIONS_FORCEDISABLED is set to its default (FALSE),
/// which changes nothing. If it is allowed, a later version can leave
/// Windows' minimize animation on (the shell hook then asks before every
/// minimize) and switch it off per window instead.
fn probe_transitions(h: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
    let off = windows::core::BOOL(0);
    let r = unsafe {
        DwmSetWindowAttribute(h, DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const _ as *const _, size_of::<windows::core::BOOL>() as u32)
    };
    log(format!(
        "genie: DWM transitions attribute on another app's window: {}",
        match r {
            Ok(()) => "allowed".to_string(),
            Err(e) => format!("refused ({:#010x})", e.code().0),
        }
    ));
}

impl Genie {
    /// `active`: the Genie is the chosen minimize effect; otherwise this
    /// stays idle and leaves Windows' animation alone.
    pub fn new(dock: HWND, capture: bool, active: bool) -> Genie {
        let enabled = active && spi_min_animate().unwrap_or(false);
        let session = enabled && capture && suppress();
        if active {
            log(format!(
            "minimize effect: genie, {}",
            match (enabled, session) {
                (false, _) => "off (Windows minimize animation is off)",
                (true, true) => "on, title-bar minimizes too",
                (true, false) => "on for the Dock only (no screen capture)",
            }
            ));
        }
        let build = super::system::reg_string(
            windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "CurrentBuild",
        );
        Genie {
            enabled,
            session,
            op_suppressed: false,
            win11: build.and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22000),
            dock,
            overlay: None,
            runs: Vec::new(),
            armed: None,
            cache: HashMap::new(),
            stamp: 0,
            borders: HashMap::new(),
            cost: (0.0, 0.0, 0),
            probed: false,
            glass_buf: Vec::new(),
            top: 0,
        }
    }

    pub fn set_top(&mut self, px: i32) {
        self.top = px;
    }

    /// Whether a title-bar minimize can play the Genie: the snapshot then has
    /// to come from the last captured frame.
    pub fn session(&self) -> bool {
        self.session
    }

    pub fn shutdown(&mut self) {
        self.armed = None;
        self.runs.clear();
        self.overlay = None;
        release_min_animate();
    }

    pub fn animating(&self) -> bool {
        !self.runs.is_empty()
    }

    pub fn busy(&self, h: HWND) -> bool {
        self.runs.iter().any(|r| r.hwnd == h)
    }

    /// Remembers the resize borders of a window on screen, to find its frame
    /// again once it is minimized.
    pub fn note_window(&mut self, h: HWND) {
        if !winops::is_minimized(h) {
            self.borders.insert(h.0 as isize, winops::borders(h));
        }
        if !self.probed {
            self.probed = true;
            probe_transitions(h);
        }
    }

    pub fn borders_of(&self, h: HWND) -> Option<(i32, i32, i32, i32)> {
        self.borders.get(&(h.0 as isize)).copied()
    }

    /// Drops what belongs to windows that are gone.
    pub fn forget_except(&mut self, live: &HashSet<isize>) {
        self.cache.retain(|k, _| live.contains(k));
        self.borders.retain(|k, _| live.contains(k));
    }

    fn room(&self) -> usize {
        MAX_RUNS.saturating_sub(self.runs.len())
    }

    /// Minimizes windows into their Dock icon. `already` is true when Windows
    /// has minimized them (a title-bar click); otherwise we minimize them once
    /// the first frame is on screen. `since`: when the click or the event was
    /// handled, for the timings in the trace.
    pub fn minimize(&mut self, items: Vec<Item>, already: bool, screen: (i32, i32), why: &str, since: Instant) {
        let items: Vec<Item> = items.into_iter().take(self.room()).collect();
        if items.is_empty() {
            return;
        }
        if !self.session && !self.op_suppressed {
            self.op_suppressed = suppress();
        }
        self.disarm();
        let now = Instant::now();
        let n = items.len();
        let first = items.first().map(|i| (i.snap.w, i.snap.h)).unwrap_or_default();
        let hwnds: Vec<HWND> = items.iter().map(|i| i.hwnd).collect();
        let via = if already { "the minimize event" } else { "the Dock or the menu" };
        for it in items {
            self.runs.retain(|r| r.hwnd != it.hwnd);
            self.runs.push(Run::new(it.hwnd, Dir::Minimize, it.path, it.snap, via));
        }
        let ms = |t: Instant| t.duration_since(since).as_secs_f32() * 1000.0;
        trace(format!(
            "genie minimize start: {n} window(s), first {}x{}, from {why}, snapshot {:.1} ms",
            first.0,
            first.1,
            ms(now)
        ));
        // The overlay shows the window before the real one goes away.
        self.step(screen, None);
        if already {
            // Windows hid the window before anything could be drawn: the
            // desktop showed where it was from the event until this frame.
            trace(format!("genie first frame presented: {:.1} ms, window already hidden (blink)", ms(Instant::now())));
        } else {
            trace(format!("genie first frame presented: {:.1} ms", ms(Instant::now())));
            // Wait for DWM to compose that frame, so no composition shows the
            // desktop where the window was.
            unsafe {
                let _ = DwmFlush();
            }
            for &h in &hwnds {
                winops::minimize(h);
            }
            trace(format!("genie window minimize sent after the frame was composed: {:.1} ms", ms(Instant::now())));
            // The app minimizes when it gets to it: hold the first frame
            // until it has, so nothing of the window shows beside the warp.
            let t = Instant::now();
            for r in &mut self.runs {
                if hwnds.contains(&r.hwnd) {
                    r.waiting = Some(t);
                }
            }
        }
        // The squeezed levels are needed from the second frame on.
        for r in &mut self.runs {
            if !r.snap.has_levels() {
                r.snap.build_levels();
            }
        }
    }

    /// Restores `h` out of its Dock icon when its snapshot is kept, else
    /// without an animation. `path_for` maps the window's frame to its
    /// path; `activate` brings it to the front.
    pub fn restore(&mut self, h: HWND, path_for: &dyn Fn(WinRect) -> Option<Path>, activate: bool, screen: (i32, i32)) {
        let key = h.0 as isize;
        let path = self.cache.get(&key).filter(|_| self.room() > 0).and_then(|c| path_for(c.rect));
        let (Some(path), Some(c)) = (path, path.and_then(|_| self.cache.remove(&key))) else {
            self.restore_native(h, activate);
            return;
        };
        if !self.session && !self.op_suppressed {
            self.op_suppressed = suppress();
        }
        self.disarm();
        trace(format!("genie restore start: {}x{}", c.w, c.h));
        let expect = genie::grid_samples(&c.px, c.w, c.h, GRID);
        let mut snap = Snapshot::new(c.px, c.w, c.h);
        // The restore starts squeezed: the levels are needed at once.
        snap.build_levels();
        self.runs.retain(|r| r.hwnd != h);
        let mut run = Run::new(h, Dir::Restore, path, snap, "dock");
        run.activate = activate;
        run.expect = expect;
        self.runs.push(run);
        self.step(screen, None);
    }

    /// No snapshot: the window comes back without an animation. Turning
    /// Windows' own one on for it also let other minimizes play it.
    fn restore_native(&mut self, h: HWND, activate: bool) {
        trace("genie restore: no snapshot, restored without the effect");
        if activate {
            winops::activate(h);
        } else {
            winops::restore(h);
        }
    }

    /// Draws the first frame of minimizing `it` into the hidden overlay, for
    /// `fire` to show: the user has pressed a title-bar minimize button, and
    /// the app minimizes on the release. Only while nothing else plays: the
    /// frame takes the overlay's bitmap.
    pub fn arm(&mut self, it: Item, screen: (i32, i32)) -> bool {
        self.disarm();
        if !self.runs.is_empty() || !self.ensure_overlay(screen) {
            return false;
        }
        let Some(ov) = &mut self.overlay else { return false };
        let (w, h) = (ov.w as usize, ov.h as usize);
        let prev = ov.drawn;
        let mut dst = Dst { px: ov.buf(), w, h, x: 0, y: 0 };
        genie::clear(&mut dst, prev);
        let drawn = genie::draw(&it.snap, &it.path, &genie::frame(&it.path, Dir::Minimize, 0.0), &mut dst);
        ov.stage(drawn);
        self.armed = Some(Run::new(it.hwnd, Dir::Minimize, it.path, it.snap, ""));
        true
    }

    pub fn armed(&self) -> Option<HWND> {
        self.armed.as_ref().map(|r| r.hwnd)
    }

    /// Drops a staged first frame that will not be needed.
    pub fn disarm(&mut self) {
        if self.armed.take().is_some()
            && self.runs.is_empty()
            && let Some(o) = &mut self.overlay
        {
            o.hide();
        }
    }

    /// Shows the staged first frame of `h` over it and waits until DWM has
    /// composed it; the Genie moves once Windows has hidden the window.
    /// `since`: when the release or the event was handled.
    pub fn fire(&mut self, h: HWND, via: &'static str, since: Instant) -> bool {
        let Some(mut run) = self.armed.take_if(|r| r.hwnd == h) else { return false };
        let Some(ov) = &mut self.overlay else { return false };
        let hidden = winops::is_minimized(h);
        ov.show(self.dock);
        unsafe {
            let _ = DwmFlush();
        }
        let ms = since.elapsed().as_secs_f32() * 1000.0;
        trace(format!(
            "genie first frame on screen from {via}: {ms:.1} ms, {}",
            if hidden { "window already hidden (blink)" } else { "window still shown (no blink)" }
        ));
        run.via = via;
        run.start = Instant::now();
        if !hidden {
            run.waiting = Some(run.start);
        }
        self.runs.push(run);
        true
    }

    /// Whether the overlay is on screen.
    pub fn overlay_shown(&self) -> bool {
        self.overlay.as_ref().is_some_and(|o| o.shown)
    }

    /// Whether a running Genie reaches into `r` (screen coordinates),
    /// counting the rows behind the Dock's bar.
    pub fn overlaps(&self, r: WinRect) -> bool {
        self.overlay_shown()
            && self.runs.iter().any(|run| {
                let w = run.path.win;
                let f = genie::frame(&run.path, run.dir, run.drawn_t);
                // From the top edge (and its shadow) down to the icon, across
                // the window and the icon.
                let (x0, x1) = (w.x.min(run.path.icon.x) - 40.0, (w.x + w.w).max(run.path.icon.x + run.path.icon.w) + 40.0);
                let (y0, y1) = (f.top - 40.0, run.path.screen_h);
                x0 < (r.x + r.w) as f32 && x1 > r.x as f32 && y0 < (r.y + r.h) as f32 && y1 > r.y as f32
            })
    }

    /// Draws the running Genies over `pm`, the captured screen under `r`
    /// (opaque RGBA), as glass there should see them: screen capture leaves
    /// the overlay out. Unlike the overlay, the rows behind the Dock's bar
    /// are drawn too, so the window shows through the glass as it goes in.
    pub fn draw_under(&mut self, r: WinRect, pm: &mut Pixmap) -> bool {
        if r.w <= 0 || r.h <= 0 || pm.width() as i32 != r.w || pm.height() as i32 != r.h {
            return false;
        }
        let (w, h) = (r.w as usize, r.h as usize);
        self.glass_buf.clear();
        self.glass_buf.resize(w * h * 4, 0);
        let mut layer = Dst { px: &mut self.glass_buf, w, h, x: r.x, y: r.y };
        let mut drawn = Bounds::default();
        for run in &self.runs {
            let f = genie::frame(&run.path, run.dir, run.drawn_t);
            let behind = Path { clip: run.path.screen_h, ..run.path };
            drawn = drawn.union(genie::draw(&run.snap, &behind, &f, &mut layer));
        }
        // draw() returns buffer coordinates; composite() wants screen ones.
        let b = Bounds { x0: drawn.x0 + r.x, y0: drawn.y0 + r.y, x1: drawn.x1 + r.x, y1: drawn.y1 + r.y };
        !drawn.is_empty() && genie::composite(&layer, b, pm.data_mut(), w, h, r.x, r.y, true)
    }

    /// The window came back by other means (Alt+Tab, the app itself).
    pub fn restored_elsewhere(&mut self, h: HWND) {
        self.runs.retain(|r| !(r.hwnd == h && r.dir == Dir::Minimize));
    }

    /// Creates the overlay ahead of the first Genie (a window and a
    /// screen-sized bitmap), so a title-bar minimize does not wait for it.
    pub fn prepare(&mut self, screen: (i32, i32)) {
        if self.enabled && self.overlay.as_ref().is_none_or(|o| (o.w, o.h, o.top) != (screen.0, screen.1, self.top)) {
            if self.overlay.as_ref().is_some_and(|o| o.shown) {
                // Recreated on the next frame instead, not under a running Genie.
                return;
            }
            self.armed = None;
            self.overlay = None;
            self.overlay = Overlay::new(screen.0, screen.1, self.top);
        }
    }

    /// The overlay exists and fits the screen; false if it cannot be made.
    fn ensure_overlay(&mut self, screen: (i32, i32)) -> bool {
        if self.overlay.as_ref().is_none_or(|o| (o.w, o.h, o.top) != (screen.0, screen.1, self.top)) {
            self.armed = None;
            self.overlay = None;
            self.overlay = Overlay::new(screen.0, screen.1, self.top);
        }
        self.overlay.is_some()
    }

    /// One frame of every running Genie; call at display refresh while
    /// `animating()`. `cap` tells when a restored window is on screen.
    pub fn step(&mut self, screen: (i32, i32), mut cap: Option<&mut Capture>) {
        let now = Instant::now();
        // Something else turned Windows' animation back on.
        if self.session && spi_min_animate() == Some(true) {
            spi_set_min_animate(false);
        }
        if self.runs.is_empty() {
            self.finish_all();
            return;
        }
        if !self.ensure_overlay(screen) {
            // No overlay: finish without the effect.
            for r in std::mem::take(&mut self.runs) {
                if r.dir == Dir::Restore {
                    winops::activate(r.hwnd);
                }
            }
            return;
        }
        let t0 = Instant::now();
        if self.runs.iter().any(|r| r.shown_at.is_some())
            && let Some(c) = cap.as_deref_mut()
        {
            c.poll();
        }
        // Which runs are over: a minimized window gone into the Dock; a
        // restored window on screen under the last frame. A first frame
        // shown over a window that did not minimize after all goes.
        let mut done = Vec::new();
        let mut dropped = Vec::new();
        for (i, r) in self.runs.iter_mut().enumerate() {
            if let Some(w) = r.waiting {
                let waited = w.elapsed().as_secs_f32();
                if winops::is_minimized(r.hwnd) {
                    r.waiting = None;
                    r.start = now;
                    trace(format!("genie: window hidden {:.0} ms after the first frame ({})", waited * 1000.0, r.via));
                } else if waited > WAIT_S || winops::frame_rect(r.hwnd).is_none_or(|f| f != r.rect()) {
                    trace(format!("genie: window not minimized after the first frame ({}), dropped", r.via));
                    dropped.push(i);
                }
                continue;
            }
            if !r.snap.has_levels() {
                r.snap.build_levels();
            }
            let t = (now - r.start).as_secs_f32();
            let end = genie::duration(r.dir);
            match r.dir {
                // Kept after it is gone behind the bar until the slide ends:
                // the Dock's glass still shows it going in (draw_under).
                Dir::Minimize => {
                    if t >= end {
                        done.push(i);
                    }
                }
                Dir::Restore if t >= end => match r.shown_at {
                    None => {
                        if r.activate {
                            winops::activate(r.hwnd);
                        } else {
                            winops::restore(r.hwnd);
                        }
                        r.shown_at = Some(now);
                        trace(format!("genie restore: window shown at {} ms", r.start.elapsed().as_millis()));
                    }
                    Some(s) => {
                        let e = s.elapsed().as_secs_f32();
                        if let Some(why) = uncovered(r, cap.as_deref_mut(), e) {
                            trace(format!("genie restore: last frame off after {:.0} ms, {why}", e * 1000.0));
                            done.push(i);
                        }
                    }
                },
                Dir::Restore => {}
            }
        }
        let all_done = done.len() + dropped.len() == self.runs.len();
        let mut gone: Vec<(usize, bool)> = done.into_iter().map(|i| (i, true)).chain(dropped.into_iter().map(|i| (i, false))).collect();
        gone.sort_unstable();
        for (i, finished) in gone.into_iter().rev() {
            let r = self.runs.remove(i);
            if !finished {
                continue;
            }
            let ms = r.start.elapsed().as_millis();
            trace(format!("genie {} end: {ms} ms", if r.dir == Dir::Minimize { "minimize" } else { "restore" }));
            if r.dir == Dir::Minimize {
                let rect = r.rect();
                self.keep(r.hwnd, rect, r.snap);
            }
        }
        if all_done {
            // Hide with the last frame still in place: an empty frame first
            // would uncover a restored window that may not be drawn yet.
            self.finish_all();
            return;
        }
        let Some(ov) = &mut self.overlay else { return };
        let (w, h) = (ov.w as usize, ov.h as usize);
        let prev = ov.drawn;
        let mut dst = Dst { px: ov.buf(), w, h, x: 0, y: 0 };
        genie::clear(&mut dst, prev);
        let mut drawn = Bounds::default();
        for r in &mut self.runs {
            let t = if r.waiting.is_some() { 0.0 } else { (now - r.start).as_secs_f32().min(genie::duration(r.dir)) };
            r.drawn_t = t;
            drawn = drawn.union(genie::draw(&r.snap, &r.path, &genie::frame(&r.path, r.dir, t), &mut dst));
        }
        ov.present(drawn, self.dock);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.cost = (self.cost.0 + ms, self.cost.1.max(ms), self.cost.2 + 1);
    }

    fn finish_all(&mut self) {
        if let Some(o) = &mut self.overlay
            && o.shown
        {
            o.hide();
            trace("genie overlay hidden");
        }
        if self.cost.2 > 0 {
            trace(format!(
                "genie frames: {}, mean {:.1} ms, worst {:.1} ms",
                self.cost.2,
                self.cost.0 / self.cost.2 as f64,
                self.cost.1
            ));
            self.cost = (0.0, 0.0, 0);
        }
        if self.op_suppressed {
            release_min_animate();
            self.op_suppressed = false;
        }
    }

    fn keep(&mut self, h: HWND, rect: WinRect, snap: Snapshot) {
        let (w, hh) = (snap.w, snap.h);
        self.stamp += 1;
        self.cache.insert(h.0 as isize, Cached { rect, px: snap.into_pixels(), w, h: hh, stamp: self.stamp });
        // Oldest out first.
        while self.cache.values().map(|c| c.px.len()).sum::<usize>() > CACHE_BYTES {
            let Some(&k) = self.cache.iter().min_by_key(|(_, c)| c.stamp).map(|(k, _)| k) else { break };
            self.cache.remove(&k);
        }
    }
}
