# Changelog

All notable changes to `palace-rs` (formerly `mempalace-rs`) are documented here.

This Rust implementation uses its own `0.x` version track.

## [Unreleased]

### Added

- **Non-developer profiles** — a usage profile (`coding` (default), `creative`,
  or `personal`) now shapes Palace for its audience. Set it with
  `palace install --profile <name>`; it persists to `~/.palace/config.json` and
  can be overridden per process with `PALACE_PROFILE`.
  - The injected agent rule (`RULE_BODY`) and the MCP `palace_status` protocol
    text are now persona-shaped: worldbuilding/canon wording for `creative`,
    notes/people/continuity wording for `personal`. `coding` is unchanged.
  - Room auto-detection uses topic-first maps per profile — characters, places,
    lore, factions, sessions, timeline for `creative`; people, health, finances,
    home, schedule, notes for `personal`.
- **MCP prompts** — the server now implements `prompts/list` and `prompts/get`
  (advertised via the `prompts` capability) with `continue-session` and
  `save-session` prompts, giving Claude Desktop users one-click session
  continuity without hooks.
- **`palace_memory_report` tool** — a human-readable inventory of what the palace
  remembers (active profile, per-wing/room counts, recent activity), for
  inspecting memory without a UI.

## [0.9.1] - 2026-07-02

### Added

- **MCP Bundle (`.mcpb`) release artifact** — the release workflow now assembles a
  single `palace-<version>.mcpb` bundle containing the Linux x86_64, macOS
  arm64, and Windows x86_64 binaries plus an MCPB `manifest.json` (binary server
  type with per-platform overrides). The bundle and its SHA-256 are uploaded as
  release assets, enabling one-click install in MCPB-aware hosts.
- **Official MCP registry auto-publish** — a `server.json` (namespace
  `io.github.ancientice/palace-rs`, MCPB package pointing at the release bundle)
  is published to `registry.modelcontextprotocol.io` on every tagged release. The
  release workflow authenticates via GitHub Actions OIDC (no stored secret) and
  fills in the version, download URL, and bundle hash automatically.

## [0.9.0] - 2026-06-29

### Added

- **Ambient warm-start recall at session start** — the `SessionStart` hook now
  injects real recalled memory alongside the protocol text: recent diary entries
  for the session's project (cross-agent, so a different agent the next day still
  benefits) plus the top drawers of the wing the `cwd` maps to. Prior
  investigations land in context deterministically, without the agent having to
  call any palace tool. Fail-open: a missing or empty palace yields the protocol
  text alone. Applies to every client with a `SessionStart` hook (Claude Code,
  Codex, Cursor).

### Fixed

- **Read recall gap** — `PostToolUse` auto-recall fired for the `Read` tool (it is
  in the installed matcher) but never surfaced anything, because the query
  extractor did not read Claude's `file_path` field. Reads now surface relevant
  prior memory. The Cursor `postToolUse` matcher was also aligned to
  `Grep|Read|Glob` for parity with Claude.
- **UTF-8 truncation panic in memory layers** — snippet truncation in the L1/L2/L3
  layer renderers sliced on a byte index (`&s[..197]` / `&s[..297]`), panicking
  when a multibyte character (e.g. `─`, `—`) straddled the boundary — reachable
  via `palace wake-up`, `palace recall`, and `palace search` on real mined
  content. Truncation is now char-boundary safe.

## [0.8.0] - 2026-06-19

### Added

- **First-class protocol tools** — the nine memory-protocol-critical MCP tools
  (`palace_status`, `palace_session_context`, `palace_diary_search`,
  `palace_project_status`, `palace_search`, `palace_kg_query`,
  `palace_preference_search`, `palace_diary_write`, `palace_kg_add`) now emit
  `_meta."anthropic/alwaysLoad" = true` in `tools/list`. Clients that honor the
  hint (Claude Code >= 2.1.121) keep these resident at session start instead of
  deferring them behind tool search, so the mandatory three-trigger protocol
  (SESSION START / BEFORE ANSWERING / AFTER WORK) no longer depends on the agent
  remembering to load the tools first. All other tools remain deferrable to keep
  standing context cost low.

### Fixed

- **Docs** — updated remaining contributor, mission, and Claude notes references
  from the old `mempalace-rs` / `mempalace mcp` names to `palace-rs` /
  `palace mcp`.

## [0.7.0] - 2026-06-13

### Added

- **First-class wings registry** — a new `wings` table is now the source of truth
  for every wing (both code repos and non-repo topics), recording `kind`
  (`project`/`topic`), `description`, `project_path`, and `last_mined_at`. It is
  populated idempotently on every database open by backfilling distinct wings
  already present on drawers (`wing_diary__*`/`general`/`conversations` are
  inferred as topics, everything else as projects), so existing palaces upgrade
  automatically without touching drawer data.
