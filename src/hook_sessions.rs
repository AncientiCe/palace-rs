//! Per-session activity recorded by the agent hooks.
//!
//! The MCP server's usage telemetry is keyed by its own process, not by the
//! agent's session, so it cannot tell whether *this* session consulted Palace.
//! The hooks receive the agent's `session_id`, so they record what each
//! session did here: prompts, tool calls, and whether it used or saved to
//! Palace. The prompt and stop hooks read it back to remind and nudge the
//! right session. All writes are best-effort; callers ignore errors.

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::usage::SAVE_TOOLS;

/// MCP tool-name prefix under which agents expose Palace's tools.
const PALACE_TOOL_PREFIX: &str = "mcp__palace__";
/// Cursor reports MCP calls as `MCP:<tool>` with no server name.
const CURSOR_MCP_PREFIX: &str = "MCP:";
/// Sessions untouched for this long are pruned.
const RETENTION_DAYS: i64 = 7;

/// What one agent session has done so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionActivity {
    pub prompts: i64,
    pub tool_calls: i64,
    pub palace_used: bool,
    pub saved: bool,
}

/// The Palace tool name behind an agent's MCP tool name, if it is one.
pub fn palace_tool(tool_name: &str) -> Option<&str> {
    tool_name.strip_prefix(PALACE_TOOL_PREFIX).or_else(|| {
        tool_name
            .strip_prefix(CURSOR_MCP_PREFIX)
            .filter(|tool| tool.starts_with("palace_"))
    })
}

/// Record a submitted prompt, pruning long-idle sessions.
pub fn record_prompt(conn: &Connection, session_id: &str, cwd: &str) -> Result<()> {
    upsert(conn, session_id, cwd, 1, 0, false, false)?;
    let cutoff = (Utc::now() - ChronoDuration::days(RETENTION_DAYS)).to_rfc3339();
    conn.execute(
        "DELETE FROM hook_sessions WHERE updated_at < ?1",
        params![cutoff],
    )
    .context("pruning idle hook sessions")?;
    Ok(())
}

/// Record a tool call; Palace tools mark the session as having used (and,
/// for save tools, saved to) Palace.
pub fn record_tool(conn: &Connection, session_id: &str, cwd: &str, tool_name: &str) -> Result<()> {
    let palace = palace_tool(tool_name);
    let saved = palace.is_some_and(|tool| SAVE_TOOLS.contains(&tool));
    upsert(conn, session_id, cwd, 0, 1, palace.is_some(), saved)
}

/// The recorded activity for `session_id`, or `None` if the hooks never saw it.
pub fn activity(conn: &Connection, session_id: &str) -> Result<Option<SessionActivity>> {
    conn.query_row(
        "SELECT prompts, tool_calls, palace_used, saved FROM hook_sessions WHERE session_id = ?1",
        params![session_id],
        |row| {
            Ok(SessionActivity {
                prompts: row.get(0)?,
                tool_calls: row.get(1)?,
                palace_used: row.get(2)?,
                saved: row.get(3)?,
            })
        },
    )
    .optional()
    .context("reading hook session activity")
}

fn upsert(
    conn: &Connection,
    session_id: &str,
    cwd: &str,
    prompts: i64,
    tool_calls: i64,
    palace_used: bool,
    saved: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO hook_sessions
             (session_id, cwd, prompts, tool_calls, palace_used, saved, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id) DO UPDATE SET
             cwd         = excluded.cwd,
             prompts     = prompts + excluded.prompts,
             tool_calls  = tool_calls + excluded.tool_calls,
             palace_used = MAX(palace_used, excluded.palace_used),
             saved       = MAX(saved, excluded.saved),
             updated_at  = excluded.updated_at",
        params![
            session_id,
            cwd,
            prompts,
            tool_calls,
            palace_used,
            saved,
            Utc::now().to_rfc3339()
        ],
    )
    .context("recording hook session activity")?;
    Ok(())
}
