#!/usr/bin/env bash
# Palace Pre-compact Hook — Rust version
# Called before context compaction to emit the wake-up context.
#
# Usage: source this in Claude Code's precompact hook config.

set -euo pipefail

PALACE_BIN="${PALACE_BIN:-palace}"
WING="${PALACE_WING:-}"

if [ -n "$WING" ]; then
    exec "$PALACE_BIN" wake-up --wing "$WING"
else
    exec "$PALACE_BIN" wake-up
fi