- **MCP project-awareness and on-demand mining** — three new MCP tools:
  `palace_project_status` (reports whether the current repo is `mined` /
  `registered_not_mined` / `unknown`, with a recommendation), `palace_mine`
  (mines a code repository on demand after the user agrees, auto-initialising
  `palace.yaml` for first-time repos and running with its own short-lived
  connection so the server's shared connection is untouched), and
  `palace_create_wing` (declares a topic wing for non-repo users such as sales
  or PMs). `palace_remember` and `palace_add_drawer` now auto-register an unknown
  wing as a topic, and `palace_list_wings` returns full registry records.
- **`palace wings` CLI subcommand** — lists registered wings with their kind,
  drawer counts, and last mined time.
- **Session-start PROJECT CHECK guidance** — the bundled memory protocol now
  tells agents to call `palace_project_status` when entering a workspace, ask the
  user before mining a repo (mining is repo-only and implicitly initialises it),
  and offer `palace_create_wing` for non-repo topics.

### Changed

- **Mining records registry state** — `miner::mine` marks the wing as mined
  (`set_wing_mined`) on success, the watcher refreshes mined state, and `mine`
  gained a `quiet` flag so MCP-triggered mining does not write progress to the
  JSON-RPC stdout stream.

## [0.6.1] - 2026-06-13

### Removed

- **Dead `llm` module and `--no-llm` flag** — the unused local LLM refinement
  scaffolding (`src/llm.rs`), its `pub mod llm;` export, the empty `llm` Cargo
  feature, and the `palace init --no-llm` flag have been removed. Nothing in the
  default path used them.

### Fixed

- **No more panic on an invalid built-in entity pattern** — `compile_regex` in
  the entity detector no longer `panic!`s if a built-in regex fails to compile;
  it logs to stderr and falls back to a never-matching pattern so detection
  degrades gracefully instead of crashing. Covered by new tests that verify the
  built-in patterns compile and the fallback behaves.

### Changed

- **Crate metadata** — removed the broken `docs.rs/palace-rs` documentation link
  (the crate is not published there) and pointed `homepage` at
  `https://palacememory.com`.
- **Docs** — corrected stale `MEMPALACE_*` / `mempalace.yaml` migration claims in
  the README (those names were removed in 0.3.0) and linked `palacememory.com`
  for remote mode.
- **CI/release** — the install smoke matrix now targets `aarch64-apple-darwin`
  to match the actual macOS runner architecture, and the release workflow runs an
  `install-smoke` job that downloads the freshly published asset on macOS and
  Linux and verifies an end-to-end install.

## [0.6.0] - 2026-06-09

### Added

- **Remote MCP mode (shared palace-server client)** — `palace mcp` can now run as
  a transparent stdio→HTTP bridge to a shared remote `palace-server` instead of the
  local palace. Every AI client keeps its existing stdio registration; when remote
  mode is on, each JSON-RPC request is forwarded to the server's `/mcp` endpoint
  with a `Bearer` API key and the response is streamed back verbatim
  (`application/json` and `text/event-stream` both handled). This lets a whole team
  share one memory backend in their own infrastructure without per-client URL/header
  wiring.
  - **`palace remote set --endpoint <url> [--api-key <key>]`** — store the remote
    endpoint and `ps_…` API key. The endpoint accepts a bare host, a base URL, or a
    full `/mcp` URL (normalised automatically). Omit `--api-key` to be prompted on
    stdin so the key never lands in shell history.
  - **`palace remote on` / `palace remote off`** — switch the MCP server between the
    remote palace-server and the local palace.
  - **`palace local`** — alias for `palace remote off`.
  - **`palace remote status`** — show the current MCP mode, normalised endpoint, and
    masked API key.
  - **`palace remote test`** — one-shot connectivity + auth probe that runs the MCP
    `initialize` handshake and reports the number of tools the remote exposes.
  - New configuration surface: `PALACE_MCP_MODE`, `PALACE_REMOTE_ENDPOINT`, and
    `PALACE_API_KEY` environment variables, plus the `mcp_mode`, `remote_endpoint`,
    and `remote_api_key` keys in `~/.palace/config.json` (written with owner-only
    `0600` permissions since the file holds the API key).
  - For non-standard responses, the proxy unwraps palace-server's text envelope for
    `initialize` and `tools/list` so any spec-compliant MCP client understands the
    handshake, while passing `tools/call` and error responses through unchanged.

## [0.5.0] - 2026-06-02

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

### Fixed

- **Resilient model download** — the embedding model fetch now retries with
  exponential backoff, so transient HuggingFace failures (notably HTTP 429 rate
  limiting) no longer fail the first embedding call or CI. CI also caches the
  downloaded model (`~/.cache/huggingface`) across runs to avoid repeated
  downloads.

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
