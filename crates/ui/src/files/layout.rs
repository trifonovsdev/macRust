//! Geometry of the file manager window. Sizes are in points (pt) as measured
//! on macOS 26 Finder (sources in out/ref/TAHOE-FILES.md); the functions
//! return physical pixels for a given scale.

use super::model::Column;

/// Height of the toolbar row that holds the traffic lights.
pub const TOOLBAR_H: f32 = 52.0;
/// The sidebar floats this far from the window's top, left and bottom edges.
pub const INSET: f32 = 8.0;
pub const SIDEBAR_W: f32 = 200.0;
/// Smaller than Tahoe's ≈18 pt: the window corner on Windows 11 is only 8 px.
pub const SIDEBAR_R: f32 = 14.0;

pub const LIGHT_D: f32 = 14.0;
pub const LIGHT_PITCH: f32 = 23.0;
/// Left edge of the close button, from the window's left edge.
pub const LIGHT_X: f32 = 19.0;
pub const LIGHT_CY: f32 = 26.0;

pub const SB_ROW_H: f32 = 32.0;
pub const SB_HEADER_CY: f32 = 62.0;
pub const SB_HEADER_TO_ROW: f32 = 24.0;
pub const SB_ROW_TO_HEADER: f32 = 39.0;
pub const SB_ICON: f32 = 17.0;
pub const SB_ICON_X: f32 = 20.0;
pub const SB_TEXT_X: f32 = 46.0;
pub const SB_SEL_L: f32 = 10.0;
pub const SB_SEL_R: f32 = 9.0;
pub const SB_SEL_H: f32 = 30.0;
pub const SB_SEL_RADIUS: f32 = 10.0;
pub const SB_FONT: f32 = 13.0;
pub const SB_HEADER_FONT: f32 = 11.0;

pub const BTN_H: f32 = 34.0;
pub const NAV_W: f32 = 71.0;
/// Gap from the sidebar's right edge to the back/forward capsule.
pub const NAV_GAP: f32 = 9.0;
pub const TITLE_GAP: f32 = 14.0;
pub const TITLE_FONT: f32 = 15.0;
pub const SEG_W: f32 = 36.0;
pub const SEARCH_W: f32 = 180.0;
pub const SEARCH_FONT: f32 = 13.0;
pub const TOOL_GAP: f32 = 10.0;
pub const RIGHT_PAD: f32 = 10.0;

pub const HEADER_H: f32 = 28.0;
pub const HEADER_FONT: f32 = 11.0;
pub const ROW_H: f32 = 20.0;
pub const ROW_INSET: f32 = 6.0;
pub const LIST_PAD: f32 = 4.0;
pub const LIST_ICON: f32 = 16.0;
pub const ROW_FONT: f32 = 13.0;
pub const COL_PAD: f32 = 10.0;

pub const CELL_W: f32 = 112.0;
pub const CELL_H: f32 = 114.0;
pub const ICON: f32 = 64.0;
/// The selection square around an icon: 4 pt on each side.
pub const TILE: f32 = 72.0;
pub const TILE_TOP: f32 = 6.0;
pub const LABEL_GAP: f32 = 3.0;
pub const LABEL_FONT: f32 = 12.0;
pub const LABEL_LINE: f32 = 15.0;
pub const GRID_PAD: f32 = 10.0;

pub const PATHBAR_H: f32 = 26.0;
pub const STATUS_H: f32 = 26.0;
pub const STATUS_FONT: f32 = 11.0;

pub const SCROLLBAR_W: f32 = 7.0;
pub const SCROLLBAR_WIDE: f32 = 11.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    /// From two corners in any order.
    pub fn span(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x: x0.min(x1), y: y0.min(y1), w: (x1 - x0).abs(), h: (y1 - y0).abs() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Icons,
    List,
}

/// Where everything is in the window, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub s: f32,
    pub w: f32,
    pub h: f32,
    pub sidebar: Rect,
    pub toolbar: Rect,
    pub nav: Rect,
    pub search: Rect,
    pub view_seg: Rect,
    pub new_folder: Rect,
    pub title_x: f32,
    /// Column headers (list view only; zero height otherwise).
    pub header: Rect,
    /// The scrolling viewport.
    pub content: Rect,
    pub pathbar: Rect,
    pub status: Rect,
}

