//! Win32 printing: printer enumeration, driver capabilities, DEVMODE editing,
//! the driver's own properties dialog and banded bitmap output via GDI.

use crate::settings::{ColorMode, Duplex, Quality, Settings, SpoolFormat};
use image::RgbImage;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

type Handle = isize;

#[repr(C)]
struct PrinterInfo2W {
    server_name: *const u16,
    printer_name: *const u16,
    share_name: *const u16,
    port_name: *const u16,
    driver_name: *const u16,
    comment: *const u16,
    location: *const u16,
    dev_mode: *mut c_void,
    sep_file: *const u16,
    print_processor: *const u16,
    datatype: *const u16,
    parameters: *const u16,
    security_descriptor: *mut c_void,
    attributes: u32,
    priority: u32,
    default_priority: u32,
    start_time: u32,
    until_time: u32,
    status: u32,
    jobs: u32,
    average_ppm: u32,
}

#[repr(C)]
pub struct DocInfoW {
    pub cb_size: i32,
    pub doc_name: *const u16,
    pub output: *const u16,
    pub datatype: *const u16,
    pub fw_type: u32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

#[link(name = "winspool")]
extern "system" {
    fn EnumPrintersW(flags: u32, name: *const u16, level: u32, buf: *mut u8, cb: u32, needed: *mut u32, returned: *mut u32) -> i32;
    fn GetDefaultPrinterW(buf: *mut u16, size: *mut u32) -> i32;
    fn OpenPrinterW(name: *const u16, h: *mut Handle, def: *const c_void) -> i32;
    fn ClosePrinter(h: Handle) -> i32;
    fn GetPrinterW(h: Handle, level: u32, buf: *mut u8, cb: u32, needed: *mut u32) -> i32;
    fn DocumentPropertiesW(hwnd: isize, h: Handle, name: *const u16, out: *mut u8, inp: *const u8, mode: u32) -> i32;
    fn DeviceCapabilitiesW(device: *const u16, port: *const u16, cap: u16, out: *mut u16, dm: *const u8) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateDCW(driver: *const u16, device: *const u16, port: *const u16, dm: *const u8) -> Handle;
    fn CreateICW(driver: *const u16, device: *const u16, port: *const u16, dm: *const u8) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
    fn GetDeviceCaps(hdc: Handle, index: i32) -> i32;
    fn StartDocW(hdc: Handle, di: *const DocInfoW) -> i32;
    fn EndDoc(hdc: Handle) -> i32;
    fn AbortDoc(hdc: Handle) -> i32;
    fn StartPage(hdc: Handle) -> i32;
    fn EndPage(hdc: Handle) -> i32;
    fn SetStretchBltMode(hdc: Handle, mode: i32) -> i32;
    fn StretchDIBits(
        hdc: Handle, xd: i32, yd: i32, wd: i32, hd: i32, xs: i32, ys: i32, ws: i32, hs: i32,
        bits: *const c_void, bmi: *const c_void, usage: u32, rop: u32,
    ) -> i32;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn from_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn from_buf(b: &[u16]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..end]).trim().to_string()
}

// ------------------------------------------------------------------ printers

#[derive(Clone, Debug, Default)]
pub struct PrinterInfo {
    pub name: String,
    pub port: String,
    pub driver: String,
    pub comment: String,
    pub location: String,
    pub status: u32,
    pub jobs: u32,
    pub is_default: bool,
}

