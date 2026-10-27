//! E-books (.epub): the XHTML chapters are read into simple blocks (paragraphs, headings, lists,
//! quotes, images) and typeset into fixed pages with the chosen font, size and margins.

use crate::i18n::tr;
use crate::settings::{EpubOptions, PageNumbers};
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use image::imageops::FilterType;
use image::{ImageReader, Rgba, RgbaImage};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use zip::ZipArchive;

const PT: f32 = 25.4 / 72.0;

// ---------------------------------------------------------------- markup

enum Tok {
    Open { name: String, attrs: Vec<(String, String)>, closed: bool },
    Close(String),
    Text(String),
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            "nbsp" => Some("\u{a0}".into()),
            "shy" => Some(String::new()),
            "mdash" => Some("—".into()),
            "ndash" => Some("–".into()),
            "hellip" => Some("…".into()),
            "lsquo" => Some("‘".into()),
            "rsquo" => Some("’".into()),
            "ldquo" => Some("“".into()),
            "rdquo" => Some("”".into()),
            "laquo" => Some("«".into()),
            "raquo" => Some("»".into()),
            "copy" => Some("©".into()),
            "reg" => Some("®".into()),
            "trade" => Some("™".into()),
            "bull" => Some("•".into()),
            "middot" => Some("·".into()),
            "deg" => Some("°".into()),
            "times" => Some("×".into()),
            "eacute" => Some("é".into()),
            "aacute" => Some("á".into()),
            "ccedil" => Some("ç".into()),
            "atilde" => Some("ã".into()),
            "otilde" => Some("õ".into()),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32)
                .map(String::from),
        };
        match ch {
            Some(c) => {
                out.push_str(&c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A forgiving tokenizer for XHTML / XML: tags, attributes and text.
fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            i += 4 + r.find("-->").map_or(r.len(), |e| e + 3);
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let e = r.find("]]>").unwrap_or(r.len());
            out.push(Tok::Text(r[..e].to_string()));
            i += 9 + (e + 3).min(r.len());
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            i += rest.find('>').map_or(rest.len(), |e| e + 1);
        } else if let Some(r) = rest.strip_prefix("</") {
            let e = r.find('>').unwrap_or(r.len());
            out.push(Tok::Close(local(r[..e].trim())));
            i += 2 + (e + 1).min(r.len());
        } else if rest.starts_with('<') {
            // find the end of the tag, honouring quotes
            let b = rest.as_bytes();
            let (mut j, mut q) = (1, 0u8);
            while j < b.len() {
                match b[j] {
                    b'"' | b'\'' if q == 0 => q = b[j],
                    c if c == q => q = 0,
                    b'>' if q == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let inner = &rest[1..j.min(rest.len())];
            let closed = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
            let name = local(&inner[..name_end]);
            let mut attrs = Vec::new();
            let mut a = inner[name_end..].trim_start();
            while !a.is_empty() {
                let k_end = a.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(a.len());
                let key = a[..k_end].to_ascii_lowercase();
                a = a[k_end..].trim_start();
                let mut val = String::new();
                if let Some(r) = a.strip_prefix('=') {
                    let r = r.trim_start();
                    let (v, rem) = match r.chars().next() {
                        Some(qc @ ('"' | '\'')) => {
                            let e = r[1..].find(qc).map_or(r.len(), |e| e + 1);
                            (&r[1..e], &r[(e + 1).min(r.len())..])
                        }
                        _ => {
                            let e = r.find(char::is_whitespace).unwrap_or(r.len());
                            (&r[..e], &r[e..])
                        }
                    };
                    val = decode_entities(v);
                    a = rem.trim_start();
                }
                if !key.is_empty() {
                    attrs.push((key, val));
                }
            }
            if !name.is_empty() {
                out.push(Tok::Open { name, attrs, closed });
            }
            i += (j + 1).min(rest.len());
        } else {
            let e = rest.find('<').unwrap_or(rest.len());
            out.push(Tok::Text(decode_entities(&rest[..e])));
            i += e;
        }
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key || local(k) == key).map(|(_, v)| v.as_str())
}

/// Resolve `href` (relative, maybe %-encoded, maybe with #fragment) against the folder of `base`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let mut decoded = Vec::new();
    let b = href.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&href[i + 1..i + 3], 16) {
                decoded.push(v);
                i += 3;
                continue;
            }
        }
        decoded.push(b[i]);
        i += 1;
    }
    let href = String::from_utf8_lossy(&decoded).into_owned();
    let mut parts: Vec<&str> = if href.starts_with('/') { Vec::new() } else { base.split('/').collect() };
    parts.pop(); // file name of base
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

// ---------------------------------------------------------------- content model

