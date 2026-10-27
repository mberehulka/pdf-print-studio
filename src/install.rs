//! Per-user installation (no admin rights needed): program copy + Start menu + Apps & features entry,
//! "Open with" registration for .pdf files and the Explorer context-menu entries.

use crate::i18n::tr;
use crate::trf;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const SHELL_KEY: &str = r"Software\Classes\SystemFileAssociations\.pdf\shell";
const VERB_MAIN: &str = "PdfPrintStudio";
const VERB_QUICK: &str = "PdfPrintStudioQuick";
const PROG_ID: &str = "PdfPrintStudio.pdf";
const APP_KEY: &str = r"Software\Classes\Applications\pdfprintstudio.exe";
const CAPS_KEY: &str = r"Software\PdfPrintStudio\Capabilities";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PdfPrintStudio";
const APP_NAME: &str = "PDF Print Studio";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
        Ok(m) => message(APP_NAME, &m, false),
        Err(e) => message(APP_NAME, &e, true),
    }
}

pub fn install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PdfPrintStudio")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("pdfprintstudio.exe")
}

fn start_menu_link() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(r"Microsoft\Windows\Start Menu\Programs\PDF Print Studio.lnk")
}

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

fn has_key(path: &str) -> bool {
    hkcu().open_subkey(path).is_ok()
}

fn running_installed() -> bool {
    std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok()) == std::fs::canonicalize(installed_exe()).ok()
}

fn notify_shell() {
    unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) }; // SHCNE_ASSOCCHANGED
}

fn set(key: &RegKey, name: &str, value: &str) -> Result<(), String> {
    key.set_value(name, &value).map_err(|e| e.to_string())
}

fn create(path: &str) -> Result<RegKey, String> {
    hkcu().create_subkey(path).map(|(k, _)| k).map_err(|e| e.to_string())
}

/// What is currently installed.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Status {
    pub program: bool,
    pub open_with: bool,
    pub context_menu: bool,
}

pub fn status() -> Status {
    Status {
        program: installed_exe().exists() && has_key(UNINSTALL_KEY),
        open_with: has_key(&format!(r"Software\Classes\{PROG_ID}\shell\open\command")),
        context_menu: has_key(&format!(r"{SHELL_KEY}\{VERB_MAIN}\command")),
    }
}

// ---------------------------------------------------------------- program

/// Copies the running exe into %LOCALAPPDATA%, adds a Start menu shortcut and an "Apps & features" entry.
pub fn install_program() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = installed_exe();
    std::fs::create_dir_all(install_dir()).map_err(|e| e.to_string())?;
    if !running_installed() {
        std::fs::copy(&exe, &target).map_err(|e| trf!("Copying program to {} failed: {}", target.display(), e))?;
    }
    let t = target.display().to_string();

    let key = create(UNINSTALL_KEY)?;
    set(&key, "DisplayName", APP_NAME)?;
    set(&key, "DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    set(&key, "DisplayIcon", &t)?;
    set(&key, "InstallLocation", &install_dir().display().to_string())?;
    set(&key, "UninstallString", &format!("\"{t}\" --uninstall"))?;
    key.set_value("NoModify", &1u32).map_err(|e| e.to_string())?;
    key.set_value("NoRepair", &1u32).map_err(|e| e.to_string())?;
    let kb = std::fs::metadata(&target).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    key.set_value("EstimatedSize", &kb).map_err(|e| e.to_string())?;

    // a shortcut needs the COM shell-link object; PowerShell's WScript.Shell is the simplest way to reach it
    let q = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.Save()",
        q(&start_menu_link()),
        q(&target),
        q(&install_dir())
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    // entries made by an older copy point to the old exe; refresh them
    let st = status();
    if st.open_with {
        register_open_with()?;
    }
    if st.context_menu {
        install_context_menu()?;
    }
    Ok(trf!("Program installed in {}", t))
}

/// Removes everything: Explorer entries, shortcut, "Apps & features" entry and the installed exe.
pub fn uninstall_program() -> Result<String, String> {
    uninstall_context_menu()?;
    unregister_open_with()?;
    let _ = hkcu().delete_subkey_all(UNINSTALL_KEY);
    let _ = std::fs::remove_file(start_menu_link());
    if running_installed() {
        // a running exe cannot delete itself: let a hidden cmd remove the folder once this process has exited
        let exe = installed_exe().display().to_string();
        let dir = install_dir().display().to_string();
        let script = format!(
            "/C for /L %i in (1,1,300) do @(del /f /q \"{exe}\" 2>nul & if not exist \"{exe}\" (rmdir /s /q \"{dir}\" & exit) else ping -n 2 127.0.0.1 >nul)"
        );
        let _ = Command::new("cmd").raw_arg(script).creation_flags(CREATE_NO_WINDOW).spawn();
    } else {
        let _ = std::fs::remove_dir_all(install_dir());
    }
    Ok(tr("Program uninstalled.").into())
}

