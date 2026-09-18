---
stage: qa
status: completed
updated: 2026-09-16 09:00
---

# SCOPE_13 — async retention, part A: policy, scanning, tombstones

OBJECTIVE: build the first half of the async-root reaper — the data model, the retention policy,
the tree scan, and tombstones. **Unwired**: this task lands a complete, tested module that nothing
calls yet. SCOPE_14 turns it on.

Upstream `background/async-retention.ts` is 912 LOC; splitting it in two keeps each half landable
green in one session, with part A having no behavioural effect at all.

> **`[AUG]` VERIFIED NOT IMPLEMENTED.** `crates/cyrup-ext-subagents/src/background/async_retention/`
> does not exist (`ls` → `No such file or directory`). `background/mod.rs:36-96` declares 30 modules
> and none is `async_retention`. `docs/gap-analysis/PARITY-GAPS.md:60` still lists
> `async-retention.ts` (912 — the async-root reaper) among the open upstream files.
> `background/wait_subscriptions/mod.rs:129-134` says it in the code:
> *"cyrup has **no `async-retention.ts` port** … Whoever ports the reaper owns the coupling."*
> `alreadyImplemented = false`.

---

## `[AUG]` §0 — READ THIS FIRST: what batch 1's `completion_replay/retention.rs` already owns

