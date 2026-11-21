//! Notification-area host. The stock taskbar is hidden, so apps' tray icons
//! (Shell_NotifyIcon) would have nowhere to go. Like RetroBar/ManagedShell we
//! register our own "Shell_TrayWnd" above Explorer's: shell32 sends
//! Shell_NotifyIcon calls to the first window of that class in z-order. Every
//! call is also forwarded to Explorer's tray so it stays in sync and still has
//! the icons after the shell exits.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GdiFlush, GetObjectW, HBITMAP, SelectObject,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Shell::NIN_SELECT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CopyIcon, CreateWindowExW, DI_FLAGS, DI_MASK, DI_NORMAL,
    DefWindowProcW, DestroyIcon, DestroyWindow, DrawIconEx, FindWindowExW, FindWindowW, GWL_STYLE, GetIconInfo,
    GetWindowLongPtrW, HICON, HWND_BROADCAST, HWND_TOPMOST, ICONINFO, IsWindow, MSGFLT_ALLOW, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SendMessageTimeoutW, SendNotifyMessageW, SetWindowLongPtrW, SetWindowPos, UnregisterClassW,
    WINDOWPOS, WM_COMMAND, WM_CONTEXTMENU, WM_COPYDATA, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_USER, WM_WINDOWPOSCHANGED, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, PWSTR, w};

use super::icons::Rgba;
use super::tracker::window_pid;
use super::util::log;

pub struct TrayIconView {
    pub id: u64,
    pub icon: Option<Rgba>,
    pub owner_exe: Option<String>,
}

/// Balloons are not reported: Windows turns them into toasts, which the
/// notification watcher already reads.
pub enum TrayEvent {
    Changed,
}

#[derive(Clone, Copy)]
pub enum TrayButton {
    Left,
    Right,
}

const TRAY_CLASS: PCWSTR = w!("Shell_TrayWnd");
const NOTIFY_CLASS: PCWSTR = w!("TrayNotifyWnd");

// WM_COPYDATA dwData values shell32 uses for the tray window.
const COPYDATA_NOTIFY_ICON: usize = 1;
const COPYDATA_ICON_RECT: usize = 3;

const TRAY_SIGNATURE: u32 = 0x3475_3423;
const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x1;
const NIF_ICON: u32 = 0x2;
const NIF_STATE: u32 = 0x8;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x1;

// Explorer's own icons for volume, network and power. Their flyouts are
// anchored to the stock taskbar, which is hidden, and the Control Center and
// the menu bar already cover them, so they are not shown.
const SYSTEM_GUIDS: [[u8; 16]; 3] = [
    guid(0x7820AE73, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]),
    guid(0x7820AE74, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]),
    guid(0x7820AE75, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]),
];

/// A GUID as it lies in memory (little-endian fields).
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
}

// NOTIFYICONDATAW with 32-bit handles, as shell32 marshals it after the
// 8-byte SHELLTRAYDATA header (signature, NIM_* message).
const NID_OFFSET: usize = 8;
const NID_LEN: usize = 956;
const NID_HWND: usize = 4;
const NID_UID: usize = 8;
const NID_FLAGS: usize = 12;
const NID_CALLBACK: usize = 16;
const NID_HICON: usize = 20;
const NID_TIP: usize = 24; // [u16; 128]
const NID_STATE: usize = 280;
const NID_STATE_MASK: usize = 284;
const NID_VERSION: usize = 800;
const NID_GUID: usize = 936;
// Smallest payload worth reading: header plus the fields through hIcon.
const NID_MIN: usize = NID_OFFSET + NID_TIP;

// Internal NOTIFYICONIDENTIFIER for Shell_NotifyIconGetRect:
// dwMagic, dwMessage, cbSize, dwPadding, hWnd (32-bit), uID, guidItem.
const NII_MESSAGE: usize = 4;
const NII_HWND: usize = 16;
const NII_UID: usize = 20;
const NII_GUID: usize = 24;
const NII_LEN: usize = 40;

// Explorer's tray expects this one posted, not sent (ManagedShell does the same).
const WM_TRAY_POST_ONLY: u32 = WM_USER + 372;
const REGISTERED_MESSAGES: u32 = 0xC000;

struct Icon {
    id: u64,
    hwnd: isize,
    uid: u32,
    guid: Option<[u8; 16]>,
    pid: u32,
    callback: u32,
    version: u32,
    state: u32,
    image: Option<Rgba>,
    rect: Option<RECT>,
    owner_exe: Option<String>,
}

