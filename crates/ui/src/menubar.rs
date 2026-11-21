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

pub const HEIGHT: f32 = 24.0;
const FONT: f32 = 13.0;

pub struct MenuBar {
    scale: f32,
    width: f32,
    status: Status,
    items: Vec<(BarItem, f32, f32)>,
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
        self.items.iter().find(|(i, _, _)| *i == item).map(|(_, a, b)| (*a, *b))
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
        let pad = 9.0 * s;
        let mut items = Vec::new();
        let mut x = 6.0 * s;
        items.push((BarItem::Logo, x, x + 34.0 * s));
        x += 34.0 * s;
        if !self.status.app.is_empty() {
            let w = t.measure(&self.status.app, fs, Weight::Bold) + 2.0 * pad;
            items.push((BarItem::App, x, x + w));
            x += w;
        }
        let w = t.measure(ctx.strings.window, fs, Weight::Regular) + 2.0 * pad;
        items.push((BarItem::Window, x, x + w));

        let mut r = self.width - 6.0 * s;
        let push_right = |item: BarItem, w: f32, items: &mut Vec<(BarItem, f32, f32)>, r: &mut f32| {
            items.push((item, *r - w, *r));
            *r -= w;
        };
        push_right(BarItem::Clock, t.measure(&self.status.clock, fs, Weight::Regular) + 2.0 * pad, &mut items, &mut r);
        push_right(BarItem::Control, 32.0 * s, &mut items, &mut r);
        if self.status.mic || self.status.camera {
            push_right(BarItem::Privacy, 16.0 * s, &mut items, &mut r);
        }
        push_right(BarItem::Search, 30.0 * s, &mut items, &mut r);
        if self.status.wifi.is_some() {
            push_right(BarItem::Wifi, 30.0 * s, &mut items, &mut r);
        }
        if let Some((pct, _)) = self.status.battery {
            let tw = t.measure(&format!("{pct}%"), fs, Weight::Regular);
            push_right(BarItem::Battery, tw + 34.0 * s + 2.0 * pad, &mut items, &mut r);
        }
        if !self.status.lang.is_empty() {
            push_right(BarItem::Lang, t.measure(&self.status.lang, fs, Weight::Medium) + 2.0 * pad, &mut items, &mut r);
        }
        // Tray icons fill leftwards but never run into the app menus.
        let menus_end = items.iter().filter(|(i, _, _)| i.is_menu()).map(|(_, _, b)| *b).fold(0.0, f32::max);
        for ti in &self.tray {
            if r - 28.0 * s < menus_end + 40.0 * s {
                break;
            }
            push_right(BarItem::Tray(ti.id), 28.0 * s, &mut items, &mut r);
        }
        self.items = items;
    }

    fn item_at(&self, x: f32) -> Option<(BarItem, f32)> {
        self.items.iter().find(|(_, a, b)| x >= *a && x < *b).map(|(i, a, _)| (*i, *a))
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
        // As in macOS 26 the bar has no material: the wallpaper shows through.
        // Alpha 1 keeps the whole strip clickable; a faint shade along the top
        // edge holds the contrast of light text on busy pictures.
        gfx::put_rect(pm, 0.0, 0.0, w, h, [0, 0, 0, 1]);
        let shade = if self.dark_text { 0.0 } else { 0.16 };
        if shade > 0.0 {
            let data = pm.data_mut();
            let (pw, rows) = (w as usize, h as usize);
            for y in 0..rows {
                let a = ((shade * (1.0 - y as f32 / rows as f32)) * 255.0) as u8;
                for px in data[y * pw * 4..(y + 1) * pw * 4].chunks_exact_mut(4) {
                    px[3] = px[3].max(a);
                }
            }
        }
        let fg = if self.dark_text { rgba(0, 0, 0, 0.86) } else { rgba(255, 255, 255, 0.96) };
        let lit_bg = if self.dark_text { rgba(0, 0, 0, 0.11) } else { rgba(255, 255, 255, 0.22) };
        let fs = FONT * s;
        let base = (h / 2.0 + ctx.text.cap_height(fs) / 2.0).round();
        let pad = 9.0 * s;
        for &(item, x0, x1) in &self.items {
            let lit = self.open == Some(item) || (self.pressed == Some(item) && self.hover == Some(item));
            if lit && let Some(p) = gfx::squircle(x0, 2.0 * s, x1 - x0, h - 4.0 * s, (h - 4.0 * s) / 2.0) {
                gfx::fill(pm, &p, lit_bg);
            }
            let cx = (x0 + x1) / 2.0;
            match item {
                BarItem::Logo => glyphs::logo(pm, (cx - 7.5 * s).round(), (h / 2.0 - 7.5 * s).round(), 15.0 * s, fg),
                BarItem::App => {
                    ctx.text.draw(pm, &self.status.app, x0 + pad, base, fs, Weight::Bold, fg, None);
                }
                BarItem::Window => {
                    ctx.text.draw(pm, ctx.strings.window, x0 + pad, base, fs, Weight::Regular, fg, None);
                }
                BarItem::Lang => {
                    ctx.text.draw(pm, &self.status.lang, x0 + pad, base, fs, Weight::Medium, fg, None);
                }
                BarItem::Battery => {
                    if let Some((pct, charging)) = self.status.battery {
                        let tw = ctx.text.draw(pm, &format!("{pct}%"), x0 + pad, base, fs, Weight::Regular, fg, None);
                        let (bw, bh) = (26.0 * s, 12.0 * s);
                        glyphs::battery(pm, (x0 + pad + tw + 6.0 * s).round(), (h / 2.0 - bh / 2.0).round(), bw, bh, fg, pct, charging);
                    }
                }
                BarItem::Wifi => {
                    let gs = 17.0 * s;
                    glyphs::wifi(pm, cx - gs / 2.0, h / 2.0 - gs * 0.5, gs, fg, self.status.wifi.unwrap_or(false));
                }
                BarItem::Search => {
                    let gs = 14.0 * s;
                    glyphs::magnifier(pm, cx - gs / 2.0, h / 2.0 - gs / 2.0, gs, fg);
                }
                BarItem::Control => {
                    let gs = 15.0 * s;
                    glyphs::switches(pm, (cx - gs / 2.0).round(), (h / 2.0 - gs / 2.0).round(), gs, fg);
                }
                BarItem::Privacy => {
                    // Orange: microphone; green: camera (it implies the microphone too).
                    let c = if self.status.camera { rgba(52, 199, 89, 1.0) } else { rgba(255, 149, 0, 1.0) };
                    if let Some(p) = gfx::circle(cx, h / 2.0, 3.5 * s) {
                        gfx::fill(pm, &p, c);
                    }
                }
                BarItem::Tray(id) => {
                    let item = self.tray.iter().find(|t| t.id == id);
                    let img = item.and_then(|t| if self.dark_text { t.icon_dark.as_ref() } else { t.icon_light.as_ref() });
                    if let Some(img) = img {
                        let size = (16.0 * s).round();
                        img.draw(pm, (cx - size / 2.0).round(), (h / 2.0 - size / 2.0).round(), size, 1.0, false);
                    }
                }
                BarItem::Clock => {
                    ctx.text.draw(pm, &self.status.clock, x0 + pad, base, fs, Weight::Regular, fg, None);
                }
            }
        }
    }
}

impl Default for MenuBar {
    fn default() -> Self {
        Self::new()
    }
}