The aug brief asked whether `background/completion_replay/retention.rs` (376 lines, landed in
PR #137 as SCOPE_4) already solves retention. **It does not solve THIS task's problem, and the two
must not be merged — but they overlap upstream and the boundary has to be drawn explicitly.**

### What `completion_replay::retention` actually sweeps

| fact | citation |
|---|---|
| entry points | `cleanup_completion_replay_if_due(results_dir, now, max_age_ms, interval_ms) -> bool` (`completion_replay/retention.rs:56-75`) and `cleanup_completion_replay(results_dir, now, max_age_ms)` (`:107-110`) |
| targets | `<results_dir>/completion-replay/` (`sweep_replay_dir`, `:113-160`) and `<results_dir>/output-archives/` (`sweep_archive_dir`, `:163-173`) — the two dirs named at `completion_replay/mod.rs:75,77` |
| **never touches** | the async root, any run directory, any `status.json` |
| throttle | process-global `LazyLock<Mutex<HashMap<PathBuf, i64>>>` keyed by `results_dir` (`:23-24`), updated BEFORE the sweep (`:71`), guard dropped before the first `.await` (`:72`) |
| policy shape | inline four-way `match (record, safe)` (`:141-158`) — **not** a pure, separately-testable classifier |
| age helper | `older_than(path, now, max_age_ms)` (`:179-191`), which re-hand-rolls the mtime→i64 conversion that `crate::time::epoch_millis` already provides |
| error policy | `errno::is_ignorable_listing_error` (`:132`) |

### Upstream's async-retention sweeps BOTH halves; cyrup has split them

`async-retention.ts` has two candidate loops:

* **result candidates** (`:839-896`) over `<resultsDir>/*.json`, `<resultsDir>/result-pending/<session>/`,
  `<resultsDir>/completion-replay/` and `<resultsDir>/output-archives/`
  (discovery worker `async-retention-discovery-worker.mjs:107-145`);
* **run candidates** (`:756-838`) over `<asyncDirRoot>/<runId>/` directories.

cyrup already covers the results-dir half with two landed, tested modules:
`completion_replay::retention` (replay + archive) and
`result_index::retention::cleanup_result_indexes` (`background/result_index/retention.rs:43`,
`DEFAULT_MAX_AGE_MS = 24h` at `:16`, `result-index/` only, with
`payloads_are_never_removed_by_the_index_sweep` at `:349` pinning that it never deletes a payload).

**THE BOUNDARY, stated once:** `async_retention` owns the **async root** — `<temp_root>/async/<cwd_key>/`
and the per-run directory trees under it — and **nothing inside `results_dir`**. It does not sweep
`completion-replay/`, `output-archives/`, `result-index/`, `result-owned/` or `result-pending/`.
Those four are already owned. SCOPE_14's DoD row `the_sweep_never_touches_the_results_dir` is
therefore not a nicety; it is the seam between this module and two landed ones.

**Consequence for the port fidelity table:** upstream's `resultSkipReason` (`:483-511`),
`resultRunId` (`:436-441`), `resultTimestamp` (`:443-455`), `terminalResult` (`:457-459`),
`resultHasResumableSession` (`:461-468`), `hasUnresolvedResultHandoff` (`:470-475`),
`completionMode` (`:477-481`), `RESULT_TOMBSTONE_PREFIX` (`:24`) and every `Result*Cursor*` type
(`:42-50`) are **deliberately out of scope for both SCOPE_13 and SCOPE_14**. Record that as a
`[CYRUP-DELTA]` in the module doc rather than leaving a reader to wonder why half of a 912-LOC file
vanished. One coupling survives the cut and is called out in §6 below:
`async-retention.ts:505`'s `"replay-reference"` (a live replay record protects an archive) is
already noted as owed-by-the-reaper in `SCOPE_4.md:1498-1499` — it is a *results*-side guard and so
stays out of scope here too; say so, do not silently drop it.

### Reuse, not duplication

`async_retention` MUST NOT re-implement `older_than`. Use `crate::time::epoch_millis`
(`src/time.rs:26`), the crate's explicit-`SystemTime` clock helper, which
`completion_replay/retention.rs:179-191` predates. Mtime-unreadable must resolve to **keep**, never
reap (that is `completion_replay/retention.rs:177-178`'s rule and upstream's `catch` at `:275`,
`:284`), and upstream states the same thing positively as `statusTimestamp` returning `undefined`
→ `"unknown-age"` → skip (`async-retention.ts:174-185`, `:301-302`).

---

## Why this is in scope despite having no session dimension

`async-retention.ts` contains **zero** `sessionId` references — it is the one item in this
programme that is not session-scoped. It is included because the directory it reaps is **shared**:
`<temp_root>/async/<cwd_key>` is resolved identically by every cyrup instance in a directory, and
orphaned run trees accumulate there without bound. Diagnosis measured **340** per-cwd result
directories. Say this in the module docs — do not imply a session justification the file does not
have.

> **`[AUG]` CITATION CORRECTED.** The draft cited `background/artifact_roots.rs:281-284`. Re-read at
> HEAD: that range is mid-doc-comment. The real seams are
> * `cwd_key(cwd: &Path) -> String` — `artifact_roots.rs:268-275`, a `DefaultHasher` of the cwd,
>   formatted `{:016x}`;
> * `pub struct RunArtifactRoots { pub async_root: PathBuf, pub results_dir: PathBuf }` —
>   `artifact_roots.rs:294-299`;
> * `pub fn run_artifact_roots(cwd: &Path) -> RunArtifactRoots` — `:307`, and
>   `run_artifact_roots_in(roots: &crate::paths::Roots, cwd: &Path)` — `:318`.
>
> The doc note the draft wanted is `artifact_roots.rs:276-293`, headed
> **"EVERY cyrup instance in a directory resolves these SAME two paths"**, whose body reads
> *"The key is `cwd_key` — the working directory, and **nothing else**. Running several cyrup
> instances in one project is ordinary, not an edge case … anything one of them deletes is gone for
> all of them."* Quote **that** in the module doc. Note that `RunStatus::session_id`'s own doc
> (`background/records.rs:226-228`) ALSO cites the stale `artifact_roots.rs:281-284`; do not copy
> the stale form forward.

> **`[AUG]` THE "340" FIGURE HAS NO LOCATABLE SOURCE.** `grep -rn "340" docs/gap-analysis/ .flux/`
> finds no diagnosis recording 340 per-cwd result directories; every hit is an unrelated line number
> or token count. **The requirement is preserved** (the module doc must carry the
> unbounded-growth justification), but the implementor MUST NOT write "340 directories, measured"
> into a doc comment as a fact this repo can back. Write the mechanism — a per-cwd shared root that
> nothing ever prunes — and cite `artifact_roots.rs:276-293` for the sharing. If the implementor
> finds the measurement, cite it; otherwise say "unbounded, unmeasured in this repo". See
> `unresolvedQuestions`.

Distinct from `result_index::cleanup_result_indexes`, which sweeps only `result-index/` and never
touches a payload or a run tree. — **`[AUG]` confirmed**: `background/result_index/retention.rs:43`,
`:16` (`DEFAULT_MAX_AGE_MS = 24h`), test `payloads_are_never_removed_by_the_index_sweep` at `:349`.

---

## `[AUG]` §1 — UPSTREAM, READ AT A PINNED TAG

Read with `git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>`. **Tag: `v0.67.0`.**

| path | exists at tag | size |
|---|---|---|
| `src/runs/background/async-retention.ts` | yes | **912 lines — the draft's figure is exact** |
| `async-retention-discovery-worker.mjs` (repo root) | yes | 180 lines |
| `test/unit/async-retention.test.ts` | yes | — |
| `src/extension/index.ts` (the scheduling site, SCOPE_14's) | yes | `:42`, `:597-614` |

The draft's `HEAD 7fe9dee1` is a real commit (`fix(runtime): keep unconfigured prompt-runtime loads
inert (#1984)`) and is the tag `wait_subscriptions` was ported at, but it is not a tag: **cite
`v0.67.0`**, where the constants sit at exactly the line numbers the draft names (`:14`–`:17`,
`:81`, `:103`). Every `:NNN` below is `async-retention.ts` @ `v0.67.0` unless it names another file.

### `[AUG]` The `AsyncRetentionOptions`/`AsyncRetentionResult` the draft names, in full

```ts
export interface AsyncRetentionOptions {          // :81-101
  asyncDirRoot: string; resultsDir: string; waitSubscriptionsDir?: string;
  protectedRunIds?: Iterable<string>; now?: () => number;
  retentionMs?: number; tombstoneGraceMs?: number; batchSize?: number;
  randomId?: () => string; maintenanceRoot?: string;
  pid?: number; hostname?: string; processStartIdentity?: string;
  isProcessAlive?: (pid) => boolean | undefined;
  getProcessStartIdentity?: (pid) => string | undefined;
  lstatSync?; signal?: AbortSignal; discoveryWorkerUrl?: URL; reconcileKill?;
}
export interface AsyncRetentionResult {           // :103-119
  acquired: boolean; scanned: number; repairedRuns: number; deletedRuns: number;
  deletedResults: number; reapedTombstones: number;
  skipped: Record<string, number>; errors: string[];
  rawReads: number; sourceExhausted: Record<string, boolean>;
  discoveryDurationMs: number; commitDurationMs: number;
  cancelled: boolean; workerFailed: boolean; durationMs: number;
}
```

`deletedResults` and the results half of `scanned` are unreachable under §0's boundary. SCOPE_14
owns `AsyncRetentionResult`; SCOPE_13 must only make sure `policy.rs`'s skip-reason vocabulary is
the exact key set `skipped` will be keyed by (see §2).

---

## SUBTASK1 — constants and options

**Where:** new module `crates/cyrup-ext-subagents/src/background/async_retention/`

```ts
ASYNC_RETENTION_DAYS = 30                    // :14
ASYNC_RETENTION_BATCH_SIZE = 100             // :15
ASYNC_RETENTION_DELAY_MS = 60_000            // :16
ASYNC_RETENTION_TOMBSTONE_GRACE_MS = 24h     // :17
interface AsyncRetentionOptions              // :81
interface AsyncRetentionResult               // :103
```

The batch size and delay are **load-shedding**, not tuning: the reaper must never stall a live
session. The 24 h tombstone grace exists so a run being reconciled concurrently is not reaped
mid-flight. Both are correctness properties; port the numbers and say why in the docs.

> ### `[AUG]` CANNOT WORK AS DESCRIBED — the tombstone grace is not what the draft says it is
>
> **Original requirement preserved above.** The numbers are exact and must be ported. But the stated
> *reason* for the grace would produce a wrong implementation, so it is corrected here rather than
> silently replaced.
>
> **What upstream actually does.** In the happy path a tombstone is created and destroyed inside one
> loop iteration with **no wait of any kind**: `:807` mints the tombstone path, `:808` writes the
> marker, `:811` renames the run dir onto it, `:817-819` re-reads and re-decides, `:830`
> `rmSync`es it. Elapsed grace: zero.
>
> `tombstoneGraceMs` is consulted at exactly **one** place for runs — `:790`,
> `currentTime - stat.mtimeMs < tombstoneGraceMs → skipped["tombstone-grace"]` — and that branch is
> only reachable for an entry **already named `.deleting-run-*` when the pass began** (`:785`),
> i.e. a tombstone a *previous, crashed or aborted* pass left behind. The grace is **crash-recovery
> hysteresis**: it stops pass N+1 from destroying a tree that pass N may still be mid-`rmSync` on,
> or that a *concurrent* instance in the same shared root is working on right now. That concurrency
> reading is the one that matters for cyrup, because `artifact_roots.rs:276-293` guarantees the root
> IS shared.
>
> The draft's "a run being reconciled concurrently is not reaped mid-flight" is served by a
> different mechanism entirely: `runSkipReason`'s liveness guards (`:292-294`) plus the
> **re-check after the rename** (`:817-829`), which re-reads the status and re-reads the wait
> subscriptions and *renames the tree back* (`:821-824`) if anything changed. That is the
> mid-flight protection; the 24 h number is not.
>
> **Consequence if not caught:** an implementor reading the draft literally writes "tombstone now,
> reap 24 h later", turning every reap into a two-pass, ≥24 h operation, and — worse — leaving a
> `.deleting-run-*` directory sitting in the shared async root for a day, where all five async-root
> scanners (§4) will see it. The correct shape is *reap in one pass; the grace only gates a
> tombstone that outlived the pass that made it.*
>
> Write BOTH readings into the constant's doc comment, exactly as
> `wait_subscriptions/mod.rs:173-183` does for its own 24 h `FOREIGN_SWEEP_GRACE_MS` — which is a
> different constant with a different job, and the doc must say so or the two will be conflated.

> ### `[AUG]` `ASYNC_RETENTION_BATCH_SIZE` is a per-PASS discovery budget, split in half
>
> `:656` clamps `batchSize` to `[1, 100]`. `:711-712` then splits it:
> `runBudget = ceil(batchSize/2)`, `resultBudget = batchSize - runBudget`. One invocation of
> `cleanupAsyncRetention` processes **one** budgeted window and returns; there is no inner loop over
> batches anywhere in the file. Under §0's boundary cyrup has no result candidates, so the cyrup
> `[CYRUP-DELTA]` is: **the whole `batchSize` is the run budget**. State that; do not silently port
> a `/2` that would halve throughput for no reason.

> ### `[AUG]` `ASYNC_RETENTION_DELAY_MS` is NOT used inside `async-retention.ts`
>
> `git grep -n ASYNC_RETENTION_DELAY_MS v0.67.0 -- src/` returns exactly three lines: the
> declaration (`:16`), an import in `src/extension/index.ts:42`, and its single use as the
> `setTimeout` delay at `src/extension/index.ts:613`. It is a **post-install one-shot delay before
> the first (and only) pass**, not an inter-batch pause. SCOPE_14's SUBTASK1 says "batches …
> separated by `ASYNC_RETENTION_DELAY_MS`" and its test
> `the_sweep_pauses_between_batches_not_between_files` assumes an inner loop upstream does not have.
> **SCOPE_13 owns the constant and its doc comment; that doc comment is where this is recorded**, so
> SCOPE_14's implementor cannot miss it. Whether cyrup adds a multi-pass loop is SCOPE_14's call,
> but it must be labelled `[CYRUP-DELTA]`, not "port".

### `[AUG]` Constants to declare, with their real Rust types

```rust
/// pi `ASYNC_RETENTION_DAYS` (`:14`).
pub const ASYNC_RETENTION_DAYS: i64 = 30;
/// pi `RETENTION_MS` (`:19`) — the derived window the policy actually compares against.
pub const ASYNC_RETENTION_MS: i64 = ASYNC_RETENTION_DAYS * 24 * 60 * 60 * 1000;
/// pi `ASYNC_RETENTION_BATCH_SIZE` (`:15`). Clamped at the entry point (`:656`).
pub const ASYNC_RETENTION_BATCH_SIZE: usize = 100;
/// pi `ASYNC_RETENTION_DELAY_MS` (`:16`). NOT an inter-batch pause — see above.
pub const ASYNC_RETENTION_DELAY_MS: i64 = 60_000;
/// pi `ASYNC_RETENTION_TOMBSTONE_GRACE_MS` (`:17`). Crash/concurrency hysteresis — see above.
pub const ASYNC_RETENTION_TOMBSTONE_GRACE_MS: i64 = 24 * 60 * 60 * 1000;
/// pi `RUN_TOMBSTONE_PREFIX` (`:23`). See §4 — this literal is a RESERVED ASYNC-ROOT NAME.
pub const RUN_TOMBSTONE_PREFIX: &str = ".deleting-run-";
/// pi `RUN_TOMBSTONE_MARKERS_DIR` (`:25`).
pub const RUN_TOMBSTONE_MARKERS_DIR: &str = "async-retention-run-tombstones";
```

`i64` throughout for time, matching `crate::time::now_epoch_millis() -> i64` (`src/time.rs:18`) and
every on-disk timestamp (`RunStatus::started_at: i64`, `records.rs:270`; `ended_at: Option<i64>`,
`:273`; `last_update: i64`, `:276`). `usize` for the batch size because it bounds a `Vec`.

Constants that are **out of scope under §0** but must be named in a doc table so the next reader
does not think they were missed: `LOCK_NAME` (`:20`), `CURSOR_NAME` (`:21`), `LOG_NAME` (`:22`),
`RESULT_TOMBSTONE_PREFIX` (`:24`), `LOCK_STALE_MS` (`:26`). `LOCK_NAME`/`LOCK_STALE_MS`/`CURSOR_NAME`
are **SCOPE_14's** (see §7); `LOG_NAME` and `RESULT_TOMBSTONE_PREFIX` are out of the programme.

---

## SUBTASK2 — `policy.rs`, the retention decision

A pure function: given a run directory's facts (age, terminal state, tombstone presence, whether it
is still tracked), decide reap / keep / tombstone. **No I/O** — same functional-core discipline as
`delivery::DeliveryDisposition`, and for the same reason: this is the decision most likely to be
wrong, and it should be exhaustively testable without a filesystem.

Model the outcome as an enum, not a bool. A two-state answer here would lose the tombstone case,
which is exactly the mistake `DeliveryDisposition` documents.

> ### `[AUG]` STALE CITATION — `delivery::DeliveryDisposition` / `delivery/disposition.rs` DO NOT EXIST
>
> `ls background/delivery/` → `custody.rs  gate.rs  mod.rs  ownership.rs  receipt.rs`. There is no
> `disposition.rs` and `grep -rn Disposition --include=*.rs` finds only
> `runner_main/settle.rs:27`'s unrelated `StepDisposition`.
>
> **The real precedent, and it is a better one:** `background/delivery/custody.rs:33-45`,
> `pub enum Attribution { Unattributed(ResultFile), Foreign(ObservableCompletion), Ours(OwnedCompletion) }`,
> classified by the pure `Attribution::classify(result, payload, owner) -> Self` (`:51-69`).
> Its doc (`custody.rs:8-31`) is the exact argument the draft is reaching for — *"Three outcomes, and
> why they are three TYPES … the permission now travels in the value instead of in a predicate the
> shell must remember to consult"* — and `delivery/mod.rs:23-28` states the functional-core rule:
> *"`ResultDeliveryOwnership` is the live, mutex-guarded state. Everything that makes a decision
> consumes an `OwnershipSnapshot` — a plain value taken once before a scan. The shell touches the
> lock and the filesystem; the core decides."* **Cite `custody.rs`, not `disposition.rs`.**

### `[AUG]` The upstream decision this is a pure refactor of

`runSkipReason` (`:279-305`) is upstream's decision. It is **not pure** — it calls `fs.existsSync`
four times (`:294` via `activeMarkerExists`, `:298` twice, `:300` via `hasResumableContract`) and
`fs.statSync` via `statusTimestamp` (`:179`). Making it pure in cyrup means **hoisting every one of
those probes into `scan.rs` as a pre-computed fact**. That is the whole design, and the fact list
must therefore be exhaustive. Upstream's order is load-bearing (cheapest/most-dangerous first) and
must be preserved, because the returned reason is what lands in `AsyncRetentionResult::skipped`.

| # | upstream guard | `:line` | fact `scan.rs` must supply | cyrup source of truth |
|---|---|---|---|---|
| 1 | `!status` | `:289` | `status: Option<RunStatus>` | `RunDir::status()` → `records.rs:210` |
| 2 | `!validRunId(status.runId)` | `:290` | — decide in policy from the id | `identity::RunDirName::parse` (`identity/run_dir_name.rs:37-53`) |
| 3 | `basename(runDir) !== status.runId` unless tombstone-prefixed | `:291` | `dir_name: String` | the scanned entry name |
| 4 | `protectedRunIds.has(id)` | `:292` | `protected_run_ids: &BTreeSet<RunId>` | SCOPE_14 threads `JobTracker` in |
| 5 | `waitRunIds.has(id)` | `:293` | `wait_run_ids: &BTreeSet<RunId>` | §5 — `wait_subscriptions` |
| 6 | `activeMarkerExists` | `:294` | `active_marker: bool` | `<async_root>/.active-runs/<id>` — **unported in cyrup**, see §4 |
| 7 | `!TERMINAL_STATES.has(state)` | `:295` | from `status.state` | `RunState::is_terminal()` (`state.rs:134-139`) |
| 8 | workflow reference | `:296` | `workflow_reference: bool` | see the delta below |
| 9 | `hasNestedReferences` | `:297` | `nested_reference: bool` | see the delta below |
| 10 | mission binding / mission observer index | `:298` | `mission_reference: bool` | `crate::missions` |
| 11 | `hasUnresolvedRunHandoff` | `:299` | `unresolved_handoff: bool` | see the delta below |
| 12 | `hasResumableContract` | `:300` | `resumable: bool` | `:191-203` |
| 13 | `statusTimestamp === undefined` | `:301-302` | `timestamp: Option<i64>` | `:174-185` |
| 14 | `timestamp > cutoff` | `:303` | compared in policy | `cutoff = now - ASYNC_RETENTION_MS` (`:709`) |

#### `[AUG]` Type deltas the implementor will hit at rows 7, 8, 9, 11, 12, 13

* **Row 7 — `TERMINAL_STATES` differs.** Upstream (`:28`) is
  `{complete, failed, stopped, rejected}`. cyrup's `RunState` (`state.rs:70-104`) is
  `Queued | Running | Paused | Complete | Failed | Stopped` — **there is no `Rejected`**, and
  `RunState::is_terminal()` (`:134-139`) is exactly `Complete | Failed | Stopped`. Use
  `is_terminal()`; do not re-spell the set. Note that `Paused` is explicitly non-terminal
  (`:124-127`), so a paused run is never reaped, which is correct and worth a test.
  Upstream also has `RUN_MODES` (`:27`) as a validity check on `status.mode`; cyrup's
  `RunMode` (`state.rs:14-38`) is a closed `serde` enum (`Single | Parallel | Chain | Workflow`), so
  an out-of-set mode is a **deserialization failure**, i.e. row 1 (`invalid-status`), not a separate
  guard. Say so — it is the same behaviour reached by a different mechanism.
* **Row 8 — `status.mode === "workflow" || status.parentWorkflowRunId || status.workflowKey`.**
  cyrup's `RunStatus` has **no `parent_workflow_run_id` and no top-level `workflow_key`**
  (`grep -n "parent_workflow_run_id\|workflow_key\|is_nested\|parallel_handoff" background/records.rs`
  → only `:108`, which is `StepStatus::workflow_key: Option<crate::workflows::WorkflowKey>`).
  What cyrup has is `RunStatus::mode == RunMode::Workflow` (`records.rs:241`) and
  `RunStatus::workflow_children: Option<crate::workflows::WorkflowChildSummary>`
  (`records.rs:339`). The honest port is: `mode == Workflow`, OR
  `workflow_children.is_some()`, OR any `steps[i].workflow_key.is_some()`. Label it `[CYRUP-DELTA]`
  and state that the two missing upstream fields do not exist on this side rather than pretending
  they were checked.
* **Row 9 — `hasNestedReferences` (`:187-189`)** is `status.isNested === true || steps.some(s => s.children?.length)`.
  cyrup has no `is_nested`. `StepStatus` does carry `children: Vec<StepStatus>` (`records.rs:198`,
  whose doc at `:195-197` says live per-member status moved into `RunStatus::steps` after
  SUBA-093; the run-level list is `RunStatus::steps` at `records.rs:289`).
  Port the half that exists — `steps.iter().any(|s| !s.children.is_empty())` — and record the other
  half as absent.
* **Row 11 — `hasUnresolvedRunHandoff` (`:268-277`)** reads `status.parallelHandoff.path` and
  `<runDir>/handoff.json`, then `unresolvedHandoff` (`:258-266`) requires
  `manifest.version === 1 && groups.length > 0 && every group's cleanup.state === "complete"`.
  cyrup has no `parallel_handoff` field. Whether cyrup writes `handoff.json` at all is an
  **open question** — see `unresolvedQuestions`. If it does not, the fact is a constant `false` and
  the doc must say the guard is vestigial on this side; if it does, the manifest shape must be
  re-derived before the guard is written.
* **Row 12 — `hasResumableContract` (`:191-203`)** checks `status.sessionFile` and every
  `step.sessionFile` for an **existing regular non-symlink file** (`existingRegularFile`, `:160-168`,
  which notably returns **`true` on a non-ENOENT stat error** — "I could not tell" resolves to
  "protected"), then `<runDir>/recovery-descriptor.json`. cyrup has `RunStatus::session_file:
  Option<PathBuf>` (`records.rs:268`) and per-step session files on `StepStatus`. Whether cyrup
  writes `recovery-descriptor.json` is an open question. The `existingRegularFile` error polarity
  is a correctness property and must be ported verbatim.
* **Row 13 — `statusTimestamp` (`:174-185`)** is
  `max(status.endedAt ?? status.lastUpdate, max(mtime(runDir), mtime(runDir/status.json)))`, and
  returns `undefined` if either logical field is present-but-non-finite, or if either `stat` throws.
  cyrup's `ended_at: Option<i64>` / `last_update: i64` are already `i64` so "non-finite" is
  unrepresentable; the `stat`-throws branch is the live one and must map to `None` → `"unknown-age"`.
  Compute both mtimes in `scan.rs` via `crate::time::epoch_millis` (`src/time.rs:26`).

### `[AUG]` The three-outcome enum, and why the draft's shape is RIGHT

Upstream's `runSkipReason -> string | undefined` looks two-valued, but the caller (`:781-833`) makes
a three-way decision, and folding it back to two is the exact `DeliveryDisposition`-style mistake the
draft warns about. The mapping is exact:

| `(already_tombstoned, skip_reason, past_grace)` | upstream | cyrup `RetentionDecision` |
|---|---|---|
| `(false, Some(r), _)` | `:781-783` increment + continue | `Keep(SkipReason)` |
| `(false, None, _)` | `:807-833` rename → recheck → rm | `Tombstone` |
| `(true,  Some(r), _)` | `:786-788` `run-tombstone-marker`, or `:781-783` | `Keep(SkipReason)` |
| `(true,  None, false)` | `:790-792` `tombstone-grace` | `Keep(SkipReason::TombstoneGrace)` |
| `(true,  None, true)` | `:794-805` recheck → `rmSync` | `Reap` |

```rust
/// The retention decision for ONE run directory. Pure — see `delivery::Attribution`
/// (`background/delivery/custody.rs:33`) for the precedent and `delivery/mod.rs:23-28` for the rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionDecision {
    /// Not a candidate. Carries WHY, because the reason is `AsyncRetentionResult::skipped`'s key.
    Keep(SkipReason),
    /// A live run tree that may be retired: rename it under `RUN_TOMBSTONE_PREFIX`, re-decide,
    /// and only then delete. SCOPE_14 performs the rename; SCOPE_13 only decides.
    Tombstone,
    /// An EXISTING `.deleting-run-*` tree, past the grace, that still decides reapable: delete it.
    Reap,
}
```

`SkipReason` must be a closed enum with an `as_str()` producing upstream's **exact** literals, since
they are the `skipped` map's keys and SCOPE_14's report is compared against upstream's vocabulary:
`invalid-status`, `identity-mismatch`, `runtime-reference`, `wait-reference`, `active-index`,
`non-terminal`, `workflow-reference`, `nested-reference`, `mission-reference`, `handoff-reference`,
`resumable`, `unknown-age`, `recent` (all `:289-303`), plus `run-tombstone-marker` (`:787`) and
`tombstone-grace` (`:791`). SCOPE_14 additionally prefixes a re-check reason with `recheck-`
(`:797`, `:827`) — provide the `as_str()` that makes that composable, do not make SCOPE_14 invent it.
Sibling precedent for the closed-enum-with-`as_str` shape:
`wait_subscriptions::WaitTargetKind::as_str` (`wait_subscriptions/record.rs:159`) and
`background::run_status::run_state_label` (`run_status.rs:45`).

### `[AUG]` Signature

```rust
/// Every fact `policy.rs` needs, gathered by `scan.rs`. Plain data — no paths are dereferenced here.
#[derive(Clone, Debug)]
pub struct RunRetentionFacts {
    pub dir_name: String,
    pub status: Option<RunStatus>,
    pub timestamp: Option<i64>,
    pub tombstone_mtime: Option<i64>,   // Some(..) iff dir_name starts with RUN_TOMBSTONE_PREFIX
    pub marker_state: TombstoneMarkerState,
    pub active_marker: bool,
    pub mission_reference: bool,
    pub unresolved_handoff: bool,
    pub resumable: bool,
}

/// Upstream `runSkipReason` (`:279-305`) plus the caller's tombstone branch (`:785-805`), as ONE
/// pure decision. No I/O, no clock read — `now` is a parameter.
#[must_use]
pub fn decide(
    facts: &RunRetentionFacts,
    now: i64,
    retention_ms: i64,
    tombstone_grace_ms: i64,
    protected_run_ids: &BTreeSet<RunId>,
    wait_run_ids: &BTreeSet<RunId>,
) -> RetentionDecision;
```

`BTreeSet<RunId>` rather than `HashSet`: `RunId` is `Arc<str>` (`background/run_id.rs:30`) and
derives `Ord`; a deterministic set makes any future report ordering stable, matching
`wait_subscriptions/manager.rs:257-259`'s stated reason for `BTreeMap` over `HashMap`.

---

## SUBTASK3 — `scan.rs`, the batched tree walk

Walks the async root in batches of `ASYNC_RETENTION_BATCH_SIZE`, yielding candidates with the facts
`policy.rs` needs. Reuses `result_index::errno`'s classification — missing/permission-denied
directories are skipped, a genuine fault propagates.

> ### `[AUG]` The batching is a CURSOR-WINDOWED SELECTION, not "read 100 entries and stop"
>
> The mechanism is `streamDirWindow(dir, limit, after, relativePath, usable)` in
> `async-retention-discovery-worker.mjs:22-48`, driven at `:58-68`. Read it literally, because a
> naive "stop after `limit` entries" scan **never converges and never wraps**:
>
> 1. `opendir` and read **every** entry to exhaustion (`:30-39`); `rawReads` counts all of them, so
>    one pass always costs a full `readdir` of the async root. The budget bounds the *output*, not
>    the read.
> 2. Keep two ascending, `limit`-bounded windows of the smallest names (`insertSmallest`, `:14-20`,
>    which inserts in sorted position and pops the tail past `limit`): `wrapped` over all usable
>    entries, and `next` over only those with `relative > after`.
> 3. `cursorCleared = after !== undefined && next.length === 0` (`:40`) — the cursor ran off the end.
>    Return `wrapped` (wrap to the beginning) and delete the cursor (`:66`). **This wrap-around is
>    what makes repeated passes eventually visit every directory**; without it the scan sticks at the
>    end of the name order forever.
> 4. `ENOENT` on the whole directory → empty, `exhausted: true`, not an error (`:43`). Everything
>    else **throws** (`:43-44`) — that is the "genuine fault propagates" half the draft names.
> 5. The run filter is `entry.isDirectory() && entry.name !== ACTIVE_RUN_INDEX_DIR` (`:63`).
> 6. The caller advances the cursor to the **last candidate returned** (`:68`), so the next pass
>    resumes strictly after it.
>
> Byte-lexicographic ordering (`compareRelative`, `:8-12`) is the ordering; in Rust that is `Ord` on
> `str`, which is byte order. Do not use a locale collation or an OS-dependent `read_dir` order.

### `[AUG]` cyrup shape

`tokio::fs::read_dir` returns `io::Result<ReadDir>`; `ReadDir::next_entry()` returns
`io::Result<Option<DirEntry>>`. Both error channels must be classified through
`crate::background::result_index::errno` — **note the visibility**: every predicate in
`background/result_index/errno.rs` is `pub(crate)`, not `pub`
(`is_unaddressable` `:22`, `is_absent` `:31`, `is_access_denied` `:47`,
`is_ignorable_listing_error` `:57`). `async_retention` is in the same crate, so
`crate::background::result_index::errno::is_ignorable_listing_error` resolves; no visibility change
is needed, and none should be made.

The split the draft asks for maps onto the module's own documented distinction
(`errno.rs:39-45`: *"Quiet when scanning, loud when asked a direct question"*):

* opening the async root, and per-entry `metadata`/`read`: `is_ignorable_listing_error` → skip;
* anything else → propagate as `io::Error`.

`scan.rs`'s public surface is therefore fallible, unlike `completion_replay::retention`'s
swallow-everything sweep — that is a deliberate difference, because the draft's
`the_scan_propagates_a_genuine_io_fault` demands it. Suggested:

```rust
/// One cursor-windowed pass over the async root. Returns at most `budget` candidates in byte order
/// strictly after `after`, wrapping to the start when the cursor runs off the end.
/// Upstream `streamDirWindow` (`async-retention-discovery-worker.mjs:22-48`) + `:58-68`.
pub(crate) async fn scan_run_candidates(
    async_root: &Path,
    after: Option<&str>,
    budget: usize,
) -> std::io::Result<RunScanWindow>;

pub(crate) struct RunScanWindow {
    pub candidates: Vec<RunRetentionFacts>,
    pub next_after: Option<String>,   // None == cursor cleared (wrapped)
    pub raw_reads: usize,             // AsyncRetentionResult::rawReads
    pub cursor_cleared: bool,
}
```

The cursor **value** is a directory name (`String`), not a `RunId`: a `.deleting-run-<uuid>` entry
is a legal cursor position and is not a run id.

`scan.rs` is also where every row-6-to-row-13 probe from §SUBTASK2's table is performed, so that
`policy.rs` stays pure. Use `RunDir::for_existing(path).status()` (`run_paths.rs:52`, `:62`) for the
status path — never re-spell `"status.json"`; `run_paths.rs:13` is the single definition and
`:56-60` explains why `RunDir::status()` exists precisely so index readers do not re-derive it.

---

## SUBTASK4 — `tombstone.rs`

Writing, reading and expiring tombstones on the `ASYNC_RETENTION_TOMBSTONE_GRACE_MS` ladder.

> ### `[AUG]` A tombstone is TWO artefacts, and the draft names only the (absent) one
>
> Upstream `:807-831`:
>
> 1. **The tombstone itself is the run tree, RENAMED**: `path.join(asyncDirRoot,
>    RUN_TOMBSTONE_PREFIX + randomId())`, then `fs.renameSync(runDir, tombstone)` (`:811`). There is
>    no "tombstone file written next to the run". The rename is the atomic step that takes the tree
>    out of every other reader's addressable namespace in one syscall.
> 2. **The marker** is a separate small JSON at
>    `<maintenanceRoot>/async-retention-run-tombstones/<encodeIndexSegment(runId)>.json`
>    (`runTombstoneMarkerPath`, `:213-215`), written with `writeAtomicJson` as
>    `{ version: 1, runId, tombstonePath, createdAt }` (`writeRunTombstoneMarker`, `:217-221`).
>
> The marker exists because after the rename **nothing on disk maps the run id back to the tree**.
> Its only *reader* upstream is `resultSkipReason`'s `run-tombstone-present` guard (`:496`) — which
> is results-side and out of scope under §0 — plus two run-side uses that ARE in scope:
> `runTombstoneMarkerMatches` (`:240-243`), which refuses to reap a `.deleting-run-*` tree whose
> marker does not resolve to it (`:786-788`), and `runTombstoneMarkerBlocks` (`:245-256`), whose
> self-healing `removeRunTombstoneMarker` on a dangling marker (`:251`) is what stops markers
> accumulating forever.
>
> **`[AUG]` The marker write must precede the rename** (`:808` before `:811`) and be rolled back if
> the rename fails (`:812-816`). A marker with no tree is self-healed at `:251`; a tree with no
> marker is never reaped (`:787`) and leaks. Order is a correctness property; write it down.

### `[AUG]` Read/parse contract, ported exactly

`readRunTombstoneMarker` (`:227-238`) is **three-valued** — `Some(marker) | None | "unreadable"` —
and collapsing it to two breaks `runTombstoneMarkerBlocks`, where `undefined` → *not blocked* and
`"unreadable"` → *blocked* (`:247-248`). Same shape as
`completion_replay::retention`'s row-2-vs-row-4 split (`completion_replay/retention.rs:88-92`:
*"Row 4 is the one that must not be simplified into row 2"*). In Rust:

```rust
/// Upstream `readRunTombstoneMarker` (`:227-238`) — THREE outcomes, deliberately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TombstoneMarkerState {
    /// No marker file (`ENOENT`). NOT blocking (`:247`).
    Absent,
    /// Present but unusable — not a regular file, a symlink, unparseable, wrong version, or a
    /// `runId` that disagrees with the address. BLOCKING (`:248`), because "I cannot tell" must
    /// never resolve to "delete it".
    Unreadable,
    /// A valid marker naming the tombstone tree.
    Present { run_id: RunId, tombstone_path: PathBuf },
}
```

Validity, verbatim from `:231` and `:236`: `lstat` must report a **regular file that is not a
symlink**; then `version == 1`, `runId` equals the address's run id, and `tombstonePath` is a
non-empty string. `runTombstoneMarkerMatches` (`:242`) compares
`path.resolve(marker.tombstonePath) === path.resolve(tombstonePath)` — canonicalise both sides
(`std::fs::canonicalize` on the parent plus the file name, since the tombstone may already be gone;
or `std::path::absolute`) and say which you chose and why.

### `[AUG]` Rust seams for the marker

* Address encoding: `crate::identity::IndexSegment::encode(run_id.as_str())`
  (`identity/path_segment.rs:45`) — upstream's `encodeIndexSegment`. **Not**
  `identity::decode_uri_component`/`ResultFileName`, which is
  `completion_replay`'s raw `encodeURIComponent` family
  (`completion_replay/mod.rs:26-27` explains the difference; `completion_replay/retention.rs:26-36`
  shows the round-trip that makes the raw one safe). Use `IndexSegment` here because upstream does
  (`:214`), and use `encode`, not `encode_bounded`/`aliases` — one write key, never the fan-out, the
  same rule `terminal_run_index/mod.rs:25-27` states.
* Atomic write: `crate::background::atomic::write_atomic_json` (`background/atomic.rs:75`,
  `pub async fn write_atomic_json<T: serde::Serialize + Sync>(…)`), upstream's `writeAtomicJson`.
* Directory creation: `crate::background::ensure_accessible_dir` (`artifact_roots.rs:416`,
  `pub async fn ensure_accessible_dir(dir: &Path) -> std::io::Result<()>`), which also carries the
  broken-ACL recovery (`:421-422`). Mirrors `wait_subscriptions/manager.rs:347`.
* Record versioning: follow `wait_subscriptions::SubscriptionVersion`
  (`wait_subscriptions/record.rs:29-44`), a unit struct with hand-written `Serialize`/`Deserialize`
  that refuses any value but `1`, rather than a bare `u32` field.
* No `unwrap`/`expect`/`panic`/indexing — `lib.rs:19-24` `#![deny]`s all four, and
  `#![forbid(unsafe_code)]` at `:25`. Test modules open with the four-line
  `#![allow(...)]` block used everywhere in this crate (e.g.
  `completion_replay/retention.rs:195-200`).

**Layout so far:** `async_retention/{mod,policy,scan,tombstone}.rs`. SCOPE_14 adds `sweep.rs` and
`report.rs`.

---

## `[AUG]` §4 — THE CROSS-CUTTING CONSEQUENCE THE DRAFT MISSES: reserved async-root names

`.deleting-run-*` directories live **inside the async root**, beside the run directories. cyrup has
**five** scanners that treat every subdirectory of the async root as a run:

| scanner | site |
|---|---|
| fleet history roster | `tui/fleet.rs:549` (guard at `:572`) |
| `resolve_run_id` prefix fallback | `background/run_status.rs:478` (guard at `:491`) |
| `list_active_runs` | `background/run_status.rs:676` (guard at `:695`) |
| `resolve_async_run_location` | `background/run_id_resolver.rs:122` (guard at `:69`) |
| executor status listing | `extension/executor/status.rs:32` (guard at `:68`) |

All five funnel through `crate::background::terminal_run_index::is_reserved_async_root_entry`
(`terminal_run_index/mod.rs:73-75`), which today is **exactly two literal names**:

```rust
pub fn is_reserved_async_root_entry(name: &str) -> bool {
    name == TERMINAL_RUN_INDEX_DIR || name == ".active-runs"
}
```

and whose own test asserts the guard is **not** a dot-glob:
`assert!(!is_reserved_async_root_entry(".hidden"))` (`:94`). `terminal_run_index/mod.rs:31-36` states
the obligation directly: *"Placement obliges every async-root scanner to skip it."*
`run_id_resolver.rs:64-71` spells out the failure mode if it does not:
*"`async_dir.exists()` below is TRUE for it and would otherwise mint a phantom `AsyncRunLocation`."*

**Therefore SCOPE_13 must extend that predicate to a prefix arm for `RUN_TOMBSTONE_PREFIX`, and for
whatever maintenance directory §7 settles on.** Without it, a tombstone that outlives its pass —
precisely the case the 24 h grace exists for — appears in `/subagents-fleet`, becomes an ambiguous
prefix match in `resolve_run_id`, and mints a phantom `AsyncRunLocation`.

**Keep the literal in `terminal_run_index`, not in `async_retention`.** The reserved-name vocabulary
belongs with the predicate that enforces it, exactly as `.active-runs` is already named there
*"even though `active-run-index.ts` is unported, so the guard does not have to be revisited when it
lands"* (`terminal_run_index/mod.rs:70-71`). `async_retention` then does
`pub use crate::background::terminal_run_index::RUN_TOMBSTONE_PREFIX;` (or re-exports it under its
pi name). This also **keeps the "called by nothing" DoD literally true**: the dependency edge points
`async_retention → terminal_run_index`, so
`grep -rn async_retention crates/ --include=*.rs | grep -v async_retention/` still returns only the
`mod` declaration.

**Row 6 of §SUBTASK2's table, resolved:** `activeMarkerExists` (`:205-207`) probes
`<asyncDirRoot>/.active-runs/<runId>`. `active-run-index.ts` is **unported in cyrup**
(`terminal_run_index/mod.rs:70-71` says so explicitly; `PARITY-GAPS.md:59` lists
`active-async-capacity.ts` among the open files). The directory therefore never exists and the probe
is always `false`. Keep the fact in `RunRetentionFacts` and keep the guard in `policy.rs` — it is a
**liveness** guard and must be present the day the index lands — and have `scan.rs` perform the real
`<async_root>/.active-runs/<run_id>` existence probe, which costs one `stat` and is correct either
way. Do **not** hard-code `false` in the policy; that is the change that would be silently wrong
later.

---

## `[AUG]` §5 — THE `wait_subscriptions` COUPLING, NOW LANDABLE

PR #139 landed `background/wait_subscriptions/`, and its module doc *names this task*
(`wait_subscriptions/mod.rs:127-134`):

> *"Out of scope, recorded so it is not re-derived — Upstream's async retention reaper reads this
> directory: `async-retention.ts:307-316`'s `parseWaitRunIds` protects a run referenced by a live
> subscription from being reaped, re-read before each destructive action, and skips the whole pass as
> `"wait-references-unknown"` when any record is unparseable. cyrup has no `async-retention.ts`
> port (`PARITY-GAPS.md:55` lists it, 912 LOC, still open), so there is nothing to wire this into.
> Whoever ports the reaper owns the coupling."*

(That comment's `PARITY-GAPS.md:55` is itself now stale — the row is at `docs/gap-analysis/PARITY-GAPS.md:60`.
Fixing it is a one-line courtesy, not a requirement.)

`parseWaitRunIds` (`:307-316`) returns `{ runIds, safe }`: it lists the directory, and for every
`*.json` file requires `typeof record.runId === "string" && typeof record.expiresAt === "number"`;
**one unparseable record makes the whole set unsafe**, which at `:752-755` aborts the entire pass
with `skipped["wait-references-unknown"]`. Fail-closed by design.

cyrup seams, all verified:

| need | seam |
|---|---|
| the directory | `crate::background::wait_subscriptions_dir_in(&roots, cwd)` — `artifact_roots.rs:336-341`, `<run_scratch>/<SUBSCRIPTIONS_SUBDIR>/<cwd_key>`; production call site `extension/executor/wait_subscriptions.rs:282` |
| the record | `WaitSubscriptionRecord { version: SubscriptionVersion, token: SubscriptionToken, session_id: SessionId, target_kind: WaitTargetKind, run_id: RunId, requested_id: String, created_at: i64, expires_at: i64 }` — `wait_subscriptions/record.rs:180-209` |
| the parser | `pub fn parse_record(bytes: &[u8]) -> Option<WaitSubscriptionRecord>` — `record.rs:245` |
| the file name | `pub fn subscription_file(dir: &Path, token: &SubscriptionToken) -> PathBuf` — `record.rs:230` |
| an in-process alternative | `WaitSubscriptionManager::armed() -> Vec<WaitSubscriptionRecord>` — `manager.rs:308` (THIS session only — not a substitute; the directory is shared, `manager.rs:818` reads it for exactly that reason) |

`parse_record`'s `Option` collapses "absent" and "invalid" the way `parseWaitRunIds` needs: a `None`
from a `*.json` file is the unsafe signal. **`wait_run_ids` must be read from the DIRECTORY, not
from `armed()`** — `armed()` is narrowed to the current session by construction
(`manager.rs:255-256`) and would leave another instance's waited-on run unprotected in the shared
root.

**Boundary:** SCOPE_13 owns the *reader* — a `pub(crate) async fn wait_run_ids(dir: &Path) ->
Option<BTreeSet<RunId>>` where `None` is upstream's `safe: false` — and `policy.rs` consumes the set
at row 5. SCOPE_14 owns the **re-read before each destructive action** (`:794`, `:818`) and the
pass-level abort (`:752-755`), because both are sweep control flow.

---

## `[AUG]` §6 — WHAT CANNOT WORK AS THE DRAFT DESCRIBES (consolidated)

Each of these preserves the original requirement above and records the mechanism that can.

1. **The 24 h tombstone grace is crash/concurrency hysteresis, not mid-flight reconciliation
   protection.** §SUBTASK1. Following the draft literally yields a ≥24 h two-pass reap and a
   day-long `.deleting-run-*` in a shared root.
2. **`delivery::DeliveryDisposition` / `background/delivery/disposition.rs` do not exist.** Use
   `delivery::Attribution` (`background/delivery/custody.rs:33-69`). §SUBTASK2.
3. **`ASYNC_RETENTION_DELAY_MS` is a post-install one-shot delay, not an inter-batch pause**
   (`src/extension/index.ts:598-614`). §SUBTASK1. SCOPE_14's
   `the_sweep_pauses_between_batches_not_between_files` is testing a cyrup invention; it may still be
   built, but must be labelled one.
4. **`ASYNC_RETENTION_BATCH_SIZE` bounds the candidate WINDOW, not the `readdir`.** Every pass reads
   the whole async root (`rawReads`). A "stop after 100 entries" scan never wraps and never
   converges. §SUBTASK3.
5. **The tombstone is a directory rename plus a separate marker, not a file written beside the
   run.** §SUBTASK4.
6. **Introducing `.deleting-run-*` into the async root breaks five existing scanners** unless
   `is_reserved_async_root_entry` grows a prefix arm. §4. This is the highest-risk omission in the
   draft: it is a silent, cross-module correctness regression, not a missing feature.
7. **The "340 directories measured" figure has no locatable source in this repo.** Preserve the
   justification, drop the unsourced number, or find and cite it. §"Why this is in scope".
8. **`maintenanceRoot = dirname(asyncDirRoot)` does not transplant.** §7.
9. **Several upstream policy inputs have no cyrup field** (`isNested`, `parentWorkflowRunId`,
   top-level `workflowKey`, `parallelHandoff`). §SUBTASK2's delta list. Port the half that exists and
   record the absent half; do not invent fields on `RunStatus`.
10. **Upstream's `runSkipReason` is not pure** (`fs.existsSync` ×4, `fs.statSync` ×2). The purity the
    draft demands is a genuine cyrup refactor, and it is only sound if `scan.rs` gathers **every**
    fact in §SUBTASK2's fourteen-row table. A missing row is a silently-never-fires guard.

---

## `[AUG]` §7 — WHERE PART A STOPS AND PART B PICKS UP (the aug brief's "exactly once")

SCOPE_14 is being augmented in parallel. The seam, unambiguous in both directions:

### SCOPE_13 (this task) owns

* `async_retention/mod.rs` — the facade, the constants (§SUBTASK1), the module narrative (§0's
  boundary, the shared-root justification, the out-of-scope upstream table).
* `async_retention/policy.rs` — `RetentionDecision`, `SkipReason`, `RunRetentionFacts`, `decide`.
  Pure. No `tokio`, no `std::fs`, no clock read.
* `async_retention/scan.rs` — `scan_run_candidates`, `RunScanWindow`, and every fact-gathering probe
  in §SUBTASK2's table. Fallible (`io::Result`). Owns the **cursor VALUE** (`next_after`,
  `cursor_cleared`).
