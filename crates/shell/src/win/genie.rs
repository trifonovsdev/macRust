//! Minimize into a Dock icon and restore out of it with the macOS Genie
//! (`macrust_ui::genie` draws it). The window is captured, Windows' own
//! minimize animation is kept off, and a click-through layered overlay just
//! below the Dock plays the warp at display refresh.
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

// ---------------------------------------------------------------------------
// Windows' own minimize animation

fn spi_min_animate() -> Option<bool> {
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
fn suppress() -> bool {
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
    /// What the last presented frame covered.
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

    /// Shows `drawn` and erases what the previous frame left outside it.
    fn present(&mut self, drawn: Bounds, below: HWND) {
        let dirty = drawn.union(self.drawn);
        self.drawn = drawn;
        if dirty.is_empty() {
            return;
        }
        self.upload(Some(dirty));
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
    /// Restore: bring this window to the front when it is back.
    activate: bool,
    /// Restore: when the real window was shown under the last frame.
    shown_at: Option<Instant>,
    /// Restore: frames in a row the real window looked painted.
    painted: u32,
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
    cache: HashMap<isize, Cached>,
    stamp: u64,
    /// Restores left to Windows' own animation, which is on until they finish.
    native: Vec<(HWND, Instant)>,
    /// Invisible resize borders of windows seen on screen.
    borders: HashMap<isize, (i32, i32, i32, i32)>,
    cost: (f64, f64, u32),
}

impl Genie {
    pub fn new(dock: HWND, capture: bool) -> Genie {
        let enabled = spi_min_animate().unwrap_or(false);
        let session = enabled && capture && suppress();
        log(format!(
            "genie: {}",
            match (enabled, session) {
                (false, _) => "off (Windows minimize animation is off)",
                (true, true) => "on, title-bar minimizes too",
                (true, false) => "on for the Dock only (no screen capture)",
            }
        ));
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
            cache: HashMap::new(),
            stamp: 0,
            native: Vec::new(),
            borders: HashMap::new(),
            cost: (0.0, 0.0, 0),
            top: 0,
        }
    }

    pub fn set_top(&mut self, px: i32) {
        self.top = px;
    }

    /// Whether a title-bar minimize can play the Genie: the snapshot then has
    /// to come from the last captured frame.
    pub fn session(&self) -> bool {
        self.session && self.native.is_empty()
    }

    pub fn shutdown(&mut self) {
        self.runs.clear();
        self.overlay = None;
        release_min_animate();
    }

    pub fn animating(&self) -> bool {
        !self.runs.is_empty() || !self.native.is_empty()
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
        let now = Instant::now();
        let n = items.len();
        let first = items.first().map(|i| (i.snap.w, i.snap.h)).unwrap_or_default();
        let hwnds: Vec<HWND> = items.iter().map(|i| i.hwnd).collect();
        for it in items {
            self.runs.retain(|r| r.hwnd != it.hwnd);
            self.runs.push(Run {
                hwnd: it.hwnd,
                dir: Dir::Minimize,
                path: it.path,
                snap: it.snap,
                start: now,
                activate: false,
                shown_at: None,
                painted: 0,
            });
        }
        let ms = |t: Instant| t.duration_since(since).as_secs_f32() * 1000.0;
        trace(format!(
            "genie minimize start: {n} window(s), first {}x{}, from {why}, snapshot {:.1} ms",
            first.0,
            first.1,
            ms(now)
        ));
        // The overlay shows the window before the real one goes away.
        self.step(screen);
        trace(format!("genie first frame presented: {:.1} ms", ms(Instant::now())));
        if !already {
            // Wait for DWM to compose that frame, so no composition shows the
            // desktop where the window was.
            unsafe {
                let _ = DwmFlush();
            }
            for h in hwnds {
                winops::minimize(h);
            }
            trace(format!("genie window minimize sent: {:.1} ms", ms(Instant::now())));
        }
        // The squeezed levels are needed from the second frame on.
        for r in &mut self.runs {
            if !r.snap.has_levels() {
                r.snap.build_levels();
            }
        }
    }

    /// Restores `h` out of its Dock icon when its snapshot is kept, else with
    /// Windows' own animation. `path_for` maps the window's frame to its
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
        trace(format!("genie restore start: {}x{}", c.w, c.h));
        let mut snap = Snapshot::new(c.px, c.w, c.h);
        // The restore starts squeezed: the levels are needed at once.
        snap.build_levels();
        self.runs.retain(|r| r.hwnd != h);
        self.runs.push(Run {
            hwnd: h,
            dir: Dir::Restore,
            path,
            snap,
            start: Instant::now(),
            activate,
            shown_at: None,
            painted: 0,
        });
        self.step(screen);
    }

    fn restore_native(&mut self, h: HWND, activate: bool) {
        if self.session && self.enabled {
            spi_set_min_animate(true);
            self.native.push((h, Instant::now()));
        }
        if activate {
            winops::activate(h);
        } else {
            winops::restore(h);
        }
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
            self.overlay = None;
            self.overlay = Overlay::new(screen.0, screen.1, self.top);
        }
    }

    /// One frame of every running Genie; call at display refresh while
    /// `animating()`.
    pub fn step(&mut self, screen: (i32, i32)) {
        let now = Instant::now();
        self.native.retain(|&(h, since)| {
            let e = since.elapsed().as_secs_f32();
            (winops::is_minimized(h) || e < 0.05) && e < 1.0
        });
        if self.native.is_empty() && self.session && spi_min_animate() == Some(true) {
            spi_set_min_animate(false);
        }
        if self.runs.is_empty() {
            self.finish_all();
            return;
        }
        if self.overlay.as_ref().is_none_or(|o| (o.w, o.h, o.top) != (screen.0, screen.1, self.top)) {
            self.overlay = None;
            self.overlay = Overlay::new(screen.0, screen.1, self.top);
        }
        if self.overlay.is_none() {
            // No overlay: finish without the effect.
            for r in std::mem::take(&mut self.runs) {
                if r.dir == Dir::Restore {
                    winops::activate(r.hwnd);
                }
            }
            return;
        }
        let t0 = Instant::now();
        // Which runs are over: a minimized window gone behind the Dock; a
        // restored window shown and painted under the last frame.
        let mut done = Vec::new();
        for (i, r) in self.runs.iter_mut().enumerate() {
            let t = (now - r.start).as_secs_f32();
            let end = genie::duration(r.dir);
            match r.dir {
                Dir::Minimize => {
                    if t >= end || !genie::visible(&r.path, &genie::frame(&r.path, r.dir, t)) {
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
                        // Keep the last frame over it until the app has
                        // drawn: restored, its update region validated, for
                        // three frames in a row.
                        let painted = !winops::is_minimized(r.hwnd) && !needs_paint(r.hwnd);
                        r.painted = if painted { r.painted + 1 } else { 0 };
                        let e = s.elapsed().as_secs_f32();
                        if r.painted >= 3 || e > 0.5 {
                            trace(format!(
                                "genie restore: window {} after {:.0} ms",
                                if r.painted >= 3 { "painted" } else { "not seen painted, gave up" },
                                e * 1000.0
                            ));
                            done.push(i);
                        }
                    }
                },
                Dir::Restore => {}
            }
        }
        let all_done = done.len() == self.runs.len();
        for i in done.into_iter().rev() {
            let r = self.runs.remove(i);
            let ms = r.start.elapsed().as_millis();
            trace(format!("genie {} end: {ms} ms", if r.dir == Dir::Minimize { "minimize" } else { "restore" }));
            if r.dir == Dir::Minimize {
                let w = r.path.win;
                self.keep(r.hwnd, WinRect { x: w.x as i32, y: w.y as i32, w: w.w as i32, h: w.h as i32 }, r.snap);
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
        for r in &self.runs {
            let t = (now - r.start).as_secs_f32().min(genie::duration(r.dir));
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
        if self.op_suppressed && self.native.is_empty() {
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
