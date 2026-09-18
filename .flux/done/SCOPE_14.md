---
stage: qa
status: completed
updated: 2026-09-16 09:00
---

# SCOPE_14 — async retention, part B: sweep, report, wiring

OBJECTIVE: complete the async-root reaper — execute the sweep, report its outcome, and schedule it.
This is the task that turns SCOPE_13's module on.

**Depends on SCOPE_13.**

---

## 0. AUGMENTATION PREFACE — read this before the subtasks

This file was a 95-line intent draft. Every citation below was re-opened in the tree on
2026-09-15 at branch `claude/subagents-scope-3` (cut from `main` @ `b2fdc7e`, after PR #137 and
PR #139 landed). Upstream was read **only** via
`git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>` — never from a working tree, never
from an unpinned HEAD.

**Upstream pin.** `v0.67.0` (tag object; `git rev-parse HEAD` = `13f8f3286c7ce42d0aecfd72bca76b26912b1445`).
`git diff v0.67.0 HEAD -- src/runs/background/async-retention.ts` is **empty**, so `v0.67.0` and
HEAD are byte-identical for this file. `git show v0.67.0:src/runs/background/async-retention.ts | wc -l`
= **912**, matching the draft's LOC claim. The draft's `HEAD 7fe9dee1` is a real commit
(`git cat-file -t 7fe9dee1` → `commit`) but is NOT the revision to cite; **cite `v0.67.0`.**

### 0.1 Five claims in the draft that are FALSE against the code. Do not implement them as written.

Each is restated in place below, next to the mechanism that actually works. Nothing is dropped —
the *objective* each claim was serving is preserved; only the *mechanism* changes.

| # | Draft claim | What the code says |
|---|---|---|
| D1 | "batches … separated by `ASYNC_RETENTION_DELAY_MS` … a 60 s pause per 100-run batch" | `ASYNC_RETENTION_DELAY_MS` is **never read inside `async-retention.ts`**. `git grep` over `v0.67.0` finds exactly one consumer: `src/extension/index.ts:613`, where it is the `setTimeout` delay of a **one-shot, post-activation** schedule. `cleanupAsyncRetention` does **one** bounded batch and returns (`:711-712`, `:756`, `:839`). There is no inter-batch sleep anywhere. See §1.4. |
| D2 | "A run tree's own `status.json` is the last thing removed" | Upstream never orders deletions. It **renames the whole run dir** to `.deleting-run-<uuid>` (`:807-816`) and then `fs.rmSync(tombstone, {recursive:true})` (`:830`). `rmSync(recursive)` / `tokio::fs::remove_dir_all` give **no** ordering guarantee. Crash-safety comes from the rename, not from a delete order. See §1.5. |
| D3 | "`the_sweep_never_touches_the_results_dir` — it reaps async run trees, not payloads" | Upstream's second half (`:839-896`) deletes candidates **inside `resultsDir`** — kinds `public`, `pending`, `replay`, `archive`, `tombstone` — and `resultsDir` is a **required** option (`:83`). The draft's invariant is the opposite of upstream. See §2.3: the invariant is kept, but only by explicitly scoping SCOPE_14 to the RUN half and recording the results half as a named gap. |
| D4 | "`AsyncRetentionResult` — what was scanned, kept, tombstoned, reaped" and "`scanned == kept + tombstoned + reaped`" | `AsyncRetentionResult` (`:103-119`) has **14** fields and there is no `kept`. The accounting identity as stated **does not hold**: a candidate whose per-entry `try` throws (`:834`, `:892`) increments `scanned` and **neither** a `skipped` bucket nor a deleted counter. See §3.2 for the identity that is actually true. |
| D5 | "three independent detached tasks racing the same roots … rather than a third ad-hoc `tokio::spawn`" and "`CLAUDE.md` records [the I/O storm]" | There is exactly **one** detached task today — `spawn_retention_sweep` (`notices.rs:648-682`) already runs `cleanup_result_indexes` **and** `cleanup_completion_replay_if_due` sequentially inside one `tokio::spawn`. The shared helper the draft asks for **already exists**. Also: **there is no `CLAUDE.md` anywhere in this repo** (`find . -name CLAUDE.md -not -path ./tmp/*` → empty). See §4. |

### 0.2 Two further unsupported claims (carried from SCOPE_13's research notes)

* **"340 leaked directories measured at diagnosis."** `grep -rn "340" .flux/ docs/gap-analysis/PARITY-GAPS.md` finds no such measurement anywhere in the repo. The unbounded-growth *argument* stands on its own (`artifact_roots.rs:278-281`, quoted in §1.1); the **number does not**. Do not repeat "340" in a doc comment or a test name. The DoD line that named it is restated in §7 without it.
* **`artifact_roots.rs:281-284`** (SCOPE_13's citation for the shared-root fact) is off by three lines. The sentence is at **`background/artifact_roots.rs:278-281`**, verified.

### 0.3 Is this already implemented? **No.**

`grep -rn "async_retention" crates/ --include=*.rs` returns **zero hits**. `ls src/background/`
shows no `async_retention` directory. `alreadyImplemented = false`.

---

## 0.4 WHAT THIS TASK REQUIRES OF SCOPE_13 (part A) — the contract

SCOPE_13 is being augmented in parallel. This task cannot be written against guesses, so the
following is stated as a **hard interface contract**. If part A lands differently, SCOPE_14's first
commit is the adapter, not a rewrite.

**R13-1 — module path and registration.** `crates/cyrup-ext-subagents/src/background/async_retention/`
with `mod.rs` declared in `background/mod.rs`'s `pub mod` block (alphabetically it lands between
`atomic` (`background/mod.rs:36`) and `auto_drain` (`:37`) — the block at `:36-51` is sorted).

**R13-2 — constants, as `pub const`, on the `i64`/`usize` shapes the rest of the crate uses**
(`completion_replay/mod.rs:67-80` is the pattern: `pub const CLEANUP_INTERVAL_MS: i64 = 60_000;`):

```rust
pub const ASYNC_RETENTION_DAYS: i64 = 30;              // upstream :14
pub const ASYNC_RETENTION_BATCH_SIZE: usize = 100;     // upstream :15
pub const ASYNC_RETENTION_DELAY_MS: i64 = 60_000;      // upstream :16  (see D1: a SCHEDULE delay)
pub const ASYNC_RETENTION_TOMBSTONE_GRACE_MS: i64 = 24 * 60 * 60 * 1000; // upstream :17
pub const RETENTION_MS: i64 = ASYNC_RETENTION_DAYS * 24 * 60 * 60 * 1000; // upstream :19
```

**R13-3 — `policy.rs` exposes a pure, I/O-free classifier** taking an already-gathered facts struct
and returning a **three-outcome enum** (SCOPE_13 SUBTASK2). SCOPE_14 requires that the facts struct
carry, at minimum, the inputs upstream's `runSkipReason` (`:279-305`) consults for a RUN candidate:
`status` presence/validity, `run_id`, directory basename, terminal-ness, `mode`/workflow linkage,
nested references, mission binding, handoff resolution, resumable contract, the timestamp, the
cutoff, **and a `protected: bool` / `wait_referenced: bool` pair**. SCOPE_14 supplies the last two
(SUBTASK4); part A must have left the *slots*.

**R13-4 — `policy.rs` returns a NAMED SKIP REASON, not just "keep".** Upstream's skip reasons are
the report's whole content (`result.skipped` is `Record<string, number>`, `:110`, incremented at
`:782`, `:787`, `:791`, `:797`, `:827`). A bare `Keep` with no reason makes §3 unimplementable. The
19 run-side reason strings that must be representable, verbatim from `v0.67.0`:
`invalid-status` (`:289`), `identity-mismatch` (`:290`, `:291`), `runtime-reference` (`:292`),
`wait-reference` (`:293`), `active-index` (`:294`), `non-terminal` (`:295`), `workflow-reference`
(`:296`), `nested-reference` (`:297`), `mission-reference` (`:298`), `handoff-reference` (`:299`),
`resumable` (`:300`), `unknown-age` (`:302`), `recent` (`:303`), plus the sweep-side
`unsafe-run-path` (`:764`), `run-tombstone-marker` (`:787`), `tombstone-grace` (`:791`), `lock-busy`
(`:702`), `lock-owner-changed` (`:744`, `:903`), `wait-references-unknown` (`:753`), `cancelled`
(`:697`), `commit-failure` (`:898`), and the `recheck-<reason>` prefixing at `:797`/`:827`.

**R13-5 — `scan.rs` yields at most `ASYNC_RETENTION_BATCH_SIZE` candidates per call AND carries a
resumable cursor.** This is the one part-A property SCOPE_14 cannot work around. Upstream persists
`RetentionCursor` (`:31-40`) in `.async-retention-cursor.json` (`:21`) and applies
`discovery.cursorOps` at the end of a successful pass (`:906-907`). **Without a cursor, a root whose
first 100 directory entries are all `recent` never advances past entry 100 and entry 101 is never
evaluated — the reaper silently does nothing forever on exactly the large root it exists for.** If
part A landed `scan.rs` without a cursor, SCOPE_14 must add it, and that is the single largest
risk in this task. `runAfter` is the only cursor key the RUN half needs.

**R13-6 — `tombstone.rs` exposes marker write/read/remove/matches** mirroring `:213-256`:
`writeRunTombstoneMarker`, `removeRunTombstoneMarker`, `readRunTombstoneMarker` (tri-state:
`Some(marker)` / `None` / `Unreadable`), `runTombstoneMarkerMatches`, `runTombstoneMarkerBlocks`,
plus the `.deleting-run-` prefix constant (`:23`). The tri-state is load-bearing: `:248` treats
**unreadable as blocking**, which is the fail-closed direction.

