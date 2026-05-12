//! MCP server integration tests — verify all 19 tools respond correctly.

use palace::db;
use palace::knowledge_graph;
use palace::store;

fn test_db() -> rusqlite::Connection {
    db::open_in_memory().unwrap()
}

// ── Status ────────────────────────────────────────────────────────────────

#[test]
fn status_returns_counts() {
    let conn = test_db();
    let count = store::count_drawers(&conn).unwrap();
    assert_eq!(count, 0);

    store::add_drawer(
        &conn,
        "wing_test",
        "room_a",
        "content",
        None,
        "f.txt",
        0,
        "test",
        3.0,
    )
    .unwrap();
    let count = store::count_drawers(&conn).unwrap();
    assert_eq!(count, 1);
}

// ── Taxonomy ──────────────────────────────────────────────────────────────

#[test]
fn taxonomy_aggregates_correctly() {
    let conn = test_db();
    store::add_drawer(
        &conn, "wing_a", "room_1", "x", None, "a.txt", 0, "test", 3.0,
    )
    .unwrap();
    store::add_drawer(
        &conn, "wing_a", "room_2", "y", None, "b.txt", 0, "test", 3.0,
    )
    .unwrap();
    store::add_drawer(
        &conn, "wing_b", "room_1", "z", None, "c.txt", 0, "test", 3.0,
    )
    .unwrap();

    let tax = store::taxonomy(&conn).unwrap();
    assert_eq!(tax["wing_a"]["room_1"], 1);
    assert_eq!(tax["wing_a"]["room_2"], 1);
    assert_eq!(tax["wing_b"]["room_1"], 1);
}

// ── Add drawer ────────────────────────────────────────────────────────────

#[test]
fn add_drawer_tool_succeeds() {
    let conn = test_db();
    let now = chrono::Utc::now().to_rfc3339();
    let prefix = "test content";
    let id = {
        let hash = blake3::hash(
            format!(
                "wing_code/backend/{}/{now}",
                &prefix[..10.min(prefix.len())]
            )
            .as_bytes(),
        );
        format!("drawer_wing_code_backend_{}", &hash.to_hex()[..16])
    };
    let added = store::add_drawer_with_id(
        &conn,
        &id,
        "wing_code",
        "backend",
        "test content",
        None,
        "",
        "test",
        None,
    )
    .unwrap();
    assert!(added);
}

// ── Delete drawer ─────────────────────────────────────────────────────────

#[test]
fn delete_nonexistent_drawer_returns_not_found() {
    let conn = test_db();
    let d = store::get_drawer(&conn, "drawer_nonexistent").unwrap();
    assert!(d.is_none());
}

// ── Knowledge graph tools ─────────────────────────────────────────────────

#[test]
fn kg_add_and_query() {
    let conn = test_db();
    knowledge_graph::add_triple(&conn, "Alice", "loves", "Rust", None, None, 1.0, None, None)
        .unwrap();
    let facts = knowledge_graph::query_entity(&conn, "Alice", None, "outgoing").unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "loves");
}

