//! Tests for per-session activity recorded by the agent hooks.

use palace::hook_sessions::{activity, palace_tool, record_prompt, record_tool};
use palace::hooks::{post_tool_use_response, user_prompt_submit_response, HookClient};
use serde_json::{json, Value};

fn test_db() -> rusqlite::Connection {
    palace::db::open_in_memory().unwrap()
}

#[test]
fn unknown_session_has_no_activity() {
    let conn = test_db();
    assert!(activity(&conn, "nope").unwrap().is_none());
}

#[test]
fn prompts_and_tool_calls_accumulate_per_session() {
    let conn = test_db();
    record_prompt(&conn, "s1", "/work/proj").unwrap();
    record_prompt(&conn, "s1", "/work/proj").unwrap();
    record_tool(&conn, "s1", "/work/proj", "Grep").unwrap();

    let s1 = activity(&conn, "s1").unwrap().unwrap();
    assert_eq!(s1.prompts, 2);
    assert_eq!(s1.tool_calls, 1);
    assert!(!s1.palace_used);
    assert!(!s1.saved);
}

#[test]
fn palace_calls_mark_used_and_saves_mark_saved() {
    let conn = test_db();
    record_tool(&conn, "s1", "/work/proj", "mcp__palace__palace_status").unwrap();
    let s1 = activity(&conn, "s1").unwrap().unwrap();
    assert!(s1.palace_used);
    assert!(!s1.saved);

    record_tool(&conn, "s1", "/work/proj", "mcp__palace__palace_diary_write").unwrap();
    let s1 = activity(&conn, "s1").unwrap().unwrap();
    assert!(s1.palace_used);
    assert!(s1.saved);
}

#[test]
fn sessions_are_isolated() {
    let conn = test_db();
    record_tool(&conn, "s1", "/work/a", "mcp__palace__palace_diary_write").unwrap();
    record_prompt(&conn, "s2", "/work/b").unwrap();

    let s2 = activity(&conn, "s2").unwrap().unwrap();
    assert!(
        !s2.palace_used,
        "another session's palace use must not count"
    );
    assert!(!s2.saved);
}

#[test]
fn palace_tool_strips_the_mcp_prefix() {
    assert_eq!(
        palace_tool("mcp__palace__palace_search"),
        Some("palace_search")
    );
    assert_eq!(palace_tool("Grep"), None);
    assert_eq!(palace_tool("mcp__other__palace_search"), None);
}

fn tool_call(tool: &str) -> Value {
    json!({
        "session_id": "s1",
        "cwd": "/work/proj",
        "tool_name": tool,
        "tool_input": {"query": "tokio http server decision"},
    })
}

#[test]
fn post_tool_use_records_palace_calls_without_recalling() {
    let conn = test_db();
    let out = post_tool_use_response(
        &conn,
        &tool_call("mcp__palace__palace_search"),
        HookClient::Claude,
    );
    assert_eq!(out, json!({}), "a palace call must not trigger recall");
    assert!(activity(&conn, "s1").unwrap().unwrap().palace_used);
}

#[test]
fn user_prompt_submit_records_every_prompt() {
    let conn = test_db();
    for text in ["go ahead", "/code-review high"] {
        user_prompt_submit_response(
            &conn,
            &json!({ "prompt": text, "cwd": "/work/proj", "session_id": "s1" }),
            HookClient::Claude,
        );
    }
    assert_eq!(activity(&conn, "s1").unwrap().unwrap().prompts, 2);
}

#[test]
fn hooks_without_session_id_record_nothing() {
    let conn = test_db();
    user_prompt_submit_response(
        &conn,
        &json!({ "prompt": "go ahead", "cwd": "/work/proj" }),
        HookClient::Claude,
    );
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM hook_sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

// ── per-prompt reminder ────────────────────────────────────────────────────

fn session_prompt(text: &str) -> Value {
    json!({ "prompt": text, "cwd": "/work/proj", "session_id": "s1" })
}

fn context(out: &Value) -> &str {
    out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("")
}

const PLAIN_PROMPT: &str = "add a flag to print the version string";

#[test]
fn prompt_reminds_claude_until_palace_is_used() {
    let conn = test_db();
    let out = user_prompt_submit_response(&conn, &session_prompt(PLAIN_PROMPT), HookClient::Claude);
    assert!(
        context(&out).contains("Palace not consulted yet this session"),
        "{out}"
    );

    record_tool(&conn, "s1", "/work/proj", "mcp__palace__palace_status").unwrap();
    let out = user_prompt_submit_response(&conn, &session_prompt(PLAIN_PROMPT), HookClient::Claude);
    assert_eq!(out, json!({}), "no reminder once palace was used: {out}");
}

#[test]
fn prompt_reminder_skips_trivial_prompts() {
    let conn = test_db();
    for text in ["go ahead", "/code-review high"] {
        let out = user_prompt_submit_response(&conn, &session_prompt(text), HookClient::Claude);
        assert_eq!(out, json!({}), "{text:?} must not get a reminder");
    }
}

#[test]
fn prompt_reminder_only_for_clients_with_session_tracking() {
    let conn = test_db();
    let out = user_prompt_submit_response(&conn, &session_prompt(PLAIN_PROMPT), HookClient::Codex);
    assert_eq!(
        out,
        json!({}),
        "codex palace calls are not tracked yet: {out}"
    );
}
