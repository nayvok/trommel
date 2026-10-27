//! Tiny runtime localization layer.
//!
//! The app ships with English source strings and Russian translations. A
//! gettext catalogue can replace this later without changing call sites; the
//! `tr(en, ru)` helper keeps the UI bilingual now and is intentionally simple.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    System,
    En,
    Ru,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::En => "English",
            Self::Ru => "Русский",
        }
    }

    pub fn resolve(self) -> Self {
        match self {
            Self::System => {
                let value = std::env::var("LANGUAGE")
                    .or_else(|_| std::env::var("LANG"))
                    .or_else(|_| std::env::var("LC_ALL"))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if value.starts_with("ru") {
                    Self::Ru
                } else {
                    Self::En
                }
            }
            other => other,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

fn code(language: Language) -> u8 {
    match language {
        Language::System => 0,
        Language::En => 1,
        Language::Ru => 2,
    }
}

fn from_code(value: u8) -> Language {
    match value {
        1 => Language::En,
        2 => Language::Ru,
        _ => Language::System,
    }
}

pub fn set_language(language: Language) {
    CURRENT.store(code(language), Ordering::Relaxed);
}

pub fn current() -> Language {
    from_code(CURRENT.load(Ordering::Relaxed)).resolve()
}

pub fn is_ru() -> bool {
    current() == Language::Ru
}

/// Translate a pair of English/Russian strings.
pub fn tr(en: &str, ru: &str) -> String {
    if is_ru() {
        ru.to_string()
    } else {
        en.to_string()
    }
}

/// Borrowed variant for static labels.
pub fn tr_ref<'a>(en: &'a str, ru: &'a str) -> &'a str {
    if is_ru() {
        ru
    } else {
        en
    }
}
