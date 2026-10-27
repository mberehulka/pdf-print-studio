//! Rasterizes one sheet side: places pages, applies image adjustments,
//! overlays (watermark, header/footer, marks) and the final colour mode.
//! The same code drives the on-screen preview, printing and PNG export.

use crate::layout::{Placement, Rect, Sheet};
use crate::settings::{ColorMode, Scaling, Settings, StampScope};
use crate::template::{self, Clock, Value, Vars};
use crate::text::{self, FontCache, Sprite};
use ab_glyph::FontVec;
use image::{imageops, Rgb, RgbImage, RgbaImage};
use std::path::Path;
use std::sync::Arc;

pub trait PageRenderer {
    fn page_size_mm(&self, page: usize) -> (f32, f32);
    /// Render normalised source rect of `page` to exactly `w`x`h` pixels.
    fn render(&mut self, page: usize, src: [f32; 4], w: u32, h: u32) -> Option<RgbaImage>;
    /// Normalised bounding box of non-white content.
    fn content_bbox(&mut self, page: usize) -> [f32; 4];
}

#[derive(Clone, Debug, Default)]
pub struct StampCtx {
    /// File name of the document.
    pub file: String,
    /// Full path of the document (may be empty).
    pub path: String,
    pub doc_pages: usize,
    /// Number of pages in the print selection.
    pub selected: usize,
    pub sheet_no: usize,
    pub sheet_total: usize,
    pub copy: usize,
    pub copies: usize,
    pub printer: String,
    pub clock: Clock,
}

const MAX_TILE_PIXELS: u64 = 40_000_000;
const STRIP_ROWS: i64 = 1024;

pub fn effective_crop(r: &mut dyn PageRenderer, page: usize, s: &Settings) -> [f32; 4] {
    let (pw, ph) = r.page_size_mm(page);
    let c = s.crop_mm;
    let mut out = [
        (c[3] / pw).clamp(0.0, 0.45),
        (c[0] / ph).clamp(0.0, 0.45),
        1.0 - (c[1] / pw).clamp(0.0, 0.45),
        1.0 - (c[2] / ph).clamp(0.0, 0.45),
    ];
    if s.auto_crop {
        let b = r.content_bbox(page);
        out = [out[0].max(b[0]), out[1].max(b[1]), out[2].min(b[2]), out[3].min(b[3])];
        if out[2] - out[0] < 0.02 || out[3] - out[1] < 0.02 {
            out = [0.0, 0.0, 1.0, 1.0];
        }
    }
    out
}

/// Where the page lands on the sheet (mm) and its effective rotation.
pub fn page_draw_rect(r: &mut dyn PageRenderer, pl: &Placement, s: &Settings) -> (Rect, u32, [f32; 4]) {
    let crop = effective_crop(r, pl.page, s);
    let (pw, ph) = r.page_size_mm(pl.page);
    let cw = pw * (crop[2] - crop[0]);
    let ch = ph * (crop[3] - crop[1]);
    let f = pl.frame;
    let mut rot = s.rotation % 360;
    if s.auto_rotate && (cw - ch).abs() > 0.5 {
        let content_land = if rot % 180 == 0 { cw > ch } else { ch > cw };
        if content_land != (f.w > f.h) {
            rot = (rot + 90) % 360;
        }
    }
    let (rw, rh) = if rot % 180 == 0 { (cw, ch) } else { (ch, cw) };
    let fit = (f.w / rw).min(f.h / rh);
    let (sx, sy) = match s.scaling {
        Scaling::Fit => (fit, fit),
        Scaling::ShrinkOnly => (fit.min(1.0), fit.min(1.0)),
        Scaling::Actual => (1.0, 1.0),
        Scaling::Custom => (s.custom_scale / 100.0, s.custom_scale / 100.0),
        Scaling::Fill => {
            let k = (f.w / rw).max(f.h / rh);
            (k, k)
        }
        Scaling::Stretch => (f.w / rw, f.h / rh),
    };
    let (dw, dh) = (rw * sx, rh * sy);
    let ah = pl.halign.unwrap_or(s.align_h);
    let x = match ah {
        0 => f.x,
        1 => f.x + (f.w - dw) / 2.0,
        _ => f.right() - dw,
    } + s.offset_mm[0];
    let y = match s.align_v {
        0 => f.y,
        1 => f.y + (f.h - dh) / 2.0,
        _ => f.bottom() - dh,
    } + s.offset_mm[1];
    (Rect::new(x, y, dw, dh), rot, crop)
}

