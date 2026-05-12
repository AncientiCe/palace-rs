use palace::db;
use palace::general_extractor::extract_memories;
use palace::mcp_server::dispatch_tool;
use palace::palace::Palace;
use palace::ranker::hybrid_search;
use palace::searcher::search_memories;
use palace::store::{add_drawer, preference_search_filtered, DrawerFilter};

fn test_db() -> rusqlite::Connection {
    db::open_in_memory().expect("in-memory DB should open")
}

fn add(conn: &rusqlite::Connection, room: &str, content: &str, source: &str, chunk: usize) {
    add_drawer(
        conn,
        "mempalace_rs",
        room,
        content,
        None,
        source,
        chunk,
        "test",
        3.0,
    )
    .expect("drawer should be added");
}

#[test]
fn sanitized_prompt_dump_retrieves_the_actual_memory_question() {
    let conn = test_db();
    add(
        &conn,
        "decisions",
        "We chose bundled SQLite because library consumers should not need system sqlite installed.",
        "sessions/001.md",
        0,
    );
    add(
        &conn,
        "frontend",
        "The dashboard uses muted teal navigation and compact cards.",
        "sessions/002.md",
        0,
    );

    let noisy_query = r#"
        You are a coding agent. Here is a huge unrelated prompt dump.
        ```json
        {"tool":"shell","output":"dashboard dashboard dashboard frontend teal cards"}
        ```
        Search query: why did we choose bundled sqlite for consumers?
    "#;

    let results = hybrid_search(&conn, noisy_query, None, &DrawerFilter::default(), 5)
        .expect("search should work");

    assert_eq!(results[0].drawer.source_file, "sessions/001.md");
}

#[test]
fn coding_agent_intents_boost_practical_memory_queries() {
    let conn = test_db();
    let cases = [
        (
            "decisions",
            "We decided to keep the Palace facade small because downstream Rust callers need stable APIs.",
            "why did we keep the Palace facade small?",
        ),
        (
            "problems",
            "The migration test failed because created_at was missing; the fix was adding an idempotent column migration.",
            "how did we fix the migration test failure last time?",
        ),
        (
            "commands",
            "For full local verification, run cargo clippy --all-targets --all-features -- -D warnings.",
            "what command should I run for clippy?",
        ),
        (
            "conventions",
            "Project convention: always keep MCP search results source-grounded and never replace verbatim drawers with summaries.",
            "what is the project convention for search results?",
        ),
        (
            "preferences",
            "The user prefers focused implementation over extra CLI commands or broad feature sprawl.",
            "what does the user prefer about feature scope?",
        ),
        (
            "current",
            "Current direction changed from broad MemPalace parity to a narrow coding-agent memory engine.",
            "what changed in the current direction?",
        ),
    ];

    for (idx, (room, content, _)) in cases.iter().enumerate() {
        add(&conn, room, content, &format!("sessions/{idx}.md"), 0);
    }

    for (room, _, query) in cases {
        let results = hybrid_search(&conn, query, None, &DrawerFilter::default(), 5)
            .expect("search should work");
        assert_eq!(
            results[0].drawer.room, room,
            "query should retrieve the {room} memory first: {query}"
        );
    }
}

#[test]
fn mcp_search_returns_bounded_source_context_and_score_provenance() {
    let conn = test_db();
    add(
        &conn,
        "decisions",
        "Earlier unrelated chunk.",
        "sessions/source.md",
        0,
    );
    add(
        &conn,
        "decisions",
        "We chose source context expansion because agents need neighboring chunks to cite decisions correctly.",
        "sessions/source.md",
        1,
    );
    add(
        &conn,
        "decisions",
        "Later implementation detail.",
        "sessions/source.md",
        2,
    );
    add(
        &conn,
        "decisions",
        "Different source should not appear.",
        "sessions/other.md",
        1,
    );

    let value = search_memories(
        &conn,
        "why did we choose source context expansion?",
        Some("mempalace_rs"),
        None,
        1,
    );

    let first = &value["results"][0];
    assert_eq!(first["source_file"], "sessions/source.md");
    assert!(first["combined"].is_number());
    assert!(first["cosine"].is_number());
    assert!(first["bm25"].is_number());
    assert!(first["coding_boost"].is_number());
    assert!(first["created_at"].is_string());
    assert!(first["filed_at"].is_string());

    let context = first["source_context"]
        .as_array()
        .expect("source_context should be an array");
    let chunks: Vec<i64> = context
        .iter()
        .map(|value| value["chunk_index"].as_i64().expect("chunk index"))
        .collect();
    assert_eq!(chunks, vec![0, 1, 2]);
    assert!(context
        .iter()
        .all(|value| value["source_file"] == "sessions/source.md"));
}

