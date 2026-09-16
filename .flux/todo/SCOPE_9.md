---
stage: qa
status: completed
updated: 2026-09-15 12:00
---

# SCOPE_9 — per-session active-async capacity

> Renamed 2026-09-09 from `SCOPE_9.md` — position 10 of 12 in the WORKFLOW_1|WORKFLOW_12 dependency-ordered sequence.
>
> ⚠ **This file is cross-referenced as `WORKFLOW_10`.** `SCOPE_16.md:12` says it "depends on
> **WORKFLOW_10** (a fired schedule spawns an async run, which must claim [capacity])", and
> `extension/executor/workflow_controllers.rs:18`, `:98`, `:113` name "WORKFLOW_10's per-session
> capacity gate" as the future consumer of `live_workflow_run_ids`. Both mean THIS task.

> ## ⚠ NOT A WORKFLOW TASK — read [`WORKFLOW_1`](WORKFLOW_1.md) §2 before scheduling
>
> Renamed into the `WORKFLOW_*` namespace on 2026-09-09, but this file **never mentions
> `workflowScript`**. It belongs to the **multi-instance session-partitioning** programme
> (with `SCOPE_11`…`SCOPE_17`), whose thesis is *"two cyrup instances sharing a directory must
> not consume each other's runs."*
>
> **It does not block `workflowScript` and `workflowScript` does not block it.** Shipping
> workflows requires [`WORKFLOW_2`](WORKFLOW_2.md) → `3` → `4` → `5`, not this file. Do not read
> the numbering as a runway.
>
> ⚠ If you are here because [`WORKFLOW_2`](WORKFLOW_2.md) or
> [`WORKFLOW_13`](WORKFLOW_13.md) deferred async-workflow work to this file: **it is not owned
> here.** That orphan is [`WORKFLOW_13`](WORKFLOW_13.md)'s — see [`WORKFLOW_1`](WORKFLOW_1.md) §4.

OBJECTIVE: port `background/active-async-capacity.ts` (516 LOC) so the async concurrency cap is
**per session** rather than global. Without it, one instance's fan-out starves every other instance
sharing the directory.

**Depends on WORKFLOW_6** (`workflow_controllers`, consumed as `liveWorkflowRunIds`).

SCOPE_9 is *not* the 30 `sessionId` references upstream — this file is session-keyed end to end.

---

## §0 — AUG 2026-09-15: what changed, and what the draft got wrong

Every line number below was re-derived by opening the file. The draft's own cites are **corrected
in place** where they had drifted; nothing is dropped.

| draft claim | verdict |
|---|---|
| upstream is 516 LOC | **CONFIRMED at tag `v0.66.0`** (`git -C tmp/pi-subagents show v0.66.0:src/runs/background/active-async-capacity.ts \| wc -l` → 516). At `v0.67.0` it is 520. The draft's pin, "HEAD `7fe9dee1`", is an unpinned moving ref and is replaced by `v0.66.0` throughout. |
| `resolveAbandonedSlotReleaseAfterMs` **clamps** to MIN/MAX | **WRONG — there is no clamping anywhere upstream.** See §D1. Two of the ten named tests change meaning. |
| SUBTASK4 "Release on terminal" | **Half wrong.** Nothing calls a `release()`. A slot is freed *lazily*, by the next `reconcile` (which every `acquire`/`inspect` performs), and only against **evidence**. See §D2. |
| the release verdict can be ported as written | **CANNOT — cyrup has no `processTerminal` artifact and no `runnerProcessInstanceId`.** Ported verbatim, a *successful* run's slot would never be released and the cap would permanently fill. See §D3 — the single most important finding in this file. |
| `run-status.ts:628`, `:4988` | drifted; at `v0.66.0` they are `:633` and `:4984`. Full corrected table at §1.3. |
| "sweep item **D22**" | `grep -rn D22` over `.flux/` and `docs/` finds **only this file**. D22 is not a live sweep-item id in this repo. The *requirement* (document the session keying beside the cwd-keyed roots) stands and is kept. |
| depends on WORKFLOW_6 + SCOPE_8 | both **LANDED** (`extension/executor/workflow_controllers.rs`, `extension/executor/workflow_detach/`). `dependsOn` is now empty. |
| — | **NEW SCOPE (task assignment, 2026-09-15): this task owns `updateActiveRunIndex` / the active-run index.** SCOPE_8 was forbidden from touching it (`SCOPE_8.md:742-747`: *"that is SCOPE_9's surface and this task must not touch it"*) and `background/terminal_run_index/mod.rs:73-75` already reserves the literal `".active-runs"` name against a module that does not exist. Fully specified at **SUBTASK5**. |

**Not already implemented.** `grep -rn "active_async_capacity\|ActiveAsyncCapacity\|max_active_async_runs_per_session\|active_run_index" crates/` returns nothing but this task file. `alreadyImplemented = false`.

---

## §1 — Upstream reference (read at a named tag)

Read with `git -C /home/user/cyrup/tmp/pi-subagents show v0.66.0:<path>`. **Never from the working
tree** (its HEAD is `13f8f328` and moves).

> **RE-VERIFIED AGAINST `v0.68.0` (latest), 2026-09-15.** This section was researched at `v0.66.0`
> because that tag reproduces the 516 LOC figure `PARITY-GAPS.md` records — which was the wrong
> reason to choose a tag. `v0.66.0` is two releases behind; **latest is what a port is measured
> against**, and the gap-analysis README says re-measuring it is the first command of an audit, not
> the last. The tag choice has been re-checked properly, and the outcome is that **the design this
> task ports is unchanged at `v0.68.0`**:
>
> - `ACTIVE_ASYNC_CAPACITY_DIR`, `activeAsyncCapacitySessionKey` (= `sha256(sessionId)`), `sessionDir`,
>   `slot-<n>`, the claim/owner protocol and the limit semantics are all **byte-identical**.
>   The session-keying question at §Q3 therefore has the SAME answer at latest — it is not a
>   decision upstream has revisited.
> - `active-async-capacity.ts` moves by **+4/−0** in the whole `v0.66.0..v0.68.0` window.
> - `active-run-index.ts` moves by **+17/−1**, additively.
>
> Two deltas are real and are folded in below (§1.1a). Neither changes a directory, a key or a
> budget; both are additive. Line numbers in the rest of this file remain `v0.66.0`-relative and
> are still correct for the functions they name.

### §1.1a the two `v0.66.0..v0.68.0` deltas this task must carry

**(a) `active-run-index.ts` — a new optional flag, `terminalIndexBeforeRelease`.**

```diff
-export function updateActiveRunIndex(asyncDir, state, toolCallId?, options: { retryCapacityErrors?: boolean } = {}): void {
+export function updateActiveRunIndex(asyncDir, state, toolCallId?, options: { retryCapacityErrors?: boolean; terminalIndexBeforeRelease?: boolean } = {}): void {
+	if (options.terminalIndexBeforeRelease) {
+		// write the marker, then the TERMINAL index, and only then release the active slot
+		...
+		releaseActiveRunIndex(asyncDir);
+		return;
+	}
```

It exists to close an ordering hole: without it the active-run entry can be released before the
terminal index is written, leaving a run in neither index if the process dies between the two.
**SUBTASK5 must port the flag and the ordering**, not just the base function — the router it
introduces over the four production `update_terminal_run_index` call sites is exactly where that
ordering is decided, and getting it wrong reproduces the hole upstream just fixed.

**(b) `active-async-capacity.ts` — a not-started guard in `workflowReleaseVerdict`.**

```diff
+		if (childStatus.processTerminal.state === "not-started"
+			&& childStatus.processTerminal.runId === step.runId
+			&& typeof childStatus.error === "string"
+			&& childStatus.error) continue;
```

A workflow child that never started but recorded an error must not pin its parent's slot forever.
Note this is expressed through `processTerminal`, which **cyrup does not have** (§D3) — so port the
BEHAVIOUR through cyrup's substitute proof: a child with no `runner_pid` bound by `mark_started`
and a recorded error is a released slot, not a retained one. Add a named test for it alongside
test 11.

### §1.1 files, confirmed to exist at `v0.66.0`

| upstream path | LOC | role |
|---|---|---|
| `src/runs/background/active-async-capacity.ts` | 516 | SUBTASK1–4 |
| `src/runs/background/active-run-index.ts` | 139 | **SUBTASK5** |
| `src/extension/config.ts` | — | `validateCapacityConfig` `:86-98`; `maxActiveAsyncRunsPerSession` validation `:164-168` |
| `src/shared/types.ts` | — | `ActiveAsyncCapacitySnapshot` `:2200-2204`; `SubagentState.activeAsyncCapacity` `:2241` |
| `src/runs/background/process-terminal.ts` | — | **the dependency cyrup does not have** — see §D3 |

### §1.2 the exported surface of `active-async-capacity.ts` @`v0.66.0`

```ts
ACTIVE_ASYNC_CAPACITY_DIR                       :10   path.join(TEMP_ROOT_DIR, "session-active-async-capacity")
DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS         :11   20 * 60 * 1000
MIN_ABANDONED_SLOT_RELEASE_AFTER_MS             :12   5 * 60 * 1000
MAX_ABANDONED_SLOT_RELEASE_AFTER_MS             :13   24 * 60 * 60 * 1000
interface ActiveAsyncCapacityOwner              :15-29
interface ActiveAsyncCapacityHandle             :31-38
interface CapacityOptions          (not exported):40-48
interface ActiveAsyncCapacityReleaseEvidence    :50-56
type ActiveAsyncCapacityReleaseVerdict          :58-61
interface ActiveAsyncCapacityInspection         :63-68
class ActiveAsyncCapacityError                  :70-78   message at :74
resolveMaxActiveAsyncRunsPerSession             :80-83
resolveAbandonedSlotReleaseAfterMs              :85-89
activeAsyncCapacitySessionKey                   :91-93   sha256 hex — see §D4
inspectActiveAsyncCapacityOwner                 :311-336
reconcileActiveAsyncCapacity                    :338-357
getActiveAsyncCapacitySnapshot                  :359-365 (a bare alias of reconcile)
acquireActiveAsyncCapacity                      :454-480
transferActiveAsyncCapacity                     :482-516
```

