#!/usr/bin/env bash
# mattermost-setup.sh — Idempotent ZeroClaw Hub provisioning
#
# Creates the ZeroClaw HQ team, 6 ATLA bot accounts, channels, and
# channel memberships on a fresh Mattermost instance.
#
# Usage:
#   # From the repo root, with infra/mattermost/.env loaded:
#   source infra/mattermost/.env
#   ./scripts/mattermost-setup.sh
#
# Or with explicit flags:
#   ./scripts/mattermost-setup.sh \
#     --url http://localhost:8065 \
#     --admin admin \
#     --password yourpassword
#
# Idempotency: every resource is checked before creation.
#   Re-running is safe; existing resources are skipped.
#
# Output: prints bot tokens at the end for use in .envrc.mattermost
#
# Requirements: curl, jq

set -euo pipefail

# ── Defaults (override via flags or env) ──────────────────────────
MM_URL="${MM_SITE_URL:-http://localhost:8065}"
MM_ADMIN="${MM_ADMIN_USERNAME:-admin}"
MM_PASSWORD="${MM_ADMIN_PASSWORD:-}"
TEAM_NAME="zeroclaw-hq"
TEAM_DISPLAY="ZeroClaw HQ"

# ── Argument parsing ──────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --url)      MM_URL="$2"; shift 2 ;;
    --admin)    MM_ADMIN="$2"; shift 2 ;;
    --password) MM_PASSWORD="$2"; shift 2 ;;
    *) echo "Unknown flag: $1" >&2; exit 1 ;;
  esac
done

if [[ -z "$MM_PASSWORD" ]]; then
  echo "ERROR: admin password required (--password or MM_ADMIN_PASSWORD env)" >&2
  exit 1
fi

# ── Dependency check ──────────────────────────────────────────────
for cmd in curl jq; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "ERROR: '$cmd' is required but not installed." >&2
    exit 1
  fi
done

API="${MM_URL}/api/v4"

# ── Helper: authenticated API call ───────────────────────────────
# Usage: mm_api METHOD /path [body_json]
# Returns: parsed JSON (via jq). Exits non-zero on HTTP error.
TOKEN=""

mm_api() {
  local method="$1" path="$2" body="${3:-}"
  local auth_header=()
  [[ -n "$TOKEN" ]] && auth_header=(-H "Authorization: Bearer $TOKEN")
  local args=(-s -X "$method" "${auth_header[@]}" -H "Content-Type: application/json")
  [[ -n "$body" ]] && args+=(-d "$body")
  curl "${args[@]}" "${API}${path}"
}

mm_api_check() {
  local resp
  resp=$(mm_api "$@")
  local status_msg
  status_msg=$(echo "$resp" | jq -r '.status_code // empty' 2>/dev/null)
  if [[ "$status_msg" =~ ^[45][0-9][0-9]$ ]]; then
    echo "ERROR: API call failed ($1 $2): $(echo "$resp" | jq -r '.message // .error // .')" >&2
    return 1
  fi
  echo "$resp"
}

# ── Step 1: Authenticate ──────────────────────────────────────────
echo ""
echo "=== Step 1: Authenticating as ${MM_ADMIN} ==="

login_resp=$(mm_api POST /users/login \
  "{\"login_id\": \"${MM_ADMIN}\", \"password\": \"${MM_PASSWORD}\"}" 2>&1 | \
  (curl -s -X POST "${API}/users/login" \
    -H "Content-Type: application/json" \
    -d "{\"login_id\": \"${MM_ADMIN}\", \"password\": \"${MM_PASSWORD}\"}" \
    -D /tmp/mm_headers.txt 2>/dev/null; cat /tmp/mm_headers.txt))

TOKEN=$(grep -i '^token:' /tmp/mm_headers.txt 2>/dev/null | awk '{print $2}' | tr -d '[:space:]') || true

if [[ -z "$TOKEN" ]]; then
  # Try without headers trick — some versions use body token
  login_json=$(curl -s -X POST "${API}/users/login" \
    -H "Content-Type: application/json" \
    -d "{\"login_id\": \"${MM_ADMIN}\", \"password\": \"${MM_PASSWORD}\"}" \
    -c /tmp/mm_cookies.txt)
  TOKEN=$(echo "$login_json" | jq -r '.token // empty')
fi

if [[ -z "$TOKEN" ]]; then
  # Third approach: use header dump properly
  TOKEN=$(curl -s -X POST "${API}/users/login" \
    -H "Content-Type: application/json" \
    -d "{\"login_id\": \"${MM_ADMIN}\", \"password\": \"${MM_PASSWORD}\"}" \
    -i 2>/dev/null | grep -i '^token:' | awk '{print $2}' | tr -d '[:space:]')
