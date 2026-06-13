use palace::miner::{chunk_text, detect_room, CHUNK_SIZE};
use palace::room_detector::Room;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn project_wing_status_unknown_for_fresh_dir() {
    use palace::miner::{project_wing_status, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    match status {
        ProjectWingStatus::Unknown {
            has_palace_yaml, ..
        } => assert!(!has_palace_yaml),
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn project_wing_status_registered_not_mined() {
    use palace::miner::{project_wing_status, wing_slug_from_dir, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let wing = wing_slug_from_dir(tmp.path());
    palace::store::ensure_wing_registered(&conn, &wing).unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    assert!(matches!(
        status,
        ProjectWingStatus::RegisteredNotMined { .. }
    ));
}

#[test]
fn project_wing_status_mined_when_drawers_present() {
    use palace::miner::{project_wing_status, wing_slug_from_dir, ProjectWingStatus};
    let conn = palace::db::open_in_memory().unwrap();
    let tmp = TempDir::new().unwrap();
    let wing = wing_slug_from_dir(tmp.path());
    palace::store::add_drawer(
        &conn, &wing, "general", "content", None, "f.txt", 0, "test", 3.0,
    )
    .unwrap();
    palace::store::ensure_wing_registered(&conn, &wing).unwrap();

    let status = project_wing_status(&conn, tmp.path()).unwrap();
    match status {
        ProjectWingStatus::Mined { drawers, .. } => assert_eq!(drawers, 1),
        other => panic!("expected Mined, got {other:?}"),
    }
}

#[test]
fn chunk_text_short_content_produces_one_chunk() {
    // Text must be >= MIN_CHUNK_SIZE (50 chars) to produce a chunk
    let text = "Hello world, this is a somewhat longer sentence that exceeds minimum size.";
    let chunks = chunk_text(text);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].1, 0);
}

#[test]
fn chunk_text_long_content_produces_multiple_chunks() {
    let text = "a".repeat(CHUNK_SIZE * 3);
    let chunks = chunk_text(&text);
    assert!(
        chunks.len() >= 2,
        "long content should produce multiple chunks"
    );
}

#[test]
fn chunk_text_respects_paragraph_breaks() {
    let text = format!("{}\n\n{}", "a".repeat(600), "b".repeat(600));
    let chunks = chunk_text(&text);
    // Should split at paragraph boundary
    assert!(chunks.len() >= 2, "should respect paragraph breaks");
}

#[test]
fn chunk_text_handles_multibyte_at_chunk_boundary() {
    // Place a 3-byte UTF-8 char ('─' = E2 94 80) so that byte index CHUNK_SIZE
    // lands strictly inside it. Without char-boundary handling, slicing panics.
    let prefix = "a".repeat(CHUNK_SIZE - 2);
    let text = format!("{prefix}─{}", "b".repeat(CHUNK_SIZE * 2));
    let chunks = chunk_text(&text);
    assert!(
        chunks.len() >= 2,
        "long multibyte content should still chunk without panicking"
    );
}

#[test]
fn chunk_text_handles_multibyte_at_overlap_rewind() {
    // After a chunk ends cleanly, the next start = cut - CHUNK_OVERLAP.
    // If that lands inside a multibyte char, the next iteration's slice panics.
    // Put '─' so its bytes straddle (CHUNK_SIZE - CHUNK_OVERLAP).
    let head = "a".repeat(CHUNK_SIZE - 100 - 1);
    let text = format!("{head}─{}", "b".repeat(CHUNK_SIZE * 2));
    let chunks = chunk_text(&text);
    assert!(
        !chunks.is_empty(),
        "overlap rewind into multibyte char must not panic"
    );
}

#[test]
fn chunk_text_indices_are_sequential() {
    let text = "w".repeat(CHUNK_SIZE * 4);
    let chunks = chunk_text(&text);
    for (i, (_, idx)) in chunks.iter().enumerate() {
        assert_eq!(*idx, i, "chunk indices should be sequential");
    }
}

#[test]
fn detect_room_uses_folder_path() {
    let rooms = vec![
        Room {
            name: "backend".into(),
            description: "backend code".into(),
            keywords: vec!["api".into()],
        },
        Room {
            name: "frontend".into(),
            description: "ui code".into(),
            keywords: vec!["ui".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/backend/server.py");
    let room = detect_room(path, "some content", &rooms, project);
    assert_eq!(room, "backend");
}

#[test]
fn detect_room_keyword_scoring() {
    let rooms = vec![
        Room {
            name: "database".into(),
            description: "db".into(),
            keywords: vec!["sql".into(), "schema".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/config.txt");
    let content = "CREATE TABLE users (id INT); ALTER TABLE schema.sql;";
    let room = detect_room(path, content, &rooms, project);
    assert_eq!(room, "database");
}

#[test]
fn detect_room_defaults_to_general() {
    let rooms = vec![
        Room {
            name: "backend".into(),
            description: "backend".into(),
            keywords: vec!["rust".into()],
        },
        Room {
            name: "general".into(),
            description: "other".into(),
            keywords: vec![],
        },
    ];
    let project = Path::new("/project");
    let path = Path::new("/project/data.csv");
    let room = detect_room(path, "some random content xyz", &rooms, project);
    assert_eq!(room, "general");
}