impl Icon {
    fn hidden(&self) -> bool {
        self.state & NIS_HIDDEN != 0 || self.guid.is_some_and(|g| SYSTEM_GUIDS.contains(&g))
    }
}

struct State {
    notify: isize,
    notify_msg: u32,
    next_id: u64,
    icons: Vec<Icon>,
    events: Vec<TrayEvent>,
    /// Last left click (icon id, when), to turn a second one into a double click.
    last_left: Option<(u64, Instant)>,
}

impl State {
    fn push(&mut self, ev: TrayEvent) {
        if matches!(ev, TrayEvent::Changed) && self.events.iter().any(|e| matches!(e, TrayEvent::Changed)) {
            return;
        }
        let wake = self.events.is_empty();
        self.events.push(ev);
        // One wake-up per batch: the app drains the queue with take_events
        // (and also every second, in case this post was lost).
        if wake {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.notify as *mut _)), self.notify_msg, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn find(&self, key: &Key) -> Option<usize> {
        self.icons.iter().position(|i| match key {
            Key::Guid(g) => i.guid.as_ref() == Some(g),
            Key::Wnd(h, u) => i.hwnd == *h && i.uid == *u,
        })
    }
}

enum Key {
    Guid([u8; 16]),
    Wnd(isize, u32),
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static BROADCAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// When the tray last broadcast "TaskbarCreated" itself. The app's own windows
/// receive that broadcast too (possibly re-posted, so a little later) and should
/// not mistake it for an Explorer restart; ignore one arriving within a couple
/// of seconds of this.
pub fn broadcast_time() -> Option<Instant> {
    BROADCAST.with(Cell::get)
}

/// Explorer's Shell_TrayWnd, skipping any of this process's. While the tray
/// runs, FindWindow("Shell_TrayWnd") returns ours, so code that means the
/// stock taskbar must use this.
pub fn explorer_tray() -> Option<HWND> {
    let own = unsafe { GetCurrentProcessId() };
    let mut prev: Option<HWND> = None;
    for _ in 0..32 {
        let h = unsafe { FindWindowExW(None, prev, TRAY_CLASS, PCWSTR::null()) }.ok()?;
        if h.is_invalid() {
            return None;
        }
        if window_pid(h) != own {
            return Some(h);
        }
        prev = Some(h);
    }
    None
}

pub struct Tray {
    hwnd: HWND,
    child: Option<HWND>,
}

impl Tray {
    /// Creates the tray window. `notify`/`notify_msg`: post this message to the app window whenever events are queued.
    pub fn start(notify: HWND, notify_msg: u32) -> Option<Tray> {
        if STATE.with(|s| s.try_borrow().map(|s| s.is_some()).unwrap_or(true)) {
            return None;
        }
        unsafe {
            let hinst = GetModuleHandleW(PCWSTR::null()).ok()?;
            for (class, proc_) in [
                (TRAY_CLASS, tray_wndproc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
                (NOTIFY_CLASS, child_wndproc),
            ] {
                let wc = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(proc_),
                    hInstance: hinst.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                // Fails harmlessly if a previous Tray left it registered.
                RegisterClassExW(&wc);
            }
            let hwnd = match CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                TRAY_CLASS,
                PCWSTR::null(),
                WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    log(format!("tray: window not created: {e}"));
                    let _ = UnregisterClassW(TRAY_CLASS, Some(hinst.into()));
                    let _ = UnregisterClassW(NOTIFY_CLASS, Some(hinst.into()));
                    return None;
                }
            };
            // Some apps look for the notify child of the tray window.
            let child = CreateWindowExW(
                Default::default(),
                NOTIFY_CLASS,
                PCWSTR::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(hwnd),
                None,
                Some(hinst.into()),
                None,
            )
            .ok();
            // Lower-integrity apps must still reach us if the shell runs elevated.
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);

            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    *s = Some(State {
                        notify: notify.0 as isize,
                        notify_msg,
                        next_id: 1,
                        icons: Vec::new(),
                        events: Vec::new(),
                        last_left: None,
                    });
                }
            });
            let tray = Tray { hwnd, child };
            tray.raise();

            // Running apps re-add their icons, now to us.
            let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
            if msg != 0 {
                BROADCAST.with(|b| b.set(Some(Instant::now())));
                let _ = SendNotifyMessageW(HWND_BROADCAST, msg, WPARAM(0), LPARAM(0));
            }
            Some(tray)
        }
    }

    /// Visible icons, in add order.
    pub fn icons(&self) -> Vec<TrayIconView> {
        with_state(|s| {
            s.icons
                .iter()
                .filter(|i| !i.hidden())
                .map(|i| TrayIconView {
                    id: i.id,
                    icon: i.image.clone(),
                    owner_exe: i.owner_exe.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn take_events(&self) -> Vec<TrayEvent> {
        with_state(|s| std::mem::take(&mut s.events)).unwrap_or_default()
    }

    /// Sends the icon's owner the mouse messages Explorer would. `anchor_x/y`
    /// (screen coordinates) only reach NOTIFYICON_VERSION_4 apps.
    pub fn click(&self, id: u64, button: TrayButton, anchor_x: i32, anchor_y: i32) {
        let double_ms = unsafe { GetDoubleClickTime() } as u128;
        let Some((hwnd, pid, callback, version, uid, double)) = with_state(|s| {
            let double = matches!(button, TrayButton::Left)
                && s.last_left.is_some_and(|(i, t)| i == id && t.elapsed().as_millis() <= double_ms);
            s.last_left = match button {
                // A double click ends the pair; the next click starts a new one.
                TrayButton::Left if !double => Some((id, Instant::now())),
                _ => None,
            };
            s.icons.iter().find(|i| i.id == id).map(|i| (i.hwnd, i.pid, i.callback, i.version, i.uid, double))
        })
        .flatten() else {
            return;
        };
        let hwnd = HWND(hwnd as *mut _);
        if callback == 0 || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
        // Explorer's sequences: the mouse messages always, plus NIN_SELECT /
        // WM_CONTEXTMENU for version 3+ icons; a second click within the
        // double-click time arrives as WM_LBUTTONDBLCLK.
        let msgs: &[u32] = match (button, version >= 3, double) {
            (TrayButton::Left, _, true) => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
            (TrayButton::Left, true, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
            (TrayButton::Left, false, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
            (TrayButton::Right, true, _) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
            (TrayButton::Right, false, _) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
        };
        unsafe {
            // The app opens a menu or a window in response; let it take focus.
            let _ = AllowSetForegroundWindow(pid);
            for &m in msgs {
                let (wp, lp) = if version >= 4 {
                    (make_long(anchor_x, anchor_y) as usize, ((uid & 0xFFFF) << 16 | (m & 0xFFFF)) as isize)
                } else {
                    (uid as usize, m as isize)
                };
                let _ = PostMessageW(Some(hwnd), callback, WPARAM(wp), LPARAM(lp));
            }
        }
    }

    /// The pointer entered an icon: apps refresh their tooltip or state on
    /// WM_MOUSEMOVE, as they get it from Explorer.
    pub fn hover(&self, id: u64, x: i32, y: i32) {
        let Some((hwnd, callback, version, uid)) =
            with_state(|s| s.icons.iter().find(|i| i.id == id).map(|i| (i.hwnd, i.callback, i.version, i.uid))).flatten()
        else {
            return;
        };
        if callback == 0 {
            return;
        }
        let (wp, lp) = if version >= 4 {
            (make_long(x, y) as usize, ((uid & 0xFFFF) << 16 | WM_MOUSEMOVE) as isize)
        } else {
            (uid as usize, WM_MOUSEMOVE as isize)
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), callback, WPARAM(wp), LPARAM(lp));
        }
    }

    /// Screen rect where the app drew the icon; answers Shell_NotifyIconGetRect.
    pub fn set_icon_rect(&self, id: u64, rect: RECT) {
        with_state(|s| {
            if let Some(i) = s.icons.iter_mut().find(|i| i.id == id) {
                i.rect = Some(rect);
            }
        });
    }

    /// Keeps our window ahead of Explorer's, which re-raises itself (and is
    /// recreated when Explorer restarts).
    pub fn keep_on_top(&self) {
        let first = unsafe { FindWindowW(TRAY_CLASS, PCWSTR::null()) };
        if first.ok() != Some(self.hwnd) {
            self.raise();
        }
    }

    /// Drops icons whose owner window is gone (the app crashed or quit without NIM_DELETE).
    pub fn prune(&self) {
        with_state(|s| {
            let before = s.icons.iter().filter(|i| !i.hidden()).count();
            let n = s.icons.len();
            s.icons.retain(|i| unsafe { IsWindow(Some(HWND(i.hwnd as *mut _))) }.as_bool());
            if s.icons.len() != n && before != s.icons.iter().filter(|i| !i.hidden()).count() {
                s.push(TrayEvent::Changed);
            }
        });
    }

    fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        STATE.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                *s = None;
            }
        });
        unsafe {
            if let Some(c) = self.child {
                let _ = DestroyWindow(c);
            }
            let _ = DestroyWindow(self.hwnd);
            if let Ok(hinst) = GetModuleHandleW(PCWSTR::null()) {
                let _ = UnregisterClassW(NOTIFY_CLASS, Some(hinst.into()));
                let _ = UnregisterClassW(TRAY_CLASS, Some(hinst.into()));
            }
        }
    }
}

