# A5 Spike: Mode Layer on ZeroClaw's Agent Architecture

> Pipeline: `20260221-nanoclaw-hub`
> Date: 2026-02-22
> Decision: **A5 resolved** — NanoClaw modes map cleanly to ZeroClaw's existing primitives

## Context

Shape A (Fork ZeroClaw) was flagged ⚠️ on part A5: "Build mode layer on ZeroClaw's agent/skill system: persona config files, skill bundles per mode, visual differentiation routing, per-thread mode state." The shaping fit check marked Shape A as ❌ on R4 (mode architecture) because the mechanism was described at a high level without concrete knowledge of how to build it.

This spike investigates ZeroClaw's existing agent primitives to determine whether NanoClaw's "mode" concept (persona + skills + tools + visual identity, isolated per-thread) can be expressed using what ZeroClaw already provides.

## Goal

Determine the concrete mapping from NanoClaw modes to ZeroClaw's Agent/Identity/Skills system, estimate the glue code required, and identify any architectural gaps.

## Questions

| # | Question | Answer |
|---|----------|--------|
| **A5-Q1** | Does ZeroClaw have a persona/identity system? | **Yes — AIEOS v1.1** (`src/identity.rs`, 1,488 lines). Full persona specification: name, bio, psychology (MBTI, OCEAN, neural matrix), linguistics (style, formality, catchphrases, forbidden words), motivations, capabilities, physicality, history, interests. Converts to markdown system prompt via `aieos_to_system_prompt()`. Config: `[identity]` section with `format` ("openclaw" or "aieos"), `aieos_path` (file), or `aieos_inline` (JSON). |
| **A5-Q2** | Does ZeroClaw support per-agent tool selection? | **Yes — two levels.** (1) `Agent::builder().tools(vec![...])` controls which tools an Agent instance has. (2) `DelegateAgentConfig` in config allows per-sub-agent `allowed_tools: Vec<String>` allowlists. Both enable mode-specific tool sets. |
| **A5-Q3** | Does ZeroClaw support composable skill bundles? | **Yes — Skill system** (`src/skills/mod.rs`). Skills loaded from `workspace/skills/{name}/SKILL.toml` or `SKILL.md`. Each skill has name, description, version, tools (shell/http/script commands), and prompts. Injected into system prompt as XML. Skills are loaded per-agent via `AgentBuilder::skills()`. |
| **A5-Q4** | Can ZeroClaw compose different system prompts per agent? | **Yes — SystemPromptBuilder** (`src/agent/prompt.rs`). Uses composable `PromptSection` trait (`name()` + `build()`). Default sections: Identity, Tools, Safety, Skills, Workspace, DateTime, Runtime. Custom sections added via `add_section()`. Each Agent gets its own `prompt_builder`. |
| **A5-Q5** | Can multiple Agent instances coexist with different configurations? | **Yes — AgentBuilder pattern** (`src/agent/agent.rs`). `Agent::builder()` creates independent instances. Each has its own: provider, tools, memory, prompt_builder, identity_config, skills, classification_config. No shared mutable state between instances. `Agent::from_config()` builds from a Config struct. |
| **A5-Q6** | Does ZeroClaw support multi-agent delegation? | **Yes — DelegateAgentConfig** (`src/config/schema.rs:215`). Top-level config has `agents: HashMap<String, DelegateAgentConfig>`. Each delegate agent specifies: provider, model, system_prompt, api_key, temperature, agentic mode, allowed_tools, max_iterations, max_depth. |
| **A5-Q7** | What's missing for NanoClaw's mode concept? | **Three things**: (1) Per-thread mode routing — selecting which Agent handles a given Slack thread. (2) Visual identity routing — mapping mode persona to Slack `username`/`icon_emoji` overrides. (3) Mode activation syntax — parsing `@nanoclaw [pm]` from Slack messages. All three are glue code, not architectural gaps. |

