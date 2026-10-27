//! In-app user manual (the ❓ button / F1) with a live template playground.

use crate::i18n::tr;
use crate::template::{Template, Vars};
use crate::theme::pal;
use eframe::egui::{self, Color32, RichText, Ui};

enum B {
    H(&'static str),
    P(&'static str),
    Tip(&'static str),
    Code(&'static str),
    /// Template plus what it does; gets "Try" / "Copy" buttons.
    Ex(&'static str, &'static str),
    Table(&'static [(&'static str, &'static str)]),
}

struct Chapter {
    title: &'static str,
    blocks: &'static [B],
}

pub const PLAYGROUND: usize = usize::MAX;
pub const CH_HEADERS: usize = 6;
pub const CH_WATERMARK: usize = 7;
pub const CH_STAMPS: usize = 8;
pub const CH_TEMPLATES: usize = 9;
pub const CH_COOKBOOK: usize = 12;

static CHAPTERS: &[Chapter] = &[
    Chapter {
        title: "🚀 Quick start",
        blocks: &[
            B::P("PDF Print Studio renders every page itself and sends finished sheets to the printer, so what you see in the Print preview is exactly what comes out."),
            B::H("Printing in four steps"),
            B::P("1. Open a PDF: drag it onto the window, use 📂 Open…, or right-click a PDF in Explorer and choose \"Print with PDF Print Studio…\".\n2. Pick a printer and paper in the Printer section on the right.\n3. Choose pages, layout and overlays. The preview updates live; use ◀ ▶ (or PageUp/PageDown) to step through sheet sides.\n4. Press Print (Ctrl+P)."),
            B::Tip("Your last settings are restored automatically. Save combinations you use often as named presets (💾 Presets)."),
            B::H("Reading the preview"),
            B::P("The red tint marks the printer's non-printable area. \"Cells\" outlines where each page is placed. The ink estimate shows how much toner/ink the current side needs (≈5% is a normal text page)."),
            B::H("Quick print"),
            B::P("\"Quick print (last settings)\" in the Explorer menu, or `pdfprintstudio --quick file.pdf`, prints immediately with the settings you used last."),
        ],
    },
    Chapter {
        title: "📄 Pages",
        blocks: &[
            B::P("Choose which pages are printed: all, the current page, a custom range, or pages you picked with Ctrl+click in the thumbnail sidebar (★)."),
            B::H("Range syntax"),
            B::Table(&[
                ("1-3, 5", "pages 1, 2, 3 and 5"),
                ("8-end  or  8-", "page 8 to the last page"),
                ("10-4", "pages 10 down to 4 (descending)"),
                ("2*3", "page 2 three times"),
                ("end", "the last page"),
            ]),
            B::H("Filters"),
            B::P("After the range you can keep only odd or even pages, every n-th page (with an offset), exclude a list of pages and reverse the order. Filters apply in that order."),
            B::Tip("Printing odd pages, flipping the stack and then printing even pages in reverse is the classic manual duplex. The Printer section has a built-in Manual duplex that does this for you."),
        ],
    },
    Chapter {
        title: "▦ Layout",
        blocks: &[
            B::H("Normal / N-up"),
            B::P("Put several pages on one sheet with any grid up to 16 × 16. The reading order (rows or columns, left-to-right or right-to-left) and the gap between cells are adjustable. Page borders and page numbers are in the overlays section."),
            B::H("Booklet"),
            B::P("Reorders and pairs pages so that the printed stack, folded in the middle, reads like a book. Print two-sided, flip on the short edge (or use manual duplex). Long documents can be split into signatures (several smaller folded stacks). Right-to-left binding is available for Arabic/Hebrew/manga."),
            B::H("Poster / tiling"),
            B::P("Enlarges one page across N × M sheets. The overlap gives you room to glue; dashed guide lines and tile labels (A1, A2, B1…) help with assembly."),
            B::H("Step & repeat"),
            B::P("Repeats the same page in a grid – labels, business cards, tickets. Combine with crop marks."),
            B::H("Margins and binding gutter"),
            B::P("Margins are measured from the paper edge. The binding gutter adds extra space on the bound edge and alternates sides on duplex backs. \"Keep content inside the printable area\" adds the printer's hardware margins."),
        ],
    },
    Chapter {
        title: "⤢ Size & position",
        blocks: &[
            B::Table(&[
                ("Fit", "scale each page to the cell, keeping its proportions"),
                ("Shrink only", "only shrink pages that are too big, never enlarge"),
                ("Actual size", "100%, the original dimensions"),
                ("Custom", "any percentage"),
                ("Fill", "fill the cell completely, cropping what sticks out"),
                ("Stretch", "fill the cell, distorting proportions"),
            ]),
            B::P("Alignment uses a 9-point grid; offsets move the page in millimetres. Rotation can be forced to 0/90/180/270°, and auto-rotate turns pages to match the cell's orientation. Mirroring is for iron-on transfers and back-lit films."),
            B::P("Trim removes a fixed number of millimetres from the source pages; auto-trim detects white borders and zooms in on the content."),
        ],
    },
    Chapter {
        title: "🎨 Color & quality",
        blocks: &[
            B::P("Color modes: color, grayscale, black & white (threshold), Floyd–Steinberg dithering and ordered halftone. Brightness, contrast, gamma and saturation are applied before the color mode. Invert is handy for dark-mode PDFs; toner saver lightens everything."),
            B::H("Rendering"),
            B::P("Pages are rendered at the printer's resolution (or a custom DPI, with a cap to keep jobs small). Supersampling renders larger and scales down for smoother edges. The resampling filter and unsharp-mask sharpening control the final look. The spool format (24-bit, 8-bit gray, 1-bit) changes how much data is sent to the printer."),
            B::Tip("For crisp text on laser printers: B&W threshold or 1-bit spool at the printer's native DPI."),
        ],
    },
    Chapter {
        title: "🖨 Printer & output",
        blocks: &[
            B::P("All installed printers are listed with their live status. \"Properties\" opens the driver's own dialog; its choices are kept. Paper size, custom paper, orientation (Auto picks whichever gives the larger result), tray, media type and quality are sent to the driver."),
            B::H("Two-sided printing"),
            B::P("Long edge = book style, short edge = calendar/booklet style. Manual duplex prints all fronts, then asks you to turn the stack over and put it back before printing the backs."),
            B::H("Copies & output"),
            B::P("Copies can be collated (1-2-3, 1-2-3) or not (1-1, 2-2, 3-3). Print to file writes the printer's output to disk. Export PNG renders every sheet side as an image."),
            B::H("Command line"),
            B::Code("pdfprintstudio file.pdf\npdfprintstudio --quick file.pdf\npdfprintstudio --install\npdfprintstudio --uninstall"),
        ],
    },
    Chapter {
        title: "📑 Headers & footers",
        blocks: &[
            B::P("Header and footer each have a left, centre and right slot. Every slot is a template: plain text plus {variables}, conditions and loops (see Template language)."),
            B::Table(&[
                ("Show when", "a condition; the band is printed only where it is true. Empty = always."),
                ("Mirror on even sides", "swaps left and right on even sides – page numbers on the outside edge, as in books."),
                ("Distance", "distance of the text from the paper edge; 0 = middle of the margin."),
                ("Separator line", "a thin rule between the band and the content."),
                ("Place on", "the whole sheet, or each page on it (n-up / booklet: every small page gets its own header)."),
                ("Font", "a different font than the overlay font, e.g. a bold file such as arialbd.ttf."),
            ]),
            B::H("Examples"),
            B::Ex("Page {page} of {pages}", "classic page counter"),
            B::Ex("{if first}{else}{name} – page {page}{end}", "nothing on the first side, file name and page afterwards"),
            B::Ex("{if odd}{name}{else}{date}{end}", "alternating content on odd and even sides"),
            B::Ex("{page|roman|lower}", "roman page numbers (i, ii, iii…) for a preface"),
            B::Ex("Printed by {user} on {weekday}, {day} {monthname} {year} at {time}", "audit line"),
            B::Ex("{name}{br}{folder}", "two lines: file name and folder"),
            B::Tip("In n-up layouts {page} is the first page on the sheet and {range} is e.g. \"5-8\". With \"Place on: each page\" every page gets its own number."),
        ],
    },
    Chapter {
        title: "💧 Watermark",
        blocks: &[
            B::P("A large text or PNG picture drawn across the sheet or each page. The text is a template, so it can change from page to page."),
            B::Table(&[
                ("Behind content", "only tints the white paper, so text on top stays perfectly readable"),
                ("One per page", "in n-up layouts each small page gets its own watermark"),
                ("Position / offset", "9-point anchor inside the margins, then moved by the offset"),
                ("Tile", "repeats the watermark over the whole area in a brick pattern (anti-copy look); the gaps control the density"),
                ("Frame", "draws a rubber-stamp box around the text"),
                ("Image", "a PNG (transparency is respected) used instead of the text; opacity and angle still apply"),
                ("Show when", "condition, e.g. only on the first side"),
            ]),
            B::H("Examples"),
            B::Ex("{if copy = 1}ORIGINAL{else}COPY {copy}{end}", "first copy says ORIGINAL, the others COPY 2, COPY 3…"),
            B::Ex("{user|upper}{br}{date}", "two-line personalised watermark (tile it for leak protection)"),
            B::Ex("{if last}END{else}DRAFT{end}", "different text on the last side"),
            B::Ex("{if file contains \"draft\"}DRAFT{end}", "only when the file name contains \"draft\""),
        ],
    },
    Chapter {
        title: "🔖 Stamps",
        blocks: &[
            B::P("Stamps are free-placed texts – as many as you like. Each has its own template, font, size, colour, opacity, angle, 9-point position with offset, optional frame and \"show when\" condition, and can be placed on the sheet or on every page."),
            B::H("Ideas"),
            B::Ex("{sheet + 1000|pad:6}", "Bates numbering starting at 001001 – put it bottom-right"),
            B::Ex("APPROVED{br}{date}", "framed approval stamp, rotated a few degrees, red"),
            B::Ex("{page}/{pages}", "small page counter in a corner of every page (Place on: each page)"),
            B::Ex("{if back}↻ back side{end}", "mark back sides when checking duplex output"),
            B::Ex("{tile}", "poster tile label (A1, B2…) in a big font"),
            B::Ex("{for p in sheetpages sep \" · \"}{p}{end}", "list all pages on an n-up sheet"),
        ],
    },
    Chapter {
        title: "🧩 Template language",
        blocks: &[
            B::P("Every header/footer slot, watermark text and stamp is a template. Anything outside curly braces is printed as it is."),
            B::H("Printing values"),
            B::Code("{page}               a variable\n{page + 1}           arithmetic: + - * / %  and ( )\n{page|roman}         a filter\n{page|pad:3}         a filter with an argument\n{file|replace(\".pdf\", \"\")}   several arguments\n{\"Page \" + page}     text in quotes; + joins text"),
            B::H("Conditions"),
            B::Code("{if odd}…{end}\n{if page = 1}…{elif page < 5}…{else}…{end}\n{if not first and copies > 1}…{end}"),
            B::P("Comparisons: =  !=  <  <=  >  >=  (also == and <>).\nLogic: and, or, not (also &&, ||, !).\nText: contains, startswith, endswith – case-insensitive.\nLists: in, e.g. {if page in [1, 3, 5]} or {if page in 10..20}.\nAny value can be a condition: 0, empty text and empty lists are false. true / false and yes / no are also accepted."),
            B::H("Loops"),
            B::Code("{for p in sheetpages sep \", \"}{p}{end}\n{for i in 1..5}{i} {end}\n{for i in 3}★{end}"),
            B::P("Inside a loop you can use loop_index (1, 2, …), loop_first, loop_last and loop_count. A number loops that many times; text loops over its characters."),
            B::H("Other tags"),
            B::Table(&[
                ("{set x = page * 2}", "store a value in a variable"),
                ("{br}", "line break (multi-line headers, watermarks and stamps)"),
                ("{# note }", "comment, prints nothing"),
                ("{{  and  }}", "literal { and }"),
                ("{end}", "closes {if} and {for}; {endif}, {endfor}, {/if} also work"),
            ]),
            B::H("Filters bind tightly"),
            B::P("A filter applies to the value right before it: {page + 1|roman} is page + roman(1). Use brackets: {(page + 1)|roman}. Filters can be chained: {name|upper|left:10}. Every filter can also be called as a function: {roman(page)}."),
            B::H("Mistakes"),
            B::P("Errors are shown in red under the field. A template with a syntax error is printed as typed so nothing disappears silently, and a broken \"show when\" condition counts as true."),
        ],
    },
    Chapter {
        title: "🔤 Variables",
        blocks: &[
            B::H("Pages"),
            B::Table(&[
                ("page", "number of the (first) page on this sheet side or cell"),
                ("lastpage", "last page on this sheet side"),
                ("range", "\"5\" or \"5-8\" on n-up sheets"),
                ("count", "how many pages are on this side"),
                ("sheetpages", "list of all page numbers on this side"),
                ("pages", "total pages in the document"),
                ("selected", "number of pages being printed"),
                ("pageodd / pageeven", "the document page number is odd / even"),
            ]),
            B::H("Sheet sides"),
            B::Table(&[
                ("sheet / sheets", "number of this printed side / total sides"),
                ("odd / even", "the side number is odd / even"),
                ("first / last", "this is the first / last side"),
                ("front / back / side", "duplex side; side is the text \"front\" or \"back\""),
                ("cell / cells", "position of the page on the sheet (with Place on: each page) / cells on the sheet"),
                ("copy / copies", "which copy is printing / number of copies (preview shows copy 1)"),
                ("layout", "normal, booklet, poster or repeat"),
                ("tile, tilerow, tilecol", "poster tile, e.g. \"B3\""),
            ]),
            B::H("File & system"),
            B::Table(&[
                ("file", "file name with extension"),
                ("name / ext", "file name without extension / the extension"),
                ("folder / path", "parent folder name / full path"),
                ("printer", "printer name"),
                ("user / computer", "Windows user and computer name"),
            ]),
            B::H("Date & time"),
            B::Table(&[
                ("date / time / datetime", "2026-10-26 / 14:05 / both"),
                ("year month day", "numbers – use {day|pad:2} for 05"),
                ("hour minute second", "numbers"),
                ("weekday / monthname", "Monday / October"),
            ]),
            B::Tip("Open the 🧪 Playground to see the current value of every variable for the side shown in the preview."),
        ],
    },
    Chapter {
        title: "🔧 Filters & functions",
        blocks: &[
            B::Table(&[
                ("upper, lower, title, trim", "change case / strip spaces"),
                ("roman", "4 → IV (add |lower for iv)"),
                ("alpha", "1 → A, 27 → AA"),
                ("words", "21 → twenty-one"),
                ("ordinal", "2 → 2nd"),
                ("pad:n", "zero-pad to n digits: 7|pad:3 → 007"),
                ("fixed:n", "n decimals: 2.5|fixed:2 → 2.50"),
                ("left:n, right:n", "first / last n characters"),
                ("replace(a, b)", "replace text"),
                ("default:x", "x if the value is empty or 0"),
                ("repeat:n", "repeat text n times"),
                ("len", "length of text or list"),
                ("join(sep)", "join a list: sheetpages|join:\" + \""),
                ("min(…), max(…)", "smallest / largest value"),
                ("abs, floor, ceil, round(n)", "number rounding"),
                ("iif(cond, a, b)", "a when cond is true, else b – inline if"),
            ]),
            B::H("Examples"),
            B::Ex("{iif(odd, \"→\", \"←\")} {page}", "arrow pointing outwards"),
            B::Ex("{name|upper|left:20}", "file name in capitals, cut to 20 characters"),
            B::Ex("Chapter {sheet|alpha} · {sheet|ordinal} side", "letters and ordinals"),
            B::Ex("{(page / pages * 100)|round}% done", "progress through the document"),
        ],
    },
    Chapter {
        title: "📚 Cookbook",
        blocks: &[
            B::H("\"Show when\" conditions"),
            B::Table(&[
                ("not first", "skip the cover / first side"),
                ("odd   /   even", "odd or even sides"),
                ("front   /   back", "only fronts or backs when printing two-sided"),
                ("page > 2 and not last", "skip the first two pages and the last side"),
                ("page in [1, 5, 9]", "only specific pages"),
                ("page in 10..20", "a page range"),
                ("page % 4 = 1", "every fourth page"),
                ("copy = 1", "only on the first copy"),
                ("layout = \"booklet\"", "only when printing booklets"),
                ("weekday = \"Friday\"", "… because you can"),
            ]),
            B::H("Book-style page numbers"),
            B::P("Footer right: {page}. Tick \"Mirror on even sides\". On a two-sided print the number is always on the outside edge."),
            B::H("Different first page"),
            B::Ex("{if first}{file}{else}{name} · {page}/{pages}{end}", "title on the first side, running header afterwards"),
            B::H("Front matter in roman numbers"),
            B::Ex("{if page <= 4}{page|roman|lower}{else}{page - 4}{end}", "pages 1–4 as i–iv, then 1, 2, 3…"),
            B::H("Leak-tracing watermark"),
            B::P("Watermark text {user}{br}{datetime}, size 18 pt, opacity 8%, angle 30°, Tile on, Behind content on."),
            B::H("Bates numbering"),
            B::P("Add a stamp with text ACME-{sheet + 4999|pad:6}, bottom-right, 8 pt. Change 4999 to continue from a previous batch."),
            B::H("Booklet with page numbers on every small page"),
            B::P("Layout: Booklet. Footer centre: {page}, Place on: each page. Each half of the sheet gets its own number."),
        ],
    },
    Chapter {
        title: "⌨ Shortcuts",
        blocks: &[B::Table(&[
            ("F1", "open this manual"),
            ("Ctrl+P", "print"),
            ("PageDown / →", "next side (preview) or page (viewer)"),
            ("PageUp / ←", "previous side or page"),
            ("Home / End", "first / last"),
            ("Ctrl+click thumbnail", "pick a page (★) for \"Picked in sidebar\""),
            ("Drag & drop", "open a PDF"),
        ])],
    },
];

pub struct Manual {
    pub open: bool,
    pub chapter: usize,
    pub playground: String,
    search: String,
}

impl Default for Manual {
    fn default() -> Self {
        Self {
            open: false,
            chapter: 0,
            playground: "{if first}{file}{else}{name}{end} – page {page} of {pages}{br}{weekday}, {day|ordinal} {monthname} {year}".into(),
            search: String::new(),
        }
    }
}

fn mono(t: &str) -> RichText {
    RichText::new(t).monospace().color(pal().code_fg)
}

impl Manual {
    pub fn open_at(&mut self, chapter: usize) {
        self.open = true;
        self.chapter = chapter;
    }

    pub fn show(&mut self, ctx: &egui::Context, vars: &Vars) {
        let mut open = self.open;
        egui::Window::new(tr("❓ PDF Print Studio – Manual"))
            .open(&mut open)
            .default_size([900.0, 640.0])
            .min_width(640.0)
            .min_height(420.0)
            .max_width(1200.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let h = ui.available_height().max(380.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(tr("🔍 search")).desired_width(190.0));
                        ui.add_space(4.0);
                        let q = self.search.trim().to_lowercase();
                        egui::ScrollArea::vertical().id_salt("man_nav").max_height(h - 30.0).show(ui, |ui| {
                            for (i, c) in CHAPTERS.iter().enumerate() {
                                if !q.is_empty() && !chapter_matches(c, &q) {
                                    continue;
                                }
                                if ui.selectable_label(self.chapter == i, tr(c.title)).clicked() {
                                    self.chapter = i;
                                }
                            }
                            ui.separator();
                            if ui.selectable_label(self.chapter == PLAYGROUND, tr("🧪 Playground")).clicked() {
                                self.chapter = PLAYGROUND;
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("man_body", self.chapter))
                            .max_height(h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if self.chapter == PLAYGROUND {
                                    self.ui_playground(ui, vars);
                                } else if let Some(c) = CHAPTERS.get(self.chapter) {
                                    self.ui_chapter(ui, c);
                                }
                            });
                    });
                });
            });
        self.open = open;
    }

    fn ui_chapter(&mut self, ui: &mut Ui, c: &Chapter) {
        ui.heading(tr(c.title));
        ui.add_space(4.0);
        for b in c.blocks {
            match b {
                B::H(t) => {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr(t)).strong().size(15.0));
                }
                B::P(t) => {
                    ui.label(tr(t));
                }
                B::Tip(t) => {
                    egui::Frame::none()
                        .fill(pal().tip_bg)
                        .rounding(4.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("💡 {}", tr(t))).color(pal().tip_fg));
                        });
                }
                B::Code(t) => {
                    egui::Frame::none()
                        .fill(pal().code_bg)
                        .rounding(4.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.label(mono(tr(t)));
                        });
                }
                B::Ex(tpl, what) => {
                    egui::Frame::none()
                        .fill(pal().code_bg)
                        .rounding(4.0)
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(mono(tpl));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button(tr("📋 Copy")).on_hover_text(tr("Copy to the clipboard")).clicked() {
                                        ui.ctx().copy_text(tpl.to_string());
                                    }
                                    if ui.small_button(tr("▶ Try")).on_hover_text(tr("Open in the playground")).clicked() {
                                        self.playground = tpl.to_string();
                                        self.chapter = PLAYGROUND;
                                    }
                                });
                            });
                            ui.label(RichText::new(tr(what)).small().color(pal().muted));
                        });
                }
                B::Table(rows) => {
                    let max = (ui.available_width() - 200.0).max(240.0);
                    egui::Grid::new(ui.next_auto_id()).num_columns(2).striped(true).spacing([14.0, 5.0]).max_col_width(max).show(ui, |ui| {
                        for (k, v) in rows.iter() {
                            ui.label(mono(tr(k)));
                            ui.label(tr(v));
                            ui.end_row();
                        }
                    });
                }
            }
            ui.add_space(3.0);
        }
    }

    fn ui_playground(&mut self, ui: &mut Ui, vars: &Vars) {
        ui.heading(tr("🧪 Playground"));
        ui.label(tr("Type a template; it is evaluated for the side currently shown in the print preview."));
        ui.add(
            egui::TextEdit::multiline(&mut self.playground)
                .font(egui::TextStyle::Monospace)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.small_button(tr("📋 Copy")).clicked() {
                ui.ctx().copy_text(self.playground.clone());
            }
            if ui.small_button(tr("Clear")).clicked() {
                self.playground.clear();
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new(tr("Result")).strong());
        let (out, err) = match Template::parse(&self.playground) {
            Ok(t) => t.render(vars),
            Err(e) => (String::new(), Some(e)),
        };
        egui::Frame::none().fill(Color32::WHITE).rounding(4.0).inner_margin(10.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let shown = if out.is_empty() { tr("(empty)").to_string() } else { out };
            ui.label(RichText::new(shown).color(Color32::BLACK).size(15.0));
        });
        if let Some(e) = err {
            ui.label(RichText::new(format!("⚠ {e}")).color(pal().err));
        }
        ui.add_space(8.0);
        ui.label(RichText::new(tr("Current values")).strong());
        let mut keys: Vec<&String> = vars.keys().collect();
        keys.sort();
        let max = (ui.available_width() - 140.0).max(200.0);
        egui::Grid::new("pg_vars").num_columns(2).striped(true).spacing([14.0, 4.0]).max_col_width(max).show(ui, |ui| {
            for k in keys {
                if ui.link(mono(k)).on_hover_text(tr("Insert into the template")).clicked() {
                    self.playground.push_str(&format!("{{{k}}}"));
                }
                ui.add(egui::Label::new(vars[k].text()).truncate());
                ui.end_row();
            }
        });
    }
}

fn chapter_matches(c: &Chapter, q: &str) -> bool {
    let has = |t: &str| t.to_lowercase().contains(q) || tr(t).to_lowercase().contains(q);
    has(c.title)
        || c.blocks.iter().any(|b| match b {
            B::H(t) | B::P(t) | B::Tip(t) | B::Code(t) => has(t),
            B::Ex(a, b) => has(a) || has(b),
            B::Table(rows) => rows.iter().any(|(a, b)| has(a) || has(b)),
        })
}
