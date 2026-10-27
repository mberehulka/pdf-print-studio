# PDF Print Studio

An advanced PDF printing tool for Windows, written in Rust. It adds **"Print with PDF Print Studio…"**
and **"Quick print (last settings)"** to the right-click menu of every `.pdf` file.

No external DLLs: PDFs are rendered with the Windows built-in `Windows.Data.Pdf` engine and printed
through GDI/winspool with full DEVMODE control.

## Build & install

```powershell
cargo build --release
.\target\release\pdfprintstudio.exe --install     # copies itself to %LOCALAPPDATA%\PdfPrintStudio and registers the menu
.\target\release\pdfprintstudio.exe --uninstall   # removes the menu entries
```

Per-user install (HKCU), no admin rights needed. On Windows 11 the entries are under **Show more options**.
You can also install/uninstall from the button in the app's top-right corner.

CLI: `pdfprintstudio [file.pdf]`, `pdfprintstudio --quick file.pdf` (prints immediately with the last used settings).

## Features

**Viewer**: page thumbnails (selection markers, ★ picks), a document viewer with zoom, and a live **print preview**
rendered with the exact same pipeline as the print job. It shows the printer's non-printable area, the cell outlines and an
estimated ink/toner coverage per side. You can drag & drop PDFs and open password-protected files.

**Page selection**: all / current / ranges (`1-3, 5, 8-end, 10-4` descending, `2*3` repeats) / pages picked with
Ctrl+click. Then odd/even only, every n-th page with offset, an exclude list, and reverse order.

**Layout**: N-up (any grid up to 16×16, 4 reading orders, gap); **booklet** (saddle-stitch imposition, signatures,
right-to-left binding, pages hug the fold); **poster/tiling** (enlarge across N×M sheets with overlap, glue lines
and tile labels); **step & repeat** (labels, business cards). Margins with presets, a binding gutter that alternates on duplex
sides, and an option to respect the hardware margins.

**Size & position**: fit / shrink-only / actual / custom % / fill / stretch, 9-point alignment, x/y offset, rotation
0/90/180/270, auto-rotate, mirror H/V (iron-on transfers), trimming of source margins, and auto-trim of white borders.

**Color & image**: color / grayscale / B&W threshold / Floyd–Steinberg dithering / ordered halftone; brightness,
contrast, gamma, saturation, invert (for dark-mode PDFs) and toner saver. One-click presets.

**Rendering & anti-aliasing**: printer-native or custom DPI (with a cap), 1×–4× supersampling, resampling filter
(nearest / bilinear / bicubic / Lanczos-3), unsharp-mask sharpening, and the spool format (24-bit / 8-bit gray / 1-bit) with a
spool-size estimate.

**Overlays**: a watermark (text or PNG image, any installed font, size, angle, opacity, color, 9-point position, tiling,
rubber-stamp frame, behind-content mode, per page or per sheet), header and footer (left/center/right, own font, mirror on
even sides, separator line, distance, per sheet or per page), any number of free-placed **stamps** (Bates numbers, COPY,
approval boxes…), crop marks, page borders and page numbers. Every overlay has a "show when" condition.

**Templates**: all overlay texts use a small template language – variables (`{page} {pages} {range} {sheet} {copy}
{file} {user} {date} {weekday}`…), arithmetic, filters (`{page|roman}`, `{sheet|pad:6}`), conditions
(`{if odd}…{elif page = 1}…{else}…{end}`), loops (`{for p in sheetpages sep ", "}{p}{end}`), `{set}` and `{br}`.
Errors are shown inline. The **❓** button (or F1) opens a built-in manual with a live template playground.

**Printer**: all installed printers with live status (paper jam, toner low, offline…), queued jobs, the driver's own
properties dialog, paper sizes, custom paper, orientation (auto picks the larger result), paper source/tray, media type,
quality/resolution, duplex long/short edge, **manual duplex** (prints fronts, asks you to flip, prints backs), copies, collation,
job name and print-to-file.

**Presets**: save/load named presets; the last used settings are restored automatically.

**Export**: render the imposed sheets to PNG at any DPI.

## Tests

```powershell
cargo test                                   # unit tests
$env:TEST_PDF="C:\path\test.pdf"; $env:TEST_OUT="C:\path\out"
cargo test e2e -- --ignored --test-threads=1 # renders every layout to PNG and prints via "Microsoft Print to PDF"
```