/// Runs `f` on the state unless it is missing or already borrowed (a nested
/// call from inside a message handler); never panics.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// ---------------------------------------------------------------------------
// Window procedures

unsafe extern "system" fn child_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            if let Some(r) = unsafe { on_copydata(wp, lp) } {
                return r;
            }
        }
        WM_WINDOWPOSCHANGED if lp.0 != 0 => {
            // Something (ShowWindow on "the taskbar") showed us; stay hidden.
            let pos = unsafe { std::ptr::read_unaligned(lp.0 as *const WINDOWPOS) };
            if pos.flags.contains(SWP_SHOWWINDOW) {
                unsafe {
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_VISIBLE.0 as isize));
                }
            }
        }
        // Commands meant for the stock taskbar (minimize all, show desktop…).
        // Registered messages are skipped: those are mostly broadcasts that
        // Explorer receives itself.
        m if m == WM_COMMAND || (WM_USER..REGISTERED_MESSAGES).contains(&m) => {
            if let Some(target) = explorer_tray() {
                if m == WM_TRAY_POST_ONLY {
                    unsafe {
                        let _ = PostMessageW(Some(target), m, wp, lp);
                    }
                } else {
                    return send_to(target, m, wp, lp, 1000);
                }
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn send_to(target: HWND, msg: u32, wp: WPARAM, lp: LPARAM, timeout_ms: u32) -> LRESULT {
    let mut res = 0usize;
    let ok = unsafe { SendMessageTimeoutW(target, msg, wp, lp, SMTO_ABORTIFHUNG, timeout_ms, Some(&mut res)) };
    if ok.0 == 0 { LRESULT(0) } else { LRESULT(res as isize) }
}

fn forward(msg: u32, wp: WPARAM, lp: LPARAM, timeout_ms: u32) -> LRESULT {
    match explorer_tray() {
        Some(t) => send_to(t, msg, wp, lp, timeout_ms),
        None => LRESULT(0),
    }
}

/// None: let DefWindowProc answer.
unsafe fn on_copydata(wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if lp.0 == 0 {
        return None;
    }
    let cds = unsafe { std::ptr::read_unaligned(lp.0 as *const COPYDATASTRUCT) };
    // The system copied the sender's buffer; cbData is its real size.
    let data: &[u8] = if cds.lpData.is_null() || cds.cbData == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }
    };
    match cds.dwData {
        COPYDATA_NOTIFY_ICON => {
            let ours = notify_icon(data);
            // State is updated first and not borrowed here: the forward pumps
            // incoming sent messages while it waits.
            let explorer = forward(WM_COPYDATA, wp, lp, 100);
            Some(ours.map(|ok| LRESULT(ok as isize)).unwrap_or(explorer))
        }
        COPYDATA_ICON_RECT => Some(icon_rect(data).unwrap_or_else(|| forward(WM_COPYDATA, wp, lp, 100))),
        // App-bar messages (dwData 0) and anything else: Explorer's answer is
        // the answer, and app bars need it (including our own menu bar's).
        _ => Some(forward(WM_COPYDATA, wp, lp, 2000)),
    }
}

