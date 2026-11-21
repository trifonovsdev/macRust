//! A file manager window: a normal resizable top-level window (snap, Alt+Tab,
//! min/max animations) whose caption is replaced by our Tahoe toolbar. The
//! left, right and bottom frames stay (invisible resize borders on Windows
//! 10/11); only the top one is taken into the client area. Everything is
//! drawn on the CPU into a DIB and repainted only when something changed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use macrust_ui::files::format::Civil;
use macrust_ui::files::layout::View;
use macrust_ui::files::model::{self, Column};
use macrust_ui::files::view::{Load, vk};
use macrust_ui::files::{Action, Browser, Hit};
use macrust_ui::gfx::Image;
use macrust_ui::tiny_skia::Pixmap;
use macrust_ui::Mods;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, ClientToScreen, DIB_RGB_COLORS, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient,
    SetDIBitsToDevice,
};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand, PKEY_AppUserModel_RelaunchDisplayNameResource,
    PKEY_AppUserModel_RelaunchIconResource,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::SystemTimeToFileTime;
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{FOLDERID_Profile, SHStrDupW};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, IDC_IBEAM, IsZoomed, KillTimer, LoadCursorW, LoadIconW, MINMAXINFO, NCCALCSIZE_PARAMS,
    PostMessageW, PostQuitMessage, RegisterClassExW, SM_CXPADDEDBORDER, SM_CYFRAME, SPI_GETWHEELSCROLLLINES, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, SystemParametersInfoW, WM_ACTIVATE, WM_APP,
    WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DEVICECHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SIZE, WM_SYSKEYDOWN,
    WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::super::util::{hwnd_from_isize, hwnd_to_isize, known_folder};
use super::dragdrop::DropTarget;
use super::fsenum::{self, Batch, Watcher};
use super::shellops::{self, Extra, Pick};
use super::{APP_NAME, AUMID, Shared, trace, with_shared};

const CLASS: PCWSTR = w!("macRust.Files");
const WM_APP_ENUM: u32 = WM_APP + 10;
const WM_APP_CHANGED: u32 = WM_APP + 11;
const WM_APP_OPDONE: u32 = WM_APP + 12;
/// In Win32_UI_Controls, a feature we do not otherwise need.
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_ANIM: usize = 1;
const T_REFRESH: usize = 2;
/// A folder that keeps changing (a download in progress) is re-listed at
/// most this often.
const REFRESH_MIN_MS: u128 = 1000;

struct Win {
    hwnd: HWND,
    b: Browser,
    pm: Pixmap,
    bgra: Vec<u32>,
    generation: u64,
    current: Arc<AtomicU64>,
    tx: Sender<Batch>,
    rx: Receiver<Batch>,
    watcher: Option<Watcher>,
    watched: u64,
    last_refresh: Option<Instant>,
    last_tick: Instant,
    anim: bool,
    tracking: bool,
    zoom_down: bool,
    /// (started, folder listed before in this process)
    opening: Option<(Instant, bool)>,
    surrogate: Option<u16>,
    drop_target: Option<IDropTarget>,
    title: String,
    frames: (u32, f64, f64),
}

thread_local! {
    static WINDOWS: RefCell<HashMap<isize, Rc<RefCell<Win>>>> = RefCell::new(HashMap::new());
}

fn get(hwnd: HWND) -> Option<Rc<RefCell<Win>>> {
    WINDOWS.with(|m| m.borrow().get(&hwnd_to_isize(hwnd)).cloned())
}

fn all() -> Vec<Rc<RefCell<Win>>> {
    WINDOWS.with(|m| m.borrow().values().cloned().collect())
}

/// Runs `f` with the window and the shared state borrowed; None when either
/// is busy (a re-entrant message while a handler runs).
fn with_win<R>(hwnd: HWND, f: impl FnOnce(&mut Win, &mut Shared) -> R) -> Option<R> {
    let win = get(hwnd)?;
    let mut w = win.try_borrow_mut().ok()?;
    with_shared(|sh| f(&mut w, sh))
}

fn mods() -> Mods {
    let down = |k: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) < 0 };
    Mods { ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xffff) as u16 as i16 as i32, ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32)
}

