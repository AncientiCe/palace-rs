use mempalace::config::MempalaceConfig;
use mempalace::i18n::{canonical_language, load_locale, supported_languages};

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
    std::env::set_var("MEMPALACE_ENTITY_LANGUAGES", "PT-BR,zh_CN");
    let config = MempalaceConfig::new();
    assert_eq!(config.entity_languages(), vec!["pt-br", "zh-cn"]);
    std::env::remove_var("MEMPALACE_ENTITY_LANGUAGES");
}