* `async_retention/tombstone.rs` — `TombstoneMarkerState`, marker path/write/read/remove,
  `marker_matches`, `marker_blocks`. Owns the marker's on-disk format.
* `async_retention/wait_refs.rs` (or a `scan.rs` function) — §5's `wait_run_ids` reader.
* `background/terminal_run_index/mod.rs` — the `RUN_TOMBSTONE_PREFIX` literal and the prefix arm in
  `is_reserved_async_root_entry` (§4). **The only file outside `async_retention/` this task edits.**

### SCOPE_14 (part B) owns

* `sweep.rs` — the rename/re-check/`remove_dir_all`/rollback dance (`:807-833`), the re-read of wait
  refs before each destructive action (`:794`, `:818`), the pass-level
  `wait-references-unknown` abort (`:752-755`), the `recheck-` reason prefix (`:797`, `:827`), and
  the `cursorCommitUnsafe` flag (`:826`, `:835`, `:885`, `:893`, `:897-900`).
* `report.rs` — `AsyncRetentionResult` (`:103-119`) and the `skipped` tally.
* **The cursor's PERSISTENCE** — `.async-retention-cursor.json` (`CURSOR_NAME`, `:21`), `readCursor`
  (`:318-321`), `applyCursorOperations` (`:636-648`) and the atomic commit (`:906-907`). SCOPE_13
  produces the cursor value; SCOPE_14 stores and commits it. *(This is the one line most likely to be
  read differently by the two augmentations — it is stated here and must be mirrored there.)*
