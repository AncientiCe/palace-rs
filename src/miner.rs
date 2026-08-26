//! Project file ingestor.
//!
//! Reads palace.yaml, walks the project with gitignore respect (via `ignore`
//! crate), chunks text (~800 chars, 100 overlap), routes to rooms, embeds
//! and stores drawers.
//!
//! File reading and embedding are parallelised with Rayon; SQLite writes remain
//! single-threaded (rusqlite Connection is not Send).
//!
//! `mine()` processes candidate files in windows of `MINE_BATCH_SIZE` rather
//! than materializing the whole repo (read → chunk → embed → write) before
//! writing anything. This bounds peak memory to one window's worth of data
//! and gets the first commit on disk in seconds instead of minutes, so a
//! killed/timed-out run keeps whatever it already committed instead of
//! losing everything.

use anyhow::{Context, Result};
use ignore::{DirEntry, WalkBuilder};
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::room_detector::{load_config, Room};
use crate::store::{
    content_hash, delete_drawers_for_file, mined_files_for_wing, replace_file_drawers,
};

/// Whether a project directory has been mined into the palace yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProjectWingStatus {
    /// The wing exists with content (or a recorded mine timestamp).
    Mined {
        wing: String,
        drawers: i64,
        last_mined_at: Option<String>,
    },
    /// The wing is declared in the registry but holds no drawers yet.
    RegisteredNotMined { wing: String, drawers: i64 },
    /// No registry entry and no drawers — the project is unknown to the palace.
    Unknown {
        suggested_wing: String,
        has_palace_yaml: bool,
    },
}

/// Slugify a project directory name into a wing name (matches `palace init`).
pub fn wing_slug_from_dir(dir: &Path) -> String {
    dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
        .replace([' ', '-'], "_")
}

/// Determine whether `project_dir` has been mined, using the wings registry.
///
/// Resolves the wing name from `palace.yaml` when present, otherwise from the
/// directory slug. Looks the wing up by canonical path first, then by name.
pub fn project_wing_status(conn: &Connection, project_dir: &Path) -> Result<ProjectWingStatus> {
    let canonical = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let has_palace_yaml = canonical.join("palace.yaml").exists();
    let wing = if has_palace_yaml {
        load_config(&canonical)
            .map(|c| c.wing)
            .unwrap_or_else(|_| wing_slug_from_dir(&canonical))
    } else {
        wing_slug_from_dir(&canonical)
    };
    let canonical_str = canonical.to_string_lossy().to_string();

    let record = match crate::store::find_wing_by_path(conn, &canonical_str)? {
        Some(r) => Some(r),
        None => crate::store::get_wing(conn, &wing)?,
    };

    if let Some(r) = record {
        let drawers = r.drawer_count;
        if r.last_mined_at.is_some() || drawers > 0 {
            return Ok(ProjectWingStatus::Mined {
                wing: r.name,
                drawers,
                last_mined_at: r.last_mined_at,
            });
        }
        return Ok(ProjectWingStatus::RegisteredNotMined {
            wing: r.name,
            drawers,
        });
    }

    Ok(ProjectWingStatus::Unknown {
        suggested_wing: wing,
        has_palace_yaml,
    })
}

pub const CHUNK_SIZE: usize = 800;
pub const CHUNK_OVERLAP: usize = 100;
pub const MIN_CHUNK_SIZE: usize = 50;

/// How many files' worth of drawers to commit per SQLite transaction during
/// mining, and (since the windowed rewrite) how many candidate files are
/// read/chunked/embedded/written together as one unit before moving to the
/// next window. Batching bounds both ends of the tradeoff: a single
/// transaction for the whole run would be fastest but would lose all
/// progress if the process is interrupted (e.g. an MCP client timing out
/// mid-mine); one transaction per drawer (the old behaviour) is what made
/// mining stall on larger projects in the first place. A few hundred files
/// per window keeps commit count low while still checkpointing regularly and
/// bounding peak memory.
///
/// Shared with `convo_miner::mine_convos`, which has the same write pattern.
pub(crate) const MINE_BATCH_SIZE: usize = 200;

pub static READABLE_EXTENSIONS: &[&str] = &[
    "txt", "md", "py", "js", "ts", "jsx", "tsx", "json", "yaml", "yml", "html", "css", "java",
    "go", "rs", "rb", "sh", "csv", "sql", "toml",
];

