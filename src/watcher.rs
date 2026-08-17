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
use crate::store::{content_hash, delete_drawers_for_file, replace_file_drawers};

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

        let is_remove = matches!(event.kind, EventKind::Remove(_));
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
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

        if let Err(e) = crate::store::set_wing_mined(&conn, &wing, &project_path.to_string_lossy())
        {
            tracing::warn!(error = %e, "recording wing mine status failed");
        }

        for filepath in &paths {
            let result = if is_remove {
                remove_file(&mut conn, filepath)
            } else {
                mine_file(&mut conn, filepath, &wing, &rooms, &project_path)
            };
            if let Err(e) = result {
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

/// Re-mine a single changed file, replacing (not appending to) its existing
/// drawers so edits are actually picked up rather than being silently
/// ignored by the old `INSERT OR IGNORE` behaviour.
fn mine_file(
    conn: &mut rusqlite::Connection,
    filepath: &Path,
    wing: &str,
    rooms: &[crate::room_detector::Room],
    project_path: &Path,
) -> Result<()> {
    let content = std::fs::read_to_string(filepath).context("reading file")?;
    let content = content.trim().to_string();
    let source_file = filepath.to_string_lossy().to_string();

    let chunks = chunk_text(&content);
    if content.len() < MIN_CHUNK_SIZE || chunks.is_empty() {
        // Content shrank below the minimum: clear any drawers left over from
        // a previous, longer version of this file instead of leaving them
        // stale and unreachable from disk.
        delete_drawers_for_file(conn, &source_file)?;
        return Ok(());
    }

    let room = detect_room(filepath, &content, rooms, project_path);
    let hash = content_hash(&content);
    let chunk_texts: Vec<&str> = chunks.iter().map(|(t, _)| t.as_str()).collect();
    let embeddings = crate::embedder::embed_batch(&chunk_texts).unwrap_or_default();
    let chunk_entries: Vec<(String, usize, Vec<f32>)> = chunks
        .into_iter()
        .enumerate()
        .map(|(idx, (text, chunk_index))| {
            let embedding = embeddings.get(idx).cloned().unwrap_or_default();
            (text, chunk_index, embedding)
        })
        .collect();

    let added = replace_file_drawers(
        conn,
        wing,
        &room,
        &source_file,
        &hash,
        &chunk_entries,
        "palace-watch",
        3.0,
    )?;

    if added > 0 {
        println!(
            "  ↺  {} → room:{room} +{added} drawer(s)",
            filepath.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    Ok(())
}

/// Remove a deleted file's drawers from the palace.
fn remove_file(conn: &mut rusqlite::Connection, filepath: &Path) -> Result<()> {
    let source_file = filepath.to_string_lossy().to_string();
    let removed = delete_drawers_for_file(conn, &source_file)?;
    if removed > 0 {
        println!(
            "  ✗  {} removed ({removed} drawer(s))",
            filepath.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_watched_file, mine_file, remove_file};
    use std::path::Path;
    use tempfile::TempDir;

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

    #[test]
    fn mine_file_updates_content_on_second_call() {
        let project = TempDir::new().unwrap();
        let rooms = vec![crate::room_detector::Room {
            name: "general".into(),
            description: "general".into(),
            keywords: vec![],
        }];
        let file_path = project.path().join("note.txt");
        std::fs::write(
            &file_path,
            "first version of the watched file with enough content to chunk.",
        )
        .unwrap();

        let mut conn = crate::db::open_in_memory().unwrap();
        mine_file(&mut conn, &file_path, "wing_x", &rooms, project.path()).unwrap();
        let drawers =
            crate::store::list_drawers(&conn, &crate::store::DrawerFilter::default(), 10).unwrap();
        assert_eq!(drawers.len(), 1);
        assert!(drawers[0].content.contains("first version"));

        std::fs::write(
            &file_path,
            "second version of the watched file with different unique content.",
        )
        .unwrap();
        mine_file(&mut conn, &file_path, "wing_x", &rooms, project.path()).unwrap();

        let drawers =
            crate::store::list_drawers(&conn, &crate::store::DrawerFilter::default(), 10).unwrap();
        assert_eq!(
            drawers.len(),
            1,
            "re-mining a modified file should replace, not append"
        );
        assert!(drawers[0].content.contains("second version"));
    }

    #[test]
    fn remove_file_deletes_drawers_for_deleted_path() {
        let project = TempDir::new().unwrap();
        let rooms = vec![crate::room_detector::Room {
            name: "general".into(),
            description: "general".into(),
            keywords: vec![],
        }];
        let file_path = project.path().join("gone.txt");
        std::fs::write(
            &file_path,
            "content that will be removed from disk and from the palace.",
        )
        .unwrap();

        let mut conn = crate::db::open_in_memory().unwrap();
        mine_file(&mut conn, &file_path, "wing_x", &rooms, project.path()).unwrap();
        assert_eq!(crate::store::count_drawers(&conn).unwrap(), 1);

        std::fs::remove_file(&file_path).unwrap();
        remove_file(&mut conn, &file_path).unwrap();

        assert_eq!(crate::store::count_drawers(&conn).unwrap(), 0);
    }
}