* **The lock** — `.async-retention.lock` (`LOCK_NAME`, `:20`), `acquireRetentionLock` (`:413-429`),
  `staleLock` (`:378-395`), `releaseRetentionLock` (`:431-434`), `LOCK_STALE_MS` (`:26`),
  and the pid/hostname/`processStartIdentity` liveness probe (`:323-362`). **Not optional**:
  `artifact_roots.rs:276-293` guarantees concurrent instances share the root, so without the lock two
  reapers race each other's renames. The draft mentions none of it; it is recorded here so SCOPE_14
  cannot miss it, and it is SCOPE_14's because it is sweep control flow.
* **The `maintenanceRoot`.** Upstream defaults it to `path.dirname(options.asyncDirRoot)` (`:658`),
  which works because upstream's `DIRS.async` is flat. **In cyrup that would be
  `<run_scratch>/async/`, shared by EVERY cwd** — a lock there would serialise retention across
  unrelated projects and a cursor there would be meaningless. cyrup must key it per-cwd. Two
  candidates: `<async_root>/.async-retention/` (needs another `is_reserved_async_root_entry` arm, so
  SCOPE_13 should add it if this is chosen) or a sibling `<run_scratch>/retention/<cwd_key>`
  (`cwd_key`, `artifact_roots.rs:268`). **Decide before SCOPE_13 writes `tombstone.rs`**, because the
  marker path hangs off it — see `unresolvedQuestions`.
