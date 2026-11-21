//! Which apps have windows: top-level windows grouped by AppUserModelID or
//! executable, the way the taskbar groups them.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::SIGDN_NORMALDISPLAY;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

use super::icons::shell_item;
use super::util::{from_wide, wide};

#[derive(Clone, Debug)]
pub struct RunningApp {
    pub key: String,
    pub exe: String,
    pub aumid: Option<String>,
    pub name: String,
    /// In z-order, topmost first.
    pub windows: Vec<HWND>,
}

pub fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn is_desktop_class(c: &str) -> bool {
    matches!(c, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

fn cloaked(h: HWND) -> bool {
    let mut c = 0u32;
    unsafe {
        DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut c as *mut u32 as *mut _, 4).is_ok() && c != 0
    }
}

/// A window that would get a taskbar button.
pub fn is_app_window(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() || cloaked(h) {
            return false;
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return false;
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let app = ex & WS_EX_APPWINDOW.0 != 0;
        if !app && (ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_NOACTIVATE.0 != 0) {
            return false;
        }
        if GetWindowTextLengthW(h) == 0 {
            return false;
        }
    }
    let c = class_name(h);
    !(is_desktop_class(&c) || c == "Windows.UI.Core.CoreWindow" || c == super::surface::CLASS)
}

fn process_info(pid: u32) -> (Option<String>, Option<String>) {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return (None, None) };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let exe = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()
            .map(|_| String::from_utf16_lossy(&buf[..len as usize]));
        let mut abuf = [0u16; 512];
        let mut alen = abuf.len() as u32;
        let aumid = (GetApplicationUserModelId(h, &mut alen, Some(PWSTR(abuf.as_mut_ptr()))) == ERROR_SUCCESS)
            .then(|| from_wide(&abuf))
            .filter(|s| !s.is_empty());
        let _ = CloseHandle(h);
        (exe, aumid)
    }
}

fn window_aumid(h: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
        let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

pub fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
    unsafe { (*(lp.0 as *mut Vec<HWND>)).push(h) };
    TRUE
}

/// UWP apps live inside ApplicationFrameHost; the real process owns a child window.
fn frame_child_pid(h: HWND, frame_pid: u32) -> Option<u32> {
    let mut kids: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(Some(h), Some(collect), LPARAM(&mut kids as *mut _ as isize));
    }
    kids.into_iter().map(window_pid).find(|&p| p != frame_pid && p != 0)
}

/// (exe path, AppUserModelID) of the app a window belongs to.
pub fn identify(h: HWND) -> (Option<String>, Option<String>) {
    let pid = window_pid(h);
    let (mut exe, mut aumid) = process_info(pid);
    if exe.as_deref().is_some_and(|e| e.to_lowercase().ends_with("\\applicationframehost.exe"))
        && let Some(child) = frame_child_pid(h, pid)
    {
        (exe, aumid) = process_info(child);
    }
    (exe, window_aumid(h).or(aumid))
}

pub fn app_key(exe: &str, aumid: Option<&str>) -> String {
    match aumid {
        Some(a) => format!("aumid:{}", a.to_lowercase()),
        None => exe.to_lowercase(),
    }
}

fn file_description(path: &str) -> Option<String> {
    let wp = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wp.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wp.as_ptr()), None, size, data.as_mut_ptr() as *mut _).ok()?;
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool()
            || len < 4
        {
            return None;
        }
        let tr = std::slice::from_raw_parts(ptr as *const u16, 2);
        let sub = wide(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", tr[0], tr[1]));
        if !VerQueryValueW(data.as_ptr() as *const _, PCWSTR(sub.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 {
            return None;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len as usize));
        let s = s.trim_end_matches('\0').trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

pub fn display_name(parse_name: &str) -> Option<String> {
    unsafe {
        let item = shell_item(parse_name)?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Human name of an app, cached by key.
pub fn app_name(cache: &mut HashMap<String, String>, exe: &str, aumid: Option<&str>) -> String {
    let key = app_key(exe, aumid);
    if let Some(n) = cache.get(&key) {
        return n.clone();
    }
    let name = aumid
        .and_then(|a| display_name(&format!("shell:AppsFolder\\{a}")))
        .or_else(|| file_description(exe))
        .unwrap_or_else(|| {
            std::path::Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        });
    cache.insert(key, name.clone());
    name
}

pub fn snapshot(names: &mut HashMap<String, String>) -> Vec<RunningApp> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
    }
    let own = unsafe { GetCurrentProcessId() };
    let mut apps: Vec<RunningApp> = Vec::new();
    for h in all {
        if !is_app_window(h) || window_pid(h) == own {
            continue;
        }
        let (exe, aumid) = identify(h);
        let Some(exe) = exe else { continue };
        let key = app_key(&exe, aumid.as_deref());
        if let Some(a) = apps.iter_mut().find(|a| a.key == key) {
            a.windows.push(h);
        } else {
            let name = app_name(names, &exe, aumid.as_deref());
            apps.push(RunningApp { key, exe, aumid, name, windows: vec![h] });
        }
    }
    apps
}

