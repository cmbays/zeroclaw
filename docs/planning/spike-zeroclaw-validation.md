# ZeroClaw Validation Spike

> Pipeline: `20260221-nanoclaw-hub`
> Date: 2026-02-22
> Decision: **Three competing shapes** passed to Shaping phase (foundation decision is genuinely non-obvious)

## Repository Overview

- **Repo**: github.com/zeroclaw-labs/zeroclaw
- **Stars**: 16,529 | **Forks**: 1,869 | **Open Issues**: 64
- **Language**: Rust (394k lines, 3,214 tests across 188 files)
- **License**: Apache 2.0 + MIT (dual)
- **Last pushed**: 2026-02-22 (daily active development)
- **Description**: "Fast, small, and fully autonomous AI assistant infrastructure"

ZeroClaw is a substantial platform — far beyond the "~500 LOC core" initially expected. It supports 20+ messaging channels, 14+ LLM providers, hardware/IoT peripherals, SQLite/Postgres memory, Docker/WASM sandboxing, and a web UI.

---

## Validation Question 1: Slack Channel Trait

**Verdict: FAILS requirements** (3 of 4 critical features missing)

| Requirement | Status | Evidence |
|---|---|---|
| Socket Mode | **MISSING** | Uses HTTP polling on `conversations.history` with 3-second interval (`slack.rs:225`). Config schema has `app_token: Option<String>` for Socket Mode but it is **completely unused** — not passed to `SlackChannel::new()` (`mod.rs:2684-2691`). |
| Per-message identity overrides (`username`/`icon_emoji`) | **MISSING** | `send()` only sends `channel`, `text`, `thread_ts` (`slack.rs:170-176`). No `username`, `icon_emoji`, or `chat:write.customize` support. |
| Block Kit (buttons, interactive messages) | **MISSING** | Messages are plain text only. No Block Kit structures, no action callbacks. |
| `reply_broadcast` (also send to channel) | **MISSING** | Not in `send()` body. |
| Threading via `thread_ts` | **Present** | Both inbound (`inbound_thread_ts`) and outbound threading work correctly. |
| Multi-channel discovery | **Present** | Auto-discovers all accessible channels when `channel_id` is omitted. |
| User allowlist | **Present** | Granular control with `*` wildcard support. |

**Bottom line**: ZeroClaw's Slack channel is a basic polling integration — adequate for simple chat bots but missing all the advanced features NanoClaw requires. However, these gaps are bounded (~300-400 LOC of fixes), not dealbreakers.

---

## Validation Question 2: Ollama Provider Trait

**Verdict: EXCELLENT** (all requirements met)

| Requirement | Status | Evidence |
|---|---|---|
| Native tool/function calling | **Full support** | `supports_native_tools() = true`, `chat_with_tools()` sends tool definitions and returns structured `ToolCall` objects (`ollama.rs:582-671`). |
| Structured JSON output | **Full support** | Tool call arguments are parsed as JSON, nested/prefixed tool call patterns are handled (`extract_tool_name_and_args`). |
| System prompts | **Full support** | `chat_with_system()` with optional system prompt (`ollama.rs:466-528`). |
| Multi-turn conversation history | **Full support** | `chat_with_history()` with full message array, `convert_messages()` handles assistant/tool/user roles (`ollama.rs:218-313`). |
| Vision/multimodal | **Supported** | `capabilities.vision = true`, inline `[IMAGE:...]` markers parsed to base64 payloads. |
| Reasoning mode ("thinking") | **Supported** | `think` parameter for models with interleaved reasoning (`ollama.rs:25-26`). |
| Cloud routing | **Supported** | `model:cloud` suffix for remote Ollama endpoints with API key auth. |

**Patterns worth stealing**: The `ProviderCapabilities` struct, automatic fallback from native tool calling to prompt-guided (XML tags in system prompt), and the quirky model tool call unwrapping (`tool_call` wrapper, `tool.shell` prefix patterns).

