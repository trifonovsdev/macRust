//! The shell: owns the four layered windows (menu bar, dock, search, menu),
//! routes Win32 messages to the UI components and turns their events into
//! actions on the system.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use macrust_ui::dock::{Dock, DockConfig, DockEntry, DockEvent, Fallback, Section};
use macrust_ui::genie::{Path as GeniePath, Rect as GenieRect};
use macrust_ui::gfx::Image;
use macrust_ui::menu::{MenuEntry, MenuEvent, PopupMenu};
use macrust_ui::menubar::{BarEvent, BarItem, MenuBar, Status};
use macrust_ui::search::{self, Index, Kind};
use macrust_ui::spotlight::{Glyph, Row, SpotEvent, Spotlight};
use macrust_ui::strings::Strings;
use macrust_ui::text::Text;
use macrust_ui::theme::Theme;
use macrust_ui::tiny_skia::Pixmap;
use macrust_ui::wallpaper::{self, Vibrancy};
use macrust_ui::{Ctx, Key, Mods, WinRect};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MOD_ALT, MOD_NOREPEAT, RegisterHotKey, SetFocus, UnregisterHotKey, VIRTUAL_KEY, VK_BACK, VK_CONTROL,
    VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE,
    VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHEmptyRecycleBinW};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GA_PARENT, GA_ROOTOWNER, GetAncestor,
    GetDesktopWindow, GetForegroundWindow, HHOOK,
    KBDLLHOOKSTRUCT, KillTimer, MSLLHOOKSTRUCT, OBJID_WINDOW, PostMessageW, PostQuitMessage, SetForegroundWindow,
    SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_ACTIVATE, WM_APP, WM_CHAR, WM_CLOSE, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_ENDSESSION, WM_HOTKEY, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCLBUTTONDOWN, WM_NCRBUTTONDOWN,
    WM_POWERBROADCAST, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SYSKEYDOWN, WM_TIMER, WS_EX_NOACTIVATE,
};
use windows::core::PCWSTR;

use super::catalog::{self, Batch};
use super::config::{self, Pin, Settings, Source};
use super::genie::{self, Genie};
use super::icons::{self, IconWorker, Request};
use super::macicons::{self, MacIcons};
use super::minbutton::{self, WM_APP_PRESS, WM_APP_RELEASE};
use super::surface::Surface;
use super::system::{self, Network, Power, Screen, WM_APPBAR};
use super::tracker::{self, RunningApp};
use super::update;
use super::workarea;
use super::util::{known_folder, log, trace};
use super::winops;

/// Not re-exported by WindowsAndMessaging in this crate version.
const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_APP_WINDOWS: u32 = WM_APP + 1;
pub const WM_APP_FOREGROUND: u32 = WM_APP + 2;
pub const WM_APP_ICON: u32 = WM_APP + 3;
pub const WM_APP_CATALOG: u32 = WM_APP + 4;
pub const WM_APP_DISMISS: u32 = WM_APP + 5;
pub const WM_APP_MENUKEY: u32 = WM_APP + 6;
/// A window starts or ends being minimized: WPARAM is the HWND, LPARAM the
/// event's GetTickCount shifted left by one, plus 1 for start.
pub const WM_APP_MINIMIZE: u32 = WM_APP + 7;
pub const WM_APP_TASKBAR: u32 = WM_APP + 8;
// WM_APP + 10 is system::WM_APPBAR.
pub const WM_APP_TRAY: u32 = WM_APP + 11;
pub const WM_APP_MEDIA: u32 = WM_APP + 12;
pub const WM_APP_NOTES: u32 = WM_APP + 13;
pub const WM_APP_UPDATE: u32 = WM_APP + 14;
pub const WM_APP_WIFI: u32 = WM_APP + 15;
// WM_APP + 17 and + 18 are minbutton::WM_APP_PRESS / WM_APP_RELEASE.

#[path = "panels.rs"]
mod panels;
#[path = "report.rs"]
mod report;

/// Render time of one surface, for the diagnostic report.
#[derive(Default)]
struct Stat {
    n: u64,
    total: f64,
    worst: f64,
}

const TIMER_CLOCK: usize = 1;
const TIMER_STATUS: usize = 2;
const TIMER_CARET: usize = 3;
const TIMER_WINDOWS: usize = 4;
const TIMER_BANNER: usize = 5;
const TIMER_REPORT: usize = 6;
const TIMER_BACKDROP: usize = 7;
/// One-shot: checks the work area once Explorer has applied the app bars.
const TIMER_WORKAREA: usize = 8;
/// Live glass refresh: 30 Hz is smooth enough for what moves behind glass.
const BACKDROP_MS: u32 = 33;
const HOTKEY_SEARCH: i32 = 1;

const KEY_DOWNLOADS: &str = "special:downloads";
const KEY_TRASH: &str = "special:trash";

// Menu item tags.
const T_ABOUT: u32 = 1;
const T_SETTINGS: u32 = 2;
const T_STORE: u32 = 3;
const T_FORCE_QUIT: u32 = 4;
const T_SLEEP: u32 = 5;
const T_RESTART: u32 = 6;
const T_SHUTDOWN: u32 = 7;
const T_LOCK: u32 = 8;
const T_LOGOUT: u32 = 9;
const T_QUIT_SHELL: u32 = 10;
const T_REPORT: u32 = 11;
const T_CHECK_UPDATES: u32 = 12;
const T_INSTALL_UPDATE: u32 = 13;
const T_CONFIRM: u32 = 20;
const T_CANCEL: u32 = 21;
const T_HIDE: u32 = 30;
const T_HIDE_OTHERS: u32 = 31;
const T_SHOW_ALL: u32 = 32;
const T_QUIT_APP: u32 = 33;
const T_MINIMIZE: u32 = 40;
const T_ZOOM: u32 = 41;
const T_LEFT: u32 = 42;
const T_RIGHT: u32 = 43;
const T_CENTER: u32 = 44;
const T_BRING_ALL: u32 = 45;
const T_DOCK_OPEN: u32 = 50;
const T_DOCK_NEW: u32 = 51;
const T_DOCK_PIN: u32 = 52;
const T_DOCK_UNPIN: u32 = 53;
const T_DOCK_REVEAL: u32 = 54;
const T_DOCK_QUIT: u32 = 55;
const T_EMPTY_TRASH: u32 = 56;
const T_WINDOW_BASE: u32 = 1000;

// Shared with the hook callbacks, which run on this thread but outside `App`.
static WINDOWS_PENDING: AtomicBool = AtomicBool::new(false);
static NOTIFY: AtomicIsize = AtomicIsize::new(0);
static MENU_RECT: [AtomicI32; 4] = [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)];
static BAR_H: AtomicI32 = AtomicI32::new(0);
static MENU_FROM_BAR: AtomicBool = AtomicBool::new(false);
/// A panel (not a menu) is open: only Esc is taken from the keyboard.
static ESC_ONLY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Confirm {
    Restart,
    ShutDown,
    LogOut,
}

#[derive(Clone, PartialEq, Debug)]
enum MenuOwner {
    Bar(BarItem),
    Dock(String),
    Confirm(Confirm),
}

enum Anchor {
    /// Body top-left corner, screen px.
    Below(f32, f32),
    /// Body bottom-center, screen px.
    Above(f32, f32),
}

enum RowAction {
    Open { target: String, kind: Kind },
    Calc(String),
    Web(String),
}

#[derive(Clone)]
enum Special {
    None,
    Downloads(String),
    Trash,
}

#[derive(Clone)]
struct DockTarget {
    source: Option<Source>,
    pinned: bool,
    /// Key of the running app, when it has windows.
    app: Option<String>,
    /// The process runs without windows (closed to the tray): reopen without a bounce.
    alive: bool,
    special: Special,
    name: String,
}

pub struct App {
    ctx: Ctx,
    settings: Settings,
    pins: Vec<Pin>,
    screen: Screen,
    bar: MenuBar,
    bar_s: Surface,
    dock: Dock,
    dock_s: Surface,
    spot: Spotlight,
    spot_s: Surface,
    spot_clock: Instant,
    menu: Option<PopupMenu>,
    menu_s: Surface,
    menu_owner: Option<MenuOwner>,
    menu_windows: Vec<HWND>,
    running: Vec<RunningApp>,
    /// Apps whose windows are gone but whose process still runs (Telegram,
    /// Discord, OBS closed to the tray). They keep their dock icon, as on a Mac.
    lingering: Vec<RunningApp>,
    /// App keys in the order they first appeared: the dock keeps this order
    /// instead of the z-order, which changes with every click.
    order: Vec<String>,
    names: HashMap<String, String>,
    fg: Option<HWND>,
    fg_name: String,
    targets: HashMap<String, DockTarget>,
    dock_icons: HashMap<String, Rc<Image>>,
    /// Source pixels of the dock icons, to reshape them when the theme changes.
    dock_failed: HashMap<String, Instant>,
    last_appbar_place: Option<Instant>,
    small_icons: HashMap<String, Option<Rc<Image>>>,
    icon_worker: IconWorker,
    mac_icons: MacIcons,
    /// Dock icons from macOS artwork or the user's folder; a Windows icon
    /// arriving later must not replace them.
    mac_keys: HashSet<String>,
    /// Request id -> (cache key, is a dock icon).
    icon_reqs: HashMap<u64, (String, bool)>,
    icon_pending: HashSet<String>,
    next_req: u64,
    catalog: Receiver<Batch>,
    index: Index,
    row_actions: Vec<RowAction>,
    row_icon_keys: Vec<Option<String>>,
    net: Network,
    net_tick: u32,
    hooks: Vec<HHOOK>,
    last_frame: Option<Instant>,
    fullscreen: bool,
    taskbar_hidden: bool,
    taskbar_msg: u32,
    own_pid: u32,
    /// Tray, Control Center, Notification Center, banners (panels.rs).
    x: panels::Extras,
    /// Minimize into and restore out of Dock icons (genie.rs).
    genie: Genie,
    /// The Dock's glass was last built with the Genie drawn into it.
    genie_glass: bool,
    /// "SHELLHOOK": shell hook notices (HSHELL_GETMINRECT) to the menu bar window.
    shell_msg: u32,
    stats: std::collections::BTreeMap<&'static str, Stat>,
}

fn win_events_install() {
    unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event), 0, 0, flags);
        SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, None, Some(win_event), 0, 0, flags);
        SetWinEventHook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED, None, Some(win_event), 0, 0, flags);
        SetWinEventHook(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND, None, Some(win_event), 0, 0, flags);
    }
}

fn notify(msg: u32, wp: usize) -> bool {
    let h = HWND(NOTIFY.load(Ordering::Relaxed) as *mut _);
    unsafe { PostMessageW(Some(h), msg, WPARAM(wp), LPARAM(0)).is_ok() }
}