struct Lut {
    t: [u8; 256],
    saturation: f32,
    identity: bool,
}

impl Lut {
    fn new(s: &Settings) -> Self {
        let c = s.contrast.clamp(-100.0, 100.0) * 2.55;
        let cf = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
        let b = s.brightness.clamp(-100.0, 100.0) * 2.55;
        let g = s.gamma.clamp(0.1, 5.0);
        let toner = (s.toner_saver / 100.0).clamp(0.0, 0.9);
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut x = i as f32;
            x = cf * (x - 128.0) + 128.0 + b;
            x = 255.0 * (x.clamp(0.0, 255.0) / 255.0).powf(1.0 / g);
            if s.invert {
                x = 255.0 - x;
            }
            x = 255.0 - (255.0 - x) * (1.0 - toner);
            *v = x.round().clamp(0.0, 255.0) as u8;
        }
        let identity = t.iter().enumerate().all(|(i, &v)| v as usize == i);
        Self { t, saturation: s.saturation / 100.0, identity }
    }

    fn apply(&self, img: &mut RgbaImage) {
        let sat = (self.saturation - 1.0).abs() > 0.001;
        if self.identity && !sat {
            return;
        }
        for p in img.pixels_mut() {
            if sat {
                let l = p.0[0] as f32 * 0.299 + p.0[1] as f32 * 0.587 + p.0[2] as f32 * 0.114;
                for c in 0..3 {
                    p.0[c] = (l + (p.0[c] as f32 - l) * self.saturation).round().clamp(0.0, 255.0) as u8;
                }
            }
            for c in 0..3 {
                p.0[c] = self.t[p.0[c] as usize];
            }
        }
    }
}

fn fill_rect(img: &mut RgbImage, x0: i64, y0: i64, x1: i64, y1: i64, color: [u8; 3]) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in y0.max(0)..y1.min(h) {
        for x in x0.max(0)..x1.min(w) {
            img.put_pixel(x as u32, y as u32, Rgb(color));
        }
    }
}

fn stroke_rect(img: &mut RgbImage, r: Rect, mmpx: f32, width_mm: f32, color: [u8; 3]) {
    let t = ((width_mm * mmpx).round() as i64).max(1);
    let x0 = (r.x * mmpx).round() as i64;
    let y0 = (r.y * mmpx).round() as i64;
    let x1 = (r.right() * mmpx).round() as i64;
    let y1 = (r.bottom() * mmpx).round() as i64;
    fill_rect(img, x0, y0, x1, y0 + t, color);
    fill_rect(img, x0, y1 - t, x1, y1, color);
    fill_rect(img, x0, y0, x0 + t, y1, color);
    fill_rect(img, x1 - t, y0, x1, y1, color);
}

fn dashed_line(img: &mut RgbImage, x0: i64, y0: i64, x1: i64, y1: i64, t: i64, dash: i64, color: [u8; 3]) {
    if y0 == y1 {
        let mut x = x0;
        while x < x1 {
            fill_rect(img, x, y0 - t / 2, (x + dash).min(x1), y0 - t / 2 + t, color);
            x += dash * 2;
        }
    } else {
        let mut y = y0;
        while y < y1 {
            fill_rect(img, x0 - t / 2, y, x0 - t / 2 + t, (y + dash).min(y1), color);
            y += dash * 2;
        }
    }
}

fn crop_marks(img: &mut RgbImage, r: Rect, mmpx: f32) {
    let t = ((0.25 * mmpx).round() as i64).max(1);
    let gap = (2.0 * mmpx) as i64;
    let len = (5.0 * mmpx) as i64;
    let k = [0u8, 0, 0];
    let xs = [(r.x * mmpx) as i64, (r.right() * mmpx) as i64];
    let ys = [(r.y * mmpx) as i64, (r.bottom() * mmpx) as i64];
    for (i, &x) in xs.iter().enumerate() {
        for (j, &y) in ys.iter().enumerate() {
            let hx = if i == 0 { x - gap - len } else { x + gap };
            fill_rect(img, hx, y, hx + len, y + t, k);
            let vy = if j == 0 { y - gap - len } else { y + gap };
            fill_rect(img, x, vy, x + t, vy + len, k);
        }
    }
}

