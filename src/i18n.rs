//! Locale metadata for entity detection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Locale {
    pub code: String,
    pub candidate_pattern: String,
    pub boundary_chars: Option<String>,
}

const SUPPORTED: &[(&str, &str, Option<&str>)] = &[
    ("en", r"[A-Za-z]", None),
    ("es", r"[A-Za-zÁÉÍÓÚÜÑáéíóúüñ]", None),
    ("fr", r"[A-Za-zÀ-ÿ]", None),
    ("de", r"[A-Za-zÄÖÜäöüß]", None),
    ("ja", r"[\p{Hiragana}\p{Katakana}\p{Han}]", None),
    ("ko", r"[\p{Hangul}]", None),
    ("zh-cn", r"[\p{Han}]", None),
    ("zh-tw", r"[\p{Han}]", None),
    ("pt-br", r"[A-Za-zÀ-ÿ]", None),
    ("ru", r"[\p{Cyrillic}]", None),
    ("it", r"[A-Za-zÀ-ÿ]", None),
    ("hi", r"[\p{Devanagari}]", Some("combining")),
    ("id", r"[A-Za-z]", None),
    ("be", r"[\p{Cyrillic}]", None),
];

pub fn canonical_language(code: &str) -> Option<String> {
    let code = code.trim().to_lowercase().replace('_', "-");
    SUPPORTED
        .iter()
        .find(|(supported, _, _)| *supported == code)
        .map(|(supported, _, _)| (*supported).to_string())
}

pub fn load_locale(code: &str) -> Option<Locale> {
    let canonical = canonical_language(code)?;
    SUPPORTED
        .iter()
        .find(|(supported, _, _)| *supported == canonical)
        .map(|(code, pattern, boundary)| Locale {
            code: (*code).to_string(),
            candidate_pattern: (*pattern).to_string(),
            boundary_chars: boundary.map(str::to_string),
        })
}

pub fn supported_languages() -> Vec<String> {
    SUPPORTED
        .iter()
        .map(|(code, _, _)| (*code).to_string())
        .collect()
}
