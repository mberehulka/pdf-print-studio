//! egui front-end: thumbnails, document viewer, live print preview and the settings panel.

use crate::compose::{self, StampCtx};
use crate::i18n::{self, tr, Lang};
use crate::install;
use crate::job::{self, JobHandle, JobMsg, Target};
use crate::layout::{self, Sheet};
use crate::manual::{self, Manual};
use crate::pages;
use crate::prefs::{LangPref, Prefs, Theme};
use crate::printer::{self, Caps, Geometry, PrinterInfo};
use crate::settings::*;
use crate::template::{self, Clock, Vars};
use crate::text;
use crate::theme::{self, pal, ACCENT};
use crate::trf;
use crate::update::{self, Updater};
use crate::worker::{self, PageSrc, Req, Resp, WHOLE};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ColorImage, FontId, Key, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Ui,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};

const THUMB_W: u32 = 180;

pub fn run(file: Option<PathBuf>, quick: bool, prefs: Prefs) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PDF Print Studio")
        .with_inner_size(prefs.window_size)
        .with_min_inner_size(MIN_WINDOW)
        .with_maximized(prefs.start_maximized)
        .with_drag_and_drop(true);
    if let Some(p) = prefs.window_pos {
        viewport = viewport.with_position(p);
    }
    let (rgba, width, height) = icon_rgba();
    viewport = viewport.with_icon(std::sync::Arc::new(egui::IconData { rgba, width, height }));
    let opts = eframe::NativeOptions { viewport, centered: prefs.window_pos.is_none(), ..Default::default() };
    eframe::run_native(
        "PDF Print Studio",
        opts,
        Box::new(move |cc| Ok(Box::new(App::new(cc, file, quick, prefs)))),
    )
}

const MIN_WINDOW: [f32; 2] = [1100.0, 650.0];

/// The app icon as RGBA pixels.
fn icon_rgba() -> (Vec<u8>, u32, u32) {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).expect("embedded icon").to_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Preview,
    Viewer,
}

struct JobUi {
    handle: JobHandle,
    done: usize,
    total: usize,
    label: String,
    need_flip: Option<usize>,
    export: bool,
}

pub struct App {
    tx: Sender<Req>,
    rx: Receiver<Resp>,
    path: Option<PathBuf>,
    password: String,
    need_password: bool,
    open_error: Option<String>,
    /// Sizes of the arranged pages (what the rest of the app calls "pages").
    sizes_mm: Vec<(f32, f32)>,
    /// Sizes of the pages in the PDF file.
    src_sizes: Vec<(f32, f32)>,
    /// Page arrangement: which PDF page (or part of one) each page is.
    order: Vec<PageSrc>,
    history: Vec<Vec<PageSrc>>,
    /// Thumbnail being dragged.
    drag: Option<usize>,
    /// The open document is a comic book archive.
    is_cbz: bool,
    cbz_meta: Vec<(String, String)>,
    /// CBZ options the open archive was loaded / arranged with.
    cbz_load_key: String,
    cbz_arrange_key: String,
    loading: bool,

    thumbs: HashMap<usize, TextureHandle>,
    thumb_req: HashSet<usize>,
    current: usize,
    picked: BTreeSet<usize>,
    scroll_thumb: bool,

    view_tex: Option<(usize, TextureHandle)>,
    view_req: Option<(usize, u32)>,
    view_zoom: f32,

    tab: Tab,
    s: Settings,

    printers: Vec<PrinterInfo>,
    caps: Option<Caps>,
    devmode: Vec<u8>,
    geom: Option<Geometry>,
    geom_key: String,

    selection: Result<Vec<usize>, String>,
    sel_set: HashSet<usize>,
    sheets: Vec<Sheet>,
    sheet_mm: (f32, f32),
    landscape: bool,
    hw: [f32; 4],
    layout_key: String,

    preview_idx: usize,
    preview_tex: Option<TextureHandle>,
    preview_gen: u64,
    preview_key: String,
    preview_busy: bool,
    coverage: Option<f32>,
    show_printable: bool,
    show_cells: bool,
    preview_zoom: f32,

    fonts: Vec<String>,
    presets: Vec<String>,
    preset_name: String,
    link_margins: bool,

    job: Option<JobUi>,
    toast: Option<(String, bool, f64)>,
    quick: bool,
    quick_started: bool,
    inst: install::Status,
    prefs: Prefs,
    prefs_open: bool,
    prefs_tab: PrefsTab,
    logo: TextureHandle,
    updater: Updater,
    manual: Manual,
    hwnd: isize,
    egui_ctx: egui::Context,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>, quick: bool, prefs: Prefs) -> Self {
        setup_style(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.theme);
        let (tx, rx) = worker::spawn(cc.egui_ctx.clone());
        let mut s = load_last().unwrap_or_default();
        if !quick {
            // a fresh session starts from "all pages" but keeps everything else
            s.range_mode = RangeMode::All;
        }
        let mut app = Self {
            tx,
            rx,
            path: None,
            password: String::new(),
            need_password: false,
            open_error: None,
            sizes_mm: Vec::new(),
            src_sizes: Vec::new(),
            order: Vec::new(),
            history: Vec::new(),
            drag: None,
            is_cbz: false,
            cbz_meta: Vec::new(),
            cbz_load_key: String::new(),
            cbz_arrange_key: String::new(),
            loading: false,
            thumbs: HashMap::new(),
            thumb_req: HashSet::new(),
            current: 0,
            picked: BTreeSet::new(),
            scroll_thumb: false,
            view_tex: None,
            view_req: None,
            view_zoom: 1.0,
            tab: Tab::Preview,
            s,
            printers: Vec::new(),
            caps: None,
            devmode: Vec::new(),
            geom: None,
            geom_key: String::new(),
            selection: Ok(Vec::new()),
            sel_set: HashSet::new(),
            sheets: Vec::new(),
            sheet_mm: (210.0, 297.0),
            landscape: false,
            hw: [0.0; 4],
            layout_key: String::new(),
            preview_idx: 0,
            preview_tex: None,
            preview_gen: 0,
            preview_key: String::new(),
            preview_busy: false,
            coverage: None,
            show_printable: true,
            show_cells: false,
            preview_zoom: 1.0,
            fonts: text::list_fonts(),
            presets: list_presets(),
            preset_name: String::new(),
            link_margins: true,
            job: None,
            toast: None,
            quick,
            quick_started: false,
            inst: install::status(),
            prefs,
            prefs_open: false,
            prefs_tab: PrefsTab::General,
            logo: {
                let (rgba, w, h) = icon_rgba();
                cc.egui_ctx.load_texture("logo", ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), TextureOptions::LINEAR)
            },
            updater: Updater::default(),
            manual: Manual::default(),
            hwnd: 0,
            egui_ctx: cc.egui_ctx.clone(),
        };
        app.refresh_printers();
        // quick print exits right after the job; no point checking then
        if !quick && app.prefs.check_updates {
            app.updater.check_in_background(&cc.egui_ctx, app.prefs.auto_update);
        }
        if let Some(f) = file {
            app.open(f);
        }
        app
    }

    // ------------------------------------------------------------ document

    fn open(&mut self, path: PathBuf) {
        self.path = Some(path.clone());
        self.sizes_mm.clear();
        self.order.clear();
        self.history.clear();
        self.thumbs.clear();
        self.thumb_req.clear();
        self.picked.clear();
        self.current = 0;
        self.preview_idx = 0;
        self.view_tex = None;
        self.view_req = None;
        self.preview_tex = None;
        self.preview_key.clear();
        self.layout_key.clear();
        self.open_error = None;
        self.loading = true;
        let pw = (!self.password.is_empty()).then(|| self.password.clone());
        self.cbz_load_key = self.s.cbz.load_key();
        let _ = self.tx.send(Req::Open { path, password: pw, cbz: self.s.cbz.clone() });
    }

    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.rx.try_recv() {
            match r {
                Resp::Opened(info) => {
                    self.loading = false;
                    self.need_password = false;
                    self.order = PageSrc::identity(info.sizes_mm.len());
                    self.sizes_mm = info.sizes_mm.clone();
                    self.src_sizes = info.sizes_mm;
                    self.history.clear();
                    self.is_cbz = info.cbz;
                    self.cbz_meta = info.meta;
                    self.cbz_arrange_key.clear();
                    if self.is_cbz && self.cbz_meta.iter().any(|(k, v)| k == "Manga" && v == "YesAndRightToLeft") {
                        self.set_rtl(true);
                    }
                    self.path = Some(info.path);
                    self.layout_key.clear();
                }
                Resp::OpenFailed { need_password, msg } => {
                    self.loading = false;
                    self.need_password = need_password;
                    if !need_password {
                        self.open_error = Some(msg);
                    }
                }
                Resp::Thumb { page, img } => {
                    let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    self.thumbs.insert(page, ctx.load_texture(format!("thumb{page}"), ci, TextureOptions::LINEAR));
                }
                Resp::View { page, img } => {
                    let ci = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    self.view_tex = Some((page, ctx.load_texture("view", ci, TextureOptions::LINEAR)));
                }
                Resp::Sheet { gen, img, coverage } => {
                    if gen == self.preview_gen {
                        let ci = ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw());
                        self.preview_tex = Some(ctx.load_texture("sheet", ci, TextureOptions::LINEAR));
                        self.coverage = Some(coverage);
                        self.preview_busy = false;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ printers

    fn refresh_printers(&mut self) {
        self.printers = printer::list_printers();
        let keep = self.printers.iter().any(|p| p.name == self.s.printer);
        let name = if keep {
            self.s.printer.clone()
        } else {
            self.printers.first().map(|p| p.name.clone()).unwrap_or_default()
        };
        self.select_printer(&name);
    }

    fn select_printer(&mut self, name: &str) {
        self.s.printer = name.to_string();
        self.devmode = printer::default_devmode(name).unwrap_or_default();
        self.caps = self
            .printers
            .iter()
            .find(|p| p.name == name)
            .map(|p| printer::capabilities(p, &self.devmode));
        if let Some(c) = &self.caps {
            if self.s.paper_id != 0 && self.s.paper_id != 256 && !c.papers.iter().any(|p| p.id == self.s.paper_id) {
                self.s.paper_id = 0;
            }
            if self.s.bin != 0 && !c.bins.iter().any(|b| b.0 == self.s.bin) {
                self.s.bin = 0;
            }
            if self.s.media != 0 && !c.media.iter().any(|m| m.0 == self.s.media) {
                self.s.media = 0;
            }
            if let Quality::Res(x, y) = self.s.quality {
                if !c.resolutions.contains(&(x, y)) {
                    self.s.quality = Quality::Default;
                }
            }
        }
        self.geom_key.clear();
        self.layout_key.clear();
    }

    fn current_printer(&self) -> Option<&PrinterInfo> {
        self.printers.iter().find(|p| p.name == self.s.printer)
    }

    fn paper_portrait_mm(&self) -> (f32, f32) {
        let (w, h) = if self.s.paper_id == 256 {
            (self.s.custom_paper_mm[0], self.s.custom_paper_mm[1])
        } else {
            let id = if self.s.paper_id == 0 { printer::read_devmode(&self.devmode).paper } else { self.s.paper_id };
            self.caps
                .as_ref()
                .and_then(|c| c.papers.iter().find(|p| p.id == id))
                .map(|p| (p.w_mm, p.h_mm))
                .unwrap_or((210.0, 297.0))
        };
        (w.min(h).max(10.0), w.max(h).max(10.0))
    }

    // ------------------------------------------------------------ layout pipeline

    fn recompute(&mut self) {
        let key = format!(
            "{}|{:?}|{}|{}|{}",
            serde_json::to_string(&self.s).unwrap_or_default(),
            self.picked,
            self.current,
            self.sizes_mm.len(),
            self.devmode.len()
        );
        if key == self.layout_key {
            return;
        }
        self.layout_key = key;
        let n = self.sizes_mm.len();
        self.selection = pages::compute_selection(&self.s, n, self.current, &self.picked);
        let sel = self.selection.clone().unwrap_or_default();
        self.sel_set = sel.iter().copied().collect();
        let paper = self.paper_portrait_mm();
        let sizes = self.sizes_mm.clone();
        let page_mm = move |p: usize| sizes.get(p).copied().unwrap_or((210.0, 297.0));
        self.landscape = layout::choose_landscape(&sel, &page_mm, paper, &self.s, self.hw);

        let gkey = format!(
            "{}|{}|{:?}|{}|{}|{:?}|{}",
            self.s.printer, self.s.paper_id, self.s.custom_paper_mm, self.landscape, self.s.bin, self.s.quality, self.devmode.len()
        );
        if gkey != self.geom_key {
            self.geom_key = gkey;
            let mut dm = self.devmode.clone();
            printer::apply_settings(&mut dm, &self.s, self.landscape, self.caps.as_ref());
            self.geom = if self.s.printer.is_empty() { None } else { printer::geometry(&self.s.printer, &dm) };
        }
        let fallback = if self.landscape { (paper.1, paper.0) } else { paper };
        match self.geom {
            Some(g) => {
                let gm = g.sheet_mm();
                // trust the driver only if it agrees with the requested orientation
                if (gm.0 > gm.1) == (fallback.0 > fallback.1) || (gm.0 - gm.1).abs() < 1.0 {
                    self.sheet_mm = gm;
                    self.hw = g.hw_margins();
                } else {
                    self.sheet_mm = fallback;
                    let m = g.hw_margins();
                    self.hw = [m[3], m[0], m[1], m[2]];
                }
            }
            None => {
                self.sheet_mm = fallback;
                self.hw = [0.0; 4];
            }
        }
        self.sheets = layout::build_sheets(&sel, self.sheet_mm, &self.s, self.hw);
        if self.preview_idx >= self.sheets.len() {
            self.preview_idx = self.sheets.len().saturating_sub(1);
        }
    }

    fn stamp(&self, sheet_no: usize) -> StampCtx {
        StampCtx {
            file: self.file_name(),
            path: self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            doc_pages: self.sizes_mm.len(),
            selected: self.selection.as_ref().map_or(0, |v| v.len()),
            sheet_no,
            sheet_total: self.sheets.len(),
            copy: 1,
            copies: self.s.copies.max(1) as usize,
            printer: self.s.printer.clone(),
            clock: Clock::now(),
        }
    }

    /// Template variables for the side shown in the print preview.
    fn preview_vars(&self) -> Vars {
        compose::template_vars(&self.stamp(self.preview_idx + 1), self.sheets.get(self.preview_idx), &self.s, None)
    }

    fn request_preview(&mut self, display_h_px: f32) {
        let Some(sheet) = self.sheets.get(self.preview_idx).cloned() else { return };
        let dpi = ((display_h_px / (self.sheet_mm.1 / 25.4)).clamp(25.0, 250.0) / 5.0).round() * 5.0;
        let key = format!("{}|{}|{}|{:?}", self.layout_key, self.preview_idx, dpi, self.sheet_mm);
        if key == self.preview_key {
            return;
        }
        self.preview_key = key;
        self.preview_gen += 1;
        self.preview_busy = true;
        let _ = self.tx.send(Req::Sheet {
            gen: self.preview_gen,
            sheet,
            sheet_mm: self.sheet_mm,
            dpi,
            settings: Box::new(self.s.clone()),
            ctx: self.stamp(self.preview_idx + 1),
        });
    }

    // ------------------------------------------------------------ jobs

    fn start_job(&mut self, export_dir: Option<PathBuf>) {
        let Some(path) = self.path.clone() else { return };
        let sel = match &self.selection {
            Ok(v) if !v.is_empty() => v.clone(),
            _ => {
                self.notify(tr("Nothing selected to print"), true);
                return;
            }
        };
        let export = export_dir.is_some();
        let target = match export_dir {
            Some(dir) => Target::PngFolder { dir, sheet_mm: self.sheet_mm, hw: self.hw },
            None => {
                if self.s.printer.is_empty() || self.devmode.is_empty() {
                    self.notify(tr("No printer available"), true);
                    return;
                }
                Target::Printer { name: self.s.printer.clone(), devmode: self.devmode.clone(), caps: self.caps.clone() }
            }
        };
        save_last(&self.s);
        let spec = job::JobSpec {
            path,
            password: (!self.password.is_empty()).then(|| self.password.clone()),
            settings: self.s.clone(),
            order: self.order.clone(),
            selection: sel,
            landscape: self.landscape,
            target,
            stamp: self.stamp(1),
        };
        let handle = job::spawn(spec, self.egui_ctx.clone());
        self.job = Some(JobUi { handle, done: 0, total: 0, label: tr("Starting…").into(), need_flip: None, export });
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), error, 0.0));
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(j) = self.job.as_mut() else { return };
        let mut finished = None;
        while let Ok(m) = j.handle.rx.try_recv() {
            match m {
                JobMsg::Progress { done, total, label } => {
                    j.done = done;
                    j.total = total;
                    j.label = label;
                }
                JobMsg::NeedFlip { sheets } => j.need_flip = Some(sheets),
                JobMsg::Done(r) => finished = Some(r),
            }
        }
        if let Some(r) = finished {
            self.job = None;
            match r {
                Ok(m) => self.notify(m, false),
                Err(e) => self.notify(e, true),
            }
            if self.quick {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}


fn setup_style(ctx: &egui::Context) {
    // Windows symbol/emoji fonts as fallbacks so icons like 🖨 ▯ → render
    let mut fonts = egui::FontDefinitions::default();
    let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for (key, file) in [("segoe_symbol", "seguisym.ttf"), ("segoe_emoji", "seguiemj.ttf")] {
        if let Ok(data) = std::fs::read(format!("{win}\\Fonts\\{file}")) {
            fonts.font_data.insert(key.into(), egui::FontData::from_owned(data));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(key.into());
            }
        }
    }
    ctx.set_fonts(fonts);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(8.0, 4.0);
        s.spacing.interact_size.y = 22.0;
    });
}

