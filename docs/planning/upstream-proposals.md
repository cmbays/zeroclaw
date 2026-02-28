# Upstream Contribution Proposals

Tracking active and potential contributions from this fork back to
[zeroclaw-labs/zeroclaw](https://github.com/zeroclaw-labs/zeroclaw).

---

## How to Contribute Upstream

The upstream repo is open source (MIT) and accepts external PRs to the `dev` branch.
The practical pathway for external contributors:

1. **Open a GitHub issue** on `zeroclaw-labs/zeroclaw` describing the proposed change.
   This is the primary communication channel. The maintainer team can then create a
   Linear issue (`RMN-XXX`) on their end and link it back to you.

2. **Wait for a "go ahead"** signal in the issue. For small, clean config additions this
   is usually fast if the feature fits the project direction.

3. **Fork, branch, and PR**. Target `dev`. Follow the PR template fully:
   - Include the Linear issue key provided by maintainers (or ask them to provide one).
   - Follow Track B (medium-risk) process for config/behavior changes.
   - Add/update `docs/config-reference.md` for any new config keys.
   - Include at least one integration test using the `DummyProvider` mock pattern.
   - Run `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test`.

**Re: the Linear issue key requirement**: The PR template marks this as "required"
(`Linear issue key(s) (required, e.g. RMN-123)`). In practice this means the maintainers
use Linear for their internal sprint tracking. External contributors cannot create Linear
issues directly — but you can ask a maintainer to create one in the issue thread before
submitting the PR. This is a normal OSS workflow: maintainer triages the GitHub issue and
creates their internal ticket, then the contributor references it in the PR.

---

## Proposal: `AgentConfig.tool_allowlist`

**Status**: GitHub issue not yet opened — pending this fork's refactor landing first.

**What**: Add `tool_allowlist: Vec<String>` to `AgentConfig` in `src/config/schema.rs`.

**Why it fits upstream**:
- `AgentConfig` already groups per-agent behavioral settings (`max_tool_iterations`,
  `parallel_tools`, `tool_dispatcher`, etc.). A capability profile field fits naturally.
- Zero new dependencies. 16 lines of implementation (`apply_tool_allowlist()` in
  `src/agent/agent.rs`). 4 unit tests.
- Useful for any multi-bot deployment where different agent processes need different
  tool subsets (security hardening, capability focus, resource limiting).
- Upstream already has `SubAgentConfig.allowed_tools` for sub-agents — this extends
  the same concept to the top-level agent process.

**What a clean upstream PR would include**:
- `schema.rs`: `tool_allowlist: Vec<String>` field on `AgentConfig` with `#[serde(default)]`
- `agent.rs`: `apply_tool_allowlist(tools, &config.agent.tool_allowlist)` call in
  `from_config_with_runtime()`, after `all_tools_with_runtime()`
- `docs/config-reference.md`: entry under `[agent]` section documenting the field,
  empty-default semantics, and an example
- Tests: at minimum the 4 unit tests in `agent.rs`; ideally one integration test
  using `DummyProvider` that verifies tool count before/after allowlist

**Fork implementation reference**: `src/agent/agent.rs:249-266` + `src/agent/agent.rs:332-333`

**Next action**: Open a GitHub issue on `zeroclaw-labs/zeroclaw` once PR #77 (this
refactor) is merged to dev. Link: https://github.com/zeroclaw-labs/zeroclaw/issues

---

## Notes on Upstream Contribution Pace

As of 2026-02-28, all merged PRs in the upstream repo are from the core maintainer
team (`theonlyhennygod`, `chumyin`). This is common for young open source projects
that have been primarily maintainer-driven. It does not mean external contributions
are unwelcome — the `CONTRIBUTING.md` explicitly invites them and outlines tracks A/B/C.

Track A (docs, tests, chore) is the fastest path to a first merged PR and helps build
the contributor relationship before attempting Track B config changes.
