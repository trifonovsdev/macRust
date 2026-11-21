//! The file manager process: `macrust.exe --files [folder] [--select name]`.
//! It runs apart from the shell so a crash while browsing cannot take the
//! Dock down. One process per user serves every window; a second launch
//! hands its folder to the running one over WM_COPYDATA.

mod dragdrop;
mod fsenum;
mod shellops;
mod window;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use macrust_ui::files::art::Art;
use macrust_ui::files::layout::View;
use macrust_ui::files::model::{Column, Sort};
use macrust_ui::files::strings::FileStrings;
use macrust_ui::files::Section;
use macrust_ui::gfx::Image;
use macrust_ui::strings::Strings;
use macrust_ui::text::Text;
use macrust_ui::theme::Theme;
use macrust_ui::Ctx;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, GetWindowThreadProcessId,
    HWND_MESSAGE, MB_ICONWARNING, MB_OK, MSG, MessageBoxW, RegisterClassExW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COPYDATA, WNDCLASSEXW,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::config;
use super::icons::{IconWorker, Request};
use super::system;
use super::util::{data_dir, log, redact};

/// AppUserModelID of the file manager's windows; the Dock maps it to the
/// Finder tile and the menu bar names it.
pub const AUMID: &str = "macRust.Files";
/// The menu bar and Dock name; macOS keeps "Finder" in every language.
pub const APP_NAME: &str = "Finder";

const HOST_CLASS: PCWSTR = w!("macRust.FilesHost");
/// WM_COPYDATA tag of an "open a window" request.
const OPEN_TAG: usize = 0x4D46_0001;

const WM_APP_ICON: u32 = WM_APP + 1;
const WM_APP_DRIVES: u32 = WM_APP + 2;

// ---------------------------------------------------------------------------
// Diagnostics: counts and timings only, never names or paths.

pub fn trace(msg: impl AsRef<str>) {
    super::util::trace(format!("files: {}", msg.as_ref()));
}

/// The process's trace, saved where the shell's report picks it up.
pub fn trace_path() -> std::path::PathBuf {
    data_dir().join("files-events.txt")
}

fn save_trace() {
    let _ = std::fs::write(trace_path(), redact(&super::util::trace_dump()));
}

// ---------------------------------------------------------------------------
// Options shared by all windows (view, sort, toggles, last window size)

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub view: View,
    pub sort: Sort,
    pub hidden: bool,
    pub pathbar: bool,
    /// Last window size, in points.
    pub w: i32,
    pub h: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self { view: View::Icons, sort: Sort::default(), hidden: false, pathbar: true, w: 920, h: 560 }
    }
}

fn options_path() -> std::path::PathBuf {
    data_dir().join("files.txt")
}

fn load_options() -> Options {
    let mut o = Options::default();
    let Ok(text) = std::fs::read_to_string(options_path()) else { return o };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let b = matches!(v, "1" | "true" | "on" | "yes");
        match k {
            "view" => o.view = if v == "list" { View::List } else { View::Icons },
            "sort" => {
                o.sort.column = match v {
                    "modified" => Column::Modified,
                    "size" => Column::Size,
                    "kind" => Column::Kind,
                    _ => Column::Name,
                }
            }
            "ascending" => o.sort.ascending = b,
            "folders_first" => o.sort.folders_first = b,
            "show_hidden" => o.hidden = b,
            "path_bar" => o.pathbar = b,
            "width" => o.w = v.parse().unwrap_or(o.w).clamp(480, 4000),
            "height" => o.h = v.parse().unwrap_or(o.h).clamp(320, 3000),
            _ => {}
        }
    }
    o
}

fn save_options(o: &Options) {
    let col = match o.sort.column {
        Column::Name => "name",
        Column::Modified => "modified",
        Column::Size => "size",
        Column::Kind => "kind",
    };
    let text = format!(
        "# macRust file manager. Ctrl+Shift+. shows hidden files.\nview = {}\nsort = {col}\nascending = {}\nfolders_first = {}\nshow_hidden = {}\npath_bar = {}\nwidth = {}\nheight = {}\n",
        if o.view == View::List { "list" } else { "icons" },
        o.sort.ascending,
        o.sort.folders_first,
        o.hidden,
        o.pathbar,
        o.w,
        o.h
    );
    let _ = std::fs::write(options_path(), text);
}

