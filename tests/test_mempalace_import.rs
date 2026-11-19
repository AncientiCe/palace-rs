//! Importing a Python MemPalace install: fixtures mirror the on-disk shapes
//! the Python version writes (ChromaDB, sqlite_exact, knowledge graph, tunnels).

use palace::db;
use palace::mempalace_import::{import, map_record, DrawerStore, LegacyRecord, MempalaceLayout};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::path::Path;
use tempfile::TempDir;

/// The ChromaDB 0.4+ tables the importer reads, trimmed to the used columns.
fn write_chroma(path: &Path, records: &[(&str, &str, &str, Value)]) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE segments (id TEXT PRIMARY KEY, type TEXT, scope TEXT, collection TEXT);
         CREATE TABLE embeddings (id INTEGER PRIMARY KEY, segment_id TEXT, embedding_id TEXT,
                                  seq_id BLOB, created_at TEXT);
         CREATE TABLE embedding_metadata (id INTEGER, key TEXT, string_value TEXT,
                                          int_value INTEGER, float_value REAL, bool_value INTEGER);
         INSERT INTO collections VALUES ('c1', 'mempalace_drawers'), ('c2', 'mempalace_closets');
         INSERT INTO segments VALUES ('s1', 'sqlite', 'METADATA', 'c1'),
                                     ('s2', 'sqlite', 'METADATA', 'c2');",
    )
    .unwrap();
    for (rowid, (collection, id, document, metadata)) in records.iter().enumerate() {
        let rowid = rowid as i64 + 1;
        let segment = if *collection == "closets" { "s2" } else { "s1" };
        conn.execute(
            "INSERT INTO embeddings (id, segment_id, embedding_id) VALUES (?1, ?2, ?3)",
            params![rowid, segment, id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO embedding_metadata (id, key, string_value) VALUES (?1, 'chroma:document', ?2)",
            params![rowid, document],
        )
        .unwrap();
        for (key, value) in metadata.as_object().unwrap() {
            match value {
                Value::String(text) => conn.execute(
                    "INSERT INTO embedding_metadata (id, key, string_value) VALUES (?1, ?2, ?3)",
                    params![rowid, key, text],
                ),
                Value::Number(n) if n.is_i64() => conn.execute(
                    "INSERT INTO embedding_metadata (id, key, int_value) VALUES (?1, ?2, ?3)",
                    params![rowid, key, n.as_i64()],
                ),
                Value::Number(n) => conn.execute(
                    "INSERT INTO embedding_metadata (id, key, float_value) VALUES (?1, ?2, ?3)",
                    params![rowid, key, n.as_f64()],
                ),
                _ => continue,
            }
            .unwrap();
        }
    }
}

fn write_knowledge_graph(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE entities (id TEXT PRIMARY KEY, name TEXT NOT NULL, type TEXT DEFAULT 'unknown',
                                properties TEXT DEFAULT '{}', created_at TEXT);
         CREATE TABLE triples (id TEXT PRIMARY KEY, subject TEXT NOT NULL, predicate TEXT NOT NULL,
                               object TEXT NOT NULL, valid_from TEXT, valid_to TEXT,
                               confidence REAL DEFAULT 1.0, source_closet TEXT, source_file TEXT,
                               source_drawer_id TEXT, adapter_name TEXT, extracted_at TEXT);
         INSERT INTO entities (id, name, type, properties) VALUES
             ('alice', 'Alice', 'person', '{\"role\":\"lead\"}'),
             ('api', 'API', 'project', '{}'),
             ('postgres', 'Postgres', 'tool', '{}'),
             ('mysql', 'MySQL', 'tool', '{}');
         INSERT INTO triples VALUES
             ('t1', 'alice', 'works_on', 'api', '2025-01-01', NULL, 1.0, NULL, NULL, NULL, NULL,
              '2025-01-01T00:00:00Z'),
             ('t2', 'api', 'uses', 'mysql', '2024-01-01', '2025-06-01', 0.9, NULL, 'notes.md', NULL,
              NULL, '2024-01-01T00:00:00Z'),
             ('t3', 'api', 'uses', 'postgres', '2025-06-01', NULL, 1.0, NULL, NULL, NULL, NULL,
              '2025-06-01T00:00:00Z');",
    )
    .unwrap();
}

