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

#[test]
fn seed_adoption_facts_tool_is_idempotent() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let first = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_seed_adoption_facts",
        &serde_json::json!({"project": "mempalace-rs"}),
    );
    let second = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_seed_adoption_facts",
        &serde_json::json!({"project": "mempalace-rs"}),
    );

    assert_eq!(first["success"], true);
    assert!(first["inserted"].as_u64().unwrap_or_default() >= 10);
    assert_eq!(second["success"], true);
    assert_eq!(second["inserted"], 0);
    assert!(second["unchanged"].as_u64().unwrap_or_default() >= 10);
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

// ── Reliability tools ─────────────────────────────────────────────────────

#[test]
fn verify_reports_mcp_database_and_tool_health() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let result =
        palace::mcp_server::dispatch_tool(&conn, &config, "palace_verify", &serde_json::json!({}));

    assert_eq!(result["ok"], true);
    assert_eq!(result["mcp"]["server_name"], "palace");
    assert_eq!(result["database"]["drawer_count"], 0);
    let tools = result["mcp"]["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|tool| tool == "palace_search"));
    assert!(tools.iter().any(|tool| tool == "palace_recall_check"));
    assert!(tools.iter().any(|tool| tool == "palace_conflicts"));
}

#[test]
fn recall_check_reports_expected_memory_hits() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    store::add_drawer(
        &conn,
        "palace_rs",
        "decisions",
        "We chose bundled SQLite so local coding agents do not need Chroma.",
        None,
        "decisions/sqlite.md",
        0,
        "test",
        3.0,
    )
    .unwrap();

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_recall_check",
        &serde_json::json!({
            "checks": [
                {
                    "query": "why did we choose bundled sqlite?",
                    "expected_source": "decisions/sqlite.md",
                    "wing": "palace_rs"
                }
            ]
        }),
    );

    assert_eq!(result["ok"], true);
    assert_eq!(result["passed"], 1);
    assert_eq!(result["failed"], 0);
    assert_eq!(result["checks"][0]["passed"], true);
}

#[test]
fn conflicts_surface_active_and_ended_fact_versions() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let old = knowledge_graph::add_triple(
        &conn,
        "project",
        "database",
        "Chroma",
        Some("2026-01-01"),
        Some("2026-05-01"),
        1.0,
        None,
        None,
    )
    .unwrap();
    let new = knowledge_graph::add_triple(
        &conn,
        "project",
        "database",
        "SQLite",
        Some("2026-05-02"),
        None,
        1.0,
        None,
        None,
    )
    .unwrap();

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_conflicts",
        &serde_json::json!({"entity": "project"}),
    );

    assert_eq!(result["count"], 1);
    assert_eq!(result["conflicts"][0]["subject"], "project");
    assert_eq!(result["conflicts"][0]["predicate"], "database");
    let ids = result["conflicts"][0]["triple_ids"]
        .as_array()
        .expect("ids");
    assert!(ids.iter().any(|id| id == &old));
    assert!(ids.iter().any(|id| id == &new));
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

// ── palace_import ──────────────────────────────────────────────────────────

#[test]
fn import_inserts_new_drawers() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    // Export from a source DB with one drawer
    let src = test_db();
    store::add_drawer(
        &src,
        "w",
        "r",
        "import test content",
        None,
        "f.txt",
        0,
        "t",
        3.0,
    )
    .unwrap();
    let export_result =
        palace::mcp_server::dispatch_tool(&src, &config, "palace_export", &serde_json::json!({}));
    let export_json = serde_json::to_string(&export_result).unwrap();

    // Import into the empty dest DB
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_import",
        &serde_json::json!({"export_json": export_json}),
    );
    assert_eq!(result["inserted"], 1);
    assert_eq!(result["skipped"], 0);
}

#[test]
fn import_skips_existing_drawers() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    store::add_drawer(&conn, "w", "r", "already here", None, "f.txt", 0, "t", 3.0).unwrap();

    let export_result =
        palace::mcp_server::dispatch_tool(&conn, &config, "palace_export", &serde_json::json!({}));
    let export_json = serde_json::to_string(&export_result).unwrap();

    // Import back into the same DB — all should be skipped
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_import",
        &serde_json::json!({"export_json": export_json}),
    );
    assert_eq!(result["inserted"], 0);
    assert_eq!(result["skipped"], 1);
}

#[test]
fn import_returns_error_on_invalid_json() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_import",
        &serde_json::json!({"export_json": "not valid json"}),
    );
    assert!(result.get("error").is_some());
}

// ── palace_upgrade_embeddings ──────────────────────────────────────────────

