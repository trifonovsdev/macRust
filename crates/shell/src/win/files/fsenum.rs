//! Folder enumeration on worker threads and live refresh with
//! ReadDirectoryChangesW. Results go to the window over a channel; a posted
//! message wakes it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use macrust_ui::files::model::Entry;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, GetLastError, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FIND_FIRST_EX_LARGE_FETCH, FindClose,
    FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetDiskFreeSpaceExW, OPEN_EXISTING,
    ReadDirectoryChangesW, WIN32_FIND_DATAW,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::PCWSTR;

use super::super::util::{from_wide, hwnd_from_isize, hwnd_to_isize, wide};

pub struct Batch {
    pub generation: u64,
    pub entries: Vec<Entry>,
    /// New (extension, kind name) pairs.
    pub kinds: Vec<(String, String)>,
    pub done: bool,
    /// A live refresh: `entries` is the whole folder.
    pub refresh: bool,
    pub free: Option<u64>,
    /// Win32 error that stopped the listing.
    pub error: Option<u32>,
    pub elapsed: Duration,
    /// Time to the first batch, for the open-time trace.
    pub first: Option<Duration>,
}

/// Sends a first batch quickly so a big folder shows at once, then larger ones.
const FIRST_BATCH: usize = 300;
const BATCH_INTERVAL: Duration = Duration::from_millis(60);

fn kind_name(ext: &str) -> Option<String> {
    let w = wide(&format!(".{ext}"));
    let mut info = SHFILEINFOW::default();
    let r = unsafe {
        SHGetFileInfoW(PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_NORMAL, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES)
    };
    if r == 0 {
        return None;
    }
    Some(from_wide(&info.szTypeName)).filter(|s| !s.is_empty())
}

fn free_space(path: &str) -> Option<u64> {
    let w = wide(path);
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None).ok()? };
    Some(free)
}

fn filetime(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

/// A listing to run.
pub struct Job {
    pub path: String,
    pub generation: u64,
    pub refresh: bool,
    /// The window's newest generation: an older listing stops as soon as it notices.
    pub current: Arc<AtomicU64>,
    /// Extensions whose kind names the process already has.
    pub known: HashSet<String>,
    pub tx: Sender<Batch>,
    pub notify: HWND,
    pub msg: u32,
}

/// Lists a folder on a new thread.
pub fn spawn(job: Job) {
    let Job { path, generation, refresh, current, known, tx, notify, msg } = job;
    let target = hwnd_to_isize(notify);
    let _ = std::thread::Builder::new().name("files-enum".into()).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let send = |b: Batch| {
            if tx.send(b).is_ok() {
                unsafe {
                    let _ = PostMessageW(Some(hwnd_from_isize(target)), msg, WPARAM(0), LPARAM(0));
                }
            }
        };
        let t0 = Instant::now();
        let mut kinds_known = known;
        let mut kinds: Vec<(String, String)> = Vec::new();
        let mut buf: Vec<Entry> = Vec::new();
        let mut first: Option<Duration> = None;
        let mut last_send = Instant::now();
        let pattern = wide(&format!("{}\\*", path.trim_end_matches(['\\', '/'])));
        let mut data = WIN32_FIND_DATAW::default();
        let handle = unsafe {
            FindFirstFileExW(PCWSTR(pattern.as_ptr()), FindExInfoBasic, &mut data as *mut _ as *mut _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH)
        };
        let handle = match handle {
            Ok(h) => h,
            Err(_) => {
                let code = unsafe { GetLastError() };
                // An empty drive root has no "." entry and reports "not found".
                let error = (code != ERROR_FILE_NOT_FOUND).then_some(code.0);
                send(Batch { generation, entries: Vec::new(), kinds, done: true, refresh, free: free_space(&path), error, elapsed: t0.elapsed(), first: None });
                return;
            }
        };
        let mut error = None;
        loop {
            if current.load(Ordering::Relaxed) != generation {
                unsafe {
                    let _ = FindClose(handle);
                }
                return;
            }
            let name = from_wide(&data.cFileName);
            if name != "." && name != ".." {
                let dir = data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
                let size = (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64;
                let e = Entry::new(name, dir, size, filetime(data.ftLastWriteTime), data.dwFileAttributes);
                if !e.dir && !e.ext.is_empty() && kinds_known.insert(e.ext.clone())
                    && let Some(k) = kind_name(&e.ext)
                {
                    kinds.push((e.ext.clone(), k));
                }
                buf.push(e);
            }
            // A live refresh replaces the whole list at once; a fresh
            // listing streams so the first screenful appears early.
            if !refresh && ((first.is_none() && buf.len() >= FIRST_BATCH) || (first.is_some() && last_send.elapsed() >= BATCH_INTERVAL)) {
                first.get_or_insert(t0.elapsed());
                last_send = Instant::now();
                send(Batch {
                    generation,
                    entries: std::mem::take(&mut buf),
                    kinds: std::mem::take(&mut kinds),
                    done: false,
                    refresh,
                    free: None,
                    error: None,
                    elapsed: t0.elapsed(),
                    first,
                });
            }
            if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
                let code = unsafe { GetLastError() };
                if code != ERROR_NO_MORE_FILES {
                    error = Some(code.0);
                }
                break;
            }
        }
        unsafe {
            let _ = FindClose(handle);
        }
        let free = free_space(&path);
        send(Batch { generation, entries: buf, kinds, done: true, refresh, free, error, elapsed: t0.elapsed(), first });
    });
}