/// Draw one page placement; processed in horizontal strips to bound memory.
fn draw_placement(img: &mut RgbImage, r: &mut dyn PageRenderer, pl: &Placement, s: &Settings, mmpx: f32, lut: &Lut) -> Rect {
    let (draw, rot, crop) = page_draw_rect(r, pl, s);
    let Some(vis) = draw.intersect(&pl.cell) else { return draw };
    let px0 = (vis.x * mmpx).round() as i64;
    let px1 = (vis.right() * mmpx).round() as i64;
    let py0 = (vis.y * mmpx).round() as i64;
    let py1 = (vis.bottom() * mmpx).round() as i64;
    if px1 <= px0 || py1 <= py0 {
        return draw;
    }
    let filter = s.filter.to_image();
    let mut sy = py0;
    while sy < py1 {
        let ey = (sy + STRIP_ROWS).min(py1);
        let vw = (px1 - px0) as u32;
        let vh = (ey - sy) as u32;
        // normalised position of this strip inside the drawn page rect
        let nx0 = ((px0 as f32 / mmpx - draw.x) / draw.w).clamp(0.0, 1.0);
        let nx1 = ((px1 as f32 / mmpx - draw.x) / draw.w).clamp(0.0, 1.0);
        let ny0 = ((sy as f32 / mmpx - draw.y) / draw.h).clamp(0.0, 1.0);
        let ny1 = ((ey as f32 / mmpx - draw.y) / draw.h).clamp(0.0, 1.0);
        let (mut x0, mut x1, mut y0, mut y1) = (nx0, nx1, ny0, ny1);
        if s.mirror_h {
            (x0, x1) = (1.0 - x1, 1.0 - x0);
        }
        if s.mirror_v {
            (y0, y1) = (1.0 - y1, 1.0 - y0);
        }
        let (u0, u1, v0, v1) = match rot {
            90 => (y0, y1, 1.0 - x1, 1.0 - x0),
            180 => (1.0 - x1, 1.0 - x0, 1.0 - y1, 1.0 - y0),
            270 => (1.0 - y1, 1.0 - y0, x0, x1),
            _ => (x0, x1, y0, y1),
        };
        let cw = crop[2] - crop[0];
        let chh = crop[3] - crop[1];
        let src = [crop[0] + u0 * cw, crop[1] + v0 * chh, crop[0] + u1 * cw, crop[1] + v1 * chh];
        let (tw, th) = if rot % 180 == 0 { (vw, vh) } else { (vh, vw) };
        let mut ss = s.supersample.clamp(1, 4);
        while ss > 1 && (tw as u64 * ss as u64) * (th as u64 * ss as u64) > MAX_TILE_PIXELS {
            ss -= 1;
        }
        if let Some(mut tile) = r.render(pl.page, src, tw * ss, th * ss) {
            if tile.dimensions() != (tw, th) {
                tile = imageops::resize(&tile, tw, th, filter);
            }
            if s.sharpen > 0.0 {
                tile = imageops::unsharpen(&tile, s.sharpen, 2);
            }
            tile = match rot {
                90 => imageops::rotate90(&tile),
                180 => imageops::rotate180(&tile),
                270 => imageops::rotate270(&tile),
                _ => tile,
            };
            if s.mirror_h {
                imageops::flip_horizontal_in_place(&mut tile);
            }
            if s.mirror_v {
                imageops::flip_vertical_in_place(&mut tile);
            }
            lut.apply(&mut tile);
            let (iw, ih) = (img.width() as i64, img.height() as i64);
            for (tx, ty, p) in tile.enumerate_pixels() {
                let x = px0 + tx as i64;
                let y = sy + ty as i64;
                if x >= 0 && y >= 0 && x < iw && y < ih {
                    let a = p.0[3] as u32;
                    let d = img.get_pixel_mut(x as u32, y as u32);
                    for c in 0..3 {
                        d.0[c] = ((p.0[c] as u32 * a + d.0[c] as u32 * (255 - a)) / 255) as u8;
                    }
                }
            }
        }
        sy = ey;
    }
    draw
}

