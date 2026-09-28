# 09b — cyrup-ext-subagents: the v0.57.0 → v0.71.0 drift window

**This file adds to `09-cyrup-ext-subagents.md` and `09a-cyrup-ext-subagents-v0.57-drift.md`. It
replaces neither of them.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071` and its
Trackers. `09a` remains the file of record for `SUBA-072`…`SUBA-106`, including the ids it filed past
its own scope line, which stay where they are. This file adds **`SUBA-107`…`SUBA-113`** (first pass)
and **`SUBA-114`…`SUBA-143`** (pass 2, 2026-09-24); the next free id is **`SUBA-144`**. Ids are
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
> none. **Next free id: `SUBA-144`.** `SUBA-113`'s tracker escalated two of its keys
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
| SUBA-107 | medium | stale-port | M | The completion-mutation guard still fails a successful run because its task wording "looked like" an implementation task. Upstream deleted the guard at v0.70.1 |
| SUBA-108 | medium | stale-port | M | Acceptance-level inference still reads task wording and agent-name regexes. At v0.70.1 upstream infers only from the declared `acceptanceRole` |
| SUBA-111 | medium | upstream-drift | M | Agent `allowedAgents` (frontmatter and `agentOverrides`, v0.70.0) is unported. The frontmatter key is silently kept as an extra field, so a declared delegation restriction fails open |
| SUBA-109 | low | stale-port | M | `fallbackModels`, the same-launch model ladder and persistent model exclusions are still live. Since v0.68.0 upstream rejects the key by name |
| SUBA-112 | low | upstream-drift | S | The bundled `worker` still has `defaultContext: fork` (upstream: `fresh`, v0.71.0) and has no `acceptanceRole: writer` (v0.70.1) |
| SUBA-113 | tracker | tracker | — | Thirteen `config.json` keys are declared unported in source (`registration/mod.rs::UNPORTED_CONFIG_KEYS`), which says "the ledger carries them". No ledger item did until this row |
| ~~SUBA-114~~ | ~~high~~ **CLOSED 2026-09-26** | stale-port | M | Child tool plans are still pruned to the PARENT session's tool registry, and reviewer/scout launches are refused when the parent was started with a narrow `--tools`. Upstream removed the prediction at v0.70.0 — **CLOSED 2026-09-26** (`850eb70`, #156): `b12496b8` ported — `exec/tool_surface.rs` has no parent-registry input (`host_builtin_tool_names`, the partition, the host `read` throw, the review/scout lane refusal, the omission warning and `unavailableHostBuiltins`/`warnings` are gone), and `RunOptions`/`RunnerConfig`/`ExecSingleStepExecutor` no longer carry `host_available_builtins` (an old runner config carrying the key still decodes). The child-side guard is brought to v0.71.0 (#1356): no builtin floor, and the child ABORTS its run at `agent_start` when its real registry lacks a required tool (`prompt_runtime::refresh_tool_diagnostic`). Verify: `cyrup-it --test subagents child_tool_plan_not_predicted_from_parent::*` (5, real parent `AgentSession`). See the section. |
| ~~SUBA-115~~ | ~~high~~ **CLOSED 2026-09-24** | upstream-drift | S | A nested run's stop / interrupt / timeout cascade reaches every live run on the ROOT route, including sibling subtrees it did not launch (v0.68.0 confines it to the issuing subtree) — **CLOSED 2026-09-24** (`7ba9e03`): `background/cascade.rs::is_control_descendant` ports `isNestedControlDescendant`; `settle.rs::cascade_to_descendants` passes this run's id as issuer when `nested_self` is set, for stop, interrupt and timeout. Verify: `background::cascade::tests::a_nested_issuer_reaches_only_its_own_subtree` (red with the filter removed). Reach note: cyrup does not yet mint a nested route for its own children (`exec/spawn_plan.rs:993`), so today the cascade only has a route when cyrup was itself launched under an inherited one. |
| SUBA-116 | medium | upstream-drift | M | A paused async run cannot be stopped (refused as "No running or queued async run"), so it keeps its active-capacity slot until resumed |
| ~~SUBA-117~~ | ~~medium~~ **CLOSED 2026-09-27** | parity-bug | S | The worktree clean-tree check does not exclude the crate's own `.cyrup-subagents/` project directory, so the crate's own chain-run / refinement / schedule files make `worktree: true` refuse a clean repository — **CLOSED 2026-09-27**: the clean-tree probe excludes the crate's own project artifact root (`crates/cyrup-ext-subagents/src/spawn/worktree.rs:495-498`, `:491-493` recording the pathspec resolution). Verify: `cyrup-ext-subagents spawn::worktree::tests::create_worktrees_ignores_the_project_artifact_root`. |
| SUBA-118 | medium | upstream-drift | M | Abort recovery is unported: a child whose provider/transport aborted after compaction settled, with useful progress, fails instead of being resumed once |
| SUBA-119 | medium | not-ported | M | A native child that reports a different model than the launch candidate is accepted silently: no `model_verification_failed`, no `modelResponseAliases` |
| ~~SUBA-120~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | Watchdog findings have no `importance`; every finding above threshold is delivered into the parent model's context, where upstream sends only `high` — **CLOSED 2026-09-27**: `importance` is required with no default and `route_warning` is wired; the false claim that upstream's notice path has no importance filter is corrected at `crates/cyrup-ext-subagents/src/watchdog/types.rs:118-148`, and `parse_test_command` now requires a non-empty remainder after ONE separator (`watchdog/register_main.rs:662`). Verify: `cyrup-ext-subagents watchdog::register_main::tests::exactly_one_trailing_space_is_not_a_test_command_but_two_are`. |
| ~~SUBA-121~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Runtime-registered agents never receive `subagents.defaultModel` / `defaultProvider` / `defaultThinking` or `agentOverrides.<name>` model / provider / fast / thinking — **CLOSED 2026-09-27**: the one line that makes `subagents.defaultModel` / `agentOverrides.<name>` reach a runtime-registered agent (`crates/cyrup-ext-subagents/src/discovery/mod.rs:2074-2076`) is now covered end to end by a real discovery integration test. Verify: `cyrup-ext-subagents tests::runtime_agent_registration_integration::settings_defaults_and_narrowed_overrides_reach_a_runtime_agent_through_discovery`, plus the four `discovery::merge::tests` unit tests. |
| ~~SUBA-122~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Agent-name resolution does not prefer a canonical name over a packaged short name, so `scout` beside `code-analysis.scout` is refused as ambiguous — **CLOSED 2026-09-27**: `resolve_agent_name` is split into a canonical pass and a local-name pass, in that order, each with its own ambiguity message (`crates/cyrup-ext-subagents/src/discovery/mod.rs:578-613`). Verify: `cyrup-ext-subagents discovery::tests::{a_canonical_name_beats_another_agents_local_name_of_the_same_string,two_distinct_agents_sharing_a_local_name_are_a_local_ambiguity_error}`. |
| ~~SUBA-123~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Top-level `subagents.agentScanDirs`, `agentExcludeDirs` and `defaultSubagentOnlyExtensions` are silently dropped: the key census walks only `agentOverrides.<name>` — **CLOSED 2026-09-27**: all three keys are parsed, validated with upstream's message shape, trimmed and applied — `agentScanDirs` (tracked as SUBA-123a), `defaultSubagentOnlyExtensions` (SUBA-123b) and `agentExcludeDirs` (SUBA-123c) — and the key census now warns on an unported top-level key (`crates/cyrup-ext-subagents/src/discovery/{types.rs:955-974,mod.rs:962-966,1283-1288,1342-1366,1500-1501},agent_dirs.rs:186-277`). Verify: `cyrup-ext-subagents discovery::agent_scan_and_exclude_dir_tests::*` (6), `discovery::merge::tests::*default_subagent_only_extensions*` (5), `discovery::tests::{an_unknown_top_level_subagents_key_warns,a_ported_top_level_subagents_key_does_not_warn}`. |
| SUBA-124 | medium | upstream-drift | M | `mcpDirectTools` cannot resolve servers contributed by settings `packages` or `agentPluginPaths`; a grant naming one resolves to no tools, silently |
| ~~SUBA-125~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Skills marked `disable-model-invocation: true` are still injected into a child that names them — **CLOSED 2026-09-27**: `disable_model_invocation` is carried onto `ResolvedSkill` and filtered at the injection chokepoint (`crates/cyrup-ext-subagents/src/discovery/skills.rs:78,96-100,158,274,302`), and the proactive recommender skips hidden skills too (`:571`). Verify: `cyrup-ext-subagents discovery::skills::tests::{an_agent_naming_a_hidden_skill_launches_without_it_in_the_child_prompt,build_skill_injection_omits_a_hidden_skill,a_hidden_skill_is_not_proactively_recommended}`. |
| SUBA-143 | medium | upstream-drift | L | The runtime-agent registration EVENT bridge (`pi-subagents:runtime-agent-register:v1`) is unported; only native Rust code can register a runtime agent |
| SUBA-126 | low | not-ported | S | A child that answers only through `structured_output` delivers an empty output and saves an empty output file; upstream substitutes the pretty-printed JSON |
| SUBA-127 | low | upstream-drift | S | Structured output is read only on a clean exit: a rejected call is reported as "Missing structured_output call", and a valid value is discarded when a later provider error fails the run |
| SUBA-128 | low | upstream-drift | S | `checkpointBeforeDeadlineMs` (async single tool param and `config.json` default) is unported, so a deadline kills a child with no handoff steer |
| SUBA-129 | low | upstream-drift | M | Typed gates (`gate: { command, output: "json", schema?, timeoutMs? }`, `verify[].output`/`schema`) and `acceptance.preserveStagedIndex` are refused |
| SUBA-130 | low | upstream-drift | M | Worktree git commands run with no deadline, stop signal or output bound, so a hung `git` outlives the run's deadline and ignores `stop` |
| SUBA-131 | low | upstream-drift | M | An external-CLI step is never flagged `needs_attention` when idle; upstream now tracks its stdout/stderr and Git fingerprint as activity |
| SUBA-132 | low | not-ported | S | `SingleResult` carries no `toolBudgetBlocked`, so a tool-budget-blocked workflow child never settles `budget_exhausted` (self-declared in `workflows/settlement.rs`) |
| SUBA-133 | low | upstream-drift | S | Agent `advertise: true` and the `<advertised_subagents>` catalog in the parent's system prompt are unported |
| SUBA-134 | low | upstream-drift | S | Child sessions get no derived human-readable name (`deriveChildSessionName` → `setSessionName`, `sessionName` in results, the `intercom:session-identity` claim) — **2026-09-27:** the intercom half is in (`ICOM-064`, on `claude/intercom-lows`): answer the `intercom:session-identity` request by emitting `intercom:session-identity-claim` `{version:1, stableId:<routing name>}` before the first `agent_start` |
| SUBA-135 | low | upstream-drift | S | No launch-cwd preflight: a missing or non-directory `cwd` fails at spawn with an OS error instead of upstream's named refusal before launch |
| SUBA-136 | low | upstream-drift | S | No renderer for `subagent_supervisor_request` messages and no `subagent_supervisor_reply` session entry |
| SUBA-137 | low | upstream-drift | S | The Ghostty inspector activates on `TERM_PROGRAM=ghostty` alone; v0.69.0 also requires the macOS host bundle id, so cmux-style embedders stop misfiring |
| SUBA-138 | low | upstream-drift | S | The in-process RPC `cost` method and `ping.capabilities.cost` are unported |
| SUBA-139 | low | upstream-drift | M | The `subagents_enable` lazy loader is unported; the full `subagent` tool is always in the prompt (upstream's unsupported-host fallback) |
| SUBA-140 | low | upstream-drift | S | `PI_SUBAGENT_CACHE_RETENTION` (child-only prompt-cache tier) is unported |
| SUBA-141 | low | upstream-drift | S | Observability drift: external-CLI stdout/stderr tails are not shown in Fleet, and a runner that dies without a result is not reported with its exit code and signal |
| SUBA-142 | tracker | tracker | — | In-process child sessions: upstream builds children in-process; cyrup spawns a process. The design question, plus the in-process-only features parked behind it |

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
worker's missing `acceptanceRole: writer`) has to land with this item.

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

---

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
| RPC `cost` / `ping.capabilities.cost` | **promoted → `SUBA-138`** | `extension/rpc/mod.rs:108-118` has eight methods; `rpc.ts:35,770` @v0.71.0 |
| `subagents_enable` lazy loader | **promoted → `SUBA-139`** | cyrup behaves as upstream's unsupported-host fallback (`tool-activation.ts:22-27` @v0.71.0) |
| Required child extensions host API (0.68.0) | **struck — subsumed by `09`'s `SUBA-022`** | upstream's API registers JS module PATHS a child must import (`shared/required-child-extensions.ts:1-60` @v0.71.0); cyrup has no JS-module child extensions, and `background/recovery_descriptor.rs:34-38` already lists `requiredExtensions` as a field cyrup cannot carry. It is a leaf of the typed extension API `SUBA-022` owns |
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
