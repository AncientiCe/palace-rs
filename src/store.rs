//! Drawer CRUD and vector search.
//!
//! Replaces ChromaDB. Stores text + embedding BLOB in SQLite drawers table.
//! Vector search: brute-force cosine similarity, fast for < 500K rows.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::embedder::{blob_to_vec, cosine_similarity, vec_to_blob};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drawer {
    pub id: String,
    pub wing: String,
    pub room: String,
    pub content: String,
    pub source_file: String,
    pub chunk_index: i64,
    pub added_by: String,
    pub filed_at: String,
    pub created_at: String,
    pub importance: f64,
    pub entity_metadata: serde_json::Value,
    pub hall: Option<String>,
    pub normalize_version: i64,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub text: String,
    pub wing: String,
    pub room: String,
    pub source_file: String,
    pub created_at: String,
    pub similarity: f64,
}

#[derive(Debug, Default)]
pub struct DrawerFilter {
    pub wing: Option<String>,
    pub room: Option<String>,
}

/// Generate a deterministic drawer ID from wing + room + source_file + chunk_index.
pub fn drawer_id(wing: &str, room: &str, source_file: &str, chunk_index: usize) -> String {
    let hash = blake3::hash(format!("{wing}/{room}/{source_file}/{chunk_index}").as_bytes());
    format!("drawer_{wing}_{room}_{}", &hash.to_hex()[..16])
}

/// Generate a diary entry ID from wing + timestamp + content prefix.
pub fn diary_id(wing: &str, timestamp: &str, content_prefix: &str) -> String {
    let hash = blake3::hash(format!("{wing}/{timestamp}/{content_prefix}").as_bytes());
    format!("diary_{wing}_{}", &hash.to_hex()[..16])
}

/// Add a drawer to the palace. Returns `true` if inserted, `false` if already exists.
#[allow(clippy::too_many_arguments)]
pub fn add_drawer(
    conn: &Connection,
    wing: &str,
    room: &str,
    content: &str,
    embedding: Option<&[f32]>,
    source_file: &str,
    chunk_index: usize,
    added_by: &str,
    importance: f64,
) -> Result<(bool, String)> {
    let id = drawer_id(wing, room, source_file, chunk_index);
    let blob = embedding.map(vec_to_blob);
    let filed_at = Utc::now().to_rfc3339();
    let entity_metadata = crate::entity_detector::entity_metadata(content);
    let entity_metadata_text =
        serde_json::to_string(&entity_metadata).unwrap_or_else(|_| "{}".to_string());
    let hall = crate::hall_router::detect_hall(content);

    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO drawers
             (id, wing, room, content, embedding, source_file, chunk_index, added_by, filed_at,
              importance, created_at, entity_metadata, hall)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12)",
            params![
                id,
                wing,
                room,
                content,
                blob,
                source_file,
                chunk_index as i64,
                added_by,
                filed_at,
                importance,
                entity_metadata_text,
                hall
            ],
        )
        .context("inserting drawer")?;
    if rows > 0 {
        index_bm25_terms(conn, &id, content)?;
    }

    Ok((rows > 0, id))
}

/// Add a drawer with an explicit ID (used by MCP add_drawer and diary_write).
#[allow(clippy::too_many_arguments)]
pub fn add_drawer_with_id(
    conn: &Connection,
    id: &str,
    wing: &str,
    room: &str,
    content: &str,
    embedding: Option<&[f32]>,
    source_file: &str,
    added_by: &str,
    extra_meta: Option<&serde_json::Value>,
) -> Result<bool> {
    let blob = embedding.map(vec_to_blob);
    let filed_at = Utc::now().to_rfc3339();
    let metadata = extra_meta.cloned().unwrap_or_else(|| serde_json::json!({}));
    let metadata_text = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());
    let hall = metadata
        .get("hall")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| crate::hall_router::detect_hall(content));
    let entity_metadata = crate::entity_detector::entity_metadata(content);
    let entity_metadata_text =
        serde_json::to_string(&entity_metadata).unwrap_or_else(|_| "{}".to_string());

    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO drawers
             (id, wing, room, content, embedding, source_file, chunk_index, added_by, filed_at,
              created_at, entity_metadata, hall, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?8, ?9, ?10, ?11)",
            params![
                id,
                wing,
                room,
                content,
                blob,
                source_file,
                added_by,
                filed_at,
                entity_metadata_text,
                hall,
                metadata_text
            ],
        )
        .context("inserting drawer with id")?;
    if rows > 0 {
        index_bm25_terms(conn, id, content)?;
    }
    Ok(rows > 0)
}

/// Delete a drawer by ID. Returns true if a row was deleted.
pub fn delete_drawer(conn: &Connection, id: &str) -> Result<bool> {
    let rows = conn
        .execute("DELETE FROM drawers WHERE id = ?1", params![id])
        .context("deleting drawer")?;
    Ok(rows > 0)
}

