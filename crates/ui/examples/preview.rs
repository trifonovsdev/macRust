//! Renders the shell components over a desktop into PNGs and measures frame
//! times, so the look and the cost can be checked without Windows.
//!
//! cargo run --release -p macrust-ui --example preview -- out/

use std::rc::Rc;
use std::time::Instant;

use macrust_ui::dock::{Dock, DockConfig, DockEntry, Fallback, Section};
use macrust_ui::gfx::{self, Image, rgba};
use macrust_ui::menu::{MenuEntry, PopupMenu};
use macrust_ui::menubar::{BarItem, MenuBar, Status};
use macrust_ui::spotlight::{Glyph, Row, Spotlight};
use macrust_ui::strings::Strings;
use macrust_ui::text::Text;
use macrust_ui::theme::Theme;
use macrust_ui::tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, SpreadMode, Transform,
};
use macrust_ui::wallpaper::{self, Vibrancy};
use macrust_ui::{Ctx, Key, Mods, WinRect};

/// Stand-in app icon: a gradient tile with a simple white mark.
fn fake_icon(top: Color, bottom: Color, mark: u8) -> Rc<Image> {
    let n = 256.0;
    let mut pm = Pixmap::new(256, 256).unwrap();
    let inset = n * 0.1;
    let path = gfx::squircle(inset, inset, n - 2.0 * inset, n - 2.0 * inset, n * 0.18).unwrap();
    let p = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
        Point::from_xy(0.0, inset),
        Point::from_xy(0.0, n - inset),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap(),
        ..Paint::default()
    };
    pm.fill_path(&path, &p, macrust_ui::tiny_skia::FillRule::Winding, Transform::identity(), None);
    let w = rgba(255, 255, 255, 0.95);
    match mark {
        0 => {
            if let Some(c) = gfx::circle(n / 2.0, n / 2.0, n * 0.22) {
                gfx::stroke(&mut pm, &c, w, n * 0.06);
            }
        }
        1 => {
            if let Some(r) = gfx::squircle(n * 0.3, n * 0.32, n * 0.4, n * 0.36, n * 0.05) {
                gfx::fill(&mut pm, &r, w);
            }
        }
        2 => {
            for i in 0..3 {
                gfx::fill_rect(&mut pm, n * 0.3, n * (0.36 + i as f32 * 0.11), n * 0.4, n * 0.05, w);
            }
        }
        3 => macrust_ui::glyphs::logo(&mut pm, n * 0.28, n * 0.28, n * 0.44, w),
        4 => {
            let mut pb = macrust_ui::tiny_skia::PathBuilder::new();
            pb.move_to(n * 0.38, n * 0.3);
            pb.line_to(n * 0.72, n * 0.5);
            pb.line_to(n * 0.38, n * 0.7);
            pb.close();
            gfx::fill(&mut pm, &pb.finish().unwrap(), w);
        }
        _ => {
            if let Some(c) = gfx::circle(n / 2.0, n / 2.0, n * 0.16) {
                gfx::fill(&mut pm, &c, w);
            }
        }
    }
    Rc::new(Image::with_shadow(pm))
}

fn entries() -> Vec<DockEntry> {
    let app = |key: &str, label: &str, icon: Rc<Image>, running: bool| DockEntry {
        key: key.into(),
        label: label.into(),
        icon: Some(icon),
        running,
        section: Section::Apps,
        fallback: Fallback::App,
    };
    vec![
        app("files", "Проводник", fake_icon(rgba(110, 200, 255, 1.0), rgba(20, 120, 240, 1.0), 1), true),
        app("web", "Браузер", fake_icon(rgba(120, 230, 200, 1.0), rgba(10, 150, 140, 1.0), 0), true),
        app("mail", "Почта", fake_icon(rgba(130, 170, 255, 1.0), rgba(60, 90, 230, 1.0), 2), false),
        app("music", "Музыка", fake_icon(rgba(255, 120, 140, 1.0), rgba(230, 40, 80, 1.0), 4), false),
        app("notes", "Заметки", fake_icon(rgba(255, 230, 120, 1.0), rgba(240, 180, 30, 1.0), 2), true),
        app("term", "Терминал", fake_icon(rgba(90, 90, 96, 1.0), rgba(30, 30, 34, 1.0), 5), false),
        app("settings", "Параметры", fake_icon(rgba(190, 190, 198, 1.0), rgba(120, 120, 130, 1.0), 3), false),
        DockEntry {
            key: "downloads".into(),
            label: "Загрузки".into(),
            icon: None,
            running: false,
            section: Section::Stacks,
            fallback: Fallback::Folder,
        },
        DockEntry {
            key: "trash".into(),
            label: "Корзина".into(),
            icon: None,
            running: false,
            section: Section::Stacks,
            fallback: Fallback::Trash,
        },
    ]
}

