//! SQLite-to-SQLite migration helpers for older palace files.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Backfill the `wings` registry from distinct wings already present on drawers.
///
/// Idempotent: existing registry rows are never overwritten (uses
/// `INSERT OR IGNORE`). Diary, `general`, and `conversations` wings are inferred
/// as topics; everything else is assumed to be a project wing. Project paths and
/// `last_mined_at` are left NULL here — they get populated by the miner.
///
/// Returns the number of registry rows newly inserted.
pub fn backfill_wings_registry(conn: &Connection) -> Result<usize> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO wings (name, kind)
             SELECT DISTINCT wing,
                    CASE
                        WHEN wing LIKE 'wing_diary__%' THEN 'topic'
                        WHEN wing IN ('general', 'conversations') THEN 'topic'
                        ELSE 'project'
                    END
             FROM drawers",
            [],
        )
        .context("backfilling wings registry")?;
    Ok(inserted)
}

pub fn migrate_sqlite(source: &Path, dest: &mut Connection) -> Result<usize> {
    let source_conn =
        Connection::open(source).with_context(|| format!("opening source {}", source.display()))?;
    let mut stmt = source_conn
        .prepare(
            "SELECT id, wing, room, content, source_file, chunk_index, added_by, importance
             FROM drawers",
        )
        .context("reading source drawers")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, f64>(7)?,
        ))
    })?;

    let mut migrated = 0usize;
    for row in rows {
        let (_id, wing, room, content, source_file, chunk_index, added_by, importance) =
            row.context("reading source drawer row")?;
        let (added, _) = crate::store::add_drawer(
            dest,
            &wing,
            &room,
            &content,
            None,
            &source_file,
            chunk_index as usize,
            &added_by,
            importance,
        )?;
        if added {
            migrated += 1;
        }
    }
    Ok(migrated)
}
