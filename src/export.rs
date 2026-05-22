//! Palace export / import — portable JSON snapshots of all drawers.
//!
//! # Export
//! Serialises every drawer (text, wing, room, metadata, etc.) into a JSON
//! array.  Embeddings are intentionally excluded — they are re-derived on
//! import using the local model, keeping the snapshot compact and
//! model-agnostic.
//!
//! # Import
//! Reads an export file produced by `export_drawers`.  Drawers that already
//! exist (same deterministic ID) are skipped; new drawers are embedded and
//! inserted.  Returns the count of newly inserted drawers.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::store::{add_drawer, DrawerFilter};

/// A single drawer entry as stored in an export file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedDrawer {
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
}

/// Top-level export document.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportDoc {
    pub palace_version: String,
    pub exported_at: String,
    pub total: usize,
    pub drawers: Vec<ExportedDrawer>,
}

/// Serialise all drawers in `conn` to an [`ExportDoc`].
///
/// Embeddings are excluded — they are re-computed on import.
pub fn export_drawers(conn: &Connection) -> Result<ExportDoc> {
    let filter = DrawerFilter::default();
    let drawers = crate::store::list_drawers(conn, &filter, usize::MAX)
        .context("listing drawers for export")?;

    let exported: Vec<ExportedDrawer> = drawers
        .into_iter()
        .map(|d| ExportedDrawer {
            id: d.id,
            wing: d.wing,
            room: d.room,
            content: d.content,
            source_file: d.source_file,
            chunk_index: d.chunk_index,
            added_by: d.added_by,
            filed_at: d.filed_at,
            created_at: d.created_at,
            importance: d.importance,
        })
        .collect();

    let total = exported.len();
    Ok(ExportDoc {
        palace_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        total,
        drawers: exported,
    })
}

/// Import drawers from an [`ExportDoc`] into `conn`.
///
/// Drawers whose ID already exists are skipped (idempotent).
/// Returns the count of newly inserted drawers.
pub fn import_drawers(conn: &Connection, doc: &ExportDoc) -> Result<usize> {
    let mut inserted = 0usize;
    for d in &doc.drawers {
        let embedding = crate::embedder::embed_one(&d.content).ok();
        let emb_ref = embedding.as_deref();
        let (new, _id) = add_drawer(
            conn,
            &d.wing,
            &d.room,
            &d.content,
            emb_ref,
            &d.source_file,
            d.chunk_index as usize,
            &d.added_by,
            d.importance,
        )
        .with_context(|| format!("importing drawer {}", d.id))?;
        if new {
            inserted += 1;
        }
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::store::add_drawer;

    fn make_db() -> Connection {
        db::open_in_memory().expect("in-memory db")
    }

    #[test]
    fn export_empty_palace_produces_empty_doc() {
        let conn = make_db();
        let doc = export_drawers(&conn).unwrap();
        assert_eq!(doc.total, 0);
        assert!(doc.drawers.is_empty());
        assert!(!doc.palace_version.is_empty());
    }

    #[test]
    fn export_roundtrip_skips_existing_drawers() {
        let conn = make_db();
        add_drawer(
            &conn,
            "test_wing",
            "test_room",
            "Hello palace export test",
            None,
            "test.txt",
            0,
            "test",
            3.0,
        )
        .unwrap();

        let doc = export_drawers(&conn).unwrap();
        assert_eq!(doc.total, 1);

        // Import into the same DB — should be a no-op (already exists).
        let conn2 = make_db();
        add_drawer(
            &conn2,
            "test_wing",
            "test_room",
            "Hello palace export test",
            None,
            "test.txt",
            0,
            "test",
            3.0,
        )
        .unwrap();
        let imported = import_drawers(&conn2, &doc).unwrap();
        assert_eq!(imported, 0, "should skip the already-existing drawer");
    }

    #[test]
    fn import_new_drawers_increments_count() {
        let src = make_db();
        add_drawer(
            &src,
            "w",
            "r",
            "unique import test content here",
            None,
            "f.txt",
            0,
            "test",
            3.0,
        )
        .unwrap();

        let doc = export_drawers(&src).unwrap();

        let dst = make_db();
        let imported = import_drawers(&dst, &doc).unwrap();
        assert_eq!(imported, 1);
    }
}
