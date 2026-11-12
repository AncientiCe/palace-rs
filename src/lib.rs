//! Local-first memory for AI assistants.
//!
//! `palace-rs` stores verbatim text as *drawers* in one SQLite file, embeds it
//! locally with ONNX Runtime, and retrieves it with hybrid (vector + BM25)
//! search. A temporal knowledge graph keeps facts with validity windows.
//! The same crate powers the `palace` CLI and MCP server.
//!
//! # Using it as a library
//!
//! Turn off the default `cli` feature to leave out the CLI dependencies:
//!
//! ```toml
//! [dependencies]
//! palace-rs = { version = "0.19", default-features = false }
//! ```
//!
//! The library is imported as `palace`:
//!
//! ```
//! use palace::{db, knowledge_graph, store};
//!
//! # fn main() -> anyhow::Result<()> {
//! let conn = db::open_in_memory()?;
//! store::add_drawer(
//!     &conn,
//!     "wing_code",
//!     "backend",
//!     "We use SQLite with WAL mode for concurrent reads.",
//!     None,
//!     "architecture.md",
//!     0,
//!     "example",
//!     4.0,
//! )?;
//! knowledge_graph::add_triple(
//!     &conn,
//!     "Palace",
//!     "stores_data_in",
//!     "SQLite",
//!     Some("2026-04-01"),
//!     None,
//!     1.0,
//!     None,
//!     None,
//! )?;
//! assert_eq!(store::count_drawers(&conn)?, 1);
//! # Ok(())
//! # }
//! ```
//!
//! Starting points:
//!
//! - [`db`] opens a palace (`db::open` for a file, `db::open_in_memory` for tests).
//! - [`store`] adds, updates, and lists drawers.
//! - [`palace::Palace`] wraps a palace for search, including
//!   [`palace::Palace::search_with_provenance`] for scored, cited results.
//! - [`knowledge_graph`] adds and queries facts.
//! - [`embedder`] embeds text with the active local model.

#![recursion_limit = "256"]

pub mod backend;
pub mod backends;
#[cfg(feature = "benchmarks")]
pub mod benchmarks;
pub mod closet;
pub mod config;
pub mod convo_miner;
pub mod db;
pub mod dialect;
pub mod embedder;
pub mod embedding_eval;
pub mod embedding_migration;
pub mod embedding_models;
pub mod entity_detector;
pub mod export;
pub mod feedback;
pub mod gain;
pub mod general_extractor;
pub mod hall_router;
pub mod hook_sessions;
pub mod hooks;
pub mod i18n;
pub mod install;
pub mod knowledge_graph;
pub mod layers;
pub mod manifest_entities;
pub mod mcp_server;
pub mod migrate;
pub mod miner;
pub mod normalize;
pub mod onboarding;
pub mod origin;
pub mod palace;
pub mod palace_graph;
pub mod preference;
pub mod query_intent;
pub mod query_rewriter;
pub mod query_sanitizer;
pub mod ranker;
pub mod remote;
pub mod reranker;
pub mod room_detector;
pub mod searcher;
pub mod split;
pub mod store;
pub mod sweep;
pub mod time_query;
pub mod usage;
pub mod watcher;
pub mod wings;

#[cfg(feature = "cli")]
pub mod cli;
