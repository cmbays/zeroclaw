# Katara PM Skill — Shape Up + Linear Conventions

> Reference doc for Katara's PM workflow. Katara can fetch this via `file_read` for
> complex planning tasks. Path: `docs/planning/katara-pm-skill.md`

---

## 1. Shape Up Methodology (How This Project Works)

### Core Principles

- **Appetite over estimates** — decide how much time a bet is *worth*, not how long it will
  *take*. If a pitch doesn't fit the appetite, scope-hammer it, not the deadline.
- **6-week cycles + 2-week cooldown** — ship in 6 weeks, then cool down (bug fixes, exploration,
  next cycle prep). No heroics to finish; if it doesn't ship in the cycle, it goes back to
  shaping, not a "backlog".
- **Scope hammering is expected** — it's OK to cut scope mid-cycle to ship on time. Shipping
  something smaller is better than shipping nothing.
- **No backlog** — unpitched, un-bet work doesn't get a queue. If it matters, someone will pitch
  it again next cycle.

### Hierarchy in Linear

| Shape Up Concept | Linear Entity | Notes |
|------------------|---------------|-------|
| Strategic bet | **Initiative** | High-level direction, multi-cycle |
| Shaped pitch | **Project** | Bounded scope, one cycle (usually) |
| Scoped task | **Issue** | Concrete unit of work |
| Sub-task | **Issue** with `parent_id` | Break down complex issues |

### Cycle Flow

1. **Cool down** — pitch ideas as Projects under Initiatives
2. **Betting table** — pick pitches, set appetite, assign teams
3. **Cycle start** — shaped issues created, added to Cycle via `add_issue_to_cycle`
4. **Mid-cycle check** — review blocked/stalled issues, surface to team
5. **Cycle end** — close done issues, archive stale ones, retro

---

## 2. Linear Workspace Conventions

### Priority Scale

| Value | Label | Meaning |
|-------|-------|---------|
| 0 | No priority | Backburner / undecided |
| 1 | Urgent | On-fire, drop everything |
| 2 | High | This cycle, near the top |
| 3 | Normal | Default for most issues |
| 4 | Low | Someday / maybe |

### Cycle Naming

- Format: `YYYY-CX Description` — e.g., `2026-C1 Foundation`, `2026-C2 Bot Intelligence`
- Or ISO week: `2026-W09` for sprint-style tracking

### Issue Title Format

`[Bot/Area] Verb + outcome` — e.g.:
- `[Katara] Add comment support to LinearTool`
- `[Sokka] Fix WebSocket reconnect on 1006 close`
- `[Ops] Set up Mattermost channel manifest`

### Label Taxonomy

Confirm live labels via `list_labels` before applying. Common patterns:
- `bug` — something broken
- `feature` — new capability
- `chore` — maintenance, refactor, docs
- `blocked` — waiting on external dependency
- `security` — security-sensitive work

---

## 3. Katara's Workflow Patterns

### New Request → Issue

1. Ask: *appetite* (XS/S/M/L), *team* (which bot owns it), *acceptance criteria*
2. `list_projects` → pick or create the right project
3. `list_states` → get state UUIDs (do this once per session, cache in memory)
4. `list_members` → get assignee UUID if assigning to a specific person
5. `create_issue` with: title, description, priority, assignee_id, project_id, estimate, due_date
6. Confirm URL with requester

### Cycle Planning

1. `list_initiatives` → understand strategic bets
2. `list_projects` → find shaped pitches ready to go
3. `list_cycles(filter: "active")` → find current cycle or create new one
4. `list_issues(project_id: ...)` → find issues for the pitch
5. `add_issue_to_cycle` for each issue
6. Post summary in #general or relevant Mattermost thread

### Status Check / Standup

1. `list_cycles(filter: "active")` → get current cycle ID
2. `get_cycle(cycle_id: ...)` → full issue list with states
3. Group by state: Done / In Progress / Todo / Blocked
4. Flag blocked and stalled (In Progress with no recent activity)
5. Post in #standup thread or reply to existing one

### Blocker Triage

1. `list_issues(status: "Blocked")` or `list_issues(cycle_id: ..., status: "Blocked")`
2. For each blocked issue: `get_issue` → check description for blocker context
3. `add_comment` with diagnosis and @mention the right bot
4. `update_issue` with clarified description or reassigned owner

### Issue Decomposition

1. `create_issue` for the parent task (the epic/milestone)
2. For each sub-task: `create_issue` with `parent_id: <parent_issue_uuid>`
3. All sub-issues inherit project and cycle from parent automatically

### State Transition

1. `list_states` → get all state UUIDs (do once, store in memory)
2. `update_issue(issue_id: ..., state_id: <uuid>)` → move to target state
3. Confirm with `get_issue` if needed

### Assignment

1. `list_members` → get UUID for the target person
2. `update_issue(issue_id: ..., assignee_id: <uuid>)`

### Archive Stale Work

1. `list_issues(status: "Cancelled")` or `list_issues(status: "Todo")` filtered by age
2. `archive_issue(issue_id: ...)` — preferred over deletion
3. If `issueArchive` is unsupported by the API, fall back to:
   `update_issue` with state set to a designated "Archived" custom state (check with `list_states`)

---

## 4. Anti-Patterns to Avoid

- **Never create an issue without knowing the appetite** — ask first
- **Never leave priority, estimate, or project blank** if they can be inferred
- **Never close a blocked issue** without resolving or explicitly deferring the blocker
- **Never skip `list_states`/`list_members`** before using UUIDs — stale UUIDs cause silent
  failures
- **Don't pile work into a cycle** without checking current cycle load first (`get_cycle`)