fn scale_of(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// Local wall-clock time and the local offset from UTC, for the date column.
fn clock() -> (Civil, i64) {
    unsafe {
        let l = GetLocalTime();
        let u = windows::Win32::System::SystemInformation::GetSystemTime();
        let ft = |t: &SYSTEMTIME| {
            let mut f = FILETIME::default();
            let _ = SystemTimeToFileTime(t, &mut f);
            ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as i64
        };
        let offset = ((ft(&l) - ft(&u)) / 10_000_000 + 30) / 60 * 60;
        let now = Civil { year: l.wYear as i32, month: l.wMonth as u32, day: l.wDay as u32, hour: l.wHour as u32, minute: l.wMinute as u32 };
        (now, offset)
    }
}

fn path_hash(p: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.to_lowercase().hash(&mut h);
    h.finish()
}

fn lpwstr(s: &str) -> Option<PROPVARIANT> {
    let p = unsafe { SHStrDupW(&HSTRING::from(s)).ok()? };
    let mut pv = PROPVARIANT::default();
    // PropVariantClear (PROPVARIANT's Drop) frees the CoTaskMem string.
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = p;
    }
    Some(pv)
}

/// Window properties that group our windows as one app under its own
/// AppUserModelID, named "Finder", relaunched with `--files`.
fn set_identity(hwnd: HWND) {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let r = (|| -> windows::core::Result<()> {
        let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd)? };
        let props = [
            (&PKEY_AppUserModel_ID, AUMID.to_string()),
            (&PKEY_AppUserModel_RelaunchCommand, format!("\"{exe}\" --files")),
            (&PKEY_AppUserModel_RelaunchDisplayNameResource, APP_NAME.to_string()),
            (&PKEY_AppUserModel_RelaunchIconResource, format!("{exe},0")),
        ];
        for (key, value) in props {
            if let Some(pv) = lpwstr(&value) {
                unsafe { store.SetValue(key, &pv)? };
            }
        }
        unsafe { store.Commit() }
    })();
    if let Err(e) = r {
        trace(format!("identity: property store failed 0x{:08x}", e.code().0));
    }
}

fn set_dark(hwnd: HWND, dark: bool) {
    let v: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as *const _, 4);
    }
}

pub fn register_class() {
    let hinst = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        // Resource 1 is the app icon (app.rc): Alt+Tab shows it.
        hIcon: unsafe { LoadIconW(Some(hinst.into()), PCWSTR(1 as _)).unwrap_or_default() },
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        lpszClassName: CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };
}

/// Opens a window on `path` (the home folder when None), selecting `select`.
pub fn open(path: Option<String>, select: Option<String>) {
    let path = path.or_else(|| known_folder(&FOLDERID_Profile).map(|p| p.display().to_string())).unwrap_or_else(|| "C:\\".into());
    let Some((opts, strings, art, sections, roots, kinds, dark)) =
        with_shared(|s| (s.opts, s.strings, s.art.clone(), s.sections.clone(), s.roots.clone(), s.kinds.clone(), s.ctx.theme.dark))
    else {
        return;
    };
    let hinst = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };
    let hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(model::leaf(&path)),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            opts.w,
            opts.h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            trace(format!("window: create failed 0x{:08x}", e.code().0));
            return;
        }
    };
    let scale = scale_of(hwnd);
    let mut b = Browser::new(strings, art, 1.0, 1.0, scale);
    b.view = opts.view;
    b.list.sort = opts.sort;
    b.list.show_hidden = opts.hidden;
    b.show_pathbar = opts.pathbar;
    b.sections = sections;
    b.root_names = roots;
    b.list.kinds.map = kinds;
    let (tx, rx) = channel();
    let win = Win {
        hwnd,
        b,
        pm: Pixmap::new(1, 1).expect("1x1"),
        bgra: Vec::new(),
        generation: 0,
        current: Arc::new(AtomicU64::new(0)),
        tx,
        rx,
        watcher: None,
        watched: 0,
        last_refresh: None,
        last_tick: Instant::now(),
        anim: false,
        tracking: false,
        zoom_down: false,
        opening: None,
        surrogate: None,
        drop_target: None,
        title: String::new(),
        frames: (0, 0.0, 0.0),
    };
    WINDOWS.with(|m| m.borrow_mut().insert(hwnd_to_isize(hwnd), Rc::new(RefCell::new(win))));
    unsafe {
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        set_dark(hwnd, dark);
        // The frame changes now that our NCCALCSIZE answers; size in points.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            (opts.w as f32 * scale) as i32,
            (opts.h as f32 * scale) as i32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    set_identity(hwnd);
    let target = DropTarget::create(hwnd);
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => {
            if let Some(w) = get(hwnd) {
                w.borrow_mut().drop_target = Some(target);
            }
        }
        Err(e) => trace(format!("window: drop target failed 0x{:08x}", e.code().0)),
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
    let actions = with_win(hwnd, |w, _| {
        let a = w.b.navigate(&path);
        if let Some(s) = &select {
            w.b.select_when_listed(s);
        }
        a
    })
    .unwrap_or_default();
    perform(hwnd, actions);
    trace(format!("window: opened, {} open", WINDOWS.with(|m| m.borrow().len())));
}

