//! Explorer thumbnail provider (IThumbnailProvider) for PDF, CBZ and EPUB files.
//!
//! Explorer hands over the file as a stream; the kind is recognised from its content: the first
//! PDF page, the first image of a comic book, or the cover of an e-book becomes the thumbnail.

use image::imageops::FilterType;
use image::{ImageReader, RgbaImage};
use std::cmp::Ordering;
use std::ffi::c_void;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Mutex;
use windows::core::{implement, IUnknown, Interface, Result, GUID, HRESULT};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::BitmapEncoder;
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};
use windows::Win32::Foundation::{BOOL, CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, S_FALSE, S_OK};
use windows::Win32::Graphics::Gdi::{CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl, IStream, STREAM_SEEK_SET};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{IThumbnailProvider, IThumbnailProvider_Impl, WTSAT_RGB, WTS_ALPHATYPE};

/// Must match `THUMB_CLSID` in the app's installer.
pub const CLSID: GUID = GUID::from_u128(0xD56D8E8B_1EAC_48CE_822F_4244DA3EB9C7);

/// Live objects; the DLL may be unloaded when this is zero.
static OBJECTS: AtomicUsize = AtomicUsize::new(0);

const MAX_FILE: u64 = 1 << 30;

// ---------------------------------------------------------------- COM objects

#[implement(IThumbnailProvider, IInitializeWithStream)]
struct Provider {
    data: Mutex<Option<Vec<u8>>>,
}

impl Provider {
    fn new() -> Self {
        OBJECTS.fetch_add(1, AtomicOrdering::SeqCst);
        Self { data: Mutex::new(None) }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, AtomicOrdering::SeqCst);
    }
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, stream: Option<&IStream>, _mode: u32) -> Result<()> {
        let stream = stream.ok_or(windows::core::Error::from(E_POINTER))?;
        let mut data = Vec::new();
        unsafe {
            let _ = stream.Seek(0, STREAM_SEEK_SET, None);
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let mut read = 0u32;
                let hr = stream.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut read));
                if hr.is_err() || read == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..read as usize]);
                if data.len() as u64 > MAX_FILE {
                    return Err(E_FAIL.into());
                }
            }
        }
        *self.data.lock().unwrap() = Some(data);
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, pdwalpha: *mut WTS_ALPHATYPE) -> Result<()> {
        if phbmp.is_null() || pdwalpha.is_null() {
            return Err(E_POINTER.into());
        }
        let data = self.data.lock().unwrap().take().ok_or(windows::core::Error::from(E_FAIL))?;
        let size = cx.clamp(16, 2048);
        // WinRT PDF rendering waits on async calls: keep that off Explorer's apartment thread
        let img = std::thread::spawn(move || {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            thumbnail(&data, size)
        })
        .join()
        .ok()
        .flatten()
        .ok_or(windows::core::Error::from(E_FAIL))?;
        unsafe {
            *phbmp = to_hbitmap(&img)?;
            *pdwalpha = WTSAT_RGB;
        }
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Option<&IUnknown>, riid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if outer.is_some() {
            return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
        }
        if riid.is_null() || object.is_null() {
            return Err(E_POINTER.into());
        }
        let unknown: IUnknown = Provider::new().into();
        unsafe { unknown.query(&*riid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// # Safety
/// Called by COM with valid pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, riid: *const GUID, object: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || riid.is_null() || object.is_null() {
        return E_POINTER;
    }
    *object = std::ptr::null_mut();
    if *clsid != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    factory.query(&*riid, object)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(AtomicOrdering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

// ---------------------------------------------------------------- bitmaps

/// White background (thumbnails are opaque), fit inside `size` × `size`.
fn fit(img: RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let k = (size as f32 / w as f32).min(size as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let mut img = if (tw, th) == (w, h) { img } else { image::imageops::resize(&img, tw, th, FilterType::Triangle) };
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p.0[3] = 255;
    }
    img
}

unsafe fn to_hbitmap(img: &RgbaImage) -> Result<HBITMAP> {
    let (w, h) = img.dimensions();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    if bits.is_null() {
        return Err(E_FAIL.into());
    }
    let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    for (d, s) in dst.chunks_exact_mut(4).zip(img.pixels()) {
        d.copy_from_slice(&[s.0[2], s.0[1], s.0[0], 255]);
    }
    Ok(bmp)
}

// ---------------------------------------------------------------- content

fn thumbnail(data: &[u8], size: u32) -> Option<RgbaImage> {
    let img = if data.starts_with(b"%PDF") || data.windows(5).take(1024).any(|w| w == b"%PDF-") {
        pdf_page(data, size)?
    } else if data.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(Cursor::new(data)).ok()?;
        epub_cover(&mut zip).or_else(|| first_image(&mut zip))?
    } else {
        return None;
    };
    Some(fit(img, size))
}

fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    Some(ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.decode().ok()?.to_rgba8())
}

fn entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

fn pdf_page(data: &[u8], size: u32) -> Option<RgbaImage> {
    let run = || -> windows::core::Result<RgbaImage> {
        let stream = InMemoryRandomAccessStream::new()?;
        let writer = DataWriter::CreateDataWriter(&stream)?;
        writer.WriteBytes(data)?;
        writer.StoreAsync()?.get()?;
        writer.DetachStream()?;
        stream.Seek(0)?;
        let doc = PdfDocument::LoadFromStreamAsync(&stream)?.get()?;
        let page = doc.GetPage(0)?;
        let sz = page.Size()?;
        let k = size as f32 / sz.Width.max(sz.Height).max(1.0);
        let opts = PdfPageRenderOptions::new()?;
        opts.SetDestinationWidth(((sz.Width * k).round() as u32).max(1))?;
        opts.SetDestinationHeight(((sz.Height * k).round() as u32).max(1))?;
        opts.SetBitmapEncoderId(BitmapEncoder::PngEncoderId()?)?;
        let out = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&out, &opts)?.get()?;
        let len = out.Size()? as u32;
        let reader = DataReader::CreateDataReader(&out.GetInputStreamAt(0)?)?;
        reader.LoadAsync(len)?.get()?;
        let mut png = vec![0u8; len as usize];
        reader.ReadBytes(&mut png)?;
        decode(&png).ok_or_else(|| E_FAIL.into())
    };
    run().ok()
}

