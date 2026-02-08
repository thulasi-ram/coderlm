#!/bin/bash
# scripts/session-init.sh
# Check if coderlm-server is running and auto-create session.
# Called by the SessionStart hook when the plugin is installed.
# Always exits 0 to never block session start.

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CLI="$PLUGIN_ROOT/.claude/skills/coderlm/scripts/coderlm_cli.py"
STATE_FILE=".claude/coderlm_state/session.json"

# Check server health
if ! curl -s --max-time 2 http://127.0.0.1:3000/api/v1/health > /dev/null 2>&1; then
    echo "[coderlm] Server not running. Start it with: cd server && cargo run -- serve" >&2
    exit 0
fi

# Auto-init if no active session
if [ ! -f "$STATE_FILE" ]; then
    if ! python3 "$CLI" init 2>&1; then
        echo "[coderlm] Failed to initialize session. Run manually: python3 $CLI init" >&2
    fi
fi
