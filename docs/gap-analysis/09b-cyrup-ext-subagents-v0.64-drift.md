# 09b — cyrup-ext-subagents: the v0.57.0 → v0.71.0 drift window

**This file adds to `09-cyrup-ext-subagents.md` and `09a-cyrup-ext-subagents-v0.57-drift.md`. It
replaces neither of them.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071` and its
Trackers. `09a` remains the file of record for `SUBA-072`…`SUBA-106`, including the ids it filed past
its own scope line, which stay where they are. This file adds **`SUBA-107`…`SUBA-113`**. Ids are
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

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | **`ea23ca2`** (2026-09-24). Merge of #152; the code commit is `df3e9a8` | `git log -1` |
| pi-subagents | **`v0.71.0`**, the newest tag. Clone HEAD is `v0.71.0-10-g6f1027f7`, and that past-the-tag range is **not read** | `git -C tmp/pi-subagents tag --sort=-v:refname \| head -1`; `git describe --tags` |

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
| `v0.57.0..v0.67.0` | 496 files, +73 195 / −25 830 | 207 files, +24 658 / −9 124 | 341 | 54 / 5 | **leads only** (the `09` and `09a` census blocks, 2026-09-14). One item, `SUBA-092`, is filed in `09a` |
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
| SUBA-110 | high | upstream-drift | S | Git routing variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_CONFIG_*`, …) are not removed from the background runner's environment or from an inherited external-CLI environment |
| SUBA-107 | medium | stale-port | M | The completion-mutation guard still fails a successful run because its task wording "looked like" an implementation task. Upstream deleted the guard at v0.70.1 |
| SUBA-108 | medium | stale-port | M | Acceptance-level inference still reads task wording and agent-name regexes. At v0.70.1 upstream infers only from the declared `acceptanceRole` |
| SUBA-111 | medium | upstream-drift | M | Agent `allowedAgents` (frontmatter and `agentOverrides`, v0.70.0) is unported. The frontmatter key is silently kept as an extra field, so a declared delegation restriction fails open |
| SUBA-109 | low | stale-port | M | `fallbackModels`, the same-launch model ladder and persistent model exclusions are still live. Since v0.68.0 upstream rejects the key by name |
| SUBA-112 | low | upstream-drift | S | The bundled `worker` still has `defaultContext: fork` (upstream: `fresh`, v0.71.0) and has no `acceptanceRole: writer` (v0.70.1) |
| SUBA-113 | tracker | tracker | — | Thirteen `config.json` keys are declared unported in source (`registration/mod.rs::UNPORTED_CONFIG_KEYS`), which says "the ledger carries them". No ledger item did until this row |

---

## SUBA-110 — Git routing variables reach the background runner and inherited external CLIs

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed (both sides read; not observed live)

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

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

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

**Escalates to an item** when a key's feature is shown to change behaviour for a user who sets it.
The likely first candidates are `checkpointBeforeDeadlineMs` (the per-call form was added at v0.68.0
for async single runs) and `worktreeBranchPrefix`. Each needs its own row, with both sides read.

---

## Items in this window held by `09a`

- **`SUBA-106`** (low, `09a`, agent frontmatter `outputSchema`) was **re-read 2026-09-24 at
  `ea23ca2`: still open.** `exec/agent_config.rs::AgentConfig` has no schema field. The only
  `outputSchema` readers under `discovery/` are chain-step parsers (`discovery/types.rs:1455-1458`,
  `discovery/management/config_parse.rs:455-460`). Upstream still parses it at v0.71.0
  (`src/agents/agents.ts:2220-2224`, `assertJsonSchemaObject`, emitted at `:2273`). `09a`'s row cites
  `:2152-2157` **@v0.68.0**, and that citation is correct at that tag. The line moved because the
  file grew. The row stays in `09a`.

## Leads — `v0.67.0..v0.71.0`, UNVERIFIED

**None of these is a finding.** Each lead was noticed in the CHANGELOG or in the `src/` adds and
deletes, and one side was grepped at most. None has an id. Ownership overlaps are marked.

- **Typed gates** (0.69.0): `gate: { command, output: "json", schema?, timeoutMs? }` makes a gate's
  JSON stdout the child's `structuredOutput`. In cyrup, `gate` appears only as a workflow host-step
  kind (`extension/tool/routing_tests.rs`). The typed form was not traced on either side.
- **`details.workflowTerminalProof`** (0.71.0, `src/runs/background/workflow-terminal-proof.ts`, new).
  cyrup has a per-step process-terminal proof (`background/process_terminal/finalize.rs:94`) and
  nothing at workflow level. The capacity-release coupling upstream describes was not read.