// ------------------------------------------------------------ small widget helpers

fn combo<T: PartialEq + Copy>(ui: &mut Ui, id: &str, val: &mut T, opts: &[(T, &str)]) {
    let sel = opts.iter().find(|o| o.0 == *val).map(|o| o.1).unwrap_or("—");
    egui::ComboBox::from_id_salt(id).selected_text(sel).width(210.0).show_ui(ui, |ui| {
        for (v, l) in opts {
            ui.selectable_value(val, *v, *l);
        }
    });
}

fn mm(ui: &mut Ui, v: &mut f32, max: f32) {
    ui.add(egui::DragValue::new(v).range(-max..=max).speed(0.25).suffix(" mm").max_decimals(1));
}

fn section(ui: &mut Ui, icon: &str, title: &str, open: bool, add: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(format!("{icon}  {title}")).strong().size(14.5))
        .default_open(open)
        .show(ui, |ui| {
            ui.add_space(2.0);
            add(ui);
            ui.add_space(4.0);
        });
    ui.separator();
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 7.0]).min_col_width(105.0).show(ui, add);
}

fn hint(ui: &mut Ui, t: &str) {
    ui.label(RichText::new(t).small().color(pal().muted));
}

fn help_button(ui: &mut Ui) -> bool {
    ui.small_button("❓").on_hover_text(tr("Open the manual")).clicked()
}

/// Header/footer presets: (name, left, center, right).
const BAND_PRESETS: &[(&str, &str, &str, &str)] = &[
    ("Page X of Y", "", "Page {range} of {pages}", ""),
    ("File · date / time", "{file}", "", "{date} {time}"),
    ("Book: title + outer page number", "{name}", "", "{page}"),
    ("Skip first side", "", "{if not first}{page}{end}", ""),
    ("Odd/even alternating", "{if odd}{name}{end}", "", "{if even}{date}{end}"),
    ("Roman numbers", "", "{page|roman|lower}", ""),
    ("Printed by…", "Printed by {user} on {computer}", "", "{weekday}, {day} {monthname} {year}"),
    ("Confidential", "CONFIDENTIAL", "{name}", "{page}/{pages}"),
    ("Clear", "", "", ""),
];

/// Stamp presets: (name, text, position, frame, angle, color).
const STAMP_PRESETS: &[(&str, &str, u8, bool, f32, [u8; 3])] = &[
    ("Bates number (bottom right)", "{sheet|pad:6}", 8, false, 0.0, [0, 0, 0]),
    ("Page x/y (top right)", "{page}/{pages}", 2, false, 0.0, [60, 60, 60]),
    ("COPY box (top right)", "{if copy > 1}COPY {copy}{else}COPY{end}", 2, true, 0.0, [200, 30, 30]),
    ("APPROVED + date", "APPROVED{br}{date}", 5, true, 8.0, [20, 120, 40]),
    ("Received stamp", "RECEIVED{br}{day|pad:2}.{month|pad:2}.{year}", 6, true, -6.0, [30, 60, 190]),
    ("File path (bottom left)", "{path}", 6, false, 0.0, [110, 110, 110]),
    ("Back-side marker", "{if back}↻ back{end}", 0, false, 0.0, [120, 120, 120]),
    ("Empty stamp", "", 4, false, 0.0, [0, 0, 0]),
];

fn font_pick(ui: &mut Ui, id: &str, val: &mut String, fonts: &[String], allow_default: bool) {
    let shown = if val.is_empty() { tr("(overlay font)") } else { val.as_str() };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(220.0).height(360.0).show_ui(ui, |ui| {
        if allow_default {
            ui.selectable_value(val, String::new(), tr("(overlay font)"));
        }
        for f in fonts {
            ui.selectable_value(val, f.clone(), f);
        }
    });
}

fn pos_pick(ui: &mut Ui, pos: &mut u8) {
    ui.vertical(|ui| {
        let arrows = [["↖", "↑", "↗"], ["←", "●", "→"], ["↙", "↓", "↘"]];
        for (v, row) in arrows.iter().enumerate() {
            ui.horizontal(|ui| {
                for (h, a) in row.iter().enumerate() {
                    let p = (v * 3 + h) as u8;
                    if ui.add(egui::SelectableLabel::new(*pos == p, *a)).clicked() {
                        *pos = p;
                    }
                }
            });
        }
    });
}

/// Red error line under a template, if it has a problem.
fn tpl_status(ui: &mut Ui, src: &str, vars: &Vars) {
    if let Some(e) = template::check(src, vars) {
        ui.label(RichText::new(format!("⚠ {e}")).small().color(pal().err));
    }
}

/// Template field: hover shows the result for the previewed side, errors appear below.
fn tpl_edit(ui: &mut Ui, src: &mut String, vars: &Vars) {
    ui.vertical(|ui| {
        let r = ui.add(egui::TextEdit::singleline(src).desired_width(240.0).font(egui::TextStyle::Monospace));
        if r.hovered() && src.contains('{') {
            let out = template::render(src, vars);
            r.on_hover_text(trf!("On this side: {}", if out.is_empty() { tr("(empty)") } else { &out }));
        }
        tpl_status(ui, src, vars);
    });
}

/// "Show when" condition with common choices and a live verdict for the previewed side.
fn cond_edit(ui: &mut Ui, id: &str, src: &mut String, vars: &Vars) {
    const CHOICES: [(&str, &str); 10] = [
        ("Always", ""),
        ("Not on the first side", "not first"),
        ("Only on the first side", "first"),
        ("Only on the last side", "last"),
        ("Odd sides", "odd"),
        ("Even sides", "even"),
        ("Front sides (duplex)", "front"),
        ("Back sides (duplex)", "back"),
        ("Only the first copy", "copy = 1"),
        ("Pages 2 to 5", "page in 2..5"),
    ];
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(src).hint_text(tr("always")).desired_width(170.0).font(egui::TextStyle::Monospace));
            egui::ComboBox::from_id_salt(id).selected_text("▾").width(30.0).show_ui(ui, |ui| {
                for (label, c) in CHOICES {
                    if ui.selectable_label(src.trim() == c, tr(label)).clicked() {
                        *src = c.to_string();
                    }
                }
            });
            if !src.trim().is_empty() {
                let (txt, col) = if template::condition(src, vars) {
                    (tr("✔ shown"), pal().ok)
                } else {
                    (tr("✖ hidden"), pal().muted)
                };
                ui.label(RichText::new(txt).small().color(col)).on_hover_text(tr("On the side shown in the preview"));
            }
        });
        if let Some(e) = template::check_condition(src, vars) {
            ui.label(RichText::new(format!("⚠ {e}")).small().color(pal().err));
        }
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {

        if self.hwnd == 0 {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.hwnd = w.hwnd.get();
                }
            }
        }
        self.poll_worker(ctx);
        self.poll_job(ctx);
        self.cbz_sync(ctx);
        self.recompute();

        // drag & drop
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(p) = dropped {
            self.password.clear();
            self.open(p);
        }

        // quick print mode: print as soon as everything is ready
        if self.quick && !self.quick_started && !self.sizes_mm.is_empty() && !self.sheets.is_empty() {
            self.quick_started = true;
            self.start_job(None);
        }

        self.handle_keys(ctx);
        self.ui_top(ctx);
        self.ui_status(ctx);
        if self.path.is_some() && !self.sizes_mm.is_empty() {
            self.ui_settings(ctx);
            self.ui_thumbs(ctx);
        }
        self.ui_central(ctx);
        self.ui_modals(ctx);
        if self.manual.open {
            let vars = self.preview_vars();
            self.manual.show(ctx, &vars);
        }
        self.ui_prefs(ctx);
        self.updater.show(ctx, self.path.as_ref());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        save_last(&self.s);
    }
}