/// Template variables for one sheet side, or for one page on it (`cell` = placement index).
pub fn template_vars(ctx: &StampCtx, sheet: Option<&Sheet>, s: &Settings, cell: Option<usize>) -> Vars {
    let pages: Vec<usize> = match (sheet, cell) {
        (Some(sh), Some(c)) => sh.placements.get(c).map(|p| vec![p.page]).unwrap_or_default(),
        (Some(sh), None) => sh.pages(),
        _ => Vec::new(),
    };
    let first = pages.first().map_or(0, |p| p + 1);
    let last = pages.last().map_or(0, |p| p + 1);
    let range = match (first, last) {
        (0, _) => String::new(),
        (a, b) if a == b => a.to_string(),
        (a, b) => format!("{a}-{b}"),
    };
    let back = sheet.is_some_and(|sh| sh.back);
    let sn = ctx.sheet_no.max(1);
    let path = Path::new(&ctx.path);
    let os = |p: Option<&std::ffi::OsStr>| p.map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file = if ctx.file.is_empty() { os(path.file_name()) } else { ctx.file.clone() };
    let fpath = Path::new(&file);
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let c = ctx.clock;
    let poster = sheet.and_then(|sh| sh.poster.as_ref());

    let mut v = Vars::new();
    let mut set = |k: &str, val: Value| {
        v.insert(k.to_string(), val);
    };
    set("page", first.into());
    set("lastpage", last.into());
    set("range", range.into());
    set("count", pages.len().into());
    set("sheetpages", Value::List(pages.iter().map(|p| Value::from(p + 1)).collect()));
    set("pages", ctx.doc_pages.into());
    set("selected", ctx.selected.into());
    set("pageodd", (first % 2 == 1).into());
    set("pageeven", (first > 0 && first % 2 == 0).into());
    set("sheet", sn.into());
    set("sheets", ctx.sheet_total.max(sn).into());
    set("odd", (sn % 2 == 1).into());
    set("even", (sn % 2 == 0).into());
    set("first", (sn == 1).into());
    set("last", (sn >= ctx.sheet_total).into());
    set("front", (!back).into());
    set("back", back.into());
    set("side", if back { "back" } else { "front" }.into());
    set("cell", cell.map_or(0, |c| c + 1).into());
    set("cells", sheet.map_or(0, |sh| sh.placements.len()).into());
    set("copy", ctx.copy.max(1).into());
    set("copies", ctx.copies.max(1).into());
    set("layout", format!("{:?}", s.layout).to_lowercase().into());
    set("tilerow", poster.map_or(0, |p| p.ty as usize + 1).into());
    set("tilecol", poster.map_or(0, |p| p.tx as usize + 1).into());
    set("tile", poster.map_or(String::new(), |p| format!("{}{}", (b'A' + (p.ty % 26) as u8) as char, p.tx + 1)).into());
    set("name", os(fpath.file_stem()).into());
    set("ext", os(fpath.extension()).into());
    set("file", file.clone().into());
    set("folder", os(path.parent().and_then(|p| p.file_name())).into());
    set("path", ctx.path.clone().into());
    set("printer", ctx.printer.clone().into());
    set("user", env("USERNAME").into());
    set("computer", env("COMPUTERNAME").into());
    set("date", c.date().into());
    set("time", c.time().into());
    set("datetime", format!("{} {}", c.date(), c.time()).into());
    set("year", (c.year as usize).into());
    set("month", (c.month as usize).into());
    set("day", (c.day as usize).into());
    set("hour", (c.hour as usize).into());
    set("minute", (c.minute as usize).into());
    set("second", (c.second as usize).into());
    set("weekday", c.weekday_name().into());
    set("monthname", c.month_name().into());
    v
}

fn inset(r: Rect, m: [f32; 4]) -> Rect {
    Rect::new(r.x + m[3], r.y + m[0], (r.w - m[1] - m[3]).max(1.0), (r.h - m[0] - m[2]).max(1.0))
}