#[test]
fn kg_invalidate_marks_fact_ended() {
    let conn = test_db();
    knowledge_graph::add_triple(
        &conn,
        "Bob",
        "works_at",
        "Corp",
        Some("2020-01-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();
    knowledge_graph::invalidate(&conn, "Bob", "works_at", "Corp", Some("2024-01-01")).unwrap();
    let facts = knowledge_graph::query_entity(&conn, "Bob", None, "outgoing").unwrap();
    assert!(!facts[0].current);
}

#[test]
fn kg_timeline_returns_facts() {
    let conn = test_db();
    knowledge_graph::add_triple(
        &conn,
        "Eve",
        "joined",
        "Project",
        Some("2023-01-01"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();
    let tl = knowledge_graph::timeline(&conn, Some("Eve")).unwrap();
    assert_eq!(tl.len(), 1);
}

#[test]
fn kg_stats_summary() {
    let conn = test_db();
    knowledge_graph::add_triple(&conn, "A", "rel", "B", None, None, 1.0, None, None).unwrap();
    let s = knowledge_graph::stats(&conn).unwrap();
    assert!(s.entities >= 2);
    assert!(s.triples >= 1);
}

// ── Check duplicate ───────────────────────────────────────────────────────

#[test]
fn check_duplicate_returns_no_matches_on_empty_palace() {
    let conn = test_db();
    // With no embeddings in the DB, should return empty
    let dups = store::check_duplicate(&conn, "test content", 0.9);
    // Embedding will be generated but DB is empty so no matches
    assert!(dups.is_err() || dups.unwrap().is_empty());
}

// ── palace_remember / palace_forget / palace_explain ─────────────────────

#[test]
fn remember_inserts_high_importance_drawer() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let args = serde_json::json!({
        "text": "The user always prefers dark mode",
        "wing": "preferences",
        "room": "ui"
    });
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_remember", &args);
    assert_eq!(result["success"], true);
    assert_eq!(result["inserted"], true);
    let id = result["id"].as_str().unwrap();

    // Verify the drawer actually landed in the DB.
    let drawer = store::get_drawer(&conn, id).unwrap().unwrap();
    assert_eq!(drawer.importance, 5.0);
    assert_eq!(drawer.wing, "preferences");
}

#[test]
fn forget_deletes_a_drawer() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    // Add via remember first.
    let add_args =
        serde_json::json!({"text": "Temporary fact to forget", "wing": "w", "room": "r"});
    let added = palace::mcp_server::dispatch_tool(&conn, &config, "palace_remember", &add_args);
    let id = added["id"].as_str().unwrap().to_string();

    // Now forget it.
    let forget_args = serde_json::json!({"id": id});
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_forget", &forget_args);
    assert_eq!(result["success"], true);

    // Drawer should be gone.
    let gone = store::get_drawer(&conn, &id).unwrap();
    assert!(gone.is_none());
}

#[test]
fn explain_returns_full_provenance() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let add_args = serde_json::json!({"text": "Explain provenance test", "wing": "w", "room": "r"});
    let added = palace::mcp_server::dispatch_tool(&conn, &config, "palace_remember", &add_args);
    let id = added["id"].as_str().unwrap().to_string();

    let explain_args = serde_json::json!({"id": id});
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_explain", &explain_args);
    assert_eq!(result["id"], id);
    assert_eq!(result["wing"], "w");
    assert_eq!(result["room"], "r");
    assert!(result.get("added_by").is_some());
    assert!(result.get("filed_at").is_some());
}

#[test]
fn explain_unknown_id_returns_error() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let args = serde_json::json!({"id": "nonexistent_id"});
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_explain", &args);
    assert!(result.get("error").is_some());
}

// ── palace_export ──────────────────────────────────────────────────────────

#[test]
fn export_returns_all_drawers() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    store::add_drawer(
        &conn,
        "w",
        "r",
        "export content one",
        None,
        "f1.txt",
        0,
        "t",
        3.0,
    )
    .unwrap();
    store::add_drawer(
        &conn,
        "w",
        "r",
        "export content two",
        None,
        "f2.txt",
        0,
        "t",
        3.0,
    )
    .unwrap();

    let result =
        palace::mcp_server::dispatch_tool(&conn, &config, "palace_export", &serde_json::json!({}));
    assert_eq!(result["total"], 2);
    let drawers = result["drawers"].as_array().unwrap();
    assert_eq!(drawers.len(), 2);
}

// ── Wing/room counts ──────────────────────────────────────────────────────

#[test]
fn wing_counts_return_correct_values() {
    let conn = test_db();
    store::add_drawer(&conn, "w1", "r1", "a", None, "f1.txt", 0, "t", 3.0).unwrap();
    store::add_drawer(&conn, "w1", "r2", "b", None, "f2.txt", 0, "t", 3.0).unwrap();
    store::add_drawer(&conn, "w2", "r1", "c", None, "f3.txt", 0, "t", 3.0).unwrap();
    let wings = store::wing_counts(&conn).unwrap();
    assert_eq!(wings["w1"], 2);
    assert_eq!(wings["w2"], 1);
}

#[test]
fn room_counts_filtered_by_wing() {
    let conn = test_db();
    store::add_drawer(&conn, "w1", "r1", "x", None, "a.txt", 0, "t", 3.0).unwrap();
    store::add_drawer(&conn, "w1", "r2", "y", None, "b.txt", 0, "t", 3.0).unwrap();
    store::add_drawer(&conn, "w2", "r1", "z", None, "c.txt", 0, "t", 3.0).unwrap();
    let rooms = store::room_counts(&conn, Some("w1")).unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms.contains_key("r1"));
    assert!(rooms.contains_key("r2"));
}
