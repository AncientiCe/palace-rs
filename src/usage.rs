//! Local usage telemetry for MemPalace MCP calls.
//!
//! The recorder is best-effort: failures never change MCP tool behavior.

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct UsageSession {
    pub session_id: String,
    pub project: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub ts: String,
    pub session_id: String,
    pub project: String,
    pub tool: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub query_hash: Option<String>,
    pub result_count: i64,
    pub top_similarity: Option<f64>,
    pub bytes_returned: i64,
    pub est_tokens_saved: i64,
    pub duration_ms: i64,
    pub outcome: String,
    pub meta: Value,
}

impl UsageSession {
    pub fn new() -> Self {
        let now = Utc::now();
        let seed = format!(
            "{}:{}:{}",
            now.to_rfc3339(),
            std::process::id(),
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        );
        let hash = blake3::hash(seed.as_bytes());
        Self {
            session_id: format!("session_{}", &hash.to_hex()[..16]),
            project: resolve_project(),
        }
    }
}

impl Default for UsageSession {
    fn default() -> Self {
        Self::new()
    }
}

pub fn record_event(
    conn: &Connection,
    session: &UsageSession,
    tool: &str,
    args: &Value,
    result: &Value,
    duration: Duration,
) -> Result<()> {
    if telemetry_disabled() {
        return Ok(());
    }

    let mut event = classify_event(conn, session, tool, args, result, duration)?;
    mark_repeat(conn, &mut event)?;
    insert_event(conn, &event)
}

pub fn insert_event(conn: &Connection, event: &UsageEvent) -> Result<()> {
    let meta = serde_json::to_string(&event.meta).unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT INTO usage_events
         (ts, session_id, project, tool, wing, room, query_hash, result_count, top_similarity,
          bytes_returned, est_tokens_saved, duration_ms, outcome, meta)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event.ts,
            event.session_id,
            event.project,
            event.tool,
            event.wing,
            event.room,
            event.query_hash,
            event.result_count,
            event.top_similarity,
            event.bytes_returned,
            event.est_tokens_saved,
            event.duration_ms,
            event.outcome,
            meta,
        ],
    )
    .context("recording usage event")?;
    Ok(())
}

fn classify_event(
    conn: &Connection,
    session: &UsageSession,
    tool: &str,
    args: &Value,
    result: &Value,
    duration: Duration,
) -> Result<UsageEvent> {
    let bytes_returned = serde_json::to_vec(result)
        .map(|bytes| bytes.len() as i64)
        .unwrap_or_default();
    let mut event = UsageEvent {
        ts: Utc::now().to_rfc3339(),
        session_id: session.session_id.clone(),
        project: session.project.clone(),
        tool: tool.to_string(),
        wing: str_arg(args, "wing"),
        room: str_arg(args, "room"),
        query_hash: query_hash(args),
        result_count: 0,
        top_similarity: None,
        bytes_returned,
        est_tokens_saved: 0,
        duration_ms: duration.as_millis().min(i64::MAX as u128) as i64,
        outcome: "noop".to_string(),
        meta: json!({}),
    };

    if result.get("error").is_some() {
        event.outcome = "error".to_string();
        return Ok(event);
    }

    match tool {
        "palace_search" => classify_search(&mut event, result),
        "palace_check_duplicate" => classify_check_duplicate(conn, &mut event, result)?,
        "palace_add_drawer" => classify_add_drawer(conn, &mut event, result)?,
        "palace_kg_query" => classify_kg_query(&mut event, result),
        "palace_diary_read" | "palace_diary_search" | "palace_session_context" => {
            classify_diary_recall(&mut event, result)
        }
        "palace_diary_write" => classify_diary_write(&mut event, args),
        "palace_delete_drawer" => classify_delete_drawer(&mut event, args),
        _ => {}
    }

    Ok(event)
}

fn classify_search(event: &mut UsageEvent, result: &Value) {
    let results = result
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    event.result_count = results.len() as i64;
    if let Some(first) = results.first() {
        event.outcome = "hit".to_string();
        event.top_similarity = first.get("similarity").and_then(Value::as_f64);
        event.wing = event.wing.clone().or_else(|| str_arg(first, "wing"));
        event.room = event.room.clone().or_else(|| str_arg(first, "room"));
        event.est_tokens_saved = event.bytes_returned / 4;
        let top_drawer_ids = results
            .iter()
            .filter_map(|value| value.get("id").and_then(Value::as_str))
            .take(5)
            .collect::<Vec<_>>();
        event.meta = json!({
            "query_id": result.get("query_id").and_then(Value::as_str),
            "intent": result.get("intent").and_then(Value::as_str),
            "rerank_enabled": result.get("rerank_enabled").and_then(Value::as_bool).unwrap_or(false),
            "top_drawer_ids": top_drawer_ids,
            "top_drawer_id": top_drawer_ids.first().copied(),
        });
    } else {
        event.outcome = "miss".to_string();
    }
}

fn classify_diary_write(event: &mut UsageEvent, args: &Value) {
    event.outcome = "diary_write".to_string();
    let entry = str_arg(args, "entry").unwrap_or_default();
    let drawer_ids = extract_drawer_ids(&entry);
    if !drawer_ids.is_empty() {
        event.meta = json!({ "referenced_drawer_ids": drawer_ids });
    }
}