pub static SKIP_FILENAMES: &[&str] = &[
    "palace.yaml",
    "palace.yml",
    ".gitignore",
    "package-lock.json",
];

/// Directories skipped outright during the walk, independent of
/// `.gitignore`. `.git` in particular can hold 100k-1M+ loose objects and
/// pack index entries on a long-lived repo, every one of which would
/// otherwise be stat'd and extension-checked only to be filtered out
/// afterward; the rest are generated/vendored trees that are never useful to
/// mine.
const DENYLISTED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "Pods",
    "DerivedData",
    "build",
    "dist",
    ".next",
    "target",
    "vendor",
    ".gradle",
    "coverage",
];

/// Files larger than this are skipped during the walk rather than read and
/// filtered out afterward — mined content is chunked prose/code, not the
/// kind of thing that comes in single files this large.
const MAX_WALK_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// A large-enough first-ever mine of a wing that it's worth paying to drop
/// and rebuild the (palace-wide) BM25 term index around the run rather than
/// maintaining it incrementally. Gated on "first-ever" (see `mine`'s use of
/// `known_hashes.is_empty()`) because the index covers every wing in the
/// palace, not just this one — rebuilding it on every ordinary incremental
/// re-mine of an already-large palace would be a net loss, not a win.
const BULK_MINE_THRESHOLD: usize = 1000;

fn is_denylisted_dir(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|ft| ft.is_dir())
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| DENYLISTED_DIRS.contains(&name))
}

/// Walk `project_path` for readable candidate files, in parallel, pruning
/// denylisted directories and oversized files before they're ever stat'd for
/// gitignore/extension filtering.
fn walk_project(
    project_path: &Path,
    respect_gitignore: bool,
    include_ignored: &[String],
) -> Vec<std::path::PathBuf> {
    let mut builder = WalkBuilder::new(project_path);
    builder
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .max_filesize(Some(MAX_WALK_FILE_SIZE))
        .filter_entry(|entry| !is_denylisted_dir(entry));

    // Force-include paths that are normally ignored.
    for path in include_ignored {
        builder.add(project_path.join(path));
    }

    let files: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.into_path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if SKIP_FILENAMES.contains(&name.as_ref()) {
                return ignore::WalkState::Continue;
            }
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if !READABLE_EXTENSIONS.contains(&ext.as_str()) {
                return ignore::WalkState::Continue;
            }
            files.lock().unwrap_or_else(|e| e.into_inner()).push(path);
            ignore::WalkState::Continue
        })
    });

    files.into_inner().unwrap_or_else(|e| e.into_inner())
}

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

/// How many chunks to embed per ONNX inference call. The embedder runs
/// behind a single mutex-guarded session (see `embedder::embed_batch`), so
/// per-file batches (often just 1-5 chunks) waste most of the session's
/// intra-op thread pool on tiny amounts of work. Grouping chunks across many
/// files into batches this size turns hundreds/thousands of tiny inference
/// calls into a much smaller number of efficient ones.
const EMBED_BATCH_SIZE: usize = 64;