fn python_install() -> TempDir {
    let root = TempDir::new().unwrap();
    let palace = root.path().join("palace");
    std::fs::create_dir_all(&palace).unwrap();
    write_chroma(
        &palace.join("chroma.sqlite3"),
        &[
            (
                "drawers",
                "drawer_api_decisions_abc",
                "We chose Postgres over MySQL for the API because of JSONB support.",
                json!({"wing": "api", "room": "decisions", "added_by": "claude",
                       "filed_at": "2025-06-01T10:00:00+00:00", "importance": 5}),
            ),
            (
                "drawers",
                "drawer_api_backend_def",
                "fn main() { serve_api(); }",
                json!({"wing": "api", "room": "backend", "source_file": "/repo/src/main.rs",
                       "chunk_index": 2, "added_by": "miner",
                       "filed_at": "2025-05-01T09:00:00+00:00"}),
            ),
            (
                "drawers",
                "diary_wing_claude_20250601_abc",
                "Fixed the flaky login test by waiting for the session cookie.",
                json!({"wing": "wing_claude", "room": "diary", "hall": "hall_diary",
                       "topic": "testing", "type": "diary_entry", "agent": "claude",
                       "date": "2025-06-02", "filed_at": "2025-06-02T08:00:00+00:00"}),
            ),
            (
                "drawers",
                "drawer_empty",
                "   ",
                json!({"wing": "api", "room": "x"}),
            ),
            (
                "closets",
                "closet_1",
                "decisions: postgres",
                json!({"wing": "api", "room": "decisions"}),
            ),
        ],
    );
    write_knowledge_graph(&root.path().join("knowledge_graph.sqlite3"));
    std::fs::write(
        root.path().join("tunnels.json"),
        json!([{
            "id": "tun_1",
            "source": {"wing": "api", "room": "decisions"},
            "target": {"wing": "web", "room": "decisions"},
            "label": "shared stack",
            "kind": "explicit",
            "created_at": "2025-06-01T00:00:00Z"
        }])
        .to_string(),
    )
    .unwrap();
    root
}

#[test]
fn detects_python_install_from_state_dir_and_palace_dir() {
    let root = python_install();
    let from_root = MempalaceLayout::detect(root.path()).expect("layout");
    assert_eq!(
        from_root.drawers,
        Some(DrawerStore::Chroma(
            root.path().join("palace").join("chroma.sqlite3")
        ))
    );
    assert_eq!(
        from_root.knowledge_graph,
        Some(root.path().join("knowledge_graph.sqlite3"))
    );
    assert_eq!(from_root.tunnels, Some(root.path().join("tunnels.json")));
    assert_eq!(
        MempalaceLayout::detect(&root.path().join("palace")),
        Some(from_root)
    );
}

#[test]
fn does_not_detect_a_palace_rs_directory() {
    let root = TempDir::new().unwrap();
    std::fs::create_dir_all(root.path().join("palace")).unwrap();
    std::fs::write(root.path().join("palace").join("palace.db"), b"").unwrap();
    assert_eq!(MempalaceLayout::detect(root.path()), None);
}

