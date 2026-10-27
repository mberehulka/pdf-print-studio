#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cbz;
mod compose;
mod doc;
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

    let mut file: Option<PathBuf> = None;
    let mut quick = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--install" => {
                install::report(install::install());
                return;
            }
            "--uninstall" => {
                install::report(install::uninstall());
                return;
            }
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
