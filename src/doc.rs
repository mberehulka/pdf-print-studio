//! An opened document: a PDF, a comic book archive (.cbz), an e-book (.epub) or pictures.

use crate::cbz::CbzDoc;
use crate::epub::EpubDoc;
use crate::pdf::{OpenError, PdfDoc};
use crate::photo::{self, ImageDoc};
use crate::settings::{CbzOptions, EpubOptions, ImageOptions};
use image::RgbaImage;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    Pdf,
    Cbz,
    Epub,
    Image,
}

/// By extension, or by content for misnamed files (zip archives with an EPUB mimetype are e-books).
/// A folder is a set of pictures.
pub fn kind_of(path: &Path) -> DocKind {
    if path.is_dir() || photo::is_image(path) {
        return DocKind::Image;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => DocKind::Pdf,
        "cbz" => DocKind::Cbz,
        "epub" => DocKind::Epub,
        _ => {
            let mut head = [0u8; 58];
            let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
            if n >= 4 && &head[..4] == b"PK\x03\x04" {
                if head[..n].windows(20).any(|w| w == b"application/epub+zip") {
                    DocKind::Epub
                } else {
                    DocKind::Cbz
                }
            } else if n >= 4 && &head[..4] != b"%PDF" && image::guess_format(&head[..n]).is_ok() {
                DocKind::Image
            } else {
                DocKind::Pdf
            }
        }
    }
}

pub enum Doc {
    Pdf(PdfDoc),
    Cbz(CbzDoc),
    Epub(EpubDoc),
    Image(ImageDoc),
}

impl Doc {
    /// `more`: further pictures opened together with `path` (one document, a page each).
    pub fn open(
        path: &Path,
        more: &[PathBuf],
        password: Option<&str>,
        cbz: &CbzOptions,
        epub: &EpubOptions,
        img: &ImageOptions,
    ) -> Result<Self, OpenError> {
        match kind_of(path) {
            DocKind::Pdf => PdfDoc::open(path, password).map(Doc::Pdf),
            DocKind::Cbz => CbzDoc::open(path, cbz).map(Doc::Cbz).map_err(OpenError::Other),
            DocKind::Epub => EpubDoc::open(path, epub).map(Doc::Epub).map_err(OpenError::Other),
            DocKind::Image => {
                let files: Vec<PathBuf> = std::iter::once(path.to_path_buf()).chain(more.iter().cloned()).collect();
                ImageDoc::open(&files, img).map(Doc::Image).map_err(OpenError::Other)
            }
        }
    }

    pub fn kind(&self) -> DocKind {
        match self {
            Doc::Pdf(_) => DocKind::Pdf,
            Doc::Cbz(_) => DocKind::Cbz,
            Doc::Epub(_) => DocKind::Epub,
            Doc::Image(_) => DocKind::Image,
        }
    }

    pub fn page_count(&self) -> usize {
        match self {
            Doc::Pdf(d) => d.page_count(),
            Doc::Cbz(d) => d.page_count(),
            Doc::Epub(d) => d.page_count(),
            Doc::Image(d) => d.page_count(),
        }
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        match self {
            Doc::Pdf(d) => d.sizes_mm(),
            Doc::Cbz(d) => d.sizes_mm(),
            Doc::Epub(d) => d.sizes_mm(),
            Doc::Image(d) => d.sizes_mm(),
        }
    }

    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        match self {
            Doc::Pdf(d) => d.render(page, src, w, h),
            Doc::Cbz(d) => d.render(page, src, w, h),
            Doc::Epub(d) => d.render(page, src, w, h),
            Doc::Image(d) => d.render(page, src, w, h),
        }
    }

    /// Metadata to show: ComicInfo.xml fields, the e-book's title and author, the photo's camera and so on.
    pub fn meta(&self) -> Vec<(String, String)> {
        match self {
            Doc::Pdf(_) => Vec::new(),
            Doc::Cbz(d) => d.meta.clone(),
            Doc::Epub(d) => {
                let mut m = d.meta.clone();
                m.push(("Chapters".into(), d.chapters().to_string()));
                m
            }
            Doc::Image(d) => d.meta.clone(),
        }
    }

    /// Per page: the picture's file name and date, for captions (empty for other documents).
    pub fn page_labels(&self) -> Vec<(String, String)> {
        match self {
            Doc::Image(d) => d.labels.clone(),
            _ => Vec::new(),
        }
    }

    /// Per page: pixels a picture puts on it (empty for other documents).
    pub fn page_px(&self) -> Vec<(u32, u32)> {
        match self {
            Doc::Image(d) => d.px.clone(),
            _ => Vec::new(),
        }
    }
}
