use palace::db;
use palace::store::*;
use rusqlite::Connection;

fn open_test_db() -> rusqlite::Connection {
    db::open_in_memory().expect("in-memory DB should open")
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table info query should prepare");
    stmt.query_map([], |row| row.get::<_, String>(1))
        .expect("table info query should run")
        .collect::<Result<Vec<_>, _>>()
        .expect("table info rows should parse")
}

#[test]
fn schema_has_phase_one_drawer_columns_and_tables() {
    let conn = open_test_db();

    let drawer_columns = table_columns(&conn, "drawers");
    for column in [
        "created_at",
        "entity_metadata",
        "hall",
        "normalize_version",
        "metadata",
        "pref_embedding",
    ] {
        assert!(
            drawer_columns.iter().any(|c| c == column),
            "drawers should include {column}"
        );
    }

    for table in ["meta", "closets", "tunnels", "bm25_terms", "bm25_doc_stats"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .expect("sqlite_master query should work");
        assert_eq!(exists, 1, "{table} table should exist");
    }
}

#[test]
fn opening_v3_schema_migrates_phase_one_columns() {
    let tmp = tempfile::tempdir().expect("tempdir should create");
    let db_path = tmp.path().join("palace.db");
    {
        let conn = Connection::open(&db_path).expect("legacy DB should open");
        conn.execute_batch(
            r#"
            CREATE TABLE drawers (
                id          TEXT PRIMARY KEY,
                wing        TEXT NOT NULL,
                room        TEXT NOT NULL,
                content     TEXT NOT NULL,
                embedding   BLOB,
                source_file TEXT NOT NULL DEFAULT '',
                chunk_index INTEGER NOT NULL DEFAULT 0,
                added_by    TEXT NOT NULL DEFAULT 'mempalace',
                filed_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                importance  REAL NOT NULL DEFAULT 3.0
            );
            "#,
        )
        .expect("legacy schema should create");
    }

    let conn = db::open(&db_path).expect("migration should succeed");
    let drawer_columns = table_columns(&conn, "drawers");
    assert!(drawer_columns.iter().any(|c| c == "metadata"));
    assert!(drawer_columns.iter().any(|c| c == "normalize_version"));

    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .expect("schema version should be stored");
    assert_eq!(version, "1");
}

#[test]
fn add_and_get_drawer() {
    let conn = open_test_db();
    let (added, id) = add_drawer(
        &conn,
        "wing_code",
        "backend",
        "Hello world",
        None,
        "test.py",
        0,
        "test",
        3.0,
    )
    .unwrap();
    assert!(added, "should be added");
    assert!(id.starts_with("drawer_wing_code_backend_"));

    let drawer = get_drawer(&conn, &id).unwrap().expect("should find drawer");
    assert_eq!(drawer.wing, "wing_code");
    assert_eq!(drawer.room, "backend");
    assert_eq!(drawer.content, "Hello world");
}

