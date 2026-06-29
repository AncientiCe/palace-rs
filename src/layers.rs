//! 4-Layer Memory Stack.
//!
//! Layer 0: Identity (~100 tokens) — Always loaded. ~/.palace/identity.txt
//! Layer 1: Essential Story (~500-800) — Top moments from the palace, by importance.
//! Layer 2: On-Demand (~200-500 each) — Wing/room filtered retrieval.
//! Layer 3: Deep Search (unlimited) — Full semantic search.
//!
//! Port of layers.py.

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::store::{list_by_importance, list_drawers, DrawerFilter};

const L1_MAX_DRAWERS: usize = 15;
const L1_MAX_CHARS: usize = 3200;

// ── Layer 0 ───────────────────────────────────────────────────────────────

pub struct Layer0 {
    path: PathBuf,
    cached: Option<String>,
}

impl Layer0 {
    pub fn new(identity_path: &Path) -> Self {
        Self {
            path: identity_path.to_path_buf(),
            cached: None,
        }
    }

    pub fn render(&mut self) -> String {
        if let Some(ref t) = self.cached {
            return t.clone();
        }
        let text = if self.path.exists() {
            std::fs::read_to_string(&self.path)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| default_identity())
        } else {
            default_identity()
        };
        self.cached = Some(text.clone());
        text
    }

    pub fn token_estimate(&mut self) -> usize {
        self.render().len() / 4
    }
}

fn default_identity() -> String {
    "## L0 — IDENTITY\nNo identity configured. Create ~/.palace/identity.txt".to_string()
}

// ── Layer 1 ───────────────────────────────────────────────────────────────

pub struct Layer1 {
    #[allow(dead_code)]
    conn_path: PathBuf,
    wing: Option<String>,
}

impl Layer1 {
    pub fn new(db_path: &Path, wing: Option<String>) -> Self {
        Self {
            conn_path: db_path.to_path_buf(),
            wing,
        }
    }

    pub fn generate(&self, conn: &Connection) -> String {
        let drawers = match list_by_importance(conn, 200) {
            Ok(d) => d,
            Err(_) => return "## L1 — No palace found. Run: palace mine <dir>".to_string(),
        };

        if drawers.is_empty() {
            return "## L1 — No memories yet.".to_string();
        }

        // Filter by wing if specified, take top N
        let filtered: Vec<_> = drawers
            .iter()
            .filter(|d| {
                if let Some(w) = &self.wing {
                    d.wing == *w
                } else {
                    true
                }
            })
            .take(L1_MAX_DRAWERS)
            .collect();

        // Group by room
        let mut by_room: HashMap<String, Vec<&crate::store::Drawer>> = HashMap::new();
        for d in &filtered {
            by_room.entry(d.room.clone()).or_default().push(d);
        }

        let mut lines = vec!["## L1 — ESSENTIAL STORY".to_string()];
        let mut total_len = lines[0].len();

        let mut rooms_sorted: Vec<&String> = by_room.keys().collect();
        rooms_sorted.sort();

        'outer: for room in rooms_sorted {
            let room_line = format!("\n[{room}]");
            lines.push(room_line.clone());
            total_len += room_line.len();

            for drawer in &by_room[room] {
                let source = Path::new(&drawer.source_file)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                let snippet = drawer.content.trim().replace('\n', " ");
                let snippet = if snippet.chars().count() > 200 {
                    let truncated: String = snippet.chars().take(197).collect();
                    format!("{truncated}...")
                } else {
                    snippet
                };

                let mut entry = format!("  - {snippet}");
                if !source.is_empty() {
                    entry.push_str(&format!("  ({source})"));
                }

                if total_len + entry.len() > L1_MAX_CHARS {
                    lines.push("  ... (more in L3 search)".to_string());
                    break 'outer;
                }

                total_len += entry.len();
                lines.push(entry);
            }
        }

        lines.join("\n")
    }
}

// ── Layer 2 ───────────────────────────────────────────────────────────────

pub struct Layer2;

impl Layer2 {
    pub fn retrieve(
        conn: &Connection,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        let filter = DrawerFilter {
            wing: wing.map(String::from),
            room: room.map(String::from),
        };
        let drawers = match list_drawers(conn, &filter, n_results) {
            Ok(d) => d,
            Err(_) => return "No palace found.".to_string(),
        };

        if drawers.is_empty() {
            let label = [
                wing.map(|w| format!("wing={w}")),
                room.map(|r| format!("room={r}")),
            ]
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
            return format!("No drawers found for {label}.");
        }

        let mut lines = vec![format!("## L2 — ON-DEMAND ({} drawers)", drawers.len())];
        for drawer in &drawers {
            let source = Path::new(&drawer.source_file)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let snippet = drawer.content.trim().replace('\n', " ");
            let snippet = if snippet.chars().count() > 300 {
                let truncated: String = snippet.chars().take(297).collect();
                format!("{truncated}...")
            } else {
                snippet
            };

            let mut entry = format!("  [{}] {snippet}", drawer.room);
            if !source.is_empty() {
                entry.push_str(&format!("  ({source})"));
            }
            lines.push(entry);
        }

        lines.join("\n")
    }
}