impl App {
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.manual.open = !self.manual.open;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Z)) {
            self.undo();
        }
        let (next, prev, home, end, print) = ctx.input(|i| {
            (
                i.key_pressed(Key::PageDown) || i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::PageUp) || i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.modifiers.command && i.key_pressed(Key::P),
            )
        });
        match self.tab {
            Tab::Preview => {
                let n = self.sheets.len();
                if next && self.preview_idx + 1 < n {
                    self.preview_idx += 1;
                }
                if prev && self.preview_idx > 0 {
                    self.preview_idx -= 1;
                }
                if home {
                    self.preview_idx = 0;
                }
                if end {
                    self.preview_idx = n.saturating_sub(1);
                }
            }
            Tab::Viewer => {
                let n = self.sizes_mm.len();
                if next && self.current + 1 < n {
                    self.current += 1;
                    self.scroll_thumb = true;
                }
                if prev && self.current > 0 {
                    self.current -= 1;
                    self.scroll_thumb = true;
                }
                if home {
                    self.current = 0;
                }
                if end {
                    self.current = n.saturating_sub(1);
                }
            }
        }
        if print && self.job.is_none() {
            self.start_job(None);
        }
    }

    // ------------------------------------------------------------ top bar

    fn ui_top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").exact_height(42.0).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(24.0, 24.0)));
                ui.label(RichText::new("PDF Print Studio").strong().size(17.0).color(ACCENT));
                ui.separator();
                if ui.button(tr("📂 Open…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter(tr("PDF or comic book (CBZ)"), &["pdf", "cbz"]).pick_file() {
                        self.password.clear();
                        self.open(p);
                    }
                }
                if !self.sizes_mm.is_empty() {
                    ui.label(RichText::new(self.file_name()).strong());
                    ui.label(RichText::new(trf!("{} pages", self.sizes_mm.len())).color(Color32::GRAY));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("❓").size(16.0)).frame(false))
                        .on_hover_text(tr("Manual & template help (F1)"))
                        .clicked()
                    {
                        self.manual.open = !self.manual.open;
                    }
                    if ui.button(tr("⚙ Settings")).on_hover_text(tr("Language, theme, window, installation and updates")).clicked() {
                        self.prefs_open = !self.prefs_open;
                        self.inst = install::status();
                    }
                });
            });
        });
    }

    fn ui_status(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").exact_height(26.0).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(p) = &self.path {
                    ui.label(RichText::new(p.display().to_string()).small().color(Color32::GRAY));
                }
                if let Some(&(w, h)) = self.sizes_mm.get(self.current) {
                    ui.separator();
                    ui.label(RichText::new(trf!("Page {}: {:.0} × {:.0} mm", self.current + 1, w, h)).small());
                }
                ui.separator();
                ui.label(
                    RichText::new(trf!(
                        "Sheet {:.0} × {:.0} mm ({})",
                        self.sheet_mm.0,
                        self.sheet_mm.1,
                        if self.landscape { tr("landscape") } else { tr("portrait") }
                    ))
                    .small(),
                );
                let now = ui.input(|i| i.time);
                if let Some((msg, err, t0)) = &mut self.toast {
                    if *t0 == 0.0 {
                        *t0 = now;
                    }
                    if now - *t0 < 8.0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let c = if *err { pal().err } else { pal().ok };
                            ui.label(RichText::new(msg.as_str()).color(c));
                        });
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    } else {
                        self.toast = None;
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ thumbnails

    fn ui_thumbs(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("thumbs").resizable(true).default_width(190.0).width_range(130.0..=340.0).show(ctx, |ui| {
            ui.add_space(4.0);
            let mut op = None;
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Pages")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("⋯", |ui| {
                        if ui.button(tr("✂ Split all pages in half")).clicked() {
                            op = Some(PageOp::SplitAll);
                            ui.close_menu();
                        }
                        if ui.add_enabled(!self.picked.is_empty(), egui::Button::new(trf!("🗑 Delete picked pages ({})", self.picked.len()))).clicked() {
                            op = Some(PageOp::DeletePicked);
                            ui.close_menu();
                        }
                        if ui.add_enabled(self.arranged(), egui::Button::new(tr("⟲ Restore original pages"))).clicked() {
                            op = Some(PageOp::Reset);
                            ui.close_menu();
                        }
                    });
                    if ui.add_enabled(!self.history.is_empty(), egui::Button::new("↶").small()).on_hover_text(tr("Undo (Ctrl+Z)")).clicked() {
                        self.undo();
                    }
                });
            });
            hint(ui, tr("drag = move · right-click = edit · ctrl+click = pick"));
            if self.arranged() {
                ui.label(RichText::new(tr("Pages rearranged — for printing only, the PDF file is not changed.")).small().color(pal().info));
            }
            ui.add_space(2.0);
            let n = self.sizes_mm.len();
            let w = (ui.available_width() - 14.0).max(60.0);
            let row_h = w * 1.2 + 26.0;
            let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if self.scroll_thumb {
                let spacing = ui.spacing().item_spacing.y;
                scroll = scroll.vertical_scroll_offset(((row_h + spacing) * self.current as f32 - 120.0).max(0.0));
                self.scroll_thumb = false;
            }
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let released = ui.input(|i| i.pointer.any_released());
            let mut drop_at = None;
            scroll.show_rows(ui, row_h, n, |ui, range| {
                // auto-scroll while dragging near the top or bottom edge
                if let (Some(_), Some(p)) = (self.drag, pointer) {
                    let clip = ui.clip_rect();
                    if p.y < clip.top() + 40.0 {
                        ui.scroll_with_delta(vec2(0.0, 10.0));
                    } else if p.y > clip.bottom() - 40.0 {
                        ui.scroll_with_delta(vec2(0.0, -10.0));
                    }
                    ui.ctx().request_repaint();
                }
                for i in range {
                    let src = self.order[i];
                    if let PageSrc::Pdf(s, _) = src {
                        if !self.thumbs.contains_key(&s) && self.thumb_req.insert(s) {
                            let _ = self.tx.send(Req::Thumb { page: s, width: THUMB_W });
                        }
                    }
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click_and_drag());
                    if resp.drag_started() {
                        self.drag = Some(i);
                    }
                    let p = ui.painter_at(rect.expand(3.0));
                    let is_cur = i == self.current;
                    let in_sel = self.sel_set.contains(&i);
                    let picked = self.picked.contains(&i);
                    let dragged = self.drag == Some(i);
                    if is_cur {
                        p.rect_filled(rect, 6.0, ACCENT.linear_multiply(0.18));
                    } else if resp.hovered() {
                        p.rect_filled(rect, 6.0, pal().hover);
                    }
                    let img_area = egui::Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(w - 16.0, row_h - 30.0));
                    let (pw, ph) = self.sizes_mm[i];
                    let k = (img_area.width() / pw).min(img_area.height() / ph);
                    let sz = vec2(pw * k, ph * k);
                    let r = egui::Rect::from_center_size(img_area.center(), sz);
                    p.rect_filled(r.translate(vec2(2.0, 2.0)), 1.0, Color32::from_black_alpha(90));
                    let tint = if dragged { Color32::from_gray(90) } else if in_sel { Color32::WHITE } else { Color32::from_gray(110) };
                    match src {
                        PageSrc::Pdf(s, reg) => match self.thumbs.get(&s) {
                            Some(t) => {
                                p.image(t.id(), r, egui::Rect::from_min_max(pos2(reg[0], reg[1]), pos2(reg[2], reg[3])), tint);
                            }
                            None => {
                                p.rect_filled(r, 1.0, Color32::from_gray(200));
                            }
                        },
                        PageSrc::Blank(..) => {
                            p.rect_filled(r, 1.0, tint);
                        }
                    }
                    let border = if picked { Color32::from_rgb(255, 190, 60) } else if is_cur { ACCENT } else { pal().thumb_border };
                    p.rect_stroke(r, 1.0, Stroke::new(if picked || is_cur { 2.0_f32 } else { 1.0 }, border));
                    let label_pos = pos2(rect.center().x, rect.bottom() - 12.0);
                    p.text(label_pos, Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(12.0), if is_cur { ACCENT } else { pal().thumb_label });
                    if in_sel {
                        p.text(pos2(r.right() - 4.0, r.top() + 4.0), Align2::RIGHT_TOP, "✔", FontId::proportional(13.0), Color32::from_rgb(70, 190, 100));
                    }
                    if picked {
                        p.text(pos2(r.left() + 4.0, r.top() + 4.0), Align2::LEFT_TOP, "★", FontId::proportional(13.0), Color32::from_rgb(255, 190, 60));
                    }
                    // drop target: insertion line above or below this page
                    if let (Some(_), Some(ptr)) = (self.drag, pointer) {
                        if rect.contains(ptr) {
                            let before = ptr.y < rect.center().y;
                            drop_at = Some(if before { i } else { i + 1 });
                            let y = if before { rect.top() - 1.0 } else { rect.bottom() + 1.0 };
                            p.line_segment([pos2(rect.left() + 4.0, y), pos2(rect.right() - 4.0, y)], Stroke::new(3.0_f32, ACCENT));
                        }
                    }
                    if resp.clicked() {
                        if ui.input(|i| i.modifiers.command) {
                            if !self.picked.remove(&i) {
                                self.picked.insert(i);
                            }
                            self.s.range_mode = RangeMode::Picked;
                        } else {
                            self.current = i;
                            if let Some(si) = self.sheets.iter().position(|s| s.placements.iter().any(|p| p.page == i)) {
                                self.preview_idx = si;
                            }
                        }
                    }
                    if resp.double_clicked() {
                        self.tab = Tab::Viewer;
                    }
                    let landscape = pw > ph;
                    resp.context_menu(|ui| {
                        if let Some(o) = page_menu(ui, i, n, landscape) {
                            op = Some(o);
                        }
                    });
                    if self.drag.is_none() {
                        let tip = match src {
                            PageSrc::Blank(..) => trf!("Page {} — blank, {:.0} × {:.0} mm", i + 1, pw, ph),
                            _ => trf!("Page {} — {:.0} × {:.0} mm", i + 1, pw, ph),
                        };
                        resp.on_hover_text(tip);
                    }
                }
            });
            if let Some(from) = self.drag {
                if let Some(ptr) = pointer {
                    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("page_drag")));
                    let label = trf!("Page {}", from + 1);
                    let galley = painter.layout_no_wrap(label, FontId::proportional(13.0), Color32::WHITE);
                    let box_rect = egui::Rect::from_min_size(ptr + vec2(14.0, 6.0), galley.size() + vec2(12.0, 6.0));
                    painter.rect_filled(box_rect, 4.0, ACCENT);
                    painter.galley(box_rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
                }
                if released {
                    self.drag = None;
                    if let Some(to) = drop_at {
                        op = Some(PageOp::Move(from, to));
                    }
                }
            }
            if let Some(o) = op {
                self.apply_op(o);
            }
        });
    }

    // ------------------------------------------------------------ page arrangement

    fn arranged(&self) -> bool {
        self.order != PageSrc::identity(self.src_sizes.len())
    }

    /// Install a new arrangement (undoable) and pass it on to the worker.
    fn set_order(&mut self, order: Vec<PageSrc>) {
        if order == self.order {
            return;
        }
        self.history.push(std::mem::replace(&mut self.order, order));
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.sync_order();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.order = prev;
            self.picked.clear();
            self.sync_order();
        }
    }

    fn sync_order(&mut self) {
        self.sizes_mm = self.order.iter().map(|p| p.size_mm(&self.src_sizes)).collect();
        let _ = self.tx.send(Req::Arrange(self.order.clone()));
        self.current = self.current.min(self.order.len().saturating_sub(1));
        self.picked.retain(|&p| p < self.order.len());
        if self.picked.is_empty() && self.s.range_mode == RangeMode::Picked {
            self.s.range_mode = RangeMode::All;
        }
        self.layout_key.clear();
        self.preview_key.clear();
    }

    fn apply_op(&mut self, op: PageOp) {
        let mut o = self.order.clone();
        let n = o.len();
        match op {
            PageOp::Move(from, to) => {
                if from >= n {
                    return;
                }
                let page = o.remove(from);
                let to = if to > from { to - 1 } else { to }.min(o.len());
                o.insert(to, page);
                self.current = to;
            }
            PageOp::Delete(i) => {
                if n <= 1 || i >= n {
                    return;
                }
                o.remove(i);
            }
            PageOp::DeletePicked => {
                let keep: Vec<PageSrc> = o.iter().enumerate().filter(|(i, _)| !self.picked.contains(i)).map(|(_, p)| *p).collect();
                if keep.is_empty() {
                    self.notify(tr("At least one page has to remain"), true);
                    return;
                }
                o = keep;
                self.current = 0;
            }
            PageOp::Duplicate(i) => {
                o.insert(i + 1, o[i]);
                self.current = i + 1;
            }
            PageOp::InsertBlank(i) => {
                let (w, h) = o[i].size_mm(&self.src_sizes);
                o.insert(i + 1, PageSrc::Blank(w, h));
                self.current = i + 1;
            }
            PageOp::Split(i) => {
                let halves = o[i].split(&self.src_sizes);
                o.splice(i..=i, halves);
            }
            PageOp::SplitAll => {
                o = o.iter().flat_map(|p| p.split(&self.src_sizes)).collect();
            }
            PageOp::Reset => o = PageSrc::identity(self.src_sizes.len()),
        }
        // page positions changed, so picks would point at other pages
        self.picked.clear();
        self.set_order(o);
    }
}

#[derive(Clone, Copy)]
enum PageOp {
    /// Move the page at `.0` so it lands before the page now at `.1`.
    Move(usize, usize),
    Delete(usize),
    DeletePicked,
    Duplicate(usize),
    InsertBlank(usize),
    Split(usize),
    SplitAll,
    Reset,
}

/// Right-click menu of a thumbnail.
fn page_menu(ui: &mut Ui, i: usize, n: usize, landscape: bool) -> Option<PageOp> {
    let mut op = None;
    let mut item = |ui: &mut Ui, enabled: bool, label: &str, o: PageOp| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            op = Some(o);
            ui.close_menu();
        }
    };
    item(ui, true, tr("⧉ Duplicate"), PageOp::Duplicate(i));
    item(ui, true, tr("➕ Insert blank page after"), PageOp::InsertBlank(i));
    let split = if landscape { tr("✂ Split into left and right halves") } else { tr("✂ Split into top and bottom halves") };
    item(ui, true, split, PageOp::Split(i));
    ui.separator();
    item(ui, i > 0, tr("⏫ Move to start"), PageOp::Move(i, 0));
    item(ui, i > 0, tr("🔼 Move up"), PageOp::Move(i, i - usize::from(i > 0)));
    item(ui, i + 1 < n, tr("🔽 Move down"), PageOp::Move(i, i + 2));
    item(ui, i + 1 < n, tr("⏬ Move to end"), PageOp::Move(i, n));
    ui.separator();
    item(ui, n > 1, tr("🗑 Delete"), PageOp::Delete(i));
    op
}

/// Millimetre rulers along the top and left edge of the visible area, aligned to `paper`.
fn draw_rulers(ui: &Ui, paper: egui::Rect, paper_mm: (f32, f32)) {
    const R: f32 = 20.0;
    let view = ui.clip_rect();
    let k = paper.width() / paper_mm.0.max(1.0);
    if !k.is_finite() || k <= 0.0 {
        return;
    }
    let p = ui.painter().with_clip_rect(view);
    let vis = ui.visuals();
    let (bg, paper_bg, fg) = (vis.faint_bg_color, vis.extreme_bg_color, vis.text_color());
    let tick = vis.weak_text_color();
    let line = vis.widgets.noninteractive.bg_stroke;
    let top = egui::Rect::from_min_size(view.min, vec2(view.width(), R));
    let left = egui::Rect::from_min_size(view.min, vec2(R, view.height()));
    p.rect_filled(top, 0.0, bg);
    p.rect_filled(left, 0.0, bg);
    p.rect_filled(egui::Rect::from_x_y_ranges(paper.x_range(), top.y_range()), 0.0, paper_bg);
    p.rect_filled(egui::Rect::from_x_y_ranges(left.x_range(), paper.y_range()), 0.0, paper_bg);

    let label_step = [5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0].into_iter().find(|s| s * k >= 48.0).unwrap_or(2000.0);
    let minor = [label_step / 10.0, label_step / 5.0, label_step / 2.0, label_step].into_iter().find(|s| s * k >= 5.0).unwrap_or(label_step);
    let font = FontId::proportional(9.5);
    let is_mult = |v: f32, step: f32| ((v / step).round() * step - v).abs() < 1e-3;

    // horizontal
    let first = ((view.left() + R - paper.left()) / k / minor).floor() as i64;
    let last = ((view.right() - paper.left()) / k / minor).ceil() as i64;
    for t in first..=last {
        let v = t as f32 * minor;
        let x = paper.left() + v * k;
        if x < view.left() + R {
            continue;
        }
        let len = if is_mult(v, label_step) { 0.55 } else if is_mult(v, label_step / 2.0) { 0.38 } else { 0.22 } * R;
        p.line_segment([pos2(x, top.bottom() - len), pos2(x, top.bottom())], Stroke::new(1.0_f32, tick));
        if is_mult(v, label_step) {
            p.text(pos2(x + 2.0, top.top() + 1.0), Align2::LEFT_TOP, format!("{}", v.round() as i64), font.clone(), fg);
        }
    }
    // vertical
    let first = ((view.top() + R - paper.top()) / k / minor).floor() as i64;
    let last = ((view.bottom() - paper.top()) / k / minor).ceil() as i64;
    for t in first..=last {
        let v = t as f32 * minor;
        let y = paper.top() + v * k;
        if y < view.top() + R {
            continue;
        }
        let len = if is_mult(v, label_step) { 0.55 } else if is_mult(v, label_step / 2.0) { 0.38 } else { 0.22 } * R;
        p.line_segment([pos2(left.right() - len, y), pos2(left.right(), y)], Stroke::new(1.0_f32, tick));
        if is_mult(v, label_step) {
            p.text(pos2(left.left() + 1.0, y + 1.0), Align2::LEFT_TOP, format!("{}", v.round() as i64), font.clone(), fg);
        }
    }
    p.line_segment([pos2(view.left(), top.bottom()), pos2(view.right(), top.bottom())], line);
    p.line_segment([pos2(left.right(), view.top()), pos2(left.right(), view.bottom())], line);
    p.rect_filled(egui::Rect::from_min_size(view.min, vec2(R, R)), 0.0, bg);
    p.text(view.min + vec2(R / 2.0, R / 2.0), Align2::CENTER_CENTER, "mm", FontId::proportional(8.5), tick);

    // pointer position on both rulers
    if let Some(h) = ui.ctx().pointer_hover_pos().filter(|h| view.contains(*h)) {
        let s = Stroke::new(1.5_f32, ACCENT);
        if h.x > view.left() + R {
            p.line_segment([pos2(h.x, top.top()), pos2(h.x, top.bottom())], s);
        }
        if h.y > view.top() + R {
            p.line_segment([pos2(left.left(), h.y), pos2(left.right(), h.y)], s);
        }
    }
}