#[test]
fn add_drawer_records_hall_and_entity_metadata() {
    let conn = open_test_db();
    let (_, id) = add_drawer(
        &conn,
        "wing_code",
        "backend",
        "Alice fixed the Rust database error in MemPalace",
        None,
        "test.py",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let drawer = get_drawer(&conn, &id)
        .unwrap()
        .expect("drawer should exist");
    assert_eq!(drawer.hall.as_deref(), Some("technical"));
    assert!(drawer.entity_metadata["entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["name"] == "Alice"));
}

#[test]
fn add_drawer_with_id_stores_metadata_separately_from_source_file() {
    let conn = open_test_db();
    let meta = serde_json::json!({
        "hall": "technical",
        "topic": "schema",
        "agent": "palace"
    });

    let added = add_drawer_with_id(
        &conn,
        "drawer_meta",
        "wing_code",
        "backend",
        "metadata should not be hidden in source_file",
        None,
        "source.rs",
        "test",
        Some(&meta),
    )
    .unwrap();
    assert!(added);

    let drawer = get_drawer(&conn, "drawer_meta")
        .unwrap()
        .expect("drawer should exist");
    assert_eq!(drawer.source_file, "source.rs");
    assert_eq!(drawer.metadata["topic"], "schema");
    assert_eq!(drawer.hall.as_deref(), Some("technical"));
}

#[test]
fn add_drawer_with_id_tags_preferences_without_dropping_metadata() {
    let conn = open_test_db();
    let meta = serde_json::json!({
        "topic": "api",
        "agent": "codex"
    });

    let added = add_drawer_with_id(
        &conn,
        "drawer_preference_meta",
        "wing_code",
        "preferences",
        "I prefer small public APIs routed through the Palace facade.",
        None,
        "session.md",
        "test",
        Some(&meta),
    )
    .unwrap();
    assert!(added);

    let drawer = get_drawer(&conn, "drawer_preference_meta")
        .unwrap()
        .expect("drawer should exist");
    assert_eq!(drawer.metadata["topic"], "api");
    assert_eq!(drawer.metadata["agent"], "codex");
    assert_eq!(drawer.metadata["preference"], true);
    assert_eq!(
        drawer.metadata["preference_span"],
        "I prefer small public APIs routed through the Palace facade."
    );
}

#[test]
fn update_drawer_content_refreshes_preference_tag() {
    let conn = open_test_db();
    let (_, id) = add_drawer(
        &conn,
        "wing_code",
        "backend",
        "Neutral implementation detail.",
        None,
        "session.md",
        0,
        "test",
        3.0,
    )
    .unwrap();

    update_drawer_content(
        &conn,
        &id,
        "My convention is to keep search results source-grounded.",
    )
    .unwrap();
    let drawer = get_drawer(&conn, &id)
        .unwrap()
        .expect("drawer should exist after preference update");
    assert_eq!(drawer.metadata["preference"], true);

    update_drawer_content(&conn, &id, "Neutral implementation detail again.").unwrap();
    let drawer = get_drawer(&conn, &id)
        .unwrap()
        .expect("drawer should exist after neutral update");
    assert!(drawer.metadata.get("preference").is_none());
}

#[test]
fn duplicate_add_returns_false() {
    let conn = open_test_db();
    let (added, _) = add_drawer(
        &conn,
        "wing_code",
        "backend",
        "Hello",
        None,
        "dup.py",
        0,
        "test",
        3.0,
    )
    .unwrap();
    assert!(added);
    let (added2, _) = add_drawer(
        &conn,
        "wing_code",
        "backend",
        "Hello",
        None,
        "dup.py",
        0,
        "test",
        3.0,
    )
    .unwrap();
    assert!(!added2, "duplicate should not be added");
}

#[test]
fn delete_drawer_works() {
    let conn = open_test_db();
    let (_, id) = add_drawer(
        &conn,
        "wing_a",
        "room_a",
        "delete me",
        None,
        "x.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    let deleted = delete_drawer(&conn, &id).unwrap();
    assert!(deleted);
    assert!(get_drawer(&conn, &id).unwrap().is_none());
}

#[test]
fn wing_counts_aggregation() {
    let conn = open_test_db();
    add_drawer(&conn, "wing_a", "r1", "a1", None, "a1.txt", 0, "test", 3.0).unwrap();
    add_drawer(&conn, "wing_a", "r2", "a2", None, "a2.txt", 0, "test", 3.0).unwrap();
    add_drawer(&conn, "wing_b", "r1", "b1", None, "b1.txt", 0, "test", 3.0).unwrap();

    let wings = wing_counts(&conn).unwrap();
    assert_eq!(wings["wing_a"], 2);
    assert_eq!(wings["wing_b"], 1);
}

#[test]
fn file_already_mined_check() {
    let conn = open_test_db();
    assert!(!file_already_mined(&conn, "unique_file.txt").unwrap());
    add_drawer(
        &conn,
        "wing_x",
        "room_x",
        "content",
        None,
        "unique_file.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    assert!(file_already_mined(&conn, "unique_file.txt").unwrap());
}

#[test]
fn vector_search_returns_results() {
    let conn = open_test_db();

    // Use random-ish f32 vectors (same dimension as real embeddings)
    let v1: Vec<f32> = (0..384).map(|i| (i as f32).sin()).collect();
    let v2: Vec<f32> = (0..384).map(|i| (i as f32).cos()).collect();
    let v3: Vec<f32> = (0..384).map(|i| -(i as f32).sin()).collect();

    add_drawer(
        &conn,
        "w",
        "r",
        "first doc",
        Some(&v1),
        "a.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "w",
        "r",
        "second doc",
        Some(&v2),
        "b.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "w",
        "r",
        "opposite doc",
        Some(&v3),
        "c.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();

    // Search with v1 — "first doc" should rank highest
    let filter = DrawerFilter::default();
    let results = vector_search(&conn, &v1, &filter, 3).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].text, "first doc",
        "first doc should be best match"
    );
}

#[test]
fn source_context_returns_adjacent_chunks() {
    let conn = open_test_db();
    for idx in 0..4 {
        add_drawer(
            &conn,
            "wing",
            "room",
            &format!("chunk {idx} content"),
            None,
            "shared.txt",
            idx,
            "test",
            3.0,
        )
        .unwrap();
    }

    let context = source_context(&conn, "shared.txt", 2, 1).unwrap();
    let chunks: Vec<_> = context.iter().map(|drawer| drawer.chunk_index).collect();
    assert_eq!(chunks, vec![1, 2, 3]);
}

#[test]
fn taxonomy_works() {
    let conn = open_test_db();
    add_drawer(
        &conn,
        "wing_code",
        "backend",
        "c1",
        None,
        "f1",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "wing_code",
        "frontend",
        "c2",
        None,
        "f2",
        0,
        "test",
        3.0,
    )
    .unwrap();
    add_drawer(
        &conn,
        "wing_docs",
        "readme",
        "c3",
        None,
        "f3",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let tax = taxonomy(&conn).unwrap();
    assert_eq!(tax["wing_code"]["backend"], 1);
    assert_eq!(tax["wing_code"]["frontend"], 1);
    assert_eq!(tax["wing_docs"]["readme"], 1);
}