// ── Layer 3 ───────────────────────────────────────────────────────────────

pub struct Layer3;

impl Layer3 {
    pub fn search(
        conn: &Connection,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        let embedding = match crate::embedder::embed_one(query) {
            Ok(e) => e,
            Err(e) => return format!("Embedding error: {e}"),
        };

        let filter = crate::store::DrawerFilter {
            wing: wing.map(String::from),
            room: room.map(String::from),
        };

        let results =
            match crate::ranker::hybrid_search(conn, query, Some(&embedding), &filter, n_results) {
                Ok(r) => r,
                Err(e) => return format!("Search error: {e}"),
            };

        if results.is_empty() {
            return "No results found.".to_string();
        }

        let mut lines = vec![format!("## L3 — SEARCH RESULTS for \"{query}\"")];
        for (i, r) in results.iter().enumerate() {
            let snippet = r.drawer.text.trim().replace('\n', " ");
            let snippet = if snippet.chars().count() > 300 {
                let truncated: String = snippet.chars().take(297).collect();
                format!("{truncated}...")
            } else {
                snippet
            };
            lines.push(format!(
                "  [{}] {}/{} (sim={:.3})",
                i + 1,
                r.drawer.wing,
                r.drawer.room,
                r.combined
            ));
            lines.push(format!("      {snippet}"));
            if !r.drawer.source_file.is_empty() {
                lines.push(format!("      src: {}", r.drawer.source_file));
            }
        }

        lines.join("\n")
    }

    pub fn search_raw(
        conn: &Connection,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> Vec<crate::store::SearchResult> {
        let embedding = match crate::embedder::embed_one(query) {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        let filter = crate::store::DrawerFilter {
            wing: wing.map(String::from),
            room: room.map(String::from),
        };

        crate::ranker::hybrid_search(conn, query, Some(&embedding), &filter, n_results)
            .map(|results| results.into_iter().map(|result| result.drawer).collect())
            .unwrap_or_default()
    }
}

// ── MemoryStack — unified interface ───────────────────────────────────────

pub struct MemoryStack {
    pub identity_path: PathBuf,
    l0: Layer0,
    l1: Layer1,
}

impl MemoryStack {
    pub fn new(db_path: &Path, identity_path: &Path) -> Self {
        Self {
            identity_path: identity_path.to_path_buf(),
            l0: Layer0::new(identity_path),
            l1: Layer1::new(db_path, None),
        }
    }

    /// Generate wake-up text: L0 (identity) + L1 (essential story).
    pub fn wake_up(&mut self, conn: &Connection, wing: Option<&str>) -> String {
        if let Some(w) = wing {
            self.l1.wing = Some(w.to_string());
        }
        let identity = self.l0.render();
        let story = self.l1.generate(conn);
        format!("{identity}\n\n{story}")
    }

    /// On-demand L2 retrieval filtered by wing/room.
    pub fn recall(
        conn: &Connection,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        Layer2::retrieve(conn, wing, room, n_results)
    }

    /// Deep L3 semantic search.
    pub fn search(
        conn: &Connection,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        Layer3::search(conn, query, wing, room, n_results)
    }

    /// Status of all layers.
    pub fn status(&mut self, conn: &Connection) -> serde_json::Value {
        let total = crate::store::count_drawers(conn).unwrap_or(0);
        let identity_exists = self.identity_path.exists();
        let l0_tokens = self.l0.token_estimate();

        serde_json::json!({
            "L0_identity": {
                "path": self.identity_path.to_string_lossy(),
                "exists": identity_exists,
                "tokens": l0_tokens,
            },
            "L1_essential": {
                "description": "Auto-generated from top palace drawers",
            },
            "L2_on_demand": {
                "description": "Wing/room filtered retrieval",
            },
            "L3_deep_search": {
                "description": "Full semantic search",
            },
            "total_drawers": total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: L1 truncation used to slice `&snippet[..197]` on a byte
    /// boundary, panicking when the 197th byte fell mid-UTF-8 char (e.g. `─`).
    /// `wake_up`/`generate` must handle multibyte content without panicking.
    #[test]
    fn layer1_generate_handles_multibyte_content() {
        let conn = crate::db::open_in_memory().unwrap();
        // Box-drawing chars (3 bytes each) make byte length far exceed char
        // count, forcing the truncation branch onto a non-char boundary.
        let content = "─".repeat(300);
        crate::store::add_drawer(
            &conn,
            "wing_test",
            "room",
            &content,
            None,
            "src.md",
            0,
            "test",
            5.0,
        )
        .unwrap();

        let l1 = Layer1::new(Path::new(":memory:"), None);
        let story = l1.generate(&conn); // must not panic on the multibyte slice
        assert!(story.contains("L1"), "expected L1 story output: {story}");
    }
}
