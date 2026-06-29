//! Cursor hook response builders for automatic, model-independent memory use.
//!
//! These power three globally-installed Cursor hooks (see [`crate::install`]):
//!
//! - `sessionStart` — inject the protocol and export the session id.
//! - `postToolUse` — recall relevant memory right when the agent investigates
//!   (e.g. before/while it greps), so prior decisions surface automatically.
//! - `stop` — when a session engaged Palace but never recorded anything, return
//!   a follow-up message so the agent saves its investigation before finishing.
//!
//! The builders are pure functions over a [`Connection`] and the hook input
//! JSON so they can be unit-tested without spawning Cursor. All paths fail open:
//! when in doubt they return an empty object and never block the agent.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::config::PalaceConfig;

/// The agent client a hook is running for. Determines the output dialect:
/// Cursor uses its own keys (`additional_context`, `followup_message`, `env`)
/// while Claude Code and Codex share a "Claude-style" dialect
/// (`hookSpecificOutput.additionalContext`, `decision`/`reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookClient {
    Cursor,
    Claude,
    Codex,
}

impl HookClient {
    /// Parse a `--client` flag value, defaulting to Cursor for back-compat.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "claude" | "claude-code" => Self::Claude,
            "codex" => Self::Codex,
            _ => Self::Cursor,
        }
    }

    /// Whether this client uses the Claude-style hook output dialect.
    fn claude_style(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }
}

/// Minimum combined similarity for a recalled memory to be worth injecting.
const RECALL_RELEVANCE_THRESHOLD: f64 = 0.3;
/// How many memories to surface in an auto-recall injection.
const RECALL_LIMIT: usize = 3;
/// Window the `stop` hook inspects to decide whether the session saved its work.
const SAVE_WINDOW_MINUTES: i64 = 360;

/// The save-nudge instruction shared by every client's `stop` hook.
const SAVE_NUDGE: &str = "Before finishing: you investigated using Palace memory this session but \
    recorded nothing. Save it so the next agent benefits — call palace_diary_write with what you \
    investigated and decided, and palace_kg_add for any durable facts or decisions (use \
    palace_kg_invalidate first if a fact changed). Then stop.";

/// Build the session-start response: inject the protocol AND — when a palace DB
/// is available — real warm-start recall (recent diary + project knowledge for
/// the session's cwd) so memory is in context deterministically, without the
/// agent having to call any palace tool. Cursor additionally exports
/// `PALACE_SESSION_ID` so later hooks can correlate the session; Claude/Codex
/// use the `hookSpecificOutput` shape instead. Fail-open: a missing/empty DB
/// just yields the protocol text alone.
pub fn session_start_response(
    conn: Option<&Connection>,
    input: &Value,
    client: HookClient,
) -> Result<String> {
    let mut context = format!(
        "# Palace Memory Protocol — MANDATORY\n\n{}",
        crate::install::RULE_BODY
    );
    if let Some(conn) = conn {
        let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("");
        if let Some(recall) = session_start_recall(conn, cwd) {
            context.push_str("\n\n");
            context.push_str(&recall);
        }
    }
    let response = if client.claude_style() {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": context,
            }
        })
    } else {
        let mut response = json!({ "additional_context": context });
        if let Some(session_id) = input.get("session_id").and_then(Value::as_str) {
            if !session_id.is_empty() {
                response["env"] = json!({ "PALACE_SESSION_ID": session_id });
            }
        }
        response
    };
    Ok(serde_json::to_string(&response)?)
}

/// Build the post-tool-use response: inject relevant memory when the tool looks
/// like an investigation and Palace has something pertinent; otherwise `{}`.
pub fn post_tool_use_response(conn: &Connection, input: &Value, client: HookClient) -> Value {
    let Some(context) = recall_context(conn, input) else {
        return json!({});
    };
    if client.claude_style() {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": context,
            }
        })
    } else {
        json!({ "additional_context": context })
    }
}

