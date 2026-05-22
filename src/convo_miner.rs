//! Conversation file ingestor.
//!
//! Scans a directory for conversation files (.txt, .md, .json, .jsonl),
//! normalizes to transcript format, chunks by exchange pairs or general extraction,
//! embeds and stores drawers. Port of convo_miner.py.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

use crate::general_extractor::extract_memories;
use crate::normalize::normalize_file;
use crate::store::{add_drawer, file_already_mined};

const CONVO_EXTENSIONS: &[&str] = &["txt", "md", "json", "jsonl"];
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    ".palace",
    "tool-results",
    "memory",
];
const MIN_CHUNK_SIZE: usize = 30;

static TOPIC_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "technical",
        &[
            "code", "python", "function", "bug", "error", "api", "database", "server", "deploy",
            "git", "test", "debug", "refactor",
        ],
    ),
    (
        "architecture",
        &[
            "architecture",
            "design",
            "pattern",
            "structure",
            "schema",
            "interface",
            "module",
            "component",
            "service",
            "layer",
        ],
    ),
    (
        "planning",
        &[
            "plan",
            "roadmap",
            "milestone",
            "deadline",
            "priority",
            "sprint",
            "backlog",
            "scope",
            "requirement",
            "spec",
        ],
    ),
    (
        "decisions",
        &[
            "decided",
            "chose",
            "picked",
            "switched",
            "migrated",
            "replaced",
            "trade-off",
            "alternative",
            "option",
            "approach",
        ],
    ),
    (
        "problems",
        &[
            "problem",
            "issue",
            "broken",
            "failed",
            "crash",
            "stuck",
            "workaround",
            "fix",
            "solved",
            "resolved",
        ],
    ),
];

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractMode {
    Exchange,
    General,
}

/// Chunk content by exchange pairs: one > turn + AI response = one chunk.
pub fn chunk_exchanges(content: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let quote_count = lines.iter().filter(|l| l.trim().starts_with('>')).count();

    if quote_count >= 3 {
        chunk_by_exchange(&lines)
    } else {
        chunk_by_paragraph(content)
    }
}

fn chunk_by_exchange(lines: &[&str]) -> Vec<(String, usize)> {
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().starts_with('>') {
            let user_turn = line.trim().to_string();
            i += 1;

            let mut ai_lines = Vec::new();
            while i < lines.len() {
                let next = lines[i].trim();
                if next.starts_with('>') || next.starts_with("---") {
                    break;
                }
                if !next.is_empty() {
                    ai_lines.push(next);
                }
                i += 1;
                if ai_lines.len() >= 8 {
                    break;
                }
            }

            let ai_response = ai_lines.join(" ");
            let content = if ai_response.is_empty() {
                user_turn
            } else {
                format!("{user_turn}\n{ai_response}")
            };

            if content.trim().len() > MIN_CHUNK_SIZE {
                chunks.push((content, chunks.len()));
            }
        } else {
            i += 1;
        }
    }

    chunks
}

fn chunk_by_paragraph(content: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let paragraphs: Vec<&str> = content
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    if paragraphs.len() <= 1 && lines.len() > 20 {
        return lines
            .chunks(25)
            .enumerate()
            .filter_map(|(i, chunk)| {
                let group = chunk.join("\n").trim().to_string();
                if group.len() > MIN_CHUNK_SIZE {
                    Some((group, i))
                } else {
                    None
                }
            })
            .collect();
    }

    paragraphs
        .into_iter()
        .enumerate()
        .filter(|(_, p)| p.len() > MIN_CHUNK_SIZE)
        .map(|(i, p)| (p.to_string(), i))
        .collect()
}

fn detect_convo_room(content: &str) -> String {
    let content_lower = content
        .get(..3000.min(content.len()))
        .unwrap_or(content)
        .to_lowercase();
    let mut scores: HashMap<&str, usize> = HashMap::new();
    for (room, keywords) in TOPIC_KEYWORDS {
        let score: usize = keywords
            .iter()
            .filter(|kw| content_lower.contains(**kw))
            .count();
        if score > 0 {
            scores.insert(room, score);
        }
    }
    scores
        .into_iter()
        .max_by_key(|(_, v)| *v)
        .map(|(k, _)| k.to_string())
        .unwrap_or_else(|| "general".to_string())
}

