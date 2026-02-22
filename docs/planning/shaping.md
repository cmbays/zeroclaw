---
shaping: true
---

# NanoClaw Agentic Development Hub — Shaping

> Pipeline: `20260221-nanoclaw-hub`
> Date: 2026-02-22
> Status: **Shape A selected** (post A5 spike)

## Requirements (R)

| ID | Requirement | Status |
|----|-------------|--------|
| R0 | Conversational PM hub in Slack that drives Linear work management (v1: PM mode only) | Core goal |
| R1 | **First-class Slack interface** | Must-have |
| R1.1 | Socket Mode (WebSocket, no public URL for Slack events) | Must-have |
| R1.2 | Per-message identity overrides (`chat:write.customize` — username + icon per mode) | Must-have |
| R1.3 | Block Kit (buttons for confirm/edit/cancel flows in ticket creation) | Must-have |
| R1.4 | `reply_broadcast` (thread reply also sent to channel) | Must-have |
| R2 | **Local LLM via Ollama** (Qwen 3 14B, native tool calling, 200K context, ~9GB q4_K_M) | Must-have |
| R3 | **Linear integration** (OAuth 2.0, initiative→issue CRUD, template-aware grooming) | Must-have |
| R4 | **Mode architecture** | Must-have |
| R4.1 | Modes are first-class concepts (persona + skills + tools + framework) | Must-have |
| R4.2 | Per-thread mode isolation (multiple modes active simultaneously in different threads) | Must-have |
| R4.3 | Per-message visual differentiation (username + icon_emoji overrides per mode) | Must-have |
| R5 | **Robust agent infrastructure** | Must-have |
| R5.1 | Agent loop (message → classify → dispatch → tool loop → respond) | Must-have |
| R5.2 | Error recovery (tool failures, LLM timeouts, malformed JSON, model quirks) | Must-have |
| R5.3 | Multi-turn conversation history management | Must-have |
| R6 | **Operational constraints** | Must-have |
| R6.1 | Total system ≤16GB (9GB model + bot + OS + Docker headroom) | Must-have |
| R6.2 | Docker Compose deployment (local M4 first, portable to VPS) | Must-have |
| R6.3 | Tunnel for inbound webhooks (Linear/GitHub → bot) | Must-have |
| R7 | **Extensibility** (trait-driven connectors, config-driven features, centralized registration) | Nice-to-have |
| R8 | **Maintainability** (solo dev + AI-written code, bounded fork/dependency surface, debuggable, testable) | Must-have |

---

## A: Fork ZeroClaw + Extend Slack

Fork the 16.5k-star ZeroClaw Rust platform. Fix Slack's missing features (~400 LOC), add Linear tool (~400 LOC), build mode layer (~300 LOC) on top of the production-hardened agent infrastructure.

| Part | Mechanism | Flag |
|------|-----------|:----:|
| **A1** | Fork `zeroclaw-labs/zeroclaw`, strip unused channels/hardware via Cargo feature flags to reduce binary size | |
| **A2** | Rewrite `slack.rs` listen loop: replace HTTP polling (`conversations.history` 3s interval) with `tokio-tungstenite` Socket Mode WebSocket | |
| **A3** | Extend `SendMessage` struct + `send()`: add Block Kit JSON payload, identity override fields (`username`, `icon_emoji`), `reply_broadcast` flag | |
| **A4** | Implement Linear Tool (Tool trait): raw GraphQL over `reqwest` for issue CRUD, initiative hierarchy, template queries. No Rust Linear SDK exists. (~400 LOC) | |
| **A5** | Mode layer via ZeroClaw's existing primitives: `ModeRegistry` maps mode name → `AgentBuilder` config (AIEOS identity + skills + tools + prompt sections). Per-thread state `HashMap<ThreadId, Agent>`. Mode activation parser for `@nanoclaw [pm]`. Visual identity routing adds `username`/`icon_emoji` to `SendMessage`. ~220 LOC. (Spike: `spike-a5-mode-layer.md`) | |
| **A6** | Build wake/sleep engine: Slack event subscription, @mention detection, discretionary response logic, ~1hr inactivity timeout with graceful sleep message | |
| **A7** | Docker Compose: compiled ZeroClaw binary + Ollama container + Cloudflare Tunnel sidecar | |
| **A8** | Config TOML: modes, skills, tools, Slack credentials, Linear OAuth, Ollama endpoint | |

