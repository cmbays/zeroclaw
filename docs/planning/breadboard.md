---
shaping: true
---

# NanoClaw Agentic Development Hub — Breadboard

> Pipeline: `20260221-nanoclaw-hub`
> Date: 2026-02-22
> Shape: A (Fork ZeroClaw + Extend Slack)
> Input: `shaping.md` Shape A parts (A1–A8), `frame.md` problem/outcome

## Breadboarding Context

NanoClaw is a **standalone Rust project** (Slack bot + Ollama LLM), not a Screen Print Pro vertical.

| Web Convention | NanoClaw Adaptation |
|---|---|
| Places = web pages/routes | Places = Slack interaction contexts (threads, DMs, Block Kit modals) |
| UI affordances = React components | UI affordances = Slack messages, Block Kit elements, identity overrides |
| Code affordances = Next.js handlers | Code affordances = Rust modules, ZeroClaw traits, new NanoClaw code |
| Phase 1/2 tagging | Not applicable (single Rust binary) |
| Backend Place | The ZeroClaw agent loop itself |

---

## Places

| # | Place | Description |
|---|-------|-------------|
| P1 | Slack Channel (Idle) | Bot is sleeping or not mentioned. Messages flow but NanoClaw does not respond. |
| P2 | Slack Thread (Active Mode) | A thread where NanoClaw is awake in a specific mode (e.g., PM). User and bot exchange messages. Block Kit buttons appear for confirm/edit/cancel flows. This is the primary interaction context. |
| P3 | Block Kit Modal (Issue Preview) | Modal showing a draft Linear issue for review before filing. Blocks interaction with the thread behind. Fields: title, description, template, labels, assignee. Buttons: Confirm, Edit, Cancel. |
| P4 | Slack DM (NanoClaw) | Direct message with the bot. Same affordances as P2 but private. Used for personal tasks or sensitive queries. |
| P5 | Agent Loop (Backend) | ZeroClaw's message → classify → dispatch → tool loop → respond pipeline. All Rust code affordances live here. |
| P6 | External Services | Linear API, GitHub API, Ollama API. Accessed by tools from P5. |

### Place Rationale (Blocking Test)

- **P1 → P2**: @mention activates the bot in a thread. The thread becomes a bounded interaction context — different affordances (bot responses, Block Kit buttons) appear.
- **P2 → P3**: "Confirm issue?" triggers a Block Kit modal. User **cannot** interact with the thread until they respond to the modal (Confirm/Edit/Cancel). Passes blocking test.
- **P4**: DMs are a separate interaction context from channel threads. Different privacy boundary.
- **P5/P6**: System boundaries — code and external services.

---

## UI Affordances

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| U1 | P1 | slack-channel | @mention message (`@nanoclaw [pm] break down this initiative`) | send | → N1 | — |
| U2 | P1 | slack-channel | Non-tagged message (bot is sleeping) | send | → N1 | — |
| U3 | P2 | slack-thread | Bot response message (with mode identity: username + icon_emoji) | render | — | ← N4 |
| U4 | P2 | slack-thread | User reply in thread | send | → N1 | — |
| U5 | P2 | slack-thread | Block Kit buttons: Confirm / Edit / Cancel (inline on draft issue message) | click | → N1 | — |
| U7 | P2 | slack-thread | Thread summary broadcast (`reply_broadcast` to channel) | render | — | ← N4 |
| U8 | P2 | slack-thread | Bot sleep notification ("Going to sleep — @mention me to wake up") | render | — | ← N18 via N4 |
| U9 | P3 | block-kit-modal | Issue preview modal (title, description, template, labels, assignee) | render | — | ← N27 |
| U10 | P3 | block-kit-modal | Confirm button | click | → N1 | — |
| U11 | P3 | block-kit-modal | Edit button | click | → N1 | — |
| U12 | P3 | block-kit-modal | Cancel button | click | → P2 | — |
| U13 | P2 | slack-thread | Issue filed confirmation (link to Linear issue, summary) | render | — | ← N4 |
| U14 | P2 | slack-thread | Error message (tool failure, LLM timeout — user-friendly) | render | — | ← N4 |
| _P2 | P4 | slack-dm | Place reference: inherits all P2 affordances (bot response, buttons, etc.) | — | → P2 | — |
| U16 | P2 | slack-thread | Status update message (stale work, PR merged, issue state change) | render | — | ← N4 |
| U17 | P1 | slack-channel | Channel creation notification (`#prj-<slug>` auto-created) | render | — | ← N23 via N4 |

