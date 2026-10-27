# PDF Print Studio

**Print PDFs exactly the way you want, on Windows.** PDF Print Studio is a small, fast print tool written in
Rust. It gives you N-up layouts, booklets, posters, watermarks, headers, stamps and fine color control, with a live
preview that matches the printed result exactly.

It is a single `.exe` with no external DLLs. Pages are rendered by the PDF engine built into Windows
(`Windows.Data.Pdf`) and sent to the printer through GDI with full driver control.

## Highlights

- **Live print preview** built by the same pipeline as the print job. It shows the printer's non-printable area and estimates the ink coverage.
- **Page selection**: ranges like `1-3, 5, 8-end, 10-4`, odd/even, every n-th page, exclusions, reverse order, and pages picked with Ctrl+click.
- **Layouts**: N-up (up to 16×16), saddle-stitch **booklets** with signatures, **posters** tiled across sheets, and step & repeat for labels and cards.
- **Size & position**: fit / fill / actual / custom scale, alignment, rotation, mirroring, and trimming of margins (manual or automatic).
- **Color**: grayscale, B&W threshold, dithering and halftone, brightness/contrast/gamma, invert for dark-mode PDFs, toner saver.
- **Overlays**: watermarks (text or PNG), headers & footers, free-placed stamps (Bates numbers, COPY, APPROVED…) and crop marks. They are all driven by a small template language with variables, conditions and loops, plus a built-in manual and playground (F1).
- **Printer control**: tray, media, quality, duplex (including manual duplex), copies, collation, presets, and PNG export.
- **English and Português (Brasil)**: the language follows your Windows regional format and can be changed in ⚙ Settings.
- **Light and dark themes**, plus a configurable default window size and position.
- **Built-in updater**: ⚙ Settings → *Check for updates* downloads the latest release from GitHub and restarts.

## Install

1. Download `pdfprintstudio.exe` from the [latest release](https://github.com/mberehulka/pdf-print-studio/releases/latest).
2. Run it and open **⚙ Settings → Installation**:
   - **Program**: copies the app to `%LOCALAPPDATA%\PdfPrintStudio` and adds a Start menu entry and an *Apps & features* entry.
   - **"Open with" for PDF files**: lists the app in Explorer's *Open with* menu.
   - **Explorer context menu**: adds *Print with PDF Print Studio…* and *Quick print (last settings)* to the right-click menu of PDFs.
   - **Default PDF app…**: opens the Windows settings page where you can make it your default PDF app.

Everything is installed per user, so you don't need admin rights. On Windows 11 the context-menu entries are under *Show more options*.

From the command line:

```powershell
pdfprintstudio file.pdf            # open a file
pdfprintstudio --quick file.pdf    # print right away with the last used settings
pdfprintstudio --install           # install program + "Open with" + context menu
pdfprintstudio --uninstall         # remove everything
```

## Build from source

```powershell
cargo build --release              # target\release\pdfprintstudio.exe
cargo test                         # unit tests
$env:TEST_PDF="C:\path\test.pdf"; $env:TEST_OUT="C:\path\out"
cargo test e2e -- --ignored --test-threads=1   # renders every layout and prints via "Microsoft Print to PDF"
```

## Releasing

Bump `version` in `Cargo.toml`, commit, then push a matching tag:

```powershell
git tag v0.2.0
git push origin v0.2.0
```

The [release workflow](.github/workflows/release.yml) tests and builds the binary, then attaches it to a new GitHub release.
Installed copies find that release through *Check for updates*.