impl PrinterInfo {
    pub fn status_text(&self) -> (String, bool) {
        const FLAGS: &[(u32, &str)] = &[
            (0x1, "Paused"),
            (0x2, "Error"),
            (0x8, "Paper jam"),
            (0x10, "Out of paper"),
            (0x40, "Paper problem"),
            (0x80, "Offline"),
            (0x200, "Busy"),
            (0x400, "Printing"),
            (0x800, "Output bin full"),
            (0x1000, "Not available"),
            (0x4000, "Processing"),
            (0x20000, "Toner/ink low"),
            (0x40000, "Out of toner/ink"),
            (0x100000, "Needs attention"),
            (0x400000, "Door open"),
        ];
        let v: Vec<&str> = FLAGS.iter().filter(|(f, _)| self.status & f != 0).map(|(_, n)| crate::i18n::tr(n)).collect();
        if v.is_empty() {
            (crate::i18n::tr("Ready").into(), false)
        } else {
            let bad = self.status & (0x2 | 0x8 | 0x10 | 0x40 | 0x80 | 0x1000 | 0x40000 | 0x100000 | 0x400000) != 0;
            (v.join(", "), bad)
        }
    }
}

pub fn default_printer() -> Option<String> {
    unsafe {
        let mut size = 0u32;
        GetDefaultPrinterW(null_mut(), &mut size);
        if size == 0 {
            return None;
        }
        let mut b = vec![0u16; size as usize];
        (GetDefaultPrinterW(b.as_mut_ptr(), &mut size) != 0).then(|| from_buf(&b))
    }
}

pub fn list_printers() -> Vec<PrinterInfo> {
    const FLAGS: u32 = 0x2 | 0x4; // local | connections
    let def = default_printer().unwrap_or_default();
    unsafe {
        let mut needed = 0u32;
        let mut count = 0u32;
        EnumPrintersW(FLAGS, null(), 2, null_mut(), 0, &mut needed, &mut count);
        if needed == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u64; (needed as usize + 7) / 8];
        if EnumPrintersW(FLAGS, null(), 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) == 0 {
            return Vec::new();
        }
        let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PrinterInfo2W, count as usize);
        let mut v: Vec<PrinterInfo> = infos
            .iter()
            .map(|i| {
                let name = from_ptr(i.printer_name);
                PrinterInfo {
                    is_default: name == def,
                    name,
                    port: from_ptr(i.port_name),
                    driver: from_ptr(i.driver_name),
                    comment: from_ptr(i.comment),
                    location: from_ptr(i.location),
                    status: i.status,
                    jobs: i.jobs,
                }
            })
            .collect();
        v.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }
}

// ------------------------------------------------------------------ capabilities