---

## Code Affordances

### Slack Channel Layer (A2, A3)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N1 | P5 | `slack.rs` | `listen()` — Socket Mode WebSocket receives Slack events (messages, mentions, actions) | observe | → N2 | — |
| N2 | P5 | `slack.rs` | `filter_event()` — checks wake/sleep state: @mention or active thread → forward, sleeping → discard | call | → N17, then → N3 or discard | — |
| N3 | P5 | `slack.rs` | `route_event()` — route by event type: message → N7+N8, block_action → N26, view_submission → N28 | call | → N7, N8, N26, N28 | — |
| N4 | P5 | `slack.rs` | `send()` — extended with `username`, `icon_emoji`, `reply_broadcast`, Block Kit JSON payload | call | → Slack API | → U3, U7, U8, U13, U14, U16, U17 |
| N29 | P5 | `webhook.rs` | `receive_webhook()` — HTTP endpoint (via Cloudflare Tunnel) receives Linear/GitHub webhook events, dispatches to tools | call | → N22, N23, N10 | — |

### Mode Layer (A5)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N5 | P5 | `modes/mod.rs` | `ModeRegistry::from_config()` — reads Config (S6), builds mode definitions, writes to mode registry | call | → S1 | ← S6 |
| N6 | P5 | `modes/mod.rs` | `ModeRegistry::build_agent()` — creates Agent instance with mode-specific AIEOS identity, skills, tools, prompt sections | call | → S2 | — |
| N7 | P5 | `modes/mod.rs` | `parse_mode_activation()` — extracts mode name from `@nanoclaw [pm]` syntax | call | — | → N6 |
| N8 | P5 | `modes/thread_state.rs` | `ThreadModeState::get_or_create()` — returns existing Agent for thread or creates new one via ModeRegistry | call | → N6 (if new) | → N9, N10 |
| N9 | P5 | `modes/thread_state.rs` | `ThreadModeState::get_visual_identity()` — returns `username` + `icon_emoji` for the active mode in a thread | call | — | → N4 |

### Agent Loop (A5 + ZeroClaw existing)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N10 | P5 | `agent.rs` | `Agent::process_message()` — the core agent loop: classify intent → select tools → execute tool loop → compose response | call | → N20, N13 | → N4 |
| N11 | P5 | `agent/prompt.rs` | `SystemPromptBuilder` — composes system prompt from Identity + Tools + Safety + Skills + ResponsePolicy sections | call | — | → N10 |
| N12 | P5 | `agent/prompt.rs` | `ResponsePolicySection` — custom PromptSection injecting per-mode response policy (when to respond, when to stay silent) | call | — | → N11 |

### Linear Tool (A4)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N13 | P5 | `tools/linear.rs` | `LinearTool::execute()` — dispatches Linear GraphQL operations (create_issue, list_issues, update_issue, get_initiative, list_templates) | call | → N14 | → N10 |
| N14 | P5 | `tools/linear.rs` | `build_graphql_query()` — constructs GraphQL query from tool arguments | call | → N16 | — |
| N15 | P5 | `tools/linear.rs` | `parse_graphql_response()` — extracts structured data from Linear's GraphQL response | call | — | → N13 |
| N16 | P6 | `reqwest` | Linear GraphQL API call (`POST https://api.linear.app/graphql`) | call | — | → N15 |

### Wake/Sleep Engine (A6)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N17 | P5 | `wake_sleep.rs` | `WakeSleepEngine::on_event()` — processes each Slack event: @mention → wake, message in active thread → reset timer, inactivity → sleep | call | → S3 | → N2 |
| N18 | P5 | `wake_sleep.rs` | `inactivity_timer` — per-thread tokio timer (~1hr), fires sleep transition on expiry | observe | → N4 (sleep msg) | — |
| N19 | P5 | `wake_sleep.rs` | `should_respond()` — discretionary response logic: checks if bot should engage with non-tagged message based on ResponsePolicy | call | — | → N2 |

