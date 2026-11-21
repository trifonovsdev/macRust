//! Renders the file manager window into PNGs in light and dark at 1x and 2x,
//! in icon and list view, and times frames of a 20 000-file folder.
//!
//! cargo run --release -p macrust-ui --example files -- out/files

use std::rc::Rc;
use std::time::Instant;

use macrust_ui::files::art::{Art, PlaceIcon};
use macrust_ui::files::format::Civil;
use macrust_ui::files::layout::{Grid, View};
use macrust_ui::files::model::{ATTR_HIDDEN, Column, Entry, Sort};
use macrust_ui::files::strings::FileStrings;
use macrust_ui::files::{Browser, Place, Section};
use macrust_ui::gfx::{self, Image, rgba};
use macrust_ui::strings::Strings;
use macrust_ui::text::{Text, Weight};
use macrust_ui::theme::Theme;
use macrust_ui::tiny_skia::{Color, Pixmap};
use macrust_ui::{Ctx, Mods};

/// A stand-in for a shell file icon: a page with a coloured band and a label.
fn doc_icon(text: &Text, band: Color, label: &str) -> Rc<Image> {
    let n = 256.0;
    let mut pm = Pixmap::new(256, 256).unwrap();
    macrust_ui::files::art::document(&mut pm, 0.0, 0.0, n, false);
    gfx::fill_rect(&mut pm, n * 0.19, n * 0.6, n * 0.62, n * 0.2, band);
    let fs = 40.0;
    let w = text.measure(label, fs, Weight::Bold);
    text.draw(&mut pm, label, (n - w) / 2.0, n * 0.75, fs, Weight::Bold, rgba(255, 255, 255, 1.0), None);
    Rc::new(Image::new(pm))
}

fn app_icon() -> Rc<Image> {
    let mut pm = Pixmap::new(256, 256).unwrap();
    if let Some(p) = gfx::squircle(24.0, 24.0, 208.0, 208.0, 46.0) {
        gfx::fill(&mut pm, &p, rgba(52, 120, 246, 1.0));
    }
    if let Some(p) = gfx::circle(128.0, 128.0, 56.0) {
        gfx::stroke(&mut pm, &p, rgba(255, 255, 255, 0.95), 16.0);
    }
    Rc::new(Image::new(pm))
}

fn photo(seed: u32) -> Rc<Image> {
    let (w, h) = (256u32, 170u32);
    let mut pm = Pixmap::new(w, h).unwrap();
    let hue = (seed * 47 % 255) as u8;
    for y in 0..h {
        let t = y as f32 / h as f32;
        let c = rgba((hue as f32 * (1.0 - t) + 40.0 * t) as u8, (120.0 + 80.0 * t) as u8, (220.0 - 120.0 * t) as u8, 1.0);
        gfx::fill_rect(&mut pm, 0.0, y as f32, w as f32, 1.0, c);
    }
    if let Some(p) = gfx::circle(180.0, 50.0, 22.0) {
        gfx::fill(&mut pm, &p, rgba(255, 236, 170, 1.0));
    }
    Rc::new(Image::new(pm))
}

fn sections(s: &'static FileStrings) -> Vec<Section> {
    let ru = s.ru;
    let fav = [
        (if ru { "Рабочий стол" } else { "Desktop" }, "C:\\Users\\anna\\Desktop", PlaceIcon::Desktop),
        (if ru { "Загрузки" } else { "Downloads" }, "C:\\Users\\anna\\Downloads", PlaceIcon::Downloads),
        (if ru { "Документы" } else { "Documents" }, "C:\\Users\\anna\\Documents", PlaceIcon::Documents),
        (if ru { "Изображения" } else { "Pictures" }, "C:\\Users\\anna\\Pictures", PlaceIcon::Pictures),
        (if ru { "Музыка" } else { "Music" }, "C:\\Users\\anna\\Music", PlaceIcon::Music),
        (if ru { "Видео" } else { "Videos" }, "C:\\Users\\anna\\Videos", PlaceIcon::Videos),
        ("anna", "C:\\Users\\anna", PlaceIcon::Home),
    ];
    let loc = [
        (if ru { "Локальный диск (C:)" } else { "Local Disk (C:)" }, "C:\\", PlaceIcon::Drive, false),
        (if ru { "Данные (D:)" } else { "Data (D:)" }, "D:\\", PlaceIcon::Drive, false),
        ("KINGSTON (E:)", "E:\\", PlaceIcon::Removable, true),
        (s.trash, "shell:RecycleBinFolder", PlaceIcon::Trash, false),
    ];
    vec![
        Section {
            title: s.favourites,
            places: fav.iter().map(|(n, p, i)| Place { name: n.to_string(), path: p.to_string(), icon: *i, eject: false }).collect(),
        },
        Section {
            title: s.locations,
            places: loc.iter().map(|(n, p, i, e)| Place { name: n.to_string(), path: p.to_string(), icon: *i, eject: *e }).collect(),
        },
    ]
}

