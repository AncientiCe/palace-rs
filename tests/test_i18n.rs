use palace::config::PalaceConfig;
use palace::i18n::{canonical_language, load_locale, supported_languages};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn language_codes_are_canonicalized_case_insensitively() {
    assert_eq!(canonical_language("PT-BR").as_deref(), Some("pt-br"));
    assert_eq!(canonical_language("zh_CN").as_deref(), Some("zh-cn"));
}

#[test]
fn locale_loader_contains_non_latin_candidate_patterns() {
    let locale = load_locale("hi").expect("Hindi locale should load");
    assert!(locale.candidate_pattern.contains("Devanagari"));
    assert!(locale.boundary_chars.is_some());
    assert!(supported_languages().len() >= 14);
}

#[test]
fn config_reads_entity_language_env_override() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::remove_var("MEMPALACE_ENTITY_LANGUAGES");
    std::env::set_var("PALACE_ENTITY_LANGUAGES", "PT-BR,zh_CN");
    let config = PalaceConfig::new();
    let result = config.entity_languages();
    std::env::remove_var("PALACE_ENTITY_LANGUAGES");
    assert_eq!(result, vec!["pt-br", "zh-cn"]);
}

#[test]
fn legacy_entity_language_env_var_still_works() {
    let _lock = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PALACE_ENTITY_LANGUAGES");
    std::env::set_var("MEMPALACE_ENTITY_LANGUAGES", "PT-BR,zh_CN");
    let config = PalaceConfig::new();
    let result = config.entity_languages();
    std::env::remove_var("MEMPALACE_ENTITY_LANGUAGES");
    assert_eq!(result, vec!["pt-br", "zh-cn"]);
}