// ---------------------------------------------------------------------------
// Rendering

impl Win {
    fn resize(&mut self, w: i32, h: i32, scale: f32) {
        let (w, h) = (w.max(1) as u32, h.max(1) as u32);
        if self.pm.width() != w || self.pm.height() != h {
            if let Some(pm) = Pixmap::new(w, h) {
                self.pm = pm;
            }
            self.bgra = vec![0; (w * h) as usize];
        }
        self.b.resize(w as f32, h as f32, scale);
        self.b.dirty = true;
    }

    fn render(&mut self, sh: &Shared) {
        let t = Instant::now();
        self.b.draw(&mut self.pm, &sh.ctx);
        // tiny-skia is RGBA, GDI wants BGRA; the window is opaque.
        for (d, s) in self.bgra.iter_mut().zip(self.pm.data().chunks_exact(4)) {
            *d = 0xFF00_0000 | (s[0] as u32) << 16 | (s[1] as u32) << 8 | s[2] as u32;
        }
        self.b.dirty = false;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        self.frames.0 += 1;
        self.frames.1 += ms;
        self.frames.2 = self.frames.2.max(ms);
    }

    fn paint(&mut self, sh: &Shared) {
        if self.b.dirty {
            self.render(sh);
        }
        let (w, h) = (self.pm.width() as i32, self.pm.height() as i32);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            SetDIBitsToDevice(hdc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, self.bgra.as_ptr() as *const _, &bmi, DIB_RGB_COLORS);
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// Bookkeeping after every event: repaint, the animation timer, icon
    /// requests. Returns a new window title when it changed.
    fn after(&mut self, sh: &mut Shared) -> Option<String> {
        let reqs = self.b.icon_requests();
        if !reqs.is_empty() {
            for (key, img) in sh.request_icons(self.hwnd, reqs) {
                self.b.icon_loaded(key, img);
            }
        }
        if self.b.dirty {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
        let anim = self.b.animating();
        if anim && !self.anim {
            self.last_tick = Instant::now();
            unsafe { SetTimer(Some(self.hwnd), T_ANIM, 15, None) };
        } else if !anim && self.anim {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), T_ANIM);
            }
        }
        self.anim = anim;
        let title = self.b.title().to_string();
        (title != self.title).then(|| {
            self.title = title.clone();
            title
        })
    }

    // -- listing --------------------------------------------------------

    fn start_listing(&mut self, sh: &mut Shared, refresh: bool) {
        self.generation += 1;
        self.current.store(self.generation, Ordering::Relaxed);
        if !refresh {
            self.watcher = None;
            let h = path_hash(&self.b.path);
            self.opening = Some((Instant::now(), !sh.seen.insert(h)));
            let (now, tz) = clock();
            self.b.now = now;
            self.b.tz = tz;
        }
        self.last_refresh = Some(Instant::now());
        fsenum::spawn(fsenum::Job {
            path: self.b.path.clone(),
            generation: self.generation,
            refresh,
            current: self.current.clone(),
            known: fsenum::known_kinds(&sh.kinds),
            tx: self.tx.clone(),
            notify: self.hwnd,
            msg: WM_APP_ENUM,
        });
    }

    fn batches(&mut self, sh: &mut Shared) {
        while let Ok(b) = self.rx.try_recv() {
            if b.generation != self.generation {
                continue;
            }
            for (k, v) in &b.kinds {
                sh.kinds.insert(k.clone(), v.clone());
            }
            let n = b.entries.len();
            if b.refresh {
                match b.error {
                    None => self.b.replaced(b.entries, b.kinds, b.free),
                    // The folder itself is gone (deleted, drive removed).
                    Some(code @ (2 | 3 | 21)) => {
                        self.b.list.set_entries(Vec::new());
                        self.b.failed(code);
                        self.watcher = None;
                    }
                    Some(_) => {}
                }
                trace(format!("refresh: {n} items in {} ms", b.elapsed.as_millis()));
                continue;
            }
            self.b.loaded(b.entries, b.kinds, b.done, b.free);
            if let Some(code) = b.error {
                if self.b.list.is_empty() {
                    self.b.failed(code);
                }
                trace(format!("list: error {code} after {} ms", b.elapsed.as_millis()));
            }
            if b.done {
                if let Some((t, warm)) = self.opening.take() {
                    trace(format!(
                        "open {}: {} items, first batch {} ms, listed {} ms, shown {} ms",
                        if warm { "warm" } else { "cold" },
                        self.b.list.entries.len(),
                        b.first.map(|d| d.as_millis().to_string()).unwrap_or_else(|| "-".into()),
                        b.elapsed.as_millis(),
                        t.elapsed().as_millis()
                    ));
                }
                if b.error.is_none() && self.watcher.is_none() {
                    self.watcher = Watcher::start(&self.b.path, self.generation, self.hwnd, WM_APP_CHANGED);
                    self.watched = self.generation;
                }
                super::save_trace();
            }
        }
    }

    fn schedule_refresh(&mut self) {
        let since = self.last_refresh.map(|t| t.elapsed().as_millis()).unwrap_or(u128::MAX);
        let wait = if since >= REFRESH_MIN_MS { 200 } else { (REFRESH_MIN_MS - since).max(200) as u32 };
        unsafe { SetTimer(Some(self.hwnd), T_REFRESH, wait, None) };
    }
}

