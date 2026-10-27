//! Pictures (.jpg, .png, .tif, .heic…): one image, several images or a whole folder, each picture
//! (or each page of a multi-page TIFF / frame of a GIF) becoming a page.
//!
//! Decoding goes through Windows' own codecs first (colour management, CMYK JPEGs, TIFF pages and
//! whatever extensions are installed: HEIF, AVIF, camera RAW…) and falls back to the `image` crate
//! for formats Windows lacks (QOI, TGA, PNM…).

use crate::cbz::natural_cmp;
use crate::settings::{ImageOptions, ImageSort, PhotoFit, PhotoSize};
use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader, Rgba, RgbaImage};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use windows::core::HSTRING;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapFrame, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode,
};
use windows::Storage::{FileAccessMode, StorageFile};

/// Extensions opened as pictures. Windows decodes the camera RAW, HEIF and AVIF ones only when the
/// matching extension from the Microsoft Store is installed.
pub const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "dib", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr",
    "wdp", "ico", "tga", "pnm", "pbm", "pgm", "ppm", "qoi", "hdr", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf",
];

pub fn is_image(path: &Path) -> bool {
    path.extension().is_some_and(|e| IMAGE_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// The pictures in a folder (not its subfolders).
pub fn folder_images(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_image(p)).collect())
        .unwrap_or_default()
}

/// What is known about one page before decoding it.
struct Page {
    path: PathBuf,
    /// Frame (TIFF page, GIF frame) inside the file.
    frame: u32,
    /// Pixel size as stored, before `orient`.
    raw: (u32, u32),
    orient: Orientation,
    /// Windows applies the orientation itself (formats the `image` crate cannot read, e.g. HEIC).
    wic_orient: bool,
    /// Already decoded (GIF frames).
    pixels: Option<Rc<RgbaImage>>,
    /// Size on paper in mm.
    size_mm: (f32, f32),
    /// Part of the picture (normalised, may reach outside 0..1 to pad) that forms the page.
    region: [f32; 4],
}

impl Page {
    fn shown_px(&self) -> (u32, u32) {
        let (w, h) = self.oriented();
        let r = self.region;
        let part = |a: f32, b: f32, n: u32| ((b.min(1.0) - a.max(0.0)) * n as f32).round().max(1.0) as u32;
        (part(r[0], r[2], w), part(r[1], r[3], h))
    }

    fn oriented(&self) -> (u32, u32) {
        let (w, h) = self.raw;
        if swaps(self.orient) { (h, w) } else { (w, h) }
    }
}

fn swaps(o: Orientation) -> bool {
    matches!(o, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH)
}

/// Facts about a picture file, for the info panel, captions and sorting.
#[derive(Default, Clone)]
struct Info {
    format: String,
    dpi: Option<f32>,
    camera: String,
    /// "YYYY:MM:DD HH:MM:SS" when taken (EXIF), else empty.
    taken: String,
    /// Last modified, same format.
    modified: String,
    exposure: String,
}

pub struct ImageDoc {
    pages: Vec<Page>,
    background: [u8; 3],
    auto_levels: bool,
    color_manage: bool,
    decoded: RefCell<VecDeque<(usize, Rc<RgbaImage>)>>,
    /// Per page: file name and date (taken, else modified) for captions.
    pub labels: Vec<(String, String)>,
    /// Per page: pixels of the picture that end up on the page.
    pub px: Vec<(u32, u32)>,
    pub meta: Vec<(String, String)>,
}

