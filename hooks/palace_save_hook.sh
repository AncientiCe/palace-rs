#!/usr/bin/env bash
# MemPalace Save Hook — Rust version
# Drop-in replacement for the Python mempalace hook.
# Pipe this script into your shell session to save conversations.
#
# Usage (in Claude Code):
#   /hooks/mempal_save_hook.sh

set -euo pipefail

MEMPALACE_BIN="${MEMPALACE_BIN:-mempalace}"

# Call the MCP server for diary entry
exec "$MEMPALACE_BIN" mcp
