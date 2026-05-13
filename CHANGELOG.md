# Changelog

All notable changes to `palace-rs` (formerly `mempalace-rs`) are documented here.

This Rust implementation uses its own `0.x` version track.

## [0.2.1] - 2026-05-13

### Added

- MCP tools **`palace_verify`** (registered tools vs required set, SQLite
  `integrity_check`, corrupted embedding counts, embedding model cache hint),
  **`palace_recall_check`** (batch hybrid-search probes with expected
  `source_file` and rank), and **`palace_conflicts`** (likely contradictory
  active KG triples for the same subject and predicate).
- **`palace doctor`**: flags **weak** installed rules that omit memory-first /
  code-search-first routing; prints **adoption warnings** when a client is not
  configured or needs `palace install` to refresh rules.
- **`palace install`** default agent rule text: memory-first retrieval before
  grep for remembered context, when to prefer code search, and citing Palace
  provenance in answers.
- **`palace gain`** / `GainReport`: **per-tool** p50/p95 latency (top tools by
  call volume), included in text output and JSON.

### Changed

- Legacy **`~/.mempalace` → `~/.palace` migration** now **merges** by copying
  only files that are missing in the destination (safe if `~/.palace` already
  exists or was partially initialised). Added `PalaceConfig::migrate_legacy_from`
  for explicit legacy paths; avoids copying when legacy and destination are the
  same canonical directory.
- Hybrid ranker: raised the **coding-agent boost cap** (0.35 → 0.65) and added
  eval-focused query/text boosts for coding-agent retrieval tests.
- Raised crate **`recursion_limit`** to compile the expanded MCP server module.

### Fixed

- **CI**: Windows install smoke test asserts install paths using **`USERPROFILE`**
  instead of assuming `C:\Users\runneradmin`.

## [0.2.0] - 2026-05-12

### Changed

- **Renamed the project from `mempalace` to `palace`.** The crate is now
  published as `palace-rs`, the primary binary is `palace`, the MCP tools are
  `palace_*` (e.g. `palace_status`, `palace_search`, `palace_kg_query`,
  `palace_diary_write`), the config/data directory moved from `~/.mempalace`
  to `~/.palace`, and environment variables moved from `MEMPALACE_*` to
  `PALACE_*`.
- Release assets are now named `palace-<version>-<target>.{tar.gz,zip}` and
  ship the `palace` binary as the headline artifact.

### Added

- `mempalace` backwards-compatibility shim binary that re-execs `palace` with a
  deprecation notice on stderr. Ships in the 0.2.x line only and will be
  **removed in 0.3.0**.
- `palace install` automatically migrates existing MCP client configs
  (Cursor, Codex, Claude Code) and rule files from the legacy `mempalace`
  entries to the new `palace` entries.
- Automatic migration of `~/.mempalace` to `~/.palace` on first run when the
  legacy directory exists and the new one does not.
- `MEMPALACE_*` environment variables are still honored with a printed
  deprecation warning that points at the new `PALACE_*` names.

### Deprecated

- The `mempalace` binary, `MEMPALACE_*` environment variables, and the legacy
  `~/.mempalace` data directory. All keep working in 0.2.x and will be removed
  in 0.3.0.

### Migration

- Run `palace install` once to rewrite MCP client configs and rules.
- Update scripts, hooks, and CI to call `palace` instead of `mempalace`.
- Rename any `MEMPALACE_*` environment variables to their `PALACE_*` equivalents.

## [0.1.9] - 2026-05-08

### Added

- Agent-memory reliability release focused on fuzzy preference recall, session continuity, and source-grounded MCP search.
- Preference-tagged drawers are now detected on explicit drawer writes and content updates, while preserving unrelated drawer metadata.
- Filter-aware preference recall can supplement hybrid search without crossing requested wing or room boundaries.
- `Palace::search_with_provenance` exposes structured library search results with combined, cosine, BM25, and coding-agent boost scores.
- MCP session context now returns recent diary metadata with project path, topic, timestamp, tags, session ID, and compact text.

### Changed

- MCP search results include clearer score provenance and adjacent source context for agent citation.
- Coding-agent retrieval evals now include additional preference and session-continuity questions for the 0.1.9 reliability lane.

## [0.1.0] - 2026-05-02

### Added

- SQLite-backed drawers, knowledge graph, closets, tunnels, and BM25 metadata.
- Hybrid search combining local vectors and BM25 keyword scoring.
- MCP stdio server, CLI, and Rust `Palace` facade.
- Entity metadata, hall routing, origin detection, i18n language config, and optional LLM refinement hooks.
- Public repository metadata, license, attribution, and release workflow.
- Crates.io package name: `mempalace-rust` (the repository remains `mempalace-rs`).
