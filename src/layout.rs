//! Imposition: turns a page selection into physical sheet sides.
//! All coordinates are millimetres on the sheet, origin top-left.

use crate::settings::{BindingEdge, LayoutMode, NupOrder, Orientation, Settings};

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
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub page: usize,
    /// Clip rectangle (the page is never drawn outside it).
    pub cell: Rect,
    /// Rectangle the page is fitted into (larger than `cell` for poster tiles).
    pub frame: Rect,
    /// Overrides the horizontal alignment (booklet pages hug the fold).
    pub halign: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PosterTile {
    pub tx: u32,
    pub ty: u32,
    pub nx: u32,
    pub ny: u32,
    pub overlap: f32,
    pub area: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub placements: Vec<Placement>,
    pub back: bool,
    pub poster: Option<PosterTile>,
}

impl Sheet {
    pub fn pages(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.placements.iter().map(|p| p.page).collect();
        v.dedup();
        v
    }
}

/// Printable content area for one sheet side; `hw` = hardware margins [t, r, b, l].
pub fn content_area(sheet: (f32, f32), s: &Settings, hw: [f32; 4], side: usize, duplex: bool) -> Rect {
    let mut m = s.margins_mm;
    if s.respect_hw_margins {
        for i in 0..4 {
            m[i] = m[i].max(hw[i]);
        }
    }
    let g = s.binding_gutter_mm.max(0.0);
    let back = duplex && side % 2 == 1;
    if g > 0.0 {
        match (s.binding_edge, back) {
            (BindingEdge::Left, false) | (BindingEdge::Right, true) => m[3] += g,
            (BindingEdge::Left, true) | (BindingEdge::Right, false) => m[1] += g,
            (BindingEdge::Top, false) => m[0] += g,
            (BindingEdge::Top, true) => m[2] += g,
        }
    }
    Rect::new(
        m[3],
        m[0],
        (sheet.0 - m[1] - m[3]).max(1.0),
        (sheet.1 - m[0] - m[2]).max(1.0),
    )
}

fn grid_cell(area: Rect, cols: u32, rows: u32, gap: f32, c: u32, r: u32) -> Rect {
    let cw = ((area.w - gap * (cols as f32 - 1.0)) / cols as f32).max(0.5);
    let ch = ((area.h - gap * (rows as f32 - 1.0)) / rows as f32).max(0.5);
    Rect::new(area.x + c as f32 * (cw + gap), area.y + r as f32 * (ch + gap), cw, ch)
}

fn order_pos(order: NupOrder, k: u32, cols: u32, rows: u32) -> (u32, u32) {
    match order {
        NupOrder::RowsLtr => (k % cols, k / cols),
        NupOrder::RowsRtl => (cols - 1 - k % cols, k / cols),
        NupOrder::ColsTtb => (k / rows, k % rows),
        NupOrder::ColsTtbRtl => (cols - 1 - k / rows, k % rows),
    }
}