impl ImageDoc {
    /// `files` are pictures or folders of pictures.
    pub fn open(files: &[PathBuf], opts: &ImageOptions) -> Result<Self, String> {
        let mut paths = Vec::new();
        for f in files {
            if f.is_dir() {
                paths.extend(folder_images(f));
            } else {
                paths.push(f.clone());
            }
        }
        if paths.is_empty() {
            return Err(crate::i18n::tr("No pictures found").into());
        }

        let mut probed: Vec<(PathBuf, Vec<Page>, Info)> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for p in paths {
            match probe(&p, opts) {
                Ok((pages, info)) if !pages.is_empty() => probed.push((p, pages, info)),
                Ok(_) => failed.push(format!("{}: no image data", file_name(&p))),
                Err(e) => failed.push(format!("{}: {e}", file_name(&p))),
            }
        }
        if probed.is_empty() {
            return Err(failed.join("\n"));
        }
        let when = |i: &Info| if i.taken.is_empty() { i.modified.clone() } else { i.taken.clone() };
        match opts.sort {
            ImageSort::Name => probed.sort_by(|a, b| natural_cmp(&file_name(&a.0), &file_name(&b.0))),
            ImageSort::Taken => probed.sort_by(|a, b| when(&a.2).cmp(&when(&b.2)).then_with(|| natural_cmp(&file_name(&a.0), &file_name(&b.0)))),
            ImageSort::Modified => probed.sort_by(|a, b| a.2.modified.cmp(&b.2.modified)),
            ImageSort::AsGiven => {}
        }
        if opts.reverse {
            probed.reverse();
        }

        let files_count = probed.len();
        let first_info = probed[0].2.clone();
        let mut pages = Vec::new();
        let mut labels = Vec::new();
        for (path, file_pages, info) in probed {
            let dpi = if opts.use_file_dpi { info.dpi.unwrap_or(opts.dpi) } else { opts.dpi }.clamp(10.0, 4800.0);
            for mut pg in file_pages {
                let (w, h) = pg.oriented();
                (pg.size_mm, pg.region) = place(w, h, dpi, opts);
                pages.push(pg);
                labels.push((file_name(&path), pretty_date(&when(&info))));
            }
        }

        let mut meta = Vec::new();
        if files_count == 1 {
            let (w, h) = pages[0].oriented();
            let info = &first_info;
            meta.push(("Format".into(), info.format.clone()));
            meta.push(("Pixels".into(), format!("{w} × {h} ({:.1} MP)", w as f32 * h as f32 / 1e6)));
            if let Some(d) = info.dpi {
                meta.push(("Resolution".into(), format!("{d:.0} dpi")));
            }
            if pages.len() > 1 {
                meta.push(("Pages".into(), pages.len().to_string()));
            }
            if !info.camera.is_empty() {
                meta.push(("Camera".into(), info.camera.clone()));
            }
            if !info.exposure.is_empty() {
                meta.push(("Exposure".into(), info.exposure.clone()));
            }
            if !info.taken.is_empty() {
                meta.push(("Taken".into(), pretty_date(&info.taken)));
            }
        } else {
            meta.push(("Pictures".into(), files_count.to_string()));
        }
        if !failed.is_empty() {
            meta.push(("Skipped".into(), failed.join("\n")));
        }

        Ok(Self {
            px: pages.iter().map(Page::shown_px).collect(),
            pages,
            background: opts.background,
            auto_levels: opts.auto_levels,
            color_manage: opts.color_manage,
            decoded: RefCell::new(VecDeque::new()),
            labels,
            meta,
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        self.pages.iter().map(|p| p.size_mm).collect()
    }

    /// The oriented picture of `page`, at least `scale` × its full resolution.
    fn image(&self, page: usize, scale: f32) -> Result<Rc<RgbaImage>, String> {
        let p = self.pages.get(page).ok_or("page out of range")?;
        let full = p.oriented().0 as f32;
        if let Some((_, img)) = self.decoded.borrow().iter().find(|(i, img)| *i == page && img.width() as f32 >= (full * scale).min(full) - 1.0) {
            return Ok(img.clone());
        }
        // small requests (thumbnails) decode a reduced copy; a little extra keeps it sharp
        let scale = if scale < 0.5 { (scale * 1.25).min(1.0) } else { 1.0 };
        let mut img = match &p.pixels {
            Some(px) => px.as_ref().clone(),
            None => decode(p, scale, self.color_manage)?,
        };
        if self.auto_levels {
            auto_levels(&mut img);
        }
        let img = Rc::new(img);
        let mut cache = self.decoded.borrow_mut();
        cache.retain(|(i, _)| *i != page);
        cache.push_back((page, img.clone()));
        while cache.len() > 4 {
            cache.pop_front();
        }
        Ok(img)
    }

    /// Render `src` (normalised rect of the page) to a `w`x`h` bitmap.
    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        let p = self.pages.get(page).ok_or("page out of range")?;
        let (w, h) = (w.max(1), h.max(1));
        let r = p.region;
        let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
        // the requested part in normalised picture coordinates
        let v = [r[0] + src[0] * rw, r[1] + src[1] * rh, r[0] + src[2] * rw, r[1] + src[3] * rh];
        let (fw, fh) = p.oriented();
        let scale = (w as f32 / ((v[2] - v[0]) * fw as f32).max(1.0)).max(h as f32 / ((v[3] - v[1]) * fh as f32).max(1.0));
        let img = self.image(page, scale.min(1.0))?;
        let (iw, ih) = img.dimensions();

        let bg = Rgba([self.background[0], self.background[1], self.background[2], 255]);
        let mut out = RgbaImage::from_pixel(w, h, bg);
        // visible part of the picture, in picture and in output pixels
        let (cx0, cy0, cx1, cy1) = (v[0].max(0.0), v[1].max(0.0), v[2].min(1.0), v[3].min(1.0));
        if cx1 <= cx0 || cy1 <= cy0 {
            return Ok(out);
        }
        let to_out = |a: f32, lo: f32, hi: f32, n: u32| (((a - lo) / (hi - lo)) * n as f32).round() as i64;
        let (ox0, ox1) = (to_out(cx0, v[0], v[2], w), to_out(cx1, v[0], v[2], w));
        let (oy0, oy1) = (to_out(cy0, v[1], v[3], h), to_out(cy1, v[1], v[3], h));
        let (ow, oh) = ((ox1 - ox0).max(1) as u32, (oy1 - oy0).max(1) as u32);
        let x0 = ((cx0 * iw as f32) as u32).min(iw - 1);
        let y0 = ((cy0 * ih as f32) as u32).min(ih - 1);
        let x1 = ((cx1 * iw as f32).ceil() as u32).clamp(x0 + 1, iw);
        let y1 = ((cy1 * ih as f32).ceil() as u32).clamp(y0 + 1, ih);
        let view = image::imageops::crop_imm(img.as_ref(), x0, y0, x1 - x0, y1 - y0).to_image();
        let part = if (ow, oh) == view.dimensions() {
            view
        } else if ow * 4 < view.width() {
            image::imageops::thumbnail(&view, ow, oh)
        } else {
            image::imageops::resize(&view, ow, oh, FilterType::CatmullRom)
        };
        // flatten transparency onto the background colour
        for (x, y, px) in part.enumerate_pixels() {
            let (tx, ty) = (ox0 + x as i64, oy0 + y as i64);
            if tx < 0 || ty < 0 || tx >= w as i64 || ty >= h as i64 {
                continue;
            }
            let a = px.0[3] as u32;
            let d = out.get_pixel_mut(tx as u32, ty as u32);
            for c in 0..3 {
                d.0[c] = ((px.0[c] as u32 * a + bg.0[c] as u32 * (255 - a)) / 255) as u8;
            }
        }
        Ok(out)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// "2026:10:27 14:05:09" → "2026-10-27 14:05".
fn pretty_date(d: &str) -> String {
    if d.len() < 16 {
        return d.to_string();
    }
    format!("{} {}", d[..10].replace(':', "-"), &d[11..16])
}

/// Photo size in mm (portrait), `None` for the picture's own size.
pub fn photo_mm(size: PhotoSize, custom: [f32; 2]) -> Option<(f32, f32)> {
    Some(match size {
        PhotoSize::Original => return None,
        PhotoSize::Passport => (35.0, 45.0),
        PhotoSize::UsPassport => (50.8, 50.8),
        PhotoSize::Id3x4 => (30.0, 40.0),
        PhotoSize::Wallet => (63.5, 88.9),
        PhotoSize::P9x13 => (89.0, 127.0),
        PhotoSize::P10x15 => (101.6, 152.4),
        PhotoSize::P13x18 => (127.0, 177.8),
        PhotoSize::P15x21 => (152.4, 203.2),
        PhotoSize::P20x25 => (203.2, 254.0),
        PhotoSize::P20x30 => (203.2, 304.8),
        PhotoSize::Square10 => (100.0, 100.0),
        PhotoSize::A6 => (105.0, 148.0),
        PhotoSize::A5 => (148.0, 210.0),
        PhotoSize::A4 => (210.0, 297.0),
        PhotoSize::Custom => (custom[0].clamp(5.0, 2000.0), custom[1].clamp(5.0, 2000.0)),
    })
}

/// Page size (mm) and the part of a `w`x`h` px picture shown on it.
fn place(w: u32, h: u32, dpi: f32, o: &ImageOptions) -> ((f32, f32), [f32; 4]) {
    let k = 25.4 / dpi;
    let Some((mut tw, mut th)) = photo_mm(o.size, o.custom_mm) else {
        return ((w as f32 * k, h as f32 * k), [0.0, 0.0, 1.0, 1.0]);
    };
    if o.match_orientation && (w > h) != (tw > th) && w != h {
        std::mem::swap(&mut tw, &mut th);
    }
    let (ia, ta) = (w as f32 / h.max(1) as f32, tw / th);
    // share of the picture's width / height that the page shows (> 1 pads with the background)
    let (sx, sy) = match o.fit {
        PhotoFit::Fill if ia > ta => (ta / ia, 1.0),
        PhotoFit::Fill => (1.0, ia / ta),
        PhotoFit::Fit if ia > ta => (1.0, ia / ta),
        PhotoFit::Fit => (ta / ia, 1.0),
    };
    // where the shown part sits: 9-point anchor, 4 = centre
    let at = |share: f32, a: u8| match a {
        0 => 0.0,
        1 => (1.0 - share) / 2.0,
        _ => 1.0 - share,
    };
    let anchor = o.anchor.min(8);
    let (x0, y0) = if o.fit == PhotoFit::Fill { (at(sx, anchor % 3), at(sy, anchor / 3)) } else { ((1.0 - sx) / 2.0, (1.0 - sy) / 2.0) };
    ((tw, th), [x0, y0, x0 + sx, y0 + sy])
}

// ---------------------------------------------------------------- probing

fn probe_info(path: &Path) -> Info {
    let mut info = Info { format: path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(), ..Default::default() };
    if let Ok(m) = std::fs::metadata(path).and_then(|m| m.modified()) {
        info.modified = local_time(m);
    }
    let mut head = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(256 * 1024).read_to_end(&mut head);
    }
    info.dpi = header_dpi(&head);
    let exif = ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.into_decoder().ok())
        .and_then(|mut d| d.exif_metadata().ok().flatten())
        .or_else(|| (head.starts_with(b"II*\0") || head.starts_with(b"MM\0*")).then(|| head.clone()));
    if let Some(e) = exif.as_deref().and_then(Exif::parse) {
        info.camera = e.camera();
        info.taken = e.taken.clone();
        info.exposure = e.exposure();
        if info.dpi.is_none() {
            info.dpi = e.dpi;
        }
    }
    info
}

/// Pages of one file, plus its info.
fn probe(path: &Path, o: &ImageOptions) -> Result<(Vec<Page>, Info), String> {
    let info = probe_info(path);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let page = |frame, raw, orient, wic_orient| Page {
        path: path.to_path_buf(),
        frame,
        raw,
        orient,
        wic_orient,
        pixels: None,
        size_mm: (1.0, 1.0),
        region: [0.0, 0.0, 1.0, 1.0],
    };

    // animated GIFs: every frame as a page, decoded now (frames build on each other)
    if ext == "gif" && o.all_frames {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let frames = GifDecoder::new(BufReader::new(file)).map_err(|e| e.to_string())?.into_frames();
        let mut pages = Vec::new();
        for (i, f) in frames.take(1000).enumerate() {
            let img = f.map_err(|e| e.to_string())?.into_buffer();
            let mut pg = page(i as u32, img.dimensions(), Orientation::NoTransforms, false);
            pg.pixels = Some(Rc::new(img));
            pages.push(pg);
        }
        return Ok((pages, info));
    }

    // what the `image` crate knows: size and EXIF orientation without decoding
    let known = ImageReader::open(path).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.into_decoder().ok()).map(|mut d| {
        let orient = if o.exif_rotate { d.orientation().unwrap_or(Orientation::NoTransforms) } else { Orientation::NoTransforms };
        (d.dimensions(), orient)
    });
    let multi = matches!(ext.as_str(), "tif" | "tiff") && o.all_frames;
    if let (Some((raw, orient)), false) = (known, multi) {
        return Ok((vec![page(0, raw, orient, false)], info));
    }
    // Windows codecs: TIFF pages, HEIC, AVIF, RAW…
    let dec = wic_decoder(path).map_err(|e| codec_hint(&ext, &e.message().to_string()))?;
    let count = if multi { dec.FrameCount().map_err(|e| e.message().to_string())?.clamp(1, 2000) } else { 1 };
    let mut pages = Vec::new();
    for i in 0..count {
        let f = dec.GetFrameAsync(i).and_then(|op| op.get()).map_err(|e| e.message().to_string())?;
        let size = |f: &BitmapFrame, oriented: bool| -> windows::core::Result<(u32, u32)> {
            Ok(if oriented { (f.OrientedPixelWidth()?, f.OrientedPixelHeight()?) } else { (f.PixelWidth()?, f.PixelHeight()?) })
        };
        match known {
            Some((_, orient)) if i == 0 => pages.push(page(i, size(&f, false).map_err(|e| e.message().to_string())?, orient, false)),
            _ => pages.push(page(i, size(&f, o.exif_rotate).map_err(|e| e.message().to_string())?, Orientation::NoTransforms, o.exif_rotate)),
        }
    }
    Ok((pages, info))
}