#[test]
fn extracted_memories_keep_source_provenance_when_stored() {
    let memories = extract_memories(
        "We decided to use SQLite because it keeps the coding-agent memory engine local.",
        0.1,
    );

    assert!(
        memories
            .iter()
            .any(|memory| memory.memory_type == "decision"
                && memory.content.contains("We decided to use SQLite")
                && memory.chunk_index == 0),
        "decision memory should preserve original text and chunk index"
    );
}

#[test]
fn palace_extracted_memories_point_back_to_voice_turn_source() {
    let palace = Palace::open_in_memory().expect("palace should open");
    palace
        .ingest_turn(
            "We decided to use SQLite because it is local instead of running Chroma.",
            "I will keep that decision source-grounded.",
        )
        .expect("turn should ingest");

    let source_file: String = palace
        .conn()
        .query_row(
            "SELECT source_file FROM drawers WHERE room = 'decision' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("extracted decision should exist");

    assert_eq!(source_file, "voice_turn");
}

#[test]
fn preference_recall_respects_search_filters() {
    let conn = test_db();
    let query_vec: Vec<f32> = (0..384).map(|i| (i as f32).sin()).collect();
    let other_vec: Vec<f32> = (0..384).map(|i| (i as f32).cos()).collect();
    add_drawer(
        &conn,
        "target_wing",
        "preferences",
        "I prefer small public APIs for embedded Rust callers.",
        Some(&query_vec),
        "target.md",
        0,
        "test",
        3.0,
    )
    .expect("target preference should insert");
    add_drawer(
        &conn,
        "other_wing",
        "preferences",
        "I prefer unrelated dashboard experiments.",
        Some(&other_vec),
        "other.md",
        0,
        "test",
        3.0,
    )
    .expect("other preference should insert");

    let filter = DrawerFilter {
        wing: Some("target_wing".to_string()),
        room: Some("preferences".to_string()),
    };
    let results =
        preference_search_filtered(&conn, &query_vec, &filter, 10).expect("search should work");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source_file, "target.md");
}

#[test]
fn palace_structured_search_exposes_score_provenance() {
    let palace = Palace::open_in_memory().expect("palace should open");
    add_drawer(
        palace.conn(),
        "mempalace_rs",
        "decisions",
        "We chose bundled SQLite because coding agents should not need Chroma.",
        None,
        "sqlite.md",
        0,
        "test",
        3.0,
    )
    .expect("drawer should insert");

    let results = palace
        .search_with_provenance("why did we choose bundled sqlite?", None, None, 3)
        .expect("structured search should work");

    let first = results.first().expect("expected a search result");
    assert_eq!(first.drawer.source_file, "sqlite.md");
    assert!(first.combined > 0.0);
    assert!(first.bm25 > 0.0);
    assert_eq!(
        first.drawer.similarity,
        (first.combined * 1000.0).round() / 1000.0
    );
}

#[test]
fn session_context_returns_recent_diary_metadata_and_compact_text() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let write = dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &serde_json::json!({
            "agent_name": "Codex",
            "entry": "PROJ:mempalace_rs | Implemented preference recall reliability.",
            "topic": "release",
            "session_id": "session-123",
            "project_path": "D:\\Dev\\Projects\\mempalace-rs",
            "tags": ["0.1.9", "reliability"]
        }),
    );
    assert_eq!(write["success"], true);

    let context = dispatch_tool(
        &conn,
        &config,
        "palace_session_context",
        &serde_json::json!({
            "agent_name": "Codex"
        }),
    );

    assert_eq!(context["has_recent_session"], true);
    assert_eq!(
        context["last_active_project"],
        "D:\\Dev\\Projects\\mempalace-rs"
    );
    let entries = context["recent_entries"]
        .as_array()
        .expect("recent_entries should be an array");
    let first = entries.first().expect("expected recent diary entry");
    assert_eq!(first["topic"], "release");
    assert_eq!(first["session_id"], "session-123");
    assert_eq!(first["project_path"], "D:\\Dev\\Projects\\mempalace-rs");
    assert!(first["timestamp"].is_string());
    assert!(first["text"].as_str().expect("compact text").len() <= 240);
}
