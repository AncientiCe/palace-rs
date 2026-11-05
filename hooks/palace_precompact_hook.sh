#!/usr/bin/env bash
# MemPalace Pre-compact Hook — Rust version
# Called before context compaction to emit the wake-up context.
#
# Usage: source this in Claude Code's precompact hook config.

set -euo pipefail

MEMPALACE_BIN="${MEMPALACE_BIN:-mempalace}"
WING="${MEMPALACE_WING:-}"

if [ -n "$WING" ]; then
    exec "$MEMPALACE_BIN" wake-up --wing "$WING"
else
    exec "$MEMPALACE_BIN" wake-up
fi