**R13-7 — `scan.rs` reuses `background::result_index::errno`.** Confirmed present and named
`errno` (the SCOPE_1 rename has happened): `background/result_index/errno.rs` exposes
`pub(crate) fn is_unaddressable` (`:22`), `is_absent` (`:31`), `is_access_denied` (`:47`),
`is_ignorable_listing_error` (`:57`). All four are `pub(crate)`, so `async_retention` can use them
**without any visibility change**.

---

## SUBTASK1 — `async_retention/sweep.rs`

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"Drives `scan.rs` → `policy.rs` → deletion, in
> batches of `ASYNC_RETENTION_BATCH_SIZE` separated by `ASYNC_RETENTION_DELAY_MS`. The delay is
> between **batches**, not between files — a 60 s pause per file would never finish; a 60 s pause
> per 100-run batch is the load-shedding upstream intends."*

### 1.1 Why this module exists at all (the doc-comment requirement)

`background/artifact_roots.rs:271-292` is the justification, and it must be quoted (not
paraphrased) in `async_retention/mod.rs`'s module doc. Verified text at **`:278-281`**:

> The key is [`cwd_key`] — the working directory, and **nothing else**. Running several cyrup
> instances in one project is ordinary, not an edge case, and they all share these roots byte for
> byte. Anything written here is therefore visible to all of them, and anything one of them
> deletes is gone for all of them.

`RunArtifactRoots` (`artifact_roots.rs:293-299`) is `{ async_root: PathBuf, results_dir: PathBuf }`;
`run_artifact_roots_in` (`:318-325`) builds them as `run_scratch()/async/<cwd_key>` and
`run_scratch()/results/<cwd_key>`. Nothing in the tree ever removes a run directory from
`async_root` on a schedule — `grep -rn "remove_dir_all" src/background/` finds only
`ensure_accessible_dir`'s broken-ACL recovery (`artifact_roots.rs:423`). That is the unbounded
growth. **State it that way; do not cite a measured directory count (§0.2).**

Also state, as SCOPE_13's module doc must: `async-retention.ts` contains **zero** `sessionId`
references (`git grep -c sessionId v0.67.0 -- src/runs/background/async-retention.ts` → no match).
It is in the session-scoping programme because the directory is *shared*, not because it is
session-scoped.

### 1.2 The signature

Model on the existing in-crate options-struct convention (`completion_replay::CompletionReplayWrite`,
`store.rs`; `result_index::ResultWrite`, `write.rs`): a borrowed-field input struct, so a 12-argument
function never happens.

```rust
/// pi `AsyncRetentionOptions` (`async-retention.ts:81-101` @ v0.67.0).
pub struct AsyncRetentionOptions<'a> {
    /// `<temp_root_dir>/async/<cwd_key>` — `RunArtifactRoots::async_root`.
    pub async_root: &'a Path,
    /// `<temp_root_dir>/results/<cwd_key>` — `RunArtifactRoots::results_dir`.
    /// Read-only in SCOPE_14 (§2.3): consulted for `mission-reference` and `replay-reference`,
    /// never written.
    pub results_dir: &'a Path,
    /// `wait_subscriptions_dir_in(&roots, cwd)` (`artifact_roots.rs:336-341`). pi `:84`.
    pub wait_subscriptions_dir: &'a Path,
    /// SUBTASK4. pi `:85` `protectedRunIds`.
    pub protected_run_ids: &'a std::collections::HashSet<RunId>,
    /// Epoch millis, injected — `crate::time::now_epoch_millis()` at the one production call site.
    pub now: i64,
    pub retention_ms: i64,              // pi `:87`, default RETENTION_MS
    pub tombstone_grace_ms: i64,        // pi `:88`, default ASYNC_RETENTION_TOMBSTONE_GRACE_MS
    pub batch_size: usize,              // pi `:89`, clamped — see 1.3
    /// pi `:91`. See §1.6 — cyrup MUST NOT use upstream's `path.dirname(asyncDirRoot)` default.
    pub maintenance_root: &'a Path,
}

pub async fn cleanup_async_retention(options: &AsyncRetentionOptions<'_>) -> AsyncRetentionResult;
```

**Never returns `Result`.** Upstream returns the report unconditionally (`:650`, `:688-693`) and
every per-candidate failure is captured into `result.errors` (`:836`, `:894`). This matches
`DoctorRunner::run`'s own "no `Result` return type at all" discipline (`registration/doctor.rs:284`
and the reasoning at `:280-283`).

`RunId` is `background/run_id.rs:30` — `pub struct RunId(std::sync::Arc<str>)`, deriving
`Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize` (`:26-30`), so
`HashSet<RunId>` is available with no newtype work. `RunId::from_token` (`:50`) validates nothing —
the `valid_run_id` guard of `:170-172` (non-empty, not `.`, not `..`, `basename(x) == x`) must
therefore be ported explicitly, in part A's policy or here.

### 1.3 Batch clamping — port `:656` exactly

```ts
const batchSize = Math.min(ASYNC_RETENTION_BATCH_SIZE, Math.max(1, Math.trunc(options.batchSize ?? ASYNC_RETENTION_BATCH_SIZE)));
```

`batch_size.clamp(1, ASYNC_RETENTION_BATCH_SIZE)` on a `usize`. A caller can only ever make the
batch **smaller**, never larger — a `batchSize: 10_000` is silently 100. Then `:711-712`:
`run_budget = batch_size.div_ceil(2)`, `result_budget = batch_size - run_budget`. In SCOPE_14's
run-only scope (§2.3) the run budget is the whole `batch_size`; **record the divergence as a
`[CYRUP-DELTA]` doc line** so the day the results half lands, the split is restored rather than
re-derived.

### 1.4 D1 — the delay is a SCHEDULE, not an inter-batch sleep

**Evidence.** `git grep -n ASYNC_RETENTION_DELAY_MS v0.67.0 -- '*.ts' '*.mjs'` returns exactly two
lines outside the module's own `export`:

```
v0.67.0:src/extension/index.ts:42:  import { ASYNC_RETENTION_DELAY_MS, cleanupAsyncRetention } from "../runs/background/async-retention.ts";
v0.67.0:src/extension/index.ts:613: }, ASYNC_RETENTION_DELAY_MS);
```

and `src/extension/index.ts:597-614` is:

```ts
const asyncRetentionAbort = new AbortController();
const asyncRetentionTimer = setTimeout(async () => {
    try { await cleanupAsyncRetention({ asyncDirRoot: DIRS.async, resultsDir: DIRS.results,
        signal: asyncRetentionAbort.signal,
        protectedRunIds: new Set([...state.asyncJobs.keys(), ...(state.workflowControllers?.keys() ?? []), ...scheduledRunManager.referencedAsyncRunIds()]) });
    } catch (error) { console.error("Failed to clean retained async subagent state:", error); }
}, ASYNC_RETENTION_DELAY_MS);
asyncRetentionTimer.unref?.();
```

One shot. One pass. `unref`'d. There is no loop and no second timer.

**THE MECHANISM THAT ACTUALLY MEETS THE DRAFT'S OBJECTIVE.** The draft's stated goal — *"the reaper
must never stall a live session"*, *"a 60 s pause per file would never finish"* — is achieved
upstream by **bounding the pass, not by pausing inside it**:

1. one invocation evaluates **at most `batch_size` candidates** and returns (`:756`, `:839`);
2. the persisted cursor (R13-5) makes the *next* invocation start where this one stopped;
3. the invocation is scheduled **once**, `ASYNC_RETENTION_DELAY_MS` after install, detached and
   unref'd, so it never sits in front of session start.

A sleeping loop inside the sweep would be strictly worse than upstream here: `spawn_retention_sweep`
is **aborted on watcher teardown** (`notices.rs:608-610`), so a task parked in a 60 s `tokio::time::sleep`
between batches would be killed mid-pass on any session that ends inside the window, leaving a
`.deleting-run-*` tombstone and an uncommitted cursor every time. Implement the bounded pass.

### 1.5 D2 — crash-safety is the RENAME, not a deletion order

**Evidence, `:807-833`** (non-tombstone run candidate):

```ts
const tombstone = path.join(options.asyncDirRoot, `${RUN_TOMBSTONE_PREFIX}${randomId()}`);
writeRunTombstoneMarker(maintenanceRoot, status!.runId, tombstone, currentTime);
mutationStateChanged = true;
try { fs.renameSync(runDir, tombstone); }
catch (error) { removeRunTombstoneMarker(maintenanceRoot, status!.runId); mutationStateChanged = false; throw error; }
const recheckStatus = readStatus(tombstone);
const freshWaitReferences = parseWaitRunIds(waitSubscriptionsDir);
const recheckReason = freshWaitReferences.safe ? runSkipReason({ runDir: tombstone, status: recheckStatus, … }) : "wait-references-unknown";
if (recheckReason) {
    if (!fs.existsSync(runDir)) { fs.renameSync(tombstone, runDir); mutationStateChanged = false; }
    removeRunTombstoneMarker(maintenanceRoot, status!.runId);
    if (mutationStateChanged) cursorCommitUnsafe = true;
    increment(result.skipped, `recheck-${recheckReason}`); continue;
}
fs.rmSync(tombstone, { recursive: true });
removeRunTombstoneMarker(maintenanceRoot, status!.runId);
result.deletedRuns += 1;
```

