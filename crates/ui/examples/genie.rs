//! Renders the Genie warp for comparison with the measured macOS frames and
//! times it at 1920x1080.
//!
//! cargo run --release -p macrust-ui --example genie -- <ref dir> <out dir>
//!
//! `<ref dir>` holds `ref_minimize_uXX.png` (Ventura, 60 fps) and
//! `ref_minimize_tahoe_uXX.png` (Tahoe, 30 fps) from `out/ref/tahoe2/genie`.
//! Writes `ours_*` frames at the same moments, a macRust desktop sequence and
//! prints the cost per frame.

use std::rc::Rc;
use std::time::Instant;

use macrust_ui::Ctx;
use macrust_ui::dock::{Dock, DockConfig, DockEntry, Fallback, Section};
use macrust_ui::genie::{self, Dir, Dst, Path, Rect, Snapshot};
use macrust_ui::gfx::{self, Image, rgba};
use macrust_ui::strings::Strings;
use macrust_ui::text::Text;
use macrust_ui::theme::Theme;
use macrust_ui::tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, SpreadMode, Transform,
};
use macrust_ui::wallpaper::{self, Vibrancy};

fn crop(pm: &Pixmap, r: Rect) -> Pixmap {
    let mut out = Pixmap::new(r.w as u32, r.h as u32).unwrap();
    out.draw_pixmap(-(r.x as i32), -(r.y as i32), pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    out
}

fn snapshot(pm: &Pixmap, radius: f32) -> Snapshot {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut px = pm.data().to_vec();
    genie::round_corners(&mut px, w, h, radius);
    let mut s = Snapshot::new(px, w, h);
    s.build_levels();
    s
}

fn render(bg: &Pixmap, snap: &Snapshot, p: &Path, dir: Dir, t: f32) -> Pixmap {
    let mut out = bg.clone();
    let mut layer = vec![0u8; out.data().len()];
    let (w, h) = (out.width() as usize, out.height() as usize);
    let mut dst = Dst { px: &mut layer, w, h, x: 0, y: 0 };
    let f = genie::frame(p, dir, t);
    genie::draw(snap, p, &f, &mut dst);
    let layer_pm =
        Pixmap::from_vec(layer, macrust_ui::tiny_skia::IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap();
    out.draw_pixmap(0, 0, layer_pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    out
}

/// Our frames at the moments of the reference frames: same window, same
/// target, same background (the last reference frame, window gone).
/// The Ventura window at rest (MZ frame 15, before any bend), scaled to `w` x `h`.
fn window_art(refdir: &str, w: f32, h: f32) -> Pixmap {
    let first = Pixmap::load_png(format!("{refdir}/ref_minimize_u00.png")).unwrap();
    let art = crop(&first, Rect { x: 892.0, y: 84.0, w: 536.0, h: 713.0 });
    let mut out = Pixmap::new(w as u32, h as u32).unwrap();
    let paint = PixmapPaint { quality: macrust_ui::tiny_skia::FilterQuality::Bicubic, ..PixmapPaint::default() };
    out.draw_pixmap(0, 0, art.as_ref(), &paint, Transform::from_scale(w / 536.0, h / 713.0), None);
    out
}

/// The Tahoe clip's first frame is already bent, so its window is not usable
/// as a snapshot: the Ventura window, scaled to the Tahoe window's rect, is.
fn compare(refdir: &str, out: &str, prefix: &str, p: Path, radius: f32, frames: &[(u32, f32)]) {
    let load = |u: u32| Pixmap::load_png(format!("{refdir}/{prefix}_u{u:02}.png")).unwrap();
    let bg = load(100);
    let snap = snapshot(&window_art(refdir, p.win.w, p.win.h), radius);
    for &(u, t) in frames {
        render(&bg, &snap, &p, Dir::Minimize, t).save_png(format!("{out}/ours_{prefix}_u{u:02}.png")).unwrap();
    }
}

fn fake_icon(top: Color, bottom: Color) -> Rc<Image> {
    let n = 256.0;
    let mut pm = Pixmap::new(256, 256).unwrap();
    let inset = n * 0.1;
    let path = gfx::squircle(inset, inset, n - 2.0 * inset, n - 2.0 * inset, n * 0.18).unwrap();
    let shader = LinearGradient::new(
        Point::from_xy(0.0, inset),
        Point::from_xy(0.0, n - inset),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    pm.fill_path(
        &path,
        &Paint { anti_alias: true, shader, ..Paint::default() },
        macrust_ui::tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    Rc::new(Image::with_shadow(pm))
}

/// The warp on a macRust desktop: the window minimizes into the third Dock
/// icon and comes back out of it, the Dock drawn over it as on screen.
fn desktop(refdir: &str, out: &str) {
    let (w, h) = (1920u32, 1080u32);
    let wall = wallpaper::procedural(w, h, true);
    let mut ctx = Ctx::new(Text::bundled(), Theme { dark: true }, &Strings::EN);
    ctx.vibrancy = Some(Vibrancy::new(&wall));
    let mut dock = Dock::new(DockConfig::default());
    dock.set_screen(w as f32, h as f32, 1.0);
    let colors = [
        ((110, 200, 255), (20, 120, 240)),
        ((120, 230, 200), (10, 150, 140)),
        ((130, 170, 255), (60, 90, 230)),
        ((255, 120, 140), (230, 40, 80)),
        ((255, 230, 120), (240, 180, 30)),
        ((90, 90, 96), (30, 30, 34)),
    ];
    let entries = colors
        .iter()
        .enumerate()
        .map(|(i, (a, b))| DockEntry {
            key: format!("app{i}"),
            label: format!("App {i}"),
            icon: Some(fake_icon(rgba(a.0, a.1, a.2, 1.0), rgba(b.0, b.1, b.2, 1.0))),
            running: true,
            section: Section::Apps,
            fallback: Fallback::App,
        })
        .collect();
    dock.set_entries(entries);
    for _ in 0..240 {
        dock.update(1.0 / 120.0);
    }
    let dr = dock.window_rect();
    let mut behind = Pixmap::new(dr.w as u32, dr.h as u32).unwrap();
    behind.draw_pixmap(-dr.x, -dr.y, wall.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    dock.set_backdrop(&behind);
    let mut dpm = Pixmap::new(dr.w as u32, dr.h as u32).unwrap();
    dock.draw(&mut dpm, &ctx);

    // The Ventura reference window (a real app) as the snapshot.
    let snap = snapshot(&window_art(refdir, 536.0, 713.0), 8.0);
    let (ix, iy, iw, ih) = dock.icon_rect("app2").unwrap();
    let p = Path {
        win: Rect { x: 560.0, y: 140.0, w: 536.0, h: 713.0 },
        icon: Rect { x: ix, y: iy, w: iw, h: ih },
        clip: dock.bar_top(),
        screen_h: h as f32,
        dock_visible: true,
        scale: 1.0,
    };
    for (dir, name) in [(Dir::Minimize, "min"), (Dir::Restore, "restore")] {
        let n = (genie::duration(dir) * 60.0).ceil() as u32;
        for i in 0..=n {
            let mut f = render(&wall, &snap, &p, dir, i as f32 / 60.0);
            f.draw_pixmap(dr.x, dr.y, dpm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            f.save_png(format!("{out}/desk_{name}_f{i:02}.png")).unwrap();
        }
    }
}

/// Worst case on the owner's screen: a maximized 1920x1080 window.
fn bench() {
    let (w, h) = (1920usize, 1080usize);
    let win = Rect { x: 0.0, y: 24.0, w: 1920.0, h: 1080.0 - 24.0 - 78.0 };
    let (sw, sh) = (win.w as usize, win.h as usize);
    let px: Vec<u8> =
        (0..sw * sh).flat_map(|i| [(i % 251) as u8, (i / sw % 253) as u8, ((i * 7) % 256) as u8, 255]).collect();
    let t0 = Instant::now();
    let mut snap = Snapshot::new(px, sw, sh);
    let t1 = Instant::now();
    snap.build_levels();
    println!(
        "snapshot 1920x{sh}: {:.2} ms before the first frame, levels {:.2} ms after, {} MB",
        (t1 - t0).as_secs_f64() * 1000.0,
        t1.elapsed().as_secs_f64() * 1000.0,
        snap.bytes() >> 20
    );
    let p = Path {
        win,
        icon: Rect { x: 1100.0, y: 1015.0, w: 42.0, h: 42.0 },
        clip: 1002.0,
        screen_h: h as f32,
        dock_visible: true,
        scale: 1.0,
    };
    // Calibration: one full-screen copy on this machine.
    let a = vec![7u8; w * h * 4];
    let mut b = vec![0u8; w * h * 4];
    b.copy_from_slice(&a);
    let t0 = Instant::now();
    for _ in 0..10 {
        b.copy_from_slice(&a);
        std::hint::black_box(&mut b);
    }
    println!("memcpy 1920x1080: {:.2} ms", t0.elapsed().as_secs_f64() * 100.0);
    // Touched once, as the overlay's bitmap is before its first frame.
    let mut buf = vec![1u8; w * h * 4];
    buf.fill(0);
    for dir in [Dir::Minimize, Dir::Restore] {
        let mut dst = Dst { px: &mut buf, w, h, x: 0, y: 0 };
        let mut prev = genie::Bounds::default();
        let n = (genie::duration(dir) * 60.0).ceil() as u32;
        let (mut total, mut worst) = (0.0f64, 0.0f64);
        for i in 0..=n {
            let t0 = Instant::now();
            genie::clear(&mut dst, prev);
            let f = genie::frame(&p, dir, i as f32 / 60.0);
            prev = genie::draw(&snap, &p, &f, &mut dst);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if std::env::var_os("GENIE_FRAMES").is_some() {
                println!("  {dir:?} f{i:02} {ms:.2} ms");
            }
            total += ms;
            worst = worst.max(ms);
        }
        println!(
            "{dir:?} 1920x{sh}: {} frames, mean {:.2} ms, worst {:.2} ms (clear + warp)",
            n + 1,
            total / (n + 1) as f64,
            worst
        );
        // The same frames again, each the best of 7 runs: the cost without
        // this shared machine's scheduling noise.
        let mut best = Vec::new();
        for i in 0..=n {
            let f = genie::frame(&p, dir, i as f32 / 60.0);
            let mut b = f64::MAX;
            for _ in 0..7 {
                let t0 = Instant::now();
                genie::clear(&mut dst, prev);
                prev = genie::draw(&snap, &p, &f, &mut dst);
                b = b.min(t0.elapsed().as_secs_f64() * 1000.0);
            }
            best.push(b);
        }
        let worst = best.iter().cloned().fold(0.0, f64::max);
        if std::env::var_os("GENIE_FRAMES").is_some() {
            println!("  best {best:.1?}");
        }
        println!("  best-of-7: mean {:.2} ms, worst {:.2} ms", best.iter().sum::<f64>() / best.len() as f64, worst);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    bench();
    let (Some(refdir), Some(out)) = (args.first(), args.get(1)) else { return };
    std::fs::create_dir_all(out).unwrap();
    // Ventura MZ, 60 fps, Dock hidden: t = (frame - 14.57) / 60.
    let mz: Vec<(u32, f32)> = [15, 18, 21, 24, 27, 30, 32, 35, 38, 41, 44]
        .iter()
        .enumerate()
        .map(|(i, &f)| (i as u32 * 10, (f as f32 - 14.57) / 60.0))
        .collect();
    let mz_path = Path {
        win: Rect { x: 892.0, y: 84.0, w: 536.0, h: 713.0 },
        icon: Rect { x: 1320.3, y: 994.0, w: 52.0, h: 66.0 },
        clip: 1080.0,
        screen_h: 1080.0,
        dock_visible: false,
        scale: 1.0,
    };
    compare(refdir, out, "ref_minimize", mz_path, 10.0, &mz);
    // Tahoe YG, 30 fps, Dock visible: t = (frame - 8.19) / 30.
    let yg: Vec<(u32, f32)> = [10, 11, 13, 14, 16, 17, 18, 20, 21, 23, 24]
        .iter()
        .enumerate()
        .map(|(i, &f)| (i as u32 * 10, (f as f32 - 8.19) / 30.0))
        .collect();
    let yg_path = Path {
        win: Rect { x: 579.0, y: 178.0, w: 705.0, h: 683.0 },
        icon: Rect { x: 1397.0, y: 1012.0, w: 58.0, h: 58.0 },
        clip: 1008.0,
        screen_h: 1080.0,
        dock_visible: true,
        scale: 1.0,
    };
    compare(refdir, out, "ref_minimize_tahoe", yg_path, 18.0, &yg);
    desktop(refdir, out);
}
