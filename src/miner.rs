//! Project file ingestor.
//!
//! Reads palace.yaml (or mempalace.yaml as a legacy fallback), walks the project
//! with gitignore respect (via `ignore` crate), chunks text (~800 chars, 100 overlap),
//! routes to rooms, embeds and stores drawers.
//!
//! File reading and embedding are parallelised with Rayon; SQLite writes remain
//! single-threaded (rusqlite Connection is not Send).

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

use crate::room_detector::{load_config, Room};
use crate::store::{add_drawer, file_already_mined};

pub const CHUNK_SIZE: usize = 800;
pub const CHUNK_OVERLAP: usize = 100;
pub const MIN_CHUNK_SIZE: usize = 50;

pub static READABLE_EXTENSIONS: &[&str] = &[
    "txt", "md", "py", "js", "ts", "jsx", "tsx", "json", "yaml", "yml", "html", "css", "java",
    "go", "rs", "rb", "sh", "csv", "sql", "toml",
];

pub static SKIP_FILENAMES: &[&str] = &[
    "palace.yaml",
    "palace.yml",
    "mempalace.yaml",
    "mempalace.yml",
    "mempal.yaml",
    "mempal.yml",
    ".gitignore",
    "package-lock.json",
];

/// Walk `index` back to the nearest UTF-8 char boundary at or below it.
///
/// `str::floor_char_boundary` is unstable, so this provides the same behaviour
/// on stable Rust. `index` may be `s.len()`; the result is always a valid byte
/// offset into `s`.
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Split content into overlapping chunks, preferring paragraph/line boundaries.
pub fn chunk_text(content: &str) -> Vec<(String, usize)> {
    let content = content.trim();
    if content.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let bytes = content.as_bytes();
    let total = bytes.len();
    let mut start = 0;
    let mut chunk_index = 0;

    while start < total {
        let end = floor_char_boundary(content, (start + CHUNK_SIZE).min(total));
        let mut cut = end;

        // Try to break at double newline first
        if cut < total {
            if let Some(pos) = content[start..cut].rfind("\n\n") {
                let abs = start + pos;
                if abs > start + CHUNK_SIZE / 2 {
                    cut = abs;
                }
            } else if let Some(pos) = content[start..cut].rfind('\n') {
                let abs = start + pos;
                if abs > start + CHUNK_SIZE / 2 {
                    cut = abs;
                }
            }
        }

        let chunk = content[start..cut].trim().to_string();
        if chunk.len() >= MIN_CHUNK_SIZE {
            chunks.push((chunk, chunk_index));
            chunk_index += 1;
        }

        if cut >= total {
            break;
        }
        let next = cut.saturating_sub(CHUNK_OVERLAP);
        let next = floor_char_boundary(content, next);
        // Guarantee forward progress even if boundary rewind collapses the window.
        start = if next > start { next } else { cut };
    }

    chunks
}

/// Route a file to the correct room based on path, filename, and keyword scoring.
pub fn detect_room(filepath: &Path, content: &str, rooms: &[Room], project_path: &Path) -> String {
    let relative = filepath
        .strip_prefix(project_path)
        .unwrap_or(filepath)
        .to_string_lossy()
        .to_lowercase();
    let filename = filepath
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let content_lower = content
        .get(..2000.min(content.len()))
        .unwrap_or(content)
        .to_lowercase();

    // Priority 1: folder path matches room name or keywords
    let path_parts: Vec<&str> = relative.split('/').collect();
    for part in path_parts.iter().take(path_parts.len().saturating_sub(1)) {
        for room in rooms {
            let candidates: Vec<String> = std::iter::once(room.name.to_lowercase())
                .chain(room.keywords.iter().map(|k| k.to_lowercase()))
                .collect();
            if candidates
                .iter()
                .any(|c| part == c || c.contains(part) || part.contains(c.as_str()))
            {
                return room.name.clone();
            }
        }
    }

    // Priority 2: filename matches room name
    for room in rooms {
        if room.name.to_lowercase().contains(&filename)
            || filename.contains(&room.name.to_lowercase())
        {
            return room.name.clone();
        }
    }

    // Priority 3: keyword scoring
    let mut scores: HashMap<&str, usize> = HashMap::new();
    for room in rooms {
        let keywords: Vec<String> = std::iter::once(room.name.clone())
            .chain(room.keywords.iter().cloned())
            .collect();
        let score: usize = keywords
            .iter()
            .map(|kw| {
                let kw_lower = kw.to_lowercase();
                content_lower.matches(kw_lower.as_str()).count()
            })
            .sum();
        if score > 0 {
            scores.insert(&room.name, score);
        }
    }

    if let Some(best) = scores.iter().max_by_key(|(_, v)| **v) {
        return best.0.to_string();
    }

    "general".to_string()
}