The order is **marker → rename → re-check → recursive delete → marker removal**, and the recursive
delete makes no promise about `status.json`. `tokio::fs::remove_dir_all` makes none either.

**THE MECHANISM THAT ACTUALLY MEETS THE DRAFT'S OBJECTIVE.** The draft wanted *"a crash mid-delete
leaves a directory that still reads as a run and gets re-evaluated next sweep, rather than a
half-deleted tree that reads as corrupt."* That is exactly what rename-first buys, and more
cleanly:

* a crash **before** the rename leaves the run dir untouched — re-evaluated next pass, indistinguishable from never having been a candidate;
* a crash **after** the rename and before/during the recursive delete leaves `.deleting-run-<uuid>` plus its marker. `:785-806` re-discovers that directory next pass, requires the marker to match (`runTombstoneMarkerMatches`, `:786`), requires `tombstoneGraceMs` to have elapsed since its mtime (`:790`), re-reads the wait references (`:794`), and only then completes the delete as `reapedTombstones`;
* `runSkipReason`'s identity check (`:291`) **exempts** names starting with `RUN_TOMBSTONE_PREFIX` precisely so a renamed tree is still attributable to its `status.runId`;
* an ORPHANED tombstone whose marker is gone is skipped as `run-tombstone-marker` (`:787`) rather than deleted — fail-closed.

`tokio::fs::rename` within one directory is the same single `rename(2)`. Note it **can** fail with
`EXDEV` if `async_root` straddles a mount, which is why the `catch` at `:812-816` rolls the marker
back — port that arm, do not `unwrap`.

> **The test named `status_json_is_removed_last` cannot be written against a `remove_dir_all`, and
> a hand-rolled ordered delete would be strictly worse than the rename** (it re-introduces the
> half-deleted tree the rename exists to prevent). It is replaced in §6 by
> `an_interrupted_delete_leaves_a_prefixed_tombstone_the_next_pass_reaps`, which pins the same
> crash-safety property against the mechanism that provides it.

### 1.6 A CYRUP-ONLY DEFECT THE LITERAL PORT WOULD INTRODUCE: `maintenance_root`

Upstream defaults `maintenanceRoot` to `path.dirname(options.asyncDirRoot)` (`:658`). Upstream's
`ASYNC_DIR` is **flat** (`shared/types.ts`; noted at `artifact_roots.rs:255-262`:
*"pi's `ASYNC_DIR`/`RESULTS_DIR` are FLAT — every project's runs share one directory"*), so its
`dirname` is a per-installation directory and the lock/cursor/log/markers it holds are correctly
scoped.

**cyrup's `async_root` is `<run_scratch>/async/<cwd_key>` (`artifact_roots.rs:322`), so
`dirname(async_root)` is `<run_scratch>/async` — SHARED BY EVERY cwd in the scope.** Taking the
literal default would put:

* `.async-retention.lock` (`:20`) in a directory shared across cwds → project A's sweep blocks
  project B's for the whole pass, reported as `lock-busy`, forever;
* `.async-retention-cursor.json` (`:21`) shared across cwds → project A's cursor advances past
  project B's entries, so project B's tail is **never scanned**;
* `async-retention-run-tombstones/` (`:25`) keyed by `encodeIndexSegment(runId)` with no cwd
  component → two cwds' runs can collide on one marker path.

This is the identical failure mode `results_dir_for_async_root` already documents at
`artifact_roots.rs:372-383`: *"which is exactly what C7's pre-fix `async_root.parent()/results`
derivation got wrong (it dropped the `<cwd_key>` and nested `results` UNDER `async` instead of
beside it)."*

**Required:** `maintenance_root` has **no default derived from `dirname`**. It is a required field
of `AsyncRetentionOptions`, and the production call site passes a cwd-keyed path. Recommended
value, matching `wait_subscriptions_dir_in`'s own shape (`artifact_roots.rs:336-341`):

```rust
roots.run_scratch().join("async-retention").join(cwd_key(cwd))
```

added as `pub fn async_retention_root_in(roots: &crate::paths::Roots, cwd: &Path) -> PathBuf`
beside it, re-exported from `background/mod.rs:98-102` alongside `wait_subscriptions_dir_in`.
(`cwd_key` is `pub(crate)`, `artifact_roots.rs:263`, and is already `pub(crate) use`d at
`background/mod.rs:103` — no visibility change needed.) The alternative, `async_root` itself, is
**rejected**: the lock and markers would then be directory entries inside the very directory the
scan enumerates, and every scan would have to special-case them the way
`terminal_run_index::is_reserved_async_root_entry` (`terminal_run_index/mod.rs:73-75`) already
special-cases `.terminal-runs` and `.active-runs`.

### 1.7 The cross-instance lock — REQUIRED, and it is inside "execute the sweep"

The draft does not mention it; it is not new scope, it is the precondition for §1.1's shared-root
fact. Upstream `:397-434` + `:701-705` + `:743-746` + `:902-905` + `:909-911`:

* `acquireRetentionLock` (`:413-429`) loops **at most 4 times**: `mkdirSync(lockDir, {mode:0o700})`
  is the acquire (an `EEXIST` is the contended case, `:401`); on contention it evaluates
  `staleLock` and, if stale, `renameSync`s the lock dir aside to `${lockDir}.stale-<sanitised token>`
  and retries. Not stale → `false` → the pass records `lock-busy` and returns.
* `staleLock` (`:378-395`) is **hostname-scoped** (`:387` — a foreign host's lock is never stale),
  then pid-liveness, then `processStartIdentity` (pid-reuse detection), then a 24 h
  `LOCK_STALE_MS` floor (`:26`).
* the owner token is re-verified **twice** after the work (`:743`, `:902`) — a pass that lost the
  lock mid-flight refuses to commit its cursor.
* release (`:431-434`) is token-checked and lives in a `finally` whose own throw is swallowed
  (`:910` — *"a stale lock blocks the next pass safely"*).

**cyrup mapping.** `std::fs::create_dir` / `tokio::fs::create_dir` on a non-existent dir is the
`mkdir` acquire; `io::ErrorKind::AlreadyExists` is `EEXIST`. Liveness has a real in-crate
implementation to reuse rather than reinvent: `background/reconcile.rs`'s
`pub enum Liveness { Alive, Dead, Unknown }` (`:70-83`) and `check_pid_liveness`, with
`Liveness::is_possibly_alive` (`:86-89`) already encoding R-SA-089's *"Unknown MUST NOT be treated
as dead"* — which is exactly upstream's `alive === false` (`:389`) vs `alive === undefined`
distinction. **Use `Liveness`; do not write a second `kill(pid, 0)`.** `processStartIdentity`
(`:325-350`, Linux `/proc/<pid>/stat` field 19) has no cyrup analogue and this crate is
`#![forbid(unsafe_code)]`; reading `/proc/<pid>/stat` with `tokio::fs::read_to_string` is safe Rust
and is the port — but see §8 Q3 for whether to land it now or take the 24 h floor alone.

`0o700` needs `std::os::unix::fs::DirBuilderExt`; the crate already has a Windows-safe precedent in
`atomic.rs`'s `write_private_atomic_json_blocking` (`:209`) — read it before choosing the cfg shape.

### 1.8 Per-candidate loop — the exact order of operations (port of `:756-838`)

For each of the ≤ `batch_size` run candidates `scan.rs` yields:

1. `result.scanned += 1` **first** (`:758`) — before any fallible work, so `scanned` counts
   candidates *considered*, not candidates *successfully processed*. §3.2 depends on this.
2. `lstat` the run dir; `!is_dir() || is_symlink()` → `unsafe-run-path` (`:762-766`). In Rust:
   `tokio::fs::symlink_metadata` (**not** `metadata` — `metadata` follows the link and defeats the
   check), then `file_type().is_dir()`; `symlink_metadata` already reports the link itself, so
   `is_dir()` alone is sufficient and `is_symlink()` is implied false.
3. read `status.json`. `RunPaths::status` is `<run_dir>/status.json` (`run_paths.rs:87-88`,
   `:62-64`, const at `:13`). The parsed type is `background::records::RunStatus` (`records.rs:210`),
   carrying `run_id: RunId` (`:212`), `session_id: Option<SessionId>` (`:230`), `mode: RunMode`
   (`:241`), `state: RunState` (`:243`), `pid: Option<u32>` (`:251`),
   `session_file: Option<PathBuf>` (`:268`), `started_at: i64` (`:270`),
   `ended_at: Option<i64>` (`:273`), `last_update: i64` (`:276`), `steps: Vec<StepStatus>` (`:289`).
4. **the `running` repair hop** (`:768-779`): if `status.state == running`, the run id is valid, the
   basename matches, and the id is not protected, upstream calls `reconcileAsyncRun` and counts a
   `repairedRuns`. cyrup's analogue is `background::reconcile::reconcile_now(paths, spawn_confirmed_at)`
   (`reconcile.rs:331-345`) → `io::Result<ReconcileOutcome>` (`:148`). See §8 Q4 — this hop is what
   makes a crashed run's tree reapable at all, so dropping it silently would defeat the task.
5. `policy.rs` (part A) with `protected` and `wait_referenced` from §5; a `Some(reason)` increments
   `skipped[reason]` and `continue`s.
6. if the basename starts with `.deleting-run-`: marker-match → grace → **re-read the wait
   references** → re-run the policy → `remove_dir_all` → remove marker → `reaped_tombstones += 1`.
7. else: write marker → rename → re-read status from the tombstone → **re-read the wait references**
   → re-run the policy → on `Some(reason)`: rename back if the original path is free, remove the
   marker, `skipped["recheck-" + reason] += 1`; on `None`: `remove_dir_all` → remove marker →
   `deleted_runs += 1`.
8. any error in 2-7 → if the tree was mutated, set `cursor_commit_unsafe`; if the error is not
   "absent", push `compact_error(e)` into `errors` (`:834-837`).

`compactError` (`:125-128`) is `message.replace(/\s+/g, " ").slice(0, 240)` — port it, including
the **240-char** cap; an unbounded error string in a report that gets serialised is an unbounded
file. Note `slice(0, 240)` is UTF-16 code units in JS; in Rust take 240 **bytes** on a char
boundary (`char_indices`), never `&s[..240]` — the crate denies `clippy::indexing_slicing`.

The `isNotFound` filter at `:836` maps to `errno::is_absent` (`errno.rs:31`), **not**
`is_ignorable_listing_error` — a permission fault on a specific run dir is a real fault the report
must carry, per `errno.rs:43-49`'s own "quiet when scanning, loud when asked a direct question".

### 1.9 `cursor_commit_unsafe` (`:687`, `:826`, `:835`, `:885`, `:893`, `:897-900`)

If any candidate mutated the tree and then failed or bailed, the cursor is **not** committed and
`commit-failure` is recorded. Rationale: the cursor says "everything before this point is handled";
a pass that left a `.deleting-run-*` behind has not handled it, and advancing past it would strand
the tombstone until the cursor wrapped. Port the flag exactly.

---

## SUBTASK2 — `async_retention/report.rs`

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"`AsyncRetentionResult` (`async-retention.ts:103`)
> — what was scanned, kept, tombstoned, reaped, and why. Surfaced through the existing
> diagnostics/doctor path (`registration/doctor.rs`) rather than a new user-facing surface."*

### 2.1 The real shape (`:103-119`, verified)

```ts
export interface AsyncRetentionResult {
    acquired: boolean;  scanned: number;  repairedRuns: number;  deletedRuns: number;
    deletedResults: number;  reapedTombstones: number;
    skipped: Record<string, number>;  errors: string[];
    rawReads: number;  sourceExhausted: Record<string, boolean>;
    discoveryDurationMs: number;  commitDurationMs: number;
    cancelled: boolean;  workerFailed: boolean;  durationMs: number;
}
```

Fourteen fields, **no `kept`**. "Kept" is `skipped` — a map from reason to count, not a scalar.

`rawReads`, `sourceExhausted`, `discoveryDurationMs`, `workerFailed` and `cancelled` are artefacts
of upstream's **worker-thread discovery** (`:587-634`: a `node:worker_threads` `Worker` runs the
directory walk off the commit thread, and `parseDiscoveryResult` at `:551-585` re-validates every
field it returns because it crosses a trust boundary). cyrup's scan is in-process
`tokio::fs`, so:

