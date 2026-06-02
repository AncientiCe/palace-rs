# Changelog

All notable changes to `palace-rs` (formerly `mempalace-rs`) are documented here.

This Rust implementation uses its own `0.x` version track.

## [Unreleased]

### Added

- **Automatic memory hooks (all clients)** — `palace install` now registers
  user-scope `session start`, `post tool use`, and `stop` hooks for every client
  that supports them, enforcing memory use in every project without per-project
  rules. `post tool use` auto-recalls relevant memory while the agent
  investigates; `stop` nudges the agent to record its work when it engaged Palace
  but saved nothing. Coverage:
  - **Cursor** — `~/.cursor/hooks.json` (recall matched to `Grep`/`Read`).
  - **Claude Code** — `~/.claude/settings.json` nested hooks (recall matched to
    `Grep`/`Read`/`Glob`).
  - **Codex** — `~/.codex/hooks.json` nested hooks (recall matched to `Bash`,
    since Codex investigates through the shell; run `/hooks` once to trust them).
  - **Claude Desktop** — no hook system, so it remains rules-only (`CLAUDE.md`).

  Claude Code and Codex share a "Claude-style" output dialect
  (`hookSpecificOutput.additionalContext`; `decision: "block"` + `reason` on
  stop), selected via `palace hook <event> --client <cursor|claude|codex>`. The
  `palace::hooks` module exposes the pure, client-aware response builders.
- **Cross-agent recall** — `palace_diary_search` accepts `all_agents` and
  `project_path` to surface investigations recorded by any agent, and
  `palace_session_context` falls back to another agent's recent project work when
  the caller has none, so a different agent the next day still benefits from
  prior work.

### Changed

- **Write-path dedup** — `palace_diary_write` (scoped per agent) and
  `palace_remember` now skip near-duplicate content, preventing diary/fact bloat
  from repeated similar writes. Durable decisions still belong in the
  deduplicated knowledge graph.

## [0.4.0] - 2026-05-22

### Added

- **Preference lane v2** — preference-shaped drawers now store a
  `preference_span` metadata field and optional `pref_embedding` secondary
  vector. Preference queries receive a separate `preference_match` score so the
  known `single-session-preference` weakness can improve without adding an LLM
  to the default path.
- **Query intent provenance** — searches classify queries as `preference`,
  `decision`, `how_to`, `definition`, `temporal`, or `unknown`, and return the
  intent in MCP and `Palace::search_with_provenance` results.
- **Optional local rerank path** — `PALACE_RERANK=1`, `palace search --rerank`,
  or `palace_search({"rerank": true})` reranks top hybrid candidates and returns
  `rerank_score` provenance. The default search path is unchanged.
- **Gain v2 feedback** — `palace_gain` now accepts an optional `record` payload
  for explicit usefulness feedback and reports `precision_at_1`,
  `precision_at_5`, and per-intent precision. Diary entries that cite returned
  drawer IDs also infer useful feedback automatically.
- **Eval CI lane** — CI now runs the coding-agent memory eval separately and
  checks the sampled LongMemEval baseline guard.

### Changed

- `palace upgrade-embeddings --refresh-preferences` and
  `palace_upgrade_embeddings({"refresh_preferences": true})` can refresh
  preference-span embeddings while re-embedding drawers.
- `Palace::search_with_provenance` is additive: results include
  `preference_match`, `intent`, and `rerank_score`.

## [0.3.2] - 2026-05-15

### Changed

- **Agent rule restructured into 3 hard trigger blocks** — `RULE_BODY` (installed
  into `.cursor/rules/palace.mdc`, `.codex/AGENTS.md`, `.claude/CLAUDE.md`) and
  `PALACE_PROTOCOL` (embedded in `palace_status` MCP response) are now structured
  as three imperative blocks: **SESSION START**, **BEFORE ANSWERING**, and
  **AFTER WORK**. The previous 11-step and 9-step numbered lists caused agents to
  front-load steps 1–2 and deprioritise the rest; the new block layout makes each
  trigger an unconditional gate, improving `palace_diary_search` and
  `palace_kg_query` call rates in practice.
