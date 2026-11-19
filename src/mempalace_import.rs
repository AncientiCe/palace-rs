//! Import a Python MemPalace installation into a palace.
//!
//! The Python version keeps its data in three places, all read directly here
//! so no Python or ChromaDB install is needed:
//!
//! - drawers (including diary entries) in a vector store under `palace/`:
//!   ChromaDB's `chroma.sqlite3` (the default backend) or the
//!   `sqlite_exact.sqlite3` backend;
//! - the knowledge graph in `knowledge_graph.sqlite3`, next to the palace or
//!   one level up;
//! - explicit tunnels in `tunnels.json`, next to the palace directory.
//!
//! Vectors are not copied: they come from a different model, so every drawer
//! is embedded again with the active one. Closets are not copied either;
//! they are an index palace derives from drawers. Every import is idempotent:
//! drawers keep a stable ID, and triples and tunnels already present are skipped.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CHROMA_DB: &str = "chroma.sqlite3";
const SQLITE_EXACT_DB: &str = "sqlite_exact.sqlite3";
const KG_DB: &str = "knowledge_graph.sqlite3";
const TUNNELS_FILE: &str = "tunnels.json";

/// Drawers embedded and written per transaction.
const IMPORT_BATCH: usize = 64;

/// Metadata keys mapped onto drawer columns rather than kept in `metadata`.
const COLUMN_KEYS: &[&str] = &[
    "wing",
    "room",
    "source_file",
    "chunk_index",
    "added_by",
    "filed_at",
    "importance",
];

/// Which vector store a Python palace uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawerStore {
    Chroma(PathBuf),
    SqliteExact(PathBuf),
}

/// The files of a Python MemPalace installation that hold data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MempalaceLayout {
    pub drawers: Option<DrawerStore>,
    pub knowledge_graph: Option<PathBuf>,
    pub tunnels: Option<PathBuf>,
}

impl MempalaceLayout {
    /// Locate a Python palace from its state directory (`~/.mempalace`), its
    /// palace directory (`~/.mempalace/palace`), or one of its database files.
    /// Returns `None` when nothing there looks like Python MemPalace data.
    pub fn detect(path: &Path) -> Option<Self> {
        let palace_dir = if path.is_file() {
            path.parent()?.to_path_buf()
        } else if has_store(path) || (path.join(KG_DB).is_file() && !path.join("palace").is_dir()) {
            path.to_path_buf()
        } else {
            path.join("palace")
        };
        let drawers = if palace_dir.join(CHROMA_DB).is_file() {
            Some(DrawerStore::Chroma(palace_dir.join(CHROMA_DB)))
        } else if palace_dir.join(SQLITE_EXACT_DB).is_file() {
            Some(DrawerStore::SqliteExact(palace_dir.join(SQLITE_EXACT_DB)))
        } else {
            None
        };
        let parent = palace_dir.parent().map(Path::to_path_buf);
        let knowledge_graph = std::iter::once(palace_dir.join(KG_DB))
            .chain(parent.iter().map(|dir| dir.join(KG_DB)))
            .find(|candidate| candidate.is_file());
        let tunnels = std::iter::once(palace_dir.join(TUNNELS_FILE))
            .chain(parent.iter().map(|dir| dir.join(TUNNELS_FILE)))
            .find(|candidate| candidate.is_file());
        let layout = Self {
            drawers,
            knowledge_graph,
            tunnels,
        };
        (layout.drawers.is_some() || layout.knowledge_graph.is_some()).then_some(layout)
    }
}

fn has_store(dir: &Path) -> bool {
    dir.join(CHROMA_DB).is_file() || dir.join(SQLITE_EXACT_DB).is_file()
}

/// What an import copied.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub drawers: usize,
    pub diary_entries: usize,
    pub drawers_skipped: usize,
    pub entities: usize,
    pub triples: usize,
    pub tunnels: usize,
}

/// One record read from a Python drawer collection.
#[derive(Debug, Clone)]
pub struct LegacyRecord {
    pub id: String,
    pub document: String,
    pub metadata: Map<String, Value>,
}

/// Import everything `layout` points at into `dest`.
pub fn import(layout: &MempalaceLayout, dest: &Connection) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    if let Some(store) = &layout.drawers {
        let records = match store {
            DrawerStore::Chroma(path) => read_chroma(path)?,
            DrawerStore::SqliteExact(path) => read_sqlite_exact(path)?,
        };
        import_drawers(dest, &records, &mut report)?;
        crate::migrate::backfill_wings_registry(dest)?;
    }
    if let Some(path) = &layout.knowledge_graph {
        import_knowledge_graph(path, dest, &mut report)?;
    }
    if let Some(path) = &layout.tunnels {
        report.tunnels = import_tunnels(path, dest)?;
    }
    Ok(report)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", path.display()))
}

/// Closets are an index over drawers; palace rebuilds its own.
fn is_drawer_collection(name: &str) -> bool {
    !name.contains("closet")
}

