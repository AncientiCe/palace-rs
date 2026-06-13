# Agent Rules

When working on this codebase, follow these rules on every task.

---

## 1. Test-Driven Development (TDD)

- **Write behavioural tests first.** Define the expected behaviour in tests before implementing.
- **See them fail.** Run the test suite and confirm the new tests fail (red).
- **Implement.** Write the minimum code to make the tests pass.
- **See them pass.** Run the test suite and confirm all tests pass (green).

Do not implement behaviour without a failing test that defines it.

---

## 2. Quality Gates on Every Task

Before considering a task done, ensure all of the following pass:

- **`cargo fmt`** — code is formatted.
- **`cargo clippy --all-targets --all-features -- -D warnings`** — no clippy warnings or errors.
- **`cargo audit`** — no known security advisories in dependencies.
- **`cargo test --all-features`** — full test suite passes.
- **Pipelines pass** — check pipeline steps that can be run locally and make sure they pass.

Match CI locally with the exact commands above (see `.github/workflows/ci.yml`).

Fix any failure before marking the task complete.

---

## 3. No Plan Markdown Files

- **Do not create `.md` files for plans** (e.g. `PLAN.md`, `TODO.md`, task plans).
- Create markdown only for **documentation** (API, README, runbooks, etc.) when necessary.
- Keep planning in conversation, tickets, or code comments—not as standalone plan documents in the repo.

---

## 4. Library-First Design

- This crate is **consumed as a library** by Rust applications. Every public API change can break consumers.
- Before changing public types, functions, or module structure, consider: downstream callers, the `Palace` facade, SQLite schema migrations, and embedding model compatibility.
- Keep the public API surface minimal and ergonomic. The `Palace` struct in `src/palace.rs` is the primary facade for library consumers.
- The CLI (`src/cli.rs`) is behind the `cli` feature flag and is secondary to the library API.

---

## 5. No Unused Variables or Dead Code

- **No unused variables.** Every declared variable must be used; remove or replace with `_` if intentionally unused in Rust.
- **No dead code.** Remove unreachable functions, branches, types, and imports — do not leave them commented out or hidden behind `#[allow(dead_code)]`.
- Treat compiler warnings for unused items as errors: they must be resolved before a task is complete.

---

## 6. No Mocks

- **Do not use mocks** in tests (e.g. mock objects, mock servers, or mock crates).
- Prefer **real implementations**, **integration tests** with real services, or **test doubles** (fakes, stubs) that are explicit and minimal.
- Tests must exercise real behaviour where practical; avoid substituting dependencies with mocks that hide integration or behaviour.
- Use in-memory SQLite (`:memory:`) for database tests.

---

## 7. No Placeholders

- **No placeholders. Ever.** Do not leave `todo!()`, `unimplemented!()`, stub returns, or "coming soon" code in the codebase.
- Deliver **only real implementations**. Every code path that is committed must do the real work or explicitly fail in a defined way (e.g. return `Err`, not panic with "unimplemented").
- If a feature is not ready, do not merge it; do not merge placeholder code.

---

## 8. No Unsafe `unwrap`/`expect`

- **Do not use `unwrap()` or `expect()` in production code.** These can panic and crash the process.
- Handle failures safely using explicit error propagation (`Result`/`?`), recoverable branches, or well-defined fallbacks.
- If a value is logically guaranteed, prove it through types/validation rather than runtime panics.
- `unwrap()` and `expect()` are acceptable **only** inside `#[cfg(test)]` test code and examples.

---

## 9. Close Running Instances When Done

- If you start a long-running process for verification (e.g. dev server, watcher, background job), ensure it is stopped/killed before marking the task complete.
- Avoid leaving stuck sessions running after verification; resolve or terminate them so they do not interfere with future tasks.

---

## 10. Keep the Changelog Current

- **Update `CHANGELOG.md` as part of every feature, fix, or behaviour change** — not as an afterthought before release.
- Add entries under a top **`## [Unreleased]`** section while work is in progress (create it if missing, above the most recent released version). Do not invent a version number or date for in-progress work.
- Use the existing Keep a Changelog headings (`### Added`, `### Changed`, `### Fixed`, `### Removed`, `### Deprecated`). The release workflow extracts notes by version heading, so leaving `Unreleased` undated keeps it out of releases until it is cut.
- When a release is tagged, the `## [Unreleased]` entries get renamed to `## [x.y.z] - YYYY-MM-DD`.

---

## Quick Reference

| Rule | Action |
|------|--------|
| TDD | Tests first → see fail → implement → see pass |
| Quality | `cargo fmt` \| `cargo clippy --all-targets --all-features -- -D warnings` \| `cargo audit` \| `cargo test --all-features` |
| No plan files | No `.md` for plans; only real documentation |
| Library-first | Protect public API; consider downstream consumers |
| No dead code | No unused variables, dead code, or `#[allow(dead_code)]` |
| No mocks | No mocks; use real impls, integration tests, or explicit test doubles |
| No placeholders | No placeholders ever; only real implementations |
| No unsafe unwrap/expect | Never use `unwrap()`/`expect()` in production code; handle errors safely |
| Close running instances | Stop any long-running verification processes when done |
| Keep changelog current | Add `### Added/Changed/Fixed` entries under `## [Unreleased]` in `CHANGELOG.md` as you build |

@RTK.md
