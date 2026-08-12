# 09 — cyrup-ext-subagents

Covers `cyrup/crates/cyrup-ext-subagents/` (~87k LOC, the largest crate) — subagent discovery, registration, foreground/background execution, chain/parallel orchestration, acceptance gating and the subagent TUI surface — measured against `pi-subagents/` at the ported baseline **v0.34.0** (the crate records no version string; v0.34.0 is the workspace-brief inference, and every upstream claim below was settled with `git show v0.34.0:<path>` rather than clone HEAD, because clone-HEAD line numbers and file existence both mislead here). Headline finding: the recent wave closed three items outright and two halfway, but the spawn cap is still enforced on the tool path only, an acceptance-verify timeout leaks whole detached process trees, nine SINGLE-mode parameters are advertised to the model and hard-rejected at dispatch, and ~3000 lines of pi-faithful acceptance port are unreachable dead code while a cyrup-original lattice runs in its place. Re-baselined against HEAD `1806375` on 2026-08-03; every line reference below was re-read at that commit.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| SUBA-001 | **CLOSED** (7a44aec) | Persona reaches the child as one argv element (`exec/mod.rs:983`), flag chosen by `SystemPromptMode`. Verified from both ends: cyrup's own `--system-prompt` is `Option<String>` with no path resolution (`cyrup/crates/cyrup/src/cli.rs:127-131`), which is what makes the inline-vs-tempfile `[CYRUP-DELTA]` (`:846-860`) load-bearing rather than a rationalization. Three tests pin replace/append/empty-body. Residual deltas moved to SUBA-030. |
| SUBA-002 | **CLOSED** (513e45a) | Cap now charged on every execution entry — slash and chain surfaces included. Was partially closed by 7c3862b (tool path only). |
| SUBA-027 | **CLOSED** (513e45a) | `verify[]` and the worktree hook are killed on timeout via `spawn/signal.rs::send_signal`, not abandoned. The `wait_with_output(self)` race that consumed the `Child` is gone. |
| SUBA-041 | **CLOSED** (513e45a), with residuals | Seven of nine SINGLE params wired; `includeProgress`/`control` were removed from the schema rather than ported — **rejected by the maintainer as a workaround** and reopened as live work in Move 21c. Residuals filed as SUBA-N03/SUBA-N04. |
| SUBA-003 | **CLOSED** (46c3868) | `modelScope` ported *and enforced*: `resolve_model_inheritance` returns `Err(violation)` for an explicit out-of-scope model (`exec/fallback.rs:254-257`), inherited sources warn — pi's severity split at `model-scope.ts:63-77`. Reporting surface only remains, as SUBA-035. |
| SUBA-004 | **CLOSED** (46c3868) | `wait` tool present (`background/wait.rs`, 860 lines), registered in the `Full` arm (`extension.rs:5668-5673`) with pi's exact param surface `id`/`all`/`timeoutMs`. Full-arm-only matches upstream — a fanout child loads `extension/fanout-child.ts`, not `index.ts`. Residuals are SUBA-031 and SUBA-034. |
| SUBA-005 | **PARTIALLY CLOSED** (46c3868) | 15 of 20 management actions land; denylist parity byte-for-byte. Remains open below. |
| SUBA-006 … SUBA-036 | **STILL OPEN** | All 31 re-derived from source on both sides this pass. Every symbol claimed absent is still absent at HEAD; the two non-zero greps (`fleet`, `spawn_budget`) were hand-inspected and are incidental. |
| — | line corrections | SUBA-036: dead runner `acceptance.rs:3366`, its race `:3418`, `model::evaluate_acceptance` `:3499` (the old doc cited `:3394/:3419`). SUBA-023: `TerminationOutcome` is `spawn/signal.rs:66-72`. SUBA-030: `spawn/mod.rs:92` / `:237`. SUBA-001: host CLI `cli.rs:127-131`. |
| — | new | SUBA-037 … SUBA-041 filed below. |