### Ollama Provider (ZeroClaw existing)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N20 | P5 | `providers/ollama.rs` | `OllamaProvider::chat_with_tools()` — sends conversation history + tool definitions to Ollama, returns structured response with optional tool calls | call | → N21 | → N10 |
| N21 | P6 | `reqwest` | Ollama API call (`POST http://ollama:11434/api/chat`) | call | — | → N20 |

### Channel-Project Lifecycle (v1 PM mode)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N22 | P5 | `tools/linear.rs` | `detect_channel_project_link()` — parses Slack channel description for Linear project URL, or Linear project for Slack channel link. Fallback: `#prj-` prefix convention. | call | → N16 | → N10 |
| N23 | P5 | `tools/slack_ops.rs` | `create_project_channel()` — creates `#prj-<slug>` Slack channel on Linear project creation webhook, sets description with bidirectional link | call | → Slack API | → U17 |

### Config & Startup (A1, A7, A8)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N24 | P5 | `config/schema.rs` | `Config::load()` — loads TOML config with `[modes]`, `[channels.slack]`, `[tools.linear]`, `[provider.ollama]` sections | call | → S6 | — |
| N25 | P5 | `main.rs` | `startup()` — loads config, initializes ModeRegistry, connects Slack Socket Mode, starts webhook listener, verifies Ollama health | call | → N24, N5, N1, N29, N21 | — |

### Block Kit Action Handling (A3)

| # | Place | Component | Affordance | Control | Wires Out | Returns To |
|---|-------|-----------|------------|---------|-----------|------------|
| N26 | P5 | `slack.rs` | `handle_block_action()` — routes Block Kit button clicks (confirm/edit/cancel) to agent for processing | call | → N10 | → N4 |
| N27 | P5 | `slack.rs` | `open_modal()` — sends Block Kit modal view to Slack for issue preview | call | → Slack API | → U9 |
| N28 | P5 | `slack.rs` | `handle_view_submission()` — processes modal form submission (Confirm/Edit from issue preview) | call | → N10 | → N4 |

---

## Data Stores

| # | Place | Store | Description |
|---|-------|-------|-------------|
| S1 | P5 | `ModeRegistry.modes` | `HashMap<String, ModeConfig>` — mode name → config (AIEOS identity path, skills dir, tools list, visual identity, response policy). Loaded once at startup from TOML. |
| S2 | P5 | `ThreadModeState.agents` | `HashMap<ThreadId, Agent>` — per-thread Agent instances with independent conversation history, identity, skills, tools. Created on first interaction, cleaned up on sleep. |
| S3 | P5 | `ThreadModeState.wake_state` | `HashMap<ThreadId, WakeState>` — per-thread wake/sleep state: `Awake { last_activity: Instant }` or `Sleeping`. |
| S4 | P6 | Linear API | Issues, initiatives, projects, templates, workflow states. Queried fresh each time (no local cache). |
| S5 | P6 | Slack API | Channels, messages, threads, user profiles. Event-driven accumulation while awake. |
| S6 | P5 | `Config` | TOML config loaded at startup: modes, Slack credentials, Linear OAuth token, Ollama endpoint. Immutable after load. |

---

## Wiring Narratives

### Flow 1: Wake + First Interaction

```
User sends: "@nanoclaw [pm] break down the auth initiative" (U1)
  → Socket Mode receives event (N1)
  → filter_event: @mention detected, wake thread (N2 → N17 → S3)
  → route_event: message type → mode resolution path (N3)
  → Parse mode activation: "[pm]" → "pm" (N7)
  → Get or create Agent for thread: ModeRegistry builds PM Agent (N8 → N6 → S2)
    → AgentBuilder wires: AIEOS PM identity + Linear tools + PM skills + ResponsePolicySection (N11, N12)
  → Agent processes message (N10):
    → SystemPrompt composed (N11)
    → Ollama called with tools (N20 → N21)
    → LLM decides to call linear_list_templates tool
    → LinearTool executes: GraphQL query for templates (N13 → N14 → N16 → N15)
    → Result fed back to LLM
    → LLM composes initiative breakdown draft
  → Get visual identity for PM mode (N9): username="NanoClaw PM", icon_emoji=":clipboard:"
  → Send response via extended send() with Block Kit confirm/edit/cancel buttons (N4)
  → User sees: PM-branded message with initiative breakdown + buttons (U3, U5)
```