* `worker_failed` — **omit**, with a `[CYRUP-DELTA]` doc line naming `:732`. There is no worker.
* `cancelled` — **keep**, but sourced from `tokio_util`-free cooperative cancellation: the
  `spawn_retention_sweep` task is `abort()`ed (`notices.rs:609`), so a cancelled pass is a dropped
  future. Record the field and set it only if a caller threads an explicit flag; otherwise leave it
  `false` and say why. See §8 Q5.
* `raw_reads`, `source_exhausted` — **keep**, cheap and real: `raw_reads` is the count of
  `read_dir`/`read`/`metadata` calls the scan made, `source_exhausted` is per-source
  ("run" → the async root listing was walked to the end this pass). They are the only signal that
  distinguishes "nothing to do" from "budget exhausted, more next pass", which is what makes the
  doctor line in §2.4 worth anything.
* `discovery_duration_ms` / `commit_duration_ms` / `duration_ms` — keep. `std::time::Instant`
  deltas; **never** `now_epoch_millis()` differences, which can go backwards on a clock step.
  (`:689-690` uses `Math.max(0, …)` for exactly that reason; `Instant` removes the need.)
* `deleted_results` — keep the field, always `0` in SCOPE_14, with the `[CYRUP-DELTA]` in §2.3.

Derive `Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize`, matching
`DoctorReport` (`registration/doctor.rs:195-197`). `skipped: BTreeMap<String, usize>` — **`BTree`,
not `Hash`** — so a serialised report and a rendered doctor line are deterministic, which is the
same reason `DoctorReport`'s doc at `:191-195` gives for its fixed check order.

### 2.2 `increment` and the maintenance log

`increment` (`:536-538`) is `*record.entry(key).or_default() += 1`.

`appendMaintenanceLog` (`:513-534`) appends **one JSON line per pass** to
`<maintenance_root>/async-retention-maintenance.jsonl` (`LOG_NAME`, `:22`), including
`deleted: deletedIds.slice(0, ASYNC_RETENTION_BATCH_SIZE)` — a bounded list of
`run:<id>` / `run-tombstone:<id>` strings (`:804`, `:833`). It is written **unconditionally**, from
`finish()` (`:691`), including on the `lock-busy` early return (`:703`).

**This file is the seam that makes SUBTASK2 possible at all** (§2.4). Append with
`tokio::fs::OpenOptions::new().create(true).append(true)`; the crate has an append-with-cap
primitive, `crate::jsonl::BoundedJsonlWriter` (`jsonl.rs:68` documents a `cap_bytes` a diagnostic
check can observe), and `run_paths.rs:89-93` records the crate rule that *"Any writer appending to
this path MUST go through `crate::jsonl::BoundedJsonlWriter`"* for `events.jsonl`. Upstream's log is
**uncapped** — an unbounded append in a shared directory that nothing ever truncates. Use
`BoundedJsonlWriter` and record the `[CYRUP-DELTA]`; an uncapped maintenance log in the exact
directory this task exists to bound would be self-defeating.

### 2.3 D3 — the results half, and how `the_sweep_never_touches_the_results_dir` becomes TRUE

Upstream `:839-896` deletes files in `resultsDir`. The draft asserts the sweep must not. Both cannot
hold. **Resolution: SCOPE_14 lands the RUN half only, and that is stated as scope, not discovered
as a bug.** Concretely:

* `AsyncRetentionOptions::results_dir` is **read-only** here. It is consulted for two guards and
  nothing else: `missionObserverIndexExists` (`:209-211`) → cyrup's
  `result_index::paths::mission_observer_path` (`paths.rs:262`, layout
  `<results_dir>/result-index/observers/mission/<seg>.json`, confirmed by its own test at `:341-342`);
  and `replay-reference` (`:505`) → `completion_replay::completion_replay_path`
  (`completion_replay/mod.rs:100-104`), which SCOPE_4 already landed and whose raw-encoder
  file-name rule (`mod.rs:82-96`) is documented as agreeing with `async-retention.ts:505` on purpose.
* `deleted_results` is permanently `0`; `the_sweep_never_touches_the_results_dir` is then a **real
  guard with a real failure mode**, not a tautology, because the two guards above do open paths
  under `results_dir`.
* The deferred half is recorded in `docs/gap-analysis/PARITY-GAPS.md:60` (which currently reads
  `` `async-retention.ts` (912 — the async-root reaper) `` in the still-open list) — that line is
  updated to say the run half is ported and the result half (`:839-896`, ~58 LOC plus
  `resultSkipReason` `:483-511`) is open, rather than deleted.

### 2.4 The doctor surface — the mechanism, and its honest limit

`registration/doctor.rs` verified shape:

* `CheckStatus { Ok, Warn, Fail }` (`:101`), `is_actionable` (`:115`).
* `DoctorCheck { name: String, status: CheckStatus, detail: String, remedy: Option<String> }`
  (`:132-146`), constructors `ok/warn/fail` (`:151`, `:162`, `:177`) — `ok` takes **no** remedy.
* `DoctorReport { checks: Vec<DoctorCheck> }` (`:197`), `all_ok` (`:205`), `actionable` (`:212`),
  `find(&str)` (`:221`).
* Stable check-name consts at `:231-245`: `CHECK_BINARY_RESOLUTION` … `CHECK_MODEL_SCOPE`.
* `DoctorRunner { async_root: PathBuf, config_json_path: PathBuf, discovery_config: AgentDiscoveryConfig, provider_catalog_path: Option<PathBuf> }` (`:257-263`).
* `DoctorRunner::run(&self) -> DoctorReport` (`:284-305`): a single `tokio::join!` of five futures
  whose results are assembled into a **fixed a..g order** vec at `:294-304`. It is `async`, never
  fails, and holds **no live process state**.

**Therefore the check cannot call the sweep and cannot read a `JobTracker`.** The only thing it can
do is read what the last pass left on disk. That is the maintenance log of §2.2. Required shape:

```rust
pub const CHECK_ASYNC_RETENTION: &str = "async-retention";
```

added to the const block at `:231-245`, an eighth check appended to the `tokio::join!` and to the
fixed vec at `:294-304` (**append — never reorder**, `:191-195`), and a new
`DoctorRunner` field `async_retention_root: Option<PathBuf>` — `Option`, for the same reason
`provider_catalog_path` is (`:260-263`: "`None` means … a normal, `Warn`-not-`Fail` condition").
Verdict ladder:

