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
    /// Template versions: dark glyph for light wallpapers, light glyph for dark ones.
    pub icon_dark: Option<Rc<Image>>,
    pub icon_light: Option<Rc<Image>>,
    /// Changes whenever the icon picture changes.
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
const DATE_TIME_GAP: f32 = 10.5;
/// The recording dot sits right of the Control Center glyph.
const DOT: f32 = 4.5;
const DOT_GAP: f32 = 5.0;
const DOT_CLOCK_GAP: f32 = 17.5;
const TRAY: f32 = 16.0;
const BADGE_H: f32 = 15.5;
const BADGE_PAD: f32 = 3.5;
const BADGE_FONT: f32 = 11.0;
// Glyph boxes and the share of the box their ink covers (left offset, width),
// sized to Tahoe: search 12 pt wide, Wi-Fi 15.5 pt, Control Center 13 pt.
const SEARCH: (f32, f32, f32) = (14.0, 0.12, 0.78);
const WIFI: (f32, f32, f32) = (15.0, 0.03, 0.94);
const SWITCHES: (f32, f32, f32) = (13.5, 0.03, 0.94);

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
        let cc = SWITCHES.0 * SWITCHES.2 * s;
        push(BarItem::Control, cc, &mut items, &mut r);
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
            push(BarItem::Wifi, WIFI.0 * WIFI.2 * s, &mut items, &mut r);
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
            if r - TRAY * s < menus_end + 40.0 * s {
                break;
            }
            push(BarItem::Tray(ti.id), (TRAY * s).round(), &mut items, &mut r);
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
        let mid = h / 2.0;
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
                BarItem::Lang => self.draw_badge(pm, ctx, x0, x1, fg),
                BarItem::Battery => {
                    if let Some((pct, charging)) = self.status.battery {
                        let tw = ctx.text.draw(pm, &format!("{pct}%"), x0, base, fs, Weight::Regular, fg, None);
                        let (bw, bh) = (26.0 * s, 12.0 * s);
                        glyphs::battery(pm, (x0 + tw + 6.0 * s).round(), (mid - bh / 2.0).round(), bw, bh, fg, pct, charging);
                    }
                }
                BarItem::Wifi => {
                    let (gx, _, gs) = gl(WIFI);
                    // The fan's ink sits low in its box; centre the ink, not the box.
                    glyphs::wifi(pm, gx, (mid - gs * 0.47).round(), gs, fg, self.status.wifi.unwrap_or(false));
                }
                BarItem::Search => {
                    let (gx, gy, gs) = gl(SEARCH);
                    glyphs::magnifier(pm, gx, gy, gs, fg);
                }
                BarItem::Control => {
                    let (gx, gy, gs) = gl(SWITCHES);
                    glyphs::switches(pm, gx, gy, gs, fg);
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
                        let size = x1 - x0;
                        img.draw(pm, x0, ((h - size) / 2.0).round(), size, fg.alpha(), false);
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
    fn draw_badge(&self, pm: &mut Pixmap, ctx: &Ctx, x0: f32, x1: f32, fg: tiny_skia::Color) {
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
        let y = ((pm.height() - bh) / 2) as i32;
        pm.draw_pixmap(x0.round() as i32, y, badge.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    }
}

/// The clock string carries date and time separated by two spaces; Tahoe
/// puts a wider, fixed 10.5 pt gap there.
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

    fn icon(f: impl Fn(u32, u32) -> [u8; 4]) -> Pixmap {
        let mut pm = Pixmap::new(16, 16).unwrap();
        for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
            let c = f(i as u32 % 16, i as u32 / 16);
            let a = c[3] as u32;
            px.copy_from_slice(&[(c[0] as u32 * a / 255) as u8, (c[1] as u32 * a / 255) as u8, (c[2] as u32 * a / 255) as u8, c[3]]);
        }
        pm
    }

    fn alpha_at(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.data()[((y * 16 + x) * 4 + 3) as usize]
    }

    #[test]
    fn templates_are_full_strength_silhouettes() {
        let inside = |x: u32, y: u32| (4..12).contains(&x) && (2..14).contains(&y);
        // A flat yellow glyph keeps its whole shape.
        let flat = icon(|x, y| if inside(x, y) { [240, 190, 40, 255] } else { [0; 4] });
        let t = gfx::template(&flat, [255, 255, 255]).unwrap();
        assert_eq!(alpha_at(&t, 8, 8), 255);
        // A faint grey glyph is brought up to full opacity.
        let faint = icon(|x, y| if inside(x, y) { [200, 200, 200, 140] } else { [0; 4] });
        let t = gfx::template(&faint, [255, 255, 255]).unwrap();
        assert_eq!(alpha_at(&t, 8, 8), 255);
        // A white mark on a blue disc becomes a hole.
        let logo = icon(|x, y| if (6..10).contains(&x) && (6..10).contains(&y) { [255, 255, 255, 255] } else { [40, 160, 230, 255] });
        let t = gfx::template(&logo, [255, 255, 255]).unwrap();
        assert_eq!(alpha_at(&t, 7, 7), 0);
        assert_eq!(alpha_at(&t, 1, 1), 255);
    }
}