// ---------------------------------------------------------------------------
// Shell_NotifyIcon

fn u32_at(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0)
}

/// A 32-bit handle value as the sender's 64-bit handle.
fn handle_at(b: &[u8], off: usize) -> isize {
    u32_at(b, off) as i32 as isize
}

fn guid_at(b: &[u8], off: usize) -> Option<[u8; 16]> {
    let g: [u8; 16] = b.get(off..off + 16)?.try_into().ok()?;
    (g != [0; 16]).then_some(g)
}

/// Applies one Shell_NotifyIcon call. None: not a call we understand.
fn notify_icon(data: &[u8]) -> Option<bool> {
    if data.len() < NID_MIN || u32_at(data, 0) != TRAY_SIGNATURE {
        return None;
    }
    let message = u32_at(data, 4);
    // cbSize describes the sender's native struct, not this wire layout, so
    // only cbData bounds the read; missing trailing fields read as zero.
    let mut nid = [0u8; NID_LEN];
    let avail = (data.len() - NID_OFFSET).min(NID_LEN);
    nid[..avail].copy_from_slice(&data[NID_OFFSET..NID_OFFSET + avail]);

    let flags = u32_at(&nid, NID_FLAGS);
    let hwnd = handle_at(&nid, NID_HWND);
    let uid = u32_at(&nid, NID_UID);
    let guid = if flags & NIF_GUID != 0 { guid_at(&nid, NID_GUID) } else { None };
    let key = match guid {
        Some(g) => Key::Guid(g),
        None => Key::Wnd(hwnd, uid),
    };

    // GDI work happens before borrowing the state.
    let new_image = (flags & NIF_ICON != 0 && matches!(message, NIM_ADD | NIM_MODIFY))
        .then(|| icon_image(handle_at(&nid, NID_HICON)));

    with_state(|s| match message {
        NIM_ADD | NIM_MODIFY => {
            let found = s.find(&key);
            let idx = match found {
                Some(i) => i,
                None => {
                    if hwnd == 0 || !unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool() {
                        return false;
                    }
                    let pid = window_pid(HWND(hwnd as *mut _));
                    let id = s.next_id;
                    s.next_id += 1;
                    s.icons.push(Icon {
                        id,
                        hwnd,
                        uid,
                        guid,
                        pid,
                        callback: 0,
                        version: 0,
                        state: 0,
                        image: None,
                        rect: None,
                        owner_exe: process_path(pid),
                    });
                    s.icons.len() - 1
                }
            };
            let Some(icon) = s.icons.get_mut(idx) else { return false };
            if hwnd != 0 && hwnd != icon.hwnd {
                // Icons identified by GUID may move to another window.
                icon.hwnd = hwnd;
                icon.pid = window_pid(HWND(hwnd as *mut _));
                icon.owner_exe = process_path(icon.pid);
            }
            if hwnd != 0 {
                icon.uid = uid;
            }
            // Explorer answers a repeated add as a fresh icon: the app sets
            // its version again afterwards, so the old one must not linger.
            if message == NIM_ADD && found.is_some() {
                icon.version = 0;
            }

            if flags & NIF_MESSAGE != 0 {
                icon.callback = u32_at(&nid, NID_CALLBACK);
            }
            if flags & NIF_STATE != 0 {
                let mask = u32_at(&nid, NID_STATE_MASK);
                icon.state = (icon.state & !mask) | (u32_at(&nid, NID_STATE) & mask);
            }
            match new_image {
                // A handle we could not read keeps the previous picture.
                Some(Some(img)) => icon.image = Some(img),
                Some(None) if handle_at(&nid, NID_HICON) == 0 => icon.image = None,
                _ => {}
            }
            s.push(TrayEvent::Changed);
            // A modify for an icon we did not have (added before the tray
            // started, app ignores TaskbarCreated) is kept but still fails, as
            // in Explorer, so apps that re-add on failure do so.
            found.is_some() || message == NIM_ADD
        }
        NIM_DELETE => match s.find(&key) {
            Some(i) => {
                let was_visible = !s.icons.remove(i).hidden();
                if was_visible {
                    s.push(TrayEvent::Changed);
                }
                true
            }
            None => false,
        },
        NIM_SETFOCUS => true,
        NIM_SETVERSION => {
            let v = u32_at(&nid, NID_VERSION);
            match s.find(&key).and_then(|i| s.icons.get_mut(i)) {
                Some(icon) if v <= 4 => {
                    icon.version = v;
                    true
                }
                _ => false,
            }
        }
        _ => false,
    })
}