Closed this cycle: **3**; **+3 more (SUBA-002/027/041) by Move 21, `513e45a`**. Nothing previously filed was overturned as misdescribed.

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 7 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~426), with
> `-S` ids — **including 1 rated critical/high**. Enumerating only this table undercounts the
> area by 7 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SUBA-N03 | high | not-ported | L | The seven SINGLE-mode overrides are still refused on the **async/background** branch |
| SUBA-N04 | **high** | parity-bug | S | Background/chain step silently drops `acceptance` — AND `share`, `sessionDir`, `skills`, `includeProgress` |
| SUBA-N05 | medium | parity-bug | S | `chainDir` is advertised in the schema, deserialized, and consumed by nothing on any path |
| SUBA-N06 | high | not-ported | L | `includeProgress` and `control` are de-advertised AND refused on both paths — untracked residual of SUBA-041 |
| SUBA-005 | medium | not-ported | L | Management actions: 5 of 20 still missing |
| SUBA-036 | medium | stale-port | M | The pi-faithful acceptance model port (~3000 lines) is unreachable |
| SUBA-028 | medium | parity-bug | S | Acceptance verification cannot be aborted |
| SUBA-030 | medium | parity-bug | S | Persona passed inline on argv: no E2BIG guard, world-readable via /proc |
| SUBA-031 | medium | parity-bug | M | `wait` scopes runs by cwd, not by session |
| SUBA-032 | medium | test-defect | S | Notice-debounce test asserts a wall-clock outcome with ~15 ms margin |
| SUBA-006 | medium | upstream-drift | S | No `--tools` for MCP-only/empty allowlist; no `--no-tools` hardening |
| SUBA-007 | medium | not-ported | M | `toolBudget` entirely unported |
| SUBA-008 | medium | not-ported | M | `turnBudget` entirely unported |
| SUBA-010 | medium | not-ported | M | Native supervisor channel unported; still on the broker relay |
| SUBA-012 | medium | not-ported | M | `fleet-view` / `/subagents-fleet` / `status view` unported |
| SUBA-013 | medium | not-ported | L | File-based live steer inbox unported (+ the `steer` action) |
| SUBA-014 | medium | not-ported | S | `requireReadTool` unported |
| SUBA-015 | medium | not-ported | M | Per-agent persistent memory unported |
| SUBA-016 | medium | not-ported | L | `scheduledRuns` unported (+ the four `schedule*` actions) |
| SUBA-018 | medium | not-ported | L | Prompt-template delegation bridge and prompt-workflow commands unported |
| SUBA-019 | medium | upstream-drift | S | Frontmatter parser lacks YAML block lists and folded scalars |
| SUBA-020 | medium | upstream-drift | S | Model fallback retries whole task on child tool failure |
| SUBA-009 | low | stale-port | S | Still ports `companion-suggestions.ts`, deleted before the baseline |
| SUBA-011 | low | upstream-drift | L | Whole `src/watchdog/` subsystem absent |
| SUBA-017 | low | not-ported | M | Completion batching unported |
| SUBA-021 | low | upstream-drift | L | Launch-contract / preflight / capability-ceiling / spawn+usage budgets unported |
| SUBA-022 | low | upstream-drift | L | Typed extension delegation API (v1 + v2) unported |
| SUBA-023 | low | upstream-drift | L | Async lifecycle hardening unported |
| SUBA-024 | low | upstream-drift | L | Post-baseline chain/parallel orchestration features unported |
| SUBA-025 | low | not-ported | S | `toolDescriptionMode` and description override unported |
| SUBA-026 | low | upstream-drift | L | Interactive admin UI, selector, `/subagents`, `/subagents-stop` unported |
| SUBA-029 | low | cyrup-original | S | Management actions read-modify-write subagents `settings.json` unlocked |
| SUBA-033 | low | test-defect | S | Tests assert a lower bound on observed concurrency |
| SUBA-034 | low | not-ported | M | `wait`'s event-bus wake unported; pure polling at a 1 s floor |
| SUBA-035 | low | not-ported | S | Active `subagents.modelScope` policy not surfaced by doctor/models |
| SUBA-037 | low | cyrup-original | S | Doctor's `--version` binary probe leaks the probe process on timeout |
| SUBA-038 | low | parity-bug | S | Child-safe / unknown-action denial messages do not carry pi's exact text |
| SUBA-039 | low | cyrup-original | M | `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a group |
| SUBA-040 | low | test-defect | S | Verify-timeout test passes with SUBA-027's leak in place and leaks a real `sleep 5` |

## SUBA-027 — Acceptance `verify[]` commands and the worktree setup hook abandoned on timeout, never killed

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/acceptance.rs:493-556`: `run_one_verify_command` sets `cmd.process_group(0)` (`:508`), spawns (`:510`), then races `tokio::time::timeout(timeout, child.wait_with_output())` (`:524`). `wait_with_output(self)` consumes the `Child` into the future, so the `Err(_elapsed)` arm (`:545-555`) drops the only handle; `kill_on_drop` is never set (exactly one in the crate, `extension.rs:7116`, unrelated). The message literally reads "exceeded its {}ms timeout and was abandoned" (`:551`). Commands are arbitrary `/bin/sh -c` strings (`:558-571`). Live path: `exec/mod.rs:2212` → `evaluate_acceptance` (`:647`) → `run_verify_commands` (`:679`) whenever `contract.required_level >= AcceptanceStatus::Verified`. Second site: `spawn/worktree.rs:633-701` owns the `Child` inside an `async` block and drops the whole future at `:695-701`.
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts:742-758` @v0.34.0: `abortVerification` sends `child.kill("SIGTERM")` then arms a 1 s hard `SIGKILL` timer, wired to the command's own timeout at `:758`. `pi-subagents/src/runs/shared/worktree.ts:290-296` uses `spawnSync(..., { timeout })`, which kills on expiry.
**Impact** — A hung verify command (`cargo test`, `npm run e2e`, a setup hook) survives its own timeout indefinitely, in a process group cyrup deliberately detached from the terminal, so Ctrl-C cannot reach it either. Every acceptance-gated run that times out leaks a whole subtree for the machine's uptime. Same orphaning class 9b3afd7 fixed one layer down in the termination ladder.
**Fix** — Split the race: spawn, keep the `Child` binding, `tokio::select!` on `child.wait()` vs the timeout, and on expiry drive the existing group-targeting ladder in `spawn/signal.rs::send_signal` (which already guards `getpgid(pid) == pid`). Apply the same at `spawn/worktree.rs:633-701`. Third site to fix or delete: `exec/acceptance.rs:3366` with its race at `:3418`, in the dead `model` module (SUBA-036).
**Verify** — Land SUBA-040's strengthened assertion: after a 100 ms timeout on `sh -c 'echo $$ > pid; exec sleep 300'`, `kill(published_pid, 0)` must fail with ESRCH.

## SUBA-041 — Nine SINGLE-mode parameters advertised in the tool schema but hard-rejected at dispatch

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/extension.rs:4073-4145`: `subagent_tool_parameters` advertises `artifacts` (`:4118`), `includeProgress` (`:4119`), `share` (`:4120`), `sessionDir` (`:4121`), `control` (`:4123`), `output` (`:4124-4127`), `outputMode` (`:4128`), `skill` (`:4129-4132`) and `acceptance` (`:4135-4142`) as top-level properties with full descriptions and enums — this is the LLM-facing schema, so the model will emit them. `route_single` (`:4423-4443`) collects exactly those nine into `unsupported_single_overrides` and returns `ToolError::new("subagent SINGLE mode does not yet support the following param(s): …")` before agent resolution. The comment at `:4415-4422` states the position outright ("this dispatch arm does not yet wire any of them into `RunOptions`/the executor (that plumbing is later-tier work)"); the rejection is tested at `:8821-8845`. Three of the fields already exist on `RunOptions` (`share`/`session_dir` `exec/mod.rs:412-413`, `skills` `:417`), and the PARALLEL/CHAIN item schemas *do* parse `output`/`outputMode` (`extension.rs:3711`, used at `:3915/:3941/:3966/:4029`) — so the gap is specifically the top-level SINGLE surface, and `tasks:[{…}]` is the only workaround.
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts` @v0.34.0 honours all nine in `executeSingle`: `:2788-2791` (`skill`, `output`, `outputMode`), `:2962` (`acceptance`, additionally validated at `:1418`), `:3008-3009` (`includeProgress`, `artifacts`), `:1171`/`:1179` (`share`, `control`).
**Impact** — A SINGLE-mode `subagent` call carrying any of nine schema-advertised parameters fails outright. `output` / `outputMode` / `skill` are everyday delegation controls ("save the result to report.md", "give it just the release-notes skill"), so an orchestrator model reading the schema will reach for them and hit a hard error on a first-class path. High rather than critical because the failure is loud and `tasks:[{…}]` works — the loud rejection is the right interim behaviour; the defect is that the schema promises what the dispatcher refuses.
**Fix** — Wire the nine through `route_single` into `RunOptions`: populate the three existing fields, normalize `output`/`output_mode` the way the tasks[] item path already does (`extension.rs:3711/:3719`), and build an `AcceptanceContract` for `acceptance` instead of falling back to `heuristic_default`. Prerequisite found in the same trace and covered by no other item: `build_attempt_spawn_plan` emits `--session` only (`exec/mod.rs:986-989`) where upstream `pi-args.ts:103-113` has the full `sessionFile / else --no-session + --session-dir` branch — `sessionDir`/`share` need that argv half first. If the whole is too large for one change, the honest interim is to *remove* the unwired properties from `subagent_tool_parameters` and keep the rejection as a backstop.
**Verify** — `{agent:"x", task:"y", output:"report.md", outputMode:"file-only"}` must complete with the output written and a concise file reference returned inline, matching `executeSingle`. The test at `extension.rs:8821-8845` pins the current rejection and must be retired or re-scoped.

## SUBA-002 — Spawn cap enforced on the tool path only; slash and chain surfaces bypass it

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/extension.rs:392-413` defines `reserve_subagent_spawns` (budget type `:191-195`, reset `:421-428`, counters `:5201-5239`). Crate-wide there is exactly **one** production call site, `:5166`, inside `SubagentTool::execute` after the dispatch guard (`:5133`) and the mode-exclusivity gate (`:5144`); it covers the routes at `:5172/:5175/:5182` only. Enumerating every execution entry: `dispatch_slash` (`:5804`) reaches `.spawn_background` (`:5832`) and `.run_foreground` (`:5845`), and `run_or_background_chain` reaches `.run_or_background_graph` (`:6099`) — none downstream of the reserve. `count_requested_subagent_spawns` (`:5201-5214`) and `chain_step_requested_spawns` (`:5219-5239`) are themselves faithful to pi, including the dynamic-fanout and `agent ? 1 : 0` arms.
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:266-282` defines `reserveSubagentSpawns` and calls it at `:3434` inside `executor.execute`. Every slash handler funnels there: `src/slash/slash-commands.ts:980-1264` registers 13 commands whose handlers call `runSlashSubagent` (`:1006/:1020/:1050/:1074/:1101`), which calls `requestSlashRun` (`:395`), which fires the bridge wired at `src/extension/index.ts:396-401` to `executeSubagentCollapsed` → `executor.execute`. The cap is unbypassable upstream.
**Impact** — A session that has exhausted its per-session spawn budget through the tool can keep spawning without limit via any `/subagents-*` command or a chain step, defeating the fan-out containment the budget exists to provide.
**Fix** — Hoist the reserve into `run_foreground_impl` / `spawn_background` / `run_or_background_graph` so every entry is charged, or charge explicitly at `extension.rs:5832`, `:5845` and `:6099` using `count_requested_subagent_spawns` / `chain_step_requested_spawns`. Must stay after mode resolution so the counted number matches the spawned number.
**Verify** — Set the cap to 1, run one subagent via the tool, then invoke `/subagents-run`; the slash invocation must be refused with the same budget error the tool path emits.

## SUBA-005 — Management actions: 5 of 20 still missing

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/extension.rs:4082`: the action enum lists exactly 15 names in pi's order (list, get, models, create, update, delete, eject, disable, enable, reset, status, interrupt, resume, append-step, doctor). Dispatch at `:4663-4665` routes nine through `route_management_action`, `:4666-4668` the four control verbs, `:4669-4673` is the unknown-action arm; the completeness assertion at `:8130-8146` pins that exact vector and explains why the missing five must not be advertised. Denylist `MUTATING_MANAGEMENT_ACTIONS` (`discovery/management.rs:1271-1272`, 7 entries) matches upstream byte-for-byte.
**upstream** — `pi-subagents/src/shared/types.ts:1121` @v0.34.0: `SUBAGENT_ACTIONS` has 20. Absent from cyrup: `steer`, `schedule`, `schedule-list`, `schedule-status`, `schedule-cancel`. Denylist at `src/runs/foreground/subagent-executor.ts:112`.
**Impact** — Five documented management verbs are unavailable; a caller naming one gets the unknown-action error. Because `SubagentToolParams` carries no `deny_unknown_fields` (`extension.rs:3328`), accompanying params like `scheduleName` are silently dropped first, so the error does not explain what was actually wrong.
**Fix** — The subsystems are owned by SUBA-013 (`steer`) and SUBA-016 (the four `schedule*`). Each absorbs its own dispatch half: enum entry `extension.rs:4082`, arm `:4663`, completeness assertion `:8140-8146`. No new ids needed.
**Verify** — After both land, the completeness assertion holds 20 names identical to `shared/types.ts:1121`.

## SUBA-036 — The pi-faithful acceptance model port (~3000 lines) is unreachable

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/acceptance.rs` is 4453 lines; `pub mod model` opens at `:1435` and runs to end of file. A grep for `acceptance::model` across all of `cyrup/crates` returns exactly two hits, both `model::strip_acceptance_report` (`exec/mod.rs:2238`, `:2265`). `model::run_verify_command` (`:3366`) and `model::evaluate_acceptance` (`:3499`) have no non-test caller. The live path is the top-level `evaluate_acceptance` (`:647`): self-report floor (`:664`), completion-guard check (`:667-676`), verify execution (`:679`).
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts` @v0.34.0 is the single implementation, and it is the criteria/evidence/report model that the **dead** submodule ports. There is no upstream analogue of the enum-lattice API that actually runs.
**Impact** — Any parity claim resting on "we ported acceptance.ts" is currently false: the ported code is dead and the live gate is a differently-shaped cyrup construction. ~3000 lines of maintenance surface with no runtime effect, and a second un-killed verify runner hiding inside it (`:3418`).
**Fix** — Decide: (a) wire `model::evaluate_acceptance` into `exec/mod.rs`'s completion path and retire the top-level gate, or (b) delete `pub mod model` except the hoisted `strip_acceptance_report` and record in `lib.rs` that cyrup's gate is deliberately spec-shaped. Either way apply SUBA-027/SUBA-028 to whichever verify runner survives.
**Verify** — After (a), a run whose contract requires `Verified` must produce the criteria/evidence report shape pi emits for the same inputs. After (b), `acceptance.rs` drops by ~3000 lines and no `model::` symbol other than the helper remains.

## SUBA-028 — Acceptance verification cannot be aborted

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/acceptance.rs:647-653`: `evaluate_acceptance(contract, gate, final_output, completion_guard, verify_cwd)` takes no cancellation parameter; neither does `run_verify_commands` (`:480-490`) nor `run_one_verify_command` (`:493-497`). The loop at `:484-487` runs every command to completion in sequence with no abort check. `DEFAULT_VERIFY_TIMEOUT` is 300 000 ms (`:430`). The caller at `exec/mod.rs:2212` demonstrably has `opts.cancel` in scope.
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts:758-760` @v0.34.0: `if (options.signal?.aborted) abortVerification(); else options.signal?.addEventListener("abort", abortVerification, { once: true })`.
**Impact** — Cancelling a subagent run (Ctrl-C, orchestrator cancel, parent timeout) does not stop acceptance verification; the caller can wait up to five minutes per command after asking to stop. Combined with SUBA-027 the abandoned processes outlive the cancel entirely.
**Fix** — Thread `CancelToken` from `exec/mod.rs:2212` into `evaluate_acceptance` → `run_verify_commands` → `run_one_verify_command`, check it before each command in the `:484-487` loop, and `select!` it against the per-command wait alongside the timeout.
**Verify** — Start a run whose verify command sleeps 60 s, cancel after 1 s; `evaluate_acceptance` must return within ~1 s and the child must be gone.