// ---------------------------------------------------------------------------
// Per-process state on the UI thread

/// The Locations section and (root, display name) of each drive.
type Drives = (Section, Vec<(String, String)>);

pub(super) struct Shared {
    pub ctx: Ctx,
    pub art: Rc<Art>,
    pub strings: &'static FileStrings,
    pub host: HWND,
    pub opts: Options,
    pub sections: Vec<Section>,
    pub roots: Vec<(String, String)>,
    /// Kind names per extension, learned from every listing.
    pub kinds: HashMap<String, String>,
    /// Folders listed before in this process (hashes), for cold/warm timings.
    pub seen: HashSet<u64>,
    icons: Vec<IconWorker>,
    icon_next: u64,
    /// Request id -> cache key.
    icon_ids: HashMap<u64, String>,
    /// Cache key -> windows waiting for it.
    icon_waiting: HashMap<String, Vec<isize>>,
    icon_cache: HashMap<String, Option<Rc<Image>>>,
    pub icon_hits: u64,
    pub icon_misses: u64,
    drives: Option<Receiver<Drives>>,
}

thread_local! {
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

pub(super) fn with_shared<R>(f: impl FnOnce(&mut Shared) -> R) -> Option<R> {
    SHARED.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn dark_wanted() -> bool {
    match config::load_settings().appearance.as_str() {
        "dark" => true,
        "light" => false,
        _ => system::system_dark(),
    }
}

impl Shared {
    /// Hands out icons: from the cache at once, else queued on a worker.
    pub fn request_icons(&mut self, hwnd: HWND, reqs: Vec<macrust_ui::files::IconRequest>) -> Vec<(String, Option<Rc<Image>>)> {
        let mut ready = Vec::new();
        for r in reqs {
            if let Some(img) = self.icon_cache.get(&r.key) {
                self.icon_hits += 1;
                ready.push((r.key, img.clone()));
                continue;
            }
            let waiting = self.icon_waiting.entry(r.key.clone()).or_default();
            let first = waiting.is_empty();
            let id = super::util::hwnd_to_isize(hwnd);
            if !waiting.contains(&id) {
                waiting.push(id);
            }
            if !first {
                continue;
            }
            self.icon_misses += 1;
            let id = self.icon_next;
            self.icon_next += 1;
            self.icon_ids.insert(id, r.key.clone());
            let worker = &self.icons[(id as usize) % self.icons.len()];
            worker.request(Request { id, parse_name: r.path, size: r.size as i32, thumbnail: r.thumbnail });
        }
        ready
    }

    fn icons_arrived(&mut self) -> Vec<(isize, String, Option<Rc<Image>>)> {
        let mut out = Vec::new();
        for wk in &self.icons {
            while let Ok((id, rgba)) = wk.rx.try_recv() {
                let Some(key) = self.icon_ids.remove(&id) else { continue };
                let img = rgba.and_then(|r| system::pixmap_from_rgba(r.w, r.h, r.data)).map(|pm| Rc::new(Image::new(pm)));
                // Per-file icons and thumbnails are many; keep the cache bounded.
                if self.icon_cache.len() > 4000 {
                    self.icon_cache.retain(|k, _| k.starts_with('e'));
                }
                self.icon_cache.insert(key.clone(), img.clone());
                for h in self.icon_waiting.remove(&key).unwrap_or_default() {
                    out.push((h, key.clone(), img.clone()));
                }
            }
        }
        out
    }

    pub fn save_options(&self) {
        save_options(&self.opts);
    }

    /// Re-reads drives on a worker thread (startup, device changes).
    pub fn refresh_drives(&mut self) {
        let (tx, rx) = channel();
        let host = super::util::hwnd_to_isize(self.host);
        let strings = self.strings;
        self.drives = Some(rx);
        let _ = std::thread::Builder::new().name("files-drives".into()).spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            let t = std::time::Instant::now();
            let r = shellops::locations(strings);
            trace(format!("drives: {} in {} ms", r.0.places.len().saturating_sub(1), t.elapsed().as_millis()));
            if tx.send(r).is_ok() {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(super::util::hwnd_from_isize(host)),
                        WM_APP_DRIVES,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        });
    }

    /// The theme follows Windows (or the shell's appearance setting).
    pub fn update_theme(&mut self) -> bool {
        let dark = dark_wanted();
        let changed = self.ctx.theme.dark != dark;
        self.ctx.theme = Theme { dark };
        changed
    }
}

fn load_fonts(text: &mut Text) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for f in ["segoeui.ttf", "seguisym.ttf"] {
        if let Ok(bytes) = std::fs::read(format!("{windir}\\Fonts\\{f}"))
            && let Ok(font) = macrust_ui::text::FontArc::try_from_vec(bytes)
        {
            text.add_fallback(font);
        }
    }
}

