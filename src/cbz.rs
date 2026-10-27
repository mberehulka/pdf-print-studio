//! Comic book archives (.cbz): a zip of page images, read in reading order.

use crate::settings::{CbzOptions, CbzSort};
use image::imageops::FilterType;
use image::{ImageReader, RgbaImage};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::rc::Rc;
use zip::ZipArchive;

const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

pub struct CbzDoc {
    zip: RefCell<ZipArchive<File>>,
    /// Archive index of each page, in reading order.
    entries: Vec<usize>,
    sizes_px: Vec<(u32, u32)>,
    dpi: f32,
    decoded: RefCell<VecDeque<(usize, Rc<RgbaImage>)>>,
    /// Fields from ComicInfo.xml, if the archive has one.
    pub meta: Vec<(String, String)>,
}

/// Compare names so that "page2" sorts before "page10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut a), num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn is_page(name: &str, exclude: &[String]) -> bool {
    let lower = name.to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = file.rsplit('.').next().unwrap_or("");
    !name.ends_with('/')
        && !lower.starts_with("__macosx/")
        && !file.starts_with('.')
        && IMAGE_EXT.contains(&ext)
        && !exclude.iter().any(|x| file.contains(x.as_str()))
}

/// Pixel size from the start of the image data, falling back to the whole entry.
fn dimensions(zip: &mut ZipArchive<File>, index: usize) -> Option<(u32, u32)> {
    let mut head = Vec::new();
    zip.by_index(index).ok()?.take(256 * 1024).read_to_end(&mut head).ok()?;
    let probe = |data: &[u8]| ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?.into_dimensions().ok();
    probe(&head).or_else(|| {
        let mut all = Vec::new();
        zip.by_index(index).ok()?.read_to_end(&mut all).ok()?;
        probe(&all)
    })
}

fn comic_info(zip: &mut ZipArchive<File>) -> Vec<(String, String)> {
    let Some(i) = (0..zip.len()).find(|&i| zip.name_for_index(i).is_some_and(|n| n.eq_ignore_ascii_case("ComicInfo.xml"))) else {
        return Vec::new();
    };
    let mut xml = String::new();
    if zip.by_index(i).map(|mut f| f.read_to_string(&mut xml)).is_err() {
        return Vec::new();
    }
    let tags = ["Series", "Number", "Title", "Volume", "Year", "Writer", "Penciller", "Publisher", "Manga", "LanguageISO", "Summary"];
    tags.iter()
        .filter_map(|t| {
            let start = xml.find(&format!("<{t}>"))? + t.len() + 2;
            let end = start + xml[start..].find(&format!("</{t}>"))?;
            let v = xml[start..end].trim().replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
            (!v.is_empty()).then(|| (t.to_string(), v))
        })
        .collect()
}

impl CbzDoc {
    pub fn open(path: &Path, opts: &CbzOptions) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut zip = ZipArchive::new(file).map_err(|e| e.to_string())?;
        let exclude: Vec<String> =
            opts.exclude.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        let mut pages: Vec<(usize, String)> = (0..zip.len())
            .filter_map(|i| zip.name_for_index(i).map(|n| (i, n.to_string())))
            .filter(|(_, n)| is_page(n, &exclude))
            .collect();
        match opts.sort {
            CbzSort::Natural => pages.sort_by(|a, b| natural_cmp(&a.1, &b.1)),
            CbzSort::Name => pages.sort_by(|a, b| a.1.cmp(&b.1)),
            CbzSort::Archive => {}
        }
        if opts.reverse {
            pages.reverse();
        }
        let mut entries = Vec::new();
        let mut sizes_px = Vec::new();
        for (i, _) in pages {
            let Some((w, h)) = dimensions(&mut zip, i) else { continue };
            if w.min(h) < opts.min_px {
                continue;
            }
            entries.push(i);
            sizes_px.push((w.max(1), h.max(1)));
        }
        if entries.is_empty() {
            return Err("No page images found in the archive".into());
        }
        let meta = comic_info(&mut zip);
        Ok(Self {
            zip: RefCell::new(zip),
            entries,
            sizes_px,
            dpi: opts.dpi.clamp(36.0, 2400.0),
            decoded: RefCell::new(VecDeque::new()),
            meta,
        })
    }

    pub fn page_count(&self) -> usize {
        self.entries.len()
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        let k = 25.4 / self.dpi;
        self.sizes_px.iter().map(|&(w, h)| (w as f32 * k, h as f32 * k)).collect()
    }

    fn image(&self, page: usize) -> Result<Rc<RgbaImage>, String> {
        if let Some((_, img)) = self.decoded.borrow().iter().find(|(p, _)| *p == page) {
            return Ok(img.clone());
        }
        let index = *self.entries.get(page).ok_or("page out of range")?;
        let mut data = Vec::new();
        self.zip.borrow_mut().by_index(index).map_err(|e| e.to_string())?.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let img = ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let img = Rc::new(img);
        let mut cache = self.decoded.borrow_mut();
        cache.push_back((page, img.clone()));
        while cache.len() > 3 {
            cache.pop_front();
        }
        Ok(img)
    }

    /// Render `src` (normalised rect x0,y0,x1,y1) of a page to a `w`x`h` bitmap.
    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        let img = self.image(page)?;
        let (iw, ih) = img.dimensions();
        let x0 = ((src[0] * iw as f32) as u32).min(iw - 1);
        let y0 = ((src[1] * ih as f32) as u32).min(ih - 1);
        let x1 = ((src[2] * iw as f32).ceil() as u32).clamp(x0 + 1, iw);
        let y1 = ((src[3] * ih as f32).ceil() as u32).clamp(y0 + 1, ih);
        let view = image::imageops::crop_imm(img.as_ref(), x0, y0, x1 - x0, y1 - y0).to_image();
        let (w, h) = (w.max(1), h.max(1));
        // small targets (thumbnails) favour speed, print-size targets quality
        Ok(if w * 4 < view.width() {
            image::imageops::thumbnail(&view, w, h)
        } else {
            image::imageops::resize(&view, w, h, FilterType::CatmullRom)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["p10.jpg", "p2.jpg", "P1.jpg", "p02b.jpg", "cover.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["cover.jpg", "P1.jpg", "p2.jpg", "p02b.jpg", "p10.jpg"]);
    }

    #[test]
    fn page_filter() {
        let ex = vec!["credit".to_string()];
        assert!(is_page("Vol 1/001.JPG", &ex));
        assert!(!is_page("__MACOSX/001.jpg", &ex));
        assert!(!is_page("Vol 1/.thumb.jpg", &ex));
        assert!(!is_page("ComicInfo.xml", &ex));
        assert!(!is_page("zz_credits.png", &ex));
    }
}