// ---------------------------------------------------------------------------
// Actions

fn client_to_screen(hwnd: HWND, x: f32, y: f32) -> POINT {
    let mut p = POINT { x: x as i32, y: y as i32 };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut p);
    }
    p
}

const ID_ICONS: u32 = 1;
const ID_LIST: u32 = 2;
const ID_SORT: u32 = 10;
const ID_FOLDERS_FIRST: u32 = 14;
const ID_HIDDEN: u32 = 20;
const ID_PATHBAR: u32 = 21;
const ID_NEW_FOLDER: u32 = 30;
const ID_REFRESH: u32 = 32;
const ID_EXPLORER: u32 = 40;

fn background_extras(b: &Browser) -> Vec<Extra> {
    let s = b.strings;
    let sort = b.list.sort;
    let col = |c: Column, i: u32| Extra::Item { id: ID_SORT + i, label: s.column(c).into(), checked: sort.column == c };
    vec![
        Extra::Item { id: ID_NEW_FOLDER, label: format!("{}\tCtrl+Shift+N", s.new_folder), checked: false },
        Extra::Separator,
        Extra::Item { id: ID_ICONS, label: format!("{}\tCtrl+1", s.view_icons), checked: b.view == View::Icons },
        Extra::Item { id: ID_LIST, label: format!("{}\tCtrl+2", s.view_list), checked: b.view == View::List },
        Extra::Sub {
            label: s.sort_by.into(),
            items: vec![
                col(Column::Name, 0),
                col(Column::Modified, 1),
                col(Column::Size, 2),
                col(Column::Kind, 3),
                Extra::Separator,
                Extra::Item { id: ID_FOLDERS_FIRST, label: s.folders_first.into(), checked: sort.folders_first },
            ],
        },
        Extra::Item { id: ID_HIDDEN, label: format!("{}\tCtrl+Shift+.", s.show_hidden), checked: b.list.show_hidden },
        Extra::Item { id: ID_PATHBAR, label: s.show_path_bar.into(), checked: b.show_pathbar },
        Extra::Item { id: ID_REFRESH, label: format!("{}\tF5", s.refresh), checked: false },
        Extra::Separator,
        Extra::Item { id: ID_EXPLORER, label: s.open_in_explorer.into(), checked: false },
        Extra::Separator,
    ]
}

