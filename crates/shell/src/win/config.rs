//! User settings and the pinned dock items, as plain text files in %APPDATA%\macRust.

use std::path::PathBuf;

use super::util::data_dir;

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A program file path.
    Exe(String),
    /// An AppUserModelID, launched through shell:AppsFolder.
    Aumid(String),
}

impl Source {
    /// The name the shell can parse for icons and launching.
    pub fn parse_name(&self) -> String {
        match self {
            Source::Exe(p) => p.clone(),
            Source::Aumid(id) => format!("shell:AppsFolder\\{id}"),
        }
    }

    pub fn key(&self) -> String {
        match self {
            Source::Exe(p) => p.to_lowercase(),
            Source::Aumid(id) => format!("aumid:{}", id.to_lowercase()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pin {
    pub name: String,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub icon_size: f32,
    pub magnified: f32,
    pub magnify: bool,
    pub hide_taskbar: bool,
    pub reserve_dock: bool,
    /// "auto", "light" or "dark".
    pub appearance: String,
    /// Host the notification area so tray icons appear in the menu bar.
    pub tray_icons: bool,
    /// Announce new Windows notifications with banners at the top right.
    pub banners: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            icon_size: 52.0,
            magnified: 92.0,
            magnify: true,
            hide_taskbar: true,
            reserve_dock: true,
            appearance: "auto".into(),
            tray_icons: true,
            banners: true,
        }
    }
}

/// Reads a text file in any of the encodings Notepad saves; `None` only if it
/// does not exist. Other read errors keep the file untouched.
fn read_text(path: &std::path::Path) -> Option<Result<String, ()>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = if bytes.starts_with(&[0xFF, 0xFE]) {
                let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&units)
            } else {
                let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
                String::from_utf8_lossy(b).into_owned()
            };
            Some(Ok(text))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(Err(())),
    }
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.txt")
}

fn dock_path() -> PathBuf {
    data_dir().join("dock.txt")
}

pub fn load_settings() -> Settings {
    let mut s = Settings::default();
    let text = match read_text(&settings_path()) {
        Some(Ok(t)) => t,
        Some(Err(())) => return s,
        None => {
            save_settings(&s);
            return s;
        }
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let b = matches!(v, "1" | "true" | "yes" | "on");
        match k {
            "icon_size" => s.icon_size = v.parse().unwrap_or(s.icon_size).clamp(24.0, 128.0),
            "magnified_size" => s.magnified = v.parse().unwrap_or(s.magnified).clamp(24.0, 160.0),
            "magnification" => s.magnify = b,
            "hide_taskbar" => s.hide_taskbar = b,
            "reserve_dock_space" => s.reserve_dock = b,
            "appearance" => s.appearance = v.to_string(),
            "tray_icons" => s.tray_icons = b,
            "banners" => s.banners = b,
            _ => {}
        }
    }
    s
}

pub fn save_settings(s: &Settings) {
    let text = format!(
        "# macRust settings. Restart macRust after editing.\n\
         icon_size = {}\nmagnified_size = {}\nmagnification = {}\nhide_taskbar = {}\nreserve_dock_space = {}\n\
         # auto | light | dark\nappearance = {}\ntray_icons = {}\nbanners = {}\n",
        s.icon_size, s.magnified, s.magnify, s.hide_taskbar, s.reserve_dock, s.appearance, s.tray_icons, s.banners
    );
    let _ = std::fs::write(settings_path(), text);
}

/// `None` when the file does not exist yet (first run).
pub fn load_pins() -> Option<Vec<Pin>> {
    let text = match read_text(&dock_path())? {
        Ok(t) => t,
        // Unreadable for now (locked, permissions): show no pins, never overwrite.
        Err(()) => return Some(Vec::new()),
    };
    let mut pins = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(name), Some(kind), Some(value)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let source = match kind {
            "exe" => Source::Exe(value.to_string()),
            "aumid" => Source::Aumid(value.to_string()),
            _ => continue,
        };
        pins.push(Pin { name: name.to_string(), source });
    }
    Some(pins)
}

pub fn save_pins(pins: &[Pin]) {
    let mut text = String::from("# name|exe|path  or  name|aumid|AppUserModelID\n");
    for p in pins {
        let (kind, value) = match &p.source {
            Source::Exe(v) => ("exe", v),
            Source::Aumid(v) => ("aumid", v),
        };
        text.push_str(&format!("{}|{kind}|{value}\n", p.name.replace('|', " ")));
    }
    let _ = std::fs::write(dock_path(), text);
}
