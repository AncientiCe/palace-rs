//! Closet layer: compact topical pointers to verbatim drawers.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::embedder::vec_to_blob;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Closet {
    pub id: String,
    pub wing: String,
    pub room: String,
    pub topic: String,
    pub pointer_drawer_ids: Vec<String>,
}

pub fn closet_id(wing: &str, room: &str, topic: &str) -> String {
    let hash = blake3::hash(format!("{wing}/{room}/{topic}").as_bytes());
    format!("closet_{wing}_{room}_{}", &hash.to_hex()[..16])
}

pub fn add_closet(
    conn: &Connection,
    wing: &str,
    room: &str,
    topic: &str,
    pointer_drawer_ids: &[String],
    embedding: Option<&[f32]>,
) -> Result<String> {
    let id = closet_id(wing, room, topic);
    let pointers =
        serde_json::to_string(pointer_drawer_ids).context("serializing closet pointers")?;
    let embedding_blob = embedding.map(vec_to_blob);
    conn.execute(
        "INSERT INTO closets (id, wing, room, topic, pointer_drawer_ids, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
            pointer_drawer_ids = excluded.pointer_drawer_ids,
            embedding = excluded.embedding",
        params![id, wing, room, topic, pointers, embedding_blob],
    )
    .context("upserting closet")?;
    Ok(id)
}

pub fn list_closets(
    conn: &Connection,
    wing: Option<&str>,
    room: Option<&str>,
) -> Result<Vec<Closet>> {
    let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match (wing, room) {
        (Some(w), Some(r)) => (
            "SELECT id, wing, room, topic, pointer_drawer_ids FROM closets
             WHERE wing = ?1 AND room = ?2 ORDER BY topic",
            vec![Box::new(w.to_string()), Box::new(r.to_string())],
        ),
        (Some(w), None) => (
            "SELECT id, wing, room, topic, pointer_drawer_ids FROM closets
             WHERE wing = ?1 ORDER BY room, topic",
            vec![Box::new(w.to_string())],
        ),
        (None, Some(r)) => (
            "SELECT id, wing, room, topic, pointer_drawer_ids FROM closets
             WHERE room = ?1 ORDER BY wing, topic",
            vec![Box::new(r.to_string())],
        ),
        (None, None) => (
            "SELECT id, wing, room, topic, pointer_drawer_ids FROM closets
             ORDER BY wing, room, topic",
            vec![],
        ),
    };

    let mut stmt = conn.prepare(sql).context("preparing closet list")?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|param| param.as_ref())),
        |row| {
            let pointers_text: String = row.get(4)?;
            let pointer_drawer_ids = serde_json::from_str(&pointers_text).unwrap_or_default();
            Ok(Closet {
                id: row.get(0)?,
                wing: row.get(1)?,
                room: row.get(2)?,
                topic: row.get(3)?,
                pointer_drawer_ids,
            })
        },
    )?;

    rows.map(|row| row.context("reading closet row")).collect()
}