#[derive(Clone, Debug)]
pub struct Paper {
    pub id: u16,
    pub name: String,
    pub w_mm: f32,
    pub h_mm: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Caps {
    pub papers: Vec<Paper>,
    pub bins: Vec<(u16, String)>,
    pub media: Vec<(u32, String)>,
    pub resolutions: Vec<(i32, i32)>,
    pub duplex: bool,
    pub color: bool,
    pub max_copies: i32,
    pub collate: bool,
}

const DC_PAPERS: u16 = 2;
const DC_PAPERSIZE: u16 = 3;
const DC_BINS: u16 = 6;
const DC_DUPLEX: u16 = 7;
const DC_BINNAMES: u16 = 12;
const DC_ENUMRESOLUTIONS: u16 = 13;
const DC_PAPERNAMES: u16 = 16;
const DC_COPIES: u16 = 18;
const DC_COLLATE: u16 = 22;
const DC_COLORDEVICE: u16 = 32;
const DC_MEDIATYPENAMES: u16 = 34;
const DC_MEDIATYPES: u16 = 35;

pub fn capabilities(p: &PrinterInfo, dm: &[u8]) -> Caps {
    let dev = wide(&p.name);
    let port = wide(&p.port);
    let dmp = if dm.is_empty() { null() } else { dm.as_ptr() };
    let query = |cap: u16, out: *mut u16| unsafe { DeviceCapabilitiesW(dev.as_ptr(), port.as_ptr(), cap, out, dmp) };
    let count = |cap: u16| query(cap, null_mut()).max(0) as usize;

    let mut caps = Caps::default();
    let n = count(DC_PAPERS);
    if n > 0 {
        let mut ids = vec![0u16; n];
        let mut names = vec![0u16; n * 64];
        let mut sizes = vec![0i32; n * 2];
        query(DC_PAPERS, ids.as_mut_ptr());
        query(DC_PAPERNAMES, names.as_mut_ptr());
        query(DC_PAPERSIZE, sizes.as_mut_ptr() as *mut u16);
        for i in 0..n {
            let name = from_buf(&names[i * 64..(i + 1) * 64]);
            let (w, h) = (sizes[i * 2] as f32 / 10.0, sizes[i * 2 + 1] as f32 / 10.0);
            if w > 0.0 && h > 0.0 {
                caps.papers.push(Paper { id: ids[i], name, w_mm: w, h_mm: h });
            }
        }
    }
    let n = count(DC_BINS);
    if n > 0 {
        let mut ids = vec![0u16; n];
        let mut names = vec![0u16; n * 24];
        query(DC_BINS, ids.as_mut_ptr());
        query(DC_BINNAMES, names.as_mut_ptr());
        caps.bins = (0..n).map(|i| (ids[i], from_buf(&names[i * 24..(i + 1) * 24]))).collect();
    }
    let n = count(DC_MEDIATYPES);
    if n > 0 {
        let mut ids = vec![0u32; n];
        let mut names = vec![0u16; n * 64];
        query(DC_MEDIATYPES, ids.as_mut_ptr() as *mut u16);
        query(DC_MEDIATYPENAMES, names.as_mut_ptr());
        caps.media = (0..n).map(|i| (ids[i], from_buf(&names[i * 64..(i + 1) * 64]))).collect();
    }
    let n = count(DC_ENUMRESOLUTIONS);
    if n > 0 {
        let mut r = vec![0i32; n * 2];
        query(DC_ENUMRESOLUTIONS, r.as_mut_ptr() as *mut u16);
        caps.resolutions = (0..n).map(|i| (r[i * 2], r[i * 2 + 1])).filter(|r| r.0 > 0).collect();
        caps.resolutions.sort();
        caps.resolutions.dedup();
    }
    caps.duplex = query(DC_DUPLEX, null_mut()) == 1;
    caps.color = query(DC_COLORDEVICE, null_mut()) == 1;
    caps.max_copies = query(DC_COPIES, null_mut()).max(1);
    caps.collate = query(DC_COLLATE, null_mut()) == 1;
    caps
}

// ------------------------------------------------------------------ DEVMODE

/// Byte offsets inside DEVMODEW (stable public ABI).
mod off {
    pub const SIZE: usize = 68;
    pub const FIELDS: usize = 72;
    pub const ORIENTATION: usize = 76;
    pub const PAPER_SIZE: usize = 78;
    pub const PAPER_LENGTH: usize = 80;
    pub const PAPER_WIDTH: usize = 82;
    pub const SCALE: usize = 84;
    pub const COPIES: usize = 86;
    pub const DEFAULT_SOURCE: usize = 88;
    pub const PRINT_QUALITY: usize = 90;
    pub const COLOR: usize = 92;
    pub const DUPLEX: usize = 94;
    pub const Y_RESOLUTION: usize = 96;
    pub const COLLATE: usize = 100;
    pub const MEDIA_TYPE: usize = 196;
}

mod dmf {
    pub const ORIENTATION: u32 = 0x1;
    pub const PAPERSIZE: u32 = 0x2;
    pub const PAPERLENGTH: u32 = 0x4;
    pub const PAPERWIDTH: u32 = 0x8;
    pub const SCALE: u32 = 0x10;
    pub const COPIES: u32 = 0x100;
    pub const DEFAULTSOURCE: u32 = 0x200;
    pub const PRINTQUALITY: u32 = 0x400;
    pub const COLOR: u32 = 0x800;
    pub const DUPLEX: u32 = 0x1000;
    pub const YRESOLUTION: u32 = 0x2000;
    pub const COLLATE: u32 = 0x8000;
    pub const MEDIATYPE: u32 = 0x0200_0000;
}

fn get_i16(dm: &[u8], o: usize) -> i16 {
    dm.get(o..o + 2).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
}
fn set_i16(dm: &mut [u8], o: usize, v: i16) {
    if let Some(b) = dm.get_mut(o..o + 2) {
        b.copy_from_slice(&v.to_le_bytes());
    }
}
fn get_u32(dm: &[u8], o: usize) -> u32 {
    dm.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}
fn set_u32(dm: &mut [u8], o: usize, v: u32) {
    if let Some(b) = dm.get_mut(o..o + 4) {
        b.copy_from_slice(&v.to_le_bytes());
    }
}
fn add_field(dm: &mut [u8], f: u32) {
    let v = get_u32(dm, off::FIELDS) | f;
    set_u32(dm, off::FIELDS, v);
}
fn public_size(dm: &[u8]) -> usize {
    get_i16(dm, off::SIZE) as u16 as usize
}

/// Values read back from a DEVMODE (e.g. after the driver dialog).
#[derive(Debug, Clone, Copy)]
pub struct DevmodeView {
    pub landscape: bool,
    pub paper: u16,
    pub bin: u16,
    pub duplex: i16,
    pub color: i16,
    pub copies: i16,
    pub collate: bool,
    pub quality: (i16, i16),
    pub media: u32,
}

pub fn read_devmode(dm: &[u8]) -> DevmodeView {
    let size = public_size(dm);
    DevmodeView {
        landscape: get_i16(dm, off::ORIENTATION) == 2,
        paper: get_i16(dm, off::PAPER_SIZE) as u16,
        bin: get_i16(dm, off::DEFAULT_SOURCE) as u16,
        duplex: get_i16(dm, off::DUPLEX),
        color: get_i16(dm, off::COLOR),
        copies: get_i16(dm, off::COPIES),
        collate: get_i16(dm, off::COLLATE) == 1,
        quality: (get_i16(dm, off::PRINT_QUALITY), get_i16(dm, off::Y_RESOLUTION)),
        media: if size >= off::MEDIA_TYPE + 4 { get_u32(dm, off::MEDIA_TYPE) } else { 0 },
    }
}

const DM_OUT_BUFFER: u32 = 2;
const DM_IN_PROMPT: u32 = 4;
const DM_IN_BUFFER: u32 = 8;

struct PrinterHandle(Handle);
impl PrinterHandle {
    fn open(name: &[u16]) -> Option<Self> {
        let mut h: Handle = 0;
        (unsafe { OpenPrinterW(name.as_ptr(), &mut h, null()) } != 0).then_some(Self(h))
    }
}
impl Drop for PrinterHandle {
    fn drop(&mut self) {
        unsafe { ClosePrinter(self.0) };
    }
}

/// Copy the DEVMODE stored for this printer: the user's saved preferences
/// (level 9) first, then the printer-wide default (level 2).
fn stored_devmode(h: Handle) -> Option<Vec<u8>> {
    for level in [9u32, 2] {
        unsafe {
            let mut needed = 0u32;
            GetPrinterW(h, level, null_mut(), 0, &mut needed);
            if needed == 0 {
                continue;
            }
            let mut buf = vec![0u64; (needed as usize + 7) / 8];
            if GetPrinterW(h, level, buf.as_mut_ptr() as *mut u8, needed, &mut needed) == 0 {
                continue;
            }
            let dm = if level == 9 {
                *(buf.as_ptr() as *const *const u8)
            } else {
                (*(buf.as_ptr() as *const PrinterInfo2W)).dev_mode as *const u8
            };
            if dm.is_null() {
                continue;
            }
            let size = u16::from_le_bytes([*dm.add(68), *dm.add(69)]) as usize;
            let extra = u16::from_le_bytes([*dm.add(70), *dm.add(71)]) as usize;
            if size >= off::COLLATE + 2 {
                return Some(std::slice::from_raw_parts(dm, size + extra).to_vec());
            }
        }
    }
    None
}

pub fn default_devmode(name: &str) -> Option<Vec<u8>> {
    let w = wide(name);
    let h = PrinterHandle::open(&w)?;
    if let Some(mut dm) = stored_devmode(h.0) {
        drop(h);
        merge_devmode(name, &mut dm);
        return Some(dm);
    }
    unsafe {
        let size = DocumentPropertiesW(0, h.0, w.as_ptr(), null_mut(), null(), 0);
        if size <= 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        (DocumentPropertiesW(0, h.0, w.as_ptr(), buf.as_mut_ptr(), null(), DM_OUT_BUFFER) >= 0).then_some(buf)
    }
}

fn devmode_call(hwnd: isize, name: &str, dm: &mut Vec<u8>, prompt: bool) -> bool {
    let w = wide(name);
    let Some(h) = PrinterHandle::open(&w) else { return false };
    unsafe {
        let size = DocumentPropertiesW(0, h.0, w.as_ptr(), null_mut(), null(), 0);
        if size <= 0 {
            return false;
        }
        let mut out = vec![0u8; (size as usize).max(dm.len())];
        let mode = DM_IN_BUFFER | DM_OUT_BUFFER | if prompt { DM_IN_PROMPT } else { 0 };
        let r = DocumentPropertiesW(hwnd, h.0, w.as_ptr(), out.as_mut_ptr(), dm.as_ptr(), mode);
        let ok = if prompt { r == 1 } else { r >= 0 };
        if ok {
            out.truncate(size as usize);
            *dm = out;
        }
        ok
    }
}

/// Let the driver validate/merge our changes.
pub fn merge_devmode(name: &str, dm: &mut Vec<u8>) -> bool {
    devmode_call(0, name, dm, false)
}

/// Show the printer driver's own "Printing Preferences" dialog. Returns true on OK.
pub fn properties_dialog(hwnd: isize, name: &str, dm: &mut Vec<u8>) -> bool {
    devmode_call(hwnd, name, dm, true)
}

/// Write the relevant print settings into a DEVMODE buffer.
pub fn apply_settings(dm: &mut [u8], s: &Settings, landscape: bool, caps: Option<&Caps>) {
    if dm.len() < off::COLLATE + 2 {
        return;
    }
    set_i16(dm, off::ORIENTATION, if landscape { 2 } else { 1 });
    add_field(dm, dmf::ORIENTATION);

    if s.paper_id == 256 {
        set_i16(dm, off::PAPER_SIZE, 256);
        set_i16(dm, off::PAPER_WIDTH, (s.custom_paper_mm[0] * 10.0) as i16);
        set_i16(dm, off::PAPER_LENGTH, (s.custom_paper_mm[1] * 10.0) as i16);
        add_field(dm, dmf::PAPERSIZE | dmf::PAPERLENGTH | dmf::PAPERWIDTH);
    } else if s.paper_id != 0 {
        set_i16(dm, off::PAPER_SIZE, s.paper_id as i16);
        add_field(dm, dmf::PAPERSIZE);
        let f = get_u32(dm, off::FIELDS) & !(dmf::PAPERLENGTH | dmf::PAPERWIDTH);
        set_u32(dm, off::FIELDS, f);
    }
    if s.bin != 0 {
        set_i16(dm, off::DEFAULT_SOURCE, s.bin as i16);
        add_field(dm, dmf::DEFAULTSOURCE);
    }
    if s.media != 0 && public_size(dm) >= off::MEDIA_TYPE + 4 {
        set_u32(dm, off::MEDIA_TYPE, s.media);
        add_field(dm, dmf::MEDIATYPE);
    }
    let q = match s.quality {
        Quality::Default => None,
        Quality::Draft => Some((-1, 0)),
        Quality::Low => Some((-2, 0)),
        Quality::Medium => Some((-3, 0)),
        Quality::High => Some((-4, 0)),
        Quality::Res(x, y) => Some((x as i16, y as i16)),
    };
    if let Some((x, y)) = q {
        set_i16(dm, off::PRINT_QUALITY, x);
        add_field(dm, dmf::PRINTQUALITY);
        if y > 0 {
            set_i16(dm, off::Y_RESOLUTION, y);
            add_field(dm, dmf::YRESOLUTION);
        }
    }
    if caps.map_or(true, |c| c.color) {
        set_i16(dm, off::COLOR, if s.color_mode == ColorMode::Color { 2 } else { 1 });
        add_field(dm, dmf::COLOR);
    }
    let dup = match s.duplex {
        Duplex::Off | Duplex::Manual => 1,
        Duplex::LongEdge => 2,
        Duplex::ShortEdge => 3,
    };
    set_i16(dm, off::DUPLEX, dup);
    add_field(dm, dmf::DUPLEX);
    // copies and collation are done in software for exact control
    set_i16(dm, off::COPIES, 1);
    set_i16(dm, off::COLLATE, 0);
    set_i16(dm, off::SCALE, 100);
    add_field(dm, dmf::COPIES | dmf::COLLATE | dmf::SCALE);
}

/// Sync UI settings from a DEVMODE edited in the driver dialog.
pub fn settings_from_devmode(dm: &[u8], s: &mut Settings) {
    let v = read_devmode(dm);
    if v.paper != 0 {
        s.paper_id = v.paper;
    }
    s.orientation = if v.landscape { crate::settings::Orientation::Landscape } else { crate::settings::Orientation::Portrait };
    s.bin = v.bin;
    if v.media != 0 {
        s.media = v.media;
    }
    if s.duplex != Duplex::Manual {
        s.duplex = match v.duplex {
            2 => Duplex::LongEdge,
            3 => Duplex::ShortEdge,
            _ => Duplex::Off,
        };
    }
    if v.color == 1 && s.color_mode == ColorMode::Color {
        s.color_mode = ColorMode::Grayscale;
    }
    if v.copies > 1 {
        s.copies = v.copies as u32;
        s.collate = v.collate;
    }
    s.quality = match v.quality {
        (-1, _) => Quality::Draft,
        (-2, _) => Quality::Low,
        (-3, _) => Quality::Medium,
        (-4, _) => Quality::High,
        (x, y) if x > 0 => Quality::Res(x as i32, if y > 0 { y as i32 } else { x as i32 }),
        _ => Quality::Default,
    };
}

// ------------------------------------------------------------------ device geometry

#[derive(Clone, Copy, Debug, Default)]
pub struct Geometry {
    pub dpi_x: i32,
    pub dpi_y: i32,
    pub phys_w: i32,
    pub phys_h: i32,
    pub off_x: i32,
    pub off_y: i32,
    pub print_w: i32,
    pub print_h: i32,
}

impl Geometry {
    pub fn sheet_mm(&self) -> (f32, f32) {
        (self.phys_w as f32 / self.dpi_x as f32 * 25.4, self.phys_h as f32 / self.dpi_y as f32 * 25.4)
    }
    /// Non-printable margins in mm: top, right, bottom, left.
    pub fn hw_margins(&self) -> [f32; 4] {
        let mx = 25.4 / self.dpi_x as f32;
        let my = 25.4 / self.dpi_y as f32;
        [
            self.off_y as f32 * my,
            (self.phys_w - self.print_w - self.off_x).max(0) as f32 * mx,
            (self.phys_h - self.print_h - self.off_y).max(0) as f32 * my,
            self.off_x as f32 * mx,
        ]
    }
}

fn read_geometry(hdc: Handle) -> Geometry {
    unsafe {
        Geometry {
            dpi_x: GetDeviceCaps(hdc, 88).max(1),
            dpi_y: GetDeviceCaps(hdc, 90).max(1),
            print_w: GetDeviceCaps(hdc, 8),
            print_h: GetDeviceCaps(hdc, 10),
            phys_w: GetDeviceCaps(hdc, 110).max(1),
            phys_h: GetDeviceCaps(hdc, 111).max(1),
            off_x: GetDeviceCaps(hdc, 112),
            off_y: GetDeviceCaps(hdc, 113),
        }
    }
}

pub fn geometry(name: &str, dm: &[u8]) -> Option<Geometry> {
    let w = wide(name);
    unsafe {
        let hdc = CreateICW(null(), w.as_ptr(), null(), if dm.is_empty() { null() } else { dm.as_ptr() });
        if hdc == 0 {
            return None;
        }
        let g = read_geometry(hdc);
        DeleteDC(hdc);
        Some(g)
    }
}

// ------------------------------------------------------------------ print device

pub struct PrintDc {
    hdc: Handle,
    pub geom: Geometry,
    in_doc: bool,
}

impl PrintDc {
    pub fn create(name: &str, dm: &[u8]) -> Result<Self, String> {
        let w = wide(name);
        let hdc = unsafe { CreateDCW(null(), w.as_ptr(), null(), dm.as_ptr()) };
        if hdc == 0 {
            return Err(crate::trf!("Could not open printer '{}'", name));
        }
        Ok(Self { hdc, geom: read_geometry(hdc), in_doc: false })
    }

