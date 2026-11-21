//! Small helpers: UTF-16 strings, logging, paths.

use std::io::Write;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::GUID;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// %APPDATA%\macRust, created on first use.
pub fn data_dir() -> PathBuf {
    let base = known_folder(&FOLDERID_RoamingAppData).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("macRust");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Appends a line to %APPDATA%\macRust\log.txt; the shell has no console.
pub fn log(msg: impl AsRef<str>) {
    let path = data_dir().join("log.txt");
    // Keep the log from growing without bound across months of use.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} {}", msg.as_ref());
    }
}

/// HWND is a raw pointer and not `Send`; worker threads carry it as an integer.
pub fn hwnd_to_isize(h: HWND) -> isize {
    h.0 as isize
}

pub fn hwnd_from_isize(v: isize) -> HWND {
    HWND(v as *mut _)
}

// ---------------------------------------------------------------------------
// Event trace for the diagnostic report: the last few hundred UI events, kept
// in memory only. Callers pass no window titles, file names or message text.

static TRACE: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn trace(msg: impl AsRef<str>) {
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_millis();
    if let Ok(mut q) = TRACE.lock() {
        if q.len() >= 600 {
            q.pop_front();
        }
        q.push_back(format!("{t:>9} {}", msg.as_ref()));
    }
}

pub fn trace_dump() -> String {
    TRACE.lock().map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Replaces the user's profile path and name, so a report names no one.
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    if let Ok(p) = std::env::var("USERPROFILE")
        && !p.is_empty()
    {
        out = out.replace(&p, "%USERPROFILE%").replace(&p.to_lowercase(), "%USERPROFILE%");
    }
    if let Ok(u) = std::env::var("USERNAME")
        && u.len() > 2
    {
        out = out.replace(&u, "<user>");
    }
    out
}
