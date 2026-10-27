//! Tests for the Cursor hook response builders that drive automatic
//! memory-in (recall on investigation) and memory-out (save on stop).

use chrono::Utc;
use palace::hooks::{
    normalize_input, post_tool_use_response, session_start_response, stop_response,
    user_prompt_submit_response, HookClient,
};
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

fn remember(conn: &rusqlite::Connection, text: &str, wing: &str) {
    let config = palace::config::PalaceConfig::new();
    palace::mcp_server::dispatch_tool(
        conn,
        &config,
        "palace_remember",
        &json!({ "text": text, "wing": wing, "room": "decisions" }),
    );
}

const TOKIO_DECISION: &str =
    "We chose tokio over async-std for the HTTP server because of ecosystem maturity.";

fn grep_tokio(cwd: &str) -> Value {
    json!({
        "tool_name": "Grep",
        "tool_input": {"pattern": "tokio async-std http server runtime choice"},
        "cwd": cwd
    })
}

#[test]
fn post_tool_use_drops_other_projects_drawers() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "otherproj");
    palace::store::set_wing_mined(&conn, "otherproj", "/work/otherproj").unwrap();

    let out = post_tool_use_response(&conn, &grep_tokio("/work/proj"), HookClient::Claude);
    assert!(
        out.get("hookSpecificOutput").is_none(),
        "another mined project's drawer must not leak into this project: {out}"
    );

    // The same drawer is relevant inside its own project.
    let out = post_tool_use_response(&conn, &grep_tokio("/work/otherproj"), HookClient::Claude);
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(ctx.contains("tokio"), "{out}");
}

#[test]
fn post_tool_use_keeps_topic_wings_and_tags_drawer_id() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");

    let out = post_tool_use_response(&conn, &grep_tokio("/work/proj"), HookClient::Claude);
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(ctx.contains("tokio"), "topic wings stay in scope: {out}");
    assert!(
        ctx.contains("(drawer_"),
        "recall must cite the drawer id: {out}"
    );
}

#[test]
fn post_tool_use_drops_diary_recorded_for_another_project() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &json!({
            "agent_name": "claude",
            "entry": TOKIO_DECISION,
            "project_path": "/work/otherproj",
        }),
    );

    let out = post_tool_use_response(&conn, &grep_tokio("/work/proj"), HookClient::Claude);
    assert!(out.get("hookSpecificOutput").is_none(), "{out}");

    let out = post_tool_use_response(&conn, &grep_tokio("/work/otherproj"), HookClient::Claude);
    assert!(out.get("hookSpecificOutput").is_some(), "{out}");
}

// ── user-prompt-submit (recall on the question itself) ────────────────────

fn prompt(text: &str) -> Value {
    json!({ "prompt": text, "cwd": "/work/proj", "session_id": "abc" })
}

#[test]
fn user_prompt_submit_injects_recall_for_the_question() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");

    let out = user_prompt_submit_response(
        &conn,
        &prompt("why did we pick tokio over async-std for the http server?"),
        HookClient::Claude,
    );
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("tokio"),
        "the prior decision must surface: {out}"
    );
    assert!(ctx.contains("(drawer_"), "{out}");
}

#[test]
fn user_prompt_submit_nudges_history_questions_without_hits() {
    let conn = test_db();
    let out = user_prompt_submit_response(
        &conn,
        &prompt("what happened last time we touched the release pipeline?"),
        HookClient::Codex,
    );
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(ctx.contains("palace_search"), "{out}");
}

#[test]
fn user_prompt_submit_silent_for_plain_or_trivial_prompts() {
    let conn = test_db();
    // The session already consulted Palace, so only recall could inject here.
    palace::hook_sessions::record_tool(&conn, "abc", "/work/proj", "mcp__palace__palace_status")
        .unwrap();
    for text in [
        "go ahead",
        "/code-review high",
        "add a flag to print the version string",
    ] {
        let out = user_prompt_submit_response(&conn, &prompt(text), HookClient::Claude);
        assert_eq!(out, json!({}), "{text:?} must not inject anything");
    }
}

#[test]
fn user_prompt_submit_is_empty_for_cursor() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    let out = user_prompt_submit_response(
        &conn,
        &prompt("why did we pick tokio over async-std for the http server?"),
        HookClient::Cursor,
    );
    assert_eq!(out, json!({}), "cursor cannot inject prompt context");
}

// ── stop (auto-save enforcement) ──────────────────────────────────────────

#[test]
fn stop_nudges_when_engaged_but_not_saved() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();

    let out: Value = stop_response(&conn, &json!({"loop_count": 0}), HookClient::OpenCode);
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

    let out: Value = stop_response(&conn, &json!({"loop_count": 0}), HookClient::OpenCode);
    assert!(
        out.get("followup_message").is_none(),
        "no nudge once the session recorded memory: {out}"
    );
}

#[test]
fn stop_silent_when_no_palace_engagement() {
    let conn = test_db();
    let out: Value = stop_response(&conn, &json!({"loop_count": 0}), HookClient::OpenCode);
    assert!(
        out.get("followup_message").is_none(),
        "trivial sessions with no palace use are not nudged: {out}"
    );
}

