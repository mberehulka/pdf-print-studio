#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cbz;
mod compose;
mod doc;
mod epub;
mod i18n;
mod install;
mod job;
mod layout;
mod manual;
mod pages;
mod pdf;
mod prefs;
mod printer;
mod settings;
mod template;
mod text;
mod theme;
mod update;
mod worker;

use std::path::PathBuf;

fn main() {
    let prefs = prefs::Prefs::load();
    prefs.apply_lang();
    update::cleanup();
    install::refresh_thumbnails();

    let mut file: Option<PathBuf> = None;
    let mut quick = false;
    // --quiet: no message box for --install / --uninstall, failure only in the exit code (for scripts)
    let quiet = std::env::args().any(|a| a == "--quiet");
    let finish = |r: Result<String, String>| {
        let failed = r.is_err();
        if !quiet {
            install::report(r);
        }
        std::process::exit(failed as i32);
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--install" => finish(install::install()),
            "--uninstall" => finish(install::uninstall()),
            "--quiet" => {}
            "--quick" => quick = true,
            "--help" | "-h" | "/?" => {
                install::message(
                    "PDF Print Studio",
                    i18n::tr("Usage:\n  pdfprintstudio [file.pdf]\n  pdfprintstudio --quick file.pdf   (print with last settings)\n  pdfprintstudio --install            (install, add \"Open with\" and context menu)\n  pdfprintstudio --uninstall          (remove everything again)"),
                    false,
                );
                return;
            }
            _ => file = Some(PathBuf::from(arg)),
        }
    }
    if let Err(e) = app::run(file, quick, prefs) {
        install::message("PDF Print Studio", &trf!("Failed to start UI: {}", e), true);
    }
}