Internals that MUST be ported because the behaviour lives in them, not in the exports:

```ts
sessionDir/slotDir            :95-101    <root>/<sessionKey>/slot-<n>
parseOwner                    :103-120   the on-disk validator — every field, incl. slot >= 0
readOwner                     :122-128   read owner.json, undefined on ANY error
occupiedSlots                 :130-139   readdir, /^slot-\d+$/ only; ENOENT => []
snapshotFor                   :141-143   { used: occupiedSlots.length, limit: limit ?? 0 }
matchingOwner                 :145-152   token AND runId AND generation must all match
withSlotClaim                 :154-177   O_EXCL "capacity.claim" lockfile, token-checked on release
removeOwnedSlot               :179-194   rename-then-rm, requireUnstarted guard, evidence event
appendAbandonedReleaseEvent   :196-210   best-effort events.jsonl line, never fatal
terminalState                 :212-214   state !== queued|running|paused
runnerReleaseVerdict          :216-236
abandonedRunnerReleaseVerdict :238-263
workflowReleaseVerdict        :265-290
ownerReleaseVerdict           :292-297
capacitySessionDirs           :299-309
createSlot                    :367-380   mkdir(EEXIST => false) THEN write owner.json — fail-closed
handleFor                     :382-452
```

### §1.3 corrected `liveWorkflowRunIds` call sites @`v0.66.0`

The draft listed six; all six exist, two at different lines, and three more were missing.

| draft | actual @`v0.66.0` | shape |
|---|---|---|
| `run-status.ts:487` | `run-status.ts:487` ✓ | `inspectActiveAsyncCapacityOwner(..., { rootDir: deps.activeCapacityRoot, liveWorkflowRunIds: new Set(deps.state?.workflowControllers?.keys() ?? []), abandonedSlotReleaseAfterMs })` |
| `run-status.ts:514` | `run-status.ts:514` ✓ | identical |
| `subagent-executor.ts:628` | **`:633`** | `getActiveAsyncCapacitySnapshot` (the session-state refresh) |
| `subagent-executor.ts:1684` | `:1684` ✓ (`liveWorkflowRunIds` at `:1690`) | `acquireActiveAsyncCapacity`, resume path, gated on `topLevelResume` `:1681` |
| `subagent-executor.ts:1927` | `:1927` ✓ (`:1933`) | `acquireActiveAsyncCapacity`, second resume path |
| `subagent-executor.ts:4988` | **`:4984`** (`:4990`) | `acquireActiveAsyncCapacity({ kind: "workflow" })`, gated on `topLevelAsyncWorkflow` `:4981` |
| — | `:2035`/`:2042` | `transferActiveAsyncCapacity` ↔ `acquire`, **no** `liveWorkflowRunIds` passed |
| — | `:5738` | `workflowCapacity?.reconcile(new Set(...))` in the settlement `finally` |
| — | `:6799` | **the main async-spawn acquire** — the one SUBTASK4 is about; gate at `:6793-6794` |
| — | `extension/index.ts:929`, `:934`; `doctor.ts:189-192` | snapshot refresh + doctor row |

A live workflow's slot is never reclaimed: `workflowReleaseVerdict` `:271`
`if (liveWorkflowRunIds.has(owner.runId)) return { state: "retained", … }`.

---

## §2 — Verified cyrup seam inventory (2026-09-15, every line opened)

| seam | file:line | current shape |
|---|---|---|
| root constants | `background/artifact_roots.rs:27` `ASYNC_SUBDIR="async"`, `:35` `RESULTS_SUBDIR="results"`, `:47` `SCRATCH_SUBDIR="scratch"`, `:64` `SUBSCRIPTIONS_SUBDIR="wait-subscriptions"` | four `const &str`, all cwd-keyed |
| the cwd-keying doc a reader will trust | `background/artifact_roots.rs:276-292` (`# EVERY cyrup instance in a directory resolves these SAME two paths`) | the draft's cite `:281-284` lands inside this block; `identity/mod.rs:10` cites it too |
| `temp_root_dir` | `background/artifact_roots.rs:220`, pure core `:237` | `pub(crate) fn temp_root_dir_from(env: crate::paths::EnvLookup<'_>, os_temp_dir: PathBuf) -> PathBuf` |
| `cwd_key` | `background/artifact_roots.rs:263` | `pub(crate) fn cwd_key(cwd: &Path) -> String` — `DefaultHasher`, `{:016x}` |
| `RunArtifactRoots` | `background/artifact_roots.rs:293-299` | `{ async_root: PathBuf, results_dir: PathBuf }` |
| `run_artifact_roots_in` | `background/artifact_roots.rs:318` | `pub fn (roots: &crate::paths::Roots, cwd: &Path) -> RunArtifactRoots` |
| the **pattern to copy** | `background/artifact_roots.rs:336` | `pub fn wait_subscriptions_dir_in(roots: &crate::paths::Roots, cwd: &Path) -> PathBuf` — SCOPE_11's root, added the same way |
| roots accessor | `paths.rs:305` | `pub fn run_scratch(&self) -> &Path` |
| `IndexSegment` | `identity/path_segment.rs:34` | `pub struct IndexSegment(String)`; `MAX_BYTES=255` `:40`; `encode` `:45`; `encode_bounded` `:58`; `aliases` `:84`; `read_aliases` `:94`; `as_str` **`pub(crate)`** `:101`; `Display` `:106`; portability predicate `:211-219` |
| re-export | `identity/mod.rs:61` | `pub use path_segment::IndexSegment;` |
| `SessionId` | `identity/session_id.rs:41` | `pub struct SessionId(Arc<str>)`; `parse` `:47` (`None` only for `""`, **no trimming**); `parse_opt` `:59`; `as_str` `:66`; deserializes **through** `parse` `:85-90` |
| extension config | `registration/mod.rs:79` | `#[serde(rename_all="camelCase", default)] pub struct SubagentExtensionConfig` |
| the neighbour to add beside | `registration/mod.rs:94` | `pub global_concurrency_limit: u32` (default `20` at `:467`) |
| nested-object precedent | `registration/mod.rs:137` + `:681-687` | `pub model_exclusions: Option<ModelExclusionsConfig>` / `pub struct ModelExclusionsConfig { pub default_ttl_ms: Option<i64> }`, both `#[serde(skip_serializing_if="Option::is_none")]` |
| second nested precedent | `registration/mod.rs:693-705` | `ExtensionChainConfig` → `DynamicFanoutConfig` |
| five-tier resolver | `registration/mod.rs:1032-1038` `InlineConfigOverrides`, `:1060-1066` `EffectiveConfig`, `:1130-1136` the `FieldCandidates{…}.resolve(…)` idiom, `:1157` the struct literal | `global_concurrency_limit` passes `settings: None, agent_frontmatter: None` |
| config-validation precedent | `exec/model_exclusions/store.rs:459` | `pub fn validate_model_exclusions_config(config: Option<&ModelExclusionsConfig>) -> Result<(), String>` — upstream's message verbatim; called from `extension/host/mod.rs:213` |
| **the async spawn path** | `extension/executor/background.rs:375` | `pub async fn spawn_background_steps(&self, cwd: &Path, spec: BackgroundStepsSpec) -> Result<RunId, SubagentError>` |
| its single-step wrapper | `extension/executor/background.rs:46` | `pub async fn spawn_background(&self, request: BackgroundSingleRequest<'_>) -> Result<RunId, SubagentError>` — delegates to the above at `:352-353` |
| depth guard (the gate to sit beside) | `extension/executor/background.rs:398-411` | `let cfg = self.config_snapshot().await;` then `resolve_effective_depth` / `is_blocked` → `SubagentError::DepthExceeded` |
| root derivation + mkdir | `extension/executor/background.rs:454-465` | `resolve_background_storage_roots` → `ensure_accessible_dir` ×3 → `RunPaths::for_run(&async_root, &results_dir, &run_id)` at `:462` |
| where the existing cap is read | `extension/executor/background.rs:514` | `global_concurrency_limit: cfg.global_concurrency_limit as usize` inside the `RunnerConfig` literal `:483` |
| **confirmed spawn** | `extension/executor/background.rs:608-614` | `let pid = spawn_detached_runner_with_command(&resolved_command, &cfg_path, …)?;` — **`pid: u32`, the only start-proof cyrup has** |
| tracking | `extension/executor/background.rs:686-692` then `Ok(run_id)` `:694` | `self.tracker.track(run_id.clone(), run_paths, Some(SystemTime::now())).await` |
| callers of the spawn path | `extension/executor/chain.rs:382`, `extension/executor/control.rs:342`, `extension/tool/routing.rs:1259`, `extension/host/slash.rs:361`, `:825` | five |
| live workflow registry | `extension/executor/mod.rs:171-172` | `workflow_controllers: Arc<std::sync::Mutex<HashMap<RunId, WorkflowController>>>` |
| **`liveWorkflowRunIds`** | `extension/executor/workflow_controllers.rs:125` | `pub(crate) fn live_workflow_run_ids(&self) -> HashSet<RunId>` — *"the `liveWorkflowRunIds` set WORKFLOW_10 feeds to `inspectActiveAsyncCapacityOwner`"* |
| its production writers | `extension/tool/routing.rs:671` register, `:853` settle | `route_workflow_mode` `:523`; workflow run id minted `:618`; status written `:654` |
| current session | `extension/executor/session_state.rs:55` | `pub fn current_session_id(&self) -> Option<String>` (empty filtered) — parse with `SessionId::parse_opt(…as_deref())`, the idiom at `extension/executor/wait_subscriptions.rs:50-56` |
| config snapshot | `extension/executor/mod.rs:511` | `pub async fn config_snapshot(&self) -> SubagentExtensionConfig` |
| `RunStatus` | `background/records.rs:210` | `run_id` `:212`, `session_id: Option<SessionId>` `:230`, `completion_owner_id` `:239`, `mode: RunMode` `:241`, `state: RunState` `:243`, **`pid: Option<u32>` `:251`**, `ended_at: Option<i64>` `:273`, **`last_update: i64` (NOT optional) `:276`**, `steps: Vec<StepStatus>` `:289`, `error: Option<String>` `:324`, `tool_call_id: Option<String>` `:332`, `telemetry: RunTelemetry` `:351` |
| last-activity | `background/telemetry.rs:149` / `:168` | `RunTelemetry.last_activity_at: Option<i64>` |
| terminal predicate | `background/state.rs:134-139` | `RunState::is_terminal()` = `Complete\|Failed\|Stopped` — **`Paused` excluded**, byte-equal to upstream's `terminalState` `:212-214` |
| pid liveness | `background/reconcile.rs:73-92` / `:108` | `pub enum Liveness { Alive, Dead, Unknown }`, `is_possibly_alive()`, `pub fn check_pid_liveness(pid: u32) -> Liveness` — *"Never collapse `Unknown` into `Dead`"* |
| status reader | `background/control.rs:316` | `pub(crate) async fn read_status_file(&Path) -> Result<Option<RunStatus>, SubagentError>` |
| atomic writes | `background/atomic.rs:75` / `:114` / `:148` / `:209` | `write_atomic_json`, `write_atomic_json_creating_parent` (pub(crate)), `write_private_atomic_json` (pub(crate), 0600), `write_private_atomic_json_blocking` |
| terminal index (the sibling SUBTASK5 delegates to) | `background/terminal_run_index/update.rs:25` | `pub async fn update_terminal_run_index(async_dir: &Path, status: &RunStatus) -> std::io::Result<()>` — self-gates on `is_indexed_state` + `session_id` |
| its address arithmetic | `background/terminal_run_index/entry.rs:15`, `:103`, `:108`, `:117`, `:125`, `:143` | `TERMINAL_RUN_INDEX_DIR=".terminal-runs"`, `is_indexed_state` (complement of `Queued\|Running`), `index_root`, `session_index_dir` (ONE `IndexSegment::encode` key, never `read_aliases`), `marker_name`, `marker_path` |
| **`.active-runs` is already reserved** | `background/terminal_run_index/mod.rs:73-75` | `pub fn is_reserved_async_root_entry(name: &str) -> bool { name == TERMINAL_RUN_INDEX_DIR \|\| name == ".active-runs" }` |
| terminal-index call sites (SUBTASK5 re-routes these) | `background/runner_main/finish.rs:365`, `background/reconcile.rs:430`, `:551`, `extension/executor/workflow_detach/mod.rs:439` | four production; `tui/fleet.rs:2313` is a test helper |
| the non-terminal status write | `background/runner_main/entry.rs:271-288` | `advance_state(RunState::Running)` then `write_atomic_json(&run_paths.status, &status)` — the ACTIVE arm's call site |
| full-scan listing the index exists to replace | `background/run_status.rs:669` | `pub async fn list_active_runs(async_root, results_dir, session_id: Option<&str>)` — `read_dir` at `:676`, reserved-entry skip already present at `:692-697` |
| dismissal | `extension/executor/foreground_actions/dismiss.rs:72`, `:177-178` | `control_dismiss`, stamps `display_dismissed_at` then `write_atomic_json` |
| refusal precedent | `error.rs:172` + `extension/tool/mod.rs:353-358` | `SubagentError::SpawnLimitExceeded(String)` with `#[error("{0}")]`, returned as `ToolError::new(limit_notice)` |
| durable-module precedent (SCOPE_11) | `background/wait_subscriptions/manager.rs:228` `WaitSubscriptionDeps`, `:273` the manager, `:284` `new`, `:324` `arm`, `:419` `reconcile` | injected deps struct, `std::sync::Mutex` for synchronous state, no guard across `.await` |
| depth envelope | `spawn/depth.rs:29`, `:92` | `DepthEnvelope { current_depth: u32, max_depth: u32 }`, `is_blocked` |
| module slot | `background/mod.rs:36-51` (alphabetical `pub mod` block), re-exports `:98-103` | add `pub mod active_async_capacity;` and `pub mod active_run_index;` |

