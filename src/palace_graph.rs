//! Palace graph traversal.
//!
//! Builds a navigable graph from drawer metadata:
//!   Nodes = rooms (named ideas)
//!   Edges = rooms that appear in multiple wings (tunnels)
//!
//! Port of palace_graph.py.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomNode {
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: i64,
    pub dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelEdge {
    pub room: String,
    pub wing_a: String,
    pub wing_b: String,
    pub hall: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalResult {
    pub room: String,
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: i64,
    pub hop: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_via: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelResult {
    pub room: String,
    pub wings: Vec<String>,
    pub halls: Vec<String>,
    pub count: i64,
    pub recent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTunnel {
    pub id: String,
    pub wing_a: String,
    pub room_a: String,
    pub wing_b: String,
    pub room_b: String,
    pub kind: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub total_rooms: usize,
    pub tunnel_rooms: usize,
    pub total_edges: usize,
    pub rooms_per_wing: HashMap<String, i64>,
    pub top_tunnels: Vec<serde_json::Value>,
}

pub fn topic_room(topic: &str) -> String {
    format!("topic:{}", crate::config::normalize_wing_name(topic))
}

pub fn create_tunnel(
    conn: &Connection,
    wing_a: &str,
    room_a: &str,
    wing_b: &str,
    room_b: &str,
    kind: &str,
) -> Result<String> {
    let wing_a = crate::config::normalize_wing_name(wing_a);
    let wing_b = crate::config::normalize_wing_name(wing_b);
    let room_a = room_a.to_string();
    let room_b = room_b.to_string();
    let kind = if kind.trim().is_empty() {
        "explicit"
    } else {
        kind.trim()
    };
    let id = tunnel_id(&wing_a, &room_a, &wing_b, &room_b, kind);
    conn.execute(
        "INSERT OR IGNORE INTO tunnels (id, wing_a, room_a, wing_b, room_b, kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, wing_a, room_a, wing_b, room_b, kind],
    )
    .context("creating tunnel")?;
    Ok(id)
}

pub fn list_tunnels(
    conn: &Connection,
    wing: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<PersistedTunnel>> {
    let wing = wing.map(crate::config::normalize_wing_name);
    let mut stmt = conn
        .prepare(
            "SELECT id, wing_a, room_a, wing_b, room_b, kind, created_at
             FROM tunnels
             WHERE (?1 IS NULL OR wing_a = ?1 OR wing_b = ?1)
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY created_at DESC, id",
        )
        .context("preparing tunnel list")?;
    let rows = stmt.query_map(params![wing.as_deref(), kind], tunnel_from_row)?;
    rows.map(|row| row.context("reading tunnel row")).collect()
}

pub fn delete_tunnel(conn: &Connection, id: &str) -> Result<bool> {
    let rows = conn
        .execute("DELETE FROM tunnels WHERE id = ?1", params![id])
        .context("deleting tunnel")?;
    Ok(rows > 0)
}

pub fn follow_tunnels(conn: &Connection, wing: &str, room: &str) -> Result<Vec<PersistedTunnel>> {
    let wing = crate::config::normalize_wing_name(wing);
    let mut stmt = conn
        .prepare(
            "SELECT id, wing_a, room_a, wing_b, room_b, kind, created_at
             FROM tunnels
             WHERE (wing_a = ?1 AND room_a = ?2)
                OR (wing_b = ?1 AND room_b = ?2)
             ORDER BY created_at DESC, id",
        )
        .context("preparing tunnel follow")?;
    let rows = stmt.query_map(params![wing, room], tunnel_from_row)?;
    rows.map(|row| row.context("reading followed tunnel row"))
        .collect()
}

fn tunnel_id(wing_a: &str, room_a: &str, wing_b: &str, room_b: &str, kind: &str) -> String {
    let hash = blake3::hash(format!("{wing_a}/{room_a}/{wing_b}/{room_b}/{kind}").as_bytes());
    format!("tunnel_{}", &hash.to_hex()[..16])
}

fn tunnel_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersistedTunnel> {
    Ok(PersistedTunnel {
        id: row.get(0)?,
        wing_a: row.get(1)?,
        room_a: row.get(2)?,
        wing_b: row.get(3)?,
        room_b: row.get(4)?,
        kind: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Build the room graph from drawer metadata in the database.
fn build_graph(conn: &Connection) -> Result<(HashMap<String, RoomNode>, Vec<TunnelEdge>)> {
    type RoomTuple = (HashSet<String>, HashSet<String>, i64, HashSet<String>);
    let mut room_data: HashMap<String, RoomTuple> = HashMap::new();

    let mut stmt = conn
        .prepare("SELECT room, wing, source_file, filed_at FROM drawers WHERE room != 'general'")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;

    for row in rows {
        let (room, wing, source_file, filed_at) = row?;
        if room.is_empty() || wing.is_empty() {
            continue;
        }
        let entry = room_data.entry(room).or_default();
        entry.0.insert(wing);
        // Extract hall from source_file metadata suffix if present
        if let Some(null_pos) = source_file.find('\x00') {
            if let Ok(meta) =
                serde_json::from_str::<serde_json::Value>(&source_file[null_pos + 1..])
            {
                if let Some(hall) = meta.get("hall").and_then(|v| v.as_str()) {
                    entry.1.insert(hall.to_string());
                }
                if let Some(date) = meta.get("date").and_then(|v| v.as_str()) {
                    entry.3.insert(date.to_string());
                }
            }
        }
        // Use filed_at date as fallback
        if let Some(date_part) = filed_at.split('T').next() {
            entry.3.insert(date_part.to_string());
        }
        entry.2 += 1;
    }

    // Build edges from rooms spanning multiple wings
    let mut edges = Vec::new();
    let mut nodes: HashMap<String, RoomNode> = HashMap::new();

    for (room, (wings, halls, count, dates)) in &room_data {
        let mut sorted_wings: Vec<String> = wings.iter().cloned().collect();
        sorted_wings.sort();

        if sorted_wings.len() >= 2 {
            for i in 0..sorted_wings.len() {
                for j in (i + 1)..sorted_wings.len() {
                    let hall = halls.iter().next().cloned().unwrap_or_default();
                    edges.push(TunnelEdge {
                        room: room.clone(),
                        wing_a: sorted_wings[i].clone(),
                        wing_b: sorted_wings[j].clone(),
                        hall,
                        count: *count,
                    });
                }
            }
        }

        let mut sorted_dates: Vec<String> = dates.iter().cloned().collect();
        sorted_dates.sort();
        sorted_dates.truncate(5);

        nodes.insert(
            room.clone(),
            RoomNode {
                wings: sorted_wings,
                halls: halls.iter().cloned().collect(),
                count: *count,
                dates: sorted_dates,
            },
        );
    }

    Ok((nodes, edges))
}

/// Walk the graph from a starting room using BFS.
pub fn traverse(conn: &Connection, start_room: &str, max_hops: usize) -> Result<serde_json::Value> {
    let (nodes, _edges) = build_graph(conn)?;

    if !nodes.contains_key(start_room) {
        let suggestions = fuzzy_match(start_room, &nodes);
        return Ok(serde_json::json!({
            "error": format!("Room '{}' not found", start_room),
            "suggestions": suggestions,
        }));
    }

    let start = &nodes[start_room];
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(start_room.to_string());

    let mut results: Vec<serde_json::Value> = vec![serde_json::json!({
        "room": start_room,
        "wings": start.wings,
        "halls": start.halls,
        "count": start.count,
        "hop": 0,
    })];

    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((start_room.to_string(), 0));

    while let Some((current_room, depth)) = frontier.pop_front() {
        if depth >= max_hops {
            continue;
        }
        let current_wings: HashSet<String> = nodes
            .get(&current_room)
            .map(|n| n.wings.iter().cloned().collect())
            .unwrap_or_default();

        for (room, data) in &nodes {
            if visited.contains(room) {
                continue;
            }
            let shared: Vec<String> = current_wings
                .iter()
                .filter(|w| data.wings.contains(*w))
                .cloned()
                .collect();
            if !shared.is_empty() {
                visited.insert(room.clone());
                let entry = serde_json::json!({
                    "room": room,
                    "wings": data.wings,
                    "halls": data.halls,
                    "count": data.count,
                    "hop": depth + 1,
                    "connected_via": shared,
                });
                results.push(entry);
                if depth + 1 < max_hops {
                    frontier.push_back((room.clone(), depth + 1));
                }
            }
        }
    }

    // Sort by hop ASC, count DESC
    results.sort_by(|a, b| {
        let hop_a = a["hop"].as_i64().unwrap_or(0);
        let hop_b = b["hop"].as_i64().unwrap_or(0);
        let cnt_a = a["count"].as_i64().unwrap_or(0);
        let cnt_b = b["count"].as_i64().unwrap_or(0);
        hop_a.cmp(&hop_b).then(cnt_b.cmp(&cnt_a))
    });
    results.truncate(50);

    Ok(serde_json::Value::Array(results))
}

/// Find rooms that connect two wings (tunnels).
pub fn find_tunnels(
    conn: &Connection,
    wing_a: Option<&str>,
    wing_b: Option<&str>,
) -> Result<Vec<TunnelResult>> {
    let (nodes, _edges) = build_graph(conn)?;

    let mut tunnels: Vec<TunnelResult> = nodes
        .iter()
        .filter(|(_, data)| data.wings.len() >= 2)
        .filter(|(_, data)| {
            if let Some(wa) = wing_a {
                if !data.wings.contains(&wa.to_string()) {
                    return false;
                }
            }
            if let Some(wb) = wing_b {
                if !data.wings.contains(&wb.to_string()) {
                    return false;
                }
            }
            true
        })
        .map(|(room, data)| TunnelResult {
            room: room.clone(),
            wings: data.wings.clone(),
            halls: data.halls.clone(),
            count: data.count,
            recent: data.dates.last().cloned().unwrap_or_default(),
        })
        .collect();

    tunnels.sort_by_key(|tunnel| std::cmp::Reverse(tunnel.count));
    tunnels.truncate(50);
    Ok(tunnels)
}

/// Summary statistics about the palace graph.
pub fn graph_stats(conn: &Connection) -> Result<GraphStats> {
    let (nodes, edges) = build_graph(conn)?;

    let tunnel_rooms = nodes.values().filter(|n| n.wings.len() >= 2).count();

    let mut rooms_per_wing: HashMap<String, i64> = HashMap::new();
    for data in nodes.values() {
        for w in &data.wings {
            *rooms_per_wing.entry(w.clone()).or_default() += 1;
        }
    }

    let mut top_tunnels: Vec<(&String, &RoomNode)> =
        nodes.iter().filter(|(_, n)| n.wings.len() >= 2).collect();
    top_tunnels.sort_by_key(|(_, node)| std::cmp::Reverse(node.wings.len()));
    top_tunnels.truncate(10);

    let top_tunnels_json: Vec<serde_json::Value> = top_tunnels
        .iter()
        .map(|(room, n)| {
            serde_json::json!({
                "room": room,
                "wings": n.wings,
                "count": n.count,
            })
        })
        .collect();

    Ok(GraphStats {
        total_rooms: nodes.len(),
        tunnel_rooms,
        total_edges: edges.len(),
        rooms_per_wing,
        top_tunnels: top_tunnels_json,
    })
}

fn fuzzy_match(query: &str, nodes: &HashMap<String, RoomNode>) -> Vec<String> {
    let query_lower = query.to_lowercase();
    let mut scored: Vec<(String, i32)> = nodes
        .keys()
        .filter_map(|room| {
            if query_lower.is_empty() {
                return None;
            }
            if room.contains(&query_lower) {
                return Some((room.clone(), 2));
            }
            let query_parts: Vec<&str> = query_lower.split('-').collect();
            if query_parts.iter().any(|p| room.contains(p)) {
                return Some((room.clone(), 1));
            }
            None
        })
        .collect();
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored.into_iter().take(5).map(|(r, _)| r).collect()
}
