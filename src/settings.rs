use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum RangeMode {
    All,
    Current,
    Custom,
    Picked,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Subset {
    All,
    Odd,
    Even,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum LayoutMode {
    Normal,
    Booklet,
    Poster,
    Repeat,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum NupOrder {
    RowsLtr,
    RowsRtl,
    ColsTtb,
    ColsTtbRtl,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Scaling {
    Fit,
    ShrinkOnly,
    Actual,
    Custom,
    Fill,
    Stretch,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Duplex {
    Off,
    LongEdge,
    ShortEdge,
    Manual,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ColorMode {
    Color,
    Grayscale,
    BlackWhite,
    Dither,
    Halftone,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Filter {
    Nearest,
    Triangle,
    CatmullRom,
    Lanczos3,
}

impl Filter {
    pub fn to_image(self) -> image::imageops::FilterType {
        use image::imageops::FilterType as F;
        match self {
            Filter::Nearest => F::Nearest,
            Filter::Triangle => F::Triangle,
            Filter::CatmullRom => F::CatmullRom,
            Filter::Lanczos3 => F::Lanczos3,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum SpoolFormat {
    Auto,
    Rgb24,
    Gray8,
    Mono1,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum BindingEdge {
    Left,
    Right,
    Top,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum DpiMode {
    Printer,
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Quality {
    Default,
    Draft,
    Low,
    Medium,
    High,
    Res(i32, i32),
}

/// What an overlay is placed on: the whole sheet side, or every page on it (n-up, booklet).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum StampScope {
    Sheet,
    EachPage,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct TextBand {
    pub enabled: bool,
    /// Templates, see `template.rs`.
    pub left: String,
    pub center: String,
    pub right: String,
    pub size_pt: f32,
    pub color: [u8; 3],
    /// Font file; empty uses the overlay font.
    pub font: String,
    /// Condition expression; empty means always.
    pub show_if: String,
    /// Swap left and right on even sides (book style).
    pub mirror_even: bool,
    /// Distance of the text line from the paper edge; 0 = half the margin.
    pub distance_mm: f32,
    /// Thin line between the band and the content.
    pub rule: bool,
    pub scope: StampScope,
}

impl Default for TextBand {
    fn default() -> Self {
        Self {
            enabled: false,
            left: String::new(),
            center: String::new(),
            right: String::new(),
            size_pt: 8.0,
            color: [60, 60, 60],
            font: String::new(),
            show_if: String::new(),
            mirror_even: false,
            distance_mm: 0.0,
            rule: false,
            scope: StampScope::Sheet,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Watermark {
    pub enabled: bool,
    pub text: String,
    pub size_pt: f32,
    pub angle: f32,
    pub opacity: f32,
    pub color: [u8; 3],
    pub behind: bool,
    pub per_page: bool,
    pub font: String,
    pub show_if: String,
    /// 9-point anchor, 0 = top-left … 4 = centre … 8 = bottom-right.
    pub position: u8,
    pub offset_mm: [f32; 2],
    /// Repeat the watermark across the whole area.
    pub tile: bool,
    pub tile_gap_mm: [f32; 2],
    /// Rubber-stamp frame around the text.
    pub boxed: bool,
    /// PNG file used instead of the text.
    pub image: String,
    pub image_width_mm: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            enabled: false,
            text: "CONFIDENTIAL".into(),
            size_pt: 72.0,
            angle: 45.0,
            opacity: 18.0,
            color: [200, 30, 30],
            behind: true,
            per_page: false,
            font: String::new(),
            show_if: String::new(),
            position: 4,
            offset_mm: [0.0, 0.0],
            tile: false,
            tile_gap_mm: [20.0, 30.0],
            boxed: false,
            image: String::new(),
            image_width_mm: 80.0,
        }
    }
}

/// A free-placed text stamp (Bates numbers, "COPY", approval boxes…).
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Stamp {
    pub enabled: bool,
    pub text: String,
    pub font: String,
    pub size_pt: f32,
    pub color: [u8; 3],
    pub opacity: f32,
    pub angle: f32,
    pub position: u8,
    pub offset_mm: [f32; 2],
    pub scope: StampScope,
    pub show_if: String,
    pub boxed: bool,
    pub behind: bool,
}

impl Default for Stamp {
    fn default() -> Self {
        Self {
            enabled: true,
            text: "{sheet|pad:6}".into(),
            font: String::new(),
            size_pt: 10.0,
            color: [0, 0, 0],
            opacity: 100.0,
            angle: 0.0,
            position: 8,
            offset_mm: [0.0, 0.0],
            scope: StampScope::Sheet,
            show_if: String::new(),
            boxed: false,
            behind: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum CbzSort {
    /// "page2" before "page10".
    Natural,
    Name,
    /// Order the files are stored in.
    Archive,
}

/// Comic book (.cbz) options: how the archive becomes pages.
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct CbzOptions {
    pub sort: CbzSort,
    pub reverse: bool,
    /// Resolution the images are assumed to have, for "actual size".
    pub dpi: f32,
    /// Skip images whose shorter side is below this (credits, thumbnails).
    pub min_px: u32,
    /// Comma-separated name parts to skip.
    pub exclude: String,
    /// Right-to-left reading (manga).
    pub rtl: bool,
    /// Cut landscape double-page spreads into two pages.
    pub split_spreads: bool,
    /// Never split the first page (cover).
    pub keep_cover: bool,
    /// Blank page after the cover so spreads stay together side by side.
    pub cover_alone: bool,
    /// A page counts as a spread when width / height exceeds this.
    pub spread_ratio: f32,
}

impl Default for CbzOptions {
    fn default() -> Self {
        Self {
            sort: CbzSort::Natural,
            reverse: false,
            dpi: 300.0,
            min_px: 0,
            exclude: String::new(),
            rtl: false,
            split_spreads: false,
            keep_cover: true,
            cover_alone: false,
            spread_ratio: 1.1,
        }
    }
}

impl CbzOptions {
    /// Options that change which images are loaded (they need the archive reopened).
    pub fn load_key(&self) -> String {
        format!("{:?}|{}|{}|{}|{}", self.sort, self.reverse, self.dpi, self.min_px, self.exclude)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    // ---- pages
    pub range_mode: RangeMode,
    pub custom_range: String,
    pub subset: Subset,
    pub every_nth: u32,
    pub nth_offset: u32,
    pub exclude: String,
    pub reverse: bool,

    // ---- layout
    pub layout: LayoutMode,
    pub nup_cols: u32,
    pub nup_rows: u32,
    pub nup_order: NupOrder,
    pub cell_gap_mm: f32,
    pub cell_border: bool,
    pub border_width_mm: f32,
    pub page_labels: bool,
    pub booklet_rtl: bool,
    pub booklet_signature: u32,
    pub booklet_align_fold: bool,
    pub poster_x: u32,
    pub poster_y: u32,
    pub poster_overlap_mm: f32,
    pub poster_guides: bool,
    pub margins_mm: [f32; 4], // top, right, bottom, left
    pub respect_hw_margins: bool,
    pub binding_gutter_mm: f32,
    pub binding_edge: BindingEdge,

    // ---- sizing
    pub scaling: Scaling,
    pub custom_scale: f32,
    pub align_h: u8, // 0 left, 1 center, 2 right
    pub align_v: u8, // 0 top, 1 middle, 2 bottom
    pub offset_mm: [f32; 2],
    pub rotation: u32,
    pub auto_rotate: bool,
    pub mirror_h: bool,
    pub mirror_v: bool,
    pub crop_mm: [f32; 4], // top, right, bottom, left
    pub auto_crop: bool,

    // ---- color / image
    pub color_mode: ColorMode,
    pub threshold: u8,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub invert: bool,
    pub toner_saver: f32,

    // ---- rendering quality
    pub dpi_mode: DpiMode,
    pub custom_dpi: u32,
    pub max_dpi: u32,
    pub supersample: u32,
    pub filter: Filter,
    pub sharpen: f32,
    pub spool: SpoolFormat,
    pub export_dpi: u32,

    // ---- overlays
    pub font: String,
    pub watermark: Watermark,
    pub header: TextBand,
    pub footer: TextBand,
    pub stamps: Vec<Stamp>,
    pub crop_marks: bool,

    // ---- printer / output
    pub printer: String,
    pub paper_id: u16, // 0 = driver default, 256 = custom
    pub custom_paper_mm: [f32; 2],
    pub orientation: Orientation,
    pub bin: u16,
    pub media: u32,
    pub quality: Quality,
    pub duplex: Duplex,
    pub manual_duplex_reverse: bool,
    pub copies: u32,
    pub collate: bool,
    pub job_name: String,
    pub print_to_file: bool,
    pub output_file: String,

    // ---- comic books
    pub cbz: CbzOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            range_mode: RangeMode::All,
            custom_range: String::new(),
            subset: Subset::All,
            every_nth: 1,
            nth_offset: 0,
            exclude: String::new(),
            reverse: false,

            layout: LayoutMode::Normal,
            nup_cols: 1,
            nup_rows: 1,
            nup_order: NupOrder::RowsLtr,
            cell_gap_mm: 4.0,
            cell_border: false,
            border_width_mm: 0.3,
            page_labels: false,
            booklet_rtl: false,
            booklet_signature: 0,
            booklet_align_fold: true,
            poster_x: 2,
            poster_y: 2,
            poster_overlap_mm: 10.0,
            poster_guides: true,
            margins_mm: [5.0; 4],
            respect_hw_margins: true,
            binding_gutter_mm: 0.0,
            binding_edge: BindingEdge::Left,

            scaling: Scaling::Fit,
            custom_scale: 100.0,
            align_h: 1,
            align_v: 1,
            offset_mm: [0.0, 0.0],
            rotation: 0,
            auto_rotate: true,
            mirror_h: false,
            mirror_v: false,
            crop_mm: [0.0; 4],
            auto_crop: false,

            color_mode: ColorMode::Color,
            threshold: 160,
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            saturation: 100.0,
            invert: false,
            toner_saver: 0.0,

            dpi_mode: DpiMode::Printer,
            custom_dpi: 300,
            max_dpi: 600,
            supersample: 2,
            filter: Filter::CatmullRom,
            sharpen: 0.0,
            spool: SpoolFormat::Auto,
            export_dpi: 200,

            font: "arial.ttf".into(),
            watermark: Watermark::default(),
            header: TextBand {
                left: "{file}".into(),
                right: "{date} {time}".into(),
                ..Default::default()
            },
            footer: TextBand {
                center: "Page {range} of {pages}".into(),
                ..Default::default()
            },
            stamps: Vec::new(),
            crop_marks: false,

            printer: String::new(),
            paper_id: 0,
            custom_paper_mm: [210.0, 297.0],
            orientation: Orientation::Auto,
            bin: 0,
            media: 0,
            quality: Quality::Default,
            duplex: Duplex::Off,
            manual_duplex_reverse: false,
            copies: 1,
            collate: true,
            job_name: String::new(),
            print_to_file: false,
            output_file: String::new(),

            cbz: CbzOptions::default(),
        }
    }
}

impl Settings {
    pub fn is_duplex(&self) -> bool {
        self.duplex != Duplex::Off
    }
}

// ---------------------------------------------------------------- presets

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("PdfPrintStudio")
}

fn presets_dir() -> PathBuf {
    data_dir().join("presets")
}

pub fn list_presets() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(presets_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension()? != "json" {
                        return None;
                    }
                    Some(p.file_stem()?.to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' })
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn save_preset(name: &str, s: &Settings) -> Result<(), String> {
    let name = sanitize(name);
    if name.is_empty() {
        return Err(crate::i18n::tr("Preset name is empty").into());
    }
    std::fs::create_dir_all(presets_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(presets_dir().join(format!("{name}.json")), json).map_err(|e| e.to_string())
}

pub fn load_preset(name: &str) -> Option<Settings> {
    let data = std::fs::read_to_string(presets_dir().join(format!("{}.json", sanitize(name)))).ok()?;
    serde_json::from_str(data.trim_start_matches('\u{feff}')).ok()
}

pub fn delete_preset(name: &str) {
    let _ = std::fs::remove_file(presets_dir().join(format!("{}.json", sanitize(name))));
}

pub fn save_last(s: &Settings) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(data_dir().join("last.json"), json);
    }
}

pub fn load_last() -> Option<Settings> {
    let data = std::fs::read_to_string(data_dir().join("last.json")).ok()?;
    serde_json::from_str(data.trim_start_matches('\u{feff}')).ok()
}
