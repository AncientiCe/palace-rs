//! Tests for the Cursor hook response builders that drive automatic
//! memory-in (recall on investigation) and memory-out (save on stop).

use chrono::Utc;
use palace::hooks::{post_tool_use_response, session_start_response, stop_response, HookClient};
use palace::usage::{insert_event, UsageEvent};
use serde_json::{json, Value};

fn test_db() -> rusqlite::Connection {
    palace::db::open_in_memory().unwrap()
}

fn usage(tool: &str) -> UsageEvent {
    UsageEvent {
        ts: Utc::now().to_rfc3339(),
        session_id: "session_test".to_string(),
        project: "proj".to_string(),
        tool: tool.to_string(),
        wing: None,
        room: None,
        query_hash: None,
        result_count: 0,
        top_similarity: None,
        bytes_returned: 0,
        est_tokens_saved: 0,
        duration_ms: 1,
        outcome: "x".to_string(),
        meta: json!({}),
    }
}

// ── post-tool-use (auto-recall) ───────────────────────────────────────────

#[test]
fn post_tool_use_injects_recall_when_relevant_memory_exists() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_remember",
        &json!({
            "text": "We chose tokio over async-std for the HTTP server because of ecosystem maturity.",
            "wing": "decisions",
            "room": "runtime"
        }),
    );

    let input = json!({
        "tool_name": "Grep",
        "tool_input": {"pattern": "tokio async-std http server runtime choice"},
        "cwd": "/proj"
    });
    let out: Value = post_tool_use_response(&conn, &input, HookClient::Cursor);
    let ctx = out["additional_context"].as_str().unwrap_or("");
    assert!(
        ctx.to_lowercase().contains("tokio"),
        "recall should surface the prior decision: {out}"
    );
}

#[test]
fn post_tool_use_claude_dialect_uses_hook_specific_output() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_remember",
        &json!({
            "text": "We chose tokio over async-std for the HTTP server because of ecosystem maturity.",
            "wing": "decisions",
            "room": "runtime"
        }),
    );

    let input = json!({
        "tool_name": "Bash",
        "tool_input": {"command": "rg 'tokio async-std http server runtime choice'"},
        "cwd": "/proj"
    });
    let out: Value = post_tool_use_response(&conn, &input, HookClient::Codex);
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.to_lowercase().contains("tokio"),
        "claude-style recall should surface the prior decision: {out}"
    );
    assert!(
        out.get("additional_context").is_none(),
        "claude-style output must not use the cursor key: {out}"
    );
}

#[test]
fn post_tool_use_is_silent_without_relevant_memory() {
    let conn = test_db();
    let input = json!({
        "tool_name": "Grep",
        "tool_input": {"pattern": "completely unrelated needle zzzzz"},
        "cwd": "/proj"
    });
    let out: Value = post_tool_use_response(&conn, &input, HookClient::Cursor);
    assert!(
        out.get("additional_context").is_none(),
        "empty palace must not inject context: {out}"
    );
}

// ── stop (auto-save enforcement) ──────────────────────────────────────────

#[test]
fn stop_nudges_when_engaged_but_not_saved() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();

    let out: Value = stop_response(
        &conn,
        &json!({"status": "completed", "loop_count": 0}),
        HookClient::Cursor,
    );
    let msg = out["followup_message"].as_str().unwrap_or("");
    assert!(
        msg.contains("palace_diary_write"),
        "should auto-nudge the agent to record its work: {out}"
    );
}

#[test]
fn stop_claude_dialect_uses_decision_block() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();

    // Claude/Codex omit `status`; they pass `stop_hook_active`.
    let out: Value = stop_response(
        &conn,
        &json!({"stop_hook_active": false}),
        HookClient::Claude,
    );
    assert_eq!(
        out["decision"], "block",
        "claude-style stop must block: {out}"
    );
    let reason = out["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("palace_diary_write"),
        "block reason should instruct the agent to save: {out}"
    );
    assert!(out.get("followup_message").is_none(), "{out}");
}

#[test]
fn stop_claude_respects_stop_hook_active() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out: Value = stop_response(&conn, &json!({"stop_hook_active": true}), HookClient::Codex);
    assert!(
        out.get("decision").is_none(),
        "must not re-block once already continuing: {out}"
    );
}

#[test]
fn stop_silent_when_work_was_saved() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    insert_event(&conn, &usage("palace_diary_write")).unwrap();

    let out: Value = stop_response(
        &conn,
        &json!({"status": "completed", "loop_count": 0}),
        HookClient::Cursor,
    );
    assert!(
        out.get("followup_message").is_none(),
        "no nudge once the session recorded memory: {out}"
    );
}

#[test]
fn stop_silent_when_no_palace_engagement() {
    let conn = test_db();
    let out: Value = stop_response(
        &conn,
        &json!({"status": "completed", "loop_count": 0}),
        HookClient::Cursor,
    );
    assert!(
        out.get("followup_message").is_none(),
        "trivial sessions with no palace use are not nudged: {out}"
    );
}

#[test]
fn stop_silent_when_not_completed() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out: Value = stop_response(
        &conn,
        &json!({"status": "aborted", "loop_count": 0}),
        HookClient::Cursor,
    );
    assert!(out.get("followup_message").is_none(), "{out}");
}

#[test]
fn stop_silent_on_repeat_loop() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out: Value = stop_response(
        &conn,
        &json!({"status": "completed", "loop_count": 1}),
        HookClient::Cursor,
    );
    assert!(
        out.get("followup_message").is_none(),
        "only nudge once per session: {out}"
    );
}

// ── session-start (env + protocol) ────────────────────────────────────────

#[test]
fn session_start_emits_session_id_env_and_protocol() {
    let raw = session_start_response(
        &json!({"session_id": "abc-123", "composer_mode": "agent"}),
        HookClient::Cursor,
    )
    .unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(out["env"]["PALACE_SESSION_ID"], "abc-123");
    let ctx = out["additional_context"].as_str().unwrap_or("");
    assert!(
        ctx.contains("Palace Memory Protocol"),
        "session start must still inject the protocol: {out}"
    );
}

#[test]
fn session_start_claude_dialect_uses_hook_specific_output() {
    let raw =
        session_start_response(&json!({"session_id": "abc-123"}), HookClient::Claude).unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("Palace Memory Protocol"),
        "claude-style session start must inject the protocol: {out}"
    );
    assert!(
        out.get("additional_context").is_none() && out.get("env").is_none(),
        "claude-style output must not use cursor keys: {out}"
    );
}