### Flow 2: Issue Confirmation

```
User clicks "Confirm" button on draft issue message (U5)
  → Socket Mode receives block_action event (N1)
  → filter_event: active thread, pass through (N2)
  → route_event: block_action type → action handler (N3)
  → handle_block_action routes to agent (N26 → N10)
  → Agent processes confirmation (N10):
    → LLM sees user confirmed, calls linear_create_issue tool
    → LinearTool executes: GraphQL mutation (N13 → N14 → N16 → N15)
    → Issue created, URL returned
  → Send confirmation message with Linear issue link (N4 → U13)
```

### Flow 3: Sleep on Inactivity

```
No messages in thread for ~1 hour
  → Inactivity timer fires (N18)
  → Update wake state to Sleeping (S3)
  → Send sleep notification (N4 → U8)
  → Agent instance kept in S2 (conversation history preserved for re-wake)
```

### Flow 4: Channel-Project Sync (Webhook Inbound)

```
Linear webhook: new project "Auth Refactor" created
  → Cloudflare Tunnel routes HTTP POST to NanoClaw
  → receive_webhook: parses Linear event payload (N29)
  → detect_channel_project_link finds no existing channel (N22 → N16)
  → create_project_channel: creates #prj-auth-refactor, sets description (N23)
  → Channel creation notification posted (N4 → U17)
```

### Flow 5: Lifecycle Status Update (Webhook Inbound)

```
GitHub webhook: PR #42 merged for issue ENG-123
  → Cloudflare Tunnel routes HTTP POST to NanoClaw
  → receive_webhook: parses GitHub event payload (N29)
  → Agent processes lifecycle event (N29 → N10):
    → LLM sees PR merged, recommends closing issue
    → LinearTool queries issue status (N13 → N14 → N16 → N15)
  → Send status update to project thread (N4 → U16)
```

---

## Mermaid Visualization