- **`rule_is_weak` detection** updated to check for the new block keywords
  (`SESSION START`, `BEFORE ANSWERING`, `AFTER WORK`) rather than the old numbered
  step phrases. Run `palace install` to refresh installed rules.

## [0.3.1] - 2026-05-15

### Added

- **`palace_import`** MCP tool — import drawers from a JSON snapshot produced
  by `palace_export`. Skips drawers that already exist (idempotent). Returns
  `inserted`, `skipped`, and `total` counts.
- **`palace_upgrade_embeddings`** MCP tool — re-embeds all drawers using the
  current embedding model. Run after upgrading the model to keep search quality
  consistent.
- **`palace_prune`** MCP tool — deletes drawers filed more than `older_than_days`
  days ago. Irreversible; export first if unsure.
- **`palace prune --older-than-days N [--dry-run]`** CLI command.
- **`palace upgrade-embeddings`** CLI command.
- **`palace timeline [--entity <name>]`** CLI command — human-readable
  chronological view of knowledge-graph facts, optionally filtered to one entity.

## [0.3.0] - 2026-05-15

### Removed

- **`mempalace` shim binary** — the backwards-compatibility binary that
  re-exec'd `palace` with a deprecation notice has been removed as promised
  in 0.2.0. If you are still calling `mempalace` in scripts or MCP configs,
  replace it with `palace`. Run `palace install` to update MCP client configs
  automatically.
- **`MEMPALACE_PALACE_PATH` and `MEMPALACE_ENTITY_LANGUAGES` environment
  variables** — only the `PALACE_*` equivalents are now honoured.
  Update any scripts or CI that set the old names.
- **`MEMPALACE_SOURCE_DIR`, `MEMPALACE_PROJECT`, `MEMPALACE_GAIN_DISABLED`
  environment variables** — replaced by `PALACE_SOURCE_DIR`,
  `PALACE_PROJECT`, and `PALACE_GAIN_DISABLED` respectively.
- **`MEMPALACE_OPENAI_COMPAT_BASE_URL`** — replaced by
  `PALACE_OPENAI_COMPAT_BASE_URL`.
- **`PalaceConfig::migrate_legacy_dir` / `migrate_legacy_from`** — the
  automatic `~/.mempalace → ~/.palace` directory migration has been removed.
  Users who have not yet migrated should copy their data manually or use
  an earlier release to perform the migration first.
- **`MempalaceConfig` type alias** — removed; use `PalaceConfig` directly.
- **Legacy `mempalace.yaml` / `mempal.yaml` project config fallbacks** —
  `palace mine` now requires a `palace.yaml` in the project root.
  Run `palace init` to generate one if it is missing.

### Migration from 0.2.x

- Replace all `mempalace` binary invocations with `palace`.
- Rename `MEMPALACE_*` environment variables to their `PALACE_*` equivalents.
- Rename `mempalace.yaml` / `mempal.yaml` project config files to
  `palace.yaml` (or run `palace init` to regenerate).

## [0.2.2] - 2026-05-15

### Added

- **`palace seed-adoption-facts`** CLI command and **`palace_seed_adoption_facts`**
  MCP tool to seed durable KG facts for the four supported clients, the
  memory-first protocol, quality gates, and project role. The seed is
  idempotent so repeated runs add only missing active facts.
- KG seed helpers for adoption facts and safe fact replacement, including
  invalidating an old active fact before inserting the new one.

### Changed

- Installed agent rules now require the full memory lifecycle: `palace_status`
  plus `palace_session_context` at session start, `palace_diary_search` before
  continuing prior work, KG tools for durable facts, repeat-question recall, and
  explicit memory-vs-code-search routing.
- `palace_status` protocol text now mirrors the stronger four-agent memory
  workflow so MCP clients see the same guidance at runtime.
- `palace doctor` weak-rule detection now checks for session warm-start, diary
  recall, KG add/query/invalidate, repeat-question routing, and memory routing.
- `palace gain` accounting now counts non-empty `palace_session_context` and
  `palace_diary_search` calls as diary recalls, recognizes additional KG query
  response shapes as KG facts, and tracks repeated diary queries as repeat
  questions avoided.

### Fixed

- Renamed the install-rule integration test binary to avoid Windows treating it
  like an installer that requires elevation.

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
