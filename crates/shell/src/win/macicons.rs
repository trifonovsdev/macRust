//! macOS icons for the dock. A user's own PNG in `%APPDATA%\macRust\icons`
//! wins; then the official macOS artwork of a catalog app
//! (`icon_logic::CATALOG`), fetched once into `%LOCALAPPDATA%\macRust\icons`.
//! Local lookups run on one thread and downloads on another, so a slow
//! network never holds back the icons already on disk.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use macrust_ui::gfx;
use macrust_ui::tiny_skia::Pixmap;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::EnhancedStorage::PKEY_Link_TargetParsingPath;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, IShellItem2};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

use super::icons::shell_item;
use super::net;
use super::util::{data_dir, hwnd_from_isize, hwnd_to_isize, known_folder, log};
use crate::icon_logic::{self as logic, App, Origin};

/// A failed download is tried again after this long (a marker file's age).
const RETRY_AFTER: Duration = Duration::from_secs(24 * 3600);
/// Twice the largest dock icon at 100% scaling; enough for 200% at rest.
const ICON_PX: u32 = 256;

const FETCH: net::Fetch = net::Fetch {
    hosts: &logic::ICON_HOSTS,
    agent: "macRust",
    headers: "",
    deadline: Duration::from_secs(60),
    tag: "icons",
};

pub enum Art {
    /// A complete icon: drawn as it is.
    AsIs(Pixmap),
    /// Full-bleed square artwork, still to be cut to the macOS shape.
    Square(Pixmap),
}

pub struct Reply {
    pub key: String,
    pub parse_name: String,
    pub art: Option<Art>,
    /// From a download that finished after the first, empty reply.
    pub fetched: bool,
}

pub struct MacIcons {
    tx: Sender<(String, String)>,
    pub rx: Receiver<Reply>,
}

impl MacIcons {
    /// Replies wake the UI thread with `notify_msg`.
    pub fn spawn(notify: HWND, notify_msg: u32) -> Self {
        let (tx, req_rx) = channel::<(String, String)>();
        let (res_tx, rx) = channel::<Reply>();
        let (fetch_tx, fetch_rx) = channel::<(String, String, &'static App)>();
        let target = hwnd_to_isize(notify);
        let post = move || unsafe {
            let _ = PostMessageW(Some(hwnd_from_isize(target)), notify_msg, WPARAM(0), LPARAM(0));
        };
        let user = data_dir().join("icons");
        let _ = std::fs::create_dir_all(&user);
        let cache = known_folder(&FOLDERID_LocalAppData).map(|d| d.join("macRust").join("icons"));
        let fetch_res = res_tx.clone();
        let fetch_cache = cache.clone();
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            while let Ok((key, parse_name)) = req_rx.recv() {
                let (art, fetch) = local(&parse_name, &user, cache.as_deref());
                if let (None, Some(app)) = (&art, fetch) {
                    let _ = fetch_tx.send((key.clone(), parse_name.clone(), app));
                }
                if res_tx.send(Reply { key, parse_name, art, fetched: false }).is_err() {
                    break;
                }
                post();
            }
        });
        std::thread::spawn(move || {
            let Some(cache) = fetch_cache else { return };
            while let Ok((key, parse_name, app)) = fetch_rx.recv() {
                if let Some(art) = cached_or_fetched(app, &cache) {
                    if fetch_res.send(Reply { key, parse_name, art: Some(art), fetched: true }).is_err() {
                        break;
                    }
                    post();
                }
            }
        });
        Self { tx, rx }
    }

    pub fn request(&self, key: String, parse_name: String) {
        let _ = self.tx.send((key, parse_name));
    }
}

/// The exe stem and AppUserModelID behind a dock source's parsing name.
fn ident(parse_name: &str) -> (Option<String>, Option<String>) {
    if let Some(id) = parse_name.strip_prefix("shell:AppsFolder\\") {
        // A Start entry of a desktop program points at its exe; packaged apps have no target.
        return (link_target(parse_name).and_then(|p| logic::exe_stem(&p)), Some(id.to_string()));
    }
    if parse_name.to_lowercase().ends_with(".exe") {
        return (logic::exe_stem(parse_name), None);
    }
    (None, None)
}

