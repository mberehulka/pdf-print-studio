//! An opened document: a PDF, or a comic book archive (.cbz).

use crate::cbz::CbzDoc;
use crate::pdf::{OpenError, PdfDoc};
use crate::settings::CbzOptions;
use image::RgbaImage;
use std::io::Read;
use std::path::Path;

pub enum Doc {
    Pdf(PdfDoc),
    Cbz(CbzDoc),
}

/// Zip archive by extension, or by content for misnamed files.
pub fn is_cbz(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "cbz" | "zip" => true,
        "pdf" => false,
        _ => {
            let mut magic = [0u8; 4];
            std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"PK\x03\x04"
        }
    }
}

impl Doc {
    pub fn open(path: &Path, password: Option<&str>, cbz: &CbzOptions) -> Result<Self, OpenError> {
        if is_cbz(path) {
            CbzDoc::open(path, cbz).map(Doc::Cbz).map_err(OpenError::Other)
        } else {
            PdfDoc::open(path, password).map(Doc::Pdf)
        }
    }

    pub fn page_count(&self) -> usize {
        match self {
            Doc::Pdf(d) => d.page_count(),
            Doc::Cbz(d) => d.page_count(),
        }
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        match self {
            Doc::Pdf(d) => d.sizes_mm(),
            Doc::Cbz(d) => d.sizes_mm(),
        }
    }

    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        match self {
            Doc::Pdf(d) => d.render(page, src, w, h),
            Doc::Cbz(d) => d.render(page, src, w, h),
        }
    }

    pub fn meta(&self) -> Vec<(String, String)> {
        match self {
            Doc::Pdf(_) => Vec::new(),
            Doc::Cbz(d) => d.meta.clone(),
        }
    }
}
