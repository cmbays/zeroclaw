# ZeroClaw Fork — Decision Log

> Synthesized from pipeline `20260221-nanoclaw-hub` (stages 1-7).
> These decisions were settled during Feature Strategy, Requirements Interrogation,
> Research Spikes, Shaping, Breadboarding, and Implementation Planning.

## Architecture

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Approach** | Fork ZeroClaw (Shape A) | ~1,100 LOC new code vs 3,000-5,000 LOC from scratch. ZeroClaw provides production agent infra (Tool trait, AgentBuilder, SystemPromptBuilder, AIEOS identity, Skill system). Fork surface = 0.3% of codebase. |
| **Runtime** | Rust (ZeroClaw fork) | Inherits ZeroClaw's 394k LOC platform. ~5-10MB binary. |
| **Fork surface** | 3 modified files + 1 new directory | `slack.rs`, `traits.rs`, `schema.rs` modified. `src/modes/` created. Feature flags isolate changes. |

## LLM

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Model** | Qwen 3 14B via Ollama | Native tool calling in Ollama. Multi-turn, vision, reasoning. NOT GLM-4.7 (lacks native tool calling in Ollama). |
| **Provider** | ZeroClaw's OllamaProvider | Already production-quality. Handles ProviderCapabilities, prompt-guided fallback. |
| **Hardware** | M4 16GB | Qwen 3 14B fits (~8-9GB q4_K_M). Fallback to 8B if tight. |

## Slack

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Transport** | Socket Mode (WebSocket) | Replaces ZeroClaw's HTTP polling. `tokio-tungstenite` already in deps. No public URL needed for events. |
| **Identity** | Per-message overrides (`chat:write.customize`) | Mode visual identity (username + icon_emoji) per response. |
| **Block Kit** | JSON blocks in SendMessage | Interactive buttons (confirm/edit/cancel), modal views for issue preview. |
| **Rate limit** | `conversations.replies` = 1 req/min | New Slack apps get restrictive tier. Use event-driven context accumulation, not polling. |

## Linear

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Auth** | OAuth 2.0 | No seat cost, app-attributed actions. |
| **API** | Raw GraphQL over `reqwest` | No Rust Linear SDK exists. ~400 LOC Tool trait impl. |
| **Hierarchy** | Initiatives -> Parent Issues -> Sub-Issues | Maps to PM workflow: initiative breakdown, ticket lifecycle. |

## Modes

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Architecture** | ModeRegistry + AIEOS identity + Skills + AgentBuilder | Each mode = Agent instance with persona, skills, tools, response policy. ~220 LOC glue. |
| **v1 scope** | PM Mode only | Initiative breakdown, ticket lifecycle, channel-project sync, template-aware grooming, wake/sleep. |
| **Config** | TOML (`[modes]` section) | ZeroClaw's native config format. Extends existing schema. |

## Infrastructure

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Persistence** | Slack + Linear ARE the databases | No separate DB. Thin config on Docker volumes. |
| **Webhooks** | Cloudflare Tunnel | ZeroClaw has `[tunnel]` config section. Routes Linear/GitHub inbound HTTP. |
| **Deployment** | Docker Compose | 3 services: zeroclaw binary, Ollama, Cloudflare Tunnel. Local first, portable to VPS. |

## Repository

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Repo** | `cmbays/zeroclaw` (personal fork) | No org needed for solo project. 4ink is the print shop, separate concern. |
| **Branch strategy** | `main` = upstream mirror, `dev` = integration, `feat/*` = feature branches | Keeps clean upstream sync. PRs merge into `dev`. |
| **Upstream sync** | `git fetch upstream && git merge --ff-only upstream/main` then merge into `dev` | Feature flags isolate fork changes, most upstream merges are clean. |

## tool_allowlist upstream evaluation (2026-02-28)

**Issue**: #66 — Should `tool_allowlist` be contributed upstream, replaced by an upstream equivalent, or kept as-is?

### What the fork has

`Config.tool_allowlist: Vec<String>` (top-level, `src/config/schema.rs`) + `apply_tool_allowlist()` (`src/agent/agent.rs`).

