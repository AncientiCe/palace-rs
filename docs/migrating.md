# Migrating from other memories

How to bring what another memory tool knows into Palace.

- [From Python MemPalace](#from-python-mempalace): a full import, one command
- [From another palace database](#from-another-palace-database)
- [From any other memory tool](#from-any-other-memory-tool): mine the originals

Whichever path you take, finish by removing the old tool's MCP server and
hooks from your agent so the two don't compete. `palace install` sets up
Palace's own.

---

## From Python MemPalace

`palace migrate` reads a Python MemPalace install directly. Python and
ChromaDB do not need to be installed.

```bash
palace migrate                       # reads ~/.mempalace
palace migrate ~/somewhere/.mempalace
palace migrate ~/.mempalace/palace   # the palace directory works too
palace status
```

### What comes across

| Python MemPalace | Palace |
|---|---|
| Drawers in `palace/chroma.sqlite3` (ChromaDB, the default backend) or `palace/sqlite_exact.sqlite3` | Drawers, with wing, room, source file, chunk index, `added_by`, importance, and the original `filed_at` |
| Diary entries in `wing_<agent>` / `diary` | Diary entries in `wing_diary__<agent>`, with topic, agent and date, so `palace_diary_read` and `palace_diary_search` find them |
| Other drawer metadata (hall, topic, tags…) | Kept in the drawer's metadata, plus `migrated_from: mempalace` and the original ID as `legacy_id` |
| `knowledge_graph.sqlite3` | Entities with their type and properties, and every fact, expired ones included, with their validity dates |
| `tunnels.json` | Tunnels between wings and rooms |

The import is safe to re-run: drawers keep a stable ID, and facts and tunnels
already in the palace are skipped. Wings it creates are registered like any
other.

### What does not

- **Vectors.** They come from a different model, so every drawer is embedded
  again with Palace's active model. Large palaces take a while.
- **Closets.** They are an index Palace builds from drawers.
- **Tunnel labels.** Palace tunnels have no label field.

Mined drawers get the ID Palace would give them, so mining the same project
again with `palace mine` lands on them rather than duplicating them.

---

## From another palace database

Point `palace migrate` at a `palace.db` file, or at a directory holding one
(for example an old `~/.mempalace` written by palace 0.1):

```bash
palace migrate ~/old-machine/palace.db
```

Drawers whose ID is already present are skipped.

---

## From any other memory tool

Palace does not import other tools' stores. Many keep summaries, extracted
"facts", or compressed text rather than what was actually said, and Palace
keeps text verbatim. Mine the originals instead, and every drawer cites the
file it came from:

```bash
palace init ~/my-project --yes --auto-mine   # your code and docs
palace split --source ~/Desktop/transcripts  # only for concatenated Claude Code transcripts
palace mine-convos ~/Desktop/transcripts     # Claude Code, Codex, Claude.ai, ChatGPT, Slack exports
palace status
```