fn classify_delete_drawer(event: &mut UsageEvent, args: &Value) {
    event.outcome = "delete_drawer".to_string();
    if let Some(drawer_id) = str_arg(args, "id") {
        event.meta = json!({ "drawer_id": drawer_id });
    }
}

fn extract_drawer_ids(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|part| part.starts_with("drawer_") || part.starts_with("diary_"))
        .map(ToOwned::to_owned)
        .collect()
}

fn classify_check_duplicate(
    conn: &Connection,
    event: &mut UsageEvent,
    result: &Value,
) -> Result<()> {
    let matches = result
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    event.result_count = matches.len() as i64;
    event.top_similarity = matches
        .first()
        .and_then(|value| value.get("similarity"))
        .and_then(Value::as_f64);
    if result
        .get("is_duplicate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        event.outcome = "duplicate_skip".to_string();
        event.est_tokens_saved = average_drawer_tokens(conn)?;
    } else {
        event.outcome = "miss".to_string();
    }
    Ok(())
}

fn classify_add_drawer(conn: &Connection, event: &mut UsageEvent, result: &Value) -> Result<()> {
    event.wing = event.wing.clone().or_else(|| str_arg(result, "wing"));
    event.room = event.room.clone().or_else(|| str_arg(result, "room"));
    if str_arg(result, "reason").as_deref() == Some("duplicate") {
        let matches = result
            .get("matches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        event.result_count = matches.len() as i64;
        event.top_similarity = matches
            .first()
            .and_then(|value| value.get("similarity"))
            .and_then(Value::as_f64);
        event.outcome = "duplicate_skip".to_string();
        event.est_tokens_saved = average_drawer_tokens(conn)?;
    }
    Ok(())
}

fn classify_kg_query(event: &mut UsageEvent, result: &Value) {
    let count = result
        .get("count")
        .and_then(Value::as_i64)
        .or_else(|| {
            result
                .get("facts")
                .and_then(Value::as_array)
                .map(|facts| facts.len() as i64)
        })
        .or_else(|| {
            result
                .get("relationships")
                .and_then(Value::as_array)
                .map(|facts| facts.len() as i64)
        })
        .or_else(|| {
            result
                .get("triples")
                .and_then(Value::as_array)
                .map(|facts| facts.len() as i64)
        })
        .unwrap_or_default();
    event.result_count = count;
    if count > 0 {
        event.outcome = "kg_fact".to_string();
        event.est_tokens_saved = event.bytes_returned / 4;
        event.meta = json!({"facts": count});
    } else {
        event.outcome = "miss".to_string();
    }
}

fn classify_diary_recall(event: &mut UsageEvent, result: &Value) {
    let count = result
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| entries.len() as i64)
        .or_else(|| {
            result
                .get("results")
                .and_then(Value::as_array)
                .map(|entries| entries.len() as i64)
        })
        .or_else(|| {
            result
                .get("last_session")
                .and_then(Value::as_array)
                .map(|entries| entries.len() as i64)
        })
        .unwrap_or_default();
    event.result_count = count;
    if count > 0 {
        event.outcome = "diary_recall".to_string();
        event.est_tokens_saved = event.bytes_returned / 4;
    } else {
        event.outcome = "miss".to_string();
    }
}

fn mark_repeat(conn: &Connection, event: &mut UsageEvent) -> Result<()> {
    let Some(hash) = event.query_hash.as_deref() else {
        return Ok(());
    };
    let since = (Utc::now() - ChronoDuration::hours(24)).to_rfc3339();
    let seen: Option<i64> = conn
        .query_row(
            "SELECT id FROM usage_events
             WHERE query_hash = ?1 AND datetime(ts) >= datetime(?2)
             ORDER BY ts DESC LIMIT 1",
            params![hash, since],
            |row| row.get(0),
        )
        .optional()
        .context("checking repeated usage query")?;
    if seen.is_some() {
        let mut meta = event.meta.as_object().cloned().unwrap_or_default();
        meta.insert("is_repeat".to_string(), json!(true));
        event.meta = Value::Object(meta);
    }
    Ok(())
}

fn average_drawer_tokens(conn: &Connection) -> Result<i64> {
    let avg_bytes: Option<f64> = conn
        .query_row("SELECT AVG(LENGTH(content)) FROM drawers", [], |row| {
            row.get(0)
        })
        .optional()
        .context("estimating average drawer size")?;
    Ok(avg_bytes.unwrap_or(0.0).round() as i64 / 4)
}

fn query_hash(args: &Value) -> Option<String> {
    let raw = str_arg(args, "query")
        .or_else(|| str_arg(args, "entity"))
        .or_else(|| str_arg(args, "content"))?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    let hash = blake3::hash(normalized.to_lowercase().as_bytes());
    Some(hash.to_hex().to_string())
}

fn str_arg(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_project() -> String {
    std::env::var("PALACE_PROJECT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::current_dir().ok().and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "global".to_string())
}

fn telemetry_disabled() -> bool {
    std::env::var("PALACE_GAIN_DISABLED")
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