- **In-process RPC `cost` method and `ping.capabilities.cost`** (0.71.0, #2378). cyrup has
  `registration/cost.rs` for `/subagent-cost`. An RPC `cost` verb was not found, and the search
  was shallow.
- **`subagents_enable` lazy tool loader** (0.71.0, `src/extension/tool-activation.ts`, new). Might
  not apply to cyrup, which has no dynamic-tool host. Not assessed.
- **Required child extensions host API** (0.68.0, `src/api/required-child-extensions.ts` and
  `src/shared/required-child-extensions.ts`, new). This is the same family as `09`'s `SUBA-022`
  (typed extension API), and a port should design them together.
- **`subagents.agentExcludeDirs`** (0.68.0) and **`subagents.defaultSubagentOnlyExtensions`**
  (0.70.0). Both have zero hits in cyrup. As settings keys they would reach the key census as
  *unknown*, which gives a warning, so neither is silent.
- **`PI_SUBAGENT_CACHE_RETENTION`** (0.68.0, `src/shared/child-cache-retention.ts`): zero hits in
  `cyrup-ext-subagents`. The `CACHE_RETENTION` hits elsewhere in `crates/` are the provider-level
  setting and were not related to this one.
- **Child session naming via `intercom:session-identity`** (0.71.0). Overlaps area 11.
- **Structured-output error summaries** (0.71.0, #2407) and **keeping valid structured output after a
  later provider error** (#2411). `src/runs/shared/structured-output.ts` grew +234 in the window
  (456 L @v0.71.0). Not read.
- **Skills with `disable-model-invocation: true` hidden from children** (0.71.0, #2400).
  `cyrup-resources/src/skill.rs` knows the flag. The child skill-injection path was not traced.
- **Duplicate completion notices when the extension is registered twice** (0.71.0, #2389), **MCP
  direct-tool names matching pi-mcp-adapter prefixes** (#2395, overlaps area 13), **external-CLI logs
  in Fleet** (#2375), **usage reconciliation from terminal session messages** (0.70.0,
  `src/runs/shared/usage-reconciliation.ts`, new), and **a retained agent resuming despite its own
  allowlist** (0.71.0, #2379; interacts with `SUBA-111`). None of these was traced.
- **Removed files not traced:** `readonly-model-continuation.ts`, `completion-evidence.ts` and
  `readonly-session-evidence.ts` were all deleted in the window. Whether cyrup ported any of them was
  not checked. If it did, each is a `stale-port` candidate like `SUBA-107`/`SUBA-109`.

### Settled by this pass — negative results about the 2026-09-14 census leads

- **`09a` census, *"Watchdog bounded model-fallback chains (`fallbackModels`)"*: DEAD.** Upstream
  removed watchdog fallback at v0.68.0, and `src/watchdog/child-status.ts:131-132` now **refuses**
  the key. `grep -rln fallback_models crates/cyrup-ext-subagents/src/watchdog` is empty, so cyrup
  never ported it. Nothing is owed. Do not re-file it.
- **`09a` census, *"In-process pi child sessions replace the spawned-child-process model"*:
  STILL OPEN AS A LEAD.** At v0.71.0, upstream still builds children in-process
  (`src/runs/shared/child-launch.ts::buildInProcessChildLaunch`). cyrup still spawns subprocesses.
  The design question that lead raises has not been answered. One consequence that is now visible:
  `SUBA-110`'s scrub only applies where upstream still spawns a process. Upstream's in-process
  foreground children see the parent's environment, so cyrup's subprocess foreground child inheriting
  it is *not* a divergence.

## Blind spots — read before the next pass

1. **`v0.57.0..v0.67.0` is still leads-only.** This file owns it by scope and did not re-verify it.
   The crate's 840 `@v0.68.0` citations mean many of the 2026-09-14 leads are probably closed by
   the port already. Check each one against the code before carrying it forward.
2. **Only the CHANGELOG and the add/delete lists were used to index `v0.68.0..v0.71.0`.** A behaviour
   change inside a *modified* file that has no changelog line is invisible to this pass. The biggest
   modified files were not read line by line: `subagent-runner.ts` (±1 033),
   `subagent-executor.ts` (±568), `async-execution.ts` (±546), `execution.ts` (±502) and
   `agents.ts` (±387).
3. **Nothing was observed live.** Every mechanism is a static reading, per README *Where this analysis
   is blind* §2.
4. **`v0.71.0..HEAD`** of the clone (10 commits) is unread by construction.