pub fn build_sheets(
    sel: &[usize],
    sheet: (f32, f32),
    s: &Settings,
    hw: [f32; 4],
) -> Vec<Sheet> {
    let duplex = s.is_duplex();
    let cols = s.nup_cols.clamp(1, 16);
    let rows = s.nup_rows.clamp(1, 16);
    let gap = s.cell_gap_mm.max(0.0);
    let mut out = Vec::new();

    match s.layout {
        LayoutMode::Normal => {
            let per = (cols * rows) as usize;
            for (si, chunk) in sel.chunks(per).enumerate() {
                let area = content_area(sheet, s, hw, si, duplex);
                let placements = chunk
                    .iter()
                    .enumerate()
                    .map(|(k, &page)| {
                        let (c, r) = order_pos(s.nup_order, k as u32, cols, rows);
                        let cell = grid_cell(area, cols, rows, gap, c, r);
                        Placement { page, cell, frame: cell, halign: None }
                    })
                    .collect();
                out.push(Sheet { placements, back: duplex && si % 2 == 1, poster: None });
            }
        }
        LayoutMode::Repeat => {
            for (si, &page) in sel.iter().enumerate() {
                let area = content_area(sheet, s, hw, si, duplex);
                let mut placements = Vec::new();
                for r in 0..rows {
                    for c in 0..cols {
                        let cell = grid_cell(area, cols, rows, gap, c, r);
                        placements.push(Placement { page, cell, frame: cell, halign: None });
                    }
                }
                out.push(Sheet { placements, back: duplex && si % 2 == 1, poster: None });
            }
        }
        LayoutMode::Poster => {
            let nx = s.poster_x.clamp(1, 20);
            let ny = s.poster_y.clamp(1, 20);
            let mut si = 0;
            for &page in sel {
                for ty in 0..ny {
                    for tx in 0..nx {
                        let area = content_area(sheet, s, hw, si, false);
                        let ov = s.poster_overlap_mm.clamp(0.0, area.w.min(area.h) * 0.45);
                        let vw = nx as f32 * area.w - (nx as f32 - 1.0) * ov;
                        let vh = ny as f32 * area.h - (ny as f32 - 1.0) * ov;
                        let frame = Rect::new(
                            area.x - tx as f32 * (area.w - ov),
                            area.y - ty as f32 * (area.h - ov),
                            vw,
                            vh,
                        );
                        out.push(Sheet {
                            placements: vec![Placement { page, cell: area, frame, halign: None }],
                            back: false,
                            poster: Some(PosterTile { tx, ty, nx, ny, overlap: ov, area }),
                        });
                        si += 1;
                    }
                }
            }
        }
        LayoutMode::Booklet => {
            let sig_pages = if s.booklet_signature == 0 {
                (sel.len() + 3) / 4 * 4
            } else {
                s.booklet_signature as usize * 4
            }
            .max(4);
            let mut pages: Vec<Option<usize>> = sel.iter().map(|&p| Some(p)).collect();
            while pages.len() % sig_pages != 0 || pages.is_empty() {
                pages.push(None);
            }
            let mut side = 0;
            for sig in pages.chunks(sig_pages) {
                let p = sig.len();
                for i in 0..p / 4 {
                    let sides = [(p - 1 - 2 * i, 2 * i), (2 * i + 1, p - 2 - 2 * i)];
                    for (l, r) in sides {
                        let (l, r) = if s.booklet_rtl { (r, l) } else { (l, r) };
                        let area = content_area(sheet, s, hw, 0, false);
                        let half = (area.w - gap) / 2.0;
                        let lc = Rect::new(area.x, area.y, half, area.h);
                        let rc = Rect::new(area.x + half + gap, area.y, half, area.h);
                        let fold = s.booklet_align_fold;
                        let mut placements = Vec::new();
                        if let Some(pg) = sig[l] {
                            placements.push(Placement { page: pg, cell: lc, frame: lc, halign: fold.then_some(2) });
                        }
                        if let Some(pg) = sig[r] {
                            placements.push(Placement { page: pg, cell: rc, frame: rc, halign: fold.then_some(0) });
                        }
                        out.push(Sheet { placements, back: side % 2 == 1, poster: None });
                        side += 1;
                    }
                }
            }
        }
    }
    out
}

/// Decide landscape vs portrait for `Orientation::Auto` by comparing how large
/// the first pages would print in each orientation.
pub fn choose_landscape(
    sel: &[usize],
    page_mm: &dyn Fn(usize) -> (f32, f32),
    paper: (f32, f32),
    s: &Settings,
    hw: [f32; 4],
) -> bool {
    match s.orientation {
        Orientation::Portrait => return false,
        Orientation::Landscape => return true,
        Orientation::Auto => {}
    }
    if s.layout == LayoutMode::Booklet {
        return true;
    }
    let score = |sheet: (f32, f32)| -> f32 {
        let sheets = build_sheets(&sel[..sel.len().min(12)], sheet, s, hw);
        let mut total = 0.0;
        let mut n = 0.0;
        for sh in sheets.iter().take(4) {
            for pl in &sh.placements {
                let (pw, ph) = page_mm(pl.page);
                let f = &pl.frame;
                let mut k = (f.w / pw).min(f.h / ph);
                if s.auto_rotate {
                    k = k.max((f.w / ph).min(f.h / pw));
                }
                total += k;
                n += 1.0;
            }
        }
        if n > 0.0 { total / n } else { 0.0 }
    };
    let (w, h) = (paper.0.min(paper.1), paper.0.max(paper.1));
    score((h, w)) > score((w, h)) * 1.01
}
