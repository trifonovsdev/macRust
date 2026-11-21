//! Operations on other apps' windows. All asynchronous: a hung app must not
//! freeze the shell.

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HWND_TOP, IsIconic, IsZoomed, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_ASYNCWINDOWPOS, SetForegroundWindow, SetWindowPos, ShowWindowAsync, WM_CLOSE,
};

pub fn activate(h: HWND) {
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
        if !SetForegroundWindow(h).as_bool() {
            // A synthetic Alt press satisfies the foreground lock rules.
            let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: flags, ..Default::default() } },
            };
            SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(h);
        }
    }
}

pub fn is_minimized(h: HWND) -> bool {
    unsafe { IsIconic(h).as_bool() }
}

pub fn minimize(h: HWND) {
    unsafe {
        let _ = ShowWindowAsync(h, SW_MINIMIZE);
    }
}

pub fn restore(h: HWND) {
    unsafe {
        let _ = ShowWindowAsync(h, SW_RESTORE);
    }
}

pub fn toggle_zoom(h: HWND) {
    unsafe {
        let _ = ShowWindowAsync(h, if IsZoomed(h).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
    }
}

pub fn close(h: HWND) {
    unsafe {
        let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn raise(h: HWND) {
    unsafe {
        let _ = SetWindowPos(h, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

fn work_area(h: HWND) -> RECT {
    unsafe {
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        mi.rcWork
    }
}

/// Invisible resize borders: window rect minus the visible frame, per side.
fn borders(h: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut wr = RECT::default();
        let mut fr = RECT::default();
        if GetWindowRect(h, &mut wr).is_err()
            || DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut RECT as *mut _, size_of::<RECT>() as u32)
                .is_err()
        {
            return (0, 0, 0, 0);
        }
        (fr.left - wr.left, fr.top - wr.top, wr.right - fr.right, wr.bottom - fr.bottom)
    }
}

/// Places the visible frame of `h` exactly on `r`.
fn place(h: HWND, r: RECT) {
    unsafe {
        if IsZoomed(h).as_bool() || IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
        let (l, t, rr, b) = borders(h);
        let _ = SetWindowPos(
            h,
            None,
            r.left - l,
            r.top - t,
            (r.right - r.left) + l + rr,
            (r.bottom - r.top) + t + b,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn tile(h: HWND, left: bool) {
    let wa = work_area(h);
    let mid = (wa.left + wa.right) / 2;
    place(h, if left { RECT { right: mid, ..wa } } else { RECT { left: mid, ..wa } });
}

pub fn center(h: HWND) {
    let wa = work_area(h);
    let mut wr = RECT::default();
    unsafe {
        if IsZoomed(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
        if GetWindowRect(h, &mut wr).is_err() {
            return;
        }
        let (w, ht) = (wr.right - wr.left, wr.bottom - wr.top);
        let x = (wa.left + wa.right - w) / 2;
        let y = (wa.top + wa.bottom - ht) / 2;
        let _ = SetWindowPos(h, None, x, y.max(wa.top), 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}
