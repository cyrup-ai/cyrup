# 09b — cyrup-ext-subagents: the v0.57.0 → v0.71.0 drift window

**This file adds to `09-cyrup-ext-subagents.md` and `09a-cyrup-ext-subagents-v0.57-drift.md`. It
replaces neither of them.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071` and its
Trackers. `09a` remains the file of record for `SUBA-072`…`SUBA-106`, including the ids it filed past
its own scope line, which stay where they are. This file adds **`SUBA-107`…`SUBA-113`** (first pass)
and **`SUBA-114`…`SUBA-143`** (pass 2, 2026-09-24); the next free id is **`SUBA-174`** (`SUBA-164`…`SUBA-173` filed 2026-10-03). Ids are
never renumbered, so an item never moves between the three files.

This file was created because README's pin table (*Which area file is pinned where*, row `09a`)
names a `09b` as the structurally correct home for the pi-subagents window that neither `09`'s nor
`09a`'s `## Scope` covers. That was `v0.57.0..v0.67.0` when the table was written. Upstream has since
tagged v0.68.0, v0.69.0, v0.70.0, v0.70.1 and v0.71.0, so the window is now **`v0.57.0..v0.71.0`**.
The file name keeps the `v0.64` stem the task brief gave it. The scope below is the authority, not
the name.

**Do not add up this file with the census blocks in `09` and `09a` without reading them.** Those
blocks hold UNVERIFIED leads for `v0.57.0..v0.67.0` (2026-09-14). By scope, this file now owns them.
They were **not** re-verified here and are not restated. `## Leads` below says which of them this pass
happened to settle.

