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
    /// When this drawer was filed into the palace (used for recency scoring).
    pub filed_at: String,
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
    let pref_blob = preference_embedding_blob(content, embedding);
    let filed_at = Utc::now().to_rfc3339();
    let entity_metadata = crate::entity_detector::entity_metadata(content);
    let entity_metadata_text =
        serde_json::to_string(&entity_metadata).unwrap_or_else(|_| "{}".to_string());
    let hall = crate::hall_router::detect_hall(content);
    let metadata_text = serde_json::to_string(&metadata_for_content(None, content))
        .unwrap_or_else(|_| "{}".to_string());

    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO drawers
             (id, wing, room, content, embedding, source_file, chunk_index, added_by, filed_at,
              importance, created_at, entity_metadata, hall, metadata, pref_embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12, ?13, ?14)",
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
                hall,
                metadata_text,
                pref_blob
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
    let pref_blob = preference_embedding_blob(content, embedding);
    let filed_at = Utc::now().to_rfc3339();
    let metadata = metadata_for_content(extra_meta, content);
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
              created_at, entity_metadata, hall, metadata, pref_embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?8, ?9, ?10, ?11, ?12)",
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
                metadata_text,
                pref_blob
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
    let embedding = crate::embedder::embed_one(content).ok();
    let blob = embedding.as_deref().map(vec_to_blob);
    let pref_blob = preference_embedding_blob(content, embedding.as_deref());
    let current_metadata = get_drawer(conn, id)
        .ok()
        .flatten()
        .map(|drawer| drawer.metadata)
        .unwrap_or_else(|| serde_json::json!({}));
    let metadata_text =
        serde_json::to_string(&metadata_for_content(Some(&current_metadata), content))
            .unwrap_or_else(|_| "{}".to_string());
    let rows = conn
        .execute(
            "UPDATE drawers
             SET content = ?1, entity_metadata = ?2, hall = ?3, embedding = ?4, metadata = ?5
                 , pref_embedding = ?6
             WHERE id = ?7",
            params![
                content,
                entity_metadata_text,
                hall,
                blob,
                metadata_text,
                pref_blob,
                id
            ],
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
        "SELECT id, wing, room, content, source_file, created_at, filed_at, embedding
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
                r.get::<_, String>(6)?,
                r.get::<_, Vec<u8>>(7)?,
            ))
        },
    )?;

    let mut scored: Vec<SearchResult> = rows
        .filter_map(|r| {
            let (id, wing, room, content, source_file, created_at, filed_at, blob) = r.ok()?;
            let emb = blob_to_vec(&blob);
            let sim = cosine_similarity(query_vec, &emb) as f64;
            Some(SearchResult {
                id,
                text: content,
                wing,
                room,
                source_file,
                created_at,
                filed_at,
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

/// Search drawers tagged as preferences, ranked by cosine similarity.
///
/// Used as a second recall pass to surface preference drawers even when BM25 has
/// no keyword overlap with the query (the R@1 weakness for preference sentences).
pub fn preference_search(
    conn: &Connection,
    query_vec: &[f32],
    n_results: usize,
) -> Result<Vec<SearchResult>> {
    preference_search_filtered(conn, query_vec, &DrawerFilter::default(), n_results)
}

/// Search preference-tagged drawers with optional wing/room filters.
pub fn preference_search_filtered(
    conn: &Connection,
    query_vec: &[f32],
    filter: &DrawerFilter,
    n_results: usize,
) -> Result<Vec<SearchResult>> {
    let (where_clause, where_params) = build_where(filter);
    let extra_filter = if where_clause.is_empty() {
        String::new()
    } else {
        format!(" AND {}", &where_clause[6..])
    };
    let sql = format!(
        "SELECT id, wing, room, content, source_file, created_at, filed_at,
                COALESCE(pref_embedding, embedding) AS preference_embedding
         FROM drawers
         WHERE json_extract(metadata, '$.preference') = 1
           AND COALESCE(pref_embedding, embedding) IS NOT NULL{extra_filter}",
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
                r.get::<_, String>(6)?,
                r.get::<_, Vec<u8>>(7)?,
            ))
        },
    )?;

    let mut scored: Vec<SearchResult> = rows
        .filter_map(|r| {
            let (id, wing, room, content, source_file, created_at, filed_at, blob) = r.ok()?;
            let emb = blob_to_vec(&blob);
            let sim = cosine_similarity(query_vec, &emb) as f64;
            Some(SearchResult {
                id,
                text: content,
                wing,
                room,
                source_file,
                created_at,
                filed_at,
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
    check_duplicate_filtered(conn, content, threshold, &DrawerFilter::default())
}

/// Check for near-duplicate content within a wing/room scope.
///
/// Used to keep dedup tight: diary entries are deduplicated only against the
/// same agent's diary, so two agents recording the same observation each keep
/// their own continuity record.
pub fn check_duplicate_filtered(
    conn: &Connection,
    content: &str,
    threshold: f64,
    filter: &DrawerFilter,
) -> Result<Vec<SearchResult>> {
    let embedding = crate::embedder::embed_one(content)?;
    let results = vector_search(conn, &embedding, filter, 5)?;
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

/// A row from the `wings` registry, enriched with the wing's drawer count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WingRecord {
    pub name: String,
    /// `project` or `topic`.
    pub kind: String,
    pub description: String,
    /// Canonical on-disk path for project wings; `None` for topics.
    pub project_path: Option<String>,
    /// RFC3339 timestamp of the last successful mine; `None` if never mined.
    pub last_mined_at: Option<String>,
    pub created_at: String,
    pub drawer_count: i64,
}

const WING_SELECT: &str = "SELECT w.name, w.kind, w.description, w.project_path, w.last_mined_at, \
     w.created_at, (SELECT COUNT(*) FROM drawers d WHERE d.wing = w.name) AS drawer_count \
     FROM wings w";

fn wing_record_from_row(row: &rusqlite::Row) -> rusqlite::Result<WingRecord> {
    Ok(WingRecord {
        name: row.get(0)?,
        kind: row.get(1)?,
        description: row.get(2)?,
        project_path: row.get(3)?,
        last_mined_at: row.get(4)?,
        created_at: row.get(5)?,
        drawer_count: row.get(6)?,
    })
}

/// Insert or update a wing registry entry.
///
/// Non-empty incoming values win; empty values preserve whatever is already
/// stored, so this never clobbers a richer record with blanks. A `project` kind
/// always wins over `topic` (a wing can be promoted but not demoted).
pub fn upsert_wing(
    conn: &Connection,
    name: &str,
    kind: &str,
    description: &str,
    project_path: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO wings (name, kind, description, project_path)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET
             kind = CASE
                 WHEN excluded.kind = 'project' OR wings.kind = 'project' THEN 'project'
                 ELSE excluded.kind
             END,
             description = CASE
                 WHEN excluded.description != '' THEN excluded.description
                 ELSE wings.description
             END,
             project_path = COALESCE(NULLIF(excluded.project_path, ''), wings.project_path)",
        params![name, kind, description, project_path],
    )
    .context("upserting wing")?;
    Ok(())
}

/// Fetch a single wing registry entry by name.
pub fn get_wing(conn: &Connection, name: &str) -> Result<Option<WingRecord>> {
    let sql = format!("{WING_SELECT} WHERE w.name = ?1");
    match conn.query_row(&sql, params![name], wing_record_from_row) {
        Ok(record) => Ok(Some(record)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Find a project wing by its canonical on-disk path.
pub fn find_wing_by_path(conn: &Connection, project_path: &str) -> Result<Option<WingRecord>> {
    let sql = format!("{WING_SELECT} WHERE w.project_path = ?1");
    match conn.query_row(&sql, params![project_path], wing_record_from_row) {
        Ok(record) => Ok(Some(record)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// List all registered wings, most-populated first.
pub fn list_wings_registry(conn: &Connection) -> Result<Vec<WingRecord>> {
    let sql = format!("{WING_SELECT} ORDER BY drawer_count DESC, w.name ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], wing_record_from_row)?;
    rows.map(|r| r.context("listing wings registry")).collect()
}

/// Mark a wing as a mined project: set kind=project, path, and last_mined_at.
pub fn set_wing_mined(conn: &Connection, name: &str, project_path: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO wings (name, kind, project_path, last_mined_at)
         VALUES (?1, 'project', ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
             kind = 'project',
             project_path = ?2,
             last_mined_at = ?3",
        params![name, project_path, now],
    )
    .context("marking wing as mined")?;
    Ok(())
}

/// Register a wing as a topic if it isn't already known. Never modifies an
/// existing entry, so project wings keep their richer metadata.
pub fn ensure_wing_registered(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO wings (name, kind) VALUES (?1, 'topic')",
        params![name],
    )
    .context("ensuring wing registered")?;
    Ok(())
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

fn metadata_for_content(existing: Option<&serde_json::Value>, content: &str) -> serde_json::Value {
    let mut metadata = existing.cloned().unwrap_or_else(|| serde_json::json!({}));
    if !metadata.is_object() {
        metadata = serde_json::json!({});
    }
    if let Some(span) = crate::preference::preference_span(content) {
        metadata["preference"] = serde_json::json!(true);
        metadata["preference_span"] = serde_json::json!(span);
    } else if let Some(object) = metadata.as_object_mut() {
        object.remove("preference");
        object.remove("preference_span");
    }
    metadata
}

fn preference_embedding_blob(content: &str, fallback_embedding: Option<&[f32]>) -> Option<Vec<u8>> {
    let span = crate::preference::preference_span(content)?;
    let fallback = fallback_embedding?;
    let embedding = crate::embedder::embed_one(&span)
        .ok()
        .unwrap_or_else(|| fallback.to_vec());
    Some(vec_to_blob(&embedding))
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
