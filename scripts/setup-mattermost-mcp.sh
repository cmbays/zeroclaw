#!/usr/bin/env bash
# setup-mattermost-mcp.sh — Generate config.local.json for pvev/mattermost-mcp.
#
# Run once when Mattermost is running to configure the Claude Code MCP bridge.
# After running, restart Claude Code so the mattermost_* tools become available.
#
# Usage:
#   ./scripts/setup-mattermost-mcp.sh [--team-name <name>]
#
# Prerequisites:
#   - Mattermost running at MM_SITE_URL (default: http://localhost:8065)
#   - ~/.envrc.mattermost or .envrc.mattermost with MM_CLAUDE_TOKEN or MM_ADMIN_TOKEN
#   - ~/Github/mattermost-mcp already cloned and built (npm run build)
#
# Token priority: MM_CLAUDE_TOKEN > MM_ADMIN_TOKEN > MM_TOKEN_SOKKA

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCP_DIR="${HOME}/Github/mattermost-mcp"
CONFIG_OUT="${MCP_DIR}/config.local.json"
TEAM_NAME="${1:-}"
shift 2>/dev/null || true

# Parse --team-name flag
while [[ $# -gt 0 ]]; do
  case "$1" in
    --team-name) TEAM_NAME="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

# ── Load credentials ──────────────────────────────────────────────────────────
ENVRC_FILE="${REPO_ROOT}/.envrc.mattermost"
if [[ -f "$ENVRC_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$ENVRC_FILE"
fi

MM_SITE_URL="${MM_SITE_URL:-http://localhost:8065}"
TEAM_NAME="${TEAM_NAME:-${MM_TEAM_NAME:-zeroclaw}}"

if [[ -n "${MM_CLAUDE_TOKEN:-}" ]]; then
  API_TOKEN="$MM_CLAUDE_TOKEN"
  TOKEN_LABEL="MM_CLAUDE_TOKEN"
elif [[ -n "${MM_ADMIN_TOKEN:-}" ]]; then
  API_TOKEN="$MM_ADMIN_TOKEN"
  TOKEN_LABEL="MM_ADMIN_TOKEN (fallback)"
elif [[ -n "${MM_TOKEN_SOKKA:-}" ]]; then
  API_TOKEN="$MM_TOKEN_SOKKA"
  TOKEN_LABEL="MM_TOKEN_SOKKA (fallback)"
else
  echo "❌ No Mattermost token found."
  echo "   Set MM_CLAUDE_TOKEN (preferred) or MM_ADMIN_TOKEN in .envrc.mattermost"
  exit 1
fi

echo "🔑 Using token: ${TOKEN_LABEL}"
echo "🌐 Site URL:    ${MM_SITE_URL}"
echo "👥 Team:        ${TEAM_NAME}"

# ── Verify MCP dir ────────────────────────────────────────────────────────────
if [[ ! -f "${MCP_DIR}/build/index.js" ]]; then
  echo "❌ MCP server not built at ${MCP_DIR}/build/index.js"
  echo "   Run: cd ~/Github/mattermost-mcp && npm install && npm run build"
  exit 1
fi

# ── Fetch team ID ─────────────────────────────────────────────────────────────
echo "🔍 Querying Mattermost for team '${TEAM_NAME}'..."
RESPONSE=$(curl -s --connect-timeout 5 \
  -H "Authorization: Bearer ${API_TOKEN}" \
  "${MM_SITE_URL}/api/v4/teams/name/${TEAM_NAME}")

TEAM_ID=$(echo "$RESPONSE" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d.get('id', ''))
except Exception:
    print('')
" 2>/dev/null)

if [[ -z "$TEAM_ID" ]]; then
  echo "❌ Could not get team ID for '${TEAM_NAME}'"
  echo "   Mattermost response: ${RESPONSE}"
  echo "   Is Mattermost running? Is the team name correct?"
  exit 1
fi

echo "✅ Team '${TEAM_NAME}' → ID: ${TEAM_ID}"

# ── Write config.local.json ───────────────────────────────────────────────────
python3 - <<PYEOF
import json, os

config = {
    "mattermostUrl": "${MM_SITE_URL}/api/v4",
    "token": "${API_TOKEN}",
    "teamId": "${TEAM_ID}",
    "monitoring": {
        "enabled": False,
        "schedule": "*/15 * * * *",
        "channels": ["town-square"],
        "topics": [],
        "messageLimit": 30
    }
}

out_path = "${CONFIG_OUT}"
with open(out_path, "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")

print(f"✅ Written to {out_path}")
PYEOF

echo ""
echo "Next steps:"
echo "  1. Restart Claude Code (or reload MCP servers)"
echo "  2. You should see mattermost_* tools available in your session"
echo "  3. Test: use mattermost_list_channels to verify connectivity"