> ### CLOSURES 2026-09-28 (second batch) — five lows (`SUBA-133`, `SUBA-136`, `SUBA-137`, `SUBA-138`, `SUBA-140`); two partials (`SUBA-134`, `SUBA-141`)
>
> The suba-surface lane. This work is on `claude/lows-next` and not yet committed. Each row and body section
> carries its evidence. All five closures are fixes. **Unlike the block below, no test in this batch was
> shown red without its fix, and none has been seen to run on the combined tree.** The verifier could not
> build anything, and the integration pass ran only `cargo fmt --all -- --check` (passed, no files changed).
> Clippy and `cargo nextest` were not run because `/` had 2.9 GB free, below the pass's 4 GB floor. Every
> named test exists in the tree; that is all this block claims for them.
> **Counted set after this pass (`count_open_items.py`): 0 critical · 0 high · 8 medium · 7 low = 15 open,
> 2 trackers, 20 closed** (was 8 medium · 12 low = 20 open, 15 closed after the block below; this supersedes
> that block's counted-set line). Open: `SUBA-107`, `SUBA-108`, `SUBA-111`, `SUBA-116`, `SUBA-118`,
> `SUBA-119`, `SUBA-124`, `SUBA-143` (medium); `SUBA-109`, `SUBA-129`, `SUBA-130`, `SUBA-131`, `SUBA-134`,
> `SUBA-139`, `SUBA-141` (low). By row:
>
> - `SUBA-133`: `advertise` is a strict boolean frontmatter field, editable through management and written
>   back. The bounded, XML-escaped `<advertised_subagents>` block is on the parent system prompt at
>   `before_agent_start` while `subagent` is selected, and it is replaced each turn. **Remaining delta:** name
>   ordering is lowercased byte order, not ICU root collation. `_` versus `-`/`.`/digits sorts differently,
>   and that can change which 16 agents make the cut. The fix is a CLDR-root primary-weight map for ASCII
>   punctuation.
> - `SUBA-136`: surfaced requests carry upstream's details, `tui/supervisor_ui.rs` ports `supervisor-ui.ts`,
>   and each reply journals one `subagent_supervisor_reply` entry. Two unfiled neighbours are outside this
>   row: v0.71.0 does not inject no-reply (`progress_update`) requests, and cyrup emits no
>   `INTERCOM_DETACH_REQUEST_EVENT`.
> - `SUBA-137`: the Ghostty gate also requires the trimmed `__CFBundleIdentifier` to be `com.mitchellh.ghostty`.
> - `SUBA-138`: RPC `cost` (the ninth method) and `ping.capabilities.cost` are in, and `/subagent-cost` shares
>   the same v0.71.0 collector. **Ledger correction:** at HEAD, `/subagent-cost` was an older transcript walk, not
>   upstream's collector. The unreferenced R-SA-140 accumulator in `registration/cost.rs` is still to delete.
> - `SUBA-140`: a non-empty `CYRUP_SUBAGENT_CACHE_RETENTION` becomes the child's `CYRUP_CACHE_RETENTION`.
> - `SUBA-134` (partial): derivation, the env hand-off, the child's `set_session_name` and `sessionName` on
>   results are in. The label arm (workflow node / chain step / dynamic task) is not: every caller passes
>   `label: None`. **Ledger correction:** the lane's "nothing is missing within the row" is wrong.
> - `SUBA-141` (partial): the runner-exit half is in (observed close, `exited with code N (signal S)`,
>   upstream's stderr-tail bounds). The Fleet external-CLI tails and step elapsed time are still open, because
>   `StepStatus` has no `externalProcess` and `exec/external_cli` has no live process hook.
>
> No other area file needed a note. `09a`'s pointers to `SUBA-133`/`134`/`136`/`137` (`09a:100-108`) are
> promotion pointers and still hold. `11`'s `ICOM-064` names `SUBA-134` as the claimant, and the child now
> sends that claim.

> ### CLOSURES 2026-09-28 — six lows (`SUBA-112`, `SUBA-126`, `SUBA-127`, `SUBA-128`, `SUBA-132`, `SUBA-135`); no partials
>
> Landed on `claude/lows-next`, not yet committed. Each row and body section carries its evidence. All six
> are fixes, and every closure has a test that the lane showed red without its fix. **The combined tree has
> not passed the gates.** The integration pass ran only `cargo fmt --all -- --check`, which passed and changed
> no files. Clippy and `cargo nextest` were not run on the combined diff, because `/` had 2.9 GB free, below
> the pass's 4 GB floor (`target/` is 24 GB). Until they run, treat "green" as per-lane only.
> **Counted set after this pass (`count_open_items.py`): 0 critical · 0 high · 8 medium · 12 low = 20 open,
> 2 trackers, 15 closed** (was 8 medium · 18 low = 26 open, 9 closed). By row:
>
> - `SUBA-112` — the bundled `worker.md` is byte-identical to `agents/worker.md` @v0.71.0: `defaultContext:
>   fresh` and the three prose drifts. **Ledger correction:** `acceptanceRole: writer` was already at HEAD
>   (`282b9fa2`, with the `SUBA-108` work); `SUBA-108`'s section carries a dated note.
> - `SUBA-126` — a structured-only answer is saved, delivered and chained as pretty JSON. "Blank" is measured
>   after `strip_acceptance_report`, as upstream measures it, and acceptance still reads the raw prose.
> - `SUBA-127` — the capture is read whenever `structured_output` was invoked, a valid value survives a failed
>   or timed-out run, and a rejected call gets the ported, bounded, redacted rejection summary. An interrupted
>   run is still not read (upstream's runner reads it; its foreground does not).
> - `SUBA-128` — `checkpointBeforeDeadlineMs` is ported as a tool parameter and a config key, resolved on the
>   async single launch, and delivered by the runner as upstream's `deadline-checkpoint` steer. The bounds
>   refusal covers every call shape. `SUBA-113` carries a dated note: `UNPORTED_CONFIG_KEYS` is 12 → 11.
> - `SUBA-132` — `SingleResult::tool_budget_blocked` is set from the child's own block message and settles a
>   workflow child `budget_exhausted`. Neighbouring gaps (upstream runner's looser match, the status-step
>   flag, `AbortRecoveryInput::tool_budget_exhausted`) are listed in the row, outside its scope.
> - `SUBA-135` — `preflightLaunchCwd` runs at the tool entry ahead of the mission binding, at the head of
>   `run_sync`, and before any async run exists. **Ledger correction:** the impact was understated: the mission
>   binding created the typo'd cwd and the child ran silently in it.
>
> No other area file needed a note: `09a`'s `SUBA-135` mention (`09a:109`) is a promotion pointer, and its
> claim still holds.

> ### RE-MEASURE — 2026-09-24 (pass 2) — the unread windows, read
>
> **Pins unchanged:** cyrup **`ea23ca2`**, pi-subagents **`v0.71.0`** (clone HEAD
> `v0.71.0-10-g6f1027f7`). Upstream read only as `git -C tmp/pi-subagents diff <tag>..<tag> -- <path>`,
> `git show <tag>:<path>` or `git show <sha>` for a commit inside a tagged window. No cargo run.
>
> **What this pass read.**
> 1. **`v0.67.0..v0.71.0`, line by line:** `src/runs/background/subagent-runner.ts` (the whole
>    1 914-line diff), `src/runs/foreground/execution.ts` (864 lines), `src/runs/foreground/subagent-executor.ts`
>    (1 411 lines), `src/runs/shared/structured-output.ts`, `src/runs/foreground/async-stop-action.ts`.
>    **Read with the Herdr-placement / model-ladder / completion-guard hunks filtered out** (those
>    are `SUBA-100`, `SUBA-109`, `SUBA-107` ground): `src/runs/background/async-execution.ts` and
>    `src/agents/agents.ts`. **Read by commit, hunk by hunk for the behaviour-bearing commits:** every
>    other `src/` file with >100 changed lines — `slash-commands.ts`, `notify.ts`, `child-session.ts`,
>    `pruned-fork.ts`, `subagent-wait.ts`, `wait-completions.ts`, `process-terminal.ts`,
>    `extension/index.ts`, `agent-management.ts`, `async-status-projection.ts`, `child-tool-plan.ts`,
>    `scripted-workflow.ts`, `acceptance.ts`, `shared/types.ts`, and the new `subagent-cost.ts`,
>    `tool-activation.ts`, `runner-http-dispatcher.ts`, `subagent-runner-bootstrap.ts`,
>    `herdr-*.ts`. The CHANGELOG was used as an index only.
> 2. **`v0.57.0..v0.67.0`:** every lead in `09a`'s census and every lead in this file was resolved
>    by reading both sides (dispositions: `09a` `## UNVERIFIED census 2026-09-14` → *Resolution*
>    table; this file's `## Leads`).
> 3. **The ten untagged commits `v0.71.0..6f1027f7`**, read as post-tag leads (`## Post-tag leads`).
>
> **Filed:** `SUBA-114`…`SUBA-143` (two high, eleven medium, sixteen low, one tracker). **Closed:**
> none. `SUBA-144`…`SUBA-148` were filed later by the low-severity batches, and the
> `v0.71.0..v0.74.0` pass filed `SUBA-150`…`SUBA-163`, and batch 6 filed `SUBA-149` in the gap that pass
> left. **Next free id: `SUBA-174`** *(2026-10-03: the `v0.74.0..v0.75.0` pass filed `SUBA-164`…`SUBA-173`; before that the counter read `SUBA-164`)*. `SUBA-113`'s tracker escalated two of its keys
> (`checkpointBeforeDeadlineMs` → `SUBA-128`, `modelResponseAliases` → `SUBA-119`).
>
> **Still unread, stated exactly.** (a) The **`v0.57.0..v0.67.0` `src/` diff was NOT read line by
> line** (207 files, +24 658 / −9 124): this pass resolved the leads the 2026-09-14 census drew from
> it, not the diff itself. A behaviour change in a *modified* file of that window that no census lead
> names is still invisible. Reason: size — it is the largest unread block left in area 09, and a
> leads-first pass could not also walk it. (b) In `v0.67.0..v0.71.0`, the hunks of
> `async-execution.ts` and `agents.ts` that belong to Herdr placement were skipped (`SUBA-100`'s
> closure owns them), and the Node-runtime/jiti/Bun launch-resolution hunks of `async-execution.ts`
> and `subagent-runner-bootstrap.ts` were read but not compared (no cyrup counterpart: cyrup's runner
> is its own binary). (c) The async-workflow-only surfaces (`onChildSettled`, `workflowTerminalProof`,
> public workflow lifecycle events, routine intermediate wake suppression) were not compared hunk by
> hunk, because cyrup refuses async workflows outright (`extension/tool/routing.rs:587`); that
> refusal is `09`'s lead at `09:192`.

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | **`ea23ca2`** (2026-09-24). Merge of #152; the code commit is `df3e9a8` | `git log -1` |
| pi-subagents | **`v0.71.0`**, the newest tag. Clone HEAD is `v0.71.0-10-g6f1027f7`; that past-the-tag range was read by pass 2 as leads only (`## Post-tag leads`) | `git -C tmp/pi-subagents tag --sort=-v:refname \| head -1`; `git describe --tags` |

Every upstream claim here was settled with `git -C tmp/pi-subagents show <tag>:<path>` at a named
tag, or with `git -C tmp/pi-subagents show <sha>` for a commit inside a named-tag window. None was read
from the working tree. Every cyrup claim was read at `ea23ca2`. **No cargo command was run**, because
the machine was busy with a coverage run, so every item here is a static reading.

**The crate's own baseline census at `ea23ca2`** (`grep -rhoE 'v0\.[0-9]+\.[0-9]+'
crates/cyrup-ext-subagents/src | sort | uniq -c | sort -rn`): **v0.68.0 × 840**, v0.43.0 × 668,
v0.64.0 × 377, v0.34.0 × 330, v0.57.0 × 128, v0.47.1 × 97, v0.66.0 × 25, v0.67.0 × 9, and **zero**
citations of v0.69.0, v0.70.x or v0.71.0. The crate is now ported *to* v0.68.0 on most of its
surface; it was at v0.43.0 plus patches on 2026-09-14. **So everything in `v0.68.0..v0.71.0` is
genuine lag. For `v0.67.0..v0.68.0`, check the `@v0.68.0` citations before filing anything as
absent.**

## Scope

**Port measured:** `crates/cyrup-ext-subagents/` at `ea23ca2`, which is 489 `.rs` files and 401 210
lines under `src/` (tests included).

**Upstream windows.** Each was measured in this clone with `git diff --shortstat` and
`git log --oneline --no-merges`:

| window | whole tree | `src/` | non-merge commits | `src/` files added / deleted | read by |
|---|---|---|---|---|---|
| `v0.57.0..v0.67.0` | 496 files, +73 195 / −25 830 | 207 files, +24 658 / −9 124 | 341 | 54 / 5 | **leads only** (the `09` and `09a` census blocks, 2026-09-14). One item, `SUBA-092`, is filed in `09a`. *Pass 2: every lead resolved on both sides; the diff itself still not walked line by line* |
| `v0.67.0..v0.68.0` | 252 files, +11 943 / −8 502 | 97 files, +4 139 / −2 491 | 78 | 11 / 3 | **mostly already ported by cyrup** (its v0.68.0 citations). `09a` filed and closed `SUBA-099`…`SUBA-105` against `@v0.68.0` surfaces: machine placement, `inheritGlobalContext`, `mutationTools`, `fast`, capability rows and `acceptance.report`. It holds `SUBA-106` (agent `outputSchema`) open, and that row was re-read by this pass (see below). Apart from that, this pass read only the three deletions and the `Removed` changelog entry |
| `v0.68.0..v0.71.0` | 256 files, +13 752 / −6 103 | 95 files, +3 285 / −2 442 | 80 | 8 / 4 | **this pass**: the CHANGELOG entries for 0.69.0, 0.70.0, 0.70.1 and 0.71.0; every `src/` add and delete; bundled `agents/` diffs |
| **`v0.67.0..v0.71.0`** (the brand-new window) | 386 files, +25 038 / −13 948 | 147 files, +7 377 / −4 886 | 158 | — | the two rows above |
| **`v0.57.0..v0.71.0`** (this file's scope) | 601 files, +90 655 / −32 200 | 238 files, +29 914 / −11 889 | 499 | 70 / 9 | — |

**The `src/` deletions in `v0.67.0..v0.71.0` matter most.** cyrup ported every one of these files, so
each is a candidate `stale-port`:

| deleted upstream | removed at | commit | cyrup still carries |
|---|---|---|---|
| `src/runs/shared/model-exclusions.ts` | v0.68.0 | `f58dfcb5` *refactor: remove automatic model fallback (#2270)* | `exec/model_exclusions/` (5 modules) → `SUBA-109` |
| `src/runs/shared/model-fallback.ts` (renamed to `model-resolution.ts`, 62 % similar) | v0.68.0 | `f58dfcb5` | `exec/fallback.rs::run_fallback_ladder` → `SUBA-109` |
| `src/runs/shared/readonly-model-continuation.ts` | v0.68.0 | `f58dfcb5` | not traced |
| `src/runs/shared/completion-guard.ts` | v0.70.1 | `7c98a696` *refactor: remove inferred no-edit completion failures (#2356)* | `exec/completion_guard.rs` → `SUBA-107` |
| `src/runs/shared/task-intent.ts` | v0.70.1 | `7c98a696` | `exec/task_intent.rs` → `SUBA-107`, `SUBA-108` |
| `src/runs/shared/llm-intent-arbiter.ts` | v0.70.1 | `7c98a696` | nothing: `llm_intent` / `LLM_INTENT` have zero hits in `crates/`. It was never ported, so nothing is stale |
| `src/runs/shared/completion-evidence.ts`, `readonly-session-evidence.ts` | `v0.68.0..v0.71.0` | not attributed | not traced |

Presence was settled with `git cat-file -e <tag>:<path>` at v0.67.0, v0.68.0, v0.69.0, v0.70.0,
v0.70.1 and v0.71.0.

## Methodology

1. `git diff --name-status v0.67.0..v0.71.0 -- src` gave the adds, deletes and the one rename. Each
   delete was dated by tag with `git cat-file -e`, then attributed with
   `git log --diff-filter=D <window> -- <path>`.
2. The CHANGELOG sections `0.68.0`…`0.71.0` came from `git show v0.71.0:CHANGELOG.md`. They were
   used **only as an index**. Each item below re-reads the source at a tag. A changelog line is a
   claim, not evidence.
3. For each candidate: a cyrup grep for the upstream symbol, config key and env name, then a read
   of the Rust that the grep landed on. An item is filed only when both sides were read. Everything
   else is under `## Leads`.
4. No commit message on either side was taken as evidence. That includes #152's "delegation that
   works end to end" (see `09`'s `SUBA-022` row).

## Open items

> Kept under the heading `## Open items`, with the standard `ID | Severity | Kind | Effort | Title`
> table, as README's *Item format* requires. **`09a`'s `## Summary — confirmed items` shape is NOT
> copied**, because README requires one `## Open items` table per file. `scripts/count_open_items.py`
> reads this file as area `09b` since the 2026-09-24 synthesis pass.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~SUBA-110~~ | ~~high~~ **CLOSED 2026-09-24** | upstream-drift | S | Git routing variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_CONFIG_*`, …) are not removed from the background runner's environment or from an inherited external-CLI environment — **CLOSED 2026-09-24** (`c936d8c`): new `spawn/git_env.rs` ports `git-environment.ts` (the 16 names plus `GIT_CONFIG_{KEY,VALUE}_<n>`, case-insensitive); applied before the overlay in `background/spawn_detached.rs` and on the inherited arm of `exec/external_cli/run.rs`; an adapter allowlist is left as written. Verify: `spawn::git_env::tests` (predicate; removals-then-overlay; a real `git rev-parse` resolves the parent repo through an inherited `GIT_DIR` and the child's own repo once filtered). The two call sites are not driven by a test (`#![forbid(unsafe_code)]` rules out setting `GIT_DIR` on the test process). |
| ~~SUBA-107~~ | ~~medium~~ **CLOSED 2026-09-29** | stale-port | M | The completion-mutation guard still fails a successful run because its task wording "looked like" an implementation task. Upstream deleted the guard at v0.70.1 — **CLOSED 2026-09-29**: the guard is retired, matching upstream: `pi-subagents 7c98a696` deletes `src/runs/shared/{completion-guard,task-intent,completion-evidence,llm-intent-arbiter}.ts` and first appears at tag v0.70.1, present at v0.71.0. `apply_completion_guard` is gone from `run_sync`'s gate sequence, `exec/completion_guard.rs` and `exec/task_intent.rs` are deleted, `completionGuard` is out of `KNOWN_FIELDS`, `apply_agent_config`, `RecoveryDescriptor` and `AgentDefinition`, and the acceptance `>= Checked` rung no longer caps on `completion_guard.triggered`. The two helper families that SURVIVE upstream were re-homed rather than deleted, to the files upstream keeps them in: `is_mutating_bash_command`/`is_mutating_tool` to `exec/control.rs` (upstream's `src/runs/shared/long-running-guard.ts:138,155`) and the output write-capability predicate to `exec/output.rs` (`src/runs/shared/single-output.ts:6-14`, twelve names in source order). Verify: `cyrup-it subagents::completion_guard_retirement::an_implementation_task_with_no_edits_finishes_clean`, `::the_retired_completion_guard_symbols_have_zero_hits_in_the_workspace`. |
| ~~SUBA-108~~ | ~~medium~~ **CLOSED 2026-09-29** | stale-port | M | Acceptance-level inference still reads task wording and agent-name regexes. At v0.70.1 upstream infers only from the declared `acceptanceRole` — **CLOSED 2026-09-29**: inference reads the declared `acceptanceRole` and nothing else: `exec/acceptance/model/level.rs::infer_level` is verbatim `pi-subagents v0.71.0 src/runs/shared/acceptance.ts:81-122`, diffed branch by branch. `AcceptanceContract::resolve_effective_for_role` now runs upstream's whole `:505-517` resolution -- the policy-presence upgrade, the MAX, the evidence union against the UPGRADED inferred level, criteria re-normalized against the merged evidence, the review fallback and `:521-522`'s none-level clear -- rather than a bare level MAX, and `AcceptanceConfig` gained `report` so `explicit_acceptance_requests_policy` reads all seven modeled non-level keys (report, criteria, evidence, verify, review, stop_rules, reason). An explicit policy therefore INHERITS the inferred branch's criteria and `requiredEvidenceForLevel`, which is what `resolveEffectiveAcceptance` gives for `{level:"checked"}` with `acceptanceRole: undefined` and what `evaluateAcceptance` (`:1476-1633`) then gates on, where cyrup's explicit contract previously gated on nothing. Verify: `exec::acceptance::model::level::tests::inference_is_invariant_across_task_text_and_agent_name` (3 roles x 3 launch shapes x 3 task texts x 2 agent names), `cyrup-it subagents::read_only_acceptance_inference::a_lowered_policy_survives_a_declared_read_only_role_at_the_live_seam`, `::a_report_only_policy_upgrades_a_declared_read_only_role`, `::an_empty_policy_does_not_upgrade_a_declared_read_only_role`. |
| ~~SUBA-111~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | Agent `allowedAgents` (frontmatter and `agentOverrides`, v0.70.0) is unported. The frontmatter key is silently kept as an extra field, so a declared delegation restriction fails open — **CLOSED 2026-09-29**: `allowedAgents` is ported on both declaration paths and enforced on the child. `discovery/frontmatter.rs:989-1017` parses the frontmatter key through the shared capability-ceiling normalizer with its verbatim message -- `pi-subagents v0.71.0 src/runs/shared/capability-ceiling.ts:66-95`: trim, `^[A-Za-z0-9_.:-]+$`, <=128 UTF-8 bytes per entry, <=256 entries, deduplicated, SORTED -- as `src/agents/agents.ts:2118-2119` does, and `allowedAgents` joined `KNOWN_FIELDS` so it stops leaking to `extra_fields`; `AgentOverrideConfig::allowed_agents` is an `OverrideField` whose `ExplicitClear` is pi's `delete next.allowedAgents` (`agents.ts:1140-1141`, where a JSON `false` clears the bound); `exec/spawn_plan.rs::env_orchestration` builds the agent-sourced ceiling `{version:1, allowedAgents:[...], denyExtensions:false, sources:["agent:<name>"]}` and intersects it into what crosses the process boundary AFTER planning (`src/runs/shared/child-launch.ts:185-187,:211`), so an agent's own bound never narrows its own tool plan; and the bound rides the `RecoveryDescriptor` and its launch-binding projection and is emitted by `discovery/management/frontmatter_write.rs` so a management rewrite cannot delete it. Verify: `exec::spawn_plan::tests::an_agents_own_allowed_agents_becomes_an_agent_sourced_ceiling_on_its_child`, `::an_agent_sourced_ceiling_can_only_narrow_the_inherited_one`, `::an_agent_naming_other_agents_must_still_launch_itself`, `discovery::frontmatter::tests::allowed_agents_frontmatter_parses_normalized_and_is_never_demoted`, `discovery::merge::tests::an_allowed_agents_override_sets_a_bound_and_a_json_false_clears_one`. |
| SUBA-109 | low | stale-port | M | `fallbackModels`, the same-launch model ladder and persistent model exclusions are still live. Since v0.68.0 upstream rejects the key by name |
| ~~SUBA-112~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `crates/cyrup-ext-subagents/resources/agents/worker.md` is now byte-identical to `agents/worker.md` @v0.71.0 (`diff` against `git show v0.71.0:agents/worker.md` is empty): `:11 defaultContext: fresh`, plus the three prose drifts the Fix listed (the `First read the provided context…` paragraph, the `contact_supervisor`-unavailable fallback, the source-discoverability bullet). A worker launched with no `context` now resolves to fresh even when a fork is available. Upstream `agents/worker.md:5,:11` @v0.71.0, CHANGELOG 0.71.0 (#2384). Verify: `cyrup-ext-subagents tests::discovery_integration::the_bundled_worker_starts_fresh_and_is_a_writer` (red without the fix). **Ledger correction:** `acceptanceRole: writer` (`:5`) was already at HEAD `48da124d`; it landed in `282b9fa2` with the `SUBA-108` work. Only `defaultContext: fork` and the three prose drifts were still stale. — *Original:* The bundled `worker` still has `defaultContext: fork` (upstream: `fresh`, v0.71.0) and has no `acceptanceRole: writer` (v0.70.1) |
| SUBA-113 | tracker | tracker | — | Thirteen `config.json` keys are declared unported in source (`registration/mod.rs::UNPORTED_CONFIG_KEYS`), which says "the ledger carries them". No ledger item did until this row — **2026-09-28:** `checkpointBeforeDeadlineMs` left `UNPORTED_CONFIG_KEYS` with `SUBA-128`'s closure (on `claude/lows-next`); the const now holds 11 entries |
| ~~SUBA-114~~ | ~~high~~ **CLOSED 2026-09-26** | stale-port | M | Child tool plans are still pruned to the PARENT session's tool registry, and reviewer/scout launches are refused when the parent was started with a narrow `--tools`. Upstream removed the prediction at v0.70.0 — **CLOSED 2026-09-26** (`850eb70`, #156): `b12496b8` ported — `exec/tool_surface.rs` has no parent-registry input (`host_builtin_tool_names`, the partition, the host `read` throw, the review/scout lane refusal, the omission warning and `unavailableHostBuiltins`/`warnings` are gone), and `RunOptions`/`RunnerConfig`/`ExecSingleStepExecutor` no longer carry `host_available_builtins` (an old runner config carrying the key still decodes). The child-side guard is brought to v0.71.0 (#1356): no builtin floor, and the child ABORTS its run at `agent_start` when its real registry lacks a required tool (`prompt_runtime::refresh_tool_diagnostic`). Verify: `cyrup-it --test subagents child_tool_plan_not_predicted_from_parent::*` (5, real parent `AgentSession`). See the section. |
| ~~SUBA-115~~ | ~~high~~ **CLOSED 2026-09-24** | upstream-drift | S | A nested run's stop / interrupt / timeout cascade reaches every live run on the ROOT route, including sibling subtrees it did not launch (v0.68.0 confines it to the issuing subtree) — **CLOSED 2026-09-24** (`7ba9e03`): `background/cascade.rs::is_control_descendant` ports `isNestedControlDescendant`; `settle.rs::cascade_to_descendants` passes this run's id as issuer when `nested_self` is set, for stop, interrupt and timeout. Verify: `background::cascade::tests::a_nested_issuer_reaches_only_its_own_subtree` (red with the filter removed). Reach note: cyrup does not yet mint a nested route for its own children (`exec/spawn_plan.rs:993`), so today the cascade only has a route when cyrup was itself launched under an inherited one. |
| ~~SUBA-116~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | A paused async run cannot be stopped (refused as "No running or queued async run"), so it keeps its active-capacity slot until resumed — **CLOSED 2026-09-29**: a paused async run can be stopped and releases its slot. `background/control.rs::seal_paused_run` (:1090-1259) ports `sealPausedRun` from `pi-subagents v0.71.0 src/runs/foreground/async-stop-action.ts:31-101`, behind upstream's widened `pausedWholeRun` guard at `:120-121` (`state == Paused && child_id.is_none()`, so a child-scoped stop is still refused): the five refusal rungs in upstream's order -- `status.processTerminal?.runnerProcessInstanceId`, the sidecar proof matched on state+runId+instance with upstream's `unknown (<reason>)` / state-word / `missing` interpolation, `sessionId`, `resultPayloadPathForSessionRun`, then parse and identity -- delivery before seal (`:149-151`), the step rewrite whose `!is_terminal()` is byte-equal to upstream's `pending\|running\|paused` with `endedAt` assigned rather than filled, the per-entry result rewrite indexed against the step list with `exitCode: 1`, the run-level rewrite, and `updateActiveRunIndex(..., "stopped", ...)`. `Paused -> Stopped` is a first-class terminal transition (`background/state.rs:213`), so the run stops holding its active-capacity slot until resumed, and the refusal sentence an operator actually hits (`:154`) is now pinned end to end at the action boundary. No runner change was needed: `runner_main/finish.rs:60-67` maps `LoopOutcome::Interrupted` to `Paused` and then EXITS, so a cyrup `Paused` run has no live runner for upstream's widened `stopRunner` to reach. Verify: `background::control::tests::stopping_a_paused_run_seals_it_stopped_and_releases_its_capacity_slot`, `::a_child_scoped_stop_of_a_paused_run_is_still_refused`, `::a_paused_run_whose_terminal_proof_is_not_observed_keeps_the_stop_request_and_refuses`, `background::state::tests::stopped_is_a_first_class_terminal_run_state_not_an_alias`, `extension::executor::foreground_actions::stop::tests::control_stop_admits_a_paused_run_and_reports_the_seal_that_is_not_ready_yet`. |
| ~~SUBA-117~~ | ~~medium~~ **CLOSED 2026-09-27** | parity-bug | S | The worktree clean-tree check does not exclude the crate's own `.cyrup-subagents/` project directory, so the crate's own chain-run / refinement / schedule files make `worktree: true` refuse a clean repository — **CLOSED 2026-09-27**: the clean-tree probe excludes the crate's own project artifact root (`crates/cyrup-ext-subagents/src/spawn/worktree.rs:495-498`, `:491-493` recording the pathspec resolution). Verify: `cyrup-ext-subagents spawn::worktree::tests::create_worktrees_ignores_the_project_artifact_root`. |
| ~~SUBA-118~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | Abort recovery is unported: a child whose provider/transport aborted after compaction settled, with useful progress, fails instead of being resumed once — **CLOSED 2026-09-29**: abort recovery is dispatched, not merely decided -- the 27-line comment at `exec/fallback.rs` asserting it could not be wired is deleted with its false premise. `DriveOutcome` now carries `after_compaction_settlement` (pi's `AFTER_COMPACTION_SETTLEMENT` stamp, `pi-subagents v0.71.0 src/runs/foreground/execution.ts:1418-1420`) and a `tool_budget_blocked` latch out of every settle path; `AttemptRecord` carries both plus a per-attempt `structured_output_failed` computed at upstream's exact position in the diagnosis chain, between `:1445-1447` and `:1472-1493`; `AttemptRunner` gains `plan_abort_recovery`/`apply_abort_recovery` on the existing `wait_startup_retry`/`apply_startup_outcome` split, with a default that settles on upstream's own `retained session unavailable`; and `classify_attempt` gains a `ResumeAbort` rung at upstream's precedence -- below the startup ladder, because upstream's startup loop lives INSIDE `runSingleAttempt` at `:1518` and is fully spent before the abort loop wrapping it gets a say, and above the context-overflow classification, which upstream decides only after its loop breaks (`:1905`). The shell latches `abort_resumed` per candidate (pi's `attemptIndex > 0`), pushes `AttemptNote::AbortRecovery` whose `Display` is `:1881` verbatim, swaps the attempt's task text to `ABORT_RECOVERY_PROMPT` while the same `--session <file>` argv makes it a resume, resets the startup counter, relaunches the same candidate, and joins the plan's diagnostic onto the attempt's error and the serialized `ModelAttempt` row (`:1886-1888`); `usage_budget_exhausted` and `acceptance_failed` are spelled out as the foreground literals at `:1885` and `:1887` rather than silently omitted. Verify: `tests::abort_recovery_integration::a_compaction_induced_abort_is_resumed_once_with_the_recovery_prompt_and_the_run_succeeds`, `::the_same_abort_without_the_compaction_evidence_settles_with_no_relaunch`, `::a_child_its_own_tool_budget_stopped_is_not_resumed`, `::a_declared_schema_with_no_captured_value_refuses_the_resume_and_says_why`, `exec::fallback::tests::the_resume_rung_sits_below_startup_retry_and_above_context_overflow`, `::a_timeout_a_detach_and_a_success_all_outrank_the_abort_recovery_rung`. |
| ~~SUBA-119~~ | ~~medium~~ **CLOSED 2026-09-29** | not-ported | M | A native child that reports a different model than the launch candidate is accepted silently: no `model_verification_failed`, no `modelResponseAliases` — **CLOSED 2026-09-29**: a child reporting a different model than the launch candidate now latches the verification error. `exec/model_verification.rs::format_subagent_model_verification_error` reproduces every clause of `pi-subagents v0.71.0 src/runs/shared/model-resolution.ts:13-33` in order -- the empty-registry off-switch, the alias match against the RAW observed id, base equality, the `fullId` lookup, `id` equality and both id leaves -- and the `verifyModel` gate is `(!model_override_from_parent).then(\|\| plan.model_arg.clone())` at `exec/attempt_runner.rs:671` (upstream's extra `Boolean(candidate)` conjunct is unreachable because `spawn_plan.rs:465` builds `model_arg` from a resolved non-empty `ModelId`). `modelResponseAliases` reaches all three production seams and each has its own test -- `extension/executor/chain.rs:110`, `extension/executor/background.rs:821`, `background/runner_main/turn_loop.rs:423` -- and `BackgroundStepsSpec.model_response_aliases` is the two-variant `RetainedModelResponseAliases`, matched exhaustively with NO fallback on the `Retained` arm, so `Retained(None)` is upstream's meaningful absence (`src/runs/foreground/subagent-executor.ts:2135-2136`) rather than a silent fall-through to the live config. Verify: `exec::drive_attempt::tests::a_child_reporting_a_different_model_latches_the_verification_error`, `::a_declared_alias_on_the_run_options_clears_the_verification_error`, `::a_message_end_carrying_a_tool_call_is_not_verified`, `extension::executor::background::tests::recovery_descriptor::a_revive_whose_descriptor_declared_no_alias_gets_none_even_when_the_live_config_declares_one`, `background::runner_main::turn_loop::tests::the_runner_configs_alias_map_reaches_the_step_executor`, `extension::executor::chain::tests::a_foreground_chain_walk_carries_the_config_alias_map`. |
| ~~SUBA-120~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | Watchdog findings have no `importance`; every finding above threshold is delivered into the parent model's context, where upstream sends only `high` — **CLOSED 2026-09-27**: `importance` is required with no default and `route_warning` is wired; the false claim that upstream's notice path has no importance filter is corrected at `crates/cyrup-ext-subagents/src/watchdog/types.rs:118-148`, and `parse_test_command` now requires a non-empty remainder after ONE separator (`watchdog/register_main.rs:662`). Verify: `cyrup-ext-subagents watchdog::register_main::tests::exactly_one_trailing_space_is_not_a_test_command_but_two_are`. |
| ~~SUBA-121~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Runtime-registered agents never receive `subagents.defaultModel` / `defaultProvider` / `defaultThinking` or `agentOverrides.<name>` model / provider / fast / thinking — **CLOSED 2026-09-27**: the one line that makes `subagents.defaultModel` / `agentOverrides.<name>` reach a runtime-registered agent (`crates/cyrup-ext-subagents/src/discovery/mod.rs:2074-2076`) is now covered end to end by a real discovery integration test. Verify: `cyrup-ext-subagents tests::runtime_agent_registration_integration::settings_defaults_and_narrowed_overrides_reach_a_runtime_agent_through_discovery`, plus the four `discovery::merge::tests` unit tests. |
| ~~SUBA-122~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Agent-name resolution does not prefer a canonical name over a packaged short name, so `scout` beside `code-analysis.scout` is refused as ambiguous — **CLOSED 2026-09-27**: `resolve_agent_name` is split into a canonical pass and a local-name pass, in that order, each with its own ambiguity message (`crates/cyrup-ext-subagents/src/discovery/mod.rs:578-613`). Verify: `cyrup-ext-subagents discovery::tests::{a_canonical_name_beats_another_agents_local_name_of_the_same_string,two_distinct_agents_sharing_a_local_name_are_a_local_ambiguity_error}`. |
| ~~SUBA-123~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Top-level `subagents.agentScanDirs`, `agentExcludeDirs` and `defaultSubagentOnlyExtensions` are silently dropped: the key census walks only `agentOverrides.<name>` — **CLOSED 2026-09-27**: all three keys are parsed, validated with upstream's message shape, trimmed and applied — `agentScanDirs` (tracked as SUBA-123a), `defaultSubagentOnlyExtensions` (SUBA-123b) and `agentExcludeDirs` (SUBA-123c) — and the key census now warns on an unported top-level key (`crates/cyrup-ext-subagents/src/discovery/{types.rs:955-974,mod.rs:962-966,1283-1288,1342-1366,1500-1501},agent_dirs.rs:186-277`). Verify: `cyrup-ext-subagents discovery::agent_scan_and_exclude_dir_tests::*` (6), `discovery::merge::tests::*default_subagent_only_extensions*` (5), `discovery::tests::{an_unknown_top_level_subagents_key_warns,a_ported_top_level_subagents_key_does_not_warn}`. |
| ~~SUBA-124~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | `mcpDirectTools` cannot resolve servers contributed by settings `packages` or `agentPluginPaths`; a grant naming one resolves to no tools, silently — **CLOSED 2026-09-29**: new `exec/mcp_config_sources.rs` is a line-referenced port of `pi-subagents v0.71.0 src/runs/shared/mcp-config-sources.ts` following upstream's own file split: `isMcpServerDefinition`, `loadPackageMcpServers` with `getConfiguredPackageRoots`/`resolvePackageRoot` (`npm:`, `git:`/scp/URL, `file:`/`~`/absolute/relative), `parseNpmPackageName`, `parseGitPackagePath`, `resolvePackageConfigPath` with both lexical and realpath containment, `loadAgentPluginMcpServers` with the manifest/config `$schema` equality gates, `PLUGIN_NAME_PATTERN`, the per-transport field allowlists, stdio translation (`./` command contained in the plugin root, `${PLUGIN_ROOT}`/`${PLUGIN_DATA}` expansion, the injected vars and their shadow refusal, `resolvePluginCwd`), http translation (URL safety, loopback-only plaintext, WHATWG header name/value and case-collision refusal) and `formatName`. It is wired into `load_mcp_config` at upstream's exact precedence, `mergeConfigs({packageOnly}, mergeConfigs({plugin}, config))` (`src/runs/shared/mcp-direct-tool-allowlist.ts:243-262,:302-321`) -- mcp.json > plugin > package, with a package server whose normalized name a plugin claims dropped outright. `cyrup-mcp` deliberately stays a dev-dependency so resolving `mcp:` selectors cannot drag rmcp/reqwest/oauth2 into a spawn; the two ports are instead held together by a cross-crate conformance test that drives `cyrup_mcp::agent_plugin::load_agent_plugins_in` over the same plugin directory and asserts identical namespaced names and an identical 15-key `configHash` pre-image. Verify: `exec::mcp_direct_tools::tests::an_agent_plugin_server_resolves_its_direct_tools`, `::a_settings_package_server_resolves_its_direct_tools`, `::a_plugin_server_beats_a_package_server_of_the_same_normalized_name`, `::an_mcp_json_server_beats_a_plugin_server_of_the_same_name`, `::a_project_settings_block_does_not_erase_a_global_tool_prefix`, `::a_non_string_agent_plugin_path_is_skipped_and_its_neighbours_still_load`, `::the_plugin_translation_agrees_with_the_adapters_own_loader`. |
| ~~SUBA-125~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Skills marked `disable-model-invocation: true` are still injected into a child that names them — **CLOSED 2026-09-27**: `disable_model_invocation` is carried onto `ResolvedSkill` and filtered at the injection chokepoint (`crates/cyrup-ext-subagents/src/discovery/skills.rs:78,96-100,158,274,302`), and the proactive recommender skips hidden skills too (`:571`). Verify: `cyrup-ext-subagents discovery::skills::tests::{an_agent_naming_a_hidden_skill_launches_without_it_in_the_child_prompt,build_skill_injection_omits_a_hidden_skill,a_hidden_skill_is_not_proactively_recommended}`. |
| ~~SUBA-143~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | L | The runtime-agent registration EVENT bridge (`pi-subagents:runtime-agent-register:v1`) is unported; only native Rust code can register a runtime agent — **CLOSED 2026-09-29**: the registration event bridge is ported. New `discovery/runtime_agent_events.rs` holds the whole of `pi-subagents v0.71.0 src/agents/runtime-agent-events.ts`: `RUNTIME_AGENT_REGISTER_EVENT` and `_VERSION` (`:4-5`, upstream's exact constant), the request builders, `read_registration_reply`/`read_disposal_reply` as the reading half of `registerAgentViaEvents` (`:35-46`, carrying pi's two refusal sentences verbatim), and `RuntimeAgentEventBridge::dispatch` as the body of `registerRuntimeAgentEventListener` (`:49-70`) -- routing to the existing `RuntimeAgentRegistry::register_value`, so every refusal a client sees is the registry's, i.e. upstream's, sentence. Because `SharedBus::emit` queues and fans out BY VALUE (`cyrup-ext/src/bus.rs:80-93`), upstream's mutated-payload return channel does not exist, so the result travels on `...:reply:<requestId>` in upstream's shapes, and the handle upstream returns as `dispose()` (`src/agents/runtime-agent-registry.ts:386-397`) is a `Uuid::new_v4` token plus a disposal topic -- random rather than a counter so one client cannot dispose another's registration, naming exactly one RECORD (upstream's `entry !== record` identity, not name equality), idempotent and infallible like `dispose(): void`, and removed rather than tombstoned so a register/dispose loop cannot grow the bridge. The `requestId` is validated FIRST because the reply topic is built from it, and only then the version, refused with upstream's sentence including its `String(request.version)` interpolation. Both topics are subscribed in the `RegistrationMode::Full` arm only, and the token map is cleared in the `SessionShutdown` arm right after `teardown_session()` runs pi's `clearRuntimeAgentsForPi` (`src/extension/index.ts:1042`). Verify: `tests::runtime_agent_event_bridge_integration::a_sibling_extension_registers_an_agent_over_the_bus_and_discovery_finds_it`, `::the_registration_token_disposes_the_agent_and_is_idempotent`, `::session_shutdown_clears_the_registry_and_the_outstanding_tokens`, `::a_forged_reply_topic_is_never_written_to`, `::a_child_safe_registration_answers_no_runtime_agent_event_at_all`, `discovery::runtime_agent_events::tests::an_unsupported_version_is_refused_with_upstreams_sentence`, `::a_stale_token_never_removes_a_later_registration_of_the_same_name`. |
| ~~SUBA-126~~ | ~~low~~ **CLOSED 2026-09-28** | not-ported | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `run_sync` reads the structured value before the output handoff and, when the prose is blank, substitutes `serde_json::to_string_pretty(value)`, so the value is saved to the output file, delivered, and passed on as `{previous}` (`crates/cyrup-ext-subagents/src/exec/mod.rs:739-758`, ahead of `resolve_saved_output` at `:772`). Acceptance still reads the child's raw prose (`:786-790`). Upstream `execution.ts:1513-1516`, `subagent-runner.ts:1372-1376` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_structured_only_answer_is_saved_and_delivered_as_pretty_json,prose_that_is_only_an_acceptance_report_still_delivers_the_structured_value}` (red without the fix). The verifier fixed one gap: "blank" is measured after `strip_acceptance_report`, as upstream measures it after `stripAcceptanceReport`; before that, prose that was only an acceptance-report block saved the machine report to the output file and delivered nothing. — *Original:* A child that answers only through `structured_output` delivers an empty output and saves an empty output file; upstream substitutes the pretty-printed JSON |
| ~~SUBA-127~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): the capture is read whenever the winning attempt's events show a `tool_execution_start` for `structured_output` (`crates/cyrup-ext-subagents/src/exec/structured.rs:401` `structured_output_tool_invoked`; `exec/mod.rs:1716-1777` `GateState::apply_structured_output_value`), and a valid value survives a failed or timed-out run. An invoked call that captured nothing fails with the ported `formatStructuredOutputRejectionError` (`exec/structured.rs:526`): bounded to 4096 bytes (`:392`, `utf8_prefix` `:413`) and redacted by `sanitize_structured_output_rejection` (`:485`) with upstream's regexes, marker and fallbacks. The missing-call error is kept for a child that never invoked the tool, and either error is reported only on an otherwise-clean run. `validate_structured_output` (`:73`) now returns the bare message, which removes the doubled prefix from the child tool and from dynamic collection. The verifier also corrected the `AgentProgress::all_events` doc (`exec/progress.rs:62-66`), which still said structured output never reads it. Upstream `structured-output.ts:11-80,388-405`, `execution.ts:1036-1038,1449-1471`, `subagent-runner.ts:1245-1270` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_rejected_structured_output_call_fails_with_the_rejection_summary,a_valid_structured_value_survives_a_later_failure,a_child_that_never_invokes_structured_output_gets_the_missing_call_error}` plus the `exec::structured` unit tests (bare message, capitalised read-back); red without the fix. Two shape differences, neither a gap in this row: an interrupted run is not read (as upstream's foreground, `execution.ts:1421-1435`; upstream's runner still reads an invoked capture on an interrupted run, `subagent-runner.ts:1252`), and the parent reads the capture file where upstream's foreground uses an in-process capture. — *Original:* Structured output is read only on a clean exit: a rejected call is reported as "Missing structured_output call", and a valid value is discarded when a later provider error fails the run |
| ~~SUBA-128~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): ported end to end. Tool parameter `checkpointBeforeDeadlineMs` with upstream's bounds and description (`crates/cyrup-ext-subagents/src/extension/tool/schema.rs:553`, `params.rs:227`). Config key carried raw and validated with upstream's sentence (`registration/mod.rs:551,877-886`, checked first in `validate_raw_config`, `:840`), and dropped from `UNPORTED_CONFIG_KEYS` (12 → 11, `:559`). `params ?? config` is resolved on the async single launch only (`extension/executor/background.rs:389-393`, upstream `executeAsyncSingle`). The detached runner arms `remainingMs - checkpoint` when that is at least 1 s and queues upstream's verbatim `source: "deadline-checkpoint"` steer, unless the run already timed out, was stopped or interrupted, or has no running step (`background/runner_main/control_watcher.rs:593,630`, armed from `entry.rs:186`). The verifier moved the bounds refusal (0 or above 2147483647) from the async single path to the tool entry, ahead of every mode (`extension/tool/mod.rs:254-264`), because pi's argument validation refuses an out-of-range value for every call shape. Upstream `schemas.ts:363`, `config.ts:145-151,215-230`, `subagent-executor.ts:3460`, `async-execution.ts:2104`, `subagent-runner.ts:210,3379-3406` @v0.71.0. Verify: `cyrup-ext-subagents tests::deadline_checkpoint_runner_integration::a_running_child_receives_the_deadline_checkpoint_and_hands_off_before_the_kill`, `background::runner_main::control_watcher::tests::{the_deadline_checkpoint_reaches_the_running_child_before_the_deadline,an_ended_or_idle_run_gets_no_deadline_checkpoint,the_checkpoint_delay_is_the_remaining_time_less_the_checkpoint_and_at_least_a_second}`, `extension::executor::background::tests::checkpoint_before_deadline_launch::the_call_wins_the_config_default_fills_in_and_a_bad_value_is_refused` (extended with foreground single and chain refusals), `registration::tests::checkpoint_before_deadline_ms_is_read_and_validated`; red without the fix. Outside this row: an invalid config warns and falls back to defaults (`crates/cyrup/src/subagent_config.rs:68-70`, the warning names the key) where upstream `loadConfig` throws, and the out-of-range tool message is cyrup's own sentence, not TypeBox's. — *Original:* `checkpointBeforeDeadlineMs` (async single tool param and `config.json` default) is unported, so a deadline kills a child with no handoff steer |
| SUBA-129 | low | upstream-drift | M | Typed gates (`gate: { command, output: "json", schema?, timeoutMs? }`, `verify[].output`/`schema`) and `acceptance.preserveStagedIndex` are refused |
| ~~SUBA-130~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | M | **CLOSED 2026-09-30** (on `claude/lows-batch4`): (1) New shared runner `spawn/bounded_argv.rs::run_bounded_argv` (`:130`), the Rust port of `worktree-setup-command.ts::runSetupCommand`: `process_group(0)`, a 1 MiB combined output cap whose overflow kills the program and FAILS the call (`ENOBUFS`; never a truncated result), stop token (`ABORT_ERR`) and absolute deadline (`ETIMEDOUT`) both kill through `signal::terminate_on_timeout` plus a `SIGKILL` of the whole group, and an already-expired bound never spawns. (2) `spawn/worktree.rs`: `GitBounds` (`:265`, stop + deadline + byte budget) rides `CreateWorktreesOptions::bounds` and `WorktreeGroupConfig::bounds`; `run_git_bounded` / `run_git_env_bounded` (`:396,:412`) replace the unbounded `Command::output()` at every call in create, preflight-shaped probe (`resolve_repo_state`), hook validation, diff, patch validation, cleanup and the cleanup gate's probes; `run_git` stays the unbounded default ONLY for the operator-invoked cleanup-plan builder (`cleanup_plan/git.rs`, `classify.rs`) and `worktree.discard` (`handoff/write.rs`), each marked at the site as having no run context. `chain_graph::assign_worktree_cwds` passes `ctx.cancel` + `ctx.deadline_at` (pi `worktree.ts:239-252`). (3) The setup hook now runs through the same runner (`run_worktree_setup_hook`, `:928`), with upstream's `Math.min(deadlineAt, now + hookTimeoutMs)` clamp, so it obeys stop and the run deadline as well as its own timeout (its own timeout keeps its `timed out after Nms` text). (4) Rollback (`GitBounds::for_compensation`, `:322`) keeps only the original deadline and not the launch signal, per `worktree.ts:1305-1325`. **[CYRUP-DELTA, two]** (a) diff capture and cleanup are bounded too (`capture_worktree_diff`, `cleanup_worktrees`, `GitBounds::for_harvest` `:343`, wired in `chain_graph::publish_worktree_handoff`): upstream's `runGit`/`runGitChecked` (`:277-296`) are unbounded `spawnSync`, and a hung git that holds a finished run open past its deadline or ignores stop is strictly worse. Because a run that ended by stop or timeout is exactly the one whose work most needs capturing, the harvest deadline gets a 10 s floor and an already-cancelled token is not obeyed (a stop arriving DURING the harvest still ends a hung git). Patch capture has a 256 MiB budget (`PATCH_CAPTURE_MAX_BYTES`) and an overflow fails the capture (worktree preserved) instead of upstream's silent 1 MiB truncation. (b) `for_compensation` floors the rollback deadline at 10 s from now: upstream hands compensation an already-expired deadline verbatim and reports the leftovers through `writeWorktreeSetupHandoff`, which cyrup does not have (the rollback report is discarded), so an exact port leaks a worktree and a branch every time the deadline fires mid-setup. Verify (all in `cyrup-ext-subagents`, no cyrup-it, no PATH faking; a real hung git is made with a `core.fsmonitor` hook that sleeps): `spawn::bounded_argv::tests::{a_hung_program_is_killed_at_the_deadline, the_stop_token_ends_a_hung_program_promptly, output_past_the_cap_fails_and_kills, an_exhausted_bound_never_spawns, a_descendant_dies_with_its_leader}` and `spawn::worktree::tests::{create_worktrees_with_an_expired_deadline_creates_nothing, a_stop_during_the_setup_hook_aborts_creation_and_rolls_back, the_run_deadline_clamps_the_hook_timeout_and_rollback_still_cleans_up, a_hung_git_in_the_diff_capture_is_ended_at_the_deadline, a_hung_git_in_cleanup_is_ended_by_stop_and_the_worktree_is_preserved, a_harvest_after_stop_and_deadline_still_captures_the_work}`; with the bounds disabled 9 fail (hung programs ran to `sleep 30`: `stop must be prompt: 30.03s`; expired-deadline create returned `Ok(WorktreeSetup{..})`; diff capture `the capture must fail at the deadline: None`; `for_harvest` obeying the run's stop: `an already-ended run's stop is not obeyed`), and with only the compensation floor removed the clamp test fails `only the main checkout may remain` (leaked worktree). *Not closed, filed as `SUBA-147`/`SUBA-148`:* the `preflightWorktreeSource` admission pass and the operator-action/turn-lock bounds. *Earlier text:* Worktree git commands run with no deadline, stop signal or output bound, so a hung `git` outlives the run's deadline and ignores `stop` |
| SUBA-131 | low | upstream-drift | M | An external-CLI step is never flagged `needs_attention` when idle; upstream now tracks its stdout/stderr and Git fingerprint as activity |
| ~~SUBA-132~~ | ~~low~~ **CLOSED 2026-09-28** | not-ported | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `is_tool_budget_blocked_message` is ported as a whole-message match against this run's own hard limit and tool name (`crates/cyrup-ext-subagents/src/exec/tool_budget.rs:282`). `run_sync` sets `SingleResult::tool_budget_blocked` (`exec/run_result.rs:277`) from the winning attempt's `tool_execution_end` events, by each event's own tool name (`exec/mod.rs:799-810`). The flag crosses the runner's `StepResult` waist (`spawn/chain_graph.rs:1311`, `background/runner_main/executor.rs:1301`, `settle.rs:627`), and `WorkflowBudgetSignals::from_single_result` reads it (`workflows/settlement.rs:452,462`). The verifier also moved a misplaced doc comment in `executor.rs` back onto its SUBA-3c test. Upstream `tool-budget.ts:74-94`, `execution.ts:1144-1156`, `subagent-runner.ts:1077,1325-1329,1544`, `workflow-settlement.ts:169-174` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_tool_budget_block_reaches_the_result_and_settles_budget_exhausted,the_blocked_message_counts_only_against_the_childs_own_budget}`, `background::runner_main::executor::tests::tool_budget_blocked_survives_the_step_result_waist_and_settles_budget_exhausted`, `exec::tool_budget::tests::only_this_runs_own_whole_blocked_message_is_a_budget_block`; red without the fix. Neighbouring gaps outside this row: upstream's runner detects the block more loosely (`includes("Tool budget hard limit reached")` over every toolResult, `subagent-runner.ts:1325-1329`) where cyrup's shared `run_sync` applies the strict v0.70.0 (#2302) foreground matcher on both paths; the `toolBudget` state and the status-step `toolBudgetBlocked` are not carried, so `workflows/checklist.rs` still reads None; `AbortRecoveryInput::tool_budget_exhausted` has no production caller. — *Original:* `SingleResult` carries no `toolBudgetBlocked`, so a tool-budget-blocked workflow child never settles `budget_exhausted` (self-declared in `workflows/settlement.rs`) |
| ~~SUBA-133~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `advertise` is a typed frontmatter field, strictly `true`/`false`; any other value skips the file with upstream's message (`crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:1362-1384`). Management create/update can set it (`""` clears it; `discovery/management/config_parse.rs:108-114`, `agent_crud.rs:52-54,262,352`), and it is written back as upstream writes it (`frontmatter_write.rs:60`). New `discovery/advertised.rs` ports `advertised-agent-prompt.ts` (16 agents, 12 288 bytes, 512-byte descriptions, XML escaping, the omitted-count line, strip-then-append; `:17-31,95,157`). `extension/executor/advertised.rs` holds the session state and refresh; `extension/host/native_impl.rs:465-467` starts it at session start, and `:810-838` appends the block at `before_agent_start` while `subagent` is in `selectedTools` (active tools as the fallback) and strips a stale block otherwise. Management mutations refresh it (`extension/tool/routing.rs:2210-2212`). Runtime, disabled and ceiling-denied agents are left out. Verify: `tests::rpc_bridge_integration::advertised_agents_ride_the_parent_system_prompt`, `discovery::advertised::tests::*` (4), `discovery::frontmatter::tests::advertise_is_a_strict_boolean_frontmatter_field`. Remaining delta, larger than the in-source `[CYRUP-DELTA]` note says: `locale_order` (`discovery/advertised.rs:82-111`) lowercases and compares bytes, which is not ICU root collation (`_` sorts before `-`, `.`, digits and letters under `localeCompare`, after digits in ASCII). So `code_review`/`code-review`/`code1` sort differently, and with more than 16 advertised agents the 16 that make the cut can differ. — Agent `advertise: true` and the `<advertised_subagents>` catalog in the parent's system prompt are unported |
| ~~SUBA-134~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch3`): the nested-run display name and run-status's external-runner block landed. **Nested exact-status view:** `NestedRunSummary`/`NestedStepSummary` carry the typed `session_name` (`spawn/nested_events.rs:206,283`; pi `shared/types.ts:1606,1659`), kept by the sanitizer every relayed event passes through (`nested_events.rs:499,597`; `nested-events.ts:311,352`); upstream's start event carries none, and cyrup's matches (`extension/executor/background.rs:1096`). `format_nested_exact_status` ports `formatNestedExactStatus` (`background/run_status.rs:734`; `run-status.ts:322-345`) with `nested_run_display_name` (`run_status.rs:711`; `:171-176`: session name, else agent, else agents joined, else id) and the step names `step.sessionName?.trim() || step.agent`; the descendant tree under it is a new port of `nested-render.ts` (`spawn/nested_render.rs:81,115,267,386` — `formatNestedAggregate`, `nestedRunLabel` with its session-name rung, `formatNestedRunStatusLines`). `status` with an id that is no exact async run resolves an exact nested descendant across the projected registries and renders the view, or upstream's `Nested run id '…' is ambiguous across authorized registries.` error (`extension/executor/status.rs:611`, `nested_control.rs:365`, `run_status.rs:991`; `run-status.ts:435-448`, `run-id-resolver.ts:152-161`). **External-runner block** (`run-status.ts:641-664`, unported as a whole and outside SUBA-141's Fleet scope): under each external-CLI step line, after the timeout-recovery lines, `format_external_cli_runner_lines` renders `Runner: external-cli (<command> <args>)`, `Adapter: <id> v<version> (<executionMode>)`, the per-family `Safety:` line (Codex/Cursor/Claude Code/legacy fallback), `Capabilities:`, the three `Unsupported …:` reasons and `Context handoff: fresh only (…)` off the re-normalized descriptor (`normalizeExternalCliRunnerStatus`), `Runner: external-cli (invalid persisted runner metadata)` when it does not normalize, then `Process:` (only with a pid), `Stdout:`, `Stderr:` and `Final output:` off SUBA-141's live `StepStatus.external_process` (`run_status.rs:501,575,635`). Verify: `background::run_status::tests::{an_external_cli_step_renders_the_runner_block_and_its_live_process, each_adapter_family_renders_its_own_safety_line_and_a_pidless_receipt_its_logs, an_unnormalizable_runner_is_invalid_metadata_and_a_native_step_has_no_block, the_nested_exact_status_names_the_run_its_steps_and_descendants_by_session_name, the_nested_display_name_falls_back_agent_then_agents_then_id}`, `extension::executor::status::tests::status_by_id_renders_a_nested_run_by_its_child_session_name` — the block, sanitizer, display-name and dispatch fixes each shown red when reverted. **Not in this row (recorded, not claimed):** cyrup's runner emits no `subagent.nested.updated`/`completed` events (`nestedSummaryFromAsyncStatus`, `nested-events.ts:1002-1050`, from `subagent-runner.ts:2128`, `async-execution.ts:794`, `stale-run-reconciler.ts:344`) and mints no root route (SUBA-115's reach note), so a cyrup-produced summary carries `session_name` only once that relay is ported; the nested lookup lists every route under `Roots::nested_events` (as `resolves_to_nested_run` already did) rather than upstream's state-scoped routes, and skips the nested prefix rung, `reconcileNestedAsyncDescendants` and the nested transcript view; the nested summary has no `turnBudget`/`model`/`thinking`, so those segments stay empty; run-status's per-step nested tree, the external runner's `Steer: unavailable; external runners do not accept live messages.` line and the `external-job` block (no cyrup external-job runner) are unported. *Earlier text:* **PARTIALLY CLOSED 2026-09-30** (on `claude/lows-batch3`): every remaining item but the nested-run display name landed. **Typed synchronous intercom claim** (replaces the JSON reply topic the project owner rejected): cyrup-ext gains a typed native bus — `InitApi::subscribe_typed_bus` (`crates/cyrup-ext/src/native.rs:622`), `NativeExtension::on_typed_bus_event` (`native.rs:709`), `SharedBus::subscribe_typed`/`emit_typed` (`bus.rs:100,120`, listeners run inline before the emit returns, held `Weak`), registered at load (`facade.rs:674`) and reached by natives through `HostServices::emit_typed_event` (`host/services.rs:578`; live impl `cyrup-session-svc/src/host_services.rs:1829`). The request is the typed `IntercomSessionIdentityRequestV1 { claim(&str), claimed() }` (`cyrup-ext-subagents/src/tui/intercom.rs:815-851`, re-exported from `cyrup-intercom/src/identity.rs:66`); the child runtime subscribes (`prompt_runtime.rs:2399`) and claims its routing name inline (`prompt_runtime.rs:2520`); intercom emits it at `session_start` and reads `claimed()` right after the emit, before any connect is scheduled (`cyrup-intercom/src/extension.rs:905-913`), so the first registration is already under the claim (`connect.rs:556`) — the reply topic, claim window and late-claim re-register are deleted. **Label arm:** `SingleStepSpec` carries `label`/`session_name` (`spawn/chain_graph.rs:205-213`) and `child_session_name` is pi's `step.sessionName ?? deriveChildSessionName({agent, task, label})` (`:221`); labels are threaded from chain files (`discovery/chains.rs:1248`), tool `tasks[]`/`chain[]` items (`extension/tool/task_items.rs:336`), slash `label=` (`registration/slash_commands.rs:1300`) and chain-append (`extension/executor/control.rs:770`); dynamic fan-out members are named from their own item's label or task at expansion (`chain_graph.rs:2180-2210`, `subagent-runner.ts:3561-3567`); a workflow row whose child never launched gets pi's label placeholder from the key's first `run` trace entry (`workflows/child_summary.rs:711`, wired at `extension/executor/workflow_launch.rs:634`; `subagent-executor.ts:5774`). **Pending statuses:** single/parallel entries carry the label-aware name and `label`, an attached root is `<agent>: Attached <runId>` (`background/flat_index.rs:118`, `StepStatus.label` in `records.rs`); settled names land on status entries and results (`runner_main/status.rs:520,574`, `settle.rs:630`, reconciler `reconcile.rs:896`). A dynamic group keeps upstream's unnamed `expand:` placeholder; its members have no status entries in cyrup (the recorded SUBA-093 splice residual) but each member's child and result are named. **Launcher-assigned branch now live:** the async runner puts the step's name into `RunOptions::child_env` (`runner_main/executor.rs:885`), which `resolve_child_session_name` reads for the child env and the result. **Displays:** `StepStatus::display_name`/`child_display_name` (`records.rs:241,254`) drive Fleet step rows, the transcript child hint and step line (`fleet_view.rs:529,690,711`), run-status and run-list step lines (`run_status.rs:480,1003`); the Fleet foreground row prefers the live child's session name (`fleet_view.rs:412`, registered at `extension/executor/foreground.rs:1476`), and a remembered foreground child keeps and shows it (`foreground_history/record.rs:257`, `extension/executor/status.rs:1045`). Verify: cyrup-ext `tests::native_dispatch::a_typed_bus_emit_runs_every_listener_before_it_returns`; cyrup-it `session_identity_claim::{a_claiming_extension_sets_the_intercom_id_over_the_stable_id, the_first_non_blank_claim_wins_and_does_not_outlive_its_runtime}`; subagents `tests::child_prompt_runtime_integration::the_child_names_its_session_and_claims_its_intercom_route`, `background::flat_index::tests::pending_entries_are_named_from_their_label_and_an_attached_root_from_its_attachment`, `background::runner_main::executor::tests::a_runner_step_names_its_child_from_its_label_and_the_foreground_walk_does_not`, `background::runner_main::status::tests::{a_settled_childs_session_name_lands_on_its_status_entry, a_step_result_carries_the_name_its_child_ran_under_else_its_declared_one}`, `spawn::chain_graph::tests::dynamic_group_names_each_member_from_its_own_item`, `extension::executor::control::tests::an_appended_step_is_named_from_its_label`, `extension::tool::routing::tests::a_workflow_child_that_never_launched_is_named_from_its_label`, `workflows::child_summary::tests::step_rows_are_named_by_their_child_session_else_by_the_trace_label`, `background::fleet_view::tests::{fleet_step_lines_show_the_child_session_name_over_the_agent, a_foreground_row_shows_the_live_childs_session_name_over_its_agent}`, `background::run_status::tests::active_run_step_lines_show_the_child_session_name_over_the_agent`, `extension::executor::foreground::tests::register_foreground_controls_derives_the_entry_and_stamps_workflow_identity`, `extension::executor::status::tests::status_by_id_finds_a_remembered_foreground_run_that_is_no_longer_live`, `background::reconcile::tests::genuinely_dead_pid_reconciles_to_failed_and_writes_both_files` — each red with its fix reverted. **CYRUP-DELTA:** a typed-bus listener downcasts to the request type instead of duck-typing `typeof request.claim === "function"` (`native.rs:709`, `tui/intercom.rs:815`). **Remaining:** `nestedRunDisplayName` (`run-status.ts:171-175`) — cyrup has no nested exact-status view (`formatNestedExactStatus`, `run-status.ts:322-345`) and its `NestedRunSummary` (`spawn/nested_events.rs:236`) has no `sessionName`, so there is no surface to name; it lands with that view's port. *Earlier text:* Child sessions get no derived human-readable name (`deriveChildSessionName` → `setSessionName`, `sessionName` in results, the `intercom:session-identity` claim) — **2026-09-27:** the intercom half is in (`ICOM-064`, on `claude/intercom-lows`): answer the `intercom:session-identity` request by emitting `intercom:session-identity-claim` `{version:1, stableId:<routing name>}` before the first `agent_start` — **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`): the base naming landed. `crates/cyrup-ext-subagents/src/exec/child_session_name.rs:35` ports `deriveChildSessionName` (60/80 bounds, `[prompt redacted]` skipped). `exec/attempt_runner.rs:541-555` passes the name to the child as `CYRUP_SUBAGENT_SESSION_NAME`. The child's `prompt_runtime.rs:2719-2733` calls `set_session_name` at `before_agent_start`, using the readable name once intercom has claimed the routing name and the routing name otherwise (`subagent-prompt-runtime.ts:532-550`). The name is carried on `SingleResult.sessionName` (`exec/mod.rs:343`, `background/runner_main/settle.rs:607`, `extension/executor/foreground.rs:1712`) and on pending status steps (`background/flat_index.rs:107`). Verify: `tests::structured_output_results_integration::the_derived_child_session_name_reaches_the_child_env_and_the_result`, `tests::child_prompt_runtime_integration::the_child_names_its_session_and_claims_its_intercom_route`, `exec::child_session_name::tests::*` (2). **Remaining:** (1) the label arm. Every caller passes `label: None`, so workflow children (`subagent-executor.ts:5774`), labelled chain steps (`subagent-runner.ts:828,1913,1967`) and dynamic parallel tasks (`:3562`) get the task excerpt instead of the label. (2) DynamicGroup and ImportAsyncRoot pending statuses carry no `sessionName`, and chain-append does not name appended steps. (3) `resolve_child_session_name`'s launcher-assigned branch (`child_session_name.rs:66-79`) reads `RunOptions::child_env`, which has no production caller, so the branch runs only in tests. (4) Fleet and run-status displays do not yet prefer `session_name` (`fleet-view.ts:79-84`, `run-status.ts:167-172`). **Ledger correction:** the lane's "nothing is missing within the row" is wrong, because this row names "agent name + task or workflow node label" and the label arm is unported. |
| ~~SUBA-135~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `preflightLaunchCwd` is ported verbatim (`crates/cyrup-ext-subagents/src/exec/launch_cwd.rs:19`: does not exist / is not a directory / could not be accessed, plus `(resolved from "<typed>")`). It runs in three places: at the tool entry for a typed `cwd`, ahead of the mission binding (`extension/tool/mod.rs:446-464`); at the head of `run_sync` (`exec/mod.rs:357-366`), which covers the foreground and every runner step; and in `spawn_background_steps`, before any async root, run directory or runner exists (`extension/executor/background.rs:515-521`). Upstream `launch-cwd.ts:3-16`, `execution.ts:1604`, `async-execution.ts:648-650`, `subagent-runner.ts:1061` @v0.71.0. Verify: `cyrup-ext-subagents extension::executor::background::tests::launch_cwd_preflight::{a_missing_single_launch_cwd_is_refused_before_launch_with_its_typed_spelling,a_non_directory_cwd_is_refused_on_the_chain_paths_too}`, `extension::executor::background::tests::an_async_launch_into_a_missing_cwd_is_refused_before_any_run_exists`, `tests::structured_output_results_integration::a_run_into_a_missing_cwd_is_refused_by_name_before_spawning`, the `exec::launch_cwd` unit tests; red without the fix. **Ledger correction:** the impact was understated. cyrup's mission store sits under the project root (`<projectRoot>/.cyrup-subagents/missions`, CYRUP-DELTA), so before the fix the mission binding created the typo'd cwd and the child then ran silently in that new empty directory. Shape differences, all forced by that store location: a typed top-level `cwd` on a chain or parallel call refuses the whole call at the tool entry, even when every task names its own valid cwd (upstream fails each task); no mission record is written for the refused launch (upstream binds first, `subagent-executor.ts:5230`, and records a failed mission); the access-failure arm appends Rust's `io::Error` text rather than Node's. — *Original:* No launch-cwd preflight: a missing or non-directory `cwd` fails at spawn with an OS error instead of upstream's named refusal before launch |
| ~~SUBA-136~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): a surfaced request's details now carry upstream's fields: id, requestId, reason, expectsReply, runId, agent, childIndex, requestBody, replyHint, and childTarget or interview when present (`crates/cyrup-ext-subagents/src/native_supervisor.rs:1233-1234`; `native-supervisor-channel.ts:757-767`). Each reply appends one `subagent_supervisor_reply` custom entry, and a journal failure is logged, not fatal (`:1117-1140`). New `tui/supervisor_ui.rs` ports `supervisor-ui.ts`: the shape checks, the 512/8 000/4 000 UTF-16 bounds with ` [truncated]`, `safeTerminalText`, the headings, a 36-row collapsed cap with a marker row, and heading-only output below 3 columns (`:26-41,380,394`). `extension/host/native_impl.rs:213-221` registers both renderers through the host's live-component tier. Verify: `tests::rpc_bridge_integration::the_supervisor_request_and_reply_cards_are_registered_with_the_host`, `native_supervisor::tests::a_surfaced_request_carries_its_details_and_a_reply_is_journalled`, `tui::supervisor_ui::tests::*` (4). Outside this row and not yet filed: v0.71.0 does not inject no-reply requests (`progress_update`, `native-supervisor-channel.ts:740-744`) and cyrup still does; cyrup emits no `INTERCOM_DETACH_REQUEST_EVENT` per surfaced ask. — No renderer for `subagent_supervisor_request` messages and no `subagent_supervisor_reply` session entry |
| ~~SUBA-137~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `available()` requires macOS, `TERM_PROGRAM` equal to `ghostty` ignoring case, and a trimmed `__CFBundleIdentifier` exactly `com.mitchellh.ghostty` (`crates/cyrup-ext-subagents/src/inspectors/ghostty/plugin.rs:111-116,134-142`; `ghostty/plugin.ts:20-24` @v0.71.0). The env map is the real process env (`inspectors::actions::process_env`), so the bundle id reaches the check in production, and a cmux-style embedder exporting `TERM_PROGRAM=ghostty` is refused. Verify: `inspectors::ghostty::plugin::tests::available_requires_the_standalone_ghostty_bundle_id`. Cosmetic: `plugin.rs:236` still cites `ghostty/plugin.ts:13, both conjuncts`; the case fold is ASCII-only and `trim` does not strip U+FEFF, which no real value reaches. — The Ghostty inspector activates on `TERM_PROGRAM=ghostty` alone; v0.69.0 also requires the macOS host bundle id, so cmux-style embedders stop misfiring |
| ~~SUBA-138~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `/subagent-cost` and the new RPC `cost` method share one port of v0.71.0 `collectSubagentCost` (`crates/cyrup-ext-subagents/src/registration/cost.rs:995`, camelCase `SubagentCostReport` `:732`, `format_subagent_cost_report` `:1243`; called from `extension/executor/reports.rs:172-224`). It counts parent assistant turns and compaction usage; `subagent`/`bg_wait` tool-result details and slash-result messages, deduplicated by `run:`/`session:` identity; and workflow children resolved through the run-dir status, the receipt and artifact `_meta.json` (2 MiB bound, run-id charset check), plus `unresolvedAsyncChildren`. `SUBAGENT_RPC_METHODS` is nine, in upstream's order (`extension/rpc/mod.rs:110-120`, dispatch `:322`); `cost` refuses non-object params, `null` included, with upstream's text; `ping.capabilities.cost = {version: 1}` (`extension/rpc/ping.rs:122`). Verify: `tests::rpc_bridge_integration::{rpc_cost_returns_the_versioned_report_over_the_live_branch,the_bridge_announces_ready_on_session_start}`, `registration::cost::tests::{cost_report_counts_compactions_bg_wait_completions_and_dedupes_by_run_id,cost_report_resolves_workflow_children_through_receipt_and_metadata}`, `extension::executor::reports::tests::run_cost_report_walks_the_session_transcript`. **Ledger correction:** this row said `/subagent-cost` already computed the data. At HEAD it was an older transcript walk (`build_subagent_cost_report`, citing `slash-commands.ts:377-416`), not v0.71.0's collector. Remaining in the same file: the cyrup-only R-SA-140 dual-recursion accumulator (`CostUsage`, `RunMetadata`, `accumulate_meta_tree`, `compute_recursive_cost`, `build_cost_report`, `format_cost_report`, …; `cost.rs:90-600,1282-1310`) has no production caller and no upstream counterpart, and the module doc (`:1-58`) still describes it as `/subagent-cost`. It should be deleted, keeping `find_latest_session_file_by_mtime` (`:611`). — The in-process RPC `cost` method and `ping.capabilities.cost` are unported |
| SUBA-139 | low | upstream-drift | M | The `subagents_enable` lazy loader is unported; the full `subagent` tool is always in the prompt (upstream's unsupported-host fallback) |
| ~~SUBA-140~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): when the parent env (read through the extension's env seam) has a non-empty `CYRUP_SUBAGENT_CACHE_RETENTION`, the child's spawn plan sets `CYRUP_CACHE_RETENTION` to it. Unset or empty writes nothing, so the child keeps inheriting the parent's retention (`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:141-151,594-603`; `child-cache-retention.ts:14-25` @v0.71.0, whose `\|\|` treats empty as unset). Every cyrup child is a spawned process, so the env form covers both of upstream's arms. Verify: `exec::spawn_plan::tests::the_child_only_cache_retention_reaches_the_child_env`. — `PI_SUBAGENT_CACHE_RETENTION` (child-only prompt-cache tier) is unported |
| ~~SUBA-141~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch3`): the Fleet half landed. `StepStatus.external_process` is the typed `externalProcess` (`background/records.rs:179`). The external CLI runner reports its process through a typed live hook — once at spawn (pid, start, log paths) and again at close (`exec/external_cli/run.rs:81,250,497`; pi `onProcess`, `external-cli-runner.ts:340-348,386-399`) — which `run_external_cli` forwards with the launch's runner descriptor as a typed `ExternalProcessUpdate` on the live sink (`exec/external_cli/mod.rs:326`, `exec/agent_config.rs:941,1004`). The async runner routes it through its telemetry channel (`TelemetryMsg::ExternalProcess`, `runner_main/executor.rs:800`) onto the step in `status.json` (`runner_main/status.rs:85,150`; pi `updateExternalProcess`, `subagent-runner.ts:2247-2251`). Pending external-CLI steps declare their runner as upstream does (`background/flat_index.rs:61`, called from `runner_main/entry.rs` and the chain-append path in `turn_loop.rs`). The logs now go into the run's own directory (`RunOptions::external_log_dir`, `exec/agent_config.rs:739`, set at `runner_main/executor.rs:1032`, read at `exec/external_cli/mod.rs:204`; pi `asyncDir: path.dirname(ctx.outputFile)`, `subagent-runner.ts:924`), inside Fleet's containment root — before this they sat in the per-cwd scratch dir, where two runs in one cwd overwrote each other's `external-0.*.log`. Fleet renders `external-cli · <elapsed>` on the step row (`background/fleet_view.rs:465`; `fleet-view.ts:342-349,365`), reads an adapter's final-output file first, and tails `External stderr tail` then `External stdout tail` ahead of the transcript while the step runs and after it once settled (`fleet_view.rs:1158`; `fleet-view.ts:569,590-609`). The stderr-tail bound now counts UTF-16 code units, JS `length` (`background/reconcile.rs:1046,1066`). Verify: `background::runner_main::executor::tests::an_external_cli_step_publishes_its_process_live_and_logs_into_the_run_dir`, `background::runner_main::status::tests::an_external_process_report_is_published_onto_its_step_in_status_json`, `background::flat_index::tests::a_pending_external_cli_step_declares_its_runner`, `background::fleet_view::tests::fleet_shows_an_external_cli_steps_elapsed_time_and_its_stream_tails`, `background::reconcile::tests::the_stderr_tail_bound_counts_utf16_units_like_upstream` — each red with its fix reverted. **CYRUP-DELTA:** a UTF-16 cut through a surrogate pair renders the orphaned half as U+FFFD instead of emitting a lone surrogate (`reconcile.rs:1066`). **Ledger correction:** "revived runners still spawn unobserved" was already untrue — a revive goes through `spawn_background_steps` (`extension/executor/control.rs:512`), whose only spawn is the observed one (`extension/executor/background.rs:1009`); no production caller of the unobserved spawn remains (only the orchestrator-sim bin). **Not here:** run-status's external-runner block (`Runner:`/`Adapter:`/`Process:`/`Stdout:` lines, `run-status.ts:640-663`) is unported as a whole and is not this row's Fleet scope — ported 2026-09-30 under `SUBA-134` (`background/run_status.rs:635`). *Earlier text:* Observability drift: external-CLI stdout/stderr tails are not shown in Fleet, and a runner that dies without a result is not reported with its exit code and signal — **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`): the runner-exit half landed. The launching process now watches the detached runner (`crates/cyrup-ext-subagents/src/background/spawn_detached.rs:302`, called from `extension/executor/background.rs:964-967`; pi `proc.once("close")`, `async-execution.ts:759-770`) and finalizes the proof with the observed exit code and signal name. An `unknown` proof carries the runner instance, and a sticky unknown gains the exit only if it has none and is not proof-write-failed (`background/process_terminal/finalize.rs:326,347,383`). The dead-pid repair reads `Async runner process {pid} exited with code N (signal S)` or `exited or disappeared`, and a live stalled pid gets its own sentence (`background/reconcile.rs:397,664-712`; `stale-run-reconciler.ts:231-238,434-443`). The stderr tail uses upstream's bounds (last 64 KiB, last 30 lines, at most 4 000 characters plus `[stderr tail truncated]`) under `Runner stderr tail:` (`reconcile.rs:741,1065`). Verify: `background::reconcile::tests::{a_runner_killed_by_a_signal_reconciles_with_its_exit_code_and_signal,an_unobserved_dead_runner_exited_or_disappeared_with_its_stderr_tail}`. **Remaining:** the Fleet half. Upstream reads `step.externalProcess` from status.json, which its runner publishes from the external runner's live process hook (`subagent-runner.ts:2248`). cyrup's `StepStatus` (`background/records.rs:24`) has no such field (only `SingleResult.external_process` does), and `exec/external_cli` has no live hook. That also blocks the step-row elapsed time (`fleet-view.ts:344`). Smaller: the 4 000 bound counts chars, not UTF-16 units, and revived runners (`background/control.rs`) still spawn unobserved. |
| SUBA-142 | tracker | tracker | — | In-process child sessions: upstream builds children in-process; cyrup spawns a process. The design question, plus the in-process-only features parked behind it **FOLD-IN 2026-10-03 (v0.75.0):** `07946874`/#2636 and `1fe508f1`/#2638 make children launch and verify on extension-registered virtual models (`pi-virtual` api); the child's `virtualModelId` (`src/runs/shared/child-session.ts:118,589` @v0.75.0) replaces the router's physical model in `formatSubagentModelVerificationError` (`run-child-session.ts:517`, `foreground/execution.ts:1114`). In-process only, so parked here, no row. When `SESS-067` (area 03, the session half of virtual models) lands, `exec/model_verification.rs` must compare the child's selected model rather than the router's physical model, or a correct child fails `model_verification_failed`. |
| SUBA-144 | low | upstream-drift | M | **NEW 2026-09-30** (filed by lane A2 of `claude/lows-batch3` while closing SUBA-134). The runner emits no `subagent.nested.updated`/`subagent.nested.completed` events (upstream `nestedSummaryFromAsyncStatus`, pi-subagents v0.71.0) and never creates a nested route for a root run (SUBA-115's reach note), so `NestedRunSummary.session_name` and the nested exact-status view added under SUBA-134 have no production producer — they are fed only by relayed events from a writer that sets them. Same area, same port: the nested lookup lists every route under `Roots::nested_events` rather than upstream's state-scoped routes; there is no nested prefix matching, no `reconcileNestedAsyncDescendants` and no nested transcript view; the nested summary carries no `turnBudget`/`model`/`thinking`, so those status segments stay empty. |
| ~~SUBA-145~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch4`): the row named three differences; the drift was wider and all of it, from the same upstream lines, is closed. (1) `fleet_view::format_activity_label` (`background/fleet_view.rs:~280-292`) now prints `active but long-running · last activity now` (no `ago`), per `status-format.ts:19-20`; test `fleet_view::tests::activity_label_matches_pis_five_branches` gained the `now` cases (red before: `last activity now ago`). (2) `format_status` (`background/run_status.rs`) is now `format_status_with(status, paths, deps, nested, now)` (`:474`) with `format_status` a thin wrapper that reads the clock and the nested registry. It renders, at upstream's positions: the run-level `Activity:` and `Steering:` lines between `Error:` and `Mode:` (`run-status.ts:589-590`; `Activity:` only for a running run), the per-step `, <activity>` text of a running step (`:624,635`), each step's nested tree with `NestedLinesOptions { indent "  ", command_hints true, max_lines 20 }` (`:676`), the unattached tail with indent `""` (`:691-693`), the `Warning:` line (`:703`; a failed nested lookup, joined with `; ` to an unreadable mission binding, which moved here from its old position under `Run:`, `:557`), the running-step `  Intercom target: … (if registered)` + `  Steer: subagent({ action: "steer", … index })` for a local non-workflow step (`:680-681`), `  Steer: unavailable; external runners do not accept live messages.` for a running external-cli step (`:683`, const `EXTERNAL_RUNNER_STEER_UNAVAILABLE`), and the run-level `Steer running child:` hint for a running, not-all-external, non-workflow run (`:707`, which the row did not list). (3) `spawn/nested_events.rs`: `attach_root_children_to_steps` (`:1629`, port of `nested-events.ts:964-975`; returns the per-step attachment because a persisted `StepStatus` has no `children` field; `MAX_CHILDREN = 16`), plus `find_nested_route_for_root_id_in` / `project_nested_registry_for_root_in` (`:1566,:1606`) over an explicit events root. (4) CYRUP-DELTA: upstream reads the nested registry from a process-global route lookup; `RunStatusRenderDeps::nested_events_root` (`run_status.rs:324`) carries the `Roots::nested_events()` tree instead (set by `extension/executor/status.rs` from the executor's roots; `None` = no lookup), which is what makes the whole path testable end to end. (5) The module header (`run_status.rs:10-30`) now lists what is and is not rendered. Verify (all in `cyrup-ext-subagents`, no cyrup-it): `run_status::tests::{a_running_local_step_gets_its_intercom_target_and_steer_hint, a_running_external_runner_step_says_steer_is_unavailable, activity_labels_ride_the_run_and_its_running_steps, root_children_attach_to_the_step_they_name, nested_runs_render_under_their_step_and_the_unattached_tail_after, a_nested_lookup_failure_and_a_bad_mission_binding_share_one_warning_line, the_nested_registry_is_projected_from_the_deps_events_root}` and `fleet_view::tests::activity_label_matches_pis_five_branches` are red with their fix lines disabled (8 failures; the attach test red with the dedupe/cap lines removed: `left: ["a", "b", "a"] right: ["b", "a"]`); `a_running_workflow_step_gets_no_intercom_target` is a guard (green both ways). `a_non_running_workflow_renders_no_steer_lines` was updated: its running-single-run half asserted no `Steer` at all, which upstream's `Steer running child:` hint contradicts. *Not closed, filed as `SUBA-146`:* upstream's `reconcileNestedAsyncDescendants` pass before the projection, the `Session:` line, the all-external `Resume:` sentence. Nothing produces nested events in production until `SUBA-144` lands, so the nested tree renders only what a test or an external writer puts in the events tree. *Earlier text:* **NEW 2026-09-30** (filed by lane A2 of `claude/lows-batch3`). `run-status` @v0.71.0 still differs in three places after SUBA-134/141: the per-step nested tree in the main report is unported; the `Steer: unavailable; external runners do not accept live messages.` line is absent; and `fleet_view::format_activity_label` prints `last activity now ago` where upstream prints `last activity now`. |
| SUBA-146 | low | upstream-drift | S | **NEW 2026-09-30** (filed while closing `SUBA-145` on `claude/lows-batch4`). **RE-SCOPED 2026-10-02** (`claude/lows-batch5`): parts (b) and (c) are DONE and no longer open — see below; (a) and (d) remain. `run-status` @v0.71.0 differences left after `SUBA-145` (line numbers in (b)/(c) are @v0.71.0; the closure below cites @v0.75.0, where they are `:711` and `:716`): (a) `reconcileNestedAsyncDescendants(route, { resultsDir, kill, now })` (`stale-run-reconciler.ts:325`) runs before the nested projection in the main report (`run-status.ts:541`) and before the nested exact-status view (`:436`); cyrup projects the registry as-is, so a nested async descendant whose runner died still shows `running`. (b) ~~The `Session: <status.sessionFile>` line (`run-status.ts:705`) is not rendered (`RunStatus::session_file` is carried); the stale comment at the `Workflow receipt:` site says cyrup has no such line.~~ **DONE 2026-10-02.** (c) ~~For an all-external run (every step external-cli/external-job) a non-running report ends in `Resume: unavailable; external runners do not persist Pi sessions.` (`:709-710`) instead of cyrup's `formatResumeGuidance` result; the external-job follow-up variant needs the unmodelled `external-job` runner.~~ **DONE 2026-10-02**, except the external-job follow-up variant, which still needs that runner and stays open under this row. (d) Per-step `, acceptance: <status>` and `, turn budget: n/m+g (outcome)` suffixes (`:631-632`) and the run-level `Turn budget:` line have no source field on `StepStatus`/`RunStatus`. Fix direction: port (a) beside `cascade.rs`'s registry walk, add (b) and (c), and decide whether (d) waits on the turn-budget/acceptance status fields. **(b)+(c) closure evidence:** `run_status.rs` now pushes `Session: <path>` right after `Workflow receipt:` (an empty path prints no line, as JS truthiness does) and, for a non-running run whose every step is `external-cli`, `Resume: unavailable; external runners do not persist Pi sessions.` (`EXTERNAL_RUNNERS_NOT_RESUMABLE`) in place of `format_resume_guidance` (`run-status.ts:711`, `:714-716` @v0.75.0). The module doc and the stale comment at the `Workflow receipt:` site were corrected. Tests (`background::run_status::tests`): `a_report_names_the_session_file_after_the_receipt_and_before_the_steer_hint` and `a_finished_all_external_run_says_external_runners_do_not_persist_sessions` (both FAILED before the fix, committed red in `d2d1bc68`), plus the guards `a_report_has_no_session_line_without_a_session_file` and `the_external_resume_sentence_needs_every_step_external_and_a_non_running_run` (pass before and after: they pin what must not change). Not verified: the `external-job` variant, and the mixed-runner and running cases beyond those guards. Remaining fix direction: port (a) beside `cascade.rs`'s registry walk, and decide whether (d) waits on the turn-budget/acceptance status fields. |
| ~~SUBA-147~~ | ~~low~~ **CLOSED 2026-10-03** | not-ported | M | **CLOSED 2026-10-03** (`claude/lows-batch6`) as a WRONG PREMISE, superseded by `SUBA-149`; no code change. Read at `tmp/pi-subagents` v0.75.0 (the row's lines were @v0.71.0 and have moved): `preflightWorktreeSource` is `src/runs/shared/worktree.ts:359-369` and runs `rev-parse --is-inside-work-tree`, `--show-toplevel` and `status --porcelain`, reporting `worktree isolation requires a git repository` or `... a clean git working tree. Commit or stash changes first.` It has exactly TWO call sites, not the row's description: (1) workflow admission, `preflightWorkflowWorktrees` (`src/runs/foreground/subagent-executor.ts:4941-4968`), invoked from the engine's `admit:` callback (`:6130` async, `:6442` foreground) once per `runs.run` / `runs.all` batch, with the `Worktree admission failed for '<keys>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.` wrapper (keys sharing a cwd are listed together); (2) the direct async single call (`:7454-7459`), which reports the RAW probe message with no wrapper. Upstream has no preflight for sync single, `tasks[]` or chain groups. The row's premise fails for cyrup: a `worktree: true` single run or workflow child never gets a worktree here. `worktree` is parsed (`extension/tool/params.rs:216`) but consumed only by the parallel `tasks[]` shape (`routing.rs:2717`) and chain parallel groups (`spawn/chain_graph.rs:1827`, `assign_worktree_cwds`); `WorkflowScriptHost::admit` has only its default `Ok(())` (`workflows/scripted/engine.rs:124`, no override). Porting the probe now would reject dirty-tree launches for children that would never use a worktree, an invented failure. "Earlier group members may have run" is also false inside one group: `assign_worktree_cwds` sets up the whole group before dispatch, so a dirty or non-git source fails before any member of that group runs. This is established by reading source and grepping every consumer of the field; the silent drop was NOT observed by running it. The real gap is filed as `SUBA-149`. Original finding: ~~**NEW 2026-09-30** (filed while closing `SUBA-130` on `claude/lows-batch4`). `preflightWorktreeSource(cwd, { signal, deadlineAt })` (`worktree.ts:359-368`) is unported: upstream's executor runs a read-only admission probe of every distinct `worktree: true` source cwd BEFORE launch (`subagent-executor.ts:4739` for workflows, `:7120` for a plain call) and fails the whole launch with `Worktree admission failed for '<key>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.`; cyrup discovers a dirty or non-git source only inside `create_worktrees`, after earlier group members may have run. The probe's git calls are the same bounded ones `SUBA-130` added (`resolve_repo_state` with a `GitBounds`), so the port is the admission loop plus the error text.~~ |
| SUBA-148 | low | not-ported | S | **NEW 2026-09-30** (filed while closing `SUBA-130` on `claude/lows-batch4`). Three places where worktree git still has no stop or deadline because the caller has no run context: `handoff::discard_preserved` (`worktree.discard`), the `worktree.cleanup` plan builder (`spawn/cleanup_plan/git.rs::run`, `classify.rs::is_patch_captured`) and `resolve_expected_worktree_agent_cwd` take `GitBounds::unbounded()`; and `create_worktrees` / the harvest wait on `worktree_turn()` (an unbounded `tokio::sync::Mutex`) without honouring stop or deadline, so a hung discard holding the turn blocks every later launch. Upstream v0.71.0 is equally unbounded in the git calls (`spawnSync`), so this is not drift; it is room to do better. Fix direction: thread the tool call's cancel token into `discard_preserved` and the plan builder, and race the turn lock against the run's stop/deadline. |
| SUBA-149 | medium | not-ported | L | **NEW 2026-10-03** (filed while closing `SUBA-147` on `claude/lows-batch6`; established by reading source, not by running it). Single-agent and scripted-workflow-child `worktree: true` isolation is unported. Upstream allocates a managed worktree for a single run (`worktreeSetup` and `retainSingleWorktreeHandoff`, `subagent-executor.ts:3998`, the single path in `worktree.ts`) and for each workflow child whose effective `worktree` resolves true. cyrup accepts the flag (`extension/tool/params.rs:216`, `workflows/scripted/engine.rs:832` whitelist and `:1519-1522` boolean validation) but only the `tasks[]` shape (`routing.rs:2717`) and chain parallel groups (`spawn/chain_graph.rs:1827`) consume it, so the request is silently dropped on every other shape and the child runs in the shared cwd. Impact: a caller who asks for isolation gets none, and no error says so. **Fix** — allocate and hand off a worktree for a single run and for each workflow child that resolves `worktree: true`; THEN add the admission probe `SUBA-147` described: (a) `WorkflowRunHost::admit` (`extension/executor/workflow.rs:750`, currently not overridden) calling a `preflight_worktree_source(cwd, GitBounds)` (the repo and clean-tree half of `resolve_repo_state`, `spawn/worktree.rs:645`, without the HEAD resolve) and reporting `Worktree admission failed for '<keys>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.`, keys sharing a cwd listed together, children with `resume` skipped; (b) the same probe with the RAW message on the async-single launch (`subagent-executor.ts:7454-7459`). Check how the admission changes the order of the engine's existing claim logic before wiring it. **Verify** — a single run and a workflow child with `worktree: true` each run in a managed worktree; a dirty source fails the whole workflow batch before any child launches. |
| SUBA-150 | medium | upstream-drift | L | **`workflow: true` reply-fenced scripts replaced `workflowScript`/`workflowScriptPath`, which v0.74.0 deleted from the tool** — one `workflow` field now takes `true` \| a path \| a resource name (`src/extension/schemas.ts:221` @v0.74.0, `src/extension/reply-workflow-script.ts:14`); cyrup still advertises both removed params at `extension/tool/schema.rs:345`. **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0):** (a) `df3b6df1`/#2610 makes the string `"true"` equal `workflow: true` for MCP clients that stringify booleans (`src/extension/reply-workflow-script.ts:29`, `rpc.ts:527`, `index.ts:744` @v0.75.0); it is part of the `workflow` field this row ports. (b) `cfb6f9a8`/#2611: `action: "validate"` now checks workflow `args` against the same limits as a launch, exports `MAX_ARGS_FIELDS/ITEMS/DEPTH/BYTES` and states them in the `args` schema description (`src/extension/schemas.ts:7,227` @v0.75.0). Cyrup already enforces 16 fields, 64 items, depth 8 and 16 KiB in `workflows/resources.rs` (`validate_plain_json` and `normalize_args`, `:385-445`; they are literals, only `MAX_ARGS_BYTES` is a private const at `:29`), but its `validate` arm (`extension/tool/routing.rs:1870`) takes only `workflowScript` and never runs `normalize_args`, and `args` is advertised and plumbed only for scheduled runs (`extension/tool/schema.rs:742`, `background/scheduled_runs/tool.rs:355`). Port: export the four limits as constants, state them in the `args` description, and run `normalize_args` in `validate`, reporting its error beside the script errors. |
| SUBA-151 | medium | upstream-drift | L | **`chain`, `tasks` and the whole dynamic-fanout schema are gone from v0.74.0's default tool** — they survive only in the reduced schema used when `disabledFeatures` lists `workflow-scripts`, and are lowered into package-owned scripts (`src/workflows/structured-workflow-scripts.ts:175`, `src/extension/schemas.ts:286,299`); cyrup advertises the full v0.71.0 shape (`extension/tool/schema.rs:259,520,527`). **FILED 2026-10-02**; body below. |
| SUBA-152 | low | not-ported | M | **`config.disabledFeatures` is unported** — 15 named feature groups an operator removes from the `subagent` tool, each mapping actions and params to the setting that disabled them (`src/shared/disabled-features.ts:8,82,98` @v0.74.0, validated `src/extension/config.ts:182`); `disabled_features` has zero hits in `crates/`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the claim that cyrup accepts `disabledFeatures` and "drops it with no warning" is false. `SubagentExtensionConfig::config_warnings` (`registration/mod.rs:927-945`) runs `discovery::key_census` and emits `unknown key 'disabledFeatures' (ignored)` (`:944`); `crates/cyrup/src/subagent_config.rs:75-77` prints it to stderr on load (only when `validate_raw_config` passes). What is true is narrower: the key is not in `UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries), so the message is the generic typo wording rather than the "is not supported by this port (...); it has no effect" wording an unported key gets, and nothing says the key is a real upstream feature. The upstream cites (`disabled-features.ts:8/82/98`, `config.ts:17/182`) hold; v0.75.0 only widens upstream's fail-closed list (see `SUBA-166`). |
| SUBA-153 | low | upstream-drift | M | **`SUBA-139`'s port target moved: the loader now has three `config.toolActivation` modes and a per-API cache-miss gate** — `auto`/`dynamic`/`eager` plus `addsToolsWithoutCheckpoint` (`src/extension/tool-activation.ts:36,85` @v0.74.0, `src/shared/types.ts:2595,2672`); cyrup has neither the loader (`SUBA-139`, open) nor the key. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the statement that the compat flags the gate reads "do exist port-side ... plus `supports_tool_search`/`supports_additional_tools`" is wrong for the second flag. `ModelCompat` (`crates/cyrup-provider/src/api/compat.rs`) has `supports_mid_convo_system_messages` (`:456`), `supports_mid_convo_tool_additions` (`:466`), `supports_mid_convo_tool_changes` (`:520`) and `supports_tool_search` (`:569`), but no `supports_additional_tools` field: a grep of `crates/**/*.rs` for `supports_additional_tools` finds nothing and for `supportsAdditionalTools` finds only a test comment (`providers/openai_codex.rs:779`). `supportsAdditionalTools` appears in the catalog JSON (`opencode-go.json`, `openai-codex.json`, `github-copilot.json`), and `ModelCompat` sets no `deny_unknown_fields`, so the catalog key is ignored. The Responses-API arm of `addsToolsWithoutCheckpoint` (`tool-activation.ts:36-53` @v0.74.0) therefore needs a new compat field and catalog plumbing first; "the predicate is portable today" holds only for the `anthropic-messages` and `openai-completions` arms. |
| ~~SUBA-154~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04**: `spawn/worktree.rs`'s `MACHINE_DIFF_OPTIONS` and `MACHINE_PATCH_OPTIONS` both spell upstream's seven-element list with `--src-prefix=a/ --dst-prefix=b/` in place of `--default-prefix`, so a capture no longer needs git ≥ 2.43 and a diff that fails no longer costs the child's patch outright; the doc paragraph says why and the citations are retagged `@v0.74.0`. Verify: `machine_diff_argv_matches_upstream_and_carries_no_default_prefix`, `the_two_machine_option_lists_move_together`, `a_hostile_prefix_configuration_still_yields_a_p1_applicable_patch` (captures through the real `diff_worktrees`, then `git apply -p1`s into a fresh checkout and compares bytes). **Ledger correction:** `1cf63c18` is `v0.74.0~36`; v0.73.1 was the last release still carrying `--default-prefix`, so the provenance reads as the swap landing *at* v0.74.0. — *Original:* **`--default-prefix` in cyrup's worktree diff/patch options needs git ≥ 2.43, and a failed capture destroys the child's work** — upstream replaced it with `--src-prefix=a/ --dst-prefix=b/` (`src/runs/shared/worktree.ts:23` @v0.74.0, `1cf63c18`/#2527); cyrup still passes it at `spawn/worktree.rs:79,95`. **FILED 2026-10-02**; body below. |
| ~~SUBA-155~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | M | **`modelScope` is still the three-key v0.33 shape: no `agents.<name>` rules, no `inherit`, no `scoped`** — so an operator copying pi's documented `allow: ["inherit"]` under `enforce` has every model rejected (`src/runs/shared/model-scope.ts:51,53,93` and `resolveModelScopesForAgent` @v0.74.0; cyrup `exec/model_scope.rs:40`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** line cite: `ModelScopeConfig` is at `exec/model_scope.rs:43` (the row's `:40` is its doc comment); it has the three fields `enforce`, `strict`, `allow` and no `inherit` or `agents`. Read in addition: `parse_model_scope_config` (`exec/model_scope.rs`, from `:274`) reads only `enforce`, `strict` and `allow` and emits no warning for another subkey, which supports the "silently dropped" wording for `modelScope.agents`; a separate key census was not checked. — **CLOSED 2026-10-04** (`1409cede`, completing `81200403`). **EVIDENCE CORRECTED — this row's `CORRECTED 2026-10-03` note is itself stale:** `81200403` (on `main`) added `ModelScopeRule`, `ModelScopeConfig.agents` (four fields, not three, at `exec/model_scope.rs:66`), `resolve_model_scopes_for_agent`, `expand_reserved_patterns`, the 8-pattern render cap and the `agents` parser arm — verified absent in `81200403^` and present in `81200403`. What that commit left were two behaviours unreachable from any launch path, and those are what `1409cede` closes. **(a)** `scoped` never resolved to anything: both production call sites passed `scoped_model_ids: None` (`exec/mod.rs:1048`, `fallback.rs:442`), citing a `RunOptions` field that does not exist, so `allow: ["scoped"]` could only degrade to `inherit` — admitting the parent's one current model and refusing every other model the operator deliberately kept in scope. Now a two-phase expansion: `ModelScopeConfig::with_scoped_snapshot` substitutes the snapshot (read once per launch from `HostServices::scoped_models()`) into the global and every per-agent `allow`, then `expand_reserved_patterns` resolves `inherit`. Because the snapshot lands inside the `model_scope` that `RunnerConfig` already serializes, a background run enforces the set its parent held at launch with no new field to drift. **(b)** An armed reserved token with no parent model failed **OPEN**. `unresolved_enforced_reserved_scope_message` was defined at `model_scope.rs:347` but `#[cfg(test)]` begins at `:693` and all four callers sat after it — ported, tested, never wired. With no parent session model the only check that ran was warn-severity, so the launch proceeded on the persona's model with a log line where pi refuses (`model-resolution.ts:347`). `resolve_model_inheritance` now consults the gate first, returning `ModelScopeRefusal::{OutOfScope, UnresolvableReservedToken}`. **Two further corrections.** The Fix's "one non-trivial piece" named the wrong seam: `cyrup-tui/src/app/selectors.rs:225` is the TUI's own selector state, not a seam a subagent launch can reach; the seam is `HostServices::scoped_models()` (`cyrup-ext/src/host/services.rs:1091`, already filled by the live backend per EXT-045), and it is ~50 lines with no struct change. And the row understated the fail-open: it treats fail-closed as a property of `expand_reserved_patterns`, where upstream's guarantee actually lives in the uncalled refusal — that belonged in Impact as a third concrete failure. Cite drift at the pin: `resolveModelScopesForAgent` is `model-scope.ts:161-188` and `expandReservedPatterns` `:107-119` (the row's `:160` and `:105-114` are v0.74.0's). Pinned by `the_documented_inherit_policy_admits_the_parent_model_and_still_refuses_another`, `an_armed_reserved_token_with_no_parent_session_refuses_the_launch_instead_of_warning`, `a_per_agent_rule_refuses_at_launch_a_model_the_global_block_admits`, `the_launch_time_policy_captures_this_sessions_scoped_model_snapshot` and three more, each proven red by reverting production code. |
| ~~SUBA-156~~ | ~~medium~~ **CLOSED 2026-10-04** | parity-bug | S | **CLOSED 2026-10-04** (`ae115d74`, one commit with `MCP-594` as both rows require): `exec/mcp_direct_tools.rs`'s `ServerEntry` carries `inheritEnv` and `literalEnv`, its pre-image gates `interpolate_env_record` on `literal_env` exactly as the writer does, both sides compute `literal_env` with the identical expression and push the same two members under the same `is_stdio` condition, and `exec/mcp_config_sources.rs::translate_plugin_stdio_server` injects `literal_env: Some(true)` as the adapter's own loader does. The agreement tests assert the reader's pre-image *string* equals the writer's, not merely the digest. Verify: `pre_image_matches_the_upstream_generated_golden_vector`, `the_two_stdio_identity_keys_agree_reader_writer_and_upstream`, `reader_and_writer_agree_on_the_fifteen_field_pre_image`, `reader_writer_and_upstream_agree_across_the_edge_cases`, `the_plugin_translation_agrees_with_the_adapters_own_loader`. — *Original:* **The MCP direct-tool reader models no `literalEnv`, so every agent-plugin MCP server's identity hash disagrees with the writer's and its `mcp:` selectors silently resolve to nothing** — upstream put `literalEnv`/`inheritEnv` into the identity at `src/runs/shared/mcp-direct-tool-allowlist.ts:470` @v0.74.0 (`847ee4de`/#2539); cyrup's reader is `exec/mcp_direct_tools.rs:1012`, its writer `cyrup-mcp/src/dirs.rs:1294`. **FILED 2026-10-02**; body below. |
| SUBA-157 | low | upstream-drift | S | **`subagents.agentOverrides.<name>.advertise` is unported** — v0.74.0 lets settings opt a builtin or custom agent into the parent-prompt catalog without editing its file (`src/agents/agents.ts:88,141`, `8dc90dca`/#2534); cyrup has `AgentDefinition::advertise` (`discovery/types.rs:1352`, `SUBA-133`) but `AgentOverrideConfig` (`discovery/types.rs:686`) has no such field. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** line cite: `AgentOverrideConfig` is at `discovery/types.rs:673` (the row's `:686` is inside the struct); `advertise` exists only on `AgentDefinition` (`:1352`). |
| ~~SUBA-158~~ | ~~medium~~ **CLOSED 2026-10-04** | not-ported | S | **A spawned subagent child does not follow the parent session's project trust**, so a child in a session-trusted project loads it as untrusted and drops its project-level config — upstream threads `projectTrusted` into the child's settings manager (`src/runs/shared/child-session.ts:59,373` @v0.74.0, `b2718fb8`/#2570); cyrup's child argv carries no trust (`exec/spawn_plan.rs:317`) and a fresh `cyrup` boots `project_trusted: false` (`crates/cyrup/src/bootstrap.rs:93`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the cyrup evidence is mis-aimed. `crates/cyrup/src/bootstrap.rs:93` (`load_startup_settings`) is the process's startup settings manager, used for settings diagnostics and the `sessionDir` lookup; it does not decide the project trust a session runs under. Real trust resolution is `SessionBuilder` in `crates/cyrup-session-svc/src/builder.rs:801-853`: settings loaded with the project untrusted, `default_project_trust`, `has_trust_requiring_resources`, the trust-store `nearest` lookup, then `TrustInputs { trust_override, saved, default_trust, mode, .. }` into `decide_trust_with_extension`. `cyrup-ext-subagents` passes no `--approve`/`--no-approve` and no trust env to the child (no hits in the crate outside unrelated tests), so a child resolves trust itself from the trust store and `defaultProjectTrust`. Direction caveat: upstream's v0.74.0 changelog describes the opposite symptom ("a child in an untrusted project still loaded that project's settings"), so the row's "child less trusting than the parent after an in-session grant" is the mirror case of the same missing hand-off. Neither was reproduced end to end; the defect is plausible but unproven. Recommended re-rating (not applied): medium to low until a spawned child is shown to resolve a different trust than its parent. — **CLOSED 2026-10-04**: A child follows the launching session's project trust through the `PARENT_PROJECT_TRUSTED_ENV` ladder, which prefers explicit then inherited then nothing. Pinned by `the_parent_trust_ladder_prefers_explicit_then_inherited_then_nothing` and `an_untrusted_parent_hands_its_child_the_no_approve_flag`. |
| SUBA-159 | low | upstream-drift | S | **Runner liveness probes are not scoped to the PID namespace, so an observer in another namespace reads `ESRCH` and fails a live run** — upstream records `pidNamespaceScope` and downgrades a cross-namespace `dead` to `unknown` (`src/runs/background/pid-namespace.ts:7`, `src/runs/background/stale-run-reconciler.ts:439,441` @v0.74.0); cyrup's probe is a bare `kill(pid, 0)` and `Dead` fails immediately (`background/reconcile.rs:144,383`). **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0, `806e3678`/#2606, Linux zombie runners):** `checkPidLiveness(pid, kill, probeZombie)` (`src/runs/background/stale-run-reconciler.ts:360-370` @v0.75.0) now reads `/proc/<pid>/stat` after a successful `kill(pid, 0)` and returns `dead` when the state field is `Z`. `probeZombie` is true only when the status's recorded `pidNamespaceScope` equals the observer's (`stale-run-reconciler.ts:446-451`; the same gate in `await-async-run.ts:15-24`). Cyrup's `check_pid_liveness` (`background/reconcile.rs:144`) is a bare `kill(pid, 0)` and reports a zombie as `Alive`. `background/spawn_detached.rs:391` drops the runner's `Child` (`None => drop(child)`), so a runner that outlives its launcher is reparented, and where PID 1 does not reap (a container without an init) it stays a zombie that reads as alive; that is the triager's reading, not run, and it needs no cross-namespace mount. Scope widens: port the `/proc/<pid>/stat` zombie probe together with `pid_namespace_scope`. Recommended re-rating (not applied): low to medium if the init-less-container zombie is judged reachable. |
| SUBA-160 | low | upstream-drift | S | **`timeoutMs`/`maxRuntimeMs` accept any `u64` although `toolTimeoutMs` and `checkpointBeforeDeadlineMs` are capped at `MAX_TIMER_DELAY_MS`** — upstream now rejects an oversized value on launch and on `action: "resume"` (`src/runs/foreground/subagent-executor.ts:2948,2950` @v0.74.0, `5655f9bb`/#2517); cyrup's `resolve_foreground_timeout` (`extension/tool/params.rs:566`) checks only `0` and the alias clash. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the Impact paragraph's claim that "a Rust/tokio deadline built from a huge `u64` saturates" is unverified; `Instant + Duration` arithmetic can panic on overflow, so the real effect of an oversized value may be a panic rather than "no deadline". The defect and the cites (`params.rs:566-588` checks only zero and the alias clash) stand. |
| SUBA-161 | low | not-ported | S | **The `council` guide topic and the multi-file guide body are unported** — v0.74.0 adds an eleventh topic that concatenates three bundled files behind `<!-- path -->` markers so `/council` survives `--no-skills` (`src/extension/subagent-guide.ts:16,25,39`, `a0fd73df`/#2469); cyrup's `SUBAGENT_GUIDE_TOPICS` is ten (`registration/guide.rs:44`) and `council` has zero hits in `crates/` or the ledger. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** effort S is optimistic (S to M): a working `council` topic needs the three skill files bundled (`SUBA-161`'s own Fix says whether they ship is a separate decision), and v0.75.0 changes eight guide docs (`docs/agents.md`, `configuration.md`, `extension-api.md`, `missions.md`, `models.md`, `observability.md`, `tool-reference.md`, `workflows.md`). Recommended re-rating (not applied): effort S to M. |
| SUBA-162 | low | not-ported | M | **The progressive async-widget tier is unported: no height lock, no workflow lane rows, and no running-agent header count** — upstream's widget locks the card to the rows its content fills and counts leaf agents the way Fleet does (`src/tui/render.ts:2635,2648,2704` @v0.74.0, `9a5a2d5e`/#2583 and `7e07a22d`/#2584); cyrup renders one full block per run with no header line (`tui/render.rs:386`, `tui/events.rs:874`). **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0, `53aee6d8`/#2621):** also unported are upstream's header-click fold (`93d47c0c`/#2235, v0.68.0; `buildWidgetComponent`, `src/tui/render.ts:2903` @v0.75.0) and v0.75.0's boolean `asyncWidgetCollapsed` (validated `src/extension/config.ts:175-176`, passed as `initiallyCollapsed` at `render.ts:3099`). `grep -rn 'asyncWidgetCollapsed' crates/` is empty, so a user who sets it gets only the generic unknown-key stderr warning. Not verified: whether the cyrup TUI can deliver clicks to an extension widget, which decides whether the click half is portable. |
| SUBA-163 | low | stale-port | S | **`/subagent-cost` reports no child usage for an async single, chain or parallel launch** — `SUBA-138` ported v0.71.0's collector, and `d556bb01`/#2490 (v0.72.0) then added the `asyncId`-with-empty-`results` and `completions` arms (`src/slash/subagent-cost.ts:163,164` @v0.74.0); cyrup's collector tracks only `workflow_run_ids` (`registration/cost.rs:1002`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (stale by v0.75.0):** the port target moved again; `ae9c9d77`/#2612 and `4998ceca`/#2615 change the same function (`git diff v0.74.0 v0.75.0 -- src/slash/subagent-cost.ts`). (a) A workflow id joins `workflowRunIds` only when `details.mode === "workflow" && details.runId && details.asyncId`: foreground workflow child usage is already in `results`, and only async workflows persist a receipt. (b) The child `runId` is read from the typed `result.runId`, with no cast. (c) Every round of a resumed foreground workflow child counts (#2612). (d) A missing or unreadable receipt now adds to `unresolvedAsyncChildren` (an `ENOENT`, also via `error.cause`, is not logged; other errors are), so an async workflow with no receipt yet is listed as `Async child usage unavailable` (#2615). Cyrup diverges on both ends (read, not run): `collect_subagent_cost` (`registration/cost.rs:995`) adds a workflow id on `mode == "workflow"` plus `runId` (`:1014-1020`), but cyrup's foreground workflow details carry `mode`, `children`, `workflowRunId` and no `results`, `runId` or `asyncId` (`extension/executor/workflow_launch.rs:456-470`, `:697`), so foreground workflow child usage is never counted; and a `NotFound` receipt is a silent `continue` (`cost.rs:1102`) instead of an unresolved count. Scope of the fix therefore widens beyond the `asyncId`/`completions` arms: add the arm, gate the workflow arm on `asyncId`, count foreground workflow `children` usage, and count unresolved on a missing receipt. |
| ~~SUBA-164~~ | ~~medium~~ **CLOSED 2026-10-04** | not-ported | M | **`tool_open_threshold` attention is unported, so a child stuck in one long tool call never raises `needs_attention`** — upstream emits `reason: "tool_open_threshold"` for each tool call left open past `activeNoticeAfterMs` (`shouldEmitOpenToolAttention`, `src/runs/shared/subagent-control.ts:114-122`; `src/runs/foreground/execution.ts:915-933`; `src/runs/background/subagent-runner.ts:2709-2781`; reason enum `src/shared/types.ts:387` @v0.75.0); cyrup's `ControlEventReason` has seven variants (`exec/control.rs:301-322`) and `derive_activity_state` returns `None` while a tool is open (`:475`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-04** (`a26aaa74`). `ControlEventReason::ToolOpenThreshold` + `ControlEvent::tool_call_id`; `should_emit_open_tool_attention` as a pure `(now, open-since, threshold)` decision reusing `is_tool_timeout_exempt`; `ControlMonitor` now tracks the open calls (`ActiveToolCall`, keyed by the existing `tool_timeout_call_key`) with a per-call `attention_emitted`, derives `current_tool`/`current_path` from them, and runs the open-tool branch between the idle and long-running branches as both upstream runners do; `control_notification_key` is keyed per call for this reason, which also makes the TUI notice dedupe per call; `drive_attempt` clears the open set on a terminal assistant stop. One monitor serves both the foreground and detached-background paths, covering both of upstream's producers. The bash nudge wording stays deferred to `SUBA-165`, as this row specified. 10 new tests, none of which sleeps or reads a clock — `now` is an explicit epoch-millis argument throughout, which is what lets the inclusive `>=` boundary be pinned at 999 vs 1000 ms against a 1000 ms threshold; all proven red by surgical reverts, and three PRE-EXISTING tests also go red when the threshold is forced true, which is what shows the branch is wired into the real fold rather than sitting beside it. **Cite corrections:** `shouldEmitOpenToolAttention` is `subagent-control.ts:114-124` — the row's `:114-122` stops one line short of the `>=` comparison at `:123`, i.e. omits the single line that fixes both the clock and the inclusive boundary; foreground `updateActivityState` is `execution.ts:910-936` with the branch at `:924-936`; the background pair is `subagent-runner.ts:2709-2711` + `:2760-2791`, **driven from `:3204`** — that driver line, which the row does not cite, is what establishes the idle → open-tool → long-running ordering and is what anyone verifying "do the two runners agree?" needs. **Upstream's own v0.75.0 CHANGELOG miscredits this fix** to `#2598` (`72682254`, opt-in command supervision); the actual change is `478f871c`/`#2613`, exactly as this row said — verified at the pin, so reconciling against pi's CHANGELOG would turn a correct row into a wrong one. Incidental fix found while porting: `pending_tool_result.path` took the derived `current_path` where upstream uses the call's own `activeTool?.path` (`execution.ts:1061`), so with two calls open a mutating failure could be attributed to the wrong path. |
| SUBA-165 | low | not-ported | L | **Opt-in `subagent_command` supervision (`command.status`, `command.yield`, `command.cancel`, `toolCallId`, bash `yieldTimeMs`) is unported** — upstream wraps a native child's `bash` so the parent can query, yield or cancel one command (`createChildCommandRuntime`, `src/runs/shared/child-commands.ts:49`; `commandAction`, `src/runs/foreground/command-action.ts:24`; actions in `SUBAGENT_ACTIONS` `src/shared/types.ts:2865`; `toolCallId` param `src/extension/schemas.ts:184` @v0.75.0, `72682254`/#2598); cyrup's `SUBAGENT_ACTIONS` (`extension/tool/text.rs:265`) has no `command.*` and no code mentions `yieldTimeMs`. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| ~~SUBA-166~~ | ~~medium~~ **CLOSED 2026-10-04** | parity-bug | S | **Any invalid config value makes cyrup load all-defaults with only a stderr warning, dropping `authorityPolicy` and `permissions`** — upstream's `loadConfig` rethrows when the file holds any `FAIL_CLOSED_CONFIG_KEYS` key (`src/extension/config.ts:17,226-240` @v0.75.0, which `9f1c2552`/#2624 widened from 8 keys to 11 by adding `authorityPolicy`, `permissions`, `toolBudget`); cyrup returns defaults on any `validate_raw_config` failure (`crates/cyrup/src/subagent_config.rs:68-73`) and on a typed-parse error (`:91-97`), and has no fail-closed key list at all. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-04** (`f194471b`, completed by `a3786e3e`): `FAIL_CLOSED_CONFIG_KEYS` (11 keys, upstream order) plus `InvalidConfigDisposition::{DefaultWithWarning, Refuse}` consulted on both the raw-validation and typed-parse arms, so a file declaring `authorityPolicy` or `permissions` is refused instead of replaced by the all-defaults config; a file declaring no listed key still warns and defaults. `f194471b` alone left the refusal escaping `attach_native_extensions`, aborting the whole launch where pi discards one extension and starts anyway; `a3786e3e` closes that with `cyrup_ext::QuarantinedNative`, so the cost is exactly the extension upstream loses. Pinned by `a_typo_beside_an_authority_policy_refuses_the_file_instead_of_lifting_the_policy`, `every_fail_closed_config_key_forces_a_refusal`, `a_refused_subagents_config_costs_the_launch_only_that_extension` and `the_attach_gate_is_consulted_before_the_config_is_read`. |
| SUBA-167 | low | not-ported | M | **Claude Code adapters accept no per-launch model or thinking level (`--model`, `--effort`)** — upstream's `resolveClaudeCodeOverride` (`src/runs/shared/claude-code-adapter.ts:132` @v0.75.0, `aafcfb31`/#2603) maps `model: "id:level"` to `--model` and `--effort`, and frontmatter `model`/`thinking` are allowed for these adapters (`src/agents/agents.ts:2091-2093`); cyrup's `launch_args` is fixed (`exec/external_cli/adapters/claude_code.rs:75-101`) and `PI_ONLY_FIELDS` rejects both fields (`runner/mod.rs:190-207`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-168 | low | not-ported | M | **`schedule.create` still refuses `missionId`: schema-version-2 mission-bound schedules are unported** — upstream accepts an existing `missionId` at create, writes `schemaVersion: 2` exactly when it is present and omits `mission: false` when bound (`src/runs/background/scheduled-runs.ts:313,326,456-467,476,641` @v0.75.0, `9240e7e2`/#2616); cyrup returns "Mission attachment is deferred" (`background/scheduled_runs/tool.rs:468-478`) and parses only version 1 (`schedule.rs:57,618`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-169 | low | upstream-drift | S | **One unreadable goal mission aborts continuation notices for every healthy goal mission** — upstream wraps each mission in try/catch with an `onError(missionId, err)` callback (`collectGoalContinuationNotices`, `src/missions/goal-driver.ts:127-175` @v0.75.0, `36081c57`/#2604), and `readLinkedRun` names the path of a malformed `status.json` (`:30-40`); cyrup's `collect_goal_continuation_notices` (`missions/goal_driver.rs:486-554`) uses `?` per mission and the caller turns that error into `return 0` (`extension/executor/notices.rs:735-745`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-170 | low | upstream-drift | S | **The global mission list shows the index entry, not the mission record** — upstream's `listGlobalMissions` overlays `title`, `status`, `updatedAt` and `lastRunId` from the readable record (`src/missions/store.ts:554-575` @v0.75.0, `d6726aea`/#2618); cyrup's `list_global_missions` (`missions/store.rs:1372`) parses the record only to set `stale` and returns the index entry unchanged (`:1424-1446`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-171 | low | parity-bug | S | **Settings save replaces a symlinked or restricted `settings.json` with a default-mode regular file** — upstream's `writeSettingsFile` (`src/agents/agents.ts:947` @v0.75.0, `63ac9c2a`/#2627) resolves the real target through symlinks (`resolveSettingsWriteTarget`, `:977`), checks `W_OK`, keeps the existing mode and writes via temp and rename; cyrup's `write_settings_file` (`discovery/settings_write.rs:102-111`) calls `cyrup_config::lock::write_atomic(path, bytes, false)` (`lock.rs:337-377`), which creates the temp at the umask default and renames over the path itself. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-172 | low | stale-port | M | **`run-history.jsonl` has the pre-hardening shape, and foreground runs are never recorded** — upstream's `recordRun` (`src/runs/shared/run-history.ts:223` @v0.75.0) writes `task: "[redacted]"` with a sha256 `taskHash` and an `outcome`, in a `0600` file under a `0700` directory, with rotation on load (`:262`); v0.75.0 (`f517481f`/#2620) adds `planBackgroundRunHistory` (`:180`) so async runs record one row per launched step. Cyrup's `record_run_history` (`background/run_history.rs:86-135`) stores the first 200 characters of the task in plaintext, with default mode, no hash, no outcome and no rotation, and only the async runner calls it (`background/runner_main/finish.rs:523`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-173 | low | parity-bug | S | **A saved profile's `machine` string is not validated before `/subagents-load-profile` writes settings** — upstream's `validateSubagentProfile` (`src/profiles/profiles.ts:125,148` @v0.75.0, `8a577c27`/#2619) calls the now-exported `validateOptionalMachine` (`src/agents/agents.ts:1035`) per override, so a bad value is rejected before any write; cyrup's `load_profile` (`registration/profiles.rs:211`) only serde-parses, and `placement/resolve.rs:33` already has `validate_optional_machine`. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |

---

## SUBA-110 — Git routing variables reach the background runner and inherited external CLIs

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-24** (`c936d8c`): new `spawn/git_env.rs` ports `git-environment.ts` (the 16 names plus `GIT_CONFIG_{KEY,VALUE}_<n>`, case-insensitive); applied before the overlay in `background/spawn_detached.rs` and on the inherited arm of `exec/external_cli/run.rs`; an adapter allowlist is left as written. Verify: `spawn::git_env::tests` (predicate; removals-then-overlay; a real `git rev-parse` resolves the parent repo through an inherited `GIT_DIR` and the child's own repo once filtered). The two call sites are not driven by a test (`#![forbid(unsafe_code)]` rules out setting `GIT_DIR` on the test process).

**cyrup** — The detached background runner is spawned at
`crates/cyrup-ext-subagents/src/background/spawn_detached.rs:213-228`. It uses
`tokio::process::Command::new(&spawn_command.binary)` with `.envs(env_overlay)` and never clears or
filters anything; the comment at `:225-227` says *"overlay (never `env_clear`)"*. The generic
external-CLI path (no adapter) is `ExternalEnv::Inherited`
(`exec/external_cli/env.rs:56-63`), which is a plain inherit. `exec/external_cli/run.rs:182-189` calls
`env_clear` only for the `Allowlisted` variant. `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and
`local-env-vars` have **zero hits** in `crates/`. (The `GIT_INDEX_FILE` hits in `spawn/cleanup_plan/`
are cyrup setting its own temporary index for its own commands. They do not filter anything.)

**upstream** — `src/runs/shared/git-environment.ts` @v0.71.0 (new in the window) is a 16-name set
(`GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_COMMON_DIR`, `GIT_CONFIG`, `GIT_CONFIG_COUNT`,
`GIT_CONFIG_PARAMETERS`, `GIT_DIR`, `GIT_GRAFT_FILE`, `GIT_IMPLICIT_WORK_TREE`, `GIT_INDEX_FILE`,
`GIT_NAMESPACE`, `GIT_NO_REPLACE_OBJECTS`, `GIT_OBJECT_DIRECTORY`, `GIT_PREFIX`,
`GIT_REPLACE_REF_BASE`, `GIT_SHALLOW_FILE`, `GIT_WORK_TREE`) plus `/^GIT_CONFIG_(?:KEY|VALUE)_\d+$/`.
The match is case-insensitive for Windows. `omitGitRoutingEnv` is applied in two places: the
background runner's env (`src/runs/background/async-execution.ts:729` @v0.71.0,
`...omitGitRoutingEnv(omitExtensionBindingsEnv(process.env))`) and the external CLI's inherited
default (`src/runs/shared/external-cli-runner.ts:91` @v0.71.0). An adapter allowlist is left
unfiltered on purpose (`:90` comment). CHANGELOG 0.71.0 *Fixed*, #2437/#2440.

**Impact** — Suppose cyrup is started from a Git hook, or under `git --git-dir=…`, or under anything
else that exports these variables. Then a background child's `git add` / `git commit` / `git
checkout` works on **the parent's repository and index**, not on the child's cwd or managed worktree.
The change is silent and lands in the wrong repository. Blast radius, recorded as scheduling
information and not as a rating: this only happens when the orchestrator's own environment carries
the variables.

**Fix** — Add a `git_routing_env_names()` predicate in `spawn/` (or `exec/external_cli/env.rs`) that
ports the set and the regex verbatim, including the case-insensitive match. In `spawn_detached.rs`,
call `command.env_remove(name)` for every matching inherited key before `.envs(env_overlay)`. This
keeps the crate's "never `env_clear`" invariant. Build `ExternalEnv::Inherited` the same way at
spawn. Leave `Allowlisted` alone, as upstream does.

**Verify** — An integration test sets `GIT_DIR=<parent>/.git` and `GIT_INDEX_FILE=<tmp>` on the
orchestrator process, launches an async child whose task runs `git rev-parse --git-dir`, and asserts
that the child reports its own worktree. A unit test covers `GIT_CONFIG_KEY_0` and a lower-case
`git_dir`.

## SUBA-107 — The completion-mutation guard is still live after upstream deleted it

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — The guard runs on the production run path: `exec/mod.rs:725` calls
`gates.apply_completion_guard(agent, task, &progress, &mut control)`. The body
(`exec/mod.rs:1695-1712`) calls `evaluate_completion_mutation_guard` and, when it triggers, sets
**`self.exit_code = 1`** and pushes `COMPLETION_GUARD_ERROR_MESSAGE`
(`exec/completion_guard.rs:760`: *"completion-mutation guard: task appeared to require an
implementation change, but no mutating edit/write/bash tool call was observed before the run
completed"*). It also raises the `completion_guard` notice. The classifier is
`exec/task_intent.rs`, which says it is a port of `task-intent.ts` @v0.43.0. #152 (`df3e9a8`) *added*
to it: its body says `mutationTools` is "counted as mutating by the completion guard".

**upstream** — `src/runs/shared/completion-guard.ts` and `src/runs/shared/task-intent.ts` exist at
v0.70.0 (`git cat-file -e`), where `execution.ts:1554` calls `evaluateCompletionMutationGuard`. Both
are **deleted at v0.70.1** by `7c98a696` (*refactor: remove inferred no-edit completion failures
(#2356)*). `completionGuard` has zero hits under `src/` at v0.71.0, and at v0.71.0 it is gone from
the override-field list (`src/agents/agents.ts:2029`). CHANGELOG 0.70.1 *Changed*: *"Stop guessing
whether task wording requires file edits. Successful tasks now follow their process result and
explicitly configured output and acceptance checks. The `completionGuard` setting and
`PI_SUBAGENTS_LLM_INTENT_ARBITER` switch have been removed."*

**Impact** — A delegated task can finish correctly without editing files: the fix was already
there, the investigation found nothing to change, or the wording ("implement…") was loose. In cyrup
that run is still reported **failed** with exit code 1 and a failure notice. Upstream reports it by
its process result. This is a wrong outcome on a normal path, and it is exactly the defect upstream
removed the guard to fix (#2351).

**Fix** — Remove `apply_completion_guard` from the `exec/mod.rs` gate sequence. Remove the
`completion_guard` frontmatter/override key and its management rendering, or accept it as a no-op
with a deprecation warning. Decide whether to keep `completion_guard::is_mutating_bash_command` and
`has_mutation_tool_call`: `exec/control.rs` (long-running guard) and startup evidence use them, so
move them rather than delete them. `SUBA-108` removes `task_intent.rs`'s last consumer. Land the two
items together.

**Verify** — A run whose task says "Implement the fix", with a writer agent that makes no edit and
exits 0, finishes with `exit_code == 0` and no `completion_guard` notice.
`grep -rn COMPLETION_GUARD_ERROR_MESSAGE crates/` is empty.

## SUBA-108 — Acceptance-level inference still reads task wording and agent names

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **STILL OPEN 2026-09-27**: DEFECT 2 (`acceptance.ts:521-522`) is ported and red-without, DEFECT 3's cyrup-it restatement is done, the MINOR doc is corrected, and the invariance-table test the row's Verify asks for exists — keep all of it. TWO blocking items remain, both empirically confirmed at HEAD. (A) Port `acceptance.ts:505`, where the explicit policy is actually COMBINED, not only in the model helper. `crates/cyrup-ext-subagents/src/exec/acceptance/lattice/contract.rs::resolve_effective_for_role` (`:378-400`) must upgrade the INFERRED floor from `NotRequired` to `Attested` when an explicit policy is present and requests one, BEFORE its MAX — i.e. the lattice seam, reached from `crates/cyrup-ext-subagents/src/exec/mod.rs:269`, has to consult the same predicate. The cleanest shape is to stop duplicating the rule: route the lowered explicit policy through `resolve_effective_acceptance` (`explicit: Some(...)`) instead of re-implementing level arithmetic, since as it stands the new `:505`/`:509` code is DEAD on every production path. Note that the lattice's `explicit_floor(NotRequired)` + `is_no_op()` encoding is what swallows the policy: after the upgrade the contract must be non-no-op so `inject_acceptance_contract` emits the block and `evaluate_acceptance` does not short-circuit at `gate.rs:205`. Proof it is still broken (scratch test, since removed): `lower_acceptance_input(json!({"evidence":["review-findings"]}))` then `AcceptanceContract::resolve_effective_for_role(lowered, "scout", Some(AcceptanceRole::ReadOnly), "Audit the flow")` gives `required_level=NotRequired`, `is_no_op()=true` and an unchanged injected task, where upstream gives `attested`, the read-only branch's findings criterion, evidence `[review-findings, residual-risks]`, an emitted prompt and an armed gate. A test at the LIVE seam (lowering → `resolve_effective_for_role` → `inject_acceptance_contract`) is required, red without the change; the existing `a_caller_supplied_policy_reaches_a_read_only_child` only exercises the model function. (B) Make the presence predicate cover every key upstream's `Object.keys` sees, then either delete the `CYRUP-DELTA` at `level.rs:215-220` or restate it truthfully. `report` is a supported acceptance key on BOTH sides (upstream `acceptance.ts:53` + `shared/types.ts:1065`; cyrup `validate_input.rs:71-79`), so `{report: "on"}` — and `{report: "off"}` — must make `explicit_acceptance_requests_policy` true; today it cannot, because `AcceptanceConfig` (`model/types.rs:269-277`) has no `report` field, so either carry `report` on `AcceptanceConfig` (letting the `AcceptanceReportMode` that `resolve_acceptance_report_mode` resolves keep its own path) or thread `acceptance_declares_report` into the predicate. Needs a test: declared `read-only` role + `{report:"on"}` resolves `attested`, red without the change. The delta may only be KEPT if it stops claiming full parity on an input set that in fact diverges; `preserveStagedIndex` (an upstream config key cyrup rejects outright) is a separate pre-existing gap and is NOT this row's to fix, but it must not be papered over by the same delta either. (Non-blocking leftover, still true: `exec/tool_surface.rs` routes through `task_intent::is_review_or_scout_lane_agent`; that is not acceptance behaviour and does not block.)

**cyrup** — `exec/acceptance/model/level.rs::infer_level` (`:234`) still calls
`task_intent::classify_task_mutation_intent` (`:245`), `task_may_mutate` (`:287`) and
`strip_severity_compounds` (`:283`). In its comment it cites `acceptance.ts:90` **@v0.57.0**. So the
inferred level (`none` / `attested` / `checked`, and the required reviewer for async or dynamic
writes) still depends on task wording, on `reviewer|oracle|scout|researcher|analyst` /
`worker` agent-name regexes, and on the risky-keyword pattern (`release|migration|…`).

**upstream** — `7c98a696`'s first half (*refactor: base acceptance inference on declared roles*)
removes all of that from `inferLevel`. At v0.71.0 (`src/runs/shared/acceptance.ts:81-122`), the
level comes from `acceptanceRole` alone:

- `writer` + async/dynamic → `checked` with a required `reviewer` (`:90`);
- `writer` → `checked` (`:101`);
- `read-only` → `none` (`:109`);
- anything else → `attested`, "default lightweight attestation" (`:119`).

CHANGELOG 0.70.1: *"Custom implementation agents must declare `acceptanceRole: writer` to receive
writer acceptance defaults."*

**Impact** — In cyrup, the same agent and task get different acceptance gates from upstream. A task
that mentions "migration" or "security" still becomes `checked` with required evidence, and a scout
given write-sounding wording is gated as a writer. The reverse also happens: once this is ported,
any cyrup agent without an explicit role drops to `attested`. That is why `SUBA-112` (the bundled
worker's missing `acceptanceRole: writer`) has to land with this item. *2026-09-28: `acceptanceRole: writer` was already in `worker.md` at HEAD (`282b9fa2`); `SUBA-112` closed on `claude/lows-next` with the rest of the frontmatter and prose sync.*

**Fix** — Port `inferLevel` @v0.71.0 verbatim into `level.rs::infer_level`. Delete
`task_intent.rs` once `SUBA-107` has removed its other consumer. `exec/tool_surface.rs:1038` uses
`is_review_or_scout_lane_agent`, which is not acceptance, so re-home that helper first. Update the
`read_only_acceptance_inference` tests in `cyrup-it` to state roles rather than wording.

**Verify** — There is one table test over (role ∈ {writer, read-only, none}) × (async, dynamic, plain)
× (task text containing "migration" / "review only" / "implement"), and the level is independent of
the task text in every row.

## SUBA-111 — Agent `allowedAgents` is unported, so a declared delegation restriction fails open

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `allowedAgents` exists only as an axis of the session **capability ceiling**
(`exec/capability_ceiling.rs:11,90,185-203`). It has **zero hits under `discovery/`**, so agent
frontmatter does not read it. `discovery/frontmatter.rs:1325` and the test at `:2185` say that an
unknown frontmatter key is "round-tripped verbatim into `extra_fields`". It is kept, and nothing
enforces it or warns about it. `agentOverrides.<name>.allowedAgents` lands in the settings key census
(`discovery/key_census.rs`) as an *unknown* key. That produces a warning and nothing more.

**upstream** — Added at v0.70.0 (#2312; `git show v0.67.0:src/agents/agents.ts | grep -c
allowedAgents` → 0). At v0.71.0, `src/agents/agents.ts:2118-2119` parses frontmatter
`allowedAgents` and normalises it with `normalizeCapabilityCeilingAllowedAgents`. `:1140-1141` parses
the override form (`string[] | false`, and `false` clears it). The foreground path
(`src/runs/foreground/execution.ts:403`) and the background path
(`src/runs/background/runner-child-launch.ts:53`) pass it as `descendantAllowedAgents`.
`src/runs/shared/child-launch.ts:185-187` turns that into an agent-sourced capability ceiling
`{ version: 1, allowedAgents: [...], denyExtensions: false, sources: ["agent:<name>"] }` on the
child. Management, serialisation (`agent-serializer.ts:82-83`) and async resume
(`async-resume.ts:374-375,622`) carry it.

**Impact** — Suppose an author writes `allowedAgents: reviewer` on an agent to limit what it may
delegate to. Under cyrup, that agent's child can launch any agent the session allows. The restriction
is written in the file, looks accepted, and is ignored. A declared narrowing that fails open is the
same shape as `SUBA-072` (critical when filed). This row is rated medium because it narrows
*delegation targets*, not tool capability, and the session ceiling still applies.

**Fix** — Add `allowed_agents: Option<Vec<String>>` to `AgentDefinition`, parsed in
`discovery/frontmatter.rs` with the ceiling's normaliser, and add the `false`-clears override form to
`AgentOverrideConfig`. At child launch (`exec/spawn_plan.rs`, background `runner_main/`), intersect
it into the child's `CapabilityCeiling` with source `agent:<name>`. Carry it in the recovery
descriptor and in management create/update/render.

**Verify** — An agent with `allowedAgents: reviewer` whose task asks it to delegate to `worker` gets
the ceiling refusal. The same agent delegating to `reviewer` succeeds. An `agentOverrides` value of
`false` restores unrestricted delegation.

## SUBA-109 — `fallbackModels` and persistent model exclusions are still live after upstream removed them

**Kind** stale-port · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `discovery/frontmatter.rs:1006` parses `fallbackModels`. `exec/fallback.rs` builds the
ladder (`build_model_candidates`, model override → primary → `fallback_models`), and
`exec/mod.rs:1021` drives it through `run_fallback_ladder(candidates, &mut runner,
opts.model_exclusions.as_deref())`. `exec/model_exclusions/` (`store`, `persist`, `filter`, `auth`) is
the persistent exclusion store that SCOPE batch 1 (`2bd76ac`) ported. The crate knows upstream moved
on. `exec/spawn_plan.rs:226-230` says *"Upstream checks its ONE launch model (v0.68.0 has no fallback
ladder). cyrup's ladder can reach…"* and marks that as a `[CYRUP-DELTA]` about fast-mode granularity.
It is not recorded as a decision to keep the ladder.

**upstream** — At v0.68.0, `model-exclusions.ts` is deleted and `model-fallback.ts` is renamed to
`model-resolution.ts`, which has no ladder (`f58dfcb5`, #2270). The key is now **refused by name** in
five places at v0.68.0:

- frontmatter (`src/agents/agents.ts:2059`, *"uses removed frontmatter field 'fallbackModels'.
  Configure one model instead."*);
- builtin overrides (`:998-999`);
- runtime definitions (`src/agents/runtime-agent-registry.ts:226`);
- profiles (`src/profiles/profiles.ts:147`);
- management config (`src/agents/agent-management.ts:459`).

The child watchdog config refuses it too (`src/watchdog/child-status.ts:131-132`). CHANGELOG 0.68.0
*Removed*: *"Remove `fallbackModels`, all same-launch model switching (including read-only HTTP 429
continuation), and persistent model exclusions. Retry another model only with a later explicit
launch."*

**Impact** — An agent file written for current upstream never contains the key. So the visible
divergence goes the other way: a cyrup agent that uses `fallbackModels` is refused when it is copied
to upstream, and in cyrup one launch can silently finish on a different model from the one the
caller asked for. `SUBA-089` already stops a re-dispatch after tools have run, and that bounds the
damage. The row is low because the ladder is cyrup behaviour working as designed, just against a
design upstream has retired. **This needs a decision, not only effort**: follow upstream, or record
a `[CYRUP-DELTA]` decision of record and re-kind this row `port-divergence`.

**Fix** — To follow upstream: refuse the key at all five parse sites with upstream's text. Collapse
`run_fallback_ladder` to one attempt. Retire `exec/model_exclusions/` and its persisted store (with a
migration that ignores an existing file).

*Pass 2 note:* the same removal replaced `attemptedModels`/`modelAttempts` with `requestedModel` in
run results and status (`2b64ced0`, v0.68.0; `subagent-runner.ts:1497,1528` @v0.71.0), and
`exec/fallback.rs:1336-1338`'s "UNPORTED" note for `ReadonlyContinuation` is now stale. Both go with
this fix.

**Verify** — An agent file with `fallbackModels:` fails discovery with upstream's message. A retryable
failure on the primary model ends the run and does not start a second attempt.

## SUBA-112 — The bundled `worker` agent's frontmatter has drifted

> **CLOSED 2026-09-28.** (on `claude/lows-next`) The bundled `resources/agents/worker.md` is byte-identical to `agents/worker.md` @v0.71.0: `defaultContext: fresh` and the three prose drifts are synced. Test: `the_bundled_worker_starts_fresh_and_is_a_writer`. **Ledger correction:** `acceptanceRole: writer` was already at HEAD (landed in `282b9fa2` with the `SUBA-108` work), so the `cyrup` paragraph below was half stale. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `crates/cyrup-ext-subagents/resources/agents/worker.md:10` is `defaultContext: fork`, and
the frontmatter has no `acceptanceRole`.

**upstream** — `agents/worker.md` @v0.71.0: `:5 acceptanceRole: writer` (added at v0.70.1 by
`7c98a696`; absent at v0.70.0) and `:11 defaultContext: fresh` (still `fork` at v0.70.1). CHANGELOG
0.71.0 *Changed*: *"Packaged `worker` agents now start with fresh context instead of forking the
parent's conversation"* (#2384). The same commit added `acceptanceRole: writer` to
`claude-code-writer.md`, `codex-exec-writer.md` and `cursor-agent-writer.md`. cyrup does not bundle
those three. `oracle.md` stays `fork` on both sides.

**Impact** — cyrup's bundled worker inherits the parent's full conversation on every call. That costs
more tokens and exposes more context than upstream now intends. Without `acceptanceRole: writer`, the
worker falls to `attested` as soon as `SUBA-108` lands.

**Fix** — Set `defaultContext: fresh` and add `acceptanceRole: writer` in `resources/agents/worker.md`.
The prose also drifted: the `First read the provided context…` paragraph, the
`contact_supervisor`-unavailable fallback, and the source-discoverability bullet. Take those as a
separate text sync.

**Verify** — `/subagents` shows `worker` with a fresh context and the writer role. A worker launched
with no `context` argument gets a fresh child session.

## SUBA-113 — tracker: thirteen `config.json` keys are self-declared unported, and until now no item carried them

**Kind** tracker · **Severity** tracker · **Effort** — · **Confidence** confirmed (cyrup side read; upstream side not re-read per key)

`crates/cyrup-ext-subagents/src/registration/mod.rs:525-553`, `UNPORTED_CONFIG_KEYS` (13 entries,
cited against `shared/types.ts:2596-2690` @v0.68.0), lists these keys: `forkContext`,
`modelResponseAliases`, `mainWindowRenderer`, `orcaProgressTabs`, `toolTimeoutMs`,
`checkpointBeforeDeadlineMs`, `toolBudget`, `usageBudget`, `worktree`, `worktreeProvider`,
`worktreeBranchPrefix`, `intercomBridge` and `resultScanLogging`. Its doc says: *"Each is a real
feature gap, not a decision that it is out of scope; **the ledger carries them**."* Before this row,
none of the thirteen had a `SUBA-` id. `modelResponseAliases` appears only as an UNVERIFIED lead in
`09a`'s census, and `mainWindowRenderer` only as a `SUBA-061` aside. The keys are **reported, not
silently dropped** (`SubagentExtensionConfig::config_warnings`), so none of them is a silent-ignore
defect. That is why this row is a tracker and not an item.

*Pass 2 (2026-09-24): `checkpointBeforeDeadlineMs` escalated to `SUBA-128`, `modelResponseAliases` to
`SUBA-119`. `worktreeProvider` stays here (the `worktrunk` provider is the 09a census lead this row
absorbs).*

*2026-09-28 (on `claude/lows-next`): `SUBA-128` closed, so `checkpointBeforeDeadlineMs` is no longer
in `UNPORTED_CONFIG_KEYS`. The const now holds 11 entries (`registration/mod.rs:559`), and
`modelResponseAliases` is not among them either.*

**Escalates to an item** when a key's feature is shown to change behaviour for a user who sets it.
The likely first candidates are `checkpointBeforeDeadlineMs` (the per-call form was added at v0.68.0
for async single runs) and `worktreeBranchPrefix`. Each needs its own row, with both sides read.

## SUBA-114 — Child tool plans are still pruned to the parent session's registry

**Kind** stale-port · **Severity** high · **Effort** M · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-26** (`850eb70`, #156)

> **Closure evidence.**
>
> * **The prediction is gone, everywhere it reached** (pi `b12496b8`). `exec/tool_surface.rs`:
>   `host_builtin_tool_names`, `HOST_BUILTIN_TOOL_NAMES`, `REPOSITORY_INSPECTION_TOOLS`,
>   `NATIVE_COORDINATION_TOOL_NAMES` (no other consumer), the host partition, the host `read`
>   throw, `is_review_or_scout_lane_agent` / `missing_permitted_repository_inspection_tools` /
>   `format_review_lane_tool_contract_failure`, the omission warning, and the
>   `ResolvedToolSurface::{unavailable_host_builtins, warnings}` wire fields (their only producer)
>   are deleted; `resolve_tool_surface{,_in}` lost the host parameter. `RunOptions`,
>   `RunnerConfig`, `ExecSingleStepExecutor` (field + `foreground` argument) and every launch site
>   (`extension/executor/{foreground,background,chain}.rs`) no longer carry
>   `host_available_builtins`; `task_intent::is_review_or_scout_lane_agent` went with its only
>   caller; `SubagentError::ToolContractUnsatisfiable` keeps one producer (the fanout refusal).
>   Old payloads stay readable: neither `RunnerConfig` nor `ResolvedToolSurface` denies unknown
>   fields (`RecoveryDescriptor` does, but never carried the key) —
>   `session_state::tests::a_runner_config_carrying_the_removed_host_observation_still_decodes`,
>   `tool_surface::tests::a_payload_carrying_the_removed_host_fields_still_decodes`.
> * **The child-side guard, brought to v0.71.0.** Upstream deleted `PI_CORE_CHILD_TOOLS` in #1356
>   (`51cca33e`, v0.55.0) and throws from `agent_start`; cyrup still carried the v0.43.0 floor
>   (which would have waved a missing `bash`/`edit` through) and only wrote a file. Now
>   `tool_availability::write_child_tool_diagnostic` diffs against the registry alone, the MCP
>   line is v0.71.0's wording, and `prompt_runtime::refresh_tool_diagnostic` aborts the child's own
>   run (`ControlOp::Abort`) when anything is missing; the parent reports the file as the run's
>   error (`attempt_runner::diagnose_attempt_error`, unchanged rank). Upstream's `host:"parent"`
>   diagnostic arm (in-process foreground child) is not ported: a cyrup child is always its own
>   process.
> * **Verify (production path, real parent `AgentSession`)** —
>   `crates/cyrup-it/tests/subagents/child_tool_plan_not_predicted_from_parent.rs`: a parent built
>   with `--tools subagent,read` AND a registry of only those two (asserted: no `bash`/`edit`/`grep`
>   definition) launches `worker` (`read, bash, edit`), `reviewer` and `scout` in the foreground
>   (`a_narrow_parent_launches_foreground_children_with_every_declared_tool`), a `/chain` step
>   (`…_a_chain_step_…`) and a detached background run through the written `runner-config.json`
>   and production `run_with` (`…_a_detached_background_child_…`); each child's REAL argv carries
>   the full `--tools`. `a_child_whose_registry_lacks_a_required_tool_is_refused_at_agent_start`:
>   a real child session (`--tools read,bash,edit`, registry `read` only, runtime from
>   `prompt_runtime_from_env`) ends its turn ABORTED without the model request ever being polled,
>   with upstream's five-line message; `the_parent_reports_the_childs_agent_start_refusal_as_the_run_error`
>   carries it back as the subagent run's error. Unit: `tool_surface::tests::{every_declared_core_tool_is_kept_so_the_child_registry_decides,
>   review_and_scout_lanes_resolve_like_any_other_agent, a_capability_ceiling_is_still_honoured}`,
>   `spawn_plan::tests::declared_tools_reach_the_child_argv_and_required_env_for_every_lane`,
>   `tool_availability::tests::a_missing_core_builtin_is_reported_there_is_no_floor`,
>   `prompt_runtime::tests::agent_start_aborts_the_run_when_the_real_registry_lacks_a_required_tool`.
> * **Correction to the reach stated below.** The CLI `--tools` bounds the ACTIVE set; cyrup's
>   registry (`DynamicToolState`, what `all_tools` reads) is bounded by `tool_availability`
>   (`Availability`, the SDK's `allowedToolNames`), which the CLI never sets. So on the CLI the
>   prediction mostly saw all eight built-ins and pruned little; it bit when the registry was
>   narrowed (SDK/harness) and for review/scout lanes there. The fix is the same either way; the IT
>   parent is narrowed on both axes so the deleted code would have pruned it.
> * **Tests deleted or rewritten** (they pinned the removed behaviour): `tool_surface` — the 6
>   `host_builtin_tool_names_*`/`a_malformed_tool_row_*` observer tests, the vocabulary guards
>   (`host_builtin_tool_names_track_the_tool_registry`, `repository_inspection_tools_match_upstream`,
>   `native_coordination_names_match_the_crate_constants`,
>   `every_intercom_persona_is_covered_by_the_coordination_exemption`), the 7 host-intersection
>   tests (two rewritten host-free as `a_capability_ceiling_is_still_honoured` and
>   `an_intentionally_empty_review_allowlist_launches_with_no_tools`),
>   `the_host_read_throw_precedes_the_ceiling_read_throw`,
>   `a_host_that_omits_subagent_does_not_revoke_the_supervisor_tool`, all 11 review-lane/warning
>   tests (the port of upstream's deleted `child-tool-plan-diagnostics.test.ts`) and the two wire
>   tests for the removed fields; `spawn_plan` —
>   `a_reviewer_launch_with_a_host_missing_read_is_refused_through_run_options` and
>   `the_same_reviewer_launch_proceeds_when_the_host_is_unknown` (replaced by
>   `declared_tools_reach_the_child_argv_and_required_env_for_every_lane`); `runner_main::executor`
>   — `the_step_executor_forwards_the_observation_to_every_step`,
>   `the_foreground_chain_executor_carries_the_observation`; `extension::executor` —
>   `the_observation_seam_reads_the_live_host`; `session_state` — the two round-trip tests
>   (replaced by the legacy-decode test); `tool_availability::tests::the_core_tool_floor_is_unioned_into_available`
>   (replaced by `a_missing_core_builtin_is_reported_there_is_no_floor`).

**cyrup** — `exec/tool_surface.rs::host_builtin_tool_names` (`:130-160`) reads the LAUNCHING session's
`HostServices::all_tools`, and production passes it into every launch: `extension/executor/foreground.rs:1177`,
`extension/executor/background.rs:744`, `extension/executor/chain.rs:161`, `extension/executor/mod.rs:1030,1042`
(and on to the detached runner via `background/runner_main/config.rs:235`). `resolve_tool_surface_in`
then partitions the agent's declared built-ins by that set (`:584-593`) and DROPS every one the parent
does not have, with a warning (`:975-990`); for a `reviewer`/`scout` it REFUSES the launch outright
(`:947-958`). The parent's registry is itself bounded by the parent's own `--tools`/`--no-tools`:
`cyrup-session-svc/src/builder.rs:1128,1566-1570` filter both built-ins and extension tools through
`resolve_allowed_tool_names`, and `session/tools.rs:94-99` filters late tools the same way — so
`all_tools` (`host_services.rs:2178-2199`, reading that registry) reports only what the parent was
allowed.

**upstream** — `b12496b8` (*fix: stop predicting a child's tools from its parent's session*, #2289,
in v0.70.0) deleted the prediction: `hostAvailableBuiltins` and `getHostBuiltinToolNames` exist at
v0.68.0 (`src/runs/shared/child-tool-plan.ts:198,354,384`) and are gone at v0.71.0
(`git show v0.71.0:src/runs/shared/child-tool-plan.ts | grep -c hostAvailable` → 0), together with
the review/scout lane check and its audit fields. The commit's reasoning: a child is a separate
session that builds its own tools, and it already validates its real registry against required tools
at `agent_start`.

**Impact** — Start the orchestrator as a narrow dispatcher (`--tools subagent,read`) and every child
silently loses the `bash`/`edit`/`grep` its agent declares; a `reviewer` or `scout` is refused
outright. Because a cyrup child is launched with its effective allowlist as its own `--tools`, a
grandchild loses more at each hop. Upstream fixed exactly this compounding loss.

**Fix** — Stop threading `host_available_builtins` into the plan (drop it from `RunOptions`, the
runner config and the recovery path), delete the partition at `tool_surface.rs:584-593`, the lane
refusal and the omission warning, and keep the child-side check (the child's own registry vs. its
required tools). Keep `NATIVE_COORDINATION_TOOL_NAMES` only if another consumer needs it.

**Verify** — A parent built with `tools: ["subagent"]` launches a `worker` declaring
`read, bash, edit`; the child's `--tools` carries all three. A `reviewer` launch from the same parent
succeeds.

## SUBA-115 — Nested control cascades escape the issuing subtree

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-24** (`7ba9e03`): `background/cascade.rs::is_control_descendant` ports `isNestedControlDescendant`; `settle.rs::cascade_to_descendants` passes this run's id as issuer when `nested_self` is set, for stop, interrupt and timeout. Verify: `background::cascade::tests::a_nested_issuer_reaches_only_its_own_subtree` (red with the filter removed). Reach note: cyrup does not yet mint a nested route for its own children (`exec/spawn_plan.rs:993`), so today the cascade only has a route when cyrup was itself launched under an inherited one.

**cyrup** — `background/cascade.rs::cascade_to_nested_async_descendants` (`:167-230`) projects the
whole registry of the ROOT route (`project_nested_events_in(&roots.nested_events(), route)`),
flattens every descendant (`flatten_nested_runs`, `:117-131`) and delivers the verb to every
`running`/`queued` one (`is_live_state`, `:137`). It never consults this run's own address, although
the runner carries it: `RunnerConfig::nested_self` (`background/runner_main/config.rs:297`), set in
production at `extension/executor/background.rs:805`. Called from `runner_main/settle.rs:481-491` for
stop, interrupt and timeout.

**upstream** — `bd03854a` (#2243, v0.68.0): `subagent-runner.ts:2384-2385` @v0.71.0 defines
`isNestedControlDescendant = (run) => !config.nestedSelf || run.path.some((entry) => entry.runId === id)`
and applies it in all three fan-outs (`:2401`, `:2432`, and the stop loop). Root-wide control is kept
for a root run (`nestedSelf` absent). The commit's test drives stop/interrupt/timeout from `A` and
asserts sibling tree `B`/`B1` is untouched.

**Impact** — A nested child that is stopped, interrupted or times out stops or pauses every live run
under the same root, including sibling subtrees it never launched. Their work is lost or parked for
no reason, and nothing tells the user why.

**Fix** — In `cascade_to_nested_async_descendants`, take `nested_self: Option<&NestedParentAddress>`
plus this run's id and skip any `run` whose `path` does not contain this run id when `nested_self` is
`Some`. Thread it from `settle.rs::cascade_to_descendants`.

**Verify** — Seed a registry with `root→{A→A1, B→B1}`; a cascade issued by `A` (with `nested_self`)
writes control files into `A1` only; issued by the root, into all four.

## SUBA-116 — A paused async run cannot be stopped

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `background/control.rs::stop` refuses anything not `Running`/`Queued` (`:950-952`,
`StopOutcome::NotStoppable`), and its doc (`:851-855`) says a stop of a `Paused` run is an error "as
upstream". A paused run keeps its active-capacity slot (`background/active_async_capacity/inspect.rs:266-273`,
pinned by `tests.rs::a_paused_run_keeps_its_slot`).

**upstream** — `03b92ee7` (#2176, v0.68.0). `src/runs/foreground/async-stop-action.ts:120-121`
@v0.71.0 admits a whole-run stop of a `paused` run, and `sealPausedRun` (`:31-101`) — after checking
the runner's process-terminal proof and the paused result's identity — rewrites status and result to
`stopped`, marks paused/pending steps stopped, and releases the active-run index. The runner side
(`subagent-runner.ts` `stopRunner`) now also acts on `paused` and converts interrupted results to
stopped.

**Impact** — A run the user paused and no longer wants can only be resumed or left alone. It stays
resumable and holds a capacity slot, so it can block new async launches when capacity is tight.

**Fix** — Port `sealPausedRun` into `control.rs::stop` for the whole-run, `Paused` case (child-scoped
stop stays pending/running-only), gated on the runner's observed process-terminal proof; release the
capacity slot and update the active-run index to `stopped`.

**Verify** — Interrupt a run, wait for `Paused`, `stop` it: status and result read `stopped`,
`resume` refuses it, and the capacity slot is released.

## SUBA-117 — The worktree clean check counts the crate's own project directory as dirt

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-27**: upstream's `probeWorktreeSource` excludes its own directory from the status call: `await tx.gitChecked(toplevel, ["status", "--porcelain", "--", `:!${PROJECT_SUBAGENTS_RELATIVE_DIR}`])` (`src/runs/shared/worktree.ts:351` @v0.71.0, read at the tag, under the comment *"pi-subagents writes durable runtime state under .pi/subagents/ by default; that state must not make managed isolation unusable for later runs"*, with the same refusal string cyrup carries). Ported at `crates/cyrup-ext-subagents/src/spawn/worktree.rs:495-498`: `resolve_repo_state` builds `format!(":!{}", crate::artifacts::PROJECT_ARTIFACT_ROOT)` and passes `["status", "--porcelain", "--", &artifact_root_exclude]`, with `:491-493` recording that the pathspec resolves against the TOPLEVEL while `PROJECT_ARTIFACT_ROOT` is `<cwd>`-relative — upstream's own asymmetry, kept rather than silently corrected. The per-worktree status probes are left alone (`:1519`), as upstream does. A chain run, refinement, schedule or cleanup plan under `.cyrup-subagents/` no longer makes a later `worktree: true` run refuse as dirty, so the user no longer has to discover and git-ignore that directory. Verify: `cyrup-ext-subagents spawn::worktree::tests::create_worktrees_ignores_the_project_artifact_root` (real temp git repo, state written under the artifact root), with the untracked-file-elsewhere refusal still asserted alongside.

**Window note.** This is in-baseline (upstream has the exclusion at v0.43.0), so by scope it is
`09`'s. It is filed here because this pass read both sides and `09` was not open for items in this
pass; ids never move.

**cyrup** — `spawn/worktree.rs::resolve_repo_state` runs `git status --porcelain` over the whole
toplevel (`:489-495`) and refuses with "worktree isolation requires a clean git working tree". The
crate writes its own project state under `<cwd>/.cyrup-subagents/` (`artifacts.rs:42,162-176`): chain
runs by DEFAULT (`resolve_chain_runs_dir`'s `project` arm, used by `extension/tool/task_items.rs:812`),
agent refinements (`exec/agent_refinements.rs:254`), schedules (`background/scheduled_runs/store.rs:137`)
and cleanup plans (`spawn/cleanup_plan/model.rs:421`). Nothing writes a `.gitignore` for it.

**upstream** — the same check excludes its own directory: `["status", "--porcelain", "--", ":!.pi-subagents"]`
at v0.43.0 (`src/runs/shared/worktree.ts:140`), `:!${PROJECT_SUBAGENTS_RELATIVE_DIR}` at v0.47.1
(`:141`) and v0.71.0 (`:351`, with the comment that this state "must not make managed isolation
unusable for later runs").

**Impact** — After a chain run (or a refinement, schedule or cleanup plan) in an untracked-clean
repository, a later `worktree: true` run is refused as dirty, and committing or stashing does not
help. The user must discover and git-ignore `.cyrup-subagents/`.

**Fix** — Append `"--", ":!.cyrup-subagents"` (the `PROJECT_ARTIFACT_ROOT` constant, relative to the
toplevel) to the status call in `resolve_repo_state`. Leave the per-worktree status probes alone, as
upstream does.

**Verify** — In a temp repo, create `.cyrup-subagents/chain-runs/x`, then `create_worktrees`
succeeds; an untracked file elsewhere is still refused.

## SUBA-118 — Abort recovery (resume once after a compaction-induced abort) is unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `exec/fallback.rs:1336-1338` names `AbortRecovery` in its "UNPORTED upstream kinds" list;
`ABORT_RECOVERY`, `after_compaction` and `compaction_settlement` have zero hits in the crate. The child
stream already carries `compaction_start` (`exec/ndjson.rs:199,267`), so the evidence exists.

**upstream** — `src/runs/shared/abort-recovery.ts:3,67-116` @v0.71.0 (entered in `v0.57.0..v0.67.0`
via `40b20b70`, `feb24a40`; still live after the v0.68.0 ladder removal). `planAbortRecovery` resumes
the retained session ONCE with `ABORT_RECOVERY_PROMPT` only when: a session file exists, no stop /
interrupt / timeout / budget / structured-output failure / in-flight tool / unresolved tool call, the
child settled after compaction (`afterCompactionSettlement`), the terminal assistant message is an
empty, zero-output abort (`stopReason: "aborted"` or a provider-abort error), and there was useful
progress. Otherwise it settles, with the diagnostic `Compaction-induced child abort could not be
resumed safely: <reason>.` when the compaction case applies. Sites: `subagent-runner.ts:1341-1362`
and `execution.ts:1855-1903` (two-attempt loop); the runner also appends "Child failure followed
session compaction and agent settlement." (`formatChildFailureDiagnostic`, `:490-510`).

**Impact** — A long child that auto-compacts and then hits a provider/transport abort fails, and
its work so far is reported as a failed run. Upstream resumes it once on the same model.

**Fix** — Track `compaction_start` / `compaction_end{willRetry}` / `agent_settled` in the attempt
driver to derive `after_compaction_settlement`, port `planAbortRecovery` verbatim, and in `run_sync`
allow ONE `--session <file>` re-dispatch with the recovery prompt when it says `resume`. Carry the
settle diagnostic into the error.

**Verify** — A fake child that emits `compaction_start`, `agent_settled`, then an empty aborted
assistant message after a successful tool call is re-launched once with the recovery prompt and the
run succeeds; without the compaction events it settles with no re-launch.

## SUBA-119 — A child served by a different model than requested is accepted silently

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **STILL OPEN 2026-09-27**: everything else in the row is closed; TWO items remain. (1) THREE of the five production wiring seams have NO test that fails without them, and they are exactly the ones carrying the background/detached path the descriptor half is about. Reverting all three simultaneously — `crates/cyrup-ext-subagents/src/extension/executor/chain.rs:171` (drop `.with_model_response_aliases(cfg.model_response_aliases.clone())`), `crates/cyrup-ext-subagents/src/extension/executor/background.rs:802-803` (`requested_model_response_aliases.and(None)`, so `RunnerConfig.model_response_aliases` is never populated at the launch site) and `crates/cyrup-ext-subagents/src/background/runner_main/turn_loop.rs:400` (`config.model_response_aliases.clone().and(None)`, so it never reaches `ExecSingleStepExecutor`) — leaves the full suite passing 4767/4767, and `grep -rn model_response_aliases --include=*.rs crates/ | grep -v ": None"` is empty outside `crates/cyrup-ext-subagents/src`, so with those three lines dead a background/detached child gets no declared alias at all and the descriptor always carries `None` in production, silently. The two green tests only cover hops DOWNSTREAM of the seam they sit at (`executor.rs:1706` sets `runner.model_response_aliases` by hand; `recovery_descriptor.rs`'s test sets `config.model_response_aliases` by hand) — a test that hands the function its own input cannot notice that nothing in production supplies it. Needed: a test at the `spawn_background_steps` seam that a declared `cfg.model_response_aliases` lands on `RunnerConfig` (and so on the descriptor `for_single_launch` writes), one at the `turn_loop` seam for the `RunnerConfig` → `ExecSingleStepExecutor` hop, and one at the `chain.rs` construction seam — each proven red by reverting its OWN line. (2) The revive does not reproduce `subagent-executor.ts:2136`; it is adjacent and MORE PERMISSIVE. `background.rs:802-803` is `requested_model_response_aliases.or_else(|| cfg.model_response_aliases.clone())` and `BackgroundStepsSpec.model_response_aliases` is a single `Option`, so "a revive whose descriptor declared nothing" is indistinguishable from "an ordinary launch" — `requests.rs:534`'s own doc admits it ("`None` from every ordinary producer, which takes the LIVE `config.json` value"), and `control.rs:598` hands over `descriptor.model_response_aliases.clone()`, which is `None` for every run launched before an alias was declared. Upstream is `recoveryDescriptor ? recoveryDescriptor.modelResponseAliases : foregroundContract?.modelResponseAliases`, whose revive never touches `deps.config.modelResponseAliases`, under the comment *"Absence in the retained contract is meaningful; never acquire current aliases"* — the descriptor's absence is LOAD-BEARING, not a hole to fill. Concrete divergence: a router starts substituting, run A fails `model_verification_failed`, the operator declares the alias in `config.json` and revives A — pi still fails A (a new independent run is required), cyrup now passes it. That leaves the shipped error text's own sentence ("Configuration changes affect new independent native runs; resumed native runs retain their launch-time declaration") still FALSE in exactly the case DEFECT 2 was refused over, and the code comment at `background.rs:800-801` asserting parity with `:2136` is not something the `or_else` has. Fix: let the spec distinguish a retained contract from no contract (`Option<Option<ModelResponseAliases>>`, or a separate `retained_model_response_aliases: bool`/enum set only by `control_resume`) so a revive uses the descriptor's value verbatim INCLUDING its absence, plus a test that a revive whose descriptor declares nothing gets `None` on the runner config while `cfg.model_response_aliases` is `Some`.

**Window note.** The verification entered in `v0.47.1..v0.57.0` (`model_verification_failed` is in
`src/runs/shared/model-fallback.ts` at v0.57.0, absent at v0.47.1); `modelResponseAliases` entered in
`v0.57.0..v0.67.0`. `09a`'s pass over its own window missed the first half.

**cyrup** — no counterpart: `model_verification_failed`, `modelResponseAliases` and
`response_alias` have zero hits in production code; `modelResponseAliases` is only listed in
`registration/mod.rs::UNPORTED_CONFIG_KEYS` (`:525-553`, `SUBA-113`). The child's reported model
reaches `progress.model` and the result without comparison.

**upstream** — `src/runs/shared/model-resolution.ts:14-34` @v0.71.0: when the launch model was chosen
by the child's own configuration (`verifyModel = Boolean(candidate) && !options.modelOverrideFromParent`,
`execution.ts:1836`), each non-tool-call assistant message's `model` is compared with the candidate
(base id, registry `id`, and the id leaf all accepted) and a mismatch not declared in
`config.modelResponseAliases[expected]` fails the run with `model_verification_failed: native Pi
child reported a different model than the launch candidate. Expected '…' but observed '…'.`
(`execution.ts:1100-1102`, `run-child-session.ts:513`). The alias map is validated at config load
(`extension/config.ts:176`) and carried in recovery descriptors.

**Impact** — A provider or router that silently substitutes a different model (a fallback tier, an
alias that moved) produces a "successful" run on a model nobody chose. Upstream fails it loudly.

**Fix** — Port `formatSubagentModelVerificationError` and apply it in the attempt driver on each
assistant message when the model came from the agent/config; add `modelResponseAliases` to
`config.json` parsing (leave `UNPORTED_CONFIG_KEYS`) and to the recovery descriptor.

**Verify** — A fake child reporting `other/model` for a launch of `prov/model` fails with the
upstream text; the same run with `modelResponseAliases: {"prov/model": ["other/model"]}` succeeds.

## SUBA-120 — Watchdog findings have no importance, so all of them reach the model

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: **Items 1, 2 and 4 of the Fix were already in the tree and independently proven** (`importance` required with no default at `exec/acceptance/.../review.rs:326`/`:336` with rejected-not-defaulted tests at `:1241`/`:1257`; `route_warning` at `watchdog/runtime.rs:1477-1485`, the port of upstream's `:548-550`; both usage strings updated). This pass fixed the record and one predicate. (1) `crates/cyrup-ext-subagents/src/watchdog/types.rs:118-148` no longer claims `notify.ts` has no importance filter. Read at the tag: there are FOUR importance filters upstream, one ported and three not — `collectWatchdogFindings` inside `buildSubagentNotifyPayload` is `if (warning.importance !== "high") continue;` (`src/runs/background/notify.ts:716` @v0.71.0) with `:743` rendering the survivors into the completion notice's `resultPreview` under `High-importance watchdog concerns:`; `childWatchdogProgressForModel` is `.filter((warning) => warning.importance === "high")` (`src/watchdog/child-status.ts:223`); and `withAggregatedToolUsage` appends the surviving findings to the subagent tool RESULT (`src/runs/foreground/subagent-executor.ts:3021`). A tag-wide grep for BOTH spellings returns exactly those four and nothing more, so the new paragraph's enumeration is exhaustive and every line number in it resolves. The paragraph also carries the explicit correction of the old claim WITH the reason the narrow grep missed `:716` (that site is spelled `!== "high"`), and a MUST for whoever ports the projection. The stated reason the three filters stay unported is established, not asserted: all three read `ChildWatchdogProgress.warnings`, and cyrup's `ChildWatchdogStateSnapshot` (`watchdog/child_status.rs:176-193`) has `phase`, `seq`, `last_update`, `follow_up_pending`, `reason` and `timed_out` and NO `warnings` field, while `grep -rn WatchdogWarning crates/cyrup-ext-subagents/src/ | grep -v '/watchdog/'` returns nothing — so there is no list to filter at any of the three sites and adding the filter without the field would be dead code. **What is missing is the PROJECTION, not the filter**, which is a finding about the row rather than a shortfall in the change: the row's third Fix item ("filter the notice and tool-result projections to high") cannot be satisfied as written because those projections do not exist in cyrup, and that is now written down at the type with a MUST instead of being denied by a false claim. The `CYRUP-DELTA` label was correctly DROPPED from that paragraph — an unported behaviour is never a delta — while the genuine delta below it keeps its label: upstream's `warningMeetsThreshold` (`runtime.ts:543-545`) re-validates `warning.importance` with `WATCHDOG_WARNING_IMPORTANCES.includes(...)` because a TypeScript string can carry an out-of-union value at runtime, which a Rust enum makes unrepresentable — a mechanism difference at identical behaviour for every well-typed input. (2) `parse_test_command` (`watchdog/register_main.rs:638`, guard at `:662`) now requires a non-empty remainder after consuming ONE separator character, which is what upstream's `^test\s+(concern|blocker)\s+(low|medium|high)\s+([\s\S]+)$` (`register-main.ts:158`) means: `\s+` is greedy but backtracks and `[\s\S]` matches whitespace, so TWO trailing spaces DO match with a whitespace capture that `.trim()` at `:160` empties, while ONE leaves the capture empty and the match fails — which is what selects the specific usage error over the general one. Proven red-without by reverting the guard to `&& true`: `1 test run: 0 passed, 1 failed`, `left: Some((Blocker, High, ""))` / `right: None`; restored, and the whole watchdog scope is 339/339. The neighbouring inputs (`test blocker `, `test `, `test blocker  `, `test concernx`, `test blocker lowish x`) were checked against the regex too and the port agrees on all of them. The function's doc records the honest scope note that both sides' handlers `args.trim()` first (`register-main.ts:249`), so no slash-command input reaches the function with trailing whitespace and the divergence was at the function contract, not at the operator. Scope: the reconcile diff is 42 lines in `types.rs` and 51 in `register_main.rs`, all under `crates/cyrup-ext-subagents/src/watchdog/`, with no `#[ignore]`/`should_panic` added. Verify: `cyrup-ext-subagents watchdog::register_main::tests::exactly_one_trailing_space_is_not_a_test_command_but_two_are`.

**cyrup** — `watchdog/types.rs:104` still has `WatchdogConfidence { Medium, High }` and
`WatchdogWarning::confidence` (`:303-305`); `watchdog/review.rs:346-348` advertises `confidence` on
`watchdog_warn`. There is no `importance` anywhere under `watchdog/`, and delivery steers every
threshold-passing finding into the parent (`WatchdogDelivery::Steer`, `watchdog/types.rs:276-282`,
`runtime.rs:1480`).

**upstream** — `52fc6b4b` (*feat: route watchdog findings by importance*, v0.68.0). `watchdog_warn`
requires `importance: low|medium|high` (`src/watchdog/review.ts:30` @v0.71.0; `confidence` removed);
`runtime.ts:548-550` `routeWarning` sends only `high` to the model (`displayWarning`) and appends
`low`/`medium` as a user-only session entry (`displayUserWarning`); completion notices and tool
results project only high-importance findings (`notify.ts` `collectWatchdogFindings`,
`subagent-executor.ts` `withAggregatedToolUsage`); `/subagents-watchdog test` takes an importance.

**Impact** — Every watchdog concern steers the parent model and costs a turn; upstream keeps low and
medium findings visible to the user and out of the model's context.

**Fix** — Replace `confidence` with a required `importance` in `WatchdogWarning`, the tool schema and
the prompt text; route by importance in `runtime.rs` (user-only entry for low/medium); filter the
notice and tool-result projections to `high`; update the test command's grammar.

**Verify** — A review emitting one `low` and one `high` finding steers only the `high` one; the `low`
one is rendered as an entry and absent from the next model request.

## SUBA-121 — Runtime-registered agents never receive settings defaults or overrides

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: No behavioural defect was alleged in the ported logic and none was changed: the narrowing (`runtime_agent_overrides`, four fields), the fill-if-unset defaults and the `defaultExtensions` exclusion still match upstream, read at the tag — `runtimeAgentOverrides` keeps only `model`, `defaultProvider`, `fast` and `thinking` and drops an entry that narrows to empty, and `applyRuntimeAgentSettings` runs `applySubagentDefaultModel` then `applySubagentDefaultThinking` then `applyCustomAgentOverrides(withDefaults, runtimeAgentOverrides(user), runtimeAgentOverrides(project), …)` and NEVER `applySubagentDefaultExtensions` (`src/agents/agents.ts:1596-1627` @v0.71.0); cyrup's `discovery/merge.rs:385-407` narrows the identical four fields with the identical empty-drop and `:438-465` runs the three passes per agent, which is equivalent because every pass is per-agent independent. What was missing was a test through the real seam: all seven SUBA-121 tests lived in `discovery::merge::tests` and called `merge::apply_runtime_agent_settings` on a hand-built `Vec`, so the ONE line that makes settings reach a runtime-registered agent — `crates/cyrup-ext-subagents/src/discovery/mod.rs:2074-2076`, `if let Some(runtime_slice) = agents.get_mut(before_runtime..) { merge::apply_runtime_agent_settings(runtime_slice, &cfg.override_settings); }` — was covered by nothing. It now is: `crates/cyrup-ext-subagents/src/tests/runtime_agent_registration_integration.rs:601` builds a real `tempfile::tempdir()` user tree, reads a real `settings.json` through `crate::discovery::load_layered_override_settings`, registers one agent through `RuntimeAgentRegistry`, and drives `discover_agents(&cfg, None)`, asserting on the DISCOVERED agent that `model == "p/m"`, `model_source == SettingsDefault` and `thinking == Some("high")`. Red-without was reproduced twice with two independent sabotages: replacing line 2075 with `let _ = runtime_slice;` fails at `:637` with ``"`subagents.defaultModel` must reach a model-less runtime agent" / left: None / right: Some("p/m")``, and disabling the narrowing in `merge.rs::runtime_agent_overrides` fails at `:661` with ``"the override narrowing keeps `tools` out: Some([Builtin("Read")])"`` — so the NEGATIVE assertions have teeth and are not vacuous, which matters because the same `agentOverrides.runtime-scout` entry is proven to have arrived by the POSITIVE `thinking == Some("high")` assertion in the same test. The `defaultExtensions` exclusion is likewise paired with a CONTROL: a second, on-disk agent `disk-scout` at the same scope IS asserted to take both `defaultModel` and `defaultExtensions`, which rules out "discovery silently applied nothing to anyone" — the failure mode this row is about. All three of the row's Verify clauses are now asserted THROUGH discovery, with `defaultProvider`/`defaultThinking`/`fast` still covered by the seven `discovery::merge::tests` unit tests already confirmed red-without and at parity. Scope: one file touched (+114 lines and one `use cyrup_core::ModelId;`), no production code changed by this pass, nothing skipped or quarantined. No `CYRUP-DELTA` was added; the one in the diffed discovery files belongs to `SUBA-123` (`discovery/types.rs`, `subagent_only_extensions_from_default`) and correctly records a mechanism at full parity. Verify: `cyrup-ext-subagents tests::runtime_agent_registration_integration::settings_defaults_and_narrowed_overrides_reach_a_runtime_agent_through_discovery`; 19/19 for `-E 'test(runtime_agent)'`. (Caveat recorded for the next reader, NOT attributable to this row: a crate-wide run during this window transiently showed `discovery::agent_scan_and_exclude_dir_tests::a_malformed_default_subagent_only_extensions_aborts_discovery_with_upstreams_text` failing, which was a CONCURRENT lane's in-flight revert in `discovery/merge.rs`; it and its four siblings pass once that lane restored, so no single full-suite number from that window is reliable.)

**cyrup** — `discovery/mod.rs::run_discovery` appends runtime agents AFTER `merge::discover_and_merge`
has applied the settings (`:1826-1846`), and `discovery/runtime_registry.rs::merge_runtime_agents`'s
doc says so: they "never take part in the four-tier precedence merge and never receive settings
overrides" (`:1249-1256`).

**upstream** — `bbb30096` (#2369, v0.70.1). `applyRuntimeAgentSettings` (`src/agents/agents.ts:1619-1627`
@v0.71.0) applies `defaultModel`/`defaultProvider`/`defaultThinking` and a narrowed `agentOverrides`
(`runtimeAgentOverrides`, `:1597-1610`: `model`, `defaultProvider`, `fast`, `thinking` only).

**Impact** — A runtime agent silently runs on the parent session's model even when the user
configured a default model, provider or thinking level for subagents, or an override for that agent.

**Fix** — After `merge_runtime_agents`, apply the same default-model/provider/thinking passes and an
override pass restricted to those four fields.

**Verify** — With `subagents.defaultModel: "p/m"`, a runtime agent without a model launches on `p/m`;
`agentOverrides.<name>.thinking` reaches it; `agentOverrides.<name>.tools` does not.

## SUBA-122 — A canonical agent name does not win over a packaged short name

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: upstream `resolveAgentName` tries canonical `name` matches first — one hit wins, several go through `effectiveAgentMatch` and otherwise raise `Ambiguous agent name '<name>': …` — and only when NONE matched does it try `localName`, with its own `Ambiguous local agent name '<name>': …` (`src/agents/agents.ts:707-724` @v0.71.0, read at the tag; the alias pass follows at `:725`). Ported at `crates/cyrup-ext-subagents/src/discovery/mod.rs:578-613`: the single combined `a.name == raw || a.local_name == raw` filter is gone; `:595` is the canonical ambiguity message and `:600`/`:611` the local pass and its distinct message, with `:557-566` documenting the three passes and the specific case the old code got wrong. A user agent `scout` beside an installed package agent `code-analysis.scout` whose local name is `scout` is no longer ambiguous — the canonical pass resolves it — and `scout` still resolves to the package agent when it is the only one. Verify: `cyrup-ext-subagents discovery::tests::a_canonical_name_beats_another_agents_local_name_of_the_same_string` and `…::two_distinct_agents_sharing_a_local_name_are_a_local_ambiguity_error`, the second pinning that a real local collision raises the LOCAL message, not the canonical one.

**cyrup** — `discovery/mod.rs::resolve_agent_name` (`:568-591`) collects `a.name == raw || a.local_name == raw`
into one set; two hits with different names make `effective_agent_match` (`:517-533`) return `None`,
and the call fails with `Ambiguous agent name '<name>'`.

**upstream** — `src/agents/agents.ts:709-724` @v0.71.0 (v0.68.0, #2214): canonical `name` matches are
tried first; only if none match are `localName` matches tried, with their own
`Ambiguous local agent name` error.

**Impact** — A user agent `scout` beside an installed package agent `code-analysis.scout` makes
`scout` unusable by name.

**Fix** — Split the first pass into a canonical pass and a local-name pass, in that order, each with
its own ambiguity message.

**Verify** — Agents `scout` (user) and `code-analysis.scout` (package, local `scout`): `scout`
resolves to the user agent; with only the package agent, `scout` resolves to it.

## SUBA-123 — Three top-level subagent settings keys are silently dropped

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: **All three sub-parts of this row are closed together, because one change covers them:** `agentScanDirs` (SUBA-123a), `defaultSubagentOnlyExtensions` (SUBA-123b) and `agentExcludeDirs` (SUBA-123c). Upstream validates all three with the SAME rule and the same message shape — `if ("<key>" in subagentsObject)`, refused when `!Array.isArray(...) || some(item => typeof item !== "string" || !item.trim())`, then `.map(item => item.trim())` (`src/agents/agents.ts:1218-1241` @v0.71.0, read at the tag: `defaultSubagentOnlyExtensions` at `:1219-1225`, `agentScanDirs` at `:1226-1233`, `agentExcludeDirs` at `:1234-1241`) — with `agentScanDirs` adding discovery roots (`readConfiguredAgentScanDirs` `:2392`, `settingsAgentScanDirs` `:2948`), `agentExcludeDirs` removing trees including nested plugin sources and symlink aliases, and `defaultSubagentOnlyExtensions` filling `subagentOnlyExtensions` for agents that set none (`resolveSubagentDefaultSubagentOnlyExtensions` `:1392-1399`, `applySubagentDefaultSubagentOnlyExtensions` `:1401-1410`). cyrup: the three keys are typed on the settings struct (`crates/cyrup-ext-subagents/src/discovery/types.rs:955-974`, with `agent_scan_dirs` and `default_subagent_only_extensions` each carrying their own doc block rather than one orphaned above the other), validated by ONE shared loop rather than three near-copies (`discovery/mod.rs:1342-1366`, whose `:1345-1348` records why `defaultSubagentOnlyExtensions` joins it), trimmed at `:962-966` and `:955-958`, resolved per scope at `:1500-1501`, and applied through `discovery/agent_dirs.rs:186` (`is_excluded`, reached from `mod.rs:844-845` and `:1607`, and consulted at every scan tier `:221`/`:230`/`:251`/`:269`) and `:277` (`settings_agent_scan_dirs`). The row's fourth Fix clause is done too: `override_key_warnings` (`mod.rs:1283-1288`) now runs over the top-level `subagents` object, so the next unported key warns instead of vanishing — which is the half the file's own earlier lead got wrong. Three reverts were each proven red independently (drop the validator key → the malformed-shape test; `let _ = list;` for the trim → the trim test; drop `project_settings_path.is_some() &&` → the pathless-project-scope test), each re-checked with a post-run presence check because a concurrent lane was rewriting the same files. The one `CYRUP-DELTA` in the region is a mechanism note at full parity, verified at the tag rather than taken on faith: `withDeclaredExtensionPaths` (`agent-management.ts:238-248`) re-reads the agent file with `parseFrontmatter(fs.readFileSync(filePath))` and restores only the frontmatter literals, and neither it nor `readAgentFrontmatterFields` (`:330-337`, called from `editableAgentConfig` `:250`/`:299`/`:327`) consults the `agentFrontmatterFields` WeakMap that the merge pass maintains (`agents.ts:1406-1407`) — cyrup's `management::handlers` `editable_base` (`handlers.rs:146-148`) clears the list and the provenance flag and reaches the same answer without the file read. A configured exclusion now actually hides agents under it, configured scan dirs and default child-only extensions take effect, and a bad value aborts discovery with upstream's text. Verify: `cyrup-ext-subagents discovery::agent_scan_and_exclude_dir_tests::{agent_exclude_dirs_hides_an_agent_under_an_excluded_root,agent_exclude_dirs_hides_an_agent_reached_through_a_symlink_alias,agent_scan_dirs_adds_a_discovery_root,agent_scan_dirs_expands_one_wildcard_segment,a_two_wildcard_agent_scan_dir_entry_contributes_nothing,a_scan_dir_sits_after_the_extras_and_before_the_ordinary_user_dirs,a_non_array_agent_scan_dirs_aborts_discovery_with_upstreams_text,a_malformed_default_subagent_only_extensions_aborts_discovery_with_upstreams_text,default_subagent_only_extensions_entries_are_stored_trimmed}`, `discovery::merge::tests::{default_subagent_only_extensions_fills_only_agents_that_declared_none,a_project_default_subagent_only_extensions_beats_the_user_one,a_project_default_subagent_only_extensions_needs_a_project_settings_path,an_explicitly_empty_subagent_only_extensions_is_not_overwritten_by_the_default,a_default_supplied_subagent_only_extensions_is_not_written_back_by_editable_base}` and `discovery::tests::{an_unknown_top_level_subagents_key_warns,a_ported_top_level_subagents_key_does_not_warn}`; crate 4767/4767.

**Correction to this file's earlier lead.** The 2026-09-24 first pass said these keys "would reach
the key census as *unknown*, which gives a warning". That is false for top-level keys:
`override_key_warnings` (`discovery/mod.rs:1179-1203`) censuses only `agentOverrides.<name>`, and
`parse_subagent_settings` (`:837-856`) deserializes the rest with serde's tolerance.

**cyrup** — `agentScanDirs`, `agentExcludeDirs` and `defaultSubagentOnlyExtensions` have zero hits in
the crate; the settings struct ignores them without a word.

**upstream** — `src/agents/agents.ts:1219-1241` @v0.71.0 validates all three (arrays of non-empty
strings, a named error otherwise). `agentScanDirs` adds discovery roots (`readConfiguredAgentScanDirs`,
`:2392`; `settingsAgentScanDirs`, `:2948`); `agentExcludeDirs` (v0.68.0, #2131) removes trees from
discovery, including nested plugin sources and symlink aliases; `defaultSubagentOnlyExtensions`
(v0.70.0, #2284) fills `subagentOnlyExtensions` for agents that do not set it.

**Impact** — A configured exclusion is ignored, so agents the user excluded are still discovered and
can shadow or collide by name; configured scan dirs and default child-only extensions have no effect.
None of it is reported.

**Fix** — Parse and validate the three keys with upstream's messages; apply scan and exclude roots in
`scan_agent_tiers_scoped`, and default `subagent_only_extensions` after the existing
`default_extensions` pass. Separately, run the key census over the top-level `subagents` object so the
next unported key warns instead of vanishing.

**Verify** — An `agentExcludeDirs` entry hides an agent under it; a bad value aborts discovery with
upstream's text; an unknown top-level key produces a warning.

## SUBA-124 — `mcpDirectTools` cannot reach package or agent-plugin MCP servers

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mcp_direct_tools.rs::load_mcp_config` / `get_config_paths` (`:514-541`) read only
the mcp.json files and their imports. `resolve_direct_tool_names` (`:662-700`) skips any selection
whose server is not in that config, with no warning. The MCP adapter itself does load plugin servers
(`crates/cyrup-mcp/src/agent_plugin.rs`), so the servers exist at runtime and the direct-tool
resolver cannot name them.

**upstream** — `src/runs/shared/mcp-config-sources.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`):
`loadPackageMcpServers` (`:83-118`, a settings `packages` entry's `package.json` `pi.mcp` config
paths, servers named `<package>__<server>`) and `loadAgentPluginMcpServers` (`:120-146`,
`agentPluginPaths` → `plugin.json` + `mcp.json`, named `<plugin>__<server>`), merged in
`mcp-direct-tool-allowlist.ts:253-254`.

**Impact** — A child granted direct tools from a package- or plugin-provided MCP server gets none of
them, silently. The child then fails at the task or improvises with other tools.

**Fix** — Port both loaders (reuse `cyrup-mcp`'s plugin reader where the shapes agree) and merge
their servers under upstream's names before direct-tool resolution. Coordinate with `13c`, which owns
the adapter's config sources.

**Verify** — A plugin at an `agentPluginPaths` entry with `mcp.json` server `db`, and a metadata cache
for `<plugin>__db`: `mcpDirectTools: ["<plugin>__db"]` resolves its tools.

## SUBA-125 — Model-hidden skills are still injected into children

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: upstream filters at the injection chokepoint — `const visibleSkills = skills.filter((skill) => !skill.disableModelInvocation);` with `if (visibleSkills.length === 0) return "";` (`src/agents/skills.ts:712-713` @v0.71.0, read at the tag, under the comment that this is *"the chokepoint every prompt-injection path funnels through, so filtering here closes the leak regardless of which caller assembled the list"*) — and `proactive-skills.ts:146` stops suggesting them. cyrup already had the flag on `cyrup_resources::Skill` (`crates/cyrup-resources/src/skill.rs:38`) but nothing in this crate read it; it is now carried through `crates/cyrup-ext-subagents/src/discovery/skills.rs:78` and `:96-100` (on both the discovered and resolved shapes), populated at `:158` and `:274`, and filtered at the injection chokepoint at `:302` (`.filter(|skill| !skill.disable_model_invocation)`) — upstream's placement, so a skill a named-skill path resolved is still dropped. The proactive recommender skips them at `:571`. A skill its author marked `disable-model-invocation: true` no longer reaches a child's prompt when an agent lists it, so the declared restriction no longer fails open. Verify: `cyrup-ext-subagents discovery::skills::tests::{an_agent_naming_a_hidden_skill_launches_without_it_in_the_child_prompt,build_skill_injection_omits_a_hidden_skill,a_hidden_skill_is_not_proactively_recommended}` — the first covering the row's Verify clause that a normal skill beside the hidden one is still injected.

**cyrup** — `discovery/skills.rs::resolve_skills_in` (`:156-190`) resolves each named skill from
`discover_skills` (`:122`), whose `cyrup_resources::Skill` already carries `disable_model_invocation`
(`crates/cyrup-resources/src/skill.rs:38`). Nothing in `cyrup-ext-subagents` reads the flag, so a
hidden skill named by an agent is resolved and injected.

**upstream** — `a1eaa728` (#2400, v0.71.0). `src/agents/skills.ts:712` @v0.71.0 filters
`visibleSkills = skills.filter((skill) => !skill.disableModelInvocation)` at the injection chokepoint
(so a requested hidden skill is dropped), and `proactive-skills.ts:146` stops suggesting them.

**Impact** — A skill its author marked `disable-model-invocation: true` (a user-only command, say)
reaches a child's prompt whenever an agent lists it. That is a declared restriction failing open.

**Fix** — Drop `disable_model_invocation` skills in `resolve_skills_in` (report them as missing, as
upstream's chokepoint does) and in the proactive-skill recommender.

**Verify** — An agent listing a hidden skill launches without it in the child prompt; a normal skill
beside it is injected.

## SUBA-126 — Structured-only answers are delivered and saved empty

> **CLOSED 2026-09-28.** (on `claude/lows-next`) A structured-only answer is saved, delivered and chained as the pretty-printed value; blank is measured after `strip_acceptance_report`, and acceptance still reads the raw prose (`exec/mod.rs:739-790`). Tests: `a_structured_only_answer_is_saved_and_delivered_as_pretty_json`, `prose_that_is_only_an_acceptance_report_still_delivers_the_structured_value`. The line numbers in the `cyrup` paragraph below describe the pre-fix code. Evidence is in the row.

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mod.rs::resolve_saved_output` (`:1520-1560`, called at `:706`) persists the captured
prose, empty or not, BEFORE the structured value is read (`apply_structured_output`, `:723`), and
`assemble_delivered_output` never substitutes the value for empty prose.

**upstream** — foreground since v0.41.0: `src/runs/foreground/execution.ts:1515` @v0.71.0,
`if (!fullOutput.trim() && result.structuredOutput !== undefined) fullOutput = JSON.stringify(result.structuredOutput, null, 2);`;
background since v0.68.0 (`a525db4d`, #2163): `subagent-runner.ts:1373-1374`.

**Impact** — A child that finishes with only a `structured_output` call leaves an empty output file
and an empty delivered output (and an empty `{previous}` in a chain), although the answer exists.

**Fix** — Read the structured value first, and when the prose is blank substitute
`serde_json::to_string_pretty(value)` before `resolve_saved_output` and assembly.

**Verify** — A structured-only child with `output: "out.json"` writes the pretty JSON there and
returns it.

## SUBA-127 — Structured-output evidence is read only on a clean exit

> **CLOSED 2026-09-28.** (on `claude/lows-next`) The capture is read whenever `structured_output` was invoked, a valid value survives a failed or timed-out run, and a rejected call fails with the ported rejection summary (`exec/structured.rs:401,526`; `exec/mod.rs:1716-1777`). Tests: `a_rejected_structured_output_call_fails_with_the_rejection_summary`, `a_valid_structured_value_survives_a_later_failure`, `a_child_that_never_invokes_structured_output_gets_the_missing_call_error`. An interrupted run is still not read (upstream's runner reads it). Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mod.rs::apply_structured_output_value` reads the capture file only when
`self.gate().is_clean()` (`:1633`), and `exec/structured.rs::read_structured_output` (`:354-366`) maps
a missing file to `STRUCTURED_OUTPUT_MISSING_ERROR` whether or not the child called the tool.

**upstream** — v0.71.0 (#2407, #2411; `b9a0ce83`, `789c6856`): `execution.ts:1449-1471` reads the
value whenever the tool was invoked and keeps it as evidence even when the run later failed; a
rejected call gets `formatStructuredOutputRejectionError` (`structured-output.ts:62-80`, a bounded,
redacted summary of the latest failed `structured_output` result) instead of the missing-call error;
the runner mirrors this (`subagent-runner.ts:1248-1265`) and stops clearing the value on timeout or
stop.

**Impact** — A child whose output was rejected is told it never called the tool, which misdirects the
fix; a valid value produced before a provider error is lost.

**Fix** — Read the capture whenever the tool was invoked; set the missing-call error only when it was
not; port the rejection summary; keep the value on a failed run.

**Verify** — A child that calls `structured_output` with an invalid value fails with the validation
summary; a child that produced a valid value and then hit a provider error keeps `structured_output`.

## SUBA-128 — `checkpointBeforeDeadlineMs` is unported

> **CLOSED 2026-09-28.** (on `claude/lows-next`) Tool parameter, config key, async-single `params ?? config` resolution and the runner's `deadline-checkpoint` steer are all ported; the bounds refusal sits at the tool entry for every call shape (`extension/tool/mod.rs:254-264`, `background/runner_main/control_watcher.rs:630`). Test: `a_running_child_receives_the_deadline_checkpoint_and_hands_off_before_the_kill`, plus the runner, launch and config tests in the row. An invalid config still warns and defaults where upstream throws. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

Escalated from `SUBA-113`.

**cyrup** — only `registration/mod.rs:541` (`UNPORTED_CONFIG_KEYS`); no tool parameter, no runner
timer.

**upstream** — v0.68.0 (#2141): tool param `checkpointBeforeDeadlineMs` (`src/extension/schemas.ts:363`
@v0.71.0), config default validated at `extension/config.ts:145-151`, resolved at
`subagent-executor.ts:3460`; the runner arms a timer `remainingMs - checkpointBeforeDeadlineMs`
(≥ 1 s) and delivers a `source: "deadline-checkpoint"` steer asking the child to finish the current
tool call and reply with a handoff (`subagent-runner.ts:3383-3406`).

**Impact** — A long async run with a deadline is killed with no chance to report changed files and
remaining work. The config key warns; the per-call parameter is simply not advertised.

**Fix** — Add the parameter and config key; in the detached runner, arm the timer and route the steer
through the existing steer inbox with upstream's text.

**Verify** — A run with `timeoutMs: 10000, checkpointBeforeDeadlineMs: 5000` receives the checkpoint
steer about five seconds before the timeout.

## SUBA-129 — Typed gates and `preserveStagedIndex` are refused

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `workflows/scripted/engine.rs:1506-1511` accepts `gate` only as a non-empty string;
`exec/acceptance/model/types.rs::AcceptanceVerifyCommand` (`:177-191`) has no `output`/`schema`; and
`validate_input.rs:71-82` rejects `preserveStagedIndex` and `verify[].output` as unknown keys.

**upstream** — v0.69.0: `36eb7660` (typed gates, #2315): `parseGateInput`/`normalizeGateAcceptance`
(`src/runs/shared/acceptance.ts:172-236` @v0.71.0), the stdout contract (`:1259-1290`, empty,
truncated, non-JSON or schema-invalid output fails the gate), never memoized, conflict with
`outputSchema` refused (`TYPED_VERIFY_OUTPUT_SCHEMA_CONFLICT`, `:212`), and a passing value becomes
`structuredOutput` (`subagent-runner.ts:1458-1463`, `execution.ts:1998-2003`). `c29a1dcf` (#2280/#2286):
`preserveStagedIndex` (checked/verified only, `:282-287`) captures the launch index tree and checks
it unchanged instead of `no-staged-files` (`:1111-1148,1600-1602`).

**Impact** — Workflow scripts and calls written for current upstream fail validation. Nothing is
silently wrong; the features are absent.

**Fix** — Extend the verify command with `output: "json"` and `schema`, port the stdout contract and
the conflict rule, bridge the value into `structured_output`; port the staged-index baseline.

**Verify** — `gate: { command: "echo '{\"ok\":true}'", output: "json" }` yields
`structuredOutput: {"ok": true}`; invalid JSON fails the gate.

## SUBA-130 — Worktree git commands are unbounded

> **CLOSED 2026-09-30** (on `claude/lows-batch4`): worktree allocation, the setup hook, rollback, diff capture and cleanup now run through `spawn/bounded_argv.rs::run_bounded_argv` under a `GitBounds` (stop + deadline + output cap). Two `[CYRUP-DELTA]`s: diff/cleanup are bounded although upstream's are not, and rollback has a 10 s deadline floor. See the table row for the cites, the tests and the remainder (`SUBA-147`, `SUBA-148`). The `Fix`/`Verify` below were written before the port: the "fake `git` on `PATH`" verify was replaced by a `core.fsmonitor` hook that really hangs git.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `spawn/worktree.rs::run_git_env` (`:265-281`) is `Command::output().await` with no
deadline, no stop signal, no output cap and no process-group kill. Every worktree create / diff /
cleanup git call goes through it. (The setup HOOK is bounded, `:734-822`, `SUBA-027`.)

**upstream** — `src/runs/shared/worktree-setup-command.ts` (new in `v0.57.0..v0.67.0`): one runner
with `signal`, `deadlineAt`, `maxBuffer` and an owned process-tree controller. At v0.71.0 worktree
setup is a transaction whose every git command honours the run's signal and deadline
(`worktree.ts:239-252`), compensation keeps only the original deadline (`:1305-1325`), and
`preflightWorktreeSource` bounds the source probe (`:359-368`).

**Impact** — A git call that hangs (an index lock, a credential helper waiting on a terminal, a slow
network filesystem) hangs worktree setup past the run's deadline, and `stop` cannot end it. This is
the third instance of the unbounded-spawn class after `SUBA-069` and `SUBA-095`.

**Fix** — Give `run_git_env` a deadline and a cancellation token, spawn in its own process group and
kill it on expiry through `spawn::signal::terminate_on_timeout`, and cap captured output.

**Verify** — A fake `git` on `PATH` that sleeps forever makes worktree creation fail at the run
deadline, and `stop` ends it promptly.

## SUBA-131 — An idle external-CLI step is never flagged

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — idle detection lives only in the native attempt driver (`exec/drive_attempt.rs:807`,
`ControlMonitor::update_activity_state`); `exec/external_cli/run.rs::run_external_cli_process`
(`:159-330`) feeds no activity to any monitor, so an external-CLI step never gets `needs_attention`.

**upstream** — `c7938169` (#2167, v0.68.0), `subagent-runner.ts` @v0.71.0: external steps count as one
turn for idle derivation (`:3138`), stdout/stderr chunks refresh activity (`onExternalStreamActivity`,
`:914-918`), and near the attention boundary a coalesced, cancellable `git` fingerprint probe
(`readGitFingerprint`, `:626-649`; `:3143-3166`) credits a changed worktree as activity; a probe
failure fails the step `PROCESS_TREE_UNVERIFIED`.

**Impact** — A stalled claude/codex/cursor child gives no attention notice at all; the user finds out
at the deadline.

**Fix** — Give the external runner a `ControlMonitor`, note activity on every chunk, and port the
throttled fingerprint probe for the attention boundary.

**Verify** — An external script that sleeps with no output past `needsAttentionAfterMs` raises
`needs_attention`; one that writes to stderr periodically does not.

## SUBA-132 — Tool-budget blocks never reach the result

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `SingleResult::tool_budget_blocked` is set from the child's own budget-block message and reaches `WorkflowBudgetSignals::from_single_result` across the runner's `StepResult` waist (`exec/tool_budget.rs:282`, `exec/mod.rs:799-810`, `workflows/settlement.rs:462`). Test: `a_tool_budget_block_reaches_the_result_and_settles_budget_exhausted`. The `workflows/settlement.rs:444-467` quote in the `cyrup` paragraph below is the pre-fix text. Neighbouring gaps are listed in the row. Evidence is in the row.

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `workflows/settlement.rs:444-467` says it in source: "`tool_budget_blocked` has NO carrier
on `SingleResult` yet … always `false`". The block itself happens (`prompt_runtime.rs:2678`).

**upstream** — `result.toolBudgetBlocked` since v0.43.0 (`execution.ts`); at v0.70.0 (#2302) the
foreground matches the blocked message by the result event's own tool name and records a count above
`hard` (`execution.ts:1144-1156` @v0.71.0), so `workflow-settlement.ts` settles `budget_exhausted`
and the delegation adapter reports `tool_budget_exhausted` (`slash/delegation-adapters.ts:364`).

**Impact** — A workflow child stopped by its tool budget settles as an ordinary failure or success,
not `budget_exhausted`, so a script cannot branch on it.

**Fix** — Carry `tool_budget_blocked` on `SingleResult` from the attempt driver (match upstream's
`isToolBudgetBlockedMessage`) and read it in `WorkflowBudgetSignals::from_single_result`.

**Verify** — A workflow child with `toolBudget.hard: 1` that tries a second tool settles
`budget_exhausted`.

## SUBA-133 — Agent advertisement to the parent prompt is unported

> **CLOSED 2026-09-28** (on `claude/lows-next`). `advertise` is parsed strictly, editable through management and written back; `discovery/advertised.rs` builds upstream's bounded, XML-escaped `<advertised_subagents>` block, and `extension/host/native_impl.rs:810-838` puts it on the parent system prompt at `before_agent_start` while `subagent` is selected, replacing the previous block each turn. Evidence and the one remaining ordering delta (`locale_order` is not ICU root collation) are on the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — no `advertise` frontmatter field (the key round-trips into `extra_fields`) and no
`<advertised_subagents>` block anywhere.

**upstream** — `src/agents/advertised-agent-prompt.ts:28-60` @v0.71.0 (new in `v0.57.0..v0.67.0`):
file-defined agents with `advertise: true` (`agents.ts:2107-2110`), not disabled, not runtime, and
allowed by the capability ceiling are listed (≤ 16 agents, ≤ 12 KiB, descriptions ≤ 512 bytes,
XML-escaped) in the parent's system prompt at `before_agent_start` (`extension/index.ts:535,825-827`).

**Impact** — Opt-in: without it the parent model never sees agents their authors chose to advertise.

**Fix** — Parse `advertise`, build the block with upstream's bounds, append it in the host's
system-prompt hook, and refresh it when discovery changes.

**Verify** — One advertised agent appears in the parent system prompt; a disabled or ceiling-denied
one does not.

## SUBA-134 — Child sessions get no human-readable name

> **CLOSED 2026-09-30** (on `claude/lows-batch3`). The nested exact-status view is ported and names a nested run by its typed `session_name`, and run-status renders the external-CLI runner block. See the `## Open items` row for the residual nested-relay reach.
>
> **PARTIALLY CLOSED 2026-09-30** (on `claude/lows-batch3`). The intercom claim is now a typed, synchronous request on a typed native bus; the label arm, attached-root and chain-append naming, the launcher-assigned branch and the Fleet/run-status/foreground displays landed. Only the nested-run display name remains (no nested exact-status view exists to name). See the `## Open items` row.
>
> **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`). The base naming is ported: derivation (`exec/child_session_name.rs`), the env hand-off (`exec/attempt_runner.rs:541-555`), the child's `set_session_name` at `before_agent_start` (`prompt_runtime.rs:2719-2733`), and `sessionName` on results and single/parallel pending steps. **Still open:** the workflow-node / chain-step / dynamic-task label arm (every caller passes `label: None`), DynamicGroup/ImportAsyncRoot pending statuses, chain-append naming, the launcher-assigned branch that has no production producer, and Fleet/run-status preferring the name. See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `derive_child_session_name` / `child_session_name` have zero hits; a child's session is
never named, and results carry no `sessionName`. (The `session_name` hits in `exec/spawn_plan.rs` are
intercom routing env, `spawn/intercom_target.rs:61-65`.)

**upstream** — `src/shared/child-session-name.ts:27` @v0.71.0 (v0.57.0..v0.67.0, `5ed2a4d4`):
derived from agent name + task or workflow node label, applied with `pi.setSessionName` in the child,
carried as `sessionName` in progress, status and results; v0.71.0 (`55b79828`, #2433) keeps it while
the intercom bridge is active by claiming the route through `intercom:session-identity`.

**Impact** — Child sessions are indistinguishable in `/resume` and session lists.

**Fix** — Port the derivation, pass it to the child (a `--session-name` flag or env the child applies
at start), and thread `session_name` into status and results. The intercom half overlaps area 11.

**Verify** — A `worker` child for "fix auth refresh" lists as `worker: fix auth refresh`.

## SUBA-135 — No launch-cwd preflight

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `preflightLaunchCwd` is ported (`exec/launch_cwd.rs`) and runs at the tool entry ahead of the mission binding, at the head of `run_sync`, and in `spawn_background_steps` before any run exists. Tests: `a_missing_single_launch_cwd_is_refused_before_launch_with_its_typed_spelling`, `an_async_launch_into_a_missing_cwd_is_refused_before_any_run_exists`. **Ledger correction:** the Impact below understated it: the mission binding created the typo'd cwd and the child ran silently in it. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (cyrup side read; the failure text was not reproduced)

**cyrup** — `extension/tool/routing.rs::resolve_requested_cwd` (`:367-372`) joins the path and never
checks it; a missing cwd surfaces as the OS error from `Command::current_dir` at spawn (in the
detached runner, after a receipt was returned).

**upstream** — `src/runs/shared/launch-cwd.ts:3-16` @v0.71.0: "Subagent launch aborted: cwd is not a
directory: <p>" / "… cwd does not exist: <p>", suffixed `(resolved from "<requested>")` when the
typed cwd was rewritten; applied in both the foreground and the runner before launch.

**Impact** — A typo'd `cwd` produces an unhelpful OS error, and for async runs only after a receipt.

**Fix** — Port `preflightLaunchCwd` and call it before any run state is written.

**Verify** — `cwd: "nope"` fails before launch with upstream's text naming the resolved path.

## SUBA-136 — Supervisor requests have no renderer and replies leave no entry

> **CLOSED 2026-09-28** (on `claude/lows-next`). `tui/supervisor_ui.rs` ports `supervisor-ui.ts` with its bounds and sanitizer, both renderers are registered (`extension/host/native_impl.rs:213-221`), surfaced requests carry upstream's details, and each reply appends one `subagent_supervisor_reply` entry (`native_supervisor.rs:1117-1140`). See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `native_supervisor.rs:1179` injects requests under `subagent_supervisor_request`, but no
renderer is registered for it (the host registers only the watchdog renderer,
`extension/host/native_impl.rs:308`), and `subagent_supervisor_reply` has zero hits.

**upstream** — `src/intercom/supervisor-ui.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`): a bounded,
sanitized renderer for requests (reason header, body ≤ 8 000 chars, interview ≤ 4 000, reply hint)
registered at `extension/index.ts:652`, and a `subagent_supervisor_reply` session entry recording
each reply (`:6`, `SupervisorReplyEntryData`).

**Impact** — Requests show as raw message content, and the session keeps no record of what the parent
answered. Per README blind-spot 3, the drawing is substrate but the bounds, sanitization and entry
contract are portable.

**Fix** — Register a renderer for the request type with upstream's bounds, and append the reply entry
when `subagent_supervisor` replies.

**Verify** — A request renders with its reason header and truncation marker; a reply appends one
`subagent_supervisor_reply` entry.

## SUBA-137 — The Ghostty inspector misfires inside Ghostty embedders

> **CLOSED 2026-09-28** (on `claude/lows-next`). `available()` also requires `__CFBundleIdentifier` (trimmed) to be `com.mitchellh.ghostty` (`inspectors/ghostty/plugin.rs:134-142`). Verify: `inspectors::ghostty::plugin::tests::available_requires_the_standalone_ghostty_bundle_id`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `inspectors/ghostty/plugin.rs:96-100` gates on `TERM_PROGRAM` (case-insensitive
`ghostty`) and macOS only.

**upstream** — v0.69.0: `src/inspectors/ghostty/plugin.ts:5,20-24` @v0.71.0 also requires
`__CFBundleIdentifier === "com.mitchellh.ghostty"`; embedders such as cmux set `TERM_PROGRAM=ghostty`
too and previously got AppleScript `-1728`/`-2741` errors or the wrong window.

**Impact** — macOS users in a Ghostty-embedding terminal get failed or misdirected inspector opens.

**Fix** — Add the bundle-id check to the availability gate.

**Verify** — `TERM_PROGRAM=ghostty` without the bundle id is unavailable; with it, available.

## SUBA-138 — The RPC `cost` method is unported

> **CLOSED 2026-09-28** (on `claude/lows-next`). `cost` is the ninth RPC method and `ping` advertises `capabilities.cost.version = 1`; it and `/subagent-cost` share a port of v0.71.0 `collectSubagentCost` (`registration/cost.rs:995`). **Ledger correction:** the **cyrup** paragraph below implied `/subagent-cost` already computed upstream's data; it was an older transcript walk. The dead R-SA-140 accumulator in `registration/cost.rs` is still to delete (see the row).

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `extension/rpc/mod.rs:108-118`, `SUBAGENT_RPC_METHODS` has eight methods, no `cost`, and
`ping.rs` advertises no cost capability. `/subagent-cost` exists (`registration/cost.rs`).

**upstream** — `858661af` (#2378, v0.71.0): `src/extension/rpc.ts:35,770` @v0.71.0 adds `cost`,
returning `{ version: 1, parent, children, childTotal, total, unresolvedAsyncChildren }`, and
`ping.capabilities.cost = { version: 1 }`.

**Impact** — Other extensions must scrape `/subagent-cost` text to read spend.

**Fix** — Add the method over the data `/subagent-cost` already computes, and advertise it.

**Verify** — An RPC `cost` request returns the versioned object; `ping` lists `cost`.

## SUBA-139 — No lazy `subagents_enable` loader

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — the `subagent` tool is registered unconditionally; `subagents_enable` has zero hits.

**upstream** — `5b679875` (#2380, v0.71.0): `src/extension/tool-activation.ts` registers a small
`subagents_enable` loader and activates the full tool through `setActiveTools` on demand; hosts
without dynamic tools (`unsupportedDynamicToolsReason`, `:22-27`) keep the always-loaded tool, which
is what cyrup does today.

**Impact** — Every prompt carries the full `subagent` tool schema; upstream keeps unrelated prompts
smaller. Cost only.

**Fix** — Needs a working late-activation path in the host (see `MCP-037a`), then the loader.

**Verify** — A fresh session's first request carries `subagents_enable` and not `subagent`; calling
the loader activates `subagent` for the next turn.

## SUBA-140 — No child-only prompt-cache retention

> **CLOSED 2026-09-28** (on `claude/lows-next`). A non-empty `CYRUP_SUBAGENT_CACHE_RETENTION` becomes the child's `CYRUP_CACHE_RETENTION` (`exec/spawn_plan.rs:594-603`). Verify: `exec::spawn_plan::tests::the_child_only_cache_retention_reaches_the_child_env`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `PI_SUBAGENT_CACHE_RETENTION` and a cyrup-named equivalent have zero hits in the crate;
children inherit the parent's retention.

**upstream** — `ce3cff20` (#2190, v0.68.0): `src/shared/child-cache-retention.ts:22` @v0.71.0 maps
the variable onto the child's `PI_CACHE_RETENTION` (spawned children) or a per-request pin
(in-process children).

**Impact** — A parent on the 1-hour tier writes its short-lived children at the same, more expensive
tier. Cost only.

**Fix** — Read `CYRUP_SUBAGENT_CACHE_RETENTION` and set the child's retention env in `spawn_plan`.

**Verify** — With the variable set, a child's spawn env carries the requested tier.

## SUBA-141 — Observability drift: external-CLI logs and runner exit status

> **CLOSED 2026-09-30** (on `claude/lows-batch3`). The Fleet half landed: a typed live process hook from the external CLI runner publishes `StepStatus.external_process` into `status.json`, the logs live in the run dir, and Fleet shows `external-cli · <elapsed>` plus the stderr/stdout tails; the stderr bound counts UTF-16 units. See the `## Open items` row.
>
> **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`). The runner-exit half is ported: the launcher observes the detached runner's close, and the reconciler reports `exited with code N (signal S)` with upstream's stderr-tail bounds (`background/reconcile.rs:664-741`). **Still open:** the Fleet external-CLI stdout/stderr tails and step elapsed time, blocked because cyrup's `StepStatus` has no `externalProcess` and `exec/external_cli` publishes no live process hook. See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `background/fleet_view.rs:1088-1106` offers only "Transcript tail"; an external-CLI
step's stdout/stderr logs are not shown. `background/reconcile.rs::synthesize_failure` (`:727-732`)
reports "reconciled as stale/dead: <reason>" plus the runner stderr tail, without the runner's exit
code or signal.

**upstream** — `be6e0ca6` (#2375, v0.71.0): `fleet-view.ts:584-605` @v0.71.0 shows
`External stderr tail` / `External stdout tail` (first while the step runs) and elapsed time on the
step row. `2e280687` (#2427, v0.71.0): `stale-run-reconciler.ts:235-238` reports
`exited with code N (signal S)` from the recorded runner exit.

**Impact** — A failing external agent's output has to be found by hand; a dead runner's diagnosis
lacks its exit status.

**Fix** — Add the two log sources to the Fleet tail; record the runner exit where the launching
process observes it and include it in the synthesized failure.

**Verify** — Fleet on a running external step shows its stderr tail; a runner killed with SIGKILL
reconciles with `signal SIGKILL` in the message.

## SUBA-143 — Runtime agents can be registered only through the native Rust API

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

Promoted from `09a`'s residual-lead list (the `SUBA-084` closure).

**cyrup** — runtime registration exists only as native methods (`extension/host/mod.rs:432`,
`extension/executor/mod.rs:396` `register_agent`); `runtime-agent-register` has zero hits, and the bus
cannot carry upstream's contract as written: `crates/cyrup-ext/src/bus.rs:80-93` QUEUES emits
(`[CYRUP-DELTA]`, to avoid re-entering a WASM guest's store) and passes payloads by value.

**upstream** — `src/agents/runtime-agent-events.ts:4-70` @v0.71.0 (v0.64.0): another extension emits
`pi-subagents:runtime-agent-register:v1` with `{ version: 1, name, definition }`; the owner's listener
registers the agent synchronously and writes `request.result` (`{ ok, registration }` or
`{ ok: false, error }`) into the same object; `registerAgentViaEvents` reads it back. Re-exported at
`src/api/agents.ts:3-10`; the listener is installed at `extension/index.ts:25`.

**Impact** — A guest (WASM) extension cannot contribute agents at runtime, which upstream extensions
do through this event; cyrup can host only built-in native registrants.

**Fix** — Design a request/response bus topic (a reply topic keyed by request id, as the RPC bridge in
`extension/rpc/` already does) that carries the same request and result shapes, and route it to
`register_agent`. Registration disposal needs a matching topic.

**Verify** — A guest extension registers an agent through the topic, sees `{ ok: true }`, and the
agent appears in discovery; a malformed definition returns `{ ok: false }` with upstream's message.

## SUBA-142 — tracker: in-process child sessions

**Kind** tracker · **Severity** tracker · **Effort** — · **Confidence** confirmed (both sides read)

Promoted from `09a`'s census lead. At v0.71.0 upstream builds every native child in-process
(`src/runs/shared/child-launch.ts::buildInProcessChildLaunch`, `src/runs/shared/child-session.ts`),
over a shared model runtime; cyrup spawns a `cyrup` process per child (`spawn/mod.rs`,
`exec/spawn_plan.rs`), and no `[CYRUP-DELTA]` records that as a decision. This is a scope question,
not schedulable work, so it is a tracker.

Features upstream built on the in-process mechanism and that this pass therefore did not file as
items: usage reconciliation from the session's own messages (`usage-reconciliation.ts`, #2295), the
parent-provider-registry inheritance and its model-not-found diagnostic (#2276,
`model-resolution-diagnostic.ts`), the per-request cache-retention pin, and the Pi-0.86/0.87 SDK
ownership fixes (`child-session.ts`). `SUBA-110`'s scrub applies only where upstream still spawns
(the runner and external CLIs), and that remains correct.

**Escalates to an item** when a decision of record says cyrup follows upstream, or when one of the
parked features is shown to change a result for a spawned cyrup child.

> **FOLD-IN 2026-10-03 (v0.75.0):** `07946874`/#2636 and `1fe508f1`/#2638 make children launch and verify on extension-registered virtual models (`pi-virtual` api); the child's `virtualModelId` (`src/runs/shared/child-session.ts:118,589` @v0.75.0) replaces the router's physical model in `formatSubagentModelVerificationError` (`run-child-session.ts:517`, `foreground/execution.ts:1114`). In-process only, so parked here, no row. When `SESS-067` (area 03, the session half of virtual models) lands, `exec/model_verification.rs` must compare the child's selected model rather than the router's physical model, or a correct child fails `model_verification_failed`.

---

## Findings filed 2026-10-02 — the `v0.71.0..v0.74.0` window

`pi-subagents` **v0.74.0** (88 commits since v0.71.0), read only through `git -C tmp/pi-subagents
show v0.74.0:<path>` and `git diff v0.71.0..v0.74.0`; cyrup read at `fe875569`. This file's window
is therefore now `v0.57.0..v0.74.0`, and its title's `v0.64` is the tag the first drift pass opened
against, not the current one.

## SUBA-150 — `workflow: true` reply-fenced scripts; `workflowScript` / `workflowScriptPath` are gone from the tool

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

**upstream** — `0538e14d` (#2588, `feat(workflows)!`, v0.74.0) is a declared breaking change to the
tool surface. The `subagent` tool now has exactly one workflow field
(`src/extension/schemas.ts:221` @v0.74.0):

- `workflow: true` runs the single ` ```js workflow ` fenced block in the **same assistant reply**
  that issued the tool call. `readReplyWorkflowScript`
  (`src/extension/reply-workflow-script.ts:14`) walks `sessionManager.getBranch()` backwards for the
  assistant message carrying this `toolCallId` and reads the script out of its text blocks. Pi
  persists the whole assistant message before running its tool calls, which is what makes this
  work.
- a string containing `/` or `\` is a script **file** read from the request cwd.
- any other string is a named workflow **resource**.

`workflowScript` and `workflowScriptPath` were **removed from the tool**; RPC spawn takes inline
text as `script` (`src/extension/rpc.ts`). The fence scanner (`workflowBlocks`,
`reply-workflow-script.ts:41`) skips the bodies of other fences, accepts ``` and `~~~` markers of
three or more characters, and has four distinct refusals: no `workflow: true` outside a model tool
call, more than one `workflow: true` call in one reply, an unclosed block, and a block count that is
not exactly one. `src/extension/public-execution.ts` and `src/runs/background/scheduled-runs.ts`
were rewritten around the single field; `workflowScript` survives only as the package's internal
carrier (`disabled-features.ts:106-110` names it as such).

**cyrup** — `extension/tool/schema.rs:345` advertises `workflowScript` with its own long description,
and `extension/tool/mod.rs:190` records that `SubagentToolParams` carries "neither
`workflowScriptPath`, nor `workflow`, nor the marker key". The whole scripted-workflow engine is
built around the name: `workflows/scripted/engine.rs:259` ("The workflowScript body"), `:1505`
(`params.contains_key("workflowScript")`), `:2160` (`NESTED_WORKFLOW_REFUSAL`, whose text says
`workflowScript cannot be started from inside a workflow child.`), `:2184`. Zero hits in `crates/`
for `js workflow`, `reply_workflow` or `readReplyWorkflowScript`, and zero in
`docs/gap-analysis/` — this is not filed anywhere.

**Impact** — Two things, and the second is the reason this is `medium` rather than `low`. (1) A model
trained on or prompted from pi's current tool reference will call `workflow: true` and cyrup will
reject it as an unknown parameter with no hint. (2) The reason upstream made the change is that a
script passed as a JSON string has every quote and newline escaped, which is a real authoring tax on
the model and a real source of malformed scripts; cyrup has that tax today.

**Fix** — This is `L` and should be split. (a) Add `workflow: true | string` and keep
`workflowScript` as the internal carrier, exactly as upstream does, so the engine's internals and
`NESTED_WORKFLOW_REFUSAL` need no rename. (b) Port `workflowBlocks` and `scriptFromReply` as a pure
function over the branch's assistant content, with upstream's four refusal strings verbatim — the
fence grammar is the load-bearing part and is pure, so it tests without a session. (c) Decide
whether to remove `workflowScript`/`workflowScriptPath` from the advertised schema. Removing them is
a breaking change to cyrup's own tool; keeping them is an advertise-vs-pi divergence. Either way the
decision belongs in a `[CYRUP-DELTA]`, because this crate's rule is that the schema and the dispatch
agree, and both would still dispatch.

**Verify** — A reply containing one ` ```js workflow ` block and a `subagent({ workflow: true })`
call runs that script; a reply with two such calls, with two blocks, with an unclosed block, and a
non-model caller each get upstream's distinct message; a ` ```js ` block without the ` workflow `
tag is ignored; a `~~~~`-fenced block closes only on four or more `~`.

> **FOLD-IN 2026-10-03 (v0.75.0):** (a) `df3b6df1`/#2610 makes the string `"true"` equal `workflow: true` for MCP clients that stringify booleans (`src/extension/reply-workflow-script.ts:29`, `rpc.ts:527`, `index.ts:744` @v0.75.0); it is part of the `workflow` field this row ports. (b) `cfb6f9a8`/#2611: `action: "validate"` now checks workflow `args` against the same limits as a launch, exports `MAX_ARGS_FIELDS/ITEMS/DEPTH/BYTES` and states them in the `args` schema description (`src/extension/schemas.ts:7,227` @v0.75.0). Cyrup already enforces 16 fields, 64 items, depth 8 and 16 KiB in `workflows/resources.rs` (`validate_plain_json` and `normalize_args`, `:385-445`; they are literals, only `MAX_ARGS_BYTES` is a private const at `:29`), but its `validate` arm (`extension/tool/routing.rs:1870`) takes only `workflowScript` and never runs `normalize_args`, and `args` is advertised and plumbed only for scheduled runs (`extension/tool/schema.rs:742`, `background/scheduled_runs/tool.rs:355`). Port: export the four limits as constants, state them in the `args` description, and run `normalize_args` in `validate`, reporting its error beside the script errors.

## SUBA-151 — `chain`, `tasks` and the dynamic-fanout schema left the default tool; they are now lowered into package-owned workflow scripts

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

**upstream** — `71d042f0` (#2596, v0.74.0) finished what #2588 started. At v0.71.0
`SubagentParamProperties` carried `chain` (built from `ChainItem`), `tasks`, `ParallelTaskSchema`,
`DynamicExpandSchema`, `DynamicParallelTemplateSchema`, `DynamicCollectSchema`, `OutputOverride`,
`ReadsOverride` and `ChainGateOverride`. **At v0.74.0 every one of those is deleted**
(`git -C tmp/pi-subagents diff v0.71.0..v0.74.0 -- src/extension/schemas.ts`), and
`git -C tmp/pi-subagents show v0.74.0:src/extension/schemas.ts | grep -n 'chain'` finds `chain` only
inside `StructuredWorkflowProperties` (`:286`) and `createSubagentParamsSchema` (`:299,305,308`).

The data-shaped surface now exists in exactly one place: when `disabledFeatures` lists
`workflow-scripts`, `createSubagentParamsSchema` drops the `workflow` family and substitutes a
deliberately small `tasks` / `chain` pair — `{agent, task}` items, and steps that are
`{agent, task?, as?}` or `{parallel: [{agent, task}]}`, with `additionalProperties: false` and no
`phase`, `label`, `cwd`, `machine`, `count`, `output`, `reads`, `progress`, `skill`, `model`,
`fast`, `toolBudget`, `acceptance`, `agentContract`, `gateOn`, `expand`, `collect`, `concurrency`,
`failFast` or `worktree`. Those inputs are then **compiled into a package-owned workflow script**
(`buildStructuredWorkflowScript`, `src/workflows/structured-workflow-scripts.ts:175`): every caller
string is embedded through `JSON.stringify` and never becomes code, `{task}`/`{previous}`/
`{outputs.<name>}` compile to string concatenation over script variables in a single
non-rescanning pass, and the generated script runs on the ordinary workflow runtime with a `settle`
/`failure` prelude. Three narrowings landed in the same diff: `usageBudget` gained
`minProperties: 1`, `toolBudget`'s description now states `soft <= hard`, and the `timeoutMs`
description says the aliases "must agree".

**cyrup** — `extension/tool/schema.rs` is a faithful port of the **v0.71.0** shape: `sj_chain_item`
(`:259`, whose doc cites `schemas.ts:190-229`), `sj_parallel_task` (`:166`, citing `:133-152`),
`sj_dynamic_parallel_template` (`:194`), `tasks` (`:520`), `concurrency` (`:525`), `worktree`
(`:526`), `chain` (`:527`), `chainDir` (`:537`). `buildStructuredWorkflowScript`,
`structured_workflow` and `StructuredWorkflow` have zero hits in `crates/`, and
`structured-workflow-scripts` has zero hits in `docs/gap-analysis/`. cyrup's chain/parallel
orchestration is native (`spawn/chain_graph.rs`, `extension/executor/chain.rs`), not a lowering.

**Impact** — This is the largest single piece of pi-subagents drift in the window, and it is a
**design** divergence, not a missing feature: upstream has decided that one execution engine
(workflow scripts) runs everything, and that the data-shaped `chain`/`tasks` surface is a
reduced-capability fallback for operators who turn scripts off. cyrup has two engines. Severity is
`medium` and not higher because cyrup's surface works and is a superset of upstream's reduced one —
nothing breaks for a cyrup user. What breaks is parity reasoning: every future `chain`/`tasks`
upstream change now lands in a generated script, so a line-for-line comparison of the two
`chain` paths stops being meaningful.

**Fix** — Needs a decision of record before any code. The options, with what each costs: (a) follow
upstream — lower `chain`/`tasks` into generated scripts and keep the native graph only as the
script runtime's executor. This is the only option that restores line-level parity, and it is a
large, risky change to the most heavily tested code in the crate. (b) Keep the native engine and the
full schema, and record a `[CYRUP-DELTA]` saying so — then the schema narrowings in this diff
(`usageBudget.minProperties`, the two descriptions) are still worth porting on their own, and future
upstream `chain` work has to be back-translated by hand. (c) Keep the native engine but port
`createSubagentParamsSchema`'s reduction so `disabledFeatures: ["workflow-scripts"]` behaves as
upstream does — this depends on `SUBA-152` and is the cheapest way to stop the surfaces diverging
further. Recommend (b) plus (c), and say so in the file rather than leaving it implied.

**Verify** — For (c): with `workflow-scripts` disabled the advertised schema has `tasks`/`chain` in
upstream's reduced shape and no `workflow`/`args`/`preflight`/`globalConcurrencyLimit`/
`maxSubagentSpawnsPerRun`; with it enabled the schema is unchanged. For the narrowings: an empty
`usageBudget: {}` is refused.

## SUBA-152 — `config.disabledFeatures` is unported, so an operator cannot trim the `subagent` tool

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**upstream** — `60905d10` (#2543, v0.73.0) added `src/shared/disabled-features.ts`.
`SUBAGENT_FEATURES` (`:8` @v0.74.0) is 15 groups, each owning the actions and the params that only
it needs: `agent-management`, `watchdog`, `panes`, `missions`, `lane-management`,
`spawn-budget-grants`, `preflight`, `lane-metadata`, `gates`, `usage-budgets`, `tool-budgets`,
`control-overrides`, `extension-bindings`, `external-machines`, `workflow-scripts`.
`resolveDisabledFeatureSurface` (`:82`) builds the disabled param/action maps and folds
`scheduledRuns.enabled: false` in as a sixteenth pseudo-feature `schedules`;
`disabledFeatureUseError` (`:98`) rejects a request that uses a disabled action or param with
`subagent action '<a>' is disabled by config disabledFeatures "<f>".` /
`subagent option '<p>' is disabled by config …`; `disabledFeatureNotice` (`:115`) prepends a
"Disabled by config in this session" block to the static reference docs. `validateDisabledFeatures`
(`:51`) refuses a non-array, an unknown name (listing all 15), a duplicate, and the literal
`"schedules"` with a message naming `scheduledRuns.enabled` instead. `disabledFeatures` is in
`FAIL_CLOSED_CONFIG_KEYS` (`src/extension/config.ts:17`) and validated at `:182`. The surface
reaches the tool at `src/extension/index.ts:705,711,715,720,721` and the executor at `:803`
(`workflowScriptsDisabled`). Declared at `src/shared/types.ts:2674`.

**cyrup** — `grep -rn 'disabled_features\|disabledFeatures\|DisabledFeature' crates/ --include='*.rs'`
is empty, and so is the same grep over `docs/gap-analysis/`. The key is not in
`UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries) either, so
`SubagentExtensionConfig`'s `#[serde(default)]` with no `deny_unknown_fields` accepts it and drops it
with no warning — the same advertise-and-ignore shape `SUBA-061` was filed for.

**Impact** — Low on its own: an operator who sets the key gets the full tool and no error. It is
listed because it is the hinge for `SUBA-151`'s option (c) — upstream's reduced `chain`/`tasks`
schema exists only when this key disables `workflow-scripts` — and because the `subagent` tool
description is the single largest prompt cost this crate imposes, so letting an operator cut 15
groups out of it is a real context win.

**Fix** — `S` for the data and the validator, `M` for the wiring. Port `SUBAGENT_FEATURES` as a
`const` table with upstream's names and member lists verbatim; add `disabled_features: Option<Vec<String>>`
to `SubagentExtensionConfig` with `validate_disabled_features` in `validate_raw_config` beside
`validate_missions`; build the surface once per session and consult it in three places — the
advertised schema (`extension/tool/schema.rs`), the tool description
(`registration/tool_description.rs`, prepending the notice) and the dispatch boundary
(`extension/tool/routing.rs`, before any action runs). The `workflow-scripts`-owns-`preflight` rule
at `disabled-features.ts:91` is load-bearing: a param shared with `workflow-scripts` is always
attributed to it whatever order the config lists features in.

**Verify** — Each of the 15 names disables exactly its own actions and params and nothing else; a
disabled action and a disabled param each get upstream's message; `"schedules"` in the array is
refused by name; a duplicate and an unknown name are refused; `scheduledRuns.enabled: false` alone
disables the nine `schedule.*` actions and their ten params.

> **CORRECTED 2026-10-03:** the claim that cyrup accepts `disabledFeatures` and "drops it with no warning" is false. `SubagentExtensionConfig::config_warnings` (`registration/mod.rs:927-945`) runs `discovery::key_census` and emits `unknown key 'disabledFeatures' (ignored)` (`:944`); `crates/cyrup/src/subagent_config.rs:75-77` prints it to stderr on load (only when `validate_raw_config` passes). What is true is narrower: the key is not in `UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries), so the message is the generic typo wording rather than the "is not supported by this port (...); it has no effect" wording an unported key gets, and nothing says the key is a real upstream feature. The upstream cites (`disabled-features.ts:8/82/98`, `config.ts:17/182`) hold; v0.75.0 only widens upstream's fail-closed list (see `SUBA-166`).

## SUBA-153 — `SUBA-139`'s port target moved: three activation modes and a per-API cache-miss gate

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**This row does not duplicate `SUBA-139`.** `SUBA-139` (open, low, `09b:242`) says the
`subagents_enable` lazy loader is unported and cyrup behaves as upstream's unsupported-host
fallback. That is still true. This row records that the thing `SUBA-139` asks to be ported **changed
shape** in this window, so anyone picking `SUBA-139` up now would port a surface that no longer
exists. Close them together.

**upstream** — `92a8a1f5` (#2595, v0.74.0) adds `config.toolActivation`
(`src/shared/types.ts:2595,2672`; validated at `src/extension/config.ts:175-176` with
`config.toolActivation must be "auto", "dynamic", or "eager"`, and listed in
`FAIL_CLOSED_CONFIG_KEYS` at `:17`):

- `eager` returns immediately from `registerSubagentToolActivation`
  (`src/extension/tool-activation.ts:85-87`) — no loader at all, `subagent` always active. Upstream
  says this equals `--exclude-tools subagents_enable`.
- `dynamic` always selects the loader.
- `auto` (the default) goes eager **only** for an empty session **and only when the model cannot add
  tools mid-conversation without a cache checkpoint**. That last predicate is
  `addsToolsWithoutCheckpoint` (`:36-53`), a per-API read of the model's compat flags:
  `anthropic-messages` needs `supportsMidConvoToolChanges`, `openai-completions` needs
  `supportsMidConvoToolAdditions`, the three Responses APIs need `supportsAdditionalTools ||
  supportsToolSearch`, and all of them first need `supportsMidConvoSystemMessages`; any other API is
  `false`. The point is stated in the comment: without those flags a mid-conversation tool change
  makes Pi resend the conversation under a new leading system message and miss the prompt cache.

Three further changes in the same file: `setSelection` now takes `includeLoader` as well as
`includeSubagent` and can deselect the loader; the recorded-selection replay tracks the whole
recorded tool set rather than `subagent`'s membership alone, so `auto` never adds the loader to a
transcript that did not declare it; and the decision is taken at `session_start`/`session_tree`
only, so switching models mid-session keeps the session's tools. Earlier commits in the window round
the loader out: `3bd1892f`/#2525 (tell the model to check its tool list instead of waiting),
`32ceaab0`/#2492 (accept stray `subagents_enable` arguments), `2f39ae2c`/#2531 (enable on in-process
hosts), `2f8c55a5`/#2514 (tell the model when enabled tools arrive on bridged providers).

**cyrup** — `toolActivation`, `tool_activation`, `ToolActivationMode` and `subagents_enable` all have
zero hits in `crates/`. The compat flags the gate reads **do** exist port-side
(`crates/cyrup-provider/src/api/compat.rs:456` `supports_mid_convo_system_messages`, `:466`
`supports_mid_convo_tool_additions`, `:520` `supports_mid_convo_tool_changes`, plus
`supports_tool_search`/`supports_additional_tools`), so the predicate is portable today.

**Impact** — Low, and it is `SUBA-139`'s impact: the full `subagent` tool is always in the prompt.
The addition here is that the fix is now three-valued, and that the `auto` arm's whole purpose is a
prompt-cache property — which is also what the v1.0.0 release post calls "deferred tool loading"
and "cache warming". A port that implements only the v0.71.0 two-state loader would cost cyrup the
cache on exactly the providers upstream protects.

**Fix** — Port with `SUBA-139`, not after it. Add `toolActivation` to `SubagentExtensionConfig` with
upstream's validator message; port `addsToolsWithoutCheckpoint` against
`crates/cyrup-provider/src/api/compat.rs`'s fields, keeping the per-API switch and the
`supportsMidConvoSystemMessages` precondition; register the loader only for `auto`/`dynamic`.

**Verify** — `eager` registers no loader; `dynamic` always selects it; `auto` on an empty session
with a model whose compat says it can add tools mid-conversation selects the loader, and with a
model that cannot, goes eager; `auto` on a session with history keeps whatever the transcript
declared; a model switch mid-session changes nothing.

> **CORRECTED 2026-10-03:** the statement that the compat flags the gate reads "do exist port-side ... plus `supports_tool_search`/`supports_additional_tools`" is wrong for the second flag. `ModelCompat` (`crates/cyrup-provider/src/api/compat.rs`) has `supports_mid_convo_system_messages` (`:456`), `supports_mid_convo_tool_additions` (`:466`), `supports_mid_convo_tool_changes` (`:520`) and `supports_tool_search` (`:569`), but no `supports_additional_tools` field: a grep of `crates/**/*.rs` for `supports_additional_tools` finds nothing and for `supportsAdditionalTools` finds only a test comment (`providers/openai_codex.rs:779`). `supportsAdditionalTools` appears in the catalog JSON (`opencode-go.json`, `openai-codex.json`, `github-copilot.json`), and `ModelCompat` sets no `deny_unknown_fields`, so the catalog key is ignored. The Responses-API arm of `addsToolsWithoutCheckpoint` (`tool-activation.ts:36-53` @v0.74.0) therefore needs a new compat field and catalog plumbing first; "the predicate is portable today" holds only for the `anthropic-messages` and `openai-completions` arms.

## SUBA-154 — `--default-prefix` needs git ≥ 2.43, and a failed worktree capture destroys the child's work

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `1cf63c18` (#2527, v0.73.1) changed `MACHINE_DIFF_OPTIONS`
(`src/runs/shared/worktree.ts:23` @v0.74.0) from
`["--no-color","--no-ext-diff","--no-textconv","--default-prefix","--line-prefix=","--no-relative"]`
to `["--no-color","--no-ext-diff","--no-textconv","--src-prefix=a/","--dst-prefix=b/","--line-prefix=","--no-relative"]`.
The changelog entry says why: explicit `a/`/`b/` prefixes "work on older Git releases while still
overriding `diff.noprefix`". `MACHINE_PATCH_OPTIONS` is `[...MACHINE_DIFF_OPTIONS, "--binary"]`, so
both move together.

**cyrup** — `spawn/worktree.rs` has the v0.68.0 list twice, spelled out rather than composed:
`MACHINE_DIFF_OPTIONS` at `:75-82` and `MACHINE_PATCH_OPTIONS` at `:89-97`, both containing
`"--default-prefix"` (`:79`, `:95`). `git diff --default-prefix` was added in git **2.43**
(Nov 2023); older git exits non-zero with `error: unknown option 'default-prefix'` and writes no
diff. `--src-prefix=`/`--dst-prefix=` have been accepted for well over a decade.

**Impact** — This is `medium`, not `low`, and the reason is in cyrup's own doc at
`spawn/worktree.rs:71-74`: "a harvested worktree is REMOVED right after capture
([`crate::spawn::chain_graph`]'s `publish_worktree_handoff`), so the patch is all that survives".
On git < 2.43 every managed-worktree child's diff and patch capture fails, and then the worktree is
removed — the child's work is gone with no patch to recover it. `SUBA-130`'s closure added a 256 MiB
`PATCH_CAPTURE_MAX_BYTES` rule precisely so that an overflow **fails the capture and preserves the
worktree**; an `unknown option` failure needs the same treatment and, better, should not happen.
`validate_worktree_patch_represents_current_worktree`'s byte-for-byte re-capture comparison also
fails on both sides, so the failure mode is total rather than partial.

**Fix** — Two one-line edits in `spawn/worktree.rs` (`:79` and `:95`) replacing `--default-prefix`
with `--src-prefix=a/` and `--dst-prefix=b/`, and the doc comment at `:67` updated (it currently
explains `--default-prefix` as the answer to `diff.mnemonicPrefix`/`diff.noPrefix`; the explicit
pair answers both as well, and the doc should say so). While there, check that a capture failure
preserves the worktree rather than letting `publish_worktree_handoff` proceed to cleanup.

**Verify** — A repo with `diff.noprefix=true` and `diff.mnemonicPrefix=true` still produces a
`-p1`-applicable patch; the argv contains no `--default-prefix`; a capture failure (any cause)
leaves the worktree on disk. The existing `spawn::worktree::tests` harvest cases cover the first.

> **CORRECTED 2026-10-03:** (1) "a failed capture destroys the child's work" is wrong. `cleanup_worktrees` is called with `WorktreeCleanupIntent::Preserve(PreserveEvidence)` (`spawn/chain_graph.rs:2001`), and `refuse_uncaptured_preserve` (`spawn/worktree.rs:1864`) refuses to remove a worktree whose capture row has an error, whose patch file is missing, or whose patch is not recorded in the handoff manifest, leaving it on disk with `preserved: true` (doc at `worktree.rs:1575-1582`). So an `unknown option` failure costs a missing patch and a kept worktree, not the work; the Impact paragraph below and the "better, should not happen" hedge overstate it, and the Fix's last sentence ("check that a capture failure preserves the worktree") is already satisfied. (2) The "git >= 2.43" figure is unsourced: the v0.73.1 changelog says only that worktree diffs "now work with older Git versions that lack the `--default-prefix` option" (`CHANGELOG.md` @v0.74.0), and the version number was not verified. The two edits at `spawn/worktree.rs:79,95` and the upstream cite (`worktree.ts:23` @v0.74.0) stand. Recommended re-rating (not applied): medium to low, because the preserve gate keeps the worktree and the effect is a missing patch.

## SUBA-155 — `modelScope` is the three-key v0.33 shape: no per-agent rules, no `inherit`, no `scoped`

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**Not in-baseline, and not previously filed.** `git -C tmp/pi-subagents show v0.43.0:src/runs/shared/model-scope.ts`
has neither `agents` nor an `inherit` expansion, so none of this belongs to `09`. The ledger's
`modelScope` rows are `SUBA-003` (closed, "ported and enforced"), `SUBA-035` (closed, the doctor and
models surfaces) and `SUBA-050` (closed, `strict`); none of them mentions `agents`, `inherit` or
`scoped`. The `agents`/`inherit` half is therefore pre-v0.74.0 drift this file's earlier passes
missed; `scoped` is new in this window.

**upstream** — At v0.74.0 `ModelScopeConfig extends ModelScopeRule` with
`agents?: Record<string, ModelScopeRule>` — "Additional restrictions keyed by canonical agent name"
— and `resolveModelScopesForAgent(config, agentName, parentModel, scopedModelIds)` returns a global
scope plus, when present, an `modelScope.agents.<name>` scope whose `enforce`/`strict` fall back to
the global ones. Allow patterns go through `expandReservedPatterns`: `inherit` becomes the parent
session's `provider/id`, and `scoped` (`SCOPED_PATTERN`, `src/runs/shared/model-scope.ts:51`,
added by `c305d4bb`/#2538, v0.73.0) becomes the parent session's scoped-model snapshot — pi's
`/scoped-models` — degrading to `inherit` semantics when the parent is unscoped. A token that
cannot be expanded is returned unexpanded so the enforced-inherit path in `model-resolution.ts`
fails **closed**. `checkModelScope` also gained a render cap: more than `MAX_RENDERED_PATTERNS = 8`
(`:53`) patterns render as `a, b, …, h, … (N patterns total)` (`:93`). Every launch path passes the
snapshot; background runs keep the set captured at start.

**cyrup** — `exec/model_scope.rs:40` `ModelScopeConfig` has exactly three fields: `enforce`,
`strict`, `allow`. There is no `agents`, no `resolve_model_scopes_for_agent`, no reserved-token
expansion (`grep -n 'inherit\|agents' exec/model_scope.rs` finds only prose and test names) and no
render cap. `allow` is matched literally with only `*` special (field doc at `:58-59`).
`settings.json`'s `subagents.modelScope.agents` is accepted by serde and dropped.

**Impact** — Two concrete failures, which is why this is `medium`. (1) An operator who copies pi's
documented `"modelScope": {"enforce": true, "allow": ["inherit"]}` gets a cyrup that matches the
literal string `inherit` against `provider/id`, matches nothing, and **rejects every explicit
model** — the run is refused with `SubagentError::ModelOutOfScope` naming an allowlist of one
meaningless pattern. (2) `modelScope.agents.<name>` is silently dropped, so a per-agent restriction
an operator believes is in force is not; that is the failure direction that matters for a policy
knob.

**Fix** — `M`. Add `agents: Option<BTreeMap<String, ModelScopeRule>>` and split the three shared
fields into a `ModelScopeRule` as upstream does; port `resolve_model_scopes_for_agent` with
upstream's `enforce`/`strict` fallback and its two origins (`modelScope`,
`modelScope.agents.<name>`); port `expand_reserved_patterns` for `inherit` and `scoped`, keeping the
unexpanded-token-fails-closed rule; add the 8-pattern render cap to the violation message. The
`scoped` arm needs the parent session's scoped-model set threaded to each launch and captured at
start for background runs — cyrup already has the set (`cyrup-tui/src/app/selectors.rs:225`
`CheckboxSelector::scoped_models`, `event_extract.rs:355`), but `RunnerConfig` does not carry it,
which is the one non-trivial piece.

**Verify** — `allow: ["inherit"]` with `enforce` admits the parent session's model and refuses
another; `allow: ["scoped"]` admits every scoped model and, with no scoping configured, behaves as
`inherit`; an enforced `scoped`/`inherit` with no parent model fails closed; `agents.<name>` adds a
restriction and inherits `enforce`/`strict` from the global block; a nine-pattern allowlist renders
eight plus `… (9 patterns total)`.

> **CORRECTED 2026-10-03:** line cite: `ModelScopeConfig` is at `exec/model_scope.rs:43` (the row's `:40` is its doc comment); it has the three fields `enforce`, `strict`, `allow` and no `inherit` or `agents`. Read in addition: `parse_model_scope_config` (`exec/model_scope.rs`, from `:274`) reads only `enforce`, `strict` and `allow` and emits no warning for another subkey, which supports the "silently dropped" wording for `modelScope.agents`; a separate key census was not checked.

## SUBA-156 — the MCP direct-tool reader models no `literalEnv`, so agent-plugin servers' digests diverge and their `mcp:` selectors resolve to nothing

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**Scope note.** Upstream's `847ee4de` (#2539) and `14dcf975`/`fa1de042` (#2573/#2576) together move
pi-subagents onto pi-mcp-adapter 3.x and onto Pi's own built-in MCP: `getConfigPaths` now reads
`mcp-adapter.json` instead of `mcp.json`, and `usesBuiltinMcp`/`resolveBuiltinMcpSelections`/
`extensionOnlyMcpServers` add a second resolution path for when the adapter is not loaded. **None of
that is filed here**: cyrup ships its MCP port as the default and has no adapter-versus-built-in
split (`EXT-092` records the one place the distinction surfaces), so there is no second path to
resolve against and no file to rename. What is filed is the one half of `#2539` that is a defect in
cyrup on its own terms.

**upstream** — `computeMcpServerHash` (`src/runs/shared/mcp-direct-tool-allowlist.ts:470` @v0.74.0)
now makes `literalEnv` part of the server identity, and skips env interpolation when it is set:

    const isStdio = definition.command !== undefined;
    const literalEnv = isStdio && definition.literalEnv === true;
    … env: literalEnv ? definition.env : interpolateEnvRecord(definition.env),
      ...(isStdio ? { inheritEnv: definition.inheritEnv !== false, literalEnv } : {}),

with the comment "A mismatch leaves that server's cached direct-tool selectors unresolved." It also
hashes `resolveConfigPath(definition.command)` rather than the raw command.

**cyrup** — the reader is `exec/mcp_direct_tools.rs::server_identity_pre_image_with` (`:1012`); the
writer is `cyrup-mcp/src/dirs.rs::server_identity_pre_image` (`:1294`). Both emit the same 15 keys
and **neither** has `inheritEnv` or `literalEnv`, so for an ordinary server they agree — that is why
this is not already broken everywhere. They diverge for `literalEnv` servers:

- `cyrup-mcp` sets `literal_env: Some(true)` on every agent-plugin stdio server
  (`cyrup-mcp/src/agent_plugin.rs:883`, and `runtime.rs:5733`), and its resolver takes those env
  values **verbatim** — no `$VAR` interpolation and no `!secret` resolution
  (`cyrup-mcp/src/secrets.rs:341,355-359,388`). The writer hashes `resolved.env`, i.e. the verbatim
  values.
- the reader's `ServerEntry` has no `literal_env` field at all
  (`grep -n 'literal_env\|inherit_env' exec/mcp_direct_tools.rs` is empty), so it always runs
  `interpolate_env_record(definition.env, env)` (`:1028`).

**Impact** — Any agent-plugin MCP server whose `env` values contain a `$` sequence or a `!`/`!!`
secret token hashes differently on the two sides. The reader's `is_server_cache_valid` then rejects
that server's cache entry, and the agent's `mcp:<plugin>__<server>/<tool>` selectors resolve to
nothing — **silently**, with no diagnostic. This is exactly the failure the module header at
`exec/mcp_direct_tools.rs:32` was written about ("the two disagreed **silently**: every cached entry
failed hash validation (so `mcp:` selectors resolved to nothing)"), reintroduced through a field the
reader does not model. The secondary divergence is `command`: the writer hashes the plugin's
already-absolutised `resolved_command` (`agent_plugin.rs:871`), the reader hashes
`definition.command` unresolved (`:1020`), and upstream now resolves it on both sides.

**Fix** — `S`, and it is a reader-side change plus one shared test. Add `literal_env` and
`inherit_env` to the reader's `ServerEntry`; gate `interpolate_env_record` on `literal_env`; add the
`inheritEnv`/`literalEnv` pair to the stdio identity on **both** sides in the same commit so they
cannot drift again; hash the resolved command on both sides. The crate already has the right place
for the guard: extend
`mcp_direct_tools`' `reader_and_writer_agree_once_every_resolver_actually_runs` (named in the module
header at `:60`) to cover a `literalEnv` server.

**Verify** — A plugin-registered stdio server with `env: { TOKEN: "$HOME/x" }` produces the same
digest from `cyrup_mcp::dirs::compute_server_hash` and
`cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash`, and an agent selecting
`mcp:<plugin>__<server>/<tool>` resolves it; the same server with `literalEnv` honoured only on one
side is red.

---

**Paired with `MCP-594` (reconciled 2026-10-02).** The writer side of the same digest contract is
filed as **`MCP-594`** (`13c-…`, medium, `upstream-drift`): `cyrup-mcp`'s
`server_identity_pre_image` is missing the same two stdio keys and hashes an interpolated `env` for a
`literalEnv: true` entry. It also names the cost this row does not: adding the keys invalidates every
stdio server's metadata-cache entry once, and voids the golden pre-image vectors pinned at
`cyrup-mcp/src/dirs.rs:1694`, `:1734` and `:1808`, which must be regenerated from `v5.0.0` rather than
hand-edited. **Land both sides in one commit**, as both rows require.

> **CORRECTED 2026-10-03:** effort reads S+, not S: the fix touches two crates (`cyrup-ext-subagents` reader and `cyrup-mcp` writer, see `MCP-594`), regenerates golden pre-image vectors and invalidates every stdio server's metadata cache once. Recommended re-rating (not applied): effort S to M. **FOLD-IN 2026-10-03 (v0.75.0):** `3f367e75`/#2607 sanitises `-` to `_` in `resolveBuiltinMcpSelections` (`src/runs/shared/mcp-direct-tool-allowlist.ts:183-189` @v0.75.0) so hyphenated `mcp:` names match Pi's built-in MCP. It belongs to the built-in-MCP path this row's scope note leaves unfiled; cyrup's direct-tool matcher already registers both the hyphen and the `_` spellings of a tool name (`exec/mcp_direct_tools.rs:1576-1590`), and has no built-in/adapter split, so nothing is owed for it.

## SUBA-157 — `subagents.agentOverrides.<name>.advertise` is unported

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `8dc90dca` (#2534, v0.73.0) adds `advertise?: boolean` to
`BuiltinAgentOverrideConfig` (`src/agents/agents.ts:88` @v0.74.0; the agent's own frontmatter field
is `:141`), parsed in `parseBuiltinOverrideEntry` with
`Builtin override '<name>' in '<file>' has invalid 'advertise'; expected a boolean.` and applied in
`applyBuiltinOverride` (`if (override.advertise !== undefined) next.advertise = override.advertise;`).
The commit message states the boundary: settings can now opt a builtin **or custom** agent into the
parent-prompt catalog without copying the agent file, and runtime-registered agents still cannot be
advertised.

**cyrup** — the frontmatter half is in: `AgentDefinition::advertise` at `discovery/types.rs:1352`,
cited to `agents.ts:140` @v0.71.0, feeding `discovery::advertised::build_advertised_agent_prompt`
(`SUBA-133`, closed 2026-09-28). The override half is not: `AgentOverrideConfig`
(`discovery/types.rs:686`) has 20 fields and no `advertise` (`grep -n advertise` over
`discovery/types.rs` and `discovery/merge.rs` finds only `AgentDefinition::advertise` and two
`advertise: None` literals in fixtures).

**Impact** — Low. A setting pi documents is accepted and dropped, so the only way to advertise a
bundled agent in cyrup is to eject and edit it. `SUBA-096` is this row's neighbour: it carries the
three override fields (`acceptance_role`, `output_mode`, `fast`) that `SUBA-081`'s partial closure
left, and `advertise` is a fourth of the same kind. If `SUBA-096` is picked up, do both in one pass.

**Fix** — One `OverrideField<bool>` on `AgentOverrideConfig` beside `description`, parsed with
upstream's message verbatim and applied in `discovery/merge.rs` where the other booleans are.

**Verify** — `subagents.agentOverrides.reviewer.advertise = true` puts a bundled agent in the
`<advertised_subagents>` block; `= "yes"` is refused with upstream's message; a runtime-registered
agent is still not advertised.

> **CORRECTED 2026-10-03:** line cite: `AgentOverrideConfig` is at `discovery/types.rs:673` (the row's `:686` is inside the struct); `advertise` exists only on `AgentDefinition` (`:1352`).

## SUBA-158 — a spawned subagent child does not follow the parent session's project trust

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**This escalates `SUBA-142`, by that tracker's own rule.** `SUBA-142` (tracker, `09b:245`) parks
upstream's in-process-child features and says it "escalates to an item … when one of the parked
features is shown to change a result for a spawned cyrup child". Project trust is such a feature:
upstream's fix is in-process (`SettingsManager.create(..., { projectTrusted })`), but the *result*
it corrects — the child loading the project as untrusted while the parent has it trusted — is
reproduced by cyrup's spawned child. The same applies to `4cd43cae`/#2541's `skillsOverride`, which
is **not** filed here because it corrects a Pi-internal path (`extendResources` merging
`resources_discover` skill paths past `noSkills`) that a cyrup child, launched with `--no-skills`
(`exec/spawn_plan.rs:862`), does not take.

**upstream** — `b2718fb8` (#2570, v0.73.1) threads the launching session's trust to every child:
`ChildSessionLaunch.projectTrusted` (`src/runs/shared/child-session.ts:59` @v0.74.0) reaches
`pi.SettingsManager.create(launch.cwd, agentDir, { projectTrusted: launch.projectTrusted })` (`:373`),
and `sessionProjectTrust(ctx)` (`src/runs/foreground/subagent-executor.ts:645`,
`typeof ctx.isProjectTrusted === "function" ? ctx.isProjectTrusted() : undefined`) is passed at
every launch site — `runSinglePath`, `runAsyncPath`, `resumeAsyncRun` (both arms) and
`resumeExternalJobFollowUp`. `undefined` keeps Pi's default for a host with no trust concept.

**cyrup** — nothing trust-shaped reaches the child. `exec/spawn_plan.rs:300-340` documents the whole
argv and env overlay and names no trust flag; `grep -n trust exec/spawn_plan.rs` finds one unrelated
comment at `:490`. The `cyrup` binary's own startup posture is
`project_trusted: false` (`crates/cyrup/src/bootstrap.rs:93`, "cyrup's standing pre-trust posture
(R-07-002)"), and the only override is the CLI `--approve`/`-a` pair
(`crates/cyrup/src/subcommands.rs:57-67`), which the child is not given.

**Impact** — A parent session whose project trust was granted in-session (the ordinary interactive
path — `crates/cyrup/src/interactive.rs:197-198` carries `auto_trust_on_reload_cwd` precisely
because that grant is not yet persisted; `TUI-037` is open for the same reason) spawns children that
load the project as **untrusted**. The child therefore resolves a different settings and
project-resource set than the parent: project-local settings and project extension discovery are
suppressed where the parent has them, and `mcp-direct-tool-allowlist.ts`' own comment in this window
("the project file is read only when the project is trusted") shows upstream treating the project
MCP file the same way. A child quietly running under a narrower configuration than its parent is a
behaviour divergence a user cannot see, which is why this is `medium` and not `low`.

**Fix** — `S`. Decide the channel — a `--approve`-shaped argv flag is the smallest, an env var
beside the existing `INHERIT_PROJECT_CONTEXT_ENV`/`INHERIT_SKILLS_ENV` pair is the most consistent
with how every other inherit flag reaches a cyrup child — then set it from the parent's
`AgentSessionServices::project_trusted` (the field `crates/cyrup-tui/src/app/session_bind.rs:131`
reads) at each launch site, with "absent means the child decides for itself" as the default so a
host with no trust state is unchanged. Record the decision as a `[CYRUP-DELTA]` against
`child-session.ts:373`, since the mechanism differs even though the result must not.

**Verify** — In a project trusted only in the parent session, a foreground child, an async child and
a resumed async child each load the project's local settings; with the parent untrusted, none of
them does; a launch with no trust information behaves exactly as today.

> **CORRECTED 2026-10-03:** the cyrup evidence is mis-aimed. `crates/cyrup/src/bootstrap.rs:93` (`load_startup_settings`) is the process's startup settings manager, used for settings diagnostics and the `sessionDir` lookup; it does not decide the project trust a session runs under. Real trust resolution is `SessionBuilder` in `crates/cyrup-session-svc/src/builder.rs:801-853`: settings loaded with the project untrusted, `default_project_trust`, `has_trust_requiring_resources`, the trust-store `nearest` lookup, then `TrustInputs { trust_override, saved, default_trust, mode, .. }` into `decide_trust_with_extension`. `cyrup-ext-subagents` passes no `--approve`/`--no-approve` and no trust env to the child (no hits in the crate outside unrelated tests), so a child resolves trust itself from the trust store and `defaultProjectTrust`. Direction caveat: upstream's v0.74.0 changelog describes the opposite symptom ("a child in an untrusted project still loaded that project's settings"), so the row's "child less trusting than the parent after an in-session grant" is the mirror case of the same missing hand-off. Neither was reproduced end to end; the defect is plausible but unproven. Recommended re-rating (not applied): medium to low until a spawned child is shown to resolve a different trust than its parent.

## SUBA-159 — runner liveness probes are not scoped to the PID namespace, so a cross-namespace observer fails a live run

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `d5d3b9ff` (#2498, v0.72.0) adds `src/runs/background/pid-namespace.ts:7`
`currentPidNamespaceScope()` — the `readlink("/proc/self/ns/pid")` value on Linux, cached,
`undefined` elsewhere — stamps it on `AsyncStatus.pidNamespaceScope`
(`src/shared/types.ts`, "Linux PID namespace identity used to scope liveness probes") at both
status-writing sites (`spawnRunner`, `buildStartedStatus`), and consults it in
`reconcileAsyncRun`:

    const pidScopeMismatch = effectiveStatus.pidNamespaceScope !== undefined && effectiveStatus.pidNamespaceScope !== observedScope;
    const liveness = observedLiveness === "dead" && pidScopeMismatch ? "unknown" : observedLiveness;

(`src/runs/background/stale-run-reconciler.ts:439,441` @v0.74.0). A cross-namespace `dead` becomes
`unknown`, which falls through to the existing 24-hour stale rule instead of failing the run, and the
failure sentence gained a second probe text: `could not be probed from this process` where the
observed liveness was not `alive`. The commit names the two shapes it fixes: another container, and
a macOS/Windows host sharing a container's `PI_SUBAGENTS_TEMP_ROOT`.

**cyrup** — `background/reconcile.rs:144` `check_pid_liveness` is a bare `kill(pid, 0)` with
`ESRCH => Liveness::Dead`, and the decision at `:383` is `Liveness::Dead => true` — fail
immediately, no stale window. `AsyncStatus` has no `pidNamespaceScope`
(`grep -rn 'pidNamespaceScope\|pid_namespace\|/proc/self/ns/pid' crates/` is empty), and the
`Liveness::Alive | Liveness::Unknown` arm still prints only `still has a live PID` (`:406`), so
upstream's second sentence has no counterpart. cyrup does have `processDemonstrablyGone`'s
start-identity check (`:166-174`) against PID **reuse**, which is a different hazard: a PID that
does not exist in this namespace has no start identity to compare.

**Impact** — `low`, honestly scoped: cyrup exposes no documented shared-temp-root env (the run tree
hangs off cyrup's own `TEMP_ROOT_DIR`, `exec/mod.rs:1365`), so reaching this needs a deployment that
shares the state directory across PID namespaces by mount. In that deployment the consequence is not
cosmetic — a **live** run is marked failed and a `synthesize_failure` record is written over it. The
row is worth having because the fix is small and because the reconciler's "dead means dead" rule is
the one place cyrup is strictly less careful than upstream about a probe it cannot trust.

**Fix** — `S`. Add `pid_namespace_scope: Option<String>` to the async status record, populated from a
cached `readlink("/proc/self/ns/pid")` on Linux and `None` elsewhere; stamp it wherever `pid` is
stamped; in `reconcile`, downgrade `Dead` to `Unknown` when a recorded scope is present and differs
from the observed one (including when the observer has none), and add upstream's second probe
sentence.

**Verify** — A status carrying a recorded scope that differs from the observer's, with a PID that is
absent locally, is **not** failed until the stale window elapses, and then fails with the
`could not be probed from this process` sentence; a matching scope, and an absent recorded scope,
both behave exactly as today.

> **FOLD-IN 2026-10-03 (v0.75.0, `806e3678`/#2606, Linux zombie runners):** `checkPidLiveness(pid, kill, probeZombie)` (`src/runs/background/stale-run-reconciler.ts:360-370` @v0.75.0) now reads `/proc/<pid>/stat` after a successful `kill(pid, 0)` and returns `dead` when the state field is `Z`. `probeZombie` is true only when the status's recorded `pidNamespaceScope` equals the observer's (`stale-run-reconciler.ts:446-451`; the same gate in `await-async-run.ts:15-24`). Cyrup's `check_pid_liveness` (`background/reconcile.rs:144`) is a bare `kill(pid, 0)` and reports a zombie as `Alive`. `background/spawn_detached.rs:391` drops the runner's `Child` (`None => drop(child)`), so a runner that outlives its launcher is reparented, and where PID 1 does not reap (a container without an init) it stays a zombie that reads as alive; that is the triager's reading, not run, and it needs no cross-namespace mount. Scope widens: port the `/proc/<pid>/stat` zombie probe together with `pid_namespace_scope`. Recommended re-rating (not applied): low to medium if the init-less-container zombie is judged reachable.

## SUBA-160 — `timeoutMs`/`maxRuntimeMs` have no timer-delay cap although two neighbouring params do

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `5655f9bb` (#2517, v0.73.0) added `timerDelayOverflowError`
(`src/runs/foreground/subagent-executor.ts:2950` @v0.74.0, over
`MAX_TIMER_DELAY_MS = 2_147_483_647` at `:2948`) returning
`<name> must be a positive integer no larger than 2147483647.`, and applies it in three places: in
`resolveForegroundTimeout`, at the executor entry for both `timeoutMs` and `maxRuntimeMs`, and in
`resumeAsyncRun` for `action: "resume"`'s own `timeoutMs`. The changelog states Node's failure mode:
a larger delay was shortened to about 1 ms, so the run timed out almost immediately.

**cyrup** — the constant and the message already exist, for other params:
`exec/tool_timeout.rs:24` `MAX_TIMER_DELAY_MS`, its error at `:221`
(`{label} must be a positive integer no larger than {MAX_TIMER_DELAY_MS}.`, upstream's string), and
`registration/mod.rs:588` `MAX_CHECKPOINT_BEFORE_DEADLINE_MS` with `SUBA-128`. But
`resolve_foreground_timeout` (`extension/tool/params.rs:566-588`) checks only `value == Some(0)` and
the `timeoutMs`/`maxRuntimeMs` alias clash; both fields are `Option<u64>`
(`extension/tool/params.rs:222-223`), so any value is accepted. `action: "resume"` has no such
check either.

**Impact** — Node's specific corruption does not reproduce: a Rust/tokio deadline built from a huge
`u64` saturates rather than wrapping to ~1 ms, so the practical effect is "no deadline" rather than
"instant timeout". The defect is the inconsistency: the tool refuses an oversized `toolTimeoutMs`
and `checkpointBeforeDeadlineMs` with a specific message and accepts an oversized `timeoutMs`, which
is the opposite of this crate's advertise-and-dispatch-agree rule. `low`, and it is worth doing
only because it is three lines against a constant that is already in the tree.

**Fix** — Apply `exec::tool_timeout`'s existing overflow check to `timeout_ms` and `max_runtime_ms`
in `resolve_foreground_timeout`, and to the `resume` arm's `timeoutMs` in
`extension/executor/background.rs` beside the existing `checkpointBeforeDeadlineMs` check. Reuse the
existing message builder rather than restating the string.

**Verify** — `timeoutMs: 2_147_483_648` and `maxRuntimeMs: 2_147_483_648` are each refused with
upstream's message before any launch; `action: "resume"` with the same value is refused;
`2_147_483_647` is accepted.

> **CORRECTED 2026-10-03:** the Impact paragraph's claim that "a Rust/tokio deadline built from a huge `u64` saturates" is unverified; `Instant + Duration` arithmetic can panic on overflow, so the real effect of an oversized value may be a panic rather than "no deadline". The defect and the cites (`params.rs:566-588` checks only zero and the alias clash) stand.

## SUBA-161 — the `council` guide topic and the multi-file guide body are unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `a0fd73df` (#2469, v0.72.0) makes `council` an eleventh guide topic
(`src/extension/subagent-guide.ts:16` @v0.74.0) and gives `readSubagentGuide` a multi-file arm
(`:39`): a topic may name several files, and when it does the body is their contents joined with a
`<!-- <path> -->` marker before each. `COUNCIL_FILES` (`:25`) is
`skills/council-mode/SKILL.md`, `skills/council-mode/references/pass-contracts.md` and
`skills/pi-subagents/references/execution-controls.md`, and the comment says why the topic exists at
all: "`/council` must work under `pi --no-skills`, which drops the package skills from context, so
this topic serves the council skill together with the references it tells the model to read."

**cyrup** — `registration/guide.rs:44` `SUBAGENT_GUIDE_TOPICS` is the ten-topic v0.47.1 list, and the
doc above it records that the order is load-bearing twice (the unknown-topic message joins it, and
`overview` first makes it the default). `council` has **zero** hits in `crates/` and zero in
`docs/gap-analysis/` — neither the topic nor the `council-mode` skill it serves has ever been filed.

**Impact** — Low and bounded: `guide topic: "council"` answers
`Unknown subagents guide topic 'council'. Valid topics: …`. Behind it is the larger question of
whether cyrup ships pi-subagents' bundled skills at all — upstream carries nine files under
`skills/` at v0.74.0 — which is a bundled-asset decision, not this row's work.

**Fix** — `S` for the mechanism, and the mechanism is what to port: add the multi-file arm and its
`<!-- path -->` marker to the guide reader, since that is reusable, and add `council` as the
eleventh topic **last** in the list, preserving the order rule. Whether the three files ship is a
separate decision; if they do not, the topic should not be advertised, because this crate does not
advertise a topic it cannot answer.

**Verify** — `guide` with `topic: "council"` returns the three files in order, each preceded by its
`<!-- path -->` marker; the unknown-topic message lists eleven topics in upstream's order;
`overview` is still the default.

> **CORRECTED 2026-10-03:** effort S is optimistic (S to M): a working `council` topic needs the three skill files bundled (`SUBA-161`'s own Fix says whether they ship is a separate decision), and v0.75.0 changes eight guide docs (`docs/agents.md`, `configuration.md`, `extension-api.md`, `missions.md`, `models.md`, `observability.md`, `tool-reference.md`, `workflows.md`). Recommended re-rating (not applied): effort S to M.

## SUBA-162 — the progressive async-widget tier is unported: no height lock, no lane rows, no running-agent header count

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**upstream** — `src/tui/render.ts` at v0.74.0 has a progressive widget tier that cyrup's port does
not: `buildProgressiveWidgetLines` (`:2704`) locks the card height to the rows its content fills at
lock time, never shrinks between relocks, grows up to the compact cap when a job starts while jobs
are hidden, and fills the rows left after the visible job lines with the visible **workflows' lane
rows** (`9a5a2d5e`/#2583 — before it, the tier padded with blank rows and a workflow's lanes stayed
hidden). `progressiveHeaderLine` (`:2648`) prints a running/queued/failed summary line, and
`runningLeafAgentCount` (`:2635`, `7e07a22d`/#2584) makes its count agree with FleetView's
"N active agents": a workflow counts its loaded child runs recursively, any other running job counts
its active steps, synthesised from `agents` when it has no step detail, with a sequential chain's
non-current `pending` steps excluded. Before the fix a four-lane workflow beside one other run read
"2 agents running" next to "5 active agents".

**cyrup** — `tui/render.rs` is 841 lines against upstream's ~2 700 and is a port of the earlier
shape: `render_background_region` (`:386`) renders one full header-plus-nested block per run up to a
cap and folds the overflow into an aggregate line (`background_region_details_up_to_cap_then_folds_overflow`),
`render_progress_header` (`:356`) is the per-run block, and `render_async_jobs_widget`
(`tui/events.rs:874`) is a thin wrapper that maps snapshots and calls it. There is no header summary
line (`grep -rn 'agents running' crates/cyrup-ext-subagents/src/` is empty), no height lock, no
progressive tier and no lane rows. `SUBA-061`'s closure wired the `asyncWidget` config key to this
renderer, so the slot and its plumbing exist; the tier does not.

**Impact** — Low: a cosmetic difference in an under-editor widget. It is filed because the two
upstream fixes in this window both describe the symptom in user terms (blank rows where a
workflow's lanes belong; a count that disagrees with the Fleet view a key away), and because the
leaf-counting rule is the kind of thing that is cheap to port with the fix in hand and expensive to
re-derive later.

**Fix** — `M`, and splittable. The `runningLeafAgentCount` rule is pure and portable on its own
against cyrup's `AsyncJobSnapshot`/`StepStatus`, and is the half worth doing first: it makes the
widget and `tui/fleet_view.rs` agree. The height lock and lane rows need a locked-rows value
threaded through `render_async_jobs_widget`, which cyrup's `data in -> Vec<Line>` contract
(`tui/render.rs:7`) can carry as a parameter without giving the module state.

**Verify** — A four-lane workflow beside one single run reports the same agent count in the widget
header as `tui/fleet_view.rs` reports active agents; a sequential chain counts one; a job with no
step detail counts its agents; the locked card keeps its height as jobs finish and grows, up to the
cap, when a job starts while jobs are hidden.

> **FOLD-IN 2026-10-03 (v0.75.0, `53aee6d8`/#2621):** also unported are upstream's header-click fold (`93d47c0c`/#2235, v0.68.0; `buildWidgetComponent`, `src/tui/render.ts:2903` @v0.75.0) and v0.75.0's boolean `asyncWidgetCollapsed` (validated `src/extension/config.ts:175-176`, passed as `initiallyCollapsed` at `render.ts:3099`). `grep -rn 'asyncWidgetCollapsed' crates/` is empty, so a user who sets it gets only the generic unknown-key stderr warning. Not verified: whether the cyrup TUI can deliver clicks to an extension widget, which decides whether the click half is portable.

## SUBA-163 — `/subagent-cost` reports no child usage for an async single, chain or parallel launch

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**Why `stale-port`.** `SUBA-138` (closed 2026-09-28, `09b:241`) ported v0.71.0's
`collectSubagentCost` as `registration/cost.rs:995`, and that port is faithful to its tag.
`d556bb01` (#2490) landed in **v0.72.0**, one tag later, and fixes a hole the v0.71.0 collector has.
So this is not a defect in `SUBA-138`'s work; it is the port being measured against a tag that has
moved.

**upstream** — `collectSubagentCost` at v0.74.0 tracks three id sets, not one
(`src/slash/subagent-cost.ts:162-164`): `workflowRunIds`, and the new `asyncRunIds` (`:163`) and
`completedRunIds` (`:164`). The new arm is

    if (details.mode === "workflow" && details.runId) workflowRunIds.add(details.runId);
    // An async launch result has no child results; its usage lands in run artifacts.
    else if (details.asyncId && details.results.length === 0) asyncRunIds.add(details.asyncId);

plus `completedRunIds.add(completion.runId)` for every `bg_wait` completion, an `identity` override
on `addChild` so a step's usage dedupes by its own identity, and an `indexes` parameter on
`metadataUsage` so a run's per-step artifact metadata is read rather than only index `0` and the
unindexed form. The changelog states the symptom: `/subagent-cost` "only read child usage from
workflow receipts and returned results, so an async `subagent` call with an empty `results` list
reported no child usage".

**cyrup** — `registration/cost.rs:1002` declares `workflow_run_ids` and nothing else;
`add_workflow_run_id` is called at `:1022` and `:1052`, the resolution loop at `:1087` walks
`workflow_run_ids` alone, and `unresolved_async_children` (`:1086`) counts only failures inside that
loop. `grep -n 'async_run_ids\|completed_run_ids' registration/cost.rs` is empty. The artifact
metadata read is likewise the two-index form the pre-fix upstream had.

**Impact** — `low`, because it is a report rather than an execution path, but it is a wrong number
shown to the user, not a missing feature: an `async: true` single, chain or parallel launch is the
common case, its tool result carries an `asyncId` and an empty `results` array, and
`/subagent-cost` attributes zero child usage to it. The run's usage is on disk in its artifact
metadata the whole time.

**Fix** — `S`. Add the `asyncId`-with-empty-`results` arm and the `completions` id set to the
collector's transcript walk, resolve those run ids through the same run-dir status / artifact
`_meta.json` path `SUBA-138` already built (the status's steps give the indices), and add the
`indexes` parameter so per-step metadata is read. Keep the existing `run:`/`session:` dedupe and add
upstream's `identity` override so a step does not collide with its run.

**Verify** — An `async: true` single launch, an async chain and an async parallel launch each
contribute their children's usage to `/subagent-cost` and to the RPC `cost` report; a `bg_wait`
completion for the same run does not double-count it; a workflow run still resolves through its
receipt exactly as it does today.

> **CORRECTED 2026-10-03 (stale by v0.75.0):** the port target moved again; `ae9c9d77`/#2612 and `4998ceca`/#2615 change the same function (`git diff v0.74.0 v0.75.0 -- src/slash/subagent-cost.ts`). (a) A workflow id joins `workflowRunIds` only when `details.mode === "workflow" && details.runId && details.asyncId`: foreground workflow child usage is already in `results`, and only async workflows persist a receipt. (b) The child `runId` is read from the typed `result.runId`, with no cast. (c) Every round of a resumed foreground workflow child counts (#2612). (d) A missing or unreadable receipt now adds to `unresolvedAsyncChildren` (an `ENOENT`, also via `error.cause`, is not logged; other errors are), so an async workflow with no receipt yet is listed as `Async child usage unavailable` (#2615). Cyrup diverges on both ends (read, not run): `collect_subagent_cost` (`registration/cost.rs:995`) adds a workflow id on `mode == "workflow"` plus `runId` (`:1014-1020`), but cyrup's foreground workflow details carry `mode`, `children`, `workflowRunId` and no `results`, `runId` or `asyncId` (`extension/executor/workflow_launch.rs:456-470`, `:697`), so foreground workflow child usage is never counted; and a `NotFound` receipt is a silent `continue` (`cost.rs:1102`) instead of an unresolved count. Scope of the fix therefore widens beyond the `asyncId`/`completions` arms: add the arm, gate the workflow arm on `asyncId`, count foreground workflow `children` usage, and count unresolved on a missing receipt.

## Findings filed 2026-10-03 — the `v0.74.0..v0.75.0` window

`pi-subagents` **v0.75.0** (31 non-merge commits since v0.74.0), read through `git -C tmp/pi-subagents show v0.75.0:<path>` and `git diff v0.74.0 v0.75.0`; cyrup read at `11664557`. This file's window is therefore now `v0.57.0..v0.75.0`. Nothing was run. Every commit has a disposition. **Existing rows extended** (see the `FOLD-IN`/`CORRECTED` notes on the rows): `SUBA-022` (#2643, #2640), `SUBA-142` (#2636, #2638), `SUBA-150` (#2610, #2611), `SUBA-156` (#2607), `SUBA-159` (#2606), `SUBA-162` (#2621, and the earlier #2235), `SUBA-163` (#2612, #2615), and `09`'s refuted-`SUBA-042` note (#2628). **New rows:** `SUBA-164` (#2613), `SUBA-165` (#2598), `SUBA-166` (#2624), `SUBA-167` (#2603), `SUBA-168` (#2616), `SUBA-169` (#2604), `SUBA-170` (#2618), `SUBA-171` (#2627 residual), `SUBA-172` (#2620), `SUBA-173` (#2619). **Already matched:** #2605 (`extension/host/native_impl.rs:690-712` logs the drain failure, then still raises the goal notices) and the atomicity half of #2627 (`SUBA-029`). **Nothing owed:** #2649 (refactor), #2648 (CI), #2647 (Windows `.cmd` launch), #2634 (JS module resolution), #2633 and #2626 (tests), the release commit, and #2630 (background `expand` steps with an external-runner agent: cyrup takes the runner from the persona, `runner_main/status.rs:665`; whether materialised `expand` status rows show the runner was not checked). The `v0.71.0..v0.74.0` commits with no row (about 25 PR numbers, mostly workflow and retention work: #2572, #2547, #2536, #2523, #2507-#2510, #2566, #2568) were noted by the review as untriaged and still are.

## SUBA-164 — `tool_open_threshold` attention is unported, so a child stuck in one long tool call never raises `needs_attention`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — The reason has been in pi-subagents since v0.50.0 (`a660ea30`, "add smart tool timeout defaults"); it is unledgered. `shouldEmitOpenToolAttention` (`src/runs/shared/subagent-control.ts:114-122` @v0.75.0) is true when control is enabled, a tool is open, it is not timeout-exempt and it has been open at least `activeNoticeAfterMs`. The foreground `updateActivityState` (`src/runs/foreground/execution.ts:915-933`) and the runner's (`src/runs/background/subagent-runner.ts:2709-2781`) emit `needs_attention` with `reason: "tool_open_threshold"`, the tool name, path and duration. v0.75.0 (`478f871c`/#2613) makes this per call: each active call carries an `attentionEmitted` flag (`subagent-runner.ts:2646,2710,2765`), the event carries `toolCallId`, and the dedupe key includes it (`subagent-control.ts:193`), so two long calls in one child give two notices instead of one per child. For a `bash` call the nudge text (`subagent-control.ts:253-259`) points at `command.status/yield/cancel` (`SUBA-165`).

**cyrup** — `ControlEventReason` has seven variants and no `ToolOpenThreshold` (`exec/control.rs:301-322`). `derive_activity_state` returns `None` whenever a tool is in flight (`:475`, `current_tool.is_some_and(...)`), so idle detection is off while a call is open. `update_activity_state` (`:1897-1919`) has only the idle branch and the elapsed/turn/token long-running triggers; there is no open-tool branch. `ControlEvent` (`:345-395`) has no `tool_call_id`.

**Impact** — A child hung inside one `bash` call raises no `needs_attention`; the parent hears only the generic elapsed/turn/token long-running notice, which names no call and does not say that a tool, not the model, is what is stuck. Medium because it is the notice upstream added specifically for the hung-command case, and v0.75.0 just fixed its per-call behaviour.

**Fix** — Add the `tool_open_threshold` variant and a `tool_call_id` field on `ControlEvent`; track an `attention_emitted` flag per active call; add the open-tool branch to `update_activity_state` after the idle check, with upstream's timeout-exempt rule; key the notification dedupe on the call id. Port the `bash` nudge wording only together with `SUBA-165`.

**Verify** — Two tools open past `activeNoticeAfterMs` in one child give two `needs_attention` notices, one per call, each with its own `toolCallId`; a call that finishes before the threshold gives none; the same call does not notify twice.


## SUBA-165 — Opt-in `subagent_command` supervision (`command.status`, `command.yield`, `command.cancel`, `toolCallId`, bash `yieldTimeMs`) is unported

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — v0.75.0 `72682254`/#2598 adds opt-in command supervision for native Pi children granted both `bash` and `subagent_command`. `createChildCommandRuntime(channelDir)` (`src/runs/shared/child-commands.ts:49`) owns command lifetimes, not shell execution: it wraps the child's `bash` with an optional `yieldTimeMs` (`:119-139`, 0 to 30 000 ms; the command keeps running and a handle is returned), persists `commands.json` in the channel directory and keeps the last 20 commands (`RECENT_COMMANDS`, `:34`). `commandAction` (`src/runs/foreground/command-action.ts:24`) serves `command.status`, `command.yield` and `command.cancel` from the parent, addressed by the new `toolCallId` parameter (`src/extension/schemas.ts:184`); all three are in `SUBAGENT_ACTIONS` (`src/shared/types.ts:2865`). The `tool_open_threshold` nudge for `bash` (`subagent-control.ts:253-259`) tells the parent which command to inspect (`SUBA-164`).

**cyrup** — `SUBAGENT_ACTIONS` (`extension/tool/text.rs:265` onward) has no `command.*` verbs. `grep -rn 'yield_time\|yieldTime' crates --include=*.rs` is empty, so a child's `bash` cannot yield. The control inbox carries one request type, `interrupt` (`background/control.rs:378`, `InterruptRequest`), and its presence is the whole state.

**Impact** — The parent cannot cancel or yield one runaway command; the only lever is interrupting the whole run, which upstream's own hint says is run-scoped and may affect siblings. Low because it is opt-in upstream and needs both tools granted. Upstream builds it on the in-process child (`SUBA-142` family); a spawned child needs the wrapper on its own side.

**Fix** — A child-side `bash` wrapper, a `command` request type on the control inbox next to `interrupt`, and three actions gated like `stop`/`steer`. This needs a bash yield seam in area 04 (built-in tools), which is the larger half and the reason for `L`.

**Verify** — A yielded command returns a handle and keeps running; `command.status` reports it; `command.cancel` stops only that call and the run continues.


## SUBA-166 — Any invalid config value makes cyrup load all-defaults with only a stderr warning, dropping `authorityPolicy` and `permissions`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `FAIL_CLOSED_CONFIG_KEYS` (`src/extension/config.ts:17` @v0.75.0) is `worktreeProvider`, `worktreeBranchPrefix`, `modelResponseAliases`, `modelExclusions`, `checkpointBeforeDeadlineMs`, `disabledFeatures`, `scheduledRuns`, `toolActivation`, `authorityPolicy`, `permissions`, `toolBudget`; v0.74.0 had the first eight. `loadConfig` (`:226`) catches a validation failure, re-reads the file, and rethrows if it contains any of those keys (`:234`); only a file with none of them falls back to `{}` with a logged error. The comment above the list: policies "must not be silently discarded and replaced by the built-in defaults after validation fails". The v0.75.0 changelog states the failure it fixes: an invalid value for any key "silently drops `authorityPolicy`, `permissions`, or `toolBudget`".

**cyrup** — `crates/cyrup/src/subagent_config.rs:68-73` runs `SubagentExtensionConfig::validate_raw_config` (`registration/mod.rs:855-865`) and, on any `Err`, prints `cyrup: warning: ... is invalid (...); using defaults` and returns the all-defaults config. The typed-parse error arm does the same (`:91-97`). There is no list of keys whose presence turns a failure into a hard error. `validate_authority_policy` exists (`registration/mod.rs:702`) and so a malformed `authorityPolicy` is caught, but its consequence is the defaults. `permissions` is carried raw and validated at use, not at load (`registration/mod.rs:526-536`); `toolBudget` is an unported key (`UNPORTED_CONFIG_KEYS`, `:576`).

**Impact** — The stderr warning does exist, but the whole file is discarded. A typo in `artifactDir` in a `config.json` that also sets `{"authorityPolicy":{"stopRun":"forbid"}}` or permission rules lifts those restrictions for the session, and a one-line stderr message at startup is the only trace. Medium because it fails open on a policy surface.

**Fix** — Port the 11-key list. When `validate_raw_config` or the typed parse fails and the raw object contains any listed key, abort the load with the same message (a hard error naming the path) instead of defaulting. Unported keys in the list (`toolBudget`, `disabledFeatures`, `toolActivation`) still count as present, so the list is complete before they are ported.

**Verify** — A bad `artifactDir` together with `authorityPolicy` makes the load fail with the path; a bad `artifactDir` alone still defaults with the warning; a bad value beside `permissions` or `toolBudget` fails.


## SUBA-167 — Claude Code adapters accept no per-launch model or thinking level (`--model`, `--effort`)

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `resolveClaudeCodeOverride` (`src/runs/shared/claude-code-adapter.ts:132-172` @v0.75.0) splits a known thinking suffix off `model` (`claude-opus-5.5:high` sets both, `:high` sets only the effort), validates the model against `CLAUDE_CODE_MODEL_PATTERN`, checks the level against the thinking ceiling, and returns `--model X` and `--effort Y`. `CLAUDE_CODE_EFFORT_BY_THINKING` (`:58-66`) maps `off` to no flag, `minimal` and `low` to `low`, and `medium`, `high`, `xhigh`, `max` to themselves. The tokens are appended after the fixed argv (`resolveClaudeCodeLaunch`'s `overrideArgs`, `:233,263`), so the `--version`/`--help` probes never see them. `assertClaudeCodeModelScope` (`:180`) fails closed under an enforced `modelScope`. Frontmatter `model` and `thinking` are exempt from the external-runner Pi-only-field refusal for these adapters (`src/agents/agents.ts:2091-2093`). A new preflight code `model_scope` reports a scope failure. An unknown model or level fails the launch and names it.

**cyrup** — `launch_args` (`exec/external_cli/adapters/claude_code.rs:75-101`) builds a fixed argv with no model or effort. `PI_ONLY_FIELDS` (`runner/mod.rs:190-207`) lists `model` and `thinking` among the fields an external runner refuses. `exec/external_cli/mod.rs:402-403` sets `model: None` for an external-runner result (comment: "Upstream resolves no model for an external runner at all"); whether a Claude Code result should now carry the override model was not checked.

**Impact** — The Claude model and effort for a `claude-code` child can only be set in the user's global Claude Code settings, never per agent or per launch. Low.

**Fix** — Port `resolveClaudeCodeOverride`, the effort map, the thinking-ceiling and `modelScope` checks and the `model_scope` preflight code; let `PI_ONLY_FIELDS` skip `model`/`thinking` for the two Claude Code adapters, as `agents.ts:2091-2093` does; append the override tokens after the fixed argv.

**Verify** — A fake `claude` records `--model X --effort high` for `model: "X:high"`; a bare `:high` sets only `--effort high`; `off` passes no effort flag; an unknown level fails the launch; under an enforced `modelScope` an out-of-scope model is refused.


## SUBA-168 — `schedule.create` still refuses `missionId`: schema-version-2 mission-bound schedules are unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `ScheduleTarget` gains `missionId?` (`scheduled-runs.ts:45`); the record's `schemaVersion` is `1 | 2` (`:48,313`) and version 2 is required exactly when `target.missionId` is present (`:326`). `schedule.create` validates the id (`:456-459`), checks the mission exists with `readMission` (`:640-641`) and stores `schemaVersion: 1` or `2` accordingly (`:661`); `schedule.list` and `schedule.show` print the mission (`:685,690`). At fire time `executionParams` omits `mission: false` when the schedule is bound (`:476`), so the workflow's `state` persists across fires. The changelog: other schedules keep the old format, and older pi-subagents versions reject version 2 "instead of silently dropping the mission".

**cyrup** — `schedule.create` returns `Mission attachment is deferred from this first schedule slice.` (`background/scheduled_runs/tool.rs:468-478`, message at `:475`) for `missionId`, `mission` and the other mission params. The record codec accepts only `SCHEDULE_VERSION` (1): the serializer writes the constant (`schedule.rs:57`) and `parse_schedule` rejects anything else (`:618`).

**Impact** — A scheduled workflow cannot keep `state` across fires. Low.

**Fix** — Add `mission_id` to the target; write version 2 exactly when it is set and make the reader accept both versions with upstream's equivalence check; validate with `read_mission` at create; skip `mission: false` at fire when bound; show the mission in `list` and `show`.

**Verify** — A version-2 record round-trips; a version-1 reader rejects version 2; a missing mission fails at create; a bound schedule's workflow sees the same mission state on the next fire.


## SUBA-169 — One unreadable goal mission aborts continuation notices for every healthy goal mission

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `collectGoalContinuationNotices` (`src/missions/goal-driver.ts:127` @v0.75.0) iterates `listMissions(...).records` and wraps the per-mission refresh, budget update and notice build in `try { ... } catch (error) { onError(listed.id, error); }`, with a default `onError` that logs `Failed to evaluate goal mission <id>`. `readLinkedRun` (`:30-40`) wraps a malformed `status.json` parse in `Failed to read linked run status '<path>': ...`. The changelog: one mission with a damaged run status or state no longer stops notices for healthy goal missions, and the damaged mission is reported separately.

**cyrup** — `collect_goal_continuation_notices` (`missions/goal_driver.rs:486-554`) calls `read_mission(...)?`, `refresh_goal_mission(...)?`, `update_mission(...)?` and `next_ready_action(...)?` inside the loop, so the first failing mission returns `Err` for the whole call. The caller (`extension/executor/notices.rs:735-745`) logs `Failed to evaluate goal missions` and returns `0`. `read_linked_run` (`goal_driver.rs:125-146`) maps a JSON parse failure to `MissionError::invalid(err.to_string())` without the status path. (The sibling `#2605`, notices after an auto-drain failure, is already matched: `extension/host/native_impl.rs:690-712` logs the drain failure and still calls `raise_goal_continuation_notices`.)

**Impact** — One damaged mission silences every goal notice for the session. Low.

**Fix** — Evaluate each mission under a `match`: on error log with the mission id and `continue`. Wrap the status parse error with its path, as upstream does.

**Verify** — A healthy and a corrupt goal mission in one store yield one notice plus one report.


## SUBA-170 — The global mission list shows the index entry, not the mission record

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `listGlobalMissions(globalIndexDir)` (`src/missions/store.ts:554` @v0.75.0) parses each index entry, then reads and parses the record it points at, checks the id and builds a projection `{ ...entry, title: record.title, status: record.status, updatedAt: record.updatedAt }` with `lastRunId` from the record's last run (`:565-575`). The changelog: a mission no longer looks out of date because its index entry is stale.

**cyrup** — `list_global_missions` (`missions/store.rs:1372`) reads the record only to produce a verdict (`parse_mission_record` plus the id check, `:1424-1435`) and pushes `GlobalMissionIndexRecord { entry, stale, stale_reason }` with the index `entry` as read (`:1437-1446`). The list is then sorted by the entry's `updated_at` (`:1449`), so a lagging pointer also misorders it.

**Impact** — A mission shows an old status, title or last run when its index pointer lags the record. Low.

**Fix** — On a readable, id-matching record overlay the four fields (and drop `last_run_id` when the record has no runs, as upstream does); do not rewrite the pointer. Sort after the overlay.

**Verify** — A stale pointer lists the record's status and title; a record with no runs lists no `lastRunId`.


## SUBA-171 — Settings save replaces a symlinked or restricted `settings.json` with a default-mode regular file

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `writeSettingsFile` (`src/agents/agents.ts:947`) first resolves the write target (`resolveSettingsWriteTarget`, `:977`: `realpath`, or the link text of a dangling link when its physical parent exists), stats the target for its mode, `accessSync(W_OK)`, and passes the existing mode (plus owner write) to the atomic writer, `chmod`ing the temp to the existing mode before the rename. #2627 is the atomicity change; `SUBA-029` covers the atomic half (cyrup already writes temp then rename). The changelog: "if the save is interrupted, the previous settings stay readable".

**cyrup** — `write_settings_file` (`discovery/settings_write.rs:102-111`) serialises and calls `cyrup_config::lock::write_atomic(path, bytes, false)`. `write_atomic` (`cyrup-config/src/lock.rs:337-377`) opens `<name>.tmp.<pid>` in the target's parent with `create(true)` and no mode (the umask default when `secret` is false), `sync_all`s it and `std::fs::rename`s it over `path` itself. It never resolves a symlink, never stats the old mode and never checks writability.

**Impact** — Saving through a dotfile-managed symlink replaces the link with a plain file; a `0600` settings file becomes `0644` (umask-dependent); a read-only file is overwritten. Low, and only reachable through the builtin-override save path.

**Fix** — Resolve the target first (follow symlinks, as `resolveSettingsWriteTarget` does), stat its mode, check `W_OK`, and give the temp the existing mode before the rename. `write_atomic` is shared with other writers, so add a variant or parameter rather than changing its defaults.

**Verify** — Saving through a symlink updates the link's target and leaves the link in place; a `0600` file stays `0600`; a read-only file is refused.


## SUBA-172 — `run-history.jsonl` has the pre-hardening shape, and foreground runs are never recorded

**Kind** stale-port · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `RunEntry` carries `taskHash` and `outcome` (`run-history.ts:12,15`); `recordRun(agent, task, exitCode, durationMs, terminal)` (`:223`) derives `outcome` (`stopped`, `interrupted`, `timed_out`, `stopped` on an unexplained process signal, else `completed` or `failed`), writes `task: "[redacted]"` plus `taskHash: hashTask(task)` (`:24,243`), and uses private modes (`PRIVATE_DIR_MODE = 0o700`, `PRIVATE_FILE_MODE = 0o600`, `:22-23`). `loadRunsForAgent` (`:262`) hardens the storage, sanitises lines and rotates past a threshold. The foreground executor calls it (`src/runs/foreground/subagent-executor.ts:4357,4382`). v0.75.0 adds `planBackgroundRunHistory` (`:180`): one row per launched step, a paused run recorded as `interrupted`, and a step that finished before a sibling was interrupted keeping its own outcome instead of the run-wide flags. The changelog: before, only foreground runs reached the file, so per-agent lookups missed every background launch.

**cyrup** — `record_run_history` (`background/run_history.rs:79`, core `record_run_history_at` `:86-135`) appends one `RunHistoryEntry { agent, task, ts, status, duration, exit }` per top-level result: `task` is `result.task.chars().take(200)` in plaintext, `duration` is the run-wide duration, the file is opened `create(true).append(true)` with the umask default, and there is no hash, `outcome`, hardening or rotation. Its only caller is the async runner's finish path (`background/runner_main/finish.rs:523`); `grep` finds no other writer of `run-history.jsonl` (the foreground executor's `persist_foreground_run_history_for` is a different file, `extension/executor/foreground_history/persist.rs`). Both halves are therefore behind: the entry shape and the foreground path. Its rows are per top-level result, so a chain or parallel run is one row.

**Impact** — Prompt text lands in a default-mode file in the clear, and `run-history.jsonl` lacks foreground runs. Low.

**Fix** — Port `RunEntry` with `taskHash` and `outcome`, the redacted task, `0600`/`0700` creation and rotation on load; port `planBackgroundRunHistory` so the async runner records one row per launched step with a per-step outcome; call a `recordRun` equivalent from the foreground path.

**Verify** — A row has `task: "[redacted]"`, a `taskHash`, an `outcome` and mode `0600`; a fail-fast chain has no row for the skipped sibling; a foreground single run appears in the file.


## SUBA-173 — A saved profile's `machine` string is not validated before `/subagents-load-profile` writes settings

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `validateSubagentProfile(filePath, parsed)` (`src/profiles/profiles.ts:125`) runs `override.machine = validateOptionalMachine(override.machine, "Profile '<file>' has invalid machine for '<name>'")` for every override (`:148`); `validateOptionalMachine` (`src/agents/agents.ts:1035`) is exported for the purpose. The changelog: a profile with an invalid `machine` is rejected "when loaded or checked, before any settings are written"; `machine: false` still clears a pin.

**cyrup** — `load_profile` (`registration/profiles.rs:211`) is `serde_json::from_str::<NamedProfile>` and nothing else; `apply_profile_to_settings_file` (`:554`) merges the profile's `subagents` block into settings and carries a string `machine` through (`:606-622`) without checking it. The function that would check it exists: `validate_optional_machine` (`placement/resolve.rs:33`: non-empty string or `false`, at most 128 characters, no control characters). Not read end to end: the slash command's own pre-checks.

**Impact** — A hand-edited profile with a blank or oversized `machine` is accepted and written to settings, and the failure shows up later when the agent is loaded. Low.

**Fix** — Call `validate_optional_machine` per override in `load_profile` (and in any "check" path) with upstream's label, before `apply_profile_to_settings_file` writes.

**Verify** — A blank `machine` is rejected with the settings file untouched; `machine: false` still clears the pin; a valid name is applied.


## Items in this window held by `09a`

- **`SUBA-106`** (low, `09a`, agent frontmatter `outputSchema`) was **re-read 2026-09-24 at
  `ea23ca2`: still open.** `exec/agent_config.rs::AgentConfig` has no schema field. The only
  `outputSchema` readers under `discovery/` are chain-step parsers (`discovery/types.rs:1455-1458`,
  `discovery/management/config_parse.rs:455-460`). Upstream still parses it at v0.71.0
  (`src/agents/agents.ts:2220-2224`, `assertJsonSchemaObject`, emitted at `:2273`). `09a`'s row cites
  `:2152-2157` **@v0.68.0**, and that citation is correct at that tag. The line moved because the
  file grew. The row stays in `09a`.

## Leads — `v0.67.0..v0.71.0` — RESOLVED 2026-09-24 (pass 2)

*The first pass listed these as unverified. Each was read on both sides by pass 2; the list below is
now a disposition record, not a lead list.*

| lead (first-pass wording) | disposition | evidence |
|---|---|---|
| Typed gates (0.69.0) | **promoted → `SUBA-129`** | `workflows/scripted/engine.rs:1506-1511` string-only; `acceptance.ts:172-236,1259-1290` @v0.71.0 |
| `details.workflowTerminalProof` (0.71.0) | **struck — subsumed** | upstream emits it for ASYNC workflows only (CHANGELOG 0.71.0; `5f326611`); cyrup refuses async workflows (`extension/tool/routing.rs:587`), which is `09`'s lead at `09:192`. Re-open with that lead |
| RPC `cost` / `ping.capabilities.cost` | **promoted → `SUBA-138`** | `extension/rpc/mod.rs:108-118` has eight methods; `rpc.ts:35,770` @v0.71.0. **2026-09-28:** nine now (`SUBA-138` closed) |
| `subagents_enable` lazy loader | **promoted → `SUBA-139`** | cyrup behaves as upstream's unsupported-host fallback (`tool-activation.ts:22-27` @v0.71.0) |
| Required child extensions host API (0.68.0) | **struck — subsumed by `09`'s `SUBA-022`** | upstream's API registers JS module PATHS a child must import (`shared/required-child-extensions.ts:1-60` @v0.71.0); cyrup has no JS-module child extensions, and `background/recovery_descriptor.rs:34-38` already lists `requiredExtensions` as a field cyrup cannot carry. It is a leaf of the typed extension API `SUBA-022` owns **FOLD-IN 2026-10-03 (v0.75.0):** `552f1dd7`/#2643 `requireForAllRunners` (`src/shared/required-child-extensions.ts:20,24-25` @v0.75.0: reject launches whose runner or machine placement cannot load the required extensions instead of dropping them) and `dbb92db1`/#2640 (a required extension that throws during `session_start` stops the launch) are further leaves of the same JS-module child-extension API owned by `09`'s `SUBA-022`; no separate row. |
| `agentExcludeDirs` / `defaultSubagentOnlyExtensions` | **promoted → `SUBA-123`**, with `agentScanDirs` | the first pass's "a warning, so neither is silent" was **wrong**: the census covers only `agentOverrides.<name>` (`discovery/mod.rs:1179-1203`) |
| `PI_SUBAGENT_CACHE_RETENTION` | **promoted → `SUBA-140`** | `shared/child-cache-retention.ts:22` @v0.71.0 |
| Child session naming via `intercom:session-identity` | **promoted → `SUBA-134`** (with the base naming) | `55b79828`; area 11 owns the intercom half |
| Structured-output error summaries / keep valid output | **promoted → `SUBA-127`** | `exec/mod.rs:1633`; `structured-output.ts:62-80`, `execution.ts:1449-1471` @v0.71.0 |
| Hidden skills (`disable-model-invocation`) | **promoted → `SUBA-125`** | `discovery/skills.rs:156-190`; `skills.ts:712` @v0.71.0 |
| Duplicate completion notices (#2389) | **struck — not applicable by construction** | upstream's fix (`9b8356d4`) dedupes sends when the npm package is loaded twice into one session; `cyrup-ext-subagents` is a native built-in registered once by the host. *Falsified if* a session can register this native extension twice |
| MCP direct-tool names vs pi-mcp-adapter (#2395) | **struck — the invariant holds against cyrup's own adapter** | `exec/mcp_direct_tools.rs::format_tool_name` (`:1315-1330`) is written as the twin of `cyrup_mcp::registration::format_tool_name`; whether cyrup-mcp's names match pi-mcp-adapter's is area 13's question |
| External-CLI logs in Fleet (#2375) | **promoted → `SUBA-141`** | `background/fleet_view.rs:1088-1106`; `fleet-view.ts:584-605` @v0.71.0 |
| Usage reconciliation (0.70.0) | **parked under tracker `SUBA-142`** | reconciles from the in-process session's `messages` (`execution.ts:1320-1325` @v0.71.0), which a spawned child does not expose |
| Retained agent resuming despite its own allowlist (#2379) | **struck — subsumed by `SUBA-111`** | the exception exists only once `allowedAgents` does; cyrup has none (`SUBA-111`) |
| Removed `readonly-model-continuation.ts` | **struck — never ported** | `exec/fallback.rs:1336-1338` lists `ReadonlyContinuation` as UNPORTED; that comment is now stale and should go with `SUBA-109`'s fix |
| Removed `completion-evidence.ts` | **struck — covered by `SUBA-107`** | its plan wrapped the guard `SUBA-107` removes; cyrup's `exec/completion_guard.rs` / `exec/mod.rs:1489` go with that item |
| Removed `readonly-session-evidence.ts` | **struck — never ported** | zero hits for `readonly_session\|ReadonlySession` in the crate |

**Negative results from pass 2's line-by-line read (read, nothing owed, recorded so no pass re-derives them):**
`chain-append`'s `flatSteps.push` fix (`subagent-runner.ts:2519`) — cyrup derives the flat layout as a
pure function of the current step list (`background/flat_index.rs:1-35`), so there is no cached flat
list to go stale after `turn_loop.rs::append_steps`; the `httpIdleTimeoutMs` runner
dispatcher (`1a7101e6`, `runner-http-dispatcher.ts`) — a cyrup runner child is a full `cyrup` process
reading its own settings, so there is no bare dispatcher to configure; the runner startup handshake
rewrite (`ack`/`confirm`/`proceed`, `085d8605`, `9003911d`) — Node-specific, cyrup's detached runner
has its own lease/startup path (`background/session_lease/`); the `requestedModel` field and removal
of `attemptedModels`/`modelAttempts` (`2b64ced0`, `f58dfcb5`) — part of `SUBA-109`'s ladder removal;
authority `inspectorOpen`/`projectOpen` (#2269) — ported (`registration/authority.rs:54-94`); the
home-directory-is-not-a-project rule (#2214, second half) — ported (`discovery/mod.rs:186-230`).

**Read on one side only, left as ownerless leads (outside this file's scope):** the top-level `gate`
tool parameter exists at v0.43.0 (`src/extension/schemas.ts`, one `gate:` property at v0.43.0,
v0.47.1, v0.57.0) and is absent from cyrup's tool schema (`extension/tool/schema.rs` has `gate` only
as a lane `mode`), so it belongs to `09`'s scope; foreground prompt audit (`runs/foreground/prompt-audit.ts`,
present from v0.57.0) has no cyrup module and sits in `09a`'s window. Neither was compared further.

## Post-tag leads (pi-subagents `v0.71.0..6f1027f7`)

README cites upstream only at tags, so these are leads, not items. All ten commits were read.

- `965dd4b8` *fix(supervisor): show question text in pending requests* — applies: cyrup's
  `native_supervisor.rs::format_pending_line` (`:738-752`) prints the header only.
- `5794f52d` *fix(fast): accept any native openai-codex model* — applies: `exec/spawn_plan.rs:207-212`
  still pins `FAST_MODE_ALLOWED_MODELS` to two models.
- `d44b514e` *fix(worktree): keep UTF-8 naming labels within byte cap* — no counterpart found: cyrup's
  `spawn/worktree.rs` has no byte-capped naming label. Not compared further.
- `ffeec95c` *fix(external-job): service bridges of child-launched runs* — cyrup's external-job bridge
  servicing was not traced; lead.
- `d8591e80` *fix: pair awaited workflow child lifecycle events* — async-workflow event surface; see the
  async-workflow refusal above.
- `4b28f863` *fix(status): report the child's actual context limit* — in-process `created.contextWindow`;
  parked with `SUBA-142`.
- `d74dc57c` (package metadata), `68cea36f`, `255f66a0`, `6f1027f7` (tests only) — nothing to port.

### Settled by this pass — negative results about the 2026-09-14 census leads

- **`09a` census, *"Watchdog bounded model-fallback chains (`fallbackModels`)"*: DEAD.** Upstream
  removed watchdog fallback at v0.68.0, and `src/watchdog/child-status.ts:131-132` now **refuses**
  the key. `grep -rln fallback_models crates/cyrup-ext-subagents/src/watchdog` is empty, so cyrup
  never ported it. Nothing is owed. Do not re-file it.
- **`09a` census, *"In-process pi child sessions replace the spawned-child-process model"*:
  STILL OPEN AS A LEAD** *(pass 2: promoted to tracker `SUBA-142`)*. At v0.71.0, upstream still builds children in-process
  (`src/runs/shared/child-launch.ts::buildInProcessChildLaunch`). cyrup still spawns subprocesses.
  The design question that lead raises has not been answered. One consequence that is now visible:
  `SUBA-110`'s scrub only applies where upstream still spawns a process. Upstream's in-process
  foreground children see the parent's environment, so cyrup's subprocess foreground child inheriting
  it is *not* a divergence.

## Blind spots — read before the next pass

*Updated by pass 2 (2026-09-24). The first pass's items 1, 2 and 4 are rewritten below; the
originals are in `git log -p` of this file.*

1. **`v0.57.0..v0.67.0` has had its leads resolved, not its diff read.** Every census lead in `09a`
   and here now has a disposition, but the 207-file `src/` diff was not walked line by line. The
   crate's `@v0.68.0` citations close much of it; a modified-file behaviour change that no lead names
   is still invisible. This is the next pass's job.
2. **`v0.67.0..v0.71.0`:** the five largest modified files were read line by line (runner, executor,
   execution in full; async-execution and agents with the Herdr/ladder hunks filtered out); every
   other `src/` file over 100 changed lines was read commit by commit. Node-runtime launch plumbing
   (`jiti`, Bun, native TypeScript hooks) was read and deliberately not compared.
3. **Nothing was observed live.** Every mechanism is a static reading, per README *Where this analysis
   is blind* §2. `SUBA-114`, `SUBA-115` and `SUBA-117` are the three worth a live repro first.
4. **`v0.71.0..HEAD`** (10 commits) was read as post-tag leads (`## Post-tag leads`); none is an item
   until a tag cuts it.
