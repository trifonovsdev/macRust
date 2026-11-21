//! Minimize into a Dock icon and restore out of it with the macOS Scale
//! effect (`macrust_ui::scale`). DWM draws it: a thumbnail of the window
//! (DwmRegisterThumbnail) in a click-through overlay just below the Dock is
//! placed exactly over the window, and its destination rect is moved toward
//! the icon once per display refresh. No pixels pass through the CPU and the
//! content is the window's own, so there is nothing to capture, cache or
//! match, and every minimize and restore we start plays the same way.
//!
//! Thread: the overlay and every thumbnail live on the "scale" thread
//! (`Host`), which does nothing else, so the menu bar, the Dock's glass and
//! the window tracker on the UI thread never delay a frame. A window's
//! messages go to the thread that created it and DwmRegisterThumbnail only
//! asks that the destination window belong to the calling process, so the
//! thread owns both ends. The UI thread sends commands (`Cmd`) through a
//! channel and an event; `arm` and `fire` wait for the answer because the
//! mouse hook holds the release until the thumbnail is on screen. The UI
//! thread never sends a message to the scale thread's window, nor the other
//! way round, so the waits cannot deadlock; the scale thread only posts
//! `WM_APP_SCALE` back to have a restored window activated, as foreground
//! rights belong to the thread that took the click.
//!
//! Pacing: after each update the thread waits in DwmFlush, which returns
//! once DWM has presented the composition that took it, so every update
//! lands in the next composition. Each frame's progress is computed for the
//! vblank it will be shown at (DwmGetCompositionTimingInfo: qpcVBlank,
//! qpcRefreshPeriod), not for the moment it is computed: a late wake-up
//! skips a vblank, which is counted, and the motion stays on time. When
//! DwmFlush returns at once (nothing to compose) the thread sleeps on a
//! high-resolution timer to just past the next vblank. The compositor clock
//! (DCompositionWaitForCompositorClock, build 22000+) is not used: it wakes
//! at the start of DWM's cycle, racing the composition for the update, and
//! its documented return values disagree with Microsoft's own sample.
//!
//! Minimize: the thumbnail covers the window first (DwmFlush waits for that
//! composition), then the window minimizes under it. A title-bar button is
//! seen by the mouse hook (`minbutton.rs`): the thumbnail is registered on
//! the press and shown on the release, before the app gets it. Win+Down and
//! app-initiated minimizes are seen through HSHELL_GETMINRECT (before the
//! window hides, where Windows sends it) or EVENT_SYSTEM_MINIMIZESTART
//! (after: the desktop shows for the event's latency; the trace says so).
//! Restore: the thumbnail of the minimized window grows out of the icon,
//! the window is restored under its last frame, the thumbnail moves onto
//! the window's real frame once it is seen shown, and goes a frame later
//! (both show the same live surface by then).
//!
//! DWM keeps the last picture of a minimized window for its thumbnail, as it
//! does for taskbar previews (Microsoft Q&A 106803, moderator answer); that
//! picture was reported to include the window's shadow on Windows 10 21H1
//! (Q&A 725780), so its size can differ from the window's. The crop to the
//! visible frame is measured on the shown window (its invisible borders);
//! when the source changes size it is mapped (`scale::remap`) so the frame
//! keeps its size and stays centred across and top-anchored down, and it is
//! measured again whenever the source is the shown window. Where DWM puts
//! the extra pixels is not documented; the trace logs both sizes and crops.
//!
//! Windows' own minimize animation (SPI_SETANIMATION) is off for the whole
//! session, as with the Genie (`genie::suppress`), and nothing here turns it
//! back on.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use macrust_ui::scale::{self, Crop, Dir, Pacing, Path, Rect};
use windows::Win32::Foundation::{CloseHandle, COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TIMING_INFO, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_VISIBLE,
    DwmFlush, DwmGetCompositionTimingInfo, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, RDW_ERASE, RDW_ERASENOW, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, GetCurrentThread, INFINITE, SetEvent,
    SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect, IsWindow, IsWindowVisible, LWA_ALPHA,
    LWA_COLORKEY, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT,
    RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetWindowPos, ShowWindow, TranslateMessage, WM_APP, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::genie;
use super::util::{hwnd_from_isize, hwnd_to_isize, log, trace};
use super::winops;

/// Posted by the scale thread to the UI thread's window: WPARAM is a window
/// to activate (its restore has played).
pub const WM_APP_SCALE: u32 = WM_APP + 19;

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
/// The UI thread waits at most this long for `arm`. The scale thread
/// answers within a frame or two (it may be inside DwmFlush).
const ARM_WAIT: Duration = Duration::from_millis(100);
/// `fire` runs while the mouse hook holds the release (minbutton.rs gives
/// it 60 ms in all).
const FIRE_WAIT: Duration = Duration::from_millis(50);
/// winnt.h TIMER_ALL_ACCESS.
const TIMER_ALL_ACCESS: u32 = 0x001F_0003;

fn hr(e: &windows::core::Error) -> String {
    format!("{:#010x}", e.code().0)
}

fn ms(since: Instant) -> f32 {
    since.elapsed().as_secs_f32() * 1000.0
}

/// A kernel handle shared between the threads; closed when the last owner drops it.
struct Handle(HANDLE);

// SAFETY: event and timer handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
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

/// Created, used and destroyed on the scale thread only.
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
// The display clock (scale thread)

struct Clock {
    freq: i64,
    timer: Option<Handle>,
    /// The last good vblank grid (a vblank, the period), for when DWM gives none.
    grid: (i64, i64),
    timing_failed: bool,
    /// DwmFlush calls that returned without waiting for a composition.
    early: u32,
}

impl Clock {
    fn new() -> Clock {
        let mut freq = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        let freq = freq.max(1);
        // Windows 10 1803+; a plain timer has the system tick's resolution.
        let timer = unsafe { CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS) }
            .or_else(|_| unsafe { CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS) })
            .ok()
            .map(Handle);
        Clock { freq, timer, grid: (0, freq / 60), timing_failed: false, early: 0 }
    }

    fn now(&self) -> i64 {
        let mut t = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut t);
        }
        t
    }

    fn ms(&self, ticks: i64) -> f32 {
        ticks as f32 * 1000.0 / self.freq as f32
    }

    fn secs(&self, ticks: i64) -> f32 {
        ticks as f32 / self.freq as f32
    }

    /// A vblank and the refresh period, from DWM.
    fn vblanks(&mut self) -> (i64, i64) {
        let mut ti = DWM_TIMING_INFO { cbSize: size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        // Since Windows 8.1 the window must be null.
        match unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut ti) } {
            Ok(()) if ti.qpcRefreshPeriod > 0 && ti.qpcVBlank > 0 => {
                self.grid = (ti.qpcVBlank as i64, ti.qpcRefreshPeriod as i64);
            }
            r => {
                if !self.timing_failed {
                    self.timing_failed = true;
                    let why = r.err().map(|e| hr(&e)).unwrap_or_else(|| "no vblank".into());
                    trace(format!("scale: DwmGetCompositionTimingInfo failed ({why}), pacing on the last known grid"));
                }
                if self.grid.0 == 0 {
                    self.grid.0 = self.now();
                }
            }
        }
        self.grid
    }

    /// The vblank an update made now is shown at (in our convention: the
    /// next one; the composition's own latency is the same every frame),
    /// and the period. An update in the last moments before a vblank misses
    /// that composition, so it counts for the vblank after.
    fn target(&mut self) -> (i64, i64) {
        let (vb, period) = self.vblanks();
        let guard = (self.freq / 500).min(period / 4);
        (scale::vblank_after(self.now() + guard, vb, period), period)
    }

    /// Waits for the composition that takes the last update; past the next
    /// vblank when there is none, or until `wake` is set.
    fn wait_frame(&mut self, wake: &Handle) {
        let t0 = self.now();
        if unsafe { DwmFlush() }.is_ok() && self.now() - t0 >= self.freq / 1000 {
            return;
        }
        self.early += 1;
        let (vb, period) = self.vblanks();
        let now = self.now();
        let due = scale::vblank_after(now, vb, period) + self.freq / 1000;
        let wait = due - now;
        let wait_ms = (self.ms(wait).ceil() as u32).clamp(1, 50);
        unsafe {
            match &self.timer {
                Some(t) => {
                    // Relative due time in 100 ns units.
                    let rel = -(wait as i128 * 10_000_000 / self.freq as i128) as i64;
                    if SetWaitableTimer(t.0, &rel, 0, None, None, false).is_ok() {
                        let _ = MsgWaitForMultipleObjectsEx(Some(&[wake.0, t.0]), wait_ms + 5, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                    } else {
                        let _ = MsgWaitForMultipleObjectsEx(Some(&[wake.0]), wait_ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                    }
                }
                None => {
                    let _ = MsgWaitForMultipleObjectsEx(Some(&[wake.0]), wait_ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runs (scale thread)

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

/// The shown window's size and its visible frame inside it, measured.
fn live_layout(h: HWND) -> Option<((i32, i32), Crop)> {
    if winops::is_minimized(h) || !unsafe { IsWindowVisible(h) }.as_bool() {
        return None;
    }
    let mut wr = RECT::default();
    unsafe { GetWindowRect(h, &mut wr) }.ok()?;
    let (l, t, r, b) = winops::borders(h);
    Some(((wr.right - wr.left, wr.bottom - wr.top), Crop { l, t, r, b, measured: true }))
}

struct Run {
    hwnd: HWND,
    thumb: Thumb,
    dir: Dir,
    path: Path,
    /// The thumbnail source's size and the part of it that is the frame.
    src: (i32, i32),
    crop: Crop,
    visible: bool,
    /// Set once the motion's clock runs (the first, still frame is placed).
    pace: Option<Pacing>,
    /// Minimize: shown over the window, which has not minimized yet; the
    /// clock starts when it has.
    waiting: Option<Instant>,
    /// What started it, for the trace.
    via: &'static str,
    /// Restore: bring the window to the front when it is back.
    activate: bool,
    /// Restore: when the window was restored under the last frame.
    shown_at: Option<Instant>,
    /// Restore: the last frame's rect, before it moved onto the window.
    last_frame: Option<RECT>,
    /// Restore: frames the window has been seen shown since.
    seen: u32,
    /// The UI thread's command reached this thread after this long.
    latency_ms: f32,
    /// Performance-counter time of the last step, for the worst gap.
    last_step: i64,
    worst_gap: i64,
    early_at_start: u32,
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
        let dest = ov.dest(scale::frame(&self.path, e));
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
    /// minimized may be kept with its shadow; a restored one is live again)
    /// so the frame shown stays the same.
    fn refresh_source(&mut self) {
        let Ok(s) = (unsafe { DwmQueryThumbnailSourceSize(self.thumb.0) }) else { return };
        let new = (s.cx, s.cy);
        if new == self.src || s.cx <= 0 || s.cy <= 0 {
            return;
        }
        let (old, was) = (self.src, self.crop);
        let (crop, how) = match live_layout(self.hwnd) {
            Some((size, c)) if size == new => (c, "measured on the shown window"),
            _ => (scale::remap(self.crop, self.src, new), "mapped, frame size kept"),
        };
        self.src = new;
        self.crop = crop;
        trace(format!(
            "scale: source size changed {}x{} -> {}x{} ({}), crop {} -> {} ({how}), frame in source {}x{} -> {}x{}",
            old.0,
            old.1,
            s.cx,
            s.cy,
            if winops::is_minimized(self.hwnd) { "window minimized" } else { "window shown" },
            crop_text(was),
            crop_text(crop),
            old.0 - was.l - was.r,
            old.1 - was.t - was.b,
            new.0 - crop.l - crop.r,
            new.1 - crop.t - crop.b,
        ));
    }

    /// Starts the motion's clock: the still frame was shown at `start`.
    fn begin(&mut self, start: i64, now: i64, early: u32) {
        self.pace = Some(Pacing::new(start));
        self.last_step = now;
        self.worst_gap = 0;
        self.early_at_start = early;
    }

    fn stats(&self, clock: &Clock, period: i64) -> String {
        let (shown, missed) = self.pace.map(|p| (p.shown, p.missed)).unwrap_or((0, 0));
        let period_s = clock.secs(period);
        // A restore also shows its end frame (progress 0) at DURATION_S; a
        // minimize ends by taking the thumbnail away.
        let expected = scale::expected_frames(period_s) + u32::from(self.dir == Dir::Restore);
        format!(
            "{shown}/{} frames at {:.1} Hz, {missed} missed vblanks, worst gap {:.1} ms, {} early DwmFlush returns, command latency {:.1} ms",
            expected,
            if period_s > 0.0 { 1.0 / period_s } else { 0.0 },
            clock.ms(self.worst_gap),
            clock.early - self.early_at_start,
            self.latency_ms
        )
    }
}

fn crop_text(c: Crop) -> String {
    format!("{},{},{},{} {}", c.l, c.t, c.r, c.b, if c.measured { "measured" } else { "derived" })
}

fn rect_text(r: RECT) -> String {
    format!("{},{} {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top)
}

// ---------------------------------------------------------------------------
// Commands from the UI thread

enum Cmd {
    Prepare,
    Arm { it: Item, reply: SyncSender<bool> },
    Fire { h: HWND, via: &'static str, since: Instant, reply: SyncSender<bool> },
    Disarm,
    Minimize { items: Vec<Item>, already: bool, via: &'static str, since: Instant },
    Restore { it: Item, activate: bool, since: Instant },
    RestoredElsewhere(HWND),
    Shutdown,
}

struct Msg {
    cmd: Cmd,
    screen: (i32, i32),
    top: i32,
    sent: Instant,
}

// SAFETY: the HWNDs inside are process-independent window handles, valid to
// pass to any thread; nothing in a command is tied to the sending thread.
unsafe impl Send for Msg {}

/// What the UI thread may ask without waiting: published by the scale thread.
#[derive(Default)]
struct Shared {
    runs: Vec<isize>,
    /// Windows the UI thread has sent a minimize or restore for that the
    /// scale thread has not taken yet.
    pending: Vec<isize>,
    armed: Option<isize>,
    overlay: bool,
}

fn lock(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------------------
// The scale thread

struct Host {
    dock: HWND,
    notify: HWND,
    shared: Arc<Mutex<Shared>>,
    clock: Clock,
    overlay: Option<Overlay>,
    runs: Vec<Run>,
    /// A thumbnail registered on a title-bar button press, hidden until `fire`.
    armed: Option<Run>,
    /// Windows whose queued minimize or restore was taken, for `publish`.
    taken: Vec<isize>,
    /// The vblank the last step was for.
    last_target: i64,
    period: i64,
}

impl Host {
    /// Under one lock, so a window is never seen neither pending nor running.
    fn publish(&mut self) {
        let mut s = lock(&self.shared);
        for h in self.taken.drain(..) {
            if let Some(i) = s.pending.iter().position(|&p| p == h) {
                s.pending.swap_remove(i);
            }
        }
        s.runs = self.runs.iter().map(|r| hwnd_to_isize(r.hwnd)).collect();
        s.armed = self.armed.as_ref().map(|r| hwnd_to_isize(r.hwnd));
        s.overlay = self.overlay.is_some();
    }

    /// Brings a restored window to the front on the UI thread.
    fn activate(&self, h: HWND) {
        if unsafe { PostMessageW(Some(self.notify), WM_APP_SCALE, WPARAM(hwnd_to_isize(h) as usize), LPARAM(0)) }.is_err() {
            winops::activate(h);
        }
    }

    fn room(&self) -> usize {
        MAX_RUNS.saturating_sub(self.runs.len())
    }

    fn ensure_overlay(&mut self, screen: (i32, i32), top: i32) -> bool {
        if self.overlay.as_ref().is_none_or(|o| (o.w, o.h, o.top) != (screen.0, screen.1, top.clamp(0, screen.1 - 1))) {
            if self.overlay.as_ref().is_some_and(|o| o.shown) {
                // Never under a running animation: the next one gets it.
                return true;
            }
            self.armed = None;
            self.overlay = None;
            match Overlay::new(screen.0, screen.1, top) {
                Ok(o) => self.overlay = Some(o),
                Err(e) => trace(format!("scale: no overlay: {e}")),
            }
        }
        self.overlay.is_some()
    }

    /// Returns false on Shutdown.
    fn handle(&mut self, m: Msg) -> bool {
        let latency = ms(m.sent);
        match m.cmd {
            Cmd::Prepare => {
                if !self.overlay.as_ref().is_some_and(|o| o.shown) {
                    self.ensure_overlay(m.screen, m.top);
                }
            }
            Cmd::Arm { it, reply } => {
                let ok = self.arm(it, m.screen, m.top, latency);
                self.publish();
                let _ = reply.send(ok);
            }
            Cmd::Fire { h, via, since, reply } => {
                let ok = self.fire(h, via, since, latency);
                self.publish();
                let _ = reply.send(ok);
            }
            Cmd::Disarm => self.armed = None,
            Cmd::Minimize { items, already, via, since } => {
                self.taken.extend(items.iter().map(|it| hwnd_to_isize(it.hwnd)));
                self.minimize(items, already, m.screen, m.top, via, since, latency);
            }
            Cmd::Restore { it, activate, since } => {
                let h = it.hwnd;
                self.taken.push(hwnd_to_isize(h));
                if !self.restore(it, activate, m.screen, m.top, since, latency) {
                    if activate {
                        self.activate(h);
                    } else {
                        winops::restore(h);
                    }
                }
            }
            Cmd::RestoredElsewhere(h) => {
                let before = self.runs.len();
                self.runs.retain(|r| !(r.hwnd == h && r.dir == Dir::Minimize && r.waiting.is_none()));
                if self.runs.len() != before {
                    trace("scale: window restored elsewhere during its minimize, thumbnail dropped");
                }
            }
            Cmd::Shutdown => return false,
        }
        self.publish();
        true
    }

    /// Registers the thumbnail of `it` and places it at the start of `dir`.
    fn start(&mut self, it: Item, dir: Dir, via: &'static str, visible: bool, latency_ms: f32) -> Result<Run, String> {
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
        let src = (s.cx, s.cy);
        let crop = match (live_layout(it.hwnd), it.borders) {
            (Some((size, c)), _) if size == src => c,
            // A kept picture of another size than the window had: the same
            // mapping as when a shown window's source changes on minimize.
            (_, Some((l, t, r, b))) if l >= 0 && t >= 0 && r >= 0 && b >= 0 && (frame.0 + l + r, frame.1 + t + b) != src => {
                scale::remap(Crop { l, t, r, b, measured: true }, (frame.0 + l + r, frame.1 + t + b), src)
            }
            _ => scale::crop(src, frame, it.borders),
        };
        let mut run = Run {
            hwnd: it.hwnd,
            thumb,
            dir,
            path: it.path,
            src,
            crop,
            visible,
            pace: None,
            waiting: None,
            via,
            activate: false,
            shown_at: None,
            last_frame: None,
            seen: 0,
            latency_ms,
            last_step: 0,
            worst_gap: 0,
            early_at_start: 0,
            failed: false,
        };
        run.place(ov, scale::progress(dir, 0.0));
        if run.failed {
            return Err("first DwmUpdateThumbnailProperties failed".into());
        }
        Ok(run)
    }

    /// After the DwmFlush that showed the first frames: their clock starts
    /// at the vblank that frame took.
    fn begin_now(&mut self, r: &mut Run) {
        let (target, period) = self.clock.target();
        let now = self.clock.now();
        self.period = period;
        r.begin(target - period, now, self.clock.early);
    }

    fn first_frame_trace(r: &Run, what: &str, since: Instant) {
        let shown = !winops::is_minimized(r.hwnd) && unsafe { IsWindowVisible(r.hwnd) }.as_bool();
        trace(format!(
            "scale {what} via {}: first thumbnail frame {:.1} ms after the trigger (command latency {:.1} ms), window {}, source {}x{}, frame {}x{}, crop {}",
            r.via,
            ms(since),
            r.latency_ms,
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

    #[allow(clippy::too_many_arguments)]
    fn minimize(
        &mut self,
        items: Vec<Item>,
        already: bool,
        screen: (i32, i32),
        top: i32,
        via: &'static str,
        since: Instant,
        latency: f32,
    ) {
        self.armed = None;
        let mut plain = Vec::new();
        let mut started = Vec::new();
        let ok = self.ensure_overlay(screen, top);
        for it in items {
            let h = it.hwnd;
            if !ok || started.len() >= self.room() {
                plain.push(h);
                continue;
            }
            self.runs.retain(|r| r.hwnd != h);
            match self.start(it, Dir::Minimize, via, true, latency) {
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
            for mut r in started {
                Self::first_frame_trace(&r, "minimize", since);
                if already {
                    self.begin_now(&mut r);
                } else {
                    winops::minimize(r.hwnd);
                    r.waiting = Some(Instant::now());
                }
                self.runs.push(r);
            }
        }
        if !already {
            for h in plain {
                winops::minimize(h);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn restore(&mut self, it: Item, activate: bool, screen: (i32, i32), top: i32, since: Instant, latency: f32) -> bool {
        self.armed = None;
        if self.room() == 0 || !self.ensure_overlay(screen, top) {
            return false;
        }
        let h = it.hwnd;
        self.runs.retain(|r| r.hwnd != h);
        let mut run = match self.start(it, Dir::Restore, "dock", true, latency) {
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
        self.begin_now(&mut run);
        self.runs.push(run);
        true
    }

    fn arm(&mut self, it: Item, screen: (i32, i32), top: i32, latency: f32) -> bool {
        self.armed = None;
        if self.room() == 0 || !self.ensure_overlay(screen, top) {
            return false;
        }
        match self.start(it, Dir::Minimize, "title-bar button", false, latency) {
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

    fn fire(&mut self, h: HWND, via: &'static str, since: Instant, latency: f32) -> bool {
        let Some(mut run) = self.armed.take_if(|r| r.hwnd == h) else { return false };
        let Some(ov) = &mut self.overlay else { return false };
        run.via = via;
        run.visible = true;
        run.latency_ms = latency;
        // Placed again: the window may have moved since the press.
        if let Some(f) = winops::frame_rect(h).filter(|_| !winops::is_minimized(h)) {
            run.path.win = Rect { x: f.x as f32, y: f.y as f32, w: f.w as f32, h: f.h as f32 };
            if let Some((size, c)) = live_layout(h).filter(|(size, _)| *size == run.src) {
                run.src = size;
                run.crop = c;
            }
        }
        run.place(ov, 0.0);
        ov.show(self.dock);
        unsafe {
            let _ = DwmFlush();
        }
        Self::first_frame_trace(&run, "minimize", since);
        if winops::is_minimized(h) {
            self.begin_now(&mut run);
        } else {
            run.waiting = Some(Instant::now());
        }
        self.runs.retain(|r| r.hwnd != h);
        self.runs.push(run);
        true
    }

    /// One frame of every running animation, for the vblank it is shown at.
    fn step(&mut self) {
        let (target, period) = self.clock.target();
        if target <= self.last_target {
            // Woken again (a command) within the frame already placed.
            return;
        }
        self.last_target = target;
        self.period = period;
        let now = self.clock.now();
        let Some(ov) = &self.overlay else {
            for r in std::mem::take(&mut self.runs) {
                if r.dir == Dir::Restore {
                    if r.activate {
                        self.activate(r.hwnd);
                    } else {
                        winops::restore(r.hwnd);
                    }
                }
            }
            return;
        };
        let mut gone = Vec::new();
        let mut activate = Vec::new();
        for (i, r) in self.runs.iter_mut().enumerate() {
            if !unsafe { IsWindow(Some(r.hwnd)) }.as_bool() {
                trace(format!("scale: window closed during its {:?}", r.dir));
                gone.push(i);
                continue;
            }
            r.worst_gap = r.worst_gap.max(now - r.last_step);
            r.last_step = now;
            r.refresh_source();
            if let Some(w) = r.waiting {
                let waited = w.elapsed().as_secs_f32();
                if winops::is_minimized(r.hwnd) {
                    r.waiting = None;
                    r.begin(target, now, self.clock.early);
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
            if let Some(s) = r.shown_at {
                // Restore, at rest: the window is coming back under the last frame.
                let e = s.elapsed().as_secs_f32();
                let shown = !winops::is_minimized(r.hwnd) && unsafe { IsWindowVisible(r.hwnd) }.as_bool();
                if shown {
                    r.seen += 1;
                }
                if shown && r.seen == 1 {
                    // Onto the window's real frame, so taking the thumbnail
                    // away changes nothing on screen; any offset shows here
                    // either way and is traced.
                    if let Some(f) = winops::frame_rect(r.hwnd) {
                        r.path.win = Rect { x: f.x as f32, y: f.y as f32, w: f.w as f32, h: f.h as f32 };
                    }
                    r.place(ov, 0.0);
                } else if r.seen >= UNCOVER_FRAMES {
                    let last = r.last_frame.map(rect_text).unwrap_or_else(|| "?".into());
                    let off = match (r.last_frame, winops::frame_rect(r.hwnd)) {
                        (Some(l), Some(f)) => {
                            let l = RECT { left: l.left, top: l.top + ov.top, right: l.right, bottom: l.bottom + ov.top };
                            format!(
                                "{},{} size {},{}",
                                f.x - l.left,
                                f.y - l.top,
                                f.w - (l.right - l.left),
                                f.h - (l.bottom - l.top)
                            )
                        }
                        _ => "?".into(),
                    };
                    let borders = live_layout(r.hwnd).map(|(_, c)| crop_text(c)).unwrap_or_else(|| "?".into());
                    trace(format!(
                        "scale restore end: uncovered after {:.0} ms; last frame {last} (overlay px), window frame off by {off}, crop {} vs window borders {borders}; {}",
                        e * 1000.0,
                        crop_text(r.crop),
                        r.stats(&self.clock, period)
                    ));
                    gone.push(i);
                } else if e > UNCOVER_MAX_S {
                    trace(format!(
                        "scale restore end: window not seen shown after {:.0} ms, gave up; {}",
                        e * 1000.0,
                        r.stats(&self.clock, period)
                    ));
                    gone.push(i);
                }
                continue;
            }
            let Some(pace) = &mut r.pace else { continue };
            let Some(ticks) = pace.frame(target, period) else { continue };
            let t = self.clock.secs(ticks);
            match r.dir {
                Dir::Minimize if scale::finished(t) => {
                    trace(format!("scale minimize end via {}: {:.0} ms; {}", r.via, t * 1000.0, r.stats(&self.clock, period)));
                    gone.push(i);
                    continue;
                }
                Dir::Restore if scale::finished(t) => {
                    pace.shown += 1;
                    r.place(ov, 0.0);
                    r.last_frame = Some(ov.dest(r.path.win));
                    if r.activate {
                        activate.push(r.hwnd);
                    } else {
                        winops::restore(r.hwnd);
                    }
                    r.shown_at = Some(Instant::now());
                    trace(format!("scale restore: last frame at {:.0} ms, window restore sent", t * 1000.0));
                    continue;
                }
                _ => pace.shown += 1,
            }
            r.place(ov, scale::progress(r.dir, t));
        }
        for i in gone.into_iter().rev() {
            // Dropping the run unregisters its thumbnail.
            self.runs.remove(i);
        }
        for h in activate {
            self.activate(h);
        }
    }

    fn finish(&mut self) {
        if let Some(o) = &mut self.overlay {
            o.hide();
        }
    }

    fn run(mut self, rx: Receiver<Msg>, wake: Arc<Handle>) {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
        let mut msg = MSG::default();
        loop {
            // The overlay's messages (and any sent by other apps) arrive here.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            loop {
                match rx.try_recv() {
                    Ok(m) => {
                        if !self.handle(m) {
                            return self.stop();
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return self.stop(),
                }
            }
            if !self.runs.is_empty() {
                self.step();
            }
            self.publish();
            if self.runs.is_empty() {
                self.finish();
                unsafe {
                    let _ = MsgWaitForMultipleObjectsEx(Some(&[wake.0]), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            } else {
                self.clock.wait_frame(&wake);
            }
        }
    }

    /// Thumbnails go before the overlay they are drawn in, all on this thread.
    fn stop(&mut self) {
        self.armed = None;
        self.runs.clear();
        self.overlay = None;
        self.publish();
    }
}

// ---------------------------------------------------------------------------
// The UI thread's side

struct Link {
    tx: Sender<Msg>,
    wake: Arc<Handle>,
    thread: JoinHandle<()>,
}

pub struct Scale {
    /// The user has Windows' minimize animation on.
    pub enabled: bool,
    /// Windows' animation is off for the session (we play every minimize).
    session: bool,
    top: i32,
    link: Option<Link>,
    shared: Arc<Mutex<Shared>>,
}

impl Scale {
    /// `active`: Scale is the chosen effect; otherwise this stays idle.
    /// `notify`: the UI thread's window, for `WM_APP_SCALE`.
    pub fn new(dock: HWND, notify: HWND, active: bool) -> Scale {
        let mut enabled = active && genie::spi_min_animate().unwrap_or(false);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let link = if enabled {
            match Self::spawn(dock, notify, shared.clone()) {
                Ok(l) => Some(l),
                Err(e) => {
                    log(format!("minimize effect: scale thread failed to start ({e})"));
                    enabled = false;
                    None
                }
            }
        } else {
            None
        };
        let session = enabled && genie::suppress();
        if active {
            log(format!(
                "minimize effect: scale, {}",
                match (enabled, session) {
                    (false, _) => "off (Windows minimize animation is off, or no thread)",
                    (true, true) => "on, Windows animation off for the session",
                    (true, false) => "on for the Dock only (Windows animation could not be turned off)",
                }
            ));
        }
        Scale { enabled, session, top: 0, link, shared }
    }

    fn spawn(dock: HWND, notify: HWND, shared: Arc<Mutex<Shared>>) -> Result<Link, String> {
        let wake = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(|e| format!("CreateEvent failed ({})", hr(&e)))?;
        let wake = Arc::new(Handle(wake));
        let (tx, rx) = mpsc::channel::<Msg>();
        let (dock, notify) = (hwnd_to_isize(dock), hwnd_to_isize(notify));
        let w = wake.clone();
        let thread = std::thread::Builder::new()
            .name("scale".into())
            .spawn(move || {
                let host = Host {
                    dock: hwnd_from_isize(dock),
                    notify: hwnd_from_isize(notify),
                    shared,
                    clock: Clock::new(),
                    overlay: None,
                    runs: Vec::new(),
                    armed: None,
                    taken: Vec::new(),
                    last_target: 0,
                    period: 0,
                };
                host.run(rx, w);
            })
            .map_err(|e| e.to_string())?;
        Ok(Link { tx, wake, thread })
    }

    fn alive(&self) -> Option<&Link> {
        self.link.as_ref().filter(|l| !l.thread.is_finished())
    }

    fn send(&self, cmd: Cmd, screen: (i32, i32)) -> bool {
        let Some(l) = self.alive() else { return false };
        if l.tx.send(Msg { cmd, screen, top: self.top, sent: Instant::now() }).is_err() {
            return false;
        }
        unsafe { SetEvent(l.wake.0) }.is_ok()
    }

    /// One line for the report.
    pub fn describe(&self) -> String {
        format!(
            "scale (DWM thumbnails, own thread): {}, Windows animation off for the session {}, thread {}, overlay {}",
            if self.enabled { "on" } else { "off" },
            self.session,
            if self.alive().is_some() { "running" } else { "not running" },
            if lock(&self.shared).overlay { "ready" } else { "not created" }
        )
    }

    pub fn set_top(&mut self, px: i32) {
        self.top = px;
    }

    /// Title-bar minimizes play the effect too.
    pub fn session(&self) -> bool {
        self.session
    }

    /// Stops the thread (which unregisters the thumbnails and destroys the
    /// overlay) before Windows' animation is given back.
    pub fn shutdown(&mut self) {
        if let Some(l) = self.link.take() {
            let _ = l.tx.send(Msg { cmd: Cmd::Shutdown, screen: (0, 0), top: 0, sent: Instant::now() });
            unsafe {
                let _ = SetEvent(l.wake.0);
            }
            if l.thread.join().is_err() {
                log("minimize effect: scale thread ended with a panic");
            }
        }
        genie::release_min_animate();
    }

    /// A minimize or restore of `h` is playing or queued.
    pub fn busy(&self, h: HWND) -> bool {
        if self.alive().is_none() {
            return false;
        }
        let v = hwnd_to_isize(h);
        let s = lock(&self.shared);
        s.runs.contains(&v) || s.pending.contains(&v)
    }

    /// Creates the overlay ahead of the first animation.
    pub fn prepare(&mut self, screen: (i32, i32)) {
        if self.enabled {
            self.send(Cmd::Prepare, screen);
        }
    }

    /// Minimizes windows into their Dock icon. `already`: Windows has
    /// minimized them (the minimize event); otherwise they are minimized
    /// once the thumbnails cover them. Windows the effect cannot take are
    /// minimized plainly. `since`: when the trigger was handled.
    pub fn minimize(&mut self, items: Vec<Item>, already: bool, screen: (i32, i32), via: &'static str, since: Instant) {
        if items.is_empty() {
            return;
        }
        let hs: Vec<isize> = items.iter().map(|i| hwnd_to_isize(i.hwnd)).collect();
        lock(&self.shared).pending.extend(&hs);
        if !self.send(Cmd::Minimize { items, already, via, since }, screen) {
            lock(&self.shared).pending.retain(|p| !hs.contains(p));
            if !already {
                for h in hs {
                    winops::minimize(hwnd_from_isize(h));
                }
            }
        }
    }

    /// Restores `it` out of its Dock icon; false if the effect cannot be
    /// asked for (the caller restores it plainly). Once asked, the scale
    /// thread restores it plainly itself if the effect cannot play.
    /// `activate` brings it to the front.
    pub fn restore(&mut self, it: Item, activate: bool, screen: (i32, i32), since: Instant) -> bool {
        let h = hwnd_to_isize(it.hwnd);
        lock(&self.shared).pending.push(h);
        if self.send(Cmd::Restore { it, activate, since }, screen) {
            return true;
        }
        lock(&self.shared).pending.retain(|&p| p != h);
        false
    }

    /// Registers the thumbnail of `it` hidden, for `fire`: the user pressed
    /// a title-bar minimize button and the app minimizes on the release.
    pub fn arm(&mut self, it: Item, screen: (i32, i32)) -> bool {
        let (reply, answer) = mpsc::sync_channel(1);
        if !self.send(Cmd::Arm { it, reply }, screen) {
            return false;
        }
        match answer.recv_timeout(ARM_WAIT) {
            Ok(ok) => ok,
            Err(RecvTimeoutError::Timeout) => {
                trace(format!("scale: arm not answered in {} ms", ARM_WAIT.as_millis()));
                false
            }
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    pub fn armed(&self) -> Option<HWND> {
        self.alive()?;
        lock(&self.shared).armed.map(hwnd_from_isize)
    }

    pub fn disarm(&mut self) {
        if lock(&self.shared).armed.is_some() {
            self.send(Cmd::Disarm, (0, 0));
        }
    }

    /// Shows the armed thumbnail over `h` and returns once DWM has composed
    /// it; the window minimizes under it. `since`: when the release or the
    /// event was handled.
    pub fn fire(&mut self, h: HWND, via: &'static str, since: Instant) -> bool {
        let (reply, answer) = mpsc::sync_channel(1);
        if !self.send(Cmd::Fire { h, via, since, reply }, (0, 0)) {
            return false;
        }
        match answer.recv_timeout(FIRE_WAIT) {
            Ok(ok) => ok,
            Err(RecvTimeoutError::Timeout) => {
                // Still fired, only later than the release.
                trace(format!("scale: fire not answered in {} ms, the release goes on", FIRE_WAIT.as_millis()));
                true
            }
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    /// The window came back by other means (Alt+Tab, the app itself).
    pub fn restored_elsewhere(&mut self, h: HWND) {
        if self.busy(h) {
            self.send(Cmd::RestoredElsewhere(h), (0, 0));
        }
    }

    /// `WM_APP_SCALE` on the UI thread: a restore has played.
    pub fn on_message(&self, wp: WPARAM) {
        let h = hwnd_from_isize(wp.0 as isize);
        if unsafe { IsWindow(Some(h)) }.as_bool() {
            winops::activate(h);
        }
    }
}