    pub fn start_doc(&mut self, job: &str, output: Option<&str>) -> Result<(), String> {
        let jn = wide(job);
        let out = output.map(wide);
        let di = DocInfoW {
            cb_size: std::mem::size_of::<DocInfoW>() as i32,
            doc_name: jn.as_ptr(),
            output: out.as_ref().map_or(null(), |o| o.as_ptr()),
            datatype: null(),
            fw_type: 0,
        };
        if unsafe { StartDocW(self.hdc, &di) } <= 0 {
            return Err("StartDoc failed (job rejected or cancelled by the driver)".into());
        }
        self.in_doc = true;
        Ok(())
    }

    pub fn start_page(&self) -> Result<(), String> {
        (unsafe { StartPage(self.hdc) } > 0).then_some(()).ok_or_else(|| "StartPage failed".into())
    }

    pub fn end_page(&self) -> Result<(), String> {
        (unsafe { EndPage(self.hdc) } > 0).then_some(()).ok_or_else(|| "EndPage failed".into())
    }

    pub fn end_doc(&mut self) {
        if self.in_doc {
            unsafe { EndDoc(self.hdc) };
            self.in_doc = false;
        }
    }

    pub fn abort(&mut self) {
        if self.in_doc {
            unsafe { AbortDoc(self.hdc) };
            self.in_doc = false;
        }
    }