impl App {

    // ------------------------------------------------------------ central

    fn ui_central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::none().fill(pal().canvas)).show(ctx, |ui| {
            if self.sizes_mm.is_empty() {
                self.ui_empty(ui);
                return;
            }
            egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 6.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Preview, RichText::new(tr("🖨  Print preview")).size(14.0));
                    ui.selectable_value(&mut self.tab, Tab::Viewer, RichText::new(tr("📄  Document viewer")).size(14.0));
                });
            });
            ui.separator();
            match self.tab {
                Tab::Preview => self.ui_preview(ui),
                Tab::Viewer => self.ui_viewer(ui),
            }
        });
    }

    fn ui_empty(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.28);
            if self.loading {
                ui.spinner();
                ui.label(tr("Opening document…"));
                return;
            }
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(96.0, 96.0)));
            ui.label(RichText::new(tr("Drop a PDF or comic book (CBZ) here, or open one")).size(22.0));
            ui.add_space(10.0);
            if ui.add(egui::Button::new(RichText::new(tr("📂  Open file…")).size(16.0)).min_size(vec2(180.0, 36.0))).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter(tr("PDF or comic book (CBZ)"), &["pdf", "cbz"]).pick_file() {
                    self.open(p);
                }
            }
            if let Some(e) = &self.open_error {
                ui.add_space(10.0);
                ui.label(RichText::new(trf!("Could not open file: {}", e)).color(pal().err));
            }
            ui.add_space(24.0);
            if !self.inst.program {
                hint(ui, tr("Tip: install the program and its Explorer entries in ⚙ Settings to open and print any PDF from Explorer."));
            }
        });
    }

    fn ui_preview(&mut self, ui: &mut Ui) {
        let n = self.sheets.len();
        egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 2.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(n > 0, |ui| {
                    if ui.button("⏮").clicked() {
                        self.preview_idx = 0;
                    }
                    if ui.button("◀").clicked() && self.preview_idx > 0 {
                        self.preview_idx -= 1;
                    }
                    let mut shown = self.preview_idx + 1;
                    ui.add(egui::DragValue::new(&mut shown).range(1..=n.max(1)).prefix(tr("Side ")).suffix(format!(" / {n}")));
                    self.preview_idx = shown.saturating_sub(1).min(n.saturating_sub(1));
                    if ui.button("▶").clicked() && self.preview_idx + 1 < n {
                        self.preview_idx += 1;
                    }
                    if ui.button("⏭").clicked() {
                        self.preview_idx = n.saturating_sub(1);
                    }
                });
                if let Some(sh) = self.sheets.get(self.preview_idx) {
                    let side = if self.s.is_duplex() || self.s.layout == LayoutMode::Booklet {
                        if sh.back { tr("back side") } else { tr("front side") }
                    } else {
                        tr("single-sided")
                    };
                    let pages = sh.pages().iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ");
                    // page list only on hover: with many pages per sheet it would push the controls off the toolbar
                    ui.label(RichText::new(side).color(Color32::GRAY)).on_hover_text(trf!("{} · pages {}", side, pages));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Fit")).clicked() {
                        self.preview_zoom = 1.0;
                    }
                    ui.add(egui::Slider::new(&mut self.preview_zoom, 0.5..=4.0).show_value(false).text(tr("Zoom")));
                    ui.checkbox(&mut self.show_cells, tr("Cells"));
                    ui.checkbox(&mut self.show_printable, tr("Printable area"));
                    if ui.checkbox(&mut self.prefs.show_rulers, tr("Rulers")).changed() {
                        self.prefs.save();
                    }
                });
            });
        });

        if n == 0 {
            ui.centered_and_justified(|ui| {
                let msg = match &self.selection {
                    Err(e) => format!("⚠ {e}"),
                    Ok(_) => tr("No pages selected").into(),
                };
                ui.label(RichText::new(msg).size(16.0).color(pal().warn));
            });
            return;
        }

        let ppp = ui.ctx().pixels_per_point();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_size();
            let (sw, sh) = self.sheet_mm;
            let fit = ((avail.x - 64.0) / sw).min((avail.y - 64.0) / sh).max(0.05);
            let k = fit * self.preview_zoom;
            let size = vec2(sw * k, sh * k);
            let (outer, _) = ui.allocate_exact_size(vec2(size.x + 64.0, size.y + 64.0).max(avail), Sense::hover());
            let rect = egui::Rect::from_center_size(outer.center(), size);
            let p = ui.painter();
            p.rect_filled(rect.translate(vec2(5.0, 6.0)), 2.0, pal().shadow);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            if let Some(t) = &self.preview_tex {
                p.image(t.id(), rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let to_screen = |x: f32, y: f32| rect.min + vec2(x * k, y * k);
            if self.show_printable && self.hw.iter().any(|&m| m > 0.0) {
                let pr = egui::Rect::from_min_max(
                    to_screen(self.hw[3], self.hw[0]),
                    to_screen(sw - self.hw[1], sh - self.hw[2]),
                );
                let shade = Color32::from_rgba_unmultiplied(255, 80, 80, 40);
                for r in [
                    egui::Rect::from_min_max(rect.min, pos2(rect.max.x, pr.min.y)),
                    egui::Rect::from_min_max(pos2(rect.min.x, pr.max.y), rect.max),
                    egui::Rect::from_min_max(pos2(rect.min.x, pr.min.y), pos2(pr.min.x, pr.max.y)),
                    egui::Rect::from_min_max(pos2(pr.max.x, pr.min.y), pos2(rect.max.x, pr.max.y)),
                ] {
                    p.rect_filled(r, 0.0, shade);
                }
                p.rect_stroke(pr, 0.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(230, 60, 60, 160)));
            }
            if self.show_cells {
                if let Some(sh) = self.sheets.get(self.preview_idx) {
                    for pl in &sh.placements {
                        let c = pl.cell;
                        let r = egui::Rect::from_min_max(to_screen(c.x, c.y), to_screen(c.right(), c.bottom()));
                        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, ACCENT));
                    }
                }
            }
            if self.preview_busy {
                ui.put(egui::Rect::from_center_size(rect.right_top() + vec2(-18.0, 18.0), vec2(20.0, 20.0)), egui::Spinner::new());
            }
            if self.prefs.show_rulers {
                draw_rulers(ui, rect, self.sheet_mm);
            }
            self.request_preview(size.y * ppp);
        });
    }

    fn ui_viewer(&mut self, ui: &mut Ui) {
        let n = self.sizes_mm.len();
        egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 2.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("◀").clicked() && self.current > 0 {
                    self.current -= 1;
                    self.scroll_thumb = true;
                }
                let mut shown = self.current + 1;
                if ui.add(egui::DragValue::new(&mut shown).range(1..=n).prefix(tr("Page ")).suffix(format!(" / {n}"))).changed() {
                    self.scroll_thumb = true;
                }
                self.current = shown.saturating_sub(1).min(n - 1);
                if ui.button("▶").clicked() && self.current + 1 < n {
                    self.current += 1;
                    self.scroll_thumb = true;
                }
                ui.separator();
                let picked = self.picked.contains(&self.current);
                if ui.selectable_label(picked, tr("★ Pick page")).on_hover_text(tr("Add/remove this page from the 'Picked' selection")).clicked() {
                    if picked {
                        self.picked.remove(&self.current);
                    } else {
                        self.picked.insert(self.current);
                    }
                    self.s.range_mode = RangeMode::Picked;
                }
                let in_sel = self.sel_set.contains(&self.current);
                ui.label(if in_sel {
                    RichText::new(tr("✔ will be printed")).color(Color32::from_rgb(70, 190, 100))
                } else {
                    RichText::new(tr("✖ not in selection")).color(Color32::GRAY)
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (z, l) in [(2.0, "200%"), (1.5, "150%"), (1.0, tr("Fit width")), (0.5, "50%")] {
                        ui.selectable_value(&mut self.view_zoom, z, l);
                    }
                    if ui.checkbox(&mut self.prefs.show_rulers, tr("Rulers")).changed() {
                        self.prefs.save();
                    }
                });
            });
        });
        let ppp = ui.ctx().pixels_per_point();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_size();
            let (pw, ph) = self.sizes_mm[self.current];
            let pad = if self.prefs.show_rulers { 30.0 } else { 20.0 };
            let w = ((avail.x - 2.0 * pad) * self.view_zoom).max(100.0);
            let h = w * ph / pw;
            let (outer, _) = ui.allocate_exact_size(vec2(w + 2.0 * pad, h + 2.0 * pad).max(avail), Sense::hover());
            let rect = egui::Rect::from_min_size(pos2(outer.center().x - w / 2.0, outer.min.y + pad), vec2(w, h));
            let p = ui.painter();
            p.rect_filled(rect.translate(vec2(4.0, 5.0)), 2.0, pal().shadow);
            p.rect_filled(rect, 0.0, Color32::WHITE);
            // the viewer shows the whole PDF page, cut to the part this page uses
            if let PageSrc::Pdf(src, reg) = self.order[self.current] {
                let tex = self.view_tex.as_ref().filter(|(pg, _)| *pg == src).map(|(_, t)| t.id());
                if let Some(id) = tex.or_else(|| self.thumbs.get(&src).map(|t| t.id())) {
                    p.image(id, rect, egui::Rect::from_min_max(pos2(reg[0], reg[1]), pos2(reg[2], reg[3])), Color32::WHITE);
                }
                let want = (((w * ppp / (reg[2] - reg[0]).max(0.05)) / 64.0).ceil() * 64.0).min(6000.0) as u32;
                if self.view_req != Some((src, want)) {
                    self.view_req = Some((src, want));
                    let _ = self.tx.send(Req::View { page: src, width: want });
                }
            }
            if self.prefs.show_rulers {
                draw_rulers(ui, rect, (pw, ph));
            }
        });
    }
}

// ------------------------------------------------------------ settings panel