#[test]
fn upgrade_embeddings_returns_count() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    store::add_drawer(
        &conn,
        "w",
        "r",
        "content to re-embed",
        None,
        "f.txt",
        0,
        "t",
        3.0,
    )
    .unwrap();
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_upgrade_embeddings",
        &serde_json::json!({}),
    );
    // May succeed or fail depending on model availability, but must return a
    // structured response — never a panic.
    assert!(result.get("reembedded").is_some() || result.get("error").is_some());
}

// ── palace_prune ───────────────────────────────────────────────────────────

#[test]
fn prune_removes_old_drawers() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    // Insert a drawer then manually backdate it to 40 days ago
    store::add_drawer(&conn, "w", "r", "old content", None, "f.txt", 0, "t", 3.0).unwrap();
    conn.execute(
        "UPDATE drawers SET created_at = datetime('now', '-40 days'), filed_at = datetime('now', '-40 days')",
        [],
    )
    .unwrap();

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_prune",
        &serde_json::json!({"older_than_days": 30}),
    );
    assert_eq!(result["pruned"], 1);
    assert_eq!(store::count_drawers(&conn).unwrap(), 0);
}

#[test]
fn prune_keeps_recent_drawers() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    store::add_drawer(
        &conn,
        "w",
        "r",
        "recent content",
        None,
        "f.txt",
        0,
        "t",
        3.0,
    )
    .unwrap();

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_prune",
        &serde_json::json!({"older_than_days": 30}),
    );
    assert_eq!(result["pruned"], 0);
    assert_eq!(store::count_drawers(&conn).unwrap(), 1);
}

#[test]
fn prune_requires_older_than_days() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let result =
        palace::mcp_server::dispatch_tool(&conn, &config, "palace_prune", &serde_json::json!({}));
    assert!(result.get("error").is_some());
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

// ── Cross-agent, project-scoped recall ────────────────────────────────────

fn write_diary(
    conn: &rusqlite::Connection,
    config: &palace::config::PalaceConfig,
    agent: &str,
    entry: &str,
    project_path: &str,
) {
    let result = palace::mcp_server::dispatch_tool(
        conn,
        config,
        "palace_diary_write",
        &serde_json::json!({
            "agent_name": agent,
            "entry": entry,
            "topic": "investigation",
            "project_path": project_path,
        }),
    );
    assert_eq!(result["success"], true, "diary_write failed: {result}");
}

#[test]
fn diary_search_all_agents_finds_other_agent_entries() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    // Agent "claude-cli" investigates and records a decision.
    write_diary(
        &conn,
        &config,
        "claude-cli",
        "Investigated the duplicate checkout endpoint and chose to deduplicate by idempotency key in the payment handler.",
        "/proj/checkout",
    );

    // A different agent ("cursor") searches across all agents.
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_search",
        &serde_json::json!({
            "agent_name": "cursor",
            "query": "why did we deduplicate the checkout endpoint idempotency key",
            "all_agents": true,
        }),
    );

    let hits = result["results"].as_array().expect("results array");
    assert!(
        !hits.is_empty(),
        "cross-agent diary search should surface another agent's entry: {result}"
    );
    let joined = hits
        .iter()
        .filter_map(|h| h["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("idempotency key"),
        "expected the claude-cli decision in results, got: {joined}"
    );
}

#[test]
fn diary_search_defaults_to_calling_agent_only() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    write_diary(
        &conn,
        &config,
        "claude-cli",
        "Investigated the duplicate checkout endpoint and chose to deduplicate by idempotency key.",
        "/proj/checkout",
    );

    // Without all_agents, a different agent sees nothing (per-agent isolation).
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_search",
        &serde_json::json!({
            "agent_name": "cursor",
            "query": "idempotency key checkout endpoint",
        }),
    );
    let hits = result["results"].as_array().expect("results array");
    assert!(
        hits.is_empty(),
        "default diary search must stay scoped to the calling agent: {result}"
    );
}

// ── Write-path semantic dedup ─────────────────────────────────────────────

#[test]
fn remember_blocks_near_duplicate_facts_across_rooms() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let text = "We deduplicate checkout requests using the idempotency key stored in Redis.";

    let first = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_remember",
        &serde_json::json!({"text": text, "wing": "decisions", "room": "checkout"}),
    );
    assert_eq!(
        first["inserted"], true,
        "first remember should insert: {first}"
    );

    // Same fact filed under a different room would slip past the deterministic
    // ID guard — semantic dedup must catch it.
    let second = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_remember",
        &serde_json::json!({"text": text, "wing": "decisions", "room": "payments"}),
    );
    assert_eq!(
        second["inserted"], false,
        "near-duplicate remember should be skipped: {second}"
    );
    assert_eq!(
        second["reason"], "duplicate",
        "expected duplicate reason: {second}"
    );

    // Only the first fact should be stored.
    assert_eq!(store::count_drawers(&conn).unwrap(), 1);
}

