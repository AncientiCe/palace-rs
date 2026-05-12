//! High-level facade for embedding the memory palace as a library.
//!
//! `Palace` owns the SQLite connection and config, exposing an ergonomic API
//! that callers can use without importing individual modules.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use palace::palace::PalaceBuilder;
//!
//! let palace = PalaceBuilder::new().build_in_memory().unwrap();
//! ```

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use crate::config::PalaceConfig;
use crate::embedder::embed_one;
use crate::general_extractor::{extract_memories, Memory};
use crate::knowledge_graph::{self as kg, KgStats, Triple};
use crate::layers::{Layer3, MemoryStack};
use crate::ranker::HybridResult;
use crate::store::{self, Drawer, DrawerFilter, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const CONVO_WING: &str = "conversations";
const CONVO_ROOM: &str = "voice_turns";
const DEFAULT_IMPORTANCE: f64 = 3.0;
const DEFAULT_INGEST_LABEL: &str = "palace";

/// High-level handle to a memory palace instance.
///
/// Owns the SQLite connection and provides methods for the full lifecycle:
/// open, search, add, ingest turns, wake-up context, and knowledge graph queries.
pub struct Palace {
    conn: Connection,
    config: PalaceConfig,
    stack: MemoryStack,
    ingest_label: String,
}

/// Builder for `Palace`. Preferred over `Palace::open` for new code.
///
/// ```rust,no_run
/// use palace::palace::PalaceBuilder;
/// use palace::config::PalaceConfig;
///
/// let palace = PalaceBuilder::new()
///     .config(PalaceConfig::new())
///     .ingest_label("my_app")
///     .build()
///     .unwrap();
/// ```
pub struct PalaceBuilder {
    config: PalaceConfig,
    ingest_label: String,
}

impl Default for PalaceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PalaceBuilder {
    pub fn new() -> Self {
        Self {
            config: PalaceConfig::new(),
            ingest_label: DEFAULT_INGEST_LABEL.to_string(),
        }
    }

    /// Override the configuration.
    pub fn config(mut self, config: PalaceConfig) -> Self {
        self.config = config;
        self
    }

    /// Override the label stored in `added_by` for all writes through this handle.
    pub fn ingest_label(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        self.ingest_label = if label.trim().is_empty() {
            DEFAULT_INGEST_LABEL.to_string()
        } else {
            label
        };
        self
    }

    /// Open (or create) a palace at the path resolved by the config.
    pub fn build(self) -> Result<Palace> {
        self.config.migrate_legacy_dir();
        let db_path = self.config.palace_db_path();
        let conn = crate::db::open(&db_path)
            .with_context(|| format!("opening palace at {}", db_path.display()))?;
        let identity_path = self.config.identity_path();
        let stack = MemoryStack::new(&db_path, &identity_path);
        Ok(Palace {
            conn,
            config: self.config,
            stack,
            ingest_label: self.ingest_label,
        })
    }

    /// Open an in-memory palace (useful for testing and ephemeral contexts).
    pub fn build_in_memory(self) -> Result<Palace> {
        let conn = crate::db::open_in_memory()?;
        let identity_path = self.config.identity_path();
        let stack = MemoryStack::new(Path::new(":memory:"), &identity_path);
        Ok(Palace {
            conn,
            config: self.config,
            stack,
            ingest_label: self.ingest_label,
        })
    }
}

/// Structured search result with score provenance for library consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalaceSearchResult {
    pub drawer: SearchResult,
    pub cosine: f64,
    pub bm25: f64,
    pub coding_boost: f64,
    pub combined: f64,
}

impl Palace {
    /// Open (or create) a palace at the path specified by `config`.
    ///
    /// Deprecated: use [`PalaceBuilder`] instead.
    #[deprecated(since = "0.2.0", note = "use PalaceBuilder::new().config(c).build()")]
    pub fn open(config: PalaceConfig) -> Result<Self> {
        PalaceBuilder::new().config(config).build()
    }

