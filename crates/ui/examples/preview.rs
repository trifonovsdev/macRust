//! Renders the shell components over a desktop into PNGs and measures frame
//! times, so the look and the cost can be checked without Windows.
//!
//! cargo run --release -p macrust-ui --example preview -- out/

use std::rc::Rc;
use std::time::Instant;

use macrust_ui::dock::{Dock, DockConfig, DockEntry, Fallback, Section};
use macrust_ui::gfx::{self, Image, rgba};
use macrust_ui::menu::{MenuEntry, PopupMenu};
use macrust_ui::control::{ControlCenter, ControlState, NowPlayingView};
use macrust_ui::menubar::{BarItem, MenuBar, Status, TrayItem};
use macrust_ui::notify::{Banner, NoteView, NotificationCenter, Today};
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

/// A Windows-style round logo with transparent corners, as `mac_icon` gets it.
fn legacy_round() -> Rc<Image> {
    let mut pm = Pixmap::new(256, 256).unwrap();
    if let Some(c) = gfx::circle(128.0, 128.0, 120.0) {
        gfx::fill(&mut pm, &c, rgba(40, 160, 230, 1.0));
    }
    macrust_ui::glyphs::play(&mut pm, 78.0, 78.0, 100.0, rgba(255, 255, 255, 1.0));
    Rc::new(Image::with_shadow(gfx::mac_icon(&pm).unwrap()))
}

/// A classic Win32 glyph icon with transparency.
fn legacy_glyph() -> Rc<Image> {
    let mut pm = Pixmap::new(256, 256).unwrap();
    macrust_ui::glyphs::document(&mut pm, 0.0, 0.0, 256.0);
    Rc::new(Image::with_shadow(gfx::mac_icon(&pm).unwrap()))
}

