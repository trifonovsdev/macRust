//! Diagnostic report: what macRust draws and how it behaves on this PC, for
//! the developer. Written to %APPDATA%\macRust\report and never sent anywhere.
//!
//! It holds no personal data: no window titles, notification text, media
//! titles or file names. Paths are reduced to file names and the user profile
//! path and user name are replaced.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use macrust_ui::tiny_skia::{Pixmap, PixmapPaint, Transform};
use macrust_ui::WinRect;
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};

use super::super::system;
use super::super::util::{data_dir, redact, trace, trace_dump};
use super::App;

pub(super) fn dir() -> PathBuf {
    data_dir().join("report")
}

pub(super) fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| redact(path))
}

/// The wallpaper under a window, with the window's pixels composited on top.
pub(super) fn over_wallpaper(wall: &Pixmap, win: &Pixmap, r: WinRect) -> Option<Pixmap> {
    let mut out = Pixmap::new(win.width(), win.height())?;
    out.draw_pixmap(-r.x, -r.y, wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    out.draw_pixmap(0, 0, win.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    Some(out)
}

fn save(pm: &Pixmap, dir: &Path, name: &str, notes: &mut String) {
    if let Err(e) = pm.save_png(dir.join(name)) {
        let _ = writeln!(notes, "could not save {name}: {e}");
    }
}

fn windows_version() -> String {
    let key = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let get = |v: &str| system::reg_string(HKEY_LOCAL_MACHINE, key, v).unwrap_or_default();
    let ubr = system::reg_dword(HKEY_LOCAL_MACHINE, key, "UBR").unwrap_or(0);
    format!("{} {} (build {}.{ubr})", get("ProductName"), get("DisplayVersion"), get("CurrentBuild"))
}

impl App {
    pub(super) fn write_report(&mut self, open_folder: bool) {
        let dir = dir();
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        trace("report: writing");
        let mut info = String::new();
        let _ = writeln!(info, "macRust {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(info, "Windows: {}", windows_version());
        let monitors = unsafe { GetSystemMetrics(SM_CMONITORS) };
        let _ = writeln!(
            info,
            "screen: {}x{} scale {:.2}, monitors {monitors}, dark {}, fullscreen app {}",
            self.screen.w, self.screen.h, self.screen.scale, self.ctx.theme.dark, self.fullscreen
        );
        let st = &self.settings;
        let _ = writeln!(
            info,
            "settings: icon {} magnified {} magnify {} hide_taskbar {} reserve_dock {} appearance {} tray_icons {} banners {}",
            st.icon_size, st.magnified, st.magnify, st.hide_taskbar, st.reserve_dock, st.appearance, st.tray_icons, st.banners
        );
        let _ = writeln!(info, "taskbar hidden: {}", self.taskbar_hidden);

        let _ = writeln!(info, "\n[dock]");
        for (key, t) in &self.targets {
            let src = match &t.source {
                Some(super::Source::Exe(p)) => format!("exe {}", file_name(p)),
                Some(super::Source::Aumid(a)) => format!("aumid {a}"),
                None => "special".into(),
            };
            let icon = t.source.as_ref().map(|s| self.dock_icons.contains_key(&s.parse_name().to_lowercase()));
            let _ = writeln!(
                info,
                "{} | {src} | pinned {} running {} tray-only {} icon {:?}",
                redact(key).replace('\\', "/").rsplit('/').next().unwrap_or(""),
                t.pinned,
                t.app.is_some(),
                t.alive,
                icon
            );
        }
        let _ = writeln!(info, "\n[running apps]");
        for a in &self.running {
            let _ = writeln!(
                info,
                "{} | aumid {:?} | windows {} | relaunch {:?}",
                file_name(&a.exe),
                a.aumid,
                a.windows.len(),
                a.relaunch_exe.as_deref().map(file_name)
            );
        }
        for a in &self.lingering {
            let _ = writeln!(info, "{} | no windows, process alive", file_name(&a.exe));
        }
        self.report_extras(&mut info);
        let _ = writeln!(info, "\n[render, ms: average / worst]");
        for (name, s) in self.stats.iter() {
            if s.n > 0 {
                let _ = writeln!(info, "{name}: {:.2} / {:.2} over {} frames", s.total / s.n as f64, s.worst, s.n);
            }
        }
        let log = std::fs::read_to_string(data_dir().join("log.txt")).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(120).collect();
        let _ = writeln!(info, "\n[log, newest last]");
        for l in tail.into_iter().rev() {
            let _ = writeln!(info, "{}", redact(l));
        }

        // Pictures of exactly what macRust draws, over the desktop picture.
        let wall = system::wallpaper(self.screen.w as u32, self.screen.h as u32).unwrap_or_else(|| {
            macrust_ui::wallpaper::procedural(self.screen.w as u32, self.screen.h as u32, self.ctx.theme.dark)
        });
        let mut notes = String::new();
        let br = self.bar.window_rect();
        if let Some(mut pm) = Pixmap::new(br.w.max(1) as u32, br.h.max(1) as u32) {
            self.bar.draw(&mut pm, &self.ctx);
            if let Some(out) = over_wallpaper(&wall, &pm, br) {
                save(&out, &dir, "menu-bar.png", &mut notes);
            }
        }
        let dr = self.dock.window_rect();
        if let Some(mut pm) = Pixmap::new(dr.w.max(1) as u32, dr.h.max(1) as u32) {
            self.dock.draw(&mut pm, &self.ctx);
            if let Some(out) = over_wallpaper(&wall, &pm, dr) {
                save(&out, &dir, "dock.png", &mut notes);
            }
        }
        self.report_images(&wall, &dir, &mut notes);
        info.push_str(&notes);
        let _ = std::fs::write(dir.join("info.txt"), info);
        let _ = std::fs::write(dir.join("events.txt"), redact(&trace_dump()));
        trace("report: done");
        if open_folder {
            system::open(&dir.to_string_lossy());
        }
    }
}
