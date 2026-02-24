# Implementation Plan: Issue #14 — Stand Up Mattermost Instance

> **Issue**: [#14](https://github.com/cmbays/zeroclaw/issues/14) — infra: stand up Mattermost instance (Docker Compose)
> **Epic**: #13 — Mattermost Hub
> **Phase**: P0 Foundation (task 1 of 5)
> **Date**: 2026-02-23
> **Branch**: `feat/14-mattermost-infra` (from `dev`)

## Goal

Stand up a self-hosted Mattermost instance on macOS (Apple Silicon) via Docker Compose. Create the team, bot accounts, and channel structure needed for the ZeroClaw multi-agent hub. Verify bots work end-to-end. Document the setup so it's reproducible.

## Prerequisites

- Docker Desktop installed with Rosetta emulation enabled (Apple Silicon)
- `direnv` configured (per global CLAUDE.md)
- Mattermost REST API v4 docs available at `https://api.mattermost.com/`

## Deliverables

| # | File | Purpose |
|---|------|---------|
| 1 | `infra/mattermost/docker-compose.yml` | Mattermost + Postgres stack |
| 2 | `infra/mattermost/.env.example` | Template for environment variables |
| 3 | `scripts/mattermost-setup.sh` | Idempotent setup script (team, bots, channels) |
| 4 | `config/config.mattermost.example.toml` | ZeroClaw config template with all 6 bot channel sections |
| 5 | `.envrc.mattermost.example` | direnv template for bot tokens |
| 6 | `docs/planning/spike-mattermost-hub.md` | Cherry-pick from `spike/mattermost-hub` branch |

## Non-Deliverables

- ZeroClaw code changes (no `src/` modifications — that's issues #15-#18)
- WebSocket upgrade (issue #20)
- Bot identity/persona files (issue #19)
- Webhook transformer (issue #24)
- Claude Code MCP bridge setup (separate task)

---

## Step-by-Step Plan

### Step 1: Docker Compose File

**File**: `infra/mattermost/docker-compose.yml`

Create a Compose file with two services:

**postgres**:
- Image: `postgres:16-alpine` (ARM64-native)
- Volume: `mattermost-pgdata` (persistent)
- Environment: `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` from `.env`
- Healthcheck: `pg_isready`
- No host port exposure (internal only)

**mattermost**:
- Image: `mattermost/mattermost-enterprise-edition:latest` (free tier features, supports ARM64 via Rosetta)
  - Alternative: `mattermost/mattermost-team-edition:latest` if enterprise image has issues
- Depends on: postgres (healthy)
- Volume: `mattermost-data` for `config/`, `data/`, `logs/`, `plugins/`
- Port: `8065:8065` (HTTP)
- Environment variables for DB connection string, site URL, bot settings
- Key settings to enable via environment:
  - `MM_SERVICESETTINGS_SITEURL=http://localhost:8065`
  - `MM_SERVICESETTINGS_ENABLEBOTACCOUNTCREATION=true`
  - `MM_SERVICESETTINGS_ENABLEUSERACCESSTOKENS=true`
  - `MM_SERVICESETTINGS_ENABLEPOSTUSERNAMEOVERRIDE=true`
  - `MM_SERVICESETTINGS_ENABLEPOSTICONOVERRIDE=true`
  - `MM_TEAMSETTINGS_ENABLEOPENSERVER=false`
  - `MM_SERVICESETTINGS_ENABLEINCOMINGWEBHOOKS=true`
- Healthcheck: `curl -f http://localhost:8065/api/v4/system/ping`

**Networks**: `mattermost-net` (bridge, internal communication)

**Volumes**: `mattermost-pgdata`, `mattermost-data`

**Validation**: `docker compose up -d` → Mattermost accessible at `http://localhost:8065`.

### Step 2: Environment Template

**File**: `infra/mattermost/.env.example`

```
# Postgres
POSTGRES_USER=mmuser
POSTGRES_PASSWORD=CHANGE_ME_mmpass
POSTGRES_DB=mattermost

# Mattermost admin (created on first boot)
MM_ADMIN_USERNAME=admin
MM_ADMIN_PASSWORD=CHANGE_ME_admin_pass
MM_ADMIN_EMAIL=admin@zeroclaw.local
```

**Note**: Actual `.env` is gitignored. Copy from example and fill in real values.

### Step 3: Setup Script

**File**: `scripts/mattermost-setup.sh`

Bash script that automates post-boot configuration via Mattermost REST API v4. Must be **idempotent** (safe to re-run).

**Inputs** (from environment or arguments):
- `MM_URL` (default: `http://localhost:8065`)
- `MM_ADMIN_USERNAME` / `MM_ADMIN_PASSWORD` (for API auth)

**Actions in order**:

1. **Wait for readiness**: Poll `/api/v4/system/ping` until `{"status":"OK"}`.

2. **Authenticate**: `POST /api/v4/users/login` with admin credentials → extract auth token from response header.

3. **Create team** "ZeroClaw HQ":
   - `POST /api/v4/teams` with `name=zeroclaw-hq`, `display_name=ZeroClaw HQ`, `type=I` (invite-only)
   - Idempotent: check if team exists first via `GET /api/v4/teams/name/zeroclaw-hq`

4. **Create bot accounts** (6 bots):

   | Username | Display Name | Description |
   |----------|-------------|-------------|
   | `sokka` | Sokka | PM — Strategic planning, Linear, backlog |
   | `katara` | Katara | Dev — Code, debugging, PR review, fixes |
   | `toph` | Toph | DevOps — Deployments, CI/CD, infrastructure |
   | `azula` | Azula | Security — Code review, vulnerability scanning |
   | `iroh` | Iroh | Creative — Brainstorming, architecture ideation |
   | `aang` | Aang | Coordinator — Routes work, big picture |

   For each bot:
   - `POST /api/v4/bots` with `username`, `display_name`, `description`
   - Idempotent: check via `GET /api/v4/bots` (filter by username)
   - `POST /api/v4/users/{bot_user_id}/tokens` to create personal access token
   - **Output tokens to stdout** (one-time, not stored in script)
   - Add bot to team: `POST /api/v4/teams/{team_id}/members` with bot's `user_id`

5. **Create channels**:

   | Name | Display Name | Type | Purpose |
   |------|-------------|------|---------|
   | `general` | General | O (public) | Open discussion, all bots |
   | `alerts` | Alerts | O | Webhook-fed notifications |
   | `standup` | Standup | O | Daily coordination |
   | `decisions` | Decisions | O | Important decisions log |
   | `zc-general` | ZC General | O | zeroclaw repo general |
   | `zc-ops` | ZC Ops | O | zeroclaw repo operations |

   For each channel:
   - `POST /api/v4/channels` with `team_id`, `name`, `display_name`, `type`
   - Idempotent: check via `GET /api/v4/teams/{team_id}/channels/name/{name}`
   - Add all bots to each channel: `POST /api/v4/channels/{channel_id}/members`

6. **Print summary**: Table of bot usernames + token hints (first/last 4 chars) for verification.

**Error handling**: Fail fast on auth failure or missing team. Warn and continue on duplicate bots/channels (idempotent).

### Step 4: ZeroClaw Config Template

**File**: `config/config.mattermost.example.toml`

Template showing how to configure ZeroClaw with 6 Mattermost channel instances (one per bot). Based on existing `docs/mattermost-setup.md` config format.

```toml
# Provider — shared Ollama instance
[provider]
type = "ollama"
api_key = "http://localhost:11434"
model = "qwen3:14b"

# Each bot gets its own [channels_config.mattermost_<name>] section
# with a unique bot_token and listen channel.

[channels_config.mattermost_sokka]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_SOKKA_TOKEN}"
channel_id = "<zc-general-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true

[channels_config.mattermost_katara]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_KATARA_TOKEN}"
channel_id = "<zc-general-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true

[channels_config.mattermost_toph]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_TOPH_TOKEN}"
channel_id = "<zc-ops-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true

[channels_config.mattermost_azula]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_AZULA_TOKEN}"
channel_id = "<zc-general-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true

[channels_config.mattermost_iroh]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_IROH_TOKEN}"
channel_id = "<general-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true

[channels_config.mattermost_aang]
url = "http://localhost:8065"
bot_token = "${MATTERMOST_AANG_TOKEN}"
channel_id = "<general-channel-id>"
allowed_users = ["*"]
thread_replies = true
mention_only = true
```

**Note**: Channel IDs are filled in after running the setup script. The setup script should print them.

### Step 5: direnv Template

**File**: `.envrc.mattermost.example`

```bash
# Mattermost bot tokens (from scripts/mattermost-setup.sh output)
export MATTERMOST_SOKKA_TOKEN="<token>"
export MATTERMOST_KATARA_TOKEN="<token>"
export MATTERMOST_TOPH_TOKEN="<token>"
export MATTERMOST_AZULA_TOKEN="<token>"
export MATTERMOST_IROH_TOKEN="<token>"
export MATTERMOST_AANG_TOKEN="<token>"

# Mattermost admin (for setup script only)
export MM_ADMIN_USERNAME="admin"
export MM_ADMIN_PASSWORD="<password>"
```

### Step 6: Cherry-Pick Spike Doc

Cherry-pick the spike document from `spike/mattermost-hub` branch into the feature branch so planning context is available alongside the implementation.

```bash
git cherry-pick spike/mattermost-hub --no-commit
git add docs/planning/spike-mattermost-hub.md
```

### Step 7: Gitignore Updates

Ensure these patterns are in `.gitignore`:

```
infra/mattermost/.env
.envrc.mattermost
config/config.mattermost.toml
```

---

## Acceptance Criteria (from issue)

| # | Criterion | Verification |
|---|-----------|--------------|
| 1 | Mattermost accessible at localhost:8065 | `curl -s http://localhost:8065/api/v4/system/ping` returns `{"status":"OK"}` |
| 2 | All 6 bot accounts created with ATLA names/bios | `curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8065/api/v4/bots` lists all 6 |
| 3 | Bots can send in channels | `curl -X POST .../api/v4/posts -d '{"channel_id":"...","message":"test"}' -H "Authorization: Bearer $BOT_TOKEN"` succeeds |
| 4 | Bots can receive in channels | Post a message mentioning `@sokka` → verify via `GET /api/v4/channels/{id}/posts` |
| 5 | DM support works | `POST /api/v4/channels/direct` between user and bot, then send/receive |
| 6 | Setup is reproducible | Fresh `docker compose up` + `scripts/mattermost-setup.sh` on clean Docker produces working state |

## Verification Commands

```bash
# 1. Start the stack
cd infra/mattermost && docker compose up -d

# 2. Wait for healthy
docker compose ps  # both services "healthy"

# 3. Run setup
./scripts/mattermost-setup.sh

# 4. Verify bots
curl -s http://localhost:8065/api/v4/users/me \
  -H "Authorization: Bearer $MATTERMOST_SOKKA_TOKEN" | jq .username
# Expected: "sokka"

# 5. Verify bot can post
curl -s -X POST http://localhost:8065/api/v4/posts \
  -H "Authorization: Bearer $MATTERMOST_SOKKA_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"<channel_id>","message":"Sokka reporting for duty!"}' | jq .id
# Expected: returns post ID

# 6. Teardown
cd infra/mattermost && docker compose down -v  # -v removes volumes for clean reset
```

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| ARM64 image issues (Rosetta) | Low | Docker Desktop Rosetta is stable; fallback to `--platform linux/amd64` explicit flag |
| Bot token creation requires admin | N/A | Setup script uses admin auth; tokens output once then stored in direnv |
| Mattermost free tier limits | None | Team Edition has no bot/channel limits for self-hosted |
| Port 8065 conflict | Low | Configurable via `.env` (`MM_PORT`) |
| Postgres data loss on `docker compose down -v` | Medium | Document `-v` flag danger; default `down` preserves volumes |

## Rollback

All changes are additive new files. Rollback = revert the commit or delete the branch. No upstream files modified.

## Dependencies

- **Blocks**: #15 (multi-mode config), #16 (per-mode runtime), #17 (ask-user tool), #18 (Sokka identity)
- **Blocked by**: Nothing — this is the first P0 task

## Estimated Scope

- **Size**: S/M
- **New files**: 6
- **Modified files**: 1 (`.gitignore`)
- **Upstream files modified**: 0
- **Risk tier**: Low (infra/docs only, no `src/` changes)