| condition | status | detail |
|---|---|---|
| no maintenance root configured, or no log file | `Ok` | "async retention has not run in this scope yet" — a fresh install is not actionable |
| last line parses, `acquired: true`, `errors` empty | `Ok` | scanned/deleted/reaped/top skip reason |
| last line parses, `errors` non-empty | `Warn` | first error, count; remedy names the log path |
| last line parses, `acquired: false` (lock-busy) for the most recent line | `Warn` | "another instance holds the retention lock"; remedy: the stale-lock path |
| log exists but the last line does not parse | `Warn` | never `Fail` — a diagnostic that cannot read its own diagnostic is not a broken installation |

**The honest limit, which must be written into the task's completion notes rather than discovered
later:** `DoctorRunner` has **no production caller**.
`grep -rn "DoctorRunner" --include=*.rs src/` returns exactly one hit outside `doctor.rs` itself —
a doc-comment reference at `extension/executor/reports.rs:27`. `registration/slash_commands.rs:54-55`
says so in as many words: the `/subagents-doctor` *execution body* is "separate" and unregistered.
So this check is reachable from `DoctorRunner::run` and from tests, and **not** from a user's
terminal today. Wiring `/subagents-doctor` is out of scope. Say this in the task notes; do not
claim an operator-visible surface.

---

## SUBTASK3 — schedule it

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"**Where:** `extension/executor/notices.rs`, beside
> the existing post-install detached schedules (`cleanup_result_indexes`, and SCOPE_4's
> `cleanup_completion_replay_if_due`). **Change:** schedule the sweep detached, after install, on
> the retention delay. Three cleanup jobs now share that seam — factor them into one scheduling
> helper rather than a third ad-hoc `tokio::spawn`. **Why a shared helper:** three independent
> detached tasks racing the same roots at session start is exactly the I/O storm that `CLAUDE.md`
> records as perturbing file watchers and socket timeouts. One scheduler, staggered."*

### 3.1 D5 — the shared helper already exists, and there is no `CLAUDE.md`

Verified at `extension/executor/notices.rs:648-682`:

```rust
pub(crate) fn spawn_retention_sweep(
    results_dir: std::path::PathBuf,
    delay: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        match crate::background::result_index::cleanup_result_indexes(
            &results_dir, crate::time::now_epoch_millis(),
            crate::background::result_index::DEFAULT_MAX_AGE_MS,
        ).await { … }
        crate::background::completion_replay::cleanup_completion_replay_if_due(
            &results_dir, crate::time::now_epoch_millis(),
            crate::background::watch::DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX),
            crate::background::completion_replay::CLEANUP_INTERVAL_MS,
        ).await;
    })
}
```

**One** `tokio::spawn`. **Two** sweeps already sequential inside it. `RETENTION_SWEEP_DELAY` is
`Duration::from_secs(30)` (`:615`, pi `extension/index.ts:451`). The handle is stored at
`notices.rs:510-512` into `SubagentExecutor::retention_sweep`
(`extension/executor/mod.rs:70`: `AsyncMutex<Option<tokio::task::JoinHandle<()>>>`, initialised
`:294`), the previous one `abort()`ed on re-install, and `abort()`ed again at
`stop_completion_watcher` (`:603-611`). `install_completion_watcher` arms it only inside the
`Ok(handle)` arm (`:507-513`) — "a degraded install has no session to sweep for".

There is no `CLAUDE.md` in the repo (`find . -name CLAUDE.md -not -path ./tmp/*` → empty). Drop the
citation; the staggering rationale is restated below from evidence that does exist.

**THE CHANGE, then, is not a refactor — it is a third stage inside the existing helper**, plus the
inputs that stage needs:

```rust
pub(crate) fn spawn_retention_sweep(
    results_dir: std::path::PathBuf,
    async_retention: Option<AsyncRetentionSchedule>,   // None = no async pass this install
    delay: std::time::Duration,                        // stage 1+2, = RETENTION_SWEEP_DELAY
    async_delay: std::time::Duration,                  // stage 3, = ASYNC_RETENTION_SCHEDULE_DELAY
) -> tokio::task::JoinHandle<()>
```

where `AsyncRetentionSchedule` is the owned bundle the async pass needs and the existing two do not:
`{ async_root, results_dir, wait_subscriptions_dir, maintenance_root, protected: Arc<JobTracker>, workflow_controllers: … }`.
All four paths are derivable at the one call site: `install_completion_watcher(&self, cwd: &Path)`
(`notices.rs:420`) already has `roots` (`:421`, from `config_snapshot().await.roots`) and `cwd`, and
`default_results_dir_in(&roots, cwd)` (`extension/executor/paths.rs:29-31`) is literally
`run_artifact_roots_in(roots, cwd).results_dir` — so `run_artifact_roots_in(&roots, cwd).async_root`,
`wait_subscriptions_dir_in(&roots, cwd)` (`artifact_roots.rs:336-341`) and
`async_retention_root_in(&roots, cwd)` (§1.6) come from the same two values.

### 3.2 The stagger, and the evidence for it

Sequential stages in one task are *already* staggered — they cannot overlap. What the third stage
adds is a **later start**, which is upstream's own choice: the index/replay sweep is armed at
**30 s** (`extension/index.ts:451`, `notices.rs:615`) and the async retention pass at **60 s**
(`ASYNC_RETENTION_DELAY_MS`, `extension/index.ts:613`). So:

```rust
/// pi `ASYNC_RETENTION_DELAY_MS` (`async-retention.ts:16`) as pi itself uses it —
/// `extension/index.ts:613`'s one-shot post-activation `setTimeout`, NOT an inter-batch pause.
const ASYNC_RETENTION_SCHEDULE_DELAY: std::time::Duration = std::time::Duration::from_secs(60);
```

and stage 3 sleeps `async_delay.saturating_sub(delay)` after stages 1-2, so the async pass begins
one full `ASYNC_RETENTION_SCHEDULE_DELAY` after install regardless of how long the first two took —
which is what makes `the_three_cleanup_jobs_are_staggered` assertable against a virtual clock.

The in-repo evidence for wanting the stagger at all (replacing the missing `CLAUDE.md` citation):
`notices.rs:620-627`'s own reasoning — *"the sweep must never hold the process open and must never
sit in front of session start"* — and `completion_replay/retention.rs:44-51`'s throttle argument,
*"whichever fires first does the work and the other is one map lookup"*. Both are about not paying
retention cost on the session-start path. Cite those.

### 3.3 The teardown contract is already correct and must stay correct

`stop_completion_watcher` (`:603-611`) `abort()`s the handle. With a third stage the abort can now
land **mid-pass**, after a rename and before the `remove_dir_all`. §1.5 shows that is safe by
construction — the `.deleting-run-*` tombstone plus its marker is the designed intermediate state
and the next pass completes it. **Write that sentence into `spawn_retention_sweep`'s doc comment**:
it is the reason the async stage is allowed to be abortable at all, and the reason §1.4 forbids a
sleeping loop inside the pass.

---

## SUBTASK4 — never reap a live run

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"The sweep consults the live `JobTracker`
> (`background/tracker.rs`) before reaping. A run this process is tracking is never a candidate
> regardless of age or on-disk state. **Why:** `policy.rs` has the liveness input but SCOPE_13 left
> it unwired; this is where the real tracker is threaded in."*

Upheld in full, and widened to the three sources upstream actually uses.

### 4.1 `JobTracker` — the real API

`background/tracker.rs`: `pub struct JobTracker { jobs: StdMutex<HashMap<RunId, TrackedJob>>, … }`
(`:144-147`). **`jobs` is PRIVATE** — the draft's `jobs.keys()` is not reachable from outside the
module. The public accessors are:

* `pub fn tracked_count(&self) -> usize` (`:208`)
* `pub fn snapshot(&self) -> Vec<TrackedJob>` (`:224-232`) — clones the whole map's values
* `pub fn get(&self, run_id: &RunId) -> Option<TrackedJob>` (`:235-243`)

`TrackedJob` (`:99-121`) is `{ run_id: RunId, paths: RunPaths, spawn_confirmed_at: Option<SystemTime>, last_status: Option<RunStatus>, events_cursor: u64, terminal_since: Option<Instant> }` —
`Clone`, `Arc`-free, deliberately cheap to snapshot (`:95-97`).

So the protected set is:

```rust
tracker.snapshot().into_iter().map(|job| job.run_id).collect::<HashSet<RunId>>()
```

**Lock discipline.** Every `jobs` access is documented as *"a short, synchronous, non-`.await`-spanning
lock acquisition"* with poison recovery `.unwrap_or_else(std::sync::PoisonError::into_inner)`
(`:208-218`, module note at `:33-37`). `snapshot()` releases the guard before returning, so the
`HashSet` may be built at any point — but **build it BEFORE the pass's first `.await` into the
filesystem and hold it as an owned value**, exactly as `completion_replay/retention.rs:62-72` takes
and drops its `MutexGuard` before `cleanup_completion_replay(...).await`. Never hold a
`std::sync::MutexGuard` across an `.await`; the crate denies `clippy::unwrap_used` and this is the
established shape.

**A terminal-but-still-tracked run counts as protected.** `TrackedJob::terminal_since` (`:117-121`)
means the tracker retains a finished job for `DEFAULT_RETENTION_WINDOW` before
`evict_expired_terminal_jobs` (`:502-510`) drops it. Upstream's `protectedRunIds` is
`state.asyncJobs.keys()` with no state filter (`extension/index.ts:605`), so **do not filter by
state** — the whole point is "this process still has a handle on it".

### 4.2 The other two protected sources upstream uses