---

## SUBTASK1 — a new root, keyed by SESSION not cwd

**Where:** `crates/cyrup-ext-subagents/src/background/artifact_roots.rs`

```ts
export const ACTIVE_ASYNC_CAPACITY_DIR = path.join(TEMP_ROOT_DIR, "session-active-async-capacity");  // :10
export function activeAsyncCapacitySessionKey(sessionId: string): string;                            // :91
```

**This is the first root in the system keyed by session.** Every existing root
(`async_root`, `results_dir`, scratch, chain-runs) is keyed by [`cwd_key`]. Add it beside them and
**say so in the doc** — sweep item **D22** — because a reader who assumes the cwd-keying convention
will place it wrong.

Use `identity::IndexSegment` for the session key; do not write a second encoder.

> ⟡ **AUG — exact shape.** Add a fifth `const` beside `SUBSCRIPTIONS_SUBDIR` (`:64`) and a resolver
> shaped exactly like `wait_subscriptions_dir_in` (`:336`) — that function is SCOPE_11's precedent
> and the one to copy, **except** for its `cwd_key` join:
>
> ```rust
> /// pi `ACTIVE_ASYNC_CAPACITY_DIR` (`active-async-capacity.ts:10` @v0.66.0).
> const CAPACITY_SUBDIR: &str = "session-active-async-capacity";
>
> /// THE ONLY ROOT IN THIS CRATE KEYED BY SESSION RATHER THAN BY `cwd_key`.
> #[must_use]
> pub fn active_async_capacity_root_in(roots: &crate::paths::Roots) -> PathBuf {
>     roots.run_scratch().join(CAPACITY_SUBDIR)
> }
>
> /// `<capacity root>/<IndexSegment(session)>` — pi `sessionDir` (`:95-97`).
> #[must_use]
> pub fn active_async_capacity_session_dir(
>     roots: &crate::paths::Roots,
>     session_id: &crate::identity::SessionId,
> ) -> PathBuf {
>     active_async_capacity_root_in(roots)
>         .join(crate::identity::IndexSegment::encode(session_id.as_str()).as_str())
> }
> ```
>
> The doc comment on `CAPACITY_SUBDIR` must state, in the same voice as `:276-292`'s
> `# EVERY cyrup instance in a directory resolves these SAME two paths`, that **this root is keyed
> by session and by nothing else**: two cyrup instances in the *same* cwd get *different* pools, and
> the *same* session opened from two cwds gets *one* pool. Add the reciprocal one-line pointer to
> the `:276-292` block ("…one exception: `active_async_capacity_root_in`") so the reader who starts
> at the cwd-keying doc learns the exception exists. That is the D22 requirement, discharged.
>
> ⚠ **`IndexSegment::as_str` is `pub(crate)`** (`identity/path_segment.rs:101`). `artifact_roots.rs`
> is in-crate, so this compiles; `Display` (`:106`) is the public alternative if the module ever
> moves. Do not add a `pub` `as_str`.
>
> ⚠ **Writes use `IndexSegment::encode` — ONE key, never `read_aliases`** (`:94`). This mirrors
> `terminal_run_index/entry.rs:112-118`'s stated reason: the owner record is re-verified against its
> own `owner_session_id` field, so the single hashed key is safe as an address. Fanning out over
> aliases here would let one session hold two pools and defeat the whole cap.
>
> **`roots: &crate::paths::Roots`, never an env re-read.** `run_artifact_roots_in` `:311-317`'s doc
> gives the reason ("the optional form put this decision in the callee"). The call site already
> holds `cfg.roots` (`extension/executor/background.rs:455`).
>
> §D4 records where this **deliberately diverges from upstream**.

---

## SUBTASK2 — configuration

**Where:** `crates/cyrup-ext-subagents/src/registration/` (`SubagentExtensionConfig`)

```ts
resolveMaxActiveAsyncRunsPerSession(value): number | undefined              // :80
resolveAbandonedSlotReleaseAfterMs(value): number | false                   // :85
DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS = 20 * 60 * 1000                    // :11
MIN_ABANDONED_SLOT_RELEASE_AFTER_MS     =  5 * 60 * 1000                    // :12
MAX_ABANDONED_SLOT_RELEASE_AFTER_MS     = 24 * 60 * 60 * 1000               // :13
```

