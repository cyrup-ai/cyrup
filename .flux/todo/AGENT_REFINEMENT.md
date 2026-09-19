---
stage: new
status: pending
updated: 2026-09-19
---

# `refine` / `refine.show` / `refine.rollback` — an agent that improves itself from its own run evidence

OBJECTIVE: port the WRITE half of `agent-refinements.ts`, so cyrup can **generate** and **revert** the
refinement overlay it already applies on every spawn. Closes `VL-S13` and 3 of the 12 verbs still
missing from `SUBAGENT_ACTIONS` (47 → 50).

## The gap — verified in the tree, and self-documented

`crates/cyrup-ext-subagents/src/exec/agent_refinements.rs` (673 lines) opens with:

> *"a port of pi-subagents' `src/agents/agent-refinements.ts` @v0.43.0, **restricted to the READ half**
> that the spawn path needs … Upstream's WRITE half — `collectBoundedRefinementEvidence`,
> `validateRefinementProposal` and `handleRefinementAction` … is a separate v0.43.0 management surface
> that this crate does not yet register."*

and later: *"an overlay file authored by any means (upstream, or by hand) is applied exactly as upstream
applies it."* So the overlay shapes every child's system prompt, and **only pi or a text editor can
write one.** `grep "write_refinement|metadata_for|hash_prompt"` over that file returns nothing.

