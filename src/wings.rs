//! Wing names: which wing a write lands in, and renaming or merging a wing.
//!
//! Agents spell wing names loosely ("my-proj", "My Proj"). Left alone, each
//! spelling becomes its own wing and searches scoped to the real one miss
//! what was filed under the others. [`resolve_wing`] sends a write to the
//! wing that already exists, and [`rename_wing`] repairs a palace that has
//! already split.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::config::normalize_wing_name;

/// Diary wings are named by the diary tools (`wing_diary__<agent>`), with a
/// double underscore and the agent's own spelling, so they are never rewritten.
const DIARY_WING_PREFIX: &str = "wing_diary__";

/// The wing a write to `name` should land in.
///
/// A wing that already exists under exactly this name is kept, so palaces
/// with older, unnormalized wings keep filing into them. Otherwise the name
/// is normalized (lower case, `_` between words), which is also how a
/// misspelling of an existing wing finds it.
pub fn resolve_wing(conn: &Connection, name: &str) -> Result<String> {
    let name = name.trim();
    if name.starts_with(DIARY_WING_PREFIX) || wing_exists(conn, name)? {
        return Ok(name.to_string());
    }
    let normalized = normalize_wing_name(name);
    if normalized.is_empty() {
        bail!("wing name '{name}' has no letters or digits");
    }
    Ok(normalized)
}

/// True when `name` is registered or has drawers.
fn wing_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM wings WHERE name = ?1)
             OR EXISTS(SELECT 1 FROM drawers WHERE wing = ?1)",
        params![name],
        |row| row.get(0),
    )
    .context("checking whether the wing exists")
}

/// What [`rename_wing`] moved.
#[derive(Debug, Clone, Serialize)]
pub struct WingRename {
    pub from: String,
    /// The wing everything is filed under now.
    pub to: String,
    /// True when `to` already existed and the two wings were merged.
    pub merged: bool,
    pub drawers: usize,
    pub closets: usize,
    pub tunnels: usize,
    pub mined_files: usize,
    /// The renamed wing's project folder, when it was a mined project. Its
    /// `palace.yaml` names the wing and should follow (see [`sync_palace_yaml`]).
    pub project_path: Option<String>,
}

/// Rename wing `from` to `to`, or merge it into `to` when that wing exists.
///
/// Drawers keep their ids, so citations of them stay valid; an id still
/// spells the wing it was filed under. Closets and tunnels are re-keyed,
/// since their ids are derived from the wing, and ones that coincide after
/// the move are joined. Runs in one transaction.
pub fn rename_wing(conn: &Connection, from: &str, to: &str) -> Result<WingRename> {
    let from = from.trim();
    if !wing_exists(conn, from)? {
        bail!("no wing named '{from}'");
    }
    let to = resolve_wing(conn, to)?;
    if to == from {
        bail!("the wing is already named '{from}'");
    }
    let merged = wing_exists(conn, &to)?;

    crate::store::with_write_batch(conn, |conn| {
        let drawers = conn
            .execute(
                "UPDATE drawers SET wing = ?2 WHERE wing = ?1",
                params![from, to],
            )
            .context("moving drawers")?;
        let mined_files = conn
            .execute(
                "UPDATE mined_files SET wing = ?2 WHERE wing = ?1",
                params![from, to],
            )
            .context("moving mined files")?;
        conn.execute(
            "UPDATE usage_events SET wing = ?2 WHERE wing = ?1",
            params![from, to],
        )
        .context("moving usage events")?;
        let closets = move_closets(conn, from, &to)?;
        let tunnels = move_tunnels(conn, from, &to)?;
        let project_path = move_registry_entry(conn, from, &to)?;
        Ok(WingRename {
            from: from.to_string(),
            to: to.clone(),
            merged,
            drawers,
            closets,
            tunnels,
            mined_files,
            project_path,
        })
    })
}