```mermaid
flowchart TB
    subgraph P1["P1: Slack Channel (Idle)"]
        U1["U1: @mention message"]
        U2["U2: Non-tagged message"]
        U17["U17: Channel creation notification"]
    end

    subgraph P2["P2: Slack Thread (Active Mode)"]
        U3["U3: Bot response (mode identity)"]
        U4["U4: User reply"]
        U5["U5: BK buttons: Confirm/Edit/Cancel"]
        U7["U7: Thread summary broadcast"]
        U8["U8: Bot sleep notification"]
        U13["U13: Issue filed confirmation"]
        U14["U14: Error message"]
        U16["U16: Status update message"]
    end

    subgraph P3["P3: Block Kit Modal (Issue Preview)"]
        U9["U9: Issue preview modal"]
        U10["U10: Confirm button"]
        U11["U11: Edit button"]
        U12["U12: Cancel button"]
    end

    subgraph P5["P5: Agent Loop (Backend)"]
        subgraph slack_layer["Slack Channel Layer"]
            N1["N1: listen() Socket Mode"]
            N2["N2: filter_event()"]
            N3["N3: route_event()"]
            N4["N4: send() extended"]
            N26["N26: handle_block_action()"]
            N27["N27: open_modal()"]
            N28["N28: handle_view_submission()"]
            N29["N29: receive_webhook()"]
        end

        subgraph mode_layer["Mode Layer"]
            N5["N5: ModeRegistry::from_config()"]
            N6["N6: ModeRegistry::build_agent()"]
            N7["N7: parse_mode_activation()"]
            N8["N8: ThreadModeState::get_or_create()"]
            N9["N9: get_visual_identity()"]
            S1["S1: ModeRegistry.modes"]
            S2["S2: ThreadModeState.agents"]
        end

        subgraph agent_loop["Agent Loop"]
            N10["N10: Agent::process_message()"]
            N11["N11: SystemPromptBuilder"]
            N12["N12: ResponsePolicySection"]
        end

        subgraph wake_sleep["Wake/Sleep Engine"]
            N17["N17: WakeSleepEngine::on_event()"]
            N18["N18: inactivity_timer"]
            N19["N19: should_respond()"]
            S3["S3: wake_state"]
        end

        subgraph tools["Tools"]
            N13["N13: LinearTool::execute()"]
            N14["N14: build_graphql_query()"]
            N15["N15: parse_graphql_response()"]
            N22["N22: detect_channel_project_link()"]
            N23["N23: create_project_channel()"]
        end

        subgraph config["Config & Startup"]
            N24["N24: Config::load()"]
            N25["N25: startup()"]
            S6["S6: Config TOML"]
        end
    end

    subgraph P6["P6: External Services"]
        N16["N16: Linear GraphQL API"]
        N20["N20: OllamaProvider::chat_with_tools()"]
        N21["N21: Ollama API"]
        S4["S4: Linear (issues, projects)"]
        S5["S5: Slack (channels, messages)"]
    end

    %% Inbound: all Slack events enter via Socket Mode → filter → route
    U1 --> N1
    U2 --> N1
    U4 --> N1
    U5 --> N1
    U10 --> N1
    U11 --> N1
    N1 --> N2
    N2 -->|@mention or active thread| N3
    N2 -->|discard if sleeping| P1

    %% Wake/sleep feeds filter
    N17 --> S3
    S3 -.-> N2
    N19 -.-> N2

    %% Route by event type
    N3 -->|message| N7
    N3 -->|message| N8
    N3 -->|block_action| N26
    N3 -->|view_submission| N28

    %% Mode resolution: parse → get/create → build → store
    N7 -.-> N8
    N8 --> N6
    N6 --> S2
    S1 -.-> N6

    %% Agent processing
    N8 -.-> N10
    N11 -.-> N10
    N12 -.-> N11
    N10 --> N20
    N20 --> N21
    N21 -.-> N20
    N20 -.-> N10

    %% Tool execution (Linear)
    N10 --> N13
    N13 --> N14
    N14 --> N16
    N16 -.-> N15
    N15 -.-> N13
    N13 -.-> N10

    %% Response output
    N9 -.-> N4
    N10 -.-> N4
    N4 --> U3
    N4 --> U7
    N4 --> U8
    N4 --> U13
    N4 --> U14
    N4 --> U16

    %% Block Kit action handling
    N26 --> N10
    N27 --> U9
    U12 --> P2
    N28 --> N10

    %% Wake/Sleep timer
    N18 --> N4

    %% Webhook inbound (Linear/GitHub via Cloudflare Tunnel)
    N29 --> N22
    N29 --> N23
    N29 --> N10
    N22 --> N16
    N23 --> N4
    N23 --> U17

    %% Startup: load config → init registry → connect
    N25 --> N24
    N25 --> N5
    N25 --> N1
    N25 --> N29
    N24 --> S6
    S6 -.-> N5
    N5 --> S1

    classDef ui fill:#ffb6c1,stroke:#d87093,color:#000
    classDef nonui fill:#d3d3d3,stroke:#808080,color:#000
    classDef store fill:#e6e6fa,stroke:#9370db,color:#000
    classDef external fill:#b3e5fc,stroke:#0288d1,color:#000

    class U1,U2,U3,U4,U5,U7,U8,U9,U10,U11,U12,U13,U14,U16,U17 ui
    class N1,N2,N3,N4,N5,N6,N7,N8,N9,N10,N11,N12,N13,N14,N15,N17,N18,N19,N22,N23,N24,N25,N26,N27,N28,N29 nonui
    class S1,S2,S3,S6 store
    class N16,N20,N21,S4,S5 external
```

---

## Vertical Slices