* Threading the live `JobTracker` in as `protected_run_ids` — `JobTracker::snapshot() -> Vec<TrackedJob>`
  (`background/tracker.rs:224`), `tracked_count()` (`:208`), `get(&RunId)` (`:235`).
* Scheduling (`extension/executor/notices.rs`) and the `criterion` benchmark.
  **`crates/cyrup-ext-subagents/benches/` does not exist** (`ls benches` → no such directory) and no
  workspace member has one; SCOPE_14's own instruction to say so if there is no convention is
  therefore already triggered.
* `reconcileAsyncRun` on a `running` candidate (`:768-779`, `repairedRuns`) via
  `crate::background::reconcile::reconcile` (`background/reconcile.rs:231`) /
  `reconcile_now` (`:331`).

### Explicitly owned by NEITHER (recorded so neither invents it)

Every results-side symbol listed in §0, and `async-retention.ts:505`'s `"replay-reference"` guard —
already flagged as owed-in-principle at `SCOPE_4.md:1498-1499`, and still out of scope because it
protects an **archive**, which lives in `results_dir`.

---

## Tests

All of `policy.rs`'s tests are pure — no tempdir, no tokio:

| test | pins |
|---|---|
| `a_run_younger_than_the_window_is_kept` | 30-day policy |
| `an_orphaned_terminal_run_past_the_window_is_reaped` | the happy path |
| `a_still_tracked_run_is_never_reaped_regardless_of_age` | the liveness guard |
| `a_non_terminal_run_is_never_reaped` | the state guard |
| `a_tombstoned_run_inside_the_grace_is_kept` | 24 h grace — the concurrent-reconciliation case |
| `a_tombstoned_run_past_the_grace_is_reaped` | grace expiry |
| `the_policy_has_three_outcomes_not_two` | tombstone is not collapsible into keep/reap |

