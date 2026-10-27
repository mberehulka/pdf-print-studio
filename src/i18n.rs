//! UI translations. The English text is the key; anything missing from a table falls back to English.

mod pt_br;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Lang {
    English,
    PortugueseBr,
}

impl Lang {
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::PortugueseBr => "Português (Brasil)",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::PortugueseBr,
        _ => Lang::English,
    }
}

/// Language of the Windows user locale ("Region → Regional format"); Portuguese locales get pt-BR.
pub fn system_lang() -> Lang {
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
    let n = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buf) };
    let name = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]).to_lowercase();
    if name.starts_with("pt") {
        Lang::PortugueseBr
    } else {
        Lang::English
    }
}

fn table(lang: Lang) -> Option<&'static HashMap<&'static str, &'static str>> {
    static PT_BR: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    match lang {
        Lang::English => None,
        Lang::PortugueseBr => Some(PT_BR.get_or_init(|| pt_br::ENTRIES.iter().copied().collect())),
    }
}

/// Translate a UI string into the current language.
pub fn tr(en: &str) -> &str {
    table(current()).and_then(|t| t.get(en).copied()).unwrap_or(en)
}

/// Fill the `{}` / `{:.N}` placeholders of an (already translated) text in order.
pub fn fill(text: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut args = args.iter();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find('}').unwrap_or(0);
        let spec = &tail[1..end.max(1)];
        let precision = spec.strip_prefix(":.").and_then(|p| p.parse::<usize>().ok());
        if end > 0 && (spec.is_empty() || precision.is_some()) {
            if let Some(a) = args.next() {
                match precision {
                    Some(p) => out.push_str(&format!("{a:.p$}")),
                    None => out.push_str(&a.to_string()),
                }
            }
            rest = &tail[end + 1..];
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `trf!("Page {} of {}", a, b)`: translate, then fill the placeholders.
#[macro_export]
macro_rules! trf {
    ($en:expr $(, $a:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($en), &[$(&$a as &dyn std::fmt::Display),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_placeholders() {
        assert_eq!(fill("Page {} of {}", &[&3, &"x"]), "Page 3 of x");
        assert_eq!(fill("{:.1} mm, {:.0}%", &[&2.345_f32, &99.6_f32]), "2.3 mm, 100%");
        assert_eq!(fill("{page} stays", &[&1]), "{page} stays");
    }

    #[test]
    fn every_entry_keeps_placeholders() {
        let count = |s: &str| s.matches("{}").count() + s.matches("{:.").count();
        for (en, pt) in pt_br::ENTRIES {
            assert_eq!(count(en), count(pt), "placeholder mismatch: {en}");
        }
    }
}
