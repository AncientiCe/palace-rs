//! Wing names on write, and renaming or merging a wing.

use palace::store::{self, add_drawer, DrawerFilter};
use palace::wings::{rename_wing, resolve_wing, sync_palace_yaml};
use serde_json::json;

fn test_db() -> rusqlite::Connection {
    palace::db::open_in_memory().unwrap()
}

fn file(conn: &rusqlite::Connection, wing: &str, room: &str, text: &str, source: &str) -> String {
    let (_, id) = add_drawer(conn, wing, room, text, None, source, 0, "test", 3.0).unwrap();
    store::ensure_wing_registered(conn, wing).unwrap();
    id
}

fn count(conn: &rusqlite::Connection, sql: &str, wing: &str) -> i64 {
    conn.query_row(sql, [wing], |row| row.get(0)).unwrap()
}

// ── wing names on write ───────────────────────────────────────────────────

#[test]
fn a_new_wing_gets_a_normalized_name() {
    let conn = test_db();
    assert_eq!(resolve_wing(&conn, "Sales Notes").unwrap(), "sales_notes");
    assert_eq!(resolve_wing(&conn, " my-proj ").unwrap(), "my_proj");
}

#[test]
fn a_misspelled_separator_lands_in_the_existing_wing() {
    let conn = test_db();
    file(&conn, "my_proj", "notes", "first note", "a");

    assert_eq!(resolve_wing(&conn, "my-proj").unwrap(), "my_proj");
    assert_eq!(resolve_wing(&conn, "My Proj").unwrap(), "my_proj");
}

#[test]
fn a_wing_that_already_exists_under_its_exact_name_is_kept() {
    let conn = test_db();
    file(
        &conn,
        "legacy-wing",
        "notes",
        "filed before names were normalized",
        "a",
    );

    assert_eq!(resolve_wing(&conn, "legacy-wing").unwrap(), "legacy-wing");
}

#[test]
fn diary_wings_keep_their_name() {
    let conn = test_db();
    assert_eq!(
        resolve_wing(&conn, "wing_diary__claude-code").unwrap(),
        "wing_diary__claude-code"
    );
}

#[test]
fn remember_and_add_drawer_file_into_the_existing_wing() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    file(&conn, "my_proj", "notes", "first note", "a");

    let remembered = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_remember",
        &json!({"text": "We chose tokio for the HTTP server.", "wing": "my-proj", "room": "notes"}),
    );
    assert_eq!(remembered["wing"], "my_proj", "{remembered}");

    let added = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_add_drawer",
        &json!({"content": "Release checklist lives in the wiki.", "wing": "My Proj", "room": "notes"}),
    );
    assert_eq!(added["wing"], "my_proj", "{added}");
    assert!(added["drawer_id"]
        .as_str()
        .unwrap()
        .starts_with("drawer_my_proj_"));

    let wings = store::wing_counts(&conn).unwrap();
    assert_eq!(wings.len(), 1, "no second wing: {wings:?}");
    assert_eq!(wings["my_proj"], 3);
}

// ── rename ────────────────────────────────────────────────────────────────