pub fn frame(w: f32, h: f32, s: f32, view: View, pathbar: bool) -> Frame {
    let sidebar = Rect::new(INSET * s, INSET * s, SIDEBAR_W * s, (h - 2.0 * INSET * s).max(0.0));
    let col_x = sidebar.right();
    let toolbar = Rect::new(col_x, 0.0, (w - col_x).max(0.0), TOOLBAR_H * s);
    let cy = toolbar.h / 2.0;
    let bh = BTN_H * s;
    let nav = Rect::new(col_x + NAV_GAP * s, cy - bh / 2.0, NAV_W * s, bh);
    let mut x = w - RIGHT_PAD * s;
    let search_w = SEARCH_W * s;
    // A narrow window drops the search field width before anything else.
    let room = x - nav.right() - 3.0 * TOOL_GAP * s - 2.0 * SEG_W * s - bh - 120.0 * s;
    let search_w = search_w.min(room).max(100.0 * s);
    x -= search_w;
    let search = Rect::new(x, cy - bh / 2.0, search_w, bh);
    x -= TOOL_GAP * s + bh;
    let new_folder = Rect::new(x, cy - bh / 2.0, bh, bh);
    x -= TOOL_GAP * s + 2.0 * SEG_W * s;
    let view_seg = Rect::new(x, cy - bh / 2.0, 2.0 * SEG_W * s, bh);
    let bottom_h = (STATUS_H + if pathbar { PATHBAR_H } else { 0.0 }) * s;
    let status = Rect::new(col_x, h - STATUS_H * s, toolbar.w, STATUS_H * s);
    let pathbar = Rect::new(col_x, status.y - if pathbar { PATHBAR_H * s } else { 0.0 }, toolbar.w, if pathbar { PATHBAR_H * s } else { 0.0 });
    let header_h = if view == View::List { HEADER_H * s } else { 0.0 };
    let header = Rect::new(col_x, toolbar.bottom(), toolbar.w, header_h);
    let content = Rect::new(col_x, header.bottom(), toolbar.w, (h - header.bottom() - bottom_h).max(0.0));
    Frame {
        s,
        w,
        h,
        sidebar,
        toolbar,
        nav,
        search,
        view_seg,
        new_folder,
        title_x: nav.right() + TITLE_GAP * s,
        header,
        content,
        pathbar,
        status,
    }
}

impl Frame {
    /// The traffic light `i` (0 close, 1 minimize, 2 zoom) as a circle box.
    pub fn light(&self, i: usize) -> Rect {
        let d = LIGHT_D * self.s;
        Rect::new((LIGHT_X + LIGHT_PITCH * i as f32) * self.s, LIGHT_CY * self.s - d / 2.0, d, d)
    }

    /// The hover area of the traffic lights as a group.
    pub fn lights(&self) -> Rect {
        let a = self.light(0);
        let c = self.light(2);
        Rect::new(a.x - 4.0 * self.s, a.y - 4.0 * self.s, c.right() - a.x + 8.0 * self.s, a.h + 8.0 * self.s)
    }
}

// ---------------------------------------------------------------------------
// Icon grid

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    pub pitch_x: f32,
    pub pitch_y: f32,
    pub pad: f32,
    pub s: f32,
}

impl Grid {
    pub fn new(content_w: f32, s: f32) -> Self {
        let pad = GRID_PAD * s;
        let avail = (content_w - 2.0 * pad).max(CELL_W * s);
        let cols = ((avail / (CELL_W * s)).floor() as usize).max(1);
        // Spare width is shared between the columns, as Finder spreads its grid.
        Self { cols, pitch_x: avail / cols as f32, pitch_y: CELL_H * s, pad, s }
    }

    pub fn rows(&self, n: usize) -> usize {
        n.div_ceil(self.cols)
    }

    pub fn height(&self, n: usize) -> f32 {
        self.rows(n) as f32 * self.pitch_y + 2.0 * self.pad
    }

    /// Cell `i` relative to the scrolled content's top-left.
    pub fn cell(&self, i: usize) -> Rect {
        let (r, c) = (i / self.cols, i % self.cols);
        Rect::new(self.pad + c as f32 * self.pitch_x, self.pad + r as f32 * self.pitch_y, self.pitch_x, self.pitch_y)
    }

