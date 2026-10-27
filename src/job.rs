//! Background print / export job.

use crate::compose::{self, StampCtx};
use crate::layout;
use crate::doc::Doc;
use crate::pdf;
use crate::printer::{self, Caps, PrintDc};
use crate::settings::{DpiMode, Duplex, Settings};
use crate::text::FontCache;
use crate::worker::{PageSrc, Renderer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub enum Target {
    Printer { name: String, devmode: Vec<u8>, caps: Option<Caps> },
    PngFolder { dir: PathBuf, sheet_mm: (f32, f32), hw: [f32; 4] },
}

pub struct JobSpec {
    pub path: PathBuf,
    pub password: Option<String>,
    pub settings: Settings,
    /// Page arrangement; `selection` indexes it.
    pub order: Vec<PageSrc>,
    pub selection: Vec<usize>,
    pub landscape: bool,
    pub target: Target,
    pub stamp: StampCtx,
}

pub enum JobMsg {
    Progress { done: usize, total: usize, label: String },
    NeedFlip { sheets: usize },
    Done(Result<String, String>),
}

pub struct JobHandle {
    pub rx: Receiver<JobMsg>,
    pub cont: Sender<bool>,
    pub cancel: Arc<AtomicBool>,
}

pub fn spawn(spec: JobSpec, ctx: eframe::egui::Context) -> JobHandle {
    let (tx, rx) = channel();
    let (cont_tx, cont_rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::Builder::new()
        .name("print-job".into())
        .spawn(move || {
            pdf::init_thread();
            let send = |m: JobMsg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let r = run(spec, &send, &cont_rx, &c2);
            send(JobMsg::Done(r));
        })
        .expect("spawn job");
    JobHandle { rx, cont: cont_tx, cancel }
}

/// Expand sheet sides into the physical output order (copies / collation / duplex pairs)
/// as (side, copy number). `None` is a blank side inserted so each copy starts on a fresh
/// sheet when duplexing.
pub fn output_sequence(n_sides: usize, s: &Settings) -> Vec<(Option<usize>, usize)> {
    let duplex = s.is_duplex();
    let mut sides: Vec<Option<usize>> = (0..n_sides).map(Some).collect();
    if duplex && sides.len() % 2 == 1 {
        sides.push(None);
    }
    let unit = if duplex { 2 } else { 1 };
    let units: Vec<&[Option<usize>]> = sides.chunks(unit).collect();
    let copies = s.copies.clamp(1, 9999) as usize;
    let mut out = Vec::new();
    if s.collate {
        for c in 1..=copies {
            for u in &units {
                out.extend(u.iter().map(|&side| (side, c)));
            }
        }
    } else {
        for u in &units {
            for c in 1..=copies {
                out.extend(u.iter().map(|&side| (side, c)));
            }
        }
    }
    out
}

fn run(spec: JobSpec, send: &dyn Fn(JobMsg), cont: &Receiver<bool>, cancel: &AtomicBool) -> Result<String, String> {
    let doc = Doc::open(&spec.path, spec.password.as_deref(), &spec.settings.cbz, &spec.settings.epub)
        .map_err(|e| crate::trf!("Cannot open file: {}", format!("{e:?}")))?;
    let mut r = Renderer::new(doc, false);
    r.set_order(spec.order.clone());
    let mut fonts = FontCache::default();
    let s = &spec.settings;
    let mut stamp = spec.stamp.clone();

    match spec.target {
        Target::PngFolder { dir, sheet_mm, hw } => {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let sheets = layout::build_sheets(&spec.selection, sheet_mm, s, hw);
            let total = sheets.len();
            stamp.sheet_total = total;
            for (i, sh) in sheets.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(crate::i18n::tr("Cancelled").into());
                }
                send(JobMsg::Progress { done: i, total, label: crate::trf!("Rendering sheet {} of {}", i + 1, total) });
                stamp.sheet_no = i + 1;
                let img = compose::compose_sheet(sh, sheet_mm, s.export_dpi as f32, s, &mut r, &mut fonts, &stamp);
                img.save(dir.join(format!("sheet_{:03}.png", i + 1))).map_err(|e| e.to_string())?;
            }
            Ok(crate::trf!("Exported {} sheet(s) to {}", total, dir.display()))
        }
        Target::Printer { name, mut devmode, caps } => {
            printer::apply_settings(&mut devmode, s, spec.landscape, caps.as_ref());
            printer::merge_devmode(&name, &mut devmode);
            let mut dc = PrintDc::create(&name, &devmode)?;
            let g = dc.geom;
            let sheet_mm = g.sheet_mm();
            let sheets = layout::build_sheets(&spec.selection, sheet_mm, s, g.hw_margins());
            if sheets.is_empty() {
                return Err(crate::i18n::tr("Nothing to print").into());
            }
            let dpi = match s.dpi_mode {
                DpiMode::Printer => (g.dpi_x.min(g.dpi_y) as f32).min(s.max_dpi.max(72) as f32),
                DpiMode::Custom => s.custom_dpi.clamp(36, 2400) as f32,
            };
            let fmt = printer::resolve_spool(s);
            stamp.sheet_total = sheets.len();
            let seq = output_sequence(sheets.len(), s);

            let phases: Vec<Vec<(Option<usize>, usize)>> = if s.duplex == Duplex::Manual {
                let mut seq = seq;
                if seq.len() % 2 == 1 {
                    seq.push((None, 0));
                }
                let fronts: Vec<_> = seq.iter().step_by(2).copied().collect();
                let mut backs: Vec<_> = seq.iter().skip(1).step_by(2).copied().collect();
                if s.manual_duplex_reverse {
                    backs.reverse();
                }
                vec![fronts, backs]
            } else {
                vec![seq]
            };

            let job = if s.job_name.trim().is_empty() { stamp.file.clone() } else { s.job_name.clone() };
            let total: usize = phases.iter().map(|p| p.len()).sum();
            let mut done = 0;
            for (pi, phase) in phases.iter().enumerate() {
                if pi == 1 {
                    send(JobMsg::NeedFlip { sheets: phase.len() });
                    if !cont.recv().unwrap_or(false) {
                        return Ok(crate::i18n::tr("Front sides printed; back sides cancelled").into());
                    }
                }
                let out = s.print_to_file.then(|| {
                    if phases.len() > 1 {
                        format!("{}.{}", s.output_file, if pi == 0 { "front" } else { "back" })
                    } else {
                        s.output_file.clone()
                    }
                });
                let label = if phases.len() > 1 { format!("{job} ({})", if pi == 0 { "fronts" } else { "backs" }) } else { job.clone() };
                dc.start_doc(&label, out.as_deref().filter(|o| !o.is_empty()))?;
                for side in phase {
                    if cancel.load(Ordering::Relaxed) {
                        dc.abort();
                        return Err(crate::i18n::tr("Cancelled — job aborted").into());
                    }
                    send(JobMsg::Progress { done, total, label: crate::trf!("Printing side {} of {}", done + 1, total) });
                    dc.start_page()?;
                    if let (Some(i), copy) = side {
                        stamp.sheet_no = i + 1;
                        stamp.copy = *copy;
                        let img = compose::compose_sheet(&sheets[*i], sheet_mm, dpi, s, &mut r, &mut fonts, &stamp);
                        dc.blit_sheet(&img, fmt)?;
                    }
                    dc.end_page()?;
                    done += 1;
                }
                dc.end_doc();
            }
            Ok(crate::trf!("Sent {} side(s) to {}", total, name))
        }
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end checks; run with TEST_PDF and TEST_OUT set:
    //! `cargo test e2e -- --ignored --nocapture`
    use super::*;
    use crate::settings::*;

    fn spec(s: Settings, target: Target) -> JobSpec {
        let pdf = std::env::var("TEST_PDF").expect("TEST_PDF");
        JobSpec {
            path: pdf.into(),
            password: None,
            order: PageSrc::identity(5),
            selection: (0..5).collect(),
            settings: s,
            landscape: false,
            target,
            stamp: StampCtx { file: "test.pdf".into(), doc_pages: 5, selected: 5, copies: 1, ..Default::default() },
        }
    }

    fn exec(sp: JobSpec) -> Result<String, String> {
        pdf::init_thread();
        let (_tx, rx) = channel();
        run(sp, &|_| {}, &rx, &AtomicBool::new(false))
    }

    #[test]
    #[ignore]
    fn e2e_export_variants() {
        let out = PathBuf::from(std::env::var("TEST_OUT").expect("TEST_OUT"));
        let a4 = (210.0, 297.0);
        let mut variants: Vec<(&str, Settings, (f32, f32))> = Vec::new();

        let mut s = Settings::default();
        s.nup_cols = 2;
        s.nup_rows = 2;
        s.cell_border = true;
        s.page_labels = true;
        s.watermark.enabled = true;
        s.header.enabled = true;
        s.footer.enabled = true;
        variants.push(("nup4", s, a4));

        let mut s = Settings::default();
        s.nup_cols = 2;
        s.watermark.enabled = true;
        s.watermark.text = "{user}{br}{if odd}ODD{else}EVEN{end} {sheet}".into();
        s.watermark.size_pt = 20.0;
        s.watermark.tile = true;
        s.watermark.boxed = true;
        s.watermark.per_page = true;
        s.header.enabled = true;
        s.header.rule = true;
        s.header.mirror_even = true;
        s.header.left = "{name|upper}".into();
        s.header.right = "{page|roman}".into();
        s.footer.enabled = true;
        s.footer.scope = StampScope::EachPage;
        s.footer.center = "{page} / {pages}".into();
        s.footer.show_if = "not first".into();
        s.stamps.push(Stamp { text: "APPROVED{br}{date}".into(), boxed: true, angle: 8.0, size_pt: 16.0, color: [20, 120, 40], ..Default::default() });
        s.stamps.push(Stamp { text: "{for p in sheetpages sep \" + \"}{p}{end}".into(), position: 6, ..Default::default() });
        variants.push(("overlays", s, (297.0, 210.0)));

        let mut s = Settings::default();
        s.layout = LayoutMode::Booklet;
        s.duplex = Duplex::ShortEdge;
        s.crop_marks = true;
        variants.push(("booklet", s, (297.0, 210.0)));

        let mut s = Settings::default();
        s.layout = LayoutMode::Poster;
        s.color_mode = ColorMode::Dither;
        variants.push(("poster", s, a4));

        let mut s = Settings::default();
        s.layout = LayoutMode::Repeat;
        s.nup_cols = 3;
        s.nup_rows = 4;
        s.crop_marks = true;
        s.rotation = 90;
        s.mirror_h = true;
        s.color_mode = ColorMode::Grayscale;
        s.auto_crop = true;
        variants.push(("repeat", s, a4));

        for (name, mut s, sheet) in variants {
            s.export_dpi = 60;
            let r = exec(spec(s, Target::PngFolder { dir: out.join(name), sheet_mm: sheet, hw: [0.0; 4] }));
            println!("{name}: {r:?}");
            assert!(r.is_ok());
        }
    }

    #[test]
    #[ignore]
    fn e2e_print_to_pdf_printer() {
        let out = PathBuf::from(std::env::var("TEST_OUT").expect("TEST_OUT"));
        let name = "Microsoft Print to PDF".to_string();
        let devmode = printer::default_devmode(&name).expect("devmode");
        let pi = printer::list_printers().into_iter().find(|p| p.name == name).expect("printer");
        let caps = printer::capabilities(&pi, &devmode);
        println!("papers={} bins={} res={:?} duplex={} color={}", caps.papers.len(), caps.bins.len(), caps.resolutions, caps.duplex, caps.color);
        let mut s = Settings::default();
        s.nup_cols = 2;
        s.nup_rows = 1;
        s.copies = 2;
        s.max_dpi = 150;
        s.print_to_file = true;
        s.output_file = out.join("printed.pdf").display().to_string();
        s.footer.enabled = true;
        let r = exec(spec(s, Target::Printer { name, devmode, caps: Some(caps) }));
        println!("print: {r:?}");
        assert!(r.is_ok());
    }
}