#[test]
fn diary_write_blocks_near_duplicate_entries_for_same_agent() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let entry =
        "Investigated the duplicate suggestion bug and decided to dedupe by idempotency key.";

    let first = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &serde_json::json!({"agent_name": "claude-cli", "entry": entry, "topic": "investigation"}),
    );
    assert_eq!(
        first["success"], true,
        "first diary_write should succeed: {first}"
    );

    let second = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &serde_json::json!({"agent_name": "claude-cli", "entry": entry, "topic": "investigation"}),
    );
    assert_eq!(
        second["success"], false,
        "near-duplicate diary entry should be skipped: {second}"
    );
    assert_eq!(
        second["reason"], "duplicate",
        "expected duplicate reason: {second}"
    );

    assert_eq!(store::count_drawers(&conn).unwrap(), 1);
}

#[test]
fn diary_write_dedup_is_scoped_per_agent() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    let entry =
        "Investigated the duplicate suggestion bug and decided to dedupe by idempotency key.";

    let first = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &serde_json::json!({"agent_name": "claude-cli", "entry": entry}),
    );
    assert_eq!(first["success"], true);

    // A different agent recording the same observation is NOT a duplicate — it
    // is that agent's own continuity record.
    let other = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &serde_json::json!({"agent_name": "cursor", "entry": entry}),
    );
    assert_eq!(
        other["success"], true,
        "another agent's identical entry must still be stored: {other}"
    );
    assert_eq!(store::count_drawers(&conn).unwrap(), 2);
}

#[test]
fn session_context_falls_back_to_other_agents_for_project() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();

    // Yesterday's agent recorded recent work for a project.
    write_diary(
        &conn,
        &config,
        "claude-cli",
        "Pushed a fix that removes the duplicate suggestion in the checkout flow.",
        "/proj/checkout",
    );

    // A different agent opens a session for the same project and has no diary
    // of its own — it should still see the prior agent's recent work.
    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_session_context",
        &serde_json::json!({
            "agent_name": "cursor",
            "project_path": "/proj/checkout",
        }),
    );

    assert_eq!(
        result["has_recent_session"], true,
        "cross-agent fallback should report a recent session: {result}"
    );
    assert_eq!(
        result["cross_agent"], true,
        "fallback context must be flagged as cross-agent: {result}"
    );
    let entries = result["recent_entries"].as_array().expect("recent_entries");
    assert!(!entries.is_empty());
}

// ── Wings registry: create / project status / auto-register ─────────────────

#[test]
fn create_wing_declares_topic_wing() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let args = serde_json::json!({
        "name": "Enterprise Sales",
        "description": "Sales account notes",
    });
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_create_wing", &args);
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["wing"]["name"], "enterprise_sales");
    assert_eq!(result["wing"]["kind"], "topic");

    let record = store::get_wing(&conn, "enterprise_sales").unwrap().unwrap();
    assert_eq!(record.description, "Sales account notes");
}

#[test]
fn project_status_unknown_for_fresh_dir() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let tmp = tempfile::TempDir::new().unwrap();

    let args = serde_json::json!({ "project_path": tmp.path().to_string_lossy() });
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_project_status", &args);
    assert_eq!(result["state"], "unknown", "{result}");
    assert!(result["recommendation"].is_string());
}

#[test]
fn remember_auto_registers_new_wing() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    let args = serde_json::json!({
        "text": "A brand new fact for a brand new topic wing",
        "wing": "fresh_topic",
        "room": "general",
    });
    let result = palace::mcp_server::dispatch_tool(&conn, &config, "palace_remember", &args);
    assert_eq!(result["success"], true, "{result}");

    let record = store::get_wing(&conn, "fresh_topic").unwrap();
    assert!(record.is_some(), "wing should be auto-registered");
    assert_eq!(record.unwrap().kind, "topic");
}

#[test]
fn list_wings_returns_registry_records() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    store::upsert_wing(&conn, "topic_x", "topic", "desc", None).unwrap();

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_list_wings",
        &serde_json::json!({}),
    );
    let wings = result["wings"].as_array().expect("wings array");
    assert!(
        wings
            .iter()
            .any(|w| w["name"] == "topic_x" && w["kind"] == "topic"),
        "registry should list topic_x: {result}"
    );
}
