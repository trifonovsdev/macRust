//! Rendering of the file manager window. Content is drawn first and the
//! toolbar, headers and bars on top, so nothing scrolled needs clipping.

use tiny_skia::{Color, Pixmap};

use super::art::{self, Dir, PlaceIcon};
use super::format;
use super::layout::{self, Columns, Frame, Grid, Rect, View};
use super::model::{self, Column, Entry};
use super::view::{Browser, Hit, IconSlot, Load};
use crate::Ctx;
use crate::gfx::{self, Image, fade, rgba};
use crate::text::{Text, Weight};

struct Pal {
    dark: bool,
    base: Color,
    content: Color,
    panel: Color,
    panel_edge: Color,
    stripe: Color,
    label: Color,
    secondary: Color,
    tertiary: Color,
    separator: Color,
    accent: Color,
    sel: Color,
    tile: Color,
    control: Color,
    control_edge: Color,
    control_on: Color,
    hover: Color,
    field: Color,
    text_sel: Color,
}

impl Pal {
    fn new(ctx: &Ctx, active: bool) -> Self {
        let t = &ctx.theme;
        let d = t.dark;
        let pick = |l: Color, dk: Color| if d { dk } else { l };
        let sel = pick(rgba(0, 99, 225, 1.0), rgba(0, 88, 208, 1.0));
        let sel_dim = pick(rgba(220, 220, 222, 1.0), rgba(70, 70, 74, 1.0));
        Self {
            dark: d,
            base: pick(rgba(240, 240, 241, 1.0), rgba(38, 38, 40, 1.0)),
            content: pick(rgba(255, 255, 255, 1.0), rgba(30, 30, 32, 1.0)),
            panel: pick(rgba(249, 249, 250, 1.0), rgba(48, 48, 51, 1.0)),
            panel_edge: pick(rgba(0, 0, 0, 0.07), rgba(255, 255, 255, 0.08)),
            stripe: pick(rgba(0, 0, 0, 0.04), rgba(255, 255, 255, 0.035)),
            label: t.label(),
            secondary: t.secondary(),
            tertiary: t.tertiary(),
            separator: t.separator(),
            accent: t.accent(),
            sel: if active { sel } else { sel_dim },
            tile: pick(rgba(0, 0, 0, 0.08), rgba(255, 255, 255, 0.12)),
            control: pick(rgba(255, 255, 255, 1.0), rgba(255, 255, 255, 0.16)),
            control_edge: pick(rgba(0, 0, 0, 0.08), rgba(255, 255, 255, 0.05)),
            control_on: pick(rgba(0, 0, 0, 0.09), rgba(255, 255, 255, 0.16)),
            hover: pick(rgba(0, 0, 0, 0.05), rgba(255, 255, 255, 0.07)),
            field: pick(rgba(255, 255, 255, 1.0), rgba(30, 30, 32, 1.0)),
            text_sel: pick(rgba(179, 215, 255, 1.0), rgba(63, 99, 139, 1.0)),
        }
    }

    /// Text on a selection: white when the window is active, else the label color.
    fn on_sel(&self, active: bool) -> Color {
        if active { rgba(255, 255, 255, 1.0) } else { self.label }
    }
}

fn rounded(pm: &mut Pixmap, r: Rect, radius: f32, c: Color) {
    if let Some(p) = gfx::squircle(r.x, r.y, r.w, r.h, radius) {
        gfx::fill(pm, &p, c);
    }
}

fn rounded_stroke(pm: &mut Pixmap, r: Rect, radius: f32, c: Color, w: f32) {
    if let Some(p) = gfx::squircle(r.x + w / 2.0, r.y + w / 2.0, r.w - w, r.h - w, radius) {
        gfx::stroke(pm, &p, c, w);
    }
}

fn snap(r: Rect) -> Rect {
    let x = r.x.round();
    let y = r.y.round();
    Rect::new(x, y, (r.x + r.w).round() - x, (r.y + r.h).round() - y)
}

/// Baseline that centers capital letters on `cy`.
fn baseline(text: &Text, cy: f32, size: f32) -> f32 {
    (cy + text.cap_height(size) / 2.0).round()
}