    /// The selection tile behind the icon of cell `i`.
    pub fn tile(&self, i: usize) -> Rect {
        let c = self.cell(i);
        let t = TILE * self.s;
        Rect::new((c.x + (c.w - t) / 2.0).round(), (c.y + TILE_TOP * self.s).round(), t.round(), t.round())
    }

    /// Items whose cells intersect rows `y0..y1` (content coordinates).
    pub fn range(&self, y0: f32, y1: f32, n: usize) -> std::ops::Range<usize> {
        let r0 = ((y0 - self.pad) / self.pitch_y).floor().max(0.0) as usize;
        let r1 = ((y1 - self.pad) / self.pitch_y).ceil().max(0.0) as usize;
        (r0 * self.cols).min(n)..(r1 * self.cols).min(n)
    }

    /// The item under a point: its icon tile or its label, not the empty
    /// parts of the cell (they start a rubber band, as in Finder).
    pub fn hit(&self, x: f32, y: f32, n: usize, label_w: impl Fn(usize) -> (f32, f32)) -> Option<usize> {
        if x < self.pad || y < self.pad {
            return None;
        }
        let c = ((x - self.pad) / self.pitch_x) as usize;
        let r = ((y - self.pad) / self.pitch_y) as usize;
        if c >= self.cols {
            return None;
        }
        let i = r * self.cols + c;
        if i >= n {
            return None;
        }
        let tile = self.tile(i);
        if tile.contains(x, y) {
            return Some(i);
        }
        let (lw, lh) = label_w(i);
        let cell = self.cell(i);
        let label = Rect::new(cell.x + (cell.w - lw) / 2.0, tile.bottom() + LABEL_GAP * self.s, lw, lh);
        label.contains(x, y).then_some(i)
    }

    /// Items whose tile or label box meet the band (content coordinates).
    pub fn in_band(&self, band: &Rect, n: usize) -> Vec<usize> {
        self.range(band.y, band.bottom(), n)
            .filter(|&i| {
                let t = self.tile(i);
                let body = Rect::new(t.x, t.y, t.w, t.h + (LABEL_GAP + LABEL_LINE) * self.s);
                body.intersects(band)
            })
            .collect()
    }

    /// Keyboard moves in the grid.
    pub fn step(&self, pos: usize, n: usize, dx: i32, dy: i32) -> usize {
        if n == 0 {
            return 0;
        }
        let p = pos as i64 + dx as i64 + dy as i64 * self.cols as i64;
        if dy != 0 && (p < 0 || p >= n as i64) {
            return if p < 0 { pos } else { n - 1 };
        }
        p.clamp(0, n as i64 - 1) as usize
    }
}

// ---------------------------------------------------------------------------
// List

#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    /// (column, x, width) relative to the content's left edge; hidden
    /// columns are left out.
    pub cols: Vec<(Column, f32, f32)>,
    pub s: f32,
}

impl Columns {
    pub fn new(content_w: f32, s: f32) -> Self {
        let inner = content_w - 2.0 * ROW_INSET * s;
        let (date_w, size_w, kind_w) = (172.0 * s, 76.0 * s, 128.0 * s);
        // Narrow windows drop Kind, then Date, keeping the name readable.
        let mut extra = vec![(Column::Modified, date_w), (Column::Size, size_w), (Column::Kind, kind_w)];
        let min_name = 200.0 * s;
        while !extra.is_empty() && inner - extra.iter().map(|e| e.1).sum::<f32>() < min_name {
            let drop = if extra.iter().any(|e| e.0 == Column::Kind) { Column::Kind } else { extra[0].0 };
            extra.retain(|e| e.0 != drop);
        }
        let name_w = (inner - extra.iter().map(|e| e.1).sum::<f32>()).max(0.0);
        let mut x = ROW_INSET * s;
        let mut cols = vec![(Column::Name, x, name_w)];
        x += name_w;
        for (c, w) in extra {
            cols.push((c, x, w));
            x += w;
        }
        Self { cols, s }
    }

    pub fn at(&self, x: f32) -> Option<Column> {
        self.cols.iter().find(|(_, cx, w)| x >= *cx && x < cx + w).map(|c| c.0)
    }
}

pub fn list_height(n: usize, s: f32) -> f32 {
    n as f32 * ROW_H * s + 2.0 * LIST_PAD * s
}

