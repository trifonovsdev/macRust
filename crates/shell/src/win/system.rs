//! System state and actions: screen, appearance, battery, input language,
//! network, power, the stock taskbar, app bars, wallpaper and the clipboard.

use std::ffi::c_void;

use macrust_ui::tiny_skia::{IntSize, Pixmap};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GENERIC_READ, HANDLE, HWND, LPARAM, LUID, POINT, RECT,
};
use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
use windows::Win32::Graphics::Gdi::{
    COLOR_BACKGROUND, GetMonitorInfoW, GetSysColor, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, NLM_CONNECTIVITY_IPV4_INTERNET, NLM_CONNECTIVITY_IPV6_INTERNET, NetworkListManager,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS, SetSuspendState};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Shutdown::{
    EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, EWX_SHUTDOWN, ExitWindowsEx, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_TOP, ABM_GETSTATE, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABM_SETSTATE, ABS_AUTOHIDE,
    APPBARDATA, SHAppBarMessage, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetWindowThreadProcessId, SPI_GETDESKWALLPAPER,
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SW_HIDE, SW_SHOWNA, SW_SHOWNORMAL,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ShowWindow, SystemParametersInfoW,
};
use windows::core::{PCWSTR, w};

use super::util::{data_dir, from_wide, log, wide};

pub struct Screen {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
}

pub fn primary_screen() -> Screen {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = mi.rcMonitor;
        Screen { w: (r.right - r.left).max(640), h: (r.bottom - r.top).max(480), scale: dx as f32 / 96.0 }
    }
}

fn reg_dword(root: HKEY, sub: &str, value: &str) -> Option<u32> {
    let (sub, value) = (wide(sub), wide(value));
    let mut data = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut len),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

fn reg_string(root: HKEY, sub: &str, value: &str) -> Option<String> {
    let (sub, value) = (wide(sub), wide(value));
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        )
    };
    (r == ERROR_SUCCESS).then(|| from_wide(&buf))
}

pub fn system_dark() -> bool {
    reg_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
        == Some(0)
}

pub fn russian_ui() -> bool {
    const LANG_RUSSIAN: u16 = 0x19;
    unsafe { GetUserDefaultUILanguage() & 0x3ff == LANG_RUSSIAN }
}

/// Charge percent and "plugged in"; `None` on machines without a battery.
pub fn battery() -> Option<(u8, bool)> {
    let mut st = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut st).ok()? };
    if st.BatteryFlag & 128 != 0 || st.BatteryFlag == 255 || st.BatteryLifePercent > 100 {
        return None;
    }
    Some((st.BatteryLifePercent, st.ACLineStatus == 1 && st.BatteryLifePercent < 100))
}

/// Two-letter code of the keyboard layout used by the window's thread ("RU", "EN").
pub fn input_language(hwnd: HWND) -> String {
    unsafe {
        let tid = GetWindowThreadProcessId(hwnd, None);
        let hkl = GetKeyboardLayout(tid);
        let lang = (hkl.0 as usize & 0xffff) as u32;
        let mut buf = [0u16; 85];
        let n = LCIDToLocaleName(lang, Some(&mut buf), 0);
        if n <= 0 {
            return String::new();
        }
        from_wide(&buf).split('-').next().unwrap_or("").to_uppercase()
    }
}

pub struct Network(Option<INetworkListManager>);

impl Network {
    pub fn new() -> Self {
        Self(unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok() })
    }

    pub fn connected(&self) -> Option<bool> {
        let nlm = self.0.as_ref()?;
        let c = unsafe { nlm.GetConnectivity().ok()? };
        Some(c.0 & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0) != 0)
    }
}

/// Local time as (weekday 0 = Sunday, day, month 0-based, hour, minute, second).
pub fn local_time() -> (usize, u32, usize, u32, u32, u32) {
    let t = unsafe { GetLocalTime() };
    (
        t.wDayOfWeek as usize,
        t.wDay as u32,
        (t.wMonth as usize).saturating_sub(1),
        t.wHour as u32,
        t.wMinute as u32,
        t.wSecond as u32,
    )
}

// ---------------------------------------------------------------------------
// Launching

/// Opens a file, folder, program or URI. Runs on its own thread: ShellExecute
/// can block or pump messages for seconds (network paths, slow handlers), and
/// the shell's UI thread must keep running meanwhile.
pub fn open(target: &str) {
    open_with(target, "");
}

pub fn open_with(target: &str, args: &str) {
    let (target, args) = (target.to_string(), args.to_string());
    std::thread::spawn(move || {
        let (t, a) = (wide(&target), wide(&args));
        let r = unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(t.as_ptr()),
                if args.is_empty() { PCWSTR::null() } else { PCWSTR(a.as_ptr()) },
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if r.0 as isize <= 32 {
            log(format!("open failed ({}): {target} {args}", r.0 as isize));
        }
    });
}

/// Opens Explorer with the item selected.
pub fn reveal(path: &str) {
    open_with("explorer.exe", &format!("/select,\"{path}\""));
}