fn codec_hint(ext: &str, msg: &str) -> String {
    let store = match ext {
        "heic" | "heif" => "HEIF Image Extensions",
        "avif" => "AV1 Video Extension",
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "rw2" | "raf" => "Raw Image Extension",
        _ => return msg.to_string(),
    };
    crate::trf!("Windows cannot decode this picture. Install \"{}\" from the Microsoft Store. ({})", store, msg)
}

// ---------------------------------------------------------------- decoding

fn wic_decoder(path: &Path) -> windows::core::Result<BitmapDecoder> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.to_string_lossy().as_ref()))?.get()?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.get()?;
    BitmapDecoder::CreateAsync(&stream)?.get()
}

/// Decode with Windows, `scale` × full size (only honoured when we do the orienting ourselves).
fn wic_decode(p: &Page, scale: f32, color_manage: bool) -> windows::core::Result<RgbaImage> {
    let dec = wic_decoder(&p.path)?;
    let frame = dec.GetFrameAsync(p.frame)?.get()?;
    let t = BitmapTransform::new()?;
    let (mut w, mut h) = (frame.PixelWidth()?, frame.PixelHeight()?);
    if p.wic_orient {
        (w, h) = (frame.OrientedPixelWidth()?, frame.OrientedPixelHeight()?);
    } else if scale < 1.0 {
        (w, h) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        t.SetScaledWidth(w)?;
        t.SetScaledHeight(h)?;
        t.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    }
    let orient = if p.wic_orient { ExifOrientationMode::RespectExifOrientation } else { ExifOrientationMode::IgnoreExifOrientation };
    let cm = if color_manage { ColorManagementMode::ColorManageToSRgb } else { ColorManagementMode::DoNotColorManage };
    let data = frame.GetPixelDataTransformedAsync(BitmapPixelFormat::Rgba8, BitmapAlphaMode::Straight, &t, orient, cm)?.get()?.DetachPixelData()?;
    let bad = || windows::core::Error::new(windows::core::HRESULT(-1), "unexpected pixel data size");
    if data.len() != (w * h * 4) as usize {
        return Err(bad());
    }
    RgbaImage::from_raw(w, h, data.to_vec()).ok_or_else(bad)
}