pub fn row_y(i: usize, s: f32) -> f32 {
    LIST_PAD * s + i as f32 * ROW_H * s
}

pub fn row_at(y: f32, n: usize, s: f32) -> Option<usize> {
    let r = (y - LIST_PAD * s) / (ROW_H * s);
    (r >= 0.0 && (r as usize) < n).then_some(r as usize)
}

pub fn row_range(y0: f32, y1: f32, n: usize, s: f32) -> std::ops::Range<usize> {
    let r0 = ((y0 - LIST_PAD * s) / (ROW_H * s)).floor().max(0.0) as usize;
    let r1 = ((y1 - LIST_PAD * s) / (ROW_H * s)).ceil().max(0.0) as usize;
    r0.min(n)..r1.min(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_regions_tile_the_window() {
        let f = frame(1200.0, 700.0, 1.0, View::List, true);
        assert_eq!(f.sidebar.right(), 208.0);
        assert_eq!(f.toolbar.x, 208.0);
        assert_eq!(f.header.y, 52.0);
        assert_eq!(f.content.y, 80.0);
        assert_eq!(f.content.bottom(), f.pathbar.y);
        assert_eq!(f.pathbar.bottom(), f.status.y);
        assert_eq!(f.status.bottom(), 700.0);
        assert!(f.nav.right() < f.view_seg.x && f.view_seg.right() < f.new_folder.x && f.new_folder.right() < f.search.x);
        assert_eq!(f.search.right(), 1190.0);
        let f2 = frame(2400.0, 1400.0, 2.0, View::Icons, false);
        assert_eq!(f2.sidebar.right(), 416.0);
        assert_eq!(f2.header.h, 0.0);
        assert_eq!(f2.content.bottom(), 1400.0 - 52.0);
        let l = f.light(2);
        assert_eq!((l.x, l.w), (65.0, 14.0));
    }

    #[test]
    fn grid_layout_and_hits() {
        let g = Grid::new(992.0, 1.0);
        assert_eq!(g.cols, 8);
        assert!((g.pitch_x - 121.5).abs() < 0.01);
        assert_eq!(g.rows(17), 3);
        let t = g.tile(9);
        assert_eq!(g.hit(t.x + 2.0, t.y + 2.0, 20, |_| (50.0, 15.0)), Some(9));
        // The empty corner of a cell is not the item.
        let c = g.cell(9);
        assert_eq!(g.hit(c.x + 1.0, c.y + 1.0, 20, |_| (50.0, 15.0)), None);
        assert_eq!(g.hit(t.x + 2.0, t.y + 2.0, 9, |_| (50.0, 15.0)), None);
        assert_eq!(g.range(0.0, 300.0, 100), 0..24);
        assert_eq!(g.step(3, 20, 0, 1), 11);
        assert_eq!(g.step(3, 20, 0, -1), 3);
        assert_eq!(g.step(15, 20, 0, 1), 19);
        assert_eq!(g.step(0, 20, -1, 0), 0);
        let band = Rect::span(0.0, 0.0, 250.0, 40.0);
        assert_eq!(g.in_band(&band, 20), [0, 1]);
    }

    #[test]
    fn list_columns_shrink() {
        let c = Columns::new(800.0, 1.0);
        assert_eq!(c.cols.len(), 4);
        assert_eq!(c.cols[0].1, 6.0);
        let last = c.cols.last().unwrap();
        assert!((last.1 + last.2 - 794.0).abs() < 0.01);
        assert_eq!(Columns::new(500.0, 1.0).cols.len(), 3);
        assert_eq!(Columns::new(300.0, 1.0).cols.len(), 2);
        assert_eq!(c.at(700.0), Some(Column::Kind));
    }

    #[test]
    fn list_rows() {
        assert_eq!(row_at(3.0, 10, 1.0), None);
        assert_eq!(row_at(4.0, 10, 1.0), Some(0));
        assert_eq!(row_at(25.0, 10, 1.0), Some(1));
        assert_eq!(row_at(205.0, 10, 1.0), None);
        assert_eq!(row_range(0.0, 100.0, 1000, 1.0), 0..5);
        assert_eq!(row_range(30.0, 50.0, 1000, 2.0), 0..2);
    }
}
