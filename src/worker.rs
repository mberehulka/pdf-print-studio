//! Background thread that owns the PDF document and produces thumbnails,
//! viewer bitmaps and composed preview sheets for the UI.

use crate::compose::{self, PageRenderer, StampCtx};
use crate::layout::Sheet;
use crate::pdf::{self, OpenError, PdfDoc};
use crate::settings::Settings;
use crate::text::FontCache;
use image::{RgbImage, RgbaImage};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

pub struct Renderer {
    doc: PdfDoc,
    sizes_mm: Vec<(f32, f32)>,
    bbox: HashMap<usize, [f32; 4]>,
    cache: VecDeque<((usize, [u32; 4], u32, u32), RgbaImage)>,
    use_cache: bool,
}

impl Renderer {
    pub fn new(doc: PdfDoc, use_cache: bool) -> Self {
        let sizes_mm = doc.sizes_mm();
        Self { doc, sizes_mm, bbox: HashMap::new(), cache: VecDeque::new(), use_cache }
    }
    pub fn sizes_mm(&self) -> &[(f32, f32)] {
        &self.sizes_mm
    }
    pub fn page_count(&self) -> usize {
        self.doc.page_count()
    }
}

impl PageRenderer for Renderer {
    fn page_size_mm(&self, page: usize) -> (f32, f32) {
        self.sizes_mm.get(page).copied().unwrap_or((210.0, 297.0))
    }

    fn render(&mut self, page: usize, src: [f32; 4], w: u32, h: u32) -> Option<RgbaImage> {
        let key = (page, src.map(|v| (v * 100_000.0) as u32), w, h);
        if self.use_cache {
            if let Some((_, img)) = self.cache.iter().find(|(k, _)| *k == key) {
                return Some(img.clone());
            }
        }
        let img = self.doc.render(page, src, w, h).ok()?;
        if self.use_cache && (w as u64 * h as u64) < 6_000_000 {
            self.cache.push_back((key, img.clone()));
            while self.cache.len() > 24 {
                self.cache.pop_front();
            }
        }
        Some(img)
    }

    fn content_bbox(&mut self, page: usize) -> [f32; 4] {
        if let Some(b) = self.bbox.get(&page) {
            return *b;
        }
        let (pw, ph) = self.page_size_mm(page);
        let w = 400u32;
        let h = ((w as f32 * ph / pw) as u32).max(1);
        let mut b = [0.0, 0.0, 1.0, 1.0];
        if let Ok(img) = self.doc.render(page, [0.0, 0.0, 1.0, 1.0], w, h) {
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            for (x, y, p) in img.enumerate_pixels() {
                if p.0[3] > 10 && (p.0[0] < 235 || p.0[1] < 235 || p.0[2] < 235) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
            if x1 > x0 && y1 > y0 {
                let pad = 0.01;
                b = [
                    (x0 as f32 / w as f32 - pad).max(0.0),
                    (y0 as f32 / h as f32 - pad).max(0.0),
                    ((x1 + 1) as f32 / w as f32 + pad).min(1.0),
                    ((y1 + 1) as f32 / h as f32 + pad).min(1.0),
                ];
            }
        }
        self.bbox.insert(page, b);
        b
    }
}

pub struct DocInfo {
    pub path: PathBuf,
    pub sizes_mm: Vec<(f32, f32)>,
}

pub enum Req {
    Open { path: PathBuf, password: Option<String> },
    Thumb { page: usize, width: u32 },
    View { page: usize, width: u32 },
    Sheet { gen: u64, sheet: Sheet, sheet_mm: (f32, f32), dpi: f32, settings: Box<Settings>, ctx: StampCtx },
}

pub enum Resp {
    Opened(DocInfo),
    OpenFailed { need_password: bool, msg: String },
    Thumb { page: usize, img: RgbaImage },
    View { page: usize, img: RgbaImage },
    Sheet { gen: u64, img: RgbImage, coverage: f32 },
}

pub fn spawn(egui_ctx: eframe::egui::Context) -> (Sender<Req>, Receiver<Resp>) {
    let (req_tx, req_rx) = channel::<Req>();
    let (resp_tx, resp_rx) = channel::<Resp>();
    std::thread::Builder::new()
        .name("render-worker".into())
        .spawn(move || run(req_rx, resp_tx, egui_ctx))
        .expect("spawn worker");
    (req_tx, resp_rx)
}

fn run(rx: Receiver<Req>, tx: Sender<Resp>, ctx: eframe::egui::Context) {
    pdf::init_thread();
    let mut doc: Option<Renderer> = None;
    let mut fonts = FontCache::default();
    let mut thumbs: VecDeque<(usize, u32)> = VecDeque::new();
    let send = |r: Resp| {
        let _ = tx.send(r);
        ctx.request_repaint();
    };

    loop {
        // gather requests: block only when there is no background work
        let mut pending = Vec::new();
        if thumbs.is_empty() {
            match rx.recv() {
                Ok(r) => pending.push(r),
                Err(_) => return,
            }
        }
        loop {
            match rx.try_recv() {
                Ok(r) => pending.push(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        let mut last_sheet = None;
        let mut last_view = None;
        for r in pending {
            match r {
                Req::Open { path, password } => {
                    thumbs.clear();
                    last_sheet = None;
                    last_view = None;
                    match PdfDoc::open(&path, password.as_deref()) {
                        Ok(d) => {
                            let rdr = Renderer::new(d, true);
                            send(Resp::Opened(DocInfo { path, sizes_mm: rdr.sizes_mm().to_vec() }));
                            doc = Some(rdr);
                        }
                        Err(OpenError::NeedPassword) => {
                            doc = None;
                            send(Resp::OpenFailed { need_password: true, msg: crate::i18n::tr("This PDF is password protected.").into() })
                        }
                        Err(OpenError::Other(m)) => {
                            doc = None;
                            send(Resp::OpenFailed { need_password: false, msg: m })
                        }
                    }
                }
                Req::Thumb { page, width } => thumbs.push_back((page, width)),
                v @ Req::View { .. } => last_view = Some(v),
                sh @ Req::Sheet { .. } => last_sheet = Some(sh),
            }
        }

        let Some(d) = doc.as_mut() else {
            thumbs.clear();
            continue;
        };

        if let Some(Req::Sheet { gen, sheet, sheet_mm, dpi, settings, ctx: sctx }) = last_sheet {
            let img = compose::compose_sheet(&sheet, sheet_mm, dpi, &settings, d, &mut fonts, &sctx);
            let coverage = compose::coverage(&img);
            send(Resp::Sheet { gen, img, coverage });
        }
        if let Some(Req::View { page, width }) = last_view {
            if page < d.page_count() {
                let (pw, ph) = d.page_size_mm(page);
                let h = ((width as f32 * ph / pw) as u32).clamp(1, 16000);
                if let Some(img) = d.render(page, [0.0, 0.0, 1.0, 1.0], width.clamp(1, 16000), h) {
                    send(Resp::View { page, img });
                }
            }
        }
        if let Some((page, width)) = thumbs.pop_front() {
            if page < d.page_count() {
                let (pw, ph) = d.page_size_mm(page);
                let h = ((width as f32 * ph / pw) as u32).clamp(1, 2000);
                if let Ok(img) = d.doc.render(page, [0.0, 0.0, 1.0, 1.0], width, h) {
                    send(Resp::Thumb { page, img });
                }
            }
        }
    }
}