fn decode(p: &Page, scale: f32, color_manage: bool) -> Result<RgbaImage, String> {
    let img = match wic_decode(p, scale, color_manage) {
        Ok(img) => img,
        Err(e) => {
            let img = ImageReader::open(&p.path)
                .map_err(|e2| format!("{}; {e2}", e.message()))?
                .with_guessed_format()
                .map_err(|e2| e2.to_string())?
                .decode()
                .map_err(|e2| format!("{}; {e2}", e.message()))?
                .to_rgba8();
            if scale < 1.0 {
                let (w, h) = img.dimensions();
                image::imageops::thumbnail(&img, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1))
            } else {
                img
            }
        }
    };
    if p.orient == Orientation::NoTransforms {
        return Ok(img);
    }
    let mut d = DynamicImage::ImageRgba8(img);
    d.apply_orientation(p.orient);
    Ok(d.into_rgba8())
}

/// Stretch each channel so the darkest and lightest 0.5% become black and white.
fn auto_levels(img: &mut RgbaImage) {
    let mut hist = [[0u32; 256]; 3];
    for p in img.pixels() {
        for c in 0..3 {
            hist[c][p.0[c] as usize] += 1;
        }
    }
    let n = (img.width() * img.height()) as u64;
    let cut = n / 200;
    let mut lut = [[0u8; 256]; 3];
    for c in 0..3 {
        let (mut lo, mut acc) = (0usize, 0u64);
        while lo < 255 && acc + hist[c][lo] as u64 <= cut {
            acc += hist[c][lo] as u64;
            lo += 1;
        }
        let (mut hi, mut acc) = (255usize, 0u64);
        while hi > 0 && acc + hist[c][hi] as u64 <= cut {
            acc += hist[c][hi] as u64;
            hi -= 1;
        }
        if hi <= lo + 16 {
            // flat picture: leave it alone
            lut[c] = std::array::from_fn(|i| i as u8);
            continue;
        }
        for (i, v) in lut[c].iter_mut().enumerate() {
            *v = ((i as f32 - lo as f32) * 255.0 / (hi - lo) as f32).round().clamp(0.0, 255.0) as u8;
        }
    }
    for p in img.pixels_mut() {
        for c in 0..3 {
            p.0[c] = lut[c][p.0[c] as usize];
        }
    }
}

