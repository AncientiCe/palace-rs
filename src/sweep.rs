//! Message-level transcript sweeper.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;

pub fn sweep_path(conn: &Connection, path: &Path, wing: Option<&str>) -> Result<usize> {
    let mut filed = 0usize;
    if path.is_dir() {
        for entry in
            std::fs::read_dir(path).with_context(|| format!("reading {}", path.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() || path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                filed += sweep_path(conn, &path, wing)?;
            }
        }
        return Ok(filed);
    }

    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Ok(0);
    }

    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let wing = wing.unwrap_or("conversations");
    for (idx, line) in text.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = value
            .get("role")
            .or_else(|| value.get("type"))
            .and_then(|value| value.as_str())
            .unwrap_or("message");
        let content = value
            .get("content")
            .or_else(|| value.get("text"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if content.trim().is_empty() {
            continue;
        }
        let body = format!("{role}: {content}");
        let (added, _) = crate::store::add_drawer(
            conn,
            wing,
            "messages",
            &body,
            None,
            &path.to_string_lossy(),
            idx,
            "sweep",
            3.0,
        )?;
        if added {
            filed += 1;
        }
    }
    Ok(filed)
}