#[test]
fn stop_silent_for_cursor_even_when_engaged_but_not_saved() {
    // Cursor posts a followup_message as a new user turn, which derails the agent.
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out: Value = stop_response(
        &conn,
        &json!({"status": "completed", "loop_count": 0}),
        HookClient::Cursor,
    );
    assert_eq!(out, json!({}), "{out}");
}

#[test]
fn stop_silent_on_repeat_loop() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out: Value = stop_response(&conn, &json!({"loop_count": 1}), HookClient::OpenCode);
    assert!(
        out.get("followup_message").is_none(),
        "only nudge once per session: {out}"
    );
}

// ── session-start (env + protocol) ────────────────────────────────────────

#[test]
fn session_start_emits_session_id_env_and_protocol() {
    let raw = session_start_response(
        None,
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
    let raw = session_start_response(None, &json!({"session_id": "abc-123"}), HookClient::Claude)
        .unwrap();
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

#[test]
fn session_start_injects_recent_diary_for_project() {
    let conn = test_db();
    let config = palace::config::PalaceConfig::new();
    // A prior agent recorded a project-scoped diary entry.
    palace::mcp_server::dispatch_tool(
        &conn,
        &config,
        "palace_diary_write",
        &json!({
            "agent_name": "claude",
            "entry": "Investigated the payment_refunded DLQ and found the retry loop bug.",
            "project_path": "/proj",
            "topic": "dlq-investigation"
        }),
    );

    let raw = session_start_response(
        Some(&conn),
        &json!({"session_id": "abc-123", "cwd": "/proj"}),
        HookClient::Claude,
    )
    .unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("Palace Memory Protocol"),
        "must still inject the protocol: {out}"
    );
    assert!(
        ctx.contains("payment_refunded DLQ"),
        "session start must inject real recall of prior diary work: {out}"
    );
}

fn add_preference(conn: &rusqlite::Connection, text: &str, chunk: usize, importance: f64) {
    palace::store::add_drawer(
        conn,
        "general",
        "preferences",
        text,
        None,
        "prefs",
        chunk,
        "mcp",
        importance,
    )
    .unwrap();
}

fn claude_session_start(conn: &rusqlite::Connection, input: Value) -> String {
    let raw = session_start_response(Some(conn), &input, HookClient::Claude).unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn session_start_injects_standing_preferences_verbatim() {
    let conn = test_db();
    let rule = "Standing instruction from the user: do NOT write to the Claude Code \
                file-memory directory; Palace is the only persistent memory. This sentence \
                is deliberately long so that any compaction would cut it off before the end.";
    add_preference(&conn, rule, 0, 3.0);
    remember(&conn, TOKIO_DECISION, "proj");

    let ctx = claude_session_start(&conn, json!({"session_id": "abc", "cwd": "/work/proj"}));
    assert!(
        ctx.contains("Standing instructions (override defaults)"),
        "{ctx}"
    );
    assert!(ctx.contains(rule), "rule must be injected verbatim: {ctx}");
    let prefs_at = ctx.find("Standing instructions").unwrap();
    let recall_at = ctx.find("Recalled from Palace memory").unwrap();
    assert!(
        prefs_at < recall_at,
        "standing rules come before project recall: {ctx}"
    );
}

#[test]
fn session_start_injects_standing_preferences_without_cwd() {
    let conn = test_db();
    add_preference(&conn, "Commit preference: no co-author trailers.", 0, 3.0);
    let ctx = claude_session_start(&conn, json!({"session_id": "abc"}));
    assert!(ctx.contains("no co-author trailers"), "{ctx}");
}

#[test]
fn session_start_caps_standing_preferences_but_keeps_importance_five() {
    let conn = test_db();
    for i in 0..25 {
        add_preference(&conn, &format!("minor rule number {i:02}"), i, 3.0);
    }
    add_preference(&conn, "critical rule that must survive the cap", 99, 5.0);
    let ctx = claude_session_start(&conn, json!({"session_id": "abc"}));
    assert!(
        ctx.contains("critical rule that must survive the cap"),
        "{ctx}"
    );
    let minor = ctx.matches("minor rule number").count();
    assert_eq!(minor, 19, "cap of 20 includes the importance-5 rule: {ctx}");
}

#[test]
fn session_start_fails_open_with_empty_db() {
    let conn = test_db();
    let raw = session_start_response(
        Some(&conn),
        &json!({"session_id": "abc-123", "cwd": "/proj"}),
        HookClient::Claude,
    )
    .unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        ctx.contains("Palace Memory Protocol"),
        "empty palace must still inject the protocol: {out}"
    );
    assert!(
        !ctx.contains("Recalled from Palace memory"),
        "nothing to recall → no recall block: {out}"
    );
}

#[test]
fn session_start_no_recall_without_db() {
    // No DB handle at all (e.g. palace missing) → protocol only, no panic.
    let raw = session_start_response(
        None,
        &json!({"session_id": "abc-123", "cwd": "/proj"}),
        HookClient::Cursor,
    )
    .unwrap();
    let out: Value = serde_json::from_str(&raw).unwrap();
    let ctx = out["additional_context"].as_str().unwrap_or("");
    assert!(ctx.contains("Palace Memory Protocol"), "{out}");
    assert!(!ctx.contains("Recalled from Palace memory"), "{out}");
}

// ── per-client dialects (Gemini, VS Code, OpenCode) ───────────────────────

#[test]
fn gemini_dialect_uses_its_own_event_names() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");

    let tool = json!({
        "tool_name": "grep_search",
        "tool_input": {"pattern": "tokio async-std http server runtime choice"},
        "cwd": "/work/proj"
    });
    let out = post_tool_use_response(&conn, &tool, HookClient::Gemini);
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "AfterTool");

    let out = user_prompt_submit_response(
        &conn,
        &prompt("why did we pick tokio over async-std for the http server?"),
        HookClient::Gemini,
    );
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "BeforeAgent");
}