#[derive(Clone, Copy, PartialEq, Debug)]
enum Style {
    Regular = 0,
    Bold = 1,
    Italic = 2,
    BoldItalic = 3,
}

impl Style {
    fn of(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => Style::Regular,
            (true, false) => Style::Bold,
            (false, true) => Style::Italic,
            (true, true) => Style::BoldItalic,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Kind {
    Para,
    Heading(u8),
    Pre,
    Quote,
    Item(String),
}

#[derive(Clone, Debug)]
enum Block {
    Text(Kind, Vec<(String, Style)>),
    Image(String),
    Rule,
}

const BLOCK_TAGS: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "blockquote", "pre", "tr", "dt", "dd", "figcaption", "caption",
    "section", "article", "header", "footer", "aside", "ul", "ol", "dl", "table", "body", "figure",
];

/// Turn one XHTML chapter into blocks; images are archive paths.
fn parse_chapter(src: &str, doc_path: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut runs: Vec<(String, Style)> = Vec::new();
    let mut kind = Kind::Para;
    let (mut bold, mut italic, mut skip, mut pre, mut quote) = (0, 0, 0, 0, 0);
    let mut lists: Vec<Option<u32>> = Vec::new();

    fn flush(blocks: &mut Vec<Block>, runs: &mut Vec<(String, Style)>, kind: &Kind, pre: bool) {
        let text: String = runs.iter().map(|r| r.0.as_str()).collect();
        if text.trim().is_empty() {
            runs.clear();
            return;
        }
        if !pre {
            if let Some(first) = runs.first_mut() {
                first.0 = first.0.trim_start().to_string();
            }
            if let Some(last) = runs.last_mut() {
                last.0 = last.0.trim_end().to_string();
            }
        }
        blocks.push(Block::Text(kind.clone(), std::mem::take(runs)));
    }

    for t in tokenize(src) {
        match t {
            Tok::Open { name, attrs, closed } => {
                match name.as_str() {
                    "head" | "script" | "style" | "title" if !closed => skip += 1,
                    "b" | "strong" | "th" if !closed => bold += 1,
                    "i" | "em" | "cite" | "var" | "dfn" if !closed => italic += 1,
                    "br" => runs.push(("\n".into(), Style::of(bold > 0, italic > 0))),
                    "hr" => {
                        flush(&mut blocks, &mut runs, &kind, pre > 0);
                        blocks.push(Block::Rule);
                    }
                    "img" | "image" => {
                        let href = attr(&attrs, "src").or_else(|| attr(&attrs, "href"));
                        if let Some(h) = href {
                            flush(&mut blocks, &mut runs, &kind, pre > 0);
                            blocks.push(Block::Image(resolve(doc_path, h)));
                        }
                    }
                    n if BLOCK_TAGS.contains(&n) => {
                        flush(&mut blocks, &mut runs, &kind, pre > 0);
                        kind = if quote > 0 { Kind::Quote } else { Kind::Para };
                        match n {
                            "ul" if !closed => lists.push(None),
                            "ol" if !closed => lists.push(Some(attr(&attrs, "start").and_then(|v| v.parse().ok()).unwrap_or(1))),
                            "blockquote" if !closed => {
                                quote += 1;
                                kind = Kind::Quote;
                            }
                            "pre" if !closed => {
                                pre += 1;
                                kind = Kind::Pre;
                            }
                            "li" => {
                                let marker = match lists.last_mut() {
                                    Some(Some(n)) => {
                                        *n += 1;
                                        format!("{}.", *n - 1)
                                    }
                                    _ => "•".into(),
                                };
                                kind = Kind::Item(marker);
                            }
                            h if h.len() == 2 && h.starts_with('h') => kind = Kind::Heading(h.as_bytes()[1] - b'0'),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Tok::Close(name) => match name.as_str() {
                "head" | "script" | "style" | "title" => skip = (skip - 1).max(0),
                "b" | "strong" | "th" => bold = (bold - 1).max(0),
                "i" | "em" | "cite" | "var" | "dfn" => italic = (italic - 1).max(0),
                n if BLOCK_TAGS.contains(&n) => {
                    flush(&mut blocks, &mut runs, &kind, pre > 0);
                    match n {
                        "ul" | "ol" => {
                            lists.pop();
                        }
                        "blockquote" => quote = (quote - 1).max(0),
                        "pre" => pre = (pre - 1).max(0),
                        _ => {}
                    }
                    kind = if quote > 0 { Kind::Quote } else { Kind::Para };
                }
                _ => {}
            },
            Tok::Text(text) => {
                if skip > 0 {
                    continue;
                }
                let text = if pre > 0 {
                    text
                } else {
                    let mut s = String::with_capacity(text.len());
                    let mut space = false;
                    for c in text.chars() {
                        if c.is_whitespace() && c != '\u{a0}' {
                            space = true;
                        } else {
                            if space {
                                s.push(' ');
                            }
                            space = false;
                            s.push(c);
                        }
                    }
                    if space {
                        s.push(' ');
                    }
                    s
                };
                let style = Style::of(bold > 0, italic > 0);
                match runs.last_mut() {
                    Some((t, st)) if *st == style => t.push_str(&text),
                    _ => runs.push((text, style)),
                }
            }
        }
    }
    flush(&mut blocks, &mut runs, &kind, pre > 0);
    blocks
}

struct Chapter {
    title: String,
    blocks: Vec<Block>,
}

struct Book {
    meta: Vec<(String, String)>,
    cover: Option<String>,
    chapters: Vec<Chapter>,
}

/// The headings a chapter opens with, e.g. "Chapter 1 · The Valley".
fn chapter_title(blocks: &[Block]) -> String {
    let parts: Vec<String> = blocks
        .iter()
        .map_while(|b| match b {
            Block::Text(Kind::Heading(l), runs) if *l <= 3 => {
                Some(runs.iter().map(|r| r.0.as_str()).collect::<String>().replace('\n', " ").trim().to_string())
            }
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .take(2)
        .collect();
    parts.join(" · ")
}

fn read_entry(zip: &mut ZipArchive<File>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok().or_else(|| None)?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

fn read_book(zip: &mut ZipArchive<File>) -> Result<Book, String> {
    let container = read_entry(zip, "META-INF/container.xml").ok_or("not an EPUB (META-INF/container.xml missing)")?;
    let container = String::from_utf8_lossy(&container);
    let opf_path = tokenize(&container)
        .into_iter()
        .find_map(|t| match t {
            Tok::Open { name, attrs, .. } if name == "rootfile" => attr(&attrs, "full-path").map(str::to_string),
            _ => None,
        })
        .ok_or("EPUB has no package document")?;
    let opf = String::from_utf8_lossy(&read_entry(zip, &opf_path).ok_or("EPUB package document missing")?).into_owned();

    let mut meta: Vec<(String, String)> = Vec::new();
    let mut items: HashMap<String, (String, String, String)> = HashMap::new(); // id → (href, media type, properties)
    let mut spine: Vec<String> = Vec::new();
    let mut cover_id = None;
    let mut current: Option<String> = None;
    for t in tokenize(&opf) {
        match t {
            Tok::Open { name, attrs, closed } => match name.as_str() {
                "title" | "creator" | "publisher" | "language" | "date" | "description" | "subject" if !closed => {
                    current = Some(name)
                }
                "item" => {
                    let get = |k| attr(&attrs, k).unwrap_or("").to_string();
                    items.insert(get("id"), (resolve(&opf_path, &get("href")), get("media-type"), get("properties")));
                }
                "itemref" if attr(&attrs, "linear") != Some("no") => spine.extend(attr(&attrs, "idref").map(str::to_string)),
                "meta" if attr(&attrs, "name") == Some("cover") => cover_id = attr(&attrs, "content").map(str::to_string),
                _ => {}
            },
            Tok::Close(_) => current = None,
            Tok::Text(text) => {
                if let Some(c) = &current {
                    let text = text.trim();
                    if !text.is_empty() {
                        let key = match c.as_str() {
                            "title" => "Title",
                            "creator" => "Author",
                            "publisher" => "Publisher",
                            "language" => "Language",
                            "date" => "Date",
                            "subject" => "Subject",
                            _ => "Description",
                        };
                        // HTML in descriptions is common; keep only its text
                        let text: String = tokenize(text).into_iter().filter_map(|t| if let Tok::Text(s) = t { Some(s) } else { None }).collect();
                        match meta.iter_mut().find(|(k, _)| k == key) {
                            Some((_, v)) if key == "Author" || key == "Subject" => {
                                v.push_str(", ");
                                v.push_str(&text);
                            }
                            Some(_) => {}
                            None => meta.push((key.to_string(), text)),
                        }
                    }
                }
            }
        }
    }
    let cover = items
        .values()
        .find(|(_, _, props)| props.split_whitespace().any(|p| p == "cover-image"))
        .map(|i| i.0.clone())
        .or_else(|| cover_id.and_then(|id| items.get(&id)).filter(|i| i.1.starts_with("image/")).map(|i| i.0.clone()));

    let mut chapters = Vec::new();
    for id in spine {
        let Some((href, media, _)) = items.get(&id) else { continue };
        if !media.contains("html") && !media.is_empty() {
            continue;
        }
        let Some(data) = read_entry(zip, href) else { continue };
        let blocks = parse_chapter(&String::from_utf8_lossy(&data), href);
        if blocks.is_empty() {
            continue;
        }
        chapters.push(Chapter { title: chapter_title(&blocks), blocks });
    }
    if chapters.is_empty() && cover.is_none() {
        return Err("EPUB has no readable chapters".into());
    }
    Ok(Book { meta, cover, chapters })
}

// ---------------------------------------------------------------- fonts

/// Regular, bold, italic and bold-italic files for a Windows font (falling back to the regular one).
fn load_family(regular: &str) -> [Arc<FontVec>; 4] {
    let dir = crate::text::fonts_dir();
    let load = |n: &str| std::fs::read(dir.join(n)).ok().and_then(|d| FontVec::try_from_vec(d).ok()).map(Arc::new);
    let base = load(regular).or_else(|| load("georgia.ttf")).or_else(|| load("arial.ttf")).or_else(|| load("segoeui.ttf"));
    let base = base.expect("no usable font in the Windows fonts folder");
    let lower = regular.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.').unwrap_or((&lower, "ttf"));
    let find = |suffixes: &[&str]| suffixes.iter().find_map(|s| load(&format!("{stem}{s}.{ext}")));
    let bold = find(&["bd", "b", "-bold", "bold"]).unwrap_or_else(|| base.clone());
    let italic = find(&["i", "it", "-italic", "italic"]).unwrap_or_else(|| base.clone());
    let bold_italic = find(&["bi", "z", "-bolditalic", "bolditalic"]).unwrap_or_else(|| bold.clone());
    [base, bold, italic, bold_italic]
}

/// Scale so that the em square is `em` units tall.
fn em_scale(font: &FontVec, em: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(em * font.height_unscaled() / upem)
}

fn text_width(font: &FontVec, em: f32, text: &str) -> f32 {
    let sf = font.as_scaled(em_scale(font, em));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

// ---------------------------------------------------------------- typesetting

#[derive(Clone, Debug)]
enum Op {
    /// A word; `y` is the baseline, everything in mm inside the text area.
    Word { x: f32, y: f32, em: f32, style: Style, text: String },
    Image { x: f32, y: f32, w: f32, h: f32, href: String },
    Rule { x0: f32, x1: f32, y: f32 },
}

#[derive(Default)]
struct Page {
    ops: Vec<Op>,
    chapter: String,
    /// First page of a chapter, cover, contents or blank: no running head.
    plain: bool,
    /// Cover and blank pages carry no page number.
    unnumbered: bool,
    /// Full-page picture (the cover), placed over the whole page.
    full_image: Option<String>,
}

struct Typesetter<'a> {
    o: &'a EpubOptions,
    fonts: &'a [Arc<FontVec>; 4],
    zip: &'a mut ZipArchive<File>,
    image_sizes: HashMap<String, Option<(u32, u32)>>,
    w: f32,
    h: f32,
    pages: Vec<Page>,
    cur: Page,
    y: f32,
}

impl Typesetter<'_> {
    fn new_page(&mut self) {
        let chapter = self.cur.chapter.clone();
        let done = std::mem::take(&mut self.cur);
        self.pages.push(done);
        self.cur.chapter = chapter;
        self.y = 0.0;
    }

    fn page_is_empty(&self) -> bool {
        self.cur.ops.is_empty() && self.cur.full_image.is_none()
    }

    fn finish_page(&mut self) {
        if !self.page_is_empty() {
            self.new_page();
        }
    }

    fn image_size(&mut self, href: &str) -> Option<(u32, u32)> {
        if let Some(s) = self.image_sizes.get(href) {
            return *s;
        }
        let size = read_entry(self.zip, href).and_then(|d| ImageReader::new(Cursor::new(d)).with_guessed_format().ok()?.into_dimensions().ok());
        self.image_sizes.insert(href.to_string(), size);
        size
    }

    fn base_em(&self) -> f32 {
        self.o.size_pt.clamp(5.0, 40.0) * PT
    }

    fn line_h(&self, em: f32) -> f32 {
        em * self.o.line_spacing.clamp(0.9, 3.0)
    }

    fn chapter(&mut self, title: &str, first: bool) {
        if self.o.chapter_new_page || first {
            self.finish_page();
            if self.o.chapter_recto && self.pages.len() % 2 == 1 {
                // the next page would be a left-hand one: leave it blank
                self.cur.plain = true;
                self.cur.unnumbered = true;
                self.new_page();
            }
            self.cur.plain = true;
        }
        self.cur.chapter = title.to_string();
    }

    fn rule(&mut self) {
        let em = self.base_em();
        let need = self.line_h(em) * 1.5;
        if self.y + need > self.h {
            self.new_page();
        }
        let y = self.y + need / 2.0;
        self.cur.ops.push(Op::Rule { x0: self.w * 0.35, x1: self.w * 0.65, y });
        self.y += need;
    }

    fn image(&mut self, href: &str) {
        let Some((pw, ph)) = self.image_size(href) else { return };
        // natural size at 150 dpi, shrunk to fit the text area
        let mut w = pw as f32 / 150.0 * 25.4;
        let mut h = ph as f32 / 150.0 * 25.4;
        let k = (self.w / w).min(self.h / h).min(1.0);
        w *= k;
        h *= k;
        if self.y + h > self.h && self.y > 0.0 {
            self.new_page();
        }
        let gap = self.base_em() * 0.5;
        self.cur.ops.push(Op::Image { x: (self.w - w) / 2.0, y: self.y, w, h, href: href.to_string() });
        self.y += h + gap;
    }

    /// Typeset a paragraph-like block.
    fn text(&mut self, kind: &Kind, runs: &[(String, Style)], indent_first: bool) {
        let base = self.base_em();
        let (em, left, right, align_center, justify, first_indent, before, after) = match kind {
            Kind::Heading(l) => {
                let k = [2.0, 1.6, 1.35, 1.15, 1.05, 1.0][(*l as usize).clamp(1, 6) - 1];
                let em = base * (1.0 + (k - 1.0) * self.o.heading_scale.clamp(0.0, 3.0));
                (em, 0.0, 0.0, self.o.center_headings, false, 0.0, if self.y > 0.0 { em * 1.2 } else { 0.0 }, em * 0.6)
            }
            Kind::Pre => (base * 0.9, 0.0, 0.0, false, false, 0.0, base * 0.3, base * 0.3),
            Kind::Quote => (base, 8.0, 8.0, false, self.o.justify, 0.0, base * 0.4, base * 0.4),
            Kind::Item(_) => (base, 6.0, 0.0, false, self.o.justify, 0.0, 0.0, base * 0.15),
            Kind::Para => {
                let indent = if indent_first { self.o.indent_mm.clamp(0.0, 30.0) } else { 0.0 };
                (base, 0.0, 0.0, false, self.o.justify, indent, 0.0, self.o.para_spacing_pt.clamp(0.0, 30.0) * PT)
            }
        };
        let heading = matches!(kind, Kind::Heading(_));
        let line_h = self.line_h(em);
        let avail = (self.w - left - right).max(10.0);

        // words with their style; "\n" forces a line break
        let mut words: Vec<(String, Style, bool)> = Vec::new(); // (text, style, space before)
        let mut space = false;
        for (text, st) in runs {
            let st = if heading && *st == Style::Regular { Style::Bold } else if heading && *st == Style::Italic { Style::BoldItalic } else { *st };
            let mut word = String::new();
            for c in text.chars() {
                if c == '\n' {
                    if !word.is_empty() {
                        words.push((std::mem::take(&mut word), st, space));
                    }
                    words.push(("\n".into(), st, false));
                    space = false;
                } else if c == ' ' || (c == '\t' && *kind != Kind::Pre) {
                    if !word.is_empty() {
                        words.push((std::mem::take(&mut word), st, space));
                    }
                    space = true;
                } else {
                    word.push(c);
                }
            }
            if !word.is_empty() {
                words.push((word, st, space));
                space = false;
            }
        }
        if words.is_empty() {
            return;
        }
        let space_w = text_width(&self.fonts[0], em, " ");
        let widths: Vec<f32> = words.iter().map(|(t, st, _)| if t == "\n" { 0.0 } else { text_width(&self.fonts[*st as usize], em, t) }).collect();

        self.y += before;
        if heading && self.y + line_h * 2.5 > self.h && self.y > 0.0 {
            // keep a heading with the text after it
            self.new_page();
        }
        let mut i = 0;
        let mut first_line = true;
        while i < words.len() {
            // collect one line
            let start_x = if first_line { first_indent } else { 0.0 };
            let mut j = i;
            let mut used = start_x;
            let mut forced = false;
            while j < words.len() {
                if words[j].0 == "\n" {
                    forced = true;
                    break;
                }
                let gap = if j > i && words[j].2 { space_w } else { 0.0 };
                if j > i && used + gap + widths[j] > avail {
                    break;
                }
                used += gap + widths[j];
                j += 1;
            }
            if j == i && !forced {
                j = i + 1; // a word wider than the line
            }
            if self.y + line_h > self.h {
                self.new_page();
            }
            let last = forced || j >= words.len();
            let gaps = (i + 1..j).filter(|&k| words[k].2).count();
            let extra = if justify && !last && gaps > 0 { ((avail - used) / gaps as f32).min(space_w * 3.0) } else { 0.0 };
            let mut x = left + start_x
                + if align_center { ((avail - used) / 2.0).max(0.0) } else { 0.0 };
            if let (true, Kind::Item(marker)) = (first_line, kind) {
                let mw = text_width(&self.fonts[0], em, marker);
                self.cur.ops.push(Op::Word { x: left - 2.0 - mw, y: self.y + em, em, style: Style::Regular, text: marker.clone() });
            }
            for k in i..j {
                if k > i && words[k].2 {
                    x += space_w + extra;
                }
                self.cur.ops.push(Op::Word { x, y: self.y + em, em, style: words[k].1, text: words[k].0.clone() });
                x += widths[k];
            }
            self.y += line_h;
            first_line = false;
            i = if forced { j + 1 } else { j };
        }
        self.y += after;
    }
}

// ---------------------------------------------------------------- document

pub struct EpubDoc {
    zip: RefCell<ZipArchive<File>>,
    opts: EpubOptions,
    fonts: [Arc<FontVec>; 4],
    size_mm: (f32, f32),
    pages: Vec<Page>,
    images: RefCell<VecDeque<(String, Rc<RgbaImage>)>>,
    pub meta: Vec<(String, String)>,
}

impl EpubDoc {
    pub fn open(path: &Path, opts: &EpubOptions) -> Result<Self, String> {
        let mut zip = ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let book = read_book(&mut zip)?;
        let fonts = load_family(&opts.font);
        let size_mm = opts.page.size_mm(opts.custom_mm);
        let m = opts.margins_mm.map(|v| v.clamp(0.0, 80.0));
        let (w, h) = ((size_mm.0 - m[1] - m[3]).max(20.0), (size_mm.1 - m[0] - m[2]).max(20.0));

        let mut ts = Typesetter { o: opts, fonts: &fonts, zip: &mut zip, image_sizes: HashMap::new(), w, h, pages: Vec::new(), cur: Page::default(), y: 0.0 };
        let mut chapters = book.chapters;
        if let Some(cover) = &book.cover {
            // the first chapter is often just the cover picture: it is either the cover page or left out
            if let Some(first) = chapters.first() {
                if first.blocks.len() == 1 && matches!(&first.blocks[0], Block::Image(h) if h == cover) {
                    chapters.remove(0);
                }
            }
            if opts.cover {
                ts.cur.full_image = Some(cover.clone());
                ts.cur.plain = true;
                ts.cur.unnumbered = true;
                ts.new_page();
            }
        }
        let cover_pages = ts.pages.len();
        let mut starts = Vec::new();
        for (n, ch) in chapters.iter().enumerate() {
            ts.chapter(&ch.title, n == 0);
            starts.push((ch.title.clone(), ts.pages.len()));
            let mut indent = false;
            for b in &ch.blocks {
                match b {
                    Block::Text(kind, runs) => {
                        ts.text(kind, runs, indent);
                        indent = *kind == Kind::Para || *kind == Kind::Quote;
                    }
                    Block::Image(href) if opts.images => {
                        ts.image(href);
                        indent = false;
                    }
                    Block::Image(_) => {}
                    Block::Rule => {
                        ts.rule();
                        indent = false;
                    }
                }
            }
        }
        ts.finish_page();
        let mut pages = std::mem::take(&mut ts.pages);

        if opts.toc {
            let entries: Vec<(String, usize)> = starts.into_iter().filter(|(t, _)| !t.is_empty()).collect();
            if !entries.is_empty() {
                // a first pass finds how many pages the contents need; the numbers after it shift by that
                let n = toc_pages(&mut ts, &entries, 0).len();
                let toc = toc_pages(&mut ts, &entries, n);
                pages.splice(cover_pages..cover_pages, toc);
            }
        }
        drop(ts);
        if pages.is_empty() {
            return Err("EPUB has no readable content".into());
        }
        Ok(Self { zip: RefCell::new(zip), opts: opts.clone(), fonts, size_mm, pages, images: RefCell::new(VecDeque::new()), meta: book.meta })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn sizes_mm(&self) -> Vec<(f32, f32)> {
        vec![self.size_mm; self.pages.len()]
    }

    pub fn chapters(&self) -> usize {
        let mut n = 0;
        let mut last = None;
        for p in &self.pages {
            if !p.chapter.is_empty() && last != Some(&p.chapter) {
                n += 1;
                last = Some(&p.chapter);
            }
        }
        n
    }

    fn picture(&self, href: &str) -> Option<Rc<RgbaImage>> {
        if let Some((_, img)) = self.images.borrow().iter().find(|(h, _)| h == href) {
            return Some(img.clone());
        }
        let data = read_entry(&mut self.zip.borrow_mut(), href)?;
        let img = Rc::new(ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?.decode().ok()?.to_rgba8());
        let mut cache = self.images.borrow_mut();
        cache.push_back((href.to_string(), img.clone()));
        while cache.len() > 4 {
            cache.pop_front();
        }
        Some(img)
    }

    /// Render `src` (normalised rect x0,y0,x1,y1) of a page to a `w`x`h` bitmap.
    pub fn render(&self, page: usize, src: [f32; 4], w: u32, h: u32) -> Result<RgbaImage, String> {
        let p = self.pages.get(page).ok_or("page out of range")?;
        let (w, h) = (w.max(1), h.max(1));
        let (pw, ph) = self.size_mm;
        let s = w as f32 / ((src[2] - src[0]).max(1e-3) * pw);
        let (ox, oy) = (src[0] * pw, src[1] * ph);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let m = self.opts.margins_mm.map(|v| v.clamp(0.0, 80.0));
        // right-hand pages (odd numbers) have the binding on the left
        let recto = page % 2 == 0;
        let left = if recto { m[3] } else { m[1] };
        let to_px = |x: f32, y: f32| ((x - ox) * s, (y - oy) * s);

        if let Some(href) = &p.full_image {
            if let Some(pic) = self.picture(href) {
                let k = (pw / pic.width() as f32).min(ph / pic.height() as f32);
                let (iw, ih) = (pic.width() as f32 * k, pic.height() as f32 * k);
                blit(&mut img, &pic, to_px((pw - iw) / 2.0, (ph - ih) / 2.0), iw * s, ih * s);
            }
        }
        for op in &p.ops {
            match op {
                Op::Word { x, y, em, style, text } => {
                    let (px, py) = to_px(left + x, m[0] + y);
                    draw_word(&mut img, &self.fonts[*style as usize], text, em * s, px, py);
                }
                Op::Image { x, y, w: iw, h: ih, href } => {
                    if let Some(pic) = self.picture(href) {
                        blit(&mut img, &pic, to_px(left + x, m[0] + y), iw * s, ih * s);
                    }
                }
                Op::Rule { x0, x1, y } => {
                    let (a, yy) = to_px(left + x0, m[0] + y);
                    let (b, _) = to_px(left + x1, m[0] + y);
                    fill(&mut img, a, yy - 0.2 * s, b, yy + 0.2 * s, 120);
                }
            }
        }

        // running head and page number in the margins
        let small = self.opts.size_pt.clamp(5.0, 40.0) * PT * 0.8;
        let text_w = pw - m[1] - m[3];
        if self.opts.running_head && !p.plain && !p.chapter.is_empty() {
            let font = &self.fonts[Style::Italic as usize];
            let mut title = p.chapter.clone();
            if text_width(font, small, &title) > text_w {
                while title.chars().count() > 1 && text_width(font, small, &format!("{title}…")) > text_w {
                    title.pop();
                }
                title.push('…');
            }
            let tw = text_width(font, small, &title);
            let (px, py) = to_px(left + (text_w - tw) / 2.0, m[0] * 0.55);
            draw_word(&mut img, font, &title, small * s, px, py);
        }
        if !p.unnumbered && self.opts.page_numbers != PageNumbers::Off {
            let label = (page + 1).to_string();
            let font = &self.fonts[0];
            let tw = text_width(font, small, &label);
            let x = match self.opts.page_numbers {
                PageNumbers::Outside if recto => left + text_w - tw,
                PageNumbers::Outside => left,
                _ => left + (text_w - tw) / 2.0,
            };
            let (px, py) = to_px(x, ph - m[2] * 0.45);
            draw_word(&mut img, font, &label, small * s, px, py);
        }
        Ok(img)
    }
}

/// Contents pages for `entries` (title, page index without the contents pages), numbered as if
/// `offset` contents pages come before the chapters.
fn toc_pages(ts: &mut Typesetter, entries: &[(String, usize)], offset: usize) -> Vec<Page> {
    let saved = std::mem::take(&mut ts.pages);
    let saved_cur = std::mem::take(&mut ts.cur);
    ts.y = 0.0;
    ts.cur.plain = true;
    ts.text(&Kind::Heading(1), &[(tr("Contents").to_string(), Style::Bold)], false);
    let em = ts.base_em();
    let line_h = ts.line_h(em);
    for (title, page) in entries {
        if ts.y + line_h > ts.h {
            ts.new_page();
            ts.cur.plain = true;
        }
        let number = (page + offset + 1).to_string();
        let nw = text_width(&ts.fonts[0], em, &number);
        let mut t = title.clone();
        while text_width(&ts.fonts[0], em, &t) > ts.w - nw - 8.0 && t.chars().count() > 4 {
            t.pop();
        }
        let tw = text_width(&ts.fonts[0], em, &t);
        let y = ts.y + em;
        ts.cur.ops.push(Op::Word { x: 0.0, y, em, style: Style::Regular, text: t });
        // dot leader
        let dot = text_width(&ts.fonts[0], em, " .");
        let mut x = tw + dot;
        while x + dot < ts.w - nw - 1.0 {
            ts.cur.ops.push(Op::Word { x, y, em, style: Style::Regular, text: ".".into() });
            x += dot;
        }
        ts.cur.ops.push(Op::Word { x: ts.w - nw, y, em, style: Style::Regular, text: number });
        ts.y += line_h;
    }
    ts.finish_page();
    let out = std::mem::replace(&mut ts.pages, saved);
    ts.cur = saved_cur;
    out
}

fn fill(img: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, v: u8) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (xa, xb) = ((x0.floor() as i32).max(0), (x1.ceil() as i32).min(w));
    let (ya, yb) = ((y0.floor() as i32).max(0), (y1.ceil().max(y0.floor() + 1.0) as i32).min(h));
    for y in ya..yb {
        for x in xa..xb {
            img.put_pixel(x as u32, y as u32, Rgba([v, v, v, 255]));
        }
    }
}

fn blit(dst: &mut RgbaImage, pic: &RgbaImage, (x, y): (f32, f32), w: f32, h: f32) {
    let (tw, th) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    if tw > 12000 || th > 12000 {
        return;
    }
    let scaled = image::imageops::resize(pic, tw, th, FilterType::Triangle);
    for (sx, sy, p) in scaled.enumerate_pixels() {
        let (dx, dy) = (x.round() as i64 + sx as i64, y.round() as i64 + sy as i64);
        if dx < 0 || dy < 0 || dx >= dst.width() as i64 || dy >= dst.height() as i64 {
            continue;
        }
        let a = p.0[3] as u32;
        let d = dst.get_pixel_mut(dx as u32, dy as u32);
        for c in 0..3 {
            d.0[c] = ((p.0[c] as u32 * a + d.0[c] as u32 * (255 - a)) / 255) as u8;
        }
    }
}

/// Black text, `em` pixels tall, baseline at `y`.
fn draw_word(img: &mut RgbaImage, font: &FontVec, text: &str, em: f32, x: f32, y: f32) {
    if em < 0.5 || y < -em * 2.0 || y > img.height() as f32 + em * 2.0 {
        return;
    }
    let scale = em_scale(font, em);
    let sf = font.as_scaled(scale);
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut caret = x;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, y));
        caret += sf.h_advance(id);
        prev = Some(id);
        if let Some(og) = font.outline_glyph(glyph) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && px < w && py < h {
                    let d = img.get_pixel_mut(px as u32, py as u32);
                    let k = 1.0 - cov.clamp(0.0, 1.0);
                    for ch in 0..3 {
                        d.0[ch] = (d.0[ch] as f32 * k) as u8;
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_blocks() {
        let html = r#"<html><head><title>x</title><style>p{}</style></head><body>
            <h1>Chapter &amp; One</h1>
            <p>Hello <i>dear</i>   <b>world</b>&nbsp;!</p>
            <ol><li>first</li><li>second</li></ol>
            <img src="../images/a%20b.png"/>
            <hr/><blockquote><p>quoted</p></blockquote></body></html>"#;
        let b = parse_chapter(html, "OEBPS/text/ch1.xhtml");
        let texts: Vec<String> = b
            .iter()
            .map(|b| match b {
                Block::Text(k, r) => format!("{k:?}:{}", r.iter().map(|x| x.0.as_str()).collect::<String>()),
                Block::Image(h) => format!("img:{h}"),
                Block::Rule => "rule".into(),
            })
            .collect();
        assert_eq!(
            texts,
            [
                "Heading(1):Chapter & One",
                "Para:Hello dear world\u{a0}!",
                "Item(\"1.\"):first",
                "Item(\"2.\"):second",
                "img:OEBPS/images/a b.png",
                "rule",
                "Quote:quoted",
            ]
        );
        if let Block::Text(_, runs) = &b[1] {
            assert_eq!(runs[1], ("dear".to_string(), Style::Italic));
        }
    }

    #[test]
    fn paths_and_entities() {
        assert_eq!(resolve("OEBPS/content.opf", "Text/c1.xhtml#p3"), "OEBPS/Text/c1.xhtml");
        assert_eq!(resolve("a/b/c.xhtml", "../../img.jpg"), "img.jpg");
        assert_eq!(decode_entities("&#233;t&#xE9; &mdash; &bogus; 5 &lt; 6"), "été — &bogus; 5 < 6");
    }
}