fn px_rect(r: Rect, mmpx: f32) -> [i64; 4] {
    [(r.x * mmpx).floor() as i64, (r.y * mmpx).floor() as i64, (r.right() * mmpx).ceil() as i64, (r.bottom() * mmpx).ceil() as i64]
}

/// Rectangles an overlay goes on: the whole sheet, or every drawn page with its placement index.
fn overlay_targets(scope: StampScope, drawn: &[(usize, Rect)], sheet_mm: (f32, f32)) -> Vec<(Rect, Option<usize>)> {
    match scope {
        StampScope::Sheet => vec![(Rect::new(0.0, 0.0, sheet_mm.0, sheet_mm.1), None)],
        StampScope::EachPage => drawn.iter().enumerate().map(|(i, d)| (d.1, Some(i))).collect(),
    }
}

/// Area positioned overlays are aligned in: inside the margins, or slightly inside a page.
fn safe_area(t: Rect, cell: Option<usize>, s: &Settings) -> Rect {
    match cell {
        Some(_) => inset(t, [2.0; 4]),
        None => inset(t, s.margins_mm.map(|m| m.max(4.0))),
    }
}

/// Centre (px) that puts a box with half extents (ex, ey) px at the 9-point `pos` of `area`.
fn anchor_point(area: Rect, pos: u8, offset_mm: [f32; 2], ex: f32, ey: f32, mmpx: f32) -> (f32, f32) {
    let x = match pos % 3 {
        0 => area.x * mmpx + ex,
        1 => (area.x + area.w / 2.0) * mmpx,
        _ => area.right() * mmpx - ex,
    };
    let y = match pos / 3 {
        0 => area.y * mmpx + ey,
        1 => (area.y + area.h / 2.0) * mmpx,
        _ => area.bottom() * mmpx - ey,
    };
    (x + offset_mm[0] * mmpx, y + offset_mm[1] * mmpx)
}

fn font_or(fonts: &mut FontCache, name: &str, fallback: &str) -> Option<Arc<FontVec>> {
    fonts.get(if name.trim().is_empty() { fallback } else { name })
}

