//! Minimal text rasterizer on top of ab_glyph, using fonts installed in Windows.

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use image::{imageops, GrayImage, Luma, RgbImage, RgbaImage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn fonts_dir() -> PathBuf {
    let win = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(win).join("Fonts")
}

pub fn list_fonts() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fonts_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| {
                    let l = n.to_lowercase();
                    l.ends_with(".ttf") || l.ends_with(".otf")
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

#[derive(Default)]
pub struct FontCache {
    fonts: HashMap<String, Option<Arc<FontVec>>>,
    images: HashMap<String, Option<Arc<RgbaImage>>>,
}

impl FontCache {
    pub fn get(&mut self, name: &str) -> Option<Arc<FontVec>> {
        if let Some(f) = self.fonts.get(name) {
            return f.clone();
        }
        let load = |n: &str| {
            std::fs::read(fonts_dir().join(n))
                .ok()
                .and_then(|d| FontVec::try_from_vec(d).ok())
                .map(Arc::new)
        };
        let f = load(name).or_else(|| load("arial.ttf")).or_else(|| load("segoeui.ttf"));
        self.fonts.insert(name.to_string(), f.clone());
        f
    }

    /// Picture used by image watermarks (PNG).
    pub fn image(&mut self, path: &str) -> Option<Arc<RgbaImage>> {
        self.images
            .entry(path.to_string())
            .or_insert_with(|| image::open(path).ok().map(|i| Arc::new(i.to_rgba8())))
            .clone()
    }
}

/// Render a single line of text into a coverage mask.
pub fn text_mask(font: &FontVec, text: &str, px: f32) -> GrayImage {
    let scale = PxScale::from(px.max(1.0));
    let sf = font.as_scaled(scale);
    let mut x = 1.0f32;
    let mut glyphs = Vec::new();
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(scale, point(x, sf.ascent() + 1.0)));
        x += sf.h_advance(id);
        prev = Some(id);
    }
    let w = (x.ceil() as u32 + 2).max(1);
    let h = ((sf.ascent() - sf.descent()).ceil() as u32 + 2).max(1);
    let mut img = GrayImage::new(w, h);
    for g in glyphs {
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let v = (cov.clamp(0.0, 1.0) * 255.0) as u8;
                    let p = img.get_pixel_mut(px as u32, py as u32);
                    p.0[0] = p.0[0].max(v);
                }
            });
        }
    }
    img
}

fn sample(mask: &GrayImage, x: f32, y: f32) -> f32 {
    let (w, h) = mask.dimensions();
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return 0.0;
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let get = |xx: i32, yy: i32| -> f32 {
        if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
            0.0
        } else {
            let Luma([v]) = *mask.get_pixel(xx as u32, yy as u32);
            v as f32 / 255.0
        }
    };
    let a = get(x0, y0) * (1.0 - fx) + get(x0 + 1, y0) * fx;
    let b = get(x0, y0 + 1) * (1.0 - fx) + get(x0 + 1, y0 + 1) * fx;
    a * (1.0 - fy) + b * fy
}

/// Render text into one coverage mask; lines are split on '\n' and aligned
/// 0 = left, 1 = centred, 2 = right. `None` for blank text.
pub fn block_mask(font: &FontVec, text: &str, px: f32, align: u8) -> Option<GrayImage> {
    if text.trim().is_empty() {
        return None;
    }
    let mut lines: Vec<GrayImage> = text.split('\n').map(|l| text_mask(font, l.trim_end_matches('\r'), px)).collect();
    if lines.len() == 1 {
        return lines.pop();
    }
    let lh = lines.iter().map(|m| m.height()).max().unwrap_or(1);
    let w = lines.iter().map(|m| m.width()).max().unwrap_or(1);
    let mut out = GrayImage::new(w, lh * lines.len() as u32);
    for (k, m) in lines.iter().enumerate() {
        let x = match align {
            0 => 0,
            1 => (w - m.width()) / 2,
            _ => w - m.width(),
        };
        imageops::replace(&mut out, m, x as i64, (k as u32 * lh) as i64);
    }
    Some(out)
}

/// Surround a mask with a rectangular frame (rubber-stamp look).
pub fn frame(m: &GrayImage, pad: f32, thick: f32) -> GrayImage {
    let t = thick.round().max(1.0) as u32;
    let p = pad.round().max(1.0) as u32 + t;
    let (w, h) = (m.width() + 2 * p, m.height() + 2 * p);
    let mut out = GrayImage::new(w, h);
    imageops::replace(&mut out, m, p as i64, p as i64);
    for (x, y, v) in out.enumerate_pixels_mut() {
        if x < t || y < t || x >= w - t || y >= h - t {
            *v = Luma([255]);
        }
    }
    out
}