## SUBA-030 — Persona system prompt passed inline on argv: no E2BIG guard, world-readable via /proc

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (E2BIG half reasoned, not observed)
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs:971-984` pushes `format!("{flag}={persona_body}")` as one argv element with no size check; the `[CYRUP-DELTA]` at `:846-860` justifies inline-vs-path but addresses neither consequence. Thirteen lines later the *task* prompt goes through `ChildSpawnSpec::resolve_task_arg` (`spawn/mod.rs:228-240`), which spills above `TASK_ARGV_INLINE_THRESHOLD = 8000` (`spawn/mod.rs:92`) explicitly to stay clear of OS argv limits — and that spill is a plain `std::fs::write` at `spawn/mod.rs:237` with the default umask, so the permissions gap is wider than the persona alone.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:158-165` @v0.34.0 writes the prompt with `writeFileSync(promptPath, input.systemPrompt, { mode: 0o600 })` into an `mkdtempSync` dir and passes the path; the task overflow at `:167-174` uses the same 0600.
**Impact** — (a) Any local user can read a subagent's full persona from `/proc/<pid>/cmdline`, and the spilled task file is world-readable; personas routinely carry project context and occasionally credential-adjacent instructions. (b) A persona above Linux `MAX_ARG_STRLEN` (131072) makes `execve` fail with E2BIG and the spawn dies with an opaque OS error rather than a diagnosable message.
**Fix** — Add the same threshold guard the task path already has: above a limit, write the persona to a 0600 file in a per-run `mkdtemp` dir and pass a path — but only after teaching `cyrup/crates/cyrup/src/cli.rs:127-131` to accept a path form, since `--system-prompt` is currently literal text. Set mode 0600 on the task spill at `spawn/mod.rs:237` unconditionally.
**Verify** — Spawn with a 200 KB persona: the run must succeed and `/proc/<child>/cmdline` must not contain the body. `stat -c %a` on the task spill must be `600`.

## SUBA-031 — `wait` scopes runs by cwd, not by session

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/background/mod.rs:1159-1166`: `run_artifact_roots` builds `<subagents_home>/async/<cwd_key>` and `<…>/results/<cwd_key>` from `cwd_key(cwd)` alone, with no session component; the wait loop drives `list_active_runs` over that root. The delta is stated at `background/wait.rs:41-47`. Deferred explicitly in 46c3868's message.
**upstream** — `pi-subagents/src/runs/background/wait.ts:208-231` @v0.34.0 passes `sessionId: deps.state.currentSessionId ?? undefined`, and the tool is registered per session at `src/extension/index.ts:509-527`.
**Impact** — Two cyrup sessions in the same repo see each other's background runs. `wait {all:true}` in session A blocks on session B's runs and reports their results; a stalled run in an unrelated session hangs an unrelated wait.
**Fix** — Add a session component to the artifact roots, or filter `list_active_runs` by a session field recorded in the run record. `SubagentExecutor::root_parent_session()` (`extension.rs:364-371`) already resolves the anchor.
**Verify** — Two sessions, same cwd, one background run each: `wait {all:true}` in each must return only its own run.

## SUBA-032 — Notice-debounce test asserts a wall-clock outcome with ~15 ms margin

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/tui/notices.rs:799-839`: 60 ms debounce (`:801-803`), sleep 20 ms (`:823`), re-ping (`:825-826`), sleep 45 ms (`:829`), assert `sink.count() == 0` (`:830-834`), sleep 40 ms (`:837`), assert 1 (`:838`). The load-bearing assertion lands at ping2+45 ms against a deadline of ping2+60 ms — 15 ms of slack, and overshoot on the *second* sleep is fatal. Plain `#[tokio::test]` on the real clock; `start_paused`/`time::pause` appear nowhere in the crate.
**upstream** — No counterpart: `tui/notices.rs` implements cyrup's own debounce over `pi-subagents/src/tui/render.ts`-derived surfaces. The in-repo precedent is commit 1806375, which removed a structurally identical assertion from `cyrup-ext/src/caps/proc.rs`.
**Impact** — On a loaded CI box or a busy dev machine the second sleep overshoots, the debounce fires early relative to the assertion, and the test flakes. Flaky tests get `#[ignore]`d, and the debounce loses coverage entirely.
**Fix** — `#[tokio::test(start_paused = true)]` plus `tokio::time::advance` for each interval, making the assertion exact rather than marginal.
**Verify** — The test must pass deterministically under `--test-threads=1` on a machine loaded to 100% CPU, and its runtime should drop to near zero once the clock is paused.

## SUBA-006 — No `--tools` for MCP-only/empty allowlist; no `--no-tools` hardening

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs:891-902` splits typed refs, `:917-930` emits `--tools` only when `!builtin_tools.is_empty()`. cyrup's host CLI already accepts `--no-tools` (`cyrup/crates/cyrup/src/cli.rs:135-136`), so the receiving half exists.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:126-139` @v0.34.0 has an outer `if (input.tools?.length)` and an inner `if (builtinTools.length > 0)` — semantics identical to cyrup's single guard; no `--no-tools` anywhere at the tag. Post-baseline e530aab (2026-07-24) adds `explicitToolAllowlist`/`--no-tools`.
**Impact** — An agent declaring only MCP tools, or an intentionally empty allowlist, spawns a child with *all* builtins available rather than none — a containment gap for deliberately tool-less agents. cyrup is correct for its baseline, so this is drift, not a regression, and it is optional until the baseline moves past v0.34.0.
**Fix** — Port `explicitToolAllowlist`: when the allowlist is declared but yields zero builtins, emit `--no-tools` instead of omitting `--tools`.
**Verify** — An agent with `tools: [mcp__x__y]` must spawn with `--no-tools` and the child must have zero builtins registered.

## SUBA-007 — `toolBudget` entirely unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `tool_budget`/`toolBudget` across `cyrup/crates/cyrup-ext-subagents/src/`. No field on `SubagentExtensionConfig` (`registration/mod.rs:99-215`); no property in `subagent_tool_parameters` (`extension.rs:4073-4145`); `SubagentToolParams` (`extension.rs:3328-3345`) has no `deny_unknown_fields` and its doc at `:3320-3327` says unknown keys are ignored, so a caller-supplied `toolBudget` is silently discarded.
**upstream** — `pi-subagents/src/runs/shared/tool-budget.ts` PRESENT at v0.34.0; threaded into the SINGLE result at `src/runs/foreground/subagent-executor.ts:3007`.
**Impact** — No per-run cap on child tool calls. A looping subagent burns tokens and wall-clock until the model stops or the run times out, and the caller's `toolBudget` is dropped without a warning.
**Fix** — Port `tool-budget.ts` as `exec/tool_budget.rs`, add the config field and the schema property, count tool calls in the NDJSON consumer in `exec/mod.rs`'s drive loop, and report the budget in the run result alongside usage.
**Verify** — A run with `toolBudget: 3` against an agent that would make 10 calls must stop after 3 and report exhaustion the way pi's result does.

## SUBA-008 — `turnBudget` entirely unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `turn_budget`/`turnBudget`/`max_turns` crate-wide at HEAD.
**upstream** — `pi-subagents/src/runs/shared/turn-budget.ts` PRESENT at v0.34.0; threaded into the result at `src/runs/foreground/subagent-executor.ts:3006`, with `appendTurnBudgetSystemPrompt` composed into the child's system prompt.
**Impact** — No per-run turn cap — the same unbounded-loop exposure as SUBA-007 — and the child is never told how many turns it has, so it cannot self-pace.
**Fix** — Port as `exec/turn_budget.rs`, count assistant turns in the drive loop, and compose `appendTurnBudgetSystemPrompt` into the persona argv channel SUBA-001 established (`exec/mod.rs:968-984`).
**Verify** — `turnBudget: 2` must terminate the run after two assistant turns with pi's budget-exhausted result shape, and the child's system prompt must contain the budget notice.

## SUBA-010 — Native supervisor channel unported; still on the broker relay

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `supervisor_channel`/`NATIVE_SUPERVISOR`. The blocking ask is recognised by pattern-matching an ordinary `ToolExecutionStart` for the `contact_supervisor` tool with reason `need_decision`/`interview`: `contact_supervisor_block_prompt`, `cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs:1533-1553` (fn at `:1538`), consumed in `drive_attempt`'s detach handling.
**upstream** — `pi-subagents/src/intercom/native-supervisor-channel.ts` PRESENT at v0.34.0 — a first-class typed channel rather than a stream-shape heuristic.
**Impact** — Supervisor asks depend on the exact stream shape of a tool-start event; a change in tool naming or event payload silently breaks the block-and-ask path, and a subagent waiting on a decision hangs instead of surfacing it.
**Fix** — Port the native channel as `spawn/supervisor_channel.rs`, carrying typed ask/answer frames over the existing intercom transport, and reduce `contact_supervisor_block_prompt` to a compatibility fallback.
**Verify** — A subagent raising `need_decision` must surface the ask to the supervisor even with the tool renamed; today that breaks detection.

## SUBA-012 — `fleet-view` / `/subagents-fleet` / `status view` unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — Only two incidental prose hits for "fleet" crate-wide (`background/wait.rs:149`, `extension.rs:4169`, both inside wait-tool description text) — no partial implementation. No `subagents-fleet` command: the 13-name table at `registration/slash_commands.rs:1824-1841` holds `subagents-companions` in that slot. No `view`/`lines` properties in `subagent_tool_parameters` (`extension.rs:4073-4145`).
**upstream** — `pi-subagents/src/runs/background/fleet-view.ts` PRESENT at v0.34.0; the command is registered at `src/slash/slash-commands.ts:1092-1095` as `runSlashSubagent(pi, ctx, { action: "status", view: "fleet" })`. In-baseline, not drift.
**Impact** — No aggregate view of concurrent background runs. With many runs in flight the operator has `status` per id and nothing else.
**Fix** — Port `fleet-view.ts` as `tui/fleet.rs`, add `view`/`lines` to the status action's params, and register `subagents-fleet` — the exact slot SUBA-009's removal frees.
**Verify** — With five background runs active, `/subagents-fleet` must render the same per-run rows and aggregate line pi renders.

## SUBA-013 — File-based live steer inbox unported (+ the `steer` action)

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `steer_inbox`/`STEER_INBOX`. `background/control.rs` exposes only `InterruptRequest` (`:290`), `ResumeOutcome` (`:606`, with a `SteerRunning` variant at `:613`) and `ChainAppendRequest` (`:776`) — no steer request type, no inbox directory. The default `NoTransportSteerChannel` reports `is_active() == false` (`tui/intercom.rs:393-395`). Deferred explicitly in 46c3868's message.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:32` defines `SUBAGENT_STEER_INBOX_ENV` and `:252` exports it into the child env @v0.34.0 — in-baseline unported work, not drift.
**Impact** — A running subagent cannot be redirected mid-flight. The only interventions are interrupt (kill) and resume, so a run heading the wrong way must be restarted from scratch.
**Fix** — Port the per-run file inbox: a directory under the run's artifact root, `SUBAGENT_STEER_INBOX` exported at spawn in `exec/mod.rs`'s env assembly, a watcher in `background/runner_main.rs`, plus the dispatch half (enum `extension.rs:4082`, arm `:4663`, completeness list `:8140-8146`).
**Verify** — `{action:"steer", id, message}` against a live run must have the child observe the message on its next turn boundary.