/// Compute the recall context string for an investigation tool, or `None` when
/// the tool is not an investigation or Palace has nothing pertinent.
fn recall_context(conn: &Connection, input: &Value) -> Option<String> {
    let query = investigation_query(input)?;
    let results = crate::searcher::search_memories(conn, &query, None, None, RECALL_LIMIT);
    let hits = results.get("results").and_then(Value::as_array)?;

    let mut lines: Vec<String> = Vec::new();
    for hit in hits {
        let similarity = hit
            .get("similarity")
            .and_then(Value::as_f64)
            .or_else(|| hit.get("combined").and_then(Value::as_f64))
            .unwrap_or(0.0);
        if similarity < RECALL_RELEVANCE_THRESHOLD {
            continue;
        }
        let text = hit.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let wing = hit.get("wing").and_then(Value::as_str).unwrap_or("");
        let room = hit.get("room").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("- [{wing}/{room}] {}", compact(text, 240)));
    }

    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Palace memory (recall before re-investigating or re-deciding — a prior agent may have \
         already done this):\n{}",
        lines.join("\n")
    ))
}

/// How many recent diary entries / project drawers to surface at session start.
const SESSION_START_RECALL_LIMIT: usize = 3;
/// How far back the session-start diary lookback reaches (days → hours).
const SESSION_START_DIARY_HOURS: i64 = 24 * 30;

/// Build the warm-start recall block injected at session start, or `None` when
/// there is nothing relevant. Pulls recent diary entries for the session's
/// project (any agent — cross-agent continuity) plus the top drawers of the
/// wing the cwd maps to. Mirrors `palace_session_context` so the agent sees the
/// same warm-start context without having to call the tool.
fn session_start_recall(conn: &Connection, cwd: &str) -> Option<String> {
    if cwd.trim().is_empty() {
        return None;
    }
    let path = std::path::Path::new(cwd);
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let project = canonical.to_string_lossy().to_string();
    let wing = crate::miner::wing_slug_from_dir(&canonical);

    let mut sections: Vec<String> = Vec::new();

    // Recent diary for this project, any agent — the high-value warm-start signal.
    let diary = crate::mcp_server::recent_diary_drawers(
        conn,
        None,
        Some(&project),
        SESSION_START_DIARY_HOURS,
        SESSION_START_RECALL_LIMIT,
    );
    if !diary.is_empty() {
        let mut lines =
            vec!["Recent diary for this project (what prior agents did here):".to_string()];
        for d in &diary {
            let topic = d
                .metadata
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or("general");
            let date = d.filed_at.get(..10).unwrap_or(&d.filed_at);
            lines.push(format!("- [{date}] ({topic}) {}", compact(&d.content, 220)));
        }
        sections.push(lines.join("\n"));
    }

    // Top drawers of the wing this cwd maps to (project knowledge, if mined).
    let project_drawers = crate::store::list_drawers(
        conn,
        &crate::store::DrawerFilter {
            wing: Some(wing.clone()),
            room: None,
        },
        SESSION_START_RECALL_LIMIT,
    )
    .unwrap_or_default();
    let project_drawers: Vec<_> = project_drawers
        .into_iter()
        .filter(|d| d.room != "diary")
        .collect();
    if !project_drawers.is_empty() {
        let mut lines = vec![format!("Project knowledge (wing `{wing}`):")];
        for d in &project_drawers {
            lines.push(format!("- [{}] {}", d.room, compact(&d.content, 200)));
        }
        sections.push(lines.join("\n"));
    }

    if sections.is_empty() {
        return None;
    }
    Some(format!(
        "# Recalled from Palace memory (this project — consult before re-investigating or \
         re-deciding; a prior agent may have already done this)\n{}",
        sections.join("\n\n")
    ))
}

/// Build the stop response: ask the agent to record its work when the session
/// engaged Palace but saved nothing; otherwise `{}` (no auto-continue).
pub fn stop_response(conn: &Connection, input: &Value, client: HookClient) -> Value {
    if stop_should_skip(input, client) {
        return json!({});
    }
    match crate::usage::recent_save_status(conn, SAVE_WINDOW_MINUTES) {
        Ok((engaged, saved)) if engaged && !saved => {
            if client.claude_style() {
                json!({ "decision": "block", "reason": SAVE_NUDGE })
            } else {
                json!({ "followup_message": SAVE_NUDGE })
            }
        }
        _ => json!({}),
    }
}

/// Whether the stop hook should stay silent: already continuing from a prior
/// nudge, or — for Cursor — the turn did not complete cleanly.
fn stop_should_skip(input: &Value, client: HookClient) -> bool {
    // Claude/Codex pass `stop_hook_active`; Cursor passes `loop_count`. Either
    // signalling "we already triggered a follow-up" means stop nudging so we
    // never loop forever.
    if input
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if input.get("loop_count").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return true;
    }
    if !client.claude_style() {
        let status = input.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "completed" {
            return true;
        }
    }
    false
}