// ---------------------------------------------------------------------------
// The hidden host window: open requests, icon and drive results

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData != OPEN_TAG || cds.lpData.is_null() {
                return LRESULT(0);
            }
            let units = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
            let text = String::from_utf16_lossy(units);
            let mut parts = text.split('\0');
            let path = parts.next().filter(|p| !p.is_empty()).map(str::to_string);
            let select = parts.next().filter(|p| !p.is_empty()).map(str::to_string);
            trace("open request from another process");
            // Opening may show a window (and so send messages back to us):
            // do it after this SendMessage has returned.
            PENDING.with(|p| p.borrow_mut().push((path, select)));
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(hwnd), WM_APP + 3, WPARAM(0), LPARAM(0));
            }
            LRESULT(1)
        }
        m if m == WM_APP + 3 => {
            let pending = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
            for (path, select) in pending {
                window::open(path, select);
            }
            LRESULT(0)
        }
        WM_APP_ICON => {
            if let Some(arrived) = with_shared(|s| s.icons_arrived()) {
                window::icons_arrived(arrived);
            }
            LRESULT(0)
        }
        WM_APP_DRIVES => {
            let got = with_shared(|s| {
                let r = s.drives.as_ref().and_then(|rx| rx.try_recv().ok());
                if let Some((sec, roots)) = r {
                    s.sections.truncate(1);
                    s.sections.push(sec);
                    s.roots = roots;
                    true
                } else {
                    false
                }
            });
            if got == Some(true) {
                window::places_changed();
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

thread_local! {
    static PENDING: RefCell<Vec<(Option<String>, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

fn find_host() -> Option<HWND> {
    unsafe { FindWindowExW(Some(HWND_MESSAGE), None, HOST_CLASS, PCWSTR::null()).ok().filter(|h| !h.is_invalid()) }
}

/// Asks the running file manager to open a window. False when none runs.
fn forward(path: Option<&str>, select: Option<&str>) -> bool {
    let Some(host) = find_host() else { return false };
    let payload: Vec<u16> = format!("{}\0{}", path.unwrap_or(""), select.unwrap_or("")).encode_utf16().collect();
    let cds = COPYDATASTRUCT { dwData: OPEN_TAG, cbData: (payload.len() * 2) as u32, lpData: payload.as_ptr() as *mut _ };
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(host, Some(&mut pid));
        // The new window may take the foreground: we are allowed to, it is not.
        let _ = AllowSetForegroundWindow(pid);
        let mut result = 0usize;
        let r = SendMessageTimeoutW(host, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 3000, Some(&mut result));
        r.0 != 0 && result == 1
    }
}

// ---------------------------------------------------------------------------
// Entry points

fn parse_args(args: &[String]) -> (Option<String>, Option<String>) {
    let mut path = None;
    let mut select = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--select" => {
                select = args.get(i + 1).cloned();
                i += 1;
            }
            a if !a.starts_with("--") && path.is_none() => path = Some(a.trim_matches('"').to_string()),
            _ => {}
        }
        i += 1;
    }
    (path, select)
}

