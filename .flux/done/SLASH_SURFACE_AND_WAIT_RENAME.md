---
stage: done
status: completed
updated: 2026-09-21
---

# The slash surface and the wait-tool name — VL-S11, VL-S12, VL-S8

OBJECTIVE: close the three remaining user-surface ledger rows. Two are genuinely small; one
(`/subagents-detach`) needs a capability cyrup does not have, and the augment must size it
honestly rather than let the exec discover it late.

Ledger: `docs/gap-analysis/PARITY-GAPS.md` VL-S11 (`:1537`), VL-S12 (`:1541`), VL-S8 (`:1521`).

## 0. Two ledger citations are already stale — verified, correct them as part of this work

- **VL-S11 says THREE commands are missing.** `/subagents-refine` landed in #145 and is
  `registration/slash_commands.rs:127` (label `:154`, descriptor `:294`). **Two remain**:
  `/subagents` and `/subagents-detach`. Fix the row's own count.
- **VL-S8 says the upstream name is `subagent_wait`.** At the pinned tag it is **`bg_wait`**
  (`src/runs/background/wait-tool.ts:38` → `name: "bg_wait"`). The row half-knows this ("or, at
  `7fe9dee1`, `bg_wait`") — settle it on the pinned tag and stop hedging.

## 1. VL-S12 — delete four commands upstream removed at v0.41.0 · *trivial*

`Chain` (`slash_commands.rs:83`), `Parallel` (`:84`), `RunChain` (`:85`), `ChainPrompts` (`:101`).
The row's own blocker is discharged: it said "do not delete before VL-S2 lands", and the
`workflowScript` runtime landed (30 files under `workflows/scripted/`). Upstream's registrations
at v0.68.0 are the 17 at `slash-commands.ts:869-1283` — none of the four is among them.

Delete the variants, their labels, their descriptors, their dispatch arms, and every test that
pins them. **Check what the deletion orphans**: `PromptWorkflow` shares
`prompt-workflows.ts` provenance with `ChainPrompts`, and `/prompt-workflow` is NOT deleted
upstream — do not take it with them.

## 2. VL-S8 — rename the wait tool to `bg_wait` · *small*

`extension/wait_tool.rs:16` `WAIT_TOOL_NAME: &str = "wait"` → `"bg_wait"`. Three files reference
it (`wait_tool.rs`, `background/wait.rs:971`, `extension/mod.rs:102`), but the blast radius is
larger than the constant: grep for the literal `"wait"` in tool-name position, in every tool
DESCRIPTION and guide document that names it, in `resources/docs/`, and in the
`children.list`/`status` output text. A child prompted by upstream's own description calls
`bg_wait` and gets "unknown tool" today — that is the whole observable.

The augment must decide and record whether the old name stays registered as an alias. Upstream has
no alias. Prefer upstream (no alias) unless a cyrup consumer is pinned to `wait` — say which.

## 3. VL-S11a — `/subagents`, the admin surface · *medium*

Upstream `src/slash/subagents-admin.ts` (460 lines), registered at `slash-commands.ts:869-875`:
*"Administer subagents: inspect metadata and update models, thinking, or prompts"*, handler
`openSubagentsAdmin(pi, ctx, args)`.

The shape (re-verify every line): `sourceRank:25` · `allVisibleAgents:32` · `agentLabel:41` ·
`agentChoices:46` · `agentSelectItems:56` · `agentMatches:60` · `sendAdminMessage:65` ·
`modelFullId:73` · `liveAvailableModels:77` · `buildBuiltinBase:94` · `savesThroughSettings:125` ·
`isReadOnlyExtraAgent:139` · `readOnlyAgentMessage:150` · `selectAgent:162` · `metadataFor:185` ·
`selectFromList:216` · `chooseModel:230` · `chooseThinking:246` · `chooseOverrideScope:265` ·
`persistSettingsField:275`.

cyrup seams the augment must map: the agent registry and `AgentConfig` equivalent; the TUI
selector (the `/subagents-stop` command already opens one — `slash_commands.rs` `SubagentsStop`,
whose no-id branch is a selector over discovered targets, so the precedent is in-tree); the
settings writer and its user/project scope split; and whether cyrup has the
builtin-override/read-only-extra-agent distinction `savesThroughSettings` and
`isReadOnlyExtraAgent` encode. **If cyrup's agent config cannot express an override scope, say so
with the grep — do not simulate one.**

## 4. VL-S11b — `/subagents-detach` · *the one that is NOT small; size it first*

Upstream's handler is thin (`slash-commands.ts:978-1001`) but it stands on a capability:

```
control = selectForegroundDetachControl(state, id)   // :980
if (control.mode !== "single") → "/subagents-detach currently supports single-subagent runs only."
if (!control.detach?.())       → `Foreground run ${runId} is not currently detachable.`
```

then a verbatim success message naming `subagent({ action: "status", id })` and
`bg_wait({ id })` — **which is the other half of §2: write that sentence with the NEW tool name.**

**cyrup does not have `detach` on its foreground registry.** `ForegroundControlEntry`
(`extension/executor/notices.rs:20`) carries `interrupt: CancelToken`, `current_agent`,
`current_index` and friends — no detach callback. `grep -rn 'fn detach' src` finds only
`workflow_detach/` (a DETACHED WORKFLOW's child settlement, a different thing) and
`parent_anchor.rs`'s `detached_runner_env_overlay`. So this verb needs the real
foreground→background handover: the live child keeps running, its run becomes addressable by id,
and `status`/`bg_wait` can recover the eventual result.

**The augment's job here is to SETTLE the size**, into `blockers` and `rustShape`:
- What does `selectForegroundDetachControl` read, and what is cyrup's equivalent registry?
- What must exist on disk after a detach for `status`/`bg_wait` to find the run — a `status.json`
  under the async root, an active-run-index entry, both?
- Does the foreground run already have a run id and a run dir, or are they minted at detach?
- Upstream's own message disclaims durability (*"does not daemonize the process or guarantee
  survival across Pi reload/restart"*) — port that honesty, do not over-promise.

If it turns out larger than the other three combined, say so in `blockers` **and implement it
anyway** — the standing directive is the whole feature, not the narrow part. Only a genuine
blocker (a dependency that does not exist) is a reason to stop, and then it is recorded as a
residual with a TRUE premise.

## Definition of done

1. Four commands deleted; nothing orphaned; the palette matches upstream's 17.
2. `bg_wait` is the tool name everywhere it is advertised, described, documented, and quoted in
   output text; no stale `wait` spelling survives a crate-wide grep.
3. `/subagents` opens the admin surface and actually persists a model/thinking/prompt override,
   with the read-only and scope refusals ported verbatim.
4. `/subagents-detach` detaches a live foreground single run, which is then addressable by
   `status` and `bg_wait`; all four of upstream's message strings byte-identical.
5. Each command REACHABLE through a production dispatch, each with a test that fails if gutted.
6. Ledger: VL-S12 and VL-S8 closed; VL-S11 closed or narrowed to exactly what is left, with its
   stale three-command count corrected.
7. Gates: fmt; clippy `--workspace --all-targets --features test-fixtures -- -D warnings`;
   `nextest run --workspace --features test-fixtures` (baseline **10683** / 9 skipped);
   `nextest run -p cyrup-it --features it` (baseline **567**).

---

## [AUG — slash-surface]

Read-only research pass. Every upstream citation below is `git -C tmp/pi-subagents show v0.68.0:<path>`.
Every cyrup citation is HEAD of this tree. Nothing was edited but this file.

### A. Citation audit of the seed (`anchorsWrong`)

| seed claim | verdict |
|---|---|
| VL-S8 row at `PARITY-GAPS.md:1521` | **WRONG** — the row header is **`:1501`** (`**VL-S8 · Wait tool is still `wait`**`). `:1502` is its upstream/cyrup evidence line. |
| VL-S11 `:1537`, VL-S12 `:1541` | correct. |
| upstream wait-tool name is `bg_wait` at `wait-tool.ts:38` | **correct** — `name: "bg_wait"` (`src/runs/background/wait-tool.ts:38`). The description also spells it at `:18`, `:24`, `:27` (twice). No alias is registered: `registerWaitTool` calls `pi.registerTool(primaryTool)` once (`:44`). |
| `Chain :83 / Parallel :84 / RunChain :85 / ChainPrompts :101` | **all correct** in `registration/slash_commands.rs`. |
| `/subagents-refine` at `registration/slash_commands.rs:127`, label `:154`, descriptor `:294` | **correct** (`:127` variant, `:154` `as_str` arm, `:294` descriptor). |
| VL-S11 "three missing" | **stale, as the seed says** — `/subagents-refine` landed. **Two remain.** Also stale inside the row: its own re-read note says "**17** variants at `:83-121`" — the enum is now **18** variants spanning **`:82-127`**. |
| "Upstream's registrations at v0.68.0 are the 17 at `slash-commands.ts:869-1283`" | **the 17 count is right** (869, 876, 914, 921, 928, 945, 960, 973, 1002, 1014, 1057, 1128, 1146, 1158, 1211, 1245, 1283) **but it is not the palette.** `registerSlashCommands` also calls `registerPromptWorkflowCommands` at `:1121`, which registers `prompt-workflow` (`src/slash/prompt-workflows.ts:254`). The extension's full command surface at v0.68.0 is **18** from these two files, plus `subagents-watchdog` (`src/watchdog/register-main.ts:405`) and `pi-subagents-bridge` (`src/extension/herdr-pi-bridge.ts:109`). **DoD item 1's "the palette matches upstream's 17" is therefore unachievable-as-written and must be restated** (see §F). |
| "the `/subagents-stop` command already opens [a TUI selector] … the precedent is in-tree" | **FALSE, and this is the most load-bearing correction in this section.** cyrup's `SubagentsStop` no-id branch is `HostSlash::slash_subagents_stop` (`extension/host/slash.rs:589-602`) → `executor.format_stop_targets(cwd)`, which returns **TEXT**. That is upstream's `!ctx.hasUI` fallback (`slash-commands.ts:1035-1038`), not its selector (`:1044-1047`). **There is no in-tree precedent for a slash command opening an interactive selector.** The capability exists one layer down (§D.2) but no slash handler uses it today. |
| "`ForegroundControlEntry` (`extension/executor/notices.rs:20`) has `interrupt: CancelToken` and no detach callback" | **correct** — `notices.rs:20-98`; `interrupt: CancelToken` at `:23`; `mode: RunMode` at `:37`; `updated_at: i64` at `:61`; `session_id: Option<SessionId>` at `:68`; `active_children: BTreeMap<usize, ForegroundChildEntry>` at `:95`. No `detach`. |
| "`grep -rn 'fn detach' src` finds only `workflow_detach/` and `parent_anchor.rs`" | correct **for `fn detach`**, and misleading as a conclusion. cyrup has a whole **detach lifecycle** already: `AttemptSignal::detached` (`exec/fallback.rs:1527`), `LadderStop::Detached` (`:1925`), `SingleResult::detached` (`exec/run_result.rs:62`), `LiveProgressStatus::Detached` (`tui/events.rs:115`), `SubagentResultStatus::Detached` (`tui/intercom.rs:135`), the `"detached"` foreground-history status (`extension/executor/foreground_history/record.rs:170`) and the never-persist rule (`persist.rs:249-253`), and `ForegroundChildState::Detached` in the wait-subscription probe (`background/wait_subscriptions/manager.rs:212-215`). What is missing is **one producer**, not a lifecycle. See §C. |
| "`docs/gap-analysis/09-cyrup-ext-subagents.md:436`'s SUBA-018 cites `/prompt-workflow` and `/chain-prompts` at `registration/slash_commands.rs:141-142`" (not in the seed, found in passing) | stale — they are `:98`/`:101` (variants), `:149`/`:150` (`as_str`), `:256`/`:261` (descriptors). Deleting `ChainPrompts` makes half that row's citation dangle. |

### B. Upstream, quoted

**`src/runs/background/wait-tool.ts:37-44`**
```ts
	const primaryTool: ToolDefinition<typeof SubagentWaitParams, Details> = {
		name: "bg_wait",
		label: "Background Wait",
		description,
		parameters: SubagentWaitParams,
		execute,
	};
	pi.registerTool(primaryTool);
```

**`src/runs/shared/permissions.ts:8`** — the one place the name is load-bearing beyond advertising:
```ts
const INTERNAL_TOOLS = new Set(["contact_supervisor", "intercom", "bg_wait", "structured_output"]);
```
`:26` throws `${label}.${tool} is reserved for child coordination and cannot be gated.`; `:49` `if (toolName === "bash" || INTERNAL_TOOLS.has(toolName)) return "allow";`.

**`src/slash/slash-commands.ts:237-250`** — `selectForegroundDetachControl`:
```ts
function selectForegroundDetachControl(state: SubagentState, requested: string) {
	const controls = [...state.foregroundControls.values()];
	if (requested) {
		const matches = controls.filter((control) => control.runId === requested || control.runId.startsWith(requested));
		if (matches.length > 1) throw new Error(`Ambiguous foreground run id prefix '${requested}' matched: ${matches.map((control) => control.runId).join(", ")}. Provide a longer id.`);
		return matches[0];
	}
	const singleControls = controls.filter((control) => control.mode === "single");
	if (state.lastForegroundControlId) {
		const latest = state.foregroundControls.get(state.lastForegroundControlId);
		if (latest?.mode === "single") return latest;
	}
	return singleControls.sort((left, right) => right.updatedAt - left.updatedAt)[0];
}
```

**`:978-1005`** — the handler and its five strings:
```ts
	const detachForegroundRun = (args: string, ctx: ExtensionContext): void => {
		const id = args.trim();
		let control: ReturnType<typeof selectForegroundDetachControl>;
		try { control = selectForegroundDetachControl(state, id); }
		catch (error) { ctx.ui.notify(error instanceof Error ? error.message : String(error), "error"); return; }
		if (!control) {
			ctx.ui.notify(id ? `No active foreground run found for '${id}'.` : "No active foreground single-subagent run to detach.", "info");
			return;
		}
		if (control.mode !== "single") { ctx.ui.notify("/subagents-detach currently supports single-subagent runs only.", "error"); return; }
		if (!control.detach?.()) { ctx.ui.notify(`Foreground run ${control.runId} is not currently detachable.`, "info"); return; }
		sendSlashText(pi, `Detached foreground run ${control.runId} without terminating its child. Use subagent({ action: "status", id: ${JSON.stringify(control.runId)} }) or bg_wait({ id: ${JSON.stringify(control.runId)} }) to recover the eventual result. This does not daemonize the process or guarantee survival across Pi reload/restart.`);
	};
	pi.registerCommand("subagents-detach", {
		description: "Detach the active foreground single-subagent run without terminating it",
		handler: async (args, ctx) => detachForegroundRun(args, ctx),
	});
```
`:1007-1012` also binds `options.foregroundDetachShortcut` to the same closure with `""`. cyrup has no
extension-configurable keybinding slot on this surface; **do not invent one** — record it as a residual.

**`src/runs/foreground/foreground-control.ts:16,59,83,119-127`** — `detach` is a per-CHILD callback lifted
to the run control by `syncCurrentChild`/`clearCurrentChild`, wrapping `input.detach` so a successful
detach clears the child's activity state and re-syncs the control.

**`src/runs/foreground/execution.ts:612-650`** — `detachForeground(reason)` is what that callback is built
over. It does NOT kill or reparent anything: it snapshots progress as `status: "detached"`, mints a
`SingleResult` receipt with `exitCode = -2`, `detached = true`, `detachedReason = reason`,
`finalOutput = "Detached at user request before task completion."`, an `outputSaveError` when an output
path was configured, and a `progressSummary`; publishes it through `options.onDetachReceipt` — and only
if the coordinator ACCEPTS does it set `detached = true` / `session.detached = true`. The in-process child
session keeps running; `onDetachedExit` (`subagent-executor.ts:3951-3976`, fired at `execution.ts:2364`)
settles it later.

**`src/runs/foreground/foreground-history.ts:10-22,137-148`** — the remembered runs are persisted to
`<resultsDir>/foreground-history.json`, `version: 1`, bounded at `MAX_REMEMBERED_FOREGROUND_RUNS = 50`,
and `compactRun` (`:89-100`) refuses any run with a non-restorable child status — `detached` is not in
`isRestorableForegroundStatus` (`:67-69`). **So a detached run is memory-only; nothing about a detach
reaches disk.**

**`src/agents/agents.ts:1410-1424`** — `applyBuiltinOverride` builds `override.{scope,path,base,fields,fieldScopes}`;
`fields` is the union of every override key ever applied and `fieldScopes[field]` accumulates the scopes.
**`:1538-1559`** — `applyCustomAgentOverrides` applies **user THEN project** (its own comment: *"Apply user
then project so project fields win without dropping user-only fields"*), which is why `fieldScopes` can
hold two scopes for one field. **`:1492-1524`** — `applyBuiltinOverrides` is winner-take-all
(project override ▷ project bulk-disable ▷ user override ▷ user bulk-disable).

**`src/slash/subagents-admin.ts`** — every symbol the seed lists **re-verified at the exact cited line**:
`sourceRank:25` `allVisibleAgents:32` `agentLabel:41` `agentChoices:46` `agentSelectItems:56`
`agentMatches:60` `sendAdminMessage:65` `modelFullId:73` `liveAvailableModels:77` `buildBuiltinBase:94`
`savesThroughSettings:125` `isReadOnlyExtraAgent:139` `readOnlyAgentMessage:150` `selectAgent:162`
`metadataFor:185` `selectFromList:216` `chooseModel:230` `chooseThinking:246` `chooseOverrideScope:265`
`persistSettingsField:275`. The seed's list stops there; the file runs to **460** and the rest is the part
that actually writes: `saveAgentModel:307` `saveAgentThinking:330` `metadataSummary:354`
`saveAgentSystemPrompt:362` `editSystemPrompt:381` `openSubagentsAdmin:396`.

### C. **THE ONE THING: the true size of `/subagents-detach`**

#### C.1 What `selectForegroundDetachControl` reads, and cyrup's equivalent

Upstream reads `state.foregroundControls` — the LIVE-run map. cyrup's equivalent is
`SubagentExecutor::foreground_controls: Arc<Mutex<HashMap<String, ForegroundControlEntry>>>`
(`extension/executor/mod.rs:165`), written by `register_foreground_controls`
(`extension/executor/foreground.rs:977-1065`) and removed by `settle_foreground_run` (`:1086-1120`).
The entry already carries **every field the selector needs**: `mode` (`notices.rs:37`) and `updated_at`
(`:61`).

Two gaps in the selector itself, both small:
1. `resolve_live_foreground_run` (`extension/executor/nested_control.rs:211-225`) returns `None` for BOTH
   "no match" and "ambiguous prefix". Upstream distinguishes them — ambiguity is an `error` notify with a
   distinct sentence, absence is an `info` notify. **A new resolver is needed**, not a reuse.
2. `state.lastForegroundControlId` has no cyrup port and is documented as
   **`[CYRUP-DELTA, unrepresentable]`** at `extension/executor/status.rs:749-752` (again at `:1091`).
   That delta stays TRUE: upstream's fallback when it is absent is exactly `singleControls.sort(updatedAt
   desc)[0]`, which `updated_at` gives verbatim. Cite the existing delta; do not write a new one.

#### C.2 Does a foreground single run already have a run id and a run dir?

**Run id: YES.** `RunId::new()` is minted in `resolve_run_channels` (`foreground.rs:650`), threaded onto
`RunOptions::run_id`, used as the artifact-quadruple id and the `foreground_controls` key, and returned
from `run_foreground_impl` as `Ok((result, run_id))` (`:412`).

**Run dir: NO.** A foreground run resolves an **artifacts** dir (`foreground.rs:684-700`), an
**output base** dir (`:700`) and a **session** dir (`:732`) — never a `RunPaths`. There is no
`status.json`, no `ResultFile`, no async-root entry. `extension/executor/workflow_detach/mod.rs:31-33`
states this in its own words: *"a FOREGROUND child writes no `ResultFile`, so nothing on disk names it"*.

#### C.3 What must exist for `status` and `bg_wait` to find a detached run

**Neither a `status.json` under the async root nor an active-run-index entry.** Upstream's own answer is
IN-MEMORY: `state.foregroundRuns`, and `foreground-history.ts:67-69,89-100` proves a `detached` run is
deliberately **never written to disk**. cyrup already ported that rule verbatim —
`foreground_history/record.rs:28-29` (*"`detached` is deliberately absent … remembered in memory and never
persisted"*) and `persist.rs:249-253`'s own test
`persist_never_writes_a_detached_run_though_it_stays_remembered_in_memory`.

So the required post-detach state is exactly: **an entry in
`SubagentExecutor::foreground_runs` (`extension/executor/mod.rs:207-225`) whose child `status == "detached"`,
while its OS child is still alive and still being driven.** Everything downstream of that already exists:

* `foreground_history_child_status` already mints `"detached"` from `result.detached`
  (`record.rs:170-171`);
* `is_trimmable` already protects a detached run from eviction (`record.rs:181-187`);
* `ExecutorForegroundProbe::probe` (`extension/executor/wait_subscriptions.rs:78-142`) — a **production**
  impl, not test scaffolding — already consults `foreground_controls` then `foreground_runs` and maps
  `"detached" → ForegroundChildState::Detached`;
* `WaitSubscriptionManager`'s whole foreground reconcile branch is ported
  (`background/wait_subscriptions/manager.rs:500-535`), including the `contact_supervisor` attention test.

Two **readers** are still missing and must be added:

* **`status` by id.** `SubagentExecutor::control_status` has a live-foreground branch
  (`extension/executor/status.rs:584-612`) that resolves through `foreground_controls` only. A DETACHED run
  is by construction NOT in `foreground_controls` (the two maps are disjoint — `mod.rs:207-211`), so the id
  falls through to `Async run not found. Provide id or dir.` Upstream's second branch is
  `deps.state?.foregroundRuns?.get(resolved.id)` (`run-status.ts:421`). **Port it.**
* **`bg_wait` by id / `all`.** `background/wait.rs`'s candidate set is `active_runs` → `list_active_runs`,
  async only. The `[CYRUP-DELTA]` at `wait.rs:955-960` says so in as many words and concludes *"this site
  can only ever arm `WaitTargetKind::Async`"*. Upstream's candidate set is async runs **plus**
  `activeDetachedForegroundRuns` (`subagent-wait.ts:224-243`), with its own progress renderer
  (`detachedForegroundWaitUpdate`, `:541-566`), attention message (`:242-249`) and session-changed refusal
  (`:568-572`). **Porting this DELETES that `[CYRUP-DELTA]`** and makes `WaitTargetKind::Foreground`
  reachable from production for the first time.

#### C.4 Is there an existing cyrup path that already does part of the handover?

**`background.rs`'s spawn path: NO, and it is the wrong shape.** `extension/executor/background.rs` mints a
`RunPaths`, writes a `status.json`, and spawns a **hop-2 detached runner process** that owns the child from
birth. A foreground child is already spawned, already parented to this process, and has no run dir — you
cannot retroactively route it through that path without re-spawning it, which is precisely what
"without terminating its child" forbids.

**`workflow_detach/mod.rs`: PARTLY, and it is the settlement half.** It is a full port of
`workflow-detach-reconcile.ts` (304 LOC) and it reconciles a paused workflow whose detached foreground
child has finally exited. But it is **workflow-scoped** (it needs `find_workflow_settlement_step`,
a `WorkflowKey`, a workflow `status.json`) and it is driven only from
`workflow_launch.rs:747-796`'s `reconcile_detached_workflow_children`. A plain `/run` single has no
workflow status to settle, so this module is **not** the reconciler for `/subagents-detach`; the
reconciler for a single is simply "call `remember_foreground_run` a second time with the terminal result".

**The real existing seam is `exec/`'s detach lifecycle** — and it is 90% of the hard part, already built:
`drive_attempt.rs:353-360` sets `detached_seen`, `attempt_runner.rs:213/637/675` carries it onto
`AttemptSignal::detached`, `classify_attempt` (`fallback.rs:2004-2006`) settles `LadderStop::Detached`,
`disarm_structured_guard_on_detach` (`exec/mod.rs:955-961`) hands the structured-output directory to the
still-live child, and `assemble_delivered_output` skips truncation for a detach (`exec/mod.rs:1739-1745`).

**The one thing cyrup's detach does NOT do is return early.** `workflow_detach/mod.rs:12-23` and
`workflow_launch.rs:722-733` both state the premise outright: *"cyrup's drive loop keeps driving a detached
child to its real exit … so `SingleResult::detached` arrives on an ALREADY-SETTLED result"*, and
*"the two upstream moments … collapse into this one settlement."* **`/subagents-detach` makes that premise
false** (see §E).

#### C.5 The shape the executor must build

`run_foreground_impl` (`foreground.rs:364-412`) is one `await` of `drive_foreground_run_sync`. A user detach
must split it in two **without touching the child process**:

1. Put a `detach: DetachSignal` on `ForegroundControlEntry` next to `interrupt` — a one-shot the slash
   handler fires. `CancelToken` is the wrong type (it means *stop*); a `tokio::sync::oneshot`/`Notify` pair
   plus an `AtomicBool` "accepted" flag is right, because upstream's `detach()` returns `bool` and the
   *caller must be told whether the coordinator accepted* (`execution.ts:640-649`).
2. `run_sync` races the signal. On fire, and only while the run is neither settled nor cancelled, it mints
   upstream's receipt verbatim (`exit_code = -2`, `detached = true`,
   `detached_reason = "user request"`, `final_output = "Detached at user request before task
   completion."`, `output_save_error` when an output path exists) and returns it — while the
   `SpawnedChild` and its drive loop are **moved** to a continuation, not dropped. Dropping the future
   kills the child (cyrup's child is a real OS process owned by that future); this is the single sharpest
   difference from upstream and the reason this item is not small.
3. `run_foreground_impl` sees `result.detached && reason == user request`, calls
   `remember_foreground_run` with the RECEIPT (status `"detached"`), calls `settle_foreground_run`
   (dropping the live control — upstream drops it too, which is why `fleet-view.ts:406-408` re-checks
   `!activeForegroundIds.has`), **skips** `persist_foreground_run_history_for` (a detached run is not
   persistable anyway — `persist.rs` already refuses it, so calling it is harmless but pointless), and
   returns the receipt.
4. A spawned task owns the continuation: it finishes driving, and on real settle calls
   `remember_foreground_run` **again** with the terminal result (this is upstream's
   `reconcileDetachedForegroundChild`, `subagent-executor.ts:833-891`) and writes the output artifacts.
   It must hold a `Weak<SubagentExecutor>` so a shutdown does not keep the executor alive.

**Honest verdict on size — see `sizing`.** This is larger than the other three items combined.

### D. What cyrup already has (`alreadyImplemented` — grepped hard)

#### D.1 For the wait rename
* `WAIT_TOOL_NAME: &str = "wait"` — `extension/wait_tool.rs:16`; read at `:132` (`Tool::name`) and
  `background/wait.rs:971`. `extension/mod.rs:102-104` explains why it is NOT re-exported.
* **Three hard-coded `"wait"` literals in the `cyrup-it` registration test**:
  `crates/cyrup-it/tests/subagents/wait_tool_registration_integration.rs:81` (`FauxResponse::tool_call`),
  `:95` (`vec!["wait"]`), `:107` (`assert_eq!(tool_name, "wait")`).
* **Prose that names the tool** and must change with it: the tool description itself
  (`wait_tool.rs:20-55`, incl. *"wait also returns when a run needs attention"* and
  *"Configured behavior: wait is disabled by config.waitTool"*); `Tool::label` `"Wait"`
  (`wait_tool.rs:144`) — upstream's label is `"Background Wait"` (`wait-tool.ts:39`);
  `registration/tool_description.rs:78`, `:126`, `:142`.
* **`resources/docs/` and the skill**: `docs/configuration.md:41`, `:109`; `docs/tool-reference.md:3`,
  `:277`, `:279-281`; `docs/extension-api.md:12`, `:25`; `docs/README.md:54`;
  `docs/workflows.md:17`; `docs/observability.md:21`, `:25`, `:30`.
* `background/wait.rs:792` is **not** a tool name — it is the `details.wait` payload KEY
  (`pi :374`). **Do not rename it.**

#### D.2 For `/subagents`
* `HostServices::select` (`crates/cyrup-ext/src/host/services.rs:256`), `::editor` (`:274`),
  `::confirm` (`:243`), `::input` (`:248`), `::custom` (`:293`), `::open_overlay` (`:297+`) — the whole
  `ctx.ui.*` surface `subagents-admin.ts` needs, reachable from the executor via
  `SubagentExecutor::host_services()`.
* `AgentSource` with all five upstream variants incl. `Runtime` (`discovery/types.rs:54-59`) and
  `source_rank` semantics in `AgentSource::rank` (`:75-78`) — note **cyrup ranks Project/Runtime 0, User 1,
  Package 2, Builtin 3**, which is `sourceRank` (`subagents-admin.ts:25-30`) exactly.
* `OverrideScope { User, Project }` (`discovery/types.rs:534`), `override_scope_str`
  (`discovery/management/helpers.rs:96`).
* `AgentOverrideInfo { scope, settings_path, base_snapshot }` (`discovery/types.rs:545-552`) — has
  upstream's `scope`, `path` and `base`.
* The settings-writer trio, `async`: `merge_builtin_agent_override`
  (`discovery/settings_write.rs:158`), `remove_builtin_agent_override` (`:193`),
  `remove_builtin_agent_override_fields` (`:223`). Note the signature is `(path, name, fields)` — the
  **scope→settings-path resolution is the caller's job** (`tier_actions.rs:299` shows the pattern).
* `serialize_agent` (`discovery/management/frontmatter_write.rs:41`), `write_agent_file` (`:28`),
  `preserved_frontmatter_fields` (`:401`) — the non-settings save path.
* `EXTRA_AGENT_DIRS_ENV_VAR = "CYRUP_SUBAGENT_EXTRA_AGENT_DIRS"` (`discovery/mod.rs:107`) and
  `resolve_extra_agent_dirs` (`:768`) — `isReadOnlyExtraAgent` is expressible **exactly**.
* `format_agent_detail` (`discovery/management/render.rs:39-140`) — cyrup's `metadataFor`; already richer
  than upstream's (aliases, fallback models, acceptance). Reuse it; do not write a second renderer.
* `registry_available_models() -> Vec<AvailableModelEntry>` (`extension/models/mod.rs:66`),
  `registry_models()` (`:61`) — `ctx.modelRegistry.getAvailable()`.
* `handle_management_action` with `MANAGEMENT_ACTIONS` incl. `get`/`models`/`update`
  (`discovery/management/mod.rs:174-208`) — the read half and the custom-agent write half.
* `discovery/runtime_registry.rs` — `mergeRuntimeAgents`/`RuntimeAgentOwner`'s equivalent.
* `exec/thinking_ceiling.rs:47` `parse_thinking_level` / `cyrup_core::ThinkingLevel` — the level
  vocabulary for `chooseThinking`.

#### D.3 For `/subagents-detach` — see §C.3/§C.4. Summary: the whole **reader** half is built and
production-wired; the **producer** is missing.

#### D.4 For the four deletions
`SlashCommandName` has **18** variants (`registration/slash_commands.rs:82-127`); `SLASH_COMMANDS` has 18
descriptors (`:191-297`). The four commands have exactly **13** `SlashCommandName::` references
crate-wide, in **3** files: `slash_commands.rs:138,139,140,150,198,203,208,261`;
`extension/host/mod.rs:1204`; `extension/host/slash.rs:396,397,398,406`.

### E. `falsePremisesInTree` — doc comments this batch makes false, or that are already false

1. **`extension/wait_tool.rs:13-15`** — *"upstream renamed it to `subagent_wait` in `9245034`
   (2026-07-14) … which is post-baseline drift this port does not pull in."* At v0.68.0 the name is
   `bg_wait` (`wait-tool.ts:38`), and this batch pulls the drift in. **DELETE the whole note**, do not
   reword it. `grep -n 'subagent_wait' crates/cyrup-ext-subagents/src/extension/wait_tool.rs`.
2. **`watchdog/permission_arbiter.rs:382-387`** — `INTERNAL_TOOLS` contains **`"subagent_wait"`**, a name
   that is neither cyrup's registered tool (`"wait"`) nor upstream's (`bg_wait`). **This is a live bug, not
   just a stale string**: `permission_decision` (`:~470`) therefore ungates a tool that does not exist while
   leaving the REAL wait tool gateable, so a parent policy `{"wait":"deny"}` can strand a child — exactly
   what `permissions.ts:26`'s refusal exists to prevent. Two tests pin the wrong name:
   `permission_arbiter.rs:1733` and `prompt_runtime.rs:2916`. `grep -rn 'subagent_wait' crates/`.
3. **`extension/executor/workflow_detach/mod.rs:12-23`** — *"**cyrup's equivalent moment is earlier, not
   absent.** … cyrup's drive loop keeps driving a detached child to its real exit … so
   `SingleResult::detached` arrives on an ALREADY-SETTLED result. Upstream's two moments … therefore
   collapse into one."* After `/subagents-detach`, that is true of the **intercom** detach only. **Narrow
   the premise to the intercom path by name, or delete it** — it must not be left stating a universal.
   `grep -n 'ALREADY-SETTLED result' crates/cyrup-ext-subagents/src/extension/executor/workflow_detach/mod.rs`.
4. **`extension/executor/workflow_launch.rs:722-736`** — same premise, second copy (*"cyrup's drive loop
   does the opposite … the two upstream moments … collapse into this one settlement"*). Same treatment.
   `grep -n "does the opposite" crates/cyrup-ext-subagents/src/extension/executor/workflow_launch.rs`.
5. **`background/wait.rs:955-960`** — the `[CYRUP-DELTA]`: *"cyrup's `wait` lists async runs only …
   so this site can only ever arm `WaitTargetKind::Async`."* Porting §C.3 makes it false. **DELETE it**
   (the surrounding sentence about porting the foreground kind anyway becomes false too).
   `grep -n 'can only ever arm' crates/cyrup-ext-subagents/src/background/wait.rs`.
6. **`extension/executor/foreground_control.rs:4-9`** — *"pi's fourth member of this quartet,
   `finishForegroundChild` … is deliberately NOT ported here: a cyrup foreground SINGLE run's one child is
   retired by dropping the WHOLE `ForegroundControlEntry`."* Still TRUE after this batch (a detach also
   drops the whole entry). **Verify before touching; do not delete reflexively.**
7. **`resources/skills/pi-subagents/SKILL.md:33-37`** lists `/run`, `/chain`, `/parallel`, `/run-chain`,
   `/subagents-doctor`; **`:770`** shows a `/run-chain` example; **`resources/docs/README.md:55`** lists
   `/chain`, `/parallel`, `/run-chain`; **`resources/docs/workflows.md:32,50,62,70`** document
   `/parallel`, `/chain`, `/run-chain`, `/chain-prompts` with worked examples. Every one of these becomes
   false on deletion. They are **shipped resources the model reads**, not comments.
8. **`docs/gap-analysis/09-cyrup-ext-subagents.md:436`** — SUBA-018 cites `/chain-prompts` at
   `registration/slash_commands.rs:141-142`; already stale, and half the citation disappears.
9. **`discovery/merge.rs:144-147`** — *"`applyCustomAgentOverrides` (`agents.ts:1193-1213`): … project
   override ▷ user override"*. At v0.68.0 it is `agents.ts:1538-1559` and the semantics are **user THEN
   project, cumulative**, with upstream's own comment saying so. `discovery/merge.rs:679-692`
   (`apply_custom_agent`) implements the winner-take-all shape. This is pre-existing and **out of this
   batch's scope to fix**, but it is exactly what `savesThroughSettings`/`persistSettingsField` depend on
   (§G.3), so the exec must cite it rather than silently assume the cumulative model.

### F. Scope corrections the exec must carry into the ledger

* **DoD 1** cannot read *"the palette matches upstream's 17."* After this batch cyrup will register
  **16** of the subagents extension's **18** v0.68.0 commands. Still absent afterwards:
  **`/subagents-steer`** (`slash-commands.ts:1057-1119`, no cyrup variant) and
  **`/subagents-inspect-rpc`** (`:928-943`; cyrup has the PARSER —
  `background/inspect_rpc/request.rs:80-93` — but no registered command). `/subagents-watchdog` is
  registered elsewhere in cyrup (driven in `crates/cyrup-it/tests/subagents/watchdog_model_turn_integration.rs:123`
  via `execute_command("subagents-watchdog", …)`), matching upstream's own separate registration.
  **Restate DoD 1 as: the four deletions land, `/subagents` and `/subagents-detach` are added, and the two
  still-missing commands are named in VL-S11's narrowed body.**
* **VL-S11 must be NARROWED, not closed**, unless `/subagents-steer` and `/subagents-inspect-rpc` are also
  landed — and they are not in this batch.
* **VL-S8's own evidence line (`PARITY-GAPS.md:1502`) cites `wait-tool.ts:9` / `name: "subagent_wait"`.**
  Correct it to `:38` / `bg_wait` as part of closing the row.
* **`docs/gap-analysis/00-residual-ledger.md:85`** prescribes *"One const plus a compat alias"*.
  **Upstream registers no alias** (`wait-tool.ts:44`, a single `registerTool`). Ship no alias, and amend
  that row's prescription so the next reader does not re-add one. No cyrup consumer is pinned to `"wait"`
  except the three lines of `wait_tool_registration_integration.rs` listed in §D.1, which are this batch's
  own test and are updated with it. The only "pinned" consumers point at `subagent_wait`, which is broken
  today either way (§E.2).

### G. The Rust shape

#### G.1 `bg_wait` (small, mechanical, one real bug fixed)
```rust
// extension/wait_tool.rs:16 — the doc note above it is DELETED, not reworded (§E.1)
/// pi `wait-tool.ts:38` @v0.68.0 — `name: "bg_wait"`. No alias is registered upstream
/// (`:44` registers one tool) and none is registered here.
pub(crate) const WAIT_TOOL_NAME: &str = "bg_wait";
```
* `Tool::label` → `Some("Background Wait")` (`wait-tool.ts:39`).
* `watchdog/permission_arbiter.rs:382-387`: `INTERNAL_TOOLS[2]` → `crate::extension::wait_tool::WAIT_TOOL_NAME`.
  **Reference the const, never a second literal** — that is what makes `permissions.ts:8`'s coupling
  un-driftable, and it is the whole reason this rename has an observable beyond advertising.
  `permission_arbiter.rs` is not a descendant of `extension::wait_tool`, so the const must be promoted from
  `pub(crate)` where it is… it already IS `pub(crate)`, so the path works; **delete the now-false claim in
  `extension/mod.rs:102-104` that *"`wait_tool` is its only reader"***.
* Description prose, `registration/tool_description.rs:78/126/142`, and the nine `resources/docs/` +
  `SKILL.md` sites from §D.1: rewrite to `bg_wait`. Prefer upstream's own wording at
  `wait-tool.ts:16-27` where cyrup's text is merely a paraphrase.

#### G.2 The four deletions (small, but with a resource-doc tail)
Delete the 4 variants (`:83,84,85,101`), 4 `as_str` arms (`:138,139,140,150`), 4 descriptors
(`:198,203,208,261`), 4 dispatch arms (`extension/host/slash.rs:396,397,398,406`), the handlers
`slash_chain` (`:688`), `slash_parallel` (`:713`), `slash_run_chain` (`:747`), `slash_chain_prompts`
(`:931`), the parsers `parse_agent_args` (`slash_commands.rs:1347`), `AgentArgsCommand` (`:1317`),
`ParsedAgentArgs` (`:1326`), `parse_chain_command` (`:1443`), `parse_parallel_command` (`:1564`),
`parse_run_chain_command` (`:1632`) + `ParsedRunChainCommand`, `split_chain_declaration`
(`registration/prompt_workflows.rs:454`, its only non-test caller is `slash.rs:932`), every pinning test
(`slash_commands.rs:2422-2465`, `:2628-2648`; `extension/host/mod.rs:1204`;
`prompt_workflows.rs:751,754`), and the resource docs in §E.7.

**What must NOT be taken with them** (grepped, not assumed):
* `PromptWorkflow` and `slash_prompt_workflow` (`slash.rs:892`) — upstream keeps `/prompt-workflow`
  (`prompt-workflows.ts:254`).
* `run_prompt_workflow_chain`, `prompt_workflows::build_chain_steps`, `::split_prompt_chain`,
  `::format_workflow_list`, `::parse_runtime_options`, `::shell_words`, `::find_workflow`,
  `::discover_prompt_workflows` — **all still reached by `/prompt-workflow`** through the recipe's own
  `chain:` frontmatter branch (`slash.rs:911-922`, pi `prompt-workflows.ts:286-295`).
* `spawn::chain_graph::{RunnerStep, SingleStepSpec, ParallelGroupSpec}` and chain discovery
  (`discovery/chains.rs`, `resolve_chain`) — the `subagent` TOOL's chain/parallel actions and the
  `workflowScript` runtime still use them. Delete only the SLASH parsers.

#### G.3 `/subagents` — `registration/subagents_admin.rs` (new module) + one `AgentOverrideInfo` extension

```rust
/// pi `subagents-admin.ts:117`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditableOverrideField { Model, Thinking, SystemPrompt }

/// pi `AgentSelection` (`:119-123`) — a closed enum, not an Option+message pair.
pub(crate) enum AgentSelection {
    Selected(Box<AgentDefinition>),
    Cancelled,
    NotFound { agents: Vec<AgentDefinition>, requested: Option<String> },
    Ambiguous { requested: String, matches: Vec<AgentDefinition> },
}

/// pi `:418-423` — the requested action token, parsed once.
pub(crate) enum AdminAction { ChangeModel, ChangeThinking, EditSystemPrompt, ShowDetails, Done }

#[derive(Debug, thiserror::Error)]
pub(crate) enum SubagentsAdminError {
    #[error("Cannot update '{agent}' {field} because that agent is runtime-registered by an extension; edit its source definition instead.")]
    RuntimeRegistered { agent: String, field: &'static str },
    #[error("Cannot update '{agent}' {field} because that field is owned by its read-only package definition.")]
    PackageOwned { agent: String, field: &'static str },
    #[error("Cannot update '{agent}' because its definition in CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is read-only.")]
    ReadOnlyExtraDir { agent: String },
}
```
`readOnlyAgentMessage` (`:150-160`) is these three, and the env-var NAME in the third is cyrup's
(`CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`, `discovery/mod.rs:107`) — the governing directive's "pi names can
change" applies to the env var, **not** to the sentence shape.

**`AgentOverrideInfo` must grow two fields** (`discovery/types.rs:545-552`):
```rust
    /// pi `BuiltinAgentOverrideInfo.fields` (`agents.ts:118`, built at `:1418`) — the union of every
    /// settings key that has been applied to this agent, across scopes. `savesThroughSettings`'s
    /// second clause (`subagents-admin.ts:129`) reads it and has no other data source.
    pub fields: std::collections::BTreeSet<String>,
    /// pi `BuiltinAgentOverrideInfo.fieldScopes` (`agents.ts:119`, accumulated at `:1421-1424`) —
    /// per field, every scope that contributed it. `persistSettingsField`'s `shadowsLowerScope`
    /// branch (`subagents-admin.ts:283`) reads it and has no other data source.
    pub field_scopes: std::collections::BTreeMap<String, std::collections::BTreeSet<OverrideScope>>,
}
```
Populated in `apply_builtin_override` (`merge.rs:437`) and `apply_custom_override` (`:701`).
**`field_scopes` can only ever hold ONE scope until `apply_custom_agent` (`merge.rs:679-692`) is made
cumulative (user THEN project) to match `agents.ts:1546-1558`.** The exec must either make that change or
say, in the seam, that `shadowsLowerScope` is unreachable for custom agents until it does — **with the
grep** (`grep -n 'fn apply_custom_agent' crates/cyrup-ext-subagents/src/discovery/merge.rs`). Do **not**
simulate the second scope.

The five save/choose functions port straight across, reusing §D.2's existing primitives:
`buildBuiltinBase` → a `BuiltinOverrideBase` built from `AgentOverrideInfo::base_snapshot`;
`persistSettingsField` → `merge_builtin_agent_override` / `remove_builtin_agent_override_fields` with the
scope→path resolution the caller already does at `tier_actions.rs:299`;
the non-settings branch → `serialize_agent` + `preserved_frontmatter_fields` + `write_agent_file`;
`metadataFor` → `format_agent_detail`; `metadataSummary` (`:354-360`) is a NEW three-field one-liner.

#### G.4 `/subagents-detach`
```rust
// extension/executor/notices.rs — beside `interrupt`
    /// pi `ForegroundRunControl.detach` (`shared/types.ts`; lifted from the child by
    /// `syncCurrentChild`, `foreground-control.ts:59`). `None` until the run's attempt publishes its
    /// detach-ready callback (pi `execution.ts:1372`'s `onDetachReady`).
    ///
    /// NOT a `CancelToken`: a cancel means *stop*, and the whole point of a detach is that the child
    /// keeps running. `request()` returns upstream's own `boolean` — `false` when the coordinator
    /// refused (already settled, already detached, already cancelled) — which is the value
    /// `/subagents-detach`'s third branch renders.
    pub(crate) detach: Option<ForegroundDetachHandle>,
```
```rust
/// The accept-or-refuse handshake behind pi's `control.detach?.()` (`execution.ts:612-649`).
#[derive(Clone)]
pub(crate) struct ForegroundDetachHandle { /* Arc<DetachCell> */ }
impl ForegroundDetachHandle { pub(crate) fn request(&self) -> bool { /* … */ } }

/// pi `selectForegroundDetachControl` (`slash-commands.ts:237-250`). Three outcomes, because the
/// caller renders three different sentences at two different severities.
pub(crate) enum ForegroundDetachTarget {
    Found { run_id: RunId, entry: ForegroundControlEntry },
    None,
    Ambiguous { requested: String, matched: Vec<RunId> },
}
```
Readers to add: `SubagentExecutor::remembered_foreground_run(&RunId) -> Option<ForegroundHistoryRun>`
(mirrors `live_foreground_controls`'s snapshot discipline); the `status` by-id branch at
`extension/executor/status.rs:584` gains a second arm over it; `background/wait.rs`'s `active_runs` gains
`active_detached_foreground_runs` (pi `subagent-wait.ts:224-243`) and the arming site at `:955-985` gains
the `WaitTargetKind::Foreground` arm it currently cannot reach.

The success text is one constant, byte-identical to `slash-commands.ts:999` with `bg_wait` in it —
which is why §2 and §4 must land in that order.

### H. Production call sites

1. `HostSlash::dispatch_slash` (`extension/host/slash.rs:~360-408`) — `SlashCommandName::Subagents` and
   `::SubagentsDetach` arms, beside the existing `::SubagentsRefine` arm (`:392`). This is the
   `NativeExtension::execute_command` path a real `/`-command reaches.
2. `SLASH_COMMANDS` (`registration/slash_commands.rs:191`) + `SlashCommandName::as_str`
   (`:130-155`) — registration and name parsing; `from_str_exact` (`:160`) derives from the table.
3. `crates/cyrup-ext-subagents/src/registration/mod.rs`'s command registration (whatever feeds
   `cyrup_ext::registry::CommandDescriptor`) — the four deletions must leave it consistent.
4. `WaitTool::name` (`extension/wait_tool.rs:132`) — the live tool registration the model sees.
5. `permission_decision` / `validate_permission_rules` (`watchdog/permission_arbiter.rs:382`, `:462`) —
   the child-coordination ungate.
6. `SubagentExecutor::control_status` (`extension/executor/status.rs:584`) — `status` by id.
7. `wait_for_subagents` (`background/wait.rs`) — `bg_wait` by id / `all`.
8. `SubagentExecutor::run_foreground_impl` (`extension/executor/foreground.rs:364-412`) — the detach split.
9. `SubagentExecutor::register_foreground_controls` (`foreground.rs:977`) — stamps the new `detach` handle.

### I. Reachability tests, and why each fails if gutted

1. **`bg_wait` dispatches under its new name** — rewrite
   `crates/cyrup-it/tests/subagents/wait_tool_registration_integration.rs:81,95,107` to `"bg_wait"`.
   *Fails if gutted:* the faux model calls `bg_wait`; a session that still registers `"wait"` produces no
   `tool_execution_start` for it and `tool_starts(&events)` is empty.
2. **The internal-tools ungate protects the REAL tool** — new unit test in `permission_arbiter.rs`:
   `permission_decision(Some(&{WAIT_TOOL_NAME: Deny}), WAIT_TOOL_NAME) == Allow`, and
   `validate_permission_rules` refuses a rule keyed on `WAIT_TOOL_NAME` with
   *"reserved for child coordination and cannot be gated."*
   *Fails if gutted:* revert `INTERNAL_TOOLS` to `"subagent_wait"` and the deny rule is honoured — the
   assert flips to `Deny`. This is the test that proves the rename fixed something rather than renaming a
   string.
3. **No stale spelling survives** — a `#[test]` that greps the crate's own
   `wait_tool_description(true)`, `Tool::label()` and `registration::tool_description` output for the
   word-boundary literal `wait` in tool-name position (`bg_wait` must be the only match).
   *Fails if gutted:* leave any one description sentence saying `wait({ id })` and the assert fires.
4. **The four commands are gone from the registered palette** — assert
   `SlashCommandName::from_str_exact("chain" | "parallel" | "run-chain" | "chain-prompts").is_none()`
   and `SLASH_COMMANDS.len() == 16`.
   *Fails if gutted:* re-adding any variant restores `from_str_exact` and bumps the length.
5. **`/prompt-workflow`'s chain branch still works after `/chain-prompts` dies** — an IT that runs
   `/prompt-workflow <recipe-with-chain-frontmatter>` and asserts two steps ran.
   *Fails if gutted:* deleting `build_chain_steps`/`split_prompt_chain` along with `/chain-prompts` makes
   it not compile; deleting the `chain:` branch in `slash_prompt_workflow` makes it run one step.
6. **`/subagents <name>` with no UI emits the metadata block** — IT through `execute_command("subagents",
   "scout", &ctx)` with `has_ui == false`; assert the output starts `Agent: scout (builtin)` and contains
   `System prompt mode:`.
   *Fails if gutted:* a stub returning `Ok(String::new())` or a `todo!()` produces no `Agent:` line.
7. **`/subagents <builtin> model` actually persists a settings override** — IT with a scripted
   `HostServices::select` returning a concrete model id and then `"user"` for the scope; assert
   `~/.cyrup/…/settings.json` gained `subagents.agentOverrides.scout.model`, AND that a re-discovery
   reports that model on the agent.
   *Fails if gutted:* the whole point — no file write, no override on re-read. The re-read half is what
   stops a test passing against a write to the wrong scope's path.
8. **A package-sourced agent refuses with upstream's sentence** — assert the exact
   `Cannot update '<n>' model because that field is owned by its read-only package definition.`
   *Fails if gutted:* collapsing `readOnlyAgentMessage` into one generic refusal changes the bytes.
9. **An `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` agent refuses** — set the env var to a temp dir holding the
   agent; assert the `…is read-only.` sentence.
   *Fails if gutted:* dropping `isReadOnlyExtraAgent`'s path-containment check lets the edit through and
   the assert sees a success message.
10. **`/subagents-detach` with no live run** — assert
    `No active foreground single-subagent run to detach.`; with an unknown id, assert
    `No active foreground run found for 'zzz'.`; with two runs sharing a prefix, assert the
    `Ambiguous foreground run id prefix …` sentence. Three distinct strings from one resolver.
    *Fails if gutted:* `resolve_live_foreground_run`'s two-way `Option` cannot produce three sentences —
    reusing it collapses ambiguity into the not-found message and the third assert fires.
11. **`/subagents-detach` on a live foreground run returns the receipt and leaves the child alive** — IT:
    launch `/run scout <long task>` on a task task that blocks; from a second task fire the command;
    assert (a) the success sentence names the run id and both `subagent({ action: "status", … })` and
    `bg_wait({ id: … })`, (b) the child PID is still alive immediately after, (c) the returned
    `SingleResult` has `detached == true` and `exit_code == -2`.
    *Fails if gutted:* an implementation that cancels the run instead of detaching kills the PID (b
    fires); one that returns a message without minting the receipt fails (c).
12. **A detached run is addressable and reconciles** — same IT, continued: immediately after the detach,
    `subagent({action:"status", id})` must render the run (not `Async run not found`); `bg_wait({id})`
    must BLOCK rather than return "no active run"; and when the child finally exits, the wait must return
    with the terminal result and `foreground_runs[id]`'s child status must have flipped off `"detached"`.
    *Fails if gutted:* without the `status.rs:584` second arm the first assert gets
    `Async run not found. Provide id or dir.`; without `active_detached_foreground_runs` the wait returns
    immediately; without the continuation task the status stays `"detached"` forever and the wait times out.
13. **A detached run never reaches disk** — assert `<resultsDir>/foreground-history.json` gains no entry
    for the detached id while it is detached, and gains one after it settles.
    *Fails if gutted:* `persist.rs`'s existing refusal already covers the first half — this test's value
    is the SECOND half, which fails if the continuation never re-remembers.

### J. Files touched (expected)

`crates/cyrup-ext-subagents/src/extension/wait_tool.rs`,
`.../src/extension/mod.rs`,
`.../src/background/wait.rs`,
`.../src/background/wait_subscriptions/manager.rs` (reachability only),
`.../src/watchdog/permission_arbiter.rs`,
`.../src/prompt_runtime.rs` (test literal),
`.../src/registration/tool_description.rs`,
`.../src/registration/slash_commands.rs`,
`.../src/registration/prompt_workflows.rs`,
`.../src/registration/mod.rs`,
`.../src/registration/subagents_admin.rs` (new),
`.../src/extension/host/slash.rs`,
`.../src/extension/host/mod.rs`,
`.../src/extension/executor/notices.rs`,
`.../src/extension/executor/foreground.rs`,
`.../src/extension/executor/foreground_control.rs`,
`.../src/extension/executor/mod.rs`,
`.../src/extension/executor/status.rs`,
`.../src/extension/executor/workflow_detach/mod.rs` (premise),
`.../src/extension/executor/workflow_launch.rs` (premise),
`.../src/exec/mod.rs`, `.../src/exec/fallback.rs`, `.../src/exec/drive_attempt.rs`,
`.../src/discovery/types.rs`, `.../src/discovery/merge.rs`,
`.../resources/docs/{README,configuration,tool-reference,extension-api,workflows,observability}.md`,
`.../resources/skills/pi-subagents/SKILL.md`,
`crates/cyrup-it/tests/subagents/wait_tool_registration_integration.rs`,
`crates/cyrup-it/tests/subagents/` (new: `subagents_admin_integration.rs`, `subagents_detach_integration.rs`),
`docs/gap-analysis/PARITY-GAPS.md`, `docs/gap-analysis/00-residual-ledger.md`,
`docs/gap-analysis/09-cyrup-ext-subagents.md`.

### K. Sizing — the honest answer

**`/subagents-detach` is bigger than the other three items combined, and the exec must sequence it LAST.**

The three small ones are genuinely small. **§2 (`bg_wait`)** is one const, one label, ~12 prose sites and
three test literals — half a day — and it carries the batch's best value-per-line because
`INTERNAL_TOOLS`'s `"subagent_wait"` is a live gating bug (§E.2) that the rename closes as a side effect.
**§1 (the four deletions)** is 13 typed references in 3 files plus ~6 parser functions and ~10 tests; the
only care needed is the "what NOT to take" list in §G.2 — the resource-doc tail (§E.7) is 8 files of prose
and is the part a careless pass will miss. **§3 (`/subagents`)** is a real 460-line port but almost every
primitive already exists (§D.2): the selector UI, the settings writer trio, the serializer, the metadata
renderer, the model registry and the extra-dirs env var are all in-tree. Its one genuine new thing is the
two `AgentOverrideInfo` fields (`fields`, `field_scopes`) and populating them in both `apply_*_override`
arms — and the honest caveat that `field_scopes` cannot hold two scopes until `apply_custom_agent`
(`merge.rs:679`) is made cumulative to match `agents.ts:1546-1558`. Call it a day and a half.

`/subagents-detach` is different in kind. The handler is 20 lines and the four strings are free; the
capability under it is not. cyrup's reader half is already complete and production-wired —
`ExecutorForegroundProbe` (`extension/executor/wait_subscriptions.rs:78-142`), the `"detached"` history
status (`record.rs:170`), the never-persist rule (`persist.rs:249`), the whole
`WaitSubscriptionManager` foreground branch — so this is NOT a green-field feature; it is a missing
**producer** feeding machinery that has been waiting for it. But that producer sits at the single hardest
seam in the crate: **cyrup's foreground child is a real OS process owned by the `drive_foreground_run_sync`
future**, where upstream's is an in-process session object whose callbacks keep firing after the tool call
returns (`workflow_detach/mod.rs:12-23` states this difference as settled fact — and this batch is what
unsettles it, §E.3/§E.4). Detaching therefore means *splitting one `await` into a receipt and a
continuation without dropping the future*, which touches `run_sync`'s settle path, the ladder's detach
classification, `run_foreground_impl`'s remember/settle/artifact ordering, and a new spawned owner holding
a `Weak<SubagentExecutor>`. Add the two reader extensions that make the run addressable at all
(`status.rs:584`'s second arm; `wait.rs`'s `active_detached_foreground_runs`, which deletes a
`[CYRUP-DELTA]`), the new three-way resolver, and the detach handle on `ForegroundControlEntry` — and it
is three to four days of careful work against a lifecycle where getting it wrong kills the child process
the feature exists to keep alive.

Recommended order: **§2 (`bg_wait`) → §1 (deletions) → §3 (`/subagents`) → §4 (`/subagents-detach`)**.
§2 must precede §4 because the detach success sentence quotes `bg_wait({ id })`; §1 and §3 both touch
`SLASH_COMMANDS` and are cheaper to land before the file is under detach-shaped churn. A checkpoint commit
after each of the first three keeps §4's blast radius isolated.

### L. Blockers (genuine missing dependencies only)

**None that stops this batch.** Two facts to record as residuals with TRUE premises:

1. **`options.foregroundDetachShortcut`** (`slash-commands.ts:1007-1012`) binds the same detach closure to
   a configurable key. cyrup's native-extension registration surface has no per-extension keybinding slot
   on this path (`cyrup_ext::registry::CommandDescriptor` carries name/usage/description/completions
   only — `crates/cyrup-ext/src/registry.rs:69-73`, as `slash_commands.rs:180-188` already records for
   completions). The slash command is fully functional without it. **Residual, not blocker.**
2. **`/subagents-steer` and `/subagents-inspect-rpc`** stay unregistered after this batch (§F). The
   inspect-rpc PARSER exists (`background/inspect_rpc/request.rs:80`) with no registered command; steer
   has neither. **They are why VL-S11 narrows rather than closes.**

---

## [SCOPE CORRECTION — orchestrator, 2026-09-20] VL-S11 CLOSES. There are no residuals.

The `[AUG — slash-surface]` section files four items as "residuals with TRUE premises". **Three of
them have FALSE premises — verified below — and the fourth is a choice, not a constraint.** All
four are IN SCOPE. VL-S11 closes; it does not narrow. The standing directive is the whole feature,
and "16 of 18 commands" is not the whole feature.

### R1 — `/subagents-steer` · the capability is already live · **IN SCOPE**

AUG: *"steer has neither [parser nor command]."* **FALSE.** The `steer` action is fully ported,
advertised and dispatched:

- advertised: `extension/tool/text.rs:281` (in `SUBAGENT_ACTIONS`), documented `:53`
- dispatched: `extension/tool/routing.rs:2316` (`"steer" => { … }`), inside the control band at
  `:1189`
- its authority gate is live — VL-S7's row records `allowSteer`/`allowStop` as the gate
  `registration/authority.rs` consults

So `/subagents-steer` (upstream `slash-commands.ts:1057-1119`) is a slash REGISTRATION over a
capability that already works. The precedent is in-tree and exact: `/subagents-stop`
(`SlashCommandName::SubagentsStop`) already opens a TUI selector on its no-id branch and then
issues the same action for the pick — upstream's `/subagents-steer` has the same shape.

### R2 — `/subagents-inspect-rpc` · the parser is already there · **IN SCOPE**

AUG: correctly notes cyrup has the parser and no command — then files it as a residual anyway.
`background/inspect_rpc/` is four modules (`mod.rs`, `request.rs`, `respond.rs`,
`read_output.rs`). Upstream's command is 15 lines (`slash-commands.ts:928-943`). Register it.

### R3 — the detach keybinding · the slot EXISTS · **IN SCOPE**

AUG: *"cyrup's native-extension registration surface has no per-extension keybinding slot on this
path — `cyrup_ext::registry::CommandDescriptor` carries name/usage/description/completions only
(`crates/cyrup-ext/src/registry.rs:69-73`)."* **FALSE — it looked at the wrong struct.**
`CommandDescriptor` is for COMMANDS. Shortcuts have their own registration path, and it is
already built:

```
crates/cyrup-ext/src/facade.rs:501-502
    for (key, desc) in shortcuts {
        self.registry.register_shortcut(id.clone(), key, desc)?;
```

That IS upstream's `pi.registerShortcut(options.foregroundDetachShortcut as KeyId, {…})`
(`slash-commands.ts:1007-1012`). Port the shortcut binding through it. If the config key that
selects the chord has no home yet, add it the way every other subagents config key is added —
that is this batch's work, not a missing dependency.

### R4 — `field_scopes` and the cumulative custom-agent merge · **IN SCOPE, land the merge**

AUG offers an either/or: make `apply_custom_agent` (`discovery/merge.rs:679-692`) cumulative
(user THEN project, matching `agents.ts:1538-1559`), **or** state with the grep that pi's
`shadowsLowerScope` branch (`subagents-admin.ts:283`) is unreachable for custom agents.

**Take the first.** The second leaves `/subagents` unable to show a user override shadowed by a
project one — which is a real thing a user does, and upstream shows it. Make the merge cumulative
and populate both `AgentOverrideInfo` fields for real. Do NOT simulate a second scope; make the
merge actually carry two.

### What this changes in the Definition of Done

- DoD item 1 stands as written: the palette matches upstream's — **18 of 18** registered
  (`/subagents`, `/subagents-detach`, `/subagents-steer`, `/subagents-inspect-rpc` added; the four
  v0.41.0 deletions removed).
- New: `/subagents-steer` reaches the live `steer` action, with the no-id selector branch.
- New: `/subagents-inspect-rpc` reaches `background/inspect_rpc/`.
- New: the detach shortcut is bound through `register_shortcut`.
- New: `apply_custom_agent` merges cumulatively and `field_scopes` can hold two scopes, pinned by
  a test in which a project override shadows a user one.
- Ledger: VL-S11 **CLOSED**, not narrowed.

### The standing rule this section exists to enforce

An augment may report that something is BIG. It may not decide that something is OUT. If a piece
is large, it is sequenced — not dropped — and the only thing that may be recorded as a residual is
a dependency that genuinely does not exist, with the grep that proves it. Three of the four above
were filed on premises that a single grep refutes. **Grep before you file.**

---

## [EXEC — slash surface] 2026-09-21 — what landed, and the eight things this spec got wrong

Seven writers, one verify pass. `VL-S8`, `VL-S11` and `VL-S12` all **CLOSED**; `SUBA-026` narrowed
to its UI half. The Definition of Done stands as amended by the scope correction: the palette is
**18 of 18**, `/subagents-steer` reaches the live `steer` action, `/subagents-inspect-rpc` reaches
`background/inspect_rpc/`, the detach shortcut is bound through `register_shortcut`, and
`apply_custom_agent` merges cumulatively with `field_scopes` holding two scopes.

### What landed

- **VL-S12** — `Chain`, `Parallel`, `RunChain`, `ChainPrompts` deleted from the enum, the table,
  the parsers and the prose. Pinned NEGATIVELY by
  `the_four_commands_upstream_deleted_at_v0_41_0_are_not_registered`, which is the only way a
  removal can be pinned. Five pure slash-surface ITs retired; `tool_parallel_chain`'s inline-group
  fan-out counting and both recipe-chain tests were RE-POINTED rather than deleted, because the
  machinery under them is real and still reachable.
- **VL-S8** — `WAIT_TOOL_NAME` is `bg_wait`, asserted off the REGISTERED tool object rather than
  off the const. **The rename was not the point.** `watchdog/permission_arbiter.rs`'s
  `INTERNAL_TOOLS` — the set a permission policy may NOT gate, because gating one of its members
  strands a child that then cannot report back — held the literal `"subagent_wait"`, **a name this
  crate has never registered at any point in its history.** A parent shipping `{"bg_wait": "deny"}`
  was ACCEPTED. The set now holds `WAIT_TOOL_NAME` itself, so the two cannot diverge again.
- **VL-S11a** — `/subagents`, the admin surface, over the five save/choose functions.
- **VL-S11b** — `/subagents-detach`, and it is the only command here that was not a registration
  over an existing capability. Upstream's detach is cheap because pi's child is an in-process
  session object; cyrup's is a real OS process owned by the `drive_foreground_run_sync` future, so
  the same move would KILL the thing the feature exists to preserve. `run_foreground_impl` is split:
  the future is built from owned inputs and boxed `Pin<Box<dyn Future + Send>>` so the whole value
  can move into `tokio::spawn` on an accepted detach. **Both reader halves were dead from
  production** — `WaitTool::execute` never chained `.with_detached_foreground(…)` and
  `DetachedForegroundRunsSource` had no implementation anywhere in the tree, so
  `active_detached_foreground_runs` always returned empty. Found by the IT that asserts `bg_wait`
  BLOCKS, which was committed knowingly red.
- **`/subagents-steer` and `/subagents-inspect-rpc`**, plus the detach keybinding.
- **Cumulative custom-agent overrides** — user pass then project pass, with `AgentOverrideInfo`'s
  `fields`/`field_scopes` populated at all three construction sites. One extra change was
  mandatory and upstream pins it: `apply_custom_override`'s `disabled` arm was gated on the runtime
  value, and once layering lands that guard makes `disabled` the single key on which a USER entry
  beats a PROJECT one. Upstream's own regression test (`agent-overrides.test.ts:646-684`) names
  that stray guard as the bug its layering commit removed.

### Gates

`cargo fmt --all --check` clean. `cargo clippy --workspace --all-targets --features
test-fixtures -- -D warnings` clean. `cargo nextest run --workspace --features test-fixtures`
**11 074 run, 11 074 passed, 9 skipped**. `cargo nextest run -p cyrup-it --features it` **594 run,
594 passed, 0 skipped**.

**The 594 is accounted for, not asserted.** This spec's brief quoted a `cyrup-it` baseline of 567
and predicted `567 - 6 + 9 = 570`. Both halves were stale. The real pre-batch figure is **590** —
`PARITY-GAPS.md`'s `VL-S6` closure records `cyrup-it 590 passed` on the herdr merge that is this
branch's own ancestor — and the retirement was **five** `#[test]`s, not six: the sixth was
RE-POINTED onto a surviving surface, so it still runs. `590 - 5 + 9 = 594`, and counting
`#[test]`/`#[tokio::test]` attributes across the batch's diff of `crates/cyrup-it/` gives
`before=22, after=26`, i.e. the same `+4`.

**Both numbers above are from clean runs, but two flakes were seen on the way there and are
recorded rather than quietly retried away.** Neither is this batch's, both are timing-dependent
under parallel load, and both were re-run in isolation before being called flaky:

* `extension::tool::routing::scheduled_runs_tests::the_armed_tick_fires_a_due_schedule_with_nobody_asking`
  failed once (`left: 0, right: 1` — the armed tick had not fired) after **14.3s** in a full
  workspace run. Alone it is **0.045s and green**. It is a wall-clock tick assertion about the
  scheduler's own timer, starved when 11 074 tests share the box.
* `cyrup-it::bin acp_session::session_new_is_built_off_the_dispatch_loop` failed once after
  **45.9s** on *"timed out waiting for the response to id 2"* — while its own frame log, printed
  in the panic, CONTAINS that response. Alone it is **0.38s and green**. Last touched at
  `8de7460`, before this batch's base commit, and nothing here is on its path.

They are written down because a suite that reaches green by retrying is not reporting anything, and
the next reader deserves to know which two tests will bite them.

### Eight spec errors, found by the writers and confirmed here

1. **§I.4 says `SLASH_COMMANDS.len() == 16`. It is 18** — and it was 18 at this batch's own base
   commit, before a single command was added or removed. The test asserts 18 and additionally
   names every entry in table order.
2. **§I.8's package-agent refusal is unreachable on the production path.** `savesThroughSettings`
   returns `true` for every package agent on its second line — `subagents-admin.ts:127`,
   `if (agent.source === "package") return true;` — so `readOnlyAgentMessage`'s package arm
   (`:154-155`) is never consulted from there. The claim is a UNIT assertion in
   `subagents_admin.rs` instead of an IT, with the reachability argument recorded in the file so
   the next reader does not re-add it.
3. **§D.2/§G.3 named five primitives as reusable from `registration::`. All five are in PRIVATE
   modules** — `mod render;` (`:73`), `mod frontmatter_write;` (`:69`) and `mod tier_actions;`
   (`:75`) are all declared without `pub` inside `discovery/management/mod.rs`, so
   `format_agent_detail`, `serialize_agent`, `write_agent_file`, `preserved_frontmatter_fields` and
   `tier_actions` are `pub(crate)` items behind a path that cannot be named from outside
   `management`. `pub(crate)` on the item is not enough when the module is private.
4. **`resolve_extra_agent_dirs` (`discovery/mod.rs:768`) is a private `fn`**, not a reusable one,
   and **`AvailableModelEntry` (`extension/models/mod.rs:34`) has private fields** — `provider`,
   `id`, `full_id` are all bare — so a `registration::` caller can construct neither.
5. **§G.2 claimed the `subagent` tool's chain/parallel actions reach `resolve_chain`. They do
   not** — they reach `discovery::chains::chain_step_to_runner_step` (`:1057`). `resolve_chain` is
   defined at `extension/executor/chain.rs:525` and `git grep -n 'resolve_chain\b'` returns that
   definition and nothing else: it is a public-API method with **no in-crate caller**, which is why
   it survives dead-code analysis. The "still use them" clause was right about the chain-graph
   types and wrong about the resolver.
6. **The `/subagents-steer` "no-id opens a selector" hypothesis is REFUTED by upstream.**
   `/subagents-stop` opens `ctx.ui.custom(…)` on its no-id branch (`slash-commands.ts:1044-1047`);
   `/subagents-steer` does not — `:1065-1068` is `sendSlashText(pi, usage)` and nothing else.
   `has_ui` is threaded into the handler and deliberately unused, with the reason in the module
   doc: steering an unnamed run is a guess, and a guess speaks into a live child's prompt. §137's
   own correction ("there is no in-tree precedent for a slash command opening an interactive
   selector") was right, and this is the upstream half of it.
7. **`DetachTarget::Ambiguous`'s frozen-contract doc said "no id was given".** Both producers only
   reach it WITH an id; the no-id path takes upstream's newest-single fallback
   (`slash-commands.ts:246-249`), where several live runs are not a refusal at all.
8. **Three upstream citations in the spec had drifted**: `reconcileDetachedForegroundChild` does
   not exist at v0.68.0 (it is `updateRememberedForegroundChild`, `subagent-executor.ts:850-899`),
   `onDetachReady` is `:4074-4081`, and `subagent-wait.ts`'s arming site is `:704-714`.

### Residuals, each with a TRUE premise and the grep that proves it

Filed in `docs/gap-analysis/00-residual-ledger.md` as `R-VLS11b-01`, `R-VLS11b-02`, `R-VLS11b-03`
and `R-SUBA087-01`: a user detach aimed at a WORKFLOW child is not refused and its reconciliation
is provisional; the detach continuation cannot persist foreground history; the detach chord is
env-tier only and defaults ON where upstream has a settings key and registers nothing unless
configured; and `step.childId`, the unported 4th identity rung. None of the four is a request to
re-scope this spec — they are the four things that genuinely did not exist, greped at HEAD.

### The standing rule, honoured

The rule this spec's last section exists to enforce held: nothing was dropped. Two things were
found BROKEN rather than missing and were fixed rather than filed — `INTERNAL_TOOLS`' phantom
gating and the two dead reader halves behind `/subagents-detach` — and one earlier "finding"
(`includeNested` "is not ported") turned out to have a premise that `/subagents-steer` itself
falsifies, so it was struck rather than carried forward.

### Mutation proof

Every claim this batch makes was gutted and the gutting was proved to turn a test red. Each
mutation was restored byte-for-byte and `git diff` over `crates/` was confirmed EMPTY before the
commit that followed it.

| # | mutation | target tests | result |
|---|---|---|---|
| M4 | `INTERNAL_TOOLS` reverted to the literal `"subagent_wait"` | `a_rule_keyed_on_the_registered_wait_tool_is_refused_at_validation`, `a_deny_rule_on_the_registered_wait_tool_cannot_strand_a_child`, `bash_and_the_internal_tools_are_allowed_even_when_a_rule_says_otherwise` | 3 RED |
| M5 | `apply_custom_agent` reverted to winner-take-all (project replaces user outright) | `custom_agent_layers_project_over_user_without_dropping_user_only_fields` | RED |
| M2 | `control_status`' SECOND foreground arm (`remembered_foreground_run`) deleted | `status_by_id_finds_a_remembered_foreground_run_that_is_no_longer_live` | RED — falls through to `Async run not found. Provide id or dir.` |
| M3 | `active_detached_foreground_runs` returns an empty candidate set | `a_wait_on_a_detached_foreground_run_blocks_until_its_child_is_reconciled`, `the_detached_foreground_candidate_set_honours_every_upstream_filter` | 2 RED |
| M1 | `spawn_detached_foreground_continuation` DROPS the drive future instead of moving it into a task that owns it — i.e. the detach cancels the run | `a_live_detach_returns_the_receipt_and_leaves_its_child_running` (`cyrup-it`) | RED |

M4, M5, M2 and M3 were applied **together, in one build**, and run in a single filtered `nextest`
invocation: their target tests live in four disjoint modules, so batching costs nothing in
attribution. M1 needed the `cyrup-it` link and was run alone.

**M1 found a real weakness in the assertion the detach IT says it exists for, and it is worth
recording.** On the first run, M1 did NOT fail assertion `(b)` — the `/proc`-backed PID-liveness
probe taken immediately after the receipt returns. It failed 40 seconds later, at the `bg_wait`
timeout. The reason: `SpawnedChild::drop` (`spawn/mod.rs:1086-1099`) terminates nothing
synchronously — it SIGKILLs the child's process GROUP, and delivery plus the transition to `Z` is
the kernel's business, not the dropping thread's. A probe with no sleep reads the state the child
had microseconds ago, which is `R`/`S`. `(b)` proves *"not killed SYNCHRONOUSLY by the detach"*,
which is worth proving and is **not** what the file's doc claimed for it.

`(b)` is kept verbatim and a second probe was ADDED after it, 750ms later — a small fraction of the
child's 10s scripted sleep. Under M1 the test now fails in **0.99s at the liveness assertion**
instead of surviving to a 40s timeout. Nothing was weakened and nothing was deleted.