// ---------------------------------------------------------------- metadata

/// Resolution stored in a JPEG (JFIF) or PNG (pHYs) header.
fn header_dpi(head: &[u8]) -> Option<f32> {
    let valid = |d: f32| (30.0..=4800.0).contains(&d).then_some(d);
    if head.starts_with(&[0xFF, 0xD8]) {
        let i = head.windows(5).position(|w| w == b"JFIF\0")?;
        let b = head.get(i + 7..i + 12)?;
        let x = u16::from_be_bytes([b[1], b[2]]) as f32;
        return match b[0] {
            1 => valid(x),
            2 => valid(x * 2.54),
            _ => None,
        };
    }
    if head.starts_with(b"\x89PNG") {
        let i = head.windows(4).position(|w| w == b"pHYs")?;
        let b = head.get(i + 4..i + 13)?;
        let ppm = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32;
        return if b[8] == 1 { valid(ppm * 0.0254) } else { None };
    }
    None
}

/// The few EXIF fields worth showing.
#[derive(Default, Debug, PartialEq)]
struct Exif {
    make: String,
    model: String,
    taken: String,
    dpi: Option<f32>,
    exposure_s: Option<f32>,
    f_number: Option<f32>,
    iso: Option<u32>,
    focal_mm: Option<f32>,
}

