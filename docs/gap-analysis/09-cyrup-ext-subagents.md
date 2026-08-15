# 09 — cyrup-ext-subagents

Covers `cyrup/crates/cyrup-ext-subagents/` (the largest crate) — subagent discovery, registration,
foreground/background execution, chain/parallel orchestration, acceptance gating and the subagent TUI
surface — measured against `pi-subagents/` at the ported baseline **v0.43.0** (the crate still records
no version string; v0.43.0 is the `PARITY-GAPS.md` inference, and every upstream claim below was
settled with `git show v0.43.0:<path>` or `git show v0.47.1:<path>`, never clone HEAD, because
clone-HEAD line numbers and file existence both mislead here).

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (last code commit; tree clean at docs-only
> `a9000b1`), against **pi-subagents `v0.43.0`** (ported baseline) and **`v0.47.1`** (latest tag).
>
> **22 items closed**, 4 partially closed, 2 re-written as misdescribed, **24 newly filed**
> (`SUBA-043` … `SUBA-066`), 1 new lead refuted and recorded in `## Coverage` so it is not
> re-derived. Open set after the audit pass was **46 items: 0 critical, 2 high, 24 medium, 20 low**;
> the repair pass below reclassifies one of those (`SUBA-005`) as a tracker, leaving **45 counted
> items + 1 tracker**.
>
> What actually changed on the cyrup side: batches 8–10 landed the child-side prompt runtime, the
> structured-output capture channel, the control/activity pipeline, the async deadline + cascade,
> the watchdog subtree, FleetView, agent memory, prompt workflows, the native supervisor channel and
> the tool-budget enforcer. **Every one of those closures was audited for what the new code does, not
> that it exists** — and that audit is where most of the 24 new items come from. The recurring shape
> is a capability that is *implemented and unreachable*: `toolBudget` is enforced but not advertised
> (`SUBA-047`), `defaultReads` is parsed and never used on a single run (`SUBA-054`), `outputSchema`
> is captured on `tasks[]` but unadvertised on SINGLE (`SUBA-043`).
>
> **Version lag.** The prior pass recorded pi-subagents "latest v0.43.0"; latest is now **v0.47.1**.
> The workspace brief measures the full range at 151 files / +10254 / −1333; the src-only sweep run
> for this pass covered **96 non-merge commits, 67 files, +4696/−769, 12 net-new source files**, none
> of which any prior pass had seen. Eleven of the new items come out of that range
> (`SUBA-044`, `SUBA-050` … `SUBA-060`, `SUBA-065`, `SUBA-066`).
>
> **The two-table split is gone.** All seven `-S` surface-sweep items closed this pass, so the open
> table below is now the complete open set for this area. The `-S` ids are retained in the status
> table so their closures can be re-audited.
>
> ---
>
> **REPAIR PASS, same day (2026-08-12), applying the completeness critique.** No item was renumbered,
> merged or deleted; no new items were filed (no sweep digest routes to this area). Three changes:
> - **`SUBA-005` is now a `tracker`, not a backlog item.** Its own Fix says "this item is the ledger,
>   not the work" — the 27-vs-50/53 verb count is an index over other people's subsystems. It keeps
>   its ID, its severity annotation and its whole body, and it moves to a separate **Trackers** table
>   below so it is excluded from the item count. What it owes (owners for the seven still-unowned
>   verbs, and a completeness assertion pinning the enum against upstream's array) is bookkeeping the
>   next pass performs, not code someone schedules.
> - **The status table now carries a row per ID.** The previous edition collapsed this pass's new
>   items into one `SUBA-043 … SUBA-066` range row, which satisfies the letter of "every item from
>   every prior pass" and defeats its purpose: a reader looking up `SUBA-057` found no row. All 24 are
>   enumerated.
> - **Severities were re-derived** against `README.md:106-107` rather than left as filed; two were
>   examined and both stand. Recorded in `## Coverage` → *Severity re-derivation*.
>
> **Open set after the repair pass: 45 items — 0 critical, 2 high, 23 medium, 20 low — plus 1
> tracker (`SUBA-005`) excluded from that count.**

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 09 — recount: 48 rows → 26 open (0 critical · 0 high · 14 medium · 12 low).** All five of the
> area's highs are closed: `SUBA-014` and `SUBA-043` in sweep 1, `SUBA-067` and `SUBA-068` before it,
> and `SUBA-069` in sweep 2.
>
> **`SUBA-069` closed only after its own premise was refuted**, and both errors are worth recording
> because they were checkable in seconds with `git show`: (1) "pi's default is the same 5000 ms" —
> pi's `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS` is **30000** at both v0.43.0:113 and v0.47.1:114, and
> cyrup's already matched it; (2) "these use the production DEFAULT … so unlike SUBA-068 they cannot
> simply be re-budgeted" — all three fixtures passed `timeout_ms: Some(5_000)` explicitly at HEAD, so
> they were always re-budgetable. The fix is real; the diagnosis was not.
>
> **`SUBA-048` closes PARITY-GAPS `PB-13` with it, as PB-13's own text instructs.** `project_chain_runs_dir`
> had ZERO references crate-wide before sweep 2; `resolve_chain_runs_dir` now gives it a live caller
> and upstream's `project` default takes effect on all three surfaces. Sweep 1 had already REWRITTEN
> this item's Impact — its central claim was measurably wrong, since both run-artifact sites called
> `temp_artifacts_dir(cwd)` directly, so the defect was the inverse of the one filed.
>
> **`SUBA-054` is handed back partial on a decision, not on effort.** The foreground SINGLE half is
> complete and is the whole of the item's Verify. The async half needs a choice the sweep would not
> guess — which of upstream's two cwds (`effectiveCwd` vs the chain dir) a runner step's reads resolve
> against — and guessing would double-emit the instruction on every chain step. **The blocker is
> written into `background/runner_main.rs` at the field rather than left as a silent `None`.** Its
> Fix line is also corrected: upstream has NO top-level `reads` param, so `defaultReads` is the entire
> SINGLE precedence chain and no new advertised param is owed.
>
> **`SUBA-028` was consciously declined and the reason is on the record**: threading a `CancelToken`
> through `evaluate_acceptance` → `run_verify_commands_memoized` → `model::run_memoized_verify_command`
> plus a field on `model::EvaluateAcceptanceInput` touches 40+ call sites, and three other crates
> broke the shared build in sequence during the pass.
>
> **AN ADJACENT REPAIR, flagged for the integration phase:** cyrup-ext's `HostServices::set_widget`
> changed signature mid-pass (`EXT-047`) from one opaque `&Value` to pi's three arguments, and its new
> doc names THIS crate as the defect's victim — "the shipped subagents extension hand-rolled
> `{"key": …, "content": null}` for it and the slot stayed occupied". All four call sites in
> `extension.rs` were adapted, `SubagentFleetStatus::widget_lines` was added beside the retained
> `widget_payload`, `FleetViewPlacement` maps onto `WidgetPlacement`, and the dispose/inspector-open
> paths now issue a real REMOVAL (`lines: None`).
>
> **COVERAGE NOTE:** no item in `12-upstream-drift-pi-core.md` lands in this crate. `DRIFT-014` is the
> only row that names cyrup-ext-subagents and its Fix explicitly says "Do not follow the original
> filing's instruction to add these to `cyrup-ext-subagents/src/exec/fallback.rs`" — recorded here so
> a future pass does not re-derive it.
>
> **`SUBA-030`'s persona half should carry a cross-area dependency marker** rather than reading as
> in-crate work: `crates/cyrup/src/cli.rs` must accept a path form for `--system-prompt` first.


## Status table (every item from every prior pass)

| ID | Status | Evidence |
|---|---|---|
| SUBA-001 | **closed** (7a44aec) | Persona reaches the child as one argv element; residual deltas live in SUBA-030. Not re-derived this pass. |
| SUBA-002 | **closed** (513e45a) — re-audited | `reserve_subagent_spawns` (`extension.rs:760`) now has **four** production call sites — `:8317`, `:10042`, `:10454`, `:10625` — covering tool, slash and chain/graph entries; per-session reset at `:9422` inside the SessionStart handler citing pi's `resetSessionState` (`extension/index.ts:695-803`). Tests at `:13699/:13716/:13753` (the prior note's `:13338-13470` range was wrong). |
| SUBA-003 | **closed** (46c3868) | `modelScope` ported and enforced. Reporting surface remains as SUBA-035; the new `strict` knob is SUBA-050. |
| SUBA-004 | **closed** (46c3868) | `wait` tool present and registered in the `Full` arm. Residuals SUBA-031, SUBA-034; new residual SUBA-056. |
| SUBA-005 | **misdescribed → still-open** | The "15 of 20" framing is stale in *both* directions. cyrup advertises **27** verbs (`extension.rs:6557`); upstream has **50** @v0.43.0 (`shared/types.ts:1885`) and **53** @v0.47.1 (`:1968`). Restated below. |
| SUBA-006 | **closed** | `exec/mod.rs:1463-1478`: `let explicit_tool_allowlist = agent.tools.is_some();` then `if allowlist.is_empty() { --no-tools } else { --tools <csv> }`, matching `pi-args.ts:389-392` @v0.43.0. Test at `exec/mod.rs:4499-4535`. |
| SUBA-007 | **partially-closed** | Enforcement landed: `exec/tool_budget.rs` (388 lines), `TOOL_BUDGET_ENV` written at `exec/mod.rs:1837-1846`, frontmatter key at `discovery/frontmatter.rs:850`. The **tool-param half did not** — residual filed as **SUBA-047**. |
| SUBA-008 | **still-open** | `turn_budget` appears only as a hard-coded `false` consumer in three files (`tui/intercom.rs:348-352`, `exec/fallback.rs`, `exec/mod.rs:2354-2360`), each commented as having no source; no schema key. Duplicate of PARITY-GAPS PB-10. |
| SUBA-009 | **closed** | `registration/slash_commands.rs:11-14` records the removal; the 16-variant `as_str` match at `:127-145` has no `SubagentsCompanions`; `registration/doctor.rs:787-788` records the deleted Companion-packages section. `git ls-tree v0.47.1 -- src \| grep -i companion` is empty upstream. The surviving `tests/companions_*.rs` files are the intercom permission COMPANION — a different subsystem. |
| SUBA-010 | **closed** | `src/native_supervisor.rs` (2251 lines) is a first-class typed channel; `ENV_SUPERVISOR_CHANNEL_DIR` written at `exec/mod.rs:3240-3243`; doctor check in `registration/doctor.rs`. |
| SUBA-011 | **closed** (severity was low, closed at medium) | `src/watchdog/` is 18 modules (~18k lines), registered at `extension.rs:9055` with nine subscriptions at `:9338-9352`. **Existence is not correctness** — PARITY-GAPS UW-3/UW-4/UW-5 record three no-op holes *inside* it and remain open there. |
| SUBA-012 | **closed** | `tui/fleet.rs` (3040 lines) + `fleet_state/fleet_status/fleet_transcript/fleet_overlay/fleet_theme`; `view`/`lines` are advertised properties; `/subagents-fleet` is in the 16-name table at `registration/slash_commands.rs:127-145`. Keystroke half remains PARITY-GAPS UW-7. |
| SUBA-013 | **partially-closed** | Inbox + verb landed: `CYRUP_SUBAGENT_STEER_INBOX` at `exec/mod.rs:1857-1868`, child-side `SteeringInbox` at `prompt_runtime.rs:157-290`, `steer` in the enum (`extension.rs:6557`) dispatched at `:7825-7837`. The **ack/mode half did not** — residual filed as **SUBA-049**. (The audit's cross-reference to "SUBA-045" for this residual was a mislabel; SUBA-045 is the tool-availability diagnostic.) |
| SUBA-014 | **still-open — severity RAISED to high** | Restated below. |
| SUBA-015 | **closed** | `discovery/agent_memory.rs` exists; the memory block is folded onto the persona at `exec/mod.rs:1544-1553` (`persona_with_memory`) before the refinement overlay. Live production path. |
| SUBA-016 | **still-open** | Zero hits for `scheduled_runs`; nothing beginning `schedule.` in the 27-verb enum. The item's verb count of **four is stale — it is nine** (`schedule.create/list/show/history/pause/resume/run/run-due/delete`, `shared/types.ts:1968` @v0.47.1). Duplicate of PARITY-GAPS PB-11. |
| SUBA-017 | **still-open — re-classified `upstream-drift` → `not-ported`** | `completion-batcher.ts` passes `git cat-file -e v0.43.0`, so it is **in-baseline**, not drift. Zero hits for `completion_batch`/`batcher`; no `completionBatch` on the 18-field `SubagentExtensionConfig`. |
| SUBA-018 | **closed** | `registration/prompt_workflows.rs` (831 lines); `/prompt-workflow` and `/chain-prompts` at `registration/slash_commands.rs:141-142`. |
| SUBA-019 | **closed** | `discovery/frontmatter.rs:301` `fold_block`, `:351` `parse_frontmatter_list` (block `- item` lists, preserving pi's absent-vs-empty distinction), used at `:780-799`; tests `:1677-1710`. **The LITERAL block-scalar half never came across** — new item **SUBA-052**. |
| SUBA-020 | **closed** | `exec/fallback.rs:559-575` defines the `TOOL_FAILURE_PREFIX` equivalent citing `model-fallback.ts:316-323`, short-circuited first inside `is_retryable_model_failure` (`:642-651`), matching pi's ordering at `:326`. |
| SUBA-021 | **misdescribed → still-open, severity raised to medium** | The "post-baseline, out of scope" framing is **dead**: `capability-ceiling.ts`, `usage-budget.ts` and `spawn-budget.ts` all pass `git cat-file -e` at **both** v0.43.0 and v0.47.1. `launch-contract.ts` is struck — it is absent at both tags at every path (it historically lived at `src/shared/launch-contract.ts`), so it was never in either baseline. Restated below; the grant half is **SUBA-046**. |
| SUBA-022 | **still-open — re-classified `upstream-drift` → `not-ported`** | `git cat-file -e v0.43.0:src/api/delegation.ts` succeeds, so it is in-baseline. `ls src/` at HEAD has no `api/`; `rg 'prompt-template:subagent' crates/` = 0. |
| SUBA-023 | **still-open** | `TerminationOutcome` (`spawn/signal.rs:90-106`) still carries only `status` + `stage`; no `ExitStatus::signal()` name mapping. `process_terminal` / `session_lease` are zero-hit crate-wide; both upstream files present at v0.47.1 (PARITY-GAPS VL-S3/VL-S4). |
| SUBA-024 | **partially-closed** | `task_intent` **is** ported (`exec/task_intent.rs`, 1967 lines, consumed from `completion_guard.rs`/`acceptance.rs`/`output.rs`/`mod.rs`). `chain_validation` is **struck from the item**: `git log --all -- src/runs/shared/chain-validation.ts` is empty upstream — the file never existed. `parallel_handoff` and `agent_contract` remain zero-hit and their upstream files are present at v0.47.1. |
| SUBA-025 | **still-open — severity raised to medium** | Restated below. |
| SUBA-026 | **partially-closed** | `/subagents-stop` now exists (`registration/slash_commands.rs:144`). `/subagents` (the interactive admin surface, `src/slash/subagents-admin.ts`, 432 lines) and the selector do not — matching PARITY-GAPS VL-S11. `/subagents-guide` is a **third** missing command, filed separately as **SUBA-066**. |
| SUBA-027 | **closed** (513e45a) — re-audited adversarially | `exec/acceptance.rs:2346-2385` carries two real process-death regression tests: `a_timed_out_verify_command_is_killed_not_abandoned` (publishes a pid via `exec sleep 300` — the `exec` is load-bearing and the test says so — and asserts `wait_for_pid_gone`) and `a_timed_out_verify_command_kills_its_whole_process_group` (asserts the **descendant** pid dies). `cmd.process_group(0)` at `:6805`, group-kill doc at `:6853`. |
| SUBA-028 | **still-open** | Restated below. |
| SUBA-029 | **still-open** | `discovery/settings_write.rs:70-81` unchanged. |
| SUBA-030 | **still-open** | Both halves confirmed at HEAD. Restated below. |
| SUBA-031 | **still-open** | Restated below. |
| SUBA-032 | **still-open** | Restated below. |
| SUBA-033 | **still-open** | Restated below, with a corrected line citation for the second instance. |
| SUBA-034 | **closed** (sweep 11 — REFUTED / already-done, landed by `844e25f`) | The `CompletionBus` wake is at HEAD; see the Open-items row. |
| SUBA-035 | **still-open** | `rg 'model_scope\|modelScope' registration/doctor.rs` = 0 across all 1803 lines. Enforcement itself is live at `exec/model_scope.rs:170-188`. |
| SUBA-036 | **closed** (a340b56) | The dual implementation was collapsed: `ls src/exec/` shows a single `acceptance.rs` with no `acceptance/` submodule, and `exec::acceptance::model` is the **live** implementation — reached from `discovery/chains.rs:714`, `extension.rs:8555`, `exec/mod.rs:3527/:3561/:3588`, `exec/acceptance.rs:739/:1152-1160`, `spawn/chain_graph.rs:1614-1618`. No orphaned ~3000-line submodule remains. |
| SUBA-037 | **still-open** | Restated below. |
| SUBA-038 | **partially-closed** | The child-safe UNAVAILABLE text is now verbatim. **Three** residual texts remain — restated below. |
| SUBA-039 | **still-open** | Restated below. |
| SUBA-040 | **closed** | The weak `sleep 5` assertion is gone; `exec/acceptance.rs:2346-2385` asserts process death, which is exactly the strengthened assertion this item asked for. |
| SUBA-041 | **closed** (513e45a), residuals now also closed | Residuals SUBA-N03/N04/N05/N06 all closed this pass. |
| SUBA-N03 | **closed** | `extension.rs:14635-14685` is a table test over `artifacts`/`acceptance`/`control`/`includeProgress`/`timeoutMs`/`maxRuntimeMs`/`share`/`sessionDir` with `async:true`, asserting the message does **not** contain `only supported for foreground`, that it reaches agent resolution (`agent not found`), and that each name is an advertised property. That assertion is now the only crate-wide occurrence of the string. Async SINGLE threads `output_path`/`output_mode`/`skills`/`session_dir` at `extension.rs:2295-2310`. |
| SUBA-N04 | **closed** | `background/runner_main.rs:2375-2385` lowers `step.acceptance` through `exec::acceptance::lower_acceptance_input` and returns `StepResult::failure` with pi's `validateAcceptanceInput` message on an invalid policy, instead of degrading to `None`. The old hard-drop at `~:1734` is gone. |
| SUBA-N05 | **closed** | `extension.rs:2924-2928`: `let chain_dir = resolve_chain_dir(chain_dir_override, cwd, &foreground_run_id);`, resolver at `:6539-6540`, tests at `:12719-12733`. PB-13's separate complaint — that the **fallback** root is temp rather than the project — is untouched and stays open there. |
| SUBA-N06 | **closed** | `extension.rs:6640` inserts `control` via `sj_control_overrides()` and `includeProgress` sits at `:6628` between `artifacts` and `share`, matching pi's ordering; `include_progress` is threaded at `:1963`, `:2179`, `:2342`, `:2393`, `:2541`, `:2773`. |
| SUBA-S01 | **closed** | `exec/mod.rs:3248-3250` creates the capture runtime, `:1818-1832` writes `STRUCTURED_OUTPUT_SCHEMA_ENV`/`_CAPTURE_ENV` into the child env overlay, `:3389-3398` reads the capture **file** back (the fence-scrape survives only on the runtime-creation-failed arm); child side `prompt_runtime.rs:1613-1620` gates `StructuredOutputTool` on both vars, attached in production at `crates/cyrup/src/main.rs:489,638,732`. Item scope was "a chain step or fanout task with `outputSchema`" — the path now wired (`extension.rs:5849`). The **SINGLE-mode** residual is **SUBA-043**. |
| SUBA-S02 | **closed** | `exec/control.rs:464` `derive_activity_state`, `:1260-1293` `update_activity_state` driven from the live monitor (`:1293/:1399`), 12 transition tests at `:1841-2039`; sink + notice formatting wired at `extension.rs:1071-1130`; `ControlEvent` aggregated through chain runs at `spawn/chain_graph.rs:1055,1906`. A real producer, which is precisely what the item said was missing. |
| SUBA-S03 | **closed** | `background/runner_main.rs:302-330` (`timeout_ms`, `deadline_at_ms`, documented against `async-execution.ts:924/983` and `subagent-runner.ts:126`), `:1260-1270` arms the deadline with pi's `Math.max(0, deadlineAt-now)` conversion, `:1653-1661` fires `timeout_message`; external `control/timeout.json` verb consumed at `:1340`. The **missing default** is split out as **SUBA-051**. |
| SUBA-S04 | **closed** | `background/cascade.rs:1-31` (module doc naming the pi function), `:45-80` `CascadeVerb::{Interrupt,Timeout,Stop}` with literal `ancestor-interrupt`/`ancestor-timeout`/`ancestor-stop` sources, `:163` `cascade_to_nested_async_descendants`. cyrup also ported the `stop` twin the item did not ask for. |
| SUBA-S05 | **closed** | `prompt_runtime.rs` is a full port of `subagent-prompt-runtime.ts` — `rewrite_subagent_prompt`, `strip_parent_only_subagent_messages` (`:660-700`, reproducing the `isParentOnlySubagentMessage` filter, the `SUBAGENT_FANOUT_CHILD` gate at `:142`, `stripAssistantSubagentToolCallBlocks` and the "return None when nothing changed" contract at `:319`), `INHERIT_PROJECT_CONTEXT`/`INHERIT_SKILLS` env readers — attached in production at `crates/cyrup/src/main.rs:489,638,732`. One omitted sub-behaviour (`sanitizeToolIds`) was investigated and **refuted** as a finding; see `## Coverage`. |
| SUBA-S06 | **closed** | `exec/mod.rs:2826-2856`: a `ChildStep::Exited` arm arms `exit_drain_at = now + POST_EXIT_DRAIN_MS` and deliberately does *not* break (so buffered stdout is not dropped), with an `exit_drain_arm` at `:2852` that breaks the `select!` so the status flows through `wait_final_drain`. Reasoning cites SUBA-S06 and the surviving-grandchild hazard verbatim. |
| SUBA-S07 | **closed** | `spawn/mod.rs:459-464`: the `Err(err)` arm of `SpawnedChild::spawn` calls `cleanup_temp_files(&temp_files)` before returning, with the R-SA-067 rationale at `:426-434` explaining that `spec` is taken by value so this is the only cleanup opportunity. No other early return bypasses it. |
| SUBA-042 | **refuted — never filed** | Inherited tool-id sanitation. See `## Coverage` → rejected with reason. The ID is burned, not reusable. |
| SUBA-043 | **new this pass** | high · SINGLE-mode `outputSchema` unadvertised and hardcoded `None`. Residual of the closed SUBA-S01. |
| SUBA-044 | **new this pass** | medium · Bundled `reviewer` agent still grants `bash`/`edit`/`write`; upstream made the lane read-only. From the v0.43.0..v0.47.1 range. |
| SUBA-045 | **new this pass** | medium · Child tool-availability diagnostic unported. From sweep 1 (child env vars, `TOOL_DIAGNOSTIC_PATH`). |
| SUBA-046 | **new this pass** | medium · `grant-spawn-budget` unported *and advertised*. Depends on SUBA-064 for the authority gate. |
| SUBA-047 | **new this pass** | medium · `toolBudget` honoured but never advertised. Residual of the partially-closed SUBA-007. |
| SUBA-048 | **new this pass** | medium · `artifactDir` config key unported; `session`/`temp` unreachable. From sweep 3. |
| SUBA-049 | **partially-closed** (sweep 11) | Ack path + `mode` + capability landed; `steeringRecovery` handed back as L. |
| SUBA-050 | **new this pass** | medium · `subagents.modelScope.strict` unported. From the version-lag range. |
| SUBA-051 | **new this pass** | medium · Async child runs have no default wall-clock timeout. Split out of the closed SUBA-S03. |
| SUBA-052 | **new this pass** | medium · YAML literal block scalars parse to the literal `"\|"`. Split out of the closed SUBA-019. |
| SUBA-053 | **new this pass** | medium · `~` never expanded in chain read/write paths. |
| SUBA-054 | **new this pass** | medium · `defaultReads` never reaches a single run. |
| SUBA-055 | **closed** (sweep 11) | `guide` + `resources/docs/` landed; `children.list` returned to SUBA-005 with its reason. |
| SUBA-056 | **new this pass** | medium · Durable completion replay and output archives unported. Residual of the closed SUBA-004. |
| SUBA-057 | **new this pass** | medium · `dismiss` unported — a recovered workflow with no live controller is stuck "running". Owns one of SUBA-005's unowned verbs. |
| SUBA-058 | **new this pass** | low · Chain read instructions not filtered by existence. |
| SUBA-059 | **new this pass** | low · `artifactConfig.cleanupDays` parsed but never wired. |
| SUBA-060 | **new this pass** | low · "Resume-first" guidance for failed async runs unported. |
| SUBA-061 | **new this pass** | low · Four config keys silently ignored (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`). From sweep 3. |
| SUBA-062 | **new this pass** | low · Bundled `researcher` cannot do web research; root cause hands off to areas 04/12. |
| SUBA-063 | **new this pass** | low · Zero-tool-budget authorisation and the runtime-extension acknowledgement path unported. From sweep 1. |
| SUBA-064 | **new this pass** | medium · The whole `authorityPolicy` subsystem unported; the `stop`/`steer` gate it drives is live-reachable. From sweep 4 (denial paths). |
| SUBA-065 | **new this pass** | low · `unknownSubagentActionMessage` did-you-mean recovery and its destructive-action gate unported. From sweep 4. |
| SUBA-066 | **closed** (sweep 11) | `/subagents-guide`, landed with SUBA-055. |
| SUBA-067 | **new — FIXED** | high · descendant-termination fixture exec-collapsed to one pid, so the test never exercised group-kill. Found by RUNNING the suite. |
| SUBA-068 | **new — FIXED** | high · setup-hook timeout fixture raced macOS's ~200 ms first-exec verification with a 200 ms budget. Found by RUNNING the suite. |
| SUBA-069 | **new — OPEN** | high · the whole setup-hook test family is wall-clock-budgeted and goes red under machine load; `-p cyrup-ext-subagents` is not a reliable gate. |

Closed this pass: **22**. Partially closed: **4** (SUBA-007, SUBA-013, SUBA-024, SUBA-026).
Newly filed: **24**. Refuted and recorded: **1**.

## Open items

> **RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 8 medium, 9 low = 17** (`SUBA-005` remains under `## Trackers` and is not counted). The table carries **48 rows: 31 fully closed, 17 open (2 partially)**. Sweep 8 closed three: **`SUBA-008`** — the whole turn-budget subsystem, the top-ranked remaining medium, landed end to end; **`SUBA-030`** and **`SUBA-035`** as already-done, both of which had been *partially* closed for an edition with residuals that were in fact already in (and `SUBA-035`'s residual named the wrong file). Four rows were re-confirmed as **decisions rather than work** — `SUBA-025` (second decline), `SUBA-054`'s async half, plus `PERM-032`-style blockers routed elsewhere. *(Previous edition: 0 / 0 / 10 / 10 = 20, 30 closed.)*
>
> **THE AREA'S REMAINING SET IS GENUINELY LARGE AND IN-AREA, and it partitions cleanly — record this so sweep 9 does not re-derive it.** `SUBA-016` (`scheduledRuns` + nine `schedule.*` verbs; **L**, and the gate is free — `AuthorityAction::ScheduleCreate` is already pre-wired by `SUBA-064`); `SUBA-021` (`capability-ceiling` + `usage-budget`; **L** — **the spawn-budget third is already closed with `SUBA-046`, so any plan quoting "all three" is stale**); and the guide/replay/dismiss trio `SUBA-055`+`SUBA-056`+`SUBA-057` (**M/L/M** — but `SUBA-055`'s guide action additionally requires AUTHORING a `resources/docs/` set, the same invention hazard as `SUBA-025`, so it needs the same sign-off, and **`SUBA-066` is its last mile, not an independent item**).
>
> **UPDATED 2026-08-15 (sweep 10) — two of the three items the routing plan above called the area's
> remaining large set are now CLOSED, and one of them was blocked on a premise that was simply
> wrong.** `SUBA-021` landed both remaining halves (capability-ceiling + usage-budget, ~1030 new
> lines, consumed at `build_attempt_spawn_plan` and at `run_sync`'s settle respectively).
> `SUBA-025` landed after **refuting its own blocker**: the routing note said porting it required
> *"AUTHORING cyrup-specific compact and safety-guidance blocks — inventing model-facing text"*,
> and that is false. `extension/tool-description.ts` exists at **v0.34.0** — the tag cyrup's own
> `SUBAGENT_TOOL_DESCRIPTION` was ported from — and its COMPACT/SAFETY constants there are written
> around SINGLE/PARALLEL/CHAIN, so both came across byte-identical with nothing authored. **Two
> sweeps declined this item, and a third routed around it, because all three read only the
> v0.43.0 copy of the file.** The general lesson, worth carrying: *when an item is blocked because
> an upstream constant describes a subsystem cyrup has not ported, check whether an EARLIER tag
> has the same symbol written around the surface cyrup actually has* — `git log --oneline -- <path>`
> costs seconds and here it was the difference between a decline and a close.
> `SUBA-016` remains BLOCKED exactly as sweep 9 recorded it.
>
> **CORRECTED 2026-08-15 (sweep 9), because the paragraph above is a routing plan and two thirds of it are now wrong.** `SUBA-057` is **CLOSED** — and it was not an M: the read half was already at HEAD, so only the writer, the enum entry, the dispatch arm and the child-safe gate were owed. `SUBA-016` is **BLOCKED, not L**: "the gate is free" is true and irrelevant — the gate was never the cost. A schedule's only legal target is `workflowScript` at **both** baselines (`scheduled-runs.ts:38` @v0.43.0), and `workflowScript` is a 916-line `node:vm` JS sandbox (`workflows/scripted-workflow.ts:8,388,392`) this crate documents as unported at `extension.rs:5990-6020`. **Do not hand `SUBA-016` to an agent as ordinary work** — it will either invent a schedule target upstream deleted or stall. It needs an owner decision, exactly like `SUBA-025`.

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 10 medium, 10 low = 20** (`SUBA-005` remains under `## Trackers` and is not counted). 28 rows are now marked CLOSED, including all five of the area's highs. **Sweep 6 closed six as REFUTED — every one was already closed at HEAD by a sweep between 3 and 5** (`SUBA-046`, `SUBA-028`, `SUBA-031`, `SUBA-039`, `SUBA-045`, `SUBA-060`) — and discharged two recorded blockers (`SUBA-021`'s spawn-budget third, `SUBA-030`'s `cli.rs` prerequisite). *(Previous edition: 0 / 0 / 14 / 12 = 26, 22 closed.)*
>
> **ONE OPEN ROW NEEDS A DECISION, NOT AN AGENT — `SUBA-025`.** Its Fix (port `toolDescriptionMode`, the 50 KiB file override and `withMandatorySafetyGuidance`) is mechanically portable, but **two of the three constants it selects between are not**: pi v0.43.0's FULL/COMPACT/SAFETY texts are written around `workflowScript`, which cyrup deliberately does not implement and documents as such (`extension.rs:5701-5716`), while cyrup's `SUBAGENT_TOOL_DESCRIPTION` is the v0.34.0-era SINGLE/CHAIN/PARALLEL text and correctly describes what cyrup can do. Landing the mechanism therefore requires **AUTHORING cyrup-specific compact and safety-guidance blocks — inventing model-facing text**, which is the class this directory blames for 29 of sweep 1's 32 fallout failures. Sweep 6 declined rather than invent it silently. It needs either owner sign-off on cyrup-original constants under a `CYRUP-DELTA`, or a decision to port `workflowScript` first.

> **AMENDED 2026-08-14 (documentation audit) — two rows added, `SUBA-070` and `SUBA-071`; the area's counted set becomes 0 critical, 0 high, 9 medium, 10 low = 19.** Both were found while writing user documentation for the subagents extension, and both were predicted by this area's own Coverage blind spots — 6 for the unwired class and 5 for `registration/profiles.rs` never being opened. A blind spot that names a file is a work item, not a disclaimer.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~SUBA-014~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | S | `requireReadTool` unported — a skill-carrying agent can be told to `read` a skill it has no `read` tool for — **CLOSED 2026-08-14**: sweep 1 — the seam is now `exec::build_attempt_spawn_plan_with_read_requirement`; the 7-arg `build_attempt_spawn_plan` survives as pi's `requireReadTool: undefined` form so no external caller broke. |
| ~~SUBA-043~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | S | SINGLE-mode `outputSchema` is unadvertised and hardcoded `None` on both single paths — **CLOSED 2026-08-14**: sweep 1 — the schema/dispatch guard the Verify asked for already existed (`every_advertised_schema_property_is_read_outside_provided_keys`) and now covers `outputSchema` and `toolBudget` automatically — narrows blind spot 6. |
| ~~SUBA-008~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | **M/L** *(re-rated from M — see the body; sweep 6's "cheapest remaining medium … WIRING plus a schema key, not a port" is measurably wrong)* | `turnBudget` unported; the only consumers read a hard-coded `false` — **CLOSED 2026-08-14 (sweep 8): the whole assistant-TURN-budget subsystem ported end to end**, ~888 lines of new module (`crates/cyrup-ext-subagents/src/exec/turn_budget.rs`, verified at HEAD) plus a new drive-loop arm, a signal ladder, three new `SingleResult` fields, a frontmatter field, a serializer arm and a config key. **14 new tests, incl. two `cyrup-it` end-to-end tests against real OS subprocesses.** Closes PARITY-GAPS PB-10. **FOUR ERRORS IN THIS ROW'S OWN BODY ARE CORRECTED THERE — read them before reading the body as history; one of them would have shipped a non-functioning feature.** |
| SUBA-016 | medium | not-ported | **XL, and BLOCKED** *(re-rated from L)* | `scheduledRuns` unported (+ **nine** `schedule.*` verbs, not four) — **BLOCKED 2026-08-15**: sweep 9 read `pi-subagents/src/runs/background/scheduled-runs.ts` at **both** baselines and found a hard prerequisite the item never recorded. **The verb count is CONFIRMED at nine** (`SCHEDULED_RUN_ACTIONS`, `:15-25` @v0.43.0 and HEAD; `shared/types.ts:2084` @v0.47.1) — the row was right and the body's "four" was stale. **But a schedule's only legal target is `workflowScript`**: `ScheduleTarget = { workflowScript: string }` (`:38` @v0.43.0, `:39` @HEAD) is the entire union, `sanitizeTarget` (`:319`/`:327`) refuses an `agent`/`task` target with *"schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\"."*, and `parseScheduleTarget` (`:196`) refuses a persisted one as a *"removed legacy agent target"*. `workflowScript` is `pi-subagents/src/workflows/scripted-workflow.ts` — **916 lines whose mechanism is a `node:vm` JS sandbox** (`:8` `require("node:vm")`, `:388` `vm.createContext`, `:392` `new vm.Script`) — which this crate documents as entirely unported at `extension.rs:5990-6020` (*"the identifier appears nowhere in it"*). Porting the nine verbs against cyrup's `agent`/`task` shape instead would **invent a schedule-target union upstream deleted**, which is the mechanism-substitution the port rules forbid; porting them against `workflowScript` needs a **scripting-host decision** (and a Rust JS engine), which is an owner call, not agent work. **TWO ERRORS IN THIS ITEM'S OWN TEXT, corrected here:** its Verify recipe `{action:"schedule.create", agent, task, cron}` describes a shape upstream **explicitly refuses**, and names a **`cron` parameter that exists nowhere upstream** — the two triggers are `at` (one-shot, `parseScheduledRunTime` `:96`) and `every` (fixed interval, `parseScheduleInterval` `:125`), and `create` refuses calendar forms outright (`:459`). **NEEDS: owner sign-off on either (a) porting `workflowScript` first, or (b) a `[CYRUP-DELTA]` schedule target carrying `agent`/`task`, explicitly diverging from both baselines.** The SUBA-064 pre-wiring the row below advertises (`AuthorityAction::ScheduleCreate`) is real but is not the expensive half. |
| ~~SUBA-021~~ | ~~medium~~ **CLOSED 2026-08-15** | not-ported | L | `capability-ceiling` / `usage-budget` unported — both are **in-baseline** — **CLOSED 2026-08-15 (sweep 10): both remaining halves ported end to end.** **(a) `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs`** (~560 lines) — `normalizeCeiling`/`parseSubagentCapabilityCeiling`/`intersectSubagentCapabilityCeilings`/`resolveSubagentCapabilityCeiling`/`isAgentAllowedByCapabilityCeiling`/`assertAgentAllowedByCapabilityCeiling`/`capabilityCeilingAgentRestrictionMessage`/`…Sources`/`encode`/`decode` plus the per-session registry, vs `runs/shared/capability-ceiling.ts:12`/`:58`/`:65`/`:95`/`:106`/`:140`/`:159`/`:168`/`:172`/`:176`/`:183`/`:188`/`:192`/`:197` @v0.43.0, every refusal byte-identical. **CONSUMED, not merely present**: `exec/mod.rs`'s `build_attempt_spawn_plan_with_read_requirement` resolves the ceiling FIRST (before any argv/env) and refuses an out-of-ceiling agent through the new `SubagentError::CapabilityCeilingViolation`, then encodes it into `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` beside the `TOOL_BUDGET_ENV` encoder — **which is exactly where this item's Fix line said to put it** — so the bound tightens monotonically across the re-exec. Closes the env-var half of PARITY-GAPS VL-S1. **(b) `exec/usage_budget.rs`** (~470 lines) — `validateUsageBudgetConfig`/`validateLimit`/`usageBudgetState`/`metricState`/`usageBudgetExceededMessage` vs `runs/shared/usage-budget.ts:3`/`:14`/`:35`/`:44`/`:61`, threaded end to end: `usageBudget` is now an advertised tool param (`extension/schemas.ts:330`), validated at BOTH dispatch boundaries with upstream's text, carried on `SingleRunOverrides`/`BackgroundSingleRequest`/`BackgroundStepsSpec`/`RunnerConfig`/`ExecSingleStepExecutor`/`RunOptions`, and enforced at `run_sync`'s settle (pi `subagent-runner.ts:4403-4411`) — an exhausted budget becomes the run's `error` and is published on the new `SingleResult::usage_budget`. **Two `CYRUP-DELTA`s, both in-source**: the registry token is a `u64` not a JS `Symbol`, and the handle disposes on `Drop` (a Rust future can be dropped at any `.await`, so an explicit-only `dispose()` would strand a live ceiling). **11 new unit tests** plus two wiring tests that were red at HEAD (`the_capability_ceiling_refuses_an_out_of_ceiling_agent_and_reaches_the_child_env`, and the `usageBudget` block inside `subagent_tool_schema_exposes_the_full_pi_parameter_union`). **ONE ERROR IN THIS ROW'S OWN HISTORY, corrected:** the item's Fix said to consult usage-budget in `build_attempt_spawn_plan` too — upstream does not; `usageBudget` is a run-level, settle-time check (`subagent-runner.ts:4403`), never a spawn-plan input, and wiring it into the plan would have enforced nothing. — *(superseded)* ~~**RESTATED 2026-08-14**: sweep 6 — the `spawn-budget.ts` third is CLOSED with `SUBA-046`; what remains is the capability-ceiling half and the usage-budget half only.~~ |
| ~~SUBA-025~~ | ~~medium~~ **CLOSED 2026-08-15 — the blocking premise REFUTED** | not-ported | S | `toolDescriptionMode`, the file override, and the mandatory safety-guidance appender unported — **CLOSED 2026-08-15 (sweep 10). The two prior declines rested on a premise that is measurably false, and the refutation is one `git cat-file`: `src/extension/tool-description.ts` is present at **v0.34.0**, not only v0.43.0.** v0.34.0's `FULL_SUBAGENT_TOOL_DESCRIPTION` (`:17-66`) is the very text `extension.rs`'s `SUBAGENT_TOOL_DESCRIPTION` was ported from, and its `COMPACT_SUBAGENT_TOOL_DESCRIPTION` (`:68-88`) and `SUBAGENT_SAFETY_GUIDANCE` (`:9-15`) siblings are written around the SAME SINGLE/PARALLEL/CHAIN surface — they name `list/get/models/create/update/delete/eject/disable/enable/reset/status/interrupt/resume/steer/append-step/doctor`, every one of which cyrup dispatches today. Only **v0.43.0's** rewrite of those two constants is `workflowScript`-shaped, and v0.43.0 was never the revision cyrup's full description came from. **So NO model-facing text was authored: both constants are byte-identical v0.34.0 upstream, and the `full` arm is passed IN rather than duplicated, so the crate still has exactly one full description.** New `crates/cyrup-ext-subagents/src/registration/tool_description.rs` (755 lines) ports `resolveToolDescriptionMode` (`:104`), `customDescriptionPaths` (`:112`), `renderCustomTemplate` (`:121`, all eight placeholders, hand-scanned rather than regex), `loadCustomToolDescription` (`:143`, the 50 KiB `CUSTOM_TOOL_DESCRIPTION_MAX_BYTES` gate and all six warnings verbatim), `withMandatorySafetyGuidance` (`:180`) and `buildSubagentToolDescription` (`:191`). Wired at registration: new `SubagentExtensionConfig::tool_description_mode` (carried RAW like `turn_budget`, because upstream WARNS on a bad value rather than throwing), resolved in `init`'s Full arm and applied through the new `SubagentTool::with_description` — the `description` field is now `String` because a custom description's bytes come off disk. The ChildSafe arm is deliberately NOT resolved: upstream's fanout child builds its own literal (`extension/fanout-child.ts:159` @v0.34.0) and never calls the resolver. **ONE `[CYRUP-DELTA]`, in-source and mechanically enforced**: the compact text drops upstream's single *"Opt-in schedule actions: …"* bullet (`:80`) because `scheduledRuns` is unported (SUBA-016) and advertising four unroutable verbs is the SUBA-046 defect class; `the_compact_description_advertises_no_verb_cyrup_cannot_dispatch` asserts it rather than prose. **10 new tests**, one of which (`the_advertised_description_honours_the_configured_mode_and_the_file_override`, in `extension.rs`) is red at HEAD against the pre-existing `Tool::description()` surface. **ONE ERROR IN THIS ITEM'S OWN TEXT, corrected:** its Verify says the compact form must apply to *"both `subagent` and `wait`"* — upstream calls `buildSubagentToolDescription` for the `subagent` tool ONLY (`extension/index.ts:458` @v0.34.0, `:540` @v0.43.0); `wait` carries its own literal (`:512`), so applying the mode to it would have been a divergence. — *(superseded)* ~~**NOT TAKEN by a SECOND sweep (2026-08-14, sweep 8) … NEEDS: owner sign-off on cyrup-original constants under a `CYRUP-DELTA`, or a decision to port `workflowScript` first.**~~ |
| ~~SUBA-028~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Acceptance verification cannot be aborted — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. **The sweep-2 decline paragraph that stood here is deleted: the 40+ call sites of churn it was based on were AVOIDED, not paid.** `exec/acceptance.rs` now has BOTH `evaluate_acceptance` (`:1366`, the unchanged 7-arg entry) and `evaluate_acceptance_with_cancel` (`:1416`), so the cancellation seam exists without breaking the entry shape. vs pi-subagents v0.43.0 `runs/shared/acceptance.ts:1073`, `:1181-1182`, `:1290`, `:1295`. |
| ~~SUBA-030~~ | ~~medium~~ **CLOSED 2026-08-14 — already-done** | parity-bug | S | Persona inline on argv; task spill written with the default umask under a doc asserting 0600 — **CLOSED 2026-08-14**: sweep 8 read the crate at HEAD and found the persona/E2BIG residual — the half sweep 6 called "now ordinary in-crate work" — **already closed**. `spawn/mod.rs:262-289` `ChildSpawnSpec::resolve_system_prompt_arg` writes the composed persona to a `0600` `<stem>.md` in the run scratch dir via `write_private`, with `sanitize_prompt_file_stem` reproducing upstream's `[\w.-]`→`_` rule (pi `runs/shared/pi-args.ts:570-585` @v0.43.0 — note upstream spills **unconditionally**, unlike the task spill at `:588`, so this is the literal mechanism and not a large-persona fallback). **And it is CALLED:** `exec/mod.rs:1802` pushes the flag and the PATH as two argv elements, with an in-source block at `:1783-1801` naming **both** halves the item filed — the `/proc/<pid>/cmdline` disclosure and the `MAX_ARG_STRLEN`/`E2BIG` spawn failure. The 0600 task spill closed earlier and is pinned by `spawn/mod.rs:1197`. **Superseded partial-closure text follows.** — ~~**PARTIALLY CLOSED 2026-08-14**~~: sweeps 1 + 2 closed the task-spill 0600 half. **PREREQUISITE DISCHARGED 2026-08-14 (sweep 6): the row's hard blocker — "`crates/cyrup/src/cli.rs` must accept a path form for `--system-prompt` before an over-threshold persona can be spilled to a file" — is DONE.** `cli.rs`'s module doc at `:8` states it reads the `--system-prompt`/`--append-system-prompt` token to decide path-vs-literal, with `resolve_prompt_input` at `:419`/`:451`. **The persona/E2BIG residual is now ordinary in-crate work for area 09, not a cross-area dependency.** |
| ~~SUBA-031~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | M | `wait` scopes runs by cwd, not by session, and says "in this session" — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `background/wait.rs:42-55` no longer states the delta; it carries a `# Scoping (SUBA-031)` section recording that the session filter landed via `WaitDeps::session_id` as the INNER partition under the cwd-derived `async_root`, and explicitly records that **this is what makes the "in this session" empty-set string true** — the exact contradiction the item was filed on. The in-tree delta comment the item quotes verbatim no longer exists. vs pi-subagents v0.43.0 `subagent-wait.ts:265` (`sessionId: deps.state.currentSessionId ?? undefined`). |
| ~~SUBA-032~~ | ~~medium~~ **CLOSED 2026-08-14** | test-defect | S | Notice-debounce test asserts a wall-clock outcome with ~15 ms margin — **CLOSED 2026-08-14**: sweep 1 — `tokio` gained a `test-util` dev-feature in this crate's Cargo.toml, so `start_paused`/`advance` are available to any other wall-clock-marginal test here. |
| ~~SUBA-044~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Bundled `reviewer` agent still grants `bash`/`edit`/`write`; upstream made the lane read-only — **CLOSED 2026-08-14**: sweep 1 — including the second correction (the delegate/worker strict-allowlist paragraph, in-baseline at v0.43.0). All six bundled agents now diff clean against v0.47.1 except researcher.md, whose divergence carries a `[CYRUP-DELTA]` header per SUBA-062. |
| ~~SUBA-045~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | Child tool-availability diagnostic unported — a silently missing tool reports nothing — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `exec/tool_availability.rs` exists and `TOOL_DIAGNOSTIC` is threaded through `exec/mod.rs` and the child-side `prompt_runtime.rs`, which is the whole item. |
| ~~SUBA-046~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | `grant-spawn-budget` unported *and advertised*, so an exhausted cap is terminal for the session — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD by a sweep between 3 and 5. `crates/cyrup-ext-subagents/src/exec/spawn_budget.rs` exists; `extension.rs:805-870` carries `reserve_subagent_spawns` on `preflight_spawn_budget`, a `spawn_budget_snapshot` accessor for the refusal details, and `grant_subagent_spawn_budget`; the verb is in the advertised action list at `extension.rs:5543`; `route_grant_spawn_budget` exists with its first gate documented at `:5491`. **The dependency this row records is DISCHARGED: the SUBA-064 authority consult is live** (`registration/authority.rs:42`/`:55`/`:83`/`:114`/`:130`/`:245`, `AuthorityAction::SpawnBudgetGrant` → `"spawnBudgetGrant"`). vs pi-subagents v0.43.0 `runs/shared/spawn-budget.ts:30`/`:50`/`:55`/`:59`/`:73`/`:85`/`:107`, `shared/types.ts:1885`. **`SUBA-021`'s spawn-budget third closes with it.** |
| ~~SUBA-047~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `toolBudget` honoured but never advertised — **CLOSED 2026-08-14**: sweep 1 — the top-level half; SUBA-007 becomes fully closed. Residual filed: the PER-ITEM `toolBudget` override on `tasks[]`/`chain[]` (schemas.ts:148, :178) is still unadvertised and unconsumed, deliberately, because a `SingleStepSpec`/`RunnerStep` field is needed first and advertising without a consumer is the defect class. |
| ~~SUBA-048~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `artifactDir` config key unported — `session` and `temp` are unreachable — **CLOSED 2026-08-14**: sweep 1 + 2 — sweep 1 REWROTE the Impact (its central claim was measurably wrong: both run-artifact sites called `artifacts::temp_artifacts_dir(cwd)` directly, so runs already landed in temp, NOT in `<cwd>/.cyrup-subagents` — the defect was the inverse, an unreachable `project` default). Sweep 2 closed it: new `artifacts::resolve_chain_runs_dir` (pi `getChainRunsDir`), and both run-artifact sites — `run_foreground_impl` and `spawn_background` — routed through `artifacts::resolve_artifacts_dir(session_file, Some(cwd), cwd, cfg.artifact_dir_preference())`. **Closes PARITY-GAPS PB-13 with it, as PB-13's own text instructs**: `project_chain_runs_dir` had ZERO references crate-wide before this. |
| ~~SUBA-049~~ | ~~medium~~ **PARTIALLY CLOSED 2026-08-15** | not-ported | M | Steer ack, delivery `mode` and `steeringRecovery` unported — a steer is fire-and-forget — **THE ACK PATH AND `mode` ARE CLOSED 2026-08-15 (sweep 11); `steeringRecovery` is handed back with its measured size.** The channel is no longer one-way. **New in `background/control.rs`** (~330 lines): `SteerDeliveryMode` (+`parse`/`as_str`/`next`), `SteerDeliveryStatus`, `SteerAckState`, `SteerAck`, `SteerCapability`, `MAX_STEER_QUEUE_SIZE = 20`, `steer_capabilities_dir`/`steer_capability_path`/`steer_acks_dir`/`steer_ack_file_name`/`steer_ack_write_path`/`write_steer_ack_at`/`write_steer_ack`/`write_steer_capability_at`/`read_steer_capability`/`consume_steer_acks`/`take_steer_acks`, plus `SteerRequest.mode` and `request_async_steer_with_mode` (which RETURNS the minted id — the correlation key the wait needs). vs pi `runs/background/control-channel.ts:63-104,154-253,304-343,396-415` @v0.43.0, every path and file-name format ported including the `(ts, state-order, state)` ack name that makes a `queued`-then-`delivered` pair replay in lifecycle order. **Parent env** — `RunOptions::steer_ack_dir`/`steer_capability_path` → `CYRUP_SUBAGENT_STEER_ACK_DIR`/`CYRUP_SUBAGENT_STEER_CAPABILITY` in `build_attempt_spawn_plan` (pi `pi-args.ts:101-102,764-768`), derived by `runner_main.rs`'s new `steer_ack_dir_for`/`steer_capability_path_for` off the SAME flat index as `steer_inbox_for`, so the request hop and the answer hop cannot address different children. **Child** — `SteeringInbox` gained `acknowledge`, `publish_capability`, `on_turn_start`, `on_turn_end`, an async `dispose` and a follow-up queue; it now writes exactly one ack per consumed request on EVERY path (delivered / queued / failed-no-host / failed-queue-full / failed-inject), publishes a capability record carrying its own pid, subscribes to `TurnStart`, and honours `mode` — `follow_up` parks and is released ONE per turn boundary, `auto` parks only mid-turn. **Parent wait** — `control_steer` takes `mode`, validates it with its own refusal, and polls `take_steer_acks` for 3 s (pi's `ackTimeoutMs ?? 3_000`) before answering pi's `Steering {delivered\|queued\|failed\|pending} for async run {id} (request {rid}).` **Closes `fleet.rs` delta 1 as a side effect**: the FleetView `Tab` mode cycle was logged and then dropped, and the file carried a SECOND `SteerDeliveryMode` enum for want of an argument to pass it to — that duplicate is deleted and re-exported from the wire type. **6 new tests** in `src/tests/steer_delivery_integration.rs` incl. both halves of the item's own Verify, and **ONE PRE-EXISTING TEST WAS CORRECTED**: `a_failed_injection_returns_the_undelivered_guidance_to_the_inbox` required the FAILED request to be re-delivered; upstream writes back only `requests.slice(index + 1)` (`subagent-prompt-runtime.ts:390-391`) and acknowledges the failed one instead — retrying it now would deliver it late and out of order behind newer guidance. **RESIDUAL, sized rather than stubbed: `steeringRecovery`.** It is not a parameter, it is a subsystem — `runs/background/steering.ts` (steering status, per-target state, `claimSteeringRecovery`/`remainingSteeringRecoveryLimits`/`waitForSteeringAction`/`actionResultFromSteeringStatus`) plus the pause-and-revive half of `runs/foreground/async-steering-action.ts:135-215` (interrupt, confirmed-pause poll, late-ack reconciliation, `resolveAsyncResumeTarget`, replacement launch, `subagent.steering.notice` events) — **~450 upstream lines and a new `status.json` sub-record**, i.e. L, not the M this row was filed at. It is deliberately NOT advertised on the schema: an unconsumed `steeringRecovery` boolean is exactly the advertised-and-inert defect class this area keeps filing (`SUBA-047`, `SUBA-054`, `SUBA-061`). **SWEEP-11 REVIEW FIX (adversarial verification):** the async port of `SteeringInbox::flush` reproduced upstream's `try` but not its `finally`. Upstream's `flush` is a SYNCHRONOUS `(): void` whose body is wrapped in `try { … } finally { flushing = false; }` (`subagent-prompt-runtime.ts:381-413`), so its re-entrancy latch cannot outlive the call; cyrup's is `async`, awaits at `consume_steer_requests_from_dir`, at every `acknowledge` and at the write-back, and is driven both from the poll task and from the turn-lifecycle handlers — so a dropped future latched `flushing = true` PERMANENTLY, every later flush took the early return, and the inbox went silently deaf for the rest of the run (requests keep being consumed off disk but never acknowledged, so the parent's `await_steer_ack` only ever times out to `pending`). Fixed with a `FlushGuard` RAII type that clears the latch in `Drop`, pinned by `prompt_runtime.rs::a_dropped_flush_future_does_not_wedge_the_steering_inbox` — verified RED before the guard (polls `flush` once, drops it, then asserts both the latch and that a following flush still drains the inbox) and green after. Also corrected the PRE-EXISTING `extension.rs::steer_action_writes_a_control_inbox_request_for_a_running_run`, which still pinned the cyrup-original "Steering queued … Delivery requires a live Cyrup child session" text this item replaced with upstream's own sentence. |
| ~~SUBA-050~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `subagents.modelScope.strict` unported — inherited/fallback out-of-scope models can only warn — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-051~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Async **child** runs have no default wall-clock timeout; upstream bounds them at 30 minutes — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-052~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | YAML literal block scalars (`\|`, `\|-`) parse to the literal string `"\|"` — **CLOSED 2026-08-14**: sweep 1 — the parser half. The second sentence (route `parseSkillDescription` through the shared parser, as `a4fc59a` did) is NOT done and is cross-crate: skill descriptions come from `cyrup_resources::Skill.front.description`, not from this crate's parser. Filed against the resources area. |
| ~~SUBA-053~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `~` never expanded in chain read/write paths — **CLOSED 2026-08-14**: sweep 1. |
| SUBA-054 | medium — **PARTIALLY CLOSED 2026-08-14** | upstream-drift | M | `defaultReads` never reaches a single run — no `[Read from: …]` outside chains — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — the FOREGROUND SINGLE half is closed, which is the item's headline and its whole Verify recipe: `RunOptions::reads` (the declared, unfiltered list, pi's `reads` binding) populated from `agent.default_reads`, and `build_task_text` prepending `[Read from: …]\n\n` through two new shared helpers in `spawn/chain_graph.rs` (`resolve_existing_read_paths`, `build_single_reads_instruction`); `build_chain_instructions` was refactored onto the same helper so the two paths cannot drift. The separator difference is deliberate and both forms are upstream's. **FIX LINE CORRECTED: "add the `reads` param on the async path" — upstream has NO top-level `reads` param.** `extension/schemas.ts`'s `SubagentParamProperties` has no such key and the three `reads` entries at :144/:174/:204 are all per-ITEM, so `defaultReads` is the entire SINGLE precedence chain and no new advertised param is owed. **RESIDUAL, and the blocker is written into `background/runner_main.rs` at the field rather than silently defaulted: the ASYNC half needs a decision — a runner step already gets its read line from `build_chain_instructions` resolved against the CHAIN dir, while upstream's async single resolves against `effectiveCwd`, so setting `RunOptions::reads` there would double-emit.** **RE-CONFIRMED UNCHANGED 2026-08-14 (sweep 8): this needs a DECISION on which cwd an async single step's read instruction resolves against — closing it means teaching the step builder which of the two cwds applies. It is not agent work as filed** (`pi-subagents/src/runs/background/async-execution.ts:1300-1302` @v0.43.0). The SUBA-044 interaction is moot — reviewer.md no longer carries `defaultReads`. |
| ~~SUBA-055~~ | ~~medium~~ **CLOSED 2026-08-15** | upstream-drift | M | The `guide` action and its packaged version-matched docs unported — **CLOSED 2026-08-15 (sweep 11), together with its last mile `SUBA-066`.** New `crates/cyrup-ext-subagents/src/registration/guide.rs` ports `SUBAGENT_GUIDE_TOPICS` (`extension/subagent-guide.ts:5-16` @v0.47.1, all ten in upstream's order), `isGuideTopic` (`:22`) and `readSubagentGuide` (`:26-38`) with the unknown-topic sentence byte-identical — *"Unknown subagents guide topic '<t>'. Valid topics: … . No files were changed."* — returned as ORDINARY TEXT, not an error, exactly as upstream returns it, because the caller is a model and an error costs it a turn. `guide` added to `SUBAGENT_ACTIONS` in pi's own position (after `models`, `shared/types.ts:1968` @v0.47.1), dispatched from `route_action`, with `topic` advertised as pi declares it (`schemas.ts:281`: a bare optional string, **no enum and no description** — reproduced including the absence, since an invented description would be cyrup-original model-facing text). **The docs are cyrup's own, describing cyrup's surface** — a new `resources/docs/` set (README + nine topics, ~35 KB) written from `docs/guide/extensions/subagents.md` and the crate's real surface, NOT pi's 130 KB `docs/` tree, which documents verbs and config keys this build does not have. **ONE `[CYRUP-DELTA]`, in-source**: `include_str!` instead of `fs.readFileSync` off a resolved `packageRoot`, which makes the version-matching STRUCTURAL rather than conventional (upstream's on-disk docs can be edited after install and drift from the binary; these cannot) and removes upstream's only throw, so the function returns `String` rather than `Result`. **A missing doc file is now a compile error.** **4 new tests**, one of which — `the_tool_reference_topic_names_every_dispatched_verb` — pins the packaged tool-reference page against `SUBAGENT_ACTIONS`, so a later verb added without documenting it goes red. **RESIDUAL: `children.list`**, the second verb this row claimed. NOT ported and the reason is recorded on the enum rather than left to inference: upstream's `children.list` lists RETAINED children (completed single runs held open under a `parentWorkflowRunId`, `subagent-executor.ts:4993-5000` @v0.47.1), which is part of the unported `workflowScript` shape — cyrup has no `parentWorkflowRunId` and no retained-child concept, so the verb would advertise a listing that is always empty. It returns to `SUBA-005`'s unowned-verb list. |
| SUBA-056 | medium | upstream-drift | L | Durable completion replay and output archives unported |
| ~~SUBA-057~~ | ~~medium~~ **CLOSED 2026-08-15** | upstream-drift | M | `dismiss` unported — a recovered workflow with no live controller is stuck "running" forever — **CLOSED 2026-08-15**: sweep 9 — **the READ half was already at HEAD and only the PRODUCER was missing.** `RunStatus::display_dismissed_at`, `ReconcileAction::DisplayDismissed`, `list_active_runs`'s `continue` and `format_display_dismissed_status`'s `State: display-dismissed` report all existed, and `background/mod.rs:933` already `[]`-linked a `SubagentExecutor::control_dismiss` that **did not exist** — a broken intra-doc link over a field with zero writers. Landed: `SubagentExecutor::control_dismiss` (`extension.rs:4222`) porting all five refusals in upstream's order with byte-identical sentences, the result-file re-reconcile, the stamp + `write_atomic_json`, the post-write re-reconcile (testing `ReconcileAction::DisplayDismissed` as the carrier of upstream's `status: null`, per that variant's own doc contract) and the `JobTracker::untrack` eviction; `dismiss` added to `SUBAGENT_ACTIONS` at pi's own position (between `stop` and `append-step`, `shared/types.ts:2084` @v0.47.1) and to `route_control_action`; the child-safe refusal (`subagent-executor.ts:5865-5870`, `dismiss` ∈ `MUTATING_MANAGEMENT_ACTIONS` `:175`) spelled on the arm rather than by widening `discovery::management::MUTATING_MANAGEMENT_ACTIONS`, which gates a different dispatcher; `background::control::read_status_file` raised to `pub(crate)` so the refusals judge the RAW record as pi's `readStatus` does. **4 new tests**, incl. the item's own Verify end to end through a real tool call. **TWO `[CYRUP-DELTA]`s recorded on `control_dismiss`**: (a) upstream's `status.mode !== "workflow"` half is NOT ported — cyrup's `RunMode` (`background/mod.rs:242`) has no `Workflow` variant because that fourth `SubagentRunMode` member (`shared/types.ts:231`) belongs to the unported `workflowScript` shape, and adding a variant nothing constructs would make the gate refuse every run and leave the verb unreachable; only the `!status` half is ported. (b) upstream's `state.workflowControllers.has(runId)` is an in-process `AbortController` map; cyrup drives every background run from a **detached process**, so the live-controller test is a zero-signal liveness probe of the recorded pid — which also identifies cyrup's exact analogue of the reload-orphaned run the verb exists for: a `Running` status with `pid: None`, which `reconcile`'s step 3 falls through as `NoneNeeded` and can therefore never advance. vs pi-subagents v0.47.1 `runs/foreground/async-dismiss-action.ts:11-85`, `runs/foreground/subagent-executor.ts:5872-5885`. |
| ~~SUBA-064~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | The whole `authorityPolicy` subsystem is unported, and the `stop`/`steer` gate it drives is live-reachable — **CLOSED 2026-08-14**: sweep 1 — the subsystem + the stop/steer gate. Hard prerequisite carried verbatim into SUBA-005's unowned-verb list: whoever lands `worktree.discard` or `destructiveCleanup` must route it through `registration::authority::resolve_authority_decision` in the same change. `spawnBudgetGrant` is now pre-wired for SUBA-046 and `scheduleCreate` for SUBA-016, so both are cheaper than filed. |
| SUBA-017 | low | not-ported | M | Completion batching unported (**in-baseline**, not drift) |
| SUBA-022 | low | not-ported | L | Typed extension delegation API unported (**in-baseline**, not drift) |
| SUBA-023 | low — **PARTIALLY CLOSED 2026-08-14** | upstream-drift | L | Async lifecycle hardening unported; no signal-name attribution — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — the signal-name attribution half closed in sweep 1; sweep 2 closed the consumer half sweep 1 left open. `exec/mod.rs::process_signal_name` carried its OWN three-entry table (SIGINT/SIGKILL/SIGTERM) plus a `SIG{n}` fallback, so a child that segfaulted reported `SIG11` and one that aborted `SIG6` on `SingleResult.process_signal`, where pi passes Node's signal NAME through and reports SIGSEGV/SIGABRT. It now delegates to `spawn::signal::signal_name_of`, the single crate-wide mapping; the numeric form survives only as the fallback for a signal that table does not name. The dead `libc_signal` module is deleted. **RESIDUAL: the two unported upstream subsystems only — `process-terminal.ts` and `session-lease.ts` (= VL-S3 / VL-S4).** |
| SUBA-024 | low | upstream-drift | L | `parallel-handoff` / `agent-contract` unported (`task-intent` closed; `chain-validation` struck) |
| SUBA-026 | low | upstream-drift | L | Interactive admin UI, selector and `/subagents` unported (`/subagents-stop` landed) |
| ~~SUBA-029~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Management actions read-modify-write subagents `settings.json` unlocked — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-033~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | Tests assert a lower bound on observed concurrency — **CLOSED 2026-08-14**: sweep 1 — both lower bounds removed; overlap is enforced by a `tokio::sync::Barrier` rendezvous inside the worker with a bounded wait, so a serialization regression fails loudly instead of hanging. |
| ~~SUBA-034~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED / already-done** | not-ported | M | `wait`'s event-bus wake unported; pure polling at a 1 s floor — **REFUTED, CLOSED 2026-08-15**: sweep 11 read the file at HEAD and the whole item is already in, landed by **sweep 9 (`844e25f`)** and never marked. `background/wait.rs` carries a `# Wake mechanism (SUBA-034)` module section, `WaitDeps::completion_bus` threads `watch::CompletionBus`, and the loop `select!`s a `broadcast::Receiver` against the sleep with `biased;` (cancellation first, then the wake, then the timer) — the `Lagged`-is-a-wake / `Closed`-retires-the-subscription handling is there too, which is the part a naive port gets wrong (a closed receiver returns instantly forever and spins the loop at 100% CPU). The subscription is taken BEFORE the first listing, with the race argument written out at the call. The surviving delta is a LATENCY FLOOR, not a mechanism gap, and is recorded in-source: cyrup's publisher is `ResultsWatcher`, bounded below by its own 500 ms `RESULTS_DIR_POLL_INTERVAL`, so what the bus removes is the SECOND 1 s interval stacked on top. `completion_bus: None` is upstream's own no-bus degradation. **No fix was manufactured and no test was added** — the item's Verify is already covered by the code's own tests. |
| ~~SUBA-035~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED / already-done, and its residual's stated LOCATION was wrong too** | not-ported | S | Active `subagents.modelScope` policy not surfaced by doctor/models — **CLOSED 2026-08-14**: sweep 8. **Both halves are in at HEAD.** Doctor: `registration/doctor.rs:646-676` `model_scope_check` (four arms), called at `:603`, tests at `:1609`-`:1642`. Models report: `crates/cyrup-ext-subagents/src/extension.rs` — the single-agent view (~`:3565-3572`) and the all-agents view (~`:3579-3585`) each push a `Model scope:` line built by `crate::exec::model_scope::model_scope_summary_line`, and each **cites SUBA-035 by name in-source**. **CORRECTION THAT MUST SURVIVE THE CLOSURE: the residual's stated location — `registration/mod.rs` / `profiles.rs` — is wrong.** The models report is `extension.rs::run_models_report`, so anybody following this row would have hunted in two files that never carried the surface. Port target `pi-subagents/src/runs/shared/model-scope.ts` @v0.43.0. |
| ~~SUBA-037~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Doctor's `--version` probe leaks the probe process on timeout — **CLOSED 2026-08-14**: sweep 1 — no test was added and the reason is recorded: `VERSION_PROBE_TIMEOUT` is not injectable and the flag's effect is a tokio guarantee. |
| ~~SUBA-038~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Three denial/unknown-action messages still diverge from pi's text — **CLOSED 2026-08-14**: sweep 1 — residual 2 was closed by porting the v0.47.1 message (SUBA-065) rather than the v0.43.0 text, since the two items rewrite the same three strings and the richer form supersedes the bare one. |
| ~~SUBA-039~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | cyrup-original | M | `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a group — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `spawn/mod.rs:849` is `impl Drop for SpawnedChild`, and `:394` documents the `Option` field that exists specifically to make that `Drop` work. **This is the JS→Rust guarantee-gap class (a dropped future orphaning a detached process group), which is why sweep 6 checked it first among the lows.** No upstream counterpart — pi's async functions always settle. |
| ~~SUBA-058~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | Chain read instructions not filtered by existence — **CLOSED 2026-08-14**: sweep 1 — citation drift recorded: the pinned expectation is at `spawn/chain_graph.rs` in `build_chain_instructions_emits_reads_output_prefix_and_previous_suffix`, not at the stated `:2522`. |
| ~~SUBA-059~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `artifactConfig.cleanupDays` never wired to the type that already parses it — **CLOSED 2026-08-14**: sweep 1 — one correction: the item says to add `artifact_config: Option<ArtifactConfig>`; upstream's type is `Pick<ArtifactConfig,"cleanupDays">`, so the new field is `Option<ArtifactRetentionConfig>` (one key). The full struct would advertise five per-run switches upstream does not read from config. |
| ~~SUBA-060~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | upstream-drift | S | "Resume-first" guidance for failed async runs unported — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `background/resume_guidance.rs` exists and is referenced from `background/wait.rs` and `background/mod.rs`. vs pi-subagents v0.47.1. |
| SUBA-061 | low | not-ported | M | Four config keys silently ignored: `asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls` |
| ~~SUBA-062~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | L | Bundled `researcher` cannot do web research — the crate's target has no web tools — **CLOSED 2026-08-14**: sweep 1 — the in-crate half (the `[CYRUP-DELTA]` header). The handoff to areas 04/12 for `web_search`/`fetch_content`/`get_search_content` stands. |
| SUBA-063 | low | not-ported | M | Zero-tool-budget authorisation and the runtime-extension acknowledgement path unported |
| ~~SUBA-065~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `unknownSubagentActionMessage` — did-you-mean recovery and its destructive-action gate — unported — **CLOSED 2026-08-14**: sweep 1 — `SUBAGENT_ACTIONS` is now a single const feeding BOTH the schema enum and the message, which is the structural half of SUBA-005's owed "completeness assertion": the enum can no longer drift from the advertised list, only from upstream's. |
| ~~SUBA-066~~ | ~~low~~ **CLOSED 2026-08-15** | upstream-drift | S | `/subagents-guide` slash command unported (outside both VL-S11 and SUBA-055) — **CLOSED 2026-08-15 (sweep 11), landed with SUBA-055 as that item's Fix line instructed.** `SlashCommandName::SubagentsGuide` + its descriptor (`description` upstream's verbatim, `slash-commands.ts:707` @v0.47.1) and a `dispatch_slash` arm routing to the SAME `registration::guide::read_subagent_guide` the `guide` action uses — two readers over one document set cannot disagree about the current contract. Upstream's multi-word-argument refusal (`ctx.ui.notify("Usage: /subagents-guide [topic]", "error")`, `:712-715`) is ported as a pre-dispatch refusal rather than falling through to the unknown-topic message, because a two-word argument is a usage mistake and the topic list is the wrong answer to it. **2 tests**, one of which is the pre-existing table-count assertion moved from `+ 4` to `+ 5` — red before the variant existed. |
| ~~SUBA-067~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | Descendant-termination fixture exec-collapses to one pid; test never exercised group-kill (FIXED) — **CLOSED 2026-08-14**: closed pre-sweep — the descendant-termination fixture exec-collapsed to one pid, so the test never exercised group-kill. Found by RUNNING the suite. |
| ~~SUBA-068~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | Setup-hook timeout fixture races macOS ~200 ms first-exec verification with a 200 ms budget (FIXED) — **CLOSED 2026-08-14**: closed pre-sweep — the setup-hook timeout fixture raced macOS's ~200 ms first-exec verification with a 200 ms budget. Found by RUNNING the suite. |
| ~~SUBA-069~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | M | Setup-hook test family is wall-clock-budgeted; 3 siblings go red under machine load (OPEN) — **CLOSED 2026-08-14**: sweep 2 — **and TWO FACTUAL ERRORS IN THE ITEM ARE CORRECTED FIRST.** (1) "pi's default is the same 5000 ms" is wrong: pi's constant is **30000** at both v0.43.0:113 and v0.47.1:114 (`DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`), and cyrup's already matched it. (2) "These use the production DEFAULT 5000 ms hook timeout rather than a fixture constant, so unlike SUBA-068 they cannot simply be re-budgeted" is wrong: all three fixtures passed `timeout_ms: Some(5_000)` explicitly at HEAD, so they were always re-budgetable. Fix landed options (a)+(b) together: `write_hook_script` emits a `[ -n "$CYRUP_HOOK_WARMUP" ] && exit 0` guard as line 2 and a new `warm_hook_exec` helper pays macOS's one-off first-`exec` verification (measured 197-242 ms) OUTSIDE any timeout budget, and the four non-timeout fixtures now use the SHIPPED 30 s default. The production constant is untouched. **The Verify (green under deliberate load ≥8, 3×) was NOT executed — the warm-up mechanism is unit-pinned, the flake reduction is not measured.** |
| SUBA-070 | low | not-ported | M | The `interactive` frontmatter key is parsed into a typed field but never enforced — **filed 2026-08-14** by a documentation audit; deliberate for v1 per an in-source note, recorded here so a later pass does not re-derive it as a discovery. |
| ~~SUBA-071~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED** | cyrup-original | M | Subagent settings are read from two files that can disagree, with no merge and no precedence rule — **REFUTED, CLOSED 2026-08-15 (sweep 11). There is exactly ONE settings store in this crate, and its precedence is upstream's.** The item's premise rests on a claim that `SubagentsSettingsView` is *"read via `SettingsManager::effective().get(\"subagents\")`"*. **That read does not exist and never has**: `rg 'SettingsManager|effective\(\)' crates/cyrup-ext-subagents/src` matches PROSE ONLY — a stale doc comment on the struct — and the struct's only data-carrying constructor is `SubagentsSettingsView::from_subagent_settings`, whose input is the discovery-side result. The item's own cyrup citation is wrong too: `registration/mod.rs:625-640` is `ProactiveSkillSubagents`, not a settings read. **What actually exists is pi's own arrangement**: `~/.cyrup/agents/settings.json` (user) and `<project_root>/.cyrup/agents/settings.json` (project), layered by `discovery::load_layered_subagent_settings` with **project beating user** on every scalar and every per-agent override — pi resolves `defaultModel` at `agents/agents.ts:924-931` @v0.43.0, `defaultThinking` at `:949-951`, `defaultExtensions` at `:969-971`, from `getUserAgentSettingsPath()` (`:674-676`) and `getProjectAgentSettingsPath(cwd)` (`:678-681`). So there is nothing to merge and no precedence to invent, and the item's Verify (*"set the same key to different values in both files, assert the documented winner"*) is ALREADY pinned at `discovery/mod.rs:2008` and `:2025`. `~/.cyrup/agent/settings.json` — `cyrup_config::Dirs::settings_path()` — is a different file this crate never reads; `profiles.rs:227-232`, the in-source note the filing cites, says exactly that, and it is a record of a store-based writer being DELETED for aiming at it, i.e. the divergence being closed, not open. **What was fixed is the documentation that caused the filing**: the stale sentence is deleted rather than softened (a doc describing a read path that does not exist is indistinguishable, to a reader, from one that does), the struct doc now names the real pair with its upstream citation, and `registration/mod.rs`'s R-SA-133 tier-2 header no longer says *"`cyrup-config`'s effective (CLI ▷ project ▷ global) settings view"*. **No test was added and the reason is stated rather than papered over: there is no defect to make red.** |

**45 items — 0 critical, 2 high, 23 medium, 20 low.** Per structural defect A in
`00-residual-ledger.md`, treat the count as a floor.

> **SWEEP 11, 2026-08-15 — the steering/guide/settings cluster.** Five rows moved: `SUBA-055` and
> `SUBA-066` closed together (the `guide` action, a packaged `resources/docs/` set embedded with
> `include_str!`, and the slash command over the same reader); `SUBA-049` partially closed (the
> acknowledgment path, the delivery `mode`, and the capability record are in end to end — a steer is
> no longer fire-and-forget — with `steeringRecovery` handed back as **L**, not the **M** it was
> filed at, and deliberately NOT advertised on the schema until it has a consumer); and **two were
> REFUTED against the code at HEAD** — `SUBA-034` (the `CompletionBus` wake landed in sweep 9 and the
> row was never marked) and `SUBA-071` (the "two settings files" premise cites a
> `SettingsManager::effective()` read that exists nowhere in the crate; the real arrangement is pi's
> own user◁project pair, already layered and already tested). That is **2 refutations in 5 rows**,
> against this ledger's published ~12% error rate — both were checkable in under a minute with `rg`.
>
> **NOT TAKEN in this cluster, with measured sizes rather than stubs**, because each is a subsystem
> and not a fix: `SUBA-056` (completion replay + output archives — upstream is `completion-replay.ts`
> 267 lines + `wait-completions.ts` 146, plus a `wait`-result projection: **L**), `SUBA-017`
> (completion batching: **M**), `SUBA-061` (four config keys, three of which need a consumer built
> first: **M**), `SUBA-063` (zero-tool-budget auth + runtime-extension acknowledgement: **M**),
> `SUBA-070` (`interactive` enforcement — needs its upstream tag/line ESTABLISHED first, which the
> row itself says was never read: **M**), `SUBA-022` (`api/delegation.ts`: **L**), `SUBA-024`
> (`parallel-handoff.ts` + `agent-contract.ts`: **L**), `SUBA-026` (`slash/subagents-admin.ts`, 432
> lines, + `tui/selector.ts`: **L**). None of them was stubbed, half-wired or advertised.
>
> **One documentation guarantee this sweep took on:** the packaged `resources/docs/` set is
> load-bearing now — it is what `{action:"guide"}` returns — so it must not describe behaviour this
> build lacks. Draft text claiming completion replay and notice batching was removed before landing
> for exactly that reason, and `the_tool_reference_topic_names_every_dispatched_verb` makes the
> verb half of that guarantee mechanical.

## Trackers (excluded from the item count)

These keep their IDs and their bodies but propose no schedulable work: they are indexes over work
other items own. A planner should not pick one up; the next audit pass maintains them.

| ID | Kind | Owner of the actual work | Note |
|---|---|---|---|
| SUBA-005 | tracking | the subsystem items it indexes (SUBA-016, SUBA-046, SUBA-055, SUBA-057, VL-S6, VL-S13) plus seven still-unowned verbs | 27-vs-50/53 management-verb census. Its own Fix says "this item is the ledger, not the work". What it owes is (a) owners for `worktree.discard`, `approve-checkpoint`, `reject-checkpoint`, `project.open`/`project.status`/`project.close`, `mission.resolve-decision`, and (b) a completeness assertion pinning the enum against a checked-in copy of upstream's array. |

---

## SUBA-014 — `requireReadTool` unported, so a skill-carrying agent can be told to `read` a skill it has no `read` tool for

**Kind** not-ported · **Severity** high *(raised from medium this pass)* · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/mod.rs:1463-1491` builds the allowlist verbatim from declared builtins plus resolved MCP tool names, with no `read` head-injection; `rg 'require_read_tool' crates/cyrup-ext-subagents/src` = 0. The same block is SUBA-006's `explicit_tool_allowlist` fix, read in full — the injection is genuinely absent from it.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:355-372` @v0.43.0 injects `["read", ...requestedBuiltinTools]` whenever `requireReadTool && requestedBuiltinTools.length > 0 && !includes("read")`. `git grep requireReadTool v0.43.0` shows **seven** live setters, all deriving it from `Boolean(resolvedSkills.length)`: `async-execution.ts:731,1324`, `subagent-runner.ts:1328,1366`, `execution.ts:322,357`, `preflight.ts:277`.
**Impact** — Severity raised because cyrup's own proactive-skill block tells the child *"Use the read tool to load a skill's file"* (`discovery/skills.rs:273`) while the allowlist may not contain `read`. An agent with an explicit `tools:` list that omits `read` plus any resolved skill silently cannot load that skill: the child is instructed to do something it has no tool for, and the failure surfaces as a model apology rather than a config error.
**Fix** — In `exec/mod.rs:1463-1491`, compute `require_read_tool` from `!resolved_skills.is_empty()` (the value is already in scope at the skill-resolution site) and inject `"read"` at the head of the builtin list under pi's exact three-way condition. Add the config/param surface only if a caller needs to force it independently of skills — upstream never sets it any other way.
**Verify** — An agent with `tools: [bash]` and one resolved skill must spawn with `--tools read,bash`; the same agent with no skills must spawn with `--tools bash`. Table-test the three-way condition including the already-contains-`read` case.

## SUBA-043 — A SINGLE-mode subagent call can never declare `outputSchema` — the param is unadvertised and the field is hardcoded `None` on both single paths

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6543-6690`: `subagent_tool_parameters()` emits 45 properties and `outputSchema` is not among them — it exists only on the `tasks[]`/`chain[]` **item** schemas (`:6300`, `:6326`, `:6371`, `:6389`). Both single-run construction sites pin the field: `structured_output_schema: None` at `:1934` (foreground SINGLE `RunOptions`) and at `:2295` (async SINGLE `SingleStepSpec`, where only `output_path`/`output_mode`/`skills`/`session_dir` are threaded). The only populated path is the `tasks[]` item lowering at `:5849`.
**upstream** — `pi-subagents/src/extension/schemas.ts:349` @v0.43.0 — `outputSchema: Type.Optional(JsonSchemaObject)` is a **top-level** `SubagentParamsSchema` property, under the "Workflow defaults forwarded to each child" comment; `runs/foreground/subagent-executor.ts:3515` (`runSinglePath`) reads `params.outputSchema` at `:3651` and `:3671`, and `pi-args.ts:759-762` writes the resulting schema/capture pair into the child env.
**Impact** — The mechanism SUBA-S01 was closed to deliver is unreachable from the surface a model actually calls. `subagent({agent, task, outputSchema:{…}})` is accepted (the root schema is `additionalProperties: true` and `SubagentToolParams` has no `deny_unknown_fields`), the schema is dropped without error, the run completes as free prose, and the orchestrator that asked for typed output gets untyped text. The only workaround is wrapping a one-item `tasks:[{…}]` — the exact SUBA-041-era shape the maintainer rejected as a workaround.
**Fix** — Add `outputSchema` to `subagent_tool_parameters()` using the existing `sj_json_schema_object()` helper the item schemas already use, deserialize it onto `SubagentToolParams`, and thread it into `RunOptions::structured_output_schema` at `extension.rs:1934` and onto the async step at `:2295` (the runner already carries the field; only the constructors pin it).
**Verify** — `{agent:"x", task:"y", outputSchema:{type:"object",properties:{n:{type:"number"}},required:["n"]}}` must set both structured-output env vars on the child, and a child that never calls `structured_output` must fail with `STRUCTURED_OUTPUT_MISSING_ERROR` rather than returning prose. Add the schema/dispatch guard test asserting every advertised property has a consumer — the single test that would have caught this, SUBA-N05 and SUBA-047 together.

## SUBA-005 — Management actions: 27 advertised against upstream's 50 (v0.43.0) / 53 (v0.47.1)

**`tracker`** — not counted in this area's 45 open items. **Kind** tracking · **Severity** n/a *(was medium)* · **Effort** n/a · **Confidence** confirmed

> **Reclassified in the 2026-08-12 repair pass.** The item's own Fix line reads "this item is the
> ledger, not the work", which is the definition of bookkeeping: every schedulable half belongs to a
> subsystem item (`SUBA-016`, `SUBA-046`, `SUBA-055`, `SUBA-057`, PARITY-GAPS VL-S6/VL-S13), and the
> two things this ID actually owes — owners for the seven unowned verbs, and a completeness assertion
> — are maintenance the next audit pass performs. Body retained unchanged below so the census is not
> lost; the ID is retained per the never-delete rule.

*(Restated: the prior "15 of 20" framing was stale in both directions. Counts below were re-derived by enumerating both arrays element by element.)*
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — the action enum lists **27** verbs.
**upstream** — `pi-subagents/src/shared/types.ts:1885` @v0.43.0 has **50**; `:1968` @v0.47.1 has **53**.
**Impact** — Set-differencing the two arrays: missing **and covered by another item** — `schedule.*` (9, SUBA-016 / PARITY-GAPS PB-11), `refine*` (3, VL-S13), `inspector.*` (3, VL-S6). Missing and covered by **no** item until this pass — `children.list`, `worktree.discard`, `grant-spawn-budget`, `approve-checkpoint`, `reject-checkpoint`, `project.open`/`project.status`/`project.close`, `guide`, `dismiss`, `mission.resolve-decision`; all confirmed zero-hit by `rg` over `crates/cyrup-ext-subagents/src`. Four of those now have owners: **SUBA-046** (`grant-spawn-budget`), **SUBA-055** (`guide`, `children.list`), **SUBA-057** (`dismiss`). The rest are unowned. Because `SubagentToolParams` carries no `deny_unknown_fields`, accompanying params are dropped *before* the unknown-action error fires, so the error never explains what was actually wrong.
**Fix** — This item is the ledger, not the work: each subsystem absorbs its own dispatch half (enum entry at `extension.rs:6557`, arm in `route_management_action`/`route_control_action`, and the completeness assertion). What is owed *here* is (a) owners for the seven still-unowned verbs, and (b) restoring a completeness assertion that pins the enum against a checked-in copy of upstream's array so the count cannot silently drift again.
**Verify** — The completeness assertion holds the same name vector as `shared/types.ts` at the pinned tag, and every advertised verb dispatches to something other than the unknown-action arm.

## SUBA-008 — `turnBudget` unported; the only consumers read a hard-coded `false` — **CLOSED 2026-08-14**

> **CLOSED 2026-08-14 (sweep 8). FOUR ERRORS IN THE BODY BELOW ARE CORRECTED HERE FIRST, because
> following one of them would have shipped a budget that nothing enforced.**
>
> **(1) The `Fix` line's mechanism is WRONG, and it is the most valuable finding on this row.** It
> says *"Port as `exec/turn_budget.rs` mirroring `exec/tool_budget.rs`'s env-handoff shape"*. The tool
> budget is env-var plus **child-side refusal** (`tool-budget.ts:70-80`, `PI_SUBAGENT_TOOL_BUDGET`).
> **The turn budget is the OPPOSITE shape.** `git grep -n TURN_BUDGET v0.43.0 -- src/` matches **only
> `runs/shared/turn-budget.ts` itself** — there is no env var and no child-side enforcement. The child
> is only **told**, via a system-prompt block; the **SUPERVISOR** enforces, by counting assistant
> `message_end` events off the child's NDJSON stdout and signalling it down
> (`foreground/execution.ts:910-924`, `:733-757`). A faithful-*looking* env-shaped port would have
> advertised a budget nobody enforced.
>
> **(2) The `Verify` line's key is WRONG.** It says `turnBudget:{hard:2}`. **`hard` is the TOOL
> budget's key.** The turn budget takes `{maxTurns, graceTurns}` (`extension/schemas.ts:104-107`
> @v0.43.0), and upstream **rejects `hard` by name** — `turnBudget.hard is not supported.`
>
> **(3) The effort rating was WRONG, and sweep 6's recount doubled down on it** ("the cheapest
> remaining medium in this area … WIRING plus a schema key, not a port"). Measurably false: **cyrup's
> `Usage` carries no `turns` counter at all** — `exec/fallback.rs:919` says so in its own doc — so
> there was no turn count to wire. **Re-rated M/L.**
>
> **(4) The "three consumers reading a hard-coded `false`" count was WRONG, and the miscount hid a
> quieter failure.** Only **two** were reachable (`tui/intercom.rs:355`, `:448`).
> `exec/fallback.rs:915` was **already reading the field correctly** and merely had no producer —
> which means `is_retryable_subagent_startup_failure` would have **RELAUNCHED the very model that
> blew its turn budget**. A correct consumer starved of a value is a different and quieter defect
> than a consumer reading a constant.
>
> **WHAT LANDED.** New `crates/cyrup-ext-subagents/src/exec/turn_budget.rs` (888 lines, verified at
> HEAD) plus its wiring. **Module:** `resolve_turn_budget_config` (upstream's verbatim error strings,
> its first-unknown-key scan, its `?? DEFAULT_TURN_BUDGET_GRACE_TURNS`, and `Number.isInteger(2.0)
> === true` so a fractionless float is accepted as upstream accepts it),
> `append_turn_budget_system_prompt`, `turn_budget_soft_note` / `_exceeded_message` /
> `_deferred_note`, `format_turn_budget_output`, `prepend_turn_budget_note`,
> `initial_turn_budget_state` / `turn_budget_state` / `turn_budget_deferred_state`,
> `turn_budget_decision`, and a `TurnBudgetTracker` gathering pi's four `updateTurnBudget` locals.
> **Enforcement (`exec/mod.rs`):** `drive_attempt` counts assistant `message_end` events
> (`progress.turn_count()` is this port's `result.usage.turns`), computes pi's
> `terminalAssistantStop || terminalStructuredOutputCall` and `hasToolCall ||
> Boolean(progress.currentTool)`, raises the one-time soft note into `progress.recent_output`, and on
> abort walks `SpawnedChild::terminate_with_graces`. **Composition:** the budget block is appended
> LAST onto the system prompt, after persona→skills→memory→refinement→output-path, matching
> `execution.ts:326` reading `shared.systemPrompt` from `:1443`. The abort message is the run's error
> at the TOP of the diagnosis chain (upstream sets `result.error` at abort time and its close handler
> only fills an unset one, `:1099`). The terminal output fold is pi's `else if` chain off `if
> (result.timedOut)`. **Surface:** `turnBudget` tool param + `TurnBudgetOverride` schema (in
> upstream's own property slot, immediately above `toolBudget`), `turnBudget:` agent frontmatter →
> `AgentDefinition::default_turn_budget` (+ `KNOWN_FIELDS` + a `serialize_agent` arm, so a management
> rewrite cannot silently delete an author's budget), and the `subagents.turnBudget` config key,
> resolving caller > frontmatter > config at one point per path (`subagent-executor.ts:4928` after
> `applySingleAgentLaunchDefaults`). **Result shape:** `SingleResult` gained `turn_budget` /
> `turn_budget_exceeded` / `wrap_up_requested`, all `skip_serializing_if`, so pre-existing
> `status.json` round-trips byte-for-byte.
>
> **`[CYRUP-DELTA]`, recorded at the call site rather than silently matching the observable
> outcome.** pi ARMS two `setTimeout`s inside `requestTurnBudgetAbort` and **keeps reading the
> child's stdout during the window**, so a child that wraps up inside it still delivers output.
> cyrup's `SpawnedChild::terminate` **CONSUMES** the child — there is no seam that signals without
> taking it — so the ladder blocks the drive loop for the same wall-clock window instead. A late final
> message written after the SIGINT is **dropped here where upstream would have read it**, which is why
> the abort message doubles as `final_output`. **The graces are pinned to reproduce upstream's
> ABSOLUTE instants: SIGINT, SIGTERM at +1 s, SIGKILL at +4 s** — pi arms both timers from the same
> moment, so the real SIGTERM→SIGKILL gap is **3 s, not 4 s**. Reading `execution.ts:752` alone gives
> the wrong number.
>
> **VERIFY — 14 new tests, all green.** `exec::turn_budget::tests` (10): the resolver's defaults and
> every verbatim rejection message; the system-prompt block's exact text with both pluralisations;
> the decision table including that a TERMINAL assistant stop is never aborted however far past the
> hard limit; the tracker's within-budget→wrap-up→abort walk asserting the soft note fires exactly
> ONCE and that `wrapUpRequestedAtTurn` (the THRESHOLD) differs from `exceededAtTurn` (the OBSERVED
> turn); a repeated deferral keeping the FIRST deferral turn; an unarmed tracker being wholly inert;
> the timed-out guard; and the wire shape omitting unset turn fields.
> `exec::tests::the_turn_budget_notice_reaches_the_child_through_the_spilled_system_prompt_file`
> reads back the SPILLED prompt file — **not the argv, which `SUBA-030` moved the persona off, so an
> argv assertion would have passed vacuously** — and asserts absence before presence.
> `extension::tests`: the schema shape incl. `graceTurns.minimum == 0` and upstream's verbatim
> description, plus the refusal path asserting `turnBudget.hard is not supported.`
> `discovery::management::tests::serialize_agent_round_trips_the_turn_budget_launch_default` pins the
> `KNOWN_FIELDS`/serializer pair against silent deletion. **cyrup-it (2, real OS subprocesses):**
> `a_turn_budget_wraps_up_at_max_turns_and_aborts_the_child_after_the_grace_turn` scripts a child that
> emits four NON-terminal turns then sleeps 30 s, and asserts the run ends on turn 3 with upstream's
> exact error string, `exceededAtTurn: 3` / `wrapUpRequestedAtTurn: 2`, the partial output preserved
> under upstream's heading, and that the post-sleep turn was NEVER observed;
> `a_child_that_finishes_inside_its_turn_budget_is_untouched` is the adversarial twin — the SAME
> budget must be completely inert for a child that stops on its own, which an always-abort bug fails.
>
> **ONE VERIFY NOT PERFORMED, stated rather than smoothed.** The abort is proven against a real OS
> subprocess, but **the soft-note text was never observed rendering in a live TUI.** Per this
> directory's standing rule that a UI surface is not done until it has been RUN, the
> `progress.recent_output` wrap-up note is **mechanism-verified, not eye-verified.**
>
> **Everything below is the filing text, retained for provenance and wrong on the four points above.**

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `turn_budget` appears in exactly three files, all as consumers of a value with no source: `tui/intercom.rs:348-352` and `exec/mod.rs:2354-2360` (both with in-tree comments saying the flag has no producer) and `exec/fallback.rs`. No `turnBudget` key among the 45 properties at `extension.rs:6543-6690`.
**upstream** — `pi-subagents/src/runs/shared/turn-budget.ts` present at v0.43.0 and v0.47.1; `turnBudget: Type.Optional(TurnBudgetOverride)` is a top-level tool param (`extension/schemas.ts:353` @v0.43.0), and `appendTurnBudgetSystemPrompt` composes the budget notice into the child's system prompt.
**Impact** — No per-run turn cap, and the child is never told how many turns it has, so it cannot self-pace. Same unbounded-loop exposure as the tool budget, minus the enforcement half that SUBA-007 already landed. Duplicate of PARITY-GAPS PB-10 — fix once, close both.
**Fix** — Port as `exec/turn_budget.rs` mirroring `exec/tool_budget.rs`'s env-handoff shape, count assistant turns in the drive loop, wire the three existing consumers to the real value, and compose `appendTurnBudgetSystemPrompt` into the persona channel at `exec/mod.rs:1597-1608`. Add the schema key at the same time — do not repeat SUBA-047.
**Verify** — `turnBudget:{hard:2}` must terminate the run after two assistant turns with pi's budget-exhausted result shape, and the child's system prompt must contain the budget notice.

## SUBA-016 — `scheduledRuns` unported (+ nine `schedule.*` verbs, not four) — **BLOCKED 2026-08-15 on `workflowScript`**

> **BLOCKED 2026-08-15 (sweep 9) — see the table row for the full evidence. Everything below is the
> filing text and its Verify line is WRONG.** The nine-verb count is confirmed. The blocker is that
> a schedule's only legal target is `workflowScript` at **both** baselines
> (`ScheduleTarget = { workflowScript: string }`, `scheduled-runs.ts:38` @v0.43.0), and
> `workflowScript` is a 916-line `node:vm` JS sandbox (`workflows/scripted-workflow.ts:8,388,392`)
> that this crate documents as unported at `extension.rs:5990-6020`. The Fix line below —
> *"then the nine dispatch arms"* — is therefore the cheap half; the target runtime is the item.
> The Verify line below is **not executable against upstream**: `schedule.create` refuses
> `agent`/`task` outright, and there is no `cron` parameter anywhere in upstream — the triggers are
> `at` and `every`.

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `scheduled_runs` crate-wide; the 27-verb enum at `extension.rs:6557` has nothing beginning `schedule.`.
**upstream** — `pi-subagents/src/runs/background/scheduled-runs.ts` present at v0.43.0 and v0.47.1; **nine** verbs in `shared/types.ts:1968` — `schedule.create`, `.list`, `.show`, `.history`, `.pause`, `.resume`, `.run`, `.run-due`, `.delete`. The item's prior count of four was stale.
**Impact** — No recurring or deferred subagent runs. A caller attempting one gets the unknown-action error with its schedule parameters silently discarded first, so the error does not explain the failure. Duplicate of PARITY-GAPS PB-11.
**Fix** — Port the job store as `background/scheduled.rs` (persisted jobs under the subagents home plus a tick loop in the extension's background task), then the nine dispatch arms and the enum entries at `extension.rs:6557`.
**Verify** — `{action:"schedule.create", agent, task, cron}` then `{action:"schedule.list"}` must round-trip, and `schedule.run-due` must fire the job on its interval.

## SUBA-021 — `capability-ceiling` / `usage-budget` / `spawn-budget` unported — all three are in-baseline

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** medium *(raised from low)* · **Effort** L · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: `launch-contract.ts` is struck not because upstream deleted it but because it is **absent at both tags at every path** — it historically lived at `src/shared/launch-contract.ts` — so it was never in either baseline and the item was wrong to name it.)*
**cyrup** — `rg 'capability_ceiling|usage_budget' crates/cyrup-ext-subagents/src` = 0. The only spawn-budget analogue is the per-session counter at `extension.rs:760` with no grant path, no snapshot, and no ceiling concept.
**upstream** — `capability-ceiling.ts`, `usage-budget.ts` and `spawn-budget.ts` all pass `git cat-file -e` at **both** v0.43.0 and v0.47.1, so the prior "post-baseline, out of scope" framing is dead — this is in-baseline unported work.
**Impact** — No pre-spawn capability ceiling and no usage budget: a child can be granted a capability set wider than its parent's, and there is no token/cost bound on a run beyond the model's own limits. `CAPABILITY_CEILING_V1` is separately tracked as PARITY-GAPS VL-S1 on the env-var surface.
**Fix** — Port `capability-ceiling.ts` and `usage-budget.ts` as `exec/capability_ceiling.rs` and `exec/usage_budget.rs`, consulted in `build_attempt_spawn_plan` beside the existing tool-budget encoder. The spawn-budget half is **SUBA-046** and should land first, since it is the one with a live user-facing dead end.
**Verify** — A child requesting a capability outside its parent's ceiling must be refused at preflight with pi's message; a run exceeding its usage budget must terminate with pi's budget result shape.

## SUBA-025 — `toolDescriptionMode`, the file override, and the mandatory safety-guidance appender unported

**Kind** not-ported · **Severity** medium *(raised from low)* · **Effort** S · **Confidence** confirmed
**cyrup** — `rg 'description_mode|toolDescriptionMode' crates/cyrup-ext-subagents/src` = 0. Descriptions are code constants selected by registration mode. `rg 'SAFETY-CRITICAL' extension.rs` hits only unrelated R-SA-055 depth-guard doc comments.
**upstream** — `pi-subagents/src/extension/tool-description.ts` present at v0.43.0 with **three** surfaces cyrup has none of: `resolveToolDescriptionMode` (`:68`); a user/project `subagent-tool-description.md` override capped at `CUSTOM_TOOL_DESCRIPTION_MAX_BYTES = 50 * 1024` (`:6-7`, `:80-81`); and `withMandatorySafetyGuidance` (`:144`) which appends `SUBAGENT_SAFETY_GUIDANCE` (`:9`). Refuter's precision note, stated inline: `withMandatorySafetyGuidance` is applied on the **`custom` branch only** (`:160`) — for `full`/`compact` the guidance is baked into the constants — so "every description including custom ones" overstates it; the load-bearing case is that a deployment *can* replace the description and cannot drop the safety guidance.
**Impact** — Deployments cannot trim the (long) subagent tool description to save context, cannot steer the orchestrator with a project-specific description, and — the reason for the severity raise — there is no mechanism guaranteeing the safety guidance survives a custom description, because there is no custom-description path at all. Severity medium rather than low because this is the surface `SUBA-046`'s advertise-vs-refuse defect and `SUBA-061`'s `legacyChainControls` both attach to.
**Fix** — Add `toolDescriptionMode` to `SubagentExtensionConfig`, resolve the description at registration rather than selecting a constant, port the 50 KiB-capped file override with the same search order, and port `withMandatorySafetyGuidance` applied to the custom branch.
**Verify** — With `compact` configured, the registered description must be the short form for both `subagent` and `wait`; with a `subagent-tool-description.md` present, the registered description must be its contents **plus** the safety guidance; a 60 KiB override must be rejected with pi's error.

## SUBA-028 — Acceptance verification cannot be aborted

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/acceptance.rs:1337-1352`: `evaluate_acceptance(contract, gate, final_output, completion_guard, verify_cwd, memo, file_output)` takes no cancellation argument, and the live `model::EvaluateAcceptanceInput` (`:6964-6989`) has no `signal` field either. Grep for `signal`/`cancel`/`abort` in the file returns only `has_generic_acceptance_report_signal` (`:5355`), an unrelated JSON-shape helper.
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts:1073` @v0.43.0 takes `signal?: AbortSignal`; `:1181-1182` is `if (options.signal?.aborted) abortVerification(); else addEventListener(...)`; `:1290` passes `signal: input.signal` down and `:1295` breaks the command loop on `input.signal?.aborted`.
**Impact** — Cancelling a subagent run (Ctrl-C, orchestrator cancel, parent timeout) does not stop acceptance verification; the caller can wait a full per-command timeout after asking to stop. SUBA-027's fix means the timed-out child is now killed, so the leak is gone — the latency is not.
**Fix** — Thread `CancelToken` from the `exec/mod.rs` caller into `evaluate_acceptance` → `run_verify_commands_memoized` → `model::run_memoized_verify_command`, check it before each command in the loop, and `select!` it against the per-command wait alongside the existing timeout. Add the field to `model::EvaluateAcceptanceInput` so both entry shapes carry it.
**Verify** — Start a run whose verify command sleeps 60 s, cancel after 1 s; `evaluate_acceptance` must return within ~1 s and the child must be gone.

## SUBA-030 — Persona passed inline on argv; task spill written with the default umask under a doc asserting 0600 — **CLOSED 2026-08-14 (already-done)**

> **CLOSED 2026-08-14 (sweep 8) as already-done — see the table row for the evidence. Everything
> below is the filing text.** Both halves are in at HEAD: `spawn/mod.rs:262-289` spills the composed
> persona to a `0600` `<stem>.md`, and `exec/mod.rs:1802` calls it, pushing the flag and the PATH as
> two argv elements with an in-source block at `:1783-1801` naming the `/proc/<pid>/cmdline`
> disclosure half and the `MAX_ARG_STRLEN`/E2BIG availability half by name.


**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (E2BIG half reasoned, not observed)
**cyrup** — Both halves confirmed at HEAD. **Persona:** `crates/cyrup-ext-subagents/src/exec/mod.rs:1597-1608` pushes `format!("{flag}={persona_body}")` as one argv element with no size check; the in-tree comment explains that the `=`-form is required by clap and says nothing about length. **Task spill:** `crates/cyrup-ext-subagents/src/spawn/mod.rs:248-260` — `resolve_task_arg` is a plain `std::fs::write(&path, task)` with the default umask, while `spawn/mod.rs:428`'s own doc calls these *"the 0600 task/system-prompt temp files"*. The code's documentation asserts a mode the code never sets; that internal contradiction is the strongest evidence in this item.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:571-593` @v0.43.0 writes **both** the system prompt and the task overflow with `{ mode: 0o600 }` into an `mkdtempSync` dir and passes paths.
**Impact** — (a) Any local user can read a subagent's full persona from `/proc/<pid>/cmdline`, and the spilled task file is world-readable; personas routinely carry project context and occasionally credential-adjacent instructions. (b) A persona above Linux `MAX_ARG_STRLEN` (131072) makes `execve` fail with E2BIG and the spawn dies with an opaque OS error rather than a diagnosable message.
**Fix** — Set mode `0600` unconditionally on the task spill at `spawn/mod.rs:248-260` (two lines, and it makes the existing doc true). For the persona, add the same threshold guard the task path already has — above a limit, write to a 0600 file in a per-run `mkdtemp` dir and pass a path — which first requires teaching `crates/cyrup/src/cli.rs` to accept a path form for `--system-prompt`.
**Verify** — `stat -c %a` on the task spill must be `600`. Spawn with a 200 KB persona: the run must succeed and `/proc/<child>/cmdline` must not contain the body.

## SUBA-031 — `wait` scopes runs by cwd, not by session, and says "in this session"

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/wait.rs:42-48` still states the delta in-tree ("pi filters runs by `state.currentSessionId`; cyrup's `RunStatus` carries no session id"), and the empty-set message at `:325-330` reads *"No active async runs in this session. Nothing to wait for."* while the actual scope is the cwd. The in-tree comment and the user-facing string disagree with each other.
**upstream** — `pi-subagents/src/runs/background/wait.ts` passes `sessionId` from `state.currentSessionId`; `subagent-wait.ts` @v0.43.0 keeps the same filter.
**Impact** — Two cyrup sessions in the same repo see each other's background runs. `wait {all:true}` in session A blocks on session B's runs and reports their results; a stalled run in an unrelated session hangs an unrelated wait — and the message tells the user the opposite of what happened.
**Fix** — Record a session id on `RunStatus` at spawn and filter `list_active_runs` by it (or add a session component to the artifact roots). Fix the empty-set string in the same change so it matches whichever scope survives.
**Verify** — Two sessions, same cwd, one background run each: `wait {all:true}` in each must return only its own run.

## SUBA-032 — Notice-debounce test asserts a wall-clock outcome with ~15 ms margin

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/tui/notices.rs:925-943`: three real-clock `tokio::time::sleep(Duration::from_millis(20/45/40))` calls around a 60 ms debounce. The load-bearing assertion lands 15 ms inside the deadline, and overshoot on the *second* sleep is fatal. `start_paused` / `time::pause` / `time::advance` are zero-hit across the whole crate.
**upstream** — No counterpart; the debounce is cyrup-original over `pi-subagents/src/tui/render.ts`-derived surfaces. The in-repo precedent is commit `1806375`, which removed a structurally identical assertion from `cyrup-ext/src/caps/proc.rs`.
**Impact** — On a loaded CI box the second sleep overshoots and the test flakes; flaky tests get `#[ignore]`d and the debounce loses coverage entirely.
**Fix** — `#[tokio::test(start_paused = true)]` plus `tokio::time::advance` for each interval, making the assertion exact rather than marginal.
**Verify** — Deterministic under `--test-threads=1` on a machine loaded to 100% CPU, with runtime near zero once the clock is paused.

## SUBA-044 — cyrup's bundled `reviewer` agent still grants `bash`, `edit` and `write`; upstream made the reviewer lane read-only

**Kind** upstream-drift · **Severity** medium *(corrected down from the auditor's high — see caveat)* · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/resources/agents/reviewer.md:4` — `tools: read, grep, find, ls, bash, edit, write, intercom`, with `defaultReads: plan.md, progress.md` at `:9` and a prose-only restraint at `:56` (*"Use `bash` only for read-only inspection…"*) over a grant that permits arbitrary shell and arbitrary writes. Registered as a builtin through `registration/resources.rs` / `discovery/mod.rs`.
**upstream** — `pi-subagents/agents/reviewer.md:4` @v0.47.1 — `tools: read, grep, find, ls, intercom`; `defaultReads` removed; the rule is now *"Do not use shell commands or write files. Report any test or Git command that a supervisor must run."* Changed by `0b1976b` ("fix: make reviewer lanes read-only by default", #1008), released v0.47.1. v0.43.0 still carried the write grant, so this is drift, not a stale port.
**Impact** — A user delegating to the shipped `reviewer` gets a lane that can edit the working tree and run shell during what pi users expect to be an inspection-only pass. **Refuter's caveat, stated inline:** severity is medium rather than high because cyrup's own shipped prose contradicts the "user believes it is read-only" premise — `reviewer.md:58` says *"Prefer small corrective edits over broad rewrites"*, i.e. this agent is documented to edit. The defect is a documented behavioural divergence from upstream, not a silent capability escalation.
**Fix** — Set `tools: read, grep, find, ls, intercom` in `resources/agents/reviewer.md`, drop `defaultReads`, and take upstream's two prose lines verbatim. **Second correction:** the "strict tool allowlist / does not inherit ambient extension tools" paragraph the audit attributed to a v0.47.1 addition in `agents/delegate.md:12-13` and `agents/worker.md:21-22` is present at **v0.43.0 too** — cyrup's copies lack an *in-baseline* paragraph, so port it as a not-ported gap in the same pass.
**Verify** — Diff every file under `crates/cyrup-ext-subagents/resources/agents/` against `git -C pi-subagents show v0.47.1:agents/<name>.md`; only the `researcher.md` divergence (SUBA-062) may remain, and only with a recorded `[CYRUP-DELTA]`.

## SUBA-045 — The child tool-availability diagnostic is entirely unported: a child that silently lacks a declared tool reports nothing

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/mod.rs:1849-1855` writes only `CYRUP_SUBAGENT_REQUIRED_TOOLS` into the child env; there is no diagnostic-path var and `rg 'TOOL_DIAGNOSTIC|tool_diagnostic' crates/cyrup-ext-subagents/src` = 0. The single consumer of the required-tools list is the intercom fallback gate (`native_supervisor.rs:1639` `read_required_child_tools`, used at `:1742`), so **nothing compares required against available**.
**upstream** — `pi-subagents/src/runs/shared/tool-availability.ts` @v0.43.0 — `:6` `CHILD_TOOL_DIAGNOSTIC_PATH_ENV`, `:18-44` `writeChildToolDiagnostic` (child side: computes `missing` against a `PI_CORE_CHILD_TOOLS` floor, writes 0600 JSON, deletes the file when nothing is missing), `:47-61` `readChildToolDiagnostic`, `:68-70` the `missingMcpDirectTools` message. Written into the env at `pi-args.ts:610-616` beside the required-tools var; the child writes it from `subagent-prompt-runtime.ts:99`; the **parent** reads it back and folds it into the run's terminal error at `foreground/execution.ts:1072-1079` (`closeError = result.error ?? toolDiagnosticError ?? assistantError`) and `background/subagent-runner.ts:1442`.
**Impact** — An agent whose `tools:` names an MCP tool the child's host never registered (stopped MCP server, renamed tool, extension that failed to load) runs to completion producing a model apology instead of a diagnosis. Upstream turns exactly that case into the run's error text, naming the missing tools and distinguishing *"a host/pi-mcp-adapter registration problem, not a tool-call failure"*.
**Fix** — Add `exec/tool_availability.rs` porting the three functions; write `CYRUP_SUBAGENT_TOOL_DIAGNOSTIC_PATH` beside `CYRUP_SUBAGENT_REQUIRED_TOOLS` at `exec/mod.rs:1849`, pointing into the run's scratch dir; have `prompt_runtime.rs`'s init write the diagnostic from the live registry; read it back in `run_attempt`'s error composition so it takes precedence over the assistant error, matching `execution.ts:1079`.
**Verify** — Declare `tools: read, mcp__nonexistent__x` and run; the run must fail with text naming `mcp__nonexistent__x` as missing from the child registry. With all tools present, no diagnostic file may survive the run.

## SUBA-046 — `grant-spawn-budget` is unported *and advertised*, so an exhausted per-session spawn cap is terminal for the whole session

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — All six `grant-spawn-budget` hits under `crates/cyrup-ext-subagents/src` are prose: `extension.rs:5268-5273` states outright that *"cyrup does not implement a `grant-spawn-budget` action at all"*, while `:5290` and `:12915` reproduce pi's child-safe tool description **advertising the verb to the model**. The action enum at `:6557` does not contain it, so a model that reads the description and calls it lands on the unknown-action arm. The counter exists (`reserve_subagent_spawns`, `:760`; reset only at SessionStart, `:9422`) with no grant path and no snapshot in tool-result details (`rg 'spawnBudget' extension.rs` = 0).
**upstream** — `pi-subagents/src/runs/shared/spawn-budget.ts` @v0.43.0 — `:30` `getSpawnBudgetSnapshot`, `:50/:55` the two formatters, `:59` `preflightSpawnBudget`, `:73` `reserveSpawnBudget`, `:85` `preflightSpawnBudgetGrant`, `:107` `grantSpawnBudget`. Dispatch at `runs/foreground/subagent-executor.ts:4457-4505`: root-interactive only, requires a session id, refuses while children are queued/running, previews, consults `resolveAuthorityDecision({action:"spawnBudgetGrant"})`, then `ctx.ui.confirm`. Param `additional` at `extension/schemas.ts:283`. `grant-spawn-budget` is in `SUBAGENT_ACTIONS` at `shared/types.ts:1885` @v0.43.0 and in `MUTATING_MANAGEMENT_ACTIONS` at `subagent-executor.ts:167` @v0.47.1. `spawnBudget` is returned in `details` on every refusal.
**Impact** — Once `maxSubagentSpawnsPerSession` is reached, a cyrup session can do no further delegation until it is restarted; there is no in-session escape hatch and no visibility into remaining budget, because the snapshot is never reported. Upstream's design is that the cap is a speed bump with a confirmed grant behind it. cyrup additionally **advertises the verb while refusing it** — the SUBA-041 defect class recurring on the description surface.
**Fix** — Port `spawn-budget.ts` as `exec/spawn_budget.rs` around the existing counter (snapshot type, `preflight_spawn_budget_grant`, `grant_spawn_budget`); add `grant-spawn-budget` to the enum at `extension.rs:6557` plus the `additional` param; gate on root-interactive (`allow_mutating_management` + a host-services UI) and route the confirm through `HostServices` (`crates/cyrup-ext/src/host/services.rs`). Attach the snapshot to the budget-refusal `ToolError` details so the cap is observable even without the grant. Depends on **SUBA-064** for the authority gate; land the counter/snapshot half first and wire the authority consult when that lands.
**Verify** — Set the cap to 1, spawn once, then `{action:"grant-spawn-budget", additional:2}` from the root session must confirm and permit two more; the same call from a fanout child must be refused with *"available only from the root interactive parent session."*

## SUBA-047 — `toolBudget` is honoured but never advertised, so the model cannot set a per-run tool cap

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Residual of SUBA-007, which is otherwise closed.)*
**cyrup** — `crates/cyrup-ext-subagents/src/exec/tool_budget.rs` (388 lines) and the env hand-off at `exec/mod.rs:1837-1846` show the enforcement half is complete, and the frontmatter key is read at `discovery/frontmatter.rs:850` — but `toolBudget` is **not** among the 45 keys emitted by `subagent_tool_parameters()` (`extension.rs:6543-6690`), and `rg 'tool_budget' extension.rs` returns exactly one hit (`tool_budget: None` at `:18993`, a test fixture). The only way to set a budget is to edit the agent file on disk.
**upstream** — `pi-subagents/src/extension/schemas.ts:279` @v0.34.0 and `:354` @v0.43.0 — `toolBudget: Type.Optional(ToolBudgetOverride)` is a top-level param (shape at `:116-120`: `soft?`, `hard`, `block?`), and also a per-item override on `ParallelTaskSchema:148` and the dynamic template at `:178`. In-baseline since before the ported tag.
**Impact** — A caller cannot bound a single delegation's tool spend without editing the agent on disk, and a per-call budget passed by an orchestrator is silently discarded. This is the mirror of the closed SUBA-N06: honoured but unadvertised, so the capability exists and is unreachable. It also blocks the per-task `toolBudget` override on `tasks[]`, which cyrup's item schema likewise omits.
**Fix** — Add `toolBudget` to `subagent_tool_parameters()` and to the `tasks[]`/`chain[]` item schemas beside `acceptance`, deserialize onto `SubagentToolParams`, and lower it into `RunOptions`/`SingleStepSpec` so it reaches the existing encoder at `exec/mod.rs:1837`. Precedence must be caller > frontmatter > extension config, matching pi.
**Verify** — `{agent:"x", task:"y", toolBudget:{hard:3}}` against an agent that would make ten calls must stop after three with the budget message; the same override inside `tasks:[{…}]` must apply per task.

## SUBA-048 — The `artifactDir` config key is unported — `resolve_artifacts_dir` has no preference parameter, so "session" and "temp" are unreachable

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/artifacts.rs:170-186` — `resolve_artifacts_dir(session_file, project_cwd, temp_cwd)` takes three path arguments and no preference: a `Some(project_cwd)` always wins, else the session sibling, else temp. Its two production callers (`extension.rs:9403`, `tui/fleet.rs:1032`) pass paths only, and `tui/fleet.rs:1030` records the delta in-tree ("pi passes `state.artifactDirPreference ?? \"project\"`"). `SubagentExtensionConfig` (`registration/mod.rs`) has 18 fields and `artifact_dir` is not one.
**upstream** — `pi-subagents/src/shared/artifacts.ts:160-183` @v0.43.0 — `getArtifactsDir(sessionFile, projectCwd?, dirPreference: ArtifactDirPreference = "project")` with distinct `session`/`temp`/`project` arms and a `throw` on an unsupported value; `getChainRunsDir` (`:145-158`) takes the same preference. The key is on `ExtensionConfig` at `shared/types.ts:1777` and **validated at `extension/config.ts:9,22-24`, which throws** on anything outside `ARTIFACT_DIR_PREFERENCES`.
**Impact** — Setting `"artifactDir": "temp"` or `"session"` does nothing — every run writes `<cwd>/.cyrup-subagents/…` into the user's repository. Users who chose `temp` specifically to keep generated transcripts, inputs and outputs out of a git working tree get them written there anyway. Upstream **errors** on a bad value where cyrup silently ignores a good one. Distinct from PARITY-GAPS PB-13, which is about the chain-runs *default* root going to temp instead of the project.
**Fix** — Add `artifact_dir: Option<ArtifactDirPreference>` to `SubagentExtensionConfig` with pi's three-variant enum and its validation error, add the parameter to `artifacts::resolve_artifacts_dir` and `artifacts::chain_runs_dir`, and thread the config snapshot through both callers. Landing it alongside PB-13 makes `project` the correct default for both.
**Verify** — With `"artifactDir": "temp"`, a foreground run must leave `<cwd>/.cyrup-subagents` untouched; with `"session"` and an active session file, artifacts must land in that file's sibling `subagent-artifacts` dir; an invalid value must be rejected at config load.

## SUBA-049 — Steer acknowledgment, delivery `mode` and `steeringRecovery` are unported, so a queued steer is fire-and-forget — **PARTIALLY CLOSED 2026-08-15**

> **PARTIALLY CLOSED 2026-08-15 (sweep 11) — see the table row for the evidence.** The ack path,
> the delivery `mode` and the capability record are in end to end; a steer is no longer
> fire-and-forget. **`steeringRecovery` is NOT ported and is not advertised**: it is `steering.ts`
> plus the pause-and-revive half of `async-steering-action.ts` (~450 upstream lines and a new
> `status.json` sub-record), i.e. **L**, not the **M** this item was filed at. The sequencing note
> below still stands and was NOT discharged: cyrup's inbox targets the v0.43.0 shape and the
> v0.44–v0.47 steering hardening (safety poll, settle fallback, `awaitingSettlement`) has still not
> been diffed line by line — schedule it WITH `steeringRecovery`, since both live in the same file.

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
*(Residual of SUBA-013, which is otherwise closed.)*
**cyrup** — `rg 'STEER_ACK|steer_ack|STEER_CAPABILITY|steer_capability' crates/cyrup-ext-subagents/src` = 0. cyrup writes only `CYRUP_SUBAGENT_STEER_INBOX` (`exec/mod.rs:1857-1868`); the child-side inbox at `prompt_runtime.rs:157-290` consumes and deletes requests but writes no ack; the dispatch arm at `extension.rs:7825-7837` calls `control_steer` and returns its text with no ack poll. Neither `mode` nor `steeringRecovery` is among the 45 advertised params.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:101-102` @v0.43.0 defines `SUBAGENT_STEER_CAPABILITY_ENV` and `SUBAGENT_STEER_ACK_DIR_ENV`, written at `:766-768`; the child acknowledges each request with `delivered`/`queued`/`failed` plus a message via `writeSteerAckAt(steerAckPathFromDir(ackDir, request.id), …)` (`subagent-prompt-runtime.ts:334-335` reads both vars; the `acknowledge` closure writes). Params `mode` (`steer`/`follow_up`/`auto`) and `steeringRecovery` are top-level at `extension/schemas.ts:283-284`.
**Impact** — `{action:"steer", id, message}` returns success as soon as the request file is written. The caller is never told whether the child took it, queued it behind a full follow-up queue (upstream caps at `MAX_STEER_QUEUE_SIZE` and answers `failed`), or could not deliver at all. A steer aimed at a child that is mid-tool and never reaches a turn boundary looks identical to one that landed. Without `mode` the caller cannot ask for the non-interrupting follow-up delivery upstream makes explicit.
**Fix** — Add an ack directory per run beside the existing inbox, export `CYRUP_SUBAGENT_STEER_ACK_DIR` and the capability path from `build_attempt_spawn_plan`, have `prompt_runtime::SteeringInbox` write an ack record per consumed/failed request, and have the `steer` dispatch arm poll for the ack before answering. Add `mode` and `steeringRecovery` to the schema with upstream's descriptions verbatim. **Sequencing note:** cyrup's inbox targets the v0.43.0 shape and upstream hardened steering across v0.44–v0.47 (safety poll, settle fallback, `awaitingSettlement`); that drift is an unfiled blind spot recorded in `## Coverage` and should be diffed in the same pass.
**Verify** — Steer a child whose follow-up queue is full; the tool must answer `failed` with upstream's *"Follow-up queue is full (N messages)."* text rather than success. Steer with `mode:"follow_up"` and assert the child receives it at the next turn boundary, not mid-turn.

## SUBA-050 — `subagents.modelScope.strict` is unported, so inherited and fallback out-of-scope models can never be hard-rejected

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/model_scope.rs:43-50` — `ModelScopeConfig` has exactly two fields, `enforce` and `allow`; the severity decision at `:175-185` is an unconditional `match source { Explicit => Error, Inherited => Warn }` with no config input. `rg 'strict' exec/model_scope.rs` returns nothing.
**upstream** — `pi-subagents/src/runs/shared/model-scope.ts` @v0.47.1 — `:20` adds `strict?: boolean` ("Reject inherited and fallback models outside the allowlist instead of warning"), `:73` becomes `source === "explicit" || scope.strict === true ? "error" : "warn"`, and `:108-113` validates the key with a typed error. `git show v0.43.0:…/model-scope.ts | grep strict` is empty, confirming drift. Landed in `94b0cb1` ("feat: enforce strict subagent model scope", closes #995), released v0.47.0.
**Impact** — An operator who configures a model allowlist to keep subagents off expensive or non-compliant models cannot make it binding: an agent whose frontmatter names an out-of-scope model, or whose fallback ladder walks onto one, only warns and then runs on it. The policy is advisory for exactly the sources that are hardest to audit.
**Fix** — Add `strict: Option<bool>` to `ModelScopeConfig` with the same serde shape and validation error text, and change the severity computation at `:175-185` to `Explicit | (_ if strict) => Error`. The `Err(violation)` propagation path already exists from SUBA-003, so nothing downstream changes.
**Verify** — With `"modelScope": {"enforce": true, "strict": true, "allow": ["anthropic/*"]}`, an agent whose frontmatter names `openai/gpt-5` must fail the run with the out-of-scope error rather than warning and running.

## SUBA-051 — Async child runs have no default wall-clock timeout; upstream bounds every async CHILD at 30 minutes

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/runner_main.rs:310-330` — `timeout_ms: Option<u64>` and `deadline_at_ms: Option<u64>`, both serde-defaulted to `None` unless the caller supplied `timeoutMs`; `:1255` states in-tree that the **default** remains "no wall-clock timeout". The deadline arm at `:1653-1661` only exists when `deadline_at` is `Some`.
**upstream** — `pi-subagents/src/runs/background/async-execution.ts:131` @v0.47.1 — `export const DEFAULT_ASYNC_TIMEOUT_MS = 30 * 60 * 1000;` and `:782` `timeoutMs: a.defaultTimeoutMs ?? DEFAULT_ASYNC_TIMEOUT_MS` inside the child-step builder, with the composite **parent** deliberately left unbounded. Landed in `635c1bd` ("fix: add default async child timeouts", fixes #978), released v0.47.0; the split is echoed in the tool description.
**Impact** — A background child that wedges — a hung `cargo test`, a non-terminating model, a retry loop — burns tokens and CPU until a human notices and issues `interrupt`. The stopping machinery now exists in cyrup (SUBA-S03 closed); only the default is missing, so every async fan-out is unbounded by default exactly as it was before that fix.
**Fix** — Add `DEFAULT_ASYNC_CHILD_TIMEOUT_MS: u64 = 30 * 60 * 1000` and apply it when building each async **child** step (the `RunnerConfig`/step construction reached from `extension.rs:2295`), leaving composite parents (`chain`/`tasks`/graph roots) unbounded to match upstream's split. Do not apply it to foreground runs, which already have their own default.
**Verify** — `{agent, task, async:true}` with no `timeoutMs` against a non-terminating child must flip to `failed` with the timeout message after 30 minutes, while an async chain **parent** with running children must not.

## SUBA-052 — YAML literal block scalars (`|`, `|-`) in agent frontmatter parse to the literal string `"|"`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Residual of SUBA-019, which is otherwise closed.)*
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:509-519` — the only block-scalar branch is `let is_folded = !is_quoted && (raw_value == ">" || raw_value == ">-");` followed by `if value.is_empty() || is_folded`. For `description: |`, `strip_matching_quotes` yields `(false, "|")`, which is neither empty nor folded, so `:518` executes `fields.push((key, "|"))`; the indented body lines then fail the `^([\w-]+):` match at `:503` and are silently discarded. The blank-line continuation predicate at `:503` (`current_folded && line.trim().is_empty()`) has no literal arm either.
**upstream** — `pi-subagents/src/agents/frontmatter.ts` @v0.47.1 — `currentLiteral` at `:86`, reset at `:113`, `const isLiteral = !isQuoted && (rawValue === "|" || rawValue === "|-")` at `:124`, folded into the defer condition at `:126` and into the blank-line continuation test at `:91`. `git show v0.43.0:…/frontmatter.ts | grep -n 'isLiteral\|currentLiteral'` is empty. Landed in `a4fc59a` ("fix: parse block scalar skill descriptions", #952), released v0.46.0, which also routed `parseSkillDescription` (`agents/skills.ts:398`) through the shared parser.
**Impact** — `|` is the most common YAML idiom for a multi-line description or prompt, and cyrup turns it into the one-character string `"|"` with the whole body dropped and no warning. An agent authored that way lists with a description of `|`, matches nothing in proactive-skill selection, and — for any multi-line key that feeds behaviour — runs with an empty value. Silent wrong value, not a hard error.
**Fix** — Mirror the upstream change in `parse_frontmatter_block`: add a `current_literal` flag beside `current_folded`, set it for `|`/`|-`, include it in the defer condition at `:513` and the blank-line continuation test at `:503`, and store the dedented block verbatim (no folding) on flush. Route skill-description parsing through the same parser as upstream did.
**Verify** — An agent whose `description: |` spans three indented lines must yield those lines joined by newlines, and `description: >` must still fold — one table test covering `|`, `|-`, `>`, `>-` and a plain scalar.

## SUBA-053 — `~` is never expanded in chain read/write paths, so `reads: ["~/notes.md"]` resolves to `<chain_dir>/~/notes.md`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:693-700` — `resolve_chain_path(file, chain_dir)` is `if file.is_absolute() { … } else { chain_dir.join(file) }` with no home expansion; `rg 'expand_home' crates/cyrup-ext-subagents/src` = 0. It feeds both the `[Read from: …]` prefix (`:730-736`) and the `[Write to: …]` line (`:737-739`).
**upstream** — `pi-subagents/src/shared/settings.ts` @v0.47.1 — `:341-345` `expandHomePath` (`~` → homedir, `~/x` → `join(homedir, x)`, `~user/` deliberately untouched), called at the head of `resolveChainPath` (`:351-354`) before the `isAbsolute` test. At v0.43.0 `resolveChainPath` (`:335`) had no expansion. Landed in `87420e5` ("fix(reads): expand home paths and wire reads into single runs"), released v0.45.0; the commit message names the exact symptom.
**Impact** — A chain step declaring `reads: ["~/.config/project.toml"]` gets an instruction pointing at a path that does not exist, so the child either reports the file missing or fabricates content; a `~`-prefixed `output` writes into a literal `~` directory under the chain dir. The failure is silent at the orchestrator — the instruction line looks well-formed.
**Fix** — Add an `expand_home` helper in `spawn/chain_graph.rs` (or `artifacts.rs` for reuse) matching upstream's three cases exactly, called at the head of `resolve_chain_path` before the `is_absolute` test. `~user/` must **not** be expanded, matching upstream.
**Verify** — Table-test `resolve_chain_path` over `~`, `~/`, `~/file`, `/abs/path`, `rel/path` and `~user/file` against upstream's eight cases in `test/unit/reads-resolution.test.ts`.

## SUBA-054 — `defaultReads` is parsed and rendered but never reaches a single run — no `[Read from: …]` instruction outside chains

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:782` parses `defaultReads` and `discovery/management.rs:779` renders it in agent listings, but `rg 'Read from' crates/cyrup-ext-subagents/src` matches only `spawn/chain_graph.rs:705,734` (the chain-instruction builder), `registration/slash_commands.rs:1255` (the `/run reads=` caller override) and `exec/task_intent.rs` (a stripper). `exec/mod.rs` never composes a read instruction — `default_reads` appears there once, at `:3735`, inside a test fixture — and the async `SingleStepSpec` at `extension.rs:2310` pins `reads: None`. So `defaultReads` is inert for every non-chain invocation.
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:3867-3869` @v0.47.1 — *"Reads: caller override > agent defaultReads > none"* with `const reads = readsOverride !== undefined ? readsOverride : agentConfig.defaultReads ?? false;`, matched at `runs/background/async-execution.ts:1300-1302`; the instruction is built through `resolveExistingReadInstructionPaths` (`shared/settings.ts:356-362`). `git show v0.43.0:…/subagent-executor.ts | grep defaultReads` is empty. Landed in `87420e5`, released v0.45.0; the commit states that single-run launches "silently dropped a top-level `reads` value".
**Impact** — cyrup's own bundled `reviewer` ships `defaultReads: plan.md, progress.md` (`resources/agents/reviewer.md:9`) and, invoked as an ordinary single subagent, is never told to read either file — the frontmatter key is documentation. Any user agent relying on `defaultReads` to pre-seed context behaves as if the key were absent, and the failure looks like the model ignoring instructions.
**Fix** — Build the `[Read from: …]` prefix in `exec/mod.rs`'s task assembly (beside `build_task_text`) from `caller reads > agent.default_reads > none`, reusing `spawn/chain_graph.rs`'s formatter, and add the `reads` param on the async path. Land **SUBA-053** first so `~` paths resolve, and **SUBA-058**'s existence filter in the same pass. Note the interaction with SUBA-044: upstream removed `defaultReads` from `reviewer.md`, so fixing this without fixing that changes the bundled reviewer's behaviour.
**Verify** — Run the bundled `reviewer` against a repo containing `plan.md`; the child's task text must open with `[Read from: <abs>/plan.md]`. An agent with `defaultReads` plus an explicit caller `reads` must use the caller's list only.

## SUBA-055 — The `guide` action and the packaged version-matched docs it serves are unported — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 11) — see the table row for the evidence.** `registration/guide.rs`
> plus a cyrup-authored `resources/docs/` set embedded with `include_str!`, the verb in pi's own
> enum position, and `topic` advertised exactly as pi declares it. The `children.list` half named in
> the Impact below is NOT ported — it lists retained children, part of the unported `workflowScript`
> shape — and returns to `SUBA-005`'s unowned-verb list.

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — the 27-verb enum has no `guide`; `rg 'children.list|"guide"' crates/cyrup-ext-subagents/src` = 0. The unknown-action arm at `:7610-7614` would answer `unknown subagent action 'guide'`. cyrup ships no `docs/` tree beside the crate's `resources/`.
**upstream** — `pi-subagents/src/extension/subagent-guide.ts` @v0.47.1 (new file; `git cat-file -e v0.43.0:…` fails) — `:5-16` `SUBAGENT_GUIDE_TOPICS` (overview, workflows, agents, missions, observability, tool-reference, configuration, models, watchdog, extension-api), `:26-38` `readSubagentGuide` resolving `README.md` for overview and `docs/<topic>.md` otherwise, with an explicit unknown-topic message. Dispatched at `runs/foreground/subagent-executor.ts:4979` and exempted from the strict-action throw at `:4797`. Landed in `0677ac0` ("feat: add subagents guide action"), released v0.46.0, and advertised in `extension/tool-description.ts:28`.
**Impact** — An orchestrator model that has drifted from the tool surface has no in-band way to re-read the current contract; upstream lets it call `{action:"guide", topic:"tool-reference"}` and get shipped, version-matched documentation. It also blocks the companion `children.list` retained-child listing that the same description paragraph advertises. The **slash-command** half is separately unported as **SUBA-066**.
**Fix** — Embed the crate's own documentation set (README plus per-topic markdown) with `include_str!` under `resources/docs/`, add `guide` to the action enum plus a `topic` param, and route it to a `registration::guide::read_subagent_guide` reproducing upstream's unknown-topic message verbatim. Embedding rather than reading from disk is the mechanism difference forced by shipping a single binary; record it as a `[CYRUP-DELTA]`.
**Verify** — `{action:"guide"}` must return the overview; `{action:"guide", topic:"bogus"}` must return upstream's exact *"Unknown subagents guide topic … Valid topics: … No files were changed."* string.

## SUBA-056 — Durable completion replay and output archives are unported, so an async result consumed once cannot be re-read

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — `rg -c 'completion_replay|output_archive|wait_completion' crates/cyrup-ext-subagents/src` = 0 across the whole crate. `background/watch.rs` invokes `CompletionSink` once per result file and nothing persists a replay record or an archive; `background/wait.rs` returns text only and carries no structured completion in tool-result details.
**upstream** — `pi-subagents/src/runs/background/completion-replay.ts` @v0.47.1 (new, 267 lines; absent at v0.43.0) — `completionReplayPath`/`completionArchivePath` (`:41,:46`), `writeCompletionArchive` (`:68`) preferring the child's output artifact, then its session file, then a 64 KiB `utf8Tail` of the result text, and `CompletionReplayRecord` with `expiresAt` (`:28-36`). Paired with `runs/background/wait-completions.ts` (new, 146 lines; `toWaitCompletion` at `:27` projecting the slim per-child shape into `details`) and wired through `result-watcher.ts` and `wait-subscriptions.ts`. Landed in `c2add95` and `a1e3fc8`, released v0.44.0/v0.45.0; hardened by `e55f083` and `0e06cb3`.
**Impact** — A background result that arrives while the orchestrator is mid-turn, or that is consumed by one `wait` and then needed again after a compaction, is unrecoverable — the notice fires once and the structured outcome never enters the tool result's `details`. The orchestrator has to re-run the child. Upstream's whole point is that the completion survives the turn that missed it.
**Fix** — Add `background/completion_replay.rs` porting the record + archive shapes (including the 64 KiB UTF-8-safe tail, which needs a char-boundary-correct truncation, not a byte slice), write both from `background/watch.rs`'s terminal-transition path, and project a `WaitCompletion` into the `wait` tool's structured result. Sequence after **SUBA-034** so the same terminal-transition seam carries both.
**Verify** — Complete a background run while no `wait` is outstanding, then call `wait {id}`; it must return the child's outcome and artifact paths from the replay record rather than "no active runs". Assert the archive prefers the output artifact path over inline text when the artifact exists.

## SUBA-057 — The `dismiss` action is unported, so a recovered async workflow with no live controller is stuck "running" in the fleet forever — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 9) — see the table row for the evidence. Everything below is the
> filing text and its cyrup line is now stale.** By the time this was taken, the whole READ half
> was already at HEAD (the `display_dismissed_at` field, `ReconcileAction::DisplayDismissed`,
> `list_active_runs`'s `continue`, the `State: display-dismissed` report) — what was missing was
> the only WRITER, `SubagentExecutor::control_dismiss`, which `background/mod.rs:933` already
> intra-doc-linked as if it existed. It now exists, with the enum entry, the dispatch arm and the
> child-safe gate.

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — no `dismiss` in the 27-verb enum; `rg 'dismiss' crates/cyrup-ext-subagents/src` = 0. cyrup's reconciliation path is `background/reconcile.rs` + `background/run_status.rs`, which can mark a run stale but has no user-driven display dismissal and no `displayDismissedAt` field.
**upstream** — `pi-subagents/src/runs/foreground/async-dismiss-action.ts` @v0.47.1 (new, 85 lines; absent at v0.43.0) — `dismissRecoveredWorkflow` refuses when the run is not a recovered workflow, not in the active session, still has a live controller, or is not `running`; otherwise stamps `displayDismissedAt`, re-reconciles, calls `updateActiveRunIndex(asyncDir, "complete")` and drops the run from `asyncJobs`/`fleetJobs`. Dispatched at `subagent-executor.ts:5266`; `dismiss` is in `SUBAGENT_ACTIONS` (`shared/types.ts:1968`) and `MUTATING_MANAGEMENT_ACTIONS` (`:167`). Landed in `ade35ea` ("fix: dismiss recovered workflow displays", #1011), released v0.47.1.
**Impact** — After a crash or restart, a workflow whose runner process is gone but whose `status.json` still says `running` is displayed as live in the fleet widget and overlay indefinitely. The operator has no supported way to clear it short of deleting files under the async root by hand.
**Fix** — Add `dismiss` to the enum at `extension.rs:6557` and a `route_control_action` arm porting the five refusal conditions verbatim (each with upstream's exact message), stamping a `display_dismissed_at` on the status record, re-running `background::reconcile`, and evicting the run from `tui/fleet_state.rs`. Add it to the child-safe mutating denylist alongside the existing seven.
**Verify** — Kill a background runner mid-flight, restart the session, then `{action:"dismiss", id}`; the run must disappear from `/subagents-fleet` and `{action:"status"}`, while a run with a live controller must be refused with *"still has a live controller and cannot be dismissed."*

## SUBA-064 — The entire `authorityPolicy` subsystem is unported, and the `stop`/`steer` gate it drives is live-reachable in cyrup today

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
*(Filed from the refuter's independent denial-path sweep; the audit cited `resolveAuthorityDecision` inside SUBA-046's upstream evidence and did not file the subsystem.)*
**cyrup** — `rg -n 'authority|Authority' crates/cyrup-ext-subagents/src` returns exactly two hits, neither an implementation: `extension.rs:7574`'s own comment places the mission arm *"after the management/control arms, before the authority-policy arm"* — naming a gate that does not exist — and `exec/task_intent.rs:7` is unrelated prose. `stop` and `steer` **are** both implemented and dispatched (`extension.rs:7805`, `:7825`), and `registration/mod.rs`'s only config validator is `validate_missions` (`:256`), so an `authorityPolicy` in `config.json` is silently dropped with no error and the action executes.
**upstream** — `pi-subagents/src/policy/authority.ts` present at **both** v0.43.0 and v0.47.1 — `:1-8` `AUTHORITY_ACTIONS` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `scheduleCreate`, `stopRun`, `steerRun`), `:11` `AuthorityDecision = auto|confirm|forbid`, `:14-21` `DEFAULT_AUTHORITY_POLICY` (three actions defaulting to `confirm`), `:23` `resolveAuthorityDecision`, `:30` `validateAuthorityPolicy` throwing typed errors for both an unknown action and a bad decision. Four consumers at v0.43.0: `subagent-executor.ts:4358` (discardWorktree), `:4491` (spawnBudgetGrant), `worktree.ts:607`, `herdr/actions.ts:205-206` — plus, critically, `subagent-executor.ts:4412-4423`, which maps `stop`→`stopRun` and `steer`→`steerRun` and then forbids with `Authority policy forbids action '${action}'.` or requires `ctx.ui.confirm` with `Authority policy requires user confirmation for action '${action}'…`, including an explicit no-UI refusal branch.
**Impact** — An operator who sets `"authorityPolicy": {"stopRun": "forbid"}` has it silently dropped and the action executes anyway — the config is accepted, unvalidated, and inert. Unlike most items here the gated actions are already live in cyrup, so this is not a dormant gap: it is a policy surface that a user can configure and that does nothing. It is also the prerequisite for **SUBA-046**'s grant path and for the `worktree.discard` / `destructiveCleanup` verbs SUBA-005 lists as unowned.
**Fix** — Port `authority.ts` as `registration/authority.rs`: the six-action enum, the three-way decision, `DEFAULT_AUTHORITY_POLICY`, `resolve_authority_decision`, and `validate_authority_policy` with both typed errors wired into config load beside `validate_missions` (`registration/mod.rs:256`). Consult it in the `stop` and `steer` dispatch arms (`extension.rs:7805`, `:7825`) with pi's exact forbid/confirm/no-UI messages, routing the confirm through `HostServices`. **Hard prerequisite, recorded so it is not rediscovered:** this item is held at `medium` only because the four destructive `AUTHORITY_ACTIONS` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `scheduleCreate`) have no dispatch in cyrup to bypass. Whoever lands `worktree.discard` or `destructiveCleanup` (two of `SUBA-005`'s unowned verbs) must land the authority gate **in the same change** and raise this item to `critical` if they do not — shipping a destructive verb behind a config key that is parsed and ignored is a permission bypass by `README.md:106-107`'s definition.
**Verify** — `"authorityPolicy": {"stopRun": "forbid"}` must make `{action:"stop", id}` fail with `Authority policy forbids action 'stop'.`; `{"steerRun": "confirm"}` with no UI available must refuse with pi's no-UI message; an unknown action key or a bad decision value must fail config load with pi's typed error rather than being ignored.

## SUBA-017 — Completion batching unported

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `completion_batch`/`batcher` crate-wide; no `completionBatch` key on the 18-field `SubagentExtensionConfig` (`registration/mod.rs`). `background/watch.rs` invokes `CompletionSink` once per result with no debounce, and its own test pins the one-notify-per-result contract.
**upstream** — `pi-subagents/src/runs/background/completion-batcher.ts` passes `git cat-file -e v0.43.0` — **in-baseline**, so this is unported work, not expected lag — and is wired at `extension/index.ts:376` @v0.43.0 (`registerSubagentNotify(pi, state, { batchConfig: config.completionBatch })`).
**Impact** — Ten background runs finishing together produce ten separate notices instead of one batched summary; in a large fan-out the notice stream buries the actual results.
**Fix** — Port as `background/batcher.rs` between `CompletionWatcher` and `CompletionSink`, with the debounce window and aggregate notice, plus the `completionBatch` config key. Same seam as SUBA-034 and SUBA-056 — do them together.
**Verify** — Complete five runs within the window; exactly one aggregate notice must be emitted.

## SUBA-022 — Typed extension delegation API unported

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `ls crates/cyrup-ext-subagents/src` at HEAD has no `api/` module; `rg -c 'prompt-template:subagent' crates/` = 0.
**upstream** — `pi-subagents/src/api/delegation.ts` passes `git cat-file -e v0.43.0` — **in-baseline** — and is present at v0.47.1, exporting five event names (`prompt-template:subagent:request|started|update|response|cancel`, `:4-8`).
**Impact** — Other extensions cannot delegate to subagents through a typed API; any integration must go through the tool surface. Low because no in-tree consumer exists yet.
**Fix** — Port as `api/delegation.rs` over the crate's existing event surface, emitting the five event names verbatim. Sequence after SUBA-018 (now closed), whose `prompt_workflows.rs` is the natural first consumer.
**Verify** — A second extension subscribing to `prompt-template:subagent:response` must receive a payload for a delegation it requested.

## SUBA-023 — Async lifecycle hardening unported; no signal-name attribution

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/signal.rs:90-106` — `TerminationOutcome` carries only `status: ExitStatus` and `stage: EscalationStage`; no `ExitStatus::signal()` name mapping anywhere in the module. `process_terminal` and `session_lease` are zero-hit crate-wide.
**upstream** — `src/runs/background/process-terminal.ts` and `src/runs/shared/session-lease.ts` both present at v0.47.1 (PARITY-GAPS VL-S3/VL-S4).
**Impact** — Signal attribution in run results stays coarse ("failed" rather than "killed by SIGKILL"), which makes escalation-ladder debugging harder, and there is no session lease to arbitrate two hosts touching the same run tree.
**Fix** — Independently useful now and cheap: map `ExitStatus::signal()` to a name in `TerminationOutcome`. The two upstream subsystems port after that.
**Verify** — A child killed at the SIGKILL rung must report the signal name in its run record.

## SUBA-024 — `parallel-handoff` / `agent-contract` unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed *(raised from probable on the two surviving names; the two named blind spots below are unchanged)*
**cyrup** — `task_intent` **is** ported (`exec/task_intent.rs`, 1967 lines, consumed from `completion_guard.rs`, `acceptance.rs`, `output.rs`, `mod.rs`) and is struck from this item. `parallel_handoff` and `agent_contract` remain zero-hit crate-wide.
**upstream** — `src/runs/shared/parallel-handoff.ts` and `src/runs/shared/agent-contract.ts` are present at **both** v0.43.0 and v0.47.1. `chain-validation.ts` is **struck from the item entirely**: `git log --all -- src/runs/shared/chain-validation.ts` is empty — the file never existed at any path, so the original item named a symbol that was never real.
**Impact** — No structured handoff between parallel siblings and no agent-contract validation, so a fan-out's per-child expectations are unenforced.
**Fix** — Port both as `spawn/parallel_handoff.rs` and `discovery/agent_contract.rs`.
**Verify** — N/A until scheduled. **Blind spot carried forward for the third pass running:** `spawn/chain_graph.rs`'s pre-walk validation and `ChainStepConfig`'s unknown-key handling have still not been re-read; either could already cover part of the handoff surface.

## SUBA-026 — Interactive admin UI, selector and `/subagents` unported

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
*(Partially closed: `/subagents-stop` now exists at `registration/slash_commands.rs:144`.)*
**cyrup** — The 16-variant `as_str` match at `registration/slash_commands.rs:127-146` has neither `subagents` nor a selector surface; `rg 'subagents-admin|SubagentsAdmin' crates/cyrup-ext-subagents/src` = 0.
**upstream** — `src/slash/subagents-admin.ts` (432 lines) and `src/tui/selector.ts`; `subagents` is registered at `slash-commands.ts:655` @v0.47.1. Matches PARITY-GAPS VL-S11.
**Impact** — No interactive agent picker and no admin surface; every management operation must go through the tool's action verbs.
**Fix** — Port alongside the existing FleetView surface (`tui/fleet*.rs`), which now supplies the rendering primitives this needs.
**Verify** — `/subagents` must open the picker and list the same agents `{action:"list"}` returns.

## SUBA-029 — Management actions read-modify-write subagents `settings.json` unlocked

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/settings_write.rs:70-81`: `write_settings_file` is `create_dir_all` then a bare `std::fs::write` — no lock, no temp-then-rename; `read_settings_file_strict` is a separate unlocked call, so the disable/enable/reset handlers perform an unsynchronised read-modify-write.
**upstream** — `pi-subagents/src/agents/agents.ts` is likewise unlocked, which is why this stays `cyrup-original` rather than a parity gap: the bar cyrup fails is its **own** (`cyrup-config/src/settings.rs` uses `FileLock` + `write_atomic`), not pi's.
**Impact** — Two concurrent disable/enable/reset actions can lose one another's write, or leave a truncated `settings.json` if the process dies mid-write, disabling every agent until it is hand-repaired.
**Fix** — Hold one lock across read+write in `settings_write.rs` and route the write through the crate's own `background/atomic.rs::write_atomic_json`.
**Verify** — Two concurrent `disable` calls on different agents must both persist; kill mid-write and the file must remain parseable.

## SUBA-033 — Tests assert a lower bound on observed concurrency

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/parallel.rs:739-742` asserts `observed_peak >= 2` (the `<= cap` upper bound at `:734` is the real invariant and is unaffected). **Citation corrected this pass:** the second instance is at **`:1133`** (`peak.load(Ordering::SeqCst) >= 2`), not at `:802-810` — `:798-812` contains only the `<= global_cap` assertion.
**upstream** — No counterpart; `spawn/parallel.rs` is a documented cyrup-original bounded-concurrency primitive. The precedent is commit `1806375`, which deleted an `orphaned > 0` lower bound for exactly this reason.
**Impact** — On a single-core or heavily loaded runner these flake, and a flaky concurrency test is the fastest route to an ignored concurrency test.
**Fix** — Make the overlap deterministic with a `tokio::sync::Barrier` sized to the expected concurrency inside the worker, then keep only the `<= cap` assertions.
**Verify** — Tests pass deterministically under `--test-threads=1` on a single-core cgroup.

## SUBA-034 — `wait`'s event-bus wake unported; pure polling at a 1 s floor — **CLOSED 2026-08-15 (REFUTED / already-done)**

> **REFUTED, CLOSED 2026-08-15 (sweep 11).** The wake landed in sweep 9 (`844e25f`) and the row was
> never marked. Everything below is the filing text and its cyrup line is stale: `background/wait.rs`
> carries a `# Wake mechanism (SUBA-034)` section, `WaitDeps::completion_bus`, and a `biased;`
> `select!` over a `broadcast::Receiver`. No fix was manufactured.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/wait.rs:30-34` documents the missing subscription as a deliberate delta; `DEFAULT_POLL_INTERVAL_MS = 1000` (`:67-69`) is the only interval used (`:179-184`); no broadcast/subscribe in the file.
**upstream** — `pi-subagents/src/runs/background/wait.ts` subscribes to the completion/control channels and wakes the instant either fires, keeping the poll as reconciliation. Upstream additionally gained `wait-subscriptions.ts` at v0.41.0 (PARITY-GAPS VL-S8).
**Impact** — Up to one second of added latency per wait, paid repeatedly by a fan-out of short runs. Low because the polling fallback is functionally correct.
**Fix** — Have `CompletionWatcher` (`background/watch.rs`) publish terminal transitions on a broadcast channel the wait loop `select!`s against, keeping the poll as reconciliation. Same seam as SUBA-017 and SUBA-056.
**Verify** — A run that completes 50 ms into a wait must return in ~50 ms, not ~1 s.

## SUBA-035 — Active `subagents.modelScope` policy not surfaced by doctor/models — **CLOSED 2026-08-14 (REFUTED / already-done)**

> **CLOSED 2026-08-14 (sweep 8). Everything below is the filing text.** Doctor half:
> `registration/doctor.rs:646-676` `model_scope_check`, called at `:603`, tests `:1609`-`:1642`.
> Models-report half: `extension.rs::run_models_report`, both views, each citing SUBA-035 in-source.
> **The residual's stated location (`registration/mod.rs` / `profiles.rs`) was wrong** — that
> correction is the part of this row worth keeping.


**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `rg 'model_scope|modelScope' crates/cyrup-ext-subagents/src/registration/doctor.rs` returns nothing across all 1803 lines; the models report reads only the current model. Enforcement itself is live (`exec/model_scope.rs:170-188`).
**upstream** — `pi-subagents/src/runs/shared/model-scope.ts` surfaces warn-severity violations and validates the config as part of its settings surface.
**Impact** — An operator debugging "why did my model choice not apply" gets no hint from `/subagents-doctor` or the models report that a scope policy is filtering it. Compounds with **SUBA-050**: once `strict` exists, an unsurfaced policy becomes an unexplained hard failure rather than an unexplained warning.
**Fix** — One diagnostic in `registration/doctor.rs` reading the resolved `ModelScopeConfig`, plus the same line in the models-report header. Include `strict` once SUBA-050 lands.
**Verify** — With a scope configured, `/subagents-doctor` must print the active scope and its severity.

## SUBA-037 — Doctor's `--version` binary probe leaks the probe process on timeout

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/registration/doctor.rs:340-352`: the probe builder sets stdin/stdout/stderr to null and calls `.status()`, then is raced by `tokio::time::timeout(VERSION_PROBE_TIMEOUT, probe)`; there is no `.kill_on_drop(true)`, so on timeout the future is dropped and the child is leaked. The crate now has three `kill_on_drop` sites (`extension.rs:11553`, `watchdog/lsp_diagnostics.rs:907`, plus the acceptance ladder), so the pattern is well established here — this one site was missed.
**upstream** — `pi-subagents/src/extension/doctor.ts` spawns no subprocess, so the probe is cyrup-original; the in-codebase contract for a timed-out child is the acceptance ladder (SIGTERM then a hard SIGKILL), now enforced after SUBA-027.
**Impact** — `/subagents-doctor` on a misconfigured install — exactly what doctor exists for — can leave a hung `cyrup --version` behind on every invocation, and the report says the probe timed out without saying anything survived.
**Fix** — Add `.kill_on_drop(true)` to the probe builder. The probe does not set `process_group(0)`, so the pid-targeted SIGKILL suffices and no group logic is needed.
**Verify** — Point the subagent binary at a script that `exec sleep 300`, run the check with a 100 ms `VERSION_PROBE_TIMEOUT`; after it returns Timeout, `kill(probe_pid, 0)` must fail with ESRCH.

## SUBA-038 — Three denial / unknown-action messages still diverge from pi's text

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed
*(Partially closed: the child-safe UNAVAILABLE text is now verbatim — `extension.rs:7585`, `:7636`, `:17190` emit `Action '{action}' is not available from child-safe subagent fanout mode.`, matching `subagent-executor.ts:4380` @v0.43.0.)*
**cyrup** — Three residuals, the third found by the refuter in the same sweep:
1. `extension.rs:7698-7702` — the MUTATING denial still emits cyrup's own *"subagent management action '{action}' is blocked in child-safe fanout mode; {list} are not permitted here."*, pinned by substring tests at `:14194` and `:18227`.
2. `extension.rs:7610-7614` — the unknown-action text is *"unknown subagent action '{other}'; valid actions are …"* against upstream's `Unknown action: ${action}. Valid: …`, **and cyrup's hand-written list omits the four `watchdog.*` verbs that do dispatch.**
3. `extension.rs:7851-7854` — the CONTROL arm's unknown-action message lists *"status, interrupt, resume, steer, append-step"* and omits `stop`, which is advertised in the enum and dispatched at `:7805`. Same advertise-vs-message drift, same fix.
**upstream** — `subagent-executor.ts:4861` @v0.43.0 for the unknown-action text; `:167` for the denylist. Note the v0.47.1 message is richer still — that is **SUBA-065**, filed separately.
**Impact** — A fanout child attempting a mutating or unknown action sees different text than pi emits, and in two of three cases a *wrong* list of valid actions, so a model recovering from the error is steered away from verbs that exist. Model-visible; no behavioural difference in what is blocked.
**Fix** — Replace all three format strings with pi's verbatim text, derive the valid-action list from the enum rather than hand-writing it (which is what let two of them drift), and rewrite the two substring assertions at `:14194`/`:18227` to equality against the new text.
**Verify** — `{action:"delete", agent:"x"}` in child-safe mode, `{action:"bogus"}`, and `{action:"bogus"}` on the control arm must all produce text byte-identical to upstream's strings, with every dispatching verb present in the list.

## SUBA-039 — `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a detached process group

**Kind** cyrup-original · **Severity** low · **Effort** M · **Confidence** confirmed on mechanism, probable on reachability
**cyrup** — `rg 'impl Drop' crates/cyrup-ext-subagents/src` returns exactly five impls — `background/runner_main.rs:2712` (`SigUsr2Guard`), `:3003` (`ControlWatcherHandle`), `background/control.rs:1684` (`AppendLockGuard`), `background/watch.rs:762` (`CompletionWatcherHandle`), `registration/profiles.rs:1006` (`RestorePerms`) — and **none is for `SpawnedChild`**, while `spawn/mod.rs` sets `command.process_group(0)` on every child. `spawn/signal.rs:205` now names the exact hazard in its own doc: *"with no kill_on_drop leaves the whole process group running for the machine's uptime"*.
**upstream** — `pi-subagents` never passes `detached`, so its children stay in pi's own process group and a terminal signal reaches the whole tree regardless of how a promise unwinds. The guard upstream gets for free must be written here.
**Impact** — An orphaned subagent subtree — a re-exec'd `cyrup` plus whatever cargo/npm/git it is blocked in — runs for the machine's uptime, unreachable by Ctrl-C. Low because neither in-crate driver drops today; this is a missing safety invariant one careless `select!`/`timeout`/`JoinHandle::abort` away from firing.
**Fix** — Add `impl Drop for SpawnedChild` that on Unix best-effort SIGKILLs `-pgid` when the child leads its group, reusing the `getpgid(pid) == pid` guard from `spawn::signal::send_signal`. The existing `exited` flag makes the guard a no-op on the normal paths. `kill_on_drop(true)` alone is **not** adequate: it targets the bare pid and leaves the descendants this item is about.
**Verify** — Construct a `SpawnedChild` running `sh -c 'sleep 300 & echo $! > gpid; wait'`, drop it without `terminate`/`finish`, and assert `kill(descendant_pid, 0)` fails with ESRCH.

## SUBA-058 — Chain read instructions are not filtered by existence, so children are told to read files that are not there

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:726-736` — the reads list is mapped straight through `resolve_chain_path` into `[Read from: {}]` with no filesystem check; the test at `:2522` pins the unfiltered output.
**upstream** — `pi-subagents/src/shared/settings.ts:356-362` @v0.47.1 — `resolveExistingReadInstructionPaths(reads, instructionCwd, existenceCwd)` resolves each path twice and emits only those where `fs.existsSync(existencePath)` holds. At v0.43.0 `settings.ts:359` was still the unfiltered `.map(resolveChainPath)`. Landed in `bc1b689` ("fix: omit missing child read files"), released v0.47.1.
**Impact** — A chain step whose upstream sibling produced no output still receives `[Read from: <chain_dir>/analysis.md]`. The child burns a turn on a failing read and often narrates the missing file as a finding, polluting the chain's `{previous}` text for every later step. Low because it degrades quality rather than correctness.
**Fix** — Add the existence filter to the instruction builder at `spawn/chain_graph.rs:730`, taking pi's two-cwd form (instruction path vs existence path) so a worktree child checks the right tree. Update the pinned expectation at `:2522`. Land with **SUBA-053** and **SUBA-054**, which touch the same builder.
**Verify** — A chain step declaring two reads where only one file exists must emit `[Read from: <the existing one>]`; a step where none exists must emit no read line at all.

## SUBA-059 — `artifactConfig.cleanupDays` is never wired to the type that already parses it

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: the gap is **narrower** than the audit described — `ArtifactConfig` already exists in cyrup and already deserializes `cleanupDays`; the whole defect is the missing wire from `SubagentExtensionConfig` to it. Do not "add a `cleanup_days` field" — it is there.)*
**cyrup** — `crates/cyrup-ext-subagents/src/artifacts.rs:85-113` already defines `#[serde(rename_all = "camelCase")] pub struct ArtifactConfig { pub cleanup_days: u64, … }` defaulting to 7 and citing pi's `DEFAULT_ARTIFACT_CONFIG`. But both call sites pass the hardcoded constant instead: `extension.rs:9235-9238` and `:9403-9406` call `cleanup_all_artifact_dirs(&self.cwd, crate::artifacts::DEFAULT_CLEANUP_DAYS)` (`artifacts.rs:50` = 7), and the 18-field `SubagentExtensionConfig` has no `artifact_config`. Confirmed separately: `cleanup_old_artifacts` (`artifacts.rs:277-302`) computes `cutoff = now - max_age_days * ONE_DAY_MS`, so a literal `0` would mean "delete everything", not "disable".
**upstream** — `pi-subagents/src/extension/index.ts:369-370` @v0.47.1 — `const artifactCleanupDays = config.artifactConfig?.cleanupDays ?? DEFAULT_ARTIFACT_CONFIG.cleanupDays; cleanupAllArtifactDirs(artifactCleanupDays);`, validated at `extension/config.ts:41-47` (non-negative integer), semantics at `shared/types.ts:1859` ("Set cleanupDays to 0 to disable cleanup"). Landed in `b69aafb` ("fix: honor artifact cleanup retention config", #1013), released v0.47.1.
**Impact** — A user who wants subagent transcripts kept for audit (or deleted sooner) cannot say so, and there is no way to disable the sweep — cyrup silently deletes run inputs, outputs and JSONL older than a week on every extension load, including where those files are the record of what a fan-out actually did.
**Fix** — Add `artifact_config: Option<ArtifactConfig>` to `SubagentExtensionConfig` (the struct already exists), validate as a non-negative integer with upstream's error text, pass it at `extension.rs:9236` and `:9404`, and add an explicit `0 ⇒ skip` arm to `cleanup_all_artifact_dirs`/`cleanup_old_artifacts` so zero disables rather than purges.
**Verify** — With `"artifactConfig": {"cleanupDays": 0}`, an artifact with an mtime a year old must survive an extension load; with `30`, a 40-day-old artifact must be removed and a 20-day-old one kept.

## SUBA-060 — "Resume-first" guidance for failed async runs is unported, so the parent relaunches work a persisted session could continue

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `rg -c 'Resume-first|resume_first|resume_guidance' crates/cyrup-ext-subagents/src` = 0. `background/run_status.rs` and the completion-notice path emit failure text with no revive suggestion, even though cyrup **has** the revive machinery (`ResumeOutcome::RespawnFromTranscript`, `background/control.rs:1214`, routed at `extension.rs:4210-4212`).
**upstream** — `pi-subagents/src/runs/background/resume-guidance.ts` @v0.47.1 (new, 33 lines; absent at v0.43.0) — `formatAsyncReviveCommand` (`:5`) picks the failed step with an existing `sessionFile` and formats a literal `subagent({ action: "resume", id: "…", index: N, message: "Continue from the persisted child session…" })`; `formatResumeFirstFailedRunDetail` (`:16`) and `formatResumeFirstFailedRunsNote` (`:25`) fold it into status output with *"Launch a replacement only if revive fails or the user explicitly asks for one."* Landed in `b283d27`, released v0.45.2.
**Impact** — When a background run fails, cyrup's status/notice text says only that it failed. An orchestrator model's default response is to spawn a fresh child, discarding the failed child's persisted session and re-paying for work already done.
**Fix** — Port the three formatters as `background/resume_guidance.rs` over `RunStatus`/step records (the `session_file` field already exists — `background/wait.rs:543` shows it on the summary shape), and append the note in `run_status`'s failure rendering and the completion-notice text. The `resume` verb and `index` param already exist. **Precision note:** upstream omits `index` entirely when `steps.length === 1` (`:12-13`) — reproduce that, do not always emit `index: 0`.
**Verify** — A failed two-step async run whose step 1 has a persisted session file must produce a status body containing the literal `subagent({ action: "resume", id: "…", index: 0, message: … })` call, byte-identical to upstream's format; a failed single-step run must omit `index`.

## SUBA-061 — Four documented config keys are silently ignored

**Kind** not-ported *(mixed — see below)* · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `SubagentExtensionConfig` (`crates/cyrup-ext-subagents/src/registration/mod.rs`) has exactly 18 fields: `async_by_default`, `force_top_level_async`, `global_concurrency_limit`, `max_subagent_spawns_per_session`, `parallel`, `control`, `chain`, `proactive_skill_subagents`, `default_session_dir`, `single_run_output_base_dir`, `max_subagent_depth`, `worktree_base_dir`, `worktree_setup_hook`, `worktree_setup_hook_timeout_ms`, `fleet_view`, `fleet_view_placement`, `wait_tool`, `missions`. `rg -c 'fleetKeybindings|legacyChainControls|asyncWidget|inlineToolDisplay'` (and the snake_case forms) = 0 for all four.
**upstream** — Two are **in-baseline** (hence the `not-ported` kind): `asyncWidget?: boolean` at `shared/types.ts:1750` @v0.43.0 ("Show the under-editor async runs widget. Defaults to true") and `inlineToolDisplay?: InlineToolDisplay` at `:1754` ("Inline chat rendering for the subagent tool. Defaults to rich"). Two are **drift**: `fleetKeybindings` (`:1827` @v0.47.1, validated at `extension/config.ts:26-36` against `FLEET_KEYBINDING_ACTIONS`, from `da98baa`, v0.46.0) and `legacyChainControls` (`:1833`, validated at `config.ts:52-54`, used at `tool-description.ts:156-170` to trim the append-step/checkpoint guidance, from `889a798`, v0.47.0).
**Impact** — Four documented keys are accepted into `config.json` and dropped with no validation error. Concretely: a user cannot hide the under-editor async widget while keeping FleetView, cannot switch the subagent tool's inline chat rendering, cannot rebind any Fleet key, and cannot trim the legacy chain-control guidance out of the tool description to save context. The same advertised-and-ignored shape as the closed SUBA-N05.
**Fix** — Add all four to `SubagentExtensionConfig` with upstream's validation errors verbatim; wire `async_widget` into the `set_widget` calls at `extension.rs:9489/:9889/:9978`, `fleet_keybindings` into `tui/fleet.rs`'s key dispatch, `inline_tool_display` into the renderers at `extension.rs:9616/:9646`, and `legacy_chain_controls` into the description builder **SUBA-025** introduces — so sequence after it.
**Verify** — Each key must round-trip through `config.json` and change observable behaviour; an invalid value for any of them must produce upstream's exact error text rather than being dropped.

## SUBA-062 — cyrup's bundled `researcher` agent cannot do web research because the crate's target has no web tools

**Kind** cyrup-original · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/resources/agents/researcher.md:4` declares `tools: read, grep, find, ls, write, intercom` and its body rewrites every web step into a filesystem step ("use `find`/`ls` to locate them, `grep` to search across them"), with a "Note on web access" section replacing upstream's workflow. The cause is upstream of this crate: `ls crates/cyrup-tools/src/tools/` is `bash, edit, edit_diff, find, globmatch, grep, ls, read, write` — no `web_search`, `fetch_content` or `get_search_content` — and `rg -l 'web_search|fetch_content|get_search_content' crates/` matches only two files inside cyrup-ext-subagents itself (a test and a doc string).
**upstream** — `pi-subagents/agents/researcher.md:3-4` @v0.47.1 — `description: Autonomous web researcher — searches, evaluates, and synthesizes a focused research brief`, `tools: read, write, web_search, fetch_content, get_search_content, intercom`, with body rules built on `web_search`.
**Impact** — A user who delegates to the shipped `researcher` expecting pi's behaviour gets a working-tree grep instead of web research, and the divergence is invisible until the brief comes back sourced entirely from local files. Filed as `cyrup-original` rather than a bundled-agent bug because the agent file is a faithful adaptation to a real missing capability.
**Fix** — **Handoff:** file the missing `web_search` / `fetch_content` / `get_search_content` tools against the tools area (04 built-in tools / 12 pi drift) — they are not this crate's to build. Inside this crate the only change owed now is a `[CYRUP-DELTA]` header in `resources/agents/researcher.md` naming the upstream file it diverges from and why, so **SUBA-044**'s bundled-agent diff does not read it as drift; restore upstream's text the moment the tools land.
**Verify** — Once the web tools exist, `diff <(git -C pi-subagents show v0.47.1:agents/researcher.md) crates/cyrup-ext-subagents/resources/agents/researcher.md` must be empty.

## SUBA-063 — Zero-tool-budget authorisation and the runtime-extension acknowledgement path are unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: the audit's third part — "the async events log is unbounded" — is **refuted and struck**. `crates/cyrup-ext-subagents/src/jsonl.rs` is a shared `BoundedJsonlWriter` with `DEFAULT_JSONL_CAP_BYTES = 50 * 1024 * 1024` at `:51`, the same 50 MB as pi's `DEFAULT_MAX_ASYNC_EVENTS_BYTES`, and `runner_main.rs:711` opens the run's `events.jsonl` through it. `events.jsonl` **is** capped. What is missing is only the `PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES` override and the `subagent.events.truncated` marker event — a much smaller gap, noted here so nobody builds a cap that already exists.)*
**cyrup** — An exhaustive diff of the child-env surface — `rg --only-matching '"CYRUP_SUBAGENTS?_[A-Z0-9_]*"' crates/cyrup-ext-subagents/src | sort -u` (48 names) against the upstream inventory (42 `PI_SUBAGENT*` names at v0.43.0) — shows no counterpart to `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` or `PI_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS`; `rg -c 'ZERO_AUTH|RUNTIME_ACKNOWLEDGED'` = 0 for both.
**upstream** — @v0.43.0: `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` is defined at `runs/shared/tool-budget.ts:5`, written at `pi-args.ts:771` (`input.allowZeroToolBudget ? "1" : undefined`) and read child-side at `subagent-prompt-runtime.ts:476` (`decodeToolBudgetEnv(…, { allowZero: … === "1" })`, `{minimumHard: 0}`); `RUNTIME_EXTENSION_ACK_PATH_ENV` is written at `pi-args.ts:604-609` and returned as `runtimeAcknowledgedExtensionsPath` at `:787` for the parent to read back.
**Impact** — (a) A `toolBudget` of zero — "this child may make no tool calls at all" — cannot be distinguished from "no budget", so a deliberately tool-less lane cannot be expressed. (b) The parent has no record of which runtime extensions the child actually acknowledged, so an extension that silently failed to load in the child looks identical to one that loaded — the same blind spot as **SUBA-045**, one layer up.
**Fix** — Add the zero-authorisation flag to `exec/tool_budget.rs`'s encode/decode pair, set from the caller's explicit `0` (depends on **SUBA-047** for a caller surface); write an acknowledgement path into the child env in `build_attempt_spawn_plan` and read it back in `run_attempt` alongside SUBA-045's tool diagnostic (same temp dir, same read-back point). Separately and cheaply: add the `ASYNC_EVENTS_MAX_BYTES` override and the truncation marker over the existing `BoundedJsonlWriter`.
**Verify** — `toolBudget:{hard:0}` must block the child's first tool call rather than being treated as unset; a child asked to load a nonexistent extension must leave an acknowledgement file that omits it and the parent must report it.

## SUBA-065 — `unknownSubagentActionMessage` — the did-you-mean recovery and its destructive-action safety gate — is unported

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
*(Filed from the refuter's sweep. Note it comes out of `28b9222`, a commit the audit's coverage claims to have diffed line by line — evidence that "diffed the commit" and "derived every behaviour in it" are different acts.)*
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:7610-7614` returns a static hand-written list with no suggestion machinery, and there is no destructive-action set anywhere in the crate (`rg DESTRUCTIVE` = 0; `MUTATING_MANAGEMENT_ACTIONS` at `discovery/management.rs:1387` is a different 7-element set serving the child-safe denylist).
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:167-208` @v0.47.1 — `DESTRUCTIVE_MANAGEMENT_ACTIONS` (13 verbs incl. `delete`, `eject`, `reset`, `stop`, `interrupt`, `reject-checkpoint`, `schedule.delete`), `editDistance` (`:170`), `hasSingleAdjacentTransposition` (`:186-192`), and `unknownSubagentActionMessage` (`:195-208`) which suggests the closest `SUBAGENT_ACTIONS` candidate but applies a **deliberately stricter** rule to destructive candidates — `distance === 1 && requested.length >= candidate.length - 1` (`:200`) — so a loose typo is never nudged toward `delete`. It also appends a fixed nextStep. At v0.43.0 the message was the bare `Unknown action: ${action}. Valid: …` at `:4861`, so this landed in range via `28b9222`.
**Impact** — A model that mistypes an action gets a wall of 27 names and no suggestion, so recovery costs a turn. More interesting is the half cyrup could get *wrong* later: porting a naive did-you-mean without the destructive gate would nudge a typo toward `delete`, which is exactly what upstream's asymmetric rule exists to prevent. Low today, but it is a landmine for whoever improves the message casually.
**Fix** — Port `DESTRUCTIVE_MANAGEMENT_ACTIONS`, `edit_distance`, `has_single_adjacent_transposition` and `unknown_subagent_action_message` into `extension.rs` beside the unknown-action arm, **including the asymmetric destructive rule verbatim**, and derive the candidate list from the enum. Land with **SUBA-038**, which rewrites the same three messages.
**Verify** — `{action:"statu"}` must suggest `status`; `{action:"delet"}` must **not** suggest `delete` (distance 1 but `requested.length < candidate.length - 1` fails the gate is the wrong direction — assert against upstream's own table); an unknown action with no near candidate must fall back to the plain list.

## SUBA-066 — The `/subagents-guide` slash command is unported, and falls outside both VL-S11 and SUBA-055 — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 11)**, landed with `SUBA-055` as this item's own Fix line instructs.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — The complete slash set is the 16-variant `as_str` match at `crates/cyrup-ext-subagents/src/registration/slash_commands.rs:127-146`; `subagents-guide` is not in it.
**upstream** — `git show v0.47.1:src/slash/slash-commands.ts | grep 'registerCommand('` yields 15: `subagents`(`:655`), `run`(`:662`), `subagent-cost`(`:692`), `subagents-doctor`(`:699`), **`subagents-guide`(`:706`)**, `subagents-refine`(`:721`), `subagents-fleet`(`:734`), `subagents-detach`(`:744`), `subagents-stop`(`:771`), `subagents-models`(`:822`), `subagents-profiles`(`:845`), `subagents-load-profile`(`:857`), `subagents-refresh-provider-models`(`:910`), `subagents-generate-profiles`(`:944`), `subagents-check-profile`(`:982`).
**Impact** — The user-facing half of the guide feature has no owner: PARITY-GAPS VL-S11 names exactly three missing commands (`/subagents`, `/subagents-refine`, `/subagents-detach`) and **SUBA-055** scopes itself to the `guide` *action* plus the packaged docs. A user who reads pi's docs and types `/subagents-guide` gets an unknown command. Low severity, but this is precisely the kind of item that silently survives a "guide is filed" checkmark on the next pass.
**Fix** — Add the `SubagentsGuide` variant and descriptor to `registration/slash_commands.rs:127-146`, routing to the same `read_subagent_guide` **SUBA-055** introduces. Land the two together; this is the last mile of that item, not an independent subsystem.
**Verify** — `/subagents-guide` must render the overview topic, and `/subagents-guide tool-reference` the tool-reference topic, identical to the corresponding `{action:"guide"}` output.

---

## SUBA-067 — The descendant-termination fixture exec-collapsed to a single process, so the test never exercised group-kill

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** FIXED
**cyrup** — `spawn/mod.rs:1633-1636` built `sh -c 'echo $$ > …; mv …; exec sleep 300'` and handed it to `sh_command()` (`mod.rs:1098-1110`), which runs `/bin/sh -c <script>`. A `-c` script that is a SINGLE simple command triggers the shell's exec-through optimization: the outer shell replaces itself with the inner `sh`, which then `exec`s `sleep` — one pid, never a tree. Measured directly: `ps -o pid,ppid,pgid,command` on the direct child showed that pid IS `sleep`, and the `assert_ne!` at `mod.rs:1653` reported `left: 22802 / right: 22802`.
**upstream** — Not an upstream-behaviour question: the fixture never constructed the scenario the test names, so the assertion could not have been evaluated either way.
**Impact** — The test is the ONLY coverage of the claim that the escalation ladder reaches a child's descendants, and it was testing nothing: it died in its own precondition before `terminate()` ran. The production mechanism it targets is in fact correct, and was verified independently — `Command::process_group(0)` (`mod.rs:515`) plus the leader-only negation in `send_signal` (`signal.rs:353-364`, `getpgid(pid)==pid` → target `-pgid`); with a fixture that really forks, one `kill(-pgid, SIGTERM)` killed both child and grandchild. A permanently-red test that also covers nothing is the worst of both.
**Fix** — LANDED. Appended a trailing `; :` so the outer shell is no longer running a single simple command and must stay resident to fork the inner one. Verified on this machine: outer `/bin/sh` 22900, descendant 22902, distinct. Deliberately NOT `… & wait`, which would make the descendant ASYNCHRONOUS and therefore `SIG_IGN` for SIGINT under POSIX — exactly the confound the test's own comment warns against.
**Verify** — `cargo test -p cyrup-ext-subagents --lib spawn::tests::terminate_reaches_the_childs_own_descendants_not_just_the_direct_child` green, and the `assert_ne!` still fires if the `; :` is removed.

---

## SUBA-068 — The worktree setup-hook timeout fixture raced macOS first-exec verification latency with a 200 ms budget

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** FIXED
**cyrup** — `spawn/worktree.rs:1632` failed inside the `wait_for_published_pid` PRECONDITION helper ("the hook never published its pid … within 5s"), not on the kill assertion; the `timed out` assertion above it passed, so the kill path had already run. Root cause is ambient: macOS pays a one-off verification cost on the first `exec` of a freshly written executable whose exact content it has not seen. Measured here with a standalone harness — unique script content: 242.38 / 197.67 / 206.91 / 207.63 / 203.26 / 206.77 ms to first write (6/6 runs); identical content reused: ~130 ms. `tempfile::tempdir()` randomizes the path, and the path is embedded in the script body, so the content is unique on EVERY run and the cost recurs every run. Against `timeout_ms: 200` (`worktree.rs:1576`) the SIGTERM always landed before `echo $$ > pid` could execute.
**upstream** — Production is already exact parity and was NOT touched: pi's `runWorktreeSetupHook` is `spawnSync(hook.hookPath, [], { …, timeout: hook.timeoutMs, shell: false })` (`pi-subagents v0.47.1 src/runs/shared/worktree.ts:328-334`), and Node's `spawnSync` timeout kills the direct pid with the default `SIGTERM`. The 1000 ms SIGTERM→SIGKILL grace in `signal.rs:190` is pi's own number from `acceptance.ts:1164-1177` (`child.kill("SIGTERM")` then `setTimeout(() => child.kill("SIGKILL"), 1000)`), read at the tag.
**Impact** — The test could not pass on this machine regardless of implementation correctness, because it asserted a property of process-startup latency that it does not control.
**Fix** — LANDED, in two parts, because raising the budget alone was NOT enough: at `timeout_ms: 3_000` the test still failed under the full suite's parallel load. The dominant term is removed instead of guessed around — the fixture now `exec`s the SAME hook file once with `$WARMUP` set (a new first-line short-circuit that exits before the pid is published), paying macOS's verification cost before the clock starts, and keeps a 3000 ms budget to absorb ordinary scheduling jitter. The assertion's meaning is unchanged: the hook still `exec sleep 300`, so it still blows its budget and the timeout arm still fires.
**Verify** — `cargo test -p cyrup-ext-subagents --lib` = **2218 passed / 0 failed**, run twice with all 24 cores saturated by spinners (load average 25-32); `spawn::worktree::tests::` alone 20/20, 3× under the same load.

---

## SUBA-069 — The whole worktree setup-hook test family is wall-clock-budgeted, and goes red under machine load

**Kind** test-defect · **Severity** high · **Effort** M · **Confidence** confirmed · **Status** OPEN
**cyrup** — Three further tests in the family — `runs_a_repo_relative_setup_hook_and_records_synthetic_paths` (`worktree.rs:1544`), `rejects_tracked_synthetic_paths_from_hook_output` (`:1691`), `excludes_hook_created_synthetic_files_from_captured_patch` (`:1717`) — all fail with `WorktreeSetup("worktree setup hook timed out after 5000ms")` when the machine is loaded, and all pass in isolation. These use the production DEFAULT 5000 ms hook timeout rather than a fixture constant, so unlike SUBA-068 they cannot simply be re-budgeted.
**Measured** — Controlled A/B on this box, varying ONLY `worktree.rs` and holding everything else constant. With two other agents running cargo concurrently (load ≈8-10): 4 failed / 2214 passed — the three above plus SUBA-068's — reproduced twice. At load ≈3-5: HEAD `worktree.rs` → 1 failed / 2217 passed (SUBA-068 only, the three siblings green); fixed `worktree.rs` → 2218 passed / 0 failed. So the three sibling failures are load-induced and are NOT caused by the SUBA-068 fix.
**Note on the load that matters** — Synthetic CPU pressure does not reproduce it: with all 24 cores saturated by spinners (load 25-32) the full `--lib` was 2218/2218 green, twice. The failures appear under CONCURRENT CARGO — compilation's I/O and memory pressure, which delays `fork`/`exec` far more than pure CPU contention does. Any reproduction attempt must use a competing build, not a busy-loop.
**upstream** — pi's default is the same 5000 ms (`worktree.ts` `DEFAULT_SETUP_HOOK_TIMEOUT_MS`, applied through `spawnSync(…, { timeout })` at `:328-334` @v0.47.1), so the CONSTANT is parity-correct and must not be raised in production to make tests pass. The defect is that the tests exercise it with real `/bin/sh` scripts under `cargo test`'s full parallelism (2218 tests), where a freshly-written script's first `exec` alone costs ~200 ms (SUBA-068) and scheduling latency adds the rest.
**Impact** — "`cargo test -p cyrup-ext-subagents` is green" is not a reliable gate today: the result depends on what else the machine is doing. An intermittent red is more corrosive than a consistent one, because it trains readers to re-run rather than investigate — and it is what let SUBA-067 and SUBA-068 sit unexamined behind a quoted, never-executed "3932 passed" figure.
**Fix** — Do NOT raise the production default. Options, in preference order: (a) inject the timeout in these three fixtures the way SUBA-068's does, so the budget is a test constant rather than the shipped default; (b) give the hook fixtures stable content at a stable path so macOS's first-exec verification is paid once per suite, not once per test; (c) serialize this module (a shared mutex, as `native_supervisor_channel_integration.rs` already does with `ENV_LOCK`) so hook tests do not compete with 2200 siblings. Re-measure under deliberate load after any of them.
**Verify** — `cargo test -p cyrup-ext-subagents --lib spawn::worktree::tests::` green while the box is held at load ≥8 (e.g. a concurrent `cargo build` of the workspace), repeated 3×.

---

## SUBA-070 — the `interactive` frontmatter key is parsed and typed but never enforced

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `interactive` is in the recognised key list (`crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:96`), parsed at `:869`, and stored in its own typed field at `:1024`. Nothing reads that field to change spawn behaviour. The deferral is deliberate and pinned by a test: `:1595-1602`, named `interactive_is_parsed_into_typed_field_and_never_dropped_from_extra_fields_expectation`, whose comment states *"func-SA §4.1: `interactive` is parsed but unenforced in v1 — it MUST still be typed"*.
**upstream** — `pi-subagents` honours `interactive` when deciding whether a child may prompt. Tag and line **not re-read this pass**; establish before fixing.
**Impact** — an agent file declaring `interactive: true` gets default behaviour with no warning. Because the key *is* recognised, it does not fall through to `extra_fields` either, so no other layer — including the permission system, which consumes `extra_fields` — can act on it. Same advertised-but-unenforced shape as `SUBA-061`, which names four different keys (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`).
**Fix** — consume the typed field where the child's prompt capability is decided; leave the parse and its test as they are.
**Verify** — an agent with `interactive: true` and one without produce different spawn behaviour on the same task, and the existing parse test still passes.
**Note** — filed because this area's `## Coverage` blind spot 6 predicts exactly this class (*"the unwired class is unsampled … a type-resolved pass over this crate specifically would very likely return more"*). The v1 rationale is recorded here so the item is not mistaken for an unnoticed defect.

---

## SUBA-071 — subagent settings are read from two files that can disagree — **CLOSED 2026-08-15 (REFUTED)**

> **REFUTED, CLOSED 2026-08-15 (sweep 11).** There is ONE settings store and its precedence is
> upstream's (project beats user, pi `agents/agents.ts:924-931` @v0.43.0). The
> `SettingsManager::effective().get("subagents")` read this item is built on **exists nowhere in the
> crate** — it was a stale doc comment, now deleted — and the item's cyrup citation
> (`registration/mod.rs:625-640`) points at `ProactiveSkillSubagents`. Everything below is the filing
> text; see the table row for the full refutation.

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — the `subagents` block is read from the layered `settings.json` via `effective().get("subagents")` (`crates/cyrup-ext-subagents/src/registration/mod.rs:625-640`) and, separately, from `~/.cyrup/agents/settings.json` and `<project>/.cyrup/agents/settings.json` on the discovery path (`crates/cyrup-ext-subagents/src/extension.rs:1580-1596`, `discovery/mod.rs:359-368`). `crates/cyrup-ext-subagents/src/registration/profiles.rs:227-232` documents the divergence **in-source**. The two are never merged and no precedence rule exists between them. Failure modes also differ: a malformed discovery-side file **aborts** discovery (`extension.rs:1590-1594`), while the layered side degrades.
**upstream** — `pi-subagents` reads one location. Tag and line **not re-read this pass**.
**Impact** — a user who sets `subagents.defaultModel` in `~/.cyrup/agent/settings.json` — the file every other part of cyrup calls *the* settings file — does not affect discovery-side behaviour, and vice versa. Which half of the crate honours a given key is not discoverable without reading the source. The two paths do not even share a root: the layered file lives under `~/.cyrup/agent` while the discovery one lives under `~/.cyrup` (`extension.rs:5138-5143`), so they are not siblings and a user correcting one will not find the other.
**Fix** — pick one location; read the other for backward compatibility behind a deprecation diagnostic that names both paths and states which won. Do not merge silently — a silent merge makes the precedence question permanently unanswerable from behaviour.
**Verify** — set the same key to different values in both files, assert the documented winner, and assert the diagnostic is emitted naming both paths.
**Note** — `registration/profiles.rs` is named in this area's `## Coverage` blind spot 5 as one of the cyrup-side files that pass never opened, which is why an in-source divergence note went unfiled.

---

## Coverage

### Read first-hand at cyrup HEAD `04c1ba2` (tree clean; docs-only `a9000b1` on top)

In full or in the cited regions: `extension.rs` (schema builder `6543-6690` — **all 45 `props.insert` names enumerated, twice, independently**; action enum `6557` — **all 27 verbs counted**; `route_management`/`route_control` `7560-7712` and `7780-7860`; dispatch tests `12580-12960`, `14120-14200`, `14640-14690`; init `9200-9420`; spawn budget `760-800`, `8317`, `10042`, `10454`, `10625`), `exec/{mod,structured,acceptance,fallback,control,model_scope,tool_budget,mcp_direct_tools}.rs`, `prompt_runtime.rs` (module doc, `660-740`, `1590-1730`), `spawn/{mod,signal,parallel,chain_graph}.rs`, `background/{wait,watch,control,cascade,runner_main}.rs`, `discovery/{frontmatter,skills,management,settings_write}.rs`, `registration/{mod,doctor,slash_commands}.rs`, `artifacts.rs`, `native_supervisor.rs`, `jsonl.rs`, `tui/notices.rs`, and all six files under `crates/cyrup-ext-subagents/resources/agents/`. Outside the crate, for the receiving and producing ends: `crates/cyrup/src/main.rs:489,638,732`, `crates/cyrup-provider/src/api/{openai_responses,anthropic_messages,google_generative_ai,openai_completions}.rs`, and `ls crates/cyrup-tools/src/tools/`.

### Read first-hand upstream, at tags only

`git show v0.43.0:<path>` and `git show v0.47.1:<path>`, never clone HEAD: `extension/{schemas,config,tool-description,subagent-guide,index}.ts`, `shared/{types,settings,artifacts,utf8,prompt-resources}.ts`, `runs/shared/{pi-args,acceptance,model-scope,model-fallback,tool-availability,tool-budget,turn-budget,subagent-prompt-runtime,spawn-budget}.ts`, `runs/foreground/{subagent-executor,execution,async-dismiss-action}.ts`, `runs/background/{async-execution,completion-replay,wait-completions,resume-guidance,active-run-index,scheduled-runs}.ts`, `agents/{frontmatter,skills}.ts`, `policy/authority.ts`, `api/delegation.ts`, `slash/slash-commands.ts`, and all six `agents/*.md`. `not-ported` vs `upstream-drift` was decided by `git cat-file -e v0.43.0:<path>` in every case where it mattered — which is how SUBA-017, SUBA-021 and SUBA-022 were re-classified.

### Version-lag sweep (new territory)

The full `v0.43.0..v0.47.1` range was swept — the workspace brief measures it at 151 files / +10254 / −1333; the src-only sweep covered 96 non-merge commits, 67 files, +4696/−769 and **12 net-new source files**, none of which any prior pass had seen (`PARITY-GAPS.md` records v0.43.0 as "latest"). All 12 new files were read: `subagent-guide.ts`, `active-run-index.ts`, `resume-guidance.ts`, `utf8.ts`, `prompt-resources.ts`, `completion-replay.ts`, `wait-completions.ts`, `async-dismiss-action.ts`, `foreground-history.ts`, `workflow-foreground-steering.ts`, `display-text.ts`, `api/project-panes.ts`. Commits diffed line by line: `94b0cb1`, `635c1bd`, `a4fc59a`, `87420e5`, `b69aafb`, `ade35ea`, `0b1976b`, `0677ac0`, `c2add95`, `28b9222`, `38bc408`, `889a798`, `b283d27`, `bc1b689`.

### Surface-driven sweeps run (three, diffed as sets, not spot-checked)

1. **Child env vars** — 42 `PI_SUBAGENT*` names @v0.43.0 vs 48 `CYRUP_SUBAGENT*` names at HEAD. Six upstream names have no cyrup counterpart; five became findings (`SUBA-045` TOOL_DIAGNOSTIC_PATH, `SUBA-049` STEER_ACK_DIR/STEER_CAPABILITY, `SUBA-063` ZERO_AUTH/RUNTIME_ACKNOWLEDGED); `CAPABILITY_CEILING_V1` is PARITY-GAPS VL-S1.
2. **Tool schema** — upstream's 66 top-level `SubagentParamsSchema` keys @v0.43.0 vs cyrup's 45. Of the ~25 with no counterpart, `outputSchema` (`SUBA-043`) and `toolBudget` (`SUBA-047`) are new; `mode`/`steeringRecovery` fold into `SUBA-049`, `additional` into `SUBA-046`; `workflowScript`/`chatProgress`/`resume`/`handoffPath`/`usageBudget`/`turnBudget`/`agentContract`/`gate`/`schedule.*` are already VL-S2/VL-S8/VL-S9/VL-S10/PB-10/PB-11.
3. **Action enum + extension config** — 27 verbs vs 50 (v0.43.0) / 53 (v0.47.1); 18 config fields vs 30 (v0.43.0) / 33 (v0.47.1). Yielded `SUBA-046`, `SUBA-048`, `SUBA-055`, `SUBA-057`, `SUBA-059`, `SUBA-061` and the `SUBA-005` restatement.
4. **Denial/gating paths** (refuter-only, a fourth lens the audit did not run) — swept every refusal site rather than every advertise site. Yielded `SUBA-064` (`authorityPolicy`), `SUBA-065` (unknown-action recovery), `SUBA-066` (`/subagents-guide`), and the third residual inside `SUBA-038`.

### Severity re-derivation (repair pass, 2026-08-12)

The completeness critique's finding 3 (`critical` = data loss, silent wrong output, a permission
bypass, or a crash on a normal path — `README.md:106-107`) was applied to this file's own items
rather than only to the ones it named elsewhere. Two candidates were examined; **both stand where
they are**, and the reasoning is recorded so the next pass does not re-litigate:

- **`SUBA-064` (`authorityPolicy`) stays `medium`, not raised.** It has the shape of a permission
  bypass — an operator writes `"authorityPolicy": {"stopRun": "forbid"}`, the key is silently
  dropped by the only config validator (`registration/mod.rs:256` validates missions and nothing
  else) and the action runs. What holds it at medium is *which* actions are reachable: of upstream's
  six `AUTHORITY_ACTIONS`, only `stopRun` and `steerRun` are implemented in cyrup at all
  (`extension.rs:7805`, `:7825`); `discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant` and
  `scheduleCreate` have no dispatch to bypass. Stopping or steering a subagent run against policy is
  a control-surface divergence, not a destructive one. **If `worktree.discard` /
  `destructiveCleanup` ever land (they are two of `SUBA-005`'s unowned verbs), this item becomes
  `critical` on the day they do, and it must be raised in the same change** — noted in `SUBA-064`'s
  Fix as a hard prerequisite rather than left to be rediscovered.
- **`SUBA-043` (SINGLE-mode `outputSchema`) stays `high`, not raised to `critical`.** The dropped
  parameter is silent, but the *output* is not silently wrong: the run returns free prose where typed
  JSON was requested, which the caller's own parse rejects. It is an unreachable capability, not a
  corrupted result. Kept at the top of the table with `SUBA-014`.
- `SUBA-014` was re-read and stays `high` for the reason the audit gave: the child is *instructed* to
  use a tool it may not have, so the failure surfaces as a model apology rather than a config error —
  bad, but loud at the point of use.

### Rejected with reason — do not re-derive

- **Inherited tool-id sanitation (proposed `SUBA-042`, claimed high).** The claim was that cyrup's `strip_parent_only_subagent_messages` (`prompt_runtime.rs:670-700`) has no counterpart to pi's `portableToolId`/`sanitizeToolHistoryMessage` (`subagent-prompt-runtime.ts:208-266`, selected at `:557`), so a `context:"fork"` child on a non-composite API inherits `call_id|item_id` ids and its first request 400s. **Refuted on mechanism:** cyrup normalizes composite ids at the **provider** boundary, which is where the 400 would occur — `crates/cyrup-provider/src/api/anthropic_messages.rs:809` `normalize_tool_call_id` maps every non-`[A-Za-z0-9_-]` char (including `|`) to `_` and truncates to 64, with a unit test at `:2437`; `google_generative_ai.rs:586` does the same; both feed `transform_messages_with_source` (`openai_completions.rs:892-935`), which rewrites the assistant toolCall id **and** the paired `tool_call_id` consistently via `tool_call_id_map`, so pairing survives. `mistral_conversations.rs:313` and `bedrock_converse_stream.rs:1399` also pass normalizers. Pi has the identical provider-level guard (cyrup's own comment cites `anthropic-messages.ts:1006-1009`), so pi's subagent-level `portableToolId` is belt-and-braces, not the load-bearing defence. What remains is that inherited ids are *persisted* in non-portable form in the child's own history rather than rewritten at the context event — cosmetic, and not worth a work batch as written. **If someone re-derives this, the burden is to exhibit a provider path with no normalizer.**
- The code fact behind it is still true and unfiled: cyrup has no per-API composite gate, so it cannot *preserve* composite ids where they are required. No known consumer needs that today.

### Handoffs to other areas

- **`SUBA-062`'s root cause** — the absent `web_search` / `fetch_content` / `get_search_content` tools — belongs to area 04 (built-in tools) / area 12 (pi drift), not here. Only the `[CYRUP-DELTA]` header is owed in this crate.
- **PARITY-GAPS ids not restated as findings**, confirmed still accurate at HEAD by spot-check: PB-8 (RPC bridge), PB-9 (clarify UI), PB-10 (turnBudget, = SUBA-008), PB-11 (schedule, = SUBA-016), PB-12..PB-14, UW-3..UW-8, VL-S1..VL-S15. The `subagent_wait` rename, wait subscriptions and auto-drain are VL-S8 and were not re-derived.
- **PARITY-GAPS is stale in its header** — it records pi-subagents latest as v0.43.0 and pi-intercom latest as v0.9.2. Both are wrong (v0.47.1 / v0.10.1). Not fixed here; flagged for whoever owns that file.

### Blind spots — read this before the next pass

1. **Static only.** Nothing was executed — no cargo, no npm, no spawned process. Every `Verify` line is a design, not an observation.
2. **The biggest unaudited mass is `workflowScript`, and everything above sits *around* it, not inside it.** Upstream deleted the entire `task`/`tasks`/`chain`/`concurrency`/`chainDir` execution surface at v0.41.0 and replaced it with `workflowScript`; at v0.43.0 the top-level schema has **no `task` key at all** and the whole model-facing tool description is workflowScript-centric. cyrup implements the v0.34.0-era surface. PARITY-GAPS files this as one `large` item (VL-S2), but it is not one item — it is a different execution model whose per-behaviour consequences (mixed sequential/parallel phases, `runs.ref`, `emit`, per-child gates, `prompts.render`, `chatProgress`, retained-child `resume`, `children.list`) were **not** decomposed by this pass or any prior one. **Treat this area's open count as a floor by a wide margin.**
3. **Not read at all on the upstream side**, so anything in them is invisible here: `src/workflows/scripted-workflow.ts` (502 lines, +231 in range), `src/inspectors/herdr/project-panes.ts` (+524), `src/runs/background/async-job-tracker.ts` (+426 — the v0.47.0 event-driven rewrite), `src/runs/foreground/foreground-history.ts` and `workflow-foreground-steering.ts` (new, 137 + 187), `src/shared/display-text.ts` (new, 100), `src/tui/{render,fleet}.ts` (+343 combined), `src/missions/workflow-state.ts` (+209), `src/extension/rpc.ts`. Related and specifically unfiled: the **steering-recovery hardening across v0.44–v0.47** in `subagent-prompt-runtime.ts` (safety poll, settle fallback, `awaitingSettlement`) was read but **not** compared line by line against cyrup's `prompt_runtime.rs` `SteeringInbox`, which targets the v0.43.0 shape. Drift there is likely; diff it when SUBA-049 is scheduled.
4. **`run-fanout-budget.ts`** (257 lines — a whole new per-run logical fan-out cap with config, doctor check and status surface) landed on pi-subagents `main` at `17b4078`/`668c587` **after v0.47.1**. Deliberately not filed, because the hard rules require citing a named tag. Pick it up on the next tag.
5. **cyrup-side files not opened this pass:** `spawn/{chain_graph beyond the instruction builder, dynamic_fanout, nested_events, worktree}.rs`, all of `missions/`, all of `watchdog/` beyond confirming the subtree exists and is registered, `tui/{fleet,fleet_transcript,fleet_status,render}.rs`, `registration/{cost,profiles,resources}.rs`, `exec/{output,ndjson,task_intent,agent_refinements}.rs`. **SUBA-024's two named blind spots (`chain_graph.rs` pre-walk validation, `ChainStepConfig` unknown-key handling) are still not re-read — third pass running.**
6. **The unwired class is unsampled.** PARITY-GAPS §7 states its unwired sweep was identifier-based and incomplete by construction, with ~120 flagged items untriaged. This pass found the class again *by accident* twice — `SUBA-047` (`toolBudget` fully implemented, unadvertised) and `SUBA-054` (`defaultReads` parsed, rendered, never used) — without running a systematic hunt. A type-resolved pass over this crate specifically would very likely return more, since it is the largest and where batches 8–10 landed the most code. **The single highest-leverage test to write in this area is the schema/dispatch guard asserting every advertised property has a consumer** (named in SUBA-043's Verify); it would have caught SUBA-N05, SUBA-043 and SUBA-047 as a class.
7. **The v0.43.0 baseline is inherited, not re-derived** — the crate still records no version string. Several items are classified `not-ported` vs `upstream-drift` on that assumption. Where it mattered, first-tag presence was re-checked directly with `git cat-file -e <tag>:<path>`, which is how SUBA-021's "post-baseline, out of scope" framing was found to be dead and SUBA-017/SUBA-022 were re-classified.
8. **`spec/` and ADR-0001 are absent from this workspace**, as documented. `R-SA-*` ids in cyrup's comments were used only as grep anchors; no finding rests on one, and where a comment invokes one to justify a divergence it was treated as an unverifiable claim — relevant to **SUBA-030**, whose `[CYRUP-DELTA]` justifies inline-argv delivery while the code's own doc at `spawn/mod.rs:428` asserts a 0600 mode the code never sets.
9. **Closure quality.** Twenty-two items closed this pass and every closure was re-derived from code on both sides, not from a commit message. The audit's own citations were wrong in three places, caught by re-reading: SUBA-002's test range (`:13699/:13716/:13753`, not `:13338-13470`), SUBA-033's second instance (`:1133`, not `:802-810`), and SUBA-008's consumer count (three files, not two). Assume a similar residue in the citations above and treat each as a lead to verify.