---

## B: Node.js from Scratch

Build NanoClaw from the ground up in Node.js/TypeScript. `@slack/bolt` for Slack, `@linear/sdk` for Linear, Ollama REST for LLM. Full control over architecture, but all agent infrastructure designed and built from zero.

| Part | Mechanism | Flag |
|------|-----------|:----:|
| **B1** | Slack connector: `@slack/bolt` with Socket Mode, Block Kit action handlers, per-message identity overrides via `chat.postMessage` params | |
| **B2** | Ollama connector: `fetch()` to `/api/chat` with tool definitions in request body, parse structured tool call responses, handle model-specific response formats | ⚠️ |
| **B3** | Linear connector: `@linear/sdk` OAuth 2.0 client, issue CRUD methods, initiative hierarchy traversal, template query helpers | |
| **B4** | Agent loop: receive Slack event → classify intent → select mode → execute tool calling loop (LLM proposes tool → execute → feed result back → repeat until done) → compose and send response | ⚠️ |
| **B5** | Error recovery: retry with backoff on LLM timeouts, graceful fallback on malformed tool calls, handle nested tool call wrappers, model-prefixed function names, partial JSON | ⚠️ |
| **B6** | Mode layer: TypeScript mode definitions (persona + skills[] + tools[] + knowledge[]), per-thread mode state Map, visual identity routing to Slack identity overrides | |
| **B7** | Wake/sleep engine: Slack `app_mention` event triggers wake, background timer for inactivity detection, discretionary response classifier (prompt-based), graceful sleep notification | |
| **B8** | Tool/skill registry: centralized `registerTool()` / `registerSkill()` functions, YAML config maps mode → enabled tools/skills | |
| **B9** | Docker Compose: Node.js app container + Ollama container + Cloudflare Tunnel sidecar + shared network | |

---

## C: Thin Node.js Wrapper + ZeroClaw Patterns

Node.js/TypeScript runtime with `@slack/bolt` and `@linear/sdk`, but architecture directly transplanted from ZeroClaw's proven patterns. Familiar language, battle-tested design, no fork maintenance.

| Part | Mechanism | Flag |
|------|-----------|:----:|
| **C1** | **Slack connector**: `@slack/bolt` with Socket Mode, Block Kit action handlers, per-message identity overrides via `chat.postMessage` params | |
| **C2** | **Ollama connector**: `fetch()` to `/api/chat` with tools. Transplant ZeroClaw's `ProviderCapabilities` pattern (declare model features, adapt calling strategy) + prompt-guided fallback (inject tools as XML in system prompt when native calling unavailable) | |
| **C3** | **Linear connector**: `@linear/sdk` OAuth 2.0 client, issue CRUD, initiative hierarchy, template queries | |
| **C4** | **Agent loop**: Transplant ZeroClaw's message → classify → dispatch → tool loop → respond into TypeScript. Includes multi-turn history management (`convert_messages` equivalent), error recovery for nested tool calls, prefixed function names, and model-specific quirks (patterns from `ollama.rs:582-671`) | |
| **C5** | **Mode layer**: TypeScript mode definitions (`{ persona, skills[], tools[], knowledge[], visualIdentity }`), per-thread mode state Map, mode activation via `@nanoclaw [pm]` syntax, visual identity routing | |
| **C6** | **Wake/sleep engine**: Slack `app_mention` event triggers wake, event-driven context accumulation (not thread fetching — avoids 1 req/min limit), discretionary response classifier, ~1hr inactivity timeout | |
| **C7** | **Tool/skill registry**: Transplant ZeroClaw's centralized registration pattern (`all_tools_with_runtime` → `registerAllTools()`). Single function registers all tools, YAML config enables/disables per mode. | |
| **C8** | **Security policy**: Transplant construction-time injection pattern — tools receive security constraints (allowed operations, rate limits, scope boundaries) when instantiated, not at runtime | |
| **C9** | **Docker Compose**: Node.js app + Ollama container + Cloudflare Tunnel sidecar + healthcheck verifying model availability | |