impl Exif {
    /// `data` is a TIFF structure, optionally preceded by "Exif\0\0".
    fn parse(data: &[u8]) -> Option<Exif> {
        let d = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
        let le = match d.get(..4)? {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        let u16_at = |o: usize| d.get(o..o + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
        let u32_at = |o: usize| d.get(o..o + 4).map(|b| {
            let a = [b[0], b[1], b[2], b[3]];
            if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
        });
        // (tag, type, count, offset of the value or of the pointer to it)
        let entries = |ifd: usize| -> Vec<(u16, u16, u32, usize)> {
            let n = u16_at(ifd).unwrap_or(0) as usize;
            (0..n.min(500)).filter_map(|i| {
                let e = ifd + 2 + i * 12;
                Some((u16_at(e)?, u16_at(e + 2)?, u32_at(e + 4)?, e + 8))
            })
            .collect()
        };
        let size = |ty: u16| match ty {
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        let value_at = |ty: u16, count: u32, at: usize| -> Option<usize> {
            if size(ty) * count as usize <= 4 { Some(at) } else { u32_at(at).map(|o| o as usize) }
        };
        let text = |ty: u16, count: u32, at: usize| -> String {
            value_at(ty, count, at)
                .and_then(|o| d.get(o..o + count as usize))
                .map(|b| String::from_utf8_lossy(b).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string())
                .unwrap_or_default()
        };
        let rational = |ty: u16, at: usize| -> Option<f32> {
            let o = value_at(ty, 1, at)?;
            let (n, dd) = (u32_at(o)? as f32, u32_at(o + 4)? as f32);
            (dd > 0.0).then(|| n / dd)
        };
        let short = |ty: u16, at: usize| -> Option<u32> {
            match ty {
                3 => u16_at(at).map(u32::from),
                4 => u32_at(at),
                _ => None,
            }
        };

        let mut e = Exif::default();
        let ifd0 = u32_at(4)? as usize;
        let (mut xres, mut unit, mut sub) = (None, 2, None);
        for (tag, ty, count, at) in entries(ifd0) {
            match tag {
                0x010F => e.make = text(ty, count, at),
                0x0110 => e.model = text(ty, count, at),
                0x011A => xres = rational(ty, at),
                0x0128 => unit = short(ty, at).unwrap_or(2),
                0x0132 if e.taken.is_empty() => e.taken = text(ty, count, at),
                0x8769 => sub = u32_at(at).map(|o| o as usize),
                _ => {}
            }
        }
        e.dpi = xres.map(|x| if unit == 3 { x * 2.54 } else { x }).filter(|&d| (30.0..=4800.0).contains(&d));
        if let Some(sub) = sub {
            for (tag, ty, count, at) in entries(sub) {
                match tag {
                    0x9003 => e.taken = text(ty, count, at),
                    0x829A => e.exposure_s = rational(ty, at),
                    0x829D => e.f_number = rational(ty, at),
                    0x8827 => e.iso = short(ty, at),
                    0x920A => e.focal_mm = rational(ty, at),
                    _ => {}
                }
            }
        }
        if e.taken.len() < 16 || e.taken.starts_with("0000") {
            e.taken.clear();
        }
        Some(e)
    }

    fn camera(&self) -> String {
        // models often repeat the make ("Canon" + "Canon EOS R6")
        if self.model.to_lowercase().starts_with(&self.make.to_lowercase()) {
            self.model.clone()
        } else {
            format!("{} {}", self.make, self.model).trim().to_string()
        }
    }

    fn exposure(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = self.exposure_s.filter(|&t| t > 0.0) {
            parts.push(if t < 0.5 { format!("1/{:.0} s", 1.0 / t) } else { format!("{t:.1} s") });
        }
        if let Some(f) = self.f_number {
            parts.push(format!("f/{f:.1}"));
        }
        if let Some(i) = self.iso {
            parts.push(format!("ISO {i}"));
        }
        if let Some(mm) = self.focal_mm {
            parts.push(format!("{mm:.0} mm"));
        }
        parts.join(" · ")
    }
}

/// A file time as local "YYYY:MM:DD HH:MM:SS".
fn local_time(t: std::time::SystemTime) -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn FileTimeToLocalFileTime(ft: *const u64, local: *mut u64) -> i32;
        fn FileTimeToSystemTime(ft: *const u64, st: *mut SystemTime) -> i32;
    }
    // FILETIME: 100 ns steps since 1601
    let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) else { return String::new() };
    let ft = d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100 + 116_444_736_000_000_000;
    let (mut local, mut st) = (0u64, SystemTime::default());
    if unsafe { FileTimeToLocalFileTime(&ft, &mut local) == 0 || FileTimeToSystemTime(&local, &mut st) == 0 } {
        return String::new();
    }
    format!("{:04}:{:02}:{:02} {:02}:{:02}:{:02}", st.year, st.month, st.day, st.hour, st.minute, st.second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(size: PhotoSize, fit: PhotoFit) -> ImageOptions {
        ImageOptions { size, fit, dpi: 254.0, ..Default::default() }
    }

    #[test]
    fn original_size_follows_dpi() {
        let (mm, r) = place(1000, 500, 25.4, &opts(PhotoSize::Original, PhotoFit::Fill));
        assert_eq!(mm, (1000.0, 500.0));
        assert_eq!(r, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn photo_size_fill_and_fit() {
        // 3:2 landscape picture on a 10×15 photo turned landscape: fill crops a little height
        let (mm, r) = place(3000, 2000, 300.0, &opts(PhotoSize::P10x15, PhotoFit::Fill));
        assert_eq!(mm, (152.4, 101.6));
        assert!((r[0] - 0.0).abs() < 1e-4 && (r[2] - 1.0).abs() < 1e-4);
        assert!((r[1] - 0.0).abs() < 1e-3 && (r[3] - 1.0).abs() < 1e-3);
        // square picture on a passport photo: fill crops the sides, fit pads top and bottom
        let (mm, r) = place(1000, 1000, 300.0, &opts(PhotoSize::Passport, PhotoFit::Fill));
        assert_eq!(mm, (35.0, 45.0));
        assert!((r[2] - r[0] - 35.0 / 45.0).abs() < 1e-4 && r[1] == 0.0 && r[3] == 1.0);
        let (_, r) = place(1000, 1000, 300.0, &opts(PhotoSize::Passport, PhotoFit::Fit));
        assert!(r[0] == 0.0 && r[2] == 1.0 && r[1] < 0.0 && r[3] > 1.0);
        // anchored at the top: the crop keeps the top of a tall picture
        let o = ImageOptions { anchor: 1, ..opts(PhotoSize::Square10, PhotoFit::Fill) };
        let (_, r) = place(1000, 2000, 300.0, &o);
        assert_eq!(r, [0.0, 0.0, 1.0, 0.5]);
    }

    #[test]
    fn header_resolution() {
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        jpg.extend_from_slice(b"JFIF\0\x01\x01\x01\x01\x2C\x01\x2C\0\0");
        assert_eq!(header_dpi(&jpg), Some(300.0));
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(b"\0\0\0\x09pHYs\0\0\x2E\x23\0\0\x2E\x23\x01");
        assert_eq!(header_dpi(&png).map(|d| d.round()), Some(300.0)); // 11811 px/m
    }

    #[test]
    fn exif_fields() {
        // little-endian TIFF: IFD0 with Make, XResolution and an EXIF sub-IFD with DateTimeOriginal and ISO
        let mut d = b"II*\0\x08\0\0\0".to_vec();
        let entry = |d: &mut Vec<u8>, tag: u16, ty: u16, count: u32, val: u32| {
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&ty.to_le_bytes());
            d.extend_from_slice(&count.to_le_bytes());
            d.extend_from_slice(&val.to_le_bytes());
        };
        // IFD0 at 8: 3 entries → ends at 8 + 2 + 36 + 4 = 50
        d.extend_from_slice(&3u16.to_le_bytes());
        entry(&mut d, 0x010F, 2, 6, 50); // "Canon\0"
        entry(&mut d, 0x011A, 5, 1, 56); // 300/1
        entry(&mut d, 0x8769, 4, 1, 64);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(b"Canon\0");
        d.extend_from_slice(&300u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        // sub-IFD at 64: 2 entries → ends at 64 + 2 + 24 + 4 = 94
        d.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut d, 0x9003, 2, 20, 94);
        entry(&mut d, 0x8827, 3, 1, 200);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(b"2024:05:17 09:30:00\0");
        let e = Exif::parse(&d).unwrap();
        assert_eq!(e.make, "Canon");
        assert_eq!(e.dpi, Some(300.0));
        assert_eq!(e.taken, "2024:05:17 09:30:00");
        assert_eq!(e.iso, Some(200));
        assert_eq!(pretty_date(&e.taken), "2024-05-17 09:30");
    }

    /// A JPEG whose EXIF says "rotate 90° clockwise to view".
    fn rotated_jpeg(w: u32, h: u32) -> Vec<u8> {
        // left half red, right half blue
        let img = image::RgbImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        let mut jpg = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
        let mut tiff = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&((tiff.len() + 2) as u16).to_be_bytes());
        app1.extend_from_slice(&tiff);
        [&jpg[..2], &app1[..], &jpg[2..]].concat()
    }

    #[test]
    fn opens_a_set_of_pictures() {
        crate::pdf::init_thread();
        let dir = std::env::temp_dir().join(format!("pps_photo_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("b 10.png");
        RgbaImage::from_fn(200, 100, |x, _| if x < 100 { Rgba([0, 0, 0, 0]) } else { Rgba([0, 255, 0, 255]) }).save(&png).unwrap();
        let jpg = dir.join("b 2.jpg");
        std::fs::write(&jpg, rotated_jpeg(300, 200)).unwrap();

        let o = ImageOptions { dpi: 25.4, use_file_dpi: false, background: [255, 255, 0], ..Default::default() };
        let doc = ImageDoc::open(&[dir.clone()], &o).unwrap();
        // natural order: "b 2" before "b 10"; the JPEG is turned upright to 200 × 300
        assert_eq!(doc.labels.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(), ["b 2.jpg", "b 10.png"]);
        assert_eq!(doc.sizes_mm(), vec![(200.0, 300.0), (200.0, 100.0)]);
        let up = doc.render(0, WHOLE, 200, 300).unwrap();
        // red was on the left; after turning clockwise it is on top
        let (top, bottom) = (up.get_pixel(100, 20).0, up.get_pixel(100, 280).0);
        assert!(top[0] > 200 && top[2] < 60, "top {top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "bottom {bottom:?}");
        // transparency lands on the background colour
        let png_page = doc.render(1, WHOLE, 200, 100).unwrap();
        assert_eq!(png_page.get_pixel(10, 50).0, [255, 255, 0, 255]);
        assert_eq!(png_page.get_pixel(190, 50).0, [0, 255, 0, 255]);
        // a thumbnail uses a reduced decode but looks the same
        let thumb = doc.render(0, WHOLE, 20, 30).unwrap();
        assert!(thumb.get_pixel(10, 2).0[0] > 200);

        // a passport photo of the turned JPEG: 35 × 45, cropped at the sides, padded when fitted
        let o = ImageOptions { size: PhotoSize::Passport, ..o };
        let doc = ImageDoc::open(&[jpg.clone()], &o).unwrap();
        assert_eq!(doc.sizes_mm(), vec![(35.0, 45.0)]);
        let o = ImageOptions { fit: PhotoFit::Fit, size: PhotoSize::Square10, ..o };
        let doc = ImageDoc::open(&[jpg], &o).unwrap();
        let sq = doc.render(0, WHOLE, 90, 90).unwrap();
        assert_eq!(sq.get_pixel(3, 45).0, [255, 255, 0, 255]);
        // the Windows codecs did the decoding (not the fallback), at full and reduced size
        assert_eq!(wic_decode(&doc.pages[0], 1.0, true).map(|i| i.dimensions()).ok(), Some((300, 200)));
        assert_eq!(wic_decode(&doc.pages[0], 0.5, true).map(|i| i.dimensions()).ok(), Some((150, 100)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    #[test]
    fn levels_stretch() {
        let mut img = RgbaImage::from_fn(100, 1, |x, _| {
            let v = 60 + x as u8;
            Rgba([v, v, v, 255])
        });
        auto_levels(&mut img);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(99, 0).0[0], 255);
    }
}