cyrup has **no** `max_active_async_runs_per_session` today. Add it next to the existing
`global_concurrency_limit`, plus `capacity.abandonedSlotReleaseAfterMs`. Port the clamping
(`resolveAbandonedSlotReleaseAfterMs` returns `false` to mean "never release", which is **not**
the same as `None`/default — model it as a three-state enum, not an `Option<Duration>`).

> ### ⟡ §D1 — AUG: **THERE IS NO CLAMPING. The draft prescribes a mechanism upstream does not have.**
>
> `resolveAbandonedSlotReleaseAfterMs` @`v0.66.0:85-89`, read in full:
>
> ```ts
> if (value === false) return false;
> if (typeof value !== "number" || !Number.isInteger(value) || value < 1) return DEFAULT_…;
> return value;                                   // ← returned VERBATIM. No min(). No max().
> ```
>
> `MIN_…`/`MAX_…` are referenced in exactly **one** place: `extension/config.ts:86-98`
> `validateCapacityConfig`, which **throws** on an out-of-range value:
>
> ```
> config.capacity.abandonedSlotReleaseAfterMs must be false or an integer from 300000 to 86400000
> ```
>
> So the real behaviour is **reject at the config boundary, substitute the default only for
> garbage**. A clamping port would silently accept `1` and run a 5-minute policy where upstream
> refuses to start — the opposite of fail-closed. The two tests named for clamping are rewritten
> accordingly in §Tests; the *constants*, the *range* and the *`false` distinction* are all kept.
>
> ### ⟡ AUG — the concrete cyrup shape
>
> ```rust
> // registration/mod.rs, beside `model_exclusions` (:137)
> /// SCOPE_9 — pi `ExtensionConfig.maxActiveAsyncRunsPerSession?: number`
> /// (validated `extension/config.ts:164-168` @v0.66.0).
> ///
> /// `None` (key absent) and `Some(0)` BOTH mean unlimited — pi
> /// `resolveMaxActiveAsyncRunsPerSession` (`:80-83`) maps a non-integer, a negative, AND `0` to
> /// `undefined`. Same asymmetry as `max_subagent_spawns_per_session` (`:95-97`), opposite to
> /// `max_subagent_spawns_per_run` (`:98-108`); both are documented at their resolvers.
> #[serde(skip_serializing_if = "Option::is_none")]
> pub max_active_async_runs_per_session: Option<u32>,
>
> /// SCOPE_9 — pi `ExtensionConfig.capacity?: { abandonedSlotReleaseAfterMs?: number | false }`.
> #[serde(skip_serializing_if = "Option::is_none")]
> pub capacity: Option<CapacityConfig>,
> ```
>
> ```rust
> #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
> #[serde(rename_all = "camelCase", default)]
> pub struct CapacityConfig {
>     #[serde(skip_serializing_if = "Option::is_none")]
>     pub abandoned_slot_release_after_ms: Option<AbandonedSlotRelease>,
> }
>
> /// The THREE-STATE value the draft demands. `Option<AbandonedSlotRelease>` is therefore
> /// genuinely three states: `None` = unset (→ DEFAULT), `Some(Never)` = the literal `false`,
> /// `Some(After(ms))` = an explicit threshold. An `Option<Duration>` cannot express `Never`.
> #[derive(Clone, Copy, Debug, PartialEq, Eq)]
> pub enum AbandonedSlotRelease {
>     /// JSON `false` — pi's `value === false` arm (`:86`). NEVER release an abandoned slot.
>     Never,
>     /// JSON integer milliseconds.
>     After(i64),
> }
> ```
>
> `AbandonedSlotRelease` needs a **hand-written** `Deserialize` over an untagged
> `bool | i64` (a `false` literal or an integer; `true` is an error) and a `Serialize` that emits
> `false` or the bare number. `#[serde(untagged)]` on an enum with a unit variant will not produce
> `false` — write the two impls, following `identity/session_id.rs:85-90`'s
> "deserialize **through** the validator" discipline and `terminal_run_index/entry.rs:28-45`'s
> `TerminalIndexVersion` as the in-crate precedent for a hand-written pair.
>
> Two resolvers in `background/active_async_capacity/config.rs`, mirroring
> `exec/model_exclusions/store.rs:459-482`:
>
> ```rust
> pub const DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 20 * 60 * 1000;
> pub const MIN_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 5 * 60 * 1000;
> pub const MAX_ABANDONED_SLOT_RELEASE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
>
> /// pi `resolveMaxActiveAsyncRunsPerSession` (`:80-83`). `0` => `None` (unlimited).
> #[must_use]
> pub fn resolve_max_active_async_runs_per_session(value: Option<u32>) -> Option<u32>;
>
> /// pi `resolveAbandonedSlotReleaseAfterMs` (`:85-89`) — NO clamping (§D1).
> /// `None` and `Some(After(n))` with `n < 1` both yield `After(DEFAULT_…)`.
> #[must_use]
> pub fn resolve_abandoned_slot_release(value: Option<AbandonedSlotRelease>) -> AbandonedSlotRelease;
>
> /// pi `validateCapacityConfig` (`extension/config.ts:86-98`) — the REJECTION the draft called
> /// clamping. Message byte-identical to `:96`.
> ///
> /// # Errors
> /// `config.capacity.abandonedSlotReleaseAfterMs must be false or an integer from {MIN} to {MAX}`
> pub fn validate_capacity_config(config: Option<&CapacityConfig>) -> Result<(), String>;
> ```
>
> `validate_capacity_config` gets a **production caller** at `extension/host/mod.rs:213`, beside the
> existing `apply_model_exclusions_config` call, so it is not dead code and its message actually
> reaches an operator. Do **not** add the pair to `InlineConfigOverrides`/`EffectiveConfig`
> (`registration/mod.rs:1032`, `:1060`): neither knob has an inline-call or frontmatter tier
> upstream, and `global_concurrency_limit` only sits there because it does.

---

## SUBTASK3 — claim, inspect, release

```ts
interface ActiveAsyncCapacityOwner       // :15
interface ActiveAsyncCapacityHandle      // :31
interface ActiveAsyncCapacityReleaseEvidence  // :50
interface ActiveAsyncCapacityInspection  // :63
inspectActiveAsyncCapacityOwner(
    { runId, sessionId, asyncDir },
    { rootDir, liveWorkflowRunIds, abandonedSlotReleaseAfterMs }): ActiveAsyncCapacityInspection  // :311
```

`liveWorkflowRunIds` is `new Set(state.workflowControllers?.keys() ?? [])` at every call site
(`run-status.ts:487`, `:514`; `subagent-executor.ts:628`, `:1684`, `:1927`, `:4988`) — a live
workflow's slot is **never** reclaimed as abandoned. This is why WORKFLOW_6 and SCOPE_8 must land
first: a leaked registry entry withholds capacity forever, a missing one reclaims a live run's slot.

**Layout:** `active_async_capacity/{mod,config,key,claim,inspect,sweep}.rs`.