Filesystem-backed:

| test | pins |
|---|---|
| `the_scan_yields_at_most_a_batch_at_a_time` | `ASYNC_RETENTION_BATCH_SIZE` |
| `the_scan_skips_an_unreadable_directory` | `errno` reuse |
| `the_scan_propagates_a_genuine_io_fault` | the other half of `errno`'s policy |
| `a_tombstone_round_trips` | `tombstone.rs` |

> ### `[AUG]` Fail-before / pass-after, per test
>
> Every test below fails before the task purely because the module does not exist (`E0433`). The
> column that matters is **what a plausible-but-wrong implementation does**, which is what makes each
> row worth writing.
>
> | test | fails-before if the module existed but was wrong |
> |---|---|
> | `a_run_younger_than_the_window_is_kept` | `>` vs `>=` at the cutoff (`:303` is `timestamp > cutoff → "recent"`). Assert `timestamp == cutoff` **reaps** and `cutoff + 1` **keeps**. |
> | `an_orphaned_terminal_run_past_the_window_is_reaped` | returns `Tombstone`, not `Reap`, for an untombstoned candidate — and `Tombstone` is the CORRECT answer here. **Rename the table row's expectation, not the code**: an untombstoned candidate past the window yields `Tombstone`; `Reap` is only ever returned for an already-`.deleting-run-*` entry past the grace. Keep the test name; assert `RetentionDecision::Tombstone`. |
> | `a_still_tracked_run_is_never_reaped_regardless_of_age` | guard ordered after the age check, so a 90-day tracked run reaps. Assert `Keep(RuntimeReference)` — the *reason*, not merely "not Reap" — at `now = i64::MAX/4`. |
> | `a_non_terminal_run_is_never_reaped` | `Paused` treated as terminal. Table-drive all six `RunState` values; assert `Complete/Failed/Stopped` proceed and `Queued/Running/Paused` yield `Keep(NonTerminal)`. |
> | `a_tombstoned_run_inside_the_grace_is_kept` | grace compared against `now - started_at` instead of the **tombstone's mtime** (`:790`). Give the facts an old `started_at` and a fresh `tombstone_mtime`; assert `Keep(TombstoneGrace)`. |
> | `a_tombstoned_run_past_the_grace_is_reaped` | same input with `tombstone_mtime = now - grace - 1`; assert `Reap`. The pair is the boundary. |
> | `the_policy_has_three_outcomes_not_two` | a `bool` or `Option<SkipReason>` return. Assert all three variants are produced by three concrete fact sets in one test body, so collapsing the enum is a compile error plus an assertion failure. |
> | **`[AUG] every_skip_reason_is_reachable_and_uniquely_named`** | a `SkipReason` variant no fact set can produce, or two variants sharing an `as_str()`. Assert both, since these strings are `AsyncRetentionResult::skipped`'s keys and SCOPE_14's report is compared to upstream's vocabulary. |
> | **`[AUG] a_run_whose_directory_name_disagrees_with_its_status_is_kept`** | `:291` dropped. Assert `Keep(IdentityMismatch)` — and assert that a `.deleting-run-*` name with a matching marker is **exempt** from the check (`:291`'s `&& !startsWith` clause), which is the half most likely to be lost. |
> | **`[AUG] a_run_with_an_unreadable_mtime_is_kept_not_reaped`** | `timestamp: None` reaping. Assert `Keep(UnknownAge)` (`:301-302`). |
> | **`[AUG] an_unreadable_tombstone_marker_blocks_the_reap`** | `TombstoneMarkerState::Unreadable` treated as `Absent` (`:247` vs `:248`). Assert `Keep`. |
> | `the_scan_yields_at_most_a_batch_at_a_time` | unbounded window. Create `budget + 5` run dirs, assert `candidates.len() == budget` AND that the returned names are the byte-order **smallest** `budget` above the cursor. |
> | **`[AUG] the_scan_wraps_when_the_cursor_runs_off_the_end`** | no wrap-around (`worker.mjs:40-41`). Pass `after` = a name past every entry; assert `cursor_cleared == true` and that the window restarts at the smallest name. **Without this the scan never revisits early directories** — the single most likely silent defect in `scan.rs`. |
> | **`[AUG] the_scan_skips_reserved_async_root_entries`** | `.terminal-runs`, `.active-runs` offered as run candidates. Also assert a `.deleting-run-*` entry **IS** offered (it is a tombstone candidate, `worker.mjs:63` only excludes the active index). |
> | `the_scan_skips_an_unreadable_directory` | a `PermissionDenied` aborting the pass. Requires a real `chmod 000`; gate on non-root (`unsafe` is forbidden, so use `std::os::unix::fs::PermissionsExt` and skip when `nix::unistd::geteuid`-equivalent is 0 — or, if root-in-CI makes that untestable, inject the error and say so in a comment). |
> | `the_scan_propagates_a_genuine_io_fault` | blanket `let Ok(..) else { return }`. Assert an `Err` escapes for a non-ignorable errno — the direct contrast with `completion_replay/retention.rs:115-117`, which returns on any error. |
> | `a_tombstone_round_trips` | `IndexSegment` vs raw `encodeURIComponent` mismatch, or a version field that accepts `99`. Write → read → `Present { run_id, tombstone_path }` equal; then corrupt the version and assert `Unreadable`, not `Absent`. |
> | **`[AUG] the_marker_write_precedes_the_rename_and_rolls_back`** | `:808` before `:811`, rollback at `:812-816`. A tombstone path whose rename cannot succeed (target parent absent) must leave **no** marker behind. |
> | **`[AUG] a_marker_pointing_at_a_vanished_tree_self_heals`** | `:249-252`. Write a marker whose `tombstone_path` does not exist; assert `marker_blocks` is `false` **and** that the marker file is gone afterwards. |
> | **`[AUG] wait_subscription_run_ids_fail_closed_on_an_unparseable_record`** | `:312`'s `return { runIds, safe: false }`. One armed valid record plus one `{}` file → the reader returns `None` (not a partial set). §5. |
> | **`[AUG] the_tombstone_prefix_is_a_reserved_async_root_entry`** | §4. In `terminal_run_index/mod.rs`'s own test module, beside `both_reserved_index_dirs_are_recognized_and_runs_are_not` (`:88-96`): assert `.deleting-run-abc` is reserved and that `.hidden` and a bare run id still are not. |
>
> Test-module conventions, verified in-crate: `tempfile` is a dev-dependency (`Cargo.toml`
> `[dev-dependencies]`, first entry); `#[tokio::test]` is used throughout
> (`completion_replay/retention.rs:238`); every test module opens with the four-line
> `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
> (`completion_replay/retention.rs:195-200`). `policy.rs`'s tests take plain `#[test]` — no runtime.

## Benchmarks

None **in this task**. The reaper is explicitly load-shed by batch size and delay rather than
optimised for throughput; if a performance question arises it belongs with SCOPE_14, which owns the
sweep that actually does I/O at scale.

---

## Definition of done

- `policy.rs` is pure, returns a three-outcome enum, and has the full table above passing with no
  filesystem.
  - **`[AUG]` checkable as:** `policy.rs` contains no `use std::fs`, no `use tokio::fs`, no
    `std::path::Path::exists`, and no `crate::time::` call; `decide` takes `now: i64` as a parameter.
    `grep -n "fs::\|now_epoch_millis\|\.exists()" src/background/async_retention/policy.rs` is empty.
- `scan.rs` batches and reuses `result_index::errno`.
  - **`[AUG]` checkable as:** `grep -n "errno::" src/background/async_retention/scan.rs` is non-empty
    and `grep -n "ErrorKind::" src/background/async_retention/scan.rs` is **empty** (no fresh
    `matches!` on `ErrorKind` — `errno.rs:6-11` states that rule).
  - **`[AUG]`** `scan_run_candidates` returns `io::Result` and `the_scan_propagates_a_genuine_io_fault`
    passes.
  - **`[AUG]`** the wrap-around is implemented: `the_scan_wraps_when_the_cursor_runs_off_the_end`
    passes.
- `tombstone.rs` round-trips and honours the grace.
  - **`[AUG]`** `TombstoneMarkerState` has **three** variants and `Unreadable` blocks.
- The module compiles and is fully tested but is **called by nothing** — `grep -rn async_retention
  crates/ --include=*.rs | grep -v async_retention/` returns only the `mod` declaration.
  - **`[AUG]` still holds** under §4's design, because the `RUN_TOMBSTONE_PREFIX` literal lives in
    `terminal_run_index`, so the edge points `async_retention → terminal_run_index` and not back.
    Run the grep and confirm it, do not assume it.
- Module docs state the no-session-dimension justification (shared directory, unbounded growth,
  340 dirs measured) rather than implying session scoping.
  - **`[AUG]`** quote `artifact_roots.rs:276-293` for the sharing; do **not** assert "340, measured"
    unless the measurement is located (§"Why this is in scope").
- **`[AUG]`** `is_reserved_async_root_entry` covers `RUN_TOMBSTONE_PREFIX`, and
  `the_tombstone_prefix_is_a_reserved_async_root_entry` passes. Re-run the five scanners' own tests.
- **`[AUG]`** `mod.rs`'s doc carries: the §0 boundary against `completion_replay::retention` and
  `result_index::retention`; the out-of-scope upstream symbol table; the corrected grace semantics;
  and the note that `ASYNC_RETENTION_DELAY_MS` is a post-install delay, not an inter-batch pause.
- **`[AUG]`** Every `[CYRUP-DELTA]` in §SUBTASK2's type-delta list is written into a doc comment
  where the guard is, not only in this task file.
- Workspace `--no-fail-fast` 0 failed; clippy exit 0.
  - **`[AUG]`** clippy must also be clean under the crate's `#![deny(clippy::unwrap_used,
    expect_used, panic, indexing_slicing)]` (`lib.rs:19-24`) and `#![forbid(unsafe_code)]` (`:25`),
    plus the workspace `[workspace.lints.clippy]` block (root `Cargo.toml`) which denies the same
    four and warns `return_self_not_must_use`.

