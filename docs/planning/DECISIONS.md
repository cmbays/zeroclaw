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

## Rejected Alternatives

| Rejected | Why |
|----------|-----|
| **Shape B: Node.js from scratch** | 3,000-5,000 LOC unbounded complexity vs 1,100 LOC bounded. Reinventing agent infra ZeroClaw already provides. |
| **Shape C: Node.js + ZeroClaw patterns** | 2,000-3,000 LOC. Better than B but still rebuilds what the fork inherits for free. |
| **GLM-4.7** | No native tool calling in Ollama. Qwen 3 14B is superior for this use case. |
| **NanoClaw name** | Project is a ZeroClaw fork. Keeping the name acknowledges lineage. |
| **GitHub org** | Unnecessary overhead for solo project. |