## SUBA-014 — `requireReadTool` unported

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — Zero hits for `require_read_tool`/`requireReadTool`. The allowlist at `cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs:891-930` is built verbatim from declared builtins with no `read` head-injection and no config or param to request one.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:120-123` @v0.34.0: `declaredBuiltinTools = input.requireReadTool && input.tools?.length && !base.includes("read") ? ["read", ...base] : base`. In-baseline.
**Impact** — An agent declaring a narrow allowlist that omits `read` gets a child that cannot read files even when the harness expects `read` to always be available; the failure looks like a model refusal rather than a config gap.
**Fix** — Add `requireReadTool` to `SubagentExtensionConfig` and the tool schema, and inject `"read"` at the head of `builtin_tools` in `exec/mod.rs:891-902` when set and the allowlist is non-empty.
**Verify** — An agent with `tools: [bash]` and `requireReadTool: true` must spawn with `--tools read,bash`.

## SUBA-015 — Per-agent persistent memory unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `agent_memory`/`agentMemory`. `AgentDefinition` (`discovery/types.rs:605-682`) has no memory field.
**upstream** — `pi-subagents/src/agents/agent-memory.ts` PRESENT at v0.34.0, bundled into the child's system prompt.
**Impact** — Every subagent invocation starts cold. Agents that upstream accumulate project knowledge across runs re-derive it each time, costing tokens and producing inconsistent conclusions.
**Fix** — Port as `discovery/memory.rs`: a per-agent store under the subagents home, read at spawn and composed into the persona argv channel (`exec/mod.rs:968-984`), written back from the run result.
**Verify** — Run the same agent twice; the second child's system prompt must contain the memory the first wrote.

## SUBA-016 — `scheduledRuns` unported (+ the four `schedule*` actions)

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `scheduled_runs`/`scheduledRuns`. The four verbs are absent from the enum at `extension.rs:4082`; because `SubagentToolParams` has no `deny_unknown_fields` (`:3328`), `schedule`/`scheduleName` params are dropped before the call fails at the unknown-action arm (`:4669-4673`).
**upstream** — `pi-subagents/src/runs/background/scheduled-runs.ts` PRESENT at v0.34.0 (a 514-line `ScheduledRunManager`); all four verbs in `src/shared/types.ts:1121`.
**Impact** — No recurring or deferred subagent runs. A caller attempting one gets an unknown-action error with its schedule parameters silently discarded, so the error does not explain the failure.
**Fix** — Port the job store as `background/scheduled.rs` (persisted jobs under the subagents home, a tick loop in the extension's background task), plus the four dispatch arms at `extension.rs:4082/:4663/:8140-8146`.
**Verify** — `{action:"schedule", agent, task, cron}` then `{action:"schedule-list"}` must round-trip, and the job must fire on its interval.

## SUBA-018 — Prompt-template delegation bridge and prompt-workflow commands unported

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `prompt_workflow`. cyrup's 13-command table (`registration/slash_commands.rs:1824-1841`) registers no workflow-derived command.
**upstream** — `pi-subagents/src/slash/prompt-workflows.ts` and `src/slash/prompt-template-bridge.ts` both PRESENT at v0.34.0; `registerPromptTemplateDelegationBridge` is wired at `src/extension/index.ts:403-430`, routing into the same `executeSubagentCollapsed`.
**Impact** — Prompt templates cannot delegate to subagents, so a whole authoring surface upstream users rely on is absent; templates that upstream fan out run inline instead.
**Fix** — Port both files (`registration/prompt_workflows.rs`, `registration/prompt_bridge.rs`), registering derived commands through the existing `registration/slash_commands.rs` path and routing into the same executor entry the tool uses. Blocks SUBA-022.
**Verify** — A prompt template declaring a subagent delegation must register its command and, when invoked, spawn the named agent.

## SUBA-019 — Frontmatter parser lacks YAML block lists and folded scalars

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:346-353`: `split_comma_list` splits on `,` only and is the sole list parser for `tools`, `skills`, `defaultReads`, `fallbackModels`, `extensions`, `subagentOnlyExtensions` (used at `:486`, `:592`, `:600`, `:605`, `:628`, `:632`). `parse_frontmatter_block` (`:241-338`, block collection `:294-298`, flush `:279-289`) collects an indented continuation verbatim into a dedented block string with no block-list or folded-scalar branch, so `tools:` followed by `  - bash` / `  - read` yields one `ToolRef::Builtin("- bash\n- read")`.
**upstream** — v0.34.0 does the same `.split(",")`, so cyrup is faithful *to the baseline*; the handling arrived post-baseline (`parseFrontmatterList` in abad0da, `foldBlock` in 944155e, both 2026-07-17).
**Impact** — An agent file written with ordinary YAML block-list syntax parses to a single garbage entry that matches no tool, so the agent silently runs with an empty effective allowlist. No error is raised — the failure looks like the model refusing to use tools.
**Fix** — Extend `parse_frontmatter_block` with a block-sequence branch and a folded-scalar (`>`/`|`) branch, and have `split_comma_list` accept an already-split list. Port `parseFrontmatterList`/`foldBlock` from upstream HEAD.
**Verify** — An agent whose `tools:` is a YAML block list must produce the same `ToolRef` vector as the comma form; add a table test covering both plus `>` and `|` scalars.

## SUBA-020 — Model fallback retries whole task on child tool failure

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/fallback.rs:504-518`: `is_retryable_model_failure` lowercases and pattern-scans per line with no tool-failure short-circuit; consulted at `:828` inside `run_fallback_ladder`, after the timeout/detach guards.
**upstream** — `pi-subagents/src/runs/shared/model-fallback.ts:282-285` @v0.34.0 is three lines (`if (!error) return false; return RETRYABLE_MODEL_FAILURE_PATTERNS.some(...)`); at clone HEAD it is four (`:325-329`) with `if (TOOL_FAILURE_PREFIX.test(error.trim())) return false;` inserted (5b93443, 2026-07-15). That one added line *is* this item.
**Impact** — A child whose *tool* failed (a failing test command, a bad path) produces error text that trips a retryable-model pattern, so the entire task is re-run on the next model in the fallback ladder. The user pays for N full re-runs of a task that was never a model problem, and the ladder is exhausted on a deterministic failure.
**Fix** — Add the tool-failure prefix short-circuit at the head of `is_retryable_model_failure` (`exec/fallback.rs:504`), mirroring `TOOL_FAILURE_PREFIX`.
**Verify** — A run whose child reports a tool failure must not advance the fallback ladder; assert the attempt count is 1.

## SUBA-009 — Still ports `companion-suggestions.ts`, deleted before the baseline

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/registration/slash_commands.rs:92` `SlashCommandName::SubagentsCompanions`, descriptor `:212-213`, completeness assertion `:1824-1841` with the count at `:1811-1812`; config field `registration/mod.rs:167-173`; doctor diagnostic `extension.rs:7628`. `grep -ril companion` matches 11 source files plus `tests/companions_wiring_proof.rs` and `tests/companions_hostservices_proof.rs`.
**upstream** — `companion-suggestions.ts` ABSENT at v0.34.0 (deleted 2026-07-03, three days before the tag). Enumerating upstream's 13 `pi.registerCommand(` calls (`src/slash/slash-commands.ts:980,1010,1024,1054,1078,1085,1092,1104,1127,1139,1192,1226,1264`) shows the set is identical to cyrup's 13 **except** that upstream's is `subagents-fleet` (`:1092`) where cyrup's is `subagents-companions` — a one-for-one slot swap that corroborates this item and SUBA-012 simultaneously.
**Impact** — A command and config surface exist that upstream removed, so `/subagents-companions` is cyrup-only behavior a pi user will not recognise, and it occupies the slot `subagents-fleet` needs.
**Fix** — Delete `SubagentsCompanions` and its descriptor, the config field at `registration/mod.rs:167-173`, the doctor diagnostic at `extension.rs:7628`, and the two proof tests. Update the completeness assertion at `registration/slash_commands.rs:1811-1841`, which currently pins `subagents-companions` as required — a stale assertion over a cited list, folded here rather than filed as an independent test-defect.
**Verify** — `grep -ril companion crates/cyrup-ext-subagents` returns nothing, and the 13-name table matches upstream's 13 exactly once SUBA-012 lands.

## SUBA-011 — Whole `src/watchdog/` subsystem absent

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — "watchdog" returns zero hits case-insensitively across the crate at HEAD.
**upstream** — `pi-subagents/src/watchdog/` ABSENT at v0.34.0 (`git cat-file -e v0.34.0:src/watchdog` fails); 15 files at clone HEAD. Post-baseline only.
**Impact** — None at the current baseline. Once cyrup re-baselines past v0.34.0, the absence of stuck-run detection becomes a real gap for long-lived background fleets.
**Fix** — Out of scope until re-baselining; then port as `background/watchdog/`, reusing `background/watch.rs`'s polling seam.
**Verify** — N/A until the baseline moves.

## SUBA-017 — Completion batching unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `completion_batch`/`completionBatch`/`batcher`. `background/watch.rs` (1356 lines) polls at `RESULTS_DIR_POLL_INTERVAL` (`:117`) and invokes `CompletionSink` once per result with no debounce; its own test at `:1300-1352` pins the one-notify-per-result contract.
**upstream** — `pi-subagents/src/runs/background/completion-batcher.ts` PRESENT at v0.34.0.
**Impact** — Ten background runs finishing together produce ten separate notices instead of one batched summary — noise in the TUI, and in a large fan-out the notice stream buries the actual results.
**Fix** — Port as `background/batcher.rs` between `CompletionWatcher` and `CompletionSink`, with a short debounce window and an aggregate notice. Same seam as SUBA-034 — do them together.
**Verify** — Complete five runs within the window; exactly one aggregate notice must be emitted.

