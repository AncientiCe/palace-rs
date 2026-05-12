//! Incremental file watcher — re-mines changed files into the palace.
//!
//! Uses the `notify` crate to watch a project directory. When a tracked file
//! is created or modified the watcher re-mines that individual file using the
//! existing room configuration.

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use crate::miner::{chunk_text, detect_room, MIN_CHUNK_SIZE, READABLE_EXTENSIONS, SKIP_FILENAMES};
use crate::room_detector::load_config;
use crate::store::add_drawer;

/// Watch `project_dir` for file changes and re-mine changed files into the
/// palace database at `db_path`.
///
/// Blocks the calling thread until the user presses Ctrl-C.
pub fn watch(db_path: &Path, project_dir: &Path, wing_override: Option<&str>) -> Result<()> {
    let project_path = project_dir
        .canonicalize()
        .context("resolving project dir")?;

    let config = load_config(&project_path)?;
    let wing = wing_override.unwrap_or(&config.wing).to_string();
    let rooms = config.rooms.clone();

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        // Ignore send errors (receiver may have closed)
        let _ = tx.send(res);
    })
    .context("creating file watcher")?;

    watcher
        .watch(&project_path, RecursiveMode::Recursive)
        .context("starting recursive watch")?;

    println!(
        "\n  Watching {} for changes (Ctrl-C to stop)…",
        project_path.display()
    );
    println!("  Wing: {wing}\n");

    for raw in rx.iter() {
        let event = match raw {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "watcher error, continuing");
                continue;
            }
        };

        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {}
            _ => continue,
        }

        let paths: Vec<PathBuf> = event
            .paths
            .into_iter()
            .filter(|p| is_watched_file(p))
            .collect();

        if paths.is_empty() {
            continue;
        }

        // Small debounce — coalesce rapid successive saves.
        std::thread::sleep(Duration::from_millis(200));

        let mut conn = crate::db::open(db_path).context("opening palace db")?;

        for filepath in &paths {
            if let Err(e) = mine_file(&mut conn, filepath, &wing, &rooms, &project_path) {
                tracing::warn!(path = %filepath.display(), error = %e, "re-mine failed");
            }
        }
    }

    Ok(())
}

fn is_watched_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if SKIP_FILENAMES.contains(&name.as_ref()) {
        return false;
    }
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    READABLE_EXTENSIONS.contains(&ext.as_str())
}

fn mine_file(
    conn: &mut rusqlite::Connection,
    filepath: &Path,
    wing: &str,
    rooms: &[crate::room_detector::Room],
    project_path: &Path,
) -> Result<()> {
    let content = std::fs::read_to_string(filepath).context("reading file")?;
    let content = content.trim().to_string();
    if content.len() < MIN_CHUNK_SIZE {
        return Ok(());
    }

    let room = detect_room(filepath, &content, rooms, project_path);
    let chunks = chunk_text(&content);
    if chunks.is_empty() {
        return Ok(());
    }

    let source_file = filepath.to_string_lossy().to_string();
    let chunk_texts: Vec<&str> = chunks.iter().map(|(t, _)| t.as_str()).collect();
    let embeddings = crate::embedder::embed_batch(&chunk_texts).unwrap_or_default();

    let mut added = 0usize;
    for (idx, (chunk_text, chunk_index)) in chunks.iter().enumerate() {
        let emb = embeddings.get(idx).map(|e| e.as_slice());
        let (new, _) = add_drawer(
            conn,
            wing,
            &room,
            chunk_text,
            emb,
            &source_file,
            *chunk_index,
            "palace-watch",
            3.0,
        )?;
        if new {
            added += 1;
        }
    }

    if added > 0 {
        println!(
            "  ↺  {} → room:{room} +{added} drawer(s)",
            filepath.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_watched_file;
    use std::path::Path;

    #[test]
    fn watched_rs_file() {
        assert!(is_watched_file(Path::new("src/main.rs")));
    }

    #[test]
    fn watched_md_file() {
        assert!(is_watched_file(Path::new("README.md")));
    }

    #[test]
    fn skip_palace_yaml() {
        assert!(!is_watched_file(Path::new("palace.yaml")));
    }

    #[test]
    fn skip_binary_file() {
        assert!(!is_watched_file(Path::new("binary.exe")));
    }

    #[test]
    fn skip_gitignore() {
        assert!(!is_watched_file(Path::new(".gitignore")));
    }
}