fn own_command(b: &mut Browser, id: u32) -> Vec<Action> {
    match id {
        ID_ICONS => b.set_view(View::Icons),
        ID_LIST => b.set_view(View::List),
        i if (ID_SORT..ID_SORT + 4).contains(&i) => {
            let c = [Column::Name, Column::Modified, Column::Size, Column::Kind][(i - ID_SORT) as usize];
            b.sort_by(c)
        }
        ID_FOLDERS_FIRST => {
            let mut s = b.list.sort;
            s.folders_first = !s.folders_first;
            b.list.set_sort(s);
            b.dirty = true;
            vec![Action::SaveOptions]
        }
        ID_HIDDEN => b.toggle_hidden(),
        ID_PATHBAR => b.toggle_pathbar(),
        ID_NEW_FOLDER => b.new_folder(),
        ID_REFRESH => vec![Action::Refresh],
        ID_EXPLORER => {
            let p = b.path.clone();
            std::thread::spawn(move || {
                let args = HSTRING::from(format!("\"{p}\""));
                unsafe {
                    windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, PCWSTR::null(), SW_SHOWNORMAL);
                }
            });
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Carries out what the browser asked for. Calls that run a modal loop
/// (menus, drag and drop, dialogs) happen here, with nothing borrowed.
fn perform(hwnd: HWND, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Navigate(_) | Action::Refresh => {
                let refresh = matches!(a, Action::Refresh);
                with_win(hwnd, |w, sh| w.start_listing(sh, refresh));
            }
            Action::OpenFiles(paths) => {
                trace(format!("open: {} files", paths.len()));
                for p in paths.into_iter().take(16) {
                    shellops::shell_open(p);
                }
            }
            Action::NewWindow(p) => open(Some(p), None),
            Action::ShellOpen(p) => shellops::shell_open(p),
            Action::Rename { path, to } => shellops::rename(hwnd, WM_APP_OPDONE, path, to),
            Action::Delete { paths, permanent } => shellops::delete(hwnd, WM_APP_OPDONE, paths, permanent),
            Action::NewFolder { path } => shellops::new_folder(hwnd, WM_APP_OPDONE, path),
            Action::Clipboard { paths, cut } => {
                shellops::set_clipboard_files(hwnd, &paths, cut);
            }
            Action::Paste { into } => match shellops::clipboard_files(hwnd) {
                Some((files, cut)) => {
                    trace(format!("paste: {} items, {}", files.len(), if cut { "move" } else { "copy" }));
                    shellops::transfer(hwnd, WM_APP_OPDONE, files, into, cut);
                }
                None => trace("paste: no files on the clipboard"),
            },
            Action::ContextMenu { paths, x, y } => {
                let Some((folder, extras)) = with_win(hwnd, |w, _| {
                    let extras = if paths.is_empty() { background_extras(&w.b) } else { Vec::new() };
                    (w.b.path.clone(), extras)
                }) else {
                    continue;
                };
                let pt = client_to_screen(hwnd, x, y);
                // A sidebar place's menu: the place itself, in its parent.
                let folder = if paths.len() == 1 && model::parent(&paths[0]).is_none_or(|p| !model::same_path(&p, &folder)) {
                    model::parent(&paths[0]).unwrap_or(folder)
                } else {
                    folder
                };
                match shellops::context_menu(hwnd, &folder, &paths, &extras, pt) {
                    Pick::Ours(id) => {
                        let more = with_win(hwnd, |w, _| own_command(&mut w.b, id)).unwrap_or_default();
                        perform(hwnd, more);
                    }
                    Pick::Rename => {
                        with_win(hwnd, |w, _| {
                            if let Some(p) = w.b.list.selected_positions().first().copied() {
                                w.b.begin_rename(p);
                            }
                        });
                    }
                    Pick::Shell => {
                        // Shell commands (paste, new, delete) change the folder;
                        // the watcher catches it, this covers a missed notice.
                        with_win(hwnd, |w, _| w.schedule_refresh());
                    }
                    Pick::Nothing => {}
                }
            }
            Action::DragOut(paths) => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                shellops::drag_out(hwnd, &paths);
            }
            Action::Eject(path) => {
                shellops::invoke_verb(hwnd, &path, "eject");
                with_shared(|s| s.refresh_drives());
            }
            Action::Close => unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Action::Minimize => unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            },
            Action::Zoom => unsafe {
                let _ = ShowWindow(hwnd, if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
            },
            Action::SaveOptions => {
                with_win(hwnd, |w, sh| {
                    sh.opts.view = w.b.view;
                    sh.opts.sort = w.b.list.sort;
                    sh.opts.hidden = w.b.list.show_hidden;
                    sh.opts.pathbar = w.b.show_pathbar;
                    sh.save_options();
                });
            }
        }
    }
    finish(hwnd);
}