/// Read every drawer from a ChromaDB `chroma.sqlite3`.
///
/// Chroma keeps each record's document and metadata as key/value rows in
/// `embedding_metadata`; the document is under the `chroma:document` key.
pub fn read_chroma(path: &Path) -> Result<Vec<LegacyRecord>> {
    let conn = open_read_only(path)?;
    // `bool_value` arrived in a later Chroma schema migration.
    let has_bool: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('embedding_metadata')
                           WHERE name = 'bool_value')",
            [],
            |row| row.get(0),
        )
        .context("reading ChromaDB schema")?;
    let bool_column = if has_bool { "m.bool_value" } else { "NULL" };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT c.name, e.id, e.embedding_id, m.key,
                    m.string_value, m.int_value, m.float_value, {bool_column}
             FROM embeddings e
             JOIN segments s ON s.id = e.segment_id
             JOIN collections c ON c.id = s.collection
             JOIN embedding_metadata m ON m.id = e.id
             ORDER BY e.id"
        ))
        .context("reading ChromaDB records (is this a ChromaDB 0.4+ palace?)")?;
    let mut rows = stmt.query([])?;
    let mut records: BTreeMap<i64, LegacyRecord> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let collection: String = row.get(0)?;
        if !is_drawer_collection(&collection) {
            continue;
        }
        let rowid: i64 = row.get(1)?;
        let id: String = row.get(2)?;
        let key: String = row.get(3)?;
        let value = if let Some(text) = row.get::<_, Option<String>>(4)? {
            Value::String(text)
        } else if let Some(int) = row.get::<_, Option<i64>>(5)? {
            Value::from(int)
        } else if let Some(float) = row.get::<_, Option<f64>>(6)? {
            Value::from(float)
        } else if let Some(flag) = row.get::<_, Option<bool>>(7)? {
            Value::Bool(flag)
        } else {
            Value::Null
        };
        let record = records.entry(rowid).or_insert_with(|| LegacyRecord {
            id,
            document: String::new(),
            metadata: Map::new(),
        });
        if key == "chroma:document" {
            record.document = value.as_str().unwrap_or_default().to_string();
        } else if !key.starts_with("chroma:") {
            record.metadata.insert(key, value);
        }
    }
    Ok(records.into_values().collect())
}

/// Read every drawer from the `sqlite_exact` backend.
pub fn read_sqlite_exact(path: &Path) -> Result<Vec<LegacyRecord>> {
    let conn = open_read_only(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT c.name, d.id, d.document, d.metadata_json
             FROM documents d JOIN collections c ON c.id = d.collection_id
             ORDER BY d.created_at, d.id",
        )
        .context("reading sqlite_exact documents")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (collection, id, document, metadata_json) = row?;
        if !is_drawer_collection(&collection) {
            continue;
        }
        let metadata = match serde_json::from_str(&metadata_json) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        records.push(LegacyRecord {
            id,
            document,
            metadata,
        });
    }
    Ok(records)
}

/// A legacy record mapped onto palace's drawer columns.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedDrawer {
    pub id: String,
    pub wing: String,
    pub room: String,
    pub content: String,
    pub source_file: String,
    pub chunk_index: i64,
    pub added_by: String,
    pub importance: f64,
    pub filed_at: Option<String>,
    pub is_diary: bool,
    pub metadata: Value,
}

