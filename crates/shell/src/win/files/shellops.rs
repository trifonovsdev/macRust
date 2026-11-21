//! Shell work for the file manager: sidebar places, file operations through
//! IFileOperation (Windows' own progress, conflict and undo), the clipboard
//! in Explorer's format, the shell context menu and drag-out.

use std::cell::RefCell;
use std::time::Instant;

use macrust_ui::files::art::PlaceIcon;
use macrust_ui::files::strings::FileStrings;
use macrust_ui::files::{Place, Section};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, GetDriveTypeW};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IDataObject,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, BHID_SFObject, BHID_SFUIObject, CFSTR_PREFERREDDROPEFFECT, CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL,
    CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DROPFILES,
    DragQueryFileW, FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_RENAMEONCOLLISION, FOF_WANTNUKEWARNING,
    FOFX_ADDUNDORECORD, FOFX_RECYCLEONDELETE, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
    FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_Videos, FileOperation, GCS_VERBW, HDROP, IContextMenu, IContextMenu2,
    IContextMenu3, IFileOperation, IShellFolder, IShellItem, IShellItemArray, ILFree, KF_FLAG_DEFAULT,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHGetKnownFolderItem, SHParseDisplayName,
    SIGDN, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, HMENU, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, AppendMenuW, PostMessageW,
    SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use windows::core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, w};

use super::super::util::{from_wide, hwnd_from_isize, hwnd_to_isize, wide};
use super::trace;

fn display(item: &IShellItem, sigdn: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(sigdn).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// The error for a path the shell could not resolve (HRESULT 0x80070002).
fn not_found() -> windows::core::Error {
    windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.0))
}

pub fn item(path: &str) -> Option<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None).ok() }
}

/// One shell item array for several paths (all must exist).
fn item_array(paths: &[String]) -> Option<IShellItemArray> {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    for p in paths {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if unsafe { SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None) }.is_ok() && !pidl.is_null() {
            pidls.push(pidl);
        }
    }
    let arr = if pidls.len() == paths.len() && !pidls.is_empty() {
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        unsafe { SHCreateShellItemArrayFromIDLists(&consts).ok() }
    } else {
        None
    };
    for p in pidls {
        unsafe { ILFree(Some(p)) };
    }
    arr
}

// ---------------------------------------------------------------------------
// Places

pub fn favourites(s: &'static FileStrings) -> Section {
    let ids = [
        (&FOLDERID_Desktop, PlaceIcon::Desktop),
        (&FOLDERID_Downloads, PlaceIcon::Downloads),
        (&FOLDERID_Documents, PlaceIcon::Documents),
        (&FOLDERID_Pictures, PlaceIcon::Pictures),
        (&FOLDERID_Music, PlaceIcon::Music),
        (&FOLDERID_Videos, PlaceIcon::Videos),
        (&FOLDERID_Profile, PlaceIcon::Home),
    ];
    let mut places = Vec::new();
    for (id, icon) in ids {
        let Ok(item) = (unsafe { SHGetKnownFolderItem::<IShellItem>(id, KF_FLAG_DEFAULT, None) }) else { continue };
        let (Some(name), Some(path)) = (display(&item, SIGDN_NORMALDISPLAY), display(&item, SIGDN_FILESYSPATH)) else { continue };
        places.push(Place { name, path, icon, eject: false });
    }
    Section { title: s.favourites, places }
}

/// Drives and the Recycle Bin. Display names of network drives can take
/// seconds, so this runs on a worker thread.
pub fn locations(s: &'static FileStrings) -> (Section, Vec<(String, String)>) {
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    let mut places = Vec::new();
    let mut roots = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let kind = unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) };
        let icon = match kind {
            DRIVE_FIXED => PlaceIcon::Drive,
            DRIVE_REMOVABLE | DRIVE_CDROM => PlaceIcon::Removable,
            DRIVE_REMOTE => PlaceIcon::Network,
            _ => continue,
        };
        let name = item(&root).and_then(|it| display(&it, SIGDN_NORMALDISPLAY)).unwrap_or_else(|| root.trim_end_matches('\\').to_string());
        roots.push((root.clone(), name.clone()));
        places.push(Place { name, path: root, icon, eject: matches!(kind, DRIVE_REMOVABLE | DRIVE_CDROM) });
    }
    places.push(Place { name: s.trash.into(), path: "shell:RecycleBinFolder".into(), icon: PlaceIcon::Trash, eject: false });
    (Section { title: s.locations, places }, roots)
}