fi

if [[ -z "$TOKEN" ]]; then
  echo "ERROR: Failed to authenticate. Check MM_ADMIN_USERNAME and MM_ADMIN_PASSWORD." >&2
  exit 1
fi

echo "  Authenticated. Token: ${TOKEN:0:8}..."

# Shorthand authenticated calls now that TOKEN is set
api_get()  { mm_api_check GET  "$1"; }
api_post() { mm_api_check POST "$1" "$2"; }
api_put()  { mm_api_check PUT  "$1" "$2"; }

# ── Step 2: Get admin user ID ─────────────────────────────────────
echo ""
echo "=== Step 2: Resolving admin user ID ==="
admin_resp=$(api_get "/users/me")
ADMIN_ID=$(echo "$admin_resp" | jq -r '.id')
echo "  Admin ID: ${ADMIN_ID}"

# ── Step 3: Create or get team ────────────────────────────────────
echo ""
echo "=== Step 3: Team: ${TEAM_DISPLAY} ==="

team_resp=$(api_get "/teams/name/${TEAM_NAME}" 2>/dev/null || true)
TEAM_ID=$(echo "$team_resp" | jq -r '.id // empty')

if [[ -z "$TEAM_ID" ]]; then
  echo "  Creating team '${TEAM_NAME}'..."
  team_resp=$(api_post /teams \
    "{\"name\": \"${TEAM_NAME}\", \"display_name\": \"${TEAM_DISPLAY}\", \"type\": \"I\"}")
  TEAM_ID=$(echo "$team_resp" | jq -r '.id')
  echo "  Created team: ${TEAM_ID}"
else
  echo "  Team already exists: ${TEAM_ID}"
fi

# ── Step 4: Create bot accounts ───────────────────────────────────
echo ""
echo "=== Step 4: Bot accounts ==="

# Format: "username|display_name|description"
declare -a BOTS=(
  "sokka|Sokka|PM Bot — Linear, project status, backlog, standups. Strategic thinker with sarcastic humor."
  "toph|Toph|DevOps Bot — Deployments, CI/CD, Docker, infrastructure. Blunt and direct."
  "azula|Azula|Security Bot — Code review, vulnerability scanning, audit. Precise and methodical."
  "iroh|Iroh|Creative Bot — Brainstorming, architecture ideation. Wise and philosophical."
  "katara|Katara|Dev Bot — Code, debugging, PR review, fixes. Detail-oriented, high standards."
  "aang|Aang|Coordinator Bot — Routes work, big picture, balance. Bridges all team members."
)

declare -A BOT_IDS
declare -A BOT_TOKENS

for bot_spec in "${BOTS[@]}"; do
  IFS='|' read -r username display_name description <<< "$bot_spec"

  echo "  Bot: @${username}..."

  # Check if bot already exists
  existing=$(api_get "/users/username/${username}" 2>/dev/null || true)
  existing_id=$(echo "$existing" | jq -r '.id // empty')

  if [[ -n "$existing_id" ]]; then
    echo "    Already exists: ${existing_id}"
    BOT_IDS["$username"]="$existing_id"
  else
    # Create bot account
    bot_resp=$(api_post /bots \
      "{\"username\": \"${username}\", \"display_name\": \"${display_name}\", \"description\": \"${description}\"}")
    bot_id=$(echo "$bot_resp" | jq -r '.user_id // .id // empty')
    if [[ -z "$bot_id" ]]; then
      echo "    ERROR: Failed to create bot @${username}: $(echo "$bot_resp" | jq -r '.message // .')" >&2
      continue
    fi
    BOT_IDS["$username"]="$bot_id"
    echo "    Created: ${bot_id}"
  fi

  # Generate or retrieve token
  # Check for existing tokens first
  existing_tokens=$(mm_api GET "/users/${BOT_IDS[$username]}/tokens" 2>/dev/null || true)
  token_count=$(echo "$existing_tokens" | jq 'length // 0' 2>/dev/null || echo 0)

  if [[ "$token_count" -gt 0 ]]; then
    echo "    Token already exists (use 'Revoke and regenerate' in Mattermost if needed)"
    echo "    NOTE: Cannot retrieve existing token value — check Mattermost System Console"
    BOT_TOKENS["$username"]="<existing-token-see-mattermost-console>"
  else
    token_resp=$(api_post "/users/${BOT_IDS[$username]}/tokens" \
      "{\"description\": \"ZeroClaw bot token for @${username}\"}")
    token_val=$(echo "$token_resp" | jq -r '.token // empty')
    if [[ -z "$token_val" ]]; then
      echo "    WARNING: Could not generate token for @${username}"
      BOT_TOKENS["$username"]="<generate-manually>"
    else
      BOT_TOKENS["$username"]="$token_val"
      echo "    Token generated"
    fi
  fi

  # Add bot to team
  team_member_check=$(mm_api GET "/teams/${TEAM_ID}/members/${BOT_IDS[$username]}" 2>/dev/null || true)
  if echo "$team_member_check" | jq -e '.user_id' &>/dev/null; then
    echo "    Already a team member"
  else
    api_post "/teams/${TEAM_ID}/members" \
      "{\"team_id\": \"${TEAM_ID}\", \"user_id\": \"${BOT_IDS[$username]}\"}" >/dev/null
    echo "    Added to team"
  fi
