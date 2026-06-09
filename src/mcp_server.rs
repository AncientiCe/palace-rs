//! MCP Server — stdio JSON-RPC tool loop for AI coding assistants.
//!
//! Install: claude mcp add palace -- palace mcp

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::time::Instant;
use tracing::{error, info, warn};

use crate::config::PalaceConfig;
use crate::dialect::{AAAK_SPEC, PALACE_PROTOCOL};
use crate::knowledge_graph as kg;
use crate::palace_graph;
use crate::searcher::search_memories_with_options;
use crate::store::{
    add_drawer_with_id, check_duplicate, count_drawers, delete_drawer, diary_id, get_drawer,
    list_drawers, preference_search_filtered, room_counts, taxonomy, update_drawer_content,
    wing_counts, DrawerFilter,
};

/// Run the MCP stdio server. Blocks until stdin closes.
pub fn run() -> Result<()> {
    let config = PalaceConfig::new();

    // Remote mode: act as a transparent stdio→HTTP proxy to a shared palace-server.
    if config.mcp_mode() == "remote" {
        let url = config.remote_endpoint_url().ok_or_else(|| {
            anyhow::anyhow!(
                "Remote MCP mode is on but no endpoint is set. Run: palace remote set --endpoint <url>"
            )
        })?;
        let key = config.remote_api_key().ok_or_else(|| {
            anyhow::anyhow!(
                "Remote MCP mode is on but no API key is set. Run: palace remote set --endpoint <url> --api-key <key>"
            )
        })?;
        return crate::remote::run(&url, &key);
    }

    let db_path = config.palace_db_path();

    // Open palace DB (or create if first run)
    let conn = match crate::db::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Palace MCP: failed to open database");
            return Err(e);
        }
    };

    info!(palace = %db_path.display(), "Palace MCP server starting");
    let session = crate::usage::UsageSession::new();

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {e}")}
                });
                let mut out = stdout.lock();
                writeln!(out, "{}", err)?;
                out.flush()?;
                continue;
            }
        };

        if let Some(response) = handle_request(&conn, &config, &session, &request) {
            let mut out = stdout.lock();
            writeln!(out, "{response}")?;
            out.flush()?;
        }
    }

    Ok(())
}

fn handle_request(
    conn: &Connection,
    config: &PalaceConfig,
    session: &crate::usage::UsageSession,
    req: &Value,
) -> Option<String> {
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_default();
    let req_id = req.get("id").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => {
            let protocol_version = params
                .get("protocolVersion")
                .and_then(|value| value.as_str())
                .unwrap_or("2024-11-05");
            Some(json!({
                "protocolVersion": protocol_version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "palace", "version": env!("CARGO_PKG_VERSION")},
            }))
        }
        "notifications/initialized" => return None,
        "tools/list" => Some(json!({"tools": tool_list()})),
        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_default();
            let result = dispatch_tool_with_usage(conn, config, session, tool_name, &args);
            Some(json!({
                "content": [{"type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default()}]
            }))
        }
        _ => {
            return Some(
                serde_json::to_string(&json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": {"code": -32601, "message": format!("Unknown method: {method}")}
                }))
                .unwrap(),
            )
        }
    };

    result.map(|r| {
        serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": r,
        }))
        .unwrap()
    })
}

pub fn dispatch_tool_with_usage(
    conn: &Connection,
    config: &PalaceConfig,
    session: &crate::usage::UsageSession,
    name: &str,
    args: &Value,
) -> Value {
    let start = Instant::now();
    let result = dispatch_tool(conn, config, name, args);
    if let Err(err) =
        crate::usage::record_event(conn, session, name, args, &result, start.elapsed())
    {
        warn!(error = %err, "Palace MCP: failed to record usage event");
    }
    result
}

pub fn dispatch_tool(conn: &Connection, config: &PalaceConfig, name: &str, args: &Value) -> Value {
    match name {
        "palace_status" => tool_status(conn, config),
        "palace_list_wings" => tool_list_wings(conn),
        "palace_list_rooms" => tool_list_rooms(conn, args),
        "palace_get_taxonomy" => tool_get_taxonomy(conn),
        "palace_get_aaak_spec" => json!({"aaak_spec": AAAK_SPEC}),
        "palace_search" => tool_search(conn, args),
        "palace_check_duplicate" => tool_check_duplicate(conn, args),
        "palace_add_drawer" => tool_add_drawer(conn, args),
        "palace_delete_drawer" => tool_delete_drawer_tool(conn, args),
        "palace_kg_query" => tool_kg_query(conn, args),
        "palace_kg_add" => tool_kg_add(conn, args),
        "palace_kg_invalidate" => tool_kg_invalidate(conn, args),
        "palace_kg_timeline" => tool_kg_timeline(conn, args),
        "palace_kg_stats" => tool_kg_stats(conn),
        "palace_seed_adoption_facts" => tool_seed_adoption_facts(conn, args),
        "palace_traverse" => tool_traverse(conn, args),
        "palace_find_tunnels" => tool_find_tunnels(conn, args),
        "palace_graph_stats" => tool_graph_stats(conn),
        "palace_create_tunnel" => tool_create_tunnel(conn, args),
        "palace_list_tunnels" => tool_list_tunnels(conn, args),
        "palace_delete_tunnel" => tool_delete_tunnel(conn, args),
        "palace_follow_tunnels" => tool_follow_tunnels(conn, args),
        "palace_get_drawer" => tool_get_drawer(conn, args),
        "palace_list_drawers" => tool_list_drawers(conn, args),
        "palace_update_drawer" => tool_update_drawer(conn, args),
        "palace_hook_settings" => json!({"save_enabled": true, "precompact_enabled": true}),
        "palace_memories_filed_away" => json!({"success": true}),
        "palace_list_agents" => tool_list_agents(conn),
        "palace_diary_write" => tool_diary_write(conn, args),
        "palace_diary_read" => tool_diary_read(conn, args),
        "palace_diary_search" => tool_diary_search(conn, args),
        "palace_session_context" => tool_session_context(conn, args),
        "palace_gain" => tool_gain(conn, args),
        "palace_verify" => tool_verify(conn, config),
        "palace_recall_check" => tool_recall_check(conn, args),
        "palace_conflicts" => tool_conflicts(conn, args),
        "palace_remember" => tool_remember(conn, args),
        "palace_forget" => tool_forget(conn, args),
        "palace_explain" => tool_explain(conn, args),
        "palace_preference_search" => tool_preference_search(conn, args),
        "palace_export" => tool_export(conn),
        "palace_import" => tool_import(conn, args),
        "palace_upgrade_embeddings" => tool_upgrade_embeddings(conn, args),
        "palace_prune" => tool_prune(conn, args),
        _ => json!({"error": format!("Unknown tool: {name}")}),
    }
}

// ── Tool implementations ──────────────────────────────────────────────────

#[allow(dead_code)]
fn no_palace() -> Value {
    json!({
        "error": "No palace found",
        "hint": "Run: palace init <dir> && palace mine <dir>",
    })
}

