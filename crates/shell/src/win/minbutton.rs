//! Title-bar minimize buttons, seen before the app acts on them.
//!
//! Windows hides a window it minimizes at once, and every notice of it
//! (WinEvents, the shell hook while the minimize animation is off) comes
//! after the fact, so a Genie started from one shows the desktop for a frame
//! or more where the window was. A low-level mouse hook, called in this
//! process (nothing is loaded into other apps), sees the left button go down
//! and up before the window under it does. The shell checks on the press
//! whether it hit a minimize button and stages the Genie's first frame
//! (`WM_APP_PRESS`); on the release the hook waits until that frame is on
//! screen (`WM_APP_RELEASE`), and only then lets the app have the release
//! and minimize.
//!
//! The hook runs on its own thread so a busy UI thread never delays the
//! mouse; it waits for the UI thread only on a release while a frame is
//! staged, and never longer than `RELEASE_WAIT_MS`.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, MSG, MSLLHOOKSTRUCT, PostMessageW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, SetWindowsHookExW, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
};

use super::util::log;

/// The press of the left button: WPARAM x, LPARAM y (physical screen px).
pub const WM_APP_PRESS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 15;
/// Its release while a frame is staged, sent: the shell returns once the
/// frame is on screen.
pub const WM_APP_RELEASE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 16;

/// Well below the system's low-level hook timeout (LowLevelHooksTimeout,
/// 300 ms by default), past which Windows skips, then removes, the hook.
const RELEASE_WAIT_MS: u32 = 60;

static NOTIFY: AtomicIsize = AtomicIsize::new(0);
/// The window whose first frame is staged, 0 for none.
static ARMED: AtomicIsize = AtomicIsize::new(0);

pub fn start(notify: HWND) {
    NOTIFY.store(notify.0 as isize, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("minbutton".into()).spawn(|| unsafe {
        let Ok(_hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), None, 0) else {
            log("genie: no mouse hook, title-bar minimizes start after Windows hides the window");
            return;
        };
        // The hook is called from this thread's message loop; it lives as
        // long as the process.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
    });
    if spawned.is_err() {
        log("genie: mouse hook thread failed to start");
    }
}

pub fn set_armed(h: Option<HWND>) {
    ARMED.store(h.map_or(0, |h| h.0 as isize), Ordering::Relaxed);
}

unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        let notify = HWND(NOTIFY.load(Ordering::Relaxed) as *mut _);
        let (x, y) = (WPARAM(info.pt.x as isize as usize), LPARAM(info.pt.y as isize));
        match wp.0 as u32 {
            WM_LBUTTONDOWN => unsafe {
                let _ = PostMessageW(Some(notify), WM_APP_PRESS, x, y);
            },
            WM_LBUTTONUP if ARMED.load(Ordering::Relaxed) != 0 => unsafe {
                // The app gets the release (and minimizes) only after this.
                let _ = SendMessageTimeoutW(notify, WM_APP_RELEASE, x, y, SMTO_ABORTIFHUNG, RELEASE_WAIT_MS, None);
            },
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}