## SUBA-021 — Launch-contract / preflight / capability-ceiling / spawn+usage budgets unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `launch_contract`, `capability_ceiling`, `usage_budget`. Symbol-collision warning for whoever picks this up: `spawn_budget` *does* hit (`extension.rs:185`, type `:191-195`) but that is SUBA-002's per-session counter, an unrelated mechanism from upstream's `spawn-budget.ts`.
**upstream** — `launch-contract.ts`, `api/preflight.ts`, `capability-ceiling.ts`, `spawn-budget.ts`, `usage-budget.ts` all ABSENT at v0.34.0. Post-baseline.
**Impact** — None at the baseline; after re-baselining, no pre-spawn validation and no capability ceiling.
**Fix** — Out of scope until re-baselining; lands naturally alongside SUBA-006's argv work.
**Verify** — N/A until the baseline moves.

## SUBA-022 — Typed extension delegation API (v1 + v2) unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `ls src/` at HEAD: `artifacts.rs`, `background/`, `bin/`, `discovery/`, `error.rs`, `exec/`, `extension.rs`, `fork_context.rs`, `jsonl.rs`, `lib.rs`, `registration/`, `spawn/`, `tui/` — no `api/`. Zero hits for `delegation_adapters`.
**upstream** — `pi-subagents/src/api/delegation.ts` ABSENT at v0.34.0.
**Impact** — None at the baseline; after re-baselining, other extensions cannot delegate to subagents through a typed API.
**Fix** — Out of scope until re-baselining; sequence after SUBA-018.
**Verify** — N/A until the baseline moves.

## SUBA-023 — Async lifecycle hardening unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `process_terminal`, `session_lease`, `startup_retry`, `auto_drain`. `TerminationOutcome` (`spawn/signal.rs:66-72`) carries `status: std::process::ExitStatus` and `stage: EscalationStage` only — no signal-name mapping anywhere in the module, so an exit-by-signal is reported as a raw status. `background/control.rs` (2503 lines) exposes only interrupt/resume/append-step. Separately verified this pass: 9b3afd7's fix is correct — `send_signal` (`:236-246`) negates the pid only when `getpgid(pid).as_raw() == raw`, so it never signals the orchestrator's own group.
**upstream** — `pi-subagents/src/runs/background/process-terminal.ts` and `session-lease.ts` ABSENT at v0.34.0. Post-baseline.
**Impact** — None at the baseline. Signal attribution in run results stays coarse ("failed" rather than "killed by SIGKILL"), which makes escalation-ladder debugging harder.
**Fix** — Port after re-baselining. Independently useful now: map `ExitStatus::signal()` to a name in `TerminationOutcome`.
**Verify** — A child killed at the SIGKILL rung must report the signal name in its run record. `background/runner_main.rs`'s exit-path signal attribution has still not been re-audited.

## SUBA-024 — Post-baseline chain/parallel orchestration features unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** probable
**cyrup** — Zero hits for `chain_validation`, `parallel_handoff`, `task_intent`, `agent_contract`. `spawn/chain_graph.rs` (2906 lines), `spawn/parallel.rs` (1278), `exec/acceptance.rs` (4453) carry none of them.
**upstream** — `chain-validation.ts`, `parallel-handoff.ts`, `task-intent.ts`, `agent-contract.ts` all ABSENT at v0.34.0. Post-baseline.
**Impact** — None at the baseline.
**Fix** — Out of scope until re-baselining.
**Verify** — N/A. Confidence stays *probable*: `spawn/chain_graph.rs`'s pre-walk validation and `ChainStepConfig`'s unknown-key handling were again not re-read this pass, and either could already cover part of `chain-validation.ts`.

## SUBA-025 — `toolDescriptionMode` and description override unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — Zero hits for `description_mode`/`toolDescriptionMode`. Both registered tools' descriptions are code-fixed: `SUBAGENT_TOOL_DESCRIPTION` (`extension.rs:3269`) / `CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION` (`:3314`) selected by registration mode at `:4345`/`:4359`, and `wait_tool_description(enabled)` (`:4162`) captured at registration (documented at `:4229-4241`).
**upstream** — `pi-subagents/src/extension/tool-description.ts` PRESENT at v0.34.0.
**Impact** — Deployments cannot trim or replace the (long) subagent tool description to save context or steer the orchestrator differently; the description is effectively a compile-time constant.
**Fix** — Add `toolDescriptionMode` plus an override string to `SubagentExtensionConfig`, and resolve the description at registration in `extension.rs:4345/:4359` and `:5668-5673` rather than selecting a constant.
**Verify** — With a `concise` mode configured, the registered description must be the short form for both `subagent` and `wait`.

## SUBA-026 — Interactive admin UI, selector, `/subagents`, `/subagents-stop` unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `registration/slash_commands.rs:1811-1841` asserts exactly 13 entries and names them; neither `subagents` nor `subagents-stop` appears. `ls src/tui/` at HEAD: `events.rs`, `intercom.rs`, `mod.rs`, `notices.rs`, `render.rs` — no admin or selector surface.
**upstream** — `pi-subagents/src/slash/subagents-admin.ts` and `src/tui/selector.ts` ABSENT at v0.34.0, and the full enumeration of upstream's 13 `registerCommand` calls (`slash-commands.ts:980-1264`) contains neither name. Post-baseline.
**Impact** — None at the baseline; after re-baselining, no interactive agent picker and no stop-all command.
**Fix** — Out of scope until re-baselining; port alongside SUBA-012 (same TUI surface).
**Verify** — N/A until the baseline moves.

## SUBA-029 — Management actions read-modify-write subagents `settings.json` unlocked

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/discovery/settings_write.rs:73-81`: `write_settings_file` is `create_dir_all` then a bare `std::fs::write` — no lock, no temp-then-rename; `read_settings_file_strict` (`:44-68`) is a separate unlocked call. The disable/enable/reset handlers in `discovery/management.rs` therefore perform an unsynchronised read-modify-write, and `route_action` returns before the single-dispatch guard is acquired (`extension.rs:5124-5126` vs `:5133`), so two concurrent management calls can interleave.
**upstream** — `pi-subagents/src/agents/agents.ts:574-577` @v0.34.0 is likewise unlocked, and `settings_write.rs`'s own doc names it as the mirror. The `cyrup-original` kind reflects that cyrup raised the bar for its *own* settings file (`cyrup-config/src/settings.rs` uses `FileLock` + `write_atomic`) and did not extend that here. Different file from cyrup-config's, so there is no `/config` race and CFG-001's latch is unreachable from this path.
**Impact** — Two concurrent disable/enable/reset actions can lose one another's write, or leave a truncated `settings.json` if the process dies mid-write, disabling every agent until it is hand-repaired.
**Fix** — Hold one lock across read+write in `settings_write.rs` and route the write through the crate's own `background/atomic.rs::write_atomic_json`.
**Verify** — Two concurrent `disable` calls on different agents must both persist; kill mid-write and the file must remain parseable. (The trust half — whether an untrusted project override is honoured elsewhere in discovery — remains untraced.)

## SUBA-033 — Tests assert a lower bound on observed concurrency

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/spawn/parallel.rs:717-722` asserts `observed_peak >= 2`, and `:1115-1119` the same for the dispatch-guard test; `peak` is a `fetch_max` over a counter incremented inside `real_child_worker` *before* the child spawns, so the assertion reduces to "the runtime polled ≥2 worker futures before the first 150 ms child exited" — likely, but not controlled by the test. The weaker third instance at `:1185-1190` asserts `unclaimed > 0`, i.e. that an 80 ms cancel timer fires before a wave of 300 ms children completes. The upper bounds (`observed_peak <= cap`, `:712-716`) are the real invariants and are unaffected.
**upstream** — No counterpart: `spawn/parallel.rs` is a documented cyrup-original bounded-concurrency primitive with zero `.ts` citations. The precedent is commit 1806375, which deleted an `orphaned > 0` lower bound for exactly this reason.
**Impact** — On a single-core or heavily loaded runner these flake, and a flaky concurrency test is the fastest route to an ignored concurrency test.
**Fix** — Make the overlap deterministic with a `tokio::sync::Barrier` sized to the expected concurrency inside the worker, then keep only the `<= cap` assertions.
**Verify** — Tests pass deterministically under `--test-threads=1` on a single-core cgroup.

## SUBA-034 — `wait`'s event-bus wake unported; pure polling at a 1 s floor

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/background/wait.rs:30-40` documents the delta; the loop sleeps `DEFAULT_POLL_INTERVAL_MS = 1000` (`:70`) between `list_active_runs` reads, with no subscription.
**upstream** — `pi-subagents/src/runs/background/wait.ts:31-36` @v0.34.0 states it "subscribes to the subagent completion/control channels and wakes the instant any fires… A poll still runs on the interval as a reconciliation fallback"; bus surface `:110-112`, subscriptions registered `:190`.
**Impact** — Up to one second of added latency per wait, and a fan-out of many short runs pays it repeatedly. Low because the polling fallback is functionally correct.
**Fix** — Have `CompletionWatcher` (`background/watch.rs`) publish terminal transitions on a broadcast channel the wait loop `select!`s against, keeping the poll as reconciliation. Same seam as SUBA-017.
**Verify** — A run that completes 50 ms into a wait must return in ~50 ms, not ~1 s.

## SUBA-035 — Active `subagents.modelScope` policy not surfaced by doctor/models

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/registration/doctor.rs` (1795 lines) contains zero `model_scope` lines; `run_models_report` (`extension.rs:1975-1980`) reads only the current model. Enforcement itself is complete (SUBA-003) and violations warn via `warn_violation`, but nothing proactively reports that a policy is in force. Deferred explicitly in 46c3868's message.
**upstream** — `pi-subagents/src/runs/shared/model-scope.ts` @v0.34.0 surfaces warn-severity violations and validates `parseModelScopeConfig` as part of its settings surface.
**Impact** — An operator debugging "why did my model choice not apply" gets no hint from `/subagents-doctor` or the models report that a scope policy is filtering it; they must read `settings.json`.
**Fix** — One diagnostic in `registration/doctor.rs` reading `AgentDiscoveryResult::model_scope`, plus the same line in the models-report header.
**Verify** — With a scope configured, `/subagents-doctor` must print the active scope and its severity.