/// FILETIME for days ago from a fixed "now" (2026-11-21 14:00 UTC).
fn ft(days_ago: f64) -> u64 {
    let now = 1_795_269_600i64;
    let t = now - (days_ago * 86_400.0) as i64;
    (t * 10_000_000 + 116_444_736_000_000_000) as u64
}

fn downloads(ru: bool) -> Vec<Entry> {
    let mut v = vec![
        Entry::new(if ru { "Проекты" } else { "Projects" }, true, 0, ft(0.1), 0),
        Entry::new(if ru { "Фото с отпуска" } else { "Holiday photos" }, true, 0, ft(1.2), 0),
        Entry::new("macRust-0.7.4", true, 0, ft(3.0), 0),
        Entry::new("node_modules", true, 0, ft(40.0), 0),
        Entry::new(if ru { "Отчёт за ноябрь.docx" } else { "November report.docx" }, false, 48_211, ft(0.02), 0),
        Entry::new("Apple-iPhone-17-color-lineup-250909.zip", false, 1_604_000, ft(2.5), 0),
        Entry::new("IMG_2531.jpeg", false, 3_276_544, ft(5.0), 0),
        Entry::new("IMG_2534.HEIC", false, 2_140_100, ft(5.1), 0),
        Entry::new("setup-tool-x64.exe", false, 88_400_000, ft(12.0), 0),
        Entry::new(if ru { "Счёт №1042.pdf" } else { "Invoice 1042.pdf" }, false, 211_000, ft(30.0), 0),
        Entry::new("notes.txt", false, 912, ft(0.9), 0),
        Entry::new("budget-2026.xlsx", false, 19_300, ft(64.0), 0),
        Entry::new("desktop.ini", false, 282, ft(400.0), ATTR_HIDDEN),
        Entry::new("screenshot 2026-11-20 at 18.42.11.png", false, 640_200, ft(0.8), 0),
        Entry::new("music.mp3", false, 7_450_000, ft(90.0), 0),
        Entry::new("archive.tar.gz", false, 1_220_000_000, ft(200.0), 0),
        Entry::new("README.md", false, 4_096, ft(7.0), 0),
        Entry::new("Telegram Desktop", true, 0, ft(10.0), 0),
        Entry::new("video_2026-11-01.mp4", false, 152_000_000, ft(20.0), 0),
        Entry::new("Ярлык Steam.lnk", false, 1_400, ft(100.0), 0),
    ];
    for i in 1..=12 {
        v.push(Entry::new(format!("Scan_{i:03}.pdf"), false, 120_000 + i * 3_000, ft(i as f64 * 1.7), 0));
    }
    v
}

fn kinds(ru: bool) -> Vec<(String, String)> {
    let k = [
        ("docx", "Документ Microsoft Word", "Microsoft Word Document"),
        ("zip", "Сжатая ZIP-папка", "Compressed (zipped) Folder"),
        ("jpeg", "Файл \"JPEG\"", "JPEG File"),
        ("heic", "Файл \"HEIC\"", "HEIC File"),
        ("exe", "Приложение", "Application"),
        ("pdf", "Документ PDF", "PDF Document"),
        ("txt", "Текстовый документ", "Text Document"),
        ("xlsx", "Лист Microsoft Excel", "Microsoft Excel Worksheet"),
        ("png", "Файл \"PNG\"", "PNG File"),
        ("mp3", "Файл \"MP3\"", "MP3 File"),
        ("gz", "Архив GZ", "GZ Archive"),
        ("md", "Файл \"MD\"", "MD File"),
        ("mp4", "Файл \"MP4\"", "MP4 File"),
        ("lnk", "Ярлык", "Shortcut"),
        ("ini", "Параметры конфигурации", "Configuration settings"),
    ];
    k.iter().map(|(e, r, en)| (e.to_string(), if ru { r } else { en }.to_string())).collect()
}

fn fill_icons(b: &mut Browser, text: &Text) {
    for r in b.icon_requests() {
        let ext = r.key.rsplit(['.', ':']).next().unwrap_or("").to_string();
        let img = if r.thumbnail {
            Some(photo(r.key.len() as u32))
        } else {
            match ext.as_str() {
                "docx" => Some(doc_icon(text, rgba(43, 87, 154, 1.0), "W")),
                "xlsx" => Some(doc_icon(text, rgba(33, 115, 70, 1.0), "X")),
                "pdf" => Some(doc_icon(text, rgba(214, 48, 40, 1.0), "PDF")),
                "zip" | "gz" => Some(doc_icon(text, rgba(120, 120, 128, 1.0), "ZIP")),
                "exe" | "lnk" => Some(app_icon()),
                "txt" | "md" => None,
                _ => Some(doc_icon(text, rgba(140, 90, 200, 1.0), &ext.to_uppercase())),
            }
        };
        b.icon_loaded(r.key, img);
    }
}

fn ctx(dark: bool, ru: bool) -> Ctx {
    let strings: &'static Strings = if ru { &Strings::RU } else { &Strings::EN };
    Ctx::new(Text::bundled(), Theme { dark }, strings)
}

fn now() -> Civil {
    Civil { year: 2026, month: 11, day: 21, hour: 17, minute: 0 }
}

