---
shaping: true
---

# NanoClaw Agentic Development Hub — Frame

> Pipeline: `20260221-nanoclaw-hub`
> Date: 2026-02-22

## Source

> **Feature Strategist findings** (stage 1):
> Linear's native Slack integration creates/comments on issues from Slack but lacks:
> conversational intelligence, template-aware grooming, channel lifecycle management,
> and multi-mode personas. Competitors (GitHub Copilot Workspace, Cursor Agent Mode)
> focus on code generation, not PM workflows. The gap is between "discussing work in
> Slack" and "structured tickets in Linear."

> **Requirements Interrogator decisions** (stage 2, 5 batches, 25+ settled):
> Single Slack bot (`@nanoclaw`) with per-message identity overrides. Qwen 3 14B via
> Ollama (not GLM-4.7 — lacks native tool calling). Socket Mode (no public URL). NanoClaw's
> own Linear account (OAuth 2.0, no seat cost). Stateless LLM calls. Slack IS the persistence
> layer. Modes are first-class concepts above skills. Event-driven wake/sleep (~1hr timeout).
> Docker Compose deployment. v1 = PM mode only.

> **Research spikes** (stage 3, 4 spikes):
> Ollama native tool calling works with Qwen 3. Slack Bolt SDK provides all needed features
> (gotcha: 1 req/min on `conversations.replies` for apps created after May 2025 — mitigate
> with event-driven context accumulation). Linear OAuth 2.0 is free (no seat cost, app-attributed
> actions). Claws ecosystem has patterns worth stealing (trait-driven, config-driven, container
> isolation).

> **ZeroClaw validation spike** (stage 3b):
> ZeroClaw (16.5k stars, 394k LOC Rust) has excellent Ollama provider (native tool calling,
> multi-turn history, vision, reasoning mode) and clean trait-driven architecture (Tool, Channel,
> Provider, Memory, Security — all pluggable). Slack is shallow (HTTP polling, no Socket Mode,
> no identity overrides, no Block Kit) but the gap is bounded (~400 LOC of fixes, not a
> dealbreaker). Foundation decision is genuinely non-obvious — three competing shapes defined.

---

## Problem

Christopher manages 4Ink's development pipeline across Slack (conversation), Linear (work tracking), and GitHub (code). The gap between "discussing work in Slack" and "structured tickets in Linear" requires manual translation: reading threads, extracting requirements, creating issues with proper templates, sequencing sub-issues, and tracking lifecycle through to PR merge.

Linear's native Slack integration closes part of this gap (create/comment from Slack) but lacks:

- **Conversational intelligence** to groom initiatives into sequenced, template-aware issues through dialogue
- **Channel lifecycle management** tied to Linear project states (create, link, detect, archive)
- **Multi-persona modes** for different workflow contexts (PM, DevOps, Orchestrator)
- **Local-first operation** without cloud LLM costs — the bot should run on the desk, not in the cloud

The methodology orchestrator (Claude Code + pipeline skills) handles deep build work but has no persistent presence in Slack for day-to-day PM coordination. NanoClaw fills this gap.

---

## Outcome

A working Slack bot (`@nanoclaw`) running on Christopher's M4 Mac (16GB) that:

1. **Breaks initiatives into work** — Takes initiative context from Slack conversation, grooms it into sequenced parent/sub-issues using Linear templates, with human confirmation at each step (draft → preview → confirm, never auto-file)
2. **Manages ticket lifecycle** — Tracks PRs linked to issues, recommends closing issues when PRs merge, surfaces stale work, performs status checks
3. **Syncs channels with projects** — Auto-creates `#prj-<slug>` channels on Linear project creation, links bidirectionally, detects manually-created channels, recommends archive on project completion
4. **Participates in project conversations** — Surfaces relevant Linear/GitHub context, asks clarifying questions, gives recommendations grounded in project data
5. **Runs locally for free** — Qwen 3 14B via Ollama, no cloud API costs, Docker Compose portable to VPS when scaling is needed
6. **Supports future modes** — Architecture accommodates DevOps, Orchestrator, and custom modes without rewrite (v1 = PM mode only)
