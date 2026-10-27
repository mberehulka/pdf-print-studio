//! PDF rendering through the Windows built-in `Windows.Data.Pdf` engine.

use image::RgbaImage;
use std::path::Path;
use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Foundation::Rect as WinRect;
use windows::Graphics::Imaging::BitmapEncoder;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const ERROR_WRONG_PASSWORD: u32 = 0x8007_052B;
const DIP_TO_MM: f32 = 25.4 / 96.0;

pub fn init_thread() {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

#[derive(Debug)]
pub enum OpenError {
    NeedPassword,
    Other(String),
}

pub struct PdfDoc {
    doc: PdfDocument,
    /// Page sizes in DIPs (1/96 inch), already rotated.
    sizes: Vec<(f32, f32)>,
}

impl PdfDoc {
    pub fn open(path: &Path, password: Option<&str>) -> Result<Self, OpenError> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let other = |e: windows::core::Error| OpenError::Other(e.message().to_string());
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.to_string_lossy().as_ref()))
            .and_then(|op| op.get())
            .map_err(other)?;
        let res = match password {
            Some(pw) if !pw.is_empty() => {
                PdfDocument::LoadFromFileWithPasswordAsync(&file, &HSTRING::from(pw)).and_then(|op| op.get())
            }
            _ => PdfDocument::LoadFromFileAsync(&file).and_then(|op| op.get()),
        };
        let doc = match res {
            Ok(d) => d,
            Err(e) if e.code().0 as u32 == ERROR_WRONG_PASSWORD => return Err(OpenError::NeedPassword),
            Err(e) => return Err(other(e)),
        };
        let count = doc.PageCount().map_err(other)?;
        let mut sizes = Vec::with_capacity(count as usize);
        for i in 0..count {
            let page = doc.GetPage(i).map_err(other)?;
            let sz = page.Size().map_err(other)?;
            sizes.push((sz.Width.max(1.0), sz.Height.max(1.0)));
        }
        Ok(Self { doc, sizes })
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        self.sizes.iter().map(|&(w, h)| (w * DIP_TO_MM, h * DIP_TO_MM)).collect()
    }

    /// Render `src` (normalised page rect x0,y0,x1,y1) to a `w`x`h` bitmap.
    pub fn render(&self, index: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        self.render_inner(index, src, w.max(1), h.max(1)).map_err(|e| e.message().to_string())
    }

    fn render_inner(&self, index: usize, src: [f32; 4], w: u32, h: u32) -> windows::core::Result<RgbaImage> {
        let (pw, ph) = self.sizes[index];
        let page = self.doc.GetPage(index as u32)?;
        let opts = PdfPageRenderOptions::new()?;
        opts.SetDestinationWidth(w)?;
        opts.SetDestinationHeight(h)?;
        let full = src == [0.0, 0.0, 1.0, 1.0];
        if !full {
            opts.SetSourceRect(WinRect {
                X: src[0] * pw,
                Y: src[1] * ph,
                Width: ((src[2] - src[0]) * pw).max(0.01),
                Height: ((src[3] - src[1]) * ph).max(0.01),
            })?;
        }
        opts.SetBitmapEncoderId(BitmapEncoder::PngEncoderId()?)?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &opts)?.get()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.get()?;
        let mut buf = vec![0u8; size as usize];
        reader.ReadBytes(&mut buf)?;
        let img = image::load_from_memory_with_format(&buf, image::ImageFormat::Png)
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        Ok(img.to_rgba8())
    }
}
