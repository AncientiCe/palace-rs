//! SQLite-to-SQLite migration helpers for older palace files.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

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
