use palace::entity_detector::{detect_entities, EntityKind};
use palace::hall_router::detect_hall;
use palace::manifest_entities::detect_manifest_projects;
use palace::origin::{detect_origin, OriginPlatform};

#[test]
fn entity_detector_handles_camelcase_and_dialogue_names() {
    let entities = detect_entities("Alice: We should ship Palace with ChromaDB support.");

    assert!(entities
        .iter()
        .any(|entity| entity.name == "Alice" && entity.kind == EntityKind::Person));
    assert!(entities.iter().any(|entity| entity.name == "Palace"));
    assert!(entities.iter().any(|entity| entity.name == "ChromaDB"));
}

#[test]
fn status_words_and_sentence_starts_are_not_entities() {
    let entities = detect_entities(
        "DONE (2026-11-02): pushed to main. Still open. ALSO DONE: the follow-up. \
         Does NOT handle quoted lines. Check Windows and release builds. \
         Possible follow-ups later. Yes, switch it. Pushed to main.\n\
         Decision: add it as an opt-in feature.\nConsumer: aice would enable it.\n\
         - Pushed again later.",
    );
    let names: Vec<&str> = entities.iter().map(|entity| entity.name.as_str()).collect();

    assert_eq!(names, vec!["Windows"]);
}

#[test]
fn names_are_found_inside_sentences_and_as_pairs() {
    let entities = detect_entities(
        "Alice Smith moved the team to Lisbon. Lisbon was the idea of Bob, and the DPIA \
         asks for it. Still, Mem0 runs its own backend.",
    );
    let kind = |name: &str| {
        entities
            .iter()
            .find(|entity| entity.name == name)
            .map(|entity| entity.kind.clone())
    };

    assert_eq!(kind("Alice Smith"), Some(EntityKind::Person));
    assert_eq!(kind("Lisbon"), Some(EntityKind::Topic));
    assert_eq!(kind("Bob"), Some(EntityKind::Topic));
    assert_eq!(kind("DPIA"), Some(EntityKind::Topic));
    assert_eq!(kind("Still"), None);
}

#[test]
fn hall_router_classifies_technical_content() {
    let hall = detect_hall("Rust code failed with a SQLite database error.");
    assert_eq!(hall, "technical");
}

#[test]
fn manifest_detector_reads_cargo_package_name() {
    let tmp = tempfile::tempdir().expect("tempdir should create");
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"memory-palace\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest should write");

    let projects = detect_manifest_projects(tmp.path()).expect("manifest detection should work");
    assert!(projects.iter().any(|project| project == "memory-palace"));
}

#[test]
fn origin_detector_recognizes_ai_dialogue() {
    let origin = detect_origin("User: hello\nClaude: I can help with that.\nAssistant: done");
    assert_eq!(origin.platform, OriginPlatform::AiDialogue);
    assert!(origin.agent_persona_names.contains(&"Claude".to_string()));
}