fn tpl(img: &Rc<Image>, rgb: [u8; 3]) -> Option<Rc<Image>> {
    gfx::template(img.base(), rgb).map(|p| Rc::new(Image::new(p)))
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
        app("legacy", "Telegram", legacy_round(), true),
        app("exe", "Старая программа", legacy_glyph(), false),
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
            mic: true,
            camera: false,
        },
        &sc.ctx,
    );
    bar.update_contrast(&sc.ctx);
    let tray: Vec<TrayItem> = [
        fake_icon(rgba(255, 90, 90, 1.0), rgba(200, 20, 30, 1.0), 5),
        fake_icon(rgba(90, 190, 255, 1.0), rgba(30, 140, 230, 1.0), 4),
        fake_icon(rgba(130, 140, 255, 1.0), rgba(88, 101, 242, 1.0), 0),
    ]
    .iter()
    .enumerate()
    .map(|(i, ic)| TrayItem { id: i as u64 + 1, icon_dark: tpl(ic, [0, 0, 0]), icon_light: tpl(ic, [255, 255, 255]), stamp: 1 })
    .collect();
    bar.set_tray(tray, &sc.ctx);

    let mut dock = Dock::new(DockConfig::default());
    dock.set_screen(sc.w, sc.h, sc.scale);
    dock.set_entries(entries());
    let dr = dock.window_rect();
    if let Some(t) = mouse_tile {
        dock.mouse_move(slot_x(&dock, sc.scale, t), dr.h as f32 - 30.0 * sc.scale);
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
        menu.mouse_move(ox as f32 + 40.0, oy as f32 + (6.0 + 24.0 * 2.0 + 11.0 + 8.0) * sc.scale);
        let r = WinRect { x: (6.0 * sc.scale) as i32 - ox, y: bar.height_px() + (2.0 * sc.scale) as i32 - oy, w: mw, h: mh };
        let mut behind = Pixmap::new(mw as u32, mh as u32).unwrap();
        behind.draw_pixmap(-r.x, -r.y, screen.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        menu.set_backdrop(&behind);
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

fn notes() -> Vec<NoteView> {
    let tg = fake_icon(rgba(90, 190, 255, 1.0), rgba(30, 140, 230, 1.0), 4);
    let ds = fake_icon(rgba(130, 140, 255, 1.0), rgba(88, 101, 242, 1.0), 0);
    vec![
        NoteView {
            id: 3,
            app: "Telegram".into(),
            title: "Аня".into(),
            body: "Скинь, пожалуйста, презентацию к завтрашнему созвону, хочу пройтись по слайдам заранее".into(),
            time: "сейчас".into(),
            icon: Some(tg.clone()),
            image: Some(fake_icon(rgba(255, 190, 120, 1.0), rgba(240, 120, 60, 1.0), 5)),
        },
        NoteView {
            id: 2,
            app: "Discord".into(),
            title: "#general — macRust".into(),
            body: "Новая сборка уже в релизах".into(),
            time: "5 мин назад".into(),
            icon: Some(ds),
            image: None,
        },
        NoteView {
            id: 1,
            app: "Почта".into(),
            title: "Счёт за ноябрь".into(),
            body: "Ваш счёт сформирован и доступен в личном кабинете.".into(),
            time: "13:02".into(),
            icon: Some(fake_icon(rgba(130, 170, 255, 1.0), rgba(60, 90, 230, 1.0), 2)),
            image: None,
        },
    ]
}

/// Control Center, Notification Center and a banner over the desktop.
fn panels(sc: &Scene, out: &str, name: &str) {
    let s = sc.scale;
    let mut screen = sc.wall.clone();
    let mut bar = MenuBar::new();
    bar.set_screen(sc.w, s, &sc.ctx);
    bar.set_status(
        Status {
            app: "Telegram".into(),
            lang: "RU".into(),
            battery: Some((64, true)),
            wifi: Some(true),
            clock: Strings::RU.clock(5, 21, 10, 14, 32),
            mic: false,
            camera: true,
        },
        &sc.ctx,
    );
    bar.update_contrast(&sc.ctx);
    bar.set_open(Some(BarItem::Control));
    let br = bar.window_rect();
    let mut bpm = pixmap_for(br);
    bar.draw(&mut bpm, &sc.ctx);
    blit(&mut screen, &bpm, br);

    let state = ControlState {
        wifi: Some((true, Some("Keenetic-5G".into()))),
        bluetooth: Some(false),
        volume: Some(0.62),
        muted: false,
        brightness: Some(0.8),
        playing: Some(NowPlayingView {
            title: "Midnight City".into(),
            artist: "M83".into(),
            playing: true,
            art: Some(fake_icon(rgba(80, 60, 160, 1.0), rgba(220, 80, 160, 1.0), 0)),
        }),
        dark: sc.ctx.theme.dark,
    };
    let mut cc = ControlCenter::new(state, s);
    for _ in 0..120 {
        cc.update(1.0 / 120.0);
    }
    let (cw, ch) = cc.window_size();
    let m = (macrust_ui::panel::MARGIN * s) as i32;
    let r = WinRect { x: sc.w as i32 - cw + m - (8.0 * s) as i32, y: br.h + (4.0 * s) as i32 - m, w: cw, h: ch };
    let crop = |r: WinRect| {
        let mut behind = Pixmap::new(r.w as u32, r.h as u32).unwrap();
        behind.draw_pixmap(-r.x, -r.y, sc.wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        behind
    };
    cc.set_backdrop(&crop(r));
    let mut cpm = pixmap_for(r);
    cc.draw(&mut cpm, &sc.ctx);
    blit(&mut screen, &cpm, r);
    screen.save_png(format!("{out}/{name}-control.png")).unwrap();

    let mut screen = sc.wall.clone();
    bar.set_open(Some(BarItem::Clock));
    bar.draw(&mut bpm, &sc.ctx);
    blit(&mut screen, &bpm, br);
    let today = Today { year: 2026, month: 10, day: 21, weekday: 6 };
    let mut nc = NotificationCenter::new(notes(), today, s, sc.h - br.h as f32);
    for _ in 0..120 {
        nc.update(1.0 / 120.0);
    }
    let (nw, nh) = nc.window_size(&sc.ctx);
    let r = WinRect { x: sc.w as i32 - nw + m - (8.0 * s) as i32, y: br.h + (4.0 * s) as i32 - m, w: nw, h: nh };
    nc.set_backdrop(&crop(r));
    nc.mouse_move(&sc.ctx, (m + 100) as f32, (m as f32 + 40.0 * s) + 20.0 * s);
    let mut npm = pixmap_for(r);
    nc.draw(&mut npm, &sc.ctx);
    blit(&mut screen, &npm, r);
    screen.save_png(format!("{out}/{name}-notifications.png")).unwrap();

    let mut screen = sc.wall.clone();
    blit(&mut screen, &bpm, br);
    let mut banner = Banner::new(notes().remove(0), s);
    for _ in 0..120 {
        banner.update(1.0 / 120.0);
    }
    let (bw, bh) = banner.window_size(&sc.ctx);
    let r = WinRect { x: sc.w as i32 - bw + m - (8.0 * s) as i32, y: br.h + (4.0 * s) as i32 - m, w: bw, h: bh };
    banner.set_backdrop(&crop(r));
    let mut bnp = pixmap_for(r);
    banner.draw(&mut bnp, &sc.ctx);
    blit(&mut screen, &bnp, r);
    screen.save_png(format!("{out}/{name}-banner.png")).unwrap();
}

/// Windows-style icons before and after `mac_icon`, light and dark plates.
fn icon_sheet(out: &str) {
    let n = 256u32;
    let mut sources = Vec::new();
    // A round logo (Telegram, Chrome style).
    let mut round = Pixmap::new(n, n).unwrap();
    if let Some(c) = gfx::circle(128.0, 128.0, 112.0) {
        gfx::fill(&mut round, &c, rgba(40, 160, 230, 1.0));
    }
    macrust_ui::glyphs::play(&mut round, 78.0, 78.0, 100.0, rgba(255, 255, 255, 1.0));
    sources.push(round);
    // A bare glyph with transparency (classic Win32 exe icon).
    let mut glyph = Pixmap::new(n, n).unwrap();
    macrust_ui::glyphs::folder(&mut glyph, 20.0, 30.0, 216.0);
    sources.push(glyph);
    // A full square tile with small rounding (UWP / Win11 style).
    let mut tile = Pixmap::new(n, n).unwrap();
    if let Some(r) = gfx::squircle(8.0, 8.0, 240.0, 240.0, 30.0) {
        gfx::fill(&mut tile, &r, rgba(120, 80, 220, 1.0));
    }
    macrust_ui::glyphs::logo(&mut tile, 68.0, 68.0, 120.0, rgba(255, 255, 255, 1.0));
    sources.push(tile);
    let cell = 280;
    let mut sheet = Pixmap::new(cell * 3, cell * 3).unwrap();
    for (row, bg) in [rgba(236, 236, 240, 1.0), rgba(236, 236, 240, 1.0), rgba(30, 30, 34, 1.0)].iter().enumerate() {
        gfx::fill_rect(&mut sheet, 0.0, (row as u32 * cell) as f32, (cell * 3) as f32, cell as f32, *bg);
    }
    for (i, src) in sources.iter().enumerate() {
        let x = (i as u32 * cell + 12) as i32;
        sheet.draw_pixmap(x, 12, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        let m = gfx::mac_icon(src).unwrap();
        for row in [1, 2] {
            sheet.draw_pixmap(x, (row * cell + 12) as i32, m.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }
    sheet.save_png(format!("{out}/icons.png")).unwrap();
}

/// Window x of slot `t` (fractional) of `entries()` in a resting dock.
fn slot_x(dock: &Dock, scale: f32, t: f32) -> f32 {
    let v = DockConfig::default().icon * 0.805 * scale;
    let pitch = 1.31 * v;
    let n = entries().len() as f32;
    let x0 = dock.window_rect().w as f32 / 2.0 - (n * pitch + 0.63 * v) / 2.0;
    x0 + (t + 0.5) * pitch + if t >= n - 2.0 { 0.63 * v } else { 0.0 }
}

/// Vertical cyan stripes like the macOS 26 wallpaper in the dark reference
/// frame (pixel values sampled just above its dock).
fn stripes_wall(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    let cols: [[f32; 3]; 4] = [[40.0, 175.0, 226.0], [27.0, 156.0, 211.0], [0.0, 120.0, 176.0], [16.0, 117.0, 181.0]];
    let data = pm.data_mut();
    for x in 0..w as usize {
        // Stripes 20 px wide drifting through four tones across the screen.
        let band = (x / 20) % 4;
        let c = cols[band];
        let shade = 0.9 + 0.1 * ((x % 20) as f32 / 19.0);
        for y in 0..h as usize {
            let o = (y * w as usize + x) * 4;
            for k in 0..3 {
                data[o + k] = (c[k] * shade) as u8;
            }
            data[o + 3] = 255;
        }
    }
    pm
}

fn flat_wall(w: u32, h: u32, c: [u8; 3]) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    pm.fill(Color::from_rgba8(c[0], c[1], c[2], 255));
    pm
}

fn scene_on(wall: Pixmap, scale: f32, dark: bool) -> Scene {
    let mut ctx = Ctx::new(Text::bundled(), Theme { dark }, &Strings::RU);
    ctx.vibrancy = Some(Vibrancy::new(&wall));
    let (w, h) = (wall.width() as f32, wall.height() as f32);
    Scene { ctx, wall, w, h, scale }
}

/// Dock crops for comparison with the macOS 26 reference frames: at rest,
/// with a label, with a pressed icon, bouncing and with an item sliding in.
fn dock_studies(sc: &Scene, out: &str, name: &str) {
    let mut dock = Dock::new(DockConfig::default());
    dock.set_screen(sc.w, sc.h, sc.scale);
    dock.set_entries(entries());
    // The dock takes a live capture of what is behind it, as in the shell.
    let dr = dock.window_rect();
    let mut behind = Pixmap::new(dr.w as u32, dr.h as u32).unwrap();
    behind.draw_pixmap(-dr.x, -dr.y, sc.wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    dock.set_backdrop(&behind);
    crop_dock(sc, out, &format!("{name}-rest"), &dock);
    let y = dr.h as f32 - 30.0 * sc.scale;
    dock.mouse_move(slot_x(&dock, sc.scale, 2.0), y);
    settle_dock(&mut dock);
    crop_dock(sc, out, &format!("{name}-label"), &dock);
    dock.mouse_down(slot_x(&dock, sc.scale, 2.0), y);
    crop_dock(sc, out, &format!("{name}-pressed"), &dock);
    dock.mouse_move(slot_x(&dock, sc.scale, 9.0), y);
    settle_dock(&mut dock);
    crop_dock(sc, out, &format!("{name}-label-stack"), &dock);
    dock.mouse_leave();
    settle_dock(&mut dock);
    // One hop, sampled at 60 fps like the reference sheet.
    dock.bounce("mail");
    for f in 0..=42 {
        if f % 3 == 0 {
            crop_dock(sc, out, &format!("{name}-bounce-{f:02}"), &dock);
        }
        for _ in 0..2 {
            dock.update(1.0 / 120.0);
        }
    }
    settle_dock(&mut dock);
    // An item slides in, sampled at 30 fps like the reference sheet.
    let mut e = entries();
    let new = e.remove(3);
    dock.set_entries(e.clone());
    settle_dock(&mut dock);
    e.insert(3, new);
    dock.set_entries(e);
    for f in 0..=14 {
        crop_dock(sc, out, &format!("{name}-insert-{f:02}"), &dock);
        for _ in 0..4 {
            dock.update(1.0 / 120.0);
        }
    }
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

    icon_sheet(&out);
    dock_studies(&scene_on(stripes_wall(3840, 2160), 2.0, true), &out, "dock-dark-cyan");
    dock_studies(&scene_on(flat_wall(1920, 1080, [0, 0, 0]), 1.0, true), &out, "dock-dark-black");
    dock_studies(&scene_on(flat_wall(2560, 1440, [17, 83, 130]), 2.0, false), &out, "dock-light-blue");
    // Real wallpapers, e.g. reference frames with their dock painted out.
    for (var, dark, name) in [("PREVIEW_DOCK_BACKDROP", true, "dock-ref-dark"), ("PREVIEW_DOCK_BACKDROP_LIGHT", false, "dock-ref-light")] {
        if let Ok(path) = std::env::var(var)
            && let Ok(wall) = Pixmap::load_png(&path)
        {
            dock_studies(&scene_on(wall, 2.0, dark), &out, name);
        }
    }
    let retina_light = scene(2880, 1800, 2.0, false);
    desktop(&retina_light, &out, "desktop-light", true, false, None);
    desktop(&retina_light, &out, "desktop-light-hover", false, false, Some(2.4));
    desktop(&retina_light, &out, "spotlight-light", false, true, None);
    panels(&retina_light, &out, "light");
    let retina_dark = scene(2880, 1800, 2.0, true);
    desktop(&retina_dark, &out, "desktop-dark", true, false, Some(5.0));
    desktop(&retina_dark, &out, "spotlight-dark", false, true, None);
    panels(&retina_dark, &out, "dark");
    let fhd = scene(1920, 1080, 1.0, false);
    desktop(&fhd, &out, "desktop-1080p", true, true, Some(3.5));

    // Magnification sweep and a launch bounce, cropped to the dock window.
    let sc = &retina_light;
    let mut dock = Dock::new(DockConfig { magnify: true, ..DockConfig::default() });
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
    // Live glass: each change of what is behind the dock costs one backdrop.
    let mut behind = Pixmap::new(dr.w as u32, dr.h as u32).unwrap();
    behind.draw_pixmap(-dr.x, -dr.y, sc.wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    let t0 = Instant::now();
    for _ in 0..10 {
        let b = macrust_ui::glass::Backdrop::new(&behind);
        b.draw(&mut dpm_probe(&dr), macrust_ui::glass::Shape { x: 0.0, y: 0.0, w: 100.0, h: 50.0, r: 10.0 }, &macrust_ui::glass::GlassStyle::dock(true, sc.scale));
    }
    println!("live backdrop for the dock at 2x: {:.2} ms", t0.elapsed().as_secs_f64() * 100.0);
    let t0 = Instant::now();
    for _ in 0..10 {
        let _ = macrust_ui::wallpaper::Vibrancy::new(&behind);
    }
    println!("  of which blur levels: {:.2} ms", t0.elapsed().as_secs_f64() * 100.0);
}

fn dpm_probe(r: &WinRect) -> Pixmap {
    Pixmap::new(r.w as u32, r.h as u32).unwrap()
}