fn meta_str(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    match metadata.get(key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn meta_f64(metadata: &Map<String, Value>, key: &str) -> Option<f64> {
    match metadata.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Map one Python record onto a palace drawer. Returns `None` for an empty one.
pub fn map_record(record: &LegacyRecord) -> Option<MappedDrawer> {
    if record.document.trim().is_empty() {
        return None;
    }
    let meta = &record.metadata;
    let mut wing = meta_str(meta, "wing").unwrap_or_else(|| "general".to_string());
    let room = meta_str(meta, "room").unwrap_or_else(|| "general".to_string());
    let is_diary = meta_str(meta, "type").as_deref() == Some("diary_entry")
        || (room == "diary" && wing.starts_with("wing_"));
    let agent = meta_str(meta, "agent");
    if is_diary {
        // Python files diaries under `wing_<agent>`; palace uses `wing_diary__<agent>`.
        let agent = agent
            .clone()
            .or_else(|| wing.strip_prefix("wing_").map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        wing = crate::mcp_server::diary_wing(&agent);
    }
    let source_file = meta_str(meta, "source_file").unwrap_or_default();
    let chunk_index = meta_f64(meta, "chunk_index").unwrap_or(0.0).max(0.0) as i64;
    // Mined chunks get palace's deterministic ID so a later re-mine of the
    // same file lands on them; hand-filed drawers and diaries keep theirs.
    let id = if source_file.is_empty() || is_diary {
        record.id.clone()
    } else {
        crate::store::drawer_id(&wing, &room, &source_file, chunk_index as usize)
    };
    let added_by = meta_str(meta, "added_by")
        .or(agent)
        .unwrap_or_else(|| "mempalace".to_string());
    let importance = meta_f64(meta, "importance").unwrap_or(3.0);

    let mut extra: Map<String, Value> = meta
        .iter()
        .filter(|(key, _)| !COLUMN_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    extra.insert("migrated_from".into(), Value::from("mempalace"));
    extra.insert("legacy_id".into(), Value::from(record.id.clone()));

    Some(MappedDrawer {
        id,
        wing,
        room,
        content: record.document.clone(),
        source_file,
        chunk_index,
        added_by,
        importance,
        filed_at: meta_str(meta, "filed_at"),
        is_diary,
        metadata: Value::Object(extra),
    })
}

fn import_drawers(
    dest: &Connection,
    records: &[LegacyRecord],
    report: &mut ImportReport,
) -> Result<()> {
    let mapped: Vec<MappedDrawer> = records.iter().filter_map(map_record).collect();
    report.drawers_skipped += records.len() - mapped.len();
    for batch in mapped.chunks(IMPORT_BATCH) {
        let texts: Vec<&str> = batch.iter().map(|d| d.content.as_str()).collect();
        let embeddings = crate::embedder::embed_documents(&texts).unwrap_or_default();
        crate::store::with_write_batch(dest, |conn| {
            for (index, drawer) in batch.iter().enumerate() {
                let added = crate::store::add_drawer_with_id(
                    conn,
                    &drawer.id,
                    &drawer.wing,
                    &drawer.room,
                    &drawer.content,
                    embeddings.get(index).map(Vec::as_slice),
                    &drawer.source_file,
                    &drawer.added_by,
                    Some(&drawer.metadata),
                )
                .with_context(|| format!("importing drawer {}", drawer.id))?;
                if !added {
                    report.drawers_skipped += 1;
                    continue;
                }
                conn.execute(
                    "UPDATE drawers
                     SET chunk_index = ?2, importance = ?3,
                         filed_at = COALESCE(?4, filed_at),
                         created_at = COALESCE(?4, created_at)
                     WHERE id = ?1",
                    rusqlite::params![
                        drawer.id,
                        drawer.chunk_index,
                        drawer.importance,
                        drawer.filed_at
                    ],
                )
                .context("restoring drawer timestamps")?;
                if drawer.is_diary {
                    report.diary_entries += 1;
                } else {
                    report.drawers += 1;
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn import_knowledge_graph(path: &Path, dest: &Connection, report: &mut ImportReport) -> Result<()> {
    let source = open_read_only(path)?;
    let mut entities = source
        .prepare("SELECT name, COALESCE(type, 'unknown'), COALESCE(properties, '{}') FROM entities")
        .context("reading knowledge graph entities")?;
    let entity_rows = entities
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Expired facts first, so a fact that was closed and re-opened keeps both rows.
    let mut triples = source
        .prepare(
            "SELECT s.name, t.predicate, o.name, t.valid_from, t.valid_to,
                    COALESCE(t.confidence, 1.0), t.source_closet, t.source_file, t.extracted_at
             FROM triples t
             JOIN entities s ON s.id = t.subject
             JOIN entities o ON o.id = t.object
             ORDER BY t.valid_to IS NULL, t.extracted_at",
        )
        .context("reading knowledge graph triples")?;
    type Row = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        f64,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let triple_rows: Vec<Row> = triples
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    crate::store::with_write_batch(dest, |conn| {
        for (name, entity_type, properties) in &entity_rows {
            let existed: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM entities WHERE name = ?1)",
                    [name],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(false);
            crate::knowledge_graph::import_entity(conn, name, entity_type, properties)?;
            if !existed {
                report.entities += 1;
            }
        }
        for row in &triple_rows {
            let triple = crate::knowledge_graph::ImportedTriple {
                subject: &row.0,
                predicate: &row.1,
                object: &row.2,
                valid_from: row.3.as_deref(),
                valid_to: row.4.as_deref(),
                confidence: row.5,
                source_closet: row.6.as_deref(),
                source_file: row.7.as_deref(),
                extracted_at: row.8.as_deref(),
            };
            if crate::knowledge_graph::import_triple(conn, &triple)? {
                report.triples += 1;
            }
        }
        Ok(())
    })
}

fn import_tunnels(path: &Path, dest: &Connection) -> Result<usize> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let Value::Array(tunnels) =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
    else {
        bail!("{} is not a JSON list of tunnels", path.display());
    };
    let before: i64 = dest.query_row("SELECT COUNT(*) FROM tunnels", [], |row| row.get(0))?;
    for tunnel in &tunnels {
        let end = |side: &str, key: &str| {
            tunnel
                .get(side)
                .and_then(|end| end.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let (Some(wing_a), Some(room_a), Some(wing_b), Some(room_b)) = (
            end("source", "wing"),
            end("source", "room"),
            end("target", "wing"),
            end("target", "room"),
        ) else {
            continue;
        };
        let kind = tunnel
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("explicit");
        crate::palace_graph::create_tunnel(dest, &wing_a, &room_a, &wing_b, &room_b, kind)?;
    }
    let after: i64 = dest.query_row("SELECT COUNT(*) FROM tunnels", [], |row| row.get(0))?;
    Ok((after - before).max(0) as usize)
}