> ### ⟡ §D3 — AUG: **the release verdict CANNOT be ported as written. Ported verbatim it leaks the whole cap.**
>
> `runnerReleaseVerdict` (`:216-236`) releases a slot on exactly one positive proof:
>
> ```ts
> const proof = readProcessTerminal(owner.asyncDir, { runId, runnerProcessInstanceId });
> return proof?.state === "observed" && … ? { state: "releasable", … }
>                                         : abandonedRunnerReleaseVerdict(status, …);
> ```
>
> and `abandonedRunnerReleaseVerdict` (`:238-263`) **refuses everything that is not `failed`**
> (`:242`: `if (status.state !== "failed") return { state: "retained", … }`).
>
> **cyrup has neither input.** Verified by grep over the whole crate:
> * no `process_terminal` / `ProcessTerminal` / `readProcessTerminal` equivalent — upstream's
>   `src/runs/background/process-terminal.ts` (`ProcessTerminalCandidate`, `writers`,
>   `expectedWriters`, `revivalLeaseToken`) has **no** cyrup port, and porting it is a
>   multi-week task of its own;
> * no `runner_process_instance_id` anywhere — `RunStatus` (`background/records.rs:210-351`) has
>   no such field and nothing mints one.
>
> Consequence of a verbatim port: a run that finishes `Complete` has no observed proof and is not
> `failed`, so its verdict is `retained` **forever**. After `limit` successful background runs the
> session can never spawn again. That is strictly worse than having no cap at all.
>
> **The mechanism that CAN work — substitute cyrup's own start-proof, the runner pid.**
>
> 1. `ActiveAsyncCapacityOwner` carries `runner_pid: Option<u32>` where upstream carries
>    `runnerProcessInstanceId: Option<string>`, **and keeps `runner_started_at: Option<i64>`
>    unchanged**. The pid is real, cyrup already has it, and it is the value
>    `background/reconcile.rs:108`'s probe consumes.
> 2. `handle.mark_started(pid)` is called with the pid
>    `spawn_detached_runner_with_command` returns (`extension/executor/background.rs:608-614`) —
>    the exact position of upstream's `markStarted` (`async-execution.ts:1409`, `:1422`).
>    `handle.rollback()` is called when that call, or the `runner-config.json` write above it,
>    fails — upstream's `:1419`.
> 3. `runner_release_verdict` becomes, in upstream's own order, with the proof rung swapped:
>
> | rung | upstream `:216-236` | cyrup |
> |---|---|---|
> | no status | `retained "status file is missing or unreadable"` | identical (`read_status_file` → `Ok(None)` **or** `Err`) |
> | no start proof | `!owner.runnerProcessInstanceId` ⇒ retained | `owner.runner_pid.is_none() && owner.runner_started_at.is_none()` ⇒ `retained "runner process identity has not been recorded"` |
> | session mismatch | `:219` | `status.session_id.as_ref() != Some(&owner.owner_session_id)` — **typed**, so the upstream `?? "unknown"` interpolation is `Option::map` |
> | run mismatch | `:220` | `status.run_id != owner.run_id` |
> | not terminal | `:221` | `!status.state.is_terminal()` (`background/state.rs:134`) — **byte-equal semantics**, `Paused` retained by both |
> | early-failure carve-out | `:222-226` (`processTerminal.state === "not-started"` + `status.error`) | **UNREPRESENTABLE, drop it** — and record the drop in the module doc. Its effect is subsumed by the pid rung below: a run that failed before child startup has a dead runner pid. |
> | **the proof** | observed process-terminal ⇒ releasable | `check_pid_liveness(pid) == Liveness::Dead` ⇒ `releasable "runner pid <n> is confirmed gone and the run is terminal"` |
> | otherwise | abandoned ladder | abandoned ladder, below |
>
> 4. `abandoned_runner_release_verdict` keeps every upstream rung **except** that the pid probe has
>    already run, so it becomes the `Alive`/`Unknown` tail: policy disabled (`Never`) ⇒ retained
>    (`:241`); `status.state != Failed` ⇒ retained (`:242`); `Liveness::Unknown` ⇒ retained with the
>    liveness word (`:245`, and `Liveness::is_possibly_alive` at `reconcile.rs:89` is the predicate —
>    **`Unknown` MUST NOT be read as dead**); last-activity age `<= threshold` ⇒ retained (`:250`);
>    otherwise `releasable` with the evidence record (`:251-262`).
>    ⚠ Upstream's "last activity timestamp is missing or invalid" rung (`:247`) is **unreachable in
>    cyrup**: the ladder is `status.telemetry.last_activity_at` (`telemetry.rs:168`, `Option<i64>`)
>    `.unwrap_or(status.last_update)`, and `last_update` is a non-optional `i64`
>    (`records.rs:276`). Collapse it to two rungs and say so, exactly as
>    `terminal_run_index/update.rs:36-39` already does for the same `last_update` non-optionality.
> 5. `workflow_release_verdict` (`:265-290`) ports **unchanged in structure**, with two swaps: the
>    controller check is `live_workflow_run_ids.contains(&owner.run_id)` (`:271`), and the per-child
>    process-terminal proof (`:282-287`) becomes the same pid probe over each async child's own
>    `RunStatus.pid`. `status.mode != RunMode::Workflow` ⇒ retained (`:269`). The `step.async`
>    classification (`:274`) maps to `StepStatus` — confirm the field exists before relying on it
>    (see §Open questions Q5).
>
> ### ⟡ AUG — the record, verbatim field-for-field
>
> ```rust
> /// pi `ActiveAsyncCapacityOwner` (`active-async-capacity.ts:15-29` @v0.66.0).
> #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
> #[serde(rename_all = "camelCase")]
> pub struct ActiveAsyncCapacityOwner {
>     pub version: CapacityOwnerVersion,          // :16  literal 1 — copy TerminalIndexVersion
>     pub reservation_token: String,              // :17  non-empty
>     pub owner_session_id: SessionId,            // :18  non-empty BY TYPE
>     pub owner_session_key: String,              // :19  == IndexSegment::encode(owner_session_id)
>     pub slot: u32,                              // :20  >= 0 by type
>     pub run_id: RunId,                          // :21
>     #[serde(skip_serializing_if = "Option::is_none")]
>     pub source_run_id: Option<RunId>,           // :22
>     pub generation: u32,                        // :23
>     pub kind: ActiveAsyncCapacityKind,          // :24  Runner | Workflow
>     pub async_dir: PathBuf,                     // :25  non-empty
>     pub reserved_at: i64,                       // :26
>     #[serde(skip_serializing_if = "Option::is_none")]
>     pub runner_pid: Option<u32>,                // :27  ← §D3 substitution
>     #[serde(skip_serializing_if = "Option::is_none")]
>     pub runner_started_at: Option<i64>,         // :28
> }
> ```
>
> `parseOwner` (`:103-120`) becomes serde + the two cross-field checks serde cannot express, applied
> after deserialization: `owner_session_key == IndexSegment::encode(owner_session_id).as_str()` and
> (at the `reconcile` call site only, `:351`) `dir.file_name() == format!("slot-{}", owner.slot)`.
> A record failing either is **skipped, never deleted** — upstream `:348-351` `continue`s.
>
> ### ⟡ §D2 — AUG: nothing "releases on terminal". Release is reconciliation.
>
> There is no `release()` on the handle (`:31-38`: `markStarted`, `markWorkflowStarted`, `rollback`,
> `rollbackBeforeRunnerProceed`, `reconcile` — **and that is all**). A slot is removed only by
> `reconcileActiveAsyncCapacity` (`:338-357`), which every `acquire` runs first (`:460`). So a
> terminal run's slot is freed by **the next spawn attempt in that session**, or by an explicit
> `reconcile()`/snapshot read. SUBTASK4's "Release on terminal" is preserved as an *outcome* — after
> a run reaches terminal, the next claim must see the slot free — not as a call. The named test
> `a_slot_is_released_when_the_run_reaches_terminal` is written that way.
>
> ### ⟡ AUG — concurrency primitive, and what must NOT change
>
> `withSlotClaim` (`:154-177`) is an **`O_EXCL` lockfile**, not an advisory lock:
> `fs.openSync(claimPath, "wx", 0o600)`; `EEXIST` **or** `ENOENT` ⇒ `{ acquired: false }` (the
> `ENOENT` arm matters — the slot dir can vanish under a concurrent release); every other error
> propagates; the `finally` deletes the claim **only if its contents still match this caller's
> token**. In Rust: `tokio::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)`.
> `createSlot` (`:367-380`) is `mkdir(slot-N)` with `EEXIST ⇒ false` **before** writing
> `owner.json`, and upstream's comment at `:376-377` is load-bearing: *"If owner persistence fails,
> the corrupt occupied directory remains and fails closed instead of becoming available to another
> admission."* Do not "improve" that into a cleanup.
>
> `removeOwnedSlot` (`:179-194`) **renames then removes** — `rename(dir, ".slot-N.released-<uuid>")`
> then `rm -rf` — so a concurrent reader never sees a half-deleted slot. Keep both steps.
>
> Async-vs-sync: make the module `async` throughout (`tokio::fs`), matching
> `background/wait_subscriptions/` and `terminal_run_index/update.rs:25`. No `std::sync::Mutex`
> guard may cross an `.await`; `live_workflow_run_ids()` returns an owned `HashSet<RunId>` precisely
> so the executor lock is released before the capacity call (`workflow_controllers.rs:97-100`
> already spells out that discipline).
>
> ### ⟡ AUG — `transferActiveAsyncCapacity` (`:482-516`) is IN SCOPE and has NO cyrup caller yet
>
> It is the resume path's slot hand-off (`subagent-executor.ts:2034-2041`). cyrup's resume surface
> is `extension/executor/control.rs:342`'s `spawn_background_steps` call. Port the function (its
> `generation + 1`, its `source_run_id` breadcrumb and its "not transferable while queued/running"
> hard error at `:495-496` are what make `inspect`'s `relation: "source"` arm `:324-331` mean
> anything), and wire it at that call site if the resume shape is recognisable there; if it is not,
> see Q4.
>
> ### ⟡ AUG — `ActiveAsyncCapacityKind::Workflow` has no acquire site, and that is FINE
>
> Upstream's workflow acquire (`:4984`) is gated on `topLevelAsyncWorkflow` — an **async** workflow.
> cyrup's `route_workflow_mode` (`extension/tool/routing.rs:523`) refuses the async shape
> (`:598-599`: *"`background` is `false` — the async shape was refused above"*), so no cyrup path
> creates a `kind: Workflow` slot today. **Do not add one.** The variant is still reachable in
> production through `serde` (a slot written by a future build or another cyrup version), and
> `workflow_release_verdict` is reachable from `reconcile` on every acquire, so **no
> `#[allow(dead_code)]` is needed anywhere** — which is the debt `SCOPE_8.md`'s open question 6
> complains about. Verify that claim by building: if clippy reports the variant unconstructed, the
> serde path is not being seen and Q5 must be answered before landing.

---

## SUBTASK4 — enforce at the async spawn path

**Where:** `extension/executor/background.rs` (where `global_concurrency_limit` is already read)

**Change:** before spawning an async run, claim a capacity slot for the current session; refuse
with upstream's message when the session is at its cap. Release on terminal.