| # | Slice | Parts | Affordances | Demo |
|---|-------|-------|-------------|------|
| V1 | Fork + Slack Socket Mode | A1, A2 | N1, N3, N4, N24, N25, S6 | "Bot connects via WebSocket, receives a Slack message, echoes it back" |
| V2 | Identity + Block Kit send | A3 | N4 (extended), U3, U7 | "Bot replies with custom username/icon and Block Kit formatting" |
| V3 | Mode activation + routing | A5 | N5, N6, N7, N8, N9, S1, S2, U1 | "Type `@nanoclaw [pm]`, bot responds with PM persona identity" |
| V4 | Agent loop + Ollama | A5 (agent) | N10, N11, N12, N20, N21 | "Ask a question, bot reasons via Ollama and responds conversationally" |
| V5 | Linear tool (issue CRUD) | A4 | N13, N14, N15, N16, S4 | "Ask bot to create an issue, it calls Linear API and returns the link" |
| V6 | Confirm/Edit/Cancel flow | A3, A4 | N26, N27, N28, U5, U9, U10, U11, U12, U13 | "Bot drafts issue with buttons → click Confirm → issue filed in Linear" |
| V7 | Wake/Sleep engine | A6 | N2, N17, N18, N19, S3, U2, U8 | "Bot sleeps after 1hr inactivity, wakes on @mention" |
| V8 | Channel-project lifecycle | A4 (extended) | N29, N22, N23, U17 | "Create Linear project → bot auto-creates #prj-slug channel" |
| V9 | Docker Compose deployment | A7, A8 | N25 (containerized) | "docker compose up — bot connects, Ollama serves model, tunnel routes webhooks" |

### Per-Slice Affordance Tables

#### V1: Fork + Slack Socket Mode

> **Demo**: "Bot connects via WebSocket, receives a Slack message, echoes it back"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N1 | `slack.rs` | `listen()` Socket Mode WebSocket | observe | → N2 | — |
| N2 | `slack.rs` | `filter_event()` (stub: always pass-through in V1) | call | → N3 | — |
| N3 | `slack.rs` | `route_event()` — extract text, thread_ts, channel | call | → N4 (echo path) | — |
| N4 | `slack.rs` | `send()` (basic: channel + text + thread_ts) | call | → Slack API | → U3 |
| N24 | `config/schema.rs` | `Config::load()` | call | → S6 | — |
| N25 | `main.rs` | `startup()` | call | → N24, N1 | — |
| S6 | — | Config TOML | store | — | → N24 |
| U3 | slack-thread | Bot echo response | render | — | ← N4 |

#### V2: Identity + Block Kit Send

> **Demo**: "Bot replies with custom username/icon and Block Kit formatting"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N4 | `slack.rs` | `send()` extended: `username`, `icon_emoji`, `reply_broadcast`, Block Kit JSON | call | → Slack API | → U3, U7 |
| U3 | slack-thread | Bot response with mode identity (username + icon) | render | — | — |
| U7 | slack-thread | Thread summary broadcast (`reply_broadcast`) | render | — | — |

#### V3: Mode Activation + Routing

> **Demo**: "Type `@nanoclaw [pm]`, bot responds with PM persona identity"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N5 | `modes/mod.rs` | `ModeRegistry::from_config()` | call | → S1 | — |
| N6 | `modes/mod.rs` | `ModeRegistry::build_agent()` | call | → S2 | — |
| N7 | `modes/mod.rs` | `parse_mode_activation()` | call | — | → N6 |
| N8 | `modes/thread_state.rs` | `ThreadModeState::get_or_create()` | call | → N6 | → N10 |
| N9 | `modes/thread_state.rs` | `get_visual_identity()` | call | — | → N4 |
| S1 | — | `ModeRegistry.modes` HashMap | store | — | → N6 |
| S2 | — | `ThreadModeState.agents` HashMap | store | — | → N8 |
| U1 | slack-channel | @mention with mode selector | send | → N1 | — |

#### V4: Agent Loop + Ollama

> **Demo**: "Ask a question, bot reasons via Ollama and responds conversationally"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N10 | `agent.rs` | `Agent::process_message()` | call | → N20 | → N4 |
| N11 | `agent/prompt.rs` | `SystemPromptBuilder` | call | — | → N10 |
| N12 | `agent/prompt.rs` | `ResponsePolicySection` | call | — | → N11 |
| N20 | `providers/ollama.rs` | `OllamaProvider::chat_with_tools()` | call | → N21 | → N10 |
| N21 | `reqwest` | Ollama API call | call | — | → N20 |
| U4 | slack-thread | User reply in thread | send | → N1 | — |

#### V5: Linear Tool (Issue CRUD)