fn is_image(name: &str) -> bool {
    let l = name.to_lowercase();
    let file = l.rsplit('/').next().unwrap_or(&l);
    !l.starts_with("__macosx/")
        && !file.starts_with('.')
        && ["jpg", "jpeg", "png", "webp", "gif", "bmp"].iter().any(|e| file.ends_with(&format!(".{e}")))
}

/// "page2" before "page10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
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
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
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

fn first_image(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<RgbaImage> {
    let mut names: Vec<String> = zip.file_names().filter(|n| is_image(n)).map(str::to_string).collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names.iter().find_map(|n| decode(&entry(zip, n)?))
}

/// Attributes of every `<tag …>` named `name` (namespace prefixes ignored).
fn tags(xml: &str, name: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    for part in xml.split('<').skip(1) {
        let body = part.split('>').next().unwrap_or("");
        let tag = body.split_whitespace().next().unwrap_or("");
        if tag.rsplit(':').next().unwrap_or(tag).eq_ignore_ascii_case(name) {
            let mut attrs = Vec::new();
            let mut rest = body[tag.len()..].trim();
            while let Some(eq) = rest.find('=') {
                let key = rest[..eq].trim().to_ascii_lowercase();
                let after = rest[eq + 1..].trim_start();
                let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
                let Some(end) = after[1..].find(q) else { break };
                attrs.push((key, after[1..1 + end].to_string()));
                rest = after[end + 2..].trim_start();
            }
            out.push(attrs);
        }
    }
    out
}

fn get<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("").replace("%20", " ");
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for p in href.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn epub_cover(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<RgbaImage> {
    let container = String::from_utf8(entry(zip, "META-INF/container.xml")?).ok()?;
    let opf_path = tags(&container, "rootfile").iter().find_map(|a| get(a, "full-path").map(str::to_string))?;
    let opf = String::from_utf8_lossy(&entry(zip, &opf_path)?).into_owned();
    let items = tags(&opf, "item");
    let cover_id = tags(&opf, "meta").iter().find(|a| get(a, "name") == Some("cover")).and_then(|a| get(a, "content").map(str::to_string));
    let href = items
        .iter()
        .find(|a| get(a, "properties").is_some_and(|p| p.split_whitespace().any(|p| p == "cover-image")))
        .or_else(|| items.iter().find(|a| cover_id.is_some() && get(a, "id") == cover_id.as_deref()))
        .and_then(|a| get(a, "href"))
        .map(|h| resolve(&opf_path, h));
    match href {
        Some(h) => decode(&entry(zip, &h)?),
        None => first_image(zip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn zip_of(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut buf);
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn comic_uses_first_page_in_natural_order() {
        let data = zip_of(&[("p10.png", png(40, 60, [0, 0, 255])), ("p2.png", png(40, 60, [255, 0, 0]))]);
        let t = thumbnail(&data, 96).unwrap();
        assert_eq!((t.width(), t.height()), (40, 60));
        assert_eq!(t.get_pixel(5, 5).0, [255, 0, 0, 255]);
    }

    #[test]
    fn ebook_uses_cover() {
        let opf = r#"<package><manifest><item id="c" href="img/cover.png" media-type="image/png" properties="cover-image"/>
            <item id="x" href="img/a.png" media-type="image/png"/></manifest></package>"#;
        let data = zip_of(&[
            ("mimetype", b"application/epub+zip".to_vec()),
            ("META-INF/container.xml", br#"<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#.to_vec()),
            ("OPS/book.opf", opf.as_bytes().to_vec()),
            ("OPS/img/a.png", png(300, 100, [0, 255, 0])),
            ("OPS/img/cover.png", png(300, 450, [10, 20, 30])),
        ]);
        let t = thumbnail(&data, 150).unwrap();
        assert_eq!((t.width(), t.height()), (100, 150));
        assert_eq!(t.get_pixel(50, 50).0, [10, 20, 30, 255]);
    }

    #[test]
    fn pdf_first_page() {
        // uses the demo file when present
        let Ok(data) = std::fs::read(r"C:\Users\Public\Documents\PrintStudioDemo\Annual-Report.pdf") else { return };
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let t = thumbnail(&data, 256).unwrap();
        assert_eq!(t.height(), 256);
        assert!(t.width() > 150 && t.width() < 256);
    }

    #[test]
    fn com_path_like_explorer() {
        use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
        use windows::Win32::UI::Shell::SHCreateMemStream;
        let data = zip_of(&[("001.png", png(200, 300, [200, 30, 30]))]);
        unsafe {
            let mut factory: *mut c_void = std::ptr::null_mut();
            assert_eq!(DllGetClassObject(&CLSID, &IClassFactory::IID, &mut factory), S_OK);
            let factory = IClassFactory::from_raw(factory);
            let provider: IThumbnailProvider = factory.CreateInstance(None).unwrap();
            let init: IInitializeWithStream = provider.cast().unwrap();
            let stream = SHCreateMemStream(Some(&data)).unwrap();
            init.Initialize(&stream, 0).unwrap();
            let mut bmp = HBITMAP::default();
            let mut alpha = WTS_ALPHATYPE::default();
            provider.GetThumbnail(96, &mut bmp, &mut alpha).unwrap();
            let mut info = BITMAP::default();
            GetObjectW(bmp, std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut c_void));
            assert_eq!((info.bmWidth, info.bmHeight), (64, 96));
            let _ = DeleteObject(bmp);
            let other = GUID::from_u128(1);
            assert_eq!(DllGetClassObject(&other, &IClassFactory::IID, &mut std::ptr::null_mut()), CLASS_E_CLASSNOTAVAILABLE);
        }
    }
}
