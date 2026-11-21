//! Minimize into a Dock icon and restore out of it with the macOS Scale
//! effect (`macrust_ui::scale`). DWM draws it: a thumbnail of the window
//! (DwmRegisterThumbnail) in a click-through overlay just below the Dock is
//! placed exactly over the window, and its destination rect is moved toward
//! the icon at display refresh. No pixels pass through the CPU and the
//! content is the window's own, so there is nothing to capture, cache or
//! match, and every minimize and restore we start plays the same way.
//!
//! Minimize: the thumbnail covers the window first (DwmFlush waits for that
//! composition), then the window minimizes under it. A title-bar button is
//! seen by the mouse hook (`minbutton.rs`): the thumbnail is registered on
//! the press and shown on the release, before the app gets it. Win+Down and
//! app-initiated minimizes are seen through HSHELL_GETMINRECT (before the
//! window hides, where Windows sends it) or EVENT_SYSTEM_MINIMIZESTART
//! (after: the desktop shows for the event's latency; the trace says so).
//! Restore: the thumbnail of the minimized window grows out of the icon,
//! the window is restored under its last frame, and the thumbnail goes once
//! the window is seen shown (both show the same live surface by then).
//!
//! DWM keeps the last picture of a minimized window for its thumbnail, as it
//! does for taskbar previews (Microsoft Q&A 106803, moderator answer); that
//! picture was reported to include the window's shadow on Windows 10 21H1
//! (Q&A 725780), so its size can differ from the window's. The source size
//! is read every frame and the crop to the visible frame follows it; the
//! trace logs both sizes.
//!
//! Windows' own minimize animation (SPI_SETANIMATION) is off for the whole
//! session, as with the Genie (`genie::suppress`), and nothing here turns it
//! back on.

use std::time::Instant;

use macrust_ui::scale::{self, Crop, Dir, Path, Rect};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_VISIBLE, DwmFlush,
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, RDW_ERASE, RDW_ERASENOW, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindow, IsWindowVisible, LWA_ALPHA, LWA_COLORKEY, RegisterClassExW,
    SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::genie;
use super::util::{log, trace};
use super::winops;

/// More windows than this at once (Win+M) minimize without it.
const MAX_RUNS: usize = 8;
/// A thumbnail shown over a window waits this long for the window to minimize.
const WAIT_S: f32 = 0.4;
/// Restore: frames the window must be seen shown before its thumbnail goes.
const UNCOVER_FRAMES: u32 = 2;
/// Restore: the last frame stays at most this long over a window not shown.
const UNCOVER_MAX_S: f32 = 0.5;
/// The overlay's background, keyed out. Not black: DWM may key thumbnail
/// pixels too (unverified), and pure black is common in dark windows.
const KEY: COLORREF = COLORREF(0x0001_0001);

fn hr(e: &windows::core::Error) -> String {
    format!("{:#010x}", e.code().0)
}

fn ms(since: Instant) -> f32 {
    since.elapsed().as_secs_f32() * 1000.0
}

// ---------------------------------------------------------------------------
// The overlay: a click-through, color-keyed layered window below the Dock.
// OnTopReplica's click-through mode shows DWM thumbnails in the same kind of
// window (WinForms TransparencyKey = a layered window with LWA_COLORKEY);
// whether a per-pixel (UpdateLayeredWindow) window shows them is not known.

unsafe extern "system" fn overlay_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

fn overlay_class() -> PCWSTR {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let name = w!("macRustScale");
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: GetModuleHandleW(PCWSTR::null()).unwrap_or_default().into(),
            // DefWindowProc erases with it: the whole surface is the key.
            hbrBackground: CreateSolidBrush(KEY),
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
    name
}

struct Overlay {
    hwnd: HWND,
    w: i32,
    h: i32,
    /// Rows above this stay out of the window: a window covering the whole
    /// monitor makes Explorer announce a full-screen app and hide our bars.
    top: i32,
    shown: bool,
}

