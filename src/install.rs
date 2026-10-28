//! Per-user installation (no admin rights needed): program copy + Start menu + Apps & features entry,
//! "Open with" registration for .pdf files and the Explorer context-menu entries.

use crate::i18n::tr;
use crate::trf;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
use winreg::RegKey;

/// Document types the app registers for (with our Explorer thumbnails).
const EXTS: [&str; 3] = ["pdf", "cbz", "epub"];
/// Picture types that get "Open with" and the context menu; Windows keeps drawing their thumbnails.
const IMAGE_EXTS: [&str; 11] = ["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "avif", "jfif"];
const VERB_MAIN: &str = "PrintStudio";
const VERB_QUICK: &str = "PrintStudioQuick";
/// Shell extensions Explorer uses for thumbnails and the preview pane.
const SHELLEX: [&str; 2] = [THUMBNAIL_HANDLER, "{8895b1c6-b41f-4c1c-a562-0d564250836f}"];
const THUMBNAIL_HANDLER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
/// Our thumbnail provider (thumbs/); must match `CLSID` there.
const THUMB_CLSID: &str = "{D56D8E8B-1EAC-48CE-822F-4244DA3EB9C7}";
/// The provider DLL, built by build.rs.
const THUMBS_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/thumbs.dll"));
const APP_KEY: &str = r"Software\Classes\Applications\print-studio.exe";
const CAPS_KEY: &str = r"Software\PrintStudio\Capabilities";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\PrintStudio";
const APP_NAME: &str = "Print Studio";
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
        .join("PrintStudio")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("print-studio.exe")
}

fn start_menu_link() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(r"Microsoft\Windows\Start Menu\Programs\Print Studio.lnk")
}

/// "Send to → Print Studio" passes every selected file to one window.
fn send_to_link() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(r"Microsoft\Windows\SendTo\Print Studio.lnk")
}

/// Every type with "Open with" and context-menu entries.
fn all_exts() -> impl Iterator<Item = &'static str> {
    EXTS.into_iter().chain(IMAGE_EXTS)
}

fn shell_key(ext: &str) -> String {
    format!(r"Software\Classes\SystemFileAssociations\.{ext}\shell")
}

fn prog_id(ext: &str) -> String {
    format!("PrintStudio.{ext}")
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
    pub thumbnails: bool,
}