/// Runs the file manager process; `args` follow `--files`.
pub fn run(args: &[String]) {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        log(format!("files: panic in {thread}: {info}"));
        trace(format!("panic in {thread}"));
        save_trace();
        if thread == "main" {
            let text = HSTRING::from(format!(
                "The file manager stopped because of an error. The Dock and menu bar keep running.\n\nDetails: {}",
                data_dir().join("log.txt").display()
            ));
            unsafe {
                MessageBoxW(None, &text, w!("macRust"), MB_OK | MB_ICONWARNING);
            }
        }
    }));
    let (path, select) = parse_args(args);
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(None, false, w!("Local\\macRust.files"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(m) = mutex {
                let _ = CloseHandle(m);
            }
            // The running one may still be starting: give its host window time to appear.
            for _ in 0..25 {
                if forward(path.as_deref(), select.as_deref()) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            log("files: the running file manager did not answer");
            return;
        }
        // Drag and drop and the clipboard need OLE on the UI thread.
        if let Err(e) = OleInitialize(None) {
            log(format!("files: OleInitialize failed 0x{:08x}", e.code().0));
        }
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID));
    }
    let t0 = std::time::Instant::now();
    let hinst = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(host_proc),
        hInstance: hinst.into(),
        lpszClassName: HOST_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };
    let Ok(host) = (unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), HOST_CLASS, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst.into()), None)
    }) else {
        log("files: could not create the host window");
        return;
    };
    let ru = system::russian_ui();
    let strings: &'static FileStrings = if ru { &FileStrings::RU } else { &FileStrings::EN };
    let mut text = Text::bundled();
    load_fonts(&mut text);
    let ctx = Ctx::new(text, Theme { dark: dark_wanted() }, if ru { &Strings::RU } else { &Strings::EN });
    let icons = (0..3).map(|_| IconWorker::spawn(host, WM_APP_ICON)).collect();
    let mut shared = Shared {
        ctx,
        art: Rc::new(Art::new()),
        strings,
        host,
        opts: load_options(),
        sections: vec![shellops::favourites(strings)],
        roots: Vec::new(),
        kinds: HashMap::new(),
        seen: HashSet::new(),
        icons,
        icon_next: 1,
        icon_ids: HashMap::new(),
        icon_waiting: HashMap::new(),
        icon_cache: HashMap::new(),
        icon_hits: 0,
        icon_misses: 0,
        drives: None,
    };
    shared.refresh_drives();
    SHARED.with(|s| *s.borrow_mut() = Some(shared));
    window::register_class();
    trace(format!("started in {} ms", t0.elapsed().as_millis()));
    window::open(path, select);

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if let Some((hits, misses)) = with_shared(|s| (s.icon_hits, s.icon_misses)) {
        trace(format!("exit: icon cache {hits} hits, {misses} loads"));
    }
    save_trace();
}

// ---------------------------------------------------------------------------
// Used by the shell process: open folders here instead of in Explorer

/// Whether folders open in our file manager (settings: file_manager).
pub fn enabled() -> bool {
    config::load_settings().file_manager
}

/// Opens a file manager window: in the running process, else a new one.
pub fn open_window(path: Option<&str>, select: Option<&str>) {
    if forward(path, select) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--files");
    if let Some(p) = path {
        cmd.arg(p);
    }
    if let Some(s) = select {
        cmd.arg("--select").arg(s);
    }
    unsafe {
        let _ = AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    if let Err(e) = cmd.spawn() {
        log(format!("files: could not start the file manager: {}", e.kind()));
    }
}

/// What the shell would hand to Explorer, as (folder, item to select) when
/// we can show it; None leaves it to Explorer.
pub fn route(target: &str, args: &str) -> Option<(Option<String>, Option<String>)> {
    use crate::files_logic::Route;
    match crate::files_logic::route(target, args)? {
        Route::Home => Some((None, None)),
        Route::Select { dir, name } => Some((Some(dir), Some(name))),
        // A file path goes to its program: only folders are ours.
        Route::Folder(p) => is_folder(&p).then_some((Some(p), None)),
    }
}

fn is_folder(p: &str) -> bool {
    std::path::Path::new(p).is_dir()
}