/// Shell_NotifyIconGetRect. shell32 asks twice: dwMessage 1 for the top-left
/// corner, 2 for the bottom-right, each packed as MAKELPARAM(x, y) in the
/// LRESULT (the same as ManagedShell answers). None: unknown icon or no rect
/// yet, so Explorer answers instead.
fn icon_rect(data: &[u8]) -> Option<LRESULT> {
    if data.len() < NII_LEN {
        return None;
    }
    let key = match guid_at(data, NII_GUID) {
        Some(g) => Key::Guid(g),
        None => Key::Wnd(handle_at(data, NII_HWND), u32_at(data, NII_UID)),
    };
    let message = u32_at(data, NII_MESSAGE);
    let rect = with_state(|s| s.find(&key).and_then(|i| s.icons.get(i)).and_then(|i| i.rect)).flatten()?;
    let packed = match message {
        1 => make_long(rect.left, rect.top),
        2 => make_long(rect.right, rect.bottom),
        _ => return None,
    };
    Some(LRESULT(packed as i32 as isize))
}

/// MAKELONG of two signed 16-bit coordinates.
fn make_long(lo: i32, hi: i32) -> u32 {
    ((hi as u16 as u32) << 16) | lo as u16 as u32
}

fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let path = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()
            .and_then(|_| buf.get(..len as usize))
            .map(String::from_utf16_lossy);
        let _ = CloseHandle(h);
        path
    }
}