---

## Validation Question 3: Extension Model

**Verdict: GOOD** (clean traits, clear examples, straightforward registration)

### Tool Trait (`src/tools/traits.rs`)

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;
}
```

- 4 methods. Clean interface. `examples/custom_tool.rs` provides a complete `HttpGetTool` example.
- Registration: add to `all_tools_with_runtime()` in `src/tools/mod.rs`.
- Security policy injected at construction time (not the trait level).

### Channel Trait (`src/channels/traits.rs`)

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn send(&self, message: &SendMessage) -> anyhow::Result<()>;
    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()>;
}
```

- 3 required + 9 optional methods (health_check, typing indicators, draft updates, reactions).
- `examples/custom_channel.rs` provides a complete `TelegramChannel` example.
- Registration: add to `create_channels()` in `src/channels/mod.rs`.

### Provider Trait (`src/providers/traits.rs`)

- Well-defined with `ProviderCapabilities` declaration.
- `examples/custom_provider.rs` exists.
- Automatic fallback: providers that don't support native tools get prompt-guided XML injection.

### Adding a Linear Tool

Would require:
1. Implement `Tool` trait (~200-400 lines for Linear GraphQL operations)
2. Register in `all_tools_with_runtime()` (~3 lines)
3. No Linear SDK for Rust — would need raw GraphQL over `reqwest`

**Assessment**: The trait interfaces are excellent and well-documented. Adding a Linear tool would be straightforward. However, the missing Linear SDK for Rust means handcrafting all GraphQL queries.

---

## Validation Question 4: Codebase Quality

**Verdict: HIGH QUALITY** (but very large)

| Metric | Value | Assessment |
|---|---|---|
| Stars | 16,529 | Top 0.1% of Rust projects |
| Tests | 3,214 across 188 files | Excellent coverage |
| Source lines | 394,000 | Far larger than expected |
| Open issues | 64 | Reasonable for project size |
| Last commit | Today (2026-02-22) | Actively maintained |
| Docs | Extensive (channels, config, custom tools, hardware, security) | Well-documented |
| Examples | 4 (custom_tool, custom_channel, custom_provider, custom_memory) | Clear patterns |
| CI | Clippy + tests + fuzzing | Professional |
| License | Apache 2.0 + MIT | Permissive |

**Concerns**:
- **394k lines is not 500 LOC** — this is a full platform, not a minimal framework
- 20+ channels, hardware/IoT, web UI, firmware flashing — massive surface area we don't need
- Active upstream with daily commits — maintaining a fork would be a continuous effort
- Feature flags (`channel-matrix`, `channel-lark`, `whatsapp-web`, `hardware`, `rag-pdf`) help but don't eliminate compile-time/binary-size overhead

---

## Decision Analysis

### Decision Criteria (from RESUME-PROMPT.md)

| Criterion | Result |
|---|---|
| All four check out → Fork ZeroClaw | **NO** — Slack fails |
| Slack support is shallow (no Socket Mode, no identity overrides, no Block Kit) → Node.js | **YES — all three conditions met** |
| Extension model is poorly documented → Node.js | No — extension model is actually good |

### What ZeroClaw Gets Right

1. **Ollama integration** — the best we've seen. Native tool calling, multi-turn history, vision, reasoning mode.
2. **Trait-driven architecture** — Tool, Channel, Provider, Memory, Runtime, Security — all pluggable.
3. **Security policy injection** — tools receive security constraints at construction.
4. **Agent loop** — production-hardened LLM → tool calling → response loop.
5. **Docker/WASM sandboxing** — container isolation per execution.
6. **Memory backends** — SQLite + vector + markdown, all behind a trait.

### What ZeroClaw Gets Wrong for NanoClaw

