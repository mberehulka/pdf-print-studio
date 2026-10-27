//! Application preferences (language, theme, window), kept apart from the print settings and presets.

use crate::i18n::{self, Lang};
use crate::settings::data_dir;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum LangPref {
    System,
    Fixed(Lang),
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Theme {
    Light,
    Dark,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Prefs {
    pub language: LangPref,
    pub theme: Theme,
    pub start_maximized: bool,
    /// Inner size in points.
    pub window_size: [f32; 2],
    /// Outer top-left corner in points; `None` = centred on the screen.
    pub window_pos: Option<[f32; 2]>,
    pub show_rulers: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            language: LangPref::System,
            theme: Theme::Light,
            start_maximized: true,
            window_size: [1560.0, 940.0],
            window_pos: None,
            show_rulers: true,
        }
    }
}

impl Prefs {
    pub fn load() -> Self {
        std::fs::read_to_string(data_dir().join("prefs.json"))
            .ok()
            .and_then(|d| serde_json::from_str(d.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(data_dir().join("prefs.json"), json);
        }
    }

    pub fn lang(&self) -> Lang {
        match self.language {
            LangPref::System => i18n::system_lang(),
            LangPref::Fixed(l) => l,
        }
    }

    /// Make the chosen language current.
    pub fn apply_lang(&self) {
        i18n::set(self.lang());
    }
}
