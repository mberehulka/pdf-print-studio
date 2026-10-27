//! An opened document: a PDF, a comic book archive (.cbz) or an e-book (.epub).

use crate::cbz::CbzDoc;
use crate::epub::EpubDoc;
use crate::pdf::{OpenError, PdfDoc};
use crate::settings::{CbzOptions, EpubOptions};
use image::RgbaImage;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    Pdf,
    Cbz,
    Epub,
}

/// By extension, or by content for misnamed files (zip archives with an EPUB mimetype are e-books).
pub fn kind_of(path: &Path) -> DocKind {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => DocKind::Pdf,
        "cbz" => DocKind::Cbz,
        "epub" => DocKind::Epub,
        _ => {
            let mut head = [0u8; 58];
            let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
            if n < 4 || &head[..4] != b"PK\x03\x04" {
                DocKind::Pdf
            } else if head[..n].windows(20).any(|w| w == b"application/epub+zip") {
                DocKind::Epub
            } else {
                DocKind::Cbz
            }
        }
    }
}

pub enum Doc {
    Pdf(PdfDoc),
    Cbz(CbzDoc),
    Epub(EpubDoc),
}

impl Doc {
    pub fn open(path: &Path, password: Option<&str>, cbz: &CbzOptions, epub: &EpubOptions) -> Result<Self, OpenError> {
        match kind_of(path) {
            DocKind::Pdf => PdfDoc::open(path, password).map(Doc::Pdf),
            DocKind::Cbz => CbzDoc::open(path, cbz).map(Doc::Cbz).map_err(OpenError::Other),
            DocKind::Epub => EpubDoc::open(path, epub).map(Doc::Epub).map_err(OpenError::Other),
        }
    }

    pub fn kind(&self) -> DocKind {
        match self {
            Doc::Pdf(_) => DocKind::Pdf,
            Doc::Cbz(_) => DocKind::Cbz,
            Doc::Epub(_) => DocKind::Epub,
        }
    }

    pub fn page_count(&self) -> usize {
        match self {
            Doc::Pdf(d) => d.page_count(),
            Doc::Cbz(d) => d.page_count(),
            Doc::Epub(d) => d.page_count(),
        }
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        match self {
            Doc::Pdf(d) => d.sizes_mm(),
            Doc::Cbz(d) => d.sizes_mm(),
            Doc::Epub(d) => d.sizes_mm(),
        }
    }

    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        match self {
            Doc::Pdf(d) => d.render(page, src, w, h),
            Doc::Cbz(d) => d.render(page, src, w, h),
            Doc::Epub(d) => d.render(page, src, w, h),
        }
    }

    /// Metadata to show: ComicInfo.xml fields, or the e-book's title, author and so on.
    pub fn meta(&self) -> Vec<(String, String)> {
        match self {
            Doc::Pdf(_) => Vec::new(),
            Doc::Cbz(d) => d.meta.clone(),
            Doc::Epub(d) => {
                let mut m = d.meta.clone();
                m.push(("Chapters".into(), d.chapters().to_string()));
                m
            }
        }
    }
}