    /// Blit a full-sheet image so that it covers the physical page exactly.
    pub fn blit_sheet(&self, img: &RgbImage, fmt: SpoolFormat) -> Result<(), String> {
        let g = self.geom;
        unsafe { SetStretchBltMode(self.hdc, 4) }; // HALFTONE
        let (w, h) = (img.width() as i32, img.height() as i32);
        let strip = 256.max((8_000_000 / (w.max(1) * 3)).min(2048));
        let mut y = 0;
        while y < h {
            let sh = strip.min(h - y);
            let (header, bits) = encode_strip(img, y as u32, sh as u32, fmt);
            let dy0 = -g.off_y + (y as i64 * g.phys_h as i64 / h as i64) as i32;
            let dy1 = -g.off_y + ((y + sh) as i64 * g.phys_h as i64 / h as i64) as i32;
            let r = unsafe {
                StretchDIBits(
                    self.hdc, -g.off_x, dy0, g.phys_w, dy1 - dy0, 0, 0, w, sh,
                    bits.as_ptr() as *const c_void, header.as_ptr() as *const c_void, 0, 0x00CC_0020,
                )
            };
            if r == 0 {
                return Err("StretchDIBits failed (driver rejected the bitmap)".into());
            }
            y += sh;
        }
        Ok(())
    }
}

impl Drop for PrintDc {
    fn drop(&mut self) {
        self.abort();
        unsafe { DeleteDC(self.hdc) };
    }
}

/// Build BITMAPINFO (+palette) and pixel data for rows [y, y+h) in the requested format.
fn encode_strip(img: &RgbImage, y0: u32, h: u32, fmt: SpoolFormat) -> (Vec<u8>, Vec<u8>) {
    let w = img.width();
    let (bpp, palette): (u16, Vec<[u8; 4]>) = match fmt {
        SpoolFormat::Mono1 => (1, vec![[0, 0, 0, 0], [255, 255, 255, 0]]),
        SpoolFormat::Gray8 => (8, (0..=255u8).map(|v| [v, v, v, 0]).collect()),
        _ => (24, vec![]),
    };
    let stride = ((w as usize * bpp as usize + 31) / 32) * 4;
    let mut bits = vec![0u8; stride * h as usize];
    for row in 0..h {
        let line = &mut bits[row as usize * stride..(row as usize + 1) * stride];
        for x in 0..w {
            let p = img.get_pixel(x, y0 + row).0;
            match bpp {
                24 => {
                    let i = x as usize * 3;
                    line[i] = p[2];
                    line[i + 1] = p[1];
                    line[i + 2] = p[0];
                }
                8 => {
                    line[x as usize] = ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8;
                }
                _ => {
                    let l = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
                    if l >= 128 {
                        line[x as usize / 8] |= 0x80 >> (x % 8);
                    }
                }
            }
        }
    }
    let bih = BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width: w as i32,
        height: -(h as i32),
        planes: 1,
        bit_count: bpp,
        compression: 0,
        size_image: bits.len() as u32,
        x_ppm: 0,
        y_ppm: 0,
        clr_used: palette.len() as u32,
        clr_important: 0,
    };
    let mut header = Vec::with_capacity(40 + palette.len() * 4);
    header.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&bih as *const _ as *const u8, std::mem::size_of::<BitmapInfoHeader>())
    });
    for c in palette {
        header.extend_from_slice(&c); // RGBQUAD is B,G,R,0 — gray values make order irrelevant
    }
    (header, bits)
}