---

## Fit Check

| Req | Requirement | Status | A | B | C |
|-----|-------------|--------|---|---|---|
| R0 | Conversational PM hub in Slack driving Linear work management | Core goal | ✅ | ✅ | ✅ |
| R1 | First-class Slack (Socket Mode, identity, Block Kit, reply_broadcast) | Must-have | ✅ | ✅ | ✅ |
| R2 | Local LLM (Qwen 3 14B, Ollama, native tool calling) | Must-have | ✅ | ✅ | ✅ |
| R3 | Linear integration (OAuth, issue CRUD, templates) | Must-have | ✅ | ✅ | ✅ |
| R4 | Mode architecture (first-class modes, per-thread isolation, visual differentiation) | Must-have | ✅ | ✅ | ✅ |
| R5 | Robust agent infrastructure (agent loop, error recovery, multi-turn) | Must-have | ✅ | ❌ | ✅ |
| R6 | Operational constraints (≤16GB, Docker Compose, tunnel) | Must-have | ✅ | ✅ | ✅ |
| R7 | Extensibility (trait-driven, config-driven, centralized registry) | Nice-to-have | ✅ | ✅ | ✅ |
| R8 | Maintainability (solo dev + AI-written code, bounded fork/dependency surface, debuggable, testable) | Must-have | ✅ | ✅ | ✅ |

**Notes:**

- A passes R4 (updated): A5 spike (`spike-a5-mode-layer.md`) confirmed modes map cleanly to ZeroClaw's existing primitives — AIEOS identity (persona), Skill system (capability bundles), AgentBuilder (per-mode tool selection), SystemPromptBuilder (composable prompt sections). ~220 LOC of glue code. No architectural gaps.
- A passes R8 (updated): R8 redefined — expertise is not the constraint (Claude writes the code, Christopher learns). Fork maintenance surface is bounded: 3 modified files (0.3% of 394k LOC), 1 new directory. Largest change (Slack Socket Mode) is a clean replacement, not interleaving. Feature flags isolate changes. 3,214 existing tests validate untouched code.
- B fails R5: Agent loop (B4), error recovery (B5), and Ollama edge case handling (B2) are all flagged ⚠️. The ZeroClaw spike revealed that Ollama tool calling involves numerous edge cases — nested tool call wrappers, model-prefixed function names (`tool.shell.execute`), partial JSON in streaming responses, quirky model behaviors — that "build from scratch" would need to rediscover through trial and error.

**Score**: **A = 8/8 must-haves** | B = 7/8 | C = 8/8

---

## Decision: Shape A Selected

**Shape A: Fork ZeroClaw + Extend Slack** — both A and C pass 8/8 must-haves. Shape A is selected because it provides dramatically more for the same cost, and the A5 spike resolved the last flagged unknown.

### Why A over C (tiebreaker analysis)

Both shapes pass all must-haves. The tiebreaker is what you get beyond the requirements:

| Dimension | A: Fork ZeroClaw | C: Node.js + Patterns |
|---|---|---|
| **Agent infrastructure** | Production-hardened, 3,214 tests, battle-tested edge case handling | Transplanted patterns (~500-800 LOC), untested, edge cases rediscovered |
| **Ollama integration** | Best-in-class: native tool calling, multi-turn, vision, reasoning mode, streaming, quirk handling | `fetch()` wrapper reimplementing patterns from `ollama.rs` — same logic, no test coverage |
| **New code to write** | ~1,100 LOC (Slack fix + Linear tool + mode glue) | ~2,000-3,000 LOC (all connectors + agent loop + mode layer) |
| **Memory footprint** | ~5-10MB binary | ~100-200MB Node.js runtime |
| **Existing test coverage** | 3,214 tests across 188 files | Zero — everything written from scratch |
| **Security** | SecurityPolicy, Docker/WASM sandboxing, tool isolation — all built-in | Must be designed and implemented |
| **Extensibility** | 20+ channel traits, 14+ providers, plugin architecture — future modes get free infrastructure | Custom extensibility layer, limited to what we build |
| **Ecosystem** | ZeroClaw community (16.5k stars), upstream improvements flow in | Solo project, all improvements self-funded |