// ---------------------------------------------------------------------------
// Opening

/// Opens a file with its program, or a shell location, without blocking the
/// UI thread (handlers can pump messages or stall on network paths).
pub fn shell_open(target: String) {
    let _ = std::thread::Builder::new().name("files-open".into()).spawn(move || {
        let t = wide(&target);
        let r = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            ShellExecuteW(None, w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
        };
        if r.0 as isize <= 32 {
            trace(format!("open failed: code {}", r.0 as isize));
        }
    });
}

// ---------------------------------------------------------------------------
// File operations

/// Runs an IFileOperation on its own STA thread, owned by `hwnd` so Windows'
/// progress and conflict dialogs sit on our window; posts `done_msg` after.
fn file_op(
    hwnd: HWND,
    done_msg: u32,
    label: &'static str,
    count: usize,
    flags: FILEOPERATION_FLAGS,
    queue: impl FnOnce(&IFileOperation) -> windows::core::Result<()> + Send + 'static,
) {
    let owner = hwnd_to_isize(hwnd);
    let _ = std::thread::Builder::new().name("files-op".into()).spawn(move || {
        let t = Instant::now();
        let result = (|| -> windows::core::Result<bool> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
                op.SetOwnerWindow(hwnd_from_isize(owner))?;
                op.SetOperationFlags(flags)?;
                queue(&op)?;
                op.PerformOperations()?;
                Ok(op.GetAnyOperationsAborted()?.as_bool())
            }
        })();
        match result {
            Ok(aborted) => trace(format!("op {label}: {count} items, {} in {} ms", if aborted { "aborted" } else { "done" }, t.elapsed().as_millis())),
            // 0x800704C7 is the user cancelling.
            Err(e) => trace(format!("op {label}: {count} items failed 0x{:08x} after {} ms", e.code().0, t.elapsed().as_millis())),
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd_from_isize(owner)), done_msg, WPARAM(0), LPARAM(0));
        }
    });
}

pub fn delete(hwnd: HWND, msg: u32, paths: Vec<String>, permanent: bool) {
    // Moving to the Recycle Bin asks nothing, as on a Mac; deleting for good
    // keeps Windows' confirmation.
    let flags = if permanent { FOF_WANTNUKEWARNING } else { FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOFX_ADDUNDORECORD };
    let n = paths.len();
    file_op(hwnd, msg, if permanent { "delete" } else { "recycle" }, n, flags, move |op| unsafe {
        let arr = item_array(&paths).ok_or_else(not_found)?;
        op.DeleteItems(&arr)
    });
}

pub fn rename(hwnd: HWND, msg: u32, path: String, to: String) {
    file_op(hwnd, msg, "rename", 1, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD, move |op| unsafe {
        let it = item(&path).ok_or_else(not_found)?;
        op.RenameItem(&it, &HSTRING::from(to.as_str()), None)
    });
}

pub fn new_folder(hwnd: HWND, msg: u32, path: String) {
    let Some((dir, name)) = path.rsplit_once('\\').map(|(d, n)| (if d.ends_with(':') { format!("{d}\\") } else { d.to_string() }, n.to_string())) else { return };
    file_op(hwnd, msg, "new-folder", 1, FOF_ALLOWUNDO | FOFX_ADDUNDORECORD, move |op| unsafe {
        let folder = item(&dir).ok_or_else(not_found)?;
        op.NewItem(&folder, FILE_ATTRIBUTE_DIRECTORY.0, &HSTRING::from(name.as_str()), PCWSTR::null(), None)
    });
}