> **Demo**: "Ask bot to create an issue, it calls Linear API and returns the link"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N13 | `tools/linear.rs` | `LinearTool::execute()` | call | → N14 | → N10 |
| N14 | `tools/linear.rs` | `build_graphql_query()` | call | → N16 | — |
| N15 | `tools/linear.rs` | `parse_graphql_response()` | call | — | → N13 |
| N16 | `reqwest` | Linear GraphQL API call | call | — | → N15 |
| S4 | — | Linear API (issues, projects, templates) | store | — | → N15 |
| U13 | slack-thread | Issue filed confirmation (link) | render | — | — |

#### V6: Confirm/Edit/Cancel Flow

> **Demo**: "Bot drafts issue with buttons → click Confirm → issue filed in Linear"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N26 | `slack.rs` | `handle_block_action()` | call | → N10 | → N4 |
| N27 | `slack.rs` | `open_modal()` | call | → Slack API | → U9 |
| N28 | `slack.rs` | `handle_view_submission()` | call | → N10 | → N4 |
| U5 | slack-thread | BK buttons: Confirm/Edit/Cancel | click | → N1 | — |
| U9 | block-kit-modal | Issue preview modal | render | — | — |
| U10 | block-kit-modal | Confirm button | click | → N1 | — |
| U11 | block-kit-modal | Edit button | click | → N1 | — |
| U12 | block-kit-modal | Cancel button | click | → P2 | — |

#### V7: Wake/Sleep Engine

> **Demo**: "Bot sleeps after 1hr inactivity, wakes on @mention"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N2 | `slack.rs` | Wake/sleep filter | call | → N3 or discard | — |
| N17 | `wake_sleep.rs` | `WakeSleepEngine::on_event()` | call | → S3 | → N2 |
| N18 | `wake_sleep.rs` | `inactivity_timer` | observe | → N4 | — |
| N19 | `wake_sleep.rs` | `should_respond()` | call | — | → N2 |
| S3 | — | `wake_state` HashMap | store | — | → N2 |
| U2 | slack-channel | Non-tagged message | send | → N1 | — |
| U8 | slack-thread | Bot sleep notification | render | — | — |

#### V8: Channel-Project Lifecycle

> **Demo**: "Create Linear project → bot auto-creates #prj-slug channel"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N29 | `webhook.rs` | `receive_webhook()` — HTTP endpoint via Cloudflare Tunnel | call | → N22, N23, N10 | — |
| N22 | `tools/linear.rs` | `detect_channel_project_link()` | call | → N16 | → N10 |
| N23 | `tools/slack_ops.rs` | `create_project_channel()` | call | → Slack API | → U17 |
| U17 | slack-channel | Channel creation notification | render | — | — |

#### V9: Docker Compose Deployment

> **Demo**: "docker compose up — bot connects, Ollama serves model, tunnel routes webhooks"

| # | Component | Affordance | Control | Wires Out | Returns To |
|---|-----------|------------|---------|-----------|------------|
| N25 | `main.rs` | `startup()` containerized — health checks Ollama, connects Slack, starts listen + webhook | call | → N24, N5, N1, N29, N21 | — |
| U14 | slack-thread | Error message (graceful) | render | — | — |

---

## Scope Coverage

| Req | Requirement | Affordances | Covered? |
|-----|-------------|-------------|----------|
| R0 | Conversational PM hub in Slack driving Linear work management | U1→N1→N10→N13→U13 (full flow from mention to filed issue) | Yes |
| R1 | First-class Slack (Socket Mode, identity, Block Kit, reply_broadcast) | N1 (Socket Mode), N4 (identity + Block Kit + reply_broadcast), N26-N28 (actions) | Yes |
| R2 | Local LLM via Ollama (Qwen 3 14B, native tool calling) | N20, N21 (Ollama provider, tool calling) | Yes |
| R3 | Linear integration (OAuth, issue CRUD, templates) | N13-N16 (LinearTool, GraphQL), S4 (Linear API) | Yes |
| R4 | Mode architecture (first-class, per-thread, visual differentiation) | N5-N9 (ModeRegistry, ThreadModeState, visual identity), S1-S2 | Yes |
| R5 | Robust agent infrastructure (agent loop, error recovery, multi-turn) | N10 (agent loop), N11-N12 (prompt composition), N20 (Ollama with history), U14 (error handling) | Yes |
| R6 | Operational constraints (≤16GB, Docker Compose, tunnel) | V9 (Docker Compose), ~5-10MB binary, Cloudflare Tunnel sidecar | Yes |
| R7 | Extensibility (trait-driven, config-driven) | Tool trait (N13), ModeRegistry config-driven (N5, S1), TOML config (N24, S6) | Yes |
| R8 | Maintainability (bounded fork, debuggable, testable) | 3 modified files + 1 new dir. Feature flags isolate changes. ZeroClaw's 3,214 tests validate untouched code. | Yes |

