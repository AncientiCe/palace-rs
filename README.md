# palace-rs

[![CI](https://github.com/AncientiCe/palace-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/AncientiCe/palace-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.82+](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)

A local-first memory retrieval engine for coding agents, implemented in Rust.

This project stores verbatim project and conversation memory, embeds it locally,
and retrieves source-grounded context through MCP. It is built for coding agents
that need to remember decisions, prior fixes, commands, project conventions, and
user preferences across sessions without running a separate vector database.

## What It Does

- Stores project files and conversation turns in a local SQLite database.
- Generates local embeddings with ONNX Runtime and `all-MiniLM-L6-v2`.
- Retrieves memories with hybrid semantic/BM25 search plus coding-agent intent boosts.
- Tags preference-shaped drawers and runs a dedicated preference recall pass for
  fuzzy "what do I prefer?" and convention questions.
- Stores preference spans with optional secondary embeddings and exposes a
  `preference_match` score for preference-shaped queries.
- Classifies search intent (`preference`, `decision`, `how_to`, `definition`,
  `temporal`, `unknown`) and can optionally rerank top results with a local
  interaction reranker.
- Sanitizes agent-generated query dumps before retrieval.
- Returns source-grounded results with score provenance and nearby source context.
- Warms up agents from recent diary entries with project, topic, timestamp, tags,
  and compact session text.
- Provides a knowledge graph for temporal entity relationships.
- Measures real-world usefulness through `palace gain` precision metrics and
  optional folded feedback on the existing `palace_gain` MCP tool.
- Exposes MCP tools for assistants that support Model Context Protocol.
- Offers a small Rust library API for embedding memory into other services.

## Agent Memory Reliability

Version `0.4.0` focuses on the retrieval cases that matter most during coding
work: preferences, project conventions, recent session continuity,
source-grounded answers, and measurable usefulness in real agent sessions.
Drawers that look like user preferences or conventions are tagged in metadata
during writes and updates, record the matched preference span, and can store a
secondary preference embedding. Preference-shaped queries receive a dedicated
`preference_match` score alongside hybrid semantic/BM25 search.

MCP search responses expose score provenance (`combined`, `cosine`, `bm25`, and
`coding_boost`, `preference_match`, optional `rerank_score`, and `intent`) plus
adjacent source context so agents can cite why a memory was returned. Diary tools
provide warm-start context for recent sessions, including project path, topic,
timestamp, session ID, tags, and compact text.

Library consumers can use `Palace::search_with_provenance` when they need the
same structured score details that MCP tools return.

## Storage

Collapses Python's dual-store (ChromaDB + SQLite) into **one file** at `~/.palace/palace.db`:

| Table | Purpose |
|---|---|
| `drawers` | Text content + embedding BLOB + metadata |
| `entities` | KG entity nodes |
| `triples` | KG temporal relationship edges |

Embeddings are stored as `f32` vectors from `all-MiniLM-L6-v2`. Search uses local cosine similarity over the stored vectors.

---

## Benchmarks

### Coding-Agent Memory Eval

The repository includes a focused eval fixture for practical coding-agent memory
questions. It stores realistic memories about project decisions, prior failures,
commands, conventions, user preferences, and current direction, then asks 40
questions such as:

- why did we choose bundled sqlite?
- how did we fix the migration test failure last time?
- what clippy command should I run?
- what is the project convention for search results?
- what changed in the current product direction?

Run it with:

```bash
cargo test --test coding_agent_eval -- --nocapture
```

The test reports `recall@1` and `recall@5` and fails if retrieval drops below the
stable threshold. This is the product-shaped proof: not broad memory theater,
but whether a coding agent can recover the right project context when it matters.

### LongMemEval

Retrieval recall on the LongMemEval `s_cleaned` split — 500 questions over conversational haystacks of ~50 sessions / ~115k tokens each (30 abstention questions are filtered out per the standard convention, leaving 470 evaluated).

The recipe behind the numbers below:

- **Granularity**: one drawer per session.
- **Indexed content**: the **full session** — both user and assistant turns are stored and embedded together. No user-turn filtering, no summarization, no LLM extraction.
- **Embedder**: `all-MiniLM-L6-v2` (384-dim, ONNX), 512-token cap, run locally — no API calls.
- **Retrieval**: **hybrid baseline** — BM25 (k1=1.5, b=0.75, weight 0.35) fused with cosine similarity (weight 0.65), top-K = 10. These reported LongMemEval numbers used pure score fusion, before the coding-agent intent boosts used by current project-memory search.
- **No LLM at any stage**: no extraction, no rerank, no answer generation. The recall numbers measure the retriever in isolation.
- **Metric**: `recall_any@K` at session granularity — does any gold session appear in the top-K results?
- **Hardware**: Apple M1 Pro, 10 cores (8P + 2E), 32 GB RAM.

| Split | R@1 | R@5 | R@10 |
|---|---:|---:|---:|
| `longmemeval_oracle` (sanity check) | 1.000 | 1.000 | 1.000 |
| `longmemeval_s_cleaned` | **0.889** | **0.981** | **0.991** |

Per-question-type on `s_cleaned`:

| Question type | R@1 | R@5 | R@10 |
|---|---:|---:|---:|
| knowledge-update | 0.944 | 1.000 | 1.000 |
| multi-session | 0.909 | 0.983 | 1.000 |
| single-session-assistant | 1.000 | 1.000 | 1.000 |
| single-session-preference | 0.633 | 0.867 | 0.933 |
| single-session-user | 0.922 | 1.000 | 1.000 |
| temporal-reasoning | 0.835 | 0.976 | 0.984 |

### Reading the numbers

- **`oracle` is a sanity check, not a real result.** That split hands the retriever only the sessions known to contain the answer, so perfect recall just confirms the pipeline is wired up correctly.
- **`s_cleaned` is the real test.** ~50 sessions / ~115k tokens of conversational haystack per question, no hints. R@5 = 0.981 means that for 461 of 470 evaluated questions, a gold session appears somewhere in the top 5 retrieved.
- **R@1 → R@5 → R@10 tells you where the failures cluster.** The jump from 0.889 to 0.981 means most "misses" at top-1 are near-misses — the right session is usually rank 2–5, displaced by a lexically similar distractor. The further jump to 0.991 at top-10 means only ~9 questions out of 470 fall outside the top-10 entirely; those are the genuinely hard cases.
- **Per-question-type breakdown is where the model's blind spots show.**
  - `single-session-assistant`, `single-session-user`, `knowledge-update`: ≥0.94 at R@1, ≈1.0 at R@5. The retriever handles direct questions where the answer is stated verbatim in one session.
  - `multi-session` and `temporal-reasoning`: strong at R@5 (~0.98) but lower at R@1 (~0.83–0.91). Multiple sessions are relevant and the "best" one is a judgement call — top-1 ranking among near-equivalents is genuinely ambiguous.
  - `single-session-preference`: the visible weak spot at 0.633 / 0.867 / 0.933. Preference questions ("what's my favorite X") are answered by sentences like *"I like…"* / *"I prefer…"* that don't share keywords with the question. Pure BM25 + frozen MiniLM has no signal for preference-shaped sentences specifically; closing this gap would require either an LLM-extracted preference index or a hand-rolled pattern booster.
- **What's deliberately *not* in these LongMemEval numbers.** No LLM at any stage — no extraction during ingest, no query rewriting, no rerank, no answer generation. No per-dataset hyperparameter tuning. No GPU. The result is the baseline retriever in isolation, on a single CPU, with fixed defaults.

---

## Installation

### Homebrew (macOS Apple Silicon / Linux)

```bash
brew tap AncientiCe/palace
brew install palace

# Configure MCP servers
palace install --all
```

**Note**: macOS Intel is not supported due to ONNX Runtime unavailability. Apple Silicon and Linux x86_64 are fully supported.

### Install Script (macOS / Linux / Windows)

**macOS / Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/AncientiCe/palace-rs/main/scripts/install.sh | sh
```

**Windows:**
```powershell
irm https://raw.githubusercontent.com/AncientiCe/palace-rs/main/scripts/install.ps1 | iex
```

The installer downloads the matching GitHub Release binary, verifies its SHA-256
checksum, installs it locally, and registers the MCP server with Cursor, Codex,
and Claude Code.

### Development Install

```bash
cargo install --path .
palace install
```

The first time you run `mine`, the embedding model is downloaded automatically from HuggingFace and cached.

> **Upgrading from `mempalace` (≤ 0.1.9)?** See [Migrating from `mempalace` to `palace`](#migrating-from-mempalace-to-palace). The 0.2.x line ships a `mempalace` shim binary that forwards to `palace` for one release; it is removed in 0.3.0.

---

## Quick Start

```bash
cargo install --path .       # development install; release installers do this for you
palace install               # configures Cursor + Codex + Claude Code
palace doctor                # verifies MCP config, rules, binary, and drawer count
palace seed-adoption-facts   # seed KG facts that make agent recall measurable
palace init ~/my-project     # detect rooms and write palace.yaml
palace mine ~/my-project     # populate the palace
```

Then restart your agent app so it reloads MCP configuration. Search manually with
`palace search "how did we decide on the database schema"` or let your agent
call the MCP tools when its installed rule tells it to consult memory.

---

## CLI Reference

| Command | Description |
|---|---|
| `palace init <dir>` | Detect rooms from folder structure, write `palace.yaml` |
| `palace mine <dir>` | Chunk, embed, and store project files |
| `palace mine-convos <dir>` | Ingest conversation exports |
| `palace search <query>` | Semantic search with similarity scores |
| `palace wake-up` | Print L0 (identity) + L1 (essential story) context |
| `palace status` | Palace overview: drawer counts by wing/room |
| `palace gain` | Show MCP usage gains, estimated savings, and per-project value |
| `palace split` | Split Claude Code mega-transcripts by session |
| `palace repair` | Re-embed any drawers missing vectors |
| `palace install` | Register the MCP server with Cursor, Codex, and Claude Code |
| `palace uninstall` | Remove palace from MCP client configs |
| `palace doctor` | Inspect binary path, palace DB, and MCP config status |
| `palace seed-adoption-facts` | Seed durable KG facts for Palace adoption and quality gates |
| `palace upgrade-embeddings` | Re-embed drawers; add `--refresh-preferences` to refresh preference-span vectors |
| `palace mcp` | Start the MCP stdio server |

### `mine` flags

```bash
palace mine ~/my-project \
  --wing my_project          # Override wing name
  --limit 100                # Cap at 100 files
  --dry-run                  # Preview without storing
  --no-gitignore             # Ignore .gitignore rules
  --include vendor,third_party  # Force-include these paths
```

### `mine-convos` flags

```bash
palace mine-convos ~/Desktop/transcripts \
  --wing claude_sessions \
  --mode exchange   # or: general (decisions/milestones/emotions)
  --limit 50
  --dry-run
```

### `split` flags

```bash
palace split \
  --source ~/Desktop/transcripts \
  --min-sessions 2 \
  --dry-run
```

### `gain`

`palace gain` summarizes automatic MCP usage by Cursor, Codex, Claude Code,
or any other MCP client. It records local tool-call metadata in `palace.db` and
estimates value from retrieval hits, duplicate skips, KG facts, diary recalls,
repeat questions, and latency.

```bash
palace gain
palace gain --project my_project --since 7d
palace gain --history
palace gain --json
palace gain --record <query_id> <drawer_id> useful
```

Example output:

```text
palace gain - last 30d (palace_rs)
  Tool calls         : 412   (sessions: 27)
  Hit rate           : 88%   (search hits 142/162)
  Precision@1        : 92%
  Precision@5        : 95%
  Tokens saved (est) : ~78,400
  Re-index skipped   : 31    (duplicate drawers avoided)
  KG facts recalled  : 56
  Diary recalls      : 8
  Repeat Qs avoided  : 19
  p95 latency        : 41 ms
  Tool latency       : palace_search(p50 18 ms, p95 41 ms)
  Top wings          : palace_rs(120), checkout(40)
```

Set `PALACE_GAIN_DISABLED=1` to disable usage recording. The legacy
`MEMPALACE_GAIN_DISABLED` is still honored in 0.2.x with a deprecation warning.

`palace_gain` also accepts an optional `record` payload for MCP callers that want
to file explicit usefulness feedback without learning a new tool:

```json
{"record": {"query_id": "query_abc", "drawer_id": "drawer_xyz", "verdict": "useful"}}
```

---

## MCP Setup

`palace install` is the normal setup command for the four supported local agent
clients: Cursor, Codex, Claude Code, and Claude Desktop. It writes both:

- an MCP server entry that starts `palace mcp`
- a small rule that tells the agent when to call `palace_status`,
  `palace_search`, `palace_preference_search`, `palace_kg_query`, and
  `palace_diary_write`

```bash
palace install
```

What gets written by default:

| Client | MCP config | Rule file |
|---|---|---|
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/rules/palace.mdc` |
| Codex | `~/.codex/config.toml` | `~/.codex/AGENTS.md` |
| Claude Code | `~/.claude/mcp_servers.json` | `~/.claude/CLAUDE.md` |
| Claude Desktop | Claude Desktop config | `~/.claude/CLAUDE.md` |

Existing 0.1.x installs that registered the server as `mempalace` are migrated
to `palace` automatically the next time you run `palace install`.

Install for one client:

```bash
palace install --client cursor
palace install --client codex
palace install --client claude
```

Install project-scoped rules instead of global rules:

```bash
palace install --scope project --path /path/to/project
```

For project scope, Cursor also gets a project-local MCP config at
`<project>/.cursor/mcp.json`. Codex and Claude Code keep MCP config in their
user-level config files, while their rules go into `<project>/AGENTS.md` and
`<project>/CLAUDE.md`.

Skip rule files if you only want MCP wiring:

```bash
palace install --no-rule
```

Inspect the current setup:

```bash
palace doctor
```

The installed rule is memory-first for remembered context: decisions, prior
fixes, conventions, preferences, prior commands, session history, and "what
happened last time?" should use Palace before grep or code search. Grep remains
the right first tool for current symbols, exact definitions, exact files, and
implementation details that may have changed since the project was mined.
It also tells agents to warm-start with `palace_session_context`, search diaries
with `palace_diary_search` before continuing old work, use KG tools for durable
facts, and write `palace_diary_write` after substantive work.

### Automatic memory hooks

`palace install` registers user-scope hooks for every client that supports
them, so memory use is automatic in **every** project without per-project rule
edits. The three hooks behave the same everywhere:

- **session start** — injects the protocol (Cursor also exports
  `PALACE_SESSION_ID`).
- **post tool use** — auto-recalls relevant memory while the agent
  investigates, so a prior agent's decisions surface even before the agent
  thinks to search.
- **stop** — if the session engaged Palace but recorded nothing, it asks the
  agent to `palace_diary_write` its investigation and `palace_kg_add` durable
  decisions before finishing. It nudges at most once.

| Client | Config file | Recall matches | Notes |
| --- | --- | --- | --- |
| Cursor | `~/.cursor/hooks.json` | `Grep`/`Read` | flat hook entries + wrapper scripts |
| Claude Code | `~/.claude/settings.json` | `Grep`/`Read`/`Glob` | nested `hooks` blocks |
| Codex | `~/.codex/hooks.json` | `Bash` (shell) | nested `hooks` blocks; run `/hooks` once to trust them |
| Claude Desktop | — | — | no hook system; rules-only (`CLAUDE.md`) |

Claude Code and Codex share a "Claude-style" output dialect
(`hookSpecificOutput.additionalContext` for context, `decision: "block"` +
`reason` to keep the agent working until it saves); Cursor uses its own
`additional_context` / `followup_message` keys. The runner that produces these
is `palace hook <event> --client <cursor|claude|codex>`.

Cross-agent continuity: `palace_diary_search` accepts `all_agents: true` (and an
optional `project_path`) to recall investigations recorded by any agent, and
`palace_session_context` falls back to another agent's recent work for the
project when you have none of your own. Durable decisions belong in the
knowledge graph (`palace_kg_add` / `palace_kg_invalidate`), which dedupes facts
and tracks changes over time, so re-recalled decisions never duplicate.

Seed durable KG facts for adoption tracking:

```bash
palace seed-adoption-facts --project my_project
```

The seed is idempotent and records the four supported clients, the memory-first
protocol, routing rules, user preference for memory-aware agents, and standard
quality gates. Agents can then recall those facts with `palace_kg_query`.

Remove palace config:

```bash
palace uninstall
palace uninstall --client cursor
```

### Cursor

After `palace install --client cursor`, restart Cursor or reload the window.
Settings -> MCP should show `palace` as an enabled stdio server.

Manual Cursor config shape:

```json
{
  "mcpServers": {
    "palace": {
      "command": "palace",
      "args": ["mcp"]
    }
  }
}
```

The rule is installed as `.cursor/rules/palace.mdc` with `alwaysApply: true`.

### Codex

After `palace install --client codex`, restart Codex so it reloads
`~/.codex/config.toml`.

Manual Codex config shape:

```toml
[mcp_servers.palace]
command = "palace"
args = ["mcp"]
```

The rule is installed as a managed palace block in `~/.codex/AGENTS.md` (or
`<project>/AGENTS.md` with `--scope project`). Existing content is preserved.

### Claude Code

After `palace install --client claude`, restart Claude Code so it reloads
`~/.claude/mcp_servers.json`.

Manual Claude JSON shape is the same as Cursor's `mcpServers` object above.
You can also use Claude Code's own MCP command:

```bash
claude mcp remove palace
claude mcp add palace -- palace mcp
```

The rule is installed as a managed palace block in `~/.claude/CLAUDE.md` (or
`<project>/CLAUDE.md` with `--scope project`). Existing content is preserved.

### MCP Tools

The server exposes tools for status, taxonomy, search, drawer CRUD, knowledge
graph operations, graph tunnels, hook acknowledgements, and agent diaries:

| Tool | Description |
|---|---|
| `palace_status` | Palace overview + protocol |
| `palace_gain` | MCP usage gains, estimated savings, and per-project value |
| `palace_verify` | Verify MCP tools, database health, embeddings, and model cache |
| `palace_recall_check` | Run project-memory probes and report expected-memory hits |
| `palace_conflicts` | Surface likely stale or contradictory KG facts |
| `palace_list_wings` | List wings with drawer counts |
| `palace_list_rooms` | List rooms within a wing |
| `palace_get_taxonomy` | Full wing → room → count tree |
| `palace_get_aaak_spec` | AAAK compressed memory dialect spec |
| `palace_search` | Semantic search over drawers |
| `palace_check_duplicate` | Check if content already exists |
| `palace_add_drawer` | File content into the palace |
| `palace_delete_drawer` | Remove a drawer by ID |
| `palace_kg_query` | Query entity relationships |
| `palace_kg_add` | Add a fact (subject → predicate → object) |
| `palace_kg_invalidate` | Mark a fact as no longer true |
| `palace_kg_timeline` | Chronological fact history |
| `palace_kg_stats` | Knowledge graph overview |
| `palace_seed_adoption_facts` | Seed durable KG facts for four-client adoption |
| `palace_traverse` | BFS graph walk from a room |
| `palace_find_tunnels` | Rooms bridging two wings |
| `palace_graph_stats` | Palace graph summary |
| `palace_diary_write` | Write a diary entry in AAAK format |
| `palace_diary_read` | Read recent diary entries |
| `palace_diary_search` | Search within an agent's diary entries |
| `palace_session_context` | Get recent diary context for agent warm-start |

---

## Migrating from `mempalace` to `palace`

The 0.2.0 release renames the project from `mempalace` to `palace`. The 0.2.x line
keeps the old names working with deprecation warnings; they will be removed in 0.3.0.

| Surface | Before (0.1.x) | After (0.2.x) |
|---|---|---|
| Crate | `mempalace-rs` | `palace-rs` |
| Primary binary | `mempalace` | `palace` (the `mempalace` binary is now a deprecation shim) |
| MCP server name | `mempalace` | `palace` (migrated automatically by `palace install`) |
| MCP tools | `mempalace_*` | `palace_*` |
| Config / data dir | `~/.mempalace` | `~/.palace` (auto-migrated on first run) |
| Project config | `mempalace.yaml` | `palace.yaml` (legacy filename still read) |
| Env vars | `MEMPALACE_*` | `PALACE_*` (legacy names accepted with a warning) |
| Cursor rule | `.cursor/rules/mempalace.mdc` | `.cursor/rules/palace.mdc` |
| Release assets | `mempalace-<ver>-<target>` | `palace-<ver>-<target>` |

One-step migration:

```bash
cargo install --path .
palace install
```

`palace install` rewrites existing MCP client configs (Cursor, Codex, Claude Code)
and rule files, replacing legacy `mempalace` entries with `palace` entries.
`~/.mempalace` is moved to `~/.palace` on first run when the legacy directory
exists and the new one does not.

## Migration from Python

The Rust version uses a new single-file database (`palace.db`). Your existing ChromaDB data cannot be migrated automatically.

**Steps:**

```bash
# 1. Re-mine your projects
palace init ~/my-project && palace mine ~/my-project

# 2. Re-index conversations
palace mine-convos ~/Desktop/transcripts

# 3. Verify
palace status
```

Your `identity.txt`, `people_map.json`, and `known_names.json` in `~/.palace/` (migrated from `~/.mempalace/` if present) are compatible and will be read automatically.

---

## Test on a Project

```bash
palace init /path/to/project
palace mine /path/to/project
palace status
```

Restart Cursor, Codex, or Claude Code, then ask the agent a project question that
should use memory, for example: "Search the palace for how this project handles
database migrations." The agent should call `palace_search` through MCP
instead of re-indexing the repository from scratch.

---

## Configuration

`~/.palace/config.json` is read on startup. Environment variables take highest priority:

| Env Var | Default | Description |
|---|---|---|
| `PALACE_PALACE_PATH` | `~/.palace/palace` | Palace data directory |

The legacy `MEMPALACE_PALACE_PATH` is still honored in 0.2.x with a deprecation warning.

### `palace.yaml` (per-project)

Created by `palace init`. The legacy filename `mempalace.yaml` is still read.
Example:

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
| L0 | Identity | `~/.palace/identity.txt` — always loaded (~100 tokens) |
| L1 | Essential Story | Top drawers by importance, grouped by room (~600–900 tokens) |
| L2 | On-Demand | Wing/room filtered retrieval |
| L3 | Deep Search | Full semantic search |

`palace wake-up` prints L0 + L1. The AI uses MCP tools for L2/L3.

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

Shell hooks that previously called `python -m mempalace.mcp_server` or
`mempalace mcp` can now call `palace mcp`. Update the binary path in your hooks:

```bash
# Before (Python)
exec python -m mempalace.mcp_server

# Before (Rust 0.1.x)
exec mempalace mcp

# After (Rust 0.2.x+)
exec palace mcp
```