Behaviour: if non-empty, the tool registry is filtered *at agent build time* — the model only sees the listed tools.
Implementation: 16 lines of logic, 4 unit tests. All 6 bot configs use it to define per-bot capability profiles.

### What upstream has (audited 2026-02-28)

| Upstream mechanism | Location | Purpose |
|--------------------|----------|---------|
| `AutonomyConfig.auto_approve` / `always_ask` | `[autonomy]` | **Approval gates** — controls whether a human must confirm a tool call. Does NOT filter which tools are presented to the model. |
| `AutonomyConfig.non_cli_excluded_tools` | `[autonomy]` | Excludes tools when running as a daemon/service. Runtime-mode exclusion, not per-bot. |
| `SubAgentConfig.allowed_tools` | `[[agents]]` | Allowlist for sub-agents in agentic mode. Only applies to spawned sub-agents, not the top-level process agent. |
| `SecurityRoleConfig.allowed_tools` / `denied_tools` | `[security.roles]` | RBAC per-role tool access, enforced at *execution time* via `RoleRegistry.resolve_tool_access()`. Not wired to tool registration; the model still sees all tools. Requires role definitions + user-role assignment. |
| `AgentConfig` | `[agent]` | Behavioural settings (iterations, history, dispatcher). No tool restriction field. |

**Gap confirmed**: upstream has no simple per-bot build-time tool filter. The security-roles path is structurally different (runtime RBAC vs. registration-time allowlist) and operationally heavier (requires role config per user, not per bot process).

### Decision: **Keep as-is — defer upstream contribution**

Rationale:

1. **No upstream equivalent.** The closest upstream mechanism (`security.roles`) operates at execution time and targets user permissions, not bot capability profiles. It cannot replace `tool_allowlist` without significant operational overhead.

2. **Feature works correctly.** No bugs, no performance concerns, zero fork-specific dependencies.

3. **Contribution barrier is process, not technical.** Upstream PR template requires a Linear issue key (`RMN-XXX`) marked as required. All recent merged PRs are from the core maintainer team. A clean external contribution is possible but requires opening a Linear issue with the upstream team first — that step hasn't happened.

4. **Better upstream placement would be `AgentConfig`, not top-level `Config`.** If contributed, the field belongs under `[agent]` alongside `max_tool_iterations`, `parallel_tools`, etc. But that refactor is a breaking config change for all 6 bot configs (move `tool_allowlist = [...]` from root → `[agent]` section). This refactor cost isn't justified until upstream confirms they want the feature.

5. **Low sync risk at current placement.** The field is appended at the tail of `Config` defaults, minimally invasive to upstream syncs.

### Defer until

Open a GitHub issue on `zeroclaw-labs/zeroclaw` proposing `AgentConfig.tool_allowlist`. If maintainer confirms interest:
- Refactor: move `Config.tool_allowlist` → `AgentConfig.tool_allowlist` in `schema.rs` + `agent.rs`
- Update all 6 bot configs: move `tool_allowlist = [...]` from root into `[agent]` section
- Add `docs/config-reference.md` entry (upstream requirement for config additions)
- Open upstream PR with one integration test using their `DummyProvider` mock pattern

### Rejected alternatives

| Alternative | Why rejected |
|-------------|--------------|
| Migrate to `security.roles` | RBAC at execution time ≠ build-time capability profiles. Requires role definitions per user. Operationally heavier, semantically different. |
| Immediate upstream contribution | Requires Linear issue key + Track B review + docs update. Premature before maintainer interest is confirmed. |
| Move to `AgentConfig` now | Breaking config change across 6 bot configs with no upstream payoff yet. |

## Rejected Alternatives

| Rejected | Why |
|----------|-----|
| **Shape B: Node.js from scratch** | 3,000-5,000 LOC unbounded complexity vs 1,100 LOC bounded. Reinventing agent infra ZeroClaw already provides. |
| **Shape C: Node.js + ZeroClaw patterns** | 2,000-3,000 LOC. Better than B but still rebuilds what the fork inherits for free. |
| **GLM-4.7** | No native tool calling in Ollama. Qwen 3 14B is superior for this use case. |
| **NanoClaw name** | Project is a ZeroClaw fork. Keeping the name acknowledges lineage. |
| **GitHub org** | Unnecessary overhead for solo project. |