/// Mine a project directory into the palace.
#[allow(clippy::too_many_arguments)]
pub fn mine(
    conn: &mut Connection,
    project_dir: &Path,
    wing_override: Option<&str>,
    agent: &str,
    limit: usize,
    dry_run: bool,
    respect_gitignore: bool,
    include_ignored: &[String],
) -> Result<()> {
    let project_path = project_dir
        .canonicalize()
        .context("resolving project dir")?;
    let config = load_config(&project_path)?;
    let wing = wing_override.unwrap_or(&config.wing).to_string();
    let rooms = config.rooms;

    // Collect files using `ignore` crate (gitignore-aware)
    let mut walker = WalkBuilder::new(&project_path);
    walker
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore);

    // Force-include paths that are normally ignored
    for path in include_ignored {
        walker.add(project_path.join(path));
    }

    let mut files: Vec<std::path::PathBuf> = walker
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if SKIP_FILENAMES.contains(&name.as_ref()) {
                return false;
            }
            let ext = p
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            READABLE_EXTENSIONS.contains(&ext.as_str())
        })
        .collect();

    if limit > 0 {
        files.truncate(limit);
    }

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Mine");
    println!("{}", "=".repeat(55));
    println!("  Wing:    {wing}");
    println!(
        "  Rooms:   {}",
        rooms
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  Files:   {}", files.len());
    if dry_run {
        println!("  DRY RUN — nothing will be filed");
    }
    println!("{}\n", "-".repeat(55));

    let mut total_drawers = 0usize;
    let mut files_skipped = 0usize;
    let mut room_counts: HashMap<String, usize> = HashMap::new();

    // ── Phase 1: filter already-mined files (serial, DB read) ──────────────
    let pending: Vec<_> = files
        .iter()
        .filter(|fp| {
            if dry_run {
                return true;
            }
            let source = fp.to_string_lossy();
            match file_already_mined(conn, &source) {
                Ok(true) => {
                    files_skipped += 1;
                    false
                }
                _ => true,
            }
        })
        .collect();

    // ── Phase 2: read + chunk + embed in parallel (Rayon) ──────────────────
    // Each element: (filepath, room, chunks_with_embeddings)
    let file_count_total = files.len();
    type ChunkEntry = (String, usize, Vec<f32>); // (text, chunk_index, embedding)
    let prepared: Vec<(std::path::PathBuf, String, Vec<ChunkEntry>)> = pending
        .par_iter()
        .filter_map(|filepath| {
            let content = std::fs::read_to_string(filepath).ok()?;
            let content = content.trim().to_string();
            if content.len() < MIN_CHUNK_SIZE {
                return None;
            }
            let room = detect_room(filepath, &content, &rooms, &project_path);
            let chunks = chunk_text(&content);
            if chunks.is_empty() {
                return None;
            }
            let chunk_texts: Vec<&str> = chunks.iter().map(|(t, _)| t.as_str()).collect();
            let embeddings = crate::embedder::embed_batch(&chunk_texts).unwrap_or_default();
            let chunk_entries: Vec<ChunkEntry> = chunks
                .into_iter()
                .enumerate()
                .map(|(idx, (text, ci))| {
                    let emb = embeddings.get(idx).cloned().unwrap_or_default();
                    (text, ci, emb)
                })
                .collect();
            Some(((*filepath).clone(), room, chunk_entries))
        })
        .collect();

    // ── Phase 3: write to DB (serial, SQLite requirement) ──────────────────
    for (i, (filepath, room, chunk_entries)) in prepared.iter().enumerate() {
        let source_file = filepath.to_string_lossy().to_string();

        if dry_run {
            println!(
                "    [DRY RUN] {} → room:{room} ({} drawers)",
                filepath.file_name().unwrap_or_default().to_string_lossy(),
                chunk_entries.len()
            );
            total_drawers += chunk_entries.len();
            *room_counts.entry(room.clone()).or_default() += 1;
            continue;
        }

        let mut drawers_added = 0usize;
        for (chunk_text, chunk_index, emb) in chunk_entries {
            let embedding = if emb.is_empty() {
                None
            } else {
                Some(emb.as_slice())
            };
            let (added, _) = add_drawer(
                conn,
                &wing,
                room,
                chunk_text,
                embedding,
                &source_file,
                *chunk_index,
                agent,
                3.0,
            )?;
            if added {
                drawers_added += 1;
            }
        }

        if drawers_added > 0 {
            *room_counts.entry(room.clone()).or_default() += 1;
            total_drawers += drawers_added;
            println!(
                "  ✓ [{:4}/{}] {:50} +{drawers_added}",
                i + 1,
                file_count_total,
                filepath.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }

    println!("\n{}", "=".repeat(55));
    println!("  Done.");
    println!("  Files processed: {}", files.len() - files_skipped);
    println!("  Files skipped (already filed): {files_skipped}");
    println!("  Drawers filed: {total_drawers}");
    println!("\n  By room:");
    let mut sorted: Vec<(&String, &usize)> = room_counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (room, count) in sorted {
        println!("    {:20} {count} files", room);
    }
    println!();
    println!("{}", "=".repeat(55));

    Ok(())
}

/// Re-embed all drawers that are missing embeddings.
pub fn repair(conn: &mut Connection) -> Result<()> {
    let unembedded = crate::store::fetch_unembedded(conn)?;
    println!(
        "  Repairing {} drawers missing embeddings...",
        unembedded.len()
    );

    for (id, content) in &unembedded {
        if let Ok(vec) = crate::embedder::embed_one(content) {
            crate::store::update_embedding(conn, id, &vec)?;
        }
    }
    println!("  Repair complete.");
    Ok(())
}

/// Print palace status dashboard.
pub fn status(conn: &Connection, palace_path: &Path) -> Result<()> {
    let total = crate::store::count_drawers(conn)?;
    let wings = crate::store::wing_counts(conn)?;

    let db_size = palace_path
        .metadata()
        .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
        .unwrap_or_else(|_| "unknown".to_string());

    let unembedded = crate::store::count_unembedded(conn).unwrap_or(0);

    println!();
    println!("  ╔══════════════════════════════════════════════════════╗");
    println!("  ║  Palace Status Dashboard                             ║");
    println!("  ╠══════════════════════════════════════════════════════╣");
    println!(
        "  ║  Database : {:<42}║",
        palace_path
            .display()
            .to_string()
            .chars()
            .take(42)
            .collect::<String>()
    );
    println!("  ║  Size     : {db_size:<42}║");
    println!("  ║  Drawers  : {total:<42}║");
    if unembedded > 0 {
        println!("  ║  ⚠ Missing embeddings: {unembedded:<34}║");
    }
    println!("  ╠══════════════════════════════════════════════════════╣");

    let mut sorted_wings: Vec<(&String, &i64)> = wings.iter().collect();
    sorted_wings.sort_by(|a, b| b.1.cmp(a.1));

    for (wing, wing_count) in &sorted_wings {
        println!("  ║                                                      ║");
        println!(
            "  ║  WING  {:<45}║",
            format!("{wing} ({wing_count} drawers)")
        );
        let rooms = crate::store::room_counts(conn, Some(wing))?;
        let mut sorted_rooms: Vec<(&String, &i64)> = rooms.iter().collect();
        sorted_rooms.sort_by(|a, b| b.1.cmp(a.1));
        for (room, count) in sorted_rooms.iter().take(10) {
            println!("  ║    {:<22} {:>5} drawers                 ║", room, count);
        }
        if sorted_rooms.len() > 10 {
            println!(
                "  ║    … and {} more rooms                               ║",
                sorted_rooms.len() - 10
            );
        }
    }

    println!("  ║                                                      ║");
    println!("  ╚══════════════════════════════════════════════════════╝");

    if total == 0 {
        println!();
        println!("  No drawers yet. Get started:");
        println!("    palace init <project-dir>");
        println!("    palace mine <project-dir>");
    }
    println!();
    Ok(())
}
