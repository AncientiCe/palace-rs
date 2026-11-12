#!/usr/bin/env bash
# Palace Save Hook — Rust version
# Drop-in replacement for the Python hook.
# Pipe this script into your shell session to save conversations.
#
# Usage (in Claude Code):
#   /hooks/palace_save_hook.sh

set -euo pipefail

PALACE_BIN="${PALACE_BIN:-palace}"

# Call the MCP server for diary entry
exec "$PALACE_BIN" mcp
