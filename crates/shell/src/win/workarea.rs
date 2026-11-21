//! The strips the menu bar and the Dock occupy are kept out of the work area,
//! so maximized windows end under the menu bar and above the Dock.
//!
//! The app-bar route (system::appbar_place) depends on Explorer's app-bar
//! manager, which does not always apply it and never moves windows that were
//! already maximized. So after placing the app bars we read the work area back,
//! set it ourselves when it is wrong, and refit maximized windows to it.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_STYLE, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsZoomed,
    SPI_SETWORKAREA, SPIF_SENDCHANGE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, SystemParametersInfoW, WS_THICKFRAME,
};
use windows::core::BOOL;

use super::util::{data_dir, log};

fn primary() -> MONITORINFO {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        mi
    }
}

pub fn current() -> RECT {
    primary().rcWork
}

fn marker() -> std::path::PathBuf {
    data_dir().join("workarea-forced.txt")
}

fn set(r: &RECT) -> bool {
    unsafe { SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(r as *const RECT as *mut _), SPIF_SENDCHANGE).is_ok() }
}

/// The primary monitor's work area without `top` px at the top and `bottom` px
/// at the bottom. Strips other app bars reserve on top of ours are kept.
/// Returns the work area now in effect and whether it had to be changed.
pub fn ensure(top: i32, bottom: i32) -> (RECT, bool) {
    let mi = primary();
    let (mon, mut wa) = (mi.rcMonitor, mi.rcWork);
    let want_top = mon.top + top;
    let want_bottom = mon.bottom - bottom;
    if wa.top >= want_top && wa.bottom <= want_bottom {
        return (wa, false);
    }
    wa.top = wa.top.max(want_top);
    wa.bottom = wa.bottom.min(want_bottom);
    if !set(&wa) {
        log("work area: SPI_SETWORKAREA failed");
        return (current(), false);
    }
    let _ = std::fs::write(marker(), "1");
    (wa, true)
}

/// Gives the whole monitor back when we set the work area ourselves; Explorer
/// re-reserves its own taskbar when its state is restored right after. Safe to
/// call from the watchdog after a crash.
pub fn release() {
    if std::fs::remove_file(marker()).is_ok() {
        let mon = primary().rcMonitor;
        let _ = set(&mon);
    }
}

struct Fit {
    wa: RECT,
    own_pid: u32,
    moved: u32,
}

/// Moves every maximized window on the primary monitor so its visible frame
/// fills `wa`, keeping it maximized. Windows already in place are untouched.
pub fn fit_maximized(wa: RECT) -> u32 {
    let mut fit = Fit { wa, own_pid: std::process::id(), moved: 0 };
    unsafe {
        let _ = EnumWindows(Some(fit_one), LPARAM(&mut fit as *mut Fit as isize));
    }
    fit.moved
}

unsafe extern "system" fn fit_one(h: HWND, lp: LPARAM) -> BOOL {
    let fit = unsafe { &mut *(lp.0 as *mut Fit) };
    unsafe {
        if !IsWindowVisible(h).as_bool() || !IsZoomed(h).as_bool() {
            return BOOL(1);
        }
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        // Borderless "maximized" windows are usually games or fullscreen video.
        if pid == fit.own_pid || GetWindowLongPtrW(h, GWL_STYLE) & WS_THICKFRAME.0 as isize == 0 {
            return BOOL(1);
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
            return BOOL(1);
        }
        let primary_mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY) != primary_mon {
            return BOOL(1);
        }
        let (mut wr, mut fr) = (RECT::default(), RECT::default());
        if GetWindowRect(h, &mut wr).is_err()
            || DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut RECT as *mut _, size_of::<RECT>() as u32)
                .is_err()
        {
            return BOOL(1);
        }
        let wa = fit.wa;
        if fr == wa {
            return BOOL(1);
        }
        // The invisible resize borders stay outside the work area, as Windows
        // places them when it maximizes.
        let (l, t, r, b) = (wr.left + wa.left - fr.left, wr.top + wa.top - fr.top, wr.right + wa.right - fr.right, wr.bottom + wa.bottom - fr.bottom);
        if SetWindowPos(h, None, l, t, r - l, b - t, SWP_NOZORDER | SWP_NOACTIVATE).is_ok() {
            fit.moved += 1;
        }
    }
    BOOL(1)
}
