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
mod photo;
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
    install::migrate_legacy();
    let prefs = prefs::Prefs::load();
    prefs.apply_lang();
    update::cleanup();
    install::refresh_thumbnails();

    let mut files: Vec<PathBuf> = Vec::new();
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
                    "Print Studio",
                    i18n::tr("Usage:\n  print-studio [file.pdf | pictures… | folder]\n  print-studio --quick file.pdf   (print with last settings)\n  print-studio --install            (install, add \"Open with\" and context menu)\n  print-studio --uninstall          (remove everything again)"),
                    false,
                );
                return;
            }
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if let Err(e) = app::run(files, quick, prefs) {
        install::message("Print Studio", &trf!("Failed to start UI: {}", e), true);
    }
}