**This is the fourth reader-without-writer in this crate.** The handoff manifest was the first
(PR #143), the recovery descriptor and the child transcript the second and third (PR #144). This is
the last one I know of.

## What ALREADY exists (grep before you build — three quarters of this is done)

In `exec/agent_refinements.rs`, all public and tested:
- `RefinementBase:72`, `RefinementEvidenceLimits:83`, `RefinementMetadata:92`,
  **`RefinementSnapshot:102`** (the rollback data model), `ParsedRefinementFile:115`
- `get_agent_refinement_path:204`, **`parse_refinement_file:252`**, `append_agent_refinement_overlay:366`
- `CURRENT_FENCE` / `SNAPSHOTS_FENCE:52` — the on-disk format, kept byte-identical to upstream
  *on purpose*, because it is a FILE FORMAT shared with pi

So the parser, the snapshot model and the overlay application are done. **The writer must round-trip
that exact parser** — that is the strongest available correctness check and the spec requires it.

## Upstream, pinned at v0.68.0

Read ONLY via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.
`src/agents/agent-refinements.ts` is 624 lines. The three missing pieces:

### 1. `collectBoundedRefinementEvidence:349`
Walks `state.asyncJobs`, filtered to this cwd and to jobs whose `agents`/`steps[].agent` names the
target agent, newest first, each row `withinAge(...)` and `pushCapped(...)`. Emits
`{ id: "live:<asyncId>[:<index>]", source: "live-state", runId, agent, at, status, refs }`.
**cyrup's source is `tui::fleet_state::FleetState::tracked_jobs: Vec<AsyncRunView>`** (`:489`), whose
rows carry `paths`, `status` (a `RunStatus`, which holds the step list and per-step agent),
`session_id`, `description`, `context`, `nested_children`. Read `RefinementEvidenceLimits` — the
caps are already typed in cyrup.

### 2. `validateRefinementProposal:448` — **THIS IS A SECURITY CONTROL, NOT A LINT**
The overlay it gates is folded into **every future spawn's system prompt**. A proposal child that
could write *"ignore the acceptance instructions"* into it would have escalated its own privileges
for every later run of that agent. Upstream refuses:
- more than **3** edits;
- any edit missing `title` / `guidance` / `rationale`;
- any edit citing **zero** evidence ids, or an id **not in the packet** (`allowed` set);
- guidance containing ``` ``` ``` or `</pi-subagents-refinement>` (fence/tag escape);
- guidance matching the blocked regex — `all agents`, `every agent`, `global`,
  `(disable|ignore|bypass|skip|override) [the] (acceptance|output|safety|policy|policies|tools?|developer|system) [instructions]`,
  `<protected> (instructions|overrides)`, `tool safety`, `review gates`, `rewrite base`,
  `base agent file`, `settings.json`, `.pi/agent`, `agents/*.md` — case-insensitive.

**Port every arm.** In Rust the regex is a `LazyLock<Regex>` with a test per alternative, not a
hand-rolled matcher. `additionalProperties: false` on the schema is part of the control.

### 3. `handleRefinementAction:546` — the three verbs
- **`refine.show`** — read-only. Prints path, revision, updatedAt, base source+filePath, **`Base
  prompt changed since overlay: yes/no`** (compares `metadata.base.systemPromptSha256` against a
  fresh `hashPrompt(agent.systemPrompt)` — the drift check), the current guidance, and the last
  **5** snapshots.
- **`refine.rollback`** — takes `snapshots.at(-1)`, writes `current = latest.before`, and **appends a
  new snapshot** `{ action: "rollback", before: <current>, after: latest.before, evidenceIds: latest.evidenceIds }`
  at `revision + 1`. It does NOT pop history; a rollback is itself a revision. Refuses when no
  overlay or no snapshot exists.
- **`refine`** — collects evidence; **if empty, launches nothing and writes nothing** (exact
  sentence at `:593`); otherwise launches a read-only proposal child with `proposalTask(...)` and
  `proposalSchema()`, validates, and on success writes `current = guidanceFromProposal(...)` (each
  edit as `- <guidance>`) plus a `refine` snapshot at `revision + 1`. Every failure path — child
  error, invalid proposal, zero edits — **writes no overlay** and says so.

Also needed: `writeRefinementFile`, `metadataFor`, `hashPrompt`, `readExisting`, `resolveOneAgent`,
`proposalFromChild:519` (structuredOutput first, then a fenced-JSON fallback), `PROPOSAL_AGENT`.

## cyrup seams

- **Dispatch**: `extension/tool/routing.rs`'s `route_action`; the action list is
  `extension/tool/text.rs`'s `SUBAGENT_ACTIONS` (47 entries — add 3 at upstream's own indices).
- **Authority**: upstream puts `refine` and `refine.rollback` in `MUTATING_MANAGEMENT_ACTIONS`
  (`subagent-executor.ts:213`) and `refine.show` is read-only. `registration/authority.rs` is live
  (it gained `worktree.discard` in PR #143) — wire these the same way, and keep `refine.show`
  reachable from child-safe fanout while the two mutators are refused there.
- **The proposal child**: it is read-only, schema-constrained and must not read files. Two existing
  precedents — the tool surface's own `structured_output_schema` path
  (`extension/tool/routing.rs:825`, `task_items.rs:286`) and the watchdog's in-process nested turn
  (`watchdog/agent_turn.rs:430`, `Agent::builder` at `:451`, which already enforces a read-only tool
  list and an execution-time refusal). **Pick one deliberately and say why**; upstream's
  `LaunchRefinementProposalChild` is a real subagent launch, not an in-process turn.
- **Slash command**: upstream's error text names `/subagents-refine <agent>`.
  `registration/slash_commands.rs` holds the `SlashCommandName` enum and `SLASH_COMMANDS` table.
- **Agent resolution**: `resolveOneAgent` → cyrup's `discovery` agent resolution, including the
  unknown-agent diagnostic.

## Definition of done

1. All three verbs advertised AND dispatched (the crate's advertise-vs-dispatch invariant), reachable
   from the production tool surface, authority-gated as upstream gates them.
2. `refine` on an agent with real run evidence launches a proposal child and writes an overlay that
   **`parse_refinement_file` reads back** — the round-trip is the correctness proof.
3. `refine.rollback` restores the previous guidance AND appends a rollback snapshot.
4. `refine.show` reports drift correctly (mutate the base prompt → `yes`).
5. **Every validator arm has a test that proves the refusal**, including one per blocked-regex
   alternative. A proposal that tries to disable acceptance/safety/tool instructions is REFUSED and
   **no overlay is written** — assert the file is unchanged on disk, not merely that an error was
   returned.
6. No overlay is written on any failure path (no evidence / child error / invalid / zero edits).

## Rules

- Port the BEHAVIOUR, adapted to Rust: enums for the action and snapshot kind, newtypes for the
  bounded strings, a typed error, `LazyLock<Regex>` for the blocked pattern. **The on-disk format
  does not change** — the existing parser and pi both read it.
- Every `[CYRUP-DELTA]` states a reason that is TRUE. Grep the premise first.
- No `allow(dead_code)`, no stub, no `todo!()`, no narrowing-and-reporting-done.
- Gates: fmt; clippy `--workspace --all-targets --features test-fixtures -- -D warnings`;
  `nextest run --workspace --features test-fixtures` (baseline **10 598** / 9 skipped);
  `nextest run -p cyrup-it --features it` (baseline **559**).