fn link_target(parse_name: &str) -> Option<String> {
    let item: IShellItem2 = shell_item(parse_name)?.cast().ok()?;
    unsafe {
        let p = item.GetString(&PKEY_Link_TargetParsingPath).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// The user's override or a cached download; otherwise the catalog app to
/// fetch, unless its last attempt failed less than a day ago.
fn local(parse_name: &str, user: &Path, cache: Option<&Path>) -> (Option<Art>, Option<&'static App>) {
    let (stem, aumid) = ident(parse_name);
    if stem.is_none() && aumid.is_none() {
        return (None, None);
    }
    let app = logic::find(stem.as_deref(), aumid.as_deref());
    for name in logic::override_names(stem.as_deref(), aumid.as_deref(), app) {
        let path = user.join(format!("{name}.png"));
        if path.is_file() {
            match load(&path) {
                Some(pm) => return (Some(Art::AsIs(pm)), None),
                None => log(format!("icons: {name}.png in the icons folder is not a usable PNG")),
            }
        }
    }
    let (Some(app), Some(cache)) = (app, cache) else { return (None, None) };
    if let Some(pm) = load(&cache.join(format!("{}.png", app.key))) {
        return (Some(classify(pm)), None);
    }
    (None, (!recently_failed(cache, app)).then_some(app))
}

fn cached_or_fetched(app: &'static App, cache: &Path) -> Option<Art> {
    let path = cache.join(format!("{}.png", app.key));
    // Another dock entry of the same app may have fetched it meanwhile.
    if let Some(pm) = load(&path) {
        return Some(classify(pm));
    }
    if recently_failed(cache, app) {
        return None;
    }
    let marker = cache.join(format!("{}.failed", app.key));
    match fetch(app).and_then(|png| {
        let pm = decode(&png).ok_or("the icon does not decode")?;
        std::fs::create_dir_all(cache).map_err(|e| format!("cache folder: {e}"))?;
        let part = cache.join(format!("{}.png.part", app.key));
        std::fs::write(&part, &png).map_err(|e| format!("write: {e}"))?;
        std::fs::rename(&part, &path).map_err(|e| format!("rename: {e}"))?;
        Ok(pm)
    }) {
        Ok(pm) => {
            let _ = std::fs::remove_file(&marker);
            log(format!("icons: {} fetched", app.key));
            Some(classify(pm))
        }
        Err(e) => {
            log(format!("icons: {}: {e}", app.key));
            let _ = std::fs::create_dir_all(cache);
            let _ = std::fs::write(&marker, b"");
            None
        }
    }
}

fn recently_failed(cache: &Path, app: &App) -> bool {
    std::fs::metadata(cache.join(format!("{}.failed", app.key)))
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < RETRY_AFTER))
}

/// The icon PNG from the app's source, checked but not yet decoded.
fn fetch(app: &App) -> Result<Vec<u8>, String> {
    let url = logic::source_url(app.origin).ok_or("source url is not allowed")?;
    let body = match app.origin {
        Origin::AppStore(_) => {
            let json = net::get_bytes(&FETCH, &url, logic::MAX_LOOKUP_BYTES)?;
            let art = logic::artwork_url(&String::from_utf8_lossy(&json)).ok_or("no artwork in the lookup")?;
            net::get_bytes(&FETCH, &art, logic::MAX_ICON_BYTES)?
        }
        Origin::Png(_) | Origin::Icns(_) => net::get_bytes(&FETCH, &url, logic::MAX_ICON_BYTES)?,
    };
    logic::icon_from_body(app.origin, &body).map(<[u8]>::to_vec).ok_or_else(|| "not an acceptable icon".into())
}

fn load(path: &Path) -> Option<Pixmap> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 2 * logic::MAX_ICON_BYTES {
        return None;
    }
    decode(&std::fs::read(path).ok()?)
}

/// Decodes after checking the header, so a tiny file cannot claim a huge picture.
fn decode(png: &[u8]) -> Option<Pixmap> {
    let (w, h) = logic::png_size(png)?;
    if w == 0 || h == 0 || w.max(h) > logic::MAX_ICON_PX {
        return None;
    }
    let pm = Pixmap::decode_png(png).ok()?;
    Some(gfx::shrink(&pm, ICON_PX))
}

fn classify(pm: Pixmap) -> Art {
    if logic::opaque_corners(pm.width(), pm.height(), pm.data()) { Art::Square(pm) } else { Art::AsIs(pm) }
}
