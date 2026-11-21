//! Renders the app logo (README) and the icon sizes for the .exe.
//!
//! cargo run --release -p macrust-ui --example logo -- docs assets

use macrust_ui::gfx::{self, rgba};
use macrust_ui::glyphs;
use macrust_ui::tiny_skia::{FillRule, GradientStop, LinearGradient, Paint, Pixmap, Point, SpreadMode, Transform};

fn logo(size: u32) -> Pixmap {
    let n = size as f32;
    let mut pm = Pixmap::new(size, size).unwrap();
    // Small sizes get less margin, so the 16 px icon is not a speck.
    let inset = if size <= 32 { n * 0.03 } else { n * 0.09 };
    let side = n - 2.0 * inset;
    let tile = gfx::squircle(inset, inset, side, side, side * 0.225).unwrap();
    if size > 32 {
        gfx::ShadowTex::new(side * 0.225, n * 0.02).draw(&mut pm, inset, inset + n * 0.015, side, side, rgba(0, 0, 0, 0.3));
    }
    let paint = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
            Point::from_xy(inset, inset),
            Point::from_xy(inset + side * 0.6, inset + side),
            vec![
                GradientStop::new(0.0, rgba(96, 170, 255, 1.0)),
                GradientStop::new(0.55, rgba(120, 96, 235, 1.0)),
                GradientStop::new(1.0, rgba(236, 112, 150, 1.0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap(),
        ..Paint::default()
    };
    pm.fill_path(&tile, &paint, FillRule::Winding, Transform::identity(), None);
    let g = side * 0.56;
    glyphs::logo(&mut pm, n / 2.0 - g / 2.0, n / 2.0 - g / 2.0, g, rgba(255, 255, 255, 0.96));
    pm
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let docs = args.first().cloned().unwrap_or_else(|| "docs".into());
    let assets = args.get(1).cloned().unwrap_or_else(|| "assets".into());
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(&assets).unwrap();
    logo(512).save_png(format!("{docs}/logo.png")).unwrap();
    for s in [16, 24, 32, 48, 64, 128, 256] {
        logo(s).save_png(format!("{assets}/icon-{s}.png")).unwrap();
    }
}