thread_local! {
    /// Top-level windows the dock knows about: destroy events of other windows are ignored.
    static KNOWN: std::cell::RefCell<HashSet<isize>> = std::cell::RefCell::new(HashSet::new());
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, id_child: i32, _: u32, ms: u32) {
    if event == EVENT_SYSTEM_FOREGROUND {
        notify(WM_APP_FOREGROUND, 0);
        return;
    }
    if id_object != OBJID_WINDOW.0 || id_child != 0 || hwnd.is_invalid() {
        return;
    }
    if event == EVENT_SYSTEM_MINIMIZESTART || event == EVENT_SYSTEM_MINIMIZEEND {
        let h = HWND(NOTIFY.load(Ordering::Relaxed) as *mut _);
        let start = (event == EVENT_SYSTEM_MINIMIZESTART) as isize;
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_MINIMIZE, WPARAM(hwnd.0 as usize), LPARAM((ms as isize) << 1 | start));
        }
        return;
    }
    if event == EVENT_OBJECT_SHOW && system::is_taskbar(hwnd) {
        notify(WM_APP_TASKBAR, 0);
        return;
    }
    // Only top-level windows change what the dock shows. A destroyed window has
    // no parent any more, so destroys count only for windows we listed.
    let relevant = if event == EVENT_OBJECT_DESTROY {
        KNOWN.with(|k| k.borrow().contains(&(hwnd.0 as isize)))
    } else {
        unsafe { GetAncestor(hwnd, GA_PARENT) == GetDesktopWindow() }
    };
    if relevant && !WINDOWS_PENDING.swap(true, Ordering::Relaxed) && !notify(WM_APP_WINDOWS, 0) {
        WINDOWS_PENDING.store(false, Ordering::Relaxed);
    }
}

/// Whether `len` bytes at `p` are committed, writable memory of this process.
fn writable(p: *const core::ffi::c_void, len: usize) -> bool {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery};
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQuery(Some(p), &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
        return false;
    }
    let end = mbi.BaseAddress as usize + mbi.RegionSize;
    mbi.State == MEM_COMMIT && (mbi.Protect.0 & (PAGE_READWRITE.0 | PAGE_WRITECOPY.0)) != 0 && p as usize + len <= end
}

fn in_menu(p: POINT) -> bool {
    let r: Vec<i32> = MENU_RECT.iter().map(|a| a.load(Ordering::Relaxed)).collect();
    p.x >= r[0] && p.x < r[2] && p.y >= r[1] && p.y < r[3]
}