/// Wakes the window when the folder changes. Dropping it stops the thread.
pub struct Watcher {
    stop: Arc<Owned>,
}

struct Owned(HANDLE);
// The handle is used from both threads only through thread-safe kernel calls.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
    }
}

impl Watcher {
    /// Watches `path` (not its subfolders); posts `msg` with `generation` in
    /// wParam on every change. None when the folder cannot be watched.
    pub fn start(path: &str, generation: u64, notify: HWND, msg: u32) -> Option<Watcher> {
        let stop = Arc::new(Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()).ok()? }));
        let w = wide(path);
        let dir = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        let dir = match dir {
            Ok(h) => Owned(h),
            Err(e) => {
                super::trace(format!("watch: open failed 0x{:08x}", e.code().0));
                return None;
            }
        };
        let io = Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()).ok()? });
        let target = hwnd_to_isize(notify);
        let stop2 = stop.clone();
        let _ = std::thread::Builder::new().name("files-watch".into()).spawn(move || {
            // Take the owners whole: a closure would otherwise capture only
            // their raw handles, which are not Send.
            let (dir, io, stop2) = (dir, io, stop2);
            // DWORD-aligned buffer, as ReadDirectoryChangesW requires.
            let mut buf = vec![0u32; 16 * 1024];
            let filter = FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_ATTRIBUTES
                | FILE_NOTIFY_CHANGE_SIZE
                | FILE_NOTIFY_CHANGE_LAST_WRITE;
            loop {
                let mut ov = OVERLAPPED { hEvent: io.0, ..Default::default() };
                unsafe {
                    let _ = ResetEvent(io.0);
                }
                let started = unsafe {
                    ReadDirectoryChangesW(dir.0, buf.as_mut_ptr() as *mut _, (buf.len() * 4) as u32, false, filter, None, Some(&mut ov), None)
                };
                if let Err(e) = started {
                    super::trace(format!("watch: read failed 0x{:08x}", e.code().0));
                    return;
                }
                let r = unsafe { WaitForMultipleObjects(&[io.0, stop2.0], false, INFINITE) };
                if r != WAIT_OBJECT_0 {
                    unsafe {
                        let _ = CancelIoEx(dir.0, Some(&ov));
                        let mut n = 0u32;
                        // The buffer must outlive the cancelled request.
                        let _ = GetOverlappedResult(dir.0, &ov, &mut n, true);
                    }
                    return;
                }
                let mut n = 0u32;
                if unsafe { GetOverlappedResult(dir.0, &ov, &mut n, false) }.is_err() {
                    // The folder itself went away (deleted, drive removed).
                    unsafe {
                        let _ = PostMessageW(Some(hwnd_from_isize(target)), msg, WPARAM(generation as usize), LPARAM(1));
                    }
                    return;
                }
                unsafe {
                    let _ = PostMessageW(Some(hwnd_from_isize(target)), msg, WPARAM(generation as usize), LPARAM(0));
                }
            }
        });
        Some(Watcher { stop })
    }
}

/// Kind names seen so far, per process: enumeration threads skip asking the
/// shell again for them.
pub fn known_kinds(map: &HashMap<String, String>) -> HashSet<String> {
    map.keys().cloned().collect()
}
