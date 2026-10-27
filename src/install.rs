//! Explorer context-menu registration (per-user, no admin rights needed).

use std::path::PathBuf;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const SHELL_KEY: &str = r"Software\Classes\SystemFileAssociations\.pdf\shell";
const VERB_MAIN: &str = "PdfPrintStudio";
const VERB_QUICK: &str = "PdfPrintStudioQuick";

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, a: *const std::ffi::c_void, b: *const std::ffi::c_void);
}

pub fn message(title: &str, text: &str, error: bool) {
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    unsafe {
        MessageBoxW(0, w(text).as_ptr(), w(title).as_ptr(), if error { 0x10 } else { 0x40 });
    }
}

pub fn report(r: Result<String, String>) {
    match r {
        Ok(m) => message("PDF Print Studio", &m, false),
        Err(e) => message("PDF Print Studio", &e, true),
    }
}

fn install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PdfPrintStudio")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("pdfprintstudio.exe")
}

pub fn is_installed() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"{SHELL_KEY}\{VERB_MAIN}\command"))
        .is_ok()
}

fn notify_shell() {
    unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) }; // SHCNE_ASSOCCHANGED
}

pub fn install() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = installed_exe();
    std::fs::create_dir_all(install_dir()).map_err(|e| e.to_string())?;
    let same = std::fs::canonicalize(&exe).ok() == std::fs::canonicalize(&target).ok();
    if !same {
        std::fs::copy(&exe, &target).map_err(|e| format!("Copying program to {} failed: {e}", target.display()))?;
    }
    let t = target.display().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let verbs = [
        (VERB_MAIN, "Print with PDF Print Studio…", format!("\"{t}\" \"%1\"")),
        (VERB_QUICK, "Quick print (last settings)", format!("\"{t}\" --quick \"%1\"")),
    ];
    for (verb, label, cmd) in verbs {
        let (key, _) = hkcu.create_subkey(format!(r"{SHELL_KEY}\{verb}")).map_err(|e| e.to_string())?;
        key.set_value("", &label).map_err(|e| e.to_string())?;
        key.set_value("MUIVerb", &label).map_err(|e| e.to_string())?;
        key.set_value("Icon", &"shell32.dll,16").map_err(|e| e.to_string())?;
        let (cmd_key, _) = key.create_subkey("command").map_err(|e| e.to_string())?;
        cmd_key.set_value("", &cmd).map_err(|e| e.to_string())?;
    }
    notify_shell();
    Ok(format!(
        "Context menu installed.\n\nRight-click any PDF → \"Print with PDF Print Studio…\"\n(on Windows 11 it is under \"Show more options\").\n\nProgram location:\n{t}"
    ))
}

pub fn uninstall() -> Result<String, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for verb in [VERB_MAIN, VERB_QUICK] {
        let _ = hkcu.delete_subkey_all(format!(r"{SHELL_KEY}\{verb}"));
    }
    notify_shell();
    let running_installed = std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::canonicalize(e).ok())
        == std::fs::canonicalize(installed_exe()).ok();
    if !running_installed {
        let _ = std::fs::remove_file(installed_exe());
    }
    Ok("Context menu entries removed.".into())
}