/// Group per-file chunk counts into batches capped at `batch_size`, returning
/// `(file_index, chunk_index_within_file)` pairs in original file/chunk
/// order. Never drops, duplicates, or reorders an entry; a `batch_size` of 0
/// is treated as 1 rather than looping forever.
fn group_into_batches(chunk_counts: &[usize], batch_size: usize) -> Vec<Vec<(usize, usize)>> {
    let batch_size = batch_size.max(1);
    let mut batches = Vec::new();
    let mut current = Vec::new();
    for (file_index, &count) in chunk_counts.iter().enumerate() {
        for chunk_index in 0..count {
            current.push((file_index, chunk_index));
            if current.len() == batch_size {
                batches.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
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

/// Outcome summary for a single `mine()` run.
///
/// `mine` is a sync, not a one-shot ingest: re-running it against the same
/// project picks up edited files (`updated_files`) and drops drawers for
/// files that no longer exist on disk (`removed_files`), instead of silently
/// skipping every file that already has drawers forever.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MineSummary {
    pub new_files: usize,
    pub updated_files: usize,
    pub unchanged_files: usize,
    pub removed_files: usize,
    pub drawers_added: usize,
    pub drawers_removed: usize,
}

/// Whether a file being (re-)mined is new to the palace or was seen before
/// under a different content hash.
#[derive(Debug, Clone, Copy)]
enum FileChange {
    New,
    Updated,
}

struct PendingFile {
    filepath: std::path::PathBuf,
    content: String,
    hash: String,
    change: FileChange,
}

struct ChunkedFile {
    filepath: std::path::PathBuf,
    hash: String,
    change: FileChange,
    room: String,
    chunks: Vec<(String, usize)>,
}

struct PreparedFile {
    filepath: std::path::PathBuf,
    hash: String,
    change: FileChange,
    room: String,
    chunk_entries: Vec<(String, usize, Vec<f32>)>, // (text, chunk_index, embedding)
}

/// A phase `mine()` is currently in, reported through `on_progress` so a
/// long-running mine looks like it's working the whole time instead of only
/// during the final write. `label()` is what gets sent to MCP clients as the
/// `notifications/progress` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinePhase {
    /// Waiting on the embedding model to finish loading — only reported if a
    /// mine call arrives before the background warm-up started at MCP server
    /// startup (`embedder::warm_up_in_background`) has completed.
    LoadingModel,
    Walking,
    Hashing,
    Chunking,
    Embedding,
    Writing,
    Pruning,
}

impl MinePhase {
    pub fn label(&self) -> &'static str {
        match self {
            MinePhase::LoadingModel => "loading_model",
            MinePhase::Walking => "walking",
            MinePhase::Hashing => "hashing",
            MinePhase::Chunking => "chunking",
            MinePhase::Embedding => "embedding",
            MinePhase::Writing => "writing",
            MinePhase::Pruning => "pruning",
        }
    }
}

/// Per-phase timing/counts, enabled by setting `PALACE_MINE_TRACE=1`. Exists
/// to turn a hypothesis about where a mine spends its time into a measured
/// one — see the phased performance plan this implements.
struct MineTrace {
    enabled: bool,
    walk: Duration,
    hash: Duration,
    chunk: Duration,
    embed: Duration,
    write: Duration,
    prune: Duration,
    chunks_total: usize,
}

impl MineTrace {
    fn new() -> Self {
        Self {
            enabled: std::env::var("PALACE_MINE_TRACE").as_deref() == Ok("1"),
            walk: Duration::ZERO,
            hash: Duration::ZERO,
            chunk: Duration::ZERO,
            embed: Duration::ZERO,
            write: Duration::ZERO,
            prune: Duration::ZERO,
            chunks_total: 0,
        }
    }

    fn print(&self, summary: &MineSummary, files_walked: usize) {
        if !self.enabled {
            return;
        }
        println!("\n  PALACE_MINE_TRACE — phase breakdown");
        println!("    walk    {:>10.2?}", self.walk);
        println!("    hash    {:>10.2?}", self.hash);
        println!("    chunk   {:>10.2?}", self.chunk);
        println!("    embed   {:>10.2?}", self.embed);
        println!("    write   {:>10.2?}", self.write);
        println!("    prune   {:>10.2?}", self.prune);
        println!(
            "    files walked={files_walked} new={} updated={} unchanged={} chunks={}",
            summary.new_files, summary.updated_files, summary.unchanged_files, self.chunks_total
        );
    }
}

/// Mine a project directory into the palace, syncing it with what's on disk.
///
/// Unlike a one-shot ingest, re-running `mine` against the same project:
/// - re-mines files whose content hash changed since the last run, replacing
///   their old drawers with fresh ones (so edits are actually picked up);
/// - leaves files with an unchanged hash untouched;
/// - removes drawers for previously-mined files that no longer exist on disk.
///
/// Candidates are processed in windows of `MINE_BATCH_SIZE` files, each
/// carried all the way through read → chunk → embed → write → commit before
/// the next window starts, rather than materializing the whole repo before
/// writing anything. Combined with `limit` now bounding *processed* (new or
/// updated) files rather than raw walk order, a `--limit`-ed call is
/// naturally resumable: whatever got committed this call is reflected in
/// `mined_files`, so the next call's hash check skips straight past it.
///
/// `on_progress`, when given, is invoked as `(phase, done, total)` from every
/// phase — including the walk and, if the embedder hasn't finished loading
/// yet, a distinct `LoadingModel` phase — not just while writing. This lets
/// long-running callers (notably the MCP server, which otherwise gives zero
/// feedback for the whole duration of a mine) report visible progress
/// instead of looking stuck.
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
    quiet: bool,
    mut on_progress: Option<&mut dyn FnMut(MinePhase, usize, usize)>,
) -> Result<MineSummary> {
    let mut trace = MineTrace::new();
    let project_path = project_dir
        .canonicalize()
        .context("resolving project dir")?;
    let config = load_config(&project_path)?;
    let wing = wing_override.unwrap_or(&config.wing).to_string();
    let rooms = config.rooms;

    // ── Walk: parallel, deny-listed, size-capped ────────────────────────────
    let walk_start = Instant::now();
    let files = walk_project(&project_path, respect_gitignore, include_ignored);
    trace.walk += walk_start.elapsed();
    if let Some(cb) = on_progress.as_mut() {
        cb(MinePhase::Walking, files.len(), files.len());
    }

    if !quiet {
        println!("\n{}", "=".repeat(55));
        println!("  Palace Mine");
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
    }

    let mut summary = MineSummary::default();
    let mut room_counts: HashMap<String, usize> = HashMap::new();
    let total_candidates = files.len();
    let mut processed_candidates = 0usize;
    // Toward `limit`: only new/updated files count, so unchanged files never
    // eat into the budget and a `--limit`-ed call always makes forward
    // progress on files that actually need work.
    let mut changed_processed = 0usize;

    // ── Bulk-load previously-mined hashes for this wing once ────────────────
    let mut known_hashes = crate::store::mined_file_hashes_for_wing(conn, &wing)?;
    let is_bulk_first_mine =
        !dry_run && known_hashes.is_empty() && total_candidates > BULK_MINE_THRESHOLD;

    crate::store::apply_mine_pragmas(conn)?;
    if is_bulk_first_mine {
        crate::store::drop_bm25_term_index(conn)?;
    }

    'windows: for window in files.chunks(MINE_BATCH_SIZE) {
        if limit > 0 && changed_processed >= limit {
            break 'windows;
        }

        // ── Hashing: read + blake3 in parallel, then classify against the
        // bulk-loaded hash map (no per-file DB query). ───────────────────────
        let hash_start = Instant::now();
        let read: Vec<(std::path::PathBuf, String, String)> = window
            .par_iter()
            .filter_map(|filepath| {
                let content = std::fs::read_to_string(filepath).ok()?;
                let content = content.trim().to_string();
                let hash = content_hash(&content);
                Some((filepath.clone(), content, hash))
            })
            .collect();

        let mut pending: Vec<PendingFile> = Vec::new();
        for (filepath, content, hash) in read {
            let source = filepath.to_string_lossy().to_string();
            let previous_hash = known_hashes.get(&source).cloned();
            if previous_hash.as_deref() == Some(hash.as_str()) {
                summary.unchanged_files += 1;
                continue;
            }
            let change = if previous_hash.is_some() {
                FileChange::Updated
            } else {
                FileChange::New
            };
            pending.push(PendingFile {
                filepath,
                content,
                hash,
                change,
            });
        }
        trace.hash += hash_start.elapsed();
        processed_candidates += window.len();
        if let Some(cb) = on_progress.as_mut() {
            cb(MinePhase::Hashing, processed_candidates, total_candidates);
        }

        // Bound work to `limit` at file granularity, not just between
        // windows — a repo whose changed-file count fits in a single
        // `MINE_BATCH_SIZE` window would otherwise ignore `limit` entirely.
        if limit > 0 {
            let remaining = limit.saturating_sub(changed_processed);
            pending.truncate(remaining);
        }

        if pending.is_empty() {
            continue;
        }

        // ── Chunking (Rayon). No embedding here: the ONNX session is a single
        // mutex-guarded resource, so per-file parallel embedding just queues
        // everyone behind that mutex anyway. Files whose content shrank below
        // the minimum chunk size still go through with an empty chunk list
        // rather than being dropped, so the write phase clears their now-stale
        // drawers instead of leaving orphans behind. ───────────────────────
        let chunk_start = Instant::now();
        let chunked: Vec<ChunkedFile> = pending
            .par_iter()
            .map(|p| {
                let room = detect_room(&p.filepath, &p.content, &rooms, &project_path);
                let chunks = chunk_text(&p.content);
                ChunkedFile {
                    filepath: p.filepath.clone(),
                    hash: p.hash.clone(),
                    change: p.change,
                    room,
                    chunks,
                }
            })
            .collect();
        trace.chunk += chunk_start.elapsed();
        trace.chunks_total += chunked.iter().map(|f| f.chunks.len()).sum::<usize>();
        if let Some(cb) = on_progress.as_mut() {
            cb(MinePhase::Chunking, processed_candidates, total_candidates);
        }

        // ── Embedding, in capped cross-file batches — skipped entirely under
        // dry_run, which only ever needs chunk counts, not actual vectors. ──
        let embed_start = Instant::now();
        let chunk_counts: Vec<usize> = chunked.iter().map(|f| f.chunks.len()).collect();
        let mut embeddings_by_file: Vec<Vec<Vec<f32>>> = chunk_counts
            .iter()
            .map(|&count| vec![Vec::new(); count])
            .collect();

        if !dry_run {
            if !crate::embedder::is_ready() {
                if let Some(cb) = on_progress.as_mut() {
                    cb(MinePhase::LoadingModel, 0, 1);
                }
            }
            for batch in group_into_batches(&chunk_counts, EMBED_BATCH_SIZE) {
                let texts: Vec<&str> = batch
                    .iter()
                    .map(|&(file_index, chunk_index)| {
                        chunked[file_index].chunks[chunk_index].0.as_str()
                    })
                    .collect();
                let embeddings = crate::embedder::embed_batch(&texts).unwrap_or_default();
                for (position, &(file_index, chunk_index)) in batch.iter().enumerate() {
                    if let Some(embedding) = embeddings.get(position) {
                        embeddings_by_file[file_index][chunk_index] = embedding.clone();
                    }
                }
            }
        }
        trace.embed += embed_start.elapsed();
        if let Some(cb) = on_progress.as_mut() {
            cb(MinePhase::Embedding, processed_candidates, total_candidates);
        }

        let prepared: Vec<PreparedFile> = chunked
            .into_iter()
            .zip(embeddings_by_file)
            .map(|(f, embeddings)| {
                let chunk_entries = f
                    .chunks
                    .into_iter()
                    .zip(embeddings)
                    .map(|((text, chunk_index), embedding)| (text, chunk_index, embedding))
                    .collect();
                PreparedFile {
                    filepath: f.filepath,
                    hash: f.hash,
                    change: f.change,
                    room: f.room,
                    chunk_entries,
                }
            })
            .collect();

        // ── Writing: one transaction per window. ────────────────────────────
        let write_start = Instant::now();
        if dry_run {
            for file in &prepared {
                match file.change {
                    FileChange::New => summary.new_files += 1,
                    FileChange::Updated => summary.updated_files += 1,
                }
                if !file.chunk_entries.is_empty() {
                    if !quiet {
                        println!(
                            "    [DRY RUN] {} → room:{} ({} drawers)",
                            file.filepath
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                            file.room,
                            file.chunk_entries.len()
                        );
                    }
                    summary.drawers_added += file.chunk_entries.len();
                    *room_counts.entry(file.room.clone()).or_default() += 1;
                }
                changed_processed += 1;
            }
        } else {
            let tx = conn.transaction().context("starting mine transaction")?;
            for file in &prepared {
                let source_file = file.filepath.to_string_lossy().to_string();

                let drawers_added = replace_file_drawers(
                    &tx,
                    &wing,
                    &file.room,
                    &source_file,
                    &file.hash,
                    &file.chunk_entries,
                    agent,
                    3.0,
                )?;
                known_hashes.insert(source_file, file.hash.clone());

                match file.change {
                    FileChange::New => summary.new_files += 1,
                    FileChange::Updated => summary.updated_files += 1,
                }
                summary.drawers_added += drawers_added;
                changed_processed += 1;
                if drawers_added > 0 {
                    *room_counts.entry(file.room.clone()).or_default() += 1;
                    if !quiet {
                        println!(
                            "  ✓ [{changed_processed:4}] {:50} +{drawers_added}",
                            file.filepath
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        );
                    }
                }
                if let Some(cb) = on_progress.as_mut() {
                    cb(
                        MinePhase::Writing,
                        changed_processed,
                        total_candidates.max(1),
                    );
                }
            }
            tx.commit().context("committing mine batch")?;
        }
        trace.write += write_start.elapsed();
    }

    if is_bulk_first_mine {
        crate::store::rebuild_bm25_term_index(conn)?;
    }

    if !dry_run {
        crate::store::set_wing_mined(conn, &wing, &project_path.to_string_lossy())
            .context("recording wing mine status")?;
    }

    // ── Prune drawers for files that no longer exist on disk ────────────────
    // Scoped to files still on disk, not to this run's (possibly limited or
    // gitignore-filtered) walk, so `--limit` or a widened `.gitignore` can
    // never be mistaken for mass deletion.
    let prune_start = Instant::now();
    let stale_sources = mined_files_for_wing(conn, &wing)?
        .into_iter()
        .filter(|source| !Path::new(source).exists())
        .collect::<Vec<_>>();
    if dry_run {
        for source in &stale_sources {
            summary.removed_files += 1;
            if !quiet {
                println!("    [DRY RUN] {source} → would remove (file no longer exists)");
            }
        }
    } else if !stale_sources.is_empty() {
        let tx = conn.transaction().context("starting prune transaction")?;
        for source in &stale_sources {
            let removed = delete_drawers_for_file(&tx, source)?;
            summary.removed_files += 1;
            summary.drawers_removed += removed;
            if !quiet {
                println!("  ✗ removed {source} ({removed} drawer(s), file no longer exists)");
            }
        }
        tx.commit().context("committing prune batch")?;
    }
    trace.prune += prune_start.elapsed();
    if let Some(cb) = on_progress.as_mut() {
        cb(
            MinePhase::Pruning,
            summary.removed_files,
            summary.removed_files.max(1),
        );
    }

    trace.print(&summary, total_candidates);

    if !quiet {
        println!("\n{}", "=".repeat(55));
        println!("  Done.");
        println!("  Files scanned:   {}", files.len());
        println!("  New:             {}", summary.new_files);
        println!("  Updated:         {}", summary.updated_files);
        println!("  Unchanged:       {}", summary.unchanged_files);
        println!("  Removed:         {}", summary.removed_files);
        println!("  Drawers filed:   {}", summary.drawers_added);
        if summary.drawers_removed > 0 {
            println!("  Drawers removed: {}", summary.drawers_removed);
        }
        println!("\n  By room:");
        let mut sorted: Vec<(&String, &usize)> = room_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (room, count) in sorted {
            println!("    {:20} {count} files", room);
        }
        println!();
        println!("{}", "=".repeat(55));
    }

    Ok(summary)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_into_batches_caps_size_without_dropping_or_reordering() {
        let counts = [3, 5, 2];
        let batches = group_into_batches(&counts, 4);
        assert_eq!(
            batches,
            vec![
                vec![(0, 0), (0, 1), (0, 2), (1, 0)],
                vec![(1, 1), (1, 2), (1, 3), (1, 4)],
                vec![(2, 0), (2, 1)],
            ]
        );
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, counts.iter().sum::<usize>());
    }

    #[test]
    fn group_into_batches_skips_files_with_no_chunks() {
        let counts = [0, 3, 0, 2];
        let batches = group_into_batches(&counts, 10);
        assert_eq!(batches, vec![vec![(1, 0), (1, 1), (1, 2), (3, 0), (3, 1)]]);
    }

    #[test]
    fn group_into_batches_single_batch_when_everything_fits() {
        let counts = [2, 2];
        let batches = group_into_batches(&counts, 100);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 4);
    }

    #[test]
    fn group_into_batches_handles_degenerate_batch_size_without_panicking() {
        let counts = [2, 1];
        let batches = group_into_batches(&counts, 0);
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(
            total, 3,
            "every chunk must still be processed even with a zero batch size"
        );
    }

    #[test]
    fn group_into_batches_empty_input_produces_no_batches() {
        assert!(group_into_batches(&[], 10).is_empty());
    }
}