fn same_folder(a: &str, b: &str) -> bool {
    macrust_ui::files::model::same_path(a, b)
}

pub fn transfer(hwnd: HWND, msg: u32, sources: Vec<String>, into: String, mv: bool) {
    if sources.is_empty() {
        return;
    }
    let parent = |p: &str| p.trim_end_matches('\\').rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default();
    let in_place = sources.iter().all(|s| same_folder(&parent(s), &into));
    if mv && in_place {
        return;
    }
    // Pasting copies into the folder they came from gets "name - Copy", as in Explorer.
    let mut flags = FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
    if in_place {
        flags |= FOF_RENAMEONCOLLISION;
    }
    let n = sources.len();
    file_op(hwnd, msg, if mv { "move" } else { "copy" }, n, flags, move |op| unsafe {
        let arr = item_array(&sources).ok_or_else(not_found)?;
        let dest = item(&into).ok_or_else(not_found)?;
        if mv { op.MoveItems(&arr, &dest) } else { op.CopyItems(&arr, &dest) }
    });
}

// ---------------------------------------------------------------------------
// Clipboard (CF_HDROP + "Preferred DropEffect", which Explorer reads)

fn global_copy(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GHND, bytes.len()).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

pub fn set_clipboard_files(hwnd: HWND, paths: &[String], cut: bool) -> bool {
    let mut data = vec![0u8; size_of::<DROPFILES>()];
    let header = DROPFILES { pFiles: size_of::<DROPFILES>() as u32, fWide: true.into(), ..Default::default() };
    unsafe { std::ptr::write_unaligned(data.as_mut_ptr() as *mut DROPFILES, header) };
    for p in paths {
        for u in p.encode_utf16().chain(std::iter::once(0)) {
            data.extend_from_slice(&u.to_le_bytes());
        }
    }
    data.extend_from_slice(&[0, 0]);
    let effect = if cut { DROPEFFECT_MOVE.0 } else { DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0 };
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            trace("clipboard: busy");
            return false;
        }
        let _ = EmptyClipboard();
        let mut ok = false;
        if let Some(h) = global_copy(&data) {
            ok = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))).is_ok();
            if !ok {
                let _ = GlobalFree(Some(h));
            }
        }
        let fmt = RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT);
        if let Some(h) = global_copy(&effect.to_le_bytes())
            && SetClipboardData(fmt, Some(HANDLE(h.0))).is_err()
        {
            let _ = GlobalFree(Some(h));
        }
        let _ = CloseClipboard();
        trace(format!("clipboard: {} {} items {}", if cut { "cut" } else { "copy" }, paths.len(), if ok { "ok" } else { "failed" }));
        ok
    }
}

/// Files on the clipboard and whether they were cut.
pub fn clipboard_files(hwnd: HWND) -> Option<(Vec<String>, bool)> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let files = (|| {
            let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let mut out = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                out.push(from_wide(&buf));
            }
            Some(out)
        })();
        let fmt = RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT);
        let cut = GetClipboardData(fmt).ok().is_some_and(|h| {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u32;
            let v = if p.is_null() { 0 } else { std::ptr::read_unaligned(p) };
            let _ = GlobalUnlock(g);
            v & DROPEFFECT_MOVE.0 != 0 && v & DROPEFFECT_COPY.0 == 0
        });
        let _ = CloseClipboard();
        files.filter(|f| !f.is_empty()).map(|f| (f, cut))
    }
}

// ---------------------------------------------------------------------------
// Context menu

thread_local! {
    /// The shell menu being shown: owner-drawn submenus ("Send to", "Open
    /// with") need its window messages.
    static ACTIVE: RefCell<Option<(Option<IContextMenu2>, Option<IContextMenu3>)>> = const { RefCell::new(None) };
}