fn scan_convos(convo_dir: &Path) -> Vec<std::path::PathBuf> {
    let skip: std::collections::HashSet<&str> = SKIP_DIRS.iter().cloned().collect();
    let mut files = Vec::new();

    fn walk(
        dir: &Path,
        skip: &std::collections::HashSet<&str>,
        files: &mut Vec<std::path::PathBuf>,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if path.is_dir() {
                    if !skip.contains(name.as_str()) {
                        walk(&path, skip, files);
                    }
                } else {
                    if name.ends_with(".meta.json") {
                        continue;
                    }
                    let ext = path
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();
                    if CONVO_EXTENSIONS.contains(&ext.as_str()) {
                        files.push(path);
                    }
                }
            }
        }
    }

    walk(convo_dir, &skip, &mut files);
    files
}

/// Mine a directory of conversation files into the palace.
pub fn mine_convos(
    conn: &mut Connection,
    convo_dir: &Path,
    wing: Option<&str>,
    agent: &str,
    limit: usize,
    dry_run: bool,
    extract_mode: ExtractMode,
) -> Result<()> {
    let convo_path = convo_dir.canonicalize().context("resolving convo dir")?;
    let wing = wing
        .unwrap_or_else(|| {
            convo_path
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or("conversations")
        })
        .to_string();
    let wing = wing.to_lowercase().replace([' ', '-'], "_");

    let mut files = scan_convos(&convo_path);
    if limit > 0 {
        files.truncate(limit);
    }

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Mine — Conversations");
    println!("{}", "=".repeat(55));
    println!("  Wing:    {wing}");
    println!("  Source:  {}", convo_path.display());
    println!("  Files:   {}", files.len());
    if dry_run {
        println!("  DRY RUN — nothing will be filed");
    }
    println!("{}\n", "-".repeat(55));

    let mut total_drawers = 0usize;
    let mut files_skipped = 0usize;
    let mut room_counts: HashMap<String, usize> = HashMap::new();

    for (i, filepath) in files.iter().enumerate() {
        let source_file = filepath.to_string_lossy().to_string();

        if !dry_run && file_already_mined(conn, &source_file)? {
            files_skipped += 1;
            continue;
        }

        let content = match normalize_file(filepath) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if content.trim().len() < MIN_CHUNK_SIZE {
            continue;
        }

        let chunks_with_rooms: Vec<(String, String, usize)> = match extract_mode {
            ExtractMode::General => {
                let memories = extract_memories(&content, 0.3);
                memories
                    .into_iter()
                    .map(|m| (m.content, m.memory_type, m.chunk_index))
                    .collect()
            }
            ExtractMode::Exchange => {
                let room = detect_convo_room(&content);
                chunk_exchanges(&content)
                    .into_iter()
                    .map(|(c, idx)| (c, room.clone(), idx))
                    .collect()
            }
        };

        if chunks_with_rooms.is_empty() {
            continue;
        }

        if dry_run {
            println!(
                "    [DRY RUN] {} → {} chunks",
                filepath.file_name().unwrap_or_default().to_string_lossy(),
                chunks_with_rooms.len()
            );
            total_drawers += chunks_with_rooms.len();
            for (_, room, _) in &chunks_with_rooms {
                *room_counts.entry(room.clone()).or_default() += 1;
            }
            continue;
        }

        let mut drawers_added = 0usize;
        let chunk_texts: Vec<&str> = chunks_with_rooms
            .iter()
            .map(|(t, _, _)| t.as_str())
            .collect();
        let embeddings = crate::embedder::embed_batch(&chunk_texts).unwrap_or_default();
        for (idx, (chunk_text, chunk_room, chunk_index)) in chunks_with_rooms.iter().enumerate() {
            let embedding = embeddings.get(idx).map(|e| e.as_slice());
            let (added, _) = add_drawer(
                conn,
                &wing,
                chunk_room,
                chunk_text,
                embedding,
                &source_file,
                *chunk_index,
                agent,
                3.0,
            )?;
            if added {
                drawers_added += 1;
                *room_counts.entry(chunk_room.clone()).or_default() += 1;
            }
        }

        total_drawers += drawers_added;
        println!(
            "  ✓ [{:4}/{}] {:50} +{drawers_added}",
            i + 1,
            files.len(),
            filepath.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    println!("\n{}", "=".repeat(55));
    println!("  Done.");
    println!("  Files processed: {}", files.len() - files_skipped);
    println!("  Files skipped (already filed): {files_skipped}");
    println!("  Drawers filed: {total_drawers}");
    if !room_counts.is_empty() {
        println!("\n  By room:");
        let mut sorted: Vec<(&String, &usize)> = room_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (room, count) in sorted {
            println!("    {:20} {count} files", room);
        }
    }
    println!("{}\n", "=".repeat(55));
    Ok(())
}
