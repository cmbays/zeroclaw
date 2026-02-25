# NanoClaw Agentic Development Hub — Requirements Handoff

> Pipeline: `20260221-nanoclaw-hub`
> Phase: Research complete, ready for Shaping
> Date: 2026-02-21
> Linear Project: [NanoClaw Agentic Development Hub](https://linear.app/print-4ink/project/nanoclaw-agentic-development-hub-c3b23ab4f522)

## What NanoClaw Is

An extensible agentic hub where Slack is the human-in-the-loop interface and external tools are connectors. NanoClaw **complements** (not replaces) Linear's native Slack integration by adding conversational intelligence, template-aware grooming, channel lifecycle management, and multi-mode personas.

**Division of labor with other tools:**
- **NanoClaw** (Ollama, always-on, free) = PM layer: grooming, tracking, status, lifecycle
- **Claude Code** (Max subscription, deep work) = BUILD layer: implements tickets
- **Linear** = shared work queue / handoff interface between NanoClaw and Claude
- **Methodology Orchestrator** (separate tool) = pipeline execution, stage-by-stage deep work

## Architecture Summary

```
NanoClaw Core Engine (~500-1000 LOC target)
+-- Mode Layer (first-class concept above skills)
|   +-- PM Mode (v1) = persona + skills + tools + framework
|   +-- DevOps Mode (future)
|   +-- Orchestrator Mode (future)
+-- Skill Layer (composable, some shared across modes)
|   +-- Core Skills: GitHub (shared), Slack operations
|   +-- Connector Skills: Linear, Vercel, Supabase, Claude Code
|   +-- Knowledge Skills: Shape Up, testing methodology, etc.
+-- Wake/Sleep Engine (event-driven)
|   +-- @mention → always respond
|   +-- Discretionary response for non-tagged messages
|   +-- ~1 hour inactivity → sleep
+-- Context Management
    +-- Thread-scoped (finest grain)
    +-- Channel-scoped (per-project, per-mode)
    +-- Global (cross-project, orchestrator)
    +-- Cycle-based lifecycle (active → cooldown → archive)
```

## Settled Decisions

### Platform & Identity
- Single Slack bot app (`@nanoclaw`) with per-message identity overrides (`chat:write.customize`)
- NanoClaw has its own Linear account (OAuth 2.0, no seat cost, app-attributed actions)
- NanoClaw has its own GitHub account (contributor access)
- Slack Socket Mode (no public URL for Slack events)
- Cloudflare Tunnel or ngrok for Linear/GitHub webhooks (inbound HTTP)

### LLM
- **Qwen 3 14B** via Ollama (native tool calling in Ollama, not GLM-4.7 which lacks it)
- Fallback: Qwen 3 8B if 16GB M4 memory is too tight
- Claude Code Max subscription for deep work (separate tool, not routed through NanoClaw)
- Stateless LLM calls: fetch context → build prompt → Ollama → respond

### Modes
- Modes are first-class concepts: persona + skills + tools + methodology/framework
- Skills are composable components within modes, some shared across modes
- Per-message visual differentiation via Slack `username` + `icon_emoji` overrides
- Mode selector: `@nanoclaw [pm]` syntax
- Per-thread mode isolation, multiple modes simultaneously in different threads
- v1 = PM mode only, architecture supports adding more

### Persistence
- **Slack IS the conversation persistence layer** (summaries/logs posted back into channels/threads)
- **Linear/GitHub ARE the structured data layers** (queried via API)
- **NanoClaw's own state is thin**: bot config + per-mode knowledge docs on Docker volumes
- Channel-project mappings: NOT stored locally. Read from platform metadata (Slack channel description links, Linear project links)
- Recovery model: "re-read the thread and catch up" — stateless reconstruction from Slack API
- Cycle-based memory: active cycle = working memory, cooldown → archive, cross-cycle = special case

### Interaction Model
- @mention to wake, bracketed mode selector `[pm]`, conversational after wake-up
- ~1 hour inactivity timeout → sleep
- Semi-passive participation: reads all messages when awake, responds when meaningful
- Confirmation flow for ticket creation: draft → preview → confirm (never auto-file)
- Event-driven context accumulation (build context from real-time Slack events, not thread fetching — 1 req/min rate limit on conversations.replies)

### Deployment
- Docker Compose: Ollama + Node.js + (tunnel service)
- Start local (M4, 16GB), port to VPS later (same compose file)
- Secrets: `.env` file or Docker secrets in v1. Proper secrets manager in v2+.

### Linear Data Model
- **Initiatives** (not epics) → Parent Issues → Sub-Issues
- Sub-issues map to pipeline stage templates
- Linear issue templates fetched fresh via API (no cache, <50 templates)
- NanoClaw uses OAuth (app-attributed, no seat cost, dynamic rate limits)

## v1 Scope: PM Mode Core Offering

### Ships in v1
1. **Initiative → Issue breakdown**: Take initiative context, break into sequenced parent/sub-issues, use Linear templates, groom each with context
2. **Ticket lifecycle management**: Track PRs, recommend closing issues when PRs merge, status checks, surface stale work
3. **Channel-project lifecycle**: Auto-create `#prj-<slug>` on Linear project creation, link bidirectionally, detect manual channels, recommend archive on completion
4. **Template-aware grooming**: Know templates, suggest right one, suggest sequence, question if new template needed
5. **Project chat participation**: Surface context, ask questions, give recommendations
6. **Wake/sleep engine**: Event-driven, @mention wake, discretionary response, timeout sleep
7. **Visual mode indicator**: Per-message username/icon overrides
8. **NanoClaw identity**: OAuth Linear account, GitHub contributor account
9. **Secrets management**: `.env` or Docker secrets for API keys

### Designed-for but NOT built in v1
- DevOps mode, Orchestrator mode, additional modes
- Vercel, Supabase, Claude Code connectors (only Linear + GitHub read in v1)
- Claude routing from NanoClaw
- Seamless restart recovery (v1 = re-read and catch up)
- Auto-archive without approval
- Cross-cycle lookups
- VPS deployment
- Proper secrets manager

## Technical Spike Findings

### LLM (Qwen 3 via Ollama)
- Native tool calling in Ollama (unlike GLM-4.7)
- 14B model: ~8-9GB at q4_K_M, fits on 16GB M4
- 200K context window
- "Interleaved thinking" mode for multi-step reasoning
- Stateless API: send full conversation history each call

### Slack Bolt SDK
- Socket Mode: production-ready, no public URL, WebSocket with auto-reconnect
- Per-message identity: `chat:write.customize` scope, works on free tier
- Block Kit: buttons for confirm/edit/cancel flows
- `reply_broadcast: true` for "also send to channel"
- **GOTCHA**: `conversations.replies` limited to 1 req/min for new non-Marketplace apps (created after May 2025). Mitigate with event-driven context accumulation.
- Minimal scopes: `app_mentions:read`, `channels:read`, `channels:manage`, `chat:write`, `chat:write.customize`, `connections:write`

### Linear API
- OAuth 2.0: no seat cost, app-attributed actions, dynamic rate limits
- Full issue CRUD, project management, initiative hierarchy, cycle queries
- Webhooks: full support but need HTTP endpoint (use tunnel)
- Templates: queryable via GraphQL, programmatic creation needs schema inspection
- GitHub integration: native auto-sync (PR merged → issue transitions) but PR linkage not queryable via Linear GraphQL — need separate GitHub API
- Terminology: Initiatives (not epics), workflow states per team

### Claws Ecosystem Patterns
- **Minimalism is security**: ~500 LOC core, <10 deps, auditable in one sitting
- **Container isolation per mode**: each agent session in own container, blast radius containment
- **Trait-driven design**: every subsystem is a pluggable interface (ZeroClaw pattern)
- **Skill injection for configurability**: `/add-telegram` → AI modifies codebase, not config files
- **SQLite + filesystem IPC**: no message brokers, scale up when you hit limits

## Open Design Items for Shaping

1. Skill definition format (directory structure, manifest)
2. Mode definition format (persona + skills + tools + knowledge bundling)
3. Thread linking UX (when summarizing long threads)
4. Discretionary response logic (prompt engineering vs explicit rules)
5. Channel detection heuristic (how NanoClaw links manually-created channels)
6. Tunnel service choice (ngrok vs Cloudflare Tunnel) and docker-compose integration
7. Ollama model management (assume pre-pulled or auto-pull?)
8. Per-mode knowledge loading (markdown files? embedded? URL?)
9. GitHub PR → Linear issue linking (branch naming convention? parsing?)
10. Concurrent mode activation mechanics (separate LLM calls per mode?)

## Hardware Constraints
- Apple M4, 16GB unified memory, 253GB SSD
- Qwen 3 14B (q4_K_M ~8-9GB) leaves ~7GB for OS + Docker + Node.js
- If too tight, fall back to Qwen 3 8B (~5GB)

## Risk Register

| Risk | Severity | Mitigation |
|------|----------|------------|
| Qwen 3 14B quality for PM reasoning | High | Validate with real initiative-to-issue tasks early in build |
| 16GB memory constraint | Medium | Monitor with `htop`, fall back to 8B model if needed |
| Slack 1 req/min on thread fetching | Medium | Event-driven context accumulation while awake |
| Tunnel reliability for webhooks | Low | Cloudflare Tunnel is stable; fallback to polling |
| Linear free tier 250 issue limit | Low | Monitor, upgrade when needed |
| Scope creep from extensibility vision | Medium | Ruthlessly enforce v1 boundary |
