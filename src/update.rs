//! Self-update from the latest GitHub release: the window checks the version, downloads the attached
//! .exe and swaps it in place of the running one (Windows allows renaming a running exe).

use crate::i18n::tr;
use crate::theme::pal;
use crate::trf;
use eframe::egui::{self, Align2, RichText};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// GitHub "owner/repository" whose releases carry the Windows binary.
pub const REPO: &str = "mberehulka/pdf-print-studio";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const USER_AGENT: &str = concat!("pdf-print-studio/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// Download URL and size of the .exe asset.
    pub exe: Option<(String, u64)>,
}

#[derive(Clone)]
enum State {
    Checking,
    UpToDate,
    Available(Release),
    Downloading { done: u64, total: u64 },
    Ready(String),
    Failed(String, Option<Release>),
}

pub struct Updater {
    pub open: bool,
    state: Arc<Mutex<State>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self { open: false, state: Arc::new(Mutex::new(State::Checking)) }
    }
}

/// "v1.2.10" → [1, 2, 10]
fn version_parts(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    version_parts(candidate) > version_parts(current)
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(timeout)
        .http_status_as_error(false)
        .build()
        .into()
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent(Some(Duration::from_secs(30)))
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Err(tr("No release published yet.").into()),
        s => return Err(format!("GitHub HTTP {s}")),
    }
    let r: GhRelease = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let exe = r
        .assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".exe"))
        .max_by_key(|a| a.name.eq_ignore_ascii_case("pdfprintstudio.exe"))
        .map(|a| (a.browser_download_url.clone(), a.size));
    Ok(Release {
        version: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
        notes: r.body.unwrap_or_default().trim().to_string(),
        page: r.html_url,
        exe,
    })
}

fn sibling(ext: &str) -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let other = exe.with_extension(ext);
    Ok((exe, other))
}

/// Downloads next to the running exe, then swaps the files.
fn download(url: &str, size: u64, state: &Arc<Mutex<State>>, ctx: &egui::Context) -> Result<(), String> {
    let (exe, new) = sibling("exe.new")?;
    let mut resp = agent(None).get(url).header("User-Agent", USER_AGENT).call().map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let total = resp.body().content_length().unwrap_or(size);
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&new).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut head = Vec::new();
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if head.len() < 2 {
            head.extend_from_slice(&buf[..n.min(2)]);
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        *state.lock().unwrap() = State::Downloading { done, total };
        ctx.request_repaint();
    }
    drop(file);
    if !head.starts_with(b"MZ") || (total > 0 && done != total) {
        let _ = std::fs::remove_file(&new);
        return Err("downloaded file is not a complete Windows program".into());
    }
    let (_, old) = sibling("exe.old")?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    Ok(())
}

/// Removes the previous exe left behind by an update.
pub fn cleanup() {
    if let Ok((_, old)) = sibling("exe.old") {
        let _ = std::fs::remove_file(old);
    }
}

impl Updater {
    pub fn check(&mut self, ctx: &egui::Context) {
        self.open = true;
        *self.state.lock().unwrap() = State::Checking;
        let (state, ctx) = (self.state.clone(), ctx.clone());
        std::thread::spawn(move || {
            let s = match fetch_latest() {
                Ok(r) if is_newer(&r.version, VERSION) => State::Available(r),
                Ok(_) => State::UpToDate,
                Err(e) => State::Failed(e, None),
            };
            *state.lock().unwrap() = s;
            ctx.request_repaint();
        });
    }

    fn start_download(&self, release: Release, ctx: &egui::Context) {
        let Some((url, size)) = release.exe.clone() else { return };
        *self.state.lock().unwrap() = State::Downloading { done: 0, total: size };
        let (state, ctx) = (self.state.clone(), ctx.clone());
        std::thread::spawn(move || {
            let s = match download(&url, size, &state, &ctx) {
                Ok(()) => State::Ready(release.version.clone()),
                Err(e) => State::Failed(e, Some(release)),
            };
            *state.lock().unwrap() = s;
            ctx.request_repaint();
        });
    }

    /// Draws the update window. `reopen` is passed to the restarted program.
    pub fn show(&mut self, ctx: &egui::Context, reopen: Option<&PathBuf>) {
        if !self.open {
            return;
        }
        let state = self.state.lock().unwrap().clone();
        let mut open = self.open;
        let mut close = false;
        egui::Window::new(tr("🔄 Update"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(420.0);
                match &state {
                    State::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tr("Checking GitHub for a new version…"));
                        });
                    }
                    State::UpToDate => {
                        ui.label(RichText::new(trf!("You are using the latest version ({}).", VERSION)).color(pal().ok));
                        if ui.button(tr("Close")).clicked() {
                            close = true;
                        }
                    }
                    State::Available(r) => {
                        ui.label(RichText::new(trf!("Version {} is available (you have {}).", r.version, VERSION)).strong());
                        if !r.notes.is_empty() {
                            ui.add_space(4.0);
                            ui.label(RichText::new(tr("What's new")).strong());
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                ui.label(r.notes.as_str());
                            });
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if r.exe.is_some() {
                                if ui.button(RichText::new(tr("Download & install")).strong()).clicked() {
                                    self.start_download(r.clone(), ctx);
                                }
                            } else {
                                ui.label(RichText::new(tr("The latest release has no Windows .exe attached.")).color(pal().warn));
                            }
                            if ui.button(tr("Open release page")).clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                            }
                            if ui.button(tr("Later")).clicked() {
                                close = true;
                            }
                        });
                    }
                    State::Downloading { done, total } => {
                        let mb = |b: u64| b as f64 / 1_048_576.0;
                        let label = if *total > 0 { trf!("Downloading… {:.1} / {:.1} MB", mb(*done), mb(*total)) } else { tr("Downloading…").into() };
                        ui.label(label);
                        let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(frac).show_percentage().animate(true));
                    }
                    State::Ready(v) => {
                        ui.label(RichText::new(trf!("Update installed. Restart to use version {}.", v)).color(pal().ok));
                        ui.horizontal(|ui| {
                            if ui.button(RichText::new(tr("Restart now")).strong()).clicked() {
                                if let Ok(exe) = std::env::current_exe() {
                                    let mut cmd = std::process::Command::new(exe);
                                    if let Some(p) = reopen {
                                        cmd.arg(p);
                                    }
                                    if cmd.spawn().is_ok() {
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                    }
                                }
                            }
                            if ui.button(tr("Later")).clicked() {
                                close = true;
                            }
                        });
                    }
                    State::Failed(e, release) => {
                        ui.label(RichText::new(trf!("Update failed: {}", e)).color(pal().err));
                        ui.horizontal(|ui| {
                            if ui.button(tr("Retry")).clicked() {
                                match release {
                                    Some(r) => self.start_download(r.clone(), ctx),
                                    None => self.check(ctx),
                                }
                            }
                            if let Some(r) = release {
                                if ui.button(tr("Open release page")).clicked() {
                                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                                }
                            }
                            if ui.button(tr("Close")).clicked() {
                                close = true;
                            }
                        });
                    }
                }
            });
        // closing mid-download only hides the window; the download keeps going
        self.open = open && !close;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
    }
}