## SUBA-037 — Doctor's `--version` binary probe leaks the probe process on timeout

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/registration/doctor.rs:344-349` builds `tokio::process::Command::new(&resolved.binary).args(&argv)…` for the `--version` probe and calls `.status()` (`:349`), then races it with `tokio::time::timeout(VERSION_PROBE_TIMEOUT, probe)` (`:351`). `kill_on_drop` is never set, and `Command::status()`'s future owns the `Child` — on the `Err(_elapsed)` arm (`:378+`) the future is dropped, the handle goes with it, and tokio's orphan reaper detaches rather than kills. The crate demonstrably knows the correct pattern: the sibling model probe at `extension.rs:7116` sets `.kill_on_drop(true)` before its own timeout race at `:7131`, and that is the *only* `kill_on_drop` in the crate.
**upstream** — `pi-subagents/src/extension/doctor.ts` @v0.34.0 spawns no subprocess, so the probe is cyrup-original with no direct counterpart; the in-codebase contract for a timed-out child is `pi-subagents/src/runs/shared/acceptance.ts:742-756` (SIGTERM then a hard SIGKILL timer).
**Impact** — `/subagents-doctor` on a misconfigured install — exactly what doctor exists for — can leave a hung `cyrup --version` behind on every invocation, and the report says the probe timed out without saying anything survived. Bounded and low-frequency, hence low rather than a peer of SUBA-027.
**Fix** — Add `.kill_on_drop(true)` to the probe builder at `doctor.rs:344-349`. The probe does not set `process_group(0)`, so kill-on-drop's pid-targeted SIGKILL suffices and no group logic is needed. Fix alongside SUBA-027 so all three timeout sites are swept in one pass.
**Verify** — Point `CYRUP_SUBAGENT_BINARY` at a script that `exec sleep 300`, run `check_binary_resolution` with a 100 ms `VERSION_PROBE_TIMEOUT`; after the check returns Timeout, `kill(probe_pid, 0)` must fail with ESRCH. Today the sleep survives.

## SUBA-038 — Child-safe / unknown-action denial messages do not carry pi's exact text

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/extension.rs:4696-4701` returns `ToolError::new(format!("subagent management action '{action}' is blocked in child-safe fanout mode; {} are not permitted here.", MUTATING_MANAGEMENT_ACTIONS.join(", ")))`, while the doc comment on `route_management_action` (`:4677-4684`) states the crate's own contract: "A pi `isError: true` outcome … maps to a ToolError carrying pi's exact text". The denylist contents are faithful (`discovery/management.rs:1271-1272`); only the text diverges. The sibling unknown-action message at `:4669-4673` diverges the same way. The existing test at `extension.rs:8794` asserts `err.to_string().contains("blocked in child-safe fanout mode")` — a substring check that actively pins cyrup's divergent wording as required.
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:3283-3289` @v0.34.0 emits `Action '${action}' is not available from child-safe subagent fanout mode.`; the unknown-action text at `:3280` is `Unknown action: ${action}. Valid: ${SUBAGENT_ACTIONS.join(", ")}`; denylist at `:112`.
**Impact** — A fanout child attempting a mutating or unknown action sees different text than pi emits. Model-visible, so a persona or skill pattern-matching upstream's wording, or a differential/golden comparison against pi output, diverges. No behavioral difference — the action is blocked either way.
**Fix** — Replace the format strings at `extension.rs:4697-4700` and `:4670-4673` with pi's verbatim text. If the denylist hint is worth keeping, append it as a second sentence and record a `[CYRUP-DELTA]`. Rewrite the assertion at `extension.rs:8794` from substring to equality against the new text.
**Verify** — `{action:"delete", agent:"x"}` in child-safe mode and `{action:"bogus"}` must both produce error text byte-identical to upstream's strings.

## SUBA-039 — `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a detached process group

**Kind** cyrup-original · **Severity** low · **Effort** M · **Confidence** confirmed on mechanism, probable on reachability
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/spawn/mod.rs:404` sets `command.process_group(0)` on every subagent child (18-line rationale at `:388-404`), and the struct at `:422-430` stores the `tokio::process::Child` alongside an `exited: bool`. There is **no** `impl Drop for SpawnedChild` — the crate's five `impl Drop`s are all unrelated (`watch.rs:745`, `runner_main.rs:1832`, `runner_main.rs:1902`, `profiles.rs:1008`, `control.rs:1082`) — and `kill_on_drop` is never set on that command. Termination is reachable only through the five explicit `child.terminate(&cancel)` calls in `drive_attempt` (`exec/mod.rs:1607`, `:1618`, `:1634`, `:1717`, `:1747`), each on a `return` path inside a `select!` or in the post-loop drain. If the `drive_attempt` future is *dropped* rather than driven, none runs, the `Child` drops silently, and the child plus its whole subtree survives in a group nothing holds a handle to and that `process_group(0)` already detached from the terminal's foreground group.
**upstream** — `pi-subagents` never passes `detached`, so its children stay in pi's own process group and a terminal signal reaches the whole tree regardless of how a promise unwinds — cyrup's own `spawn/signal.rs:220-223` says exactly this. The guard upstream gets for free must be written here; pi's closest analogue is the `finally`-shaped cleanup around `trySignalChild` in `src/runs/foreground/execution.ts` @v0.34.0.
**Impact** — An orphaned subagent subtree — a re-exec'd `cyrup` plus whatever cargo/npm/git it is blocked in — runs for the machine's uptime, unreachable by Ctrl-C. Low because neither in-crate driver drops today: `cyrup-agent` awaits the tool future to completion (`cyrup/crates/cyrup-agent/src/agent.rs:1287-1302`) and cancels via a `CancelToken` child, and `spawn/parallel.rs` drains its `JoinSet`. This is a missing safety invariant one careless `select!`/`timeout`/`JoinHandle::abort` away from firing, not a defect with a demonstrated production trigger.
**Fix** — Add `impl Drop for SpawnedChild` that on Unix best-effort SIGKILLs `-pgid` when the child leads its group, reusing the `getpgid(pid) == pid` guard from `spawn::signal::send_signal` (`spawn/signal.rs:236-246`) — negating a pid whose group we merely belong to would signal the orchestrator. The existing `exited` flag (`spawn/mod.rs:429`) lets the normal `terminate`/`finish` paths make the guard a no-op. `kill_on_drop(true)` alone is **not** adequate: it targets the bare pid and leaves the descendants this item is about.
**Verify** — Construct a `SpawnedChild` running `sh -c 'sleep 300 & echo $! > gpid; wait'`, drop it without `terminate`/`finish`, and assert `kill(descendant_pid, 0)` fails with ESRCH. Re-run the existing group-orphaning test to confirm the terminate path is unchanged. Before sizing, check whether `cyrup-session-svc` or `cyrup-tui` ever aborts a `JoinHandle` transitively owning a `drive_attempt` future — that path would raise this sharply.

## SUBA-040 — Verify-timeout test passes with SUBA-027's leak in place and leaks a real `sleep 5`

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `cyrup/crates/cyrup-ext-subagents/src/exec/acceptance.rs:1064-1075`: `a_hanging_command_times_out_and_is_recorded_as_failed` runs `run_one_verify_command("sleep 5", dir.path(), Duration::from_millis(100))` and asserts only `!result.passed` (`:1073`) and that `spawn_error` contains "timeout" (`:1074`). It asserts nothing about the process, so it is green both before and after SUBA-027 is fixed — a test that cannot fail on the defect it sits directly on top of. It also leaves a real `sleep 5` running past the test binary's exit (`process_group(0)` at `:508` detaches it from the runner's group). Weaker neighbouring instance at `:1077-1088`: fine as written, but it would silently inherit the leak if a timeout case were added.
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts:742-758` @v0.34.0 makes termination the observable contract of a timed-out verify command, so upstream's behavior under this exact input is "the process is gone", not merely "the result says timeout". A test that does not assert termination is not testing the ported contract.
**Impact** — The suite reports coverage of the verify-timeout path while leaving its most important property unasserted — which is how SUBA-027 stayed invisible. Secondary and immediate: stray `sleep 5` processes accumulate one per full-workspace run, one of the two shapes 1806375 called out as eroding suite trustworthiness.
**Fix** — Change the command to `sh -c 'echo $$ > "$PWD/pid"; exec sleep 300'`, then after the call returns assert the published pid is gone using the crate's existing `read_published_pid` / `pid_is_terminated` test support (`spawn/mod.rs`). File with SUBA-027 so the assertion is written as part of that fix rather than after it.
**Verify** — The strengthened test must fail at HEAD (proving it now tests the defect) and pass once `run_one_verify_command` drives the timed-out child through the group-targeting ladder. `ps -eo pid,cmd | grep 'sleep 300'` after a full `cargo test -p cyrup-ext-subagents` must be empty.

## Coverage

**Read at HEAD 1806375**, in full or in the cited regions: `extension.rs` (registration, dispatch, action enum, tool schemas, `route_single`, `route_management_action`, doctor probe, spawn budget), `exec/{mod,acceptance,fallback,model_scope}.rs`, `spawn/{mod,signal,parallel,worktree}.rs`, `background/{mod,wait,watch,control,runner_main}.rs`, `discovery/{frontmatter,types,management,settings_write}.rs`, `registration/{mod,slash_commands,doctor}.rs`, `tui/{notices,intercom}.rs`, plus `cyrup/crates/cyrup/src/cli.rs` and `cyrup/crates/cyrup-agent/src/agent.rs:1287-1302` for the receiving and driving ends.

**Upstream** was read at the `v0.34.0` tag (never clone HEAD) for every item: `runs/shared/{pi-args,acceptance,worktree,model-scope,model-fallback,tool-budget,turn-budget}.ts`, `runs/foreground/subagent-executor.ts`, `runs/background/{wait,fleet-view,scheduled-runs,completion-batcher}.ts`, `slash/slash-commands.ts`, `extension/index.ts`, `agents/agents.ts`, `shared/types.ts`. A batch `git cat-file -e v0.34.0:<path>` over 26 named paths returned 10 PRESENT / 16 ABSENT, which is what separates `not-ported` from `upstream-drift` throughout.