/// Something that can be stamped onto a sheet.
pub enum Sprite<'a> {
    /// Coverage mask painted in a flat colour.
    Mask(&'a GrayImage, [u8; 3]),
    Image(&'a RgbaImage),
}

impl Sprite<'_> {
    fn size(&self) -> (f32, f32) {
        match self {
            Sprite::Mask(m, _) => (m.width() as f32, m.height() as f32),
            Sprite::Image(i) => (i.width() as f32, i.height() as f32),
        }
    }

    /// Half width / height of the bounding box after rotating by `angle_deg`.
    pub fn extent(&self, angle_deg: f32) -> (f32, f32) {
        let (sin, cos) = angle_deg.to_radians().sin_cos();
        let (w, h) = self.size();
        (cos.abs() * w / 2.0 + sin.abs() * h / 2.0, sin.abs() * w / 2.0 + cos.abs() * h / 2.0)
    }
}

/// Bilinear sample of premultiplied RGBA.
fn sample_rgba(img: &RgbaImage, x: f32, y: f32) -> ([f32; 3], f32) {
    let (w, h) = img.dimensions();
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let mut rgb = [0.0f32; 3];
    let mut a = 0.0f32;
    for (dx, dy, k) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
        let (xx, yy) = (x0 + dx, y0 + dy);
        if k <= 0.0 || xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
            continue;
        }
        let p = img.get_pixel(xx as u32, yy as u32).0;
        let pa = p[3] as f32 / 255.0 * k;
        for c in 0..3 {
            rgb[c] += p[c] as f32 * pa;
        }
        a += pa;
    }
    (rgb, a)
}

/// Composite a sprite centred at (cx, cy), rotated counter-clockwise by `angle_deg`.
/// `behind` only tints light (paper) areas so content stays legible.
/// `clip` limits drawing to [x0, y0, x1, y1) in pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw_sprite(
    dst: &mut RgbImage,
    sprite: &Sprite,
    cx: f32,
    cy: f32,
    angle_deg: f32,
    opacity: f32,
    behind: bool,
    clip: Option<[i64; 4]>,
) {
    let a = -angle_deg.to_radians();
    let (sin, cos) = a.sin_cos();
    let (mw, mh) = sprite.size();
    let (ex, ey) = sprite.extent(angle_deg);
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let [l, t, r, b] = clip.unwrap_or([0, 0, dw, dh]);
    let x0 = ((cx - ex).floor() as i64).max(0).max(l);
    let x1 = ((cx + ex).ceil() as i64).min(dw - 1).min(r - 1);
    let y0 = ((cy - ey).floor() as i64).max(0).max(t);
    let y1 = ((cy + ey).ceil() as i64).min(dh - 1).min(b - 1);
    let opacity = opacity.clamp(0.0, 1.0);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let mx = cos * dx + sin * dy + mw / 2.0 - 0.5;
            let my = -sin * dx + cos * dy + mh / 2.0 - 0.5;
            let (color, cov) = match sprite {
                Sprite::Mask(m, c) => ([c[0] as f32, c[1] as f32, c[2] as f32], sample(m, mx, my)),
                Sprite::Image(img) => {
                    let (pre, a) = sample_rgba(img, mx, my);
                    if a <= 0.0 {
                        continue;
                    }
                    ([pre[0] / a, pre[1] / a, pre[2] / a], a)
                }
            };
            if cov <= 0.0 {
                continue;
            }
            let p = dst.get_pixel_mut(x as u32, y as u32);
            let mut alpha = cov * opacity;
            if behind {
                let lum = (p.0[0] as f32 * 0.299 + p.0[1] as f32 * 0.587 + p.0[2] as f32 * 0.114) / 255.0;
                alpha *= lum.powi(6);
            }
            for c in 0..3 {
                p.0[c] = (p.0[c] as f32 * (1.0 - alpha) + color[c] * alpha).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Composite a coverage mask centred at (cx, cy), rotated counter-clockwise by `angle_deg`.
#[allow(clippy::too_many_arguments)]
pub fn draw_mask(
    dst: &mut RgbImage,
    mask: &GrayImage,
    cx: f32,
    cy: f32,
    angle_deg: f32,
    color: [u8; 3],
    opacity: f32,
    behind: bool,
) {
    draw_sprite(dst, &Sprite::Mask(mask, color), cx, cy, angle_deg, opacity, behind, None);
}

/// Draw text with its top edge at `y`; `align` 0 = left edge at x, 1 = centred on x, 2 = right edge at x.
pub fn draw_text(dst: &mut RgbImage, font: &FontVec, text: &str, px: f32, x: f32, y: f32, align: u8, color: [u8; 3]) {
    if text.is_empty() {
        return;
    }
    let m = text_mask(font, text, px);
    let w = m.width() as f32;
    let cx = match align {
        0 => x + w / 2.0,
        1 => x,
        _ => x - w / 2.0,
    };
    draw_mask(dst, &m, cx, y + m.height() as f32 / 2.0, 0.0, color, 1.0, false);
}