done

# ── Step 5: Create channels ───────────────────────────────────────
echo ""
echo "=== Step 5: Channels ==="

# Format: "name|display_name|purpose|type"  (type: O=public, P=private)
declare -a CHANNELS=(
  "general|General|Open discussion for the whole team|O"
  "alerts|Alerts|Webhook-fed alerts from GitHub, Vercel, Supabase, Upstash|O"
  "standup|Standup|Daily coordination and async standups|O"
  "decisions|Decisions|Important decisions logged here for reference|O"
  "zc-general|ZC General|zeroclaw repo — general discussion|O"
  "zc-ops|ZC Ops|zeroclaw repo — operations, deployments, infra|O"
)

declare -A CHANNEL_IDS

for ch_spec in "${CHANNELS[@]}"; do
  IFS='|' read -r name display_name purpose type <<< "$ch_spec"

  existing_ch=$(api_get "/teams/${TEAM_ID}/channels/name/${name}" 2>/dev/null || true)
  ch_id=$(echo "$existing_ch" | jq -r '.id // empty')

  if [[ -n "$ch_id" ]]; then
    echo "  #${name}: already exists (${ch_id})"
  else
    ch_resp=$(api_post /channels \
      "{\"team_id\": \"${TEAM_ID}\", \"name\": \"${name}\", \"display_name\": \"${display_name}\", \"purpose\": \"${purpose}\", \"type\": \"${type}\"}")
    ch_id=$(echo "$ch_resp" | jq -r '.id // empty')
    if [[ -z "$ch_id" ]]; then
      echo "  ERROR: Failed to create #${name}: $(echo "$ch_resp" | jq -r '.message // .')" >&2
      continue
    fi
    echo "  #${name}: created (${ch_id})"
  fi
  CHANNEL_IDS["$name"]="$ch_id"
done

# ── Step 6: Add bots to channels ─────────────────────────────────
echo ""
echo "=== Step 6: Channel membership ==="

# All bots join all channels
for ch_name in "${!CHANNEL_IDS[@]}"; do
  ch_id="${CHANNEL_IDS[$ch_name]}"
  for bot_name in "${!BOT_IDS[@]}"; do
    bot_id="${BOT_IDS[$bot_name]}"

    # Check membership
    member_check=$(mm_api GET "/channels/${ch_id}/members/${bot_id}" 2>/dev/null || true)
    if echo "$member_check" | jq -e '.user_id' &>/dev/null; then
      : # already a member, skip
    else
      mm_api POST "/channels/${ch_id}/members" \
        "{\"user_id\": \"${bot_id}\"}" >/dev/null 2>/dev/null || true
    fi
  done
  echo "  #${ch_name}: all bots added"
done

# ── Step 7: Print summary and token output ────────────────────────
echo ""
echo "======================================================================"
echo "  ZeroClaw HQ provisioning complete"
echo "======================================================================"
echo ""
echo "Team:     ${TEAM_DISPLAY} (${TEAM_ID})"
echo "URL:      ${MM_URL}"
echo ""
echo "Channels created:"
for ch_name in general alerts standup decisions zc-general zc-ops; do
  ch_id="${CHANNEL_IDS[$ch_name]:-<unknown>}"
  echo "  #${ch_name}: ${ch_id}"
done

echo ""
echo "Bot tokens (copy to .envrc.mattermost):"
echo ""
echo "# ── Mattermost Bot Tokens ──"
for bot_name in sokka toph azula iroh katara aang; do
  token="${BOT_TOKENS[$bot_name]:-<not-generated>}"
  upper=$(echo "$bot_name" | tr '[:lower:]' '[:upper:]')
  echo "export MM_TOKEN_${upper}=${token}"
done

echo ""
echo "Next steps:"
echo "  1. Copy the tokens above into .envrc.mattermost"
echo "  2. Run: direnv allow"
echo "  3. Start ZeroClaw with config/config.mattermost.example.toml as a template"
echo ""