`extension/index.ts:604-608` is `new Set([...state.asyncJobs.keys(), ...(state.workflowControllers?.keys() ?? []), ...scheduledRunManager.referencedAsyncRunIds()])`.

* **workflow controllers.** `SubagentExecutor::workflow_controllers` is
  `Arc<std::sync::Mutex<HashMap<crate::background::RunId, WorkflowController>>>`
  (`extension/executor/mod.rs:171-172`), keyed by `RunId` with the same short-critical-section
  `std::sync::Mutex` discipline (`:169-170`). Its keys **must** join the protected set: a live
  workflow shell's run tree being reaped mid-flight is the worst case this task can produce. There
  is no public keys accessor today — add a `pub(crate) fn live_workflow_run_ids(&self) -> HashSet<RunId>`
  in `extension/executor/workflow_controllers.rs` beside `abort_and_clear_workflow_controllers`
  (`:169`), using the same poison-recovery lock line.
* **`scheduledRunManager.referencedAsyncRunIds()`** — `grep -rn "referenced_async_run_ids\|observed_completion_run_ids"` over `src/` returns **nothing**. cyrup has no scheduled-run manager. Record as a
  `[CYRUP-DELTA]`; it contributes no ids.

### 4.3 `wait-reference` — the FOURTH guard, and it is fail-closed

Distinct from `protected_run_ids` and **not optional**. `parseWaitRunIds` (`:307-316`):

```ts
function parseWaitRunIds(dir: string): { runIds: Set<string>; safe: boolean } {
    const runIds = new Set<string>();
    for (const entry of listDir(dir)) {
        if (!entry.isFile() || !entry.name.endsWith(".json")) continue;
        const record = readJson(path.join(dir, entry.name));
        if (!record || typeof record.runId !== "string" || typeof record.expiresAt !== "number") return { runIds, safe: false };
        runIds.add(record.runId);
    }
    return { runIds, safe: true };
}
```

and its consumers: read once before the loop (`:751`), **`safe === false` aborts the WHOLE pass** as
`wait-references-unknown` (`:752-755`), and it is **re-read before every destructive action**
(`:794`, `:818`, `:861`, `:878`) with the comment at `:748-750`:

> This safety scan stays on the commit thread. Wait records are the active, swept subscription set.
> Each destructive action re-reads it to close the race with a newly armed wait.