pub fn set_clipboard(hwnd: HWND, text: &str) {
    let data: Vec<u16> = wide(text);
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2) {
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                let _ = GlobalUnlock(mem);
                const CF_UNICODETEXT: u32 = 13;
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn clipboard_text(hwnd: HWND) -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let mut out = None;
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
            let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0)) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 4096 {
                    n += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Starts macRust at sign-in (HKCU Run key).
pub fn set_autostart(on: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide("macRust"));
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let value = wide(&format!("\"{exe}\""));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                REG_SZ.0,
                Some(value.as_ptr() as *const c_void),
                (value.len() * 2) as u32,
            ) == ERROR_SUCCESS
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr())) == ERROR_SUCCESS
        }
    }
}

// ---------------------------------------------------------------------------
// Power

fn enable_shutdown_privilege() {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).is_err() {
            return;
        }
        let mut luid = LUID::default();
        if LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).is_ok() {
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            let _ = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        }
        let _ = CloseHandle(token);
    }
}

pub enum Power {
    Sleep,
    Restart,
    ShutDown,
    LogOut,
    Lock,
}

pub fn power(action: Power) {
    unsafe {
        let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
        let r = match action {
            Power::Sleep => {
                let _ = SetSuspendState(false, false, false);
                Ok(())
            }
            Power::Lock => LockWorkStation(),
            Power::LogOut => ExitWindowsEx(EWX_LOGOFF, reason),
            Power::Restart => {
                enable_shutdown_privilege();
                ExitWindowsEx(EWX_REBOOT, reason)
            }
            Power::ShutDown => {
                enable_shutdown_privilege();
                ExitWindowsEx(EWX_SHUTDOWN | EWX_POWEROFF, reason)
            }
        };
        if let Err(e) = r {
            log(format!("power action failed: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// The stock taskbar

fn taskbars() -> Vec<HWND> {
    let mut v = Vec::new();
    unsafe {
        if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null())
            && !h.is_invalid()
        {
            v.push(h);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) {
            if h.is_invalid() {
                break;
            }
            v.push(h);
            prev = Some(h);
        }
    }
    v
}

fn taskbar_state_file() -> std::path::PathBuf {
    data_dir().join("taskbar-state.txt")
}

fn appbar_data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() }
}

/// Hides the stock taskbar and sets it to auto-hide so it frees its strip of the
/// work area. The previous state goes to a file first, so a crash or a forced
/// kill can still be undone on the next start or with `--restore`.
pub fn hide_taskbar() {
    let bars = taskbars();
    let Some(&tray) = bars.first() else { return };
    unsafe {
        let mut abd = appbar_data(tray);
        let prev = if let Some(saved) = std::fs::read_to_string(taskbar_state_file()).ok().and_then(|s| s.trim().parse().ok()) {
            saved
        } else {
            let prev = SHAppBarMessage(ABM_GETSTATE, &mut abd) as u32;
            let _ = std::fs::write(taskbar_state_file(), prev.to_string());
            prev
        };
        abd.lParam = LPARAM((prev | ABS_AUTOHIDE) as isize);
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
        for h in bars {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

/// Runs as `macrust.exe --watch <pid>`: waits for the shell process to end by
/// any means (crash, Task Manager, TerminateProcess) and puts the taskbar back
/// if the shell did not. A clean exit deletes the state file first, so this is
/// then a no-op.
pub fn watch(pid: u32) {
    use windows::Win32::System::Threading::{INFINITE, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let Ok(h) = windows::Win32::System::Threading::OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return };
        WaitForSingleObject(h, INFINITE);
        let _ = CloseHandle(h);
    }
    if taskbar_state_file().exists() {
        restore_taskbar();
    }
}

pub fn spawn_watchdog() {
    if let Ok(exe) = std::env::current_exe() {
        let pid = std::process::id().to_string();
        if let Err(e) = std::process::Command::new(exe).args(["--watch", &pid]).spawn() {
            log(format!("watchdog did not start: {e}"));
        }
    }
}

pub fn taskbar_state_saved() -> bool {
    taskbar_state_file().exists()
}

pub fn restore_taskbar() {
    let Some(prev) = std::fs::read_to_string(taskbar_state_file()).ok().and_then(|s| s.trim().parse::<u32>().ok()) else {
        // Nothing saved: we never hid it, but make sure it is visible.
        for h in taskbars() {
            unsafe {
                let _ = ShowWindow(h, SW_SHOWNA);
            }
        }
        return;
    };
    let bars = taskbars();
    unsafe {
        if let Some(&tray) = bars.first() {
            let mut abd = appbar_data(tray);
            abd.lParam = LPARAM(prev as isize);
            SHAppBarMessage(ABM_SETSTATE, &mut abd);
        }
        for h in bars {
            let _ = ShowWindow(h, SW_SHOWNA);
        }
    }
    let _ = std::fs::remove_file(taskbar_state_file());
}

// ---------------------------------------------------------------------------
// App bars: the menu bar and the dock reserve their strips of the work area.

pub const WM_APPBAR: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 10;

pub fn appbar_register(hwnd: HWND) {
    let mut abd = appbar_data(hwnd);
    abd.uCallbackMessage = WM_APPBAR;
    unsafe { SHAppBarMessage(ABM_NEW, &mut abd) };
}

pub fn appbar_place(hwnd: HWND, top: bool, screen: &Screen, thickness: i32) {
    let mut abd = appbar_data(hwnd);
    abd.uEdge = if top { ABE_TOP } else { ABE_BOTTOM };
    abd.rc = if top {
        RECT { left: 0, top: 0, right: screen.w, bottom: thickness }
    } else {
        RECT { left: 0, top: screen.h - thickness, right: screen.w, bottom: screen.h }
    };
    unsafe {
        SHAppBarMessage(ABM_QUERYPOS, &mut abd);
        if top {
            abd.rc.bottom = abd.rc.top + thickness;
        } else {
            abd.rc.top = abd.rc.bottom - thickness;
        }
        SHAppBarMessage(ABM_SETPOS, &mut abd);
    }
}

pub fn appbar_remove(hwnd: HWND) {
    let mut abd = appbar_data(hwnd);
    unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
}

// ---------------------------------------------------------------------------
// Wallpaper

fn desktop_color() -> [u8; 3] {
    let c = unsafe { GetSysColor(COLOR_BACKGROUND) };
    [(c & 0xff) as u8, ((c >> 8) & 0xff) as u8, ((c >> 16) & 0xff) as u8]
}

/// The desktop picture as it covers a `w` x `h` screen, decoded and scaled by WIC.
pub fn wallpaper(w: u32, h: u32) -> Option<Pixmap> {
    let mut buf = [0u16; 1024];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    let path = from_wide(&buf);
    let bg = desktop_color();
    let mut screen = Pixmap::new(w, h)?;
    screen.fill(macrust_ui::tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], 255));
    if path.is_empty() {
        return Some(screen);
    }
    let style: u32 = reg_string(HKEY_CURRENT_USER, r"Control Panel\Desktop", "WallpaperStyle")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(10);
    match decode_into(&mut screen, &path, style) {
        Ok(()) => Some(screen),
        Err(e) => {
            log(format!("wallpaper '{path}': {e}"));
            None
        }
    }
}

fn decode_into(screen: &mut Pixmap, path: &str, style: u32) -> windows::core::Result<()> {
    let (sw, sh) = (screen.width() as i64, screen.height() as i64);
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let wp = wide(path);
        let dec = factory.CreateDecoderFromFilename(PCWSTR(wp.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let (mut iw, mut ih) = (0u32, 0u32);
        frame.GetSize(&mut iw, &mut ih)?;
        let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
        // WallpaperStyle: 0 center, 2 stretch, 6 fit, 10 fill, 22 span (treated as fill).
        let (tw, th) = match style {
            0 => (iw, ih),
            2 => (sw as f64, sh as f64),
            6 => {
                let k = (sw as f64 / iw).min(sh as f64 / ih);
                (iw * k, ih * k)
            }
            _ => {
                let k = (sw as f64 / iw).max(sh as f64 / ih);
                (iw * k, ih * k)
            }
        };
        let (tw, th) = (tw.round().max(1.0) as i64, th.round().max(1.0) as i64);
        let (ox, oy) = ((sw - tw) / 2, (sh - th) / 2);
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&frame, tw as u32, th as u32, WICBitmapInterpolationModeHighQualityCubic)?;
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (sx, sy) = ((-ox).max(0), (-oy).max(0));
        let (dx, dy) = (ox.max(0), oy.max(0));
        let cw = (tw - sx).min(sw - dx);
        let ch = (th - sy).min(sh - dy);
        if cw <= 0 || ch <= 0 {
            return Ok(());
        }
        let mut px = vec![0u8; (cw * ch * 4) as usize];
        let rect = WICRect { X: sx as i32, Y: sy as i32, Width: cw as i32, Height: ch as i32 };
        conv.CopyPixels(&rect, (cw * 4) as u32, &mut px)?;
        let data = screen.data_mut();
        for row in 0..ch {
            let src = &px[(row * cw * 4) as usize..((row + 1) * cw * 4) as usize];
            let start = (((dy + row) * sw + dx) * 4) as usize;
            let dst = &mut data[start..start + (cw * 4) as usize];
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                // Wallpapers are opaque; PBGRA -> RGBA with alpha forced to 255.
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = 255;
            }
        }
    }
    Ok(())
}

/// Saves `pm` as the desktop picture (the bundled gradient, for `--wallpaper`).
pub fn set_wallpaper(pm: &Pixmap) -> bool {
    let path = data_dir().join("wallpaper.png");
    if pm.save_png(&path).is_err() {
        return false;
    }
    let wp = wide(&path.to_string_lossy());
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wp.as_ptr() as *mut c_void),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .is_ok()
    }
}

pub fn pixmap_from_rgba(w: u32, h: u32, data: Vec<u8>) -> Option<Pixmap> {
    Pixmap::from_vec(data, IntSize::from_wh(w, h)?)
}