/// Hands menu messages to the shell's menu while it is open.
pub fn menu_message(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    use windows::Win32::UI::WindowsAndMessaging::{WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR};
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    ACTIVE.with(|a| {
        let a = a.borrow();
        let (cm2, cm3) = a.as_ref()?;
        unsafe {
            if let Some(cm3) = cm3 {
                let mut r = LRESULT(0);
                if cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut r)).is_ok() {
                    return Some(r);
                }
            } else if let Some(cm2) = cm2
                && cm2.HandleMenuMsg(msg, wp, lp).is_ok()
            {
                return Some(LRESULT(0));
            }
        }
        None
    })
}

/// One of our own items in the folder background menu.
pub enum Extra {
    Item { id: u32, label: String, checked: bool },
    Sub { label: String, items: Vec<Extra> },
    Separator,
}

pub enum Pick {
    Nothing,
    Rename,
    Ours(u32),
    /// The shell ran the command.
    Shell,
}

const FIRST_SHELL: u32 = 0x1000;
/// Not in the windows crate: the Unicode fields of CMINVOKECOMMANDINFOEX are valid.
const CMIC_MASK_UNICODE: u32 = 0x4000;

fn append(menu: HMENU, items: &[Extra]) {
    unsafe {
        for e in items {
            match e {
                Extra::Item { id, label, checked } => {
                    let flags = if *checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                    let _ = AppendMenuW(menu, flags, *id as usize, &HSTRING::from(label.as_str()));
                }
                Extra::Sub { label, items } => {
                    if let Ok(sub) = CreatePopupMenu() {
                        append(sub, items);
                        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, &HSTRING::from(label.as_str()));
                    }
                }
                Extra::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
    }
}

fn shell_menu(hwnd: HWND, folder: &str, paths: &[String]) -> windows::core::Result<IContextMenu> {
    unsafe {
        if paths.is_empty() {
            let it = item(folder).ok_or_else(not_found)?;
            let sf: IShellFolder = it.BindToHandler(None, &BHID_SFObject)?;
            sf.CreateViewObject(hwnd)
        } else {
            let arr = item_array(paths).ok_or_else(not_found)?;
            arr.BindToHandler(None, &BHID_SFUIObject)
        }
    }
}

fn verb(cm: &IContextMenu, id: u32) -> Option<String> {
    let mut buf = [0u16; 128];
    unsafe {
        cm.GetCommandString(id as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32).ok()?;
    }
    Some(from_wide(&buf))
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn invoke(cm: &IContextMenu, hwnd: HWND, id: u32, dir: &str, pt: POINT) -> windows::core::Result<()> {
    let dirw = wide(dir);
    let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
    if key_down(VK_CONTROL) {
        mask |= CMIC_MASK_CONTROL_DOWN;
    }
    if key_down(VK_SHIFT) {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: mask,
        hwnd,
        // MAKEINTRESOURCE: the command offset in place of a verb string.
        lpVerb: PCSTR(id as usize as *const u8),
        lpVerbW: PCWSTR(id as usize as *const u16),
        lpDirectoryW: PCWSTR(dirw.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: pt,
        ..Default::default()
    };
    unsafe { cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO) }
}

/// Shows the shell's context menu for `paths` in `folder` (none: the folder
/// background, with `extras` on top) at screen point `pt`.
pub fn context_menu(hwnd: HWND, folder: &str, paths: &[String], extras: &[Extra], pt: POINT) -> Pick {
    let cm = match shell_menu(hwnd, folder, paths) {
        Ok(cm) => Some(cm),
        Err(e) => {
            trace(format!("context menu: no shell menu 0x{:08x} for {} items", e.code().0, paths.len()));
            None
        }
    };
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else { return Pick::Nothing };
    append(menu, extras);
    let at = unsafe { windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(menu)) }.max(0) as u32;
    if let Some(cm) = &cm {
        let mut flags = CMF_NORMAL | CMF_CANRENAME;
        if key_down(VK_SHIFT) {
            flags |= CMF_EXTENDEDVERBS;
        }
        let hr = unsafe { cm.QueryContextMenu(menu, at, FIRST_SHELL, 0x7FFF, flags) };
        if hr.is_err() {
            trace(format!("context menu: query failed 0x{:08x}", hr.0));
        }
        ACTIVE.with(|a| *a.borrow_mut() = Some((cm.cast().ok(), cm.cast().ok())));
    }
    let t = Instant::now();
    let cmd = unsafe { TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None) }.0 as u32;
    ACTIVE.with(|a| *a.borrow_mut() = None);
    let pick = match (&cm, cmd) {
        (_, 0) => Pick::Nothing,
        (_, c) if c < FIRST_SHELL => Pick::Ours(c),
        (Some(cm), c) => {
            let id = c - FIRST_SHELL;
            let v = verb(cm, id).unwrap_or_default();
            if v.eq_ignore_ascii_case("rename") {
                Pick::Rename
            } else {
                if let Err(e) = invoke(cm, hwnd, id, folder, pt) {
                    trace(format!("context menu: command failed 0x{:08x}", e.code().0));
                }
                Pick::Shell
            }
        }
        (None, _) => Pick::Nothing,
    };
    trace(format!(
        "context menu: {} items, {}, open {} ms",
        paths.len(),
        match pick {
            Pick::Nothing => "dismissed",
            Pick::Rename => "rename",
            Pick::Ours(_) => "own command",
            Pick::Shell => "shell command",
        },
        t.elapsed().as_millis()
    ));
    unsafe {
        let _ = DestroyMenu(menu);
    }
    pick
}

