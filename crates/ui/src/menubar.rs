//! The menu bar along the top edge of the screen.

use std::rc::Rc;

use tiny_skia::Pixmap;

use crate::gfx::{self, Image, rgba};
use crate::text::Weight;
use crate::{Ctx, WinRect, glyphs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarItem {
    Logo,
    App,
    Window,
    Lang,
    Battery,
    Wifi,
    Search,
    /// Control Center.
    Control,
    /// Orange/green dot while an app records the microphone or camera.
    Privacy,
    /// A notification-area icon of another app, by tray id.
    Tray(u64),
    /// Opens the Notification Center.
    Clock,
}

impl BarItem {
    pub fn is_menu(self) -> bool {
        matches!(self, BarItem::Logo | BarItem::App | BarItem::Window)
    }
}

#[derive(Debug, PartialEq)]
pub enum BarEvent {
    /// Open the menu of `item`; `x` is its left edge and `bottom` the bar's bottom, window px.
    Open { item: BarItem, x: f32, bottom: f32 },
    CloseMenus,
    Click(BarItem),
    /// Right click on a tray icon; `x` is the icon's center, window px.
    Context { item: BarItem, x: f32 },
}

#[derive(Clone)]
pub struct TrayItem {
    pub id: u64,
    /// Extras (`extras::templates`) in a 16 pt box: dark glyph for light
    /// wallpapers, light glyph for dark ones.
    pub icon_dark: Option<Rc<Image>>,
    pub icon_light: Option<Rc<Image>>,
    /// Changes whenever the icon picture (or what it is drawn as) changes.
    pub stamp: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub app: String,
    pub lang: String,
    /// Charge percent and whether it is charging; `None` without a battery.
    pub battery: Option<(u8, bool)>,
    /// Connected to a network; `None` when unknown.
    pub wifi: Option<bool>,
    pub clock: String,
    pub mic: bool,
    pub camera: bool,
}

// Points, measured on Tahoe 26.0 (TAHOE-SPEC.md section 2).
pub const HEIGHT: f32 = 24.0;
const FONT: f32 = 13.0;
const LOGO_X: f32 = 20.0;
const LOGO: f32 = 14.0;
/// Half the text-edge to text-edge gap between menu titles (21.5 pt).
const TITLE_PAD: f32 = 10.75;
/// The open-item capsule: content plus this per side, this tall.
const CAPSULE_PAD: f32 = 10.0;
const CAPSULE_H: f32 = 19.5;
/// Glyph-to-glyph gap of the status items and tray icons.
const STATUS_GAP: f32 = 20.0;
const CLOCK_RIGHT: f32 = 18.5;
const DATE_TIME_GAP: f32 = 8.5;
/// The recording dot sits right of the Control Center glyph.
const DOT: f32 = 4.5;
const DOT_GAP: f32 = 5.0;
const DOT_CLOCK_GAP: f32 = 17.5;
/// The box of a tray extra; its ink sets the slot width.
const TRAY: f32 = 16.0;
/// Status glyphs and the input badge sit this far below the text's cap
/// centre (BB +1, MM +0.5-1, NL +0.4-0.6 pt).
const GLYPH_DROP: f32 = 0.75;
const BADGE_H: f32 = 15.5;
const BADGE_PAD: f32 = 3.5;
const BADGE_FONT: f32 = 11.0;
// Search: glyph box and the share of it the ink covers (left offset, width),
// 12 pt wide as in Tahoe. Wi-Fi and Control Center draw their ink exactly:
// 16 and 12.5 pt wide (NL, BB).
const SEARCH: (f32, f32, f32) = (14.0, 0.12, 0.78);
const WIFI_W: f32 = 16.0;
const SWITCHES_W: f32 = 12.5;

/// One laid-out item: hit area `a..b` and the drawn content `c0..c1`, window px.
#[derive(Clone, Copy, Debug)]
struct Slot {
    item: BarItem,
    a: f32,
    b: f32,
    c0: f32,
    c1: f32,
}

/// The input-source badge text: Tahoe shows «РУ» for Russian.
fn badge_text(code: &str) -> &str {
    if code == "RU" { "РУ" } else { code }
}

pub struct MenuBar {
    scale: f32,
    width: f32,
    status: Status,
    items: Vec<Slot>,
    hover: Option<BarItem>,
    pressed: Option<BarItem>,
    open: Option<BarItem>,
    dark_text: bool,
    tray: Vec<TrayItem>,
    pub events: Vec<BarEvent>,
}

impl MenuBar {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            width: 1920.0,
            status: Status::default(),
            items: Vec::new(),
            hover: None,
            pressed: None,
            open: None,
            dark_text: true,
            tray: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn set_screen(&mut self, w: f32, scale: f32, ctx: &Ctx) {
        self.width = w;
        self.scale = scale;
        self.relayout(ctx);
    }

    pub fn height_px(&self) -> i32 {
        (HEIGHT * self.scale).round() as i32
    }

    pub fn window_rect(&self) -> WinRect {
        WinRect { x: 0, y: 0, w: self.width as i32, h: self.height_px() }
    }

    /// Returns true when something visible changed.
    pub fn set_status(&mut self, st: Status, ctx: &Ctx) -> bool {
        if st == self.status {
            return false;
        }
        self.status = st;
        self.relayout(ctx);
        true
    }

    /// Returns true when the icons changed.
    pub fn set_tray(&mut self, tray: Vec<TrayItem>, ctx: &Ctx) -> bool {
        let same = tray.len() == self.tray.len()
            && tray.iter().zip(&self.tray).all(|(a, b)| a.id == b.id && a.stamp == b.stamp);
        if same {
            return false;
        }
        self.tray = tray;
        self.relayout(ctx);
        true
    }

    /// Screen-independent rect (window px) of a tray icon, for the tray host.
    pub fn item_rect(&self, item: BarItem) -> Option<(f32, f32)> {
        self.items.iter().find(|sl| sl.item == item).map(|sl| (sl.a, sl.b))
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Picks dark or light text from the wallpaper behind the bar.
    pub fn update_contrast(&mut self, ctx: &Ctx) {
        self.dark_text = match &ctx.vibrancy {
            Some(v) => v.luma(0.0, 0.0, self.width, HEIGHT * self.scale) > 0.52,
            None => !ctx.theme.dark,
        };
    }

    pub fn set_open(&mut self, item: Option<BarItem>) {
        self.open = item;
    }

    pub fn open_item(&self) -> Option<BarItem> {
        self.open
    }

    fn relayout(&mut self, ctx: &Ctx) {
        let s = self.scale;
        let fs = FONT * s;
        let t = &ctx.text;
        let mut items: Vec<Slot> = Vec::new();
        // Left: logo and menu titles, text edge to text edge 21.5 pt.
        let lx = (LOGO_X * s).round();
        let lw = (LOGO * s).round();
        items.push(Slot { item: BarItem::Logo, a: 0.0, b: lx + lw + TITLE_PAD * s, c0: lx, c1: lx + lw });
        let mut x = lx + lw + 2.0 * TITLE_PAD * s;
        let mut title = |item: BarItem, w: f32, items: &mut Vec<Slot>| {
            items.push(Slot { item, a: x - TITLE_PAD * s, b: x + w + TITLE_PAD * s, c0: x, c1: x + w });
            x += w + 2.0 * TITLE_PAD * s;
        };
        if !self.status.app.is_empty() {
            title(BarItem::App, t.measure(&self.status.app, fs, Weight::Bold), &mut items);
        }
        title(BarItem::Window, t.measure(ctx.strings.window, fs, Weight::Regular), &mut items);

        // Right: the clock's text ends 18.5 pt from the edge; every other
        // item is its ink width with 20 pt between neighbours.
        let (date, time) = split_clock(&self.status.clock);
        let mut cw = t.measure(date, fs, Weight::Regular);
        if let Some(time) = time {
            cw += DATE_TIME_GAP * s + t.measure(time, fs, Weight::Regular);
        }
        let right = self.width - CLOCK_RIGHT * s;
        let half = STATUS_GAP * s / 2.0;
        items.push(Slot { item: BarItem::Clock, a: right - cw - half, b: self.width, c0: right - cw, c1: right });
        let mut r = right - cw - if self.status.mic || self.status.camera { DOT_CLOCK_GAP * s + DOT * s + DOT_GAP * s } else { STATUS_GAP * s };
        let push = |item: BarItem, w: f32, items: &mut Vec<Slot>, r: &mut f32| {
            // Whole-pixel left edges keep 1x glyphs and tray icons crisp.
            let c0 = (*r - w).round();
            let b = items.last().map_or(*r + half, |l| l.a);
            items.push(Slot { item, a: c0 - half, b, c0, c1: c0 + w });
            *r = c0 - STATUS_GAP * s;
        };
        push(BarItem::Control, SWITCHES_W * s, &mut items, &mut r);
        if self.status.mic || self.status.camera {
            // Control Center was pushed right after the clock; the dot widens it.
            let n = items.len();
            let (head, cc) = items.split_at_mut(n - 1);
            cc[0].c1 += (DOT_GAP + DOT) * s;
            cc[0].b = cc[0].c1 + DOT_CLOCK_GAP * s / 2.0;
            head[n - 2].a = cc[0].b;
        }
        push(BarItem::Search, SEARCH.0 * SEARCH.2 * s, &mut items, &mut r);
        if self.status.wifi.is_some() {
            push(BarItem::Wifi, WIFI_W * s, &mut items, &mut r);
        }
        if !self.status.lang.is_empty() {
            let w = t.measure(badge_text(&self.status.lang), BADGE_FONT * s, Weight::Medium) + 2.0 * BADGE_PAD * s;
            push(BarItem::Lang, w.round(), &mut items, &mut r);
        }
        if let Some((pct, _)) = self.status.battery {
            let tw = t.measure(&format!("{pct}%"), fs, Weight::Regular);
            push(BarItem::Battery, tw + 32.0 * s, &mut items, &mut r);
        }
        // Tray icons fill leftwards but never run into the app menus.
        let menus_end = items.iter().filter(|sl| sl.item.is_menu()).map(|sl| sl.b).fold(0.0, f32::max);
        for ti in &self.tray {
            let (i0, i1) = ti.icon_light.as_deref().map_or((0.0, 1.0), ink_span);
            let w = ((i1 - i0) * TRAY * s).round().max(1.0);
            if r - w < menus_end + 40.0 * s {
                break;
            }
            push(BarItem::Tray(ti.id), w, &mut items, &mut r);
        }
        self.items = items;
    }

    fn item_at(&self, x: f32) -> Option<(BarItem, f32)> {
        // Menus drop from the left end of their capsule.
        self.items.iter().find(|sl| x >= sl.a && x < sl.b).map(|sl| (sl.item, sl.c0 - CAPSULE_PAD * self.scale))
    }

    fn bottom(&self) -> f32 {
        self.height_px() as f32
    }

    pub fn mouse_move(&mut self, x: f32, _y: f32) {
        let hit = self.item_at(x);
        self.hover = hit.map(|h| h.0);
        // While a menu is open, sliding across titles switches menus.
        if let (Some(open), Some((item, ix))) = (self.open, hit)
            && item.is_menu()
            && item != open
        {
            self.open = Some(item);
            self.events.push(BarEvent::Open { item, x: ix, bottom: self.bottom() });
        }
    }

    /// The tray icon under the pointer, if any.
    pub fn hovered_tray(&self) -> Option<u64> {
        match self.hover {
            Some(BarItem::Tray(id)) => Some(id),
            _ => None,
        }
    }

    pub fn mouse_leave(&mut self) {
        self.hover = None;
        self.pressed = None;
    }

    pub fn mouse_down(&mut self, x: f32, _y: f32) {
        let Some((item, ix)) = self.item_at(x) else {
            if self.open.is_some() {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            }
            return;
        };
        if item.is_menu() {
            if self.open == Some(item) {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            } else {
                self.open = Some(item);
                self.events.push(BarEvent::Open { item, x: ix, bottom: self.bottom() });
            }
        } else {
            self.pressed = Some(item);
        }
    }

    pub fn right_click(&mut self, x: f32) {
        if let Some((item @ BarItem::Tray(_), a)) = self.item_at(x) {
            let b = self.item_rect(item).map(|r| r.1).unwrap_or(a);
            self.events.push(BarEvent::Context { item, x: (a + b) / 2.0 });
        }
    }

    pub fn mouse_up(&mut self, x: f32, _y: f32) {
        if let Some(p) = self.pressed.take()
            && self.item_at(x).map(|h| h.0) == Some(p)
        {
            if self.open.is_some() {
                self.open = None;
                self.events.push(BarEvent::CloseMenus);
            }
            self.events.push(BarEvent::Click(p));
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let s = self.scale;
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        // As in macOS 26 the bar has no material at all; alpha 1 only keeps
        // the whole strip clickable.
        gfx::put_rect(pm, 0.0, 0.0, w, h, [0, 0, 0, 1]);
        let fg = if self.dark_text { rgba(0, 0, 0, 0.86) } else { rgba(255, 255, 255, 0.96) };
        // A white wash over the wallpaper, as the measured capsule (25,125,175) -> (117,200,243).
        let lit_bg = if self.dark_text { rgba(0, 0, 0, 0.11) } else { rgba(255, 255, 255, 0.4) };
        let fs = FONT * s;
        let base = (h / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
        let mid = base - ctx.text.cap_height(fs) / 2.0 + GLYPH_DROP * s;
        for sl in &self.items {
            let (item, x0, x1) = (sl.item, sl.c0, sl.c1);
            let lit = self.open == Some(item) || (self.pressed == Some(item) && self.hover == Some(item));
            let ch = (CAPSULE_H * s).round();
            let cy = ((h - ch) / 2.0).round();
            let pad = CAPSULE_PAD * s;
            if lit && let Some(p) = gfx::squircle(x0 - pad, cy, x1 - x0 + 2.0 * pad, ch, ch / 2.0) {
                gfx::fill(pm, &p, lit_bg);
            }
            // Top-left of a glyph box whose ink starts at x0, centred vertically.
            let gl = |g: (f32, f32, f32)| ((x0 - g.0 * g.1 * s).round(), (mid - g.0 * s / 2.0).round(), g.0 * s);
            match item {
                BarItem::Logo => glyphs::logo(pm, x0, (mid - (x1 - x0) / 2.0).round(), x1 - x0, fg),
                BarItem::App => {
                    ctx.text.draw(pm, &self.status.app, x0, base, fs, Weight::Bold, fg, None);
                }
                BarItem::Window => {
                    ctx.text.draw(pm, ctx.strings.window, x0, base, fs, Weight::Regular, fg, None);
                }
                BarItem::Lang => self.draw_badge(pm, ctx, x0, x1, mid, fg),
                BarItem::Battery => {
                    if let Some((pct, charging)) = self.status.battery {
                        let tw = ctx.text.draw(pm, &format!("{pct}%"), x0, base, fs, Weight::Regular, fg, None);
                        let (bw, bh) = (26.0 * s, 12.0 * s);
                        glyphs::battery(pm, (x0 + tw + 6.0 * s).round(), (mid - bh / 2.0).round(), bw, bh, fg, pct, charging);
                    }
                }
                BarItem::Wifi => {
                    let w = WIFI_W * s;
                    glyphs::wifi(pm, x0, (mid - w * 0.37).round(), w, fg, self.status.wifi.unwrap_or(false));
                }
                BarItem::Search => {
                    let (gx, gy, gs) = gl(SEARCH);
                    glyphs::magnifier(pm, gx, gy, gs, fg);
                }
                BarItem::Control => {
                    let w = SWITCHES_W * s;
                    glyphs::switches(pm, x0, (mid - w * 0.48).round(), w, fg);
                    if self.status.mic || self.status.camera {
                        // Orange: microphone; green: camera (it implies the microphone too).
                        let c = if self.status.camera { rgba(52, 199, 89, 1.0) } else { rgba(255, 149, 0, 1.0) };
                        if let Some(p) = gfx::circle(x1 - DOT * s / 2.0, mid, DOT * s / 2.0) {
                            gfx::fill(pm, &p, c);
                        }
                    }
                }
                BarItem::Privacy => {}
                BarItem::Tray(id) => {
                    let item = self.tray.iter().find(|t| t.id == id);
                    let img = item.and_then(|t| if self.dark_text { t.icon_dark.as_ref() } else { t.icon_light.as_ref() });
                    if let Some(img) = img {
                        let size = TRAY * s;
                        let x = (x0 - ink_span(img).0 * size).round();
                        img.draw(pm, x, (mid - size / 2.0).round(), size, fg.alpha(), false);
                    }
                }
                BarItem::Clock => {
                    let (date, time) = split_clock(&self.status.clock);
                    let dw = ctx.text.draw(pm, date, x0, base, fs, Weight::Regular, fg, None);
                    if let Some(time) = time {
                        ctx.text.draw(pm, time, x0 + dw + DATE_TIME_GAP * s, base, fs, Weight::Regular, fg, None);
                    }
                }
            }
        }
    }

    /// The input source as Tahoe draws it: a rounded badge in the text
    /// colour with the letters cut out of it.
    fn draw_badge(&self, pm: &mut Pixmap, ctx: &Ctx, x0: f32, x1: f32, mid: f32, fg: tiny_skia::Color) {
        let s = self.scale;
        let (bw, bh) = ((x1 - x0).round() as u32, (BADGE_H * s).round() as u32);
        let (Some(mut badge), Some(mut letters)) = (Pixmap::new(bw, bh), Pixmap::new(bw, bh)) else { return };
        if let Some(p) = gfx::squircle(0.0, 0.0, bw as f32, bh as f32, 4.5 * s) {
            gfx::fill(&mut badge, &p, fg);
        }
        let text = badge_text(&self.status.lang);
        let fs = BADGE_FONT * s;
        let tw = ctx.text.measure(text, fs, Weight::Medium);
        let base = (bh as f32 / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
        ctx.text.draw(&mut letters, text, ((bw as f32 - tw) / 2.0).round(), base, fs, Weight::Medium, rgba(0, 0, 0, 1.0), None);
        for (b, l) in badge.data_mut().chunks_exact_mut(4).zip(letters.data().chunks_exact(4)) {
            let keep = 255 - l[3] as u32;
            for v in b.iter_mut() {
                *v = (*v as u32 * keep / 255) as u8;
            }
        }
        let y = (mid - bh as f32 / 2.0).round() as i32;
        pm.draw_pixmap(x0.round() as i32, y, badge.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    }
}

/// Left and right ends of an image's ink, as fractions of its width.
fn ink_span(img: &Image) -> (f32, f32) {
    let pm = img.base();
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let d = pm.data();
    let inked = |x: usize| (0..h).any(|y| d[(y * w + x) * 4 + 3] > 12);
    match ((0..w).find(|&x| inked(x)), (0..w).rev().find(|&x| inked(x))) {
        (Some(a), Some(b)) => (a as f32 / w as f32, (b + 1) as f32 / w as f32),
        _ => (0.0, 1.0),
    }
}

/// The clock string carries date and time separated by two spaces; Tahoe
/// puts a fixed gap there: 10 pt of clear space from «.» to «1» (NL).
fn split_clock(clock: &str) -> (&str, Option<&str>) {
    match clock.split_once("  ") {
        Some((d, t)) => (d, Some(t)),
        None => (clock, None),
    }
}

impl Default for MenuBar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::Strings;
    use crate::text::Text;
    use crate::theme::Theme;

    fn bar(scale: f32) -> (MenuBar, Ctx) {
        let ctx = Ctx::new(Text::bundled(), Theme { dark: true }, &Strings::RU);
        let mut bar = MenuBar::new();
        bar.set_screen(1920.0 * scale, scale, &ctx);
        bar.set_status(
            Status { app: "Проводник".into(), lang: "RU".into(), wifi: Some(true), clock: Strings::RU.clock(6, 21, 10, 14, 36), ..Status::default() },
            &ctx,
        );
        (bar, ctx)
    }

    fn slot(bar: &MenuBar, item: BarItem) -> Slot {
        *bar.items.iter().find(|s| s.item == item).unwrap()
    }

    #[test]
    fn layout_follows_tahoe_spacing() {
        for scale in [1.0, 2.0] {
            let (bar, _) = bar(scale);
            let logo = slot(&bar, BarItem::Logo);
            assert_eq!(logo.c0, 20.0 * scale);
            let app = slot(&bar, BarItem::App);
            let win = slot(&bar, BarItem::Window);
            assert!((app.c0 - logo.c1 - 21.5 * scale).abs() < 0.01);
            assert!((win.c0 - app.c1 - 21.5 * scale).abs() < 0.01);
            let clock = slot(&bar, BarItem::Clock);
            assert!((1920.0 * scale - clock.c1 - 18.5 * scale).abs() < 0.01);
            let (cc, search, wifi, lang) =
                (slot(&bar, BarItem::Control), slot(&bar, BarItem::Search), slot(&bar, BarItem::Wifi), slot(&bar, BarItem::Lang));
            for (l, r) in [(search, cc), (wifi, search), (lang, wifi)] {
                assert!(((r.c0 - l.c1) - STATUS_GAP * scale).abs() <= 1.0, "{:?} -> {:?}", l.item, r.item);
                // Hit areas tile the bar without holes.
                assert_eq!(l.b, r.a);
            }
            assert_eq!(badge_text("RU"), "РУ");
            assert_eq!(badge_text("EN"), "EN");
        }
    }

    #[test]
    fn menus_drop_from_the_capsule_edge() {
        let (mut bar, _) = bar(1.0);
        let app = slot(&bar, BarItem::App);
        bar.mouse_down(app.c0 + 2.0, 10.0);
        assert_eq!(bar.events, vec![BarEvent::Open { item: BarItem::App, x: app.c0 - CAPSULE_PAD, bottom: 24.0 }]);
    }

    #[test]
    fn tray_slots_hug_the_ink_and_sit_on_the_glyph_line() {
        let (mut bar, ctx) = bar(1.0);
        // A 64 px template whose ink covers columns 16..40: 6 pt of a 16 pt box.
        let mut pm = Pixmap::new(64, 64).unwrap();
        gfx::fill_rect(&mut pm, 16.0, 8.0, 24.0, 48.0, rgba(255, 255, 255, 1.0));
        let img = Rc::new(Image::new(pm));
        bar.set_tray(vec![TrayItem { id: 7, icon_dark: Some(img.clone()), icon_light: Some(img), stamp: 1 }], &ctx);
        let t = slot(&bar, BarItem::Tray(7));
        assert_eq!(t.c1 - t.c0, 6.0);
        let lang = slot(&bar, BarItem::Lang);
        assert!(((lang.c0 - t.c1) - STATUS_GAP).abs() <= 1.0);
        let mut pm = Pixmap::new(1920, 24).unwrap();
        bar.draw(&mut pm, &ctx);
        let inked = |x: u32, y: u32| pm.pixel(x, y).unwrap().alpha() > 128;
        let x = (t.c0 + 3.0) as u32;
        let rows: Vec<u32> = (0..24).filter(|&y| inked(x, y)).collect();
        // 12 pt of ink centred 0.75 pt under the cap centre (12.3 at 1x).
        let mid = (rows[0] + rows[rows.len() - 1] + 1) as f32 / 2.0;
        assert!((mid - 13.0).abs() <= 0.5, "{rows:?}");
        assert!(inked(t.c0 as u32, 12) && !inked(t.c0 as u32 - 1, 12));
    }
}