## Research notes

* Upstream: `pi-subagents/src/runs/background/async-retention.ts` — **verified 912 LOC at tag
  `v0.67.0`**; the draft's `HEAD 7fe9dee1` is a real commit but not a tag. Read everything at
  `v0.67.0`. The discovery half is a **separate file**, `async-retention-discovery-worker.mjs`
  (180 LOC, repo root), which the draft does not mention and which is where `scan.rs`'s entire
  mechanism lives.
* The shared-root fact: `background/artifact_roots.rs:276-293` (the doc note) and `:294-299`
  (`RunArtifactRoots`) — **not** `:281-284`, which is mid-comment.
* Functional-core precedent to copy: **`background/delivery/custody.rs:33-69`** (`Attribution`,
  three variants, pure `classify`) plus `background/delivery/mod.rs:23-28` (the functional-core
  rule). `background/delivery/disposition.rs` **does not exist**.
* Error-policy module to reuse: `background/result_index/errno.rs` — **present under that name**
  (the draft's `io_class.rs` question is settled). All four predicates are `pub(crate)`:
  `:22`, `:31`, `:47`, `:57`.
* Do **not** confuse with `result_index::cleanup_result_indexes`
  (`background/result_index/retention.rs:43`; 24 h at `:16`; `result-index/` only).
* Do **not** confuse with `completion_replay::cleanup_completion_replay_if_due`
  (`background/completion_replay/retention.rs:56`) — §0.
* Wait-subscription coupling: `background/wait_subscriptions/mod.rs:127-134` names this task as the
  owner; seams in §5.
* `PARITY-GAPS.md` row: `docs/gap-analysis/PARITY-GAPS.md:60`.