pub fn resolve_spool(s: &Settings) -> SpoolFormat {
    match (s.spool, s.color_mode) {
        (SpoolFormat::Auto, ColorMode::Color) => SpoolFormat::Rgb24,
        (SpoolFormat::Auto, ColorMode::Grayscale) => SpoolFormat::Gray8,
        (SpoolFormat::Auto, _) => SpoolFormat::Mono1,
        (f, _) => f,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn probe_printers() {
        for p in super::list_printers() {
            let dm = super::default_devmode(&p.name).unwrap_or_default();
            let v = super::read_devmode(&dm);
            let caps = super::capabilities(&p, &dm);
            let paper = caps.papers.iter().find(|x| x.id == v.paper).map(|x| (x.name.clone(), x.w_mm, x.h_mm));
            let g = super::geometry(&p.name, &dm);
            if let Some(h) = super::PrinterHandle::open(&super::wide(&p.name)) {
                unsafe {
                    for level in [9u32, 2] {
                        let mut needed = 0u32;
                        super::GetPrinterW(h.0, level, std::ptr::null_mut(), 0, &mut needed);
                        let mut buf = vec![0u64; (needed as usize + 7) / 8];
                        if needed > 0 && super::GetPrinterW(h.0, level, buf.as_mut_ptr() as *mut u8, needed, &mut needed) != 0 {
                            let dm = if level == 9 { *(buf.as_ptr() as *const *const u8) } else { (*(buf.as_ptr() as *const super::PrinterInfo2W)).dev_mode as *const u8 };
                            let id = if dm.is_null() { -1 } else { i16::from_le_bytes([*dm.add(78), *dm.add(79)]) as i32 };
                            println!("  level {level}: paper id {id}");
                        }
                    }
                }
            }
            println!("{} | dm.len={} paper_id={} {:?} | geom={:?} sheet={:?}", p.name, dm.len(), v.paper, paper, g, g.map(|g| g.sheet_mm()));
        }
    }
}