impl Overlay {
    fn new(w: i32, h: i32, top: i32) -> Result<Overlay, String> {
        let top = top.clamp(0, h - 1);
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                overlay_class(),
                w!("macRust scale"),
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
            .map_err(|e| format!("CreateWindowEx failed ({})", hr(&e)))?;
            if let Err(e) = SetLayeredWindowAttributes(hwnd, KEY, 0, LWA_COLORKEY | LWA_ALPHA) {
                let _ = DestroyWindow(hwnd);
                return Err(format!("SetLayeredWindowAttributes failed ({})", hr(&e)));
            }
            Ok(Overlay { hwnd, w, h, top, shown: false })
        }
    }

    /// Shows the window right below the Dock with its surface erased to the
    /// key: it is shown fully transparent (alpha 0) until then, so an
    /// unpainted surface never reaches the screen.
    fn show(&mut self, below: HWND) {
        if self.shown {
            return;
        }
        unsafe {
            let _ = SetLayeredWindowAttributes(self.hwnd, KEY, 0, LWA_COLORKEY | LWA_ALPHA);
            let _ = SetWindowPos(self.hwnd, Some(below), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = RedrawWindow(Some(self.hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ERASENOW | RDW_UPDATENOW);
            let _ = SetLayeredWindowAttributes(self.hwnd, KEY, 255, LWA_COLORKEY | LWA_ALPHA);
        }
        self.shown = true;
    }

    fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }

    /// Screen rect to client coordinates.
    fn dest(&self, r: Rect) -> RECT {
        let (l, t, rr, b) = r.edges();
        RECT { left: l, top: t - self.top, right: rr, bottom: b - self.top }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Runs

/// One window to animate.
pub struct Item {
    pub hwnd: HWND,
    pub path: Path,
    /// Its invisible resize borders (left, top, right, bottom), when known.
    pub borders: Option<(i32, i32, i32, i32)>,
}

/// A registered thumbnail; unregistered when dropped.
struct Thumb(isize);

impl Drop for Thumb {
    fn drop(&mut self) {
        unsafe {
            let _ = DwmUnregisterThumbnail(self.0);
        }
    }
}

struct Run {
    hwnd: HWND,
    thumb: Thumb,
    dir: Dir,
    path: Path,
    borders: Option<(i32, i32, i32, i32)>,
    /// The thumbnail source's size and the part of it that is the frame.
    src: (i32, i32),
    crop: Crop,
    visible: bool,
    start: Instant,
    /// Minimize: shown over the window, which has not minimized yet; the
    /// clock starts when it has.
    waiting: Option<Instant>,
    /// What started it, for the trace.
    via: &'static str,
    /// Restore: bring the window to the front when it is back.
    activate: bool,
    /// Restore: when the window was restored under the last frame.
    shown_at: Option<Instant>,
    /// Restore: frames the window has been seen shown since.
    seen: u32,
    frames: u32,
    last: Instant,
    worst_gap: f32,
    /// A DWM call failed during the run (traced once).
    failed: bool,
}

impl Run {
    fn frame_size(&self) -> (i32, i32) {
        (self.path.win.w.round() as i32, self.path.win.h.round() as i32)
    }

    fn src_rect(&self) -> RECT {
        let c = self.crop;
        RECT { left: c.l, top: c.t, right: self.src.0 - c.r, bottom: self.src.1 - c.b }
    }

    /// Puts the thumbnail at progress `e`.
    fn place(&mut self, ov: &Overlay, e: f32) {
        let r = scale::frame(&self.path, e);
        let dest = ov.dest(r);
        let visible = self.visible && dest.right > dest.left && dest.bottom > dest.top;
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
            rcDestination: dest,
            rcSource: self.src_rect(),
            opacity: 255,
            fVisible: visible.into(),
            fSourceClientAreaOnly: false.into(),
        };
        if let Err(e) = unsafe { DwmUpdateThumbnailProperties(self.thumb.0, &props) }
            && !self.failed
        {
            self.failed = true;
            trace(format!("scale: DwmUpdateThumbnailProperties failed ({}) via {}", hr(&e), self.via));
        }
    }

    /// Follows a change of the source's size (a window that has just
    /// minimized may be kept with its shadow).
    fn refresh_source(&mut self) {
        let Ok(s) = (unsafe { DwmQueryThumbnailSourceSize(self.thumb.0) }) else { return };
        if (s.cx, s.cy) != self.src && s.cx > 0 && s.cy > 0 {
            let old = self.src;
            self.src = (s.cx, s.cy);
            self.crop = scale::crop(self.src, self.frame_size(), self.borders);
            trace(format!(
                "scale: source size changed {}x{} -> {}x{} ({}), crop {}",
                old.0,
                old.1,
                s.cx,
                s.cy,
                if winops::is_minimized(self.hwnd) { "window minimized" } else { "window shown" },
                crop_text(self.crop)
            ));
        }
    }

    fn stats(&self) -> String {
        format!("{} frames, worst gap {:.1} ms", self.frames, self.worst_gap)
    }
}

fn crop_text(c: Crop) -> String {
    format!("{},{},{},{} {}", c.l, c.t, c.r, c.b, if c.measured { "measured" } else { "derived" })
}

pub struct Scale {
    /// The user has Windows' minimize animation on.
    pub enabled: bool,
    /// Windows' animation is off for the session (we play every minimize).
    session: bool,
    top: i32,
    dock: HWND,
    overlay: Option<Overlay>,
    runs: Vec<Run>,
    /// A thumbnail registered on a title-bar button press, hidden until `fire`.
    armed: Option<Run>,
}

impl Scale {
    /// `active`: Scale is the chosen effect; otherwise this stays idle.
    pub fn new(dock: HWND, active: bool) -> Scale {
        let enabled = active && genie::spi_min_animate().unwrap_or(false);
        let session = enabled && genie::suppress();
        if active {
            log(format!(
                "minimize effect: scale, {}",
                match (enabled, session) {
                    (false, _) => "off (Windows minimize animation is off)",
                    (true, true) => "on, Windows animation off for the session",
                    (true, false) => "on for the Dock only (Windows animation could not be turned off)",
                }
            ));
        }
        Scale { enabled, session, top: 0, dock, overlay: None, runs: Vec::new(), armed: None }
    }

    /// One line for the report.
    pub fn describe(&self) -> String {
        format!(
            "scale (DWM thumbnails): {}, Windows animation off for the session {}, overlay {}",
            if self.enabled { "on" } else { "off" },
            self.session,
            if self.overlay.is_some() { "ready" } else { "not created" }
        )
    }

    pub fn set_top(&mut self, px: i32) {
        self.top = px;
    }

    /// Title-bar minimizes play the effect too.
    pub fn session(&self) -> bool {
        self.session
    }

    pub fn shutdown(&mut self) {
        self.armed = None;
        self.runs.clear();
        self.overlay = None;
        genie::release_min_animate();
    }

    pub fn animating(&self) -> bool {
        !self.runs.is_empty()
    }

    pub fn busy(&self, h: HWND) -> bool {
        self.runs.iter().any(|r| r.hwnd == h)
    }

    fn room(&self) -> usize {
        MAX_RUNS.saturating_sub(self.runs.len())
    }

    /// Creates the overlay ahead of the first animation.
    pub fn prepare(&mut self, screen: (i32, i32)) {
        if self.enabled && !self.overlay.as_ref().is_some_and(|o| o.shown) {
            self.ensure_overlay(screen);
        }
    }

    fn ensure_overlay(&mut self, screen: (i32, i32)) -> bool {
        if self.overlay.as_ref().is_none_or(|o| (o.w, o.h, o.top) != (screen.0, screen.1, self.top)) {
            if self.overlay.as_ref().is_some_and(|o| o.shown) {
                // Never under a running animation: the next one gets it.
                return true;
            }
            self.armed = None;
            self.overlay = None;
            match Overlay::new(screen.0, screen.1, self.top) {
                Ok(o) => self.overlay = Some(o),
                Err(e) => trace(format!("scale: no overlay: {e}")),
            }
        }
        self.overlay.is_some()
    }

    /// Registers the thumbnail of `it` and places it at the start of `dir`.
    fn start(&mut self, it: Item, dir: Dir, via: &'static str, visible: bool) -> Result<Run, String> {
        let ov = self.overlay.as_ref().ok_or("no overlay")?;
        let thumb = unsafe { DwmRegisterThumbnail(ov.hwnd, it.hwnd) }
            .map(Thumb)
            .map_err(|e| format!("DwmRegisterThumbnail failed ({})", hr(&e)))?;
        let s = unsafe { DwmQueryThumbnailSourceSize(thumb.0) }
            .map_err(|e| format!("DwmQueryThumbnailSourceSize failed ({})", hr(&e)))?;
        let frame = (it.path.win.w.round() as i32, it.path.win.h.round() as i32);
        // A minimized window DWM kept no picture of (160x28 or less): nothing
        // to grow out of the icon.
        if s.cx < frame.0 / 2 || s.cy < frame.1 / 2 {
            return Err(format!("source {}x{} for a {}x{} frame, no picture", s.cx, s.cy, frame.0, frame.1));
        }
        let crop = scale::crop((s.cx, s.cy), frame, it.borders);
        let now = Instant::now();
        let mut run = Run {
            hwnd: it.hwnd,
            thumb,
            dir,
            path: it.path,
            borders: it.borders,
            src: (s.cx, s.cy),
            crop,
            visible,
            start: now,
            waiting: None,
            via,
            activate: false,
            shown_at: None,
            seen: 0,
            frames: 0,
            last: now,
            worst_gap: 0.0,
            failed: false,
        };
        run.place(ov, scale::progress(dir, 0.0));
        if run.failed {
            return Err("first DwmUpdateThumbnailProperties failed".into());
        }
        Ok(run)
    }

    fn first_frame_trace(r: &Run, what: &str, since: Instant) {
        let shown = !winops::is_minimized(r.hwnd) && unsafe { IsWindowVisible(r.hwnd) }.as_bool();
        trace(format!(
            "scale {what} via {}: first thumbnail frame {:.1} ms after the trigger, window {}, source {}x{}, frame {}x{}, crop {}",
            r.via,
            ms(since),
            match (what, shown) {
                ("minimize", true) => "still shown (no blink)",
                ("minimize", false) => "already hidden (blink)",
                (_, true) => "shown",
                (_, false) => "minimized",
            },
            r.src.0,
            r.src.1,
            r.frame_size().0,
            r.frame_size().1,
            crop_text(r.crop)
        ));
    }

    /// Minimizes windows into their Dock icon. `already`: Windows has
    /// minimized them (the minimize event); otherwise they are minimized
    /// here once the thumbnails cover them. Windows the effect cannot take
    /// are minimized plainly. `since`: when the trigger was handled.
    pub fn minimize(&mut self, items: Vec<Item>, already: bool, screen: (i32, i32), via: &'static str, since: Instant) {
        self.disarm();
        let mut plain = Vec::new();
        let mut started = Vec::new();
        let ok = self.ensure_overlay(screen);
        for it in items {
            let h = it.hwnd;
            if !ok || started.len() >= self.room() {
                plain.push(h);
                continue;
            }
            self.runs.retain(|r| r.hwnd != h);
            match self.start(it, Dir::Minimize, via, true) {
                Ok(r) => started.push(r),
                Err(e) => {
                    trace(format!("scale minimize via {via}: {e}, minimized without the effect"));
                    plain.push(h);
                }
            }
        }
        if !started.is_empty() {
            if let Some(ov) = &mut self.overlay {
                ov.show(self.dock);
            }
            // The window goes only after a composition with its thumbnail on top.
            unsafe {
                let _ = DwmFlush();
            }
            for r in &mut started {
                Self::first_frame_trace(r, "minimize", since);
                r.start = Instant::now();
                r.last = r.start;
                if !already {
                    winops::minimize(r.hwnd);
                    r.waiting = Some(r.start);
                }
            }
            self.runs.extend(started);
        }
        if !already {
            for h in plain {
                winops::minimize(h);
            }
        }
    }

    /// Restores `it` out of its Dock icon; false if the effect cannot play
    /// (the caller restores it plainly). `activate` brings it to the front.
    pub fn restore(&mut self, it: Item, activate: bool, screen: (i32, i32), since: Instant) -> bool {
        self.disarm();
        if self.room() == 0 || !self.ensure_overlay(screen) {
            return false;
        }
        let h = it.hwnd;
        self.runs.retain(|r| r.hwnd != h);
        let mut run = match self.start(it, Dir::Restore, "dock", true) {
            Ok(r) => r,
            Err(e) => {
                trace(format!("scale restore: {e}, restored without the effect"));
                return false;
            }
        };
        run.activate = activate;
        if let Some(ov) = &mut self.overlay {
            ov.show(self.dock);
        }
        unsafe {
            let _ = DwmFlush();
        }
        Self::first_frame_trace(&run, "restore", since);
        run.start = Instant::now();
        run.last = run.start;
        self.runs.push(run);
        true
    }

    /// Registers the thumbnail of `it` hidden, for `fire`: the user pressed
    /// a title-bar minimize button and the app minimizes on the release.
    pub fn arm(&mut self, it: Item, screen: (i32, i32)) -> bool {
        self.disarm();
        if self.room() == 0 || !self.ensure_overlay(screen) {
            return false;
        }
        match self.start(it, Dir::Minimize, "title-bar button", false) {
            Ok(r) => {
                self.armed = Some(r);
                true
            }
            Err(e) => {
                trace(format!("scale: minimize button pressed, {e}"));
                false
            }
        }
    }

    pub fn armed(&self) -> Option<HWND> {
        self.armed.as_ref().map(|r| r.hwnd)
    }

    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// Shows the armed thumbnail over `h` and waits until DWM has composed
    /// it; the window minimizes under it. `since`: when the release or the
    /// event was handled.
    pub fn fire(&mut self, h: HWND, via: &'static str, since: Instant) -> bool {
        let Some(mut run) = self.armed.take_if(|r| r.hwnd == h) else { return false };
        let Some(ov) = &mut self.overlay else { return false };
        run.via = via;
        run.visible = true;
        // Placed again: the window may have moved since the press.
        if let Some(f) = winops::frame_rect(h).filter(|_| !winops::is_minimized(h)) {
            run.path.win = Rect { x: f.x as f32, y: f.y as f32, w: f.w as f32, h: f.h as f32 };
        }
        run.place(ov, 0.0);
        ov.show(self.dock);
        unsafe {
            let _ = DwmFlush();
        }
        Self::first_frame_trace(&run, "minimize", since);
        run.start = Instant::now();
        run.last = run.start;
        if !winops::is_minimized(h) {
            run.waiting = Some(run.start);
        }
        self.runs.retain(|r| r.hwnd != h);
        self.runs.push(run);
        true
    }

    /// The window came back by other means (Alt+Tab, the app itself).
    pub fn restored_elsewhere(&mut self, h: HWND) {
        let before = self.runs.len();
        self.runs.retain(|r| !(r.hwnd == h && r.dir == Dir::Minimize && r.waiting.is_none()));
        if self.runs.len() != before {
            trace("scale: window restored elsewhere during its minimize, thumbnail dropped");
        }
    }

    /// One frame of every running animation; call at display refresh while
    /// `animating()`.
    pub fn step(&mut self) {
        if self.runs.is_empty() {
            self.finish();
            return;
        }
        let Some(ov) = &self.overlay else {
            for r in std::mem::take(&mut self.runs) {
                if r.dir == Dir::Restore {
                    winops::activate(r.hwnd);
                }
            }
            return;
        };
        let now = Instant::now();
        let mut gone = Vec::new();
        for (i, r) in self.runs.iter_mut().enumerate() {
            if !unsafe { IsWindow(Some(r.hwnd)) }.as_bool() {
                trace(format!("scale: window closed during its {:?}", r.dir));
                gone.push(i);
                continue;
            }
            r.frames += 1;
            r.worst_gap = r.worst_gap.max((now - r.last).as_secs_f32() * 1000.0);
            r.last = now;
            r.refresh_source();
            if let Some(w) = r.waiting {
                let waited = w.elapsed().as_secs_f32();
                if winops::is_minimized(r.hwnd) {
                    r.waiting = None;
                    r.start = now;
                    trace(format!("scale: window minimized {:.0} ms after its thumbnail covered it ({})", waited * 1000.0, r.via));
                } else if waited > WAIT_S {
                    trace(format!("scale: window not minimized {:.0} ms after the thumbnail ({}), dropped", waited * 1000.0, r.via));
                    gone.push(i);
                    continue;
                }
                // Kept in place over the window; the crop may have changed.
                r.place(ov, 0.0);
                continue;
            }
            let t = (now - r.start).as_secs_f32();
            match (r.dir, r.shown_at) {
                (Dir::Minimize, _) if scale::finished(t) => {
                    trace(format!("scale minimize end via {}: {:.0} ms, {}", r.via, t * 1000.0, r.stats()));
                    gone.push(i);
                    continue;
                }
                (Dir::Restore, None) if scale::finished(t) => {
                    r.place(ov, 0.0);
                    if r.activate {
                        winops::activate(r.hwnd);
                    } else {
                        winops::restore(r.hwnd);
                    }
                    r.shown_at = Some(now);
                    trace(format!("scale restore: window restore sent at {:.0} ms", t * 1000.0));
                    continue;
                }
                (Dir::Restore, Some(s)) => {
                    let e = s.elapsed().as_secs_f32();
                    if !winops::is_minimized(r.hwnd) {
                        r.seen += 1;
                    }
                    if r.seen >= UNCOVER_FRAMES {
                        // The predicted rect against where the window came back.
                        let off = winops::frame_rect(r.hwnd).map(|f| {
                            let w = r.path.win;
                            (f.x - w.x as i32, f.y - w.y as i32, f.w - w.w as i32, f.h - w.h as i32)
                        });
                        trace(format!(
                            "scale restore end: uncovered after {:.0} ms, window frame off by {}, {}",
                            e * 1000.0,
                            off.map(|o| format!("{},{} size {},{}", o.0, o.1, o.2, o.3)).unwrap_or_else(|| "?".into()),
                            r.stats()
                        ));
                        gone.push(i);
                    } else if e > UNCOVER_MAX_S {
                        trace(format!("scale restore end: window not seen shown after {:.0} ms, gave up, {}", e * 1000.0, r.stats()));
                        gone.push(i);
                    }
                    continue;
                }
                _ => {}
            }
            r.place(ov, scale::progress(r.dir, t));
        }
        for i in gone.into_iter().rev() {
            // Dropping the run unregisters its thumbnail.
            self.runs.remove(i);
        }
        if self.runs.is_empty() {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if let Some(o) = &mut self.overlay {
            o.hide();
        }
    }
}
