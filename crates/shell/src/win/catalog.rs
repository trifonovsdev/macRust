//! Everything the search panel can find, collected on a background thread:
//! all apps from shell:AppsFolder (Win32 and Store), Settings pages, and the
//! files and folders under the user's own folders.

use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use macrust_ui::search::{Entry, Kind};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, IEnumShellItems, IShellItem, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::icons::shell_item;
use super::util::{hwnd_from_isize, hwnd_to_isize, known_folder, log};

pub enum Batch {
    /// Replaces all apps and settings.
    Apps(Vec<Entry>),
    /// Files and folders; `first` starts a new scan (drop the old ones).
    Files { entries: Vec<Entry>, first: bool },
}

const MAX_FILES: usize = 300_000;
const MAX_DEPTH: usize = 8;
const RESCAN: Duration = Duration::from_secs(30 * 60);

fn display(item: &IShellItem, sigdn: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(sigdn).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn apps() -> Vec<Entry> {
    let mut out = Vec::new();
    let Some(folder) = shell_item("shell:AppsFolder") else {
        log("catalog: shell:AppsFolder is not available");
        return out;
    };
    unsafe {
        let Ok(en) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return out };
        loop {
            let mut items: [Option<IShellItem>; 32] = Default::default();
            let mut fetched = 0u32;
            if en.Next(&mut items, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            for item in items.iter().take(fetched as usize).flatten() {
                let (Some(name), Some(id)) = (display(item, SIGDN_NORMALDISPLAY), display(item, SIGDN_PARENTRELATIVEPARSING)) else {
                    continue;
                };
                if name.trim().is_empty() {
                    continue;
                }
                out.push(Entry::new(name, format!("shell:AppsFolder\\{id}"), Kind::App));
            }
        }
    }
    out
}

fn settings(ru: bool) -> Vec<Entry> {
    const PAGES: &[(&str, &str, &str)] = &[
        ("Параметры", "Settings", "ms-settings:"),
        ("Дисплей", "Display", "ms-settings:display"),
        ("Звук", "Sound", "ms-settings:sound"),
        ("Bluetooth и устройства", "Bluetooth & devices", "ms-settings:bluetooth"),
        ("Wi-Fi", "Wi-Fi", "ms-settings:network-wifi"),
        ("Сеть и Интернет", "Network & internet", "ms-settings:network"),
        ("VPN", "VPN", "ms-settings:network-vpn"),
        ("Персонализация", "Personalization", "ms-settings:personalization"),
        ("Фон рабочего стола", "Background", "ms-settings:personalization-background"),
        ("Цвета", "Colors", "ms-settings:colors"),
        ("Темы", "Themes", "ms-settings:themes"),
        ("Экран блокировки", "Lock screen", "ms-settings:lockscreen"),
        ("Ночной свет", "Night light", "ms-settings:nightlight"),
        ("Приложения", "Apps", "ms-settings:appsfeatures"),
        ("Приложения по умолчанию", "Default apps", "ms-settings:defaultapps"),
        ("Автозагрузка", "Startup apps", "ms-settings:startupapps"),
        ("Учётные записи", "Accounts", "ms-settings:yourinfo"),
        ("Время и язык", "Time & language", "ms-settings:dateandtime"),
        ("Язык и регион", "Language & region", "ms-settings:regionlanguage"),
        ("Клавиатура", "Keyboard", "ms-settings:keyboard"),
        ("Мышь", "Mouse", "ms-settings:mousetouchpad"),
        ("Сенсорная панель", "Touchpad", "ms-settings:devices-touchpad"),
        ("Принтеры и сканеры", "Printers & scanners", "ms-settings:printers"),
        ("Питание и батарея", "Power & battery", "ms-settings:powersleep"),
        ("Память", "Storage", "ms-settings:storagesense"),
        ("Уведомления", "Notifications", "ms-settings:notifications"),
        ("Центр обновления Windows", "Windows Update", "ms-settings:windowsupdate"),
        ("Конфиденциальность и защита", "Privacy & security", "ms-settings:privacy"),
        ("Безопасность Windows", "Windows Security", "windowsdefender:"),
        ("Специальные возможности", "Accessibility", "ms-settings:easeofaccess"),
        ("О системе", "About", "ms-settings:about"),
        ("Панель управления", "Control Panel", "control"),
    ];
    PAGES.iter().map(|(r, e, uri)| Entry::new(if ru { *r } else { *e }, *uri, Kind::Setting)).collect()
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('$')
        || matches!(name, "node_modules" | "__pycache__" | "target" | "obj" | "bin" | "AppData" | "My Music" | "My Pictures" | "My Videos")
}

fn scan(send: &mut dyn FnMut(Vec<Entry>, bool)) {
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const REPARSE: u32 = 0x400;
    let roots: Vec<PathBuf> = [
        &FOLDERID_Desktop,
        &FOLDERID_Documents,
        &FOLDERID_Downloads,
        &FOLDERID_Pictures,
        &FOLDERID_Music,
        &FOLDERID_Videos,
    ]
    .iter()
    .filter_map(|id| known_folder(id))
    .collect();
    let mut stack: Vec<(PathBuf, usize)> = roots.into_iter().map(|p| (p, 0)).collect();
    let mut batch: Vec<Entry> = Vec::with_capacity(4096);
    let mut total = 0usize;
    let mut first = true;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let Ok(md) = ent.metadata() else { continue };
            let attrs = md.file_attributes();
            if attrs & (HIDDEN | SYSTEM) != 0 {
                continue;
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            let path = ent.path();
            if md.is_dir() {
                if skip_dir(&name) {
                    continue;
                }
                batch.push(Entry::new(name, path.to_string_lossy(), Kind::Folder));
                // Junctions can loop back on themselves.
                if depth + 1 < MAX_DEPTH && attrs & REPARSE == 0 {
                    stack.push((path, depth + 1));
                }
            } else {
                batch.push(Entry::new(name, path.to_string_lossy(), Kind::File));
            }
            total += 1;
            if batch.len() >= 4096 {
                send(std::mem::take(&mut batch), first);
                first = false;
            }
            if total >= MAX_FILES {
                send(batch, first);
                return;
            }
        }
    }
    send(batch, first);
}

pub fn spawn(notify: HWND, notify_msg: u32, ru: bool) -> Receiver<Batch> {
    let (tx, rx) = channel();
    let target = hwnd_to_isize(notify);
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let wake = || unsafe {
            let _ = PostMessageW(Some(hwnd_from_isize(target)), notify_msg, WPARAM(0), LPARAM(0));
        };
        loop {
            let mut list = apps();
            list.extend(settings(ru));
            if tx.send(Batch::Apps(list)).is_err() {
                return;
            }
            wake();
            let mut alive = true;
            scan(&mut |entries, first| {
                if alive && tx.send(Batch::Files { entries, first }).is_ok() {
                    wake();
                } else {
                    alive = false;
                }
            });
            if !alive {
                return;
            }
            std::thread::sleep(RESCAN);
        }
    });
    rx
}