#[allow(clippy::too_many_arguments)]
fn shot(out: &str, name: &str, dark: bool, scale: f32, view: View, ru: bool, setup: &dyn Fn(&mut Browser, &Ctx)) {
    let ctx = ctx(dark, ru);
    let fs: &'static FileStrings = if ru { &FileStrings::RU } else { &FileStrings::EN };
    let (w, h) = (1100.0 * scale, 640.0 * scale);
    let mut b = Browser::new(fs, Rc::new(Art::new()), w, h, scale);
    b.sections = sections(fs);
    b.root_names = vec![("C:\\".into(), if ru { "Локальный диск (C:)".into() } else { "Local Disk (C:)".into() })];
    b.now = now();
    b.tz = 3 * 3600;
    let _ = b.navigate("C:\\Users\\anna\\Downloads");
    b.loaded(downloads(ru), kinds(ru), true, Some(120_600_000_000));
    let _ = b.set_view(view);
    setup(&mut b, &ctx);
    fill_icons(&mut b, &ctx.text);
    let mut pm = Pixmap::new(b.w as u32, b.h as u32).unwrap();
    b.draw(&mut pm, &ctx);
    pm.save_png(format!("{out}/{name}.png")).unwrap();
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "out/files".into());
    std::fs::create_dir_all(&out).unwrap();
    for &(dark, tag) in &[(false, "light"), (true, "dark")] {
        for &scale in &[1.0f32, 2.0] {
            let sfx = format!("{tag}-{}x", scale as u32);
            shot(&out, &format!("icons-{sfx}"), dark, scale, View::Icons, true, &|b, _| {
                b.list.click(5, false, false);
                b.list.click(7, true, false);
            });
            shot(&out, &format!("list-{sfx}"), dark, scale, View::List, true, &|b, _| {
                b.list.click(4, false, false);
                b.list.click(6, false, true);
                b.list.set_sort(Sort { column: Column::Modified, ascending: false, folders_first: true });
                b.wheel(40.0 * b.scale, false);
            });
        }
    }
    // States: inactive window with rename, search, hover on lights, band.
    shot(&out, "state-rename-en", false, 1.0, View::Icons, false, &|b, _| {
        b.list.select_only(4);
        b.begin_rename(4);
    });
    shot(&out, "state-search-dark", true, 1.0, View::List, true, &|b, ctx| {
        b.key(0x46, Mods { ctrl: true, ..Mods::default() });
        for c in "scan".chars() {
            b.char(c);
        }
        let f = b.frame();
        b.mouse_move(f.light(1).x + 3.0, f.light(1).y + 3.0, &ctx.text);
    });
    shot(&out, "state-band-inactive", false, 1.0, View::Icons, true, &|b, ctx| {
        b.active = false;
        let f = b.frame();
        let g = Grid::new(f.content.w, b.scale);
        b.mouse_down(f.content.x + 4.0, f.content.y + 4.0, Mods::default(), false, &ctx.text);
        let t = g.tile(g.cols + 2);
        b.mouse_move(f.content.x + t.x + 30.0, f.content.y + t.y + 40.0, &ctx.text);
    });
    shot(&out, "state-narrow-list", false, 1.0, View::List, true, &|b, _| {
        b.resize(640.0, 640.0, 1.0);
    });
    shot(&out, "state-error", true, 1.0, View::Icons, true, &|b, _| {
        let _ = b.navigate("C:\\System Volume Information");
        b.failed(5);
    });

    // 20 000 files: frame times while scrolling, both views.
    for (view, tag) in [(View::List, "list"), (View::Icons, "icons")] {
        let ctx = ctx(false, true);
        let mut b = Browser::new(&FileStrings::RU, Rc::new(Art::new()), 1440.0, 900.0, 1.0);
        b.sections = sections(&FileStrings::RU);
        b.now = now();
        let _ = b.navigate("C:\\big");
        let t0 = Instant::now();
        let entries: Vec<Entry> = (0..20_000).map(|i| Entry::new(format!("file_{i}.dat"), i % 50 == 0, i as u64 * 1000, ft((i % 300) as f64), 0)).collect();
        b.loaded(entries, kinds(true), true, None);
        let load = t0.elapsed();
        let _ = b.set_view(view);
        fill_icons(&mut b, &ctx.text);
        let mut pm = Pixmap::new(1440, 900).unwrap();
        let mut worst = 0.0f64;
        let mut total = 0.0f64;
        let n = 120;
        for i in 0..n {
            b.wheel(37.0, false);
            if i % 10 == 0 {
                fill_icons(&mut b, &ctx.text);
            }
            let t = Instant::now();
            b.draw(&mut pm, &ctx);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            total += ms;
            worst = worst.max(ms);
        }
        let t = Instant::now();
        b.list.set_sort(Sort { column: Column::Size, ascending: false, folders_first: true });
        let sort = t.elapsed();
        println!(
            "20000 files {tag}: load+sort {:.1} ms, resort {:.1} ms, frame avg {:.2} ms worst {:.2} ms (1440x900 @1x)",
            load.as_secs_f64() * 1000.0,
            sort.as_secs_f64() * 1000.0,
            total / n as f64,
            worst
        );
    }
}