impl App {
    fn ui_settings(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("settings").resizable(true).default_width(440.0).width_range(360.0..=640.0).show(ctx, |ui| {
            egui::TopBottomPanel::bottom("actions").show_inside(ui, |ui| self.ui_actions(ui));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(6.0);
                self.ui_printer(ui);
                if self.is_cbz {
                    self.ui_cbz(ui);
                }
                self.ui_pages(ui);
                self.ui_layout(ui);
                self.ui_sizing(ui);
                self.ui_color(ui);
                self.ui_quality(ui);
                self.ui_overlays(ui);
                self.ui_presets(ui);
            });
        });
    }

    fn ui_printer(&mut self, ui: &mut Ui) {
        section(ui, "🖨", tr("Printer"), true, |ui| {
            let mut chosen = None;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("printer")
                    .selected_text(if self.s.printer.is_empty() { tr("No printers found") } else { &self.s.printer })
                    .width(ui.available_width() - 70.0)
                    .show_ui(ui, |ui| {
                        for p in &self.printers {
                            let label = if p.is_default { format!("★ {}", p.name) } else { p.name.clone() };
                            if ui.selectable_label(p.name == self.s.printer, label).clicked() {
                                chosen = Some(p.name.clone());
                            }
                        }
                    });
                if ui.button("⟳").on_hover_text(tr("Refresh printer list and status")).clicked() {
                    self.refresh_printers();
                }
            });
            if let Some(n) = chosen {
                self.select_printer(&n);
            }
            if let Some(p) = self.current_printer().cloned() {
                let (st, bad) = p.status_text();
                ui.horizontal(|ui| {
                    let c = if bad { pal().err } else { pal().ok };
                    ui.label(RichText::new("●").color(c));
                    ui.label(RichText::new(st).color(c));
                    if p.jobs > 0 {
                        ui.label(RichText::new(trf!("· {} job(s) queued", p.jobs)).color(Color32::GRAY));
                    }
                    if let Some(c) = &self.caps {
                        ui.label(RichText::new(if c.color { tr("· color") } else { tr("· monochrome") }).color(Color32::GRAY));
                        if c.duplex {
                            ui.label(RichText::new(tr("· duplex")).color(Color32::GRAY));
                        }
                    }
                });
                let mut meta = vec![p.driver.clone(), p.port.clone()];
                if !p.location.is_empty() {
                    meta.push(p.location.clone());
                }
                if !p.comment.is_empty() {
                    meta.push(p.comment.clone());
                }
                hint(ui, &meta.join(" · "));
            }
            if ui.button(tr("⚙  Printer properties (driver dialog)…")).clicked() && !self.s.printer.is_empty() {
                let mut dm = self.devmode.clone();
                printer::apply_settings(&mut dm, &self.s, self.landscape, self.caps.as_ref());
                if printer::properties_dialog(self.hwnd, &self.s.printer, &mut dm) {
                    printer::settings_from_devmode(&dm, &mut self.s);
                    self.devmode = dm;
                    self.geom_key.clear();
                    self.layout_key.clear();
                }
            }
            ui.add_space(4.0);
            let caps = self.caps.clone().unwrap_or_default();
            grid(ui, "printer_grid", |ui| {
                ui.label(tr("Paper size"));
                let paper_label = |id: u16| -> String {
                    match id {
                        0 => tr("Printer default").into(),
                        256 => tr("Custom size").into(),
                        _ => caps
                            .papers
                            .iter()
                            .find(|p| p.id == id)
                            .map(|p| format!("{}  ({:.0}×{:.0})", p.name, p.w_mm, p.h_mm))
                            .unwrap_or_else(|| format!("#{id}")),
                    }
                };
                egui::ComboBox::from_id_salt("paper").selected_text(paper_label(self.s.paper_id)).width(230.0).height(400.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.s.paper_id, 0, tr("Printer default"));
                    for p in &caps.papers {
                        ui.selectable_value(&mut self.s.paper_id, p.id, format!("{}  ({:.0}×{:.0} mm)", p.name, p.w_mm, p.h_mm));
                    }
                    ui.selectable_value(&mut self.s.paper_id, 256, tr("Custom size…"));
                });
                ui.end_row();
                if self.s.paper_id == 256 {
                    ui.label(tr("Custom (w × h)"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.s.custom_paper_mm[0]).range(20.0..=2000.0).suffix(" mm"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.s.custom_paper_mm[1]).range(20.0..=5000.0).suffix(" mm"));
                    });
                    ui.end_row();
                }
                ui.label(tr("Orientation"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.orientation, Orientation::Auto, tr("Auto"));
                    ui.selectable_value(&mut self.s.orientation, Orientation::Portrait, tr("▯ Portrait"));
                    ui.selectable_value(&mut self.s.orientation, Orientation::Landscape, tr("▭ Landscape"));
                });
                ui.end_row();
                if self.s.orientation == Orientation::Auto {
                    ui.label("");
                    hint(ui, &trf!("auto → {}", if self.landscape { tr("landscape") } else { tr("portrait") }));
                    ui.end_row();
                }
                ui.label(tr("Paper source"));
                let mut bins: Vec<(u16, &str)> = vec![(0, tr("Automatic / driver default"))];
                bins.extend(caps.bins.iter().map(|(i, n)| (*i, n.as_str())));
                combo(ui, "bin", &mut self.s.bin, &bins);
                ui.end_row();
                if !caps.media.is_empty() {
                    ui.label(tr("Media type"));
                    let mut media: Vec<(u32, &str)> = vec![(0, tr("Driver default"))];
                    media.extend(caps.media.iter().map(|(i, n)| (*i, n.as_str())));
                    combo(ui, "media", &mut self.s.media, &media);
                    ui.end_row();
                }
                ui.label(tr("Print quality"));
                let mut q: Vec<(Quality, String)> = vec![
                    (Quality::Default, tr("Driver default").into()),
                    (Quality::Draft, tr("Draft").into()),
                    (Quality::Low, tr("Low").into()),
                    (Quality::Medium, tr("Medium").into()),
                    (Quality::High, tr("High").into()),
                ];
                q.extend(caps.resolutions.iter().map(|&(x, y)| (Quality::Res(x, y), format!("{x} × {y} dpi"))));
                let qref: Vec<(Quality, &str)> = q.iter().map(|(a, b)| (*a, b.as_str())).collect();
                combo(ui, "quality", &mut self.s.quality, &qref);
                ui.end_row();
                ui.label(tr("Two-sided"));
                combo(
                    ui,
                    "duplex",
                    &mut self.s.duplex,
                    &[
                        (Duplex::Off, tr("One-sided")),
                        (Duplex::LongEdge, tr("Two-sided · flip on long edge")),
                        (Duplex::ShortEdge, tr("Two-sided · flip on short edge")),
                        (Duplex::Manual, tr("Manual duplex (reinsert paper)")),
                    ],
                );
                ui.end_row();
                if matches!(self.s.duplex, Duplex::LongEdge | Duplex::ShortEdge) && !caps.duplex && self.caps.is_some() {
                    ui.label("");
                    ui.label(RichText::new(tr("⚠ printer reports no duplex unit — try Manual")).small().color(pal().warn));
                    ui.end_row();
                }
                if self.s.duplex == Duplex::Manual {
                    ui.label("");
                    ui.checkbox(&mut self.s.manual_duplex_reverse, tr("Print back sides in reverse order"));
                    ui.end_row();
                }
                ui.label(tr("Copies"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.s.copies).range(1..=9999).speed(0.2));
                    ui.checkbox(&mut self.s.collate, tr("Collate")).on_hover_text(tr("1,2,3 1,2,3 instead of 1,1 2,2 3,3"));
                });
                ui.end_row();
                ui.label(tr("Job name"));
                let file_name = self.file_name();
                ui.add(egui::TextEdit::singleline(&mut self.s.job_name).hint_text(file_name).desired_width(220.0));
                ui.end_row();
                ui.label(tr("Output"));
                ui.checkbox(&mut self.s.print_to_file, tr("Print to file (driver output)"));
                ui.end_row();
                if self.s.print_to_file {
                    ui.label(tr("File"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.s.output_file).desired_width(180.0));
                        if ui.button("…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().set_file_name("output.prn").save_file() {
                                self.s.output_file = p.display().to_string();
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });
    }

    fn ui_pages(&mut self, ui: &mut Ui) {
        let n = self.sizes_mm.len();
        section(ui, "📑", tr("Pages"), true, |ui| {
            ui.radio_value(&mut self.s.range_mode, RangeMode::All, trf!("All pages (1-{})", n));
            ui.radio_value(&mut self.s.range_mode, RangeMode::Current, trf!("Current page ({})", self.current + 1));
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.s.range_mode, RangeMode::Custom, tr("Range"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.s.custom_range)
                        .hint_text("1-3, 5, 8-end, 10-4, 2*3")
                        .desired_width(ui.available_width()),
                );
                if r.gained_focus() || r.changed() {
                    self.s.range_mode = RangeMode::Custom;
                }
            });
            ui.radio_value(&mut self.s.range_mode, RangeMode::Picked, trf!("Picked in sidebar ({} ★)", self.picked.len()));
            ui.horizontal_wrapped(|ui| {
                let half = n / 2;
                let set = |s: &mut Settings, r: String| {
                    s.range_mode = RangeMode::Custom;
                    s.custom_range = r;
                };
                if ui.small_button(tr("First half")).clicked() {
                    set(&mut self.s, format!("1-{}", half.max(1)));
                }
                if ui.small_button(tr("Second half")).clicked() {
                    set(&mut self.s, format!("{}-end", (half + 1).min(n)));
                }
                if ui.small_button(tr("Last page")).clicked() {
                    set(&mut self.s, "end".into());
                }
                if ui.small_button(tr("Reverse all")).clicked() {
                    set(&mut self.s, "end-1".into());
                }
                if ui.small_button(tr("★ all")).clicked() {
                    self.picked = (0..n).collect();
                }
                if ui.small_button(tr("★ invert")).clicked() {
                    self.picked = (0..n).filter(|p| !self.picked.contains(p)).collect();
                }
                if ui.small_button(tr("★ clear")).clicked() {
                    self.picked.clear();
                }
            });
            ui.add_space(4.0);
            grid(ui, "pages_grid", |ui| {
                ui.label(tr("Include"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.subset, Subset::All, tr("All"));
                    ui.selectable_value(&mut self.s.subset, Subset::Odd, tr("Odd only"));
                    ui.selectable_value(&mut self.s.subset, Subset::Even, tr("Even only"));
                });
                ui.end_row();
                ui.label(tr("Every n-th"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.s.every_nth).range(1..=100).prefix(tr("every ")));
                    ui.add(egui::DragValue::new(&mut self.s.nth_offset).range(0..=99).prefix(tr("offset ")));
                });
                ui.end_row();
                ui.label(tr("Exclude"));
                ui.add(egui::TextEdit::singleline(&mut self.s.exclude).hint_text(tr("e.g. 1, 7-9")).desired_width(220.0));
                ui.end_row();
                ui.label(tr("Order"));
                ui.checkbox(&mut self.s.reverse, tr("Reverse (last page first)"));
                ui.end_row();
            });
            match &self.selection {
                Ok(v) => {
                    let mut sorted = v.clone();
                    sorted.sort();
                    sorted.dedup();
                    let mut d = pages::describe(&sorted);
                    if d.len() > 90 {
                        d.truncate(90);
                        d.push('…');
                    }
                    ui.label(RichText::new(trf!("→ {} page(s): {}", v.len(), d)).color(pal().ok));
                }
                Err(e) => {
                    ui.label(RichText::new(format!("⚠ {e}")).color(pal().err));
                }
            }
        });
    }

    fn ui_layout(&mut self, ui: &mut Ui) {
        section(ui, "▦", tr("Layout"), true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.s.layout, LayoutMode::Normal, tr("▦ Pages per sheet"));
                ui.selectable_value(&mut self.s.layout, LayoutMode::Booklet, tr("📖 Booklet"));
                ui.selectable_value(&mut self.s.layout, LayoutMode::Poster, tr("🧩 Poster / tiling"));
                ui.selectable_value(&mut self.s.layout, LayoutMode::Repeat, tr("🔁 Step & repeat"));
            });
            ui.add_space(4.0);
            match self.s.layout {
                LayoutMode::Normal => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Quick:"));
                        for (n, c, r) in [(1, 1, 1), (2, 2, 1), (4, 2, 2), (6, 2, 3), (8, 2, 4), (9, 3, 3), (12, 3, 4), (16, 4, 4)] {
                            let on = self.s.nup_cols * self.s.nup_rows == n;
                            if ui.selectable_label(on, format!("{n}")).clicked() {
                                self.s.nup_cols = c;
                                self.s.nup_rows = r;
                            }
                        }
                    });
                    grid(ui, "nup_grid", |ui| {
                        ui.label(tr("Grid"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.nup_cols).range(1..=16).suffix(tr(" cols")));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.nup_rows).range(1..=16).suffix(tr(" rows")));
                        });
                        ui.end_row();
                        ui.label(tr("Page order"));
                        combo(
                            ui,
                            "nup_order",
                            &mut self.s.nup_order,
                            &[
                                (NupOrder::RowsLtr, tr("Z  → across, then down")),
                                (NupOrder::RowsRtl, tr("←  across (right to left)")),
                                (NupOrder::ColsTtb, tr("N  ↓ down, then across")),
                                (NupOrder::ColsTtbRtl, tr("↓  down, right to left")),
                            ],
                        );
                        ui.end_row();
                        ui.label(tr("Gap between pages"));
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                    });
                }
                LayoutMode::Booklet => {
                    hint(ui, tr("Pages are reordered so the folded stack reads as a book. Use two-sided printing, flip on short edge (or manual duplex)."));
                    grid(ui, "booklet_grid", |ui| {
                        ui.label(tr("Binding"));
                        ui.checkbox(&mut self.s.booklet_rtl, tr("Right-to-left (manga / Arabic)"));
                        ui.end_row();
                        ui.label(tr("Signatures"));
                        ui.add(egui::DragValue::new(&mut self.s.booklet_signature).range(0..=64).custom_formatter(|v, _| {
                            if v == 0.0 { tr("single booklet").into() } else { trf!("{} sheets each", v) }
                        }));
                        ui.end_row();
                        ui.label(tr("Center gutter"));
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.s.booklet_align_fold, tr("Push pages towards the fold"));
                        ui.end_row();
                    });
                    if self.s.duplex == Duplex::Off {
                        ui.label(RichText::new(tr("⚠ duplex is off — set Two-sided in the Printer section")).small().color(pal().warn));
                    }
                }
                LayoutMode::Poster => {
                    hint(ui, tr("Each page is enlarged across several sheets you can trim and tape together."));
                    grid(ui, "poster_grid", |ui| {
                        ui.label(tr("Tiles"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.poster_x).range(1..=20).suffix(tr(" across")));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.poster_y).range(1..=20).suffix(tr(" down")));
                        });
                        ui.end_row();
                        ui.label(tr("Overlap"));
                        mm(ui, &mut self.s.poster_overlap_mm, 60.0);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.s.poster_guides, tr("Glue lines and tile labels"));
                        ui.end_row();
                    });
                    if let Some(&p) = self.selection.as_ref().ok().and_then(|v| v.first()) {
                        if let Some(pl) = self.sheets.first().and_then(|s| s.placements.first()) {
                            let (pw, ph) = self.sizes_mm[p];
                            let k = (pl.frame.w / pw).min(pl.frame.h / ph) * 100.0;
                            hint(ui, &trf!("Final poster ≈ {:.0} × {:.0} mm ({:.0}% of original)", pl.frame.w, pl.frame.h, k));
                        }
                    }
                }
                LayoutMode::Repeat => {
                    hint(ui, tr("Fills each sheet with copies of the same page — labels, business cards, flyers."));
                    grid(ui, "repeat_grid", |ui| {
                        ui.label(tr("Grid"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.nup_cols).range(1..=16).suffix(tr(" cols")));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.nup_rows).range(1..=16).suffix(tr(" rows")));
                        });
                        ui.end_row();
                        ui.label(tr("Gap"));
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                    });
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new(tr("Margins")).strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.link_margins, tr("🔗 Same on all sides"));
                for (label, v) in [(tr("None"), 0.0), (tr("Narrow"), 5.0), (tr("Normal"), 12.7), (tr("Wide"), 25.4)] {
                    if ui.small_button(label).clicked() {
                        self.s.margins_mm = [v; 4];
                    }
                }
            });
            let before = self.s.margins_mm;
            grid(ui, "margins_grid", |ui| {
                ui.label(tr("Top / Bottom"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.margins_mm[0], 200.0);
                    mm(ui, &mut self.s.margins_mm[2], 200.0);
                });
                ui.end_row();
                ui.label(tr("Left / Right"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.margins_mm[3], 200.0);
                    mm(ui, &mut self.s.margins_mm[1], 200.0);
                });
                ui.end_row();
                ui.label(tr("Binding gutter"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.binding_gutter_mm, 100.0);
                    combo(ui, "bind_edge", &mut self.s.binding_edge, &[(BindingEdge::Left, tr("left")), (BindingEdge::Right, tr("right")), (BindingEdge::Top, tr("top"))]);
                });
                ui.end_row();
            });
            if self.link_margins {
                if let Some(i) = (0..4).find(|&i| self.s.margins_mm[i] != before[i]) {
                    self.s.margins_mm = [self.s.margins_mm[i]; 4];
                }
            }
            ui.checkbox(&mut self.s.respect_hw_margins, tr("Keep content inside the printer's printable area"));
            if self.hw.iter().any(|&m| m > 0.0) {
                hint(ui, &trf!(
                    "Hardware margins: top {:.1}, right {:.1}, bottom {:.1}, left {:.1} mm",
                    self.hw[0], self.hw[1], self.hw[2], self.hw[3]
                ));
            }
        });
    }

    fn ui_sizing(&mut self, ui: &mut Ui) {
        section(ui, "⤢", tr("Size & position"), true, |ui| {
            grid(ui, "size_grid", |ui| {
                ui.label(tr("Scaling"));
                combo(
                    ui,
                    "scaling",
                    &mut self.s.scaling,
                    &[
                        (Scaling::Fit, tr("Fit to printable area")),
                        (Scaling::ShrinkOnly, tr("Shrink oversized pages only")),
                        (Scaling::Actual, tr("Actual size (100%)")),
                        (Scaling::Custom, tr("Custom scale")),
                        (Scaling::Fill, tr("Fill (crop to fill)")),
                        (Scaling::Stretch, tr("Stretch (ignore aspect)")),
                    ],
                );
                ui.end_row();
                if self.s.scaling == Scaling::Custom {
                    ui.label(tr("Scale"));
                    ui.add(egui::Slider::new(&mut self.s.custom_scale, 5.0..=400.0).suffix(" %").logarithmic(true));
                    ui.end_row();
                }
                ui.label(tr("Alignment"));
                ui.vertical(|ui| {
                    let arrows = [["↖", "↑", "↗"], ["←", "●", "→"], ["↙", "↓", "↘"]];
                    for (v, row) in arrows.iter().enumerate() {
                        ui.horizontal(|ui| {
                            for (h, a) in row.iter().enumerate() {
                                let on = self.s.align_h == h as u8 && self.s.align_v == v as u8;
                                if ui.add(egui::SelectableLabel::new(on, *a)).clicked() {
                                    self.s.align_h = h as u8;
                                    self.s.align_v = v as u8;
                                }
                            }
                        });
                    }
                });
                ui.end_row();
                ui.label(tr("Offset (x, y)"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.offset_mm[0], 500.0);
                    mm(ui, &mut self.s.offset_mm[1], 500.0);
                });
                ui.end_row();
                ui.label(tr("Rotation"));
                ui.horizontal(|ui| {
                    for r in [0, 90, 180, 270] {
                        ui.selectable_value(&mut self.s.rotation, r, format!("{r}°"));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.auto_rotate, tr("Auto-rotate pages to best fit"));
                ui.end_row();
                ui.label(tr("Mirror"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.s.mirror_h, tr("Horizontal ⇋"));
                    ui.checkbox(&mut self.s.mirror_v, tr("Vertical ⇵"));
                });
                ui.end_row();
                ui.label(tr("Trim source (T/B)"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.crop_mm[0], 200.0);
                    mm(ui, &mut self.s.crop_mm[2], 200.0);
                });
                ui.end_row();
                ui.label(tr("Trim source (L/R)"));
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.crop_mm[3], 200.0);
                    mm(ui, &mut self.s.crop_mm[1], 200.0);
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.auto_crop, tr("Auto-trim white borders"))
                    .on_hover_text(tr("Detects the content bounding box of every page and zooms in on it"));
                ui.end_row();
            });
            for v in self.s.crop_mm.iter_mut() {
                *v = v.max(0.0);
            }
        });
    }

    fn ui_color(&mut self, ui: &mut Ui) {
        section(ui, "🎨", tr("Color & image"), false, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Presets:"));
                if ui.small_button(tr("Original")).clicked() {
                    self.reset_color();
                }
                if ui.small_button(tr("Crisp text")).clicked() {
                    self.reset_color();
                    self.s.color_mode = ColorMode::BlackWhite;
                    self.s.threshold = 170;
                    self.s.contrast = 20.0;
                }
                if ui.small_button(tr("Photo")).clicked() {
                    self.reset_color();
                    self.s.saturation = 115.0;
                    self.s.contrast = 8.0;
                    self.s.gamma = 1.05;
                }
                if ui.small_button(tr("Eco draft")).clicked() {
                    self.reset_color();
                    self.s.color_mode = ColorMode::Grayscale;
                    self.s.toner_saver = 45.0;
                }
                if ui.small_button(tr("Dark-mode PDF")).clicked() {
                    self.reset_color();
                    self.s.invert = true;
                    self.s.color_mode = ColorMode::Grayscale;
                }
                if ui.small_button(tr("Faded scan fix")).clicked() {
                    self.reset_color();
                    self.s.contrast = 35.0;
                    self.s.gamma = 0.8;
                    self.s.brightness = 5.0;
                }
            });
            grid(ui, "color_grid", |ui| {
                ui.label(tr("Color mode"));
                combo(
                    ui,
                    "color_mode",
                    &mut self.s.color_mode,
                    &[
                        (ColorMode::Color, tr("Color")),
                        (ColorMode::Grayscale, tr("Grayscale")),
                        (ColorMode::BlackWhite, tr("Black & white (threshold)")),
                        (ColorMode::Dither, tr("Black & white (error diffusion)")),
                        (ColorMode::Halftone, tr("Black & white (halftone pattern)")),
                    ],
                );
                ui.end_row();
                if self.s.color_mode == ColorMode::BlackWhite {
                    ui.label(tr("Threshold"));
                    ui.add(egui::Slider::new(&mut self.s.threshold, 1..=254));
                    ui.end_row();
                }
                ui.label(tr("Brightness"));
                ui.add(egui::Slider::new(&mut self.s.brightness, -100.0..=100.0));
                ui.end_row();
                ui.label(tr("Contrast"));
                ui.add(egui::Slider::new(&mut self.s.contrast, -100.0..=100.0));
                ui.end_row();
                ui.label(tr("Gamma"));
                ui.add(egui::Slider::new(&mut self.s.gamma, 0.2..=3.0).max_decimals(2));
                ui.end_row();
                ui.label(tr("Saturation"));
                ui.add(egui::Slider::new(&mut self.s.saturation, 0.0..=200.0).suffix(" %"));
                ui.end_row();
                ui.label(tr("Toner saver"));
                ui.add(egui::Slider::new(&mut self.s.toner_saver, 0.0..=80.0).suffix(" %"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.invert, tr("Invert colors (negative)"));
                ui.end_row();
            });
        });
    }

    fn reset_color(&mut self) {
        let d = Settings::default();
        self.s.color_mode = d.color_mode;
        self.s.threshold = d.threshold;
        self.s.brightness = d.brightness;
        self.s.contrast = d.contrast;
        self.s.gamma = d.gamma;
        self.s.saturation = d.saturation;
        self.s.invert = d.invert;
        self.s.toner_saver = d.toner_saver;
    }

    fn ui_quality(&mut self, ui: &mut Ui) {
        section(ui, "✨", tr("Rendering & anti-aliasing"), false, |ui| {
            grid(ui, "quality_grid", |ui| {
                ui.label(tr("Raster resolution"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.dpi_mode, DpiMode::Printer, tr("Printer native"));
                    ui.selectable_value(&mut self.s.dpi_mode, DpiMode::Custom, tr("Custom"));
                });
                ui.end_row();
                match self.s.dpi_mode {
                    DpiMode::Printer => {
                        ui.label(tr("Max DPI"));
                        combo(ui, "max_dpi", &mut self.s.max_dpi, &[(150, "150"), (200, "200"), (300, "300"), (600, "600"), (1200, "1200")]);
                        ui.end_row();
                    }
                    DpiMode::Custom => {
                        ui.label("DPI");
                        ui.add(egui::DragValue::new(&mut self.s.custom_dpi).range(36..=2400).suffix(" dpi"));
                        ui.end_row();
                    }
                }
                if let Some(g) = self.geom {
                    ui.label("");
                    hint(ui, &trf!("printer reports {} × {} dpi", g.dpi_x, g.dpi_y));
                    ui.end_row();
                }
                ui.label(tr("Anti-aliasing"));
                combo(
                    ui,
                    "ss",
                    &mut self.s.supersample,
                    &[(1, tr("Standard (1×)")), (2, tr("Supersample 2× (4 samples)")), (3, tr("Supersample 3× (9 samples)")), (4, tr("Supersample 4× (16 samples)"))],
                );
                ui.end_row();
                ui.label(tr("Resampling filter"));
                combo(
                    ui,
                    "filter",
                    &mut self.s.filter,
                    &[
                        (Filter::Nearest, tr("Nearest (pixel-sharp)")),
                        (Filter::Triangle, tr("Bilinear (soft)")),
                        (Filter::CatmullRom, tr("Bicubic Catmull-Rom")),
                        (Filter::Lanczos3, tr("Lanczos-3 (sharpest)")),
                    ],
                );
                ui.end_row();
                ui.label(tr("Sharpen"));
                ui.add(egui::Slider::new(&mut self.s.sharpen, 0.0..=3.0).max_decimals(1).custom_formatter(|v, _| if v == 0.0 { tr("off").into() } else { format!("{v:.1}") }));
                ui.end_row();
                ui.label(tr("Spool format"));
                combo(
                    ui,
                    "spool",
                    &mut self.s.spool,
                    &[
                        (SpoolFormat::Auto, tr("Auto (smallest for color mode)")),
                        (SpoolFormat::Rgb24, tr("24-bit color")),
                        (SpoolFormat::Gray8, tr("8-bit gray")),
                        (SpoolFormat::Mono1, tr("1-bit monochrome")),
                    ],
                );
                ui.end_row();
                let dpi = match self.s.dpi_mode {
                    DpiMode::Printer => self.geom.map(|g| g.dpi_x.min(g.dpi_y) as u32).unwrap_or(300).min(self.s.max_dpi),
                    DpiMode::Custom => self.s.custom_dpi,
                } as f32;
                let px = (self.sheet_mm.0 / 25.4 * dpi) * (self.sheet_mm.1 / 25.4 * dpi);
                let bpp = match printer::resolve_spool(&self.s) {
                    SpoolFormat::Mono1 => 1.0 / 8.0,
                    SpoolFormat::Gray8 => 1.0,
                    _ => 3.0,
                };
                ui.label("");
                hint(ui, &trf!("≈ {:.0} × {:.0} px, {:.1} MB per side before driver compression",
                    self.sheet_mm.0 / 25.4 * dpi, self.sheet_mm.1 / 25.4 * dpi, px * bpp / 1_048_576.0));
                ui.end_row();
                ui.label(tr("PNG export DPI"));
                ui.add(egui::DragValue::new(&mut self.s.export_dpi).range(36..=1200).suffix(" dpi"));
                ui.end_row();
            });
        });
    }

    fn ui_overlays(&mut self, ui: &mut Ui) {
        let vars = self.preview_vars();
        let mut help = None;
        section(ui, "🏷", tr("Watermark, headers & stamps"), false, |ui| {
            ui.horizontal(|ui| {
                if ui.button(tr("❓ Template help")).on_hover_text(tr("Variables, conditions, loops and examples (F1)")).clicked() {
                    help = Some(manual::CH_TEMPLATES);
                }
                if ui.button(tr("📚 Cookbook")).clicked() {
                    help = Some(manual::CH_COOKBOOK);
                }
                if ui.button(tr("🧪 Playground")).on_hover_text(tr("Try templates against the side in the preview")).clicked() {
                    help = Some(manual::PLAYGROUND);
                }
            });
            grid(ui, "font_grid", |ui| {
                ui.label(tr("Font"));
                font_pick(ui, "font", &mut self.s.font, &self.fonts, false);
                ui.end_row();
            });

            // ---- watermark
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.s.watermark.enabled, RichText::new(tr("Watermark")).strong());
                if help_button(ui) {
                    help = Some(manual::CH_WATERMARK);
                }
            });
            if self.s.watermark.enabled {
                let wm = &mut self.s.watermark;
                grid(ui, "wm_grid", |ui| {
                    ui.label(tr("Text"));
                    tpl_edit(ui, &mut wm.text, &vars);
                    ui.end_row();
                    ui.label(tr("Quick"));
                    ui.horizontal_wrapped(|ui| {
                        for t in [tr("CONFIDENTIAL"), tr("DRAFT"), tr("COPY"), tr("SAMPLE"), tr("PAID")] {
                            if ui.small_button(t).clicked() {
                                wm.text = t.into();
                            }
                        }
                        if ui.small_button(tr("ORIGINAL / COPY n")).on_hover_text(tr("First copy ORIGINAL, then COPY 2, COPY 3…")).clicked() {
                            wm.text = tr("{if copy = 1}ORIGINAL{else}COPY {copy}{end}").into();
                        }
                        if ui.small_button(tr("User + date")).on_hover_text(tr("Leak-tracing text; tile it")).clicked() {
                            wm.text = "{user}{br}{datetime}".into();
                        }
                    });
                    ui.end_row();
                    ui.label(tr("Image (PNG)"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut wm.image).hint_text(tr("none – use the text")).desired_width(150.0));
                        if ui.small_button(tr("Browse…")).clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).pick_file() {
                                wm.image = p.display().to_string();
                            }
                        }
                        if !wm.image.is_empty() && ui.small_button("✕").on_hover_text(tr("Use the text again")).clicked() {
                            wm.image.clear();
                        }
                    });
                    ui.end_row();
                    if wm.image.trim().is_empty() {
                        ui.label(tr("Text font"));
                        font_pick(ui, "wm_font", &mut wm.font, &self.fonts, true);
                        ui.end_row();
                        ui.label(tr("Size"));
                        ui.add(egui::Slider::new(&mut wm.size_pt, 8.0..=300.0).suffix(" pt"));
                        ui.end_row();
                        ui.label(tr("Color"));
                        ui.color_edit_button_srgb(&mut wm.color);
                        ui.end_row();
                    } else {
                        ui.label(tr("Image width"));
                        ui.add(egui::Slider::new(&mut wm.image_width_mm, 5.0..=400.0).suffix(" mm"));
                        ui.end_row();
                    }
                    ui.label(tr("Angle"));
                    ui.add(egui::Slider::new(&mut wm.angle, -90.0..=90.0).suffix("°"));
                    ui.end_row();
                    ui.label(tr("Opacity"));
                    ui.add(egui::Slider::new(&mut wm.opacity, 1.0..=100.0).suffix(" %"));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.tile, tr("Tile across the whole area"));
                    ui.end_row();
                    if wm.tile {
                        ui.label(tr("Gap (x, y)"));
                        ui.horizontal(|ui| {
                            mm(ui, &mut wm.tile_gap_mm[0], 300.0);
                            mm(ui, &mut wm.tile_gap_mm[1], 300.0);
                        });
                        ui.end_row();
                    } else {
                        ui.label(tr("Position"));
                        pos_pick(ui, &mut wm.position);
                        ui.end_row();
                    }
                    ui.label(tr("Offset (x, y)"));
                    ui.horizontal(|ui| {
                        mm(ui, &mut wm.offset_mm[0], 500.0);
                        mm(ui, &mut wm.offset_mm[1], 500.0);
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.behind, tr("Behind content (only on paper)"));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.per_page, tr("One per page (n-up)"));
                    ui.end_row();
                    if wm.image.trim().is_empty() {
                        ui.label("");
                        ui.checkbox(&mut wm.boxed, tr("Frame (rubber stamp)"));
                        ui.end_row();
                    }
                    ui.label(tr("Show when"));
                    cond_edit(ui, "wm_cond", &mut wm.show_if, &vars);
                    ui.end_row();
                });
            }

            // ---- header / footer
            ui.add_space(4.0);
            for (title, band, id) in [(tr("Header"), &mut self.s.header, "hdr"), (tr("Footer"), &mut self.s.footer, "ftr")] {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut band.enabled, RichText::new(title).strong());
                    if band.enabled {
                        ui.menu_button(tr("✨ Presets"), |ui| {
                            for (name, l, c, r) in BAND_PRESETS {
                                if ui.button(tr(name)).clicked() {
                                    band.left = tr(l).to_string();
                                    band.center = tr(c).to_string();
                                    band.right = tr(r).to_string();
                                    if name.starts_with("Book") {
                                        band.mirror_even = true;
                                    }
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                    if help_button(ui) {
                        help = Some(manual::CH_HEADERS);
                    }
                });
                if band.enabled {
                    grid(ui, id, |ui| {
                        for (label, txt) in [(tr("Left"), &mut band.left), (tr("Center"), &mut band.center), (tr("Right"), &mut band.right)] {
                            ui.label(label);
                            tpl_edit(ui, txt, &vars);
                            ui.end_row();
                        }
                        ui.label(tr("Size / color"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut band.size_pt).range(4.0..=48.0).suffix(" pt"));
                            ui.color_edit_button_srgb(&mut band.color);
                        });
                        ui.end_row();
                        ui.label(tr("Font"));
                        font_pick(ui, &format!("{id}_font"), &mut band.font, &self.fonts, true);
                        ui.end_row();
                        ui.label(tr("Distance"));
                        ui.add(egui::DragValue::new(&mut band.distance_mm).range(0.0..=100.0).speed(0.25).suffix(" mm").max_decimals(1))
                            .on_hover_text(tr("From the paper edge (or the page edge with \"each page\"); 0 = automatic"));
                        ui.end_row();
                        ui.label(tr("Place on"));
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut band.scope, StampScope::Sheet, tr("Sheet"));
                            ui.selectable_value(&mut band.scope, StampScope::EachPage, tr("Each page"));
                        });
                        ui.end_row();
                        ui.label("");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut band.mirror_even, tr("Mirror on even sides"))
                                .on_hover_text(tr("Swap left and right on even sides so page numbers sit on the outside edge"));
                            ui.checkbox(&mut band.rule, tr("Separator line"));
                        });
                        ui.end_row();
                        ui.label(tr("Show when"));
                        cond_edit(ui, &format!("{id}_cond"), &mut band.show_if, &vars);
                        ui.end_row();
                    });
                }
            }

            // ---- stamps
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(trf!("Stamps ({})", self.s.stamps.len())).strong());
                ui.menu_button(tr("➕ Add stamp"), |ui| {
                    for (name, text, pos, boxed, angle, color) in STAMP_PRESETS {
                        if ui.button(tr(name)).clicked() {
                            self.s.stamps.push(Stamp {
                                text: tr(text).to_string(),
                                position: *pos,
                                boxed: *boxed,
                                angle: *angle,
                                color: *color,
                                size_pt: if *boxed { 16.0 } else { 9.0 },
                                ..Default::default()
                            });
                            ui.close_menu();
                        }
                    }
                });
                if help_button(ui) {
                    help = Some(manual::CH_STAMPS);
                }
            });
            let mut remove = None;
            let mut duplicate = None;
            for (i, st) in self.s.stamps.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).inner_margin(6.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut st.enabled, "");
                        ui.add(egui::TextEdit::singleline(&mut st.text).desired_width(ui.available_width() - 62.0).font(egui::TextStyle::Monospace));
                        if ui.small_button("⧉").on_hover_text(tr("Duplicate")).clicked() {
                            duplicate = Some(i);
                        }
                        if ui.small_button("🗑").on_hover_text(tr("Remove")).clicked() {
                            remove = Some(i);
                        }
                    });
                    tpl_status(ui, &st.text, &vars);
                    egui::CollapsingHeader::new(tr("Options")).id_salt(("stamp", i)).show(ui, |ui| {
                        grid(ui, &format!("stamp_grid_{i}"), |ui| {
                            ui.label(tr("Position"));
                            pos_pick(ui, &mut st.position);
                            ui.end_row();
                            ui.label(tr("Offset (x, y)"));
                            ui.horizontal(|ui| {
                                mm(ui, &mut st.offset_mm[0], 500.0);
                                mm(ui, &mut st.offset_mm[1], 500.0);
                            });
                            ui.end_row();
                            ui.label(tr("Size / color"));
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut st.size_pt).range(4.0..=300.0).suffix(" pt"));
                                ui.color_edit_button_srgb(&mut st.color);
                            });
                            ui.end_row();
                            ui.label(tr("Font"));
                            font_pick(ui, &format!("stamp_font_{i}"), &mut st.font, &self.fonts, true);
                            ui.end_row();
                            ui.label(tr("Angle"));
                            ui.add(egui::Slider::new(&mut st.angle, -90.0..=90.0).suffix("°"));
                            ui.end_row();
                            ui.label(tr("Opacity"));
                            ui.add(egui::Slider::new(&mut st.opacity, 1.0..=100.0).suffix(" %"));
                            ui.end_row();
                            ui.label(tr("Place on"));
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut st.scope, StampScope::Sheet, tr("Sheet"));
                                ui.selectable_value(&mut st.scope, StampScope::EachPage, tr("Each page"));
                            });
                            ui.end_row();
                            ui.label("");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut st.boxed, tr("Frame"));
                                ui.checkbox(&mut st.behind, tr("Behind content"));
                            });
                            ui.end_row();
                            ui.label(tr("Show when"));
                            cond_edit(ui, &format!("stamp_cond_{i}"), &mut st.show_if, &vars);
                            ui.end_row();
                        });
                    });
                });
            }
            if let Some(i) = duplicate {
                let copy = self.s.stamps[i].clone();
                self.s.stamps.insert(i + 1, copy);
            }
            if let Some(i) = remove {
                self.s.stamps.remove(i);
            }
            if self.s.stamps.is_empty() {
                hint(ui, tr("Free-placed texts: Bates numbers, COPY, approval boxes, corner page numbers…"));
            }
            hint(ui, tr("Templates: {page} {pages} {range} {sheet} {file} {date} {user}…, {if odd}…{else}…{end}, {page|roman}. ❓ for everything."));

            // ---- marks
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Marks")).strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.s.crop_marks, tr("Crop marks"));
                ui.checkbox(&mut self.s.cell_border, tr("Page borders"));
                ui.checkbox(&mut self.s.page_labels, tr("Page numbers"));
            });
            if self.s.cell_border {
                ui.horizontal(|ui| {
                    ui.label(tr("Border width"));
                    ui.add(egui::DragValue::new(&mut self.s.border_width_mm).range(0.05..=5.0).speed(0.05).suffix(" mm"));
                });
            }
        });
        if let Some(ch) = help {
            self.manual.open_at(ch);
        }
    }

    fn ui_presets(&mut self, ui: &mut Ui) {
        section(ui, "💾", tr("Presets"), false, |ui| {
            let mut load = None;
            ui.horizontal_wrapped(|ui| {
                if self.presets.is_empty() {
                    hint(ui, tr("No saved presets yet."));
                }
                for p in &self.presets {
                    if ui.button(p).on_hover_text(tr("Load preset (printer stays as is)")).clicked() {
                        load = Some(p.clone());
                    }
                }
            });
            if let Some(name) = load {
                if let Some(mut ps) = load_preset(&name) {
                    ps.printer = self.s.printer.clone();
                    self.s = ps;
                    self.preset_name = name.clone();
                    self.notify(trf!("Loaded preset '{}'", name), false);
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text(tr("preset name")).desired_width(150.0));
                if ui.button(tr("Save")).clicked() {
                    match save_preset(&self.preset_name, &self.s) {
                        Ok(()) => self.notify(trf!("Saved preset '{}'", self.preset_name), false),
                        Err(e) => self.notify(e, true),
                    }
                    self.presets = list_presets();
                }
                if ui.button(tr("Delete")).clicked() {
                    delete_preset(&self.preset_name);
                    self.presets = list_presets();
                }
                if ui.button(tr("Reset all")).on_hover_text(tr("Restore every setting to its default")).clicked() {
                    let printer = self.s.printer.clone();
                    self.s = Settings::default();
                    self.s.printer = printer;
                }
            });
        });
    }

    fn ui_actions(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        let sides = self.sheets.len();
        let copies = self.s.copies.max(1) as usize;
        let seq = job::output_sequence(sides, &self.s).len();
        let physical = if self.s.is_duplex() || self.s.layout == LayoutMode::Booklet && self.s.is_duplex() { seq.div_ceil(2) } else { seq };
        let pages = self.selection.as_ref().map(|v| v.len()).unwrap_or(0);
        ui.label(RichText::new(trf!(
            "{} page(s) → {} side(s){} → {} sheet(s) of paper",
            pages,
            sides,
            if copies > 1 { trf!(" × {} copies", copies) } else { String::new() },
            physical
        )).strong());
        if let Some(c) = self.coverage {
            ui.label(RichText::new(trf!("Current side ≈ {:.1}% coverage", c)).small().color(pal().muted))
                .on_hover_text(tr("Estimated toner/ink coverage of this side (5% is a typical text page)"));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can = self.job.is_none() && pages > 0 && !self.s.printer.is_empty();
            let btn = egui::Button::new(RichText::new(tr("🖨  Print")).size(17.0).strong().color(Color32::WHITE))
                .fill(ACCENT)
                .min_size(vec2(ui.available_width() - 130.0, 38.0));
            if ui.add_enabled(can, btn).on_hover_text("Ctrl+P").clicked() {
                self.start_job(None);
            }
            if ui
                .add_enabled(self.job.is_none() && pages > 0, egui::Button::new(tr("🖼 Export PNG…")).min_size(vec2(120.0, 38.0)))
                .on_hover_text(tr("Render the imposed sheets to PNG files"))
                .clicked()
            {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.start_job(Some(dir));
                }
            }
        });
        ui.add_space(4.0);
    }

    // ------------------------------------------------------------ modals

    fn ui_modals(&mut self, ctx: &egui::Context) {
        if self.need_password {
            let mut submit = false;
            let mut cancel = false;
            egui::Window::new(tr("🔒 Password required"))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(tr("This document is encrypted. Enter its password:"));
                    let r = ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(260.0));
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        submit = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(tr("Open")).clicked() {
                            submit = true;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            if submit {
                if let Some(p) = self.path.clone() {
                    self.need_password = false;
                    self.open(p);
                }
            }
            if cancel {
                self.need_password = false;
                self.path = None;
            }
        }

        let mut flip_answer = None;
        if let Some(j) = &self.job {
            egui::Window::new(if j.export { tr("Exporting") } else { tr("Printing") })
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    if let Some(n) = j.need_flip {
                        ui.label(RichText::new(tr("Front sides are printed.")).strong());
                        ui.label(trf!(
                            "Take the stack from the output tray, flip it (keep the same edge leading for {} binding) and put it back in the input tray. {} back side(s) will be printed.",
                            if self.s.binding_edge == BindingEdge::Top { tr("top") } else { tr("side") },
                            n
                        ));
                        ui.horizontal(|ui| {
                            if ui.button(tr("▶ Continue with back sides")).clicked() {
                                flip_answer = Some(true);
                            }
                            if ui.button(tr("Stop")).clicked() {
                                flip_answer = Some(false);
                            }
                        });
                    } else {
                        let frac = if j.total > 0 { j.done as f32 / j.total as f32 } else { 0.0 };
                        ui.label(&j.label);
                        ui.add(egui::ProgressBar::new(frac).show_percentage().animate(true));
                        if ui.button(tr("Cancel")).clicked() {
                            j.handle.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
        }
        if let (Some(a), Some(j)) = (flip_answer, self.job.as_mut()) {
            j.need_flip = None;
            let _ = j.handle.cont.send(a);
        }
    }
}

// ------------------------------------------------------------ app settings window

/// Resize/move the window to the saved preferences right away.
fn apply_window(ctx: &egui::Context, p: &Prefs) {
    let size = vec2(p.window_size[0], p.window_size[1]);
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    let pos = p.window_pos.map(|[x, y]| pos2(x, y)).or_else(|| {
        let monitor = ctx.input(|i| i.viewport().monitor_size)?;
        Some(((monitor - size) / 2.0).max(vec2(0.0, 0.0)).to_pos2())
    });
    if let Some(pos) = pos {
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
    }
    if p.start_maximized {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

type Toggle = fn() -> Result<String, String>;

pub const AUTHOR: &str = "Muriel Guedes Pucci Berehulka";

#[derive(Clone, Copy, PartialEq)]
enum PrefsTab {
    General,
    Installation,
    About,
}

impl App {
    fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let before = self.prefs.clone();
        let mut open = true;
        let mut action: Option<Toggle> = None;
        let mut check_updates = false;
        egui::Window::new(tr("Settings"))
            .id(egui::Id::new("app_settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.prefs_tab, PrefsTab::General, RichText::new(tr("General")).size(14.0));
                    ui.selectable_value(&mut self.prefs_tab, PrefsTab::Installation, RichText::new(tr("Installation")).size(14.0));
                    ui.selectable_value(&mut self.prefs_tab, PrefsTab::About, RichText::new(tr("About")).size(14.0));
                });
                ui.separator();
                if self.prefs_tab == PrefsTab::General {
                    ui.label(RichText::new(tr("Appearance")).strong());
                    grid(ui, "prefs_look", |ui| {
                        ui.label(tr("Language"));
                        let system = trf!("System ({})", i18n::system_lang().native_name());
                        let shown = match self.prefs.language {
                            LangPref::System => system.clone(),
                            LangPref::Fixed(l) => l.native_name().to_string(),
                        };
                        egui::ComboBox::from_id_salt("pref_lang").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.prefs.language, LangPref::System, system)
                                .on_hover_text(tr("Follows the Windows regional format"));
                            for l in [Lang::English, Lang::PortugueseBr] {
                                ui.selectable_value(&mut self.prefs.language, LangPref::Fixed(l), l.native_name());
                            }
                        });
                        ui.end_row();
                        ui.label(tr("Theme"));
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.prefs.theme, Theme::Light, tr("☀ Light"));
                            ui.selectable_value(&mut self.prefs.theme, Theme::Dark, tr("🌙 Dark"));
                        });
                        ui.end_row();
                    });

                    ui.separator();
                    ui.label(RichText::new(tr("Window")).strong());
                    grid(ui, "prefs_window", |ui| {
                        ui.label("");
                        ui.checkbox(&mut self.prefs.start_maximized, tr("Start maximized"));
                        ui.end_row();
                        ui.label(tr("Size"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.prefs.window_size[0]).range(MIN_WINDOW[0]..=7680.0));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.prefs.window_size[1]).range(MIN_WINDOW[1]..=4320.0));
                        });
                        ui.end_row();
                        ui.label(tr("Position"));
                        ui.horizontal(|ui| {
                            if ui.selectable_label(self.prefs.window_pos.is_none(), tr("Centered")).clicked() {
                                self.prefs.window_pos = None;
                            }
                            if ui.selectable_label(self.prefs.window_pos.is_some(), tr("Custom")).clicked() && self.prefs.window_pos.is_none() {
                                self.prefs.window_pos = Some([40.0, 40.0]);
                            }
                            if let Some(p) = &mut self.prefs.window_pos {
                                ui.add(egui::DragValue::new(&mut p[0]).range(-7680.0..=7680.0).prefix("x "));
                                ui.add(egui::DragValue::new(&mut p[1]).range(-4320.0..=4320.0).prefix("y "));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            if ui.button(tr("Use current size & position")).clicked() {
                                let (inner, outer, maximized) =
                                    ui.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect, i.viewport().maximized));
                                if let Some(r) = inner {
                                    self.prefs.window_size = [r.width().round(), r.height().round()];
                                }
                                if let Some(r) = outer {
                                    self.prefs.window_pos = Some([r.min.x.round(), r.min.y.round()]);
                                }
                                if let Some(m) = maximized {
                                    self.prefs.start_maximized = m;
                                }
                            }
                            if ui.button(tr("Apply now")).clicked() {
                                apply_window(ctx, &self.prefs);
                            }
                        });
                        ui.end_row();
                    });
                    hint(ui, tr("Used every time the app starts."));
                }
                if self.prefs_tab == PrefsTab::Installation {
                    let st = self.inst;
                    // (title, description, installed, enable label, disable label, enable, disable)
                    let rows: [(&str, &str, bool, &str, &str, Toggle, Toggle); 3] = [
                        (
                            "Program",
                            "Copies the program to your user folder and adds it to the Start menu and to Settings → Apps (no admin rights needed).",
                            st.program,
                            "Install",
                            "Uninstall",
                            install::install_program,
                            install::uninstall_program,
                        ),
                        (
                            "\"Open with\" for PDF files",
                            "Lists PDF Print Studio in Explorer's \"Open with\" menu for .pdf files.",
                            st.open_with,
                            "Enable",
                            "Disable",
                            install::register_open_with,
                            install::unregister_open_with,
                        ),
                        (
                            "Explorer context menu",
                            "Adds \"Print with PDF Print Studio…\" and \"Quick print\" to the right-click menu of PDF files.",
                            st.context_menu,
                            "Enable",
                            "Disable",
                            install::install_context_menu,
                            install::uninstall_context_menu,
                        ),
                    ];
                    egui::Grid::new("prefs_install").num_columns(3).spacing([12.0, 7.0]).show(ui, |ui| {
                        for (title, about, on, enable, disable, add, remove) in rows {
                            ui.label(tr(title)).on_hover_text(tr(about));
                            if on {
                                ui.label(RichText::new("✔").color(pal().ok));
                            } else {
                                ui.label(RichText::new("—").color(pal().muted));
                            }
                            if ui.button(tr(if on { disable } else { enable })).on_hover_text(tr(about)).clicked() {
                                action = Some(if on { remove } else { add });
                            }
                            ui.end_row();
                        }
                    });
                    let location = if st.program {
                        trf!("Installed in {}", install::install_dir().display())
                    } else {
                        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
                        trf!("Not installed — running from {}", exe)
                    };
                    hint(ui, &location);
                    if ui
                        .button(tr("Default PDF app…"))
                        .on_hover_text(tr("Windows only lets you choose the default app yourself; this opens the Default apps settings."))
                        .clicked()
                    {
                        install::open_default_apps();
                    }
                }
                if self.prefs_tab == PrefsTab::About {
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(64.0, 64.0)));
                        ui.vertical(|ui| {
                            ui.label(RichText::new("PDF Print Studio").strong().size(20.0));
                            ui.label(trf!("Version {}", update::VERSION));
                            ui.label(tr("Advanced PDF and comic book printing for Windows."));
                        });
                    });
                    ui.add_space(8.0);
                    ui.label(trf!("Made by {}", AUTHOR));
                    ui.hyperlink_to(format!("🔗 github.com/{}", update::REPO), format!("https://github.com/{}", update::REPO))
                        .on_hover_text(tr("Source code, releases and issues on GitHub"));
                    ui.hyperlink_to(tr("Report a problem"), format!("https://github.com/{}/issues", update::REPO));
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new(tr("Updates")).strong());
                    if ui.button(tr("🔄 Check for updates…")).clicked() {
                        check_updates = true;
                    }
                    ui.checkbox(&mut self.prefs.check_updates, tr("Check for updates when the app starts"));
                    ui.add_enabled(self.prefs.check_updates, egui::Checkbox::new(&mut self.prefs.auto_update, tr("Download and install updates automatically")))
                        .on_hover_text(tr("The new version is installed in the background; you choose when to restart"));
                }
            });
        self.prefs_open = open;

        if let Some(f) = action {
            match f() {
                Ok(m) => self.notify(m, false),
                Err(e) => self.notify(e, true),
            }
            self.inst = install::status();
        }
        if check_updates {
            self.updater.check(ctx);
        }
        if self.prefs != before {
            if self.prefs.language != before.language {
                self.prefs.apply_lang();
            }
            if self.prefs.theme != before.theme {
                theme::apply(ctx, self.prefs.theme);
            }
            self.prefs.save();
        }
    }
}