#[test]
fn rename_moves_everything_filed_under_the_wing() {
    let conn = test_db();
    let id = file(&conn, "old_name", "notes", "tokio decision", "/repo/a.rs");
    file(&conn, "other", "notes", "unrelated", "b");
    store::upsert_mined_file(&conn, "/repo/a.rs", "old_name", "hash", 1).unwrap();
    palace::closet::add_closet(
        &conn,
        "old_name",
        "notes",
        "runtime",
        std::slice::from_ref(&id),
        None,
    )
    .unwrap();
    palace::palace_graph::create_tunnel(&conn, "old_name", "notes", "other", "notes", "explicit")
        .unwrap();

    let report = rename_wing(&conn, "old_name", "new_name").unwrap();

    assert_eq!(report.from, "old_name");
    assert_eq!(report.to, "new_name");
    assert!(!report.merged);
    assert_eq!(report.drawers, 1);
    assert_eq!(report.closets, 1);
    assert_eq!(report.tunnels, 1);
    assert_eq!(report.mined_files, 1);

    let drawer = store::get_drawer(&conn, &id).unwrap().expect("same id");
    assert_eq!(drawer.wing, "new_name");
    for sql in [
        "SELECT COUNT(*) FROM drawers WHERE wing = ?1",
        "SELECT COUNT(*) FROM closets WHERE wing = ?1",
        "SELECT COUNT(*) FROM mined_files WHERE wing = ?1",
        "SELECT COUNT(*) FROM wings WHERE name = ?1",
        "SELECT COUNT(*) FROM tunnels WHERE wing_a = ?1 OR wing_b = ?1",
    ] {
        assert_eq!(count(&conn, sql, "old_name"), 0, "{sql}");
        assert_eq!(count(&conn, sql, "new_name"), 1, "{sql}");
    }
    assert_eq!(
        palace::closet::closet_id("new_name", "notes", "runtime"),
        conn.query_row("SELECT id FROM closets", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "a later upsert of the same topic must hit this closet"
    );
    assert_eq!(
        palace::palace_graph::follow_tunnels(&conn, "new_name", "notes")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rename_normalizes_the_new_name() {
    let conn = test_db();
    file(&conn, "old_name", "notes", "tokio decision", "a");

    let report = rename_wing(&conn, "old_name", "New Name").unwrap();

    assert_eq!(report.to, "new_name");
}

#[test]
fn rename_refuses_an_unknown_or_unchanged_wing() {
    let conn = test_db();
    file(&conn, "my_proj", "notes", "tokio decision", "a");

    assert!(rename_wing(&conn, "missing", "elsewhere").is_err());
    assert!(rename_wing(&conn, "my_proj", "my_proj").is_err());
    assert!(rename_wing(&conn, "my_proj", "My-Proj").is_err());
    assert!(rename_wing(&conn, "my_proj", "  ").is_err());
    assert_eq!(store::wing_counts(&conn).unwrap()["my_proj"], 1);
}

// ── merge ─────────────────────────────────────────────────────────────────

#[test]
fn renaming_onto_an_existing_wing_merges_the_two() {
    let conn = test_db();
    file(
        &conn,
        "my_proj",
        "notes",
        "mined chunk about the runtime",
        "/repo/a.rs",
    );
    store::set_wing_mined(&conn, "my_proj", "/repo").unwrap();
    let stray = file(
        &conn,
        "my-proj",
        "planning",
        "parked plan for mixed profiles",
        "x",
    );

    let report = rename_wing(&conn, "my-proj", "my_proj").unwrap();

    assert!(report.merged);
    assert_eq!(report.to, "my_proj");
    assert_eq!(report.drawers, 1);
    let wings = store::wing_counts(&conn).unwrap();
    assert_eq!(wings.len(), 1, "{wings:?}");
    assert_eq!(wings["my_proj"], 2);
    assert!(store::get_wing(&conn, "my-proj").unwrap().is_none());
    let kept = store::get_wing(&conn, "my_proj").unwrap().expect("target");
    assert_eq!(kept.kind, "project");
    assert_eq!(kept.project_path.as_deref(), Some("/repo"));

    let filter = DrawerFilter {
        wing: Some("my_proj".to_string()),
        room: None,
    };
    let hits = palace::ranker::hybrid_search(&conn, "parked plan mixed profiles", None, &filter, 5)
        .unwrap();
    assert_eq!(hits[0].drawer.id, stray);
}

#[test]
fn merge_keeps_what_only_the_renamed_wing_knew() {
    let conn = test_db();
    file(&conn, "target", "notes", "a note", "a");
    file(&conn, "source", "notes", "mined chunk", "/repo/a.rs");
    store::set_wing_mined(&conn, "source", "/repo").unwrap();

    rename_wing(&conn, "source", "target").unwrap();

    let kept = store::get_wing(&conn, "target").unwrap().expect("target");
    assert_eq!(kept.kind, "project");
    assert_eq!(kept.project_path.as_deref(), Some("/repo"));
    assert!(kept.last_mined_at.is_some());
}

#[test]
fn merge_joins_closets_and_tunnels_that_now_coincide() {
    let conn = test_db();
    let a = file(&conn, "target", "notes", "note a", "a");
    let b = file(&conn, "source", "notes", "note b", "b");
    palace::closet::add_closet(
        &conn,
        "target",
        "notes",
        "runtime",
        std::slice::from_ref(&a),
        None,
    )
    .unwrap();
    palace::closet::add_closet(
        &conn,
        "source",
        "notes",
        "runtime",
        std::slice::from_ref(&b),
        None,
    )
    .unwrap();
    for wing in ["target", "source"] {
        palace::palace_graph::create_tunnel(&conn, wing, "notes", "other", "notes", "explicit")
            .unwrap();
    }
    palace::palace_graph::create_tunnel(&conn, "source", "notes", "target", "notes", "explicit")
        .unwrap();

    rename_wing(&conn, "source", "target").unwrap();

    let closets = palace::closet::list_closets(&conn, Some("target"), None).unwrap();
    assert_eq!(closets.len(), 1);
    let mut pointers = closets[0].pointer_drawer_ids.clone();
    pointers.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(pointers, expected);
    let tunnels = palace::palace_graph::list_tunnels(&conn, None, None).unwrap();
    assert_eq!(
        tunnels.len(),
        1,
        "the duplicate and the tunnel from a room to itself are dropped: {tunnels:?}"
    );
}

// ── tool and palace.yaml ──────────────────────────────────────────────────

#[test]
fn rename_wing_tool_reports_the_merge() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    file(&conn, "my_proj", "notes", "a note", "a");
    file(&conn, "my-proj", "notes", "a stray note", "b");

    let result = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_rename_wing",
        &json!({"from": "my-proj", "to": "my_proj"}),
    );

    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["merged"], true);
    assert_eq!(result["drawers"], 1);

    let missing = palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_rename_wing",
        &json!({"from": "nope", "to": "my_proj"}),
    );
    assert_eq!(missing["success"], false);
    assert!(missing["error"].as_str().unwrap().contains("nope"));
}

#[test]
fn palace_yaml_follows_a_renamed_project_wing() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = dir.path().join("palace.yaml");
    std::fs::write(&yaml, "# my project\nwing: old_name\nrooms:\n- name: src\n").unwrap();

    let changed = sync_palace_yaml(dir.path(), "old_name", "new_name").unwrap();

    assert_eq!(changed.as_deref(), Some(yaml.as_path()));
    assert_eq!(
        std::fs::read_to_string(&yaml).unwrap(),
        "# my project\nwing: new_name\nrooms:\n- name: src\n"
    );
    assert_eq!(
        sync_palace_yaml(dir.path(), "old_name", "newer").unwrap(),
        None,
        "a palace.yaml naming another wing is left alone"
    );
}