> ### ⟡ AUG — exact position, exact gate, exact refusal
>
> **Position:** inside `spawn_background_steps` (`:375`), **after** the depth guard (`:405-411`) and
> **after** the roots are derived and created (`:454-465`, because the owner record stores
> `async_dir = run_paths.run_dir`), and **before** the `RunnerConfig` literal at `:483`. That is
> upstream's own order: `:6793-6814` sits between the depth check and the fan-out budget.
> `spawn_background` (`:46`) needs **no** change — it delegates at `:352-353`, so charging in the
> callee covers both entry points and cannot be double-charged. All five callers
> (`chain.rs:382`, `control.rs:342`, `routing.rs:1259`, `slash.rs:361`, `:825`) inherit the gate.
>
> **The eligibility gate.** Upstream `:6793`:
> `depth === 0 && !inheritedNestedRouteValue && !effectiveParams.workflowParentRunId`. In cyrup:
>
> | term | cyrup | already in hand at the call site? |
> |---|---|---|
> | `depth === 0` | `resolve_effective_depth(cfg.max_subagent_depth).current_depth == 0` | **yes** — `depth` is bound at `:405` |
> | `!inheritedNestedRoute` | `inherited_nested_route.is_none()` | **yes** — bound at `:436-439`, but **currently BELOW the intended insertion point**; hoist it above the claim (it is a pure env read, `crate::spawn::nested_events::resolve_inherited_nested_route_from_env`) |
> | `!workflowParentRunId` | **NOT AVAILABLE.** `BackgroundStepsSpec` (`requests.rs:349-418`) carries no workflow-parent marker; `parent_workflow_run_id` exists only on the FOREGROUND request (`requests.rs:184`) | **no** — see Q1 |
>
> **The claim:**
>
> ```rust
> let capacity = match crate::identity::SessionId::parse_opt(
>     self.current_session_id().as_deref(),
> ) {
>     Some(session) if top_level_async => {
>         crate::background::active_async_capacity::acquire(
>             AcquireInput {
>                 session_id: &session,
>                 limit: resolve_max_active_async_runs_per_session(
>                     cfg.max_active_async_runs_per_session,
>                 ),
>                 run_id: &run_id,
>                 kind: ActiveAsyncCapacityKind::Runner,
>                 async_dir: &run_paths.run_dir,
>             },
>             CapacityOptions {
>                 roots: &cfg.roots,
>                 live_workflow_run_ids: self.live_workflow_run_ids(),
>                 abandoned_slot_release: resolve_abandoned_slot_release(
>                     cfg.capacity.as_ref().and_then(|c| c.abandoned_slot_release_after_ms),
>                 ),
>                 now: crate::time::now_epoch_millis,
>                 pid_liveness: crate::background::reconcile::check_pid_liveness,
>             },
>         )
>         .await?
>     }
>     _ => None,
> };
> ```
>
> `acquire` returns `Ok(None)` when the limit is `None` (upstream `:458`
> `if (input.limit === undefined) return undefined`) — **unlimited means no slot is written at
> all**, so an unconfigured install touches the filesystem zero extra times.
>
> **The refusal.** New variant, modelled exactly on `SpawnLimitExceeded` (`error.rs:172`):
>
> ```rust
> /// SCOPE_9 — pi `ActiveAsyncCapacityError` (`active-async-capacity.ts:70-78` @v0.66.0),
> /// carrying its message verbatim:
> /// `Active async run capacity exhausted: {used}/{limit} used.`
> #[error("{0}")]
> ActiveAsyncCapacityExhausted(String),
> ```
>
> The message is formatted at the throw site from the reconciled snapshot, byte-for-byte with
> `:74`, **including the trailing period**. It surfaces as `ToolError::new(...)` the same way
> `extension/tool/mod.rs:353-358` surfaces the spawn-budget notice, so the refusal reads identically
> on the tool and slash surfaces.
>
> **Rollback.** Between the claim and `Ok(run_id)` (`:694`) there are three fallible steps —
> the `runner-config.json` write, `spawn_detached_runner_with_command` (`:608`), and the
> `resolve_spawn_command` above it. **Every early return after the claim must `rollback()` first**
> (upstream `:1419`, `:2032`, `:6824`). A held-but-never-spawned slot is a permanent leak that no
> reconcile can clear, because there is no status file for the verdict to read. Do this with an
> explicit `if let Err(..) = … { capacity.rollback().await; return Err(..); }` at each site — not a
> `Drop` impl, because `rollback` is `async`.
>
> **`mark_started`.** Immediately after `:608-614` succeeds:
> `capacity.mark_started(pid).await` (§D3). It binds `runner_pid` + `runner_started_at`, which is
> what promotes the slot from "rollbackable reservation" to "real run" — `removeOwnedSlot`'s
> `requireUnstarted` guard (`:180`, `:183`) is exactly this distinction.
>
> **Snapshot surfacing (upstream `:6806`, `extension/index.ts:929-935`, `doctor.ts:189-192`).**
> cyrup has no `SubagentState.activeAsyncCapacity` field. Adding one is **NOT in scope**; instead
> expose `pub async fn snapshot(session, limit, options) -> ActiveAsyncCapacitySnapshot`
> (`{ used: u32, limit: u32 }`, `types.ts:2200-2204`, `limit: 0` meaning disabled) and add **one**
> row to `registration/doctor.rs` mirroring `doctor.ts:189-192`. That is the operator-visible half
> and it costs no new state.

---

## SUBTASK5 — the active-run index (NEW, 2026-09-15; this task owns it)

**Where:** `crates/cyrup-ext-subagents/src/background/active_run_index.rs`
**Upstream:** `v0.66.0:src/runs/background/active-run-index.ts` (139 LOC, whole file)

`SCOPE_8.md:742-747` deferred this here in as many words; `background/terminal_run_index/mod.rs:73-75`
already reserves the literal `".active-runs"` against it. It is the **write-side twin** of the
terminal index cyrup already has, and `updateActiveRunIndex` is the single function every status
writer in the crate is supposed to call instead of `update_terminal_run_index` directly.

### §5.1 the surface, verbatim

```ts
ACTIVE_RUN_INDEX_DIR = ".active-runs"                       :9    ← ALREADY the reserved name
DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS = 24 * 60 * 60 * 1000 :10
TOOL_CALL_INDEX_DIR = "tool-calls"            (not exported):12
indexDir / toolCallIndexDir / toolCallIndexPath / markerPath:14-28
removeEmptyAncestors                                        :30-40
isActiveAsyncState(state) => queued | running               :42-44
releaseToolCallAliases                                      :46-68
releaseActiveRunIndex(asyncDir)                             :70-77
updateActiveRunIndex(asyncDir, state, toolCallId?, { retryCapacityErrors? }) :79-107
activeRunMarkerAgeMs(asyncDir, now?)                        :109-116
readActiveRunIndex(asyncDirRoot)                            :118-127
readActiveRunToolCallIndex(asyncDirRoot, toolCallId)        :129-138
```