/// The installed exe, installing the program first if needed.
fn ensure_installed() -> Result<String, String> {
    if !status().program {
        install_program()?;
    }
    Ok(installed_exe().display().to_string())
}

// ---------------------------------------------------------------- "Open with"

pub fn register_open_with() -> Result<String, String> {
    let t = ensure_installed()?;
    let open = format!("\"{t}\" \"%1\"");
    let icon = format!("{t},0");

    let prog = create(&format!(r"Software\Classes\{PROG_ID}"))?;
    set(&prog, "", "PDF")?;
    set(&prog, "FriendlyTypeName", "PDF")?;
    set(&create(&format!(r"Software\Classes\{PROG_ID}\DefaultIcon"))?, "", &icon)?;
    set(&create(&format!(r"Software\Classes\{PROG_ID}\shell\open\command"))?, "", &open)?;
    set(&create(r"Software\Classes\.pdf\OpenWithProgids")?, PROG_ID, "")?;

    let app = create(APP_KEY)?;
    set(&app, "FriendlyAppName", APP_NAME)?;
    set(&create(&format!(r"{APP_KEY}\SupportedTypes"))?, ".pdf", "")?;
    set(&create(&format!(r"{APP_KEY}\shell\open\command"))?, "", &open)?;

    // lets the app show up in Settings → Default apps
    let caps = create(CAPS_KEY)?;
    set(&caps, "ApplicationName", APP_NAME)?;
    set(&caps, "ApplicationDescription", "Advanced PDF printing")?;
    set(&create(&format!(r"{CAPS_KEY}\FileAssociations"))?, ".pdf", PROG_ID)?;
    set(&create(r"Software\RegisteredApplications")?, APP_NAME, CAPS_KEY)?;

    notify_shell();
    Ok(tr("\"Open with\" registered.").into())
}

pub fn unregister_open_with() -> Result<String, String> {
    let h = hkcu();
    let _ = h.delete_subkey_all(format!(r"Software\Classes\{PROG_ID}"));
    let _ = h.delete_subkey_all(APP_KEY);
    let _ = h.delete_subkey_all(r"Software\PdfPrintStudio");
    if let Ok(k) = h.open_subkey_with_flags(r"Software\Classes\.pdf\OpenWithProgids", winreg::enums::KEY_WRITE) {
        let _ = k.delete_value(PROG_ID);
    }
    if let Ok(k) = h.open_subkey_with_flags(r"Software\RegisteredApplications", winreg::enums::KEY_WRITE) {
        let _ = k.delete_value(APP_NAME);
    }
    notify_shell();
    Ok(tr("\"Open with\" removed.").into())
}

/// Windows does not let programs set the default app; open the settings page where the user can.
pub fn open_default_apps() {
    let _ = Command::new("explorer").arg("ms-settings:defaultapps").spawn();
}

// ---------------------------------------------------------------- context menu

pub fn install_context_menu() -> Result<String, String> {
    let t = ensure_installed()?;
    let verbs = [
        (VERB_MAIN, tr("Print with PDF Print Studio…"), format!("\"{t}\" \"%1\"")),
        (VERB_QUICK, tr("Quick print (last settings)"), format!("\"{t}\" --quick \"%1\"")),
    ];
    for (verb, label, cmd) in verbs {
        let key = create(&format!(r"{SHELL_KEY}\{verb}"))?;
        set(&key, "", label)?;
        set(&key, "MUIVerb", label)?;
        set(&key, "Icon", "shell32.dll,16")?;
        set(&create(&format!(r"{SHELL_KEY}\{verb}\command"))?, "", &cmd)?;
    }
    notify_shell();
    Ok(tr("Context menu installed.").into())
}

pub fn uninstall_context_menu() -> Result<String, String> {
    for verb in [VERB_MAIN, VERB_QUICK] {
        let _ = hkcu().delete_subkey_all(format!(r"{SHELL_KEY}\{verb}"));
    }
    notify_shell();
    Ok(tr("Context menu entries removed.").into())
}

// ---------------------------------------------------------------- command line

/// `--install`: program, "Open with" and context menu in one go.
pub fn install() -> Result<String, String> {
    install_program()?;
    register_open_with()?;
    install_context_menu()?;
    Ok(trf!(
        "PDF Print Studio is installed.\n\nRight-click any PDF → \"Print with PDF Print Studio…\" or \"Open with\" (on Windows 11 under \"Show more options\").\n\nProgram location:\n{}",
        installed_exe().display()
    ))
}

/// `--uninstall` (also what "Apps & features" runs).
pub fn uninstall() -> Result<String, String> {
    uninstall_program()?;
    Ok(tr("PDF Print Studio was removed.").into())
}
