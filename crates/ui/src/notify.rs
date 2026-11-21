//! Notification Center (notifications above a calendar widget, in a column
//! under the clock, with no panel behind) and the banners that announce new
//! notifications.

use std::rc::Rc;

use tiny_skia::Pixmap;

use crate::anim::Spring;
use crate::gfx::{self, Image, rgba};
use crate::glass::{self, Backdrop, GlassStyle, Rim, Shape};
use crate::panel::{self, MARGIN, Reveal, inside};
use crate::text::Weight;
use crate::{Ctx, glyphs};

#[derive(Clone)]
pub struct NoteView {
    pub id: i64,
    pub app: String,
    pub title: String,
    pub body: String,
    /// Already formatted: "now", "5 min ago", "14:32".
    pub time: String,
    pub icon: Option<Rc<Image>>,
    /// Sender picture (Telegram puts the avatar here); shown instead of the icon.
    pub image: Option<Rc<Image>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Today {
    pub year: i32,
    /// 0..11
    pub month: usize,
    pub day: u32,
    /// 0 = Sunday.
    pub weekday: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoteEvent {
    Open(i64),
    Dismiss(i64),
    ClearAll,
}

// Points. The column is a medium widget wide (TAHOE-SPEC.md section 5).
pub const WIDTH: f32 = 322.0;
const SMALL: f32 = 153.5;
const WIDGET_R: f32 = 19.5;
const WIDGET_GAP: f32 = 15.5;
const CARD_R: f32 = 18.0;
const CARD_PAD: f32 = 14.0;
const ICON: f32 = 34.0;
const GAP: f32 = 8.0;
const HEADER: f32 = 30.0;
const CAL_H: f32 = SMALL;
/// Calendar widget, pt (BB 26.0 at 2 px/pt, widget x 705-1012, y 86-393):
/// month cap 7.5 pt, weekday and date caps 7-7.25 pt.
const CAL_LEFT: f32 = 17.5;
const CAL_MONTH: f32 = 29.5;
const CAL_MONTH_TEXT: f32 = 10.3;
const CAL_WEEKDAYS: f32 = 46.5;
const CAL_TEXT: f32 = 10.0;
const CAL_COL: f32 = 18.46;
const CAL_DISC: f32 = 17.0;
const TITLE: f32 = 13.0;
const BODY: f32 = 13.0;
const LINE: f32 = 17.0;

fn body_lines(ctx: &Ctx, n: &NoteView, s: f32) -> Vec<String> {
    let w = (WIDTH - 2.0 * CARD_PAD - ICON - 10.0) * s;
    ctx.text.wrap(&n.body, BODY * s, Weight::Regular, w, 3)
}

fn card_h(ctx: &Ctx, n: &NoteView, s: f32) -> f32 {
    let lines = body_lines(ctx, n, s).len() as f32;
    ((CARD_PAD * 2.0 + 16.0 + lines * LINE) * s).max((CARD_PAD * 2.0 + ICON) * s)
}

/// A card's or widget's material: the measured widget glass over the captured
/// backdrop (or a plain fill without one) and the lit 1 pt rim. No outer ring
/// or shadow: none shows around Tahoe's widgets.
#[allow(clippy::too_many_arguments)]
fn plate(pm: &mut Pixmap, ctx: &Ctx, bd: Option<&Backdrop>, x: f32, y: f32, w: f32, h: f32, r: f32, s: f32) {
    let dark = ctx.theme.dark;
    let shape = Shape { x, y, w, h, r };
    match bd {
        Some(b) => b.draw(pm, shape, &GlassStyle::card(dark, s)),
        None => {
            let fill = if dark { rgba(17, 17, 18, 0.92) } else { rgba(250, 250, 252, 0.94) };
            glass::hairline(pm, shape, 0.0, 1e4, fill, fill);
        }
    }
    glass::lit_rim(pm, shape, Rim::widget(dark), s);
}

/// One notification card; `close` draws the dismiss button on its corner.
#[allow(clippy::too_many_arguments)]
fn draw_card(pm: &mut Pixmap, ctx: &Ctx, bd: Option<&Backdrop>, n: &NoteView, x: f32, y: f32, s: f32, close: bool, pressed: bool) -> f32 {
    let th = ctx.theme;
    let w = WIDTH * s;
    let h = card_h(ctx, n, s);
    plate(pm, ctx, bd, x, y, w, h, CARD_R * s, s);
    if pressed && let Some(p) = gfx::squircle(x, y, w, h, CARD_R * s) {
        gfx::fill(pm, &p, th.separator());
    }
    let (ix, iy, isz) = (x + CARD_PAD * s, y + CARD_PAD * s, ICON * s);
    match (&n.image, &n.icon) {
        (Some(img), icon) => {
            img.draw(pm, ix, iy, isz, 1.0, false);
            if let Some(icon) = icon {
                let b = 16.0 * s;
                icon.draw(pm, ix + isz - b * 0.75, iy + isz - b * 0.75, b, 1.0, false);
            }
        }
        (None, Some(icon)) => icon.draw(pm, ix, iy, isz, 1.0, false),
        (None, None) => glyphs::app_tile(pm, ix, iy, isz),
    }
    let tx = ix + isz + 10.0 * s;
    let right = x + w - CARD_PAD * s;
    let small = 11.5 * s;
    let tw = ctx.text.measure(&n.time, small, Weight::Regular);
    let base = y + CARD_PAD * s + 12.0 * s;
    ctx.text.draw(pm, &n.time, right - tw, base, small, Weight::Regular, th.secondary(), None);
    let title = if n.title.is_empty() { &n.app } else { &n.title };
    let title = ctx.text.ellipsize(title, TITLE * s, Weight::Bold, right - tx - tw - 8.0 * s);
    ctx.text.draw(pm, &title, tx, base, TITLE * s, Weight::Bold, th.label(), None);
    for (i, line) in body_lines(ctx, n, s).iter().enumerate() {
        ctx.text.draw(pm, line, tx, base + (i as f32 + 1.0) * LINE * s, BODY * s, Weight::Regular, th.label(), None);
    }
    if close {
        let (cx, cy, r) = (x + 4.0 * s, y + 4.0 * s, 10.0 * s);
        if let Some(p) = gfx::circle(cx, cy, r) {
            gfx::fill(pm, &p, th.panel());
            gfx::stroke(pm, &p, th.panel_border(), 1.0);
        }
        let g = 14.0 * s;
        glyphs::close_x(pm, cx - g / 2.0, cy - g / 2.0, g, th.secondary());
    }
    h
}

fn close_rect(x: f32, y: f32, s: f32) -> (f32, f32, f32, f32) {
    (x - 8.0 * s, y - 8.0 * s, 24.0 * s, 24.0 * s)
}

fn days_in(year: i32, month: usize) -> u32 {
    match month {
        1 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        1 => 28,
        3 | 5 | 8 | 10 => 30,
        _ => 31,
    }
}

/// The small month widget, measured on Tahoe 26.0 (BB, 2 px/pt; TAHOE-CC-MOTION.md
/// section 6): month name cap top 22 pt and baseline 29.5 pt, weekday letters
/// on a 46.5 pt baseline, dates from 64.5 pt on a 17.25 pt pitch, seven
/// columns 18.46 pt apart centred in the widget, today a 17 pt disc.
///
/// Rows are laid out in whole device pixels from one rounded pitch (rounding
/// each row on its own made 1x rows alternate 14/15 px). Each number starts
/// on a whole pixel: quarter-pixel glyphs rendered some digits bolder or
/// blurrier than their neighbours on the owner's 1x screen.
#[allow(clippy::too_many_arguments)]
fn draw_calendar(pm: &mut Pixmap, ctx: &Ctx, bd: Option<&Backdrop>, t: Today, x: f32, y: f32, s: f32) {
    let th = ctx.theme;
    let size = SMALL * s;
    plate(pm, ctx, bd, x, y, size, size, WIDGET_R * s, s);
    let red = rgba(227, 49, 65, 1.0);
    let st = ctx.strings;
    let fs = CAL_TEXT * s;
    let month = st.months_full[t.month % 12].to_uppercase();
    ctx.text.draw(pm, &month, (x + CAL_LEFT * s).round(), (y + CAL_MONTH * s).round(), CAL_MONTH_TEXT * s, Weight::Bold, red, None);
    let col_pitch = CAL_COL * s;
    let col0 = x + size / 2.0 - 3.0 * col_pitch;
    let col_x = |c: usize| col0 + col_pitch * c as f32;
    // Columns of Saturday and Sunday.
    let weekend = |c: usize| matches!((c + st.week_start) % 7, 0 | 6);
    let label = th.label();
    let dim = th.secondary();
    for (i, l) in st.week_letters.iter().enumerate() {
        let lw = ctx.text.measure(l, fs, Weight::Bold);
        ctx.text.draw(pm, l, (col_x(i) - lw / 2.0).round(), (y + CAL_WEEKDAYS * s).round(), fs, Weight::Bold, dim, None);
    }
    let first_wd = (t.weekday as i32 - (t.day as i32 - 1)).rem_euclid(7) as usize;
    let lead = (first_wd + 7 - st.week_start) % 7;
    let days = days_in(t.year, t.month);
    let rows = (lead + days as usize).div_ceil(7);
    let (first, pitch) = cal_rows(rows);
    let first = (y + first * s).round();
    let pitch = (pitch * s).round();
    let cap = ctx.text.cap_height(fs);
    for d in 1..=days {
        let cell = lead + d as usize - 1;
        let (col, row) = (cell % 7, cell / 7);
        let cx = col_x(col);
        let base = first + row as f32 * pitch;
        let text = d.to_string();
        let lw = ctx.text.measure(&text, fs, Weight::Bold);
        let today = d == t.day;
        if today && let Some(p) = gfx::circle(cx, base - cap / 2.0, CAL_DISC / 2.0 * s) {
            gfx::fill(pm, &p, red);
        }
        let c = if today {
            rgba(255, 255, 255, 1.0)
        } else if weekend(col) {
            dim
        } else {
            label
        };
        ctx.text.draw(pm, &text, (cx - lw / 2.0).round(), base, fs, Weight::Bold, c, None);
    }
}

/// First date baseline and row pitch, pt from the widget top. Five rows are
/// measured (64.5 + 17.25 n, the last on 133.5). Six rows were not captured
/// (NF): they keep the weekday-to-last-row span and share it evenly, so the
/// grid stays inside the same margins.
fn cal_rows(rows: usize) -> (f32, f32) {
    if rows > 5 {
        let pitch = (133.5 - CAL_WEEKDAYS) / 6.0;
        (CAL_WEEKDAYS + pitch, pitch)
    } else {
        (64.5, 17.25)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Card(usize),
    Close(usize),
    Clear,
}

pub struct NotificationCenter {
    scale: f32,
    notes: Vec<NoteView>,
    today: Today,
    max_h: f32,
    reveal: Reveal,
    backdrop: Option<Backdrop>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    pub events: Vec<NoteEvent>,
}

impl NotificationCenter {
    /// `max_h`: tallest window allowed, px.
    pub fn new(notes: Vec<NoteView>, today: Today, scale: f32, max_h: f32) -> Self {
        Self { scale, notes, today, max_h, reveal: Reveal::new(), backdrop: None, hover: None, pressed: None, events: Vec::new() }
    }

    /// What is behind the window, window-sized; the cards are glass over it.
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
    }

    pub fn set_notes(&mut self, notes: Vec<NoteView>, today: Today) {
        self.notes = notes;
        self.today = today;
    }

    /// Card positions (index, y, h) in window px, plus the calendar's y.
    fn layout(&self, ctx: &Ctx) -> (Vec<(usize, f32, f32)>, f32) {
        let s = self.scale;
        let m = MARGIN * s;
        let limit = self.max_h - m - (CAL_H + WIDGET_GAP) * s;
        let mut y = m;
        let mut cards = Vec::new();
        if !self.notes.is_empty() {
            y += HEADER * s;
        }
        for (i, n) in self.notes.iter().enumerate() {
            let h = card_h(ctx, n, s);
            if y + h > limit {
                break;
            }
            cards.push((i, y, h));
            y += h + GAP * s;
        }
        if !cards.is_empty() {
            y += (WIDGET_GAP - GAP) * s;
        }
        (cards, y)
    }

    pub fn window_size(&self, ctx: &Ctx) -> (i32, i32) {
        let s = self.scale;
        let (_, cal_y) = self.layout(ctx);
        (((WIDTH + 2.0 * MARGIN) * s).ceil() as i32, (cal_y + (CAL_H + MARGIN) * s).ceil() as i32)
    }

    fn dx(&self) -> f32 {
        (1.0 - self.reveal.alpha()) * 28.0 * self.scale
    }

    fn clear_rect(&self, ctx: &Ctx) -> (f32, f32, f32, f32) {
        let s = self.scale;
        let fs = 12.0 * s;
        let w = ctx.text.measure(ctx.strings.clear_all, fs, Weight::Medium) + 20.0 * s;
        (MARGIN * s + WIDTH * s - w + self.dx(), MARGIN * s, w, 22.0 * s)
    }

    fn hit(&self, ctx: &Ctx, x: f32, y: f32) -> Option<Hit> {
        if !self.notes.is_empty() && inside(self.clear_rect(ctx), x, y) {
            return Some(Hit::Clear);
        }
        let cx = MARGIN * self.scale + self.dx();
        let (cards, _) = self.layout(ctx);
        for &(i, cy, h) in &cards {
            if self.hover.is_some_and(|hv| matches!(hv, Hit::Card(k) | Hit::Close(k) if k == i)) && inside(close_rect(cx, cy, self.scale), x, y) {
                return Some(Hit::Close(i));
            }
            if inside((cx, cy, WIDTH * self.scale, h), x, y) {
                return Some(Hit::Card(i));
            }
        }
        None
    }

    pub fn mouse_move(&mut self, ctx: &Ctx, x: f32, y: f32) {
        self.hover = self.hit(ctx, x, y);
    }
    pub fn mouse_leave(&mut self) {
        self.hover = None;
    }
    pub fn mouse_down(&mut self, ctx: &Ctx, x: f32, y: f32) {
        self.pressed = self.hit(ctx, x, y);
    }
    pub fn mouse_up(&mut self, ctx: &Ctx, x: f32, y: f32) {
        let Some(p) = self.pressed.take() else { return };
        if self.hit(ctx, x, y) != Some(p) {
            return;
        }
        match p {
            Hit::Card(i) => self.events.extend(self.notes.get(i).map(|n| NoteEvent::Open(n.id))),
            Hit::Close(i) => {
                if i < self.notes.len() {
                    let n = self.notes.remove(i);
                    self.events.push(NoteEvent::Dismiss(n.id));
                }
            }
            Hit::Clear => {
                self.notes.clear();
                self.events.push(NoteEvent::ClearAll);
            }
        }
        self.hover = None;
    }

    pub fn close(&mut self) {
        self.reveal.close();
    }
    pub fn closing(&self) -> bool {
        self.reveal.closing()
    }
    pub fn finished(&self) -> bool {
        self.reveal.finished()
    }
    pub fn animating(&self) -> bool {
        self.reveal.animating()
    }
    pub fn update(&mut self, dt: f32) {
        self.reveal.update(dt);
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let s = self.scale;
        let th = ctx.theme;
        let x = MARGIN * s + self.dx();
        let (cards, cal_y) = self.layout(ctx);
        // Keep the gaps between cards clickable so the panel does not flicker
        // between the pointer's targets.
        gfx::put_rect(pm, x, MARGIN * s, WIDTH * s, cal_y - MARGIN * s, [0, 0, 0, 1]);
        if !self.notes.is_empty() {
            let r = self.clear_rect(ctx);
            plate(pm, ctx, self.backdrop.as_ref(), r.0, r.1, r.2, r.3, r.3 / 2.0, s);
            if self.pressed == Some(Hit::Clear)
                && let Some(p) = gfx::squircle(r.0, r.1, r.2, r.3, r.3 / 2.0)
            {
                gfx::fill(pm, &p, th.separator());
            }
            let fs = 12.0 * s;
            ctx.text.draw(pm, ctx.strings.clear_all, r.0 + 10.0 * s, r.1 + r.3 / 2.0 + ctx.text.cap_height(fs) / 2.0, fs, Weight::Medium, th.label(), None);
        }
        for &(i, cy, _) in &cards {
            let hovered = matches!(self.hover, Some(Hit::Card(k) | Hit::Close(k)) if k == i);
            let pressed = self.pressed == Some(Hit::Card(i));
            draw_card(pm, ctx, self.backdrop.as_ref(), &self.notes[i], x, cy, s, hovered, pressed);
        }
        draw_calendar(pm, ctx, self.backdrop.as_ref(), self.today, x, cal_y, s);
        panel::fade_all(pm, self.reveal.alpha());
    }
}

/// A banner for one new notification, top-right, sliding in from the edge.
pub struct Banner {
    scale: f32,
    pub note: NoteView,
    slide: Spring,
    backdrop: Option<Backdrop>,
    hover: bool,
    close_hover: bool,
    pressed: bool,
    closing: bool,
    pub events: Vec<NoteEvent>,
}

impl Banner {
    pub fn new(note: NoteView, scale: f32) -> Self {
        let mut slide = Spring::new(0.0, 0.42, 0.82).epsilon(0.002);
        slide.to(1.0);
        Self { scale, note, slide, backdrop: None, hover: false, close_hover: false, pressed: false, closing: false, events: Vec::new() }
    }

    pub fn window_size(&self, ctx: &Ctx) -> (i32, i32) {
        let s = self.scale;
        (((WIDTH + 2.0 * MARGIN) * s).ceil() as i32, (card_h(ctx, &self.note, s) + 2.0 * MARGIN * s).ceil() as i32)
    }

    pub fn close(&mut self) {
        if !self.closing {
            self.closing = true;
            self.slide = Spring::new(self.slide.value, 0.3, 1.0).epsilon(0.002);
            self.slide.to(0.0);
        }
    }
    pub fn finished(&self) -> bool {
        self.closing && self.slide.settled()
    }
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
    }

    /// The host times the banner out; the pointer on it keeps it up.
    pub fn hovered(&self) -> bool {
        self.hover
    }
    pub fn animating(&self) -> bool {
        !self.slide.settled()
    }
    pub fn update(&mut self, dt: f32) {
        self.slide.step(dt);
    }

    fn origin(&self) -> (f32, f32) {
        let s = self.scale;
        let off = (1.0 - self.slide.value.clamp(0.0, 1.2)) * (WIDTH + MARGIN) * s;
        (MARGIN * s + off, MARGIN * s)
    }

    pub fn mouse_move(&mut self, ctx: &Ctx, x: f32, y: f32) {
        let (ox, oy) = self.origin();
        let s = self.scale;
        self.hover = inside((ox, oy, WIDTH * s, card_h(ctx, &self.note, s)), x, y) || inside(close_rect(ox, oy, s), x, y);
        self.close_hover = self.hover && inside(close_rect(ox, oy, s), x, y);
    }
    pub fn mouse_leave(&mut self) {
        self.hover = false;
        self.close_hover = false;
        self.pressed = false;
    }
    pub fn mouse_down(&mut self) {
        self.pressed = self.hover;
    }
    pub fn mouse_up(&mut self) {
        if std::mem::take(&mut self.pressed) && self.hover {
            self.events.push(if self.close_hover { NoteEvent::Dismiss(self.note.id) } else { NoteEvent::Open(self.note.id) });
            self.close();
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let (ox, oy) = self.origin();
        draw_card(pm, ctx, self.backdrop.as_ref(), &self.note, ox, oy, self.scale, self.hover, self.pressed && !self.close_hover);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_lengths() {
        assert_eq!(days_in(2024, 1), 29);
        assert_eq!(days_in(2026, 1), 28);
        assert_eq!(days_in(2026, 10), 30);
        assert_eq!(days_in(2026, 11), 31);
    }
}
