#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod compose;
mod install;
mod job;
mod layout;
mod manual;
mod pages;
mod pdf;
mod printer;
mod settings;
mod template;
mod text;
mod worker;

use std::path::PathBuf;

fn main() {
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
                    "Usage:\n  pdfprintstudio [file.pdf]\n  pdfprintstudio --quick file.pdf   (print with last settings)\n  pdfprintstudio --install            (add Explorer context menu)\n  pdfprintstudio --uninstall          (remove Explorer context menu)",
                    false,
                );
                return;
            }
            _ => file = Some(PathBuf::from(arg)),
        }
    }
    if let Err(e) = app::run(file, quick) {
        install::message("PDF Print Studio", &format!("Failed to start UI: {e}"), true);
    }
}