/// Runs `after` and applies a changed title outside the borrow
/// (SetWindowText sends a message back to this window).
fn finish(hwnd: HWND) {
    if let Some(Some(title)) = with_win(hwnd, |w, sh| w.after(sh)) {
        unsafe {
            let _ = SetWindowTextW(hwnd, &HSTRING::from(title));
        }
    }
}

// ---------------------------------------------------------------------------
// Called from the host window and the drop target

pub fn icons_arrived(list: Vec<(isize, String, Option<Rc<Image>>)>) {
    let mut touched = Vec::new();
    for (h, key, img) in list {
        let hwnd = hwnd_from_isize(h);
        if with_win(hwnd, |w, _| w.b.icon_loaded(key, img)).is_some() && !touched.contains(&h) {
            touched.push(h);
        }
    }
    for h in touched {
        finish(hwnd_from_isize(h));
    }
}

pub fn places_changed() {
    let Some((sections, roots)) = with_shared(|s| (s.sections.clone(), s.roots.clone())) else { return };
    for w in all() {
        let hwnd = w.borrow().hwnd;
        with_win(hwnd, |w, _| {
            w.b.sections = sections.clone();
            w.b.root_names = roots.clone();
            w.b.dirty = true;
        });
        finish(hwnd);
    }
}

pub fn drag_over(hwnd: HWND, x: f32, y: f32) -> Option<String> {
    let r = with_win(hwnd, |w, sh| w.b.drag_over(x, y, &sh.ctx.text)).flatten();
    finish(hwnd);
    r
}

pub fn drag_leave(hwnd: HWND) {
    with_win(hwnd, |w, _| w.b.drag_leave());
    finish(hwnd);
}

pub fn transfer(hwnd: HWND, sources: Vec<String>, into: String, mv: bool) {
    shellops::transfer(hwnd, WM_APP_OPDONE, sources, into, mv);
}

// ---------------------------------------------------------------------------
// Window procedure