fn blit(dst: &mut Pixmap, src: &Pixmap, r: WinRect) {
    dst.draw_pixmap(r.x, r.y, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn pixmap_for(r: WinRect) -> Pixmap {
    Pixmap::new(r.w as u32, r.h as u32).unwrap()
}

struct Scene {
    ctx: Ctx,
    wall: Pixmap,
    w: f32,
    h: f32,
    scale: f32,
}

fn scene(w: u32, h: u32, scale: f32, dark: bool) -> Scene {
    let wall = wallpaper::procedural(w, h, dark);
    let mut ctx = Ctx::new(Text::bundled(), Theme { dark }, &Strings::RU);
    ctx.vibrancy = Some(Vibrancy::new(&wall));
    Scene { ctx, wall, w: w as f32, h: h as f32, scale }
}

fn settle_dock(dock: &mut Dock) {
    for _ in 0..240 {
        dock.update(1.0 / 120.0);
    }
}

fn desktop(sc: &Scene, out: &str, name: &str, with_menu: bool, with_spot: bool, mouse_tile: Option<f32>) {
    let mut screen = sc.wall.clone();
    let mut bar = MenuBar::new();
    bar.set_screen(sc.w, sc.scale, &sc.ctx);
    bar.set_status(
        Status {
            app: "Проводник".into(),
            lang: "RU".into(),
            battery: Some((82, false)),
            wifi: Some(true),
            clock: Strings::RU.clock(5, 21, 10, 14, 32),
        },
        &sc.ctx,
    );
    bar.update_contrast(&sc.ctx);

    let mut dock = Dock::new(DockConfig::default());
    dock.set_screen(sc.w, sc.h, sc.scale);
    dock.set_entries(entries());
    let dr = dock.window_rect();
    if let Some(t) = mouse_tile {
        let tile = 52.0 * 1.13 * sc.scale;
        let n = 9.0;
        let x0 = dr.w as f32 / 2.0 - (n * tile + 16.0 * sc.scale) / 2.0;
        dock.mouse_move(x0 + t * tile, dr.h as f32 - 30.0 * sc.scale);
        settle_dock(&mut dock);
    }

    let mut dpm = pixmap_for(dr);
    dock.draw(&mut dpm, &sc.ctx);
    blit(&mut screen, &dpm, dr);

    if with_menu {
        bar.set_open(Some(BarItem::Logo));
    }
    let br = bar.window_rect();
    let mut bpm = pixmap_for(br);
    bar.draw(&mut bpm, &sc.ctx);
    blit(&mut screen, &bpm, br);

    if with_menu {
        let s = Strings::RU;
        let mut menu = PopupMenu::new(
            vec![
                MenuEntry::item(s.about, 1),
                MenuEntry::sep(),
                MenuEntry::item(s.settings, 2),
                MenuEntry::item(s.store, 3),
                MenuEntry::sep(),
                MenuEntry::item(s.force_quit, 4).shortcut("Ctrl+Shift+Esc"),
                MenuEntry::sep(),
                MenuEntry::item(s.sleep, 5),
                MenuEntry::item(s.restart, 6),
                MenuEntry::item(s.shut_down, 7),
                MenuEntry::sep(),
                MenuEntry::item(s.lock, 8).shortcut("Win+L"),
                MenuEntry::item(s.log_out, 9).enabled(false),
            ],
            1,
            sc.scale,
            &sc.ctx,
        );
        for _ in 0..60 {
            menu.update(1.0 / 60.0);
        }
        let (mw, mh) = menu.window_size();
        let (ox, oy) = menu.body_offset();
        menu.mouse_move(ox as f32 + 40.0, oy as f32 + (5.0 + 22.0 * 2.0 + 11.0 + 8.0) * sc.scale);
        let r = WinRect { x: (6.0 * sc.scale) as i32 - ox, y: bar.height_px() + (2.0 * sc.scale) as i32 - oy, w: mw, h: mh };
        let mut mpm = pixmap_for(r);
        menu.draw(&mut mpm, &sc.ctx);
        blit(&mut screen, &mpm, r);
    }

    if with_spot {
        let mut sp = Spotlight::new();
        sp.set_screen(sc.w, sc.h, sc.scale);
        sp.show();
        for c in "блок".chars() {
            sp.char_input(c);
        }
        sp.set_rows(vec![
            Row { title: "Блокнот".into(), subtitle: "Программа".into(), icon: Some(fake_icon(rgba(130, 170, 255, 1.0), rgba(60, 90, 230, 1.0), 2)), glyph: Glyph::App },
            Row { title: "Блокировка экрана".into(), subtitle: "Параметры".into(), icon: None, glyph: Glyph::Settings },
            Row { title: "блок-схема проекта.pdf".into(), subtitle: "C:\\Users\\me\\Documents\\Работа".into(), icon: None, glyph: Glyph::Document },
            Row { title: "Блоки".into(), subtitle: "C:\\Users\\me\\Pictures".into(), icon: None, glyph: Glyph::Folder },
            Row { title: "Искать в интернете «блок»".into(), subtitle: String::new(), icon: None, glyph: Glyph::Web },
        ]);
        sp.key(Key::Down, Mods::default());
        for _ in 0..120 {
            sp.update(1.0 / 120.0);
        }
        let r = sp.window_rect();
        let mut spm = pixmap_for(r);
        sp.draw(&mut spm, &sc.ctx);
        blit(&mut screen, &spm, r);
    }
    screen.save_png(format!("{out}/{name}.png")).unwrap();
}

fn crop_dock(sc: &Scene, out: &str, name: &str, dock: &Dock) {
    let dr = dock.window_rect();
    let mut bg = Pixmap::new(dr.w as u32, dr.h as u32).unwrap();
    bg.draw_pixmap(-dr.x, -dr.y, sc.wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    let mut dpm = pixmap_for(dr);
    dock.draw(&mut dpm, &sc.ctx);
    bg.draw_pixmap(0, 0, dpm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    bg.save_png(format!("{out}/{name}.png")).unwrap();
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "out".into());
    std::fs::create_dir_all(&out).unwrap();

    let retina_light = scene(2880, 1800, 2.0, false);
    desktop(&retina_light, &out, "desktop-light", true, false, None);
    desktop(&retina_light, &out, "desktop-light-hover", false, false, Some(2.4));
    desktop(&retina_light, &out, "spotlight-light", false, true, None);
    let retina_dark = scene(2880, 1800, 2.0, true);
    desktop(&retina_dark, &out, "desktop-dark", true, false, Some(5.0));
    desktop(&retina_dark, &out, "spotlight-dark", false, true, None);
    let fhd = scene(1920, 1080, 1.0, false);
    desktop(&fhd, &out, "desktop-1080p", true, true, Some(3.5));

    // Magnification sweep and a launch bounce, cropped to the dock window.
    let sc = &retina_light;
    let mut dock = Dock::new(DockConfig::default());
    dock.set_screen(sc.w, sc.h, sc.scale);
    dock.set_entries(entries());
    let dr = dock.window_rect();
    for (i, fx) in [0.30f32, 0.42, 0.5, 0.62].iter().enumerate() {
        dock.mouse_move(dr.w as f32 * fx, dr.h as f32 - 40.0);
        settle_dock(&mut dock);
        crop_dock(sc, &out, &format!("dock-mag-{i}"), &dock);
    }
    dock.mouse_leave();
    settle_dock(&mut dock);
    dock.bounce("mail");
    for f in 0..4 {
        for _ in 0..(0.155 * 120.0) as usize {
            dock.update(1.0 / 120.0);
        }
        crop_dock(sc, &out, &format!("dock-bounce-{f}"), &dock);
    }

    // Frame cost: the dock while the cursor sweeps across it (worst case).
    let mut dpm = pixmap_for(dr);
    let frames = 240;
    let t0 = Instant::now();
    for f in 0..frames {
        let fx = 0.3 + 0.4 * (f as f32 / frames as f32);
        dock.mouse_move(dr.w as f32 * fx, dr.h as f32 - 40.0);
        dock.update(1.0 / 120.0);
        dock.draw(&mut dpm, &sc.ctx);
    }
    let dock_ms = t0.elapsed().as_secs_f64() * 1000.0 / frames as f64;

    let mut sp = Spotlight::new();
    sp.set_screen(sc.w, sc.h, sc.scale);
    sp.show();
    let r = sp.window_rect();
    let mut spm = pixmap_for(r);
    let t0 = Instant::now();
    let mut n = 0;
    while sp.animating() {
        sp.update(1.0 / 120.0);
        sp.draw(&mut spm, &sc.ctx);
        n += 1;
    }
    let spot_ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let rows: Vec<Row> = (0..8)
        .map(|i| Row { title: format!("Результат {i}"), subtitle: "C:\\Users\\me".into(), icon: None, glyph: Glyph::Document })
        .collect();
    sp.set_rows(rows);
    let t0 = Instant::now();
    let mut m = 0;
    for i in 0..60 {
        if i % 6 == 0 {
            sp.key(Key::Down, Mods::default());
        }
        sp.update(1.0 / 120.0);
        sp.draw(&mut spm, &sc.ctx);
        m += 1;
    }
    let list_ms = t0.elapsed().as_secs_f64() * 1000.0 / m as f64;
    println!("spotlight: {n} opening frames, list with 8 rows {list_ms:.2} ms");

    let mut bar = MenuBar::new();
    bar.set_screen(sc.w, sc.scale, &sc.ctx);
    let mut bpm = pixmap_for(bar.window_rect());
    let t0 = Instant::now();
    for _ in 0..60 {
        bar.draw(&mut bpm, &sc.ctx);
    }
    let bar_ms = t0.elapsed().as_secs_f64() * 1000.0 / 60.0;
    println!(
        "frame cost at 2x: dock {dock_ms:.2} ms ({}x{}), spotlight opening {spot_ms:.2} ms ({}x{}), menu bar {bar_ms:.2} ms",
        dr.w, dr.h, r.w, r.h
    );
}
