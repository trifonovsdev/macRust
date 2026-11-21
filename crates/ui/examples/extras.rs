//! Renders Windows tray icons as menu-bar extras and the right end of the bar
//! carrying them, for side-by-side checks against Tahoe without Windows.
//!
//! cargo run --release -p macrust-ui --example extras -- <src dir with set.txt> <out dir>
//!
//! `set.txt` lists one icon per line: `exe<TAB>tooltip<TAB>16 px png<TAB>24 px png`,
//! rightmost first, as the tray reports them.

use std::rc::Rc;

use macrust_ui::extras::Extra;
use macrust_ui::gfx::{Image, rgba};
use macrust_ui::menubar::{BarItem, MenuBar, Status, TrayItem};
use macrust_ui::strings::Strings;
use macrust_ui::text::Text;
use macrust_ui::theme::Theme;
use macrust_ui::tiny_skia::{Pixmap, PixmapPaint, Transform};
use macrust_ui::wallpaper::Vibrancy;
use macrust_ui::Ctx;

struct Icon {
    exe: String,
    tip: String,
    px16: Pixmap,
    px24: Pixmap,
}

fn load(dir: &str) -> Vec<Icon> {
    let list = std::fs::read_to_string(format!("{dir}/set.txt")).expect("set.txt");
    list.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let png = |n: &str| Pixmap::load_png(format!("{dir}/{n}")).expect("png");
            Icon { exe: f[0].into(), tip: f[1].into(), px16: png(f[2]), px24: png(f[3]) }
        })
        .collect()
}

fn flat(w: u32, h: u32, c: [u8; 3]) -> Pixmap {
    let mut pm = Pixmap::new(w, h).unwrap();
    pm.fill(rgba(c[0], c[1], c[2], 1.0));
    pm
}

fn blit(dst: &mut Pixmap, src: &Pixmap, x: i32, y: i32) {
    dst.draw_pixmap(x, y, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn zoom(src: &Pixmap, k: u32) -> Pixmap {
    let mut out = Pixmap::new(src.width() * k, src.height() * k).unwrap();
    let (w, d) = (src.width(), src.data());
    let o = out.data_mut();
    for y in 0..src.height() * k {
        for x in 0..w * k {
            let s = (((y / k) * w + x / k) * 4) as usize;
            let t = ((y * w * k + x) * 4) as usize;
            o[t..t + 4].copy_from_slice(&d[s..s + 4]);
        }
    }
    out
}

/// One row per icon: the source, the generic conversion and the final
/// extra, each at 1x (16 px) and 1.5x (24 px), on a dark and a light bar.
fn grid(icons: &[Icon], out: &str) {
    let cell = 30u32;
    let cols = 2 * 3 * 2;
    let mut sheet = flat(cols * cell, icons.len() as u32 * cell, [0, 0, 0]);
    for (row, ic) in icons.iter().enumerate() {
        for (bg_i, (bg, rgb)) in [([2u8, 10, 28], [255u8, 255, 255]), ([226, 230, 238], [0, 0, 0])].iter().enumerate() {
            for (k, (src, size)) in [(&ic.px16, 16.0f32), (&ic.px24, 24.0)].iter().enumerate() {
                let generic = Extra::convert(src).and_then(|e| e.paint(*rgb));
                let fin = Extra::new(src, &ic.exe, &ic.tip).and_then(|e| e.paint(*rgb));
                for (c, pic) in [Some((*src).clone()), generic, fin].into_iter().enumerate() {
                    let col = (bg_i * 6 + c * 2 + k) as u32;
                    let mut tile = flat(cell, cell, *bg);
                    if let Some(p) = pic {
                        let img = Image::new(p);
                        let o = (cell as f32 - size) / 2.0;
                        img.draw(&mut tile, o.round(), o.round(), *size, if bg_i == 0 { 0.96 } else { 0.86 }, false);
                    }
                    blit(&mut sheet, &tile, (col * cell) as i32, (row as u32 * cell) as i32);
                }
            }
        }
    }
    sheet.save_png(format!("{out}/extras_grid_1x.png")).unwrap();
    zoom(&sheet, 4).save_png(format!("{out}/extras_grid_x4.png")).unwrap();
}

fn bar(icons: &[Icon], wall: &Pixmap, scale: f32, dark: bool, open: Option<BarItem>) -> Pixmap {
    let mut ctx = Ctx::new(Text::bundled(), Theme { dark }, &Strings::RU);
    ctx.vibrancy = Some(Vibrancy::new(wall));
    let mut bar = MenuBar::new();
    bar.set_screen(wall.width() as f32, scale, &ctx);
    bar.set_status(
        Status { app: "Проводник".into(), lang: "RU".into(), wifi: Some(true), clock: Strings::RU.clock(6, 21, 10, 17, 45), ..Status::default() },
        &ctx,
    );
    bar.update_contrast(&ctx);
    let tray: Vec<TrayItem> = icons
        .iter()
        .enumerate()
        .map(|(i, ic)| {
            let src = if scale > 1.2 { &ic.px24 } else { &ic.px16 };
            let e = Extra::new(src, &ic.exe, &ic.tip);
            let img = |rgb| e.as_ref().and_then(|e| e.paint(rgb)).map(|p| Rc::new(Image::new(p)));
            TrayItem { id: i as u64 + 1, icon_dark: img([0, 0, 0]), icon_light: img([255, 255, 255]), stamp: 1 }
        })
        .collect();
    bar.set_tray(tray, &ctx);
    bar.set_open(open);
    let r = bar.window_rect();
    let mut pm = Pixmap::new(r.w as u32, r.h as u32).unwrap();
    bar.draw(&mut pm, &ctx);
    let mut screen = wall.clone();
    blit(&mut screen, &pm, 0, 0);
    let w = (760.0 * scale) as u32;
    let mut out = Pixmap::new(w, r.h as u32).unwrap();
    blit(&mut out, &screen, -(wall.width() as i32 - w as i32), 0);
    out
}

fn main() {
    let src = std::env::args().nth(1).expect("source dir");
    let out = std::env::args().nth(2).expect("out dir");
    std::fs::create_dir_all(&out).unwrap();
    let icons = load(&src);
    grid(&icons, &out);
    // The owner's black-navy desktop, a mid blue (MM), a light grey and a bright pink (BB).
    let walls: [(&str, [u8; 3], bool); 4] =
        [("navy", [2, 10, 28], true), ("blue", [30, 125, 172], true), ("light", [226, 230, 238], false), ("pink", [190, 120, 150], false)];
    for (name, c, dark) in walls {
        for (tag, scale) in [("1x", 1.0f32), ("1.5x", 1.5)] {
            let wall = flat((1920.0 * scale) as u32, (60.0 * scale) as u32, c);
            bar(&icons, &wall, scale, dark, None).save_png(format!("{out}/bar_{name}_{tag}.png")).unwrap();
        }
    }
    let wall = flat(1920, 60, [30, 125, 172]);
    bar(&icons, &wall, 1.0, true, Some(BarItem::Tray(5))).save_png(format!("{out}/bar_blue_open_1x.png")).unwrap();
}
