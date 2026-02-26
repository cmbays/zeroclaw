#!/usr/bin/env bash
# setup-mattermost-mcp.sh — Generate config.local.json for pvev/mattermost-mcp.
#
# Run once when Mattermost is running to configure the Claude Code MCP bridge.
# After running, restart Claude Code so the mattermost_* tools become available.
#
# Usage:
#   ./scripts/setup-mattermost-mcp.sh [--team-name <name>] [--mcp-dir <path>]
#
# Prerequisites:
#   - Mattermost running at MM_SITE_URL (default: http://localhost:8065)
#   - .envrc.mattermost with MM_CLAUDE_TOKEN or MM_ADMIN_TOKEN set
#   - MCP server cloned and built: cd ~/Github/mattermost-mcp && npm install && npm run build
#   - curl and python3 in PATH
#
# Token priority: MM_CLAUDE_TOKEN > MM_ADMIN_TOKEN > MM_TOKEN_SOKKA

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCP_DIR="${HOME}/Github/mattermost-mcp"
TEAM_NAME=""

# ── Parse flags ───────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --team-name)
      if [[ $# -lt 2 ]]; then
        echo "ERROR: --team-name requires a value" >&2; exit 1
      fi
      TEAM_NAME="$2"; shift 2 ;;
    --mcp-dir)
      if [[ $# -lt 2 ]]; then
        echo "ERROR: --mcp-dir requires a value" >&2; exit 1
      fi
      MCP_DIR="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

CONFIG_OUT="${MCP_DIR}/config.local.json"

# ── Prerequisite checks ───────────────────────────────────────────────────────
for cmd in curl python3; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "ERROR: '$cmd' is required but not found in PATH." >&2
    exit 1
  fi
done

# ── Load credentials ──────────────────────────────────────────────────────────
ENVRC_FILE="${REPO_ROOT}/.envrc.mattermost"
if [[ -f "$ENVRC_FILE" ]]; then
  # shellcheck disable=SC1090
  if ! source "$ENVRC_FILE"; then
    echo "ERROR: Failed to source ${ENVRC_FILE} — check for syntax errors." >&2
    exit 1
  fi
else
  echo "NOTE: ${ENVRC_FILE} not found — relying on existing environment variables." >&2
fi

MM_SITE_URL="${MM_SITE_URL:-http://localhost:8065}"
TEAM_NAME="${TEAM_NAME:-${MM_TEAM_NAME:-zeroclaw-hq}}"

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
  echo "ERROR: No Mattermost token found." >&2
  echo "  Set MM_CLAUDE_TOKEN (preferred) or MM_ADMIN_TOKEN in .envrc.mattermost" >&2
  exit 1
fi

echo "🔑 Using token: ${TOKEN_LABEL}"
echo "🌐 Site URL:    ${MM_SITE_URL}"
echo "👥 Team:        ${TEAM_NAME}"

# ── Verify MCP dir ────────────────────────────────────────────────────────────
if [[ ! -f "${MCP_DIR}/build/index.js" ]]; then
  echo "ERROR: MCP server not built at ${MCP_DIR}/build/index.js" >&2
  echo "  Run: cd '${MCP_DIR}' && npm install && npm run build" >&2
  exit 1
fi

# ── Fetch team ID ─────────────────────────────────────────────────────────────
echo "🔍 Querying Mattermost for team '${TEAM_NAME}'..."
HTTP_CODE=$(curl -s -o /tmp/mm_team_response.json -w "%{http_code}" \
  --connect-timeout 5 --max-time 15 \
  -H "Authorization: Bearer ${API_TOKEN}" \
  "${MM_SITE_URL}/api/v4/teams/name/${TEAM_NAME}" 2>/tmp/mm_curl_err) || {
  CURL_ERR=$(cat /tmp/mm_curl_err 2>/dev/null || true)
  echo "ERROR: curl failed connecting to ${MM_SITE_URL}" >&2
  [[ -n "$CURL_ERR" ]] && echo "  ${CURL_ERR}" >&2
  exit 1
}

if [[ "$HTTP_CODE" != "200" ]]; then
  ERROR_MSG=$(python3 -c "
import json, sys
try:
    d = json.load(open('/tmp/mm_team_response.json'))
    print(d.get('message', 'unknown error'))
except Exception:
    print('(could not parse response)')
" 2>/dev/null || echo "(could not parse response)")
  echo "ERROR: Mattermost returned HTTP ${HTTP_CODE} for team '${TEAM_NAME}'" >&2
  echo "  ${ERROR_MSG}" >&2
  case "$HTTP_CODE" in
    401|403) echo "  Check that the token is valid and has team read permissions." >&2 ;;
    404)     echo "  Team '${TEAM_NAME}' not found. Use --team-name to specify the correct name." >&2 ;;
  esac
  exit 1
fi

TEAM_ID=$(python3 -c "
import json, sys
try:
    d = json.load(open('/tmp/mm_team_response.json'))
    if 'status_code' in d:
        print('ERROR: API returned error: ' + d.get('message', '?'), file=sys.stderr)
        sys.exit(1)
    team_id = d.get('id', '')
    if not team_id or len(team_id) < 10:
        print('ERROR: response contains no valid team id', file=sys.stderr)
        sys.exit(1)
    print(team_id)
except json.JSONDecodeError as e:
    print(f'ERROR: Mattermost returned non-JSON response: {e}', file=sys.stderr)
    sys.exit(1)
" 2>&1) || { echo "ERROR: ${TEAM_ID}" >&2; exit 1; }

echo "✅ Team '${TEAM_NAME}' → ID: ${TEAM_ID}"

# ── Write config.local.json (pass values via env, never interpolate into code) ─
echo "📝 Writing ${CONFIG_OUT}..."
_MM_SITE_URL="$MM_SITE_URL" _MM_TOKEN="$API_TOKEN" _MM_TEAM_ID="$TEAM_ID" \
  _MM_CONFIG_OUT="$CONFIG_OUT" python3 - <<'PYEOF'
import json, os, sys

token    = os.environ["_MM_TOKEN"]
site_url = os.environ["_MM_SITE_URL"]
team_id  = os.environ["_MM_TEAM_ID"]
out_path = os.environ["_MM_CONFIG_OUT"]

config = {
    "mattermostUrl": f"{site_url}/api/v4",
    "token": token,
    "teamId": team_id,
    "monitoring": {
        "enabled": False,
        "schedule": "*/15 * * * *",
        "channels": ["town-square"],
        "topics": [],
        "messageLimit": 30
    }
}

try:
    with open(out_path, "w") as f:
        json.dump(config, f, indent=2)
        f.write("\n")
except OSError as e:
    print(f"ERROR: Could not write config to {out_path}: {e}", file=sys.stderr)
    sys.exit(1)

print(f"✅ Written to {out_path}")
PYEOF

echo ""
echo "Next steps:"
echo "  1. Restart Claude Code (or reload MCP servers)"
echo "  2. You should see mattermost_* tools available in your session"
echo "  3. Test: use mattermost_list_channels to verify connectivity"