## Mapping: NanoClaw Modes → ZeroClaw Primitives

| NanoClaw Mode Concept | ZeroClaw Primitive | Evidence |
|---|---|---|
| **Persona** (name, voice, personality, response policy) | `IdentityConfig` + AIEOS v1.1 | `identity.rs` — 8 AIEOS sections cover name, psychology, linguistics (formality, catchphrases, forbidden words), motivations. Injected into system prompt as markdown. |
| **Skills** (capability bundles) | `Skill` system | `skills/mod.rs` — TOML/MD manifests with tools and prompts. Loaded per-agent. Configurable via workspace directory. |
| **Tools** (per-mode tool selection) | `AgentBuilder::tools()` + `DelegateAgentConfig::allowed_tools` | `agent.rs:305-329` — builder wires specific tools. `schema.rs:237` — allowlist per delegate agent. |
| **System prompt composition** | `SystemPromptBuilder` + `PromptSection` trait | `prompt.rs` — composable sections. Custom sections via `add_section()`. Each agent builds its own prompt. |
| **Per-thread isolation** | Separate `Agent` instances | `agent.rs` — no shared mutable state. Each Agent has own history, identity, skills, tools. |
| **Visual identity** (Slack username + icon) | **NOT in ZeroClaw** | Slack `send()` only sends `channel`, `text`, `thread_ts`. Missing: `username`, `icon_emoji`. |
| **Mode activation** (`@nanoclaw [pm]`) | **NOT in ZeroClaw** | No concept of mode switching. Single agent per channel. |
| **Mode registry** (name → config) | `HashMap<String, DelegateAgentConfig>` pattern exists | `schema.rs:196` — `agents: HashMap<String, DelegateAgentConfig>`. Same pattern for modes: `modes: HashMap<String, ModeConfig>`. |

## Concrete Implementation Path (~200-300 LOC)

### 1. Mode Config (TOML) — ~30 LOC of schema additions

```toml
[modes.pm]
identity_format = "aieos"
aieos_path = "modes/pm/identity.json"
skills_dir = "modes/pm/skills"
visual_identity = { username = "NanoClaw PM", icon_emoji = ":clipboard:" }
response_policy = "respond when work items are mentioned, stay silent for social chat"
tools = ["linear_create_issue", "linear_list_issues", "linear_update_issue"]
```

This extends ZeroClaw's existing config schema pattern. The `[modes]` section mirrors the existing `[agents]` HashMap pattern.

### 2. Mode Registry (~60 LOC)

```rust
// src/modes/mod.rs
pub struct ModeRegistry {
    modes: HashMap<String, ModeConfig>,
    default_mode: String,
}

impl ModeRegistry {
    pub fn from_config(config: &Config) -> Self { ... }
    pub fn build_agent(&self, mode_name: &str, config: &Config) -> Result<Agent> {
        let mode = &self.modes[mode_name];
        Agent::builder()
            .identity_config(mode.identity.clone())
            .skills(load_skills(&mode.skills_dir))
            .tools(filter_tools(&mode.tools, all_tools))
            .prompt_builder(SystemPromptBuilder::with_defaults()
                .add_section(ResponsePolicySection::new(&mode.response_policy)))
            .build()
    }
}
```

### 3. Per-Thread Mode State (~40 LOC)

```rust
// src/modes/thread_state.rs
pub struct ThreadModeState {
    agents: HashMap<String, Agent>,  // thread_ts → Agent
    modes: HashMap<String, String>,  // thread_ts → mode_name
}

impl ThreadModeState {
    pub fn get_or_create(&mut self, thread_ts: &str, mode: &str, registry: &ModeRegistry) -> &mut Agent { ... }
    pub fn activate(&mut self, thread_ts: &str, mode: &str, registry: &ModeRegistry) { ... }
}
```

### 4. Mode Activation Parser (~30 LOC)