    /// Open a palace with explicit database and identity paths.
    pub fn open_paths(db_path: &Path, identity_path: &Path) -> Result<Self> {
        let conn = crate::db::open(db_path)
            .with_context(|| format!("opening palace at {}", db_path.display()))?;
        let config = PalaceConfig::new();
        let stack = MemoryStack::new(db_path, identity_path);
        Ok(Self {
            conn,
            config,
            stack,
            ingest_label: DEFAULT_INGEST_LABEL.to_string(),
        })
    }

    /// Open an in-memory palace (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        PalaceBuilder::new().build_in_memory()
    }

    /// Set the label used for memories ingested through this facade.
    ///
    /// Library consumers can use this to identify their integration without
    /// baking private downstream names into the public crate.
    pub fn set_ingest_label(&mut self, label: impl Into<String>) {
        let label = label.into();
        self.ingest_label = if label.trim().is_empty() {
            DEFAULT_INGEST_LABEL.to_string()
        } else {
            label
        };
    }

    /// Label used in the `added_by` column for facade-driven ingestion.
    pub fn ingest_label(&self) -> &str {
        &self.ingest_label
    }

    // ── Layer stack ──────────────────────────────────────────────────────────

    /// Generate L0 (identity) + L1 (essential story) wake-up context.
    ///
    /// Returns a string suitable for prepending to an LLM system prompt.
    pub fn wake_up(&mut self, wing: Option<&str>) -> String {
        self.stack.wake_up(&self.conn, wing)
    }

    /// On-demand L2 retrieval filtered by wing/room.
    pub fn recall(&self, wing: Option<&str>, room: Option<&str>, n_results: usize) -> String {
        MemoryStack::recall(&self.conn, wing, room, n_results)
    }

    /// Deep L3 semantic search, returning formatted text.
    pub fn search_text(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String {
        MemoryStack::search(&self.conn, query, wing, room, n_results)
    }

    /// Deep L3 semantic search, returning structured results.
    pub fn search(&self, query: &str, n_results: usize) -> Result<Vec<SearchResult>> {
        self.search_with_provenance(query, None, None, n_results)
            .map(|results| results.into_iter().map(|result| result.drawer).collect())
    }

    /// Filtered semantic search.
    pub fn search_filtered(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_with_provenance(query, wing, room, n_results)
            .map(|results| results.into_iter().map(|result| result.drawer).collect())
    }

    /// Filtered semantic search with score provenance.
    pub fn search_with_provenance(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> Result<Vec<PalaceSearchResult>> {
        let sanitized_query = crate::query_sanitizer::sanitize_query(query);
        let effective_query = if sanitized_query.is_empty() {
            query
        } else {
            &sanitized_query
        };
        let embedding = embed_one(effective_query)?;
        let filter = DrawerFilter {
            wing: wing.map(String::from),
            room: room.map(String::from),
        };
        let results = crate::ranker::hybrid_search(
            &self.conn,
            effective_query,
            Some(&embedding),
            &filter,
            n_results,
        )?;
        let results = merge_preference_results(&self.conn, &embedding, &filter, results, n_results);
        Ok(results
            .into_iter()
            .map(|result| PalaceSearchResult {
                drawer: result.drawer,
                cosine: result.cosine,
                bm25: result.bm25,
                coding_boost: result.coding_boost,
                combined: result.combined,
            })
            .collect())
    }

    /// Return the best-matching drawer plus adjacent chunks from the same source file.
    pub fn drawer_grep(&self, query: &str, context_radius: usize) -> Result<Vec<Drawer>> {
        let best = self.search(query, 1)?.into_iter().next();
        let Some(best) = best else {
            return Ok(Vec::new());
        };
        let Some(drawer) = store::get_drawer(&self.conn, &best.id)? else {
            return Ok(Vec::new());
        };
        store::source_context(
            &self.conn,
            &drawer.source_file,
            drawer.chunk_index,
            context_radius,
        )
    }

    // ── Drawer operations ────────────────────────────────────────────────────

    /// Add a memory (drawer) to the palace with an auto-generated embedding.
    ///
    /// Returns `(was_inserted, drawer_id)`.
    pub fn add_memory(
        &self,
        wing: &str,
        room: &str,
        content: &str,
        source: &str,
        importance: f64,
    ) -> Result<(bool, String)> {
        let embedding = embed_one(content)?;
        store::add_drawer(
            &self.conn,
            wing,
            room,
            content,
            Some(&embedding),
            source,
            0,
            &self.ingest_label,
            importance,
        )
    }

    /// Total number of drawers in the palace.
    pub fn drawer_count(&self) -> Result<i64> {
        store::count_drawers(&self.conn)
    }

    // ── Conversation ingestion ───────────────────────────────────────────────

    /// Ingest a single conversation turn (user + assistant) into the palace.
    ///
    /// Stores the exchange as a drawer in the "conversations" wing and
    /// extracts typed memories (decisions, preferences, milestones, etc.)
    /// from the combined text using heuristic pattern matching.
    pub fn ingest_turn(&self, user_text: &str, assistant_text: &str) -> Result<()> {
        let combined = format!("User: {user_text}\nAssistant: {assistant_text}");

        let embedding = embed_one(&combined)?;
        store::add_drawer(
            &self.conn,
            CONVO_WING,
            CONVO_ROOM,
            &combined,
            Some(&embedding),
            "voice_turn",
            0,
            &self.ingest_label,
            DEFAULT_IMPORTANCE,
        )?;

        let memories = extract_memories(&combined, 0.3);
        for mem in &memories {
            self.store_extracted_memory(mem)?;
        }

        Ok(())
    }

    fn store_extracted_memory(&self, mem: &Memory) -> Result<()> {
        let embedding = embed_one(&mem.content)?;
        store::add_drawer(
            &self.conn,
            CONVO_WING,
            &mem.memory_type,
            &mem.content,
            Some(&embedding),
            "voice_turn",
            mem.chunk_index,
            &self.ingest_label,
            DEFAULT_IMPORTANCE + 1.0,
        )?;
        Ok(())
    }

    // ── Knowledge graph ──────────────────────────────────────────────────────

    /// Query all relationships for an entity.
    pub fn kg_query(&self, entity: &str) -> Result<Vec<Triple>> {
        kg::query_entity(&self.conn, entity, None, "both")
    }

    /// Query entity relationships as of a specific date.
    pub fn kg_query_as_of(&self, entity: &str, as_of: &str) -> Result<Vec<Triple>> {
        kg::query_entity(&self.conn, entity, Some(as_of), "both")
    }

    /// Add a relationship triple.
    pub fn kg_add_triple(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        confidence: f64,
    ) -> Result<String> {
        kg::add_triple(
            &self.conn, subject, predicate, object, None, None, confidence, None, None,
        )
    }

    /// Knowledge graph statistics.
    pub fn kg_stats(&self) -> Result<KgStats> {
        kg::stats(&self.conn)
    }

    // ── Raw search (for advanced use) ────────────────────────────────────────

    /// L3 raw search returning `Vec<SearchResult>` (no formatting).
    pub fn search_raw(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> Vec<SearchResult> {
        Layer3::search_raw(&self.conn, query, wing, room, n_results)
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Reference to the underlying config.
    pub fn config(&self) -> &PalaceConfig {
        &self.config
    }

    /// Reference to the underlying SQLite connection (escape hatch).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

fn merge_preference_results(
    conn: &Connection,
    query_vec: &[f32],
    filter: &DrawerFilter,
    mut results: Vec<HybridResult>,
    n_results: usize,
) -> Vec<HybridResult> {
    let existing_ids: HashSet<String> = results.iter().map(|r| r.drawer.id.clone()).collect();
    let pref_candidates =
        match crate::store::preference_search_filtered(conn, query_vec, filter, 10) {
            Ok(candidates) => candidates,
            Err(_) => return results,
        };

    for pref in pref_candidates {
        if pref.similarity < 0.25 {
            break;
        }
        if existing_ids.contains(&pref.id) {
            continue;
        }
        let combined = pref.similarity * 0.4;
        results.push(HybridResult {
            cosine: pref.similarity,
            bm25: 0.0,
            coding_boost: 0.0,
            combined,
            drawer: SearchResult {
                similarity: (combined * 1000.0).round() / 1000.0,
                ..pref
            },
        });
    }

    results.sort_by(|a, b| {
        b.combined
            .partial_cmp(&a.combined)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(n_results);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_creates_empty_palace() {
        let palace = Palace::open_in_memory().unwrap();
        assert_eq!(palace.drawer_count().unwrap(), 0);
    }

    #[test]
    fn builder_in_memory_creates_empty_palace() {
        let palace = PalaceBuilder::new().build_in_memory().unwrap();
        assert_eq!(palace.drawer_count().unwrap(), 0);
    }

    #[test]
    fn builder_custom_ingest_label() {
        let palace = PalaceBuilder::new()
            .ingest_label("my_app")
            .build_in_memory()
            .unwrap();
        assert_eq!(palace.ingest_label(), "my_app");
    }

    #[test]
    fn builder_empty_label_falls_back_to_default() {
        let palace = PalaceBuilder::new()
            .ingest_label("")
            .build_in_memory()
            .unwrap();
        assert_eq!(palace.ingest_label(), "palace");
    }

    #[test]
    fn add_memory_and_count() {
        let palace = Palace::open_in_memory().unwrap();
        let (inserted, _id) = palace
            .add_memory(
                "test_wing",
                "test_room",
                "hello world this is a test memory",
                "test",
                3.0,
            )
            .unwrap();
        assert!(inserted);
        assert_eq!(palace.drawer_count().unwrap(), 1);
    }

    #[test]
    fn ingest_turn_stores_exchange() {
        let palace = Palace::open_in_memory().unwrap();
        palace
            .ingest_turn("What is the weather?", "The weather is sunny today.")
            .unwrap();
        assert!(palace.drawer_count().unwrap() >= 1);
    }

    #[test]
    fn ingest_turn_uses_public_default_ingest_label() {
        let palace = Palace::open_in_memory().unwrap();
        palace
            .ingest_turn("Remember this", "Stored for later.")
            .unwrap();

        let added_by: String = palace
            .conn()
            .query_row(
                "SELECT added_by FROM drawers WHERE source_file = 'voice_turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(added_by, "palace");
    }

    #[test]
    fn search_empty_palace_returns_empty() {
        let palace = Palace::open_in_memory().unwrap();
        let results = palace.search("anything", 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_finds_added_memory() {
        let palace = Palace::open_in_memory().unwrap();
        palace
            .add_memory(
                "test_wing",
                "test_room",
                "Rust is a systems programming language focused on safety",
                "test",
                5.0,
            )
            .unwrap();
        let results = palace.search("systems programming safety", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].similarity > 0.3);
    }

    #[test]
    fn kg_add_and_query() {
        let palace = Palace::open_in_memory().unwrap();
        palace
            .kg_add_triple("Alice", "works_at", "Acme", 1.0)
            .unwrap();
        let triples = palace.kg_query("Alice").unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "works_at");
    }

    #[test]
    fn wake_up_returns_identity_and_story() {
        let mut palace = Palace::open_in_memory().unwrap();
        let text = palace.wake_up(None);
        assert!(text.contains("L0"));
    }

    #[test]
    fn recall_empty_palace() {
        let palace = Palace::open_in_memory().unwrap();
        let text = palace.recall(None, None, 10);
        assert!(text.contains("No drawers found") || text.contains("No palace found"));
    }
}