Shape C's advantage was "no fork maintenance." But the A5 spike showed NanoClaw's fork surface is 3 modified files (0.3% of codebase). The maintenance cost is bounded and predictable. Shape A's advantages — production agent loop, 3,214 tests, Ollama provider, security infrastructure, plugin ecosystem — are substantial and would take months to replicate in Shape C.

### Why A over B

Shape B fails R5 (agent infrastructure). The ZeroClaw spike revealed that production agent infrastructure involves numerous edge cases that "build from scratch" would need to rediscover through trial and error. The realistic LOC estimate (3,000-5,000) is 3-5x the initial "500-1,000" estimate.

### The expertise question (settled)

The previous analysis incorrectly weighted "TypeScript expertise" as a differentiator. The working model is: Claude writes code, Christopher learns concepts. Language is not a constraint — understanding architecture is. Shape A's architecture is well-documented (examples for custom tools, channels, providers), and Rust's type system actually makes AI-generated code more reliable (compiler catches errors that TypeScript's type system misses at runtime).

### Fork maintenance (bounded)

- **Files modified**: 3 (slack.rs, traits.rs, schema.rs) + 1 new directory (modes/)
- **Lines touched**: ~1,300 of 394,000 (0.3%)
- **Merge risk**: Slack Socket Mode rewrite is a clean replacement (not interleaving). Config additions are additive. Modes are a new directory.
- **Upstream benefit**: Bug fixes, security patches, new providers, and performance improvements flow in for free. NanoClaw benefits from ZeroClaw's active development rather than maintaining equivalent code solo.

### Resource efficiency comparison

| Shape | Bot memory | Total (w/ 9GB model + OS) | 16GB headroom |
|-------|-----------|--------------------------|---------------|
| **A** | **~5-10MB** | **~10.0GB** | **~6.0GB** |
| B | ~100-200MB | ~10.2GB | ~5.8GB |
| C | ~100-200MB | ~10.2GB | ~5.8GB |

Shape A leaves ~200MB more headroom — meaningful on a 16GB M4 running a 9GB model.

---

## Open Design Items (Resolved Under Shape A)

| # | Item (from requirements handoff) | Resolution |
|---|----------------------------------|------------|
| 1 | Skill definition format | ZeroClaw's native `Tool` trait: `name()`, `description()`, `parameters_schema()`, `execute()`. Register in `all_tools_with_runtime()`. Skill TOML/MD manifests in `workspace/skills/{name}/SKILL.toml`. |
| 2 | Mode definition format | TOML config in `[modes.{name}]` section: `identity_format`, `aieos_path`, `skills_dir`, `visual_identity`, `response_policy`, `tools[]`. AIEOS JSON for persona. Mirrors existing `[agents]` HashMap pattern. |
| 3 | Thread linking UX | Extend `SendMessage` with `reply_broadcast: bool`. Set in Slack `send()` body. Summaries are LLM-generated with mode persona voice (AIEOS linguistics section). |
| 4 | Discretionary response logic | Custom `PromptSection` implementation (`ResponsePolicySection`). Injected into `SystemPromptBuilder` per-mode. Policy text from mode TOML config. |
| 5 | Channel detection heuristic | New Tool (Tool trait): parse Slack channel description for Linear project URL. Parse Linear project description for Slack channel link. Fallback: `#prj-` prefix convention. |
| 6 | Tunnel service | Cloudflare Tunnel (free tier, more stable than ngrok for always-on, auto-reconnect). Docker Compose sidecar container. ZeroClaw's `[tunnel]` config section already exists. |
| 7 | Ollama model management | ZeroClaw already manages Ollama connection. Docker Compose healthcheck: `curl http://ollama:11434/api/tags`. Startup script pulls model if missing. |
| 8 | Per-mode knowledge loading | Markdown files in `workspace/modes/{name}/knowledge/`. Loaded via custom `PromptSection` at mode activation. ZeroClaw's existing workspace + memory system handles file access. |
| 9 | GitHub PR → Linear issue linking | New Tool (Tool trait): parse branch naming convention `{TEAM}-{NUMBER}-description` on GitHub webhook. Link via Linear GraphQL `attachmentCreate` over `reqwest`. |
| 10 | Concurrent mode activation | `HashMap<ThreadId, Agent>` in `ThreadModeState`. Each thread gets its own `Agent` instance (built by `ModeRegistry`). Separate Ollama call per thread. No shared mutable state between Agent instances. |

---

## Decision Points Log

| # | Decision | Method | Rationale |
|---|----------|--------|-----------|
| 1 | R0-R8 defined | Derived from requirements handoff (25+ settled decisions) | Requirements extracted from 5 interrogator batches + 4 research spikes + ZeroClaw validation spike |
| 2 | R1-R6, R8 as Must-have | Assessed against v1 constraints | All are required for a functional v1 PM bot on M4 hardware |
| 3 | R7 as Nice-to-have | Assessed against v1 scope | v1 only exercises PM mode; extensibility is designed-for but not exercised until v2 modes |
| 4 | Shape C initially selected | Fit check (C: 8/8 vs B: 7/8 vs A: 6/8) | Only shape passing all must-haves at the time. A failed R4 (flagged ⚠️) and R8 (expertise-biased definition). |
| 5 | R8 redefined | User feedback | "I didn't know anything about Node.js before we started." Expertise is not the constraint — Claude writes code, Christopher learns. R8 redefined: bounded fork/dependency surface, debuggable, testable. |
| 6 | A5 spike executed | Spike (`spike-a5-mode-layer.md`) | Investigated ZeroClaw's Agent/Identity/Skills primitives. Found modes map cleanly: ~220 LOC of glue. A5 flag resolved. |
| 7 | Shape A selected | Tiebreaker (A: 8/8 vs C: 8/8, B: 7/8) | Both A and C pass all must-haves. A provides dramatically more: production agent loop (3,214 tests), Ollama provider, security infrastructure, plugin ecosystem. Fork surface bounded (0.3% of codebase). |
| 8 | Open design items 1-10 resolved | Resolved under Shape A constraints | Each resolution uses ZeroClaw's native primitives (Tool trait, AIEOS, PromptSection, config schema, workspace system). |

---

## Shape A Parts (Selected Shape — Reference for Breadboarding)

| Part | Mechanism | Flag |
|------|-----------|:----:|
| **A1** | Fork `zeroclaw-labs/zeroclaw`, strip unused channels/hardware via Cargo feature flags to reduce binary size | |
| **A2** | Rewrite `slack.rs` listen loop: replace HTTP polling (`conversations.history` 3s interval) with `tokio-tungstenite` Socket Mode WebSocket | |
| **A3** | Extend `SendMessage` struct + `send()`: add Block Kit JSON payload, identity override fields (`username`, `icon_emoji`), `reply_broadcast` flag | |
| **A4** | Implement Linear Tool (Tool trait): raw GraphQL over `reqwest` for issue CRUD, initiative hierarchy, template queries. No Rust Linear SDK exists. (~400 LOC) | |
| **A5** | Mode layer via ZeroClaw's existing primitives: `ModeRegistry` maps mode name → `AgentBuilder` config (AIEOS identity + skills + tools + prompt sections). Per-thread state `HashMap<ThreadId, Agent>`. Mode activation parser for `@nanoclaw [pm]`. Visual identity routing adds `username`/`icon_emoji` to `SendMessage`. ~220 LOC. (Spike: `spike-a5-mode-layer.md`) | |
| **A6** | Build wake/sleep engine: Slack event subscription, @mention detection, discretionary response logic via `ResponsePolicySection`, ~1hr inactivity timeout with graceful sleep message | |
| **A7** | Docker Compose: compiled ZeroClaw binary + Ollama container + Cloudflare Tunnel sidecar | |
| **A8** | Config TOML: `[modes]` section (per-mode identity + skills + tools + visual identity), Slack credentials, Linear OAuth, Ollama endpoint. Extends ZeroClaw's existing config schema. | |
