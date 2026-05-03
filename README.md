# mempalace-rs

[![CI](https://github.com/AncientiCe/mempalace-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/AncientiCe/mempalace-rs/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/mempalace-rust.svg)](https://crates.io/crates/mempalace-rust)
[![Docs.rs](https://docs.rs/mempalace-rust/badge.svg)](https://docs.rs/mempalace-rust)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.82+](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)

A local memory palace for AI assistants, implemented in Rust.

This project stores verbatim text, embeds it locally, and retrieves relevant
drawers with semantic search. It can be used as a CLI, an MCP stdio server, or
as a Rust library through the `Palace` facade.

## What It Does

- Stores project files and conversation turns in a local SQLite database.
- Generates local embeddings with ONNX Runtime and `all-MiniLM-L6-v2`.
- Retrieves memories by semantic similarity with optional wing/room filters.
- Provides a knowledge graph for temporal entity relationships.
- Exposes MCP tools for assistants that support Model Context Protocol.
- Offers a small Rust library API for embedding memory into other services.

## Storage

Collapses Python's dual-store (ChromaDB + SQLite) into **one file** at `~/.mempalace/palace.db`:

| Table | Purpose |
|---|---|
| `drawers` | Text content + embedding BLOB + metadata |
| `entities` | KG entity nodes |
| `triples` | KG temporal relationship edges |

Embeddings are stored as `f32` vectors from `all-MiniLM-L6-v2`. Search uses local cosine similarity over the stored vectors.

---

## Installation

```bash
git clone https://github.com/AncientiCe/mempalace-rs
cd mempalace-rs
cargo install --path .
```

The first time you run `mine`, the embedding model is downloaded automatically from HuggingFace and cached.

---

## Quick Start

```bash
# 1. Detect rooms from your project folder structure
mempalace init ~/my-project

# 2. Index the project into the palace
mempalace mine ~/my-project

# 3. Index conversations
mempalace mine-convos ~/Desktop/transcripts

# 4. Search
mempalace search "how did we decide on the database schema"

# 5. Wake-up (L0 + L1 context for the AI)
mempalace wake-up
```

---

## CLI Reference

| Command | Description |
|---|---|
| `mempalace init <dir>` | Detect rooms from folder structure, write `mempalace.yaml` |
| `mempalace mine <dir>` | Chunk, embed, and store project files |
| `mempalace mine-convos <dir>` | Ingest conversation exports |
| `mempalace search <query>` | Semantic search with similarity scores |
| `mempalace wake-up` | Print L0 (identity) + L1 (essential story) context |
| `mempalace status` | Palace overview: drawer counts by wing/room |
| `mempalace split` | Split Claude Code mega-transcripts by session |
| `mempalace repair` | Re-embed any drawers missing vectors |
| `mempalace mcp` | Start the MCP stdio server |

### `mine` flags

```bash
mempalace mine ~/my-project \
  --wing my_project          # Override wing name
  --limit 100                # Cap at 100 files
  --dry-run                  # Preview without storing
  --no-gitignore             # Ignore .gitignore rules
  --include vendor,third_party  # Force-include these paths
```

### `mine-convos` flags

```bash
mempalace mine-convos ~/Desktop/transcripts \
  --wing claude_sessions \
  --mode exchange   # or: general (decisions/milestones/emotions)
  --limit 50
  --dry-run
```

### `split` flags

```bash
mempalace split \
  --source ~/Desktop/transcripts \
  --min-sessions 2 \
  --dry-run
```

---

## MCP Setup for Claude Code

Replace the Python server with the native Rust binary — zero config changes needed:

```bash
# Remove old Python MCP server
claude mcp remove mempalace

# Add Rust version (same tool names/schemas)
claude mcp add mempalace -- mempalace mcp
```

Or add manually to `~/.claude/mcp_servers.json`:

```json
{
  "mempalace": {
    "command": "mempalace",
    "args": ["mcp"]
  }
}
```

### MCP Tools

The server exposes tools for status, taxonomy, search, drawer CRUD, knowledge
graph operations, graph tunnels, hook acknowledgements, and agent diaries:

| Tool | Description |
|---|---|
| `mempalace_status` | Palace overview + protocol |
| `mempalace_list_wings` | List wings with drawer counts |
| `mempalace_list_rooms` | List rooms within a wing |
| `mempalace_get_taxonomy` | Full wing → room → count tree |
| `mempalace_get_aaak_spec` | AAAK compressed memory dialect spec |
| `mempalace_search` | Semantic search over drawers |
| `mempalace_check_duplicate` | Check if content already exists |
| `mempalace_add_drawer` | File content into the palace |
| `mempalace_delete_drawer` | Remove a drawer by ID |
| `mempalace_kg_query` | Query entity relationships |
| `mempalace_kg_add` | Add a fact (subject → predicate → object) |
| `mempalace_kg_invalidate` | Mark a fact as no longer true |
| `mempalace_kg_timeline` | Chronological fact history |
| `mempalace_kg_stats` | Knowledge graph overview |
| `mempalace_traverse` | BFS graph walk from a room |
| `mempalace_find_tunnels` | Rooms bridging two wings |
| `mempalace_graph_stats` | Palace graph summary |
| `mempalace_diary_write` | Write a diary entry in AAAK format |
| `mempalace_diary_read` | Read recent diary entries |

---

## Migration from Python

The Rust version uses a new single-file database (`palace.db`). Your existing ChromaDB data cannot be migrated automatically.

**Steps:**

```bash
# 1. Re-mine your projects
mempalace init ~/my-project && mempalace mine ~/my-project

# 2. Re-index conversations
mempalace mine-convos ~/Desktop/transcripts

# 3. Verify
mempalace status
```

Your `identity.txt`, `people_map.json`, and `known_names.json` in `~/.mempalace/` are compatible and will be read automatically.

---

## Configuration

`~/.mempalace/config.json` is read on startup. Environment variables take highest priority:

| Env Var | Default | Description |
|---|---|---|
| `MEMPALACE_PALACE_PATH` | `~/.mempalace/palace` | Palace data directory |

### `mempalace.yaml` (per-project)

Created by `mempalace init`. Example:

```yaml
wing: my_project
rooms:
  - name: backend
    description: Server and API code
    keywords: [api, server, routes, models]
  - name: frontend
    description: UI components
    keywords: [ui, components, pages, views]
  - name: general
    description: Everything else
    keywords: []
```

---

## Memory Stack

| Layer | Name | Description |
|---|---|---|
| L0 | Identity | `~/.mempalace/identity.txt` — always loaded (~100 tokens) |
| L1 | Essential Story | Top drawers by importance, grouped by room (~600–900 tokens) |
| L2 | On-Demand | Wing/room filtered retrieval |
| L3 | Deep Search | Full semantic search |

`mempalace wake-up` prints L0 + L1. The AI uses MCP tools for L2/L3.

---

## Development

```bash
cargo build
cargo test
cargo clippy
```

Tests use in-memory SQLite — no palace.db needed. The embedding model is not loaded in tests that don't require it.

---

## Hooks Compatibility

Shell hooks that previously called `python -m mempalace.mcp_server` can now call `mempalace mcp`. Update the binary path in your hooks:

```bash
# Before (Python)
exec python -m mempalace.mcp_server

# After (Rust)
exec mempalace mcp
```