#[allow(clippy::too_many_arguments)]
fn draw_watermark(
    img: &mut RgbImage,
    sheet: &Sheet,
    drawn: &[(usize, Rect)],
    sheet_mm: (f32, f32),
    mmpx: f32,
    s: &Settings,
    fonts: &mut FontCache,
    ctx: &StampCtx,
) {
    let wm = &s.watermark;
    let picture = match wm.image.trim() {
        "" => None,
        path => fonts.image(path).map(|p| {
            let w = (wm.image_width_mm * mmpx).round().max(1.0) as u32;
            let h = ((w as f32 * p.height() as f32 / p.width().max(1) as f32).round() as u32).max(1);
            imageops::resize(&*p, w, h, imageops::FilterType::Triangle)
        }),
    };
    let font = font_or(fonts, &wm.font, &s.font);
    let px = wm.size_pt / 72.0 * 25.4 * mmpx;
    let scope = if wm.per_page { StampScope::EachPage } else { StampScope::Sheet };
    for (t, cell) in overlay_targets(scope, drawn, sheet_mm) {
        let vars = template_vars(ctx, Some(sheet), s, cell);
        if !template::condition(&wm.show_if, &vars) {
            continue;
        }
        let mask;
        let sprite = match (&picture, &font) {
            (Some(p), _) => Sprite::Image(p),
            (None, Some(f)) => {
                let Some(m) = text::block_mask(f, &template::render(&wm.text, &vars), px, 1) else { continue };
                mask = if wm.boxed { text::frame(&m, px * 0.18, px * 0.07) } else { m };
                Sprite::Mask(&mask, wm.color)
            }
            _ => continue,
        };
        let (ex, ey) = sprite.extent(wm.angle);
        let clip = cell.map(|_| px_rect(t, mmpx));
        let opacity = wm.opacity / 100.0;
        if wm.tile {
            let sx = (2.0 * ex + wm.tile_gap_mm[0] * mmpx).max(4.0);
            let sy = (2.0 * ey + wm.tile_gap_mm[1] * mmpx).max(4.0);
            let cx = (t.x + t.w / 2.0 + wm.offset_mm[0]) * mmpx;
            let cy = (t.y + t.h / 2.0 + wm.offset_mm[1]) * mmpx;
            let nx = (t.w * mmpx / sx / 2.0).ceil() as i32 + 1;
            let ny = (t.h * mmpx / sy / 2.0).ceil() as i32 + 1;
            for row in -ny..=ny {
                // every other row is shifted by half a step (brick pattern)
                let shift = if row.rem_euclid(2) == 1 { sx / 2.0 } else { 0.0 };
                for col in -nx..=nx {
                    let x = cx + col as f32 * sx + shift;
                    let y = cy + row as f32 * sy;
                    text::draw_sprite(img, &sprite, x, y, wm.angle, opacity, wm.behind, clip);
                }
            }
        } else {
            let (x, y) = anchor_point(safe_area(t, cell, s), wm.position, wm.offset_mm, ex, ey, mmpx);
            text::draw_sprite(img, &sprite, x, y, wm.angle, opacity, wm.behind, clip);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bands(
    img: &mut RgbImage,
    sheet: &Sheet,
    drawn: &[(usize, Rect)],
    sheet_mm: (f32, f32),
    mmpx: f32,
    s: &Settings,
    fonts: &mut FontCache,
    ctx: &StampCtx,
) {
    for (band, is_header) in [(&s.header, true), (&s.footer, false)] {
        if !band.enabled {
            continue;
        }
        let Some(f) = font_or(fonts, &band.font, &s.font) else { continue };
        let px = band.size_pt / 72.0 * 25.4 * mmpx;
        for (t, cell) in overlay_targets(band.scope, drawn, sheet_mm) {
            let vars = template_vars(ctx, Some(sheet), s, cell);
            if !template::condition(&band.show_if, &vars) {
                continue;
            }
            let (left, right, cy) = match cell {
                Some(_) => {
                    let d = if band.distance_mm > 0.0 { band.distance_mm } else { band.size_pt * 0.353 * 0.8 + 1.5 };
                    (t.x + 2.0, t.right() - 2.0, if is_header { t.y + d } else { t.bottom() - d })
                }
                None => {
                    let m = s.margins_mm;
                    let d = |edge: f32| if band.distance_mm > 0.0 { band.distance_mm } else { (edge / 2.0).max(3.0) };
                    (m[3].max(4.0), sheet_mm.0 - m[1].max(4.0), if is_header { d(m[0]) } else { sheet_mm.1 - d(m[2]) })
                }
            };
            let even = vars.get(if cell.is_some() { "pageeven" } else { "even" }).is_some_and(|v| v.truthy());
            let (l, r) = if band.mirror_even && even { (&band.right, &band.left) } else { (&band.left, &band.right) };
            let cy = cy * mmpx;
            let clip = cell.map(|_| px_rect(t, mmpx));
            let mut tallest = 0.0f32;
            for (txt, x, al) in [(l, left, 0u8), (&band.center, t.x + t.w / 2.0, 1), (r, right, 2)] {
                let Some(m) = text::block_mask(&f, &template::render(txt, &vars), px, al) else { continue };
                let (x, w) = (x * mmpx, m.width() as f32);
                let cx = match al {
                    0 => x + w / 2.0,
                    1 => x,
                    _ => x - w / 2.0,
                };
                text::draw_sprite(img, &Sprite::Mask(&m, band.color), cx, cy, 0.0, 1.0, false, clip);
                tallest = tallest.max(m.height() as f32);
            }
            if band.rule {
                let th = ((0.2 * mmpx).round() as i64).max(1);
                let half = tallest.max(px) / 2.0 + mmpx;
                let y = (if is_header { cy + half } else { cy - half }) as i64;
                fill_rect(img, (left * mmpx) as i64, y, (right * mmpx) as i64, y + th, band.color);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_stamps(
    img: &mut RgbImage,
    sheet: &Sheet,
    drawn: &[(usize, Rect)],
    sheet_mm: (f32, f32),
    mmpx: f32,
    s: &Settings,
    fonts: &mut FontCache,
    ctx: &StampCtx,
) {
    for st in s.stamps.iter().filter(|st| st.enabled && !st.text.trim().is_empty()) {
        let Some(f) = font_or(fonts, &st.font, &s.font) else { continue };
        let px = st.size_pt / 72.0 * 25.4 * mmpx;
        for (t, cell) in overlay_targets(st.scope, drawn, sheet_mm) {
            let vars = template_vars(ctx, Some(sheet), s, cell);
            if !template::condition(&st.show_if, &vars) {
                continue;
            }
            let Some(m) = text::block_mask(&f, &template::render(&st.text, &vars), px, st.position % 3) else { continue };
            let m = if st.boxed { text::frame(&m, px * 0.25, px * 0.08) } else { m };
            let sprite = Sprite::Mask(&m, st.color);
            let (ex, ey) = sprite.extent(st.angle);
            let (x, y) = anchor_point(safe_area(t, cell, s), st.position, st.offset_mm, ex, ey, mmpx);
            let clip = cell.map(|_| px_rect(t, mmpx));
            text::draw_sprite(img, &sprite, x, y, st.angle, st.opacity / 100.0, st.behind, clip);
        }
    }
}

pub fn compose_sheet(
    sheet: &Sheet,
    sheet_mm: (f32, f32),
    dpi: f32,
    s: &Settings,
    r: &mut dyn PageRenderer,
    fonts: &mut FontCache,
    ctx: &StampCtx,
) -> RgbImage {
    let mmpx = dpi / 25.4;
    let w = ((sheet_mm.0 * mmpx).round() as u32).max(1);
    let h = ((sheet_mm.1 * mmpx).round() as u32).max(1);
    let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
    let lut = Lut::new(s);
    let font = fonts.get(&s.font);
    let pt = |p: f32| p / 72.0 * dpi;

    let mut drawn = Vec::new();
    for pl in &sheet.placements {
        let d = draw_placement(&mut img, r, pl, s, mmpx, &lut);
        drawn.push((pl.page, d.intersect(&pl.cell).unwrap_or(d)));
    }

    // --- watermark
    if s.watermark.enabled && (!s.watermark.text.trim().is_empty() || !s.watermark.image.trim().is_empty()) {
        draw_watermark(&mut img, sheet, &drawn, sheet_mm, mmpx, s, fonts, ctx);
    }

    // --- cell borders, labels, crop marks
    for (page, d) in &drawn {
        if s.cell_border {
            stroke_rect(&mut img, *d, mmpx, s.border_width_mm, [0, 0, 0]);
        }
        if s.crop_marks {
            crop_marks(&mut img, *d, mmpx);
        }
        if s.page_labels {
            if let Some(f) = &font {
                let label = format!("{}", page + 1);
                let x = (d.right() - 1.5) * mmpx;
                let y = (d.bottom() - 1.5) * mmpx - pt(7.0) * 1.3;
                text::draw_text(&mut img, f, &label, pt(7.0), x, y, 2, [90, 90, 90]);
            }
        }
    }

    // --- poster guides
    if let (true, Some(pt_)) = (s.poster_guides, &sheet.poster) {
        let a = pt_.area;
        let t = ((0.2 * mmpx) as i64).max(1);
        let dash = ((2.0 * mmpx) as i64).max(2);
        let col = [120, 120, 120];
        if pt_.tx + 1 < pt_.nx {
            let x = ((a.right() - pt_.overlap) * mmpx) as i64;
            dashed_line(&mut img, x, (a.y * mmpx) as i64, x, (a.bottom() * mmpx) as i64, t, dash, col);
        }
        if pt_.ty + 1 < pt_.ny {
            let y = ((a.bottom() - pt_.overlap) * mmpx) as i64;
            dashed_line(&mut img, (a.x * mmpx) as i64, y, (a.right() * mmpx) as i64, y, t, dash, col);
        }
        stroke_rect(&mut img, a, mmpx, 0.15, [170, 170, 170]);
        if let Some(f) = &font {
            let label = format!("{}{}  ({} x {})", (b'A' + (pt_.ty % 26) as u8) as char, pt_.tx + 1, pt_.nx, pt_.ny);
            text::draw_text(&mut img, f, &label, pt(7.0), (a.x + 1.5) * mmpx, (a.y + 1.0) * mmpx, 0, [120, 120, 120]);
        }
    }

    // --- header / footer, stamps
    draw_bands(&mut img, sheet, &drawn, sheet_mm, mmpx, s, fonts, ctx);
    draw_stamps(&mut img, sheet, &drawn, sheet_mm, mmpx, s, fonts, ctx);

    apply_color_mode(&mut img, s);
    img
}

fn luma(p: &Rgb<u8>) -> f32 {
    p.0[0] as f32 * 0.299 + p.0[1] as f32 * 0.587 + p.0[2] as f32 * 0.114
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

pub fn apply_color_mode(img: &mut RgbImage, s: &Settings) {
    match s.color_mode {
        ColorMode::Color => {}
        ColorMode::Grayscale => {
            for p in img.pixels_mut() {
                let l = luma(p).round() as u8;
                *p = Rgb([l, l, l]);
            }
        }
        ColorMode::BlackWhite => {
            let t = s.threshold as f32;
            for p in img.pixels_mut() {
                let v = if luma(p) >= t { 255 } else { 0 };
                *p = Rgb([v, v, v]);
            }
        }
        ColorMode::Halftone => {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let t = (BAYER8[(y % 8) as usize][(x % 8) as usize] as f32 + 0.5) * 4.0;
                let v = if luma(p) >= t { 255 } else { 0 };
                *p = Rgb([v, v, v]);
            }
        }
        ColorMode::Dither => {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let mut cur: Vec<f32> = vec![0.0; w + 2];
            let mut next: Vec<f32> = vec![0.0; w + 2];
            for y in 0..h {
                std::mem::swap(&mut cur, &mut next);
                next.iter_mut().for_each(|v| *v = 0.0);
                for x in 0..w {
                    let p = img.get_pixel(x as u32, y as u32);
                    let old = luma(p) + cur[x + 1];
                    let new = if old >= 128.0 { 255.0 } else { 0.0 };
                    let err = old - new;
                    cur[x + 2] += err * 7.0 / 16.0;
                    next[x] += err * 3.0 / 16.0;
                    next[x + 1] += err * 5.0 / 16.0;
                    next[x + 2] += err * 1.0 / 16.0;
                    let v = new as u8;
                    img.put_pixel(x as u32, y as u32, Rgb([v, v, v]));
                }
            }
        }
    }
}

/// Approximate ink/toner coverage of a sheet in percent.
pub fn coverage(img: &RgbImage) -> f32 {
    let mut sum = 0.0f64;
    let step = 2;
    let mut n = 0u64;
    for y in (0..img.height()).step_by(step) {
        for x in (0..img.width()).step_by(step) {
            sum += (255.0 - luma(img.get_pixel(x, y)) as f64) / 255.0;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64 * 100.0) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Stamp;

    struct Blank;
    impl PageRenderer for Blank {
        fn page_size_mm(&self, _: usize) -> (f32, f32) {
            (100.0, 140.0)
        }
        fn render(&mut self, _: usize, _: [f32; 4], w: u32, h: u32) -> Option<RgbaImage> {
            Some(RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255])))
        }
        fn content_bbox(&mut self, _: usize) -> [f32; 4] {
            [0.0, 0.0, 1.0, 1.0]
        }
    }

    fn ink(s: &Settings, sheet_no: usize) -> usize {
        let cell = Rect::new(10.0, 10.0, 100.0, 140.0);
        let sheet = Sheet { placements: vec![Placement { page: 2, cell, frame: cell, halign: None }], back: false, poster: None };
        let ctx = StampCtx { sheet_no, sheet_total: 3, doc_pages: 5, ..Default::default() };
        let img = compose_sheet(&sheet, (120.0, 160.0), 100.0, s, &mut Blank, &mut FontCache::default(), &ctx);
        img.pixels().filter(|p| p.0[0] < 200).count()
    }

    #[test]
    fn show_when_controls_overlays() {
        let mut s = Settings::default();
        s.footer.enabled = true;
        s.footer.scope = StampScope::EachPage;
        s.footer.center = "{page} / {pages}".into();
        s.footer.show_if = "not first".into();
        assert_eq!(ink(&s, 1), 0);
        assert!(ink(&s, 2) > 0);
        s.footer.enabled = false;
        s.stamps.push(Stamp { text: "{if even}EVEN{end}".into(), ..Default::default() });
        assert_eq!(ink(&s, 1), 0);
        assert!(ink(&s, 2) > 0);
    }
}