---

## Breadboard Reflection Findings

Audit performed after initial breadboard completion. User stories from Frame outcomes traced through wiring; naming test applied to all 28 code affordances; diagram-only nodes and dangling wires checked.

### Smells Found and Fixed

| # | Smell | Severity | Finding | Fix |
|---|-------|----------|---------|-----|
| F1 | Missing path | High | No inbound path for Linear/GitHub webhooks. N22, N23, U16 had no trigger — lifecycle events (project created, PR merged) couldn't enter the system. | Added N29 `receive_webhook()` — HTTP endpoint via Cloudflare Tunnel. Wires: N29 → N22, N23, N10. Added to V8 slice and N25 startup. |
| F2 | Incoherent wiring | Medium | N1 wired → N2, N3 — events could bypass wake/sleep filter (N2) and reach route_event (N3) directly. Sleeping threads would receive messages. | N1 now wires → N2 only. N2 → N3 is the single path. Events must pass filter. |
| F3 | Wrong causality | Medium | N6 `build_agent()` wired → N7 `parse_mode_activation()`. Reversed — parsing happens first, build uses the result. | N6 Wires Out → S2 (writes built agent to store). N7 Returns To → N6 (parse result feeds build). |
| F4 | Incoherent wiring | Medium | N26 `handle_block_action()` wired → N10, N13. Block action handler shouldn't call Linear tool directly — it should route through the agent loop (N10) which decides what tools to invoke. | N26 Wires Out → N10 only. Agent loop handles tool dispatch. Same fix applied to N28. |
| F5 | Wrong causality | Low | N24 `Config::load()` wired → S1 (ModeRegistry). Config loads raw TOML (S6); ModeRegistry (N5) reads S6 and builds S1. Config shouldn't know about ModeRegistry. | N24 → S6. N5 reads S6 (← S6), writes → S1. Proper separation of concerns. |
| F6 | Naming resistance | Low | N17 `on_event()` — "on" suggests passive listener, but it actively updates wake state and returns filter decision. Could be two affordances (update + decide). | Accepted as-is. Event handler pattern is idiomatic Rust (single entry point that updates state and returns action). The "or" is implicit in the event handler contract. |
| F7 | Naming resistance | Low | N8 `get_or_create()` — "or" connecting two verbs (get, create). | Accepted as-is. This is an idiomatic Rust `HashMap::entry().or_insert_with()` pattern. Splitting would create artificial boundaries. |
| F8 | Redundant affordance | Low | U5 (Confirm/Edit/Cancel buttons) and U6 (File as issue button) were separate UI affordances both wiring to N26. U6 was just another Block Kit button — same control, same handler. | U6 removed. All Block Kit action buttons consolidated under U5. |

### Smells Not Found

- **Diagram-only nodes**: All diagram nodes have table rows.
- **Stale affordances**: N/A (no existing code yet — Phase 1 only).
- **Implementation mismatch**: N/A (no code to compare against).

## Quality Gate

- [x] Every Place passes the blocking test (P2: thread context, P3: modal blocks thread, P4: DM boundary)
- [x] Every R from shaping has corresponding affordances (scope coverage table above)
- [x] Every U has at least one Wires Out or Returns To
- [x] Every N has a trigger and either Wires Out or Returns To
- [x] Every S has at least one reader and one writer
- [x] No dangling wire references
- [x] Slices defined with demo statements (V1-V9)
- [x] Mermaid diagram matches tables (tables are truth)
- [x] No phase indicators needed (single Rust binary)
