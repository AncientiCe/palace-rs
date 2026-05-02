use mempalace::entity_detector::{detect_entities, EntityKind};
use mempalace::hall_router::detect_hall;
use mempalace::manifest_entities::detect_manifest_projects;
use mempalace::origin::{detect_origin, OriginPlatform};

#[test]
fn entity_detector_handles_camelcase_and_dialogue_names() {
    let entities = detect_entities("Alice: We should ship MemPalace with ChromaDB support.");

    assert!(entities
        .iter()
        .any(|entity| entity.name == "Alice" && entity.kind == EntityKind::Person));
    assert!(entities.iter().any(|entity| entity.name == "MemPalace"));
    assert!(entities.iter().any(|entity| entity.name == "ChromaDB"));
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