pub fn status() -> Status {
    Status {
        program: installed_exe().exists() && has_key(UNINSTALL_KEY),
        open_with: has_key(&format!(r"Software\Classes\{}\shell\open\command", prog_id("pdf"))),
        context_menu: has_key(&format!(r"{}\{VERB_MAIN}\command", shell_key("pdf"))),
        thumbnails: has_key(&format!(r"Software\Classes\CLSID\{THUMB_CLSID}\InprocServer32")),
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

    shortcut(&start_menu_link(), &target);

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

/// A shortcut needs the COM shell-link object; PowerShell's WScript.Shell is the simplest way to reach it.
fn shortcut(link: &std::path::Path, target: &std::path::Path) {
    let q = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.Save()",
        q(link),
        q(target),
        q(&install_dir())
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

/// Removes everything: Explorer entries, shortcut, "Apps & features" entry and the installed exe.
pub fn uninstall_program() -> Result<String, String> {
    uninstall_thumbnails()?;
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

/// The handler (CLSID) another program registered for `ext` under `shellex`, so files keep their
/// page thumbnails and preview pane when this app becomes the default.
fn existing_shellex(ext: &str, shellex: &str) -> Option<String> {
    let root = RegKey::predef(HKEY_CLASSES_ROOT);
    let mut keys = vec![format!(r".{ext}\ShellEx\{shellex}"), format!(r"SystemFileAssociations\.{ext}\ShellEx\{shellex}")];
    let mut prog_ids: Vec<String> = Vec::new();
    if let Ok(k) = hkcu().open_subkey(format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.{ext}\UserChoice")) {
        prog_ids.extend(k.get_value::<String, _>("ProgId").ok());
    }
    if let Ok(k) = root.open_subkey(format!(".{ext}")) {
        prog_ids.extend(k.get_value::<String, _>("").ok());
    }
    if let Ok(k) = root.open_subkey(format!(r".{ext}\OpenWithProgids")) {
        prog_ids.extend(k.enum_values().flatten().map(|(name, _)| name));
    }
    keys.extend(prog_ids.iter().filter(|p| !p.starts_with("PrintStudio.")).map(|p| format!(r"{p}\ShellEx\{shellex}")));
    keys.iter().find_map(|k| {
        root.open_subkey(k).ok()?.get_value::<String, _>("").ok().filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case(THUMB_CLSID))
    })
}

pub fn register_open_with() -> Result<String, String> {
    let t = ensure_installed()?;
    let open = format!("\"{t}\" \"%1\"");
    let icon = format!("{t},0");

    let app = create(APP_KEY)?;
    set(&app, "FriendlyAppName", APP_NAME)?;
    set(&create(&format!(r"{APP_KEY}\shell\open\command"))?, "", &open)?;
    set(&create(&format!(r"{APP_KEY}\DefaultIcon"))?, "", &icon)?;

    // lets the app show up in Settings → Default apps
    let caps = create(CAPS_KEY)?;
    set(&caps, "ApplicationName", APP_NAME)?;
    set(&caps, "ApplicationDescription", "Advanced PDF, comic book, e-book and photo printing")?;
    set(&caps, "ApplicationIcon", &icon)?;
    set(&create(r"Software\RegisteredApplications")?, APP_NAME, CAPS_KEY)?;

    for ext in all_exts() {
        let id = prog_id(ext);
        let kind = match ext {
            "pdf" => "PDF".to_string(),
            "cbz" => "Comic book (CBZ)".to_string(),
            "epub" => "E-book (EPUB)".to_string(),
            _ => format!("{} image", ext.to_uppercase()),
        };
        let prog = create(&format!(r"Software\Classes\{id}"))?;
        set(&prog, "", &kind)?;
        set(&prog, "FriendlyTypeName", &kind)?;
        set(&create(&format!(r"Software\Classes\{id}\DefaultIcon"))?, "", &icon)?;
        set(&create(&format!(r"Software\Classes\{id}\shell\open\command"))?, "", &open)?;
        for shellex in SHELLEX {
            // pictures keep Windows' own thumbnails
            let ours = shellex == THUMBNAIL_HANDLER && status().thumbnails && EXTS.contains(&ext);
            if let Some(clsid) = if ours { Some(THUMB_CLSID.to_string()) } else { existing_shellex(ext, shellex) } {
                set(&create(&format!(r"Software\Classes\{id}\ShellEx\{shellex}"))?, "", &clsid)?;
            }
        }
        set(&create(&format!(r"Software\Classes\.{ext}\OpenWithProgids"))?, &id, "")?;
        set(&create(&format!(r"{APP_KEY}\SupportedTypes"))?, &format!(".{ext}"), "")?;
        set(&create(&format!(r"{CAPS_KEY}\FileAssociations"))?, &format!(".{ext}"), &id)?;
    }

    notify_shell();
    Ok(tr("\"Open with\" registered.").into())
}

pub fn unregister_open_with() -> Result<String, String> {
    let h = hkcu();
    for ext in all_exts() {
        let id = prog_id(ext);
        let _ = h.delete_subkey_all(format!(r"Software\Classes\{id}"));
        if let Ok(k) = h.open_subkey_with_flags(format!(r"Software\Classes\.{ext}\OpenWithProgids"), winreg::enums::KEY_WRITE) {
            let _ = k.delete_value(&id);
        }
    }
    let _ = h.delete_subkey_all(APP_KEY);
    let _ = h.delete_subkey_all(r"Software\PrintStudio");
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

// ---------------------------------------------------------------- Explorer thumbnails

fn thumbs_dll() -> PathBuf {
    install_dir().join("print_studio_thumbs.dll")
}

/// Put the embedded DLL next to the installed exe. A loaded DLL cannot be overwritten, but it can be
/// renamed out of the way; leftovers are removed on a later run.
fn write_thumbs_dll() -> Result<(), String> {
    let path = thumbs_dll();
    if std::fs::read(&path).is_ok_and(|d| d == THUMBS_DLL) {
        return Ok(());
    }
    if path.exists() && std::fs::remove_file(&path).is_err() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        std::fs::rename(&path, install_dir().join(format!("print_studio_thumbs.{stamp}.old"))).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, THUMBS_DLL).map_err(|e| e.to_string())
}

fn remove_old_dlls() {
    if let Ok(rd) = std::fs::read_dir(install_dir()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Previews (first page, comic page, book cover) instead of the app icon for PDF, CBZ and EPUB files.
pub fn install_thumbnails() -> Result<String, String> {
    ensure_installed()?;
    write_thumbs_dll()?;
    let clsid = format!(r"Software\Classes\CLSID\{THUMB_CLSID}");
    set(&create(&clsid)?, "", "Print Studio thumbnails")?;
    let server = create(&format!(r"{clsid}\InprocServer32"))?;
    set(&server, "", &thumbs_dll().display().to_string())?;
    set(&server, "ThreadingModel", "Apartment")?;
    for ext in EXTS {
        set(&create(&format!(r"Software\Classes\.{ext}\ShellEx\{THUMBNAIL_HANDLER}"))?, "", THUMB_CLSID)?;
        if has_key(&format!(r"Software\Classes\{}", prog_id(ext))) {
            set(&create(&format!(r"Software\Classes\{}\ShellEx\{THUMBNAIL_HANDLER}", prog_id(ext)))?, "", THUMB_CLSID)?;
        }
    }
    notify_shell();
    Ok(tr("Explorer thumbnails enabled. Files already shown may keep their old picture until Windows refreshes its thumbnail cache.").into())
}

pub fn uninstall_thumbnails() -> Result<String, String> {
    let h = hkcu();
    for ext in EXTS {
        for key in [format!(r"Software\Classes\.{ext}\ShellEx\{THUMBNAIL_HANDLER}"), format!(r"Software\Classes\{}\ShellEx\{THUMBNAIL_HANDLER}", prog_id(ext))] {
            let ours = h.open_subkey(&key).ok().and_then(|k| k.get_value::<String, _>("").ok()).is_some_and(|v| v.eq_ignore_ascii_case(THUMB_CLSID));
            if ours {
                let _ = h.delete_subkey_all(&key);
            }
        }
    }
    let _ = h.delete_subkey_all(format!(r"Software\Classes\CLSID\{THUMB_CLSID}"));
    // "Open with" entries go back to borrowing Windows' own previews
    if status().open_with {
        register_open_with()?;
    }
    notify_shell();
    let _ = std::fs::remove_file(thumbs_dll());
    Ok(tr("Explorer thumbnails disabled.").into())
}

/// After an update, the installed DLL may be older than the one in the exe.
pub fn refresh_thumbnails() {
    remove_old_dlls();
    if running_installed() && status().thumbnails {
        let _ = write_thumbs_dll();
    }
}

// ---------------------------------------------------------------- context menu

pub fn install_context_menu() -> Result<String, String> {
    let t = ensure_installed()?;
    let verbs = [
        (VERB_MAIN, tr("Print with Print Studio…"), format!("\"{t}\" \"%1\"")),
        (VERB_QUICK, tr("Quick print (last settings)"), format!("\"{t}\" --quick \"%1\"")),
    ];
    for ext in all_exts() {
        let shell = shell_key(ext);
        for (verb, label, cmd) in &verbs {
            let key = create(&format!(r"{shell}\{verb}"))?;
            set(&key, "", label)?;
            set(&key, "MUIVerb", label)?;
            set(&key, "Icon", &format!("{t},0"))?;
            set(&create(&format!(r"{shell}\{verb}\command"))?, "", cmd)?;
        }
    }
    shortcut(&send_to_link(), &installed_exe());
    notify_shell();
    Ok(tr("Context menu installed.").into())
}

pub fn uninstall_context_menu() -> Result<String, String> {
    let _ = std::fs::remove_file(send_to_link());
    for ext in all_exts() {
        for verb in [VERB_MAIN, VERB_QUICK] {
            let _ = hkcu().delete_subkey_all(format!(r"{}\{verb}", shell_key(ext)));
        }
    }
    notify_shell();
    Ok(tr("Context menu entries removed.").into())
}

// ---------------------------------------------------------------- command line

// ---------------------------------------------------------------- rename from "PDF Print Studio"

const LEGACY_ID: &str = "PdfPrintStudio";
const LEGACY_NAME: &str = "PDF Print Studio";

fn legacy_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(LEGACY_ID)
}

/// Up to 0.4.0 the app was called "PDF Print Studio". Moves its settings over and replaces an old
/// installation (reached by the self-update, which swaps the exe in the old folder) with the new one.
pub fn migrate_legacy() {
    let data = crate::settings::data_dir();
    let old_data = legacy_dir("APPDATA");
    if old_data.exists() && !data.exists() {
        let _ = std::fs::rename(&old_data, &data);
    }

    let h = hkcu();
    let old_uninstall = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{LEGACY_ID}");
    if !has_key(&old_uninstall) {
        return;
    }
    let old_prog_id = |ext: &str| format!("{LEGACY_ID}.{ext}");
    let had_open_with = has_key(&format!(r"Software\Classes\{}\shell\open\command", old_prog_id("pdf")));
    let had_context_menu = has_key(&format!(r"{}\{LEGACY_ID}\command", shell_key("pdf")));
    let had_thumbnails = status().thumbnails; // same CLSID, re-pointed at the new DLL below

    for ext in all_exts() {
        let _ = h.delete_subkey_all(format!(r"Software\Classes\{}", old_prog_id(ext)));
        if let Ok(k) = h.open_subkey_with_flags(format!(r"Software\Classes\.{ext}\OpenWithProgids"), winreg::enums::KEY_WRITE) {
            let _ = k.delete_value(old_prog_id(ext));
        }
        for verb in [LEGACY_ID.to_string(), format!("{LEGACY_ID}Quick")] {
            let _ = h.delete_subkey_all(format!(r"{}\{verb}", shell_key(ext)));
        }
    }
    let _ = h.delete_subkey_all(r"Software\Classes\Applications\pdfprintstudio.exe");
    let _ = h.delete_subkey_all(format!(r"Software\{LEGACY_ID}"));
    if let Ok(k) = h.open_subkey_with_flags(r"Software\RegisteredApplications", winreg::enums::KEY_WRITE) {
        let _ = k.delete_value(LEGACY_NAME);
    }
    let _ = h.delete_subkey_all(&old_uninstall);
    for folder in [r"Microsoft\Windows\Start Menu\Programs", r"Microsoft\Windows\SendTo"] {
        if let Some(base) = std::env::var_os("APPDATA") {
            let _ = std::fs::remove_file(PathBuf::from(base).join(folder).join(format!("{LEGACY_NAME}.lnk")));
        }
    }

    if install_program().is_ok() {
        if had_open_with {
            let _ = register_open_with();
        }
        if had_context_menu {
            let _ = install_context_menu();
        }
        if had_thumbnails {
            let _ = install_thumbnails();
        }
    }
    notify_shell();

    // the old exe may be the one running, and Explorer may hold the old thumbnail DLL: retry for a while
    let dir = legacy_dir("LOCALAPPDATA").display().to_string();
    let script = format!("/C for /L %i in (1,1,300) do @(rmdir /s /q \"{dir}\" 2>nul & if not exist \"{dir}\" (exit) else ping -n 2 127.0.0.1 >nul)");
    let _ = Command::new("cmd").raw_arg(script).creation_flags(CREATE_NO_WINDOW).spawn();
}

/// `--install`: program, "Open with" and context menu in one go.
pub fn install() -> Result<String, String> {
    install_program()?;
    register_open_with()?;
    install_context_menu()?;
    install_thumbnails()?;
    Ok(trf!(
        "Print Studio is installed.\n\nRight-click any PDF or picture → \"Print with Print Studio…\" or \"Open with\" (on Windows 11 under \"Show more options\"). To print several pictures together, select them and use \"Send to → Print Studio\".\n\nProgram location:\n{}",
        installed_exe().display()
    ))
}

/// `--uninstall` (also what "Apps & features" runs).
pub fn uninstall() -> Result<String, String> {
    uninstall_program()?;
    Ok(tr("Print Studio was removed.").into())
}