// ------------------------------------------------------------ comic books

impl App {
    /// Reopen the archive when load options changed, re-arrange pages when spread options changed.
    fn cbz_sync(&mut self, ctx: &egui::Context) {
        if !self.is_cbz || self.loading || self.src_sizes.is_empty() {
            return;
        }
        // wait until the user has finished typing or dragging a value
        let busy = ctx.memory(|m| m.focused().is_some()) || ctx.input(|i| i.pointer.any_down());
        if busy {
            return;
        }
        if self.s.cbz.load_key() != self.cbz_load_key {
            if let Some(p) = self.path.clone() {
                self.open(p);
            }
            return;
        }
        let c = &self.s.cbz;
        let key = format!("{}|{}|{}|{}|{}", c.rtl, c.split_spreads, c.keep_cover, c.cover_alone, c.spread_ratio);
        if key != self.cbz_arrange_key {
            let first = self.cbz_arrange_key.is_empty();
            self.cbz_arrange_key = key;
            let order = self.cbz_order();
            if first {
                // initial arrangement of a freshly opened archive: nothing to undo
                self.order = order;
                self.sync_order();
            } else {
                self.picked.clear();
                self.set_order(order);
            }
        }
    }

    /// Pages of the archive with spreads split (in reading order) and an optional blank after the cover.
    fn cbz_order(&self) -> Vec<PageSrc> {
        let c = &self.s.cbz;
        let mut out = Vec::new();
        for (i, &(w, h)) in self.src_sizes.iter().enumerate() {
            let page = PageSrc::Pdf(i, WHOLE);
            if c.split_spreads && w / h > c.spread_ratio && !(c.keep_cover && i == 0) {
                let [a, b] = page.split(&self.src_sizes);
                out.extend(if c.rtl { [b, a] } else { [a, b] });
            } else {
                out.push(page);
            }
            if i == 0 && c.cover_alone {
                out.push(PageSrc::Blank(w, h));
            }
        }
        out
    }

