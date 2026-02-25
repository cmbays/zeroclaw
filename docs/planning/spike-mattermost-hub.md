# Spike: Mattermost Hub — ZeroClaw Multi-Agent Team Platform

> Spike for issue #12. Date: 2026-02-23.

## Problem Statement

The current ZeroClaw fork uses Slack with a single bot identity and clunky `@ZeroClaw [pm]` mode syntax. Small local LLMs (4B) choke on 24+ tools and vague requests. The agent never asks clarifying questions — it guesses or spirals. We need a better foundation for a multi-agent team.

## Decision: Mattermost

**Chosen platform: Mattermost (self-hosted)**

### Why Not Slack

- 1 bot per app install — can't have natural `@pm-bot` / `@devops-bot` @mentions
- Rate limits: `conversations.replies` = 1 req/min for new apps
- Free tier caps app installs (~10)
- Per-message identity (`chat:write.customize`) is visual only, still one @mention

### Why Mattermost

- **Unlimited free bot accounts** (don't count toward user limits)
- **No rate limits** (self-hosted, we control the server)
- **Thread support** (Collapsed Reply Threads, `root_id` threading)
- **WebSocket API** for real-time events (replaces polling)
- **DM support** for bots (both directions)
- **Official MCP server** (Claude Code can read/post to channels)
- **Incoming webhooks** with per-message username/icon override
- **Bot @mentions trigger notifications** via REST API
- **GitHub official plugin** (`mattermost-plugin-github`)
- **ZeroClaw already has Mattermost channel** (918 LOC, tested, documented)

### Trade-offs Accepted

- No Slack Block Kit (interactive buttons/modals) — text Q&A is primary UX
- ARM64 Docker image needs Rosetta or community builds
- Sidebar categories are per-user (cosmetic, not a blocker for solo dev)
- No native Linear/Vercel/Supabase plugins — we build custom tools anyway

## Architecture

### Single Process, Multiple Bots

```
┌──────────────────────────────────────────────────────┐
│                    MATTERMOST                         │
│                                                      │
│  @sokka (PM)    @toph (DevOps)   @azula (Security)   │
│  @iroh (Creative) @katara (Dev)  @aang (Coordinator) │
└────┬──────────────┬────────────────┬─────────────────┘
     │              │                │
     ▼              ▼                ▼
┌──────────────────────────────────────────────────────┐
│              ZEROCLAW (single process)                │
│                                                      │
│  MattermostChannel  MattermostChannel  Mattermost... │
│  (token: sokka)     (token: toph)     (token: azula) │
│       │                  │                 │         │
│       ▼                  ▼                 ▼         │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────┐  │
│  │ PM Mode     │  │ DevOps Mode  │  │ Security   │  │
│  │ tools: 5    │  │ tools: 6     │  │ tools: 5   │  │
│  │ temp: 0.1   │  │ temp: 0.3    │  │ temp: 0.1  │  │
│  │ persona: ⬆  │  │ persona: ⬆   │  │ persona: ⬆ │  │
│  └─────────────┘  └──────────────┘  └────────────┘  │
│                                                      │
│  Shared: Memory (SQLite), Provider (Ollama)          │
└──────────────────────┬───────────────────────────────┘
                       │
                    OLLAMA (qwen3:14b+)
```

### Bot Team (Avatar: The Last Airbender Theme)

| Role | Character | Tools | Temp | Style |
|------|-----------|-------|------|-------|
| PM | **Sokka** | linear, memory_store, memory_recall, ask_user | 0.1 | Strategic, lists, plans, sarcastic humor |
| DevOps | **Toph** | shell, file_read, http_request, ask_user | 0.3 | Blunt, direct, no sugarcoating |
| Security | **Azula** | content_search, file_read, http_request, ask_user | 0.1 | Precise, methodical, zero tolerance |
| Creative | **Iroh** | memory_store, memory_recall, ask_user | 0.8 | Wise, philosophical, lateral thinking |
| Dev | **Katara** | shell, file_read, file_write, file_edit, ask_user | 0.3 | Detail-oriented, high standards, fixes things |
| Coordinator | **Aang** | delegate, memory_recall, ask_user | 0.5 | Bridges all modes, routes work, sees big picture |

### Channel Structure

```
TEAM: ZeroClaw HQ

Global (cross-repo)
  #general          — All bots, open discussion
  #alerts           — Webhook-fed (GitHub, Vercel, Supabase, Upstash)
  #standup          — Daily coordination
  #decisions        — Important decisions logged here

zeroclaw (repo)
  #zc-general
  #zc-<project>     — Per-project channels as needed
  #zc-ops

project-b (repo)
  #pb-general
  #pb-<project>
  #pb-ops

DMs
  Direct 1:1 with any bot
```

### Key Design Properties

1. **Adding a bot** = config section + Mattermost bot account. No code changes.
2. **Adding a repo** = create channels, add bots. No code changes.
3. **Adding a webhook** = create incoming webhook, point service at it. No code changes.
4. **Upgrading model** = one config line. All bots benefit.
5. **Cross-bot context** = shared Memory backend + shared channel visibility.
6. **Delegation** = @mention another bot in thread. They pick it up naturally.
7. **Human notification** = bot @mentions user via REST API.

### Resource Budget (~10 bots on 64 GB Mac)

| Component | RAM |
|-----------|-----|
| Ollama + qwen3:14b | ~10-12 GB |
| Mattermost + Postgres | ~2-3 GB |
| ZeroClaw (1 process, 10 channels) | ~100 MB |
| **Total** | ~15 GB |

### Agent Registry (in each bot's system prompt)

```markdown
## Team Members
- @sokka (PM): Linear, project status, backlog, standups
- @toph (DevOps): Deployments, CI/CD, Docker, infrastructure
- @azula (Security): Code review, vulnerability scanning, audit
- @iroh (Creative): Brainstorming, architecture ideation
- @katara (Dev): Code, debugging, PR review, fixes
- @aang (Coordinator): Routes work, big picture, balance

When you need human input, @mention @christopher.
When delegating, @mention the appropriate team member in the thread.
```

### Webhook Ownership

Bots own certain webhook sources. Initially via system prompt instructions, later via structured config:

- **Toph** owns: Vercel deploys, CI failures, Upstash alerts
- **Azula** owns: GitHub security advisories, Dependabot
- **Sokka** owns: Linear webhooks, project status changes
- **Katara** owns: GitHub PR/review notifications

### Claude Code Bridge

Community MCP server (`pvev/mattermost-mcp`, MIT license) enables Claude Code to:
- Read channels and threads
- Search across messages
- Post messages and thread replies
- Add reactions

Install: `claude mcp add mattermost -- node /path/to/mattermost-mcp/build/index.js`

## Implementation Phases

### P0 — Foundation
1. Stand up Mattermost (Docker Compose)
2. Multi-mode config schema (`[modes.*]`)
3. Per-mode channel instantiation + runtime context
4. Ask-user tool
5. Sokka (PM) identity for end-to-end testing

### P1 — Core Experience
6. WebSocket upgrade (real-time, DMs)
7. Remaining ATLA identities
8. Agent registry in system prompts
9. Bot profile sync at startup

### P2 — Integration
10. Webhook transformer
11. GitHub plugin
12. Webhook ownership config
13. Channel/bot setup automation

## References

- Issue #12: spike: ask-user tool + model selection
- ZeroClaw Mattermost channel: `src/channels/mattermost.rs` (918 LOC)
- ZeroClaw Mattermost docs: `docs/mattermost-setup.md`
- Fork mode system: `src/modes/` (existing Slack implementation)
- Mattermost MCP: https://github.com/pvev/mattermost-mcp
- Mattermost GitHub plugin: `mattermost-plugin-github`