1. **Slack is a second-class citizen** — HTTP polling, no Socket Mode, no identity overrides, no Block Kit. Discord and Telegram have richer implementations.
2. **No Linear integration** — marked "Coming Soon" in the integration registry.
3. **394k lines of cargo** — hardware, firmware, 20+ channels, web UI. We need ~5% of this.
4. **Fork maintenance burden** — daily upstream commits mean perpetual merge conflict risk.
5. **No mode/persona architecture** — ZeroClaw has agents and skills but not the "first-class mode" concept we designed.
6. **Rust learning curve** — adding Socket Mode requires deep knowledge of `tokio-tungstenite` or `slack-morphism` crates.

### Cost Comparison (Informing Three Shapes)

| Task | Shape A: Fork ZeroClaw | Shape B: Node.js Scratch | Shape C: Node.js + ZeroClaw Patterns |
|---|---|---|---|
| Socket Mode | Rewrite `slack.rs` (~300 LOC) | `@slack/bolt` free | `@slack/bolt` free |
| Per-message identity | Add to `send()` (~5 LOC) | `chat.postMessage` (~5 LOC) | `chat.postMessage` (~5 LOC) |
| Block Kit | Extend `SendMessage` (~100 LOC) | Bolt SDK types (~10 LOC) | Bolt SDK types (~10 LOC) |
| Ollama tool calling | Already done (excellent) | `fetch()` to `/api/chat` (~150 LOC) | `fetch()` + ZeroClaw patterns (~100 LOC) |
| Linear API | Raw GraphQL/`reqwest` (~400 LOC) | `@linear/sdk` (~50 LOC) | `@linear/sdk` (~50 LOC) |
| Agent loop | **Already done** (production-hardened) | From scratch (~800-1,500 LOC) | Transplanted from ZeroClaw (~500-800 LOC) |
| Mode architecture | Build on agents (~300 LOC) | From scratch (~400-600 LOC) | From scratch (~400-600 LOC) |
| Wake/sleep engine | Build from scratch (~200 LOC) | Build from scratch (~200 LOC) | Build from scratch (~200 LOC) |
| Memory/security | **Already done** | From scratch (~500-1,000 LOC) | Transplanted patterns (~300-500 LOC) |
| Total new code | ~1,100 LOC on 394k LOC fork | 3,000-5,000 LOC | 2,000-3,000 LOC |
| Memory footprint | ~5-10MB | ~200-500MB | ~200-500MB |
| Maintenance | Fork drift (daily upstream) | Full control | Full control |

---

## Decision: Three Competing Shapes → Shaping Phase

The spike revealed that the foundation decision is **genuinely non-obvious** and belongs in the shaping phase, not the spike. The initial instinct ("Slack shallow → Node.js from scratch") was too hasty — the Slack gap is ~300-400 LOC of bounded fixes, while rebuilding ZeroClaw's agent infrastructure from scratch is 3,000-5,000 LOC of unbounded complexity.

### Shape A: Fork ZeroClaw + Extend Slack

| Dimension | Assessment |
|---|---|
| **Slack fix scope** | ~400 LOC: replace HTTP polling with `tokio-tungstenite` Socket Mode, add Block Kit to `SendMessage`, add identity fields to `send()` |
| **Linear tool** | ~400 LOC: implement Tool trait, raw GraphQL over `reqwest` (no Rust Linear SDK) |
| **Mode architecture** | ~300 LOC: build on ZeroClaw's existing agent/skill system |
| **What you get for free** | Production-hardened agent loop, Ollama provider (best-in-class), memory backends, security policy injection, Docker/WASM sandboxing, 3,214 tests |
| **Memory footprint** | ~5-10MB (Rust binary). Leaves maximum headroom for Qwen 3 14B (~9GB) on 16GB M4 |
| **Risk** | Fork maintenance against active upstream (daily commits). Rust learning curve for Slack extensions. 394k LOC surface area we don't need (but feature flags help). |

### Shape B: Node.js from Scratch