pub fn update_drawer_content(conn: &Connection, id: &str, content: &str) -> Result<bool> {
    let entity_metadata = crate::entity_detector::entity_metadata(content);
    let entity_metadata_text =
        serde_json::to_string(&entity_metadata).unwrap_or_else(|_| "{}".to_string());
    let hall = crate::hall_router::detect_hall(content);
    let rows = conn
        .execute(
            "UPDATE drawers
             SET content = ?1, entity_metadata = ?2, hall = ?3
             WHERE id = ?4",
            params![content, entity_metadata_text, hall, id],
        )
        .context("updating drawer content")?;
    if rows > 0 {
        index_bm25_terms(conn, id, content)?;
    }
    Ok(rows > 0)
}

/// Check whether a source file has already been mined.
pub fn file_already_mined(conn: &Connection, source_file: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM drawers WHERE source_file = ?1 LIMIT 1",
            params![source_file],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

/// Total number of drawers in the palace.
pub fn count_drawers(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM drawers", [], |r| r.get(0))
        .context("counting drawers")
}

/// List drawers filtered by optional wing/room, ordered by filed_at DESC.
pub fn list_drawers(conn: &Connection, filter: &DrawerFilter, limit: usize) -> Result<Vec<Drawer>> {
    let (where_clause, where_params) = build_where(filter);
    let sql = format!(
        "SELECT id, wing, room, content, source_file, chunk_index, added_by, filed_at, created_at,
                importance, entity_metadata, hall, normalize_version, metadata
         FROM drawers {where_clause} ORDER BY filed_at DESC LIMIT ?",
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut bind_params: Vec<Box<dyn rusqlite::ToSql>> = where_params;
    bind_params.push(Box::new(limit as i64));

    let rows = stmt.query_map(
        rusqlite::params_from_iter(bind_params.iter().map(|p| p.as_ref())),
        drawer_from_row,
    )?;

    rows.map(|r| r.context("reading drawer row")).collect()
}

/// Get a single drawer by ID.
pub fn get_drawer(conn: &Connection, id: &str) -> Result<Option<Drawer>> {
    let result = conn.query_row(
        "SELECT id, wing, room, content, source_file, chunk_index, added_by, filed_at, created_at,
                importance, entity_metadata, hall, normalize_version, metadata
         FROM drawers WHERE id = ?1",
        params![id],
        drawer_from_row,
    );
    match result {
        Ok(d) => Ok(Some(d)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Return drawers adjacent to a chunk from the same source file.
pub fn source_context(
    conn: &Connection,
    source_file: &str,
    center_chunk_index: i64,
    radius: usize,
) -> Result<Vec<Drawer>> {
    let radius = radius as i64;
    let mut stmt = conn.prepare(
        "SELECT id, wing, room, content, source_file, chunk_index, added_by, filed_at, created_at,
                importance, entity_metadata, hall, normalize_version, metadata
         FROM drawers
         WHERE source_file = ?1
           AND chunk_index BETWEEN ?2 AND ?3
         ORDER BY chunk_index",
    )?;
    let rows = stmt.query_map(
        params![
            source_file,
            center_chunk_index.saturating_sub(radius),
            center_chunk_index.saturating_add(radius)
        ],
        drawer_from_row,
    )?;
    rows.map(|row| row.context("source context row")).collect()
}

pub fn get_search_result_drawer(conn: &Connection, id: &str) -> Result<Option<Drawer>> {
    get_drawer(conn, id)
}

/// Semantic vector search over drawers.
///
/// Loads embeddings from SQLite (optionally filtered), computes cosine similarity,
/// returns top-n results sorted by descending similarity.
pub fn vector_search(
    conn: &Connection,
    query_vec: &[f32],
    filter: &DrawerFilter,
    n_results: usize,
) -> Result<Vec<SearchResult>> {
    let (where_clause, where_params) = build_where(filter);
    let sql = format!(
        "SELECT id, wing, room, content, source_file, created_at, embedding
         FROM drawers WHERE embedding IS NOT NULL {extra} ORDER BY filed_at DESC",
        extra = if where_clause.is_empty() {
            String::new()
        } else {
            // where_clause already has "WHERE", replace it with AND
            format!("AND {}", &where_clause[6..])
        }
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(where_params.iter().map(|p| p.as_ref())),
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Vec<u8>>(6)?,
            ))
        },
    )?;

    let mut scored: Vec<SearchResult> = rows
        .filter_map(|r| {
            let (id, wing, room, content, source_file, created_at, blob) = r.ok()?;
            let emb = blob_to_vec(&blob);
            let sim = cosine_similarity(query_vec, &emb) as f64;
            Some(SearchResult {
                id,
                text: content,
                wing,
                room,
                source_file: std::path::Path::new(&source_file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| source_file.clone()),
                created_at,
                similarity: (sim * 1000.0).round() / 1000.0,
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(n_results);
    Ok(scored)
}

/// Check for near-duplicate content. Returns matching drawers above threshold.
pub fn check_duplicate(
    conn: &Connection,
    content: &str,
    threshold: f64,
) -> Result<Vec<SearchResult>> {
    let embedding = crate::embedder::embed_one(content)?;
    let results = vector_search(conn, &embedding, &DrawerFilter::default(), 5)?;
    Ok(results
        .into_iter()
        .filter(|r| r.similarity >= threshold)
        .collect())
}

/// Wing-level drawer counts.
pub fn wing_counts(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT wing, COUNT(*) FROM drawers GROUP BY wing")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.map(|r| r.context("wing counts")).collect()
}

/// Room-level drawer counts (optionally filtered by wing).
pub fn room_counts(conn: &Connection, wing: Option<&str>) -> Result<HashMap<String, i64>> {
    let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(w) = wing {
        (
            "SELECT room, COUNT(*) FROM drawers WHERE wing = ?1 GROUP BY room",
            vec![Box::new(w.to_string())],
        )
    } else {
        ("SELECT room, COUNT(*) FROM drawers GROUP BY room", vec![])
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )?;
    rows.map(|r| r.context("room counts")).collect()
}

/// Full taxonomy: wing → room → count.
pub fn taxonomy(conn: &Connection) -> Result<HashMap<String, HashMap<String, i64>>> {
    let mut stmt = conn.prepare("SELECT wing, room, COUNT(*) FROM drawers GROUP BY wing, room")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;

    let mut tax: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for row in rows {
        let (wing, room, count) = row.context("taxonomy row")?;
        tax.entry(wing).or_default().insert(room, count);
    }
    Ok(tax)
}

/// List all drawers ordered by importance DESC, limited to `limit`.
pub fn list_by_importance(conn: &Connection, limit: usize) -> Result<Vec<Drawer>> {
    let mut stmt = conn.prepare(
        "SELECT id, wing, room, content, source_file, chunk_index, added_by, filed_at, created_at,
                importance, entity_metadata, hall, normalize_version, metadata
         FROM drawers ORDER BY importance DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], drawer_from_row)?;
    rows.map(|r| r.context("importance list row")).collect()
}

/// Count drawers missing embeddings (for repair command).
pub fn count_unembedded(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM drawers WHERE embedding IS NULL",
        [],
        |r| r.get(0),
    )
    .context("counting unembedded drawers")
}

/// Fetch all drawers that are missing embeddings (for repair).
pub fn fetch_unembedded(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, content FROM drawers WHERE embedding IS NULL ORDER BY filed_at")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.map(|r| r.context("unembedded row")).collect()
}

/// Update the embedding for an existing drawer.
pub fn update_embedding(conn: &Connection, id: &str, embedding: &[f32]) -> Result<()> {
    let blob = vec_to_blob(embedding);
    conn.execute(
        "UPDATE drawers SET embedding = ?1 WHERE id = ?2",
        params![blob, id],
    )
    .context("updating embedding")?;
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn build_where(filter: &DrawerFilter) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(w) = &filter.wing {
        params.push(Box::new(w.clone()));
        clauses.push(format!("wing = ?{}", params.len()));
    }
    if let Some(r) = &filter.room {
        params.push(Box::new(r.clone()));
        clauses.push(format!("room = ?{}", params.len()));
    }

    if clauses.is_empty() {
        (String::new(), params)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), params)
    }
}

fn drawer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Drawer> {
    let entity_metadata_text: String = row.get(10)?;
    let metadata_text: String = row.get(13)?;
    Ok(Drawer {
        id: row.get(0)?,
        wing: row.get(1)?,
        room: row.get(2)?,
        content: row.get(3)?,
        source_file: row.get(4)?,
        chunk_index: row.get(5)?,
        added_by: row.get(6)?,
        filed_at: row.get(7)?,
        created_at: row.get(8)?,
        importance: row.get(9)?,
        entity_metadata: parse_json_object(&entity_metadata_text),
        hall: row.get(11)?,
        normalize_version: row.get(12)?,
        metadata: parse_json_object(&metadata_text),
    })
}

fn parse_json_object(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({}))
}

fn index_bm25_terms(conn: &Connection, drawer_id: &str, content: &str) -> Result<()> {
    let terms = crate::ranker::tokenize(content);
    let doc_len = terms.len() as i64;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for term in terms {
        *counts.entry(term).or_default() += 1;
    }

    conn.execute(
        "INSERT INTO bm25_doc_stats (drawer_id, doc_len)
         VALUES (?1, ?2)
         ON CONFLICT(drawer_id) DO UPDATE SET
            doc_len = excluded.doc_len,
            updated_at = CURRENT_TIMESTAMP",
        params![drawer_id, doc_len],
    )
    .context("upserting BM25 doc stats")?;

    conn.execute(
        "DELETE FROM bm25_terms WHERE drawer_id = ?1",
        params![drawer_id],
    )
    .context("clearing old BM25 terms")?;

    for (term, tf) in counts {
        conn.execute(
            "INSERT INTO bm25_terms (drawer_id, term, tf) VALUES (?1, ?2, ?3)",
            params![drawer_id, term, tf],
        )
        .context("inserting BM25 term")?;
    }

    Ok(())
}