**Method for the closures.** Commit messages were not used as evidence. Each closed item was re-derived from code on both sides, including the receiving end (SUBA-001's host CLI signature), the enforcement rather than the file's existence (SUBA-003's `Err` return on an explicit out-of-scope model), and the registration shape upstream actually uses (SUBA-004's Full-arm-only). SUBA-002 and SUBA-005 survived deliberate attempts to refute their residuals — for SUBA-002 by enumerating every execution entry point and resolving each to its enclosing function, then tracing pi's slash path end to end into `reserveSubagentSpawns`.

**Defect-class hunt.** All 34 `tokio::time::sleep(Duration::from_millis(` sites in `src/` and `tests/` were swept. SUBA-032 and SUBA-033 are the only genuine instances; four candidates were cleared by reading them (`background/watch.rs:1339`, `tests/background_runner_main_integration.rs:1163`, `tests/background_spawn_detached_integration.rs:162`, and notably `tests/background_runner_main_integration.rs:806-820`, which documents that both interleavings are legitimate and asserts only the outcome-independent invariant — the pattern 1806375 was moving toward, not a defect). For the "pins current-but-wrong behavior" shape, two were found and folded into their parent items rather than given ids, because each dies with its fix: `extension.rs:8794` (→ SUBA-038) and `registration/slash_commands.rs:1811-1841` (→ SUBA-009).

**Commit-message-only debt** was re-mined from `git log c8bd2ab..HEAD` for the five subagents commits (7a44aec, 7c3862b, 46c3868, 9b3afd7, 1806375). Everything deferred in a message is filed: `steer` → SUBA-013, the four `schedule*` verbs → SUBA-016, the event-bus wake → SUBA-034, wait session scoping → SUBA-031, modelScope surfacing → SUBA-035. Nothing message-only remains unfiled in this area; the other deferrals named in the refresh brief (f777e44's WIT ABI break, d2c5509's thinking replay, c2a7acb's splitter, 289c089's `operations`) belong to other areas.

**Blind spots and things taken on trust.** (1) Nothing was executed — no cargo, no npm, no spawned process; every `Verify` line is a design, not an observation. (2) SUBA-039's low severity rests on a reachability argument traced only two levels up; whether `cyrup-session-svc` or `cyrup-tui` ever aborts a `JoinHandle` transitively owning a `drive_attempt` future was not checked. (3) SUBA-024's two named blind spots (`spawn/chain_graph.rs` pre-walk validation, `ChainStepConfig` unknown-key handling) were again not re-read; its confidence is carried forward as *probable* rather than laundered into confirmed. (4) SUBA-030's E2BIG consequence is reasoned from Linux `MAX_ARG_STRLEN` (131072), not observed. (5) SUBA-029's trust half — whether an untrusted project override is honoured elsewhere in discovery — was not traced. (6) SUBA-041's severity assumes an orchestrator model will emit schema-advertised parameters; a strong inference from the schema being the model-facing surface, but an inference. (7) The v0.34.0 baseline is inherited from the workspace brief and not independently re-derived; the crate still records no version string. (8) `spec/` is absent from this workspace as documented — requirement ids were used only as grep anchors and no requirement text was invented.



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SUBA-S01 | high | not-ported | M | A declared `outputSchema` never reaches the child in any form — no env, no argv, no instruction — so structured output degrades to scraping a ```json fence out of prose |
| SUBA-S02 | medium | not-ported | L | The live-control-notice pipeline has no producer: `activity_state` is never derived, so `tui/notices.rs` (914 lines) and all six `ControlConfig` thresholds are inert |
| SUBA-S03 | medium | not-ported | M | Background (`async: true`) runs cannot be bounded by wall clock at all, and the `timeout` control-inbox verb is unported |
| SUBA-S04 | medium | not-ported | M | No interrupt/timeout cascade to nested async descendants: interrupting a background run leaves every background run it spawned running |
| SUBA-S05 | medium | not-ported | L | The child-side prompt runtime is entirely unported: `inheritProjectContext:false` is a no-op, no child is told it is a child, and forked children inherit the parent's orchestration history |
| SUBA-S06 | medium | not-ported | S | Post-exit stdio guard unported: `drive_attempt` has no `child.wait()` arm, so a child that exits while a descendant holds its stdout pipe hangs the orchestrator |
| SUBA-S07 | low | not-ported | S | Child spawn failure leaks the 0600 task temp file — every cleanup path is downstream of the failure point |

## SUBA-S01 — A declared `outputSchema` never reaches the child in any form — no env, no argv, no instruction — so structured output degrades to scraping a ```json fence out of prose

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:248-249` writes `PI_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE` and `PI_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA` into the child env. `subagent-prompt-runtime.ts:15-19 STRUCTURED_OUTPUT_INSTRUCTIONS` ("Your final action must be to call the `structured_output` tool with JSON matching the provided schema… if you do not call `structured_output`, the parent will fail this step.") is appended to the child system prompt at `:107` whenever the capture env is set, and the runtime registers the `structured_output` tool that writes to the capture file. Parent reads the FILE back (`runs/shared/structured-output.ts:55-68`), whose defining property is that a missing capture file is a hard failure even when prose was produced.

**cyrup** — ABSENT. 

**Impact** — A chain step or fanout task with `outputSchema` gives the child zero knowledge a schema exists. The child answers in prose, no fence is found, `StructuredOutcome::Missing` → hard failure with `STRUCTURED_OUTPUT_MISSING_ERROR`. When it appears to work it is because the model spontaneously emitted a fenced block that happened to validate. Worse than flaky: `first_parseable_json_fence` takes the FIRST parseable ```json block in the last message, so a model whose final message discusses a config sample can return a confidently-wrong structured value that passes validation. `exec/structured.rs:236-249` documents that pi's mechanism is "NOT event-scraping" and calls the child side "out of this crate's scope" — and nothing else in the workspace picked it up.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S02 — The live-control-notice pipeline has no producer: `activity_state` is never derived, so `tui/notices.rs` (914 lines) and all six `ControlConfig` thresholds are inert

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**upstream** — pi-subagents/src/runs/shared/subagent-control.ts @v0.34.0 — verified the exact export list: `:37 resolveControlConfig`, `:73 deriveActivityState`, `:86 buildControlEvent`, `:136 shouldNotifyControlEvent`, `:140 controlNotificationKey`, `:145 claimControlNotification`, `:164 formatControlNoticeMessage`, `:206 formatControlIntercomMessage`. Producers: `runs/foreground/execution.ts` 1 s activityTimer → `updateActivityState`; `runs/shared/long-running-guard.ts` `nextLongRunningTrigger` / `recordMutatingFailure` / `shouldEscalateMutatingFailures`.

**cyrup** — ABSENT. 

**Impact** — `subagents.control` is a silent no-op: an operator configuring `activeNoticeAfterMs` / `failedToolAttemptsBeforeAttention` / `notifyOn` / `notifyChannels` gets nothing and no error. `background/wait.rs:26` documents that a `needs_attention` run ends the wait early; `needs_attention()` at `:202-203` tests `telemetry.activity_state == Some(NeedsAttention)`, a value no production code writes, so `wait` always runs to its full timeout on a wedged child. 914 lines of tested notice machinery are unreachable.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S03 — Background (`async: true`) runs cannot be bounded by wall clock at all, and the `timeout` control-inbox verb is unported

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi HONOURS `timeoutMs` on the async path — I verified the whole chain at v0.34.0: `runs/background/async-execution.ts:677` and `:924` compute `deadlineAt = Date.now() + params.timeoutMs`, pass it into the runner config at `:724` / `:983`, and surface it in `details` at `:843` / `:1063`. `runs/background/subagent-runner.ts:2078-2081` arms `setTimeout(timeoutRunner, deadlineAt - now)`; `timeoutRunner` (`:2029-2062`) sets `state:"failed"`, `timedOut:true`, fails every running/pending step, appends `subagent.run.timed_out`, and aborts `timeoutAbortController`. Independently deliverable from outside: `runs/background/control-channel.ts:41 TimeoutRequest`, `:71 timeoutRequestPath`, `:110 requestAsyncTimeout`, `:209 consumeTimeoutRequest`, `:257 deliverTimeoutRequest`, routed via `watchAsyncControlInbox` (`:274`) → `onTimeout: timeoutRunner` (`subagent-runner.ts:2070`). All in-baseline at v0.34.0.

**cyrup** — ABSENT. 

**Impact** — There is no way, from any surface, to bound a background subagent's runtime. A stuck retry loop, a hung `cargo test`, or a non-terminating model burns tokens and CPU until a human notices and issues `interrupt` — and no external `timeout` verb exists to impose one after the fact. The tool rejects `timeoutMs` on background rather than honouring it, so the user is told the feature is unavailable rather than silently losing it. Note the crate self-authorises this omission by citing `R-SA-036`, a requirement in the absent `spec/` tree that no reviewer in this workspace can check against upstream.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S04 — No interrupt/timeout cascade to nested async descendants: interrupting a background run leaves every background run it spawned running

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — `pi-subagents/src/runs/background/subagent-runner.ts:1535-1564 interruptNestedAsyncDescendants()` — projects the nested-run registry and calls `deliverInterruptRequest({ asyncDir, pid, source: "ancestor-interrupt" })` for every `running`/`queued` descendant; `:1565-1594` is the `timeoutNestedAsyncDescendants()` twin. I confirmed the call sites at `:2026` (last line of `interruptRunner`, immediately before `interruptActiveChildren()`) and `:2061` (last line of `timeoutRunner`, before `timeoutActiveChildren()`). `deliverInterruptRequest` (`control-channel.ts:230`) writes the authoritative file request AND best-effort SIGUSR2.

**cyrup** — ABSENT. 

**Impact** — A chain step that launches `async: true` children and is then interrupted (or times out) leaves those children running as fully detached process groups — `spawn/mod.rs:404` sets `process_group(0)`, so they sit outside the terminal's foreground group and Ctrl-C cannot reach them either. The run flips to `Paused` and the user reasonably believes work stopped; it did not. Confidence on absence is high; today's reachability is limited because cyrup's production paths do not yet mint the nested-event route, which makes the orphans harder to *enumerate*, not less likely to exist.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S05 — The child-side prompt runtime is entirely unported: `inheritProjectContext:false` is a no-op, no child is told it is a child, and forked children inherit the parent's orchestration history

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed

**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:13` resolves `PROMPT_RUNTIME_EXTENSION_PATH`, `:142-143` injects it as `--extension` on EVERY child spawn. I read `runs/shared/subagent-prompt-runtime.ts` @v0.34.0 in full: `:21-27 CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS` ("You are a child subagent, not the parent orchestrator… Ignore prior parent-only orchestration instructions in inherited conversation history. Do not propose or run subagents."), `:29-36` the fanout variant, `:38-46 PARENT_ONLY_CUSTOM_MESSAGE_TYPES` (7 types: `subagent-orchestration-instructions`, `subagent-slash-result`, `subagent-slash-text-result`, `subagent-notify`, `subagent_control_notice`, `subagent-control`, `subagent-control-notice`), `:47 SUBAGENT_ORCHESTRATION_SKILL_NAME_PATTERN`, `:68 stripProjectContext`, `:76 stripInheritedSkills`, `:84 stripSubagentOrchestrationSkill`, `:97 rewriteSubagentPrompt`, `:141 stripParentOnlySubagentMessages`. Knobs read from env at `:11-12`.

**cyrup** — ABSENT. 

**Impact** — (a) `inheritProjectContext: false` — a documented, frontmatter-parseable, test-covered persona field (`discovery/frontmatter.rs:617-619`, name-sensitive default at `:474`) — does nothing; every child gets the full AGENTS.md/CLAUDE.md context. Silent violation of an explicit config, plus a context-budget regression. (b) No child is told it is a child, so a fanout-authorized child has nothing counteracting the orchestration framing it inherits. (c) With `context: "fork"` (`fork_context.rs:197 create_branched_session`), the child reads the parent's `subagent-notify` messages, the parent's orchestration instructions, and the parent's own `subagent` tool call that spawned it — exactly what pi strips to stop role confusion and re-delegation.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S06 — Post-exit stdio guard unported: `drive_attempt` has no `child.wait()` arm, so a child that exits while a descendant holds its stdout pipe hangs the orchestrator

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — `pi-subagents/src/shared/post-exit-stdio-guard.ts:26-84 attachPostExitStdioGuard` — read in full at v0.34.0. On `child.on("exit")` it sets `exited = true`, arms an idle timer re-armed on every subsequent stdout/stderr `data` chunk, and a one-shot hard timer; either calls `destroyUnendedStdio()`, force-destroying streams that never emitted `end`. `clearTimers` on `close`/`error`. Attached unconditionally on BOTH spawn paths with `{ idleMs: 2000, hardMs: 8000 }`: `runs/foreground/execution.ts:736` and `runs/background/subagent-runner.ts:508`. Purpose: the child is gone but the pipe write-end is still open because a grandchild inherited it, so `close` never fires.

**cyrup** — ABSENT. 

**Impact** — A subagent whose descendant inherits the stdout pipe (a `setsid`/`nohup`'d server, an `&`-backgrounded watcher, an MCP server the child spawned) and which then exits abnormally before emitting a terminal assistant stop parks `drive_attempt` on a `next_line()` that never returns EOF. With no timeout configured the orchestrator's `subagent` tool call never completes and the turn never ends — a permanently spinning tool. Medium because it requires stdio inheritance reaching a surviving descendant. Structurally the exact analogue of the OSC-11 bug: pi's guard lives in an unglamorous 84-line 'stdio plumbing' file under `shared/` and is pure behaviour.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SUBA-S07 — Child spawn failure leaks the 0600 task temp file — every cleanup path is downstream of the failure point

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi-subagents/src/runs/foreground/execution.ts:797` — the `proc.on("error")` (spawn-failure) handler calls `cleanupTempDir(tempDir)`, exactly as the `close` handler does at `:756`. pi cleans on all three terminal events.

**cyrup** — ABSENT. 

**Impact** — Task text accumulates as orphaned 0600 files in the scratch dir for the life of the host. Low: mode 0600 limits exposure and volume is small — but it is a pure oversight with a two-line fix, on the path a misconfigured install hits repeatedly.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.


---

## SUBA-N03 — The seven SINGLE-mode overrides are still refused on the **async/background** branch

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed
*(Opened 2026-08-06 as a residual of closing SUBA-041. This is the SAME defect class SUBA-041 existed
to remove — a parameter both advertised in the LLM-facing schema and refused at dispatch — surviving
on a second code path that SUBA-041's fix did not reach.)*

**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:~4855-4870`: `route_single`'s background
branch refuses all seven now-wired params with *"the following param(s) are only supported for
foreground SINGLE runs: …"* whenever `async: true`. SUBA-041 wired `output`, `outputMode`, `skill`,
`acceptance`, `share`, `sessionDir` and `artifacts` into the **foreground** path only.

**upstream** — `pi-subagents/src/runs/background/async-execution.ts:1156+` (`executeAsyncSingle`)
consumes `shareEnabled`, `sessionRoot`, `artifactConfig`, `skills`, `output`, `outputMode` and
`outputBaseDir`; `subagent-executor.ts:3242-3270` passes every one of them through. Upstream honours
the full set on the async path.

**Impact** — Larger than it looks: `asyncByDefault` / `forceTopLevelAsync` config can make **every**
top-level SINGLE call background, at which point `output` / `skill` / `acceptance` are categorically
unusable while still being advertised. ~~The refusal is loud and named, and mirrors pi's own precedent
of erroring on `timeoutMs` + `async` (`subagent-executor.ts:3022`), so it is a defensible interim~~ —
**THAT PRECEDENT DOES NOT EXIST.** Audited 2026-08-07 against
`git show v0.34.0:src/runs/foreground/subagent-executor.ts`: `:3015-3030` is intercom-receipt
construction, entirely unrelated, and no such refusal appears anywhere in upstream `src/`. Upstream
**honours** `timeoutMs` on the async path — `schemas.ts:265-266` and `tool-description.ts:25,:73` both
state it applies to "foreground and async/background runs", and `async-execution.ts:850`
(`executeAsyncSingle`) arms a deadline from it. So there is no interim to defend: this is a plain
unported gap.

**The same false claim sits in the port's own provenance record at `extension.rs:4862`** and must be
corrected there regardless of whether this item is scheduled — a confidently wrong upstream citation
is worse than none, and `CLAUDE.md` names those comments as the mechanism by which parity is audited.

**Scope correction from the same audit: it is EIGHT params, not seven** — `timeoutMs`/`maxRuntimeMs`
is refused just above at `:4862-4868`. The item is also currently **untested**.

**Fix** — Costed during SUBA-041, and it is genuinely `L`, not a tweak:
- `output` / `outputMode` are cheapest — `SingleStepSpec.output_path`/`output_mode` already exist and
  `background/runner_main.rs:1694-1718` honours them — but they need the run-scoped output base dir,
  and the `RunId` is minted *inside* `spawn_background_steps`.
- `acceptance` exists as `SingleStepSpec.acceptance: Option<String>` but is hard-dropped (see SUBA-N04).
- `skill` has no `SingleStepSpec` field at all.
- `share` / `sessionDir` / `artifacts` have no `RunnerConfig` fields.

**Verify** — `{agent:"x", task:"y", async:true, output:"report.md"}` completes with the file written,
matching `executeAsyncSingle`. Until then, add the missing test pinning the *current* refusal so the
divergence is at least asserted rather than merely present.

## SUBA-N04 — CHAIN/background step `acceptance` is silently dropped to `None`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Found incidentally while costing SUBA-N03. Independent of it — this one is already reachable today
through the `tasks:[{…}]` surface that SUBA-041 named as the supported workaround.)*

**cyrup** — `crates/cyrup-ext-subagents/src/background/runner_main.rs:1734` hard-drops
`SingleStepSpec.acceptance` to `None`. The field is parsed and carried all the way to the runner and
then discarded without a warning.

**Impact** — A chain or background step that declares an `acceptance` contract runs **unverified**,
and reports success on the same code path an accepted run would. Silent, not loud: the caller gets no
signal that the contract was ignored, which is worse than SUBA-N03's explicit refusal. Note this is
reachable through `tasks:[{…}]`, which SUBA-041's entry recommends as the workaround for the SINGLE
surface — so the documented workaround silently loses acceptance.

**Fix** — Lower the string onto a real `AcceptanceContract` the way the foreground path now does
after SUBA-041 (`parse_single_acceptance`), or, if the plumbing is not there yet, fail loudly at
parse time instead of dropping at dispatch.

**Verify** — A chain step with `acceptance: "verified"` and a failing `verify[]` command must not
report success.


---

## SUBA-N05 — `chainDir` is advertised, deserialized, and consumed by nothing

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Found 2026-08-07 by the schema inventory, not by an item. It is a THIRD live instance of the
`SUBA-041` class and the only silent one.)*

**cyrup** — `chainDir` is advertised in `subagent_tool_parameters()` and deserialized into
`SubagentToolParams::chain_dir`, but is read **only** by `provided_keys()`. The chain directory is
always `artifacts::chain_runs_dir(cwd).join(run_id)` regardless. No consumer, on any path, on either
the foreground or async branch. **No test complains**, because nothing in the suite notices an
advertised-but-unwired property.

**Impact** — Worse in kind than `SUBA-041`, which at least failed loudly: a caller sets `chainDir`,
gets no error, and the run silently uses a different directory. `SUBA-041`'s defect was *advertised
and refused*; this is *advertised and ignored*.

**Fix** — Either wire it to override the chain-runs directory (check pi's shape first), or remove it
from the schema. Whichever, the schema/dispatch guard test (suggested-order 0) is what stops the next
one.

**Verify** — The guard test fails against the pre-fix schema.

## SUBA-N06 — `includeProgress` and `control` are de-advertised AND refused, and were never re-filed

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed
*(Filed 2026-08-07. These were folded into `SUBA-041`'s CLOSED row rather than tracked, so they were
invisible to every subsequent planning pass.)*

**cyrup** — Both are absent from `subagent_tool_parameters()` and **actively refused** at
`extension.rs:4815`, before the `is_background` branch, so BOTH paths error. Tests at `:8780` and
`:9804` pin the refusal. Note the schema is `additionalProperties: true` (30 advertised, 32 parsed),
so removing them never made them unreachable — the removal did not work on its own terms.

**upstream** — `subagent-executor.ts:1171`/`:1179` (`control`), `:3008` (`includeProgress`).

**Impact** — Two first-class delegation controls are unavailable. More importantly this is the item
the maintainer explicitly rejected as a workaround: *"why are we OKing workarounds rather than solving
the problems like software artisans?"* Removing a feature from a schema to avoid implementing it is
not a fix.

**Fix** — Port both properly and restore them to the schema. As of 2026-08-07 `exec/control.rs` is a
~2000-line complete port of `subagent-control.ts` + the control half of `long-running-guard.ts`, wired
end-to-end and honoured on the foreground path — so `control` is currently **honoured but
unadvertised**, the mirror image of the original defect. `includeProgress` has its model ported
(`LiveProgressStatus`, `RECENT_TOOLS_CAP`, `SingleResult.progress`) but the field is **never
populated**: `run_sync` has a placeholder and there is no `AgentProgress` -> `LiveProgressSnapshot`
bridge.

**Verify** — Both advertised, both honoured on both paths, and the schema/dispatch guard passes.