SCOPE_11 landed the cyrup side and its task file already flagged this exact coupling
(`SCOPE_11.md:945-951`: *"cyrup has no `async-retention.ts` port … so there is nothing to wire this
into and it is OUT OF SCOPE here. Recorded so … whoever ports the reaper knows the coupling
exists."*). **This is that port.** The seam:

* dir: `wait_subscriptions_dir_in(&roots, cwd)` → `<run_scratch>/wait-subscriptions/<cwd_key>`
  (`artifact_roots.rs:336-341`), re-exported at `background/mod.rs:101`.
* parse: `pub fn parse_record(bytes: &[u8]) -> Option<WaitSubscriptionRecord>`
  (`wait_subscriptions/record.rs:245`, re-exported `wait_subscriptions/mod.rs:151-154`).
* record: `WaitSubscriptionRecord { version: SubscriptionVersion, token: SubscriptionToken, session_id: SessionId, target_kind: WaitTargetKind, run_id: RunId, requested_id: String, created_at: i64, expires_at: i64 }`
  (`record.rs:180-209`).

**Do NOT use `WaitSubscriptionManager::armed()`** (`manager.rs:308`): it returns only **this
session's in-memory** records (the session gate is applied at `restore`, `manager.rs:390-393`),
and the whole point of this guard is that it must see **another instance's** subscription.

**A divergence to record, not to fix silently.** `parse_record` is *stricter* than upstream's check:
it requires `version == 1`, a parseable `SubscriptionToken` and a non-empty `SessionId`
(`record.rs:180-209` and the argument at `:188-195`), where upstream requires only
`runId: string` + `expiresAt: number`. Consequence: a subscription file written by a **newer** build
makes `safe == false` and **halts the whole retention pass** — which is fail-safe but means one
foreign record can disable the reaper indefinitely. That is the same two-builds-sharing-a-directory
hazard `completion_replay/retention.rs:88-92` documents ("row 4 … an unparseable file may be a
record written by a NEWER build"). See §8 Q2.

### 4.4 `active-index` — no cyrup analogue

Upstream's `activeMarkerExists` (`:205-207`) reads `<asyncDirRoot>/<ACTIVE_RUN_INDEX_DIR>/<runId>`
(imported from `active-run-index.ts`, `:10`). **`active-run-index.ts` is unported**, stated in three
places in the tree: `tui/fleet.rs:528`, `terminal_run_index/mod.rs:71`
(*"`active-run-index.ts` is unported, so the guard does not have to be revisited when it lands"*),
and `foreground_actions/dismiss.rs:206`. `terminal_run_index::is_reserved_async_root_entry`
(`terminal_run_index/mod.rs:73-75`) already reserves the name `".active-runs"` for the day it lands.

**Consequence for this task:** the `active-index` skip reason has no input, so a run that is live in
**another process** is protected only by its `wait-reference`, by being non-terminal, or by being
younger than 30 days — **not** by an active marker. Record this as the single largest correctness
gap versus upstream, in the module doc and in §7's DoD, rather than letting a reader assume parity.
It is materially mitigated by the 30-day window and by the `non-terminal` guard, and it is the
reason §4.5 exists.

### 4.5 The one guard that closes §4.4's gap cheaply

Upstream's `runSkipReason` runs `!TERMINAL_STATES.has(status.state)` → `non-terminal` (`:295`).
Upstream's terminal set (`:28`) is `["complete","failed","stopped","rejected"]`. **cyrup's
`RunState` (`background/state.rs:70-105`) has five variants: `Queued`, `Running`, `Paused`,
`Complete`, `Failed`, `Stopped` — there is no `Rejected`**, and `is_terminal` is documented at
`:126-133` as `Complete|Failed|Stopped`, with `Paused` explicitly **not** terminal (R-SA-084).
So `Queued`, `Running` and `Paused` are all `non-terminal` and never reaped. Port
`RunState::is_terminal`; do not hand-roll a match. Record the missing `rejected` as a
`[CYRUP-DELTA]` — it is a state cyrup cannot produce, so nothing is lost.

---

## 5. What the sweep must NOT do

* **Never delete a symlinked run dir.** `symlink_metadata`, not `metadata` (§1.8 step 2).
* **Never delete anything whose age it could not determine.** `statusTimestamp` (`:174-185`) returns
  `undefined` on any stat failure and `runSkipReason` maps that to `unknown-age` (`:302`) — a KEEP.
  `completion_replay/retention.rs:175-191`'s `older_than` already states the rule for this crate:
  *"'I could not tell how old this is' must never resolve to 'delete it'"*. Same shape here.
* **Never treat a reserved async-root entry as a run.** `terminal_run_index::is_reserved_async_root_entry`
  (`:73-75`) — `.terminal-runs`, `.active-runs`; `run_status.rs:690-694` shows the established call
  shape. The `.deleting-run-*` prefix is handled separately (§1.8 step 6), and the lock/cursor/log
  now live outside the async root entirely (§1.6), so they need no exclusion.
* **Never widen `errno`'s policy.** Listing failures use `is_ignorable_listing_error` (`errno.rs:57`);
  a per-candidate failure uses `is_absent` (`:31`) and everything else is reported (§1.8 step 8).

---

## 6. Tests — named, with fail-before / pass-after semantics

Rows marked **[REPLACED]** carry the original test's *objective* against a mechanism that exists;
the original row is kept in the table so nothing is silently dropped.

### 6.1 `async_retention/sweep.rs` — `#[cfg(test)] mod tests`

| test | pins | fails before / passes after |
|---|---|---|
| `the_sweep_reaps_only_what_the_policy_selects` | integration of 13's parts | tempdir async root with one orphaned terminal run backdated past `RETENTION_MS` (`filetime` dev-dep, Cargo.toml — present for exactly this, per its comment) and one fresh run. Before: no `cleanup_async_retention` symbol → does not compile. After: `deleted_runs == 1`, fresh run's dir still exists, `skipped["recent"] == 1` |
| `the_sweep_processes_in_batches` | `ASYNC_RETENTION_BATCH_SIZE` | 250 reapable run dirs, `batch_size: 100`. `result.scanned <= 100` (upstream's own assertion, `test/unit/async-retention.test.ts:281`); a second pass reaps the next ≤ 100; a third the rest. Proves the clamp AND the cursor (R13-5): without a cursor pass 2 re-reaps nothing new and this fails |
| ~~`the_sweep_pauses_between_batches_not_between_files`~~ | ~~load-shedding via an injected clock~~ | **[REPLACED — see D1/§1.4]** there is no inter-batch pause to count |
| `a_pass_evaluates_at_most_one_batch_and_returns_without_sleeping` **[REPLACED]** | the load-shedding shape that actually exists | `#[tokio::test(start_paused = true)]` (the crate's convention — `tui/notices.rs:946`, `background/watch/sink.rs:622`, and the `tokio` `test-util` dev-dep exists for it). Assert the pass completes with `tokio::time::Instant::now()` unchanged: a sweep that sleeps inside the pass hangs the virtual clock and fails |
| `a_run_tracked_by_the_live_tracker_is_never_reaped` | SUBTASK4 — the dangerous case | a run dir backdated past the window, terminal on disk, with `JobTracker::track_restored` holding its id. Before: reaped. After: `skipped["runtime-reference"] == 1`, dir intact |
| `a_run_referenced_by_a_foreign_wait_subscription_is_never_reaped` **[NEW, §4.3]** | the fourth guard, SCOPE_11's coupling | write a `WaitSubscriptionRecord` for the run into `wait_subscriptions_dir` with a **different** `session_id`. After: `skipped["wait-reference"] == 1` |
| `an_unparseable_wait_subscription_halts_the_whole_pass` **[NEW, `:752-755`]** | fail-closed | drop `b"{"` into the subscriptions dir. After: `skipped["wait-references-unknown"] == 1`, `scanned == 0`, nothing deleted |
| `a_live_workflow_controller_protects_its_run_tree` **[NEW, §4.2]** | the worst case | id present in `workflow_controllers`. After: `runtime-reference`, dir intact |
| ~~`status_json_is_removed_last`~~ | ~~SUBTASK1's crash-safety ordering~~ | **[REPLACED — see D2/§1.5]** `remove_dir_all` cannot order its deletes |
| `an_interrupted_delete_leaves_a_prefixed_tombstone_the_next_pass_reaps` **[REPLACED]** | the same crash-safety property, against the rename | hand-create `<async_root>/.deleting-run-<uuid>` with a valid `status.json` plus a matching marker under `maintenance_root`, mtime backdated past the grace. Before: it is skipped as an unattributable run. After: `reaped_tombstones == 1` |
| `a_tombstone_whose_marker_is_missing_is_skipped_not_deleted` **[NEW, `:786-789`]** | fail-closed on the marker | same fixture, no marker. After: `skipped["run-tombstone-marker"] == 1`, dir intact |
| `a_tombstone_inside_the_grace_is_kept` **[NEW, `:790-793`]** | 24 h grace at the sweep layer | fresh mtime. After: `skipped["tombstone-grace"] == 1` |
| `a_crash_mid_sweep_leaves_a_re_evaluable_tree` | drop the sweep partway, re-run, assert convergence | drive one pass with a `batch_size: 1` fixture whose first candidate is engineered to leave a tombstone (a marker write that succeeds, a re-check that fails), then re-run to convergence. Upstream's own analogue: `test/unit/async-retention.test.ts:407-408` |
| `the_report_accounts_for_every_candidate` | scanned == kept + tombstoned + reaped | **restated per D4/§3.2 below** |
| `the_sweep_never_touches_the_results_dir` | it reaps async run trees, not payloads | seed `results_dir` with a payload, a `result-index/` tree, a `completion-replay/` record and an `output-archives/` archive. After: every one still exists, `deleted_results == 0`. Real per §2.3 — the sweep *opens* two paths under `results_dir` for its guards |
| `a_second_instance_is_refused_the_lock_and_reports_lock_busy` **[NEW, §1.7]** | the shared-root premise | pre-create `<maintenance_root>/.async-retention.lock` with a live-pid, current-host `owner.json`. After: `acquired == false`, `skipped["lock-busy"] == 1`, nothing deleted |
| `a_stale_lock_is_broken_and_the_pass_proceeds` **[NEW, `:378-395`]** | the other half of the lock policy | `owner.json` with a dead pid on this host. After: `acquired == true` |
| `a_lock_owned_by_another_host_is_never_broken` **[NEW, `:387`]** | the guard that keeps NFS-shared scopes safe | `hostname: "elsewhere"`. After: `acquired == false` |
| `a_run_whose_age_cannot_be_determined_is_kept` **[NEW, `:302`]** | §5 | remove `status.json` after the scan yields the candidate. After: a keep, never a delete |
| `a_paused_run_is_never_reaped` **[NEW, §4.5]** | R-SA-084 | `RunState::Paused`, backdated. After: `skipped["non-terminal"] == 1` |

### 6.2 `async_retention/report.rs`

| test | pins |
|---|---|
| `the_report_accounts_for_every_candidate` | the identity of §3.2, asserted on a fixture containing one reap, one tombstone reap, one skip and one induced error |
| `the_report_serialises_deterministically` | `BTreeMap` skip ordering — two structurally equal reports produce byte-identical JSON |
| `an_error_string_is_compacted_and_bounded` | `compact_error` — whitespace collapsed, ≤ 240 bytes, cut on a char boundary (feed a multi-byte error) |
| `the_maintenance_log_appends_one_line_per_pass` | `:513-534` — two passes, two lines, each a parseable object; `deleted` bounded to `ASYNC_RETENTION_BATCH_SIZE` |
| `the_maintenance_log_is_written_even_when_the_lock_is_busy` | `:691` via `:703` — `finish()` always logs |

### 6.3 `registration/doctor.rs`

| test | pins |
|---|---|
| `the_async_retention_check_reports_the_last_pass` | `Ok` with counts from a synthetic last log line |
| `the_async_retention_check_warns_on_a_pass_that_recorded_errors` | `Warn` + a remedy naming the log path |
| `a_missing_maintenance_log_is_ok_not_warn` | a fresh install is not actionable |
| `the_doctor_report_order_is_unchanged_and_the_new_check_is_appended` | `:191-195`'s fixed-order contract: existing indices 0..6 unchanged, `CHECK_ASYNC_RETENTION` at index 7 |

### 6.4 `extension/executor/notices.rs`

| test | pins |
|---|---|
| `the_three_cleanup_jobs_are_staggered` | SUBTASK3 — `#[tokio::test(start_paused = true)]`, `tokio::time::advance` past 30 s and observe stages 1-2 done and stage 3 not started; advance to 60 s and observe stage 3 done. No simultaneous start |
| `the_async_retention_pass_is_armed_by_the_watcher_install_and_cancelled_by_teardown` | extends the existing `the_retention_sweep_is_scheduled_after_the_watcher_installs` (`notices.rs:709-785`) — same `Roots::sandboxed` + `install_completion_watcher` + `stop_completion_watcher` shape, now asserting the async stage's own effect |
| `aborting_the_sweep_mid_pass_leaves_a_recoverable_tombstone` | §3.3 — `abort()` the handle between rename and delete, then run a fresh pass and assert convergence |

### 6.5 The accounting identity (D4 / §3.2) — state it correctly

The draft's `scanned == kept + tombstoned + reaped` is **false**: `:758` increments `scanned` before
the per-candidate `try`, and the `catch` at `:834-837` increments **neither** a `skipped` bucket nor
a deleted counter. The identity that holds, over the run loop only (the `lock-busy` / `cancelled` /
`wait-references-unknown` increments at `:702`, `:697`, `:753` happen **outside** the loop and must
be excluded):

```
scanned == sum(skipped_from_the_candidate_loop) + deleted_runs + reaped_tombstones + errored_candidates
```

Since `errors: Vec<String>` also collects, `errored_candidates` must be tracked as its own counter
(upstream does not, which is precisely why its own report does not balance). **Add a
`errored_candidates: usize` field** — a `[CYRUP-DELTA]` that makes the draft's stated DoD
("The report accounts for every candidate") actually checkable. With it, the test is a single
`assert_eq!`.

---

## 7. Definition of done — checkable by reading code and running named tests

- [ ] `cleanup_async_retention` reaps orphaned run trees on the 30-day policy, **one bounded batch
      per pass**, with a persisted cursor so successive passes advance
      (`the_sweep_reaps_only_what_the_policy_selects`, `the_sweep_processes_in_batches`).
      *(Restates the draft's "batched and paused between batches" against D1/§1.4's mechanism.)*
- [ ] A live-tracked run is never reaped — and neither is one held by a live workflow controller or
      referenced by a foreign wait subscription
      (`a_run_tracked_by_the_live_tracker_is_never_reaped`,
      `a_live_workflow_controller_protects_its_run_tree`,
      `a_run_referenced_by_a_foreign_wait_subscription_is_never_reaped`,
      `an_unparseable_wait_subscription_halts_the_whole_pass`).
- [ ] Deletion is **rename-then-recursive-delete**, and an interrupted delete converges on re-run
      (`an_interrupted_delete_leaves_a_prefixed_tombstone_the_next_pass_reaps`,
      `a_tombstone_whose_marker_is_missing_is_skipped_not_deleted`,
      `a_crash_mid_sweep_leaves_a_re_evaluable_tree`).
      *(Restates the draft's "`status.json` is removed last" against D2/§1.5's mechanism.)*
- [ ] The report accounts for every candidate, by the identity in §6.5
      (`the_report_accounts_for_every_candidate`).
- [ ] The three cleanup stages share `spawn_retention_sweep` and are staggered 30 s / 30 s / 60 s
      (`the_three_cleanup_jobs_are_staggered`).
      *(The helper already existed — D5/§3.1. The DoD is that the async stage joined it, not that a
      helper was created.)*
- [ ] `maintenance_root` is cwd-keyed and is **not** `dirname(async_root)` (§1.6); a test asserting
      two distinct cwds get distinct lock paths.
- [ ] The cross-instance lock is honoured, stale locks are broken, foreign-host locks are not
      (`a_second_instance_is_refused_the_lock_and_reports_lock_busy`,
      `a_stale_lock_is_broken_and_the_pass_proceeds`,
      `a_lock_owned_by_another_host_is_never_broken`).
- [ ] The doctor check exists, is appended at index 7, and never `Fail`s (§6.3). Its **unreachability
      from a user terminal today** (§2.4) is recorded in the completion notes.
- [ ] **A synthetic async root of 1,000 orphaned run directories is fully reapable** across repeated
      passes, and one pass's `scanned` stays ≤ `ASYNC_RETENTION_BATCH_SIZE`.
      *(This replaces the DoD line that cited "the 340 leaked directories measured at diagnosis" —
      §0.2: no such measurement exists in this repo. The shape of the check the line asked for —
      "verify against a synthetic root of that shape, not against the real one" — is preserved and
      strengthened to 1,000.)*
- [ ] **Benchmark.** See §8 Q1 and the completion-note requirement below.
- [ ] `docs/gap-analysis/PARITY-GAPS.md:60` updated: run half ported, result half (`:839-896` +
      `resultSkipReason` `:483-511`) and `active-run-index.ts` still open.
- [ ] Workspace `cargo nextest run --workspace --no-fail-fast` 0 failed; `cargo clippy` exit 0.

### 7.1 REQUIRED COMPLETION NOTE — the benchmark

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"**One, and it is the reason this task owns it.** A
> `criterion` benchmark over a synthetic async root of 1,000 run directories, asserting the sweep's
> *per-batch* wall time stays bounded — the property that matters is that a large root does not
> stall a live session, not raw throughput. Place it under `crates/cyrup-ext-subagents/benches/`.
> If the workspace has no `benches/` convention yet, state that in the task's completion notes
> rather than inventing one silently."*

**The workspace has no `benches/` convention.** Verified:

* `find /home/user/cyrup -path /home/user/cyrup/tmp -prune -o -name benches -type d -print` → **empty**.
* `grep -n "criterion\|\[\[bench\]\]\|harness = false" Cargo.toml crates/*/Cargo.toml` → **no hits**.
  `criterion` is not a dependency, not a dev-dependency, and not in `Cargo.lock` as a direct edge.

The draft's own escape hatch therefore applies, and this note **is** the statement it asked for. Two
further reasons a `criterion` wall-time bound is the wrong instrument here, recorded so the decision
is made once:

1. `criterion` **reports**, it does not **assert**. A "per-batch wall time stays bounded" gate needs
   a threshold, and a wall-clock threshold on shared CI is the flake shape this crate has already
   paid for once — `tokio` `test-util` is a dev-dependency specifically because *"its load-bearing
   assertion landed 15 ms inside the deadline and flaked under machine load"*
   (`Cargo.toml`, the `tokio` dev-dep comment).
2. The property the draft actually names — *"a large root does not stall a live session"* — is
   **structural, not temporal**: it is `scanned <= batch_size`, which is deterministic and already
   pinned by `the_sweep_processes_in_batches` and by the 1,000-directory DoD line above.

**Recommendation (Q1):** land the 1,000-directory bound as a deterministic nextest test asserting
the *candidate-count* bound and convergence, and add `criterion` + `benches/` only if the workspace
decides to open that convention. Do **not** invent a `benches/` layout for one benchmark.

---

## 8. Open questions the implementor MUST NOT silently decide

**Q1 — benchmark instrument.** `criterion` + a new workspace-wide `benches/` convention, or the
deterministic candidate-count bound of §7.1? Recommendation: the latter; §7.1 is the required note
either way. *(Affects `crates/cyrup-ext-subagents/Cargo.toml` and whether `benches/` is created.)*

**Q2 — `parse_record` strictness (§4.3).** Upstream's wait-reference gate needs only
`{runId, expiresAt}`; cyrup's `parse_record` demands a full valid record, so one foreign or
newer-build subscription file halts the reaper indefinitely. Port a **loose** `wait_run_ids` reader
(two fields, mirroring `:312`) beside `parse_record`, or accept the stricter fail-closed behaviour
and document it? A loose reader is upstream-faithful; the strict one is safer but can wedge.

**Q3 — `processStartIdentity` (§1.7).** Port the Linux `/proc/<pid>/stat` field-19 read (`:332-343`)
for pid-reuse detection, or ship the lock with hostname + liveness + the 24 h `LOCK_STALE_MS` floor
alone and defer it? Without it, a reused pid makes a dead owner's lock look live for up to 24 h —
a delayed sweep, never a wrong delete.

**Q4 — the `running`-run repair hop (§1.8 step 4).** Port `reconcileAsyncRun` (`:772-778`) via
`reconcile::reconcile_now`, or skip a `running` run outright as `non-terminal`? **Skipping is safe
but defeats a large part of the objective**: a run whose runner was SIGKILLed stays `running` on
disk forever and is *never* reapable, which is precisely the orphan class this task exists for.
Recommendation: port it. Note that `reconcile_now` does real I/O and may **write** `status.json`,
so it must run before the policy call and its `repaired` outcome feeds `repaired_runs`.

**Q5 — `cancelled` / `AbortSignal` (§2.1).** Upstream threads an `AbortSignal` checked at `:694-699`,
`:742`, `:757`, `:840`. cyrup's stage is `abort()`ed as a whole (`notices.rs:609`), which drops the
future rather than setting a flag. Keep `cancelled` as an always-`false` field for wire
compatibility, thread a real `CancelToken` (the crate has one — `notices.rs`'s foreground-control
tests use `CancelToken::new()`), or omit the field?

**Q6 — results-half deferral (§2.3).** Confirm SCOPE_14 is run-half-only and the results half gets
its own ledger entry, rather than being quietly folded in. The draft's
`the_sweep_never_touches_the_results_dir` only makes sense under this reading.

**Q7 — `async_retention_root_in` placement (§1.6).** Add it to `background/artifact_roots.rs` beside
`wait_subscriptions_dir_in` and re-export from `background/mod.rs:98-102` (recommended, since
`SUBSCRIPTIONS_SUBDIR` and `cwd_key` already live there), or keep it inside `async_retention/mod.rs`?
The former touches a file other tasks may also touch; the latter duplicates the `cwd_key` arithmetic
the C7 note at `artifact_roots.rs:372-383` warns about duplicating.

**Q8 — SCOPE_13 cursor (R13-5).** If part A's `scan.rs` has no persisted cursor, does SCOPE_14 add
it, or does SCOPE_13 get amended? This is the single largest scope risk in the task.

---

## 9. Research notes (verified)

* **Upstream, tag `v0.67.0`** (== HEAD for this file): `src/runs/background/async-retention.ts`,
  912 lines. `AsyncRetentionResult` at **`:103-119`**; `AsyncRetentionOptions` at `:81-101`;
  constants `:14-29`; `cleanupAsyncRetention` at `:650-912`; the run-candidate loop at `:756-838`;
  the result-candidate loop at `:839-896`.
* **Upstream scheduler:** `src/extension/index.ts:597-614` (`v0.67.0`) — the one-shot
  `ASYNC_RETENTION_DELAY_MS` `setTimeout`, `unref`'d, with the three-source `protectedRunIds` set.
* **Upstream tests** worth mining for fixtures: `test/unit/async-retention.test.ts` at `v0.67.0`
  (`:281`, `:346`, `:378`, `:389-393`, `:407-408`, `:421`, `:480-484`, `:506-509` all assert
  `result.scanned <= ASYNC_RETENTION_BATCH_SIZE` or drive repeated passes to convergence).
* SCOPE_13 builds `policy.rs`, `scan.rs`, `tombstone.rs`; this task adds `sweep.rs`, `report.rs`.
  The contract SCOPE_14 needs from it is §0.4 (R13-1 … R13-7).
* **Scheduling seam:** `extension/executor/notices.rs:648-682` (`spawn_retention_sweep`), armed at
  `:507-513`, cancelled at `:603-611`, delay const at `:615`, existing test at `:709-785`. The
  handle field is `extension/executor/mod.rs:70`.
* **Live-run source:** `background/tracker.rs` — `JobTracker::snapshot` (`:224-232`) returning
  `Vec<TrackedJob>` (`:99-121`). `jobs` is **private**; there is no `jobs.keys()` accessor.
* **Workflow-controller source:** `extension/executor/mod.rs:171-172`.
* **Wait-reference source:** `background/wait_subscriptions/record.rs:180-209` + `parse_record`
  (`:245`); dir from `artifact_roots.rs:336-341`; the coupling was pre-recorded at
  `SCOPE_11.md:945-951`.
* **Shared-root fact:** `background/artifact_roots.rs:271-292`, sentence at **`:278-281`**
  (SCOPE_13's `:281-284` is off by three).
* **Error policy to reuse:** `background/result_index/errno.rs:22/31/47/57`, all `pub(crate)`.
* **Terminal-state source:** `background/state.rs:70-133` (`RunState`, `is_terminal`); no `Rejected`.
* **Atomic write:** `background/atomic.rs:75` `write_atomic_json<T: Serialize + Sync>(&Path, &T) -> io::Result<()>`;
  private-mode blocking variant at `:209`.
* **Doctor:** `registration/doctor.rs` — `CheckStatus` `:101`, `DoctorCheck` `:132`, `DoctorReport`
  `:197`, name consts `:231-245`, `DoctorRunner` `:257`, `run` `:284-305`. **No production caller**
  (`registration/slash_commands.rs:54-55`).
* **`CLAUDE.md` does not exist in this repo**, and no "340 leaked directories" measurement exists
  anywhere in `.flux/` or `docs/gap-analysis/`. Neither may be cited (§0.1 D5, §0.2).
* **No `benches/` and no `criterion`** anywhere in the workspace (§7.1).
* `filetime` (dev-dep) is present precisely for mtime backdating in sweep tests — see its comment in
  `crates/cyrup-ext-subagents/Cargo.toml`. `tokio` `test-util` is present for
  `#[tokio::test(start_paused = true)]`.