    /// Reading direction, also applied to the n-up order and booklet binding.
    fn set_rtl(&mut self, rtl: bool) {
        self.s.cbz.rtl = rtl;
        self.s.booklet_rtl = rtl;
        self.s.nup_order = match (rtl, self.s.nup_order) {
            (true, NupOrder::RowsLtr) => NupOrder::RowsRtl,
            (true, NupOrder::ColsTtb) => NupOrder::ColsTtbRtl,
            (false, NupOrder::RowsRtl) => NupOrder::RowsLtr,
            (false, NupOrder::ColsTtbRtl) => NupOrder::ColsTtb,
            (_, o) => o,
        };
    }

    fn ui_cbz(&mut self, ui: &mut Ui) {
        section(ui, "📚", tr("Comic book (CBZ)"), true, |ui| {
            if !self.cbz_meta.is_empty() {
                grid(ui, "cbz_meta", |ui| {
                    for (k, v) in &self.cbz_meta {
                        if k == "Summary" {
                            continue;
                        }
                        ui.label(tr(k));
                        ui.label(RichText::new(v).strong());
                        ui.end_row();
                    }
                });
                if let Some((_, summary)) = self.cbz_meta.iter().find(|(k, _)| k == "Summary") {
                    egui::CollapsingHeader::new(tr("Summary")).id_salt("cbz_summary").show(ui, |ui| {
                        ui.label(summary);
                    });
                }
                ui.add_space(4.0);
            }

            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Presets:"));
                if ui.small_button(tr("📖 Manga")).on_hover_text(tr("Right-to-left, split spreads, crisp grayscale")).clicked() {
                    self.set_rtl(true);
                    self.s.cbz.split_spreads = true;
                    self.reset_color();
                    self.s.color_mode = ColorMode::Grayscale;
                    self.s.contrast = 12.0;
                    self.s.sharpen = 0.6;
                }
                if ui.small_button(tr("🦸 Comic")).on_hover_text(tr("Left-to-right, split spreads, vivid color")).clicked() {
                    self.set_rtl(false);
                    self.s.cbz.split_spreads = true;
                    self.reset_color();
                    self.s.saturation = 108.0;
                    self.s.contrast = 5.0;
                }
                if ui.small_button(tr("📰 Two pages per sheet")).on_hover_text(tr("Side by side on landscape paper, like an open book")).clicked() {
                    self.s.layout = LayoutMode::Normal;
                    self.s.nup_cols = 2;
                    self.s.nup_rows = 1;
                    self.s.cell_gap_mm = 0.0;
                    self.s.orientation = Orientation::Landscape;
                    self.s.scaling = Scaling::Fit;
                    self.s.cbz.cover_alone = true;
                    self.set_rtl(self.s.cbz.rtl);
                }
                if ui.small_button(tr("📚 Booklet")).on_hover_text(tr("Fold-and-staple booklet, printed two-sided")).clicked() {
                    self.s.layout = LayoutMode::Booklet;
                    self.s.cell_gap_mm = 0.0;
                    self.s.duplex = if self.caps.as_ref().is_some_and(|c| c.duplex) { Duplex::ShortEdge } else { Duplex::Manual };
                    self.set_rtl(self.s.cbz.rtl);
                }
                if ui.small_button(tr("🖼 Full bleed")).on_hover_text(tr("Fill the paper edge to edge, cropping the overflow")).clicked() {
                    self.s.scaling = Scaling::Fill;
                    self.s.margins_mm = [0.0; 4];
                    self.s.respect_hw_margins = false;
                }
            });

            let mut rtl = self.s.cbz.rtl;
            grid(ui, "cbz_grid", |ui| {
                ui.label(tr("Reading"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut rtl, false, tr("Left → right"));
                    ui.selectable_value(&mut rtl, true, tr("← Right to left (manga)"));
                });
                ui.end_row();

                ui.label(tr("Page order"));
                combo(
                    ui,
                    "cbz_sort",
                    &mut self.s.cbz.sort,
                    &[
                        (CbzSort::Natural, tr("By name, numbers in order (2 before 10)")),
                        (CbzSort::Name, tr("By name, character by character")),
                        (CbzSort::Archive, tr("As stored in the archive")),
                    ],
                );
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.cbz.reverse, tr("Reverse the image order"));
                ui.end_row();

                ui.label(tr("Skip small images"));
                ui.add(egui::DragValue::new(&mut self.s.cbz.min_px).range(0..=4000).speed(5.0).custom_formatter(|v, _| {
                    if v == 0.0 { tr("off").into() } else { trf!("under {} px", v) }
                }))
                .on_hover_text(tr("Leaves out credit pages, banners and thumbnails whose shorter side is smaller"));
                ui.end_row();
                ui.label(tr("Skip files named"));
                ui.add(egui::TextEdit::singleline(&mut self.s.cbz.exclude).hint_text(tr("e.g. credits, scan, ad")).desired_width(220.0))
                    .on_hover_text(tr("Comma-separated parts of file names to leave out"));
                ui.end_row();

                ui.label(tr("Double-page spreads"));
                ui.checkbox(&mut self.s.cbz.split_spreads, tr("Split into two pages"));
                ui.end_row();
                if self.s.cbz.split_spreads {
                    ui.label(tr("Spread when wider than"));
                    ui.add(egui::DragValue::new(&mut self.s.cbz.spread_ratio).range(1.0..=3.0).speed(0.01).max_decimals(2).prefix("× "))
                        .on_hover_text(tr("Width ÷ height above which an image counts as a spread"));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.s.cbz.keep_cover, tr("Never split the cover"));
                    ui.end_row();
                }
                ui.label(tr("Cover"));
                ui.checkbox(&mut self.s.cbz.cover_alone, tr("On its own (blank page after it)"))
                    .on_hover_text(tr("Keeps facing pages together when printing two pages per sheet or as a booklet"));
                ui.end_row();

                ui.label(tr("Image resolution"));
                ui.add(egui::DragValue::new(&mut self.s.cbz.dpi).range(36.0..=2400.0).speed(1.0).suffix(" dpi"))
                    .on_hover_text(tr("Only matters for \"Actual size\"; the other scaling modes fit the page anyway"));
                ui.end_row();
            });
            if rtl != self.s.cbz.rtl {
                self.set_rtl(rtl);
            }
            let spreads = self.src_sizes.iter().filter(|(w, h)| w / h > self.s.cbz.spread_ratio).count();
            hint(ui, &trf!("{} images · {} spreads · {} pages after arranging", self.src_sizes.len(), spreads, self.order.len()));
        });
    }
}