#[test]
fn gemini_stop_denies_to_continue() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out = stop_response(
        &conn,
        &json!({"stop_hook_active": false}),
        HookClient::Gemini,
    );
    assert_eq!(out["decision"], "deny", "{out}");
    assert!(out["reason"]
        .as_str()
        .unwrap_or("")
        .contains("palace_diary_write"));
}

#[test]
fn vscode_stop_nests_decision_in_hook_specific_output() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    let out = stop_response(
        &conn,
        &json!({"stop_hook_active": false}),
        HookClient::Vscode,
    );
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "Stop");
    assert_eq!(out["hookSpecificOutput"]["decision"], "block", "{out}");
    assert!(out.get("decision").is_none(), "{out}");
}

#[test]
fn vscode_prompt_hook_cannot_inject() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    let out = user_prompt_submit_response(
        &conn,
        &prompt("why did we pick tokio over async-std for the http server?"),
        HookClient::Vscode,
    );
    assert_eq!(out, json!({}));
}

#[test]
fn post_tool_use_skips_mutating_tools_without_matchers() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    let edit = json!({
        "tool_name": "replace_string_in_file",
        "tool_input": {"filePath": "tokio async-std http server runtime choice"},
        "cwd": "/work/proj"
    });
    assert_eq!(
        post_tool_use_response(&conn, &edit, HookClient::Vscode),
        json!({})
    );
}

#[test]
fn opencode_uses_flat_keys_and_stops_without_status() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    let read = json!({
        "tool_name": "read",
        "tool_input": {"filePath": "tokio async-std http server runtime choice"},
        "cwd": "/work/proj"
    });
    let out = post_tool_use_response(&conn, &read, HookClient::OpenCode);
    assert!(out["additional_context"]
        .as_str()
        .unwrap_or("")
        .contains("tokio"));

    insert_event(&conn, &usage("palace_search")).unwrap();
    // OpenCode passes no `status`; Cursor's completed-turn check must not apply.
    let out = stop_response(&conn, &json!({}), HookClient::OpenCode);
    assert!(out["followup_message"]
        .as_str()
        .unwrap_or("")
        .contains("palace_diary_write"));
}

// ── Cline and Goose ───────────────────────────────────────────────────────

#[test]
fn cline_input_is_normalized_and_context_uses_context_modification() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    let raw = json!({
        "hookName": "PostToolUse",
        "taskId": "task-1",
        "workspaceRoots": ["/work/proj"],
        "postToolUse": {
            "toolName": "search_codebase",
            "parameters": {"query": "tokio async-std http server runtime choice"}
        }
    });
    let input = normalize_input(raw, HookClient::Cline);
    assert_eq!(input["cwd"], "/work/proj");
    assert_eq!(input["session_id"], "task-1");
    let out = post_tool_use_response(&conn, &input, HookClient::Cline);
    assert!(out["contextModification"]
        .as_str()
        .unwrap_or("")
        .contains("tokio"));

    let prompt = normalize_input(
        json!({
            "workspaceRoots": ["/work/proj"],
            "userPromptSubmit": {"prompt": "why did we pick tokio over async-std for the http server?"}
        }),
        HookClient::Cline,
    );
    let out = user_prompt_submit_response(&conn, &prompt, HookClient::Cline);
    assert!(out["contextModification"].is_string(), "{out}");
}

#[test]
fn cline_cannot_continue_after_completion() {
    let conn = test_db();
    insert_event(&conn, &usage("palace_search")).unwrap();
    assert_eq!(
        stop_response(&conn, &json!({}), HookClient::Cline),
        json!({})
    );
}

#[test]
fn goose_blocks_stop_but_never_injects_context() {
    let conn = test_db();
    remember(&conn, TOKIO_DECISION, "decisions");
    insert_event(&conn, &usage("palace_search")).unwrap();

    let input = normalize_input(
        json!({"event": "Stop", "session_id": "g1", "working_dir": "/work/proj"}),
        HookClient::Goose,
    );
    assert_eq!(input["cwd"], "/work/proj");
    let out = stop_response(&conn, &input, HookClient::Goose);
    assert_eq!(out["decision"], "block", "{out}");

    let out = post_tool_use_response(&conn, &grep_tokio("/work/proj"), HookClient::Goose);
    assert_eq!(out, json!({}));
}