#[test]
fn imports_drawers_diaries_graph_and_tunnels() {
    let root = python_install();
    let layout = MempalaceLayout::detect(root.path()).unwrap();
    let conn = db::open_in_memory().unwrap();

    let report = import(&layout, &conn).unwrap();
    assert_eq!(report.drawers, 2);
    assert_eq!(report.diary_entries, 1);
    assert_eq!(report.drawers_skipped, 1, "the empty record");
    assert_eq!(report.entities, 4);
    assert_eq!(report.triples, 3);
    assert_eq!(report.tunnels, 1);

    let closets: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM drawers WHERE content LIKE 'decisions: %'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(closets, 0, "closets are not drawers");

    let (wing, room, filed_at, importance, added_by, metadata): (
        String,
        String,
        String,
        f64,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT wing, room, filed_at, importance, added_by, metadata
             FROM drawers WHERE id = 'drawer_api_decisions_abc'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!((wing.as_str(), room.as_str()), ("api", "decisions"));
    assert_eq!(filed_at, "2025-06-01T10:00:00+00:00");
    assert_eq!(importance, 5.0);
    assert_eq!(added_by, "claude");
    let metadata: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["migrated_from"], "mempalace");

    let mined_id = palace::store::drawer_id("api", "backend", "/repo/src/main.rs", 2);
    let chunk: i64 = conn
        .query_row(
            "SELECT chunk_index FROM drawers WHERE id = ?1",
            [&mined_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(chunk, 2, "mined chunks get palace's deterministic id");

    let (diary_wing, hall, diary_meta): (String, String, String) = conn
        .query_row(
            "SELECT wing, hall, metadata FROM drawers WHERE id = 'diary_wing_claude_20250601_abc'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(diary_wing, "wing_diary__claude");
    assert_eq!(hall, "hall_diary");
    let diary_meta: Value = serde_json::from_str(&diary_meta).unwrap();
    assert_eq!(diary_meta["topic"], "testing");
    assert_eq!(diary_meta["agent"], "claude");

    let alice_type: String = conn
        .query_row("SELECT type FROM entities WHERE name = 'Alice'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(alice_type, "person");
    let expired: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM triples WHERE predicate = 'uses' AND valid_to = '2025-06-01'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(expired, 1, "fact history is kept");

    let wings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM wings WHERE name IN ('api', 'wing_diary__claude')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(wings, 2, "imported wings are registered");
}

#[test]
fn rerunning_an_import_copies_nothing_twice() {
    let root = python_install();
    let layout = MempalaceLayout::detect(root.path()).unwrap();
    let conn = db::open_in_memory().unwrap();
    import(&layout, &conn).unwrap();

    let again = import(&layout, &conn).unwrap();
    assert_eq!(again.drawers + again.diary_entries, 0);
    assert_eq!(again.triples, 0);
    assert_eq!(again.entities, 0);
    assert_eq!(again.tunnels, 0);
    let drawers: i64 = conn
        .query_row("SELECT COUNT(*) FROM drawers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(drawers, 3);
}

#[test]
fn imports_the_sqlite_exact_backend() {
    let root = TempDir::new().unwrap();
    let palace = root.path().join("palace");
    std::fs::create_dir_all(&palace).unwrap();
    let conn = Connection::open(palace.join("sqlite_exact.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TABLE collections (id INTEGER PRIMARY KEY, name TEXT, dimension INTEGER, created_at TEXT);
         CREATE TABLE documents (collection_id INTEGER, id TEXT, document TEXT, metadata_json TEXT,
                                 embedding BLOB, dim INTEGER, created_at TEXT, updated_at TEXT);
         INSERT INTO collections VALUES (1, 'mempalace_drawers', 384, '2025-01-01');
         INSERT INTO documents VALUES (1, 'd1', 'Deploys go through the staging gate first.',
             '{\"wing\":\"ops\",\"room\":\"runbooks\",\"filed_at\":\"2025-03-01T00:00:00+00:00\"}',
             x'00', 384, '2025-03-01', '2025-03-01');",
    )
    .unwrap();
    drop(conn);

    let layout = MempalaceLayout::detect(root.path()).unwrap();
    assert!(matches!(layout.drawers, Some(DrawerStore::SqliteExact(_))));
    let dest = db::open_in_memory().unwrap();
    let report = import(&layout, &dest).unwrap();
    assert_eq!(report.drawers, 1);
    let wing: String = dest
        .query_row("SELECT wing FROM drawers WHERE id = 'd1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(wing, "ops");
}

#[test]
fn diary_without_agent_metadata_takes_agent_from_wing() {
    let mut metadata = Map::new();
    metadata.insert("wing".into(), json!("wing_codex"));
    metadata.insert("room".into(), json!("diary"));
    let mapped = map_record(&LegacyRecord {
        id: "diary_x".into(),
        document: "entry".into(),
        metadata,
    })
    .unwrap();
    assert!(mapped.is_diary);
    assert_eq!(mapped.wing, "wing_diary__codex");
    assert_eq!(mapped.added_by, "mempalace");
}