// ---------------------------------------------------------------------------
// Icon pixels

/// Copies another process's HICON and renders it to premultiplied RGBA at its
/// own size. The copy is freed right away; only the pixels are kept.
fn icon_image(handle: isize) -> Option<Rgba> {
    if handle == 0 {
        return None;
    }
    unsafe {
        let copy = CopyIcon(HICON(handle as *mut _)).ok()?;
        let out = render_icon(copy);
        let _ = DestroyIcon(copy);
        out
    }
}

unsafe fn bitmap_size(b: HBITMAP) -> Option<(i32, i32)> {
    if b.is_invalid() {
        return None;
    }
    let mut bm = BITMAP::default();
    let n = unsafe { GetObjectW(b.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) };
    (n != 0).then_some((bm.bmWidth, bm.bmHeight.abs()))
}

unsafe fn render_icon(icon: HICON) -> Option<Rgba> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let color = unsafe { bitmap_size(info.hbmColor) };
    // A monochrome icon stacks its AND and XOR masks in one double-height bitmap.
    let mask = unsafe { bitmap_size(info.hbmMask) }.map(|(w, h)| (w, h / 2));
    unsafe {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    }
    let (w, h) = color.or(mask)?;
    if w <= 0 || h <= 0 {
        return None;
    }
    let (w, h) = (w.min(256), h.min(256));

    let mut px = unsafe { draw_icon(icon, w, h, DI_NORMAL, 0) }?;
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        // No alpha channel: transparency lives in the mask (white = transparent),
        // drawn over white so the AND leaves the mask itself.
        let m = unsafe { draw_icon(icon, w, h, DI_MASK, 0xFF) }?;
        for (p, m) in px.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
            if m[0] == 0 {
                p[3] = 255;
            } else {
                p.fill(0);
            }
        }
    }
    // DrawIconEx blends onto the zeroed DIB, so colours come out premultiplied;
    // clamp stray values and turn BGRA into RGBA.
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
        let a = p[3];
        for c in &mut p[..3] {
            *c = (*c).min(a);
        }
    }
    Some(Rgba { w: w as u32, h: h as u32, data: px })
}

/// Draws `icon` into a 32bpp top-down DIB prefilled with `fill` and returns its BGRA bytes.
unsafe fn draw_icon(icon: HICON, w: i32, h: i32, flags: DI_FLAGS, fill: u8) -> Option<Vec<u8>> {
    let len = (w as usize) * (h as usize) * 4;
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
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) if !bits.is_null() => b,
            Ok(b) => {
                let _ = DeleteObject(b.into());
                let _ = DeleteDC(dc);
                return None;
            }
            Err(_) => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        std::ptr::write_bytes(bits as *mut u8, fill, len);
        let old = SelectObject(dc, bmp.into());
        let drawn = DrawIconEx(dc, 0, 0, icon, w, h, 0, None, flags).is_ok();
        let _ = GdiFlush();
        let out = drawn.then(|| std::slice::from_raw_parts(bits as *const u8, len).to_vec());
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        out
    }
}