/// Run a hook end-to-end: read the JSON event from `stdin`, dispatch on the
/// event name, and print the response object to `stdout`. Fail-open: unknown
/// events and errors yield an empty object so the agent is never blocked.
pub fn run(event: &str, client: HookClient) -> Result<()> {
    use std::io::Read;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();
    let input: Value = serde_json::from_str(raw.trim()).unwrap_or_else(|_| json!({}));

    let response = match event {
        "session-start" | "sessionStart" | "SessionStart" => {
            // Open the DB for warm-start recall; fail-open to protocol-only.
            let conn = open_palace();
            session_start_response(conn.as_ref(), &input, client)?
        }
        "post-tool-use" | "postToolUse" | "PostToolUse" => {
            serde_json::to_string(&with_db(|conn| {
                post_tool_use_response(conn, &input, client)
            }))?
        }
        "stop" | "Stop" => {
            serde_json::to_string(&with_db(|conn| stop_response(conn, &input, client)))?
        }
        other => {
            tracing::warn!(event = other, "palace hook: unknown event, ignoring");
            "{}".to_string()
        }
    };
    println!("{response}");
    Ok(())
}

/// Open the palace DB and run `f`, returning `{}` if the DB cannot be opened
/// (fail-open: a missing palace must never block the agent).
fn with_db<F>(f: F) -> Value
where
    F: FnOnce(&Connection) -> Value,
{
    match open_palace() {
        Some(conn) => f(&conn),
        None => json!({}),
    }
}

/// Open the palace DB, or `None` if it cannot be opened (fail-open).
fn open_palace() -> Option<Connection> {
    let config = PalaceConfig::new();
    match crate::db::open(&config.palace_db_path()) {
        Ok(conn) => Some(conn),
        Err(err) => {
            tracing::warn!(error = %err, "palace hook: could not open palace, skipping");
            None
        }
    }
}

/// Extract a search query from an investigation tool's input, or `None` if the
/// tool is not an investigation (so non-search tools stay silent).
fn investigation_query(input: &Value) -> Option<String> {
    let tool_input = input.get("tool_input");
    let candidate = tool_input
        .and_then(|ti| ti.get("pattern").and_then(Value::as_str)) // Grep / Glob
        .or_else(|| tool_input.and_then(|ti| ti.get("query").and_then(Value::as_str)))
        .or_else(|| tool_input.and_then(|ti| ti.get("command").and_then(Value::as_str))) // Codex/Bash shell
        .or_else(|| tool_input.and_then(|ti| ti.get("file_path").and_then(Value::as_str))) // Claude Read/Edit
        .or_else(|| tool_input.and_then(|ti| ti.get("path").and_then(Value::as_str))) // Grep/Glob path scope
        .or_else(|| tool_input.and_then(|ti| ti.get("file").and_then(Value::as_str)));
    let text = candidate?.trim();
    if text.len() < 3 {
        return None;
    }
    Some(text.to_string())
}

fn compact(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(tool_input: Value) -> Option<String> {
        investigation_query(&json!({ "tool_input": tool_input }))
    }

    #[test]
    fn investigation_query_extracts_each_investigation_tool_field() {
        assert_eq!(
            q(json!({"pattern": "retry loop"})).as_deref(),
            Some("retry loop")
        ); // Grep/Glob
        assert_eq!(q(json!({"command": "rg dlq"})).as_deref(), Some("rg dlq")); // Codex/Bash
                                                                                // Regression: Claude's Read passes `file_path`. The installed matcher
                                                                                // fires the hook for Read, so we MUST extract it or recall stays silent.
        assert_eq!(
            q(json!({"file_path": "/proj/src/dlq_consumer.rs"})).as_deref(),
            Some("/proj/src/dlq_consumer.rs")
        );
    }

    #[test]
    fn investigation_query_silent_for_non_investigation_tools() {
        // No recognised field (e.g. a Write with only `content`) → no recall.
        assert_eq!(q(json!({"content": "hello"})), None);
        // Too-short query is not worth a search.
        assert_eq!(q(json!({"pattern": "x"})), None);
        // Missing tool_input entirely.
        assert_eq!(investigation_query(&json!({})), None);
    }
}