```rust
// Parse "@nanoclaw pm" or "@nanoclaw [pm]" from Slack messages
pub fn parse_mode_activation(text: &str, bot_id: &str) -> Option<String> { ... }
```

### 5. Visual Identity Routing (~40 LOC)

Extend `SendMessage` with optional `username` and `icon_emoji` fields, and update Slack `send()` to include them in the API payload:

```rust
// In channels/traits.rs — add to SendMessage
pub username: Option<String>,
pub icon_emoji: Option<String>,

// In channels/slack.rs — add to send() body
if let Some(ref username) = message.username {
    body["username"] = serde_json::json!(username);
}
if let Some(ref icon) = message.icon_emoji {
    body["icon_emoji"] = serde_json::json!(icon);
}
```

### 6. Response Policy Prompt Section (~20 LOC)

```rust
pub struct ResponsePolicySection { policy: String }

impl PromptSection for ResponsePolicySection {
    fn name(&self) -> &str { "Response Policy" }
    fn build(&self, _ctx: &PromptContext) -> Option<String> {
        Some(format!("## Response Policy\n\n{}", self.policy))
    }
}
```

**Total: ~220 LOC** of new Rust code, all in bounded, well-understood patterns that follow ZeroClaw's existing architecture.

## Fork Maintenance Surface

Files NanoClaw would modify:

| File | Lines | Change Type | Merge Risk |
|---|---|---|---|
| `src/channels/slack.rs` | 535 | Rewrite listen loop (Socket Mode), extend send() | **Medium** — this file changes upstream when Slack features are added. Our Socket Mode rewrite replaces the entire listen loop, making upstream changes to polling logic irrelevant (they'd conflict but we'd keep ours). |
| `src/channels/traits.rs` | 259 | Add fields to SendMessage | **Low** — struct is stable. Adding optional fields is backwards-compatible. |
| `src/config/schema.rs` | 6,963 | Add `[modes]` section | **Low** — additive only. New section, no modifications to existing fields. |
| `src/modes/` (new) | ~150 | New directory: registry, thread state, parser | **None** — new files don't conflict with upstream. |
| `src/agent/prompt.rs` | 479 | Add ResponsePolicySection | **None** — uses public `add_section()` API. Custom section in separate file. |

**3 modified files + 1 new directory**. The only file with meaningful merge risk is `slack.rs`, and the rewrite is a clean replacement (Socket Mode replaces polling entirely, no interleaving).

Contrast with the 394,000 LOC total: NanoClaw touches **~1,300 lines across 3 existing files** (0.3% of the codebase). Feature flags (`channel-slack`) already gate Slack compilation. The other 393,700 lines are untouched.

## Conclusion

**A5 is resolved.** NanoClaw's mode concept maps cleanly to ZeroClaw's existing primitives:

- **Persona** → AIEOS identity system (1,488 lines, mature, battle-tested)
- **Skills per mode** → Skill system (TOML/MD manifests, already per-agent)
- **Tools per mode** → AgentBuilder tool selection (already per-agent)
- **Per-thread isolation** → Separate Agent instances (no shared mutable state)
- **System prompt composition** → PromptSection trait (composable, extensible)

The mode layer is **~220 LOC of glue code** that connects these existing primitives. No architectural gaps — only integration work. The patterns are well-understood (HashMap registry, builder pattern, trait implementation) and follow ZeroClaw's existing conventions.

### Impact on Fit Check

- **R4 (Mode architecture)**: Shape A should now pass ✅. The mechanism is concrete: ModeRegistry builds Agent instances with mode-specific identity, skills, tools, and prompt sections. Per-thread state is a HashMap. ~220 LOC of bounded work.
- **R8 (Maintainability)**: The fork maintenance surface is bounded: 3 modified files (0.3% of codebase), 1 new directory. The largest change (Slack Socket Mode) is a clean replacement, not an interleaving. Feature flags isolate NanoClaw's changes from the rest of ZeroClaw.