`updateActiveRunIndex` is a **two-armed router** (`:81-106`):
* `isActiveAsyncState(state)` ⇒ `mkdir -p` + touch `<async_root>/.active-runs/<runDir>`, plus a
  best-effort alias `<async_root>/.active-runs/tool-calls/<enc(toolCallId)>/<runDir>` whose failure
  is logged and swallowed (`:89-93` — *"the authoritative active-run marker must keep the launch
  discoverable even when alias indexing fails"*); **return**.
* otherwise ⇒ `releaseActiveRunIndex` (unlink marker + every alias, pruning empty ancestors), then
  re-read `status.json` and, **only if the on-disk state still equals the state we were asked to
  publish** (`:99`), call `updateTerminalRunIndex`. Its failure is logged, **except** a storage-
  capacity error when `retryCapacityErrors` is set, which is rethrown (`:103`).

### §5.2 the cyrup shape

```rust
/// pi `ACTIVE_RUN_INDEX_DIR` (`active-run-index.ts:9`). Already reserved at
/// `terminal_run_index/mod.rs:74`.
pub const ACTIVE_RUN_INDEX_DIR: &str = ".active-runs";
pub const DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS: i64 = 24 * 60 * 60 * 1000;

#[must_use] pub const fn is_active_async_state(state: RunState) -> bool;   // Queued | Running —
                                                                          // the EXACT complement of
                                                                          // terminal_run_index::entry::is_indexed_state (:103)
pub async fn update_active_run_index(async_dir: &Path, status: &RunStatus) -> std::io::Result<()>;
pub async fn release_active_run_index(async_dir: &Path) -> std::io::Result<()>;
pub async fn active_run_marker_age_ms(async_dir: &Path, now: i64) -> Option<i64>;
pub async fn read_active_run_index(async_root: &Path) -> Option<Vec<String>>;
pub async fn read_active_run_tool_call_index(async_root: &Path, tool_call_id: &str) -> Vec<String>;
```

⟡ **Take `&RunStatus`, not `(state, tool_call_id)`.** Upstream passes the two loose values because
its `AsyncStatus` is a bag; cyrup already threads a typed `&RunStatus` to `update_terminal_run_index`
(`terminal_run_index/update.rs:25-27`), it carries both `state` (`records.rs:243`) and
`tool_call_id` (`records.rs:332`), and the delegating arm needs the whole record anyway. The one
caller that has a state but no record — `async-status.ts:513`/`:561`'s "synthesize `failed` for an
unreadable run" — is handled by a second, explicit entry point
`pub async fn mark_active_run_failed(async_dir: &Path)`.

⟡ **Alias-segment encoding:** upstream uses `encodeIndexSegment(toolCallId)` (`:19`) — so
`IndexSegment::encode`, the same single-key discipline as `session_index_dir`
(`terminal_run_index/entry.rs:112-118`). Not `read_aliases`.

⟡ **The `retryCapacityErrors` rung (`:103`) has no cyrup analog** — there is no
`isStorageCapacityError` in the crate. Port the function **without** the flag and record the
omission in the module doc; a `std::io::Result` that the caller already logs-and-continues is the
same outcome for every error class cyrup can distinguish. Do not invent an errno classifier here.

### §5.3 the call sites to re-route (every one verified)

| upstream | cyrup, today | after |
|---|---|---|
| `subagent-runner.ts:2137` (`retryCapacityErrors: true`) | `background/runner_main/finish.rs:365` calls `update_terminal_run_index` directly | call `update_active_run_index` — its terminal arm delegates. The comment at `finish.rs:361-364` **already says** *"reached from `updateActiveRunIndex`'s terminal arm, `active-run-index.ts:100-108`"*: this closes that loop. |
| — (the ACTIVE arm) | `background/runner_main/entry.rs:271-288` writes the first `Running` status | add `update_active_run_index(&run_paths.run_dir, &status)` right after `:288`'s write — best-effort, logged |
| `stale-run-reconciler.ts:286` | `background/reconcile.rs:430` | re-route |
| `stale-run-reconciler.ts:382` | `background/reconcile.rs:551` | re-route |
| `workflow-detach-reconcile.ts:255` | `extension/executor/workflow_detach/mod.rs:439` | re-route — this is precisely the line `SCOPE_8.md:1201` flagged and was forbidden to change |
| `async-dismiss-action.ts:77` (`updateActiveRunIndex(asyncDir, "complete")`) | `extension/executor/foreground_actions/dismiss.rs:177-178` | add the release after the `display_dismissed_at` write |
| `async-status.ts:507-514`, `:561`, `:564-566` | `background/run_status.rs:669-738` full `read_dir` | **reader wiring is OPTIONAL in this task** — see Q6. The 24 h `DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS` rung (`:566`) belongs with it. |
| `tui/fleet.rs:2313` | test helper only | leave |

⟡ Every one of these is **best-effort**: `terminal_run_index/update.rs:20-24` states the contract
(*"an advisory index that failed to write must never fail the run that was finishing"*) and
`finish.rs:365-376` is the shape to copy — `if let Err(err) = … { tracing::warn!(…); }`.

---

## Tests

| test | pins |
|---|---|
| `two_sessions_each_get_their_own_cap` | **the core property** — session A at its cap does not block session B |
| `a_slot_is_released_when_the_run_reaches_terminal` | lifecycle |
| `an_abandoned_slot_releases_after_the_configured_delay` | `DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS`, injected clock |
| `a_live_workflows_slot_is_never_reclaimed` | `liveWorkflowRunIds` — the WORKFLOW_6 dependency |
| `release_after_ms_clamps_below_the_minimum` | `MIN_…` = 5 min |
| `release_after_ms_clamps_above_the_maximum` | `MAX_…` = 24 h |
| `release_after_false_means_never_release` | the three-state distinction from "unset" |
| `no_configured_cap_means_unlimited` | `resolveMaxActiveAsyncRunsPerSession` → `undefined` |
| `the_capacity_root_is_keyed_by_session_not_cwd` | SUBTASK1 — two cwds, one session, one slot dir |
| `a_session_id_that_is_a_path_produces_one_component` | `IndexSegment` reuse |

> ### ⟡ AUG — the ten above, made executable (fail-before / pass-after), plus the gaps they leave
>
> Unit tests live in-module (`#[cfg(test)] mod tests` with the crate's standard
> `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
> prelude, e.g. `terminal_run_index/entry.rs:153-161`). All ten **fail to compile** today (no
> module), which is the fail-before.
>
> 1. `two_sessions_each_get_their_own_cap` — `limit = 1`. `acquire(session_a)` OK; a second
>    `acquire(session_a)` ⇒ `Err(ActiveAsyncCapacityExhausted)` whose message is exactly
>    `Active async run capacity exhausted: 1/1 used.`; `acquire(session_b)` ⇒ **OK**. Assert both
>    pool dirs exist and are siblings.
> 2. `a_slot_is_released_when_the_run_reaches_terminal` — §D2: acquire, `mark_started(pid)` with a
>    **genuinely reaped pid** (the `reconcile.rs:876-878` idiom), write a `Complete` `status.json`
>    at `owner.async_dir`, then assert the **next** `acquire` in the same session succeeds and
>    `snapshot().used == 1`. There is no `release()` to call — asserting one exists is the failure
>    mode this test prevents.
> 3. `an_abandoned_slot_releases_after_the_configured_delay` — `state = Failed`, dead pid,
>    `telemetry.last_activity_at = now - (DEFAULT + 1)`, injected `now`. Releasable. Assert the
>    `subagent.capacity.released` line landed in `<async_dir>/events.jsonl` with `releasedBy:
>    "abandoned-timeout"`, `processProof: "unknown"`, `runnerPid: "gone"` (`:200-206`, `:251-257`).
>    Sibling: `..._not_before` at `DEFAULT - 1` ⇒ retained.
> 4. `a_live_workflows_slot_is_never_reclaimed` — a `kind: Workflow` owner whose run id IS in
>    `live_workflow_run_ids`, with an otherwise fully releasable terminal status ⇒
>    `retained "workflow controller is still live"` (`:271`). Removing the id from the set flips it
>    to releasable — the second half is what proves the set is actually consulted.
> 5. `release_after_ms_clamps_below_the_minimum` → **RENAME to
>    `release_after_ms_below_the_minimum_is_refused_by_config_validation`** (§D1): `CapacityConfig {
>    abandoned_slot_release_after_ms: Some(After(MIN - 1)) }` ⇒ `validate_capacity_config` returns
>    `Err` with upstream's `:96` message; and `resolve_abandoned_slot_release` on the same value
>    returns `After(MIN - 1)` **unchanged**, proving no clamp exists. A clamping implementation fails
>    the second assertion.
> 6. `release_after_ms_clamps_above_the_maximum` → **RENAME to
>    `release_after_ms_above_the_maximum_is_refused_by_config_validation`** — same, at `MAX + 1`.
>    Add `release_after_ms_at_the_minimum_and_maximum_are_accepted` (inclusive bounds, `:94-95`).
> 7. `release_after_false_means_never_release` — three-way: `Some(Never)` ⇒ an otherwise fully
>    abandoned slot is `retained "…abandoned-timeout policy is disabled"` (`:241`); `None` ⇒ released
>    on the DEFAULT ladder; `Some(After(n))` ⇒ released on `n`. **All three in one test**, because
>    the bug it guards is the two-state collapse.
> 8. `no_configured_cap_means_unlimited` — `None` **and** `Some(0)` both ⇒
>    `resolve_max_active_async_runs_per_session` is `None` ⇒ `acquire` returns `Ok(None)` ⇒ assert
>    **the capacity root does not exist on disk at all** after 10 spawns.
> 9. `the_capacity_root_is_keyed_by_session_not_cwd` — one `SessionId`, two `cwd`s, **one**
>    `Roots`: `active_async_capacity_session_dir` is equal for both, and the pool holds one slot
>    after one acquire. The inverse, `two_sessions_in_one_cwd_get_two_pools`, is test 1.
> 10. `a_session_id_that_is_a_path_produces_one_component` — `SessionId::parse("/home/u/.cyrup/sessions/x.jsonl")`.
>     Assert the pool dir's `file_name()` is `Some(_)` and its parent is exactly
>     `active_async_capacity_root_in(&roots)` — i.e. depth 1, no traversal. Note
>     `is_portable_segment` (`path_segment.rs:211-219`) rejects a trailing extension, so this
>     specific id takes the `~sha256-` branch: assert the prefix, which is the on-disk-format claim.
>
> **Additional named tests this aug requires** (the draft's ten do not cover §D3, SUBTASK4's
> rollback, or SUBTASK5 at all):
>
> 11. `a_completed_run_with_a_dead_runner_pid_releases_its_slot` — **the §D3 regression test.**
>     `state = Complete`, `runner_pid` = reaped pid ⇒ releasable. A verbatim upstream port makes this
>     `retained` forever; this test is the whole reason §D3 exists.
> 12. `an_unknown_pid_liveness_never_releases_a_slot` — injected probe returns `Liveness::Unknown`
>     with `state = Failed` and an aged-out `last_activity_at` ⇒ retained
>     (`reconcile.rs:68-71`'s standing rule).
> 13. `a_paused_run_keeps_its_slot` — `RunState::Paused` ⇒ retained, both here and upstream
>     (`:213`, `state.rs:134-139`).
> 14. `a_slot_whose_owner_session_key_disagrees_with_its_directory_is_skipped_not_deleted` —
>     `:348-351`.
> 15. `a_corrupt_owner_json_keeps_the_slot_occupied` — `createSlot`'s fail-closed comment
>     (`:376-377`): an unparseable `owner.json` must **not** free the slot.
> 16. `two_concurrent_acquires_in_one_session_never_share_a_slot` — spawn N tasks against
>     `limit = N`; assert N distinct `slot-<i>` dirs and N distinct reservation tokens.
> 17. `a_failed_detached_spawn_rolls_the_slot_back` — drive `spawn_background_steps` with a
>     `cfg.spawn_command` that cannot exec (`registration/mod.rs:186-187`'s `#[serde(skip)]` override
>     exists for exactly this); assert the pool is empty afterwards. **The leak test.**
> 18. `a_spawn_with_no_session_id_claims_no_slot` — headless `current_session_id() == None` ⇒ spawn
>     succeeds, root absent (pins Q2's answer, whichever it is).
> 19. `the_refusal_message_is_upstreams_verbatim` — byte-compare against
>     `Active async run capacity exhausted: 2/2 used.`
> 20. `update_active_run_index_writes_a_marker_for_a_running_run` — SUBTASK5, ACTIVE arm; marker at
>     `<async_root>/.active-runs/<run dir>`.
> 21. `update_active_run_index_releases_and_delegates_to_the_terminal_index_on_terminal` —
>     SUBTASK5: the active marker is gone **and** a `.terminal-runs/<enc(session)>/…` marker exists.
> 22. `a_tool_call_alias_is_written_and_released_with_its_marker` — `:84-94`, `:46-68`, including
>     empty-ancestor pruning (`:30-40`).
> 23. `an_alias_write_failure_does_not_lose_the_authoritative_marker` — `:89-93`.
> 24. `the_terminal_delegation_is_skipped_when_the_on_disk_state_has_moved_on` — `:99`'s
>     `status.state === state` guard.
> 25. `the_active_index_dir_is_never_listed_as_a_run` — `list_active_runs` over a root containing
>     `.active-runs` (the `is_reserved_async_root_entry` `mod.rs:73-75` contract, now with a real
>     directory behind it).
>
> **Integration:** one `cyrup-it`-style test under the crate's existing `CYRUP_HOME`-sandboxed
> convention (`artifact_roots.rs:210-215` names the 19 tests that use it) driving
> `spawn_background_steps` twice at `limit = 1` and asserting the second returns
> `SubagentError::ActiveAsyncCapacityExhausted`.

---

## Benchmarks

None. The claim is one small file write per async spawn — negligible beside process spawn itself,
which the existing spawn path already dominates.

> ⟡ **AUG — still none, and now quantified.** The claim is `read_dir` of one pool dir (≤ `limit`
> entries) + `read` of ≤ `limit` `owner.json` files + one `mkdir` + one `create_new` + one small
> write, against a `fork`/`exec` of a full agent binary. Note that `acquire` runs a **full
> reconcile** first (`:460`), so the cost is O(limit) status reads, not O(1) — still nothing beside
> a spawn, but say so rather than implying a single write.

---

## Definition of done

- The async cap is per session: one instance exhausting its cap does not affect another.
- A live workflow's slot is never reclaimed; an abandoned one releases on the clamped ladder.
- `false` (never release) is distinguishable from unset.
- The capacity root is keyed by session, documented as the first such root (D22).
- Config surfaces `maxActiveAsyncRunsPerSession` and `capacity.abandonedSlotReleaseAfterMs`.
- Tests pass; workspace 0 failed; clippy exit 0.

> ### ⟡ AUG — checkable by reading code and running named tests
>
> 1. `background/artifact_roots.rs` exports `active_async_capacity_root_in` /
>    `active_async_capacity_session_dir`, hanging off `Roots::run_scratch()` with **no `cwd_key`
>    join**, and the `:276-292` doc block carries the reciprocal "one exception" pointer. Pinned by
>    tests 9 and 10. *(the "clamped ladder" phrase in the bullet above is superseded by §D1 —
>    the ladder is validated-then-defaulted, never clamped.)*
> 2. `registration::SubagentExtensionConfig` has `max_active_async_runs_per_session: Option<u32>` and
>    `capacity: Option<CapacityConfig>`; `AbandonedSlotRelease` is a **three-state** enum with
>    hand-written serde that round-trips JSON `false`. `validate_capacity_config` has a production
>    caller at `extension/host/mod.rs:213`. Pinned by tests 5–8.
> 3. `background/active_async_capacity/{mod,config,key,claim,inspect,sweep}.rs` exists; `acquire`,
>    `transfer`, `inspect`, `reconcile`, `snapshot` are public; the handle exposes `mark_started`,
>    `rollback`, `reconcile`. `grep -c "allow(dead_code)" background/active_async_capacity/` is `0`.
> 4. `extension/executor/background.rs` claims between the depth guard and the `RunnerConfig`
>    literal, calls `mark_started(pid)` after `:608-614`, and **every** error path between the claim
>    and `Ok(run_id)` rolls back. Pinned by tests 17 and 19.
> 5. `background/active_run_index.rs` exists and is the **only** module in the crate that calls
>    `update_terminal_run_index` from production code:
>    `grep -rn "update_terminal_run_index" --include=*.rs crates/cyrup-ext-subagents/src | grep -v "terminal_run_index/\|active_run_index.rs\|mod tests"` is **empty**. Pinned by tests 20–25.
> 6. `cargo nextest run -p cyrup-ext-subagents` — 0 failed; every test 1–25 present and passing.
> 7. `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
> 8. `cargo fmt --check` exits 0.

---

## Open questions — the implementor must NOT silently decide these

1. **The `workflowParentRunId` term of the top-level gate has no cyrup input.**
   `BackgroundStepsSpec` (`requests.rs:349-418`) carries no workflow-parent marker, while
   `ForegroundSingleRequest` does (`:184`). Add `parent_workflow_run_id: Option<RunId>` to
   `BackgroundStepsSpec` and thread it from all five callers, or accept that a workflow's async
   child would charge the session cap (which upstream deliberately exempts)? Threading it touches
   five files; not threading it changes behaviour.
2. **No session id ⇒ no cap, or refuse?** `current_session_id()` is `Option<String>`
   (`session_state.rs:55`); `SessionId` cannot be constructed from `None`. Upstream asserts
   non-null (`:2036` `state.currentSessionId!`). The spec above assumes **skip the gate** (a run
   with no session cannot be partitioned), pinned by test 18 — confirm, because the alternative
   (refuse every headless async spawn) is a visible behaviour change.
3. **Does the same session in two different `cwd`s legitimately share one pool?** That is what
   session-keying means and test 9 asserts it, but it also means a single session's runs across two
   projects compete for one cap. Upstream has one flat `DIRS.async`, so the question does not arise
   there. Confirm this is intended before it becomes an on-disk format.
4. **Is there a resume path to wire `transfer` to?** Upstream transfers at
   `subagent-executor.ts:2034-2041` when `target.source === "async"`. cyrup's nearest is
   `extension/executor/control.rs:342`. If that call site cannot distinguish a resume, `transfer`
   lands with tests only — and §SUBTASK3's "no `allow(dead_code)`" claim needs re-checking for it
   specifically.
5. **Does `StepStatus` carry the `async` classification `workflowReleaseVerdict` requires?**
   Upstream `:274` hard-retains a workflow slot when `typeof step.async !== "boolean"`. If cyrup's
   `StepStatus` has no equivalent, every workflow-kind slot would be permanently retained — decide
   between "treat a missing classification as non-async" (releases more) and "retain" (upstream's
   fail-closed) before writing the arm.
6. **Does SUBTASK5 also wire the READER?** Writing the index without teaching
   `run_status.rs:669`'s `list_active_runs` to read it means the index is maintained and unused —
   correct but inert. Wiring it (plus the 24 h `DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS` staleness
   rung at `async-status.ts:564-566`) is a materially larger change to a function five surfaces
   render from. The spec above scopes the reader as **optional**; decide explicitly and say which,
   because `estimatedEffort` assumes writer-only.
7. **Where does the on-disk `owner.json` version live?** `TerminalIndexVersion`
   (`terminal_run_index/entry.rs:28-45`) is the in-crate precedent for a literal-1 version type.
   Reuse the pattern (a second identical type) or generalise it? Do not simply use `u32` — the
   "a future version is `None`, not a panic" test (`entry.rs:189`) is the behaviour being bought.
8. **`SCOPE_8.md` open question 7 is still open and this task is its answer.** *"Is writing a
   terminal marker for a `Paused` workflow correct here, or should the call be gated on
   `is_terminal()`?"* — `is_indexed_state` (`entry.rs:103-105`) admits `Paused`, and
   `update_active_run_index`'s router (`:81`) sends `Paused` down the **terminal** arm too (it is
   neither queued nor running). So SUBTASK5 preserves the current behaviour. Confirm that is the
   intended resolution rather than a second silent decision.

---

## Research notes

* Upstream: `v0.66.0:src/runs/background/active-async-capacity.ts` — **516 LOC, the figure
  PARITY-GAPS records, confirmed at that tag** (520 at `v0.67.0`). The draft's `HEAD 7fe9dee1` pin
  is unpinned and is superseded. Read with
  `git -C /home/user/cyrup/tmp/pi-subagents show v0.66.0:<path>` — never from the working tree
  (HEAD is `13f8f328` and moves).
* Upstream, SUBTASK5: `v0.66.0:src/runs/background/active-run-index.ts` (139 LOC) — confirmed to
  exist at that tag.
* Upstream config: `v0.66.0:src/extension/config.ts:86-98` (`validateCapacityConfig`) and
  `:164-168` (`maxActiveAsyncRunsPerSession`) — the source of §D1.
* `liveWorkflowRunIds` call sites, corrected against `v0.66.0`: `run-status.ts:487`, `:514`;
  `subagent-executor.ts:633`, `:1684`/`:1690`, `:1927`/`:1933`, `:4984`/`:4990`, `:5738`,
  `:6799`/`:6805`; `extension/index.ts:929`, `:934`; `doctor.ts:189-192`. The draft's `:628` and
  `:4988` were drifted.
* cyrup roots to sit beside: `background/artifact_roots.rs:27`/`:35`/`:47`/`:64` (the four
  `cwd_key`-keyed leaves) and the doc at `:276-292` that a reader will believe. `:336`
  (`wait_subscriptions_dir_in`, SCOPE_11) is the function to copy.
* Existing concurrency config: `SubagentExtensionConfig::global_concurrency_limit`
  (`registration/mod.rs:94`, default `20` at `:467`), threaded into `RunnerConfig` at
  `extension/executor/background.rs:514`.
* `live_workflow_run_ids` (`extension/executor/workflow_controllers.rs:125`) exists and names THIS
  task as its consumer; `is_top_level_resume` was explicitly deferred to it
  (`workflow_controllers.rs:16-19`).
* `.active-runs` is already a reserved async-root entry (`terminal_run_index/mod.rs:73-75`) with no
  module behind it — SUBTASK5 supplies the module.
* `D22` appears nowhere in this repo outside this file. The documentation requirement it names is
  kept and discharged at SUBTASK1.
