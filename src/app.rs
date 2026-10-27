//! egui front-end: thumbnails, document viewer, live print preview and the settings panel.

use crate::compose::{self, StampCtx};
use crate::install;
use crate::job::{self, JobHandle, JobMsg, Target};
use crate::layout::{self, Sheet};
use crate::manual::{self, Manual};
use crate::pages;
use crate::printer::{self, Caps, Geometry, PrinterInfo};
use crate::settings::*;
use crate::template::{self, Clock, Vars};
use crate::text;
use crate::worker::{self, Req, Resp};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ColorImage, FontId, Key, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Ui,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};

const ACCENT: Color32 = Color32::from_rgb(64, 156, 255);
const THUMB_W: u32 = 180;

pub fn run(file: Option<PathBuf>, quick: bool) -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PDF Print Studio")
            .with_inner_size([1560.0, 940.0])
            .with_min_inner_size([1100.0, 650.0])
            .with_maximized(true)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "PDF Print Studio",
        opts,
        Box::new(move |cc| Ok(Box::new(App::new(cc, file, quick)))),
    )
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
    sizes_mm: Vec<(f32, f32)>,
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
    installed: bool,
    manual: Manual,
    hwnd: isize,
    egui_ctx: egui::Context,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>, quick: bool) -> Self {
        setup_style(&cc.egui_ctx);
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
            installed: install::is_installed(),
            manual: Manual::default(),
            hwnd: 0,
            egui_ctx: cc.egui_ctx.clone(),
        };
        app.refresh_printers();
        if let Some(f) = file {
            app.open(f);
        }
        app
    }

    // ------------------------------------------------------------ document

    fn open(&mut self, path: PathBuf) {
        self.path = Some(path.clone());
        self.sizes_mm.clear();
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
        let _ = self.tx.send(Req::Open { path, password: pw });
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
                    self.sizes_mm = info.sizes_mm;
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
                self.notify("Nothing selected to print", true);
                return;
            }
        };
        let export = export_dir.is_some();
        let target = match export_dir {
            Some(dir) => Target::PngFolder { dir, sheet_mm: self.sheet_mm, hw: self.hw },
            None => {
                if self.s.printer.is_empty() || self.devmode.is_empty() {
                    self.notify("No printer available", true);
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
            selection: sel,
            landscape: self.landscape,
            target,
            stamp: self.stamp(1),
        };
        let handle = job::spawn(spec, self.egui_ctx.clone());
        self.job = Some(JobUi { handle, done: 0, total: 0, label: "Starting…".into(), need_flip: None, export });
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

    let mut v = egui::Visuals::dark();
    v.selection.bg_fill = ACCENT.linear_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.panel_fill = Color32::from_rgb(28, 30, 34);
    v.window_fill = Color32::from_rgb(34, 36, 41);
    v.extreme_bg_color = Color32::from_rgb(20, 21, 24);
    v.faint_bg_color = Color32::from_rgb(38, 40, 46);
    v.widgets.noninteractive.bg_fill = Color32::from_rgb(34, 36, 41);
    v.window_rounding = 8.0.into();
    ctx.set_visuals(v);
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
    ui.label(RichText::new(t).small().color(Color32::from_gray(140)));
}

fn help_button(ui: &mut Ui) -> bool {
    ui.small_button("❓").on_hover_text("Open the manual").clicked()
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
    let shown = if val.is_empty() { "(overlay font)" } else { val.as_str() };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(220.0).height(360.0).show_ui(ui, |ui| {
        if allow_default {
            ui.selectable_value(val, String::new(), "(overlay font)");
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
        ui.label(RichText::new(format!("⚠ {e}")).small().color(Color32::from_rgb(255, 120, 110)));
    }
}

/// Template field: hover shows the result for the previewed side, errors appear below.
fn tpl_edit(ui: &mut Ui, src: &mut String, vars: &Vars) {
    ui.vertical(|ui| {
        let r = ui.add(egui::TextEdit::singleline(src).desired_width(240.0).font(egui::TextStyle::Monospace));
        if r.hovered() && src.contains('{') {
            let out = template::render(src, vars);
            r.on_hover_text(format!("On this side: {}", if out.is_empty() { "(empty)" } else { &out }));
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
            ui.add(egui::TextEdit::singleline(src).hint_text("always").desired_width(170.0).font(egui::TextStyle::Monospace));
            egui::ComboBox::from_id_salt(id).selected_text("▾").width(30.0).show_ui(ui, |ui| {
                for (label, c) in CHOICES {
                    if ui.selectable_label(src.trim() == c, label).clicked() {
                        *src = c.to_string();
                    }
                }
            });
            if !src.trim().is_empty() {
                let (txt, col) = if template::condition(src, vars) {
                    ("✔ shown", Color32::from_rgb(120, 200, 120))
                } else {
                    ("✖ hidden", Color32::from_gray(150))
                };
                ui.label(RichText::new(txt).small().color(col)).on_hover_text("On the side shown in the preview");
            }
        });
        if let Some(e) = template::check_condition(src, vars) {
            ui.label(RichText::new(format!("⚠ {e}")).small().color(Color32::from_rgb(255, 120, 110)));
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
                ui.label(RichText::new("🖨 PDF Print Studio").strong().size(17.0).color(ACCENT));
                ui.separator();
                if ui.button("📂 Open…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                        self.password.clear();
                        self.open(p);
                    }
                }
                if !self.sizes_mm.is_empty() {
                    ui.label(RichText::new(self.file_name()).strong());
                    ui.label(RichText::new(format!("{} pages", self.sizes_mm.len())).color(Color32::GRAY));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("❓").size(16.0)).frame(false))
                        .on_hover_text("Manual & template help (F1)")
                        .clicked()
                    {
                        self.manual.open = !self.manual.open;
                    }
                    let (label, tip) = if self.installed {
                        ("✔ Context menu installed", "Click to remove the Explorer context-menu entries")
                    } else {
                        ("➕ Install context menu", "Adds \"Print with PDF Print Studio…\" to the right-click menu of PDF files")
                    };
                    if ui.button(label).on_hover_text(tip).clicked() {
                        let r = if self.installed { install::uninstall() } else { install::install() };
                        match r {
                            Ok(m) => self.notify(m.lines().next().unwrap_or_default().to_string(), false),
                            Err(e) => self.notify(e, true),
                        }
                        self.installed = install::is_installed();
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
                    ui.label(RichText::new(format!("Page {}: {:.0} × {:.0} mm", self.current + 1, w, h)).small());
                }
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "Sheet {:.0} × {:.0} mm ({})",
                        self.sheet_mm.0,
                        self.sheet_mm.1,
                        if self.landscape { "landscape" } else { "portrait" }
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
                            let c = if *err { Color32::from_rgb(255, 110, 100) } else { Color32::from_rgb(120, 220, 140) };
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Pages").strong());
                hint(ui, "click = view · ctrl+click = pick");
            });
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
            scroll.show_rows(ui, row_h, n, |ui, range| {
                for i in range {
                    if !self.thumbs.contains_key(&i) && self.thumb_req.insert(i) {
                        let _ = self.tx.send(Req::Thumb { page: i, width: THUMB_W });
                    }
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
                    let p = ui.painter_at(rect);
                    let is_cur = i == self.current;
                    let in_sel = self.sel_set.contains(&i);
                    let picked = self.picked.contains(&i);
                    if is_cur {
                        p.rect_filled(rect, 6.0, ACCENT.linear_multiply(0.18));
                    } else if resp.hovered() {
                        p.rect_filled(rect, 6.0, Color32::from_white_alpha(8));
                    }
                    let img_area = egui::Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(w - 16.0, row_h - 30.0));
                    let (pw, ph) = self.sizes_mm[i];
                    let k = (img_area.width() / pw).min(img_area.height() / ph);
                    let sz = vec2(pw * k, ph * k);
                    let r = egui::Rect::from_center_size(img_area.center(), sz);
                    p.rect_filled(r.translate(vec2(2.0, 2.0)), 1.0, Color32::from_black_alpha(90));
                    if let Some(t) = self.thumbs.get(&i) {
                        p.image(t.id(), r, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if in_sel { Color32::WHITE } else { Color32::from_gray(110) });
                    } else {
                        p.rect_filled(r, 1.0, Color32::from_gray(200));
                    }
                    let border = if picked { Color32::from_rgb(255, 190, 60) } else if is_cur { ACCENT } else { Color32::from_gray(70) };
                    p.rect_stroke(r, 1.0, Stroke::new(if picked || is_cur { 2.0_f32 } else { 1.0 }, border));
                    let label_pos = pos2(rect.center().x, rect.bottom() - 12.0);
                    p.text(label_pos, Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(12.0), if is_cur { ACCENT } else { Color32::from_gray(190) });
                    if in_sel {
                        p.text(pos2(r.right() - 4.0, r.top() + 4.0), Align2::RIGHT_TOP, "✔", FontId::proportional(13.0), Color32::from_rgb(70, 190, 100));
                    }
                    if picked {
                        p.text(pos2(r.left() + 4.0, r.top() + 4.0), Align2::LEFT_TOP, "★", FontId::proportional(13.0), Color32::from_rgb(255, 190, 60));
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
                    resp.on_hover_text(format!("Page {} — {:.0} × {:.0} mm", i + 1, pw, ph));
                }
            });
        });
    }

    // ------------------------------------------------------------ central

    fn ui_central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::none().fill(Color32::from_rgb(22, 23, 26))).show(ctx, |ui| {
            if self.sizes_mm.is_empty() {
                self.ui_empty(ui);
                return;
            }
            egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 6.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Preview, RichText::new("🖨  Print preview").size(14.0));
                    ui.selectable_value(&mut self.tab, Tab::Viewer, RichText::new("📄  Document viewer").size(14.0));
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
                ui.label("Opening document…");
                return;
            }
            ui.label(RichText::new("🖨").size(64.0).color(ACCENT));
            ui.label(RichText::new("Drop a PDF here or open one").size(22.0));
            ui.add_space(10.0);
            if ui.add(egui::Button::new(RichText::new("📂  Open PDF…").size(16.0)).min_size(vec2(180.0, 36.0))).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                    self.open(p);
                }
            }
            if let Some(e) = &self.open_error {
                ui.add_space(10.0);
                ui.label(RichText::new(format!("Could not open file: {e}")).color(Color32::from_rgb(255, 110, 100)));
            }
            ui.add_space(24.0);
            if !self.installed {
                hint(ui, "Tip: use \"Install context menu\" (top right) to print any PDF from Explorer's right-click menu.");
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
                    ui.add(egui::DragValue::new(&mut shown).range(1..=n.max(1)).prefix("Side ").suffix(format!(" / {n}")));
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
                        if sh.back { "back side" } else { "front side" }
                    } else {
                        "single-sided"
                    };
                    let pages = sh.pages().iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ");
                    ui.label(RichText::new(format!("{side} · pages {pages}")).color(Color32::GRAY));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Fit").clicked() {
                        self.preview_zoom = 1.0;
                    }
                    ui.add(egui::Slider::new(&mut self.preview_zoom, 0.5..=4.0).show_value(false).text("Zoom"));
                    ui.checkbox(&mut self.show_cells, "Cells");
                    ui.checkbox(&mut self.show_printable, "Printable area");
                    if let Some(c) = self.coverage {
                        ui.label(RichText::new(format!("≈ {c:.1}% ink coverage")).color(Color32::from_rgb(150, 200, 255)))
                            .on_hover_text("Estimated toner/ink coverage of this side (5% is a typical text page)");
                    }
                });
            });
        });

        if n == 0 {
            ui.centered_and_justified(|ui| {
                let msg = match &self.selection {
                    Err(e) => format!("⚠ {e}"),
                    Ok(_) => "No pages selected".into(),
                };
                ui.label(RichText::new(msg).size(16.0).color(Color32::from_rgb(255, 170, 90)));
            });
            return;
        }

        let ppp = ui.ctx().pixels_per_point();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_size();
            let (sw, sh) = self.sheet_mm;
            let fit = ((avail.x - 48.0) / sw).min((avail.y - 48.0) / sh).max(0.05);
            let k = fit * self.preview_zoom;
            let size = vec2(sw * k, sh * k);
            let (outer, _) = ui.allocate_exact_size(vec2(size.x + 48.0, size.y + 48.0).max(avail), Sense::hover());
            let rect = egui::Rect::from_center_size(outer.center(), size);
            let p = ui.painter();
            p.rect_filled(rect.translate(vec2(5.0, 6.0)), 2.0, Color32::from_black_alpha(120));
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
                if ui.add(egui::DragValue::new(&mut shown).range(1..=n).prefix("Page ").suffix(format!(" / {n}"))).changed() {
                    self.scroll_thumb = true;
                }
                self.current = shown.saturating_sub(1).min(n - 1);
                if ui.button("▶").clicked() && self.current + 1 < n {
                    self.current += 1;
                    self.scroll_thumb = true;
                }
                ui.separator();
                let picked = self.picked.contains(&self.current);
                if ui.selectable_label(picked, "★ Pick page").on_hover_text("Add/remove this page from the 'Picked' selection").clicked() {
                    if picked {
                        self.picked.remove(&self.current);
                    } else {
                        self.picked.insert(self.current);
                    }
                    self.s.range_mode = RangeMode::Picked;
                }
                let in_sel = self.sel_set.contains(&self.current);
                ui.label(if in_sel {
                    RichText::new("✔ will be printed").color(Color32::from_rgb(70, 190, 100))
                } else {
                    RichText::new("✖ not in selection").color(Color32::GRAY)
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (z, l) in [(2.0, "200%"), (1.5, "150%"), (1.0, "Fit width"), (0.5, "50%")] {
                        ui.selectable_value(&mut self.view_zoom, z, l);
                    }
                });
            });
        });
        let ppp = ui.ctx().pixels_per_point();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_size();
            let (pw, ph) = self.sizes_mm[self.current];
            let w = ((avail.x - 40.0) * self.view_zoom).max(100.0);
            let h = w * ph / pw;
            let (outer, _) = ui.allocate_exact_size(vec2(w + 40.0, h + 40.0).max(avail), Sense::hover());
            let rect = egui::Rect::from_min_size(pos2(outer.center().x - w / 2.0, outer.min.y + 20.0), vec2(w, h));
            let p = ui.painter();
            p.rect_filled(rect.translate(vec2(4.0, 5.0)), 2.0, Color32::from_black_alpha(120));
            p.rect_filled(rect, 0.0, Color32::WHITE);
            let tex = self.view_tex.as_ref().filter(|(pg, _)| *pg == self.current).map(|(_, t)| t.id());
            let tex = tex.or_else(|| self.thumbs.get(&self.current).map(|t| t.id()));
            if let Some(id) = tex {
                p.image(id, rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let want = (((w * ppp) / 64.0).ceil() * 64.0).min(6000.0) as u32;
            if self.view_req != Some((self.current, want)) {
                self.view_req = Some((self.current, want));
                let _ = self.tx.send(Req::View { page: self.current, width: want });
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
        section(ui, "🖨", "Printer", true, |ui| {
            let mut chosen = None;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("printer")
                    .selected_text(if self.s.printer.is_empty() { "No printers found" } else { &self.s.printer })
                    .width(ui.available_width() - 70.0)
                    .show_ui(ui, |ui| {
                        for p in &self.printers {
                            let label = if p.is_default { format!("★ {}", p.name) } else { p.name.clone() };
                            if ui.selectable_label(p.name == self.s.printer, label).clicked() {
                                chosen = Some(p.name.clone());
                            }
                        }
                    });
                if ui.button("⟳").on_hover_text("Refresh printer list and status").clicked() {
                    self.refresh_printers();
                }
            });
            if let Some(n) = chosen {
                self.select_printer(&n);
            }
            if let Some(p) = self.current_printer().cloned() {
                let (st, bad) = p.status_text();
                ui.horizontal(|ui| {
                    let c = if bad { Color32::from_rgb(255, 110, 100) } else { Color32::from_rgb(100, 210, 120) };
                    ui.label(RichText::new("●").color(c));
                    ui.label(RichText::new(st).color(c));
                    if p.jobs > 0 {
                        ui.label(RichText::new(format!("· {} job(s) queued", p.jobs)).color(Color32::GRAY));
                    }
                    if let Some(c) = &self.caps {
                        ui.label(RichText::new(if c.color { "· color" } else { "· monochrome" }).color(Color32::GRAY));
                        if c.duplex {
                            ui.label(RichText::new("· duplex").color(Color32::GRAY));
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
            if ui.button("⚙  Printer properties (driver dialog)…").clicked() && !self.s.printer.is_empty() {
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
                ui.label("Paper size");
                let paper_label = |id: u16| -> String {
                    match id {
                        0 => "Printer default".into(),
                        256 => "Custom size".into(),
                        _ => caps
                            .papers
                            .iter()
                            .find(|p| p.id == id)
                            .map(|p| format!("{}  ({:.0}×{:.0})", p.name, p.w_mm, p.h_mm))
                            .unwrap_or_else(|| format!("#{id}")),
                    }
                };
                egui::ComboBox::from_id_salt("paper").selected_text(paper_label(self.s.paper_id)).width(230.0).height(400.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.s.paper_id, 0, "Printer default");
                    for p in &caps.papers {
                        ui.selectable_value(&mut self.s.paper_id, p.id, format!("{}  ({:.0}×{:.0} mm)", p.name, p.w_mm, p.h_mm));
                    }
                    ui.selectable_value(&mut self.s.paper_id, 256, "Custom size…");
                });
                ui.end_row();
                if self.s.paper_id == 256 {
                    ui.label("Custom (w × h)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.s.custom_paper_mm[0]).range(20.0..=2000.0).suffix(" mm"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.s.custom_paper_mm[1]).range(20.0..=5000.0).suffix(" mm"));
                    });
                    ui.end_row();
                }
                ui.label("Orientation");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.orientation, Orientation::Auto, "Auto");
                    ui.selectable_value(&mut self.s.orientation, Orientation::Portrait, "▯ Portrait");
                    ui.selectable_value(&mut self.s.orientation, Orientation::Landscape, "▭ Landscape");
                });
                ui.end_row();
                if self.s.orientation == Orientation::Auto {
                    ui.label("");
                    hint(ui, &format!("auto → {}", if self.landscape { "landscape" } else { "portrait" }));
                    ui.end_row();
                }
                ui.label("Paper source");
                let mut bins: Vec<(u16, &str)> = vec![(0, "Automatic / driver default")];
                bins.extend(caps.bins.iter().map(|(i, n)| (*i, n.as_str())));
                combo(ui, "bin", &mut self.s.bin, &bins);
                ui.end_row();
                if !caps.media.is_empty() {
                    ui.label("Media type");
                    let mut media: Vec<(u32, &str)> = vec![(0, "Driver default")];
                    media.extend(caps.media.iter().map(|(i, n)| (*i, n.as_str())));
                    combo(ui, "media", &mut self.s.media, &media);
                    ui.end_row();
                }
                ui.label("Print quality");
                let mut q: Vec<(Quality, String)> = vec![
                    (Quality::Default, "Driver default".into()),
                    (Quality::Draft, "Draft".into()),
                    (Quality::Low, "Low".into()),
                    (Quality::Medium, "Medium".into()),
                    (Quality::High, "High".into()),
                ];
                q.extend(caps.resolutions.iter().map(|&(x, y)| (Quality::Res(x, y), format!("{x} × {y} dpi"))));
                let qref: Vec<(Quality, &str)> = q.iter().map(|(a, b)| (*a, b.as_str())).collect();
                combo(ui, "quality", &mut self.s.quality, &qref);
                ui.end_row();
                ui.label("Two-sided");
                combo(
                    ui,
                    "duplex",
                    &mut self.s.duplex,
                    &[
                        (Duplex::Off, "One-sided"),
                        (Duplex::LongEdge, "Two-sided · flip on long edge"),
                        (Duplex::ShortEdge, "Two-sided · flip on short edge"),
                        (Duplex::Manual, "Manual duplex (reinsert paper)"),
                    ],
                );
                ui.end_row();
                if matches!(self.s.duplex, Duplex::LongEdge | Duplex::ShortEdge) && !caps.duplex && self.caps.is_some() {
                    ui.label("");
                    ui.label(RichText::new("⚠ printer reports no duplex unit — try Manual").small().color(Color32::from_rgb(255, 170, 90)));
                    ui.end_row();
                }
                if self.s.duplex == Duplex::Manual {
                    ui.label("");
                    ui.checkbox(&mut self.s.manual_duplex_reverse, "Print back sides in reverse order");
                    ui.end_row();
                }
                ui.label("Copies");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.s.copies).range(1..=9999).speed(0.2));
                    ui.checkbox(&mut self.s.collate, "Collate").on_hover_text("1,2,3 1,2,3 instead of 1,1 2,2 3,3");
                });
                ui.end_row();
                ui.label("Job name");
                let file_name = self.file_name();
                ui.add(egui::TextEdit::singleline(&mut self.s.job_name).hint_text(file_name).desired_width(220.0));
                ui.end_row();
                ui.label("Output");
                ui.checkbox(&mut self.s.print_to_file, "Print to file (driver output)");
                ui.end_row();
                if self.s.print_to_file {
                    ui.label("File");
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
        section(ui, "📑", "Pages", true, |ui| {
            ui.radio_value(&mut self.s.range_mode, RangeMode::All, format!("All pages (1-{n})"));
            ui.radio_value(&mut self.s.range_mode, RangeMode::Current, format!("Current page ({})", self.current + 1));
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.s.range_mode, RangeMode::Custom, "Range");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.s.custom_range)
                        .hint_text("1-3, 5, 8-end, 10-4, 2*3")
                        .desired_width(ui.available_width()),
                );
                if r.gained_focus() || r.changed() {
                    self.s.range_mode = RangeMode::Custom;
                }
            });
            ui.radio_value(&mut self.s.range_mode, RangeMode::Picked, format!("Picked in sidebar ({} ★)", self.picked.len()));
            ui.horizontal_wrapped(|ui| {
                let half = n / 2;
                let set = |s: &mut Settings, r: String| {
                    s.range_mode = RangeMode::Custom;
                    s.custom_range = r;
                };
                if ui.small_button("First half").clicked() {
                    set(&mut self.s, format!("1-{}", half.max(1)));
                }
                if ui.small_button("Second half").clicked() {
                    set(&mut self.s, format!("{}-end", (half + 1).min(n)));
                }
                if ui.small_button("Last page").clicked() {
                    set(&mut self.s, "end".into());
                }
                if ui.small_button("Reverse all").clicked() {
                    set(&mut self.s, "end-1".into());
                }
                if ui.small_button("★ all").clicked() {
                    self.picked = (0..n).collect();
                }
                if ui.small_button("★ invert").clicked() {
                    self.picked = (0..n).filter(|p| !self.picked.contains(p)).collect();
                }
                if ui.small_button("★ clear").clicked() {
                    self.picked.clear();
                }
            });
            ui.add_space(4.0);
            grid(ui, "pages_grid", |ui| {
                ui.label("Include");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.subset, Subset::All, "All");
                    ui.selectable_value(&mut self.s.subset, Subset::Odd, "Odd only");
                    ui.selectable_value(&mut self.s.subset, Subset::Even, "Even only");
                });
                ui.end_row();
                ui.label("Every n-th");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.s.every_nth).range(1..=100).prefix("every "));
                    ui.add(egui::DragValue::new(&mut self.s.nth_offset).range(0..=99).prefix("offset "));
                });
                ui.end_row();
                ui.label("Exclude");
                ui.add(egui::TextEdit::singleline(&mut self.s.exclude).hint_text("e.g. 1, 7-9").desired_width(220.0));
                ui.end_row();
                ui.label("Order");
                ui.checkbox(&mut self.s.reverse, "Reverse (last page first)");
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
                    ui.label(RichText::new(format!("→ {} page(s): {d}", v.len())).color(Color32::from_rgb(120, 200, 140)));
                }
                Err(e) => {
                    ui.label(RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(255, 110, 100)));
                }
            }
        });
    }

    fn ui_layout(&mut self, ui: &mut Ui) {
        section(ui, "▦", "Layout", true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.s.layout, LayoutMode::Normal, "▦ Pages per sheet");
                ui.selectable_value(&mut self.s.layout, LayoutMode::Booklet, "📖 Booklet");
                ui.selectable_value(&mut self.s.layout, LayoutMode::Poster, "🧩 Poster / tiling");
                ui.selectable_value(&mut self.s.layout, LayoutMode::Repeat, "🔁 Step & repeat");
            });
            ui.add_space(4.0);
            match self.s.layout {
                LayoutMode::Normal => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Quick:");
                        for (n, c, r) in [(1, 1, 1), (2, 2, 1), (4, 2, 2), (6, 2, 3), (8, 2, 4), (9, 3, 3), (12, 3, 4), (16, 4, 4)] {
                            let on = self.s.nup_cols * self.s.nup_rows == n;
                            if ui.selectable_label(on, format!("{n}")).clicked() {
                                self.s.nup_cols = c;
                                self.s.nup_rows = r;
                            }
                        }
                    });
                    grid(ui, "nup_grid", |ui| {
                        ui.label("Grid");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.nup_cols).range(1..=16).suffix(" cols"));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.nup_rows).range(1..=16).suffix(" rows"));
                        });
                        ui.end_row();
                        ui.label("Page order");
                        combo(
                            ui,
                            "nup_order",
                            &mut self.s.nup_order,
                            &[
                                (NupOrder::RowsLtr, "Z  → across, then down"),
                                (NupOrder::RowsRtl, "←  across (right to left)"),
                                (NupOrder::ColsTtb, "N  ↓ down, then across"),
                                (NupOrder::ColsTtbRtl, "↓  down, right to left"),
                            ],
                        );
                        ui.end_row();
                        ui.label("Gap between pages");
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                    });
                }
                LayoutMode::Booklet => {
                    hint(ui, "Pages are reordered so the folded stack reads as a book. Use two-sided printing, flip on short edge (or manual duplex).");
                    grid(ui, "booklet_grid", |ui| {
                        ui.label("Binding");
                        ui.checkbox(&mut self.s.booklet_rtl, "Right-to-left (manga / Arabic)");
                        ui.end_row();
                        ui.label("Signatures");
                        ui.add(egui::DragValue::new(&mut self.s.booklet_signature).range(0..=64).custom_formatter(|v, _| {
                            if v == 0.0 { "single booklet".into() } else { format!("{v} sheets each") }
                        }));
                        ui.end_row();
                        ui.label("Center gutter");
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.s.booklet_align_fold, "Push pages towards the fold");
                        ui.end_row();
                    });
                    if self.s.duplex == Duplex::Off {
                        ui.label(RichText::new("⚠ duplex is off — set Two-sided in the Printer section").small().color(Color32::from_rgb(255, 170, 90)));
                    }
                }
                LayoutMode::Poster => {
                    hint(ui, "Each page is enlarged across several sheets you can trim and tape together.");
                    grid(ui, "poster_grid", |ui| {
                        ui.label("Tiles");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.poster_x).range(1..=20).suffix(" across"));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.poster_y).range(1..=20).suffix(" down"));
                        });
                        ui.end_row();
                        ui.label("Overlap");
                        mm(ui, &mut self.s.poster_overlap_mm, 60.0);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.s.poster_guides, "Glue lines and tile labels");
                        ui.end_row();
                    });
                    if let Some(&p) = self.selection.as_ref().ok().and_then(|v| v.first()) {
                        if let Some(pl) = self.sheets.first().and_then(|s| s.placements.first()) {
                            let (pw, ph) = self.sizes_mm[p];
                            let k = (pl.frame.w / pw).min(pl.frame.h / ph) * 100.0;
                            hint(ui, &format!("Final poster ≈ {:.0} × {:.0} mm ({k:.0}% of original)", pl.frame.w, pl.frame.h));
                        }
                    }
                }
                LayoutMode::Repeat => {
                    hint(ui, "Fills each sheet with copies of the same page — labels, business cards, flyers.");
                    grid(ui, "repeat_grid", |ui| {
                        ui.label("Grid");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.s.nup_cols).range(1..=16).suffix(" cols"));
                            ui.label("×");
                            ui.add(egui::DragValue::new(&mut self.s.nup_rows).range(1..=16).suffix(" rows"));
                        });
                        ui.end_row();
                        ui.label("Gap");
                        mm(ui, &mut self.s.cell_gap_mm, 100.0);
                        ui.end_row();
                    });
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Margins").strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.link_margins, "🔗 Same on all sides");
                for (label, v) in [("None", 0.0), ("Narrow", 5.0), ("Normal", 12.7), ("Wide", 25.4)] {
                    if ui.small_button(label).clicked() {
                        self.s.margins_mm = [v; 4];
                    }
                }
            });
            let before = self.s.margins_mm;
            grid(ui, "margins_grid", |ui| {
                ui.label("Top / Bottom");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.margins_mm[0], 200.0);
                    mm(ui, &mut self.s.margins_mm[2], 200.0);
                });
                ui.end_row();
                ui.label("Left / Right");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.margins_mm[3], 200.0);
                    mm(ui, &mut self.s.margins_mm[1], 200.0);
                });
                ui.end_row();
                ui.label("Binding gutter");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.binding_gutter_mm, 100.0);
                    combo(ui, "bind_edge", &mut self.s.binding_edge, &[(BindingEdge::Left, "left"), (BindingEdge::Right, "right"), (BindingEdge::Top, "top")]);
                });
                ui.end_row();
            });
            if self.link_margins {
                if let Some(i) = (0..4).find(|&i| self.s.margins_mm[i] != before[i]) {
                    self.s.margins_mm = [self.s.margins_mm[i]; 4];
                }
            }
            ui.checkbox(&mut self.s.respect_hw_margins, "Keep content inside the printer's printable area");
            if self.hw.iter().any(|&m| m > 0.0) {
                hint(ui, &format!(
                    "Hardware margins: top {:.1}, right {:.1}, bottom {:.1}, left {:.1} mm",
                    self.hw[0], self.hw[1], self.hw[2], self.hw[3]
                ));
            }
        });
    }

    fn ui_sizing(&mut self, ui: &mut Ui) {
        section(ui, "⤢", "Size & position", true, |ui| {
            grid(ui, "size_grid", |ui| {
                ui.label("Scaling");
                combo(
                    ui,
                    "scaling",
                    &mut self.s.scaling,
                    &[
                        (Scaling::Fit, "Fit to printable area"),
                        (Scaling::ShrinkOnly, "Shrink oversized pages only"),
                        (Scaling::Actual, "Actual size (100%)"),
                        (Scaling::Custom, "Custom scale"),
                        (Scaling::Fill, "Fill (crop to fill)"),
                        (Scaling::Stretch, "Stretch (ignore aspect)"),
                    ],
                );
                ui.end_row();
                if self.s.scaling == Scaling::Custom {
                    ui.label("Scale");
                    ui.add(egui::Slider::new(&mut self.s.custom_scale, 5.0..=400.0).suffix(" %").logarithmic(true));
                    ui.end_row();
                }
                ui.label("Alignment");
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
                ui.label("Offset (x, y)");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.offset_mm[0], 500.0);
                    mm(ui, &mut self.s.offset_mm[1], 500.0);
                });
                ui.end_row();
                ui.label("Rotation");
                ui.horizontal(|ui| {
                    for r in [0, 90, 180, 270] {
                        ui.selectable_value(&mut self.s.rotation, r, format!("{r}°"));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.auto_rotate, "Auto-rotate pages to best fit");
                ui.end_row();
                ui.label("Mirror");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.s.mirror_h, "Horizontal ⇋");
                    ui.checkbox(&mut self.s.mirror_v, "Vertical ⇵");
                });
                ui.end_row();
                ui.label("Trim source (T/B)");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.crop_mm[0], 200.0);
                    mm(ui, &mut self.s.crop_mm[2], 200.0);
                });
                ui.end_row();
                ui.label("Trim source (L/R)");
                ui.horizontal(|ui| {
                    mm(ui, &mut self.s.crop_mm[3], 200.0);
                    mm(ui, &mut self.s.crop_mm[1], 200.0);
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.auto_crop, "Auto-trim white borders")
                    .on_hover_text("Detects the content bounding box of every page and zooms in on it");
                ui.end_row();
            });
            for v in self.s.crop_mm.iter_mut() {
                *v = v.max(0.0);
            }
        });
    }

    fn ui_color(&mut self, ui: &mut Ui) {
        section(ui, "🎨", "Color & image", false, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Presets:");
                if ui.small_button("Original").clicked() {
                    self.reset_color();
                }
                if ui.small_button("Crisp text").clicked() {
                    self.reset_color();
                    self.s.color_mode = ColorMode::BlackWhite;
                    self.s.threshold = 170;
                    self.s.contrast = 20.0;
                }
                if ui.small_button("Photo").clicked() {
                    self.reset_color();
                    self.s.saturation = 115.0;
                    self.s.contrast = 8.0;
                    self.s.gamma = 1.05;
                }
                if ui.small_button("Eco draft").clicked() {
                    self.reset_color();
                    self.s.color_mode = ColorMode::Grayscale;
                    self.s.toner_saver = 45.0;
                }
                if ui.small_button("Dark-mode PDF").clicked() {
                    self.reset_color();
                    self.s.invert = true;
                    self.s.color_mode = ColorMode::Grayscale;
                }
                if ui.small_button("Faded scan fix").clicked() {
                    self.reset_color();
                    self.s.contrast = 35.0;
                    self.s.gamma = 0.8;
                    self.s.brightness = 5.0;
                }
            });
            grid(ui, "color_grid", |ui| {
                ui.label("Color mode");
                combo(
                    ui,
                    "color_mode",
                    &mut self.s.color_mode,
                    &[
                        (ColorMode::Color, "Color"),
                        (ColorMode::Grayscale, "Grayscale"),
                        (ColorMode::BlackWhite, "Black & white (threshold)"),
                        (ColorMode::Dither, "Black & white (error diffusion)"),
                        (ColorMode::Halftone, "Black & white (halftone pattern)"),
                    ],
                );
                ui.end_row();
                if self.s.color_mode == ColorMode::BlackWhite {
                    ui.label("Threshold");
                    ui.add(egui::Slider::new(&mut self.s.threshold, 1..=254));
                    ui.end_row();
                }
                ui.label("Brightness");
                ui.add(egui::Slider::new(&mut self.s.brightness, -100.0..=100.0));
                ui.end_row();
                ui.label("Contrast");
                ui.add(egui::Slider::new(&mut self.s.contrast, -100.0..=100.0));
                ui.end_row();
                ui.label("Gamma");
                ui.add(egui::Slider::new(&mut self.s.gamma, 0.2..=3.0).max_decimals(2));
                ui.end_row();
                ui.label("Saturation");
                ui.add(egui::Slider::new(&mut self.s.saturation, 0.0..=200.0).suffix(" %"));
                ui.end_row();
                ui.label("Toner saver");
                ui.add(egui::Slider::new(&mut self.s.toner_saver, 0.0..=80.0).suffix(" %"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.s.invert, "Invert colors (negative)");
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
        section(ui, "✨", "Rendering & anti-aliasing", false, |ui| {
            grid(ui, "quality_grid", |ui| {
                ui.label("Raster resolution");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.s.dpi_mode, DpiMode::Printer, "Printer native");
                    ui.selectable_value(&mut self.s.dpi_mode, DpiMode::Custom, "Custom");
                });
                ui.end_row();
                match self.s.dpi_mode {
                    DpiMode::Printer => {
                        ui.label("Max DPI");
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
                    hint(ui, &format!("printer reports {} × {} dpi", g.dpi_x, g.dpi_y));
                    ui.end_row();
                }
                ui.label("Anti-aliasing");
                combo(
                    ui,
                    "ss",
                    &mut self.s.supersample,
                    &[(1, "Standard (1×)"), (2, "Supersample 2× (4 samples)"), (3, "Supersample 3× (9 samples)"), (4, "Supersample 4× (16 samples)")],
                );
                ui.end_row();
                ui.label("Resampling filter");
                combo(
                    ui,
                    "filter",
                    &mut self.s.filter,
                    &[
                        (Filter::Nearest, "Nearest (pixel-sharp)"),
                        (Filter::Triangle, "Bilinear (soft)"),
                        (Filter::CatmullRom, "Bicubic Catmull-Rom"),
                        (Filter::Lanczos3, "Lanczos-3 (sharpest)"),
                    ],
                );
                ui.end_row();
                ui.label("Sharpen");
                ui.add(egui::Slider::new(&mut self.s.sharpen, 0.0..=3.0).max_decimals(1).custom_formatter(|v, _| if v == 0.0 { "off".into() } else { format!("{v:.1}") }));
                ui.end_row();
                ui.label("Spool format");
                combo(
                    ui,
                    "spool",
                    &mut self.s.spool,
                    &[
                        (SpoolFormat::Auto, "Auto (smallest for color mode)"),
                        (SpoolFormat::Rgb24, "24-bit color"),
                        (SpoolFormat::Gray8, "8-bit gray"),
                        (SpoolFormat::Mono1, "1-bit monochrome"),
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
                hint(ui, &format!("≈ {:.0} × {:.0} px, {:.1} MB per side before driver compression",
                    self.sheet_mm.0 / 25.4 * dpi, self.sheet_mm.1 / 25.4 * dpi, px * bpp / 1_048_576.0));
                ui.end_row();
                ui.label("PNG export DPI");
                ui.add(egui::DragValue::new(&mut self.s.export_dpi).range(36..=1200).suffix(" dpi"));
                ui.end_row();
            });
        });
    }

    fn ui_overlays(&mut self, ui: &mut Ui) {
        let vars = self.preview_vars();
        let mut help = None;
        section(ui, "🏷", "Watermark, headers & stamps", false, |ui| {
            ui.horizontal(|ui| {
                if ui.button("❓ Template help").on_hover_text("Variables, conditions, loops and examples (F1)").clicked() {
                    help = Some(manual::CH_TEMPLATES);
                }
                if ui.button("📚 Cookbook").clicked() {
                    help = Some(manual::CH_COOKBOOK);
                }
                if ui.button("🧪 Playground").on_hover_text("Try templates against the side in the preview").clicked() {
                    help = Some(manual::PLAYGROUND);
                }
            });
            grid(ui, "font_grid", |ui| {
                ui.label("Font");
                font_pick(ui, "font", &mut self.s.font, &self.fonts, false);
                ui.end_row();
            });

            // ---- watermark
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.s.watermark.enabled, RichText::new("Watermark").strong());
                if help_button(ui) {
                    help = Some(manual::CH_WATERMARK);
                }
            });
            if self.s.watermark.enabled {
                let wm = &mut self.s.watermark;
                grid(ui, "wm_grid", |ui| {
                    ui.label("Text");
                    tpl_edit(ui, &mut wm.text, &vars);
                    ui.end_row();
                    ui.label("Quick");
                    ui.horizontal_wrapped(|ui| {
                        for t in ["CONFIDENTIAL", "DRAFT", "COPY", "SAMPLE", "PAID"] {
                            if ui.small_button(t).clicked() {
                                wm.text = t.into();
                            }
                        }
                        if ui.small_button("ORIGINAL / COPY n").on_hover_text("First copy ORIGINAL, then COPY 2, COPY 3…").clicked() {
                            wm.text = "{if copy = 1}ORIGINAL{else}COPY {copy}{end}".into();
                        }
                        if ui.small_button("User + date").on_hover_text("Leak-tracing text; tile it").clicked() {
                            wm.text = "{user}{br}{datetime}".into();
                        }
                    });
                    ui.end_row();
                    ui.label("Image (PNG)");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut wm.image).hint_text("none – use the text").desired_width(150.0));
                        if ui.small_button("Browse…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).pick_file() {
                                wm.image = p.display().to_string();
                            }
                        }
                        if !wm.image.is_empty() && ui.small_button("✕").on_hover_text("Use the text again").clicked() {
                            wm.image.clear();
                        }
                    });
                    ui.end_row();
                    if wm.image.trim().is_empty() {
                        ui.label("Text font");
                        font_pick(ui, "wm_font", &mut wm.font, &self.fonts, true);
                        ui.end_row();
                        ui.label("Size");
                        ui.add(egui::Slider::new(&mut wm.size_pt, 8.0..=300.0).suffix(" pt"));
                        ui.end_row();
                        ui.label("Color");
                        ui.color_edit_button_srgb(&mut wm.color);
                        ui.end_row();
                    } else {
                        ui.label("Image width");
                        ui.add(egui::Slider::new(&mut wm.image_width_mm, 5.0..=400.0).suffix(" mm"));
                        ui.end_row();
                    }
                    ui.label("Angle");
                    ui.add(egui::Slider::new(&mut wm.angle, -90.0..=90.0).suffix("°"));
                    ui.end_row();
                    ui.label("Opacity");
                    ui.add(egui::Slider::new(&mut wm.opacity, 1.0..=100.0).suffix(" %"));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.tile, "Tile across the whole area");
                    ui.end_row();
                    if wm.tile {
                        ui.label("Gap (x, y)");
                        ui.horizontal(|ui| {
                            mm(ui, &mut wm.tile_gap_mm[0], 300.0);
                            mm(ui, &mut wm.tile_gap_mm[1], 300.0);
                        });
                        ui.end_row();
                    } else {
                        ui.label("Position");
                        pos_pick(ui, &mut wm.position);
                        ui.end_row();
                    }
                    ui.label("Offset (x, y)");
                    ui.horizontal(|ui| {
                        mm(ui, &mut wm.offset_mm[0], 500.0);
                        mm(ui, &mut wm.offset_mm[1], 500.0);
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.behind, "Behind content (only on paper)");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut wm.per_page, "One per page (n-up)");
                    ui.end_row();
                    if wm.image.trim().is_empty() {
                        ui.label("");
                        ui.checkbox(&mut wm.boxed, "Frame (rubber stamp)");
                        ui.end_row();
                    }
                    ui.label("Show when");
                    cond_edit(ui, "wm_cond", &mut wm.show_if, &vars);
                    ui.end_row();
                });
            }

            // ---- header / footer
            ui.add_space(4.0);
            for (title, band, id) in [("Header", &mut self.s.header, "hdr"), ("Footer", &mut self.s.footer, "ftr")] {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut band.enabled, RichText::new(title).strong());
                    if band.enabled {
                        ui.menu_button("✨ Presets", |ui| {
                            for (name, l, c, r) in BAND_PRESETS {
                                if ui.button(*name).clicked() {
                                    band.left = l.to_string();
                                    band.center = c.to_string();
                                    band.right = r.to_string();
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
                        for (label, txt) in [("Left", &mut band.left), ("Center", &mut band.center), ("Right", &mut band.right)] {
                            ui.label(label);
                            tpl_edit(ui, txt, &vars);
                            ui.end_row();
                        }
                        ui.label("Size / color");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut band.size_pt).range(4.0..=48.0).suffix(" pt"));
                            ui.color_edit_button_srgb(&mut band.color);
                        });
                        ui.end_row();
                        ui.label("Font");
                        font_pick(ui, &format!("{id}_font"), &mut band.font, &self.fonts, true);
                        ui.end_row();
                        ui.label("Distance");
                        ui.add(egui::DragValue::new(&mut band.distance_mm).range(0.0..=100.0).speed(0.25).suffix(" mm").max_decimals(1))
                            .on_hover_text("From the paper edge (or the page edge with \"each page\"); 0 = automatic");
                        ui.end_row();
                        ui.label("Place on");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut band.scope, StampScope::Sheet, "Sheet");
                            ui.selectable_value(&mut band.scope, StampScope::EachPage, "Each page");
                        });
                        ui.end_row();
                        ui.label("");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut band.mirror_even, "Mirror on even sides")
                                .on_hover_text("Swap left and right on even sides so page numbers sit on the outside edge");
                            ui.checkbox(&mut band.rule, "Separator line");
                        });
                        ui.end_row();
                        ui.label("Show when");
                        cond_edit(ui, &format!("{id}_cond"), &mut band.show_if, &vars);
                        ui.end_row();
                    });
                }
            }

            // ---- stamps
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Stamps ({})", self.s.stamps.len())).strong());
                ui.menu_button("➕ Add stamp", |ui| {
                    for (name, text, pos, boxed, angle, color) in STAMP_PRESETS {
                        if ui.button(*name).clicked() {
                            self.s.stamps.push(Stamp {
                                text: text.to_string(),
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
                        if ui.small_button("⧉").on_hover_text("Duplicate").clicked() {
                            duplicate = Some(i);
                        }
                        if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                    tpl_status(ui, &st.text, &vars);
                    egui::CollapsingHeader::new("Options").id_salt(("stamp", i)).show(ui, |ui| {
                        grid(ui, &format!("stamp_grid_{i}"), |ui| {
                            ui.label("Position");
                            pos_pick(ui, &mut st.position);
                            ui.end_row();
                            ui.label("Offset (x, y)");
                            ui.horizontal(|ui| {
                                mm(ui, &mut st.offset_mm[0], 500.0);
                                mm(ui, &mut st.offset_mm[1], 500.0);
                            });
                            ui.end_row();
                            ui.label("Size / color");
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut st.size_pt).range(4.0..=300.0).suffix(" pt"));
                                ui.color_edit_button_srgb(&mut st.color);
                            });
                            ui.end_row();
                            ui.label("Font");
                            font_pick(ui, &format!("stamp_font_{i}"), &mut st.font, &self.fonts, true);
                            ui.end_row();
                            ui.label("Angle");
                            ui.add(egui::Slider::new(&mut st.angle, -90.0..=90.0).suffix("°"));
                            ui.end_row();
                            ui.label("Opacity");
                            ui.add(egui::Slider::new(&mut st.opacity, 1.0..=100.0).suffix(" %"));
                            ui.end_row();
                            ui.label("Place on");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut st.scope, StampScope::Sheet, "Sheet");
                                ui.selectable_value(&mut st.scope, StampScope::EachPage, "Each page");
                            });
                            ui.end_row();
                            ui.label("");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut st.boxed, "Frame");
                                ui.checkbox(&mut st.behind, "Behind content");
                            });
                            ui.end_row();
                            ui.label("Show when");
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
                hint(ui, "Free-placed texts: Bates numbers, COPY, approval boxes, corner page numbers…");
            }
            hint(ui, "Templates: {page} {pages} {range} {sheet} {file} {date} {user}…, {if odd}…{else}…{end}, {page|roman}. ❓ for everything.");

            // ---- marks
            ui.add_space(4.0);
            ui.label(RichText::new("Marks").strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.s.crop_marks, "Crop marks");
                ui.checkbox(&mut self.s.cell_border, "Page borders");
                ui.checkbox(&mut self.s.page_labels, "Page numbers");
            });
            if self.s.cell_border {
                ui.horizontal(|ui| {
                    ui.label("Border width");
                    ui.add(egui::DragValue::new(&mut self.s.border_width_mm).range(0.05..=5.0).speed(0.05).suffix(" mm"));
                });
            }
        });
        if let Some(ch) = help {
            self.manual.open_at(ch);
        }
    }

    fn ui_presets(&mut self, ui: &mut Ui) {
        section(ui, "💾", "Presets", false, |ui| {
            let mut load = None;
            ui.horizontal_wrapped(|ui| {
                if self.presets.is_empty() {
                    hint(ui, "No saved presets yet.");
                }
                for p in &self.presets {
                    if ui.button(p).on_hover_text("Load preset (printer stays as is)").clicked() {
                        load = Some(p.clone());
                    }
                }
            });
            if let Some(name) = load {
                if let Some(mut ps) = load_preset(&name) {
                    ps.printer = self.s.printer.clone();
                    self.s = ps;
                    self.preset_name = name.clone();
                    self.notify(format!("Loaded preset '{name}'"), false);
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("preset name").desired_width(150.0));
                if ui.button("Save").clicked() {
                    match save_preset(&self.preset_name, &self.s) {
                        Ok(()) => self.notify(format!("Saved preset '{}'", self.preset_name), false),
                        Err(e) => self.notify(e, true),
                    }
                    self.presets = list_presets();
                }
                if ui.button("Delete").clicked() {
                    delete_preset(&self.preset_name);
                    self.presets = list_presets();
                }
                if ui.button("Reset all").on_hover_text("Restore every setting to its default").clicked() {
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
        ui.label(RichText::new(format!(
            "{pages} page(s) → {sides} side(s){} → {physical} sheet(s) of paper",
            if copies > 1 { format!(" × {copies} copies") } else { String::new() }
        )).strong());
        if let Some(c) = self.coverage {
            hint(ui, &format!("Current side ≈ {c:.1}% coverage"));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can = self.job.is_none() && pages > 0 && !self.s.printer.is_empty();
            let btn = egui::Button::new(RichText::new("🖨  Print").size(17.0).strong().color(Color32::WHITE))
                .fill(ACCENT)
                .min_size(vec2(ui.available_width() - 130.0, 38.0));
            if ui.add_enabled(can, btn).on_hover_text("Ctrl+P").clicked() {
                self.start_job(None);
            }
            if ui
                .add_enabled(self.job.is_none() && pages > 0, egui::Button::new("🖼 Export PNG…").min_size(vec2(120.0, 38.0)))
                .on_hover_text("Render the imposed sheets to PNG files")
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
            egui::Window::new("🔒 Password required")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("This document is encrypted. Enter its password:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(260.0));
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        submit = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            submit = true;
                        }
                        if ui.button("Cancel").clicked() {
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
            egui::Window::new(if j.export { "Exporting" } else { "Printing" })
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    if let Some(n) = j.need_flip {
                        ui.label(RichText::new("Front sides are printed.").strong());
                        ui.label(format!(
                            "Take the stack from the output tray, flip it (keep the same edge leading for {} binding) and put it back in the input tray. {n} back side(s) will be printed.",
                            if self.s.binding_edge == BindingEdge::Top { "top" } else { "side" }
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("▶ Continue with back sides").clicked() {
                                flip_answer = Some(true);
                            }
                            if ui.button("Stop").clicked() {
                                flip_answer = Some(false);
                            }
                        });
                    } else {
                        let frac = if j.total > 0 { j.done as f32 / j.total as f32 } else { 0.0 };
                        ui.label(&j.label);
                        ui.add(egui::ProgressBar::new(frac).show_percentage().animate(true));
                        if ui.button("Cancel").clicked() {
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