/// Cuts the middle of `s` so it fits, keeping its end (and so the
/// extension) visible, as Finder does: "Apple-iPhon…-250909.zip".
pub(super) fn ellipsize_middle(text: &Text, s: &str, size: f32, w: Weight, max_w: f32) -> String {
    if text.measure(s, size, w) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let ell = text.measure("…", size, w);
    let budget = max_w - ell;
    if budget <= 0.0 {
        return "…".into();
    }
    let widths: Vec<f32> = chars.iter().map(|c| text.measure(c.encode_utf8(&mut [0; 4]), size, w)).collect();
    // Keep a little more of the head than of the tail.
    let (mut head, mut tail) = (0usize, 0usize);
    let (mut hw, mut tw) = (0.0f32, 0.0f32);
    loop {
        let take_head = hw <= tw * 1.2;
        let progressed = if take_head && head + tail < chars.len() && hw + tw + widths[head] <= budget {
            hw += widths[head];
            head += 1;
            true
        } else if head + tail < chars.len() && hw + tw + widths[chars.len() - 1 - tail] <= budget {
            tw += widths[chars.len() - 1 - tail];
            tail += 1;
            true
        } else if head + tail < chars.len() && hw + tw + widths[head] <= budget {
            hw += widths[head];
            head += 1;
            true
        } else {
            false
        };
        if !progressed {
            break;
        }
    }
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Up to two lines for an icon label: a long name breaks by characters
/// (preferring a break after a space, dash, underscore or dot) and the
/// second line is cut in the middle.
pub(super) fn label_lines(text: &Text, s: &str, size: f32, max_w: f32) -> Vec<String> {
    let w = Weight::Regular;
    if text.measure(s, size, w) <= max_w {
        return vec![s.to_string()];
    }
    let chars: Vec<char> = s.chars().collect();
    let mut x = 0.0;
    let mut fit = 0;
    for (i, c) in chars.iter().enumerate() {
        x += text.measure(c.encode_utf8(&mut [0; 4]), size, w);
        if x > max_w {
            break;
        }
        fit = i + 1;
    }
    let fit = fit.max(1);
    let soft = (fit * 2 / 5..fit).rev().find(|&i| matches!(chars[i], ' ' | '-' | '_' | '.')).map(|i| i + 1);
    let cut = soft.unwrap_or(fit);
    let first: String = chars[..cut].iter().collect::<String>().trim_end().to_string();
    let rest: String = chars[cut..].iter().collect::<String>().trim_start().to_string();
    let second = ellipsize_middle(text, &rest, size, w, max_w);
    vec![first, second]
}

impl Browser {
    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let f = self.frame();
        let p = Pal::new(ctx, self.active);
        pm.fill(p.base);
        gfx::fill_rect(pm, f.toolbar.x, 0.0, f.toolbar.w, f.h, p.content);
        self.draw_content(pm, ctx, &f, &p);
        self.draw_toolbar(pm, ctx, &f, &p);
        if self.view == View::List {
            self.draw_header(pm, ctx, &f, &p);
        } else if self.scroll_y() > 0.0 {
            // Content scrolled under the toolbar gets an edge, as in Tahoe.
            gfx::fill_rect(pm, f.content.x, f.toolbar.bottom(), f.content.w, f.s.max(1.0).round(), p.separator);
        }
        self.draw_bottom(pm, ctx, &f, &p);
        self.draw_sidebar(pm, ctx, &f, &p);
        self.draw_lights(pm, &f, &p);
    }

    fn icon_image(&self, e: &Entry) -> Option<&Image> {
        if e.dir {
            return Some(&self.art.folder);
        }
        let (key, size, thumb) = self.icon_key(e)?;
        match self.icons.get(&key) {
            Some(IconSlot::Ready(img)) => Some(img),
            _ if thumb => {
                // Until the thumbnail is in, show the type icon if we have it.
                let ext_key = format!("e{size}:{}", e.ext);
                match self.icons.get(&ext_key) {
                    Some(IconSlot::Ready(img)) => Some(img),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn draw_icon(&self, pm: &mut Pixmap, e: &Entry, x: f32, y: f32, size: f32, dark: bool) {
        match self.icon_image(e) {
            Some(img) => {
                // Thumbnails are not square: fit the larger side, center the other.
                let (iw, ih) = (img.base().width() as f32, img.base().height() as f32);
                let k = size / iw.max(ih);
                let (dw, dh) = (iw * k, ih * k);
                img.draw(pm, (x + (size - dw) / 2.0).round(), (y + (size - dh) / 2.0).round(), size.round(), 1.0, false);
            }
            None => {
                let doc = if dark { &self.art.document_dark } else { &self.art.document };
                doc.draw(pm, x.round(), y.round(), size.round(), 1.0, false);
            }
        }
    }

    // -- content ----------------------------------------------------------

    fn draw_content(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal) {
        let c = f.content;
        let s = f.s;
        let clip = Some((c.x as i32, c.y as i32, c.right() as i32, c.bottom() as i32));
        match self.view {
            View::List => self.draw_list(pm, ctx, f, p, clip),
            View::Icons => self.draw_grid(pm, ctx, f, p, clip),
        }
        if self.drop_hover == Some(None) {
            let w = (2.0 * s).round();
            rounded_stroke(pm, snap(Rect::new(c.x + 2.0 * s, c.y + 2.0 * s, c.w - 4.0 * s, c.h - 4.0 * s)), 6.0 * s, p.accent, w);
        }
        if let Some(b) = self.band() {
            let sy = self.scroll_y();
            let r = Rect::new(c.x + b.x, c.y + b.y - sy, b.w, b.h);
            let top = r.y.max(c.y);
            let bottom = r.bottom().min(c.bottom());
            if bottom > top {
                let r = snap(Rect::new(r.x, top, r.w, bottom - top));
                let (fill, edge) = if p.dark {
                    (rgba(255, 255, 255, 0.12), rgba(255, 255, 255, 0.4))
                } else {
                    (rgba(0, 0, 0, 0.08), rgba(0, 0, 0, 0.3))
                };
                gfx::fill_rect(pm, r.x, r.y, r.w, r.h, fill);
                let t = s.max(1.0).round();
                gfx::fill_rect(pm, r.x, r.y, r.w, t, edge);
                gfx::fill_rect(pm, r.x, r.bottom() - t, r.w, t, edge);
                gfx::fill_rect(pm, r.x, r.y, t, r.h, edge);
                gfx::fill_rect(pm, r.right() - t, r.y, t, r.h, edge);
            }
        }
        if let Some(t) = self.scrollbar() {
            let a = self.scrollbar_alpha();
            if a > 0.0 {
                let col = if p.dark { rgba(255, 255, 255, 0.45 * a) } else { rgba(0, 0, 0, 0.42 * a) };
                rounded(pm, t, t.w / 2.0, col);
            }
        }
        let msg = match &self.load {
            Load::Failed(code) => Some(match *code {
                5 => self.strings.access_denied,
                2 | 3 | 123 | 161 | 267 => self.strings.not_found,
                21 => self.strings.not_ready,
                _ => self.strings.cannot_open,
            }),
            Load::Done if self.list.is_empty() && !self.list.filter().is_empty() => Some(self.strings.no_matches),
            Load::Done if self.list.is_empty() => Some(self.strings.empty),
            _ => None,
        };
        if let Some(m) = msg {
            let fs = 13.0 * s;
            let mw = ctx.text.measure(m, fs, Weight::Regular);
            let x = (c.x + (c.w - mw) / 2.0).round();
            ctx.text.draw(pm, m, x, baseline(&ctx.text, c.y + c.h * 0.42, fs), fs, Weight::Regular, p.tertiary, clip);
        }
    }

    fn draw_list(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal, clip: Option<(i32, i32, i32, i32)>) {
        let c = f.content;
        let s = f.s;
        let sy = self.scroll_y();
        let rh = layout::ROW_H * s;
        let n = self.list.len();
        let cols = Columns::new(c.w, s);
        // Stripes fill the whole view, past the last row too, as in Finder.
        let first = ((sy - layout::LIST_PAD * s) / rh).floor().max(0.0) as usize;
        let last = ((sy + c.h - layout::LIST_PAD * s) / rh).ceil().max(0.0) as usize;
        for i in first..=last {
            if i % 2 == 1 {
                let y = c.y + layout::row_y(i, s) - sy;
                let y0 = y.max(c.y);
                let y1 = (y + rh).min(c.bottom());
                if y1 > y0 {
                    gfx::fill_rect(pm, c.x, y0.round(), c.w, (y1 - y0).round(), p.stripe);
                }
            }
        }
        let fs = layout::ROW_FONT * s;
        let renaming = self.renaming().map(|r| r.0.to_string());
        for pos in layout::row_range(sy, sy + c.h, n, s) {
            let Some(e) = self.list.entry(pos) else { continue };
            let y = (c.y + layout::row_y(pos, s) - sy).round();
            let selected = self.list.is_selected(pos);
            let row = Rect::new(c.x + layout::ROW_INSET * s, y, c.w - 2.0 * layout::ROW_INSET * s, rh.round());
            if selected {
                let r = 5.0 * s;
                rounded(pm, row, r, p.sel);
                // Neighbouring selected rows join into one block.
                if pos > 0 && self.list.is_selected(pos - 1) {
                    gfx::fill_rect(pm, row.x, row.y, row.w, (rh / 2.0).round(), p.sel);
                }
                if self.list.is_selected(pos + 1) {
                    gfx::fill_rect(pm, row.x, row.y + (rh / 2.0).round(), row.w, row.h - (rh / 2.0).round(), p.sel);
                }
            }
            if self.drop_hover == Some(Some(pos)) {
                rounded_stroke(pm, snap(row), 5.0 * s, p.accent, (2.0 * s).round());
            }
            let fg = if selected { p.on_sel(self.active) } else { p.label };
            let fg2 = if selected { fade(p.on_sel(self.active), 0.85) } else { p.secondary };
            let cy = y + rh / 2.0;
            let base = baseline(&ctx.text, cy, fs);
            for &(col, cx, cw) in &cols.cols {
                let x0 = c.x + cx;
                match col {
                    Column::Name => {
                        let ix = x0 + layout::COL_PAD * s;
                        let isz = (layout::LIST_ICON * s).round();
                        self.draw_icon(pm, e, ix, (cy - isz / 2.0).round(), isz, p.dark);
                        if renaming.as_deref() == Some(e.name.as_str()) {
                            continue;
                        }
                        let tx = (ix + isz + 6.0 * s).round();
                        let max = x0 + cw - tx - 6.0 * s;
                        let name = ellipsize_middle(&ctx.text, &e.name, fs, Weight::Regular, max);
                        ctx.text.draw(pm, &name, tx, base, fs, Weight::Regular, fg, clip);
                    }
                    Column::Modified => {
                        let d = format::date(e.modified, self.now, self.tz, self.strings.ru);
                        let d = ctx.text.ellipsize(&d, fs, Weight::Regular, cw - 2.0 * layout::COL_PAD * s);
                        ctx.text.draw(pm, &d, (x0 + layout::COL_PAD * s).round(), base, fs, Weight::Regular, fg2, clip);
                    }
                    Column::Size => {
                        let t = if e.dir { "--".to_string() } else { format::size(e.size, self.strings.ru) };
                        let tw = ctx.text.measure(&t, fs, Weight::Regular);
                        ctx.text.draw(pm, &t, (x0 + cw - layout::COL_PAD * s - tw).round(), base, fs, Weight::Regular, fg2, clip);
                    }
                    Column::Kind => {
                        let k = ctx.text.ellipsize(self.list.kinds.of(e), fs, Weight::Regular, cw - 2.0 * layout::COL_PAD * s);
                        ctx.text.draw(pm, &k, (x0 + layout::COL_PAD * 1.6 * s).round(), base, fs, Weight::Regular, fg2, clip);
                    }
                }
            }
        }
        self.draw_rename(pm, ctx, p, layout::ROW_FONT * s);
    }

    fn draw_grid(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal, clip: Option<(i32, i32, i32, i32)>) {
        let c = f.content;
        let s = f.s;
        let sy = self.scroll_y();
        let g = Grid::new(c.w, s);
        let fs = layout::LABEL_FONT * s;
        let line = layout::LABEL_LINE * s;
        let renaming = self.renaming().map(|r| r.0.to_string());
        for pos in g.range(sy, sy + c.h, self.list.len()) {
            let Some(e) = self.list.entry(pos) else { continue };
            let cell = g.cell(pos);
            let tile = g.tile(pos);
            let tile = Rect::new(c.x + tile.x, c.y + tile.y - sy, tile.w, tile.h);
            let selected = self.list.is_selected(pos);
            if selected {
                rounded(pm, tile, 6.0 * s, p.tile);
            }
            if self.drop_hover == Some(Some(pos)) {
                rounded_stroke(pm, tile, 6.0 * s, p.accent, (2.0 * s).round());
            }
            let isz = (layout::ICON * s).round();
            self.draw_icon(pm, e, tile.x + (tile.w - isz) / 2.0, tile.y + (tile.h - isz) / 2.0, isz, p.dark);
            if renaming.as_deref() == Some(e.name.as_str()) {
                continue;
            }
            let max_w = g.pitch_x - 8.0 * s;
            let lines = label_lines(&ctx.text, &e.name, fs, max_w);
            let mut ly = tile.bottom() + layout::LABEL_GAP * s;
            let cx = c.x + cell.x + cell.w / 2.0;
            for l in &lines {
                let lw = ctx.text.measure(l, fs, Weight::Regular);
                if selected {
                    let pad = 4.0 * s;
                    let r = snap(Rect::new(cx - lw / 2.0 - pad, ly, lw + 2.0 * pad, line));
                    rounded(pm, r, 4.0 * s, p.sel);
                }
                let fg = if selected { p.on_sel(self.active) } else { p.label };
                ctx.text.draw(pm, l, (cx - lw / 2.0).round(), baseline(&ctx.text, ly + line / 2.0, fs), fs, Weight::Regular, fg, clip);
                ly += line;
            }
        }
        self.draw_rename(pm, ctx, p, fs);
    }

    fn draw_rename(&self, pm: &mut Pixmap, ctx: &Ctx, p: &Pal, fs: f32) {
        let s = self.scale;
        let (Some((_, field)), Some(r)) = (self.renaming(), self.rename_box(Some(&ctx.text))) else { return };
        let r = snap(r);
        // Focus ring, then the white box.
        rounded(pm, Rect::new(r.x - 3.0 * s, r.y - 3.0 * s, r.w + 6.0 * s, r.h + 6.0 * s), 6.0 * s, fade(p.accent, 0.45));
        rounded(pm, r, 3.0 * s, p.field);
        self.draw_field_text(pm, ctx, p, field, r.x + 3.0 * s, r.y + r.h / 2.0, fs, r);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_field_text(&self, pm: &mut Pixmap, ctx: &Ctx, p: &Pal, field: &super::field::Field, x: f32, cy: f32, fs: f32, clip: Rect) {
        let s = self.scale;
        let text = field.text();
        let (a, b) = field.selection();
        let clip_i = Some((clip.x as i32, clip.y as i32, clip.right() as i32, clip.bottom() as i32));
        let lh = (fs * 1.25).round();
        if a != b {
            let x0 = x + ctx.text.measure(&field.prefix(a), fs, Weight::Regular);
            let x1 = x + ctx.text.measure(&field.prefix(b), fs, Weight::Regular);
            let r0 = x0.max(clip.x);
            let r1 = x1.min(clip.right());
            if r1 > r0 {
                gfx::fill_rect(pm, r0.round(), (cy - lh / 2.0).round(), (r1 - r0).round(), lh, p.text_sel);
            }
        }
        ctx.text.draw(pm, &text, x.round(), baseline(&ctx.text, cy, fs), fs, Weight::Regular, p.label, clip_i);
        if a == b && self.caret_on() {
            let cx = (x + ctx.text.measure(&field.prefix(field.caret), fs, Weight::Regular)).round();
            if cx < clip.right() {
                gfx::fill_rect(pm, cx, (cy - lh / 2.0).round(), (1.5 * s).round().max(1.0), lh, p.label);
            }
        }
    }

    // -- chrome -----------------------------------------------------------

    fn control(&self, pm: &mut Pixmap, ctx: &Ctx, r: Rect, p: &Pal) {
        let r = snap(r);
        let s = self.scale;
        if !p.dark {
            let sh = ctx.shadow(r.h / 2.0, 3.0 * s);
            sh.draw(pm, r.x, r.y + 1.0 * s, r.w, r.h, rgba(0, 0, 0, 0.1));
        }
        rounded(pm, r, r.h / 2.0, p.control);
        rounded_stroke(pm, r, r.h / 2.0, p.control_edge, s.max(1.0).round() * 0.75);
    }

    fn draw_toolbar(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal) {
        let s = f.s;
        gfx::fill_rect(pm, f.toolbar.x, f.toolbar.y, f.toolbar.w, f.toolbar.h, p.content);
        let pressed = |h: Hit| self.hover == h;
        let lw = (1.7 * s).max(1.2);

        // Back / forward.
        self.control(pm, ctx, f.nav, p);
        let half = f.nav.w / 2.0;
        for (i, (hit, enabled)) in [(Hit::Back, self.can_back()), (Hit::Forward, self.can_forward())].into_iter().enumerate() {
            let r = Rect::new(f.nav.x + i as f32 * half, f.nav.y, half, f.nav.h);
            if enabled && pressed(hit) {
                let inset = 3.0 * s;
                rounded(pm, snap(Rect::new(r.x + inset, r.y + inset, r.w - 2.0 * inset, r.h - 2.0 * inset)), (r.h - 2.0 * inset) / 2.0, p.hover);
            }
            let col = if enabled { p.label } else { p.tertiary };
            let dir = if i == 0 { Dir::Left } else { Dir::Right };
            art::chevron(pm, (r.x + r.w / 2.0).round(), f.nav.y + f.nav.h / 2.0, 13.0 * s, dir, col, lw);
        }

        // View switch.
        self.control(pm, ctx, f.view_seg, p);
        let seg = f.view_seg.w / 2.0;
        for (i, (hit, v)) in [(Hit::ViewIcons, View::Icons), (Hit::ViewList, View::List)].into_iter().enumerate() {
            let r = Rect::new(f.view_seg.x + i as f32 * seg, f.view_seg.y, seg, f.view_seg.h);
            let inset = 3.0 * s;
            let inner = snap(Rect::new(r.x + inset, r.y + inset, r.w - 2.0 * inset, r.h - 2.0 * inset));
            if self.view == v {
                rounded(pm, inner, inner.h / 2.0, p.control_on);
            } else if pressed(hit) {
                rounded(pm, inner, inner.h / 2.0, p.hover);
            }
            let (cx, cy) = ((r.x + r.w / 2.0).round(), (r.y + r.h / 2.0).round());
            if v == View::Icons {
                art::grid_glyph(pm, cx, cy, 15.0 * s, p.label, (1.3 * s).max(1.0));
            } else {
                art::list_glyph(pm, cx, cy, 15.0 * s, p.label, (1.4 * s).max(1.0));
            }
        }

        // New folder.
        self.control(pm, ctx, f.new_folder, p);
        if pressed(Hit::NewFolder) {
            let inset = 3.0 * s;
            let r = snap(Rect::new(f.new_folder.x + inset, f.new_folder.y + inset, f.new_folder.w - 2.0 * inset, f.new_folder.h - 2.0 * inset));
            rounded(pm, r, r.h / 2.0, p.hover);
        }
        art::new_folder_glyph(
            pm,
            (f.new_folder.x + f.new_folder.w / 2.0).round(),
            (f.new_folder.y + f.new_folder.h / 2.0).round(),
            18.0 * s,
            p.label,
            (1.4 * s).max(1.0),
        );

        // Search field.
        let r = snap(f.search);
        if self.search_focus {
            let k = 3.0 * s;
            rounded(pm, Rect::new(r.x - k, r.y - k, r.w + 2.0 * k, r.h + 2.0 * k), (r.h + 2.0 * k) / 2.0, fade(p.accent, 0.4));
        }
        self.control(pm, ctx, r, p);
        art::magnifier(pm, r.x + 17.0 * s, r.y + r.h / 2.0, 14.0 * s, p.secondary, (1.5 * s).max(1.0));
        let fs = layout::SEARCH_FONT * s;
        let tx = self.field_x(false).unwrap_or(r.x);
        let clip = Rect::new(tx - 2.0 * s, r.y, r.right() - tx - r.h * 0.8, r.h);
        if self.search.is_empty() && !self.search_focus {
            ctx.text.draw(pm, self.strings.search, tx.round(), baseline(&ctx.text, r.y + r.h / 2.0, fs), fs, Weight::Regular, p.tertiary, None);
        } else {
            self.draw_field_text(pm, ctx, p, &self.search, tx, r.y + r.h / 2.0, fs, clip);
        }
        if !self.search.is_empty() {
            let d = 14.0 * s;
            art::clear_button(pm, r.right() - r.h / 2.0, r.y + r.h / 2.0, d, p.tertiary, p.control_solid());
        }

        // Title: the folder's name, bold, between the buttons.
        let fs = layout::TITLE_FONT * s;
        let max = f.view_seg.x - f.title_x - 12.0 * s;
        if max > 20.0 * s {
            let t = ctx.text.ellipsize(self.title(), fs, Weight::Bold, max);
            ctx.text.draw(pm, &t, f.title_x.round(), baseline(&ctx.text, f.toolbar.h / 2.0, fs), fs, Weight::Bold, p.label, None);
        }
    }

    fn draw_header(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal) {
        let s = f.s;
        let h = f.header;
        gfx::fill_rect(pm, h.x, h.y, h.w, h.h, p.content);
        let t = s.max(1.0).round();
        gfx::fill_rect(pm, h.x, h.bottom() - t, h.w, t, p.separator);
        let fs = layout::HEADER_FONT * s;
        let cols = Columns::new(f.content.w, s);
        let sort = self.list.sort;
        let cy = h.y + h.h / 2.0;
        for (i, &(col, cx, cw)) in cols.cols.iter().enumerate() {
            let x0 = h.x + cx;
            let on = sort.column == col;
            let (weight, color) = if on { (Weight::Medium, p.label) } else { (Weight::Regular, p.secondary) };
            let label = self.strings.column(col);
            let lx = if col == Column::Name { x0 + (layout::COL_PAD + layout::LIST_ICON + 6.0) * s } else { x0 + layout::COL_PAD * s };
            let lw = ctx.text.measure(label, fs, weight);
            let lx = if col == Column::Size { x0 + cw - layout::COL_PAD * s - lw } else if col == Column::Kind { x0 + layout::COL_PAD * 1.6 * s } else { lx };
            let max = cw - 2.0 * layout::COL_PAD * s - 14.0 * s;
            let shown = ctx.text.ellipsize(label, fs, weight, max);
            ctx.text.draw(pm, &shown, lx.round(), baseline(&ctx.text, cy, fs), fs, weight, color, None);
            if on {
                let ax = if col == Column::Size { lx - 10.0 * s } else { x0 + cw - 14.0 * s };
                let dir = if sort.ascending { Dir::Up } else { Dir::Down };
                art::chevron(pm, ax.round(), cy, 8.0 * s, dir, p.secondary, (1.4 * s).max(1.0));
            }
            if i > 0 {
                gfx::fill_rect(pm, x0.round(), (h.y + 7.0 * s).round(), t, (h.h - 14.0 * s).round(), p.separator);
            }
        }
    }

    fn draw_bottom(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal) {
        let s = f.s;
        let t = s.max(1.0).round();
        let fs = layout::STATUS_FONT * s;
        let top = if f.pathbar.h > 0.0 { f.pathbar.y } else { f.status.y };
        gfx::fill_rect(pm, f.content.x, top, f.content.w, f.h - top, p.content);
        gfx::fill_rect(pm, f.content.x, top.round(), f.content.w, t, p.separator);
        if f.pathbar.h > 0.0 {
            let crumbs = self.crumbs();
            let boxes = self.crumb_boxes(&ctx.text);
            let cy = f.pathbar.y + f.pathbar.h / 2.0;
            let clip = Some((f.pathbar.x as i32, f.pathbar.y as i32, f.pathbar.right() as i32, f.pathbar.bottom() as i32));
            for (i, ((_, name), (bx, bw))) in crumbs.iter().zip(&boxes).enumerate() {
                let isz = (12.0 * s).round();
                if bx + bw < f.pathbar.x {
                    continue;
                }
                if i == 0 && model::parent(&crumbs[0].0).is_none() {
                    art::place(pm, PlaceIcon::Drive, bx.round(), (cy - isz / 2.0).round(), isz, p.secondary);
                } else if *bx >= f.pathbar.x {
                    self.art.folder.draw(pm, bx.round(), (cy - isz / 2.0).round(), isz, 1.0, false);
                }
                let col = if self.hover == Hit::Crumb(i) && i + 1 < crumbs.len() { p.label } else { p.secondary };
                let shown = ctx.text.ellipsize(name, fs, Weight::Regular, 220.0 * s);
                ctx.text.draw(pm, &shown, (bx + isz + 4.0 * s).round(), baseline(&ctx.text, cy, fs), fs, Weight::Regular, col, clip);
                if i + 1 < crumbs.len() {
                    let ax = bx + bw + 9.0 * s;
                    if ax > f.pathbar.x {
                        art::chevron(pm, ax.round(), cy, 7.0 * s, Dir::Right, p.tertiary, (1.3 * s).max(1.0));
                    }
                }
            }
            gfx::fill_rect(pm, f.content.x, f.status.y.round(), f.content.w, t, p.separator);
        }
        let text = match self.load {
            Load::Loading if self.list.is_empty() => self.strings.loading.to_string(),
            _ => format::status(self.list.len(), self.list.selected_count(), self.free, self.strings.ru),
        };
        let tw = ctx.text.measure(&text, fs, Weight::Regular);
        let x = f.status.x + (f.status.w - tw) / 2.0;
        ctx.text.draw(pm, &text, x.round(), baseline(&ctx.text, f.status.y + f.status.h / 2.0, fs), fs, Weight::Regular, p.secondary, None);
    }

    fn draw_sidebar(&self, pm: &mut Pixmap, ctx: &Ctx, f: &Frame, p: &Pal) {
        let s = f.s;
        let sb = snap(f.sidebar);
        let r = layout::SIDEBAR_R * s;
        let sh = ctx.shadow(r, 5.0 * s);
        sh.draw(pm, sb.x, sb.y + 1.0 * s, sb.w, sb.h, if p.dark { rgba(0, 0, 0, 0.35) } else { rgba(0, 0, 0, 0.07) });
        rounded(pm, sb, r, p.panel);
        rounded_stroke(pm, sb, r, p.panel_edge, s.max(1.0).round() * 0.75);

        let clip = Some((sb.x as i32, sb.y as i32, sb.right() as i32, sb.bottom() as i32));
        let hf = layout::SB_HEADER_FONT * s;
        for (si, cy) in self.section_headers().into_iter().enumerate() {
            let Some(sec) = self.sections.get(si) else { continue };
            if cy > sb.bottom() {
                break;
            }
            let col = if p.dark { rgba(235, 235, 245, 0.45) } else { rgba(60, 60, 67, 0.55) };
            ctx.text.draw(pm, sec.title, (sb.x + 17.0 * s).round(), baseline(&ctx.text, cy, hf), hf, Weight::Bold, col, clip);
        }
        let fs = layout::SB_FONT * s;
        for (si, pi, cy) in self.sidebar_rows() {
            if cy - layout::SB_ROW_H * s / 2.0 > sb.bottom() - 6.0 * s {
                break;
            }
            let place = &self.sections[si].places[pi];
            let current = !self.path.is_empty() && model::same_path(&place.path, &self.path);
            if current {
                let row = snap(Rect::new(
                    sb.x + layout::SB_SEL_L * s,
                    cy - layout::SB_SEL_H * s / 2.0,
                    sb.w - (layout::SB_SEL_L + layout::SB_SEL_R) * s,
                    layout::SB_SEL_H * s,
                ));
                rounded(pm, row, layout::SB_SEL_RADIUS * s, if p.dark { rgba(255, 255, 255, 0.1) } else { rgba(0, 0, 0, 0.065) });
            }
            let col = if current { p.accent } else { p.label };
            let icol = if current { p.accent } else { fade(p.label, 0.8) };
            let isz = (layout::SB_ICON * s).round();
            art::place(pm, place.icon, (sb.x + layout::SB_ICON_X * s).round(), (cy - isz / 2.0).round(), isz, icol);
            let tx = (sb.x + layout::SB_TEXT_X * s).round();
            let right = if place.eject { sb.right() - 34.0 * s } else { sb.right() - 12.0 * s };
            let name = ctx.text.ellipsize(&place.name, fs, Weight::Regular, right - tx);
            ctx.text.draw(pm, &name, tx, baseline(&ctx.text, cy, fs), fs, Weight::Regular, col, clip);
            if place.eject {
                let e = (12.0 * s).round();
                let ec = if self.hover == Hit::Eject(si, pi) { p.label } else { p.secondary };
                art::eject(pm, (sb.right() - 26.0 * s).round(), (cy - e / 2.0).round(), e, ec);
            }
        }
    }

    fn draw_lights(&self, pm: &mut Pixmap, f: &Frame, p: &Pal) {
        let colors = [rgba(255, 95, 87, 1.0), rgba(254, 188, 46, 1.0), rgba(40, 200, 64, 1.0)];
        let lit = self.active || self.lights_hover;
        for (i, c) in colors.into_iter().enumerate() {
            let r = f.light(i);
            let (cx, cy, rad) = (r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0);
            let fill = if lit { c } else if p.dark { rgba(255, 255, 255, 0.18) } else { rgba(0, 0, 0, 0.13) };
            if let Some(path) = gfx::circle(cx, cy, rad) {
                gfx::fill(pm, &path, fill);
                if lit {
                    gfx::stroke(pm, &path, rgba(0, 0, 0, 0.12), (0.5 * f.s).max(0.6));
                }
            }
            if self.lights_hover {
                art::light_glyph(pm, i, r.x, r.y, r.w, rgba(0, 0, 0, 0.55));
            }
        }
    }
}

impl Pal {
    /// The opaque version of the control fill, for marks cut out of glyphs.
    fn control_solid(&self) -> Color {
        if self.dark { rgba(70, 70, 74, 1.0) } else { rgba(255, 255, 255, 1.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_ellipsis_keeps_the_extension() {
        let t = Text::bundled();
        let s = ellipsize_middle(&t, "Apple-iPhone-17-color-lineup-250909.zip", 13.0, Weight::Regular, 150.0);
        assert!(s.contains('…'));
        assert!(s.ends_with(".zip"), "{s}");
        assert!(s.starts_with("Apple"));
        assert!(t.measure(&s, 13.0, Weight::Regular) <= 150.0);
        assert_eq!(ellipsize_middle(&t, "short.txt", 13.0, Weight::Regular, 150.0), "short.txt");
    }

    #[test]
    fn labels_break_in_two_lines() {
        let t = Text::bundled();
        let l = label_lines(&t, "Apple-iPhone-17-color-lineup-250909.zip", 12.0, 104.0);
        assert_eq!(l.len(), 2);
        assert!(l[0].ends_with('-'), "{l:?}");
        assert!(l[1].ends_with(".zip"));
        for line in &l {
            assert!(t.measure(line, 12.0, Weight::Regular) <= 104.0, "{line}");
        }
        assert_eq!(label_lines(&t, "Docs", 12.0, 104.0), ["Docs"]);
    }
}