fn tool_status(conn: &Connection, config: &PalaceConfig) -> Value {
    let count = count_drawers(conn).unwrap_or(0);
    let wings = wing_counts(conn).unwrap_or_default();
    let rooms = room_counts(conn, None)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect::<serde_json::Map<_, _>>();

    // Surface recent diary entries for any known agent (warm-start context).
    let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let last_session: Vec<Value> = wings
        .keys()
        .filter(|w| w.starts_with("wing_diary__"))
        .flat_map(|wing| {
            let filter = DrawerFilter {
                wing: Some(wing.clone()),
                room: Some("diary".to_string()),
            };
            crate::store::list_drawers(conn, &filter, 10000)
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d.filed_at >= cutoff)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(3)
        .map(|d| {
            json!({
                "agent": d.metadata.get("agent").and_then(|v| v.as_str()).unwrap_or(""),
                "topic": d.metadata.get("topic").and_then(|v| v.as_str()).unwrap_or("general"),
                "timestamp": d.filed_at,
                "session_id": d.metadata.get("session_id").and_then(|v| v.as_str()).unwrap_or(""),
                "project_path": d.metadata.get("project_path").and_then(|v| v.as_str()).unwrap_or(""),
                "text": compact_text(&d.content, 200),
            })
        })
        .collect();

    let mut response = json!({
        "total_drawers": count,
        "wings": wings,
        "rooms": rooms,
        "palace_path": config.palace_db_path().to_string_lossy(),
        "protocol": PALACE_PROTOCOL,
        "aaak_dialect": AAAK_SPEC,
    });

    if !last_session.is_empty() {
        response["last_session"] = json!(last_session);
    }

    response
}

fn tool_gain(conn: &Connection, args: &Value) -> Value {
    if let Some(record) = args.get("record") {
        let feedback = crate::gain::FeedbackRecord {
            query_id: match str_arg(record, "query_id") {
                Some(value) => value,
                None => return json!({"success": false, "error": "record.query_id is required"}),
            },
            drawer_id: match str_arg(record, "drawer_id") {
                Some(value) => value,
                None => return json!({"success": false, "error": "record.drawer_id is required"}),
            },
            verdict: match str_arg(record, "verdict") {
                Some(value) => value,
                None => return json!({"success": false, "error": "record.verdict is required"}),
            },
            note: str_arg(record, "note"),
        };
        return match crate::gain::record_feedback(conn, &feedback) {
            Ok(()) => json!({"success": true, "recorded": feedback}),
            Err(err) => json!({"success": false, "error": err.to_string()}),
        };
    }

    let project = str_arg(args, "project");
    let since_text = str_arg(args, "since").unwrap_or_else(|| "30d".to_string());
    let since = match crate::gain::SinceWindow::parse(&since_text) {
        Ok(window) => window,
        Err(err) => return json!({"error": err.to_string()}),
    };
    if bool_arg(args, "reset") {
        return match crate::gain::reset(conn, project.as_deref()) {
            Ok(deleted) => json!({"success": true, "deleted": deleted, "project": project}),
            Err(err) => json!({"success": false, "error": err.to_string()}),
        };
    }

    let options = crate::gain::GainOptions { project, since };
    if bool_arg(args, "history") {
        let limit = int_arg(args, "limit").unwrap_or(20).max(0) as usize;
        return match crate::gain::history(conn, &options, limit) {
            Ok(events) => json!({"history": events}),
            Err(err) => json!({"error": err.to_string()}),
        };
    }

    match crate::gain::summarize(conn, &options) {
        Ok(report) => json!(report),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_verify(conn: &Connection, config: &PalaceConfig) -> Value {
    let drawer_count = count_drawers(conn).unwrap_or(0);
    let unembedded = crate::store::count_unembedded(conn).unwrap_or(0);
    let corrupted_embeddings = count_corrupted_embeddings(conn);
    let db_integrity = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .unwrap_or_else(|_| "not checked".to_string());
    let model_cached = embedding_model_cached();
    let tools = tool_names();
    let required_tools = [
        "palace_status",
        "palace_search",
        "palace_preference_search",
        "palace_recall_check",
        "palace_conflicts",
        "palace_diary_write",
    ];
    let missing_tools = required_tools
        .iter()
        .filter(|name| !tools.iter().any(|tool| tool == **name))
        .copied()
        .collect::<Vec<_>>();
    let ok = missing_tools.is_empty() && db_integrity == "ok" && corrupted_embeddings == 0;

    json!({
        "ok": ok,
        "mcp": {
            "server_name": "palace",
            "version": env!("CARGO_PKG_VERSION"),
            "tools": tools,
            "missing_required_tools": missing_tools,
        },
        "database": {
            "path": config.palace_db_path().to_string_lossy(),
            "drawer_count": drawer_count,
            "unembedded_drawers": unembedded,
            "corrupted_embeddings": corrupted_embeddings,
            "integrity": db_integrity,
        },
        "model": {
            "name": "all-MiniLM-L6-v2",
            "cached": model_cached,
            "cold_start_note": if model_cached {
                "model is cached; normal search should avoid download latency"
            } else {
                "first embedding search may download and warm the model"
            },
        },
        "reranker": {
            "enabled": crate::reranker::should_rerank(false),
            "model": crate::reranker::model_name(),
            "cold_start_note": "local interaction reranker has no remote dependency",
        },
    })
}

fn tool_recall_check(conn: &Connection, args: &Value) -> Value {
    let checks = match args.get("checks").and_then(Value::as_array) {
        Some(checks) if !checks.is_empty() => checks,
        _ => return json!({"ok": false, "error": "checks array is required"}),
    };
    let limit = int_arg(args, "limit").unwrap_or(5).max(1) as usize;
    let mut passed = 0usize;
    let mut rows = Vec::new();

    for check in checks {
        let query = check
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let expected_source = check
            .get("expected_source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() || expected_source.is_empty() {
            rows.push(json!({
                "query": query,
                "expected_source": expected_source,
                "passed": false,
                "error": "query and expected_source are required",
            }));
            continue;
        }
        let filter = DrawerFilter {
            wing: check
                .get("wing")
                .and_then(Value::as_str)
                .map(str::to_string),
            room: check
                .get("room")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        let results = match crate::ranker::hybrid_search(conn, query, None, &filter, limit) {
            Ok(results) => results,
            Err(err) => {
                rows.push(json!({
                    "query": query,
                    "expected_source": expected_source,
                    "passed": false,
                    "error": err.to_string(),
                }));
                continue;
            }
        };
        let rank = results
            .iter()
            .position(|result| result.drawer.source_file == expected_source)
            .map(|idx| idx + 1);
        let check_passed = rank.is_some();
        if check_passed {
            passed += 1;
        }
        rows.push(json!({
            "query": query,
            "expected_source": expected_source,
            "passed": check_passed,
            "rank": rank,
            "top_source": results.first().map(|result| result.drawer.source_file.clone()),
            "top_similarity": results.first().map(|result| result.drawer.similarity),
        }));
    }

    let failed = checks.len().saturating_sub(passed);
    json!({
        "ok": failed == 0,
        "passed": passed,
        "failed": failed,
        "checks": rows,
    })
}

fn tool_conflicts(conn: &Connection, args: &Value) -> Value {
    let entity = str_arg(args, "entity");
    match find_conflicts(conn, entity.as_deref()) {
        Ok(conflicts) => json!({
            "entity": entity,
            "count": conflicts.len(),
            "conflicts": conflicts,
        }),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_list_wings(conn: &Connection) -> Value {
    json!({"wings": wing_counts(conn).unwrap_or_default()})
}

fn tool_list_rooms(conn: &Connection, args: &Value) -> Value {
    let wing = str_arg(args, "wing");
    let rooms = room_counts(conn, wing.as_deref()).unwrap_or_default();
    json!({"wing": wing.unwrap_or_else(|| "all".to_string()), "rooms": rooms})
}

fn tool_get_taxonomy(conn: &Connection) -> Value {
    json!({"taxonomy": taxonomy(conn).unwrap_or_default()})
}

fn tool_search(conn: &Connection, args: &Value) -> Value {
    let query = match str_arg(args, "query") {
        Some(q) => q,
        None => return json!({"error": "query is required"}),
    };
    let limit = int_arg(args, "limit").unwrap_or(5) as usize;
    let wing = str_arg(args, "wing");
    let room = str_arg(args, "room");
    let rerank = bool_arg(args, "rerank");
    search_memories_with_options(
        conn,
        &query,
        wing.as_deref(),
        room.as_deref(),
        limit,
        rerank,
    )
}

fn tool_check_duplicate(conn: &Connection, args: &Value) -> Value {
    let content = match str_arg(args, "content") {
        Some(c) => c,
        None => return json!({"error": "content is required"}),
    };
    let threshold = float_arg(args, "threshold").unwrap_or(0.9);
    match check_duplicate(conn, &content, threshold) {
        Ok(matches) => {
            let is_dup = !matches.is_empty();
            let matches_json: Vec<Value> = matches
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "wing": m.wing,
                        "room": m.room,
                        "similarity": m.similarity,
                        "content": if m.text.len() > 200 { format!("{}...", &m.text[..200]) } else { m.text.clone() },
                    })
                })
                .collect();
            json!({"is_duplicate": is_dup, "matches": matches_json})
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_add_drawer(conn: &Connection, args: &Value) -> Value {
    let wing = match str_arg(args, "wing") {
        Some(w) => w,
        None => return json!({"success": false, "error": "wing is required"}),
    };
    let room = match str_arg(args, "room") {
        Some(r) => r,
        None => return json!({"success": false, "error": "room is required"}),
    };
    let content = match str_arg(args, "content") {
        Some(c) => c,
        None => return json!({"success": false, "error": "content is required"}),
    };
    let source_file = str_arg(args, "source_file").unwrap_or_default();
    let added_by = str_arg(args, "added_by").unwrap_or_else(|| "mcp".to_string());

    // Duplicate check
    match check_duplicate(conn, &content, 0.9) {
        Ok(dups) if !dups.is_empty() => {
            let matches_json: Vec<Value> = dups
                .iter()
                .map(|m| json!({"id": m.id, "wing": m.wing, "room": m.room, "similarity": m.similarity}))
                .collect();
            return json!({"success": false, "reason": "duplicate", "matches": matches_json});
        }
        _ => {}
    }

    let now = Utc::now().to_rfc3339();
    let drawer_id = {
        let hash = blake3::hash(
            format!("{wing}/{room}/{}/{now}", &content[..100.min(content.len())]).as_bytes(),
        );
        format!("drawer_{wing}_{room}_{}", &hash.to_hex()[..16])
    };

    let embedding = crate::embedder::embed_one(&content).ok();
    match add_drawer_with_id(
        conn,
        &drawer_id,
        &wing,
        &room,
        &content,
        embedding.as_deref(),
        &source_file,
        &added_by,
        None,
    ) {
        Ok(true) => json!({"success": true, "drawer_id": drawer_id, "wing": wing, "room": room}),
        Ok(false) => json!({"success": false, "reason": "already exists"}),
        Err(e) => json!({"success": false, "error": e.to_string()}),
    }
}

fn tool_delete_drawer_tool(conn: &Connection, args: &Value) -> Value {
    let id = match str_arg(args, "drawer_id") {
        Some(id) => id,
        None => return json!({"success": false, "error": "drawer_id is required"}),
    };
    match get_drawer(conn, &id) {
        Ok(None) => json!({"success": false, "error": format!("Drawer not found: {id}")}),
        Ok(Some(_)) => match delete_drawer(conn, &id) {
            Ok(true) => json!({"success": true, "drawer_id": id}),
            Ok(false) => json!({"success": false, "error": "delete failed"}),
            Err(e) => json!({"success": false, "error": e.to_string()}),
        },
        Err(e) => json!({"success": false, "error": e.to_string()}),
    }
}

fn tool_kg_query(conn: &Connection, args: &Value) -> Value {
    let entity = match str_arg(args, "entity") {
        Some(e) => e,
        None => return json!({"error": "entity is required"}),
    };
    let as_of = str_arg(args, "as_of");
    let direction = str_arg(args, "direction").unwrap_or_else(|| "both".to_string());
    match kg::query_entity(conn, &entity, as_of.as_deref(), &direction) {
        Ok(facts) => {
            json!({"entity": entity, "as_of": as_of, "facts": facts, "count": facts.len()})
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_kg_add(conn: &Connection, args: &Value) -> Value {
    let subject = match str_arg(args, "subject") {
        Some(s) => s,
        None => return json!({"error": "subject is required"}),
    };
    let predicate = match str_arg(args, "predicate") {
        Some(p) => p,
        None => return json!({"error": "predicate is required"}),
    };
    let object = match str_arg(args, "object") {
        Some(o) => o,
        None => return json!({"error": "object is required"}),
    };
    let valid_from = str_arg(args, "valid_from");
    let source_closet = str_arg(args, "source_closet");

    match kg::add_triple(
        conn,
        &subject,
        &predicate,
        &object,
        valid_from.as_deref(),
        None,
        1.0,
        source_closet.as_deref(),
        None,
    ) {
        Ok(id) => json!({
            "success": true,
            "triple_id": id,
            "fact": format!("{subject} → {predicate} → {object}"),
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_kg_invalidate(conn: &Connection, args: &Value) -> Value {
    let subject = match str_arg(args, "subject") {
        Some(s) => s,
        None => return json!({"error": "subject is required"}),
    };
    let predicate = match str_arg(args, "predicate") {
        Some(p) => p,
        None => return json!({"error": "predicate is required"}),
    };
    let object = match str_arg(args, "object") {
        Some(o) => o,
        None => return json!({"error": "object is required"}),
    };
    let ended = str_arg(args, "ended");
    match kg::invalidate(conn, &subject, &predicate, &object, ended.as_deref()) {
        Ok(()) => json!({
            "success": true,
            "fact": format!("{subject} → {predicate} → {object}"),
            "ended": ended.unwrap_or_else(|| "today".to_string()),
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_kg_timeline(conn: &Connection, args: &Value) -> Value {
    let entity = str_arg(args, "entity");
    match kg::timeline(conn, entity.as_deref()) {
        Ok(t) => {
            json!({"entity": entity.unwrap_or_else(|| "all".to_string()), "timeline": t, "count": t.len()})
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_kg_stats(conn: &Connection) -> Value {
    match kg::stats(conn) {
        Ok(s) => {
            serde_json::to_value(s).unwrap_or_else(|_| json!({"error": "serialization failed"}))
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_seed_adoption_facts(conn: &Connection, args: &Value) -> Value {
    let project = str_arg(args, "project").unwrap_or_else(|| "current project".to_string());
    match kg::seed_agent_adoption_facts(conn, &project) {
        Ok(report) => json!({
            "success": true,
            "project": project,
            "inserted": report.inserted,
            "unchanged": report.unchanged,
            "invalidated": report.invalidated,
        }),
        Err(err) => json!({"success": false, "error": err.to_string()}),
    }
}

fn tool_traverse(conn: &Connection, args: &Value) -> Value {
    let start_room = match str_arg(args, "start_room") {
        Some(r) => r,
        None => return json!({"error": "start_room is required"}),
    };
    let max_hops = int_arg(args, "max_hops").unwrap_or(2) as usize;
    palace_graph::traverse(conn, &start_room, max_hops)
        .unwrap_or_else(|e| json!({"error": e.to_string()}))
}

fn tool_find_tunnels(conn: &Connection, args: &Value) -> Value {
    let wing_a = str_arg(args, "wing_a");
    let wing_b = str_arg(args, "wing_b");
    match palace_graph::find_tunnels(conn, wing_a.as_deref(), wing_b.as_deref()) {
        Ok(t) => serde_json::to_value(t).unwrap_or_else(|_| json!([])),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_graph_stats(conn: &Connection) -> Value {
    match palace_graph::graph_stats(conn) {
        Ok(s) => serde_json::to_value(s).unwrap_or_else(|_| json!({"error": "serialization"})),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_create_tunnel(conn: &Connection, args: &Value) -> Value {
    let wing_a = match str_arg(args, "wing_a") {
        Some(value) => value,
        None => return json!({"error": "wing_a is required"}),
    };
    let room_a = match str_arg(args, "room_a") {
        Some(value) => value,
        None => return json!({"error": "room_a is required"}),
    };
    let wing_b = match str_arg(args, "wing_b") {
        Some(value) => value,
        None => return json!({"error": "wing_b is required"}),
    };
    let room_b = match str_arg(args, "room_b") {
        Some(value) => value,
        None => return json!({"error": "room_b is required"}),
    };
    let kind = str_arg(args, "kind").unwrap_or_else(|| "explicit".to_string());
    match palace_graph::create_tunnel(conn, &wing_a, &room_a, &wing_b, &room_b, &kind) {
        Ok(id) => json!({"success": true, "tunnel_id": id}),
        Err(err) => json!({"success": false, "error": err.to_string()}),
    }
}

fn tool_list_tunnels(conn: &Connection, args: &Value) -> Value {
    let wing = str_arg(args, "wing");
    let kind = str_arg(args, "kind");
    match palace_graph::list_tunnels(conn, wing.as_deref(), kind.as_deref()) {
        Ok(tunnels) => json!({"tunnels": tunnels, "count": tunnels.len()}),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_delete_tunnel(conn: &Connection, args: &Value) -> Value {
    let id = match str_arg(args, "tunnel_id") {
        Some(value) => value,
        None => return json!({"success": false, "error": "tunnel_id is required"}),
    };
    match palace_graph::delete_tunnel(conn, &id) {
        Ok(deleted) => json!({"success": deleted, "tunnel_id": id}),
        Err(err) => json!({"success": false, "error": err.to_string()}),
    }
}

fn tool_follow_tunnels(conn: &Connection, args: &Value) -> Value {
    let wing = match str_arg(args, "wing") {
        Some(value) => value,
        None => return json!({"error": "wing is required"}),
    };
    let room = match str_arg(args, "room") {
        Some(value) => value,
        None => return json!({"error": "room is required"}),
    };
    match palace_graph::follow_tunnels(conn, &wing, &room) {
        Ok(tunnels) => json!({"tunnels": tunnels, "count": tunnels.len()}),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_get_drawer(conn: &Connection, args: &Value) -> Value {
    let id = match str_arg(args, "drawer_id") {
        Some(value) => value,
        None => return json!({"error": "drawer_id is required"}),
    };
    match get_drawer(conn, &id) {
        Ok(Some(drawer)) => json!({"drawer": drawer}),
        Ok(None) => json!({"error": format!("Drawer not found: {id}")}),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_list_drawers(conn: &Connection, args: &Value) -> Value {
    let filter = DrawerFilter {
        wing: str_arg(args, "wing"),
        room: str_arg(args, "room"),
    };
    let limit = int_arg(args, "limit").unwrap_or(50) as usize;
    match list_drawers(conn, &filter, limit) {
        Ok(drawers) => json!({"drawers": drawers, "count": drawers.len()}),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_update_drawer(conn: &Connection, args: &Value) -> Value {
    let id = match str_arg(args, "drawer_id") {
        Some(value) => value,
        None => return json!({"success": false, "error": "drawer_id is required"}),
    };
    let content = match str_arg(args, "content") {
        Some(value) => value,
        None => return json!({"success": false, "error": "content is required"}),
    };
    match update_drawer_content(conn, &id, &content) {
        Ok(updated) => json!({"success": updated, "drawer_id": id}),
        Err(err) => json!({"success": false, "error": err.to_string()}),
    }
}

/// Canonical diary wing name for an agent (uses `wing_diary__` prefix to avoid
/// collisions with project wings that happen to start with `wing_`).
fn diary_wing(agent_name: &str) -> String {
    format!(
        "wing_diary__{}",
        agent_name.to_lowercase().replace(' ', "_")
    )
}

fn tool_list_agents(conn: &Connection) -> Value {
    match wing_counts(conn) {
        Ok(wings) => {
            let agents: Vec<_> = wings
                .keys()
                .filter(|wing| wing.starts_with("wing_diary__"))
                .map(|wing| wing.trim_start_matches("wing_diary__").to_string())
                .collect();
            json!({"agents": agents})
        }
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn tool_diary_write(conn: &Connection, args: &Value) -> Value {
    let agent_name = match str_arg(args, "agent_name") {
        Some(n) => n,
        None => return json!({"success": false, "error": "agent_name is required"}),
    };
    let entry = match str_arg(args, "entry") {
        Some(e) => e,
        None => return json!({"success": false, "error": "entry is required"}),
    };
    let topic = str_arg(args, "topic").unwrap_or_else(|| "general".to_string());
    let session_id = str_arg(args, "session_id").unwrap_or_default();
    let project_path = str_arg(args, "project_path").unwrap_or_default();
    let tags: Vec<String> = args
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let wing = diary_wing(&agent_name);
    let room = "diary".to_string();
    let now = Utc::now();
    let timestamp = now.to_rfc3339();
    let date = now.format("%Y-%m-%d").to_string();

    // Semantic dedup scoped to this agent's diary: skip near-identical entries
    // (e.g. the same session summary written twice) so the diary grows with new
    // information rather than repetition. Scoped per agent so a different agent
    // recording the same observation keeps its own continuity record.
    let dedup_filter = crate::store::DrawerFilter {
        wing: Some(wing.clone()),
        room: Some(room.clone()),
    };
    match crate::store::check_duplicate_filtered(conn, &entry, 0.95, &dedup_filter) {
        Ok(dups) if !dups.is_empty() => {
            let matches_json: Vec<Value> = dups
                .iter()
                .map(|m| json!({"id": m.id, "similarity": m.similarity}))
                .collect();
            return json!({
                "success": false,
                "reason": "duplicate",
                "matches": matches_json,
            });
        }
        _ => {}
    }

    let entry_prefix = &entry[..50.min(entry.len())];
    let id = diary_id(&wing, &timestamp, entry_prefix);

    let extra = json!({
        "hall": "hall_diary",
        "topic": topic,
        "type": "diary_entry",
        "agent": agent_name,
        "date": date,
        "session_id": session_id,
        "project_path": project_path,
        "tags": tags,
    });

    let embedding = crate::embedder::embed_one(&entry).ok();
    match add_drawer_with_id(
        conn,
        &id,
        &wing,
        &room,
        &entry,
        embedding.as_deref(),
        "",
        &agent_name,
        Some(&extra),
    ) {
        Ok(true) => json!({
            "success": true,
            "entry_id": id,
            "agent": agent_name,
            "topic": topic,
            "timestamp": timestamp,
        }),
        Ok(false) => json!({"success": false, "reason": "already exists"}),
        Err(e) => json!({"success": false, "error": e.to_string()}),
    }
}

fn tool_diary_read(conn: &Connection, args: &Value) -> Value {
    let agent_name = match str_arg(args, "agent_name") {
        Some(n) => n,
        None => return json!({"error": "agent_name is required"}),
    };
    let last_n = int_arg(args, "last_n").unwrap_or(10) as usize;

    let wing = diary_wing(&agent_name);

    let filter = crate::store::DrawerFilter {
        wing: Some(wing.clone()),
        room: Some("diary".to_string()),
    };

    match crate::store::list_drawers(conn, &filter, 10000) {
        Ok(mut drawers) => {
            drawers.sort_by(|a, b| b.filed_at.cmp(&a.filed_at));
            let total = drawers.len();
            drawers.truncate(last_n);

            let entries: Vec<Value> = drawers
                .iter()
                .map(|d| {
                    let date = d
                        .metadata
                        .get("date")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let topic = d
                        .metadata
                        .get("topic")
                        .and_then(|v| v.as_str())
                        .unwrap_or("general")
                        .to_string();
                    let session_id = d
                        .metadata
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let project_path = d
                        .metadata
                        .get("project_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tags = d.metadata.get("tags").cloned().unwrap_or_else(|| json!([]));
                    json!({
                        "date": date,
                        "timestamp": d.filed_at,
                        "topic": topic,
                        "content": d.content,
                        "session_id": session_id,
                        "project_path": project_path,
                        "tags": tags,
                    })
                })
                .collect();

            if entries.is_empty() {
                json!({"agent": agent_name, "entries": [], "message": "No diary entries yet."})
            } else {
                json!({"agent": agent_name, "entries": entries, "total": total, "showing": entries.len()})
            }
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn tool_diary_search(conn: &Connection, args: &Value) -> Value {
    let agent_name = match str_arg(args, "agent_name") {
        Some(n) => n,
        None => return json!({"error": "agent_name is required"}),
    };
    let query = match str_arg(args, "query") {
        Some(q) => q,
        None => return json!({"error": "query is required"}),
    };
    let limit = int_arg(args, "limit").unwrap_or(5) as usize;
    let tag_filter = str_arg(args, "tag");
    // Cross-agent recall: when `all_agents` is set, search every agent's diary
    // (room = "diary" across all `wing_diary__*` wings) instead of just the
    // caller's own wing. This is what lets a different agent the next day pick
    // up an investigation recorded by another agent.
    let all_agents = bool_arg(args, "all_agents");
    let project_path = str_arg(args, "project_path");

    let wing = if all_agents {
        None
    } else {
        Some(diary_wing(&agent_name))
    };
    let room = Some("diary".to_string());

    let _ = tag_filter; // tag filtering is on the roadmap; search is already wing/room scoped
                        // When filtering by project we over-fetch, then post-filter on the
                        // `project_path` stored in each drawer's metadata.
    let fetch_limit = if project_path.is_some() {
        limit.saturating_mul(4).max(limit)
    } else {
        limit
    };
    let results = crate::searcher::search_memories(
        conn,
        &query,
        wing.as_deref(),
        room.as_deref(),
        fetch_limit,
    );
    if let Some(hits) = results.get("results").and_then(|v| v.as_array()) {
        let mut hits = hits.clone();
        if let Some(project) = project_path.as_deref() {
            hits.retain(|hit| hit_project_path(conn, hit).as_deref() == Some(project));
        }
        hits.truncate(limit);
        json!({
            "agent": agent_name,
            "all_agents": all_agents,
            "query": query,
            "results": hits,
        })
    } else {
        results
    }
}

/// Look up the `project_path` recorded in the metadata of the drawer behind a
/// search hit. Used to scope cross-agent recall to a single project.
fn hit_project_path(conn: &Connection, hit: &Value) -> Option<String> {
    let id = hit.get("id").and_then(|v| v.as_str())?;
    let drawer = get_drawer(conn, id).ok().flatten()?;
    drawer
        .metadata
        .get("project_path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Gather recent diary drawers, newest first.
///
/// `wing = None` searches every agent's diary (cross-agent). When `project` is
/// set, only entries for that project are returned.
fn recent_diary_drawers(
    conn: &Connection,
    wing: Option<&str>,
    project: Option<&str>,
    within_hours: i64,
    take: usize,
) -> Vec<crate::store::Drawer> {
    let filter = crate::store::DrawerFilter {
        wing: wing.map(String::from),
        room: Some("diary".to_string()),
    };
    let cutoff = (Utc::now() - chrono::Duration::hours(within_hours)).to_rfc3339();
    let mut drawers = crate::store::list_drawers(conn, &filter, 10000).unwrap_or_default();
    drawers.sort_by(|a, b| b.filed_at.cmp(&a.filed_at));
    drawers
        .into_iter()
        .filter(|d| d.filed_at >= cutoff)
        .filter(|d| {
            project.is_none_or(|p| {
                d.metadata
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .map(|stored| stored == p)
                    .unwrap_or(false)
            })
        })
        .take(take)
        .collect()
}

fn tool_session_context(conn: &Connection, args: &Value) -> Value {
    let agent_name = match str_arg(args, "agent_name") {
        Some(n) => n,
        None => return json!({"error": "agent_name is required"}),
    };
    let project_path = str_arg(args, "project_path");

    let wing = diary_wing(&agent_name);

    // 1. Prefer the calling agent's own recent diary (last 24h).
    let mut cross_agent = false;
    let mut recent = recent_diary_drawers(conn, Some(&wing), project_path.as_deref(), 24, 3);

    // 2. Fall back to any other agent's recent work (wider 7-day window) so a
    //    different agent the next day still benefits from prior investigations.
    if recent.is_empty() {
        recent = recent_diary_drawers(conn, None, project_path.as_deref(), 24 * 7, 3);
        cross_agent = !recent.is_empty();
    }

    {
        if recent.is_empty() {
            return json!({
                "agent": agent_name,
                "has_recent_session": false,
                "context": null,
            });
        }

        let project = recent
            .first()
            .and_then(|d| d.metadata.get("project_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let entries: Vec<Value> = recent
            .iter()
            .map(|d| {
                let topic = d
                    .metadata
                    .get("topic")
                    .and_then(|v| v.as_str())
                    .unwrap_or("general");
                let session_id = d
                    .metadata
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let project_path = d
                    .metadata
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tags = d.metadata.get("tags").cloned().unwrap_or_else(|| json!([]));
                json!({
                    "topic": topic,
                    "timestamp": d.filed_at,
                    "session_id": session_id,
                    "project_path": project_path,
                    "tags": tags,
                    "text": compact_text(&d.content, 240),
                })
            })
            .collect();
        let summary = entries
            .iter()
            .map(|entry| {
                format!(
                    "[{}] {}",
                    entry
                        .get("topic")
                        .and_then(|v| v.as_str())
                        .unwrap_or("general"),
                    entry.get("text").and_then(|v| v.as_str()).unwrap_or("")
                )
            })
            .collect::<Vec<_>>();

        json!({
            "agent": agent_name,
            "has_recent_session": true,
            "cross_agent": cross_agent,
            "last_active_project": project,
            "recent_entries": entries,
            "context": format!(
                "{}({}): {}",
                if cross_agent { "Recent work by another agent " } else { "Last session " },
                recent.first().map(|d| d.filed_at.as_str()).unwrap_or("unknown"),
                summary.join(" | ")
            ),
        })
    }
}

// ── Phase-4 tools ─────────────────────────────────────────────────────────

/// Store a fact with high importance — a high-level "palace remember X" shortcut.
fn tool_remember(conn: &Connection, args: &Value) -> Value {
    let text = match args.get("text").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return json!({"error": "text is required"}),
    };
    let wing = args
        .get("wing")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let room = args
        .get("room")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();

    // Semantic dedup: a remembered fact filed under a different wing/room would
    // otherwise slip past the deterministic-ID guard and create a duplicate.
    match check_duplicate(conn, &text, 0.9) {
        Ok(dups) if !dups.is_empty() => {
            let matches_json: Vec<Value> = dups
                .iter()
                .map(|m| json!({"id": m.id, "wing": m.wing, "room": m.room, "similarity": m.similarity}))
                .collect();
            return json!({
                "success": true,
                "inserted": false,
                "reason": "duplicate",
                "matches": matches_json,
            });
        }
        _ => {}
    }

    let embedding = crate::embedder::embed_one(&text).ok();
    let emb_ref = embedding.as_deref();

    match crate::store::add_drawer(
        conn,
        &wing,
        &room,
        &text,
        emb_ref,
        "palace_remember",
        0,
        "mcp",
        5.0,
    ) {
        Ok((inserted, id)) => json!({
            "success": true,
            "inserted": inserted,
            "id": id,
            "wing": wing,
            "room": room,
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Delete a drawer by ID — the counterpart to `palace_remember`.
fn tool_forget(conn: &Connection, args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim().to_string(),
        _ => return json!({"error": "id is required"}),
    };
    match delete_drawer(conn, &id) {
        Ok(true) => json!({"success": true, "deleted_id": id}),
        Ok(false) => json!({"success": false, "error": "drawer not found", "id": id}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Return full metadata about a stored drawer — useful for explaining why a
/// memory exists, who filed it, when, and from which source.
fn tool_explain(conn: &Connection, args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim().to_string(),
        _ => return json!({"error": "id is required"}),
    };
    match get_drawer(conn, &id) {
        Ok(Some(d)) => json!({
            "id": d.id,
            "wing": d.wing,
            "room": d.room,
            "content": d.content,
            "source_file": d.source_file,
            "chunk_index": d.chunk_index,
            "added_by": d.added_by,
            "filed_at": d.filed_at,
            "created_at": d.created_at,
            "importance": d.importance,
            "hall": d.hall,
            "metadata": d.metadata,
            "entity_metadata": d.entity_metadata,
        }),
        Ok(None) => json!({"error": "drawer not found", "id": id}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Dedicated preference query — surfaces convention/preference drawers even
/// when BM25 has no keyword overlap with the query.
fn tool_preference_search(conn: &Connection, args: &Value) -> Value {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => return json!({"error": "query is required"}),
    };
    let limit = int_arg(args, "limit").unwrap_or(10) as usize;
    let filter = DrawerFilter {
        wing: args.get("wing").and_then(|v| v.as_str()).map(String::from),
        room: args.get("room").and_then(|v| v.as_str()).map(String::from),
    };

    let embedding = match crate::embedder::embed_one(&query) {
        Ok(e) => e,
        Err(e) => return json!({"error": format!("embedding error: {e}")}),
    };

    match preference_search_filtered(conn, &embedding, &filter, limit) {
        Ok(results) => {
            let hits: Vec<_> = results
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "text": r.text,
                        "wing": r.wing,
                        "room": r.room,
                        "source_file": r.source_file,
                        "filed_at": r.filed_at,
                        "similarity": r.similarity,
                    })
                })
                .collect();
            json!({
                "query": query,
                "filters": {"wing": filter.wing, "room": filter.room},
                "results": hits,
            })
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Export all drawers as a portable JSON snapshot (embeddings excluded).
fn tool_export(conn: &Connection) -> Value {
    match crate::export::export_drawers(conn) {
        Ok(doc) => match serde_json::to_value(&doc) {
            Ok(v) => v,
            Err(e) => json!({"error": e.to_string()}),
        },
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Import drawers from a JSON export snapshot produced by `palace_export`.
fn tool_import(conn: &Connection, args: &Value) -> Value {
    let export_json = match str_arg(args, "export_json") {
        Some(s) => s,
        None => return json!({"error": "missing required argument: export_json"}),
    };
    let doc: crate::export::ExportDoc = match serde_json::from_str(&export_json) {
        Ok(d) => d,
        Err(e) => return json!({"error": format!("invalid export JSON: {e}")}),
    };
    let total = doc.drawers.len();
    match crate::export::import_drawers(conn, &doc) {
        Ok(inserted) => json!({
            "inserted": inserted,
            "skipped": total - inserted,
            "total": total,
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// Re-embed all drawers using the current embedding model.
///
/// Useful after upgrading the embedding model. Each drawer's text is
/// re-embedded and the stored embedding is overwritten in-place.
fn tool_upgrade_embeddings(conn: &Connection, args: &Value) -> Value {
    let refresh_preferences = bool_arg(args, "refresh_preferences");
    let ids_and_content: Vec<(String, String)> =
        match conn.prepare("SELECT id, content FROM drawers ORDER BY rowid") {
            Ok(mut stmt) => match stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))) {
                Ok(rows) => {
                    let collected: Vec<(String, String)> = rows.filter_map(|r| r.ok()).collect();
                    collected
                }
                Err(e) => return json!({"error": e.to_string()}),
            },
            Err(e) => return json!({"error": e.to_string()}),
        };

    let mut reembedded = 0usize;
    let mut errors = 0usize;
    for (id, content) in &ids_and_content {
        match crate::embedder::embed_one(content) {
            Ok(emb) => {
                let bytes = crate::embedder::vec_to_blob(&emb);
                let pref_bytes = if refresh_preferences {
                    crate::preference::preference_span(content)
                        .and_then(|span| crate::embedder::embed_one(&span).ok())
                        .map(|embedding| crate::embedder::vec_to_blob(&embedding))
                } else {
                    None
                };
                let update = if refresh_preferences {
                    conn.execute(
                        "UPDATE drawers SET embedding = ?1, pref_embedding = ?2 WHERE id = ?3",
                        rusqlite::params![bytes, pref_bytes, id],
                    )
                } else {
                    conn.execute(
                        "UPDATE drawers SET embedding = ?1 WHERE id = ?2",
                        rusqlite::params![bytes, id],
                    )
                };
                match update {
                    Ok(_) => reembedded += 1,
                    Err(_) => errors += 1,
                }
            }
            Err(_) => errors += 1,
        }
    }
    json!({
        "reembedded": reembedded,
        "errors": errors,
        "total": ids_and_content.len(),
        "refresh_preferences": refresh_preferences,
    })
}

/// Delete drawers that have not been accessed and were filed more than
/// `older_than_days` days ago.
fn tool_prune(conn: &Connection, args: &Value) -> Value {
    let older_than_days = match int_arg(args, "older_than_days") {
        Some(d) if d > 0 => d,
        Some(_) => return json!({"error": "older_than_days must be a positive integer"}),
        None => return json!({"error": "missing required argument: older_than_days"}),
    };
    match conn.execute(
        "DELETE FROM drawers WHERE datetime(filed_at) <= datetime('now', printf('-%d days', ?1))",
        rusqlite::params![older_than_days],
    ) {
        Ok(pruned) => json!({"pruned": pruned, "older_than_days": older_than_days}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn count_corrupted_embeddings(conn: &Connection) -> i64 {
    let expected_bytes = (crate::embedder::EMBEDDING_DIM * std::mem::size_of::<f32>()) as i64;
    conn.query_row(
        "SELECT COUNT(*) FROM drawers WHERE embedding IS NOT NULL AND length(embedding) != ?1",
        [expected_bytes],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn embedding_model_cached() -> bool {
    let cache_base = if let Ok(path) = std::env::var("HF_HUB_CACHE") {
        std::path::PathBuf::from(path)
    } else if let Some(home) =
        directories::UserDirs::new().map(|user| user.home_dir().to_path_buf())
    {
        home.join(".cache").join("huggingface").join("hub")
    } else {
        return false;
    };
    cache_base
        .join("models--Qdrant--all-MiniLM-L6-v2-onnx")
        .exists()
}

fn find_conflicts(conn: &Connection, entity: Option<&str>) -> Result<Vec<Value>> {
    let entity_filter = entity.map(|name| name.to_lowercase().replace(' ', "_").replace('\'', ""));
    let mut stmt = conn.prepare(
        "SELECT t.id, s.name, t.predicate, o.name, t.valid_from, t.valid_to
         FROM triples t
         JOIN entities s ON t.subject = s.id
         JOIN entities o ON t.object = o.id
         WHERE (?1 IS NULL OR t.subject = ?1 OR t.object = ?1)
         ORDER BY s.name, t.predicate, t.valid_from",
    )?;
    let rows = stmt.query_map([entity_filter.as_deref()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut grouped: std::collections::HashMap<(String, String), Vec<Value>> =
        std::collections::HashMap::new();
    for row in rows {
        let (id, subject, predicate, object, valid_from, valid_to) = row?;
        let current = valid_to.is_none();
        grouped
            .entry((subject.clone(), predicate.clone()))
            .or_default()
            .push(json!({
                "id": id,
                "subject": subject,
                "predicate": predicate,
                "object": object,
                "valid_from": valid_from,
                "valid_to": valid_to,
                "current": current,
            }));
    }

    let mut conflicts = Vec::new();
    for ((subject, predicate), facts) in grouped {
        let objects = facts
            .iter()
            .filter_map(|fact| fact.get("object").and_then(Value::as_str))
            .collect::<std::collections::HashSet<_>>();
        if objects.len() <= 1 || facts.len() <= 1 {
            continue;
        }
        let triple_ids = facts
            .iter()
            .filter_map(|fact| fact.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        conflicts.push(json!({
            "subject": subject,
            "predicate": predicate,
            "triple_ids": triple_ids,
            "facts": facts,
        }));
    }
    conflicts.sort_by(|a, b| {
        a.get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("subject").and_then(Value::as_str).unwrap_or(""))
            .then_with(|| {
                a.get("predicate")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("predicate").and_then(Value::as_str).unwrap_or(""))
            })
    });
    Ok(conflicts)
}

fn tool_names() -> Vec<String> {
    tool_list()
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

// ── Tool schema list ──────────────────────────────────────────────────────

fn tool_list() -> Value {
    json!([
        {
            "name": "palace_status",
            "description": "Palace overview — total drawers, wing and room counts",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_gain",
            "description": "Show local MCP usage gains: hits, estimated tokens saved, skipped duplicates, recall, latency, and per-project value.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Project to filter by (optional)"},
                    "since": {"type": "string", "description": "Window like 7d, 24h, 30d, or all (default: 30d)"},
                    "history": {"type": "boolean", "description": "Return recent usage events instead of summary"},
                    "limit": {"type": "integer", "description": "History limit (default: 20)"},
                    "reset": {"type": "boolean", "description": "Delete usage events for the project, or all projects if omitted"},
                    "record": {
                        "type": "object",
                        "description": "Optional explicit feedback record. When omitted, palace_gain returns the normal read-only summary.",
                        "properties": {
                            "query_id": {"type": "string"},
                            "drawer_id": {"type": "string"},
                            "verdict": {"type": "string", "enum": ["useful", "not_useful", "wrong_answer"]},
                            "note": {"type": "string"}
                        },
                        "required": ["query_id", "drawer_id", "verdict"]
                    }
                }
            }
        },
        {
            "name": "palace_verify",
            "description": "Verify Palace MCP health: visible tools, database integrity, embedding status, and model cache.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_recall_check",
            "description": "Run project-memory probes and report whether expected memories are retrievable.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "checks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "query": {"type": "string"},
                                "expected_source": {"type": "string"},
                                "wing": {"type": "string"},
                                "room": {"type": "string"}
                            },
                            "required": ["query", "expected_source"]
                        }
                    },
                    "limit": {"type": "integer", "description": "Max results per probe (default 5)"}
                },
                "required": ["checks"]
            }
        },
        {
            "name": "palace_conflicts",
            "description": "Surface likely stale or contradictory knowledge graph facts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Entity to inspect (optional)"}
                }
            }
        },
        {
            "name": "palace_list_wings",
            "description": "List all wings with drawer counts",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_list_rooms",
            "description": "List rooms within a wing (or all rooms if no wing given)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wing": {"type": "string", "description": "Wing to list rooms for (optional)"}
                }
            }
        },
        {
            "name": "palace_get_taxonomy",
            "description": "Full taxonomy: wing → room → drawer count",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_get_aaak_spec",
            "description": "Get the AAAK dialect specification — the compressed memory format Palace uses.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_search",
            "description": "Semantic search. Returns verbatim drawer content with similarity scores.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search for"},
                    "limit": {"type": "integer", "description": "Max results (default 5)"},
                    "wing": {"type": "string", "description": "Filter by wing (optional)"},
                    "room": {"type": "string", "description": "Filter by room (optional)"},
                    "rerank": {"type": "boolean", "description": "Rerank top hybrid candidates with the local interaction reranker"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "palace_check_duplicate",
            "description": "Check if content already exists in the palace before filing",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "Content to check"},
                    "threshold": {"type": "number", "description": "Similarity threshold 0-1 (default 0.9)"}
                },
                "required": ["content"]
            }
        },
        {
            "name": "palace_add_drawer",
            "description": "File verbatim content into the palace. Checks for duplicates first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wing": {"type": "string", "description": "Wing (project name)"},
                    "room": {"type": "string", "description": "Room (aspect)"},
                    "content": {"type": "string", "description": "Verbatim content to store"},
                    "source_file": {"type": "string", "description": "Where this came from (optional)"},
                    "added_by": {"type": "string", "description": "Who is filing this (default: mcp)"}
                },
                "required": ["wing", "room", "content"]
            }
        },
        {
            "name": "palace_delete_drawer",
            "description": "Delete a drawer by ID. Irreversible.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "drawer_id": {"type": "string", "description": "ID of the drawer to delete"}
                },
                "required": ["drawer_id"]
            }
        },
        {
            "name": "palace_kg_query",
            "description": "Query the knowledge graph for an entity's relationships.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Entity to query"},
                    "as_of": {"type": "string", "description": "Date filter (YYYY-MM-DD, optional)"},
                    "direction": {"type": "string", "description": "outgoing/incoming/both (default: both)"}
                },
                "required": ["entity"]
            }
        },
        {
            "name": "palace_kg_add",
            "description": "Add a fact to the knowledge graph. Subject → predicate → object.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string"},
                    "predicate": {"type": "string"},
                    "object": {"type": "string"},
                    "valid_from": {"type": "string", "description": "YYYY-MM-DD (optional)"},
                    "source_closet": {"type": "string", "description": "Closet ID (optional)"}
                },
                "required": ["subject", "predicate", "object"]
            }
        },
        {
            "name": "palace_kg_invalidate",
            "description": "Mark a fact as no longer true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string"},
                    "predicate": {"type": "string"},
                    "object": {"type": "string"},
                    "ended": {"type": "string", "description": "YYYY-MM-DD (default: today)"}
                },
                "required": ["subject", "predicate", "object"]
            }
        },
        {
            "name": "palace_kg_timeline",
            "description": "Chronological timeline of facts, optionally for one entity.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Entity to filter by (optional)"}
                }
            }
        },
        {
            "name": "palace_kg_stats",
            "description": "Knowledge graph overview: entities, triples, relationship types.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_seed_adoption_facts",
            "description": "Seed durable KG facts for the current project and four supported Palace clients.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Project/entity name to seed facts for"}
                }
            }
        },
        {
            "name": "palace_traverse",
            "description": "Walk the palace graph from a room. Find connected ideas across wings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "start_room": {"type": "string", "description": "Room to start from"},
                    "max_hops": {"type": "integer", "description": "How many connections to follow (default: 2)"}
                },
                "required": ["start_room"]
            }
        },
        {
            "name": "palace_find_tunnels",
            "description": "Find rooms that bridge two wings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wing_a": {"type": "string"},
                    "wing_b": {"type": "string"}
                }
            }
        },
        {
            "name": "palace_graph_stats",
            "description": "Palace graph overview: total rooms, tunnel connections, edges between wings.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_create_tunnel",
            "description": "Create a persisted tunnel between two wing/room pairs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wing_a": {"type": "string"},
                    "room_a": {"type": "string"},
                    "wing_b": {"type": "string"},
                    "room_b": {"type": "string"},
                    "kind": {"type": "string"}
                },
                "required": ["wing_a", "room_a", "wing_b", "room_b"]
            }
        },
        {
            "name": "palace_list_tunnels",
            "description": "List persisted tunnels.",
            "inputSchema": {"type": "object", "properties": {
                "wing": {"type": "string"},
                "kind": {"type": "string"}
            }}
        },
        {
            "name": "palace_delete_tunnel",
            "description": "Delete a persisted tunnel.",
            "inputSchema": {"type": "object", "properties": {
                "tunnel_id": {"type": "string"}
            }, "required": ["tunnel_id"]}
        },
        {
            "name": "palace_follow_tunnels",
            "description": "Follow persisted tunnels from a wing/room pair.",
            "inputSchema": {"type": "object", "properties": {
                "wing": {"type": "string"},
                "room": {"type": "string"}
            }, "required": ["wing", "room"]}
        },
        {
            "name": "palace_get_drawer",
            "description": "Get a drawer by ID.",
            "inputSchema": {"type": "object", "properties": {
                "drawer_id": {"type": "string"}
            }, "required": ["drawer_id"]}
        },
        {
            "name": "palace_list_drawers",
            "description": "List drawers with optional wing/room filters.",
            "inputSchema": {"type": "object", "properties": {
                "wing": {"type": "string"},
                "room": {"type": "string"},
                "limit": {"type": "integer"}
            }}
        },
        {
            "name": "palace_update_drawer",
            "description": "Update drawer content and refresh metadata.",
            "inputSchema": {"type": "object", "properties": {
                "drawer_id": {"type": "string"},
                "content": {"type": "string"}
            }, "required": ["drawer_id", "content"]}
        },
        {
            "name": "palace_hook_settings",
            "description": "Return hook settings.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_memories_filed_away",
            "description": "Acknowledge that memories have been filed.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_list_agents",
            "description": "List agent diary wings.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_diary_write",
            "description": "Write to your personal agent diary. Supports session metadata for warm-start context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": {"type": "string", "description": "Your agent name"},
                    "entry": {"type": "string", "description": "Your diary entry (AAAK format recommended)"},
                    "topic": {"type": "string", "description": "Topic tag (default: general)"},
                    "session_id": {"type": "string", "description": "Session UUID for grouping entries"},
                    "project_path": {"type": "string", "description": "Active project path for warm-start context"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Searchable tags"}
                },
                "required": ["agent_name", "entry"]
            }
        },
        {
            "name": "palace_diary_read",
            "description": "Read your recent diary entries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": {"type": "string"},
                    "last_n": {"type": "integer", "description": "Number of recent entries (default: 10)"}
                },
                "required": ["agent_name"]
            }
        },
        {
            "name": "palace_diary_search",
            "description": "Semantic search within diary entries. Set all_agents=true to recall investigations recorded by any agent (cross-agent continuity).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": {"type": "string"},
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "integer", "description": "Max results (default: 5)"},
                    "tag": {"type": "string", "description": "Optional tag filter"},
                    "all_agents": {"type": "boolean", "description": "Search every agent's diary instead of only your own (default: false)"},
                    "project_path": {"type": "string", "description": "Restrict results to entries recorded for this project path"}
                },
                "required": ["agent_name", "query"]
            }
        },
        {
            "name": "palace_session_context",
            "description": "Get warm-start context from recent diary entries. Falls back to other agents' recent work for the project when you have none of your own.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_name": {"type": "string"},
                    "project_path": {"type": "string", "description": "Restrict and scope fallback recall to this project path"}
                },
                "required": ["agent_name"]
            }
        },
        {
            "name": "palace_remember",
            "description": "File a key fact with high importance (5.0). Shortcut for palace_add_drawer with importance=5.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "The fact to remember"},
                    "wing": {"type": "string", "description": "Wing (default: general)"},
                    "room": {"type": "string", "description": "Room (default: general)"}
                },
                "required": ["text"]
            }
        },
        {
            "name": "palace_forget",
            "description": "Delete a drawer by ID. Use when a memory is outdated or incorrect.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Drawer ID to delete"}
                },
                "required": ["id"]
            }
        },
        {
            "name": "palace_explain",
            "description": "Return full provenance for a drawer — who filed it, when, from which source file, and its importance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Drawer ID to explain"}
                },
                "required": ["id"]
            }
        },
        {
            "name": "palace_preference_search",
            "description": "Search drawers tagged as preferences or conventions. Use when asking what the user prefers, their coding style, or standing rules.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search for"},
                    "limit": {"type": "integer", "description": "Max results (default 10)"},
                    "wing": {"type": "string", "description": "Filter by wing (optional)"},
                    "room": {"type": "string", "description": "Filter by room (optional)"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "palace_export",
            "description": "Export all palace drawers as a portable JSON snapshot (embeddings excluded). Use for backup or migration.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "palace_import",
            "description": "Import drawers from a JSON snapshot produced by palace_export. Skips drawers that already exist (idempotent).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "export_json": {"type": "string", "description": "Full JSON string from a palace_export result"}
                },
                "required": ["export_json"]
            }
        },
        {
            "name": "palace_upgrade_embeddings",
            "description": "Re-embed all drawers using the current embedding model. Run after upgrading the model to keep search quality consistent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "refresh_preferences": {"type": "boolean", "description": "Also refresh preference-span embeddings"}
                }
            }
        },
        {
            "name": "palace_prune",
            "description": "Delete drawers filed more than N days ago. Use to keep the palace lean. Irreversible — export first if unsure.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "older_than_days": {"type": "integer", "description": "Delete drawers filed more than this many days ago"}
                },
                "required": ["older_than_days"]
            }
        }
    ])
}

// ── Argument helpers ──────────────────────────────────────────────────────

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

fn int_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| {
        if let Some(n) = v.as_i64() {
            Some(n)
        } else if let Some(f) = v.as_f64() {
            Some(f as i64)
        } else if let Some(s) = v.as_str() {
            s.parse().ok()
        } else {
            None
        }
    })
}

fn bool_arg(args: &Value, key: &str) -> bool {
    args.get(key)
        .and_then(|value| {
            value.as_bool().or_else(|| {
                value.as_str().map(|text| {
                    matches!(
                        text.trim(),
                        "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                    )
                })
            })
        })
        .unwrap_or(false)
}

fn float_arg(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| {
        if let Some(f) = v.as_f64() {
            Some(f)
        } else if let Some(s) = v.as_str() {
            s.parse().ok()
        } else {
            None
        }
    })
}

fn compact_text(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