/// Runs a canonical verb ("eject") on one item.
pub fn invoke_verb(hwnd: HWND, path: &str, name: &'static str) {
    let r = (|| -> windows::core::Result<()> {
        let cm = shell_menu(hwnd, "", &[path.to_string()])?;
        unsafe {
            let menu = CreatePopupMenu()?;
            let _ = cm.QueryContextMenu(menu, 0, FIRST_SHELL, 0x7FFF, CMF_NORMAL);
            let verb_a = format!("{name}\0");
            let verb_w = wide(name);
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE,
                hwnd,
                lpVerb: PCSTR(verb_a.as_ptr()),
                lpVerbW: PCWSTR(verb_w.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let r = cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO);
            let _ = DestroyMenu(menu);
            r
        }
    })();
    trace(format!("verb {name}: {}", match r {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed 0x{:08x}", e.code().0),
    }));
}

// ---------------------------------------------------------------------------
// Drag out

/// Drags `paths` out of the window with the shell's data object and drag
/// image; returns when the drop is over (a modal loop).
pub fn drag_out(hwnd: HWND, paths: &[String]) {
    let t = Instant::now();
    let r = (|| -> windows::core::Result<DROPEFFECT> {
        let arr = item_array(paths).ok_or_else(not_found)?;
        unsafe {
            let data: IDataObject = arr.BindToHandler(None, &BHID_DataObject)?;
            SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK)
        }
    })();
    match r {
        Ok(e) => trace(format!("drag out: {} items, effect {}, {} ms", paths.len(), e.0, t.elapsed().as_millis())),
        Err(e) => trace(format!("drag out: {} items failed 0x{:08x}", paths.len(), e.code().0)),
    }
}

/// File-system paths in a dropped data object; virtual items are left out.
pub fn dropped_paths(data: &IDataObject) -> Vec<String> {
    let Ok(arr) = (unsafe { windows::Win32::UI::Shell::SHCreateShellItemArrayFromDataObject::<_, IShellItemArray>(data) }) else {
        return Vec::new();
    };
    let n = unsafe { arr.GetCount() }.unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..n {
        if let Ok(it) = unsafe { arr.GetItemAt(i) }
            && let Some(p) = display(&it, SIGDN_FILESYSPATH)
        {
            out.push(p);
        }
    }
    if out.len() < n as usize {
        trace(format!("drop: {} of {n} items have no file path", n as usize - out.len()));
    }
    out
}