/// While a menu is open: a click anywhere else closes it (the menu window never
/// takes focus, so there is no deactivation to watch for).
unsafe extern "system" fn ll_mouse(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let m = wp.0 as u32;
        if matches!(m, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN) {
            let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
            let on_bar = MENU_FROM_BAR.load(Ordering::Relaxed) && info.pt.y < BAR_H.load(Ordering::Relaxed);
            if !in_menu(info.pt) && !on_bar {
                notify(WM_APP_DISMISS, 0);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// While a menu is open: arrows, Enter and Esc drive it.
unsafe extern "system" fn ll_keyboard(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let info = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let vk = VIRTUAL_KEY(info.vkCode as u16);
        let wanted = if ESC_ONLY.load(Ordering::Relaxed) { vk == VK_ESCAPE } else { matches!(vk, VK_UP | VK_DOWN | VK_RETURN | VK_ESCAPE) };
        if wanted {
            notify(WM_APP_MENUKEY, vk.0 as usize);
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn lparam_point(lp: LPARAM) -> (f32, f32) {
    let x = (lp.0 & 0xffff) as u16 as i16 as f32;
    let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32;
    (x, y)
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn icon_size_for(scale: f32) -> i32 {
    let want = (30.0 * scale).ceil() as i32;
    [32, 48, 64, 96, 128].into_iter().find(|&s| s >= want).unwrap_or(128)
}

fn image_from(rgba: icons::Rgba, shadow: bool) -> Option<Rc<Image>> {
    let pm = system::pixmap_from_rgba(rgba.w, rgba.h, rgba.data)?;
    Some(Rc::new(if shadow { Image::with_shadow(pm) } else { Image::new(pm) }))
}

/// An app icon in the macOS 26 shape (squircle tile or free-standing art).
fn app_image(rgba: &icons::Rgba, shadow: bool) -> Option<Rc<Image>> {
    let pm = system::pixmap_from_rgba(rgba.w, rgba.h, rgba.data.clone())?;
    Some(shaped(pm, shadow))
}

fn shaped(pm: Pixmap, shadow: bool) -> Rc<Image> {
    Rc::new(match macrust_ui::gfx::mac_icon(&pm) {
        Some(m) if m.tile && shadow => Image::with_shadow(m.pixmap),
        // Free-standing art carries its own shadow.
        Some(m) => Image::new(m.pixmap),
        None if shadow => Image::with_shadow(pm),
        None => Image::new(pm),
    })
}

/// macOS artwork already has the macOS shape and shadow; square art is cut.
fn mac_image(art: macicons::Art) -> Rc<Image> {
    match art {
        macicons::Art::AsIs(pm) => Rc::new(Image::new(pm)),
        macicons::Art::Square(pm) => shaped(pm, true),
    }
}

fn load_fonts(text: &mut Text) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for f in ["segoeui.ttf", "seguisym.ttf"] {
        if let Ok(bytes) = std::fs::read(format!("{windir}\\Fonts\\{f}"))
            && let Ok(font) = ab_glyph_font(bytes)
        {
            text.add_fallback(font);
        }
    }
}

fn ab_glyph_font(bytes: Vec<u8>) -> Result<macrust_ui::text::FontArc, ()> {
    macrust_ui::text::FontArc::try_from_vec(bytes).map_err(|_| ())
}

fn default_pins(names: &mut HashMap<String, String>) -> Vec<Pin> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".into());
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let exists = |p: &str| std::path::Path::new(p).exists();
    let has_app = |id: &str| icons::shell_item(&format!("shell:AppsFolder\\{id}")).is_some();
    let mut out: Vec<Source> = vec![Source::Exe(format!("{windir}\\explorer.exe"))];
    let browsers = [
        format!("{pf}\\Google\\Chrome\\Application\\chrome.exe"),
        format!("{local}\\Google\\Chrome\\Application\\chrome.exe"),
        format!("{pf}\\Mozilla Firefox\\firefox.exe"),
        format!("{pf86}\\Microsoft\\Edge\\Application\\msedge.exe"),
    ];
    if let Some(b) = browsers.iter().find(|b| exists(b)) {
        out.push(Source::Exe(b.clone()));
    }
    for (id, fallback) in [
        ("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", None),
        ("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App", Some(format!("{windir}\\notepad.exe"))),
        ("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", None),
        ("windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel", None),
        ("Microsoft.WindowsStore_8wekyb3d8bbwe!App", None),
    ] {
        if has_app(id) {
            out.push(Source::Aumid(id.into()));
        } else if let Some(f) = fallback
            && exists(&f)
        {
            out.push(Source::Exe(f));
        }
    }
    out.push(Source::Exe(format!("{windir}\\System32\\Taskmgr.exe")));
    out.into_iter()
        .map(|s| {
            let name = match &s {
                Source::Exe(p) => tracker::app_name(names, p, None),
                Source::Aumid(a) => tracker::display_name(&s.parse_name()).unwrap_or_else(|| a.clone()),
            };
            Pin { name, source: s }
        })
        .collect()
}

/// Our file manager's windows belong to the Explorer pin, which stands for
/// Finder; they win over Explorer's own windows for that tile.
fn files_pin(src: &Source, app: &RunningApp) -> bool {
    let ours = app.aumid.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(super::files::AUMID));
    ours && matches!(src, Source::Exe(p) if p.to_lowercase().ends_with("\\explorer.exe"))
}

fn pin_matches(src: &Source, app: &RunningApp) -> bool {
    if files_pin(src, app) {
        return true;
    }
    match src {
        Source::Exe(p) => app.exe.eq_ignore_ascii_case(p) || app.relaunch_exe.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(p)),
        Source::Aumid(a) => app.aumid.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(a)),
    }
}

impl App {
    pub fn new() -> Self {
        let ru = system::russian_ui();
        let strings: &'static Strings = if ru { &Strings::RU } else { &Strings::EN };
        let settings = config::load_settings();
        let dark = match settings.appearance.as_str() {
            "dark" => true,
            "light" => false,
            _ => system::system_dark(),
        };
        let mut text = Text::bundled();
        load_fonts(&mut text);
        let ctx = Ctx::new(text, Theme { dark }, strings);

        let bar_s = Surface::new(WS_EX_NOACTIVATE, "macRust menu bar");
        let dock_s = Surface::new(WS_EX_NOACTIVATE, "macRust dock");
        let spot_s = Surface::new(WINDOW_EX_STYLE(0), "macRust search");
        let menu_s = Surface::new(WS_EX_NOACTIVATE, "macRust menu");
        NOTIFY.store(bar_s.hwnd.0 as isize, Ordering::Relaxed);

        let icon_worker = IconWorker::spawn(bar_s.hwnd, WM_APP_ICON);
        let mac_icons = MacIcons::spawn(bar_s.hwnd, WM_APP_ICON);
        let extras = panels::Extras::new(bar_s.hwnd);
        let catalog = catalog::spawn(bar_s.hwnd, WM_APP_CATALOG, ru);
        let mut names = HashMap::new();
        let pins = config::load_pins().unwrap_or_else(|| {
            let p = default_pins(&mut names);
            config::save_pins(&p);
            p
        });
        let taskbar_msg = unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("TaskbarCreated")) };

        let cfg = DockConfig { icon: settings.icon_size, magnified: settings.magnified, magnify: settings.magnify };
        let genie = Genie::new(dock_s.hwnd, extras.capture.is_some());
        let mut app = Self {
            ctx,
            settings,
            pins,
            screen: system::primary_screen(),
            bar: MenuBar::new(),
            bar_s,
            dock: Dock::new(cfg),
            dock_s,
            spot: Spotlight::new(),
            spot_s,
            spot_clock: Instant::now(),
            menu: None,
            menu_s,
            menu_owner: None,
            menu_windows: Vec::new(),
            running: Vec::new(),
            lingering: Vec::new(),
            order: Vec::new(),
            names,
            fg: None,
            fg_name: String::new(),
            targets: HashMap::new(),
            dock_icons: HashMap::new(),
            dock_failed: HashMap::new(),
            last_appbar_place: None,
            small_icons: HashMap::new(),
            icon_worker,
            mac_icons,
            mac_keys: HashSet::new(),
            icon_reqs: HashMap::new(),
            icon_pending: HashSet::new(),
            next_req: 1,
            catalog,
            index: Index::default(),
            row_actions: Vec::new(),
            row_icon_keys: Vec::new(),
            net: Network::new(),
            net_tick: 0,
            hooks: Vec::new(),
            last_frame: None,
            fullscreen: false,
            taskbar_hidden: false,
            taskbar_msg,
            own_pid: unsafe { GetCurrentProcessId() },
            x: extras,
            genie,
            genie_glass: false,
            shell_msg: 0,
            stats: std::collections::BTreeMap::new(),
        };

        if app.settings.hide_taskbar {
            system::hide_taskbar();
            app.taskbar_hidden = true;
            system::spawn_watchdog();
        }
        app.extras_start();
        update::start(app.bar_s.hwnd, WM_APP_UPDATE);
        system::appbar_register(app.bar_s.hwnd);
        if app.settings.reserve_dock {
            system::appbar_register(app.dock_s.hwnd);
        }
        app.configure(true);
        app.refresh_windows();
        app.refresh_foreground();
        win_events_install();
        app.genie_hooks_install();
        unsafe {
            if RegisterHotKey(Some(app.bar_s.hwnd), HOTKEY_SEARCH, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32).is_err() {
                log("Alt+Space is taken by another program; open search from the menu bar");
            }
            SetTimer(Some(app.bar_s.hwnd), TIMER_STATUS, 1000, None);
            // A fresh diagnostic report after every start, once things settle.
            SetTimer(Some(app.bar_s.hwnd), TIMER_REPORT, 20_000, None);
            if app.x.capture.is_some() {
                SetTimer(Some(app.bar_s.hwnd), TIMER_BACKDROP, BACKDROP_MS, None);
            }
        }
        app.schedule_clock();
        app
    }

    /// Screen size, scale, wallpaper material and app-bar reservations.
    fn configure(&mut self, reload_wallpaper: bool) {
        self.screen = system::primary_screen();
        let (w, h, s) = (self.screen.w as f32, self.screen.h as f32, self.screen.scale);
        self.bar.set_screen(w, s, &self.ctx);
        self.dock.set_screen(w, h, s);
        self.spot.set_screen(w, h, s);
        BAR_H.store(self.bar.height_px(), Ordering::Relaxed);
        self.genie.set_top(self.bar.height_px());
        self.genie.prepare((self.screen.w, self.screen.h));
        if reload_wallpaper {
            let wall = system::wallpaper(self.screen.w as u32, self.screen.h as u32)
                .unwrap_or_else(|| wallpaper::procedural(self.screen.w as u32, self.screen.h as u32, self.ctx.theme.dark));
            self.ctx.vibrancy = Some(Vibrancy::new(&wall));
        }
        self.bar.update_contrast(&self.ctx);
        self.place_appbars();
        self.bar_s.dirty = true;
        self.dock_s.dirty = true;
        self.spot_s.dirty = true;
    }

    fn place_appbars(&mut self) {
        self.last_appbar_place = Some(Instant::now());
        system::appbar_place(self.bar_s.hwnd, true, &self.screen, self.bar.height_px());
        if self.settings.reserve_dock {
            system::appbar_place(self.dock_s.hwnd, false, &self.screen, self.dock.reserved_height());
        }
        unsafe {
            SetTimer(Some(self.bar_s.hwnd), TIMER_WORKAREA, 500, None);
        }
    }

    /// Makes sure maximized windows end under the menu bar and above the Dock,
    /// whether or not Explorer applied our app bars (see `workarea`).
    fn check_work_area(&mut self, why: &str) {
        if self.fullscreen {
            return;
        }
        let bottom = if self.settings.reserve_dock { self.dock.reserved_height() } else { 0 };
        let (wa, forced) = workarea::ensure(self.bar.height_px(), bottom);
        let moved = workarea::fit_maximized(wa);
        if forced || moved > 0 {
            log(format!("work area ({why}): set by macRust {forced}, maximized windows refitted {moved}"));
        }
    }

    fn schedule_clock(&self) {
        let (_, _, _, _, _, sec) = system::local_time();
        let ms = (60 - sec.min(59)) * 1000 + 50;
        unsafe {
            SetTimer(Some(self.bar_s.hwnd), TIMER_CLOCK, ms, None);
        }
    }

    pub fn shutdown(&mut self) {
        self.genie.shutdown();
        if self.shell_msg != 0 {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DeregisterShellHookWindow(self.bar_s.hwnd);
            }
        }
        // Apps fall back to Explorer's tray, which got every icon forwarded.
        self.x.tray = None;
        self.unhook_menu();
        unsafe {
            let _ = UnregisterHotKey(Some(self.bar_s.hwnd), HOTKEY_SEARCH);
        }
        system::appbar_remove(self.bar_s.hwnd);
        if self.settings.reserve_dock {
            system::appbar_remove(self.dock_s.hwnd);
        }
        workarea::release();
        if self.taskbar_hidden {
            system::restore_taskbar();
            self.taskbar_hidden = false;
        }
        // Explorer re-reserves its taskbar asynchronously.
        std::thread::sleep(std::time::Duration::from_millis(300));
        workarea::fit_maximized(workarea::current());
    }

    fn quit(&mut self) {
        self.shutdown();
        unsafe { PostQuitMessage(0) };
    }

    // -----------------------------------------------------------------------
    // Windows, apps and the dock

    /// The cached dock icon; a missing one is requested (macOS artwork first,
    /// then the Windows icon) and the dock is rebuilt when it arrives.
    /// Failures are retried after a minute.
    fn dock_icon(&mut self, parse_name: &str) -> Option<Rc<Image>> {
        let key = parse_name.to_lowercase();
        if let Some(i) = self.dock_icons.get(&key) {
            return Some(i.clone());
        }
        if self.dock_failed.get(&key).is_some_and(|t| t.elapsed().as_secs() < 60) || self.icon_pending.contains(&key) {
            return None;
        }
        self.icon_pending.insert(key.clone());
        self.mac_icons.request(key, parse_name.to_string());
        None
    }

    fn mac_icons_arrived(&mut self) -> bool {
        let mut changed = false;
        while let Ok(r) = self.mac_icons.rx.try_recv() {
            match r.art {
                Some(art) => {
                    self.icon_pending.remove(&r.key);
                    self.dock_icons.insert(r.key.clone(), mac_image(art));
                    self.mac_keys.insert(r.key);
                    changed = true;
                }
                None if !r.fetched => {
                    // Nothing on disk: the Windows icon now, macOS artwork if a download lands.
                    let id = self.next_req;
                    self.next_req += 1;
                    self.icon_reqs.insert(id, (r.key, true));
                    self.icon_worker.request(Request { id, parse_name: r.parse_name, size: 256, thumbnail: false });
                }
                None => {}
            }
        }
        changed
    }

    fn refresh_windows(&mut self) {
        let previous = std::mem::take(&mut self.running);
        self.running = tracker::snapshot(&mut self.names);
        let gone = previous.into_iter().filter(|a| !self.running.iter().any(|r| r.key == a.key));
        let mut lingering: Vec<RunningApp> = std::mem::take(&mut self.lingering)
            .into_iter()
            .chain(gone)
            .filter(|a| !self.running.iter().any(|r| r.key == a.key))
            .collect();
        lingering.dedup_by(|a, b| a.key == b.key);
        for a in &mut lingering {
            a.windows.clear();
            let exe = a.exe.clone();
            a.pids.retain(|&p| tracker::process_alive(p, &exe));
        }
        lingering.retain(|a| !a.pids.is_empty());
        self.lingering = lingering;
        let live: HashSet<&str> = self.running.iter().chain(&self.lingering).map(|a| a.key.as_str()).collect();
        self.order.retain(|k| live.contains(k.as_str()));
        for a in self.running.iter().chain(&self.lingering) {
            if !self.order.contains(&a.key) {
                self.order.push(a.key.clone());
            }
        }
        let rank = |k: &str, order: &[String]| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
        let order = self.order.clone();
        self.running.sort_by_key(|a| rank(&a.key, &order));
        self.lingering.sort_by_key(|a| rank(&a.key, &order));
        KNOWN.with(|k| {
            let mut k = k.borrow_mut();
            k.clear();
            k.extend(self.running.iter().flat_map(|a| a.windows.iter().map(|w| w.0 as isize)));
        });
        let live: HashSet<isize> = self.running.iter().flat_map(|a| a.windows.iter().map(|w| w.0 as isize)).collect();
        self.genie.forget_except(&live);
        self.rebuild_dock();
    }

    fn rebuild_dock(&mut self) {
        let mut entries = Vec::new();
        let mut targets = HashMap::new();
        let mut used = vec![false; self.running.len()];
        let mut used_lingering = vec![false; self.lingering.len()];
        for pin in self.pins.clone() {
            let idx = self
                .running
                .iter()
                .position(|a| files_pin(&pin.source, a))
                .or_else(|| self.running.iter().position(|a| pin_matches(&pin.source, a)));
            if let Some(i) = idx {
                used[i] = true;
            }
            let ling = self.lingering.iter().position(|a| pin_matches(&pin.source, a));
            if let Some(i) = ling {
                used_lingering[i] = true;
            }
            // Running only as a tray icon (Telegram, Discord closed to the tray).
            let in_tray = matches!(&pin.source, Source::Exe(p) if self.x.tray_exes.contains(&p.to_lowercase()));
            let key = pin.source.key();
            let icon = self.dock_icon(&pin.source.parse_name());
            entries.push(DockEntry {
                key: key.clone(),
                label: pin.name.clone(),
                icon,
                running: idx.is_some() || ling.is_some() || in_tray,
                section: Section::Apps,
                fallback: Fallback::App,
            });
            targets.insert(
                key,
                DockTarget {
                    source: Some(pin.source.clone()),
                    pinned: true,
                    alive: idx.is_none() && (ling.is_some() || in_tray),
                    app: idx.map(|i| self.running[i].key.clone()),
                    special: Special::None,
                    name: pin.name.clone(),
                },
            );
        }
        let unpinned = self.running.clone().into_iter().zip(used).filter(|(_, u)| !u).map(|(a, _)| (a, true));
        let unpinned_lingering =
            self.lingering.clone().into_iter().zip(used_lingering).filter(|(_, u)| !u).map(|(a, _)| (a, false));
        for (a, has_windows) in unpinned.chain(unpinned_lingering) {
            // An id that is not a registered app has no icon or name in
            // shell:AppsFolder; the program the window names for relaunch does.
            let source = match &a.aumid {
                Some(id) if a.registered => Source::Aumid(id.clone()),
                _ => Source::Exe(a.relaunch_exe.clone().unwrap_or_else(|| a.exe.clone())),
            };
            let icon = self.dock_icon(&source.parse_name());
            entries.push(DockEntry {
                key: a.key.clone(),
                label: a.name.clone(),
                icon,
                running: true,
                section: Section::Apps,
                fallback: Fallback::App,
            });
            targets.insert(
                a.key.clone(),
                DockTarget {
                    source: Some(source),
                    pinned: false,
                    alive: !has_windows,
                    app: has_windows.then(|| a.key.clone()),
                    special: Special::None,
                    name: a.name,
                },
            );
        }
        if let Some(dl) = known_folder(&FOLDERID_Downloads) {
            let path = dl.to_string_lossy().into_owned();
            // Downloads and Trash are drawn by the dock in the macOS 26 manner,
            // not taken from the Windows shell.
            entries.push(DockEntry {
                key: KEY_DOWNLOADS.into(),
                label: self.ctx.strings.downloads.into(),
                icon: None,
                running: false,
                section: Section::Stacks,
                fallback: Fallback::Folder,
            });
            targets.insert(
                KEY_DOWNLOADS.into(),
                DockTarget {
                    source: None,
                    pinned: true,
                    alive: false,
                    app: None,
                    special: Special::Downloads(path),
                    name: String::new(),
                },
            );
        }
        entries.push(DockEntry {
            key: KEY_TRASH.into(),
            label: self.ctx.strings.trash.into(),
            icon: None,
            running: false,
            section: Section::Stacks,
            fallback: Fallback::Trash,
        });
        targets.insert(
            KEY_TRASH.into(),
            DockTarget { source: None, pinned: true, alive: false, app: None, special: Special::Trash, name: String::new() },
        );
        self.targets = targets;
        self.dock.set_entries(entries);
        self.dock_s.dirty = true;
    }

    fn app_by_key(&self, key: &str) -> Option<&RunningApp> {
        self.running.iter().find(|a| a.key == key)
    }

    fn fg_app(&self) -> Option<&RunningApp> {
        let fg = self.fg?;
        self.running.iter().find(|a| a.windows.contains(&fg))
    }

    fn refresh_foreground(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_invalid() || tracker::window_pid(fg) == self.own_pid {
            return;
        }
        let (exe, _) = tracker::identify(fg);
        trace(format!("foreground: {} class {}", exe.as_deref().map(report::file_name).unwrap_or_default(), tracker::class_name(fg)));
        let root = unsafe { GetAncestor(fg, GA_ROOTOWNER) };
        let root = if root.is_invalid() { fg } else { root };
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        if tracker::is_desktop_class(&tracker::class_name(root)) {
            self.fg = None;
            self.fg_name = tracker::app_name(&mut self.names, &format!("{windir}\\explorer.exe"), None);
        } else {
            self.fg = Some(root);
            self.genie.note_window(root);
            let (exe, aumid) = tracker::identify(root);
            self.fg_name = exe.map(|e| tracker::app_name(&mut self.names, &e, aumid.as_deref())).unwrap_or_default();
        }
        self.refresh_status();
    }

    fn refresh_status(&mut self) {
        let (wd, day, mon, h, m, _) = system::local_time();
        let lang_window = self.fg.unwrap_or_else(|| unsafe { GetForegroundWindow() });
        self.net_tick += 1;
        let wifi = if self.net_tick % 5 == 1 { self.net.connected() } else { self.bar.status().wifi };
        let st = Status {
            app: self.fg_name.clone(),
            lang: system::input_language(lang_window),
            battery: system::battery(),
            wifi,
            clock: self.ctx.strings.clock(wd, day, mon, h, m),
            mic: self.x.privacy.0,
            camera: self.x.privacy.1,
        };
        if self.bar.set_status(st, &self.ctx) {
            self.bar_s.dirty = true;
            self.tray_rects();
        }
    }

    fn launch(&mut self, key: &str, src: &Source) {
        system::open(&src.parse_name());
        if self.settings.launch_bounce {
            self.dock.bounce(key);
        }
    }

    fn dock_click(&mut self, key: &str) {
        let n = self.targets.get(key).and_then(|t| t.app.as_deref()).and_then(|k| self.app_by_key(k)).map(|a| {
            let min = a.windows.iter().filter(|&&w| winops::is_minimized(w)).count();
            format!("{} windows, {min} minimized", a.windows.len())
        });
        trace(format!("dock click {}: {}", report::file_name(key), n.unwrap_or_else(|| "not running".into())));
        let Some(t) = self.targets.get(key).cloned() else { return };
        match t.special {
            Special::Downloads(p) => system::open(&p),
            Special::Trash => system::open("shell:RecycleBinFolder"),
            Special::None => {
                let app = t.app.as_deref().and_then(|k| self.app_by_key(k)).cloned();
                match (app, t.source) {
                    // The owner's rule, unlike a Mac: clicking the app in front
                    // minimizes its windows into the icon; all minimized come
                    // back out of it; any other app comes forward.
                    (Some(app), _) => {
                        let shown: Vec<HWND> = app.windows.iter().copied().filter(|&w| !winops::is_minimized(w)).collect();
                        let front = self.fg_app().is_some_and(|f| f.key == app.key);
                        if shown.is_empty() {
                            self.genie_restore(key, &app.windows);
                        } else if front {
                            self.genie_minimize(key, &shown, None, "dock");
                        } else {
                            for &w in shown.iter().skip(1).rev() {
                                winops::raise(w);
                            }
                            winops::activate(shown[0]);
                        }
                    }
                    // Running in the tray: a second launch makes single-instance
                    // apps show their window, which is what the Dock does.
                    (None, Some(src)) if t.alive => system::open(&src.parse_name()),
                    (None, Some(src)) => self.launch(key, &src),
                    _ => {}
                }
            }
        }
    }

    /// The Dock entry that stands for the app owning window `h`.
    fn dock_key_of(&self, h: HWND) -> Option<String> {
        let app = self.running.iter().find(|a| a.windows.contains(&h))?;
        self.targets.iter().find(|(_, t)| t.app.as_deref() == Some(app.key.as_str())).map(|(k, _)| k.clone())
    }

    /// The window frame `win` flowing into the Dock icon of `key`; None when
    /// the Genie cannot play (Dock hidden for a full-screen app, window off
    /// the primary screen, icon gone).
    fn genie_path(dock: &Dock, screen: &Screen, fullscreen: bool, key: &str, win: WinRect) -> Option<GeniePath> {
        let (cx, cy) = (win.x + win.w / 2, win.y + win.h / 2);
        if fullscreen || win.w <= 0 || win.h <= 0 || !(0..screen.w).contains(&cx) || !(0..screen.h).contains(&cy) {
            return None;
        }
        let (x, y, w, h) = dock.icon_rect(key)?;
        Some(GeniePath {
            win: GenieRect { x: win.x as f32, y: win.y as f32, w: win.w as f32, h: win.h as f32 },
            icon: GenieRect { x, y, w, h },
            clip: dock.bar_top(),
            screen_h: screen.h as f32,
            dock_visible: true,
            scale: screen.scale,
        })
    }

    /// Minimizes `windows` into the Dock icon of `key` with the Genie where it
    /// can play, plainly otherwise. `event`: Windows is already minimizing
    /// them (a title-bar click) since this GetTickCount, so the snapshot comes
    /// from a frame captured before it.
    fn genie_minimize(&mut self, key: &str, windows: &[HWND], event: Option<u32>, why: &str) {
        let since = Instant::now();
        let mut items = Vec::new();
        for &h in windows {
            let item = if self.genie.enabled { self.genie_item(key, h, event) } else { None };
            match item {
                Some(i) => items.push(i),
                None if event.is_none() => winops::minimize(h),
                None => trace("genie skipped: no frame from before the minimize"),
            }
        }
        self.genie.minimize(items, event.is_some(), (self.screen.w, self.screen.h), why, since);
    }

    fn genie_item(&mut self, key: &str, h: HWND, event: Option<u32>) -> Option<genie::Item> {
        let (frame, maxed) = if event.is_some() || winops::is_minimized(h) {
            winops::restore_frame(h, self.genie.borders_of(h))?
        } else {
            (winops::frame_rect(h)?, winops::is_maximized(h))
        };
        let path = Self::genie_path(&self.dock, &self.screen, self.fullscreen, key, frame)?;
        // Windows 11 rounds the corners of windows that are not maximized.
        let radius = if self.genie.win11 && !maxed { 8.0 * self.screen.scale } else { 0.0 };
        let snap = genie::grab(self.x.capture.as_mut(), h, frame, event, radius)?;
        Some(genie::Item { hwnd: h, path, snap })
    }

    /// Restores `windows` out of the Dock icon of `key`; the first comes to the front.
    fn genie_restore(&mut self, key: &str, windows: &[HWND]) {
        let screen = (self.screen.w, self.screen.h);
        // The one to activate goes last, so it ends up in front.
        for (i, &h) in windows.iter().enumerate().rev() {
            let (dock, scr, full) = (&self.dock, &self.screen, self.fullscreen);
            let path = |r: WinRect| Self::genie_path(dock, scr, full, key, r);
            self.genie.restore(h, &path, i == 0, screen);
        }
    }

    /// A window started (`start`) or stopped being minimized, by any means;
    /// `tick` is the event's GetTickCount.
    fn minimize_event(&mut self, h: HWND, start: bool, tick: u32) {
        if !start {
            self.genie.restored_elsewhere(h);
            return;
        }
        if self.genie.busy(h) || !self.genie.session() {
            return;
        }
        let late = unsafe { windows::Win32::System::SystemInformation::GetTickCount() }.wrapping_sub(tick);
        if self.genie.armed() == Some(h) {
            // The release went by without the hook waiting (the frame was
            // still being staged): the staged frame is the quickest cover.
            trace(format!("genie event: minimize of the armed window, handled {late} ms after it"));
            self.genie.fire(h, "the minimize event", Instant::now());
            minbutton::set_armed(None);
            return;
        }
        if let Some(key) = self.dock_key_of(h) {
            trace(format!("genie event: minimize seen only after Windows hid the window, handled {late} ms after it"));
            self.genie_minimize(&key, &[h], Some(tick), "title bar");
        }
    }

    // -----------------------------------------------------------------------
    // Genie: minimizes started outside the Dock, seen before the window hides

    fn genie_hooks_install(&mut self) {
        if !self.genie.session() {
            return;
        }
        minbutton::start(self.bar_s.hwnd);
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{RegisterShellHookWindow, RegisterWindowMessageW};
            if RegisterShellHookWindow(self.bar_s.hwnd).as_bool() {
                self.shell_msg = RegisterWindowMessageW(windows::core::w!("SHELLHOOK"));
            }
        }
        log(format!("genie: mouse hook started, shell hook {}", if self.shell_msg != 0 { "registered" } else { "refused" }));
    }

    /// The top-level window at screen `x, y` and what part of it is there
    /// (WM_NCHITTEST), when it is a window in the Dock; 0 if it did not
    /// answer within 30 ms.
    fn hit_test(&self, x: i32, y: i32) -> Option<(HWND, u32)> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_NCHITTEST, WindowFromPoint,
        };
        let h = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
        if h.is_invalid() || tracker::window_pid(h) == self.own_pid || self.dock_key_of(h).is_none() {
            return None;
        }
        let lp = LPARAM(((y as u16 as u32) << 16 | x as u16 as u32) as i32 as isize);
        let mut hit = 0usize;
        // Blocking: no other message of ours may run while the app answers.
        let ok = unsafe {
            SendMessageTimeoutW(h, WM_NCHITTEST, WPARAM(0), lp, SMTO_BLOCK | SMTO_ABORTIFHUNG, 30, Some(&mut hit))
        };
        Some((h, if ok.0 == 0 { 0 } else { hit as u32 }))
    }

    /// The left button went down at `x, y`: on a minimize button of a window
    /// in the Dock, the Genie's first frame is staged for the release.
    fn press(&mut self, x: i32, y: i32) {
        use windows::Win32::UI::WindowsAndMessaging::HTMINBUTTON;
        self.genie.disarm();
        minbutton::set_armed(None);
        if !self.genie.session() || self.genie.animating() || self.fullscreen {
            return;
        }
        let Some((h, hit)) = self.hit_test(x, y) else { return };
        if hit != HTMINBUTTON || winops::is_minimized(h) {
            return;
        }
        let Some(key) = self.dock_key_of(h) else { return };
        let since = Instant::now();
        let Some(item) = self.genie_item(&key, h, None) else {
            trace("genie: minimize button pressed, no snapshot");
            return;
        };
        if self.genie.arm(item, (self.screen.w, self.screen.h)) {
            minbutton::set_armed(Some(h));
            trace(format!("genie armed: minimize button pressed, first frame staged in {:.1} ms", since.elapsed().as_secs_f32() * 1000.0));
        }
    }

    /// The left button went up while a first frame is staged; the hook holds
    /// the release until this returns. True if the frame went on screen.
    fn release(&mut self, x: i32, y: i32) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::HTMINBUTTON;
        let since = Instant::now();
        minbutton::set_armed(None);
        let Some(h) = self.genie.armed() else { return false };
        // DefWindowProc minimizes only on a release over the button.
        let on = self.hit_test(x, y).is_some_and(|(at, hit)| at == h && hit == HTMINBUTTON);
        if on && self.genie.fire(h, "the minimize button", since) {
            return true;
        }
        self.genie.disarm();
        trace("genie disarmed: released off the minimize button");
        false
    }

    /// HSHELL_GETMINRECT: Windows asks where a window minimizes to or
    /// restores from. It asks before it hides the window, and waits for the
    /// answer, so a minimize started here uncovers nothing. The answer, the
    /// Dock icon, also aims Windows' own animation where it plays.
    fn min_rect(&mut self, lp: LPARAM) -> LRESULT {
        /// SHELLHOOKINFO as Windows passes it: the rect has 16-bit fields,
        /// unlike the documented RECT (cairoshell/ManagedShell does the same).
        #[repr(C)]
        struct Info {
            hwnd: HWND,
            rc: [i16; 4],
        }
        let info = lp.0 as *mut Info;
        // The pointer is only valid here when Windows sent the notice (and
        // copied the struct into this process), never when it posted it.
        let sent = unsafe { windows::Win32::UI::WindowsAndMessaging::InSendMessage() }.as_bool();
        if info.is_null() || !sent || !writable(info as *const _, size_of::<Info>()) {
            trace("genie: minimize-rect notice not sent synchronously, ignored");
            return LRESULT(0);
        }
        let h = unsafe { (*info).hwnd };
        let Some(key) = self.dock_key_of(h) else { return LRESULT(0) };
        let iconic = winops::is_minimized(h);
        trace(format!("genie: Windows asks for the minimize rect of a {} window", if iconic { "minimized" } else { "shown" }));
        if !iconic && self.genie.session() && !self.genie.busy(h) && !self.fullscreen {
            let since = Instant::now();
            let armed = self.genie.armed() == Some(h)
                || self.genie_item(&key, h, None).is_some_and(|it| self.genie.arm(it, (self.screen.w, self.screen.h)));
            if armed {
                self.genie.fire(h, "the shell hook", since);
            }
            minbutton::set_armed(None);
        }
        let Some((x, y, w, hh)) = self.dock.icon_rect(&key) else { return LRESULT(0) };
        let c = |v: f32| v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        unsafe {
            (*info).rc = [c(x), c(y), c(x + w), c(y + hh)];
        }
        LRESULT(1)
    }

    // -----------------------------------------------------------------------
    // Menus

    fn hook_menu(&mut self) {
        if !self.hooks.is_empty() {
            return;
        }
        unsafe {
            if let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse), None, 0) {
                self.hooks.push(h);
            }
            if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard), None, 0) {
                self.hooks.push(h);
            }
        }
    }

    fn unhook_menu(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
        }
    }

    fn open_menu(&mut self, entries: Vec<MenuEntry>, owner: MenuOwner, anchor: Anchor) {
        let s = self.screen.scale;
        let mut m = PopupMenu::new(entries, 0, s, &self.ctx);
        let (w, h) = m.window_size();
        let (ox, oy) = m.body_offset();
        let (bw, bh) = m.body_size();
        let (bx, by) = match anchor {
            Anchor::Below(x, y) => (x, y),
            Anchor::Above(cx, bottom) => (cx - bw / 2.0, bottom - bh),
        };
        let bx = bx.clamp(4.0 * s, self.screen.w as f32 - bw - 4.0 * s).round() as i32;
        let by = by.max(0.0).round() as i32;
        let r = WinRect { x: bx - ox, y: by - oy, w, h };
        if let Some(behind) = system::capture(r) {
            m.set_backdrop(&behind);
        }
        self.menu_s.prepare(r);
        for (a, v) in MENU_RECT.iter().zip([bx, by, bx + bw as i32, by + bh as i32]) {
            a.store(v, Ordering::Relaxed);
        }
        MENU_FROM_BAR.store(matches!(owner, MenuOwner::Bar(_) | MenuOwner::Confirm(_)), Ordering::Relaxed);
        ESC_ONLY.store(false, Ordering::Relaxed);
        self.dock.set_menu_open(matches!(owner, MenuOwner::Dock(_)));
        self.menu = Some(m);
        self.menu_owner = Some(owner);
        self.menu_s.dirty = true;
        self.render();
        self.menu_s.raise();
        // Windows silently drops a low-level hook that was once too slow; a
        // fresh pair per menu keeps "click outside closes" working.
        self.unhook_menu();
        self.hook_menu();
    }

    /// Fades the open menu out (or drops it at once when another one replaces it).
    fn close_menu(&mut self, animated: bool) {
        self.unhook_menu();
        self.close_panels(animated);
        if let Some(m) = &mut self.menu {
            if animated {
                m.close();
            } else {
                self.menu = None;
                self.menu_s.hide();
            }
        }
        if matches!(self.menu_owner, Some(MenuOwner::Bar(_)) | Some(MenuOwner::Confirm(_))) {
            self.bar.set_open(None);
        }
        self.dock.set_menu_open(false);
        self.menu_owner = None;
        self.bar_s.dirty = true;
        self.dock_s.dirty = true;
        self.menu_s.dirty = true;
    }

    fn bar_menu(&self, item: BarItem) -> Vec<MenuEntry> {
        let s = self.ctx.strings;
        let app = self.fg_name.as_str();
        let has_fg = self.fg.is_some();
        match item {
            BarItem::Logo => vec![
                MenuEntry::item(s.about, T_ABOUT),
                MenuEntry::sep(),
                MenuEntry::item(s.settings, T_SETTINGS),
                MenuEntry::item(s.store, T_STORE),
                MenuEntry::sep(),
                MenuEntry::item(s.force_quit, T_FORCE_QUIT).shortcut("Ctrl+Shift+Esc"),
                MenuEntry::sep(),
                MenuEntry::item(s.sleep, T_SLEEP),
                MenuEntry::item(s.restart, T_RESTART),
                MenuEntry::item(s.shut_down, T_SHUTDOWN),
                MenuEntry::sep(),
                MenuEntry::item(s.lock, T_LOCK).shortcut("Win+L"),
                MenuEntry::item(s.log_out, T_LOGOUT),
                MenuEntry::sep(),
                self.check_updates_entry(),
                self.install_update_entry(),
                MenuEntry::sep(),
                MenuEntry::item(s.report, T_REPORT),
                MenuEntry::item(s.quit_shell, T_QUIT_SHELL),
            ],
            BarItem::App => vec![
                MenuEntry::item(s.with_app(s.hide, app), T_HIDE).enabled(has_fg),
                MenuEntry::item(s.hide_others, T_HIDE_OTHERS),
                MenuEntry::item(s.show_all, T_SHOW_ALL),
                MenuEntry::sep(),
                MenuEntry::item(s.with_app(s.quit, app), T_QUIT_APP).enabled(has_fg).shortcut("Alt+F4"),
            ],
            _ => vec![
                MenuEntry::item(s.minimize, T_MINIMIZE).enabled(has_fg).shortcut("Win+↓"),
                MenuEntry::item(s.zoom, T_ZOOM).enabled(has_fg),
                MenuEntry::sep(),
                MenuEntry::item(s.tile_left, T_LEFT).enabled(has_fg).shortcut("Win+←"),
                MenuEntry::item(s.tile_right, T_RIGHT).enabled(has_fg).shortcut("Win+→"),
                MenuEntry::item(s.center, T_CENTER).enabled(has_fg),
                MenuEntry::sep(),
                MenuEntry::item(s.bring_all, T_BRING_ALL).enabled(has_fg),
            ],
        }
    }

    fn check_updates_entry(&self) -> MenuEntry {
        let s = self.ctx.strings;
        match update::status() {
            update::Status::Checking => MenuEntry::item(s.checking_updates, T_CHECK_UPDATES).enabled(false),
            update::Status::Downloading(v) => MenuEntry::item(s.update_label(s.downloading_update, &v), T_CHECK_UPDATES).enabled(false),
            update::Status::UpToDate => MenuEntry::item(s.up_to_date, T_CHECK_UPDATES),
            update::Status::Failed => MenuEntry::item(s.update_failed, T_CHECK_UPDATES),
            update::Status::Idle => MenuEntry::item(s.check_updates, T_CHECK_UPDATES),
        }
    }

    fn install_update_entry(&self) -> MenuEntry {
        let s = self.ctx.strings;
        let version = update::ready_version();
        let label = s.update_label(s.install_update, version.as_deref().unwrap_or(""));
        MenuEntry::item(label, T_INSTALL_UPDATE).enabled(version.is_some())
    }

    fn dock_menu(&mut self, key: &str) -> Vec<MenuEntry> {
        let s = self.ctx.strings;
        let Some(t) = self.targets.get(key).cloned() else { return Vec::new() };
        self.menu_windows.clear();
        match t.special {
            Special::Downloads(_) => {
                vec![MenuEntry::item(s.open, T_DOCK_OPEN), MenuEntry::item(s.show_in_folder, T_DOCK_REVEAL)]
            }
            Special::Trash => vec![MenuEntry::item(s.open, T_DOCK_OPEN), MenuEntry::sep(), MenuEntry::item(s.empty_trash, T_EMPTY_TRASH)],
            Special::None => {
                let mut e = Vec::new();
                let app = t.app.as_deref().and_then(|k| self.app_by_key(k)).cloned();
                if let Some(app) = &app {
                    for (i, &w) in app.windows.iter().take(12).enumerate() {
                        let title = tracker::title(w);
                        e.push(MenuEntry::item(title, T_WINDOW_BASE + i as u32).checked(self.fg == Some(w)));
                        self.menu_windows.push(w);
                    }
                    e.push(MenuEntry::sep());
                }
                e.push(if t.pinned {
                    MenuEntry::item(s.remove_from_dock, T_DOCK_UNPIN)
                } else {
                    MenuEntry::item(s.keep_in_dock, T_DOCK_PIN)
                });
                if matches!(t.source, Some(Source::Exe(_))) {
                    e.push(MenuEntry::item(s.show_in_folder, T_DOCK_REVEAL));
                }
                e.push(MenuEntry::sep());
                if app.is_some() {
                    e.push(MenuEntry::item(s.new_window, T_DOCK_NEW));
                    e.push(MenuEntry::item(s.with_app(s.quit, &t.name), T_DOCK_QUIT));
                } else {
                    e.push(MenuEntry::item(s.open, T_DOCK_OPEN));
                }
                e
            }
        }
    }

    fn confirm(&mut self, c: Confirm) {
        let s = self.ctx.strings;
        let label = match c {
            Confirm::Restart => s.restart_now,
            Confirm::ShutDown => s.shut_down_now,
            Confirm::LogOut => s.log_out_now,
        };
        let x = 6.0 * self.screen.scale;
        let y = self.bar.height_px() as f32;
        self.bar.set_open(Some(BarItem::Logo));
        self.open_menu(
            vec![MenuEntry::item(label, T_CONFIRM), MenuEntry::sep(), MenuEntry::item(s.cancel, T_CANCEL)],
            MenuOwner::Confirm(c),
            Anchor::Below(x, y),
        );
    }

    fn handle_pick(&mut self, tag: u32, owner: Option<MenuOwner>) {
        let fg_windows: Vec<HWND> = self.fg_app().map(|a| a.windows.clone()).unwrap_or_default();
        match tag {
            T_ABOUT => system::open("ms-settings:about"),
            T_SETTINGS => system::open("ms-settings:"),
            T_STORE => system::open("ms-windows-store:"),
            T_FORCE_QUIT => system::open("taskmgr.exe"),
            T_SLEEP => system::power(Power::Sleep),
            T_LOCK => system::power(Power::Lock),
            T_RESTART => self.confirm(Confirm::Restart),
            T_SHUTDOWN => self.confirm(Confirm::ShutDown),
            T_LOGOUT => self.confirm(Confirm::LogOut),
            T_CONFIRM => match owner {
                // WM_ENDSESSION tears the shell down if the session really ends;
                // an app or the user can still cancel it.
                Some(MenuOwner::Confirm(Confirm::Restart)) => system::power(Power::Restart),
                // WM_ENDSESSION tears the shell down if the session really ends;
                // an app or the user can still cancel it.
                Some(MenuOwner::Confirm(Confirm::ShutDown)) => system::power(Power::ShutDown),
                // WM_ENDSESSION tears the shell down if the session really ends;
                // an app or the user can still cancel it.
                Some(MenuOwner::Confirm(Confirm::LogOut)) => system::power(Power::LogOut),
                _ => {}
            },
            T_CANCEL => {}
            T_QUIT_SHELL => self.quit(),
            T_REPORT => self.write_report(true),
            T_CHECK_UPDATES => update::check_now(),
            T_INSTALL_UPDATE => match update::install_ready() {
                // The new exe is started by `finish` once the shell has shut down.
                Ok(()) => self.quit(),
                Err(e) => log(format!("update: install failed: {e}")),
            },
            T_HIDE => fg_windows.iter().for_each(|&w| winops::minimize(w)),
            T_HIDE_OTHERS => {
                for a in &self.running {
                    if !a.windows.iter().any(|w| fg_windows.contains(w)) {
                        a.windows.iter().for_each(|&w| winops::minimize(w));
                    }
                }
            }
            T_SHOW_ALL => {
                for a in &self.running {
                    a.windows.iter().filter(|&&w| winops::is_minimized(w)).for_each(|&w| winops::restore(w));
                }
            }
            T_QUIT_APP => fg_windows.iter().for_each(|&w| winops::close(w)),
            T_MINIMIZE | T_ZOOM | T_LEFT | T_RIGHT | T_CENTER => {
                if let Some(w) = self.fg {
                    match tag {
                        T_MINIMIZE => match self.dock_key_of(w) {
                            Some(key) => self.genie_minimize(&key, &[w], None, "menu"),
                            None => winops::minimize(w),
                        },
                        T_ZOOM => winops::toggle_zoom(w),
                        T_LEFT => winops::tile(w, true),
                        T_RIGHT => winops::tile(w, false),
                        _ => winops::center(w),
                    }
                }
            }
            T_BRING_ALL => {
                for &w in fg_windows.iter().rev() {
                    winops::raise(w);
                }
                if let Some(&w) = fg_windows.first() {
                    winops::activate(w);
                }
            }
            _ => {
                let Some(MenuOwner::Dock(key)) = owner else { return };
                self.handle_dock_pick(tag, &key);
            }
        }
    }

    fn handle_dock_pick(&mut self, tag: u32, key: &str) {
        let Some(t) = self.targets.get(key).cloned() else { return };
        match tag {
            T_DOCK_OPEN => self.dock_click(key),
            T_DOCK_NEW => {
                if let Some(src) = &t.source {
                    self.launch(key, src);
                }
            }
            T_DOCK_PIN => {
                if let Some(src) = t.source {
                    self.pins.push(Pin { name: t.name.clone(), source: src });
                    config::save_pins(&self.pins);
                    self.rebuild_dock();
                }
            }
            T_DOCK_UNPIN => {
                self.pins.retain(|p| p.source.key() != key);
                config::save_pins(&self.pins);
                self.rebuild_dock();
            }
            T_DOCK_REVEAL => match (&t.special, &t.source) {
                (Special::Downloads(p), _) => system::open(p),
                (_, Some(Source::Exe(p))) => system::reveal(p),
                _ => {}
            },
            T_DOCK_QUIT => {
                if let Some(app) = t.app.as_deref().and_then(|k| self.app_by_key(k)) {
                    app.windows.iter().for_each(|&w| winops::close(w));
                }
            }
            T_EMPTY_TRASH => {
                // The shell shows its own confirmation, a modal loop: keep it off this thread.
                std::thread::spawn(|| unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
                    let _ = SHEmptyRecycleBinW(None, PCWSTR::null(), 0);
                });
            }
            t if t >= T_WINDOW_BASE => {
                if let Some(&w) = self.menu_windows.get((t - T_WINDOW_BASE) as usize) {
                    winops::activate(w);
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Search

    fn show_search(&mut self) {
        self.close_menu(false);
        self.spot.show();
        self.spot_clock = Instant::now();
        self.spot_s.dirty = true;
        self.render();
        self.spot_s.raise();
        unsafe {
            if !SetForegroundWindow(self.spot_s.hwnd).as_bool() {
                winops::activate(self.spot_s.hwnd);
            }
            let _ = SetFocus(Some(self.spot_s.hwnd));
            SetTimer(Some(self.spot_s.hwnd), TIMER_CARET, 265, None);
        }
        if !self.spot.query().is_empty() {
            self.on_query(self.spot.query());
        }
    }

    fn hide_search(&mut self) {
        self.spot.hide();
        self.spot_s.dirty = true;
        unsafe {
            let _ = KillTimer(Some(self.spot_s.hwnd), TIMER_CARET);
        }
    }

    fn on_query(&mut self, q: String) {
        let s = self.ctx.strings;
        let mut rows = Vec::new();
        self.row_actions.clear();
        self.row_icon_keys.clear();
        if let Some(v) = search::calc(&q) {
            let text = search::format_number(v);
            rows.push(Row { title: format!("= {text}"), subtitle: format!("{} · {}", s.kind_calc, s.calc_hint), icon: None, glyph: Glyph::Calculator });
            self.row_actions.push(RowAction::Calc(text));
            self.row_icon_keys.push(None);
        }
        for i in self.index.search(&q, 40) {
            let e = self.index.get(i);
            let parent = || std::path::Path::new(&e.target).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let (subtitle, glyph) = match e.kind {
                Kind::App => (s.kind_app.to_string(), Glyph::App),
                Kind::Setting => (s.kind_setting.to_string(), Glyph::Settings),
                Kind::Folder => (parent(), Glyph::Folder),
                Kind::File => (parent(), Glyph::Document),
            };
            let icon_key = match e.kind {
                Kind::Setting => None,
                Kind::File => {
                    let ext = std::path::Path::new(&e.target)
                        .extension()
                        .map(|x| x.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    // Type icons are shared; these types carry their own.
                    if matches!(ext.as_str(), "exe" | "lnk" | "url" | "ico" | "appref-ms" | "") {
                        Some(e.target.clone())
                    } else {
                        Some(format!("ext:{ext}|{}", e.target))
                    }
                }
                _ => Some(e.target.clone()),
            };
            let icon = icon_key.as_ref().and_then(|k| self.small_icons.get(&cache_key(k)).cloned().flatten());
            rows.push(Row { title: e.name.clone(), subtitle, icon, glyph });
            self.row_actions.push(RowAction::Open { target: e.target.clone(), kind: e.kind });
            self.row_icon_keys.push(icon_key.map(|k| cache_key(&k)));
            if let Some(k) = self.row_icon_keys.last().cloned().flatten() {
                let parse = e.target.clone();
                self.request_icon(k, parse);
            }
        }
        if !q.trim().is_empty() {
            rows.push(Row { title: format!("{} «{}»", s.web_search, q.trim()), subtitle: String::new(), icon: None, glyph: Glyph::Web });
            self.row_actions.push(RowAction::Web(q.trim().to_string()));
            self.row_icon_keys.push(None);
        }
        self.spot.set_rows(rows);
        self.spot_s.dirty = true;
    }

    fn request_icon(&mut self, key: String, parse_name: String) {
        self.request_image(key, parse_name, false);
    }

    fn request_image(&mut self, key: String, parse_name: String, thumbnail: bool) {
        if self.small_icons.contains_key(&key) || self.icon_pending.contains(&key) {
            return;
        }
        let id = self.next_req;
        self.next_req += 1;
        self.icon_pending.insert(key.clone());
        self.icon_reqs.insert(id, (key, false));
        let size = if thumbnail { 96 } else { icon_size_for(self.screen.scale) };
        self.icon_worker.request(Request { id, parse_name, size, thumbnail });
    }

    fn icons_arrived(&mut self) {
        let mut dock_changed = self.mac_icons_arrived();
        let mut notes_changed = false;
        while let Ok((id, rgba)) = self.icon_worker.rx.try_recv() {
            let Some((key, dock)) = self.icon_reqs.remove(&id) else { continue };
            self.icon_pending.remove(&key);
            if dock && self.mac_keys.contains(&key) {
                continue;
            }
            if dock {
                // Folders and the recycle bin keep their own shape, as on a Mac.
                let special = key.starts_with("::") || std::path::Path::new(&key).is_dir();
                let img = rgba.as_ref().and_then(|r| if special { image_from(r.clone(), true) } else { app_image(r, true) });
                match img {
                    Some(img) => {
                        self.dock_icons.insert(key, img);
                        dock_changed = true;
                    }
                    None => {
                        self.dock_failed.insert(key, Instant::now());
                    }
                }
                continue;
            }
            let img = if key.starts_with("note:") {
                rgba.as_ref().and_then(|r| app_image(r, false))
            } else {
                rgba.and_then(|r| image_from(r, false))
            };
            if let Some(img) = &img {
                for (i, k) in self.row_icon_keys.iter().enumerate() {
                    if k.as_deref() == Some(key.as_str()) {
                        self.spot.set_row_icon(i, img.clone());
                    }
                }
                self.spot_s.dirty = true;
            }
            if key.starts_with("note:") || key.starts_with("img:") {
                notes_changed = true;
            }
            self.small_icons.insert(key, img);
        }
        if dock_changed {
            self.rebuild_dock();
        }
        if notes_changed {
            self.refresh_note_views();
        }
    }

    fn catalog_arrived(&mut self) {
        let mut changed = false;
        while let Ok(b) = self.catalog.try_recv() {
            changed = true;
            match b {
                Batch::Apps(list) => {
                    self.index.remove_kind(Kind::App);
                    self.index.remove_kind(Kind::Setting);
                    self.index.extend(list);
                }
                Batch::Files { entries, first } => {
                    if first {
                        self.index.remove_kind(Kind::File);
                        self.index.remove_kind(Kind::Folder);
                    }
                    self.index.extend(entries);
                }
            }
        }
        // Refresh visible results unless the user is already moving through them.
        if changed && self.spot.is_visible() && self.spot.selected().unwrap_or(0) == 0 && !self.spot.query().is_empty() {
            self.on_query(self.spot.query());
        }
    }

    fn run_row(&mut self, i: usize, reveal: bool) {
        let Some(a) = self.row_actions.get(i) else { return };
        match a {
            RowAction::Calc(text) => system::set_clipboard(self.spot_s.hwnd, text),
            RowAction::Web(q) => {
                let enc: String = q
                    .bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                        b' ' => "+".into(),
                        _ => format!("%{b:02X}"),
                    })
                    .collect();
                system::open(&format!("https://www.google.com/search?q={enc}"));
            }
            RowAction::Open { target, kind } => {
                if reveal && matches!(kind, Kind::File | Kind::Folder) {
                    system::reveal(target);
                } else {
                    system::open(target);
                }
            }
        }
        self.hide_search();
    }

    fn bar_click(&mut self, item: BarItem) {
        match item {
            BarItem::Search => {
                if self.spot.is_visible() {
                    self.hide_search();
                } else {
                    self.show_search();
                }
            }
            BarItem::Lang => {
                if let Some(w) = self.fg {
                    const HKL_NEXT: isize = 1;
                    unsafe {
                        let _ = PostMessageW(Some(w), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(HKL_NEXT));
                    }
                }
            }
            BarItem::Battery => system::open("ms-settings:batterysaver"),
            BarItem::Wifi => self.toggle_wifi(),
            BarItem::Clock => self.toggle_notes(),
            BarItem::Control | BarItem::Privacy => self.toggle_control(),
            BarItem::Tray(id) => self.tray_click(id, false),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Events and frames

    fn pump_events(&mut self) {
        for e in std::mem::take(&mut self.bar.events) {
            trace(format!("bar: {e:?}"));
            match e {
                BarEvent::Open { item, x, bottom } => {
                    self.close_menu(false);
                    self.bar.set_open(Some(item));
                    let entries = self.bar_menu(item);
                    self.open_menu(entries, MenuOwner::Bar(item), Anchor::Below(x, bottom));
                }
                BarEvent::CloseMenus => self.close_menu(true),
                BarEvent::Click(item) => self.bar_click(item),
                BarEvent::Context { item: BarItem::Tray(id), .. } => self.tray_click(id, true),
                BarEvent::Context { .. } => {}
            }
        }
        for e in std::mem::take(&mut self.dock.events) {
            trace(format!("dock: {}", match &e {
                DockEvent::Click(k) => format!("click {}", report::file_name(k)),
                DockEvent::Menu { key, .. } => format!("menu {}", report::file_name(key)),
            }));
            match e {
                DockEvent::Click(key) => {
                    self.close_menu(true);
                    self.dock_click(&key);
                }
                DockEvent::Menu { key, x, y } => {
                    self.close_menu(false);
                    let entries = self.dock_menu(&key);
                    let r = self.dock.window_rect();
                    let gap = 10.0 * self.screen.scale;
                    self.open_menu(entries, MenuOwner::Dock(key), Anchor::Above(r.x as f32 + x, r.y as f32 + y - gap));
                }
            }
        }
        for e in std::mem::take(&mut self.spot.events) {
            match e {
                SpotEvent::Query(q) => self.on_query(q),
                SpotEvent::Run(i) => self.run_row(i, false),
                SpotEvent::Reveal(i) => self.run_row(i, true),
                SpotEvent::Close => self.hide_search(),
            }
        }
        let picks: Vec<MenuEvent> = self.menu.as_mut().map(|m| std::mem::take(&mut m.events)).unwrap_or_default();
        for MenuEvent::Pick(tag) in picks {
            let owner = self.menu_owner.clone();
            self.close_menu(true);
            self.handle_pick(tag, owner);
        }
    }

    pub fn animating(&self) -> bool {
        self.genie.animating()
            || self.dock.animating()
            || self.spot.animating()
            || self.menu.as_ref().is_some_and(|m| m.animating())
            || self.x.animating()
    }

    pub fn frame(&mut self) {
        let now = Instant::now();
        let dt = self.last_frame.map(|l| (now - l).as_secs_f32()).unwrap_or(1.0 / 120.0).min(0.05);
        self.last_frame = Some(now);
        if self.dock.animating() {
            self.dock.update(dt);
            self.dock_s.dirty = true;
        }
        if self.spot.shown() {
            let caret = self.spot.caret_visible();
            let sdt = (now - self.spot_clock).as_secs_f32();
            self.spot_clock = now;
            let moving = self.spot.animating();
            self.spot.update(sdt);
            if moving || caret != self.spot.caret_visible() {
                self.spot_s.dirty = true;
            }
        }
        if let Some(m) = &mut self.menu
            && m.animating()
        {
            m.update(dt);
            self.menu_s.dirty = true;
        }
        self.extras_update(dt);
        if self.genie.animating() {
            let t = Instant::now();
            self.genie.step((self.screen.w, self.screen.h), self.x.capture.as_mut());
            self.stat("genie", t);
        }
        if self.genie.overlay_shown() || self.genie_glass {
            let t = Instant::now();
            self.genie_glass_tick();
            self.stat("genie glass", t);
        }
        self.pump_events();
        if self.menu.as_ref().is_some_and(|m| m.finished()) {
            self.menu = None;
            self.menu_s.hide();
        }
        if !self.spot.shown() && self.spot_s.shown {
            self.spot_s.hide();
        }
        self.render();
        if !self.animating() {
            self.last_frame = None;
        }
    }

    fn stat(&mut self, name: &'static str, t: Instant) {
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let st = self.stats.entry(name).or_default();
        st.n += 1;
        st.total += ms;
        st.worst = st.worst.max(ms);
    }

    fn render(&mut self) {
        if !self.fullscreen {
            if self.bar_s.dirty {
                let t = Instant::now();
                self.bar_s.prepare(self.bar.window_rect());
                self.bar.draw(&mut self.bar_s.pm, &self.ctx);
                self.bar_s.present();
                self.stat("menu bar", t);
            }
            if self.dock_s.dirty {
                let t = Instant::now();
                self.dock_s.prepare(self.dock.window_rect());
                self.dock.draw(&mut self.dock_s.pm, &self.ctx);
                self.dock_s.present();
                self.stat("dock", t);
            }
        }
        if self.spot_s.dirty && self.spot.shown() {
            let t = Instant::now();
            self.spot_s.prepare(self.spot.window_rect());
            self.spot.draw(&mut self.spot_s.pm, &self.ctx);
            self.spot_s.present();
            self.stat("search", t);
        }
        if self.menu_s.dirty
            && let Some(m) = &self.menu
        {
            let r = self.menu_s.rect;
            self.menu_s.prepare(r);
            m.draw(&mut self.menu_s.pm, &self.ctx);
            panels::clear_rows(&mut self.menu_s.pm, self.bar.height_px() - r.y);
            self.menu_s.present();
        }
        self.extras_render();
    }

    fn set_fullscreen(&mut self, on: bool) {
        if on == self.fullscreen {
            return;
        }
        self.fullscreen = on;
        if on {
            self.close_menu(false);
            self.bar_s.hide();
            self.dock_s.hide();
        } else {
            self.bar_s.dirty = true;
            self.dock_s.dirty = true;
        }
    }

    // -----------------------------------------------------------------------
    // Win32 messages

    pub fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let is_bar = hwnd == self.bar_s.hwnd;
        let is_dock = hwnd == self.dock_s.hwnd;
        let is_spot = hwnd == self.spot_s.hwnd;
        let is_menu = hwnd == self.menu_s.hwnd;
        if self.x.owns(hwnd) && matches!(msg, WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP) {
            let (x, y) = lparam_point(lp);
            if self.panel_mouse(hwnd, msg, x, y).is_some() {
                return Some(LRESULT(0));
            }
        }
        if msg == self.shell_msg && self.shell_msg != 0 {
            const HSHELL_GETMINRECT: usize = 5;
            return Some(if wp.0 == HSHELL_GETMINRECT { self.min_rect(lp) } else { LRESULT(0) });
        }
        // Our tray host broadcasts TaskbarCreated itself so apps re-add icons.
        let own_broadcast = super::tray::broadcast_time().is_some_and(|t| t.elapsed().as_secs() < 3);
        if msg == self.taskbar_msg && self.taskbar_msg != 0 && is_bar && !own_broadcast {
            // Explorer restarted: its taskbar is back and our app bars are gone.
            if self.settings.hide_taskbar {
                system::hide_taskbar();
            }
            system::appbar_register(self.bar_s.hwnd);
            if self.settings.reserve_dock {
                system::appbar_register(self.dock_s.hwnd);
            }
            self.place_appbars();
            if let Some(t) = &self.x.tray {
                t.keep_on_top();
            }
            return Some(LRESULT(0));
        }
        match msg {
            WM_MOUSEACTIVATE if !is_spot => {
                const MA_NOACTIVATE: isize = 3;
                return Some(LRESULT(MA_NOACTIVATE));
            }
            WM_MOUSEMOVE => {
                let (x, y) = lparam_point(lp);
                if is_bar {
                    self.bar_s.track_leave();
                    let before = self.bar.hovered_tray();
                    self.bar.mouse_move(x, y);
                    if let Some(id) = self.bar.hovered_tray()
                        && before != Some(id)
                    {
                        self.tray_hover(id);
                    }
                    self.bar_s.dirty = true;
                } else if is_dock {
                    self.dock_s.track_leave();
                    self.dock.mouse_move(x, y);
                    self.dock_s.dirty = true;
                } else if is_spot {
                    self.spot.mouse_move(x, y);
                    self.spot_s.dirty = true;
                } else if is_menu && let Some(m) = &mut self.menu {
                    self.menu_s.track_leave();
                    m.mouse_move(x, y);
                    self.menu_s.dirty = true;
                }
            }
            WM_MOUSELEAVE => {
                if is_bar {
                    self.bar_s.left();
                    self.bar.mouse_leave();
                    self.bar_s.dirty = true;
                } else if is_dock {
                    self.dock_s.left();
                    self.dock.mouse_leave();
                    self.dock_s.dirty = true;
                } else if is_menu {
                    self.menu_s.left();
                    if let Some(m) = &mut self.menu {
                        m.mouse_leave();
                    }
                    self.menu_s.dirty = true;
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = lparam_point(lp);
                if is_bar {
                    self.bar.mouse_down(x, y);
                    self.bar_s.dirty = true;
                } else if is_dock {
                    self.dock.mouse_down(x, y);
                    self.dock_s.dirty = true;
                } else if is_spot {
                    if !self.spot.mouse_down(x, y, &self.ctx) {
                        self.hide_search();
                    }
                    self.spot_s.dirty = true;
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = lparam_point(lp);
                if is_bar {
                    self.bar.mouse_up(x, y);
                    self.bar_s.dirty = true;
                } else if is_dock {
                    self.dock.mouse_up(x, y);
                    self.dock_s.dirty = true;
                } else if is_menu && let Some(m) = &mut self.menu {
                    m.mouse_up(x, y);
                    self.menu_s.dirty = true;
                }
            }
            WM_RBUTTONUP if is_bar => {
                let (x, _) = lparam_point(lp);
                self.bar.right_click(x);
            }
            WM_RBUTTONUP if is_dock => {
                let (x, y) = lparam_point(lp);
                self.dock.right_click(x, y);
                self.dock_s.dirty = true;
            }
            WM_RBUTTONUP if is_menu => {
                let (x, y) = lparam_point(lp);
                if let Some(m) = &mut self.menu {
                    m.mouse_up(x, y);
                }
            }
            WM_MOUSEWHEEL if is_spot => {
                let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32;
                self.spot.wheel(delta);
                self.spot_s.dirty = true;
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if is_spot => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let mods = Mods { ctrl: key_down(VK_CONTROL), shift: key_down(VK_SHIFT), alt: false };
                let key = match vk {
                    VK_UP => Some(Key::Up),
                    VK_DOWN => Some(Key::Down),
                    VK_LEFT => Some(Key::Left),
                    VK_RIGHT => Some(Key::Right),
                    VK_HOME => Some(Key::Home),
                    VK_END => Some(Key::End),
                    VK_PRIOR => Some(Key::PageUp),
                    VK_NEXT => Some(Key::PageDown),
                    VK_RETURN => Some(Key::Enter),
                    VK_ESCAPE => Some(Key::Escape),
                    VK_BACK => Some(Key::Backspace),
                    VK_DELETE => Some(Key::Delete),
                    VK_TAB => Some(Key::Tab),
                    VIRTUAL_KEY(0x41) if mods.ctrl => Some(Key::SelectAll),
                    _ => None,
                };
                if vk == VIRTUAL_KEY(0x56) && mods.ctrl {
                    if let Some(t) = system::clipboard_text(self.spot_s.hwnd) {
                        for c in t.chars().filter(|c| !c.is_control()) {
                            self.spot.char_input(c);
                        }
                    }
                } else if let Some(k) = key {
                    self.spot.key(k, mods);
                }
                self.spot_s.dirty = true;
                if msg == WM_SYSKEYDOWN {
                    return None;
                }
            }
            WM_CHAR if is_spot => {
                // Ctrl+letter arrives as control characters; the spotlight ignores them.
                if let Some(c) = char::from_u32(wp.0 as u32) {
                    self.spot.char_input(c);
                    self.spot_s.dirty = true;
                }
            }
            WM_ACTIVATE if is_spot => {
                const WA_INACTIVE: usize = 0;
                if wp.0 & 0xffff == WA_INACTIVE && self.spot.is_visible() {
                    self.hide_search();
                }
            }
            WM_CLOSE => {
                if is_spot {
                    self.hide_search();
                }
                return Some(LRESULT(0));
            }
            WM_HOTKEY if wp.0 as i32 == HOTKEY_SEARCH => {
                if self.spot.is_visible() {
                    self.hide_search();
                } else {
                    self.show_search();
                }
            }
            WM_TIMER => match wp.0 {
                TIMER_CLOCK => {
                    self.refresh_status();
                    self.schedule_clock();
                }
                TIMER_STATUS => {
                    self.refresh_status();
                    self.extras_tick();
                    if self.taskbar_hidden {
                        system::keep_taskbar_hidden();
                    }
                    if self.net_tick.is_multiple_of(5) {
                        self.check_work_area("periodic");
                    }
                    // A tray app that exits sends no window event.
                    if self.net_tick.is_multiple_of(3)
                        && self.lingering.iter().any(|a| !a.pids.iter().any(|&p| tracker::process_alive(p, &a.exe)))
                    {
                        self.refresh_windows();
                    }
                }
                TIMER_CARET => self.spot_s.dirty = self.spot.shown(),
                TIMER_BANNER => self.banner_timer(),
                TIMER_BACKDROP => self.backdrop_tick(),
                TIMER_WORKAREA => {
                    unsafe {
                        let _ = KillTimer(Some(self.bar_s.hwnd), TIMER_WORKAREA);
                    }
                    self.check_work_area("app bars placed");
                }
                TIMER_REPORT => {
                    unsafe {
                        let _ = KillTimer(Some(self.bar_s.hwnd), TIMER_REPORT);
                    }
                    self.write_report(false);
                }
                TIMER_WINDOWS => {
                    unsafe {
                        let _ = KillTimer(Some(self.bar_s.hwnd), TIMER_WINDOWS);
                    }
                    WINDOWS_PENDING.store(false, Ordering::Relaxed);
                    self.refresh_windows();
                }
                _ => {}
            },
            WM_APP_WINDOWS => unsafe {
                // Window storms (an app opening ten tool windows) collapse into one refresh.
                SetTimer(Some(self.bar_s.hwnd), TIMER_WINDOWS, 60, None);
            },
            WM_APP_FOREGROUND => {
                // Alt+Tab or a notification took the foreground: the open menu goes away.
                let fg = unsafe { GetForegroundWindow() };
                if (self.menu_owner.is_some() || self.x.panel_open()) && !fg.is_invalid() && tracker::window_pid(fg) != self.own_pid {
                    self.close_menu(true);
                }
                self.refresh_foreground();
            }
            WM_APP_MINIMIZE => {
                self.minimize_event(HWND(wp.0 as *mut _), lp.0 & 1 != 0, (lp.0 >> 1) as u32);
            }
            WM_APP_PRESS => self.press(wp.0 as isize as i32, lp.0 as i32),
            WM_APP_RELEASE => return Some(LRESULT(self.release(wp.0 as isize as i32, lp.0 as i32) as isize)),
            WM_APP_TASKBAR => {
                if self.taskbar_hidden {
                    system::keep_taskbar_hidden();
                }
                // Explorer's tray window came forward: icon calls would go to it.
                if let Some(t) = &self.x.tray {
                    t.keep_on_top();
                }
            }
            WM_APP_ICON => self.icons_arrived(),
            WM_APP_TRAY => self.on_tray(),
            WM_APP_MEDIA => self.on_media(),
            WM_APP_NOTES => self.on_notes(),
            WM_APP_WIFI => self.on_wifi(),
            WM_APP_UPDATE => trace("update state changed"),
            WM_APP_CATALOG => self.catalog_arrived(),
            WM_APP_DISMISS => {
                trace("outside click: close menus and panels");
                self.close_menu(true);
            }
            WM_APP_MENUKEY => {
                let key = match VIRTUAL_KEY(wp.0 as u16) {
                    VK_UP => Key::Up,
                    VK_DOWN => Key::Down,
                    VK_RETURN => Key::Enter,
                    _ => Key::Escape,
                };
                if key == Key::Escape {
                    self.close_menu(true);
                } else if let Some(m) = &mut self.menu {
                    m.key(key);
                    self.menu_s.dirty = true;
                }
            }
            WM_APPBAR => {
                const ABN_POSCHANGED: usize = 1;
                const ABN_FULLSCREENAPP: usize = 2;
                match wp.0 {
                    // Our own SETPOS notifies the other bar; ignore the echo.
                    ABN_POSCHANGED if self.last_appbar_place.is_none_or(|t| t.elapsed().as_millis() > 500) => {
                        self.place_appbars()
                    }
                    // Explorer also announces our own layered windows and
                    // topmost tool windows; hide only for a real one.
                    ABN_FULLSCREENAPP if is_bar => self.set_fullscreen(lp.0 != 0 && winops::foreground_is_fullscreen(self.own_pid)),
                    _ => {}
                }
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED if is_bar => self.configure(true),
            WM_SETTINGCHANGE if is_bar => {
                const SPI_SETDESKWALLPAPER: usize = 0x14;
                // lParam is a string only for the wParam == 0 broadcasts.
                let topic = if wp.0 == 0 && lp.0 != 0 {
                    unsafe { PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default() }
                } else {
                    String::new()
                };
                if wp.0 == SPI_SETDESKWALLPAPER {
                    self.configure(true);
                } else if topic == "ImmersiveColorSet" && self.settings.appearance == "auto" {
                    let dark = system::system_dark();
                    if dark != self.ctx.theme.dark {
                        self.ctx.theme = Theme { dark };
                        self.configure(false);
                        self.rebuild_dock();
                    }
                }
            }
            WM_POWERBROADCAST => self.refresh_status(),
            WM_ENDSESSION if wp.0 != 0 => self.shutdown(),
            _ => return None,
        }
        Some(LRESULT(0))
    }

    pub fn has_work(&self) -> bool {
        self.animating()
            || self.bar_s.dirty
            || self.dock_s.dirty
            || self.menu_s.dirty
            || (self.spot_s.dirty && self.spot.shown())
            || self.x.dirty()
    }
}

fn cache_key(k: &str) -> String {
    // "ext:pdf|C:\\a.pdf" shares one icon per extension.
    match k.split_once('|') {
        Some((ext, _)) if ext.starts_with("ext:") => ext.to_string(),
        _ => k.to_lowercase(),
    }
}