/// Re-key the wing's closets. A closet whose room and topic already exist
/// in the target is folded into that one.
fn move_closets(conn: &Connection, from: &str, to: &str) -> Result<usize> {
    let closets = crate::closet::list_closets(conn, Some(from), None)?;
    for closet in &closets {
        let new_id = crate::closet::closet_id(to, &closet.room, &closet.topic);
        let existing: Option<String> = conn
            .query_row(
                "SELECT pointer_drawer_ids FROM closets WHERE id = ?1",
                params![new_id],
                |row| row.get(0),
            )
            .optional()
            .context("reading the target closet")?;
        match existing {
            Some(pointers) => {
                let mut joined: Vec<String> = serde_json::from_str(&pointers).unwrap_or_default();
                for pointer in &closet.pointer_drawer_ids {
                    if !joined.contains(pointer) {
                        joined.push(pointer.clone());
                    }
                }
                let joined = serde_json::to_string(&joined).context("serializing pointers")?;
                conn.execute(
                    "UPDATE closets SET pointer_drawer_ids = ?2 WHERE id = ?1",
                    params![new_id, joined],
                )
                .context("joining closets")?;
                conn.execute("DELETE FROM closets WHERE id = ?1", params![closet.id])
                    .context("removing the joined closet")?;
            }
            None => {
                conn.execute(
                    "UPDATE closets SET id = ?2, wing = ?3 WHERE id = ?1",
                    params![closet.id, new_id, to],
                )
                .context("moving closet")?;
                conn.execute(
                    "UPDATE embedding_vectors SET owner_id = ?2
                     WHERE owner_kind = 'closet' AND owner_id = ?1",
                    params![closet.id, new_id],
                )
                .context("moving closet vectors")?;
            }
        }
    }
    Ok(closets.len())
}

/// Re-key the wing's tunnels. Tunnels store normalized wing names. One that
/// would duplicate an existing tunnel, or join a room to itself, is dropped.
fn move_tunnels(conn: &Connection, from: &str, to: &str) -> Result<usize> {
    let from = normalize_wing_name(from);
    let to = normalize_wing_name(to);
    if from == to {
        return Ok(0);
    }
    let tunnels = crate::palace_graph::list_tunnels(conn, Some(&from), None)?;
    for tunnel in &tunnels {
        conn.execute("DELETE FROM tunnels WHERE id = ?1", params![tunnel.id])
            .context("removing the old tunnel")?;
        let wing_a = if tunnel.wing_a == from {
            &to
        } else {
            &tunnel.wing_a
        };
        let wing_b = if tunnel.wing_b == from {
            &to
        } else {
            &tunnel.wing_b
        };
        if wing_a == wing_b && tunnel.room_a == tunnel.room_b {
            continue;
        }
        crate::palace_graph::create_tunnel(
            conn,
            wing_a,
            &tunnel.room_a,
            wing_b,
            &tunnel.room_b,
            &tunnel.kind,
        )?;
    }
    Ok(tunnels.len())
}

/// Move the registry row, or fold it into the target's: a project stays a
/// project, and the target keeps its own description, path and mine time
/// where it has them. Returns the renamed wing's project path.
fn move_registry_entry(conn: &Connection, from: &str, to: &str) -> Result<Option<String>> {
    let Some(source) = crate::store::get_wing(conn, from)? else {
        crate::store::ensure_wing_registered(conn, to)?;
        return Ok(None);
    };
    conn.execute("DELETE FROM wings WHERE name = ?1", params![from])
        .context("removing the old registry entry")?;
    conn.execute(
        "INSERT INTO wings (name, kind, description, project_path, last_mined_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET
             kind = CASE
                 WHEN excluded.kind = 'project' OR wings.kind = 'project' THEN 'project'
                 ELSE wings.kind
             END,
             description = CASE
                 WHEN wings.description != '' THEN wings.description
                 ELSE excluded.description
             END,
             project_path = COALESCE(wings.project_path, excluded.project_path),
             last_mined_at = COALESCE(wings.last_mined_at, excluded.last_mined_at)",
        params![
            to,
            source.kind,
            source.description,
            source.project_path,
            source.last_mined_at,
            source.created_at
        ],
    )
    .context("moving the registry entry")?;
    Ok(source.project_path)
}

/// Point `project_dir/palace.yaml` at wing `to` when it names wing `from`,
/// so the next `palace mine` files into the renamed wing. Only the `wing:`
/// line changes. Returns the file's path when it was rewritten.
pub fn sync_palace_yaml(project_dir: &Path, from: &str, to: &str) -> Result<Option<PathBuf>> {
    let path = project_dir.join("palace.yaml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut changed = false;
    let lines: Vec<String> = content
        .split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\r', '\n']);
            let names_wing = body
                .strip_prefix("wing:")
                .is_some_and(|value| value.trim().trim_matches(['"', '\'']) == from);
            if names_wing && !changed {
                changed = true;
                format!("wing: {to}{}", &line[body.len()..])
            } else {
                line.to_string()
            }
        })
        .collect();
    if !changed {
        return Ok(None);
    }
    std::fs::write(&path, lines.concat())
        .with_context(|| format!("updating {}", path.display()))?;
    Ok(Some(path))
}
