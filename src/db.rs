//! Database layer: SQLite with WAL mode and schema migrations.
//!
//! Single palace.db file replaces both ChromaDB (drawers) and SQLite KG (triples).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

/// Open (or create) the palace SQLite database with WAL mode enabled.
pub fn open(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating palace directory {}", parent.display()))?;
    }

    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .with_context(|| format!("opening database {}", db_path.display()))?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
    )?;

    migrate(&conn)?;
    Ok(conn)
}

/// Open an in-memory database for testing.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Run schema migrations — idempotent, safe to call on every startup.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- ── Metadata ─────────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- ── Drawers ──────────────────────────────────────────────────────────
        -- Replaces ChromaDB palace_drawers collection.
        -- embedding is a little-endian f32 byte array (384 floats = 1536 bytes).
        CREATE TABLE IF NOT EXISTS drawers (
            id          TEXT PRIMARY KEY,
            wing        TEXT NOT NULL,
            room        TEXT NOT NULL,
            content     TEXT NOT NULL,
            embedding   BLOB,
            source_file TEXT NOT NULL DEFAULT '',
            chunk_index INTEGER NOT NULL DEFAULT 0,
            added_by    TEXT NOT NULL DEFAULT 'palace',
            filed_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            importance  REAL NOT NULL DEFAULT 3.0,
            created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            entity_metadata TEXT NOT NULL DEFAULT '{}',
            hall        TEXT,
            normalize_version INTEGER NOT NULL DEFAULT 0,
            metadata    TEXT NOT NULL DEFAULT '{}',
            pref_embedding BLOB
        );
        CREATE INDEX IF NOT EXISTS idx_drawers_wing ON drawers(wing);
        CREATE INDEX IF NOT EXISTS idx_drawers_room ON drawers(room);
        CREATE INDEX IF NOT EXISTS idx_drawers_source ON drawers(source_file);

        -- ── Closets ──────────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS closets (
            id          TEXT PRIMARY KEY,
            wing        TEXT NOT NULL,
            room        TEXT NOT NULL,
            topic       TEXT NOT NULL,
            pointer_drawer_ids TEXT NOT NULL DEFAULT '[]',
            embedding   BLOB,
            created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_closets_wing_room ON closets(wing, room);
        CREATE INDEX IF NOT EXISTS idx_closets_topic ON closets(topic);

        -- ── Tunnels ──────────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS tunnels (
            id          TEXT PRIMARY KEY,
            wing_a      TEXT NOT NULL,
            room_a      TEXT NOT NULL,
            wing_b      TEXT NOT NULL,
            room_b      TEXT NOT NULL,
            kind        TEXT NOT NULL DEFAULT 'explicit',
            created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(wing_a, room_a, wing_b, room_b, kind)
        );
        CREATE INDEX IF NOT EXISTS idx_tunnels_a ON tunnels(wing_a, room_a);
        CREATE INDEX IF NOT EXISTS idx_tunnels_b ON tunnels(wing_b, room_b);

        -- ── BM25 index ───────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS bm25_terms (
            drawer_id   TEXT NOT NULL REFERENCES drawers(id) ON DELETE CASCADE,
            term        TEXT NOT NULL,
            tf          INTEGER NOT NULL,
            PRIMARY KEY(drawer_id, term)
        );
        CREATE INDEX IF NOT EXISTS idx_bm25_terms_term ON bm25_terms(term);

        CREATE TABLE IF NOT EXISTS bm25_doc_stats (
            drawer_id   TEXT PRIMARY KEY REFERENCES drawers(id) ON DELETE CASCADE,
            doc_len     INTEGER NOT NULL,
            updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        -- ── KG Entities ───────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS entities (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            type        TEXT NOT NULL DEFAULT 'unknown',
            properties  TEXT NOT NULL DEFAULT '{}',
            created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        -- ── KG Triples ────────────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS triples (
            id              TEXT PRIMARY KEY,
            subject         TEXT NOT NULL REFERENCES entities(id),
            predicate       TEXT NOT NULL,
            object          TEXT NOT NULL REFERENCES entities(id),
            valid_from      TEXT,
            valid_to        TEXT,
            confidence      REAL NOT NULL DEFAULT 1.0,
            source_closet   TEXT,
            source_file     TEXT,
            extracted_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_triples_subject   ON triples(subject);
        CREATE INDEX IF NOT EXISTS idx_triples_object    ON triples(object);
        CREATE INDEX IF NOT EXISTS idx_triples_predicate ON triples(predicate);
        CREATE INDEX IF NOT EXISTS idx_triples_valid     ON triples(valid_from, valid_to);

        -- ── Usage telemetry ──────────────────────────────────────────────────
        CREATE TABLE IF NOT EXISTS usage_events (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            ts               TEXT NOT NULL,
            session_id       TEXT NOT NULL,
            project          TEXT NOT NULL,
            tool             TEXT NOT NULL,
            wing             TEXT,
            room             TEXT,
            query_hash       TEXT,
            result_count     INTEGER NOT NULL DEFAULT 0,
            top_similarity   REAL,
            bytes_returned   INTEGER NOT NULL DEFAULT 0,
            est_tokens_saved INTEGER NOT NULL DEFAULT 0,
            duration_ms      INTEGER NOT NULL DEFAULT 0,
            outcome          TEXT NOT NULL,
            meta             TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_usage_events_ts ON usage_events(ts);
        CREATE INDEX IF NOT EXISTS idx_usage_events_project_ts ON usage_events(project, ts);
        CREATE INDEX IF NOT EXISTS idx_usage_events_tool ON usage_events(tool);
        CREATE INDEX IF NOT EXISTS idx_usage_events_query_hash ON usage_events(query_hash);

        CREATE TABLE IF NOT EXISTS gain_feedback (
            query_id   TEXT NOT NULL,
            drawer_id  TEXT NOT NULL,
            verdict    TEXT NOT NULL,
            source     TEXT NOT NULL,
            note       TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(query_id, drawer_id, source)
        );
        CREATE INDEX IF NOT EXISTS idx_gain_feedback_query ON gain_feedback(query_id);
        CREATE INDEX IF NOT EXISTS idx_gain_feedback_drawer ON gain_feedback(drawer_id);
        "#,
    )
    .context("running schema migrations")?;

    add_column_if_missing(conn, "drawers", "created_at", "TEXT NOT NULL DEFAULT ''")?;
    add_column_if_missing(
        conn,
        "drawers",
        "entity_metadata",
        "TEXT NOT NULL DEFAULT '{}'",
    )?;
    add_column_if_missing(conn, "drawers", "hall", "TEXT")?;
    add_column_if_missing(
        conn,
        "drawers",
        "normalize_version",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(conn, "drawers", "metadata", "TEXT NOT NULL DEFAULT '{}'")?;
    add_column_if_missing(conn, "drawers", "pref_embedding", "BLOB")?;

    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_drawers_created_at ON drawers(created_at);
        CREATE INDEX IF NOT EXISTS idx_drawers_hall ON drawers(hall);
        "#,
    )
    .context("creating phase one drawer indexes")?;

    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params!["schema_version", "1"],
    )
    .context("recording schema version")?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("reading columns for {table}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if columns.iter().any(|existing| existing == column) {
        return Ok(());
    }

    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )
    .with_context(|| format!("adding {table}.{column}"))?;
    Ok(())
}