| Dimension | Assessment |
|---|---|
| **Slack** | `@slack/bolt` gives Socket Mode, Block Kit, per-message identity, interactive messages out of the box |
| **Linear** | `@linear/sdk` — 50 LOC vs 400 LOC raw GraphQL |
| **LLM** | Ollama REST API (`/api/chat` with tools) — ~100-150 LOC |
| **Realistic LOC** | 3,000-5,000 LOC for production quality (NOT the initially estimated 500-1,000). Agent loop, tool execution, error recovery, memory, security — all from scratch. |
| **Memory footprint** | ~200-500MB (Node.js runtime). Significant on 16GB M4 alongside Qwen 3 14B. |
| **Risk** | Underestimating agent infrastructure complexity. No battle-tested agent loop. "From scratch" always takes 3-5x the estimate. |

### Shape C: Thin Node.js Wrapper + ZeroClaw Patterns

| Dimension | Assessment |
|---|---|
| **Approach** | Node.js runtime (Bolt SDK, `@linear/sdk`) but architecture directly transplanted from ZeroClaw's proven patterns |
| **Patterns extracted** | Tool interface, ProviderCapabilities, centralized registry, config-driven features, security policy injection, agent loop structure |
| **LOC** | ~2,000-3,000 (less than Shape B because patterns are pre-designed) |
| **Memory footprint** | ~200-500MB (same Node.js tradeoff as Shape B) |
| **Risk** | Translating Rust patterns to TypeScript may lose the type safety guarantees. Middle ground = potentially worst of both worlds. No fork maintenance, but also no free agent infrastructure. |

### Patterns to Steal from ZeroClaw (All Shapes)

Regardless of shape chosen, these patterns should inform the architecture:

1. **Tool trait interface**: `name()`, `description()`, `parameters_schema()`, `execute()` — translate to TypeScript
2. **ProviderCapabilities pattern**: declare what each LLM can do, adapt tool calling automatically
3. **Prompt-guided fallback**: inject tools as XML in system prompt when native calling unavailable
4. **Security policy injection**: tools receive constraints at construction, not runtime
5. **Agent loop structure**: message → classify → dispatch → tool loop → respond
6. **Centralized registry**: single function registers all tools/skills, easy audit
7. **Config drives everything**: TOML/YAML determines features without code changes

---

## Patterns Document (for Shaping to Consume)

The following patterns from ZeroClaw should inform the shaping phase:

1. **Stateless LLM calls**: ZeroClaw's Ollama provider sends full conversation history each call. No session state in the provider. This matches our "re-read and catch up" recovery model.

2. **Tool registration is centralized**: All tools registered in one function (`all_tools_with_runtime`). Easy to audit, easy to enable/disable. Apply to NanoClaw's skill/connector registry.

3. **Channel trait has optional capabilities**: Base trait is minimal (name, send, listen). Advanced features (typing, drafts, reactions) are opt-in. Apply to NanoClaw's connector trait.

4. **Config drives everything**: Single TOML file determines which channels, providers, tools, and features are active. No code changes needed to enable/disable. Apply to NanoClaw's mode/skill configuration.

5. **Security policy is a first-class parameter**: Not an afterthought — tools receive it at construction. Apply to NanoClaw's tool execution layer.

---

## Next Step

Proceed to **Shaping** (stage 4) with `docs/workspace/20260221-nanoclaw-hub/requirements-handoff.md` as input. The shaping skill should produce a frame document and shaping document using R x S methodology, evaluating:

1. **Shape A**: Fork ZeroClaw + extend Slack (~1,100 LOC on top of 394k LOC production platform)
2. **Shape B**: Node.js from scratch (`@slack/bolt` + `@linear/sdk` + Ollama REST, realistically 3,000-5,000 LOC)
3. **Shape C**: Thin Node.js wrapper extracting ZeroClaw patterns (familiar runtime, proven architecture, ~2,000-3,000 LOC)

Inputs:
1. The requirements handoff (all 25+ settled decisions)
2. This spike's findings (ZeroClaw patterns to steal, per-shape cost analysis)
3. The 10 open design items from requirements-handoff.md