fn frame_y(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = shellops::menu_message(msg, wp, lp) {
        return r;
    }
    if get(hwnd).is_none() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    match handle(hwnd, msg, wp, lp) {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => {
            // Keep Windows' left, right and bottom frame (resize borders and
            // shadow) but give the caption's space to our toolbar.
            let p = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
            let top = p.rgrc[0].top;
            let r = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            p.rgrc[0].top = top;
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                // A maximized window hangs over the screen edge by its frame.
                p.rgrc[0].top += frame_y(hwnd);
            }
            Some(r)
        }
        WM_NCHITTEST => {
            let r = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            if r.0 as u32 != HTCLIENT {
                return Some(r);
            }
            let (sx, sy) = point(lp);
            let mut p = POINT { x: sx, y: sy };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
            }
            let zoomed = unsafe { IsZoomed(hwnd) }.as_bool();
            let border = frame_y(hwnd);
            if !zoomed && p.y < border {
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut rc);
                }
                let code = if p.x < border * 2 { HTTOPLEFT } else if p.x >= rc.right - border * 2 { HTTOPRIGHT } else { HTTOP };
                return Some(LRESULT(code as isize));
            }
            let hit = with_win(hwnd, |w, sh| w.b.hit(p.x as f32, p.y as f32, &sh.ctx.text)).unwrap_or(Hit::None);
            let code = match hit {
                Hit::Caption => HTCAPTION,
                // Windows 11 shows its snap layouts over a maximize button.
                Hit::Light(2) => HTMAXBUTTON,
                _ => HTCLIENT,
            };
            Some(LRESULT(code as isize))
        }
        WM_NCLBUTTONDOWN if wp.0 as u32 == HTMAXBUTTON => {
            with_win(hwnd, |w, _| w.zoom_down = true);
            Some(LRESULT(0))
        }
        WM_NCLBUTTONUP if wp.0 as u32 == HTMAXBUTTON => {
            let zoom = with_win(hwnd, |w, _| std::mem::take(&mut w.zoom_down)).unwrap_or(false);
            if zoom {
                perform(hwnd, vec![Action::Zoom]);
            }
            Some(LRESULT(0))
        }
        WM_NCMOUSEMOVE => {
            let (sx, sy) = point(lp);
            let mut p = POINT { x: sx, y: sy };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
            }
            with_win(hwnd, |w, sh| w.b.mouse_move(p.x as f32, p.y as f32, &sh.ctx.text));
            finish(hwnd);
            None
        }
        WM_NCMOUSELEAVE => {
            with_win(hwnd, |w, _| {
                w.zoom_down = false;
                w.b.mouse_leave();
            });
            finish(hwnd);
            None
        }
        WM_GETMINMAXINFO => {
            let s = scale_of(hwnd);
            let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = (600.0 * s) as i32;
            mmi.ptMinTrackSize.y = (360.0 * s) as i32;
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            Some(LRESULT(0))
        }
        WM_SIZE => {
            let (w, h) = ((lp.0 & 0xffff) as i32, ((lp.0 >> 16) & 0xffff) as i32);
            let s = scale_of(hwnd);
            let zoomed = unsafe { IsZoomed(hwnd) }.as_bool();
            with_win(hwnd, |win, sh| {
                if w > 0 && h > 0 {
                    win.resize(w, h, s);
                    win.b.maximized = zoomed;
                    if !zoomed && wp.0 == 0 {
                        sh.opts.w = (w as f32 / s) as i32;
                        sh.opts.h = (h as f32 / s) as i32;
                    }
                }
            });
            finish(hwnd);
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            let done = with_win(hwnd, |w, sh| w.paint(sh));
            // Busy (a re-entrant paint): let Windows validate; `after` repaints.
            done.map(|_| LRESULT(0))
        }
        WM_ACTIVATE => {
            let active = wp.0 & 0xffff != 0;
            with_win(hwnd, |w, _| {
                w.b.active = active;
                w.b.dirty = true;
            });
            finish(hwnd);
            None
        }
        WM_TIMER => {
            match wp.0 {
                T_ANIM => {
                    with_win(hwnd, |w, _| {
                        let dt = w.last_tick.elapsed().as_secs_f32();
                        w.last_tick = Instant::now();
                        w.b.tick(dt);
                    });
                    finish(hwnd);
                }
                T_REFRESH => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), T_REFRESH);
                    }
                    let busy = with_win(hwnd, |w, _| w.b.load == Load::Loading).unwrap_or(true);
                    if !busy {
                        perform(hwnd, vec![Action::Refresh]);
                    }
                }
                _ => {}
            }
            Some(LRESULT(0))
        }
        m if m == WM_APP_ENUM => {
            with_win(hwnd, |w, sh| w.batches(sh));
            finish(hwnd);
            Some(LRESULT(0))
        }
        m if m == WM_APP_CHANGED => {
            with_win(hwnd, |w, _| {
                if wp.0 as u64 == w.watched {
                    w.schedule_refresh();
                }
            });
            Some(LRESULT(0))
        }
        m if m == WM_APP_OPDONE => {
            with_win(hwnd, |w, _| w.schedule_refresh());
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lp);
            let actions = with_win(hwnd, |w, sh| {
                if !w.tracking {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                    w.tracking = true;
                }
                w.b.mouse_move(x as f32, y as f32, &sh.ctx.text)
            });
            perform(hwnd, actions.unwrap_or_default());
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with_win(hwnd, |w, _| {
                w.tracking = false;
                w.b.mouse_leave();
            });
            finish(hwnd);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = point(lp);
            unsafe { SetCapture(hwnd) };
            let m = mods();
            let actions = with_win(hwnd, |w, sh| w.b.mouse_down(x as f32, y as f32, m, msg == WM_LBUTTONDBLCLK, &sh.ctx.text));
            perform(hwnd, actions.unwrap_or_default());
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lp);
            unsafe {
                let _ = ReleaseCapture();
            }
            let actions = with_win(hwnd, |w, sh| w.b.mouse_up(x as f32, y as f32, &sh.ctx.text));
            perform(hwnd, actions.unwrap_or_default());
            Some(LRESULT(0))
        }
        WM_RBUTTONUP => {
            let (x, y) = point(lp);
            let actions = with_win(hwnd, |w, sh| w.b.right_click(x as f32, y as f32, &sh.ctx.text));
            perform(hwnd, actions.unwrap_or_default());
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32;
            let mut lines = 3u32;
            unsafe {
                let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            }
            // A notch (120) animates; a precision touchpad sends small
            // deltas that apply at once so the content tracks the fingers.
            let smooth = delta as i32 % 120 == 0;
            with_win(hwnd, |w, _| {
                let line = 20.0 * w.b.scale;
                w.b.wheel(-delta / 120.0 * lines.clamp(1, 20) as f32 * line, smooth);
            });
            finish(hwnd);
            Some(LRESULT(0))
        }
        WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
            let text_cursor = with_win(hwnd, |w, _| matches!(w.b.hover(), Hit::Search) || (w.b.renaming().is_some() && matches!(w.b.hover(), Hit::Item(_))))
                .unwrap_or(false);
            unsafe {
                SetCursor(LoadCursorW(None, if text_cursor { IDC_IBEAM } else { IDC_ARROW }).ok());
            }
            Some(LRESULT(1))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let key = wp.0 as u32;
            let m = mods();
            // Text clipboard inside the search and rename fields.
            if m.ctrl && !m.alt && matches!(key, vk::C | vk::X | vk::V) {
                let editing = with_win(hwnd, |w, _| w.b.editing().then(|| w.b.field_selection_text()).flatten()).flatten();
                if let Some(sel) = editing {
                    match key {
                        vk::V => {
                            if let Some(t) = super::super::system::clipboard_text(hwnd) {
                                with_win(hwnd, |w, _| w.b.paste_text(&t));
                            }
                        }
                        _ => {
                            if !sel.is_empty() {
                                super::super::system::set_clipboard(hwnd, &sel);
                            }
                            if key == vk::X {
                                with_win(hwnd, |w, _| w.b.key(key, m));
                            }
                        }
                    }
                    finish(hwnd);
                    return Some(LRESULT(0));
                }
            }
            const VK_APPS: u32 = 0x5D;
            const VK_F10: u32 = 0x79;
            let r = if key == VK_APPS || (key == VK_F10 && m.shift) {
                with_win(hwnd, |w, _| Some(w.b.menu_key())).flatten()
            } else {
                with_win(hwnd, |w, _| w.b.key(key, m)).flatten()
            };
            match r {
                Some(actions) => {
                    perform(hwnd, actions);
                    Some(LRESULT(0))
                }
                None => None,
            }
        }
        WM_CHAR => {
            let unit = wp.0 as u16;
            let c = with_win(hwnd, |w, _| {
                if (0xD800..0xDC00).contains(&unit) {
                    w.surrogate = Some(unit);
                    return None;
                }
                let s = match w.surrogate.take() {
                    Some(hi) if (0xDC00..0xE000).contains(&unit) => String::from_utf16_lossy(&[hi, unit]),
                    _ => String::from_utf16_lossy(&[unit]),
                };
                s.chars().next()
            })
            .flatten();
            if let Some(c) = c {
                let actions = with_win(hwnd, |w, _| w.b.char(c));
                perform(hwnd, actions.unwrap_or_default());
            }
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            let what = if lp.0 != 0 { unsafe { PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default() } } else { String::new() };
            if what == "ImmersiveColorSet" && with_shared(|s| s.update_theme()) == Some(true) {
                let dark = with_shared(|s| s.ctx.theme.dark).unwrap_or(false);
                for w in all() {
                    let h = w.borrow().hwnd;
                    set_dark(h, dark);
                    with_win(h, |w, _| w.b.dirty = true);
                    finish(h);
                }
            }
            None
        }
        WM_DEVICECHANGE => {
            // DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE: drives came or went.
            if matches!(wp.0, 0x8000 | 0x8004) {
                with_shared(|s| s.refresh_drives());
            }
            None
        }
        WM_DESTROY => {
            unsafe {
                let _ = RevokeDragDrop(hwnd);
                let _ = KillTimer(Some(hwnd), T_ANIM);
                let _ = KillTimer(Some(hwnd), T_REFRESH);
            }
            if let Some(w) = get(hwnd)
                && let Ok(mut w) = w.try_borrow_mut()
            {
                w.watcher = None;
                w.current.store(u64::MAX, Ordering::Relaxed);
                let (n, total, worst) = w.frames;
                if n > 0 {
                    trace(format!("window: closed, render avg {:.1} ms worst {:.1} ms over {n} frames", total / n as f64, worst));
                }
            }
            WINDOWS.with(|m| m.borrow_mut().remove(&hwnd_to_isize(hwnd)));
            with_shared(|s| s.save_options());
            if WINDOWS.with(|m| m.borrow().is_empty()) {
                unsafe { PostQuitMessage(0) };
            }
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}
