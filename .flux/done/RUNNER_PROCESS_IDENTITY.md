---
stage: done
status: completed
updated: 2026-09-20
---

# Runner process identity — the session lease (VL-S3) and the process-terminal proof (VL-S4)

OBJECTIVE: close the two open ledger rows that are really one feature — **who owns this run's
session file, and how did its process actually end**. Today cyrup can answer neither, and a second
subsystem already runs on a substitute for the second answer.

Upstream: `src/runs/shared/session-lease.ts` (293 lines) + `src/runs/background/process-terminal.ts`
(310 lines) @v0.68.0. They are COUPLED: `process-terminal.ts:12` imports `canonicalSessionId` and
`inspectSessionLease`, and `sessionProjection` (`:150`) folds the lease state into the proof. Port
one without the other and you port half a mechanism.

Ledger rows: `docs/gap-analysis/PARITY-GAPS.md` VL-S3 (`:1445`) and VL-S4 (`:1449`).

## Why this, and why now

1. **`status` cannot tell a crash from a slow start.** When a runner dies without writing a result,
   cyrup reports the reconciler's "stale" guess. An agent that delegated the work gets no answer.
2. **It is already load-bearing.** `background/active_async_capacity/mod.rs` §D3 records that
   upstream releases a capacity slot on ONE positive proof — a `processTerminal` whose
   `state === "observed"` matches the owner's `runnerProcessInstanceId` — and that cyrup has
   neither input, so the release rung was substituted with runner-pid liveness
   (`active_async_capacity/inspect.rs:258-275`). **That substitute has a hole**:
   `reconcile::check_pid_liveness` (`background/reconcile.rs:49-58`) is `kill(pid, 0)` and nothing
   else, so a RECYCLED pid reads `Alive` and the slot is retained forever. Upstream closes exactly
   that with `getProcessStartIdentity` (`session-lease.ts:68-83`), which reads
   `/proc/<pid>/stat` field 20 (`starttime`, ticks) and returns `linux:<ticks>`.
3. **Nothing stops two runners writing one session file.**
   `grep -rn 'session_lease\|SessionLease' crates/cyrup-ext-subagents/src` → **0**. No lease, no
   dead-owner reclaim on revival. Do NOT mistake `background/async_retention/lock.rs` for it — that
   guards the retention sweep and is a different lock (VL-S3 says so itself).
4. **It finishes `debug.run`.** The verb we shipped in #146 prints
   `Process terminal: not recorded — …` because there is nothing to read
   (`background/run_lifecycle_debug.rs`). Closing VL-S4 turns that line into upstream's real
   `formatProcessTerminal` pair (`run-status.ts:47-58`: a `sidecar`/`overlay` split).

## Upstream, pinned at v0.68.0 — the anchors the augment must re-verify and expand

### `src/runs/shared/session-lease.ts` (293)

- `:8` `SESSION_LEASES_DIR = path.join(TEMP_ROOT_DIR, "session-leases")`.
- `:10-15` `SessionLeaseRequest` · `:17-33` `SessionLeaseOwner` (version, token,
  canonicalSessionFile, runId, sourceRunId, parentSessionId?, pid, hostname,
  processStartIdentity?, writerState `none|spawning|running`, writerPid?,
  writerProcessStartIdentity?, acquiredAt, acquiredAtMs, updatedAtMs) · `:35-40` `SessionLeaseHandle`
  (`updateWriter`, `release`) · `:42-45` `SessionLeaseState` = `free | owned | unreadable` ·
  `:58-66` `SessionLeaseConflictError`.
- `:68-83` `getProcessStartIdentity` — linux only, `/proc/<pid>/stat`, field 20 after the last
  `)`, `linux:<startTicks>`; anything else `undefined`.
- `:84-95` `processIsAlive` — `kill(pid,0)`; `ESRCH` ⇒ false; **`EPERM` ⇒ TRUE**; else `undefined`.
- `:96-108` `canonicalSessionFilePath` (realpath of resolve) · `canonicalSessionId`
  (sha256 hex of the canonical path, lowercased on win32) · `sessionLeaseDir`.
- `:110-119` `inspectSessionLease`.
- `:121-152` `parseOwner` — a STRICT validator; note the two cross-field rules at `:145-146`
  (`running` requires `writerPid`; non-`running` forbids `writerPid`/`writerProcessStartIdentity`).
- `:154-160` `conflictMessage` — two sentences, both verbatim deliverables.
- `:162-181` `processDemonstrablyGone` (dead, OR alive-but-start-identity-CHANGED) and
  `demonstrablyStale` — four rungs: hostname must match; owner pid demonstrably gone;
  **`writerState === "spawning"` is NEVER stale** (the unobservable window); `none` ⇒ stale;
  `running` ⇒ the writer pid must ALSO be demonstrably gone.
- `:183-201` `createLeaseDirectory` — build `<leaseDir>.candidate-<token>` at 0700, `owner.json`
  at 0600, then `rename` onto `leaseDir`. **The rename is the atomic claim**: POSIX refuses to
  rename onto a non-empty directory, so the loser sees `leaseDir` exist and returns false.
- `:202-293` `acquireSessionLease` — 4 attempts; on a losing claim, re-read the owner and throw
  `SessionLeaseConflictError` unless `demonstrablyStale`; then rename the stale dir aside to a
  PER-OWNER tombstone `<leaseDir>.stale-<sanitized token>` (`[^A-Za-z0-9._-]` → `-`) so only the
  first contender can reclaim and a later one cannot move a SUCCESSOR lease by mistake.

### `src/runs/background/process-terminal.ts` (310)

- `:15-24` `ProcessTerminalCandidate` (writers keyed by step index, `expectedWriters?`,
  `sessionFile?`, `revivalLeaseToken?`, `revivalLeaseReleaseAcknowledged?`) · `:26-31`
  `RunnerCloseObservation`.
- `:63-69` the two paths (candidate + proof) under the async dir.
- `:75-113` `readProcessTerminalCandidate` · `:114-117` `writeProcessTerminalCandidate` (PRIVATE
  atomic — it can carry a session path) · `:119-133` `initializeProcessTerminal` (candidate +
  a `state: "pending"` proof, written at launch, BEFORE any child session is authorized) ·
  `:134-139` `markProcessTerminalCandidateLeaseRelease`.
- `:140-143` `unknownProof` · `:144-149` `resumeDisposition` · `:150-160` `sessionProjection`
  (requires the lease to be `free` AND, when a revival token was held, an ACKNOWLEDGED release).
- `:161-179` `validateProof` — every throw sentence is a deliverable; note `observed` additionally
  requires `observedAt`, `instances`, and a matching `runner` instance.
- `:180-200` `sanitizeProcessTerminal` (overlay reader; a bad value becomes an `unknown` proof with
  reason `proof-write-failed`, never a throw) · `readProcessTerminal` (ENOENT ⇒ `undefined`).
- `:201-242` `stepProcessTerminalProof` + `overlayStatus` — writes `status.processTerminal` AND a
  per-step `step.processTerminal`, with the per-step state derived from
  `expectedWriters[index]` vs the recorded writers.
- `:243-310` `finalizeProcessTerminal` — the whole decision ladder, in order: existing observed or
  unknown proof short-circuits; candidate missing ⇒ `runner-candidate-missing`; run/instance
  mismatch ⇒ `runner-instance-mismatch`; **lease not free ⇒ `canonical-session-lease-active` |
  `canonical-session-unavailable`**; unacknowledged revival release ⇒
  `canonical-session-release-unverified`; inconsistent or empty writers ⇒
  `writer-close-unverified`; an unobserved pi-writer process tree ⇒ `process-tree-unverified`;
  otherwise `observed` with `instances: [runner, ...allWriters]`. Then: write the sidecar; ONLY on
  `observed` call `releaseActiveRunIndex`; overlay the status; append a
  `subagent.run.process_terminal` event. **A non-durable write emits no event and returns an
  `unknown` proof** — the augment must keep that ordering.
- Types: `shared/types.ts:633-643` `ProcessTerminalReason` (10 variants), `:645-651`
  `RunnerProcessInstanceExit`, `:653+` `ProcessTreeTerminal`, `:684-689`
  `CanonicalSessionTerminal`, `:691-713` `ProcessTerminal` (a 3-arm discriminated union).

### The consumers (the augment must map every one onto a cyrup seam)

| upstream call site | what it does |
|---|---|
| `async-execution.ts:851` | `initializeProcessTerminal(launchAsyncDir, launchRunId, runnerProcessInstanceId)` at launch |
| `async-execution.ts:784` + `:790` | `finalizeProcessTerminal` on runner close, then read it back |
| `subagent-runner.ts:5186` | `writeProcessTerminalCandidate` — the writers ledger |
| `subagent-runner.ts:5219`, `:5242` | `acquireSessionLease(config.revivalLease)` |
| `subagent-runner.ts:5288` | `markProcessTerminalCandidateLeaseRelease` |
| **`active-async-capacity.ts:227`, `:287`** | **`readProcessTerminal` — THE capacity release rung cyrup substituted** |
| `async-job-tracker.ts:520` | a published-vs-closed check |
| `async-status.ts:185`, `:303` | the status projection |

## cyrup seams — and the things the augment MUST settle

- **Where the runner process instance id comes from.** cyrup has no `runnerProcessInstanceId`;
  `active_async_capacity/key.rs:148-155` carries `runner_pid` + `runner_started_at` instead, bound
  by `mark_started(pid)` at `extension/executor/background.rs`. §D3 says that pair IS cyrup's
  process identity. **Decide and record**: mint a real instance id (pid + start-identity, or a
  uuid written at launch), or key the proof on the existing pair. Whichever — the proof's
  `runnerProcessInstanceId` must be something a reader can MATCH, which is the whole point of
  `validateProof`'s fallback check.
- **`check_pid_liveness` must gain the start-identity rung** (`background/reconcile.rs:49-58`), or
  the lease's `processDemonstrablyGone` cannot be ported honestly. Note every existing caller of
  `check_pid_liveness` and say whether each wants the new rung (the capacity verdict does).
- **`RunStatus`/`StepStatus` need the overlay fields** (`background/records.rs`) —
  `process_terminal` on both, as `overlayStatus` writes them. Today serde would drop both keys.
- **`RunDir`/`RunPaths` need two new path accessors** (`background/run_paths.rs:74-111` is the
  pattern; `recovery_descriptor()` at `:109` is the closest precedent).
- **`write_private_atomic_json`** already exists (`background/atomic.rs:185`) and is the right
  writer for the candidate; `write_atomic_json` (`:75`) for the proof.
- **The capacity release rung** (`active_async_capacity/inspect.rs:258-275`) is where the
  substitute lives. When the real proof lands, revisit it — VL-S4's own row says closing it
  "should revisit that substitution". Do NOT simply delete the pid ladder: it is the fallback when
  no proof exists. Add the proof as the FIRST rung, keep the pid ladder beneath it, and rewrite
  §D3's note to say what is now true.
- **`debug.run`'s dump** (`background/run_lifecycle_debug.rs` + `extension/executor/status.rs`)
  prints `Process terminal: not recorded — …` today. With a reader it becomes upstream's
  `debugProcessTerminal` pair (`run-status.ts:52-58`): a `sidecar` line and an `overlay` line.
  **Delete the "not recorded" delta rather than rewording it**, and the `children.list` /
  `run_lifecycle_debug` notes that cite VL-S4 as open must be re-read.
- **The runner's own close observation.** cyrup's runner is `background/runner_main/` (11 modules);
  `finish.rs` is the terminal sweep and `settle.rs` the settle. The augment must name the exact
  site that observes the runner's exit code/signal, and whether `TerminationOutcome`
  (`spawn/signal.rs:90-106`) needs the `ExitStatus::signal()` name mapping area 09 `SUBA-023`
  says is missing — that is the OTHER half of VL-S4 and it is in scope.
- **Where the lease is acquired.** Upstream acquires it for REVIVAL
  (`subagent-runner.ts:5242`, `config.revivalLease`). cyrup's revival is
  `extension/executor/control.rs` → `background/control::resume` → `revive_from_transcript`, which
  #144 taught to read the recovery descriptor. That is the seam. Say exactly which process holds
  the lease and for how long.
- **A temp root.** `SESSION_LEASES_DIR` is under upstream's `TEMP_ROOT_DIR`. Find cyrup's
  equivalent (`paths.rs`) or say there is none and choose one; tests must confine it (the
  `confined_base()` tempdir pattern — a leaked lease dir under a shared root poisons reruns, which
  bit this project before).

## Definition of done

1. `session_lease` and `process_terminal` modules under `background/`, both REACHABLE from a
   production caller, both with typed errors and newtypes (no stringly-typed states).
2. The lease's four staleness rungs each pinned by a test, including the two that are easy to get
   wrong: `spawning` is never stale, and alive-but-different-start-identity IS stale.
3. `createLeaseDirectory`'s rename-claim proven by a test in which a second acquirer loses.
4. The conflict sentences and every `validateProof` throw sentence byte-identical to upstream.
5. `finalizeProcessTerminal`'s ladder: one test per reason arm, the `observed` arm included.
6. The capacity release rung reads the real proof first and falls back to the pid ladder, with
   §D3's note rewritten to what is true.
7. `debug.run` prints the real sidecar/overlay pair; the "not recorded" delta is DELETED.
8. `check_pid_liveness` grows the start-identity rung, with pid-reuse pinned by a test.
9. Ledger: VL-S3 and VL-S4 closed with evidence; every in-tree note citing them as open re-read
   and corrected or deleted (grep `VL-S4` and `process-terminal` crate-wide — #146 left several).
10. Gates: fmt; clippy `--workspace --all-targets --features test-fixtures -- -D warnings`;
    `nextest run --workspace --features test-fixtures` (baseline **10683** / 9 skipped);
    `nextest run -p cyrup-it --features it` (baseline **567**).

---

## [AUG — runner-identity]

Read IN FULL at the pin (`git -C tmp/pi-subagents show v0.68.0:<path>`): `src/runs/shared/session-lease.ts`
(293), `src/runs/background/process-terminal.ts` (310), `src/shared/types.ts:620-730`,
`src/runs/background/async-execution.ts:555-880`, `src/runs/background/subagent-runner.ts:5150-5300`,
`src/runs/background/active-async-capacity.ts:205-300`, `src/runs/background/async-job-tracker.ts:505-535`,
`src/runs/background/async-status.ts:175-200,:295-315`, `src/runs/background/run-status.ts:40-110`,
`src/extension/rpc.ts:440-470`.

### 0. Citations in the seed that are WRONG — correct these before using them

| seed claim | truth at the pin / at cyrup HEAD |
|---|---|
| `session-lease.ts:121-152 parseOwner`, cross-field rules `:145-146` | `parseOwner` is `:121-144`; `readLeaseOwner` is `:146-152`. The two cross-field rules are **`:141-142`**. |
| `:68-83 getProcessStartIdentity` | `:68-82`. |
| `:84-95 processIsAlive` | `:84-94`. |
| `:183-201 createLeaseDirectory` | `:183-200`. |
| `:180-200 sanitizeProcessTerminal … readProcessTerminal` | `sanitizeProcessTerminal` `:180-188`; `readProcessTerminal` `:190-199`. |
| `types.ts:645-651 RunnerProcessInstanceExit` (implied to be the only exit type) | correct, but the union member the candidate's `writers` actually holds is **`PiWriterProcessInstanceExit` (`:672-680`)**, and the union is `ProcessInstanceExit` (`:682`). `process-terminal.ts` imports the UNION name, not `RunnerProcessInstanceExit`. Also unmentioned: `ProcessTerminalState` (`:632`, 4 variants) and `SUBAGENT_LIFECYCLE_ARTIFACT_VERSION = 3` (`:629`), which the event line stamps. |
| `run-status.ts:47-58` / `:52-58` | the file is `src/runs/background/run-status.ts`, NOT `src/runs/run-status.ts`. `formatProcessTerminal` `:47-50`; `debugProcessTerminal` `:52-58`; the two lines it feeds are `:102-103`; the file-path line is `:95`; the two call sites are `:487` and `:514`. |
| `async-execution.ts:784 + :790` | correct, but the mint site the seed leaves unnamed is **`:707` `const runnerProcessInstanceId = randomUUID();`** — before the spawn, in the PARENT, carried to the child in `launchConfig` (`:710`). This is the single most load-bearing upstream line in the batch. |
| "`TerminationOutcome` (`spawn/signal.rs:90-106`) … the `ExitStatus::signal()` name mapping area 09 `SUBA-023` says is missing. That mapping is IN SCOPE." | **FALSE, twice over, and it is in scope for DELETION not for implementation.** `TerminationOutcome` is at `spawn/signal.rs:112-127` and already carries `pub signal_name: Option<&'static str>` (`:127`); `signal_name(i32)` is `:136-158`; `signal_name_of(&ExitStatus)` is `:162-170` and does exactly `status.signal().and_then(signal_name)`. `09-cyrup-ext-subagents.md:617` (SUBA-023) states in its own body that this observation "is REFUTED" and that the signal half closed in sweep 1. The seed copied the STALE sentence from `PARITY-GAPS.md:1453`, which still says "`TerminationOutcome` (`spawn/signal.rs:90-106`) carries only `status` + `stage`, with no `ExitStatus::signal()` name mapping". See §6. |
| `active_async_capacity/inspect.rs:258-275` | the proof rung's comment is at `:257`, the rung body `:257-274`, and the whole `runner_release_verdict` is `:212-275` (doc `:179-211`). |
| `active_async_capacity/key.rs:148-155` | correct (`runner_pid` `:152`, `runner_started_at` `:155`); the doc block that names the substitution is `:101-111` and `is_started` is `:159-166`. |
| `background/reconcile.rs:49-58` for `check_pid_liveness` | `:49-58` is module doc. `Liveness` is `:68-77`, `is_possibly_alive` `:83-91`, and **`check_pid_liveness` is `:94-122` (`pub fn` at `:107`)**. |
| `spawn/signal.rs:90-106` for `TerminationOutcome` | `:112-127`. `EscalationStage` is `:100-107`. |
| VL-S3 row's own anchors (`session-lease.ts:9,:59,:208`, "299 lines", `subagent-runner.ts:4618/:4648`) | v0.47.1-era. At v0.68.0 the file is **293** lines and the acquire/release pair is **`:5242` / `:5283`**. |
| VL-S4 row's own anchors (`process-terminal.ts:52,:163,:216`, "280 lines", `inspect.rs:152`) | v0.47.1-era. At v0.68.0 the file is **310** lines; `inspect.rs`'s §D3 doc is now `:179-211`. |

### 1. What cyrup ALREADY has — do not rebuild any of it

- **A zero-hit confirmation.** `grep -rn 'session_lease\|SessionLease' --include=*.rs crates/` → **0**, re-run this pass. `process_terminal`/`ProcessTerminal` in `cyrup-ext-subagents` is **comments and string literals only** (full list in §5). The `cyrup-tui` hits are a different, unrelated `ProcessTerminal` (pi's TUI terminal object) — never touch them.
- **`machine_hostname()` — `background/async_retention/lock.rs:115`.** Reads `/proc/sys/kernel/hostname` then `/etc/hostname`, falls back to `"unknown-host"`; no `unsafe`, no new dep. This IS the lease's `os.hostname()`. **Reuse it; do not write a second one.** Its own doc (`:109-114`) already explains why the fallback is the safe direction, which is the same argument `demonstrablyStale`'s rung 1 needs.
- **The rename-claim + hostname/pid staleness ladder, as a working precedent** — `background/async_retention/lock.rs` (721 LOC, `:259` hostname rung, `:264-268` the `Dead`-only rung). It is a DIFFERENT lock (VL-S3 says so, and so does its own doc `:30-31`) — but its shape, its injectable `liveness: fn(u32) -> Liveness` seam (`:82-84`, defaulted to `check_pid_liveness` at `:96`) and its test vocabulary (`identity(hostname, liveness)` at `:425`) are the template. Copy the SHAPE, not the file.
- **`check_pid_liveness` + `Liveness{Alive,Dead,Unknown}`** — `background/reconcile.rs:94-122`. This is `processIsAlive` already ported, and **it already gets `EPERM ⇒ TRUE` right** (`Err(_) => Liveness::Unknown`, plus `is_possibly_alive()` at `:83-91` folding `Unknown` in with `Alive`) — upstream `session-lease.ts:91` returns `true` for `EPERM`, `undefined` for anything else, and `processDemonstrablyGone` (`:168`) treats both identically (`alive === false` is the only early `true`). cyrup's three-way enum is a STRICTLY better expression of the same predicate. Do not add a fourth state.
- **`release_active_run_index(async_dir)`** — `background/active_run_index.rs:198`. This is upstream's `releaseActiveRunIndex` (`process-terminal.ts:303`), already ported, already `async`, already reachable. The observed arm calls it.
- **Both atomic writers** — `background/atomic.rs:75` `write_atomic_json` (the proof), `:185` `write_private_atomic_json` (the candidate, 0600), `:246` `write_private_atomic_json_blocking` for a sync caller. `writePrivateAtomicJson`/`writeAtomicJson` need no port.
- **`RunDir` accessors** — `background/run_paths.rs`: `status()` `:73-76`, `events()` `:82-85`, `handoff()` `:92-97`, `recovery_descriptor()` `:102-111`. The pattern is four deep; the two new ones are the fifth and sixth.
- **The run-scratch root and its sibling-leaf convention** — `background/artifact_roots.rs`: `temp_root_dir` `:243`, `temp_root_dir_from` `:260`, and five leaves already hanging off it (`ASYNC_SUBDIR` `:26`, `RESULTS_SUBDIR` `:34`, `SCRATCH_SUBDIR` `:44`, `SUBSCRIPTIONS_SUBDIR` `:64`, `CAPACITY_SUBDIR` `:87`). `active_async_capacity_root_in(roots)` (`:384-386`) is `roots.run_scratch().join(CAPACITY_SUBDIR)` — the exact one-liner §4 copies.
- **`Roots`** — `paths.rs:178-196`, `from_env` `:214`, `from_lookup` `:227-243`, `sandboxed` `:268-280`, `run_scratch()` `:305`. `Roots::sandboxed(tmp.path())` is the tempdir confinement §4 mandates.
- **`uuid` (v4), `sha2` 0.11, `nix`, `libc`, `tempfile`** are all already direct dependencies (`crates/cyrup-ext-subagents/Cargo.toml`). `RunId::new()` (`background/run_id.rs`) is `uuid::Uuid::new_v4().as_simple().to_string()` — the exact mint idiom §2 reuses. `sha2::{Digest as _, Sha256}` is already used at `exec/mutation_evidence/repo.rs:19,:139`. **NO new dependency is needed for any part of this batch.**
- **`append_event`** — `background/runner_main/events.rs:75-105`, `pub(super)`, writes `{"type": "subagent.run.*", "ts": …, …detail}` through the capped `BoundedJsonlWriter`. Upstream's `fs.appendFileSync(events.jsonl, {type:"subagent.run.process_terminal", …})` (`process-terminal.ts:305`) is this function with one new type string.
- **The typed-error precedent** — `background/recovery_descriptor.rs:93-130` (`RecoveryDescriptorError`, `thiserror`, one variant per upstream throw) + `error.rs:270-275` (the `#[from]` bridge into `SubagentError`). Copy this shape exactly; #144 established it for the same kind of on-disk contract.
- **`spawn/signal.rs`'s signal-name mapping** — `:127`, `:136-158`, `:162-170`. Already done. See §6.
- **The §D3 substitute itself** — `active_async_capacity/inspect.rs:212-275`, `mod.rs:59-72`, `key.rs:101-166`. This is the thing being *extended*, not replaced.

### 2. THE FOUR THINGS, SETTLED

#### (1) The runner process instance id — MINT A REAL ONE. Do not key on `runner_pid`/`runner_started_at`.

**Upstream, verbatim:** `async-execution.ts:707` `const runnerProcessInstanceId = randomUUID();` — minted in the PARENT before the spawn, carried into the child at `:710` (`launchConfig = { ...cfg, runnerProcessInstanceId, … }`), doubling as the `launchBarrierToken` when no revival lease is held (`:709`), stamped into the initial status's pending proof at `:839`, into the candidate + pending sidecar at `:851`, and matched back on close at `:785` / `:790`.

**Why not the pid pair.** The whole point of VL-S4 is the pid-reuse hole `PARITY-GAPS.md:1451` records: `check_pid_liveness` is `kill(pid,0)` and a recycled pid reads `Alive`. Keying the PROOF's identity on that same reusable pid re-opens the hole one layer up — a recycled pid plus a re-used `runner_started_at` millisecond is a matchable collision, and `validateProof`'s whole job (`process-terminal.ts:166`) is to refuse a proof that belongs to a *different* runner. A v4 uuid cannot collide. It also costs nothing: `uuid` is already a dependency and `RunId::new()` already mints exactly this shape.

**The decision, and what writes it:**

- New newtype `RunnerProcessInstanceId(Arc<str>)` in `background/process_terminal/id.rs`, modelled on `RunId` (`background/run_id.rs`): `new()` = `uuid::Uuid::new_v4().as_simple().to_string()`, `from_token`, `as_str`, `Display`, `Serialize`/`Deserialize` as a plain string (on-disk format unchanged — upstream's field is `runnerProcessInstanceId: string`).
- **Minted at `extension/executor/background.rs`**, in `spawn_background_steps`, immediately before `write_atomic_json(&cfg_path, &runner_config)` (`:801`). One value, written to FOUR places in the launch, all of which the orchestrator already writes:
  1. `RunnerConfig::runner_process_instance_id` (`background/runner_main/config.rs`, new field beside `run_id` at `+6`) — this is upstream's `launchConfig` at `:710`.
  2. `<run_dir>/process-terminal-candidate.json` + `<run_dir>/process-terminal.json` (`state:"pending"`), via `initialize_process_terminal`, called **BEFORE** the spawn — see the ordering note below.
  3. `ActiveAsyncCapacityOwner::runner_process_instance_id: Option<RunnerProcessInstanceId>` — a NEW field **beside** `runner_pid`/`runner_started_at` (`key.rs:152,:155`), never replacing them. Bound by `ActiveAsyncCapacityHandle::mark_started` (`claim.rs:312`, `mark_started_locked` `:340-346`), whose signature grows to `mark_started(&mut self, pid: u32, instance: RunnerProcessInstanceId)`. `is_started()` (`key.rs:159-166`) stays as-is — the pid/started-at disjunct is still upstream's `:180` test and must not narrow.
  4. `RunStatus::process_terminal` — written by the RUNNER at `publish_initial_status` (`runner_main/entry.rs:266+`) from `config.runner_process_instance_id`, mirroring `subagent-runner.ts:2102`; and per-step at the step-status seed, mirroring `:2057`.

**Ordering — the one place cyrup's line position must differ from upstream's, and why.** Upstream calls `initializeProcessTerminal` AFTER the spawn (`:851`) because it is inside a barrier: the runner blocks on `waitForStartupControl(startupProceedPath, launchBarrierToken, "proceed")` (`subagent-runner.ts:5233-5240`) and cannot touch a child session until `:868` writes the proceed token. **cyrup has no startup barrier** (`grep -rn 'launch_barrier\|startup-proceed\|runner-startup' crates/cyrup-ext-subagents/src` → 0; `spawn_detached_runner_with_command` returns and the runner is already free). So in cyrup the SPAWN *is* the authorization, and the doc contract on `initializeProcessTerminal` — *"Establish ownership before authorizing a runner to start any child session"* (`process-terminal.ts:118`) — is satisfied only by writing it **before** `spawn_detached_runner_with_command` (`background.rs:839`). Writing after would race the runner's own candidate write. Nothing in `initializeProcessTerminal` needs the pid, so there is no obstacle. **This ordering is the [CYRUP-DELTA] to write**, with `process-terminal.ts:118` and the absent-barrier grep as its evidence.

#### (2) The lease: held by the RUNNER process, for the whole of its run.

**Upstream, verbatim** — `subagent-runner.ts:5217-5294`, `runConfiguredSubagent`:
- `:5219` `let lease: ReturnType<typeof acquireSessionLease> | undefined;`
- `:5224-5231` a `process.once("exit", releaseOnExit)` safety net whose comment is the contract: *"Exit cleanup is best effort; a dead-owner lease is reclaimed on the next revival."*
- `:5241-5242` `} else if (config.revivalLease) { lease = acquireSessionLease(config.revivalLease);` — the acquire, in the RUNNER, before any child session, on the revival path ONLY.
- `:5243` `config.revivalLeaseToken = lease.owner.token;` — the token that later reaches the candidate at `:5183`.
- `:5278-5293` `finally { process.off(...); if (lease) { acknowledged = lease.release(); markProcessTerminalCandidateLeaseRelease(config.asyncDir, lease.owner.token, acknowledged); } }`

So: **one process (the runner), from just after config load to just after the step loop settles, on the revival path only.**

**cyrup's seam, exactly:**
- The orchestrator builds the REQUEST. `extension/executor/control.rs::revive_from_transcript` (`:283`) already holds all four fields: `session_file` (param, and set on `BackgroundStepsSpec.session_file` at `:472`), the new `run_id` (`RunId::new()`), `source_run_id` (`source_id`, already built at `:305` and already passed as `transfer_from: Some(source_id)` at `:468`), and the parent session id via `self.current_session_id()`. Add `BackgroundStepsSpec::revival_lease: Option<SessionLeaseRequest>` (`extension/executor/requests.rs:358`, beside `transfer_from` at `+98`) — **explicit, not derived**: `session_file.is_some()` is true on non-revival launches too, so deriving it would put a lease on every seeded launch, which upstream does not do.
- `background.rs::spawn_background_steps` copies it onto `RunnerConfig::revival_lease: Option<SessionLeaseRequest>` before `:801`. Nothing else in the orchestrator touches it.
- **The runner acquires it** in `background/runner_main/entry.rs::run_with`, immediately after `load_runner_config` (`:103`) and **before** `publish_initial_status` (`:110`) — upstream's position relative to `runSubagent`. On `Err(SessionLeaseConflictError)` the runner does **not** proceed: it funnels straight to `finish_run` with `RunState::Failed` and the conflict sentence as `error`, so R-SA-077's "status.json then ResultFile on every exit path" invariant (`runner_main/mod.rs:33-45`) is preserved. There is no `?` that bypasses `finish_run`, and there must not be.
- The acquired token is stamped into the candidate at the same site the candidate is written (§3).
- **The runner releases it** in `run_with`'s tail, **after** `finish_run(...)` (`entry.rs:202-216`) and before `Ok(())`, then calls `mark_process_terminal_candidate_lease_release(async_dir, token, acknowledged)`. Then `finalize_process_terminal` (§3) runs LAST, so `sessionProjection`'s "lease is free AND the release was acknowledged" (`process-terminal.ts:150-159`) can actually be true. **This ordering — release, mark, finalize — is the whole reason `canonical-session-release-unverified` (`:275-276`) is a distinguishable arm; get it wrong and every revived run's proof is `unknown`.**
- Rust's `Drop` cannot stand in for `process.once("exit")` (the release is a filesystem op on an `async` runtime, and `claim.rs:29` already records this crate's rule: *"a `Drop` guard cannot do this: `rollback` is `async`"*). The equivalent is the explicit tail plus the fact that a runner killed mid-run leaves a lease that the NEXT revival reclaims via `demonstrablyStale` — which is exactly upstream's own stated fallback at `:5228`.

#### (3) The runner's close observation — `background/runner_main/finish.rs`, and the signal half is a DELETION.

**The architectural fact that settles this.** Upstream observes the runner's exit from the PARENT: `async-execution.ts:778` `proc.once("close", (exitCode, signal) => …)` → `:784-789` `finalizeProcessTerminal(asyncDir, runId, { processInstanceId, closeObservedAt: Date.now(), exitCode, signal })`. **cyrup's orchestrator cannot do this**: `spawn_detached_runner_with_command` spawns a genuinely detached process in its own process group, stdio to files, and *"the spawned `tokio::process::Child` handle dropped without ever being awaited"* (`crates/cyrup/src/subagent_runner_cmd.rs:1-7`). There is no `close` event and no parent left by the time the runner ends (R-SA-078).

**So the runner observes its own close.** The exact site:

> `background/runner_main/finish.rs:236` `pub(super) async fn finish_run(...)` — the ONE funnel every exit path goes through (`runner_main/mod.rs:33-45` states this as an enforced-by-construction invariant, and `entry.rs:202-216` is its single call).

`finalize_process_terminal` is called from **`entry.rs`'s tail, immediately after the `finish_run(...)` call returns** — not from inside `finish_run` — because `finish_run` owns R-SA-077's status→ResultFile ordering and must not grow a third write between them, and because the lease release (§2) must happen between the two.

The `RunnerCloseObservation` the runner builds for itself:
- `process_instance_id` = `config.runner_process_instance_id`.
- `close_observed_at` = `crate::time::now_epoch_millis()`.
- `exit_code` = **`Some(0)`**, and this is a TRUE statement, not a fabrication: `crates/cyrup/src/subagent_runner_cmd.rs:216-223` maps `run(...)` to `Ok(()) => 0 / Err(_) => 1`, and `entry.rs::run_with` returns `Ok(())` on every path that reaches `finish_run` (the doc at `entry.rs:44-54` says so outright: *"every internal failure is captured into a terminal `Failed` … rather than propagated"*). A run whose steps FAILED still exits 0 — that is the run's outcome, carried by `RunState`, not the runner's.
- `signal` = **`None`, always.** A runner that died of a signal never reached this line, so its sidecar stays `pending` and every reader falls to the pid ladder. **That is upstream's crash semantics reproduced exactly**, and it is the answer to the seed's *"`status` cannot tell a crash from a slow start"*: a crash now reads `pending` forever where a clean end reads `observed`.

`RunnerCloseObservation` is therefore a 4-field struct with `exit_code: Option<i32>` and `signal: Option<String>` (upstream's `number | null` / `string | null`), populated as above. **Do not invent a way to make `signal` non-`None`** — there is none, and a fabricated one is the "diagnostic tool that names a file that does not exist is lying" failure `run_lifecycle_debug.rs:20-22` already warns about.

**`TerminationOutcome` needs NOTHING.** See §6 — the seed's premise here is false and the corrective work is deleting the stale ledger sentence.

**`writers` / `expectedWriters`: port upstream's shape, which is EMPTY.** `subagent-runner.ts:5168-5190` is the only writer of the field, it is unconditional, and its own comment is `// Children run inside this process, so no step has writer processes to prove terminal.` (`:5169`) — every `writers[i]` is `[]` and every `expectedWriters[i]` is `0`. Trace it through `finalizeProcessTerminal`: `allWriters` is empty, `inconsistentWriters` is false (every index is present in both maps and every expected is 0), `expectedEntries.length === steps` so the `writer-close-unverified` guard at `:277` does not fire, and `:279`'s `process-tree-unverified` cannot fire over an empty list — so upstream reaches `observed` at `:284`. **Reproduce exactly that.** cyrup's children ARE real OS processes with real `TerminationOutcome`s and a process-GROUP kill path (`spawn/signal.rs:476-514`), so cyrup *could* populate real `PiWriterProcessInstanceExit` records — that would go BEYOND upstream v0.68.0 and is deliberately NOT in this batch (see `sizing`). The `process-tree-unverified` and `writer-close-unverified` arms are then reachable exactly as `ActiveAsyncCapacityKind::Workflow` is — through serde, from a candidate written by another build — and `active_async_capacity/mod.rs:74-78` is the in-tree precedent for stating that without `#[allow(dead_code)]`.

**The crash signal falls out of this for free, and must be preserved.** `initialize_process_terminal` writes `writers: {}` and NO `expectedWriters`. If the runner dies before reaching the candidate write, `finalize` (run by a reader, or never) sees `expectedWriters = {}` and `allWriters = []` → `:277`'s `allWriters.length === 0 && expectedEntries.length === 0` → **`writer-close-unverified`**. Reached the end → all-zero expected over N steps → `observed`. Keep that asymmetry; it is the crash/clean discriminator.

#### (4) `SESSION_LEASES_DIR` — cyrup HAS an equivalent root, and tests confine it via `Roots::sandboxed`.

Upstream: `session-lease.ts:8` `path.join(TEMP_ROOT_DIR, "session-leases")`, `TEMP_ROOT_DIR` at `types.ts:2775`.

cyrup's `TEMP_ROOT_DIR` is **`background::temp_root_dir`** (`background/artifact_roots.rs:243`, injectable form `:260`), surfaced as **`Roots::run_scratch()`** (`paths.rs:305`, resolved at `:235`). Five leaves already hang off it. Add the sixth, in `artifact_roots.rs` beside `CAPACITY_SUBDIR`:

```rust
/// Path segment, under [`temp_root_dir`], holding one directory per CANONICAL SESSION FILE's
/// revival lease — pi `SESSION_LEASES_DIR`, `path.join(TEMP_ROOT_DIR, "session-leases")`
/// (`runs/shared/session-lease.ts:8` @v0.68.0). The leaf name is upstream's, byte for byte.
///
/// The SECOND root in this crate keyed by neither `cwd_key` nor `SessionId` — see
/// [`CAPACITY_SUBDIR`]'s block for why that convention is stated rather than assumed. This one is
/// keyed by the sha256 of the REALPATH of the session file, so two cyrup instances in different
/// working directories that revive the SAME session file contend for the SAME lease, which is the
/// entire hazard VL-S3 names.
const SESSION_LEASES_SUBDIR: &str = "session-leases";

#[must_use]
pub fn session_leases_root_in(roots: &crate::paths::Roots) -> PathBuf {
    roots.run_scratch().join(SESSION_LEASES_SUBDIR)
}
```

**Test confinement — mandatory, and this is the project's own scar.** `paths.rs:186-191` records that an unconfined root *"once left 59,321 files there"*. Every lease test builds `Roots::sandboxed(tmp.path())` (`paths.rs:268-280`) and passes `session_leases_root_in(&roots)` as the `root_dir` argument. **Every lease function takes `root_dir: &Path` explicitly** — mirroring upstream's `rootDir = SESSION_LEASES_DIR` default parameter (`:106`, `:110`, `:204`) but with NO default, so there is no path by which a test can accidentally reach the shared root. The production callers (§3) are the only places `session_leases_root_in` is named. A `#[test]` that names it is a bug.

### 3. The Rust shape

New directory `crates/cyrup-ext-subagents/src/background/session_lease/` and
`crates/cyrup-ext-subagents/src/background/process_terminal/`, each behind the crate's private-module
facade (`background/mod.rs` re-exports; the `runner_main/`, `active_async_capacity/`,
`async_retention/` pattern).

```
background/session_lease/
  mod.rs        module doc (the four staleness rungs, the rename claim, the tombstone), re-exports
  types.rs      SessionLeaseRequest, SessionLeaseOwner, WriterState, SessionLeaseState,
                LeaseToken, CanonicalSessionId, parse_owner
  error.rs      SessionLeaseError (thiserror)
  identity.rs   ProcessStartIdentity, process_start_identity(pid), process_demonstrably_gone
  acquire.rs    create_lease_directory, acquire_session_lease, SessionLeaseHandle
  inspect.rs    canonical_session_file_path, canonical_session_id, session_lease_dir,
                inspect_session_lease, demonstrably_stale
background/process_terminal/
  mod.rs        module doc (the ladder, in order), re-exports
  id.rs         RunnerProcessInstanceId
  types.rs      ProcessTerminalState, ProcessTerminalReason, ProcessInstanceExit,
                ProcessTreeTerminal, CanonicalSessionTerminal, ProcessTerminal,
                ProcessTerminalCandidate, RunnerCloseObservation
  error.rs      ProcessTerminalError (thiserror)
  candidate.rs  read/write candidate, initialize_process_terminal,
                mark_process_terminal_candidate_lease_release
  proof.rs      validate_proof, sanitize_process_terminal, read_process_terminal
  finalize.rs   finalize_process_terminal, overlay_status, step_process_terminal_proof
```

**Newtypes and enums — no stringly-typed states anywhere.**

```rust
// session_lease/types.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriterState { None, Spawning, Running }      // upstream `:27`

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] pub struct LeaseToken(String);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] pub struct CanonicalSessionId(String); // 64 hex

/// The two cross-field rules (`:141-142`) are expressed BY TYPE, not by a validator:
/// `Running` carries its pid; the other two arms cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseWriter {
    None,
    Spawning,
    Running { pid: u32, start_identity: Option<ProcessStartIdentity> },
}

pub enum SessionLeaseState {                           // upstream `:42-45`
    Free   { canonical_session_file: PathBuf, canonical_session_id: CanonicalSessionId },
    Owned  { canonical_session_file: PathBuf, canonical_session_id: CanonicalSessionId,
             owner: Box<SessionLeaseOwner> },
    Unreadable { canonical_session_file: PathBuf, canonical_session_id: CanonicalSessionId },
}
```

`SessionLeaseOwner` keeps upstream's on-disk field set verbatim (`version:1`, `token`,
`canonicalSessionFile`, `runId`, `sourceRunId`, `parentSessionId?`, `pid`, `hostname`,
`processStartIdentity?`, `writerState`, `writerPid?`, `writerProcessStartIdentity?`, `acquiredAt`,
`acquiredAtMs`, `updatedAtMs`), camelCase, `skip_serializing_if = "Option::is_none"` on every
optional — **on-disk format unchanged**. `parse_owner` (upstream `:121-144`) is a hand-written
validator over `serde_json::Value`, NOT a derive: the two cross-field rules at `:141-142` and the
`pid > 0` integer rule at `:129-131` are refusals, and a derive would accept them. It returns
`Option<SessionLeaseOwner>` exactly as upstream does — a malformed owner is `unreadable`, never an
error.

```rust
// session_lease/identity.rs — upstream `:68-82`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessStartIdentity(String);               // `linux:<startTicks>`

/// `/proc/<pid>/stat`, field 20 (`starttime`) counted AFTER the last `)` so a comm containing
/// spaces or parens cannot shift it. Non-Linux, or any read/parse failure, is `None`.
#[must_use] pub fn process_start_identity(pid: u32) -> Option<ProcessStartIdentity>;
```

```rust
// session_lease/error.rs
#[derive(Debug, thiserror::Error)]
pub enum SessionLeaseError {
    /// `conflictMessage` with an owner (`:158-159`) — VERBATIM, and the deliverable.
    #[error("Direct revival of session '{canonical_session_file}' is already owned by run \
             '{run_id}' (source run '{source_run_id}'{parent}, pid {pid} on {hostname}). Wait for \
             that revival to finish or start a separate continuation without reusing this session \
             file.")]
    Conflict { canonical_session_file: String, run_id: String, source_run_id: String,
               parent: String, pid: u32, hostname: String, owner: Box<SessionLeaseOwner> },
    /// `conflictMessage` with no readable owner (`:156`) — VERBATIM.
    #[error("Direct revival of session '{canonical_session_file}' is blocked by an existing lease \
             with unreadable owner metadata. Refusing to reclaim it without proof that the owner \
             is stale.")]
    ConflictUnreadableOwner { canonical_session_file: String },
    /// `updateWriter`'s throw (`:244`) — VERBATIM.
    #[error("Session revival lease ownership changed for run '{run_id}'.")]
    OwnershipChanged { run_id: String },
    #[error("Failed to resolve canonical session path '{path}': {source}")]
    Canonicalize { path: PathBuf, #[source] source: std::io::Error },
    #[error("Failed to claim session lease directory '{path}': {source}")]
    Claim { path: PathBuf, #[source] source: std::io::Error },
}
```

`parent` is `""` or `format!(", parent session '{id}'")` — upstream `:158`. Bridged into
`SubagentError` with `#[from]` at `error.rs:270-275`'s pattern.

```rust
// session_lease/acquire.rs
pub struct SessionLeaseHandle {
    lease_dir: PathBuf,
    owner: SessionLeaseOwner,
    root_dir: PathBuf,
}
impl SessionLeaseHandle {
    pub fn owner(&self) -> &SessionLeaseOwner;
    pub fn token(&self) -> &LeaseToken;
    /// upstream `:241-264`
    pub async fn update_writer(&mut self, writer: LeaseWriter) -> Result<(), SessionLeaseError>;
    /// upstream `:265-270` — `true` == ACKNOWLEDGED, which is what the candidate records.
    pub async fn release(&mut self) -> bool;
}

/// upstream `:202-293`. `root_dir` is REQUIRED (see §2.4).
pub async fn acquire_session_lease(
    request: &SessionLeaseRequest,
    root_dir: &Path,
    options: &SessionLeaseOptions,
) -> Result<SessionLeaseHandle, SessionLeaseError>;

/// upstream `:47-56`, every ambient input injected. `Default::default()` is production.
pub struct SessionLeaseOptions {
    pub now: fn() -> i64,                               // `crate::time::now_epoch_millis`
    pub token: fn() -> LeaseToken,                      // uuid v4
    pub pid: Option<u32>,
    pub hostname: Option<String>,                       // default: `machine_hostname()`
    pub process_start_identity: Option<ProcessStartIdentity>,
    pub liveness: fn(u32) -> Liveness,                  // default: `check_pid_liveness`
    pub start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
}
```

**`update_writer` is NOT dead code and must not be `#[allow(dead_code)]`.** Upstream never calls it
at v0.68.0 (`git grep -n updateWriter v0.68.0 -- 'src/**/*.ts'` → the definition only). cyrup DOES
have a caller: the runner's step loop spawns a REAL child process per step, so
`update_writer(LeaseWriter::Spawning)` before the dispatch and
`update_writer(LeaseWriter::Running { pid })` once `SpawnedChild` reports its pid is a true and
production-reachable use, in `runner_main/executor.rs`'s per-step dispatch. **This is the one place
cyrup's out-of-process children make the port MORE complete than upstream's own runner, and it is
what makes the `spawning` and `running` staleness rungs reachable rather than serde-only.** Write
the `[CYRUP-DELTA]` saying exactly that, with the `git grep` as its evidence.

**`processStartIdentity`'s fallback rung.** Upstream `:210-212`:
`options.processStartIdentity ?? getIdentity(pid) ?? (pid === process.pid ? `runtime:${…uptime…}` : undefined)`.
Port the third rung as `ProcessStartIdentity(format!("runtime:{}", process_start_epoch_millis()))`
for `pid == std::process::id()` only — on Linux rung 2 always answers, so rung 3 is the non-Linux
path and a `/proc`-less container. Do not silently drop it: without it, a non-Linux owner has NO
start identity, `processDemonstrablyGone` degrades to bare liveness, and pid reuse is back.

```rust
// process_terminal/types.rs — all three of upstream's shapes, as discriminated unions
#[derive(…)] #[serde(rename_all = "kebab-case")]
pub enum ProcessTerminalReason {                        // types.ts:633-643, ALL TEN
    ObserverUnavailable, RunnerCandidateMissing, RunnerInstanceMismatch,
    WriterCloseUnverified, ProcessTreeUnverified, CanonicalSessionUnavailable,
    CanonicalSessionLeaseActive, CanonicalSessionReleaseUnverified,
    ProofWriteFailed, StaleRepair,
}

#[derive(…)] #[serde(tag = "state", rename_all = "kebab-case")]
pub enum ProcessTerminal {                              // types.ts:701-713
    Pending    { #[serde(flatten)] base: ProcessTerminalBase },
    NotStarted { #[serde(flatten)] base: ProcessTerminalBase },
    Observed   { #[serde(flatten)] base: ProcessTerminalBase, observed_at: i64,
                 instances: Vec<ProcessInstanceExit>,
                 #[serde(skip_serializing_if="Option::is_none")]
                 canonical_session: Option<CanonicalSessionTerminal> },
    Unknown    { #[serde(flatten)] base: ProcessTerminalBase, reason: ProcessTerminalReason,
                 #[serde(skip_serializing_if="Option::is_none")] diagnostic: Option<String> },
}

#[derive(…)] #[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProcessInstanceExit {                          // types.ts:645-651 + :672-680, union :682
    Runner   { process_instance_id: RunnerProcessInstanceId, close_observed_at: i64,
               exit_code: Option<i32>, signal: Option<String> },
    PiWriter { process_instance_id: String, attempt: u32, close_observed_at: i64,
               exit_code: Option<i32>, signal: Option<String>, process_tree: ProcessTreeTerminal },
}
```

`ProcessTreeTerminal` (`types.ts:653-670`) has three arms; note upstream's own `validProcessInstance`
(`process-terminal.ts:46-53`) accepts ONLY `posix-process-group` on the observed side and rejects
`windows-taskkill` — an upstream inconsistency. **Port the TYPE with all three arms (on-disk format)
and the VALIDATOR as upstream wrote it**, with a `[CYRUP-DELTA]` naming the inconsistency and the two
line numbers, so a reader does not "fix" it into divergence.

`ProcessTerminalError` carries upstream's throw sentences verbatim, one variant each:
`process-terminal.ts:79` (`Invalid process-terminal candidate in '{dir}'.`), `:83`, `:88`, `:91`,
`:95`, `:96`, `:97`, `:163`, `:165`, `:166`, `:168`, `:171`, `:172`, `:174`, `:176`. **Fifteen
sentences, all deliverables.**

```rust
// process_terminal/finalize.rs — the ladder, upstream `:243-310`, in upstream's order
pub async fn finalize_process_terminal(
    run_dir: &RunDir,
    run_id: &RunId,
    close: &RunnerCloseObservation,
    lease_root: &Path,
) -> ProcessTerminal;
```

`RunDir` grows two accessors (`background/run_paths.rs`, after `recovery_descriptor()` at `:102-111`):
`process_terminal_candidate()` → `<run_dir>/process-terminal-candidate.json`,
`process_terminal()` → `<run_dir>/process-terminal.json`. **Two new `const` file-name literals, each
spelled once**, per that family's own stated drift-prevention rationale (`:92-97`).

`RunStatus` and `StepStatus` (`background/records.rs:233-235`, `:22-25`; both `camelCase`, neither
`deny_unknown_fields`) each gain
`#[serde(default, skip_serializing_if = "Option::is_none")] pub process_terminal: Option<ProcessTerminal>`
— additive and round-trip-safe for statuses written by older builds. `overlay_status`
(upstream `:224-241`) writes BOTH, with the per-step state derived from `expectedWriters[i]` vs
`writers[i]` exactly as `:233` derives it, and **swallows every failure** (`:238-240`:
*"The proof sidecar remains authoritative when terminal status is unavailable."*).

`SUBAGENT_LIFECYCLE_ARTIFACT_VERSION: u32 = 3` (`types.ts:629`) — a new `pub const` in
`process_terminal/mod.rs`, stamped on the event line (`:305`).

### 4. The PRODUCTION call sites — every one, with its upstream counterpart

| # | cyrup site | what lands there | upstream |
|---|---|---|---|
| P1 | `extension/executor/background.rs::spawn_background_steps`, just before `write_atomic_json(&cfg_path, …)` (`:801`) | mint `RunnerProcessInstanceId`, set it + `revival_lease` on `RunnerConfig` | `async-execution.ts:707`, `:710` |
| P2 | same fn, **before** `spawn_detached_runner_with_command` (`:839`) | `initialize_process_terminal(run_dir, run_id, instance)` — candidate (0600, `write_private_atomic_json` `atomic.rs:185`) + `state:"pending"` proof (`write_atomic_json` `:75`) | `async-execution.ts:851` (ordering delta: §2.1) |
| P3 | same fn, `handle.mark_started(pid)` (`:866`) | signature grows the instance id; `ActiveAsyncCapacityOwner::runner_process_instance_id` is bound | `async-execution.ts:859` `onBeforeProceed?.(runnerProcessInstanceId)` |
| P4 | `extension/executor/control.rs::revive_from_transcript` (`:283-500`), on the `BackgroundStepsSpec` it builds (`:468-472`) | `revival_lease: Some(SessionLeaseRequest{ session_file, run_id, source_run_id: source_id, parent_session_id })` | `subagent-runner.ts:219` `config.revivalLease` |
| P5 | `background/runner_main/entry.rs::run_with`, after `load_runner_config` (`:103`), before `publish_initial_status` (`:110`) | `acquire_session_lease` when `config.revival_lease.is_some()`; conflict ⇒ straight to `finish_run(Failed, <conflict sentence>)` | `subagent-runner.ts:5241-5242` |
| P6 | `background/runner_main/entry.rs::publish_initial_status` (`:266+`) | `status.process_terminal = Pending{run_id, instance}` from the config | `subagent-runner.ts:2102` |
| P7 | `background/runner_main/executor.rs`, per-step dispatch | `handle.update_writer(Spawning)` before spawn, `Running{pid}` once `SpawnedChild` has a pid | `session-lease.ts:241-264` (no upstream caller — see §3's delta) |
| P8 | `background/runner_main/finish.rs`, tail of `finish_run` (`:236`) | write the candidate: `run_id`, instance, `writers`/`expected_writers` one empty entry per step, `session_file`, `revival_lease_token` | `subagent-runner.ts:5168-5190` |
| P9 | `background/runner_main/entry.rs`, after the `finish_run(…)` call (`:202-216`) | `lease.release()` → `mark_process_terminal_candidate_lease_release(dir, token, acknowledged)` | `subagent-runner.ts:5280-5292` |
| P10 | `background/runner_main/entry.rs`, immediately after P9, before `Ok(())` | `finalize_process_terminal(run_dir, run_id, &close, lease_root)` — writes sidecar, on `Observed` calls `release_active_run_index` (`active_run_index.rs:198`), overlays status, appends `subagent.run.process_terminal` via `append_event` (`events.rs:75`) | `async-execution.ts:784-789` + `process-terminal.ts:299-308` |
| P11 | `background/active_async_capacity/inspect.rs::runner_release_verdict`, new FIRST rung at `:256` | `read_process_terminal(run_dir, {run_id, instance})` — `Observed` + both ids match ⇒ `releasable("matching observed process-terminal proof is present")`; anything else falls THROUGH to the existing pid rung (`:257-274`) and then to `abandoned_runner_release_verdict` | `active-async-capacity.ts:227-235` |
| P12 | `…/inspect.rs::runner_release_verdict`, before P11 | the early-failure carve-out `:222-226`: `status.process_terminal` is `NotStarted` + both ids match + `status.error` non-empty ⇒ `releasable("run failed before child startup completed")`. **The `[CYRUP-DELTA]` at `inspect.rs:205-208` calling this rung "unrepresentable and dropped" becomes FALSE and must be DELETED.** | `active-async-capacity.ts:222-226` |
| P13 | `…/inspect.rs::workflow_release_verdict` (`:360-…`, the per-child loop at `:483`) | same: `status.process_terminal.runner_process_instance_id` present-check, `NotStarted` carve-out, then `read_process_terminal(child_dir, …)` | `active-async-capacity.ts:282-291` |
| P14 | `background/reconcile.rs::check_pid_liveness` — a NEW sibling, not a mutation | `pub fn check_pid_identity(pid: u32, expected: Option<&ProcessStartIdentity>) -> Liveness` — `check_pid_liveness` first, then on `Alive` with an `expected` present, re-read `process_start_identity(pid)` and answer `Dead` when it is `Some(other)` and differs. `None`/`Unknown` never downgrade to `Dead`. | `session-lease.ts:162-172` |
| P15 | `background/run_lifecycle_debug.rs` + `extension/executor/status.rs:439` | `RunnerLiveness` is REPLACED by a `sidecar`/`overlay` pair; see §5 | `run-status.ts:52-58`, `:95`, `:102-103` |
| P16 | `background/active_run_index.rs::read_live_active_run_ids` (`:370-…`) | the staleness rung gains upstream's OTHER disjunct: release the marker when `status.process_terminal` is `Observed`, else on age | `async-status.ts:575`, and cyrup's own `:56-59` / `:350-366` notes |
| P17 | `extension/rpc/ping.rs::ping_data` (`:71+`) | ADD `capabilities.processTerminalProof = {version:1, lifecycleArtifactVersion:3}` and `events.processTerminal = "subagent:process-terminal"` | `rpc.ts:458`, `:466`; `types.ts:2356` |

**`check_pid_liveness`'s existing callers — which want the new rung (P14):**

| caller | wants it? |
|---|---|
| `active_async_capacity/config.rs:149` (`pid_liveness` default, consumed by `inspect.rs:259`) | **YES.** This is the §D3 substitute; the recycled-pid hole `PARITY-GAPS.md:1451` names is exactly here. Widen `CapacityOptions::pid_liveness` to carry the owner's recorded start identity, or add a second injected `identity_of` alongside it. |
| `async_retention/lock.rs:96` (`liveness`, used at `:264-268`) | **YES**, and it is nearly free: `RetentionLockIdentity` already carries `pid` + `hostname` + `startedAt` and already refuses a hostname mismatch (`:259`). A recycled pid there retains the sweep lock forever, the same defect. |
| `background/control.rs:568` | **NO.** It is a "is there anything to signal" probe before a stop/interrupt, not a staleness verdict; there is no recorded start identity to compare and the answer is used immediately. |
| `extension/executor/foreground_actions/dismiss.rs:134` | **NO.** `is_possibly_alive()` gating a dismissal; a false `Dead` would dismiss a live run. |
| `extension/executor/status.rs:439` (via `RunnerLiveness::probe`) | **NO** — it is being replaced wholesale by P15. |
| `background/reconcile.rs`'s own step 4 | **NO**, deliberately. Its `Dead` arm SYNTHESISES A FAILURE (`reconcile.rs:35-38`), and its 24h staleness threshold (`DEFAULT_STALE_AFTER`, `:135`) already exists *because* "OS pid reuse makes indefinite alive trust unsound" (`:133-135`). Adding a same-tick identity rung there would make a run whose runner is genuinely alive-under-a-different-identity fail instantly, which is a behaviour change reconciliation does not want and no ledger row asks for. **Say this out loud in the module doc**, or the next reader will "complete" the rollout and break it. |

### 5. In-tree premises this batch makes FALSE — each DELETED, never reworded

Run first: `grep -rn 'process-terminal\|process_terminal\|ProcessTerminal\|processTerminal' --include=*.rs crates/`
and `grep -rn 'VL-S3\|VL-S4' --include=*.rs --include=*.md . | grep -v '^./tmp/'`.
**`crates/cyrup-tui/**` hits are pi's TUI `ProcessTerminal` object — unrelated, do not touch.**

1. **`background/run_lifecycle_debug.rs:6-27`** — the whole `# The process-terminal lines, and why there are two instead of upstream's three` block, including its `grep` claim (*"finds only `//!`/`///`/`//` comments and string literals"*), *"cyrup has neither the sidecar nor the overlay, neither a reader nor a writer"*, and *"upstream's overlay keys would be dropped by serde on read"*. **DELETE the block.** Replace with upstream's real three lines.
2. **`background/run_lifecycle_debug.rs:74-82`** — the `[CYRUP-DELTA]` on `RunnerLiveness`. **DELETE the type and the delta.** `RunnerLiveness` (`:82-120`), `PROCESS_TERMINAL_NOT_RECORDED` (`:121-124`), `RunLifecycleDebug::runner` (`:67`) and every test at `:264-330` go with it, replaced by `sidecar: Option<ProcessTerminal>` / `overlay: Option<ProcessTerminal>` and a `format_process_terminal` porting `run-status.ts:47-50` exactly:
   `{state}{ (reason)? }{ ` · runner {id}`? }`, `"missing"` for `None`.
   The three lines become upstream's: `Process terminal file: <dir>/process-terminal.json` (`:95`),
   `Status process terminal: …` (`:102`), `Sidecar process terminal: …` (`:103`).
3. **`extension/executor/status.rs:439`** `RunnerLiveness::probe(&status, check_pid_liveness)` — the production producer; becomes `debug_process_terminal(run_dir, &status)` (upstream `:52-58`: sidecar = `read_process_terminal`, overlay = `sanitize_process_terminal(status.process_terminal, …)`).
4. **`background/active_async_capacity/mod.rs:59-72` (§D3)** — *"**cyrup has neither input**"*, *"nothing in this crate mints a `runnerProcessInstanceId`"*. **False the moment P1 lands. REWRITE the section** to what is true: the proof is now the first rung, the pid ladder is the no-proof fallback beneath it, and the reason the pid ladder STAYS is that a run whose runner died before `finalize` has no proof and would otherwise be retained forever. The seed is right that deleting the pid ladder would be a regression.
5. **`background/active_async_capacity/inspect.rs:179-211`** — the `runner_release_verdict` doc. `:183-188` (*"cyrup has neither input … nothing in this crate mints a `runnerProcessInstanceId`; `RunStatus` has no such field"*) is false; `:205-208` (the early-failure carve-out *"unrepresentable and dropped"*) is false once P12 lands — **DELETE that bullet, do not reword it.** `:209-211` (`rollbackBeforeRunnerProceed` has no cyrup handshake) stays TRUE — cyrup still has no startup barrier; leave it.
6. **`background/active_async_capacity/inspect.rs:33`** — *"Always `"unknown"` (pi `:52`) — cyrup has no process-terminal artifact at all (§D3)"*, on the `ActiveAsyncCapacityReleaseEvidence` field. False; the field now carries the real proof state.
7. **`background/active_async_capacity/inspect.rs:257`, `:366`, `:469`, `:483`** — four in-body comments asserting the pid probe stands in for the proof. Each becomes "the SECOND rung, beneath the real proof".
8. **`background/active_async_capacity/key.rs:101-111`** — *"**cyrup has neither the artifact nor the identity**"*. False. Rewrite to describe `runner_process_instance_id` as upstream's field, now present, WITH `runner_pid` retained as the fallback ladder's input.
9. **`background/active_run_index.rs:56-59`** and **`:359-363`** — *"cyrup's `RunStatus` carries no process-terminal record at all, so `read_live_active_run_ids` applies the age disjunct alone"*. False once P16 lands. Both notes rewritten to describe the restored disjunction.
10. **`extension/rpc/ping.rs:60-66`** — the `processTerminalProof` and `events.processTerminal` bullets in the `[CYRUP-DELTA]`. **DELETE both bullets** and add the two keys (P17). The delta shrinks from five capability keys + two event keys to three + one.
11. **`src/tests/rpc_bridge_integration.rs:512-528`** — `processTerminalProof` must move OUT of the `dropped` capability array and `processTerminal` OUT of the `dropped` events array, each replaced by a positive `assert_eq!` on its value. **This test will FAIL loudly if P17 is done without it — which is the point; do not weaken it.**
12. **`registration/doctor.rs:1019-1027`** — the `[CYRUP-DELTA]` on the capacity release line (*"upstream's sentence names its process-terminal proof, which cyrup does not have"*) and the operator-facing string beneath it. Both become upstream's real sentence plus the retained fallback.
13. **`background/active_async_capacity/tests.rs:340`** — *"a `Complete` run has no observed process-terminal proof and is not `failed`"*. False for a run that reached `finalize`; the test's premise comment must be rewritten to say it covers the NO-PROOF path specifically.
14. **`crates/cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs:17-19`** — *"says the sidecar … `process-terminal.json` no code path writes"*. False. The IT's assertions change with P15.
15. **`docs/gap-analysis/PARITY-GAPS.md:1445-1447` (VL-S3)** and **`:1449-1453` (VL-S4)** — closed with evidence. **`:1453`'s sentence *"`TerminationOutcome` (`spawn/signal.rs:90-106`) carries only `status` + `stage`, with no `ExitStatus::signal()` name mapping"* is **ALREADY FALSE TODAY** — independently of this batch — and is the sentence the seed inherited. `09-cyrup-ext-subagents.md:617` (SUBA-023) already records it as REFUTED with the correct anchors. Delete it from the VL-S4 row as part of closing the row; it must not survive into the closure note.
16. **`docs/gap-analysis/09-cyrup-ext-subagents.md:617` (SUBA-023)** — its RESIDUAL (*"the two unported upstream subsystems only — `process-terminal.ts` and `session-lease.ts`"*, restated four times across four re-reads) is fully discharged. The row closes. Its `Kind` correction (`not-ported`, owed per its own last paragraph and `:1235`) is settled by the closure.
17. **`docs/gap-analysis/00-residual-ledger.md:91` (row 12), `:256-257`, `:368`** and **`PARITY-GAPS.md:1321`, `:36`** and **`09a-…-drift.md:3414`** — every list naming VL-S3/VL-S4 as open.
18. **`docs/gap-analysis/03-cyrup-session.md:936`** — *"it is the same hazard `VL-S3` records for pi-subagents, and cyrup has no lease machinery on either path"*. The second clause becomes false. That row is about `cyrup-session`'s SQLite backend and stays open, but its cross-reference must be corrected, not left claiming a zero-hit that no longer holds.

### 6. `TerminationOutcome` and SUBA-023 — the seed's item 3, corrected

**No work is needed on `spawn/signal.rs`, and the seed's "that mapping is IN SCOPE" is a false premise.**

- `TerminationOutcome` is at `spawn/signal.rs:112-127`, not `:90-106`.
- It already carries `pub signal_name: Option<&'static str>` (`:127`), documented at `:120-126` with the note that it is derived from the OBSERVED status rather than from `EscalationStage` deliberately.
- `signal_name(i32) -> Option<&'static str>` is `:136-158` (13 signals, `None` for an unrecognised number — *"a wrong name is worse than a number"*).
- `signal_name_of(&ExitStatus)` is `:162-170` and is literally `status.signal().and_then(signal_name)` under `#[cfg(unix)]`.
- `09-cyrup-ext-subagents.md:617` states: *"The 'no `ExitStatus::signal()` name mapping anywhere in the module' observation is REFUTED"*, names these exact anchors, and warns that the row's Verify clause is *"a closure trap"* because it PASSES on an unclosed item.
- And, per §2.3, the runner's own close observation can never carry a signal anyway: a signalled runner does not reach `finish_run`.

**The in-scope work is therefore item 15 of §5**: delete the stale sentence from `PARITY-GAPS.md:1453` and close SUBA-023's residual. Do not touch `spawn/signal.rs`.

### 7. Reachability tests — and why each FAILS if the implementation is gutted

Unit tests live beside their modules; the three ITs go in `crates/cyrup-it/tests/subagents/`.
**Every lease test builds `Roots::sandboxed(tempdir)` and passes `session_leases_root_in(&roots)`
explicitly (§2.4).**

**Lease — the four staleness rungs (`session-lease.ts:174-181`), one test each:**

1. `lease_on_a_foreign_hostname_is_never_stale` — owner `hostname: "elsewhere"`, pid injected `Dead`, `writer_state: None`. Second acquire must raise `Conflict`. *Gutted (rung 1 dropped): the owner looks dead, the acquirer steals a lease held by a live runner on another machine and two runners write one session file — VL-S3's exact hazard. The test goes from `Err(Conflict)` to `Ok(handle)`.*
2. `lease_whose_writer_is_spawning_is_never_stale_even_with_a_dead_owner` — owner pid `Dead`, `writer_state: Spawning`. Must `Conflict`. *Gutted (`:177` dropped): the unobservable window — a child that has been forked but has not yet reported its pid — is treated as gone; the lease is stolen while a writer is mid-spawn. `Err` → `Ok`.*
3. `lease_alive_at_a_different_start_identity_is_stale` — liveness `Alive`, recorded `processStartIdentity: "linux:100"`, injected `start_identity_of` returns `linux:999`. Second acquire must SUCCEED and must have renamed the old dir to `<leaseDir>.stale-<token>`. *Gutted (`:169-171` dropped, i.e. bare liveness): the recycled pid reads `Alive`, the lease is never reclaimable, and a legitimate revival is refused forever. `Ok` → `Err(Conflict)`. **This is the single test that proves the pid-reuse hole is closed.*** Assert the tombstone path too: *gutted to a plain `rm`, two concurrent contenders can each reclaim.*
4. `lease_running_writer_requires_both_pids_gone` — owner pid `Dead`, `writer_state: Running{pid: W}`, `W` `Alive` ⇒ must `Conflict`; flip `W` to `Dead` ⇒ must succeed. *Gutted (`:179-180` dropped): the lease is reclaimed while the actual writer process is still writing the session file.*

5. `a_second_acquirer_loses_the_rename_claim` (DoD 3) — two `create_lease_directory` calls against one `leaseDir` with different tokens, the first left in place. The second returns `false`, the loser's `<leaseDir>.candidate-<token>` is gone, and the winner's `owner.json` is byte-unchanged. *Gutted to `mkdir -p` / `create_dir_all`: both "win", both write `owner.json`, last-writer-wins, and `demonstrablyStale`'s whole contract is vacuous.* Add `lease_directory_is_0700_and_owner_json_is_0600` (`:185`, `:189`) — *gutted to default perms, another user on the box reads and forges the owner record.*

6. `conflict_sentences_are_byte_identical_to_upstream` (DoD 4) — both of `:156` and `:158-159`, with and without `parentSessionId`, asserted against string literals. *Gutted to a paraphrase: the operator-facing refusal an agent surfaces changes, and the `parent` clause's exact `, parent session '…'` comma-space placement is the sort of thing a rewrite silently loses.*

7. `parse_owner_refuses_the_two_cross_field_shapes` (`:141-142`) — `{writerState:"running"}` with no `writerPid` ⇒ `None`; `{writerState:"none", writerPid: 7}` ⇒ `None`. Plus `pid: 0` and `pid: -1` ⇒ `None` (`:129-131`). *Gutted to a serde derive: a hand-edited or truncated owner file parses, `demonstrablyStale` reads `Running` with no writer pid, `owner.writerPid !== undefined` is false, the lease is declared NOT stale and is never reclaimable — a permanent deadlock from one malformed byte.*

8. `canonical_session_id_is_the_sha256_of_the_realpath` — a symlink and its target must produce ONE id and ONE lease dir. *Gutted to hashing the raw argument: a revival through `~/.cyrup/sessions/x.jsonl` and one through its realpath take two different leases, and both run.*

**Process-terminal — one test per reason arm (DoD 5), all through `finalize_process_terminal`:**

9. `finalize_without_a_candidate_is_runner_candidate_missing` (`:258`). *Gutted to a bare `observed`: a runner killed before it wrote its candidate is reported as cleanly closed and its capacity slot is released while a zombie may still hold the session file.*
10. `finalize_with_a_mismatched_instance_is_runner_instance_mismatch` (`:259`). *Gutted: a proof written by run A is accepted as run B's. This is the arm the whole uuid decision (§2.1) exists to make meaningful.*
11. `finalize_with_an_owned_lease_is_canonical_session_lease_active` and `…_unreadable_lease_is_canonical_session_unavailable` (`:273-274`) — two arms off `SessionLeaseState`. *Gutted (the lease check dropped): a run whose session file is still leased by a live successor is declared terminally observed; `sessionProjection` then reports `freeAtObservation: true` about a file that is not free.*
12. `finalize_with_an_unacknowledged_revival_release_is_canonical_session_release_unverified` (`:275-276`) — candidate has `revivalLeaseToken`, `revivalLeaseReleaseAcknowledged` absent. *Gutted: the batch's P9→P10 ordering silently stops mattering and every revived run reports `observed` whether or not its lease actually came off.*
13. `finalize_with_inconsistent_writers_is_writer_close_unverified` (`:277`) — and the crash case: candidate straight from `initialize_process_terminal` (empty `writers`, no `expectedWriters`). *Gutted: **the crash-vs-clean discriminator disappears** — a runner that died at startup and one that finished cleanly both report `observed`, which is the exact ambiguity VL-S4 names.*
14. `finalize_with_an_unobserved_writer_process_tree_is_process_tree_unverified` (`:279`) — over a hand-written candidate (serde-reachable; see §2.3's note and the `mod.rs:74-78` precedent).
15. `finalize_reaches_observed_over_the_upstream_empty_writer_shape` (DoD 5's observed arm) — N steps, `writers[i] = []`, `expected[i] = 0` as P8 writes them. Assert `instances == [runner]`, `observedAt == close.close_observed_at`, and `resumeDisposition`. *Gutted (`:277`'s guard mis-ported to a bare `allWriters.is_empty()`): **every** clean run reports `writer-close-unverified`, no slot is ever released on proof, and §D3's fear comes true from the other direction.*
16. `an_existing_observed_or_unknown_proof_short_circuits` (`:248-252`) — call `finalize` twice; the second returns the first verbatim and appends NO second event. *Gutted: a reconciler that finalizes a second time overwrites a good proof and double-emits the lifecycle event.*
17. `a_non_durable_proof_write_emits_no_event_and_returns_unknown` (`:299-309`) — read-only run dir. Assert: no `subagent.run.process_terminal` line in `events.jsonl`, `release_active_run_index` NOT called, returned proof is `Unknown{ProofWriteFailed}`. *Gutted (the `durable` flag dropped): a consumer sees the event, believes the run closed, and no sidecar exists to confirm it — the exact "event without artifact" the seed flags.*
18. `validate_proof_throw_sentences_are_byte_identical` (DoD 4) — all fifteen, table-driven.
19. `sanitize_turns_a_bad_overlay_into_proof_write_failed_and_never_panics` (`:180-188`). *Gutted to a `?`: one corrupt `status.processTerminal` key makes `debug.run` and `async-status`'s projection fail outright instead of degrading.*

**Capacity, liveness, index, RPC:**

20. `capacity_releases_on_a_matching_observed_proof_before_probing_the_pid` (P11, DoD 6) — terminal run, real `observed` sidecar, injected `pid_liveness` that **panics if called**. Verdict `releasable("matching observed process-terminal proof is present")`. *Gutted (proof rung not first): the injected probe panics — the test cannot pass by accident.*
21. `capacity_still_falls_back_to_the_pid_ladder_with_no_proof` (DoD 6) — terminal, no sidecar, pid `Dead` ⇒ releasable via the pid reason. *Gutted (pid ladder DELETED when the proof rung landed): a run whose runner was SIGKILLed holds its slot forever; at `limit` the session can never spawn again — the §D3 catastrophe, re-introduced.*
22. `capacity_releases_a_pre_startup_failure_on_the_not_started_carve_out` (P12).
23. `check_pid_identity_reports_dead_for_a_recycled_pid` (P14, DoD 8) — `Alive` + recorded `linux:100` + current `linux:999` ⇒ `Dead`; `Alive` + current `None` ⇒ `Alive`; `Unknown` + any identity ⇒ `Unknown`. *Gutted to `check_pid_liveness`: the recycled pid reads `Alive`, the slot is retained forever — `PARITY-GAPS.md:1451`'s named defect. The `None` and `Unknown` cases are the counter-gutting: an over-eager rung that reads absence as death kills live runs.*
24. `active_run_marker_is_released_on_an_observed_proof_before_the_24h_age_rung` (P16) — *gutted: a marker from a run that closed one second ago is held for 24 hours and the run shows live in every listing.*
25. `ping_advertises_the_process_terminal_proof_and_event` — the rewrite of `rpc_bridge_integration.rs:512-528`. *Gutted (P17 skipped): the existing test already fails; with P17 but no test update it fails the other way. Either direction is loud.*

**Integration (cyrup-it, real detached runner, `--features it`):**

26. `process_terminal_sidecar_lands_on_a_real_background_run` — spawn a real background single through `spawn_background_steps`, wait for the ResultFile, then assert `<run_dir>/process-terminal.json` exists, is `observed`, and its `runnerProcessInstanceId` equals the one on the capacity owner AND on `status.processTerminal`. **This is the end-to-end production-path proof.** *Gutted anywhere in P1/P2/P8/P10: the file is missing, is `pending`, or its ids do not match.*
27. `a_killed_runner_leaves_a_pending_proof_and_the_pid_ladder_releases_the_slot` — launch, SIGKILL the runner pid, reconcile capacity. Sidecar stays `pending`; the slot releases via the pid rung. *This is the test that states VL-S4's whole claim: a killed runner is now DISTINGUISHABLE from a clean one.*
28. `two_concurrent_revivals_of_one_session_file_refuse_the_second` — two `revive_from_transcript` calls on one `session_file`; the second surfaces the `:158-159` conflict sentence and never spawns. *This is VL-S3's whole claim. Gutted: both spawn and both write the session file.*
29. `debug_run_prints_the_real_sidecar_overlay_pair` (DoD 7) — the rewrite of `debug_run_lifecycle_integration.rs`, asserting `Process terminal file:`, `Status process terminal:` and `Sidecar process terminal:` and the ABSENCE of `Process terminal: not recorded`. *Gutted: the deleted string is still there.*

### 8. Files touched

**New** — `background/session_lease/{mod,types,error,identity,acquire,inspect}.rs`,
`background/process_terminal/{mod,id,types,error,candidate,proof,finalize}.rs`,
`crates/cyrup-it/tests/subagents/process_terminal_lifecycle_integration.rs`,
`crates/cyrup-it/tests/subagents/session_lease_revival_integration.rs`.

**Modified** — `background/mod.rs` (two `pub mod` + re-exports), `background/run_paths.rs`
(two accessors + two consts), `background/records.rs` (two `process_terminal` fields),
`background/reconcile.rs` (`check_pid_identity` + the module-doc note on why step 4 does NOT take it),
`background/active_run_index.rs` (P16 + two notes), `background/artifact_roots.rs`
(`SESSION_LEASES_SUBDIR` + `session_leases_root_in`), `background/run_lifecycle_debug.rs`
(P15 — large rewrite), `background/active_async_capacity/{mod,inspect,key,claim,config,tests}.rs`
(P3/P11/P12/P13 + §5.4-8,13), `background/runner_main/{config,entry,executor,finish,events}.rs`
(P5-P10), `background/async_retention/lock.rs` (P14 adoption; `machine_hostname` gains a doc
pointer), `extension/executor/background.rs` (P1/P2/P3), `extension/executor/control.rs` (P4),
`extension/executor/requests.rs` (`BackgroundStepsSpec::revival_lease`),
`extension/executor/status.rs` (P15's producer), `extension/rpc/ping.rs` (P17), `error.rs`
(two `#[from]` variants), `src/tests/rpc_bridge_integration.rs` (§5.11),
`registration/doctor.rs` (§5.12),
`crates/cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs` (§5.14),
`docs/gap-analysis/{PARITY-GAPS.md,00-residual-ledger.md,09-cyrup-ext-subagents.md,09a-cyrup-ext-subagents-v0.57-drift.md,03-cyrup-session.md}` (§5.15-18).
**Untouched: `crates/cyrup-tui/**` (a different `ProcessTerminal`) and `spawn/signal.rs` (§6).**

### 9. Sizing — honest

This is **two medium features plus a wide re-statement pass**, and the re-statement is not the small
part. Order matters more than raw size, because three pieces are cheap only if the one before them
already landed.

**Small (do first, each is an afternoon):** the `RunnerProcessInstanceId` newtype and its mint
(`RunId::new()` verbatim, ~40 LOC); the two `RunDir` accessors and two file-name consts; the
`SESSION_LEASES_SUBDIR` leaf and `session_leases_root_in` (a five-line copy of
`active_async_capacity_root_in`); the two `process_terminal` fields on `RunStatus`/`StepStatus`;
`check_pid_identity` (~30 LOC over the existing `check_pid_liveness`); and `ping.rs`'s two keys.
**Do the mint FIRST** — every other piece either writes it or reads it, and it is the thing that
makes `validateProof`'s matching meaningful at all.

**Medium:** `process_terminal/` as a whole — ~600 LOC of Rust for 310 lines of TypeScript, because
the four-arm `ProcessTerminal` union, the two-arm `ProcessInstanceExit` union, the three-arm
`ProcessTreeTerminal` and fifteen verbatim error sentences are all type-level work that TypeScript
gets for free. `finalize_process_terminal`'s ladder itself is mechanical once the types exist; the
fiddly parts are `overlay_status`'s per-step state derivation (`:233`, one dense ternary) and the
`durable` flag's ordering (`:299-309`), both of which have a test above that fails loudly if mis-ported.

**Medium-large:** `session_lease/`. ~700 LOC, and the risk is concentrated in two places: the
rename-claim (`:183-200` — POSIX `rename` onto a non-empty directory is the atomic primitive, and
`std::fs::rename` on Linux gives it; a `create_dir_all` "simplification" silently destroys the
whole mechanism) and the per-owner tombstone (`:278-288`, whose three-line upstream comment is the
only explanation of why it is per-owner and must be reproduced). `parse_owner` is tedious rather
than hard.

**Large, and the piece most likely to be underestimated:** §5's eighteen items. Nine separate
modules carry a `[CYRUP-DELTA]` or a doc block whose premise this batch falsifies, several of them
long-form (`run_lifecycle_debug.rs`'s module doc is 40 lines and goes entirely; `inspect.rs`'s
`runner_release_verdict` doc is 33 lines and loses a third). Two of them — `rpc_bridge_integration.rs:512-528`
and `debug_run_lifecycle_integration.rs` — are **tests that will FAIL** until they are updated, and
that is a feature: they are the tripwires that stop the delta pass being skipped. **Budget this at
roughly a third of the batch.** It is where "a delta whose premise has become false is DELETED,
never reworded" gets tested, and `run_lifecycle_debug.rs`'s block is the one most tempting to
soften rather than remove.

**Explicitly OUT of this batch, and it is not a narrowing:** populating real
`PiWriterProcessInstanceExit` records from cyrup's out-of-process children. Upstream v0.68.0 writes
all-empty `writers`/all-zero `expectedWriters` unconditionally (`subagent-runner.ts:5168-5190`,
comment at `:5169`), so porting that shape IS the port. Threading real per-step exits would cross
into `exec/` (`attempt_runner.rs`, `run_sync`, `SpawnedChild`) and go BEYOND upstream — file it as a
residual whose premise is true as written: *"cyrup's children are real OS processes with real
`TerminationOutcome`s (`spawn/signal.rs:112-127`) and a process-group kill path (`:476-514`), so
`ProcessTreeTerminal::Observed{posix-process-group}` is producible here where it is not in upstream
v0.68.0; the candidate's `writers` map is currently written in upstream's own all-empty shape."*
Do NOT file it as "VL-S4 partially closed" — VL-S4 closes in this batch.

**Genuine blockers: none.** Every dependency (`uuid`, `sha2`, `nix`, `tempfile`), every primitive
(`machine_hostname`, `check_pid_liveness`, both atomic writers, `release_active_run_index`,
`append_event`, `Roots::sandboxed`) and every seam (`RunnerConfig`, `BackgroundStepsSpec`,
`finish_run`, `mark_started`) already exists in the tree.

---

## [EXEC — proof]

**Step 1 of 3 — the mint and the process-terminal proof.** VL-S4's positive half is CLOSED and
reachable end to end through a real background run. VL-S3 (the session lease's WRITE half) and the
delta/re-statement pass (§5) remain for steps 2 and 3.

### What landed

**The mint (P1).** `RunnerProcessInstanceId` — a v4 uuid on `RunId::new()`'s idiom — is minted in
the ORCHESTRATOR at `extension/executor/background.rs::spawn_background_steps`, immediately before
`write_atomic_json(&cfg_path, &runner_config)`, which is pi `async-execution.ts:707` at pi's own
position. It reaches three places from there: `RunnerConfig::runner_process_instance_id` (pi `:710`),
the candidate + pending sidecar (`initialize_process_terminal`), and — written by the RUNNER from
the config — `RunStatus::process_terminal` (pi `subagent-runner.ts:2102`). `Option` on the config
field solely so a `runner-config.json` written by an older build still reads; `None` makes the
runner write no candidate and no proof rather than inventing an identity no reader holds.

**Ordering [CYRUP-DELTA].** `initialize_process_terminal` is called **before**
`spawn_detached_runner_with_command`, not after it as pi does at `:851`. pi can wait because its
runner blocks on `waitForStartupControl` (`subagent-runner.ts:5233-5240`); cyrup has no such
barrier, and the grep that proves it now matches only the two comments quoting it — the premise is
stated that way in both, because a grep-claim that its own comment falsifies is exactly the kind of
false premise this project deletes. Both artifacts' write is a FALLIBLE PRE-SPAWN step: a failure
refuses the launch and rolls the capacity slot back, which is upstream's `:853-857` behaviour
reached more cheaply (no process exists yet to terminate).

**`background/process_terminal/`** — `mod.rs` (the ladder in upstream's order,
`SUBAGENT_LIFECYCLE_ARTIFACT_VERSION = 3`, `PROCESS_TERMINAL_EVENT_TYPE`), `id.rs`, `types.rs`
(the four-arm `ProcessTerminal`, the two-arm `ProcessInstanceExit` — the candidate's writers hold
`PiWriter`, the union name is the import — the three-arm `ProcessTreeTerminal`,
`CanonicalSessionTerminal`, `ProcessTerminalCandidate`, `RunnerCloseObservation`, and the
cyrup-only `WriterProcessLedger`), `error.rs` (all **fifteen** verbatim sentences), `candidate.rs`
(`valid_process_instance` over RAW JSON so pi `:44`'s *absence* rule survives,
`read/write_process_terminal_candidate`, `initialize_process_terminal`,
`mark_process_terminal_candidate_lease_release`), `proof.rs` (`validate_proof`,
`sanitize_process_terminal`, `read_process_terminal`, `unknown_proof`, and the two tolerant field
decoders), `finalize.rs` (`resume_disposition`, `session_projection`, `step_process_terminal_proof`,
`overlay_status`, `finalize_process_terminal`).

**`session_lease/`** — the READ half that already existed (`canonical_session_id`,
`inspect_session_lease`, `parse_owner`) is now wired into `background/mod.rs` and consumed by the
ladder's lease rungs. `session_leases_root_in` + `SESSION_LEASES_SUBDIR` landed in
`artifact_roots.rs`; every lease/ladder entry point takes `root_dir: &Path` with NO default, so no
test can reach the shared machine-wide root by omission.

**Also landed as asked** — `RunStatus::process_terminal` / `StepStatus::process_terminal`, the two
`RunDir` accessors + two file-name consts, `ping.rs`'s `capabilities.processTerminalProof` and
`events.processTerminal`.

### The writers map is REAL, and that is the point

Upstream writes all-empty `writers` and all-zero `expectedWriters` unconditionally
(`subagent-runner.ts:5168-5190`) for one stated reason (`:5169`): *"Children run inside this process,
so no step has writer processes to prove terminal."* That is false of cyrup, so reproducing the
empty shape would have been a proof that verified nothing about the children that actually wrote the
session. What ships instead:

- `LiveEventSink` gains a third channel, `with_writer_process_sink` / `emit_writer_process`, carrying
  a `WriterProcessObservation` (`Launched` at the spawn, `Closed(exit)` at the close). No new
  `RunOptions` field — the runner already installs this sink per step with the flat index.
- `exec/attempt_runner.rs` emits `Launched` immediately after `SpawnedChild::spawn` succeeds, and
  `report_writer_process_close` on **every** path out of `run_attempt` (ordinary, interrupted,
  timed out) — above the two early returns, which is why it sits where it does.
- `spawn/signal.rs::verify_process_group_terminated` is the evidence: every subagent child is its own
  process-group leader (`command.process_group(0)`, `spawn/mod.rs:795`, so `pgid == pid`), so
  `kill(-pgid, 0)` asks about the child AND every descendant it did not deliberately detach. `ESRCH`
  ⇒ `ObservedProcessGroup{processGroupId, verifiedAt}`; a group that still has members after a
  250 ms re-probe budget, or an `EPERM`, ⇒ `verification-failed`; no addressable pid ⇒ **`signal-failed`**
  (the probe was never sent, which is a different fact); non-Unix ⇒ `unsupported-platform`. All three
  of upstream's reasons are reachable, each meaning exactly what its name says.
- The runner accumulates one `WriterProcessLedger` per flat step (`launched` counted at the spawn,
  `exits` at the close — deliberately different moments, which is what keeps
  `inconsistentWriters` a live verdict) and writes them as `writers`/`expectedWriters` at its close,
  seeding an explicit zero-count entry for every declared step so a run whose steps were all skipped
  stays distinguishable from a runner that died before declaring anything.

`finalize`'s `process-tree-unverified` (`:279`) and `inconsistentWriters` (`:271-272`) rungs are
therefore live verdicts about real OS processes, not arms upstream's own shape can never reach.
`finalize_own_process_terminal` also carries the run's `session_file` onto the candidate, where pi
carries only `config.revivalLease?.sessionFile` — a strictly stronger true statement, documented as
a delta.

### Two findings the port forced

1. **A per-step overlay proof must NOT go through `validateProof`.** `stepProcessTerminalProof`
   (`:201-222`) writes `instances: records` — that step's WRITERS and nothing else — so an observed
   step proof carries no runner instance, which is precisely what `:174` refuses. pi never hits the
   contradiction because it only ever sanitizes `status.processTerminal` (`run-status.ts:56`) and its
   `AsyncStatus` is untyped JSON elsewhere. `StepStatus` therefore uses `deserialize_step_overlay`
   (lenient typed decode) and `RunStatus` uses `deserialize_overlay` (full sanitize). Unifying them
   degrades every observed step to `proof-write-failed`; the comment on both says so. This was found
   by a test going red, not by reading.
2. **The `:277` guard's two disjuncts need separate mutations to prove.** Gutting it to a bare
   `all_writers.is_empty()` leaves the clean-run test GREEN *because cyrup's writers are non-empty* —
   the mutation that would have exposed the mis-port upstream is inert here. The two halves are
   pinned separately (M10, M11) instead.

### Production call sites

| # | site | what lands |
|---|---|---|
| P1 | `extension/executor/background.rs::spawn_background_steps`, before `write_atomic_json(&cfg_path, …)` | mint `RunnerProcessInstanceId`, set it on `RunnerConfig` |
| P2 | same fn, **before** `spawn_detached_runner_with_command` | `initialize_process_terminal` — candidate (0600) + `state:"pending"` proof; failure refuses the launch and rolls back the slot |
| P6 | `background/runner_main/entry.rs::publish_initial_status` | `status.process_terminal = Pending{run_id, instance}` from the config |
| P7' | `background/runner_main/executor.rs::build_step_run_options` | installs the writer-process sink on the step's `LiveEventSink` |
| W | `exec/attempt_runner.rs::run_attempt` / `prepare_attempt` | emits `Launched` / `Closed` with a verified `ProcessTreeTerminal` |
| P8 | `background/runner_main/entry.rs::finalize_own_process_terminal` | writes the candidate from the per-step ledgers |
| P10 | same fn, after `finish_run` returns | `finalize_process_terminal` — sidecar, `release_active_run_index` on `Observed`, overlay, `subagent.run.process_terminal` event |
| P17 | `extension/rpc/ping.rs::ping_data` | `capabilities.processTerminalProof` + `events.processTerminal` |

The runner observes its OWN close because cyrup's orchestrator cannot: `spawn_detached_runner_with_command`
detaches the child and drops the `Child` handle unawaited (`crates/cyrup/src/subagent_runner_cmd.rs:1-7`),
so there is no `close` event and no parent left (R-SA-078). `exit_code` is `Some(0)` — a true
statement, not a convenience (`subagent_runner_cmd.rs:216-223` plus `run_with`'s
"every internal failure is captured into a terminal `Failed` … rather than propagated"). `signal` is
**always `None`**: a signalled runner never reaches that line, so its sidecar stays `pending`
forever — which IS the crash discriminator the seed asks for, and the reason no signal name is
fabricated there.

### Reachability tests (33 new)

`background/process_terminal/tests.rs` (24) — one per ladder arm through `finalize_process_terminal`
(`runner-candidate-missing`, `runner-instance-mismatch`, `canonical-session-lease-active`,
`canonical-session-unavailable`, `canonical-session-release-unverified`, `writer-close-unverified`
in all three shapes, `process-tree-unverified`, `observed`), the short-circuit, the durability
ordering, `overlay_status`'s `:233` derivation and its swallow-everything contract, all fifteen
sentences byte-for-byte plus each one raised from a value that trips it, `validProcessInstance`'s
exact accept set (including the `windows-taskkill` asymmetry and the runner-`attempt` absence rule),
the candidate's seven refusals, the lease-release stamp's token match, `resume_disposition`, the
wire-word pin, and the on-disk JSON shapes.

`spawn/signal.rs` (2) — a real child that exits leaving nothing verifies GONE; a real child that
forks a `sleep 30` into its group and exits does NOT. Plus the unaddressable-pid cases (including
pid 0, which under `kill(2)` would address the CALLER's own group).

`extension/executor/background.rs` (2) — `a_launch_mints_one_identity_and_establishes_both_lifecycle_artifacts`
and `a_launch_that_cannot_establish_ownership_is_refused_and_rolls_back_its_slot`, both through the
real `spawn_background_steps`.

`crates/cyrup-it/tests/subagents/process_terminal_lifecycle_integration.rs` (4) — the end-to-end
proof. A real two-step background run through `run_with` spawning real `cyrup-subagent-fixture`
children: the candidate names one real writer per step with a verified posix process group (and each
record is re-checked through pi's own `validProcessInstance`), the sidecar is `observed` carrying
`[runner, writer, writer]` with the minted identity, the overlay lands on the run and on every step,
the event line is emitted exactly once at `lifecycleArtifactVersion: 3`, and no lease directory is
created for a run that held no session file. Plus: a foreign expectation is refused with upstream's
own sentence; a launch with no minted identity writes nothing rather than inventing one; and — read
MID-RUN, because the close overlays the same key — the runner's first `status.json` carries the
`pending` proof with the LAUNCH's identity.

`src/tests/rpc_bridge_integration.rs` — `processTerminalProof` and `processTerminal` moved OUT of the
dropped lists into positive `assert_eq!`s.

### Gutting mutations — 25, each RED, each restored byte-for-byte

| # | gutted | test that went RED |
|---|---|---|
| M1 | `runner-candidate-missing` rung | `finalize_without_a_candidate_is_runner_candidate_missing` |
| M2 | run/instance identity rung | `finalize_with_a_mismatched_instance_is_runner_instance_mismatch` |
| M3 | lease-not-free rung | `finalize_with_a_held_lease_refuses_on_the_matching_arm` |
| M4 | unacknowledged-release rung | `finalize_with_an_unacknowledged_revival_release_is_release_unverified` |
| M6 | process-tree rung | `finalize_with_an_unobserved_writer_process_tree_is_process_tree_unverified` |
| M7 | the `durable` gate | `a_non_durable_proof_write_emits_no_event_and_returns_unknown` |
| M8 | existing-proof short circuit | `an_existing_observed_or_unknown_proof_short_circuits` |
| M9 | `:233`'s `expected == 0` head | `overlay_derives_each_steps_state_from_its_own_writer_counts` |
| M10 | `inconsistentWriters` half of `:277` | `finalize_with_inconsistent_or_empty_writers_is_writer_close_unverified` |
| M11 | both-empty (crash) half of `:277` | `finalize_with_inconsistent_or_empty_writers_is_writer_close_unverified` |
| M12 | seeded the launch candidate with per-step counts | `initialize_writes_an_empty_candidate_and_a_pending_proof` + the `:277` test |
| M13 | pi `:44`'s runner-`attempt` absence rule | `valid_process_instance_reproduces_upstreams_exact_accept_set` |
| M14b | `validateProof`'s runner-identity check (`:166`) | `validate_proof_raises_each_guard_from_a_value_that_trips_it` |
| M15 | the status overlay's tolerant decode | `a_corrupt_status_overlay_degrades_instead_of_failing_the_whole_status_read` |
| M16 | the process-group probe (always "gone") | `an_empty_group_verifies_gone_and_a_surviving_descendant_does_not` |
| M17 | the mint (a constant token) | `a_minted_instance_id_is_unique_across_many_mintings` |
| M18 | the pre-spawn `initialize_process_terminal` | both `spawn_background_steps` launch tests |
| M19 | the mint never reaching `RunnerConfig` | `a_launch_mints_one_identity_and_establishes_both_lifecycle_artifacts` |
| M20 | `ping`'s two keys | `a_host_drives_ping_over_the_inter_extension_bus_and_gets_a_reply` |
| M21 | `RunDir::process_terminal` naming the candidate file | `initialize_writes_an_empty_candidate_and_a_pending_proof` |
| M22 | the attempt reporting no close | IT `a_real_background_run_leaves_an_observed_proof_naming_its_real_children` |
| M23 | the writer sink never installed | same IT |
| M24 | upstream's all-empty writer shape | same IT |
| M25 | the runner never finalizing | same IT |
| M26 | no pending proof on the first status write | IT `the_runner_publishes_a_pending_proof_on_its_first_status_write` |

(A 26th attempt — gutting `:277` to a bare `all_writers.is_empty()` against the clean-run test —
came back GREEN and was replaced by M10/M11. That inertness is itself the finding recorded above.)

### Gates

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` —
  **`Summary [  95.770s] 10716 tests run: 10716 passed, 9 skipped`** (baseline 10683 + 33).
- `cargo nextest run -p cyrup-it --features it` —
  **`Summary [ 309.286s] 571 tests run: 571 passed, 0 skipped`** (baseline 567 + 4).

### For step 2 and step 3

- **VL-S3's write half is untouched and unclaimed.** `session_lease/mod.rs`'s own doc states which
  half is present; there is no `acquire`, no handle and no release, so no caller can believe a lease
  is being held. `finalize`'s lease rungs already consume `inspect_session_lease`, so wiring
  `acquire_session_lease` into the runner (P4/P5/P9) changes no signature in this step's work — the
  candidate's `revival_lease_token` / `revival_lease_release_acknowledged` fields, their refusals and
  the `canonical-session-release-unverified` rung all exist and are tested.
- **§5's delta pass is untouched.** Every premise this step falsified is still in the tree:
  `active_async_capacity/{mod,inspect,key}.rs`'s "cyrup has neither input" / "nothing in this crate
  mints a `runnerProcessInstanceId`" (now FALSE — P1 mints it, `RunStatus` carries it),
  `run_lifecycle_debug.rs`'s whole "there is no sidecar and no overlay" block and its
  `RunnerLiveness` delta, `active_run_index.rs:56-59` / `:359-363`, `registration/doctor.rs:1019-1027`,
  `debug_run_lifecycle_integration.rs:17-19`, and the ledger rows. P11–P16 are the consuming half and
  are not claimed here.
- **No residual is filed.** The one the augment pre-authorised — "populating real
  `PiWriterProcessInstanceExit` records is out of this batch" — is not filed, because it was done.

---

## [EXEC — lease]

**Step 2 of 3 — the session lease, the identity probe, and the capacity rungs.** VL-S3 is CLOSED
and reachable end to end: a second revival of one session file is refused, by a real runner, with
upstream's sentence. VL-S4's release rung now reads the real proof FIRST. The §5 delta/re-statement
pass outside the three modules touched here remains for step 3.

### The lease's write half — `background/session_lease/`

`acquire.rs` (new), `error.rs` (new), plus `identity.rs` and `inspect.rs` grown from the read half
step 1 left. `mod.rs`'s "the WRITE half … is not in this module yet" block is **deleted**, not
softened: its premise is false as of this step.

**The rename-claim (`session-lease.ts:183-200`).** `create_lease_directory` builds
`<leaseDir>.candidate-<token>` at 0700, writes `owner.json` at 0600 into it, and `rename`s the whole
DIRECTORY onto `<leaseDir>`. POSIX `rename(2)` refuses to rename onto a non-empty directory,
atomically, which is the entire claim. The candidate is removed on every exit path including the
successful one (pi's `finally`, `:197-199`). Eight concurrent acquirers against one empty root
produce exactly ONE winner (`a_second_acquirer_loses_the_rename_claim`); gutted to `create_dir_all`
the count goes to 8 and the losers overwrite the winner's `owner.json`.

**The per-owner tombstone (`:278-288`).** Upstream's three-line comment is reproduced verbatim on
`acquire_session_lease` and then unpacked, because it is the only explanation that exists: the
tombstone is named after the token of the record the contender READ, so N contenders that observed
the same stale owner all compute the SAME occupied destination, `rename(2)` gives it to one, and the
rest loop and find the winner's lease instead. Named after the CONTENDER, every one of them would
have a free destination — all N would "break" the lease and the second through would move the
FIRST's freshly-claimed successor aside. M-L11 is that mutation, and it goes red.

**The four staleness rungs (`:174-181`)**, each pinned:

| rung | test | what the gutted build does |
|---|---|---|
| `:175` foreign host ⇒ never stale | `lease_on_a_foreign_hostname_is_never_stale` | steals a lease held by a live runner on another machine |
| `:176` owner pid demonstrably gone | `lease_alive_at_a_different_start_identity_is_stale` | a recycled pid reads alive forever; the lease is never reclaimable |
| `:177` `spawning` ⇒ NEVER stale | `lease_whose_writer_is_spawning_is_never_stale_even_with_a_dead_owner` | steals the lease from a child that is mid-fork |
| `:179-180` `running` ⇒ writer pid also gone | `lease_running_writer_requires_both_pids_gone` | reclaims while the writer is still writing the session file |

`parse_owner`'s two cross-field rules are at **`:141-142`** (the AUG's correction of the seed's
`:145-146`) and were already pinned in step 1; this step adds the write-side counterpart —
`the_record_this_build_writes_is_the_record_its_validator_reads`, which is what stops a build from
producing a record its own validator refuses and deadlocking every future revival of that file.

### Who holds the lease, and for how long

**One process — the RUNNER — from just after config load to just after the step loop settles, on the
revival path only** (pi `subagent-runner.ts:5241-5293`).

| # | site | what lands |
|---|---|---|
| P4 | `extension/executor/control.rs::revive_from_transcript` | `BackgroundStepsSpec::revival_lease` — EXPLICIT, never derived from `session_file.is_some()` (a seeded fork carries one too) |
| P1' | `extension/executor/background.rs::spawn_background_steps` | copies it onto `RunnerConfig::revival_lease` |
| P5 | `runner_main/entry.rs::run_with`, after `load_runner_config`, before `publish_initial_status` | `acquire_session_lease`; a conflict funnels to `finish_run(Failed, <sentence>)` |
| P7 | `runner_main/executor.rs::run_single` + its `LiveEventSink` | `WriterUpdate::Spawning` at the dispatch, `Running { pid }` from the child's own `Launched`, `None` at its close |
| P8 | `runner_main/entry.rs::finalize_own_process_terminal` | the candidate's `revivalLeaseToken` + `revivalLeaseReleaseAcknowledged` |
| P9 | `runner_main/entry.rs`, after `finish_run` returns | `release()` → `mark_process_terminal_candidate_lease_release` |
| P10 | immediately after P9 | `finalize_process_terminal` — so `sessionProjection`'s "free AND acknowledged" can actually be true |

The P9→P10 order is the whole reason `canonical-session-release-unverified` is a distinguishable
arm; reversed, every revived run's proof is `unknown`. M-I1 gutted the release and the IT went red.

`update_writer` reaches the handle through a channel drained by ONE task (`spawn_lease_writer_task`),
modelled on the telemetry pump: the observations arrive inside a synchronous `LiveEventSink`
callback while `update_writer` is an `async` filesystem write that re-checks the token on every
call, which two concurrent holders would race.

**`[CYRUP-DELTA]` — cyrup feeds a field upstream defined and never fed.**
`git grep -n updateWriter v0.68.0 -- 'src/**/*.ts'` finds the definition and nothing else, because
pi's children run INSIDE the runner (`subagent-runner.ts:5169`). cyrup's are real OS processes with
real pids, so the `spawning` and `running` rungs are reachable here rather than serde-only —
`the_lease_records_the_running_child_as_its_writer_mid_run` reads a genuine child's pid out of
`owner.json` while that child is alive.

### A refused revival is upstream's `persistPreProceedStartupFailure`, not an invention

`refuse_run` writes pi `async-execution.ts:617-652`'s record field for field: `state: "failed"`, the
conflict sentence as `error`, and `processTerminal: { state: "not-started", runId,
runnerProcessInstanceId }` (`:636-640`). The SIDECAR stays `pending` — nothing observed a close, so
nothing claims one. That triple is not decoration: it is the input to the capacity pool's
early-failure carve-out, which is what gives P12 below a REAL producer instead of a hand-written
status.

### The identity probe — `check_pid_identity` (P14)

`background/reconcile.rs` gains `check_pid_identity(pid, expected)` and its injectable core
`check_pid_identity_with`. It is ONE ladder with three spellings and no second copy: the lease's
`process_demonstrably_gone` is that verdict compared against `Liveness::Dead`, and
`async_retention/lock.rs`'s rungs 3+4 are now one call to it. `Liveness` keeps three states — the
AUG is right that cyrup's three-way enum already expresses upstream's `true`/`false`/`undefined`
split correctly, including `EPERM ⇒ alive`.

The `/proc/<pid>/stat` parser is **not** rebuilt: `async_retention/lock.rs`'s copy was DELETED and
its callers now use `session_lease::process_start_identity`, typed as `ProcessStartIdentity`
(`#[serde(transparent)]`, so the retention lock's on-disk record is unchanged). Two parsers that
drift would give two incompatible answers about one pid.

**Which existing `check_pid_liveness` callers take the new rung — stated in the module doc**, as a
table, because the next reader will otherwise "complete" the rollout:

* **yes** — the lease's ladder, `async_retention/lock.rs` rung 4, the capacity release verdict.
* **no** — `background/control.rs`'s pre-signal probe (no recorded identity; the answer is consumed
  in the same breath), `foreground_actions/dismiss.rs` (a false `Dead` dismisses a live run), and
  **`reconcile`'s own step 4, deliberately**: its `Dead` arm SYNTHESISES A FAILURE, its 24-hour
  `DEFAULT_STALE_AFTER` exists *precisely because* pid reuse makes indefinite alive-trust unsound,
  and `RunStatus` carries a pid but no start identity to compare against. A same-tick rung there
  would fail a live run instantly.

### The capacity rungs — the proof is FIRST, the pid ladder STAYS BENEATH

`ActiveAsyncCapacityOwner` gains pi's own `runnerProcessInstanceId` (`:27`) plus cyrup's
`runner_process_start_identity`, both bound by `mark_started(pid, instance)` at the spawn.
`is_started()` widens to the three-way disjunct so a slot written by an older build still reads as
started.

`runner_release_verdict` is now `async` and runs upstream's order:

* **P12** `:222-226` — `not-started` + matching ids + non-empty `status.error` ⇒
  `"run failed before child startup completed"`. **The `[CYRUP-DELTA]` at `inspect.rs` calling this
  rung "unrepresentable and dropped" is DELETED**, because it is false.
* **P11** `:227-235` — `read_process_terminal(run_dir, ProofExpectation::new(run_id, instance))`;
  `observed` with both ids matching ⇒ `"matching observed process-terminal proof is present"`.
* **the fallback ladder** — `check_pid_identity_with` over the owner's pid and recorded identity.
  Reached only when the first two have no answer.
* `abandoned_runner_release_verdict`, with pi's own `process-terminal proof is <state|missing>`
  prefix now genuinely populated.

**Why the pid ladder stays, stated where a reader will find it:** the proof is written by the RUNNER
at its own close, because cyrup's orchestrator detaches its runner and drops the `Child` handle
unawaited (R-SA-078). A `SIGKILL`ed runner writes no proof, ever. Delete the fallback and every such
run holds its slot forever — at `limit`, the session can never spawn again.
`capacity_still_falls_back_to_the_pid_ladder_with_no_proof` is the existing
`a_completed_run_with_a_dead_runner_pid_releases_its_slot`, whose premise comment is rewritten to
say it covers the NO-PROOF path specifically.

P13 does the same for `workflow_release_verdict`'s per-child loop: the child's own published
identity, its `not-started` carve-out, then `read_process_terminal(child_dir, …)`, with the pid rung
beneath.

**Premises rewritten because this step made them false** (each DELETED, never softened):
`active_async_capacity/mod.rs` §D3 ("cyrup has neither input", "nothing in this crate mints a
`runnerProcessInstanceId`"); `key.rs`'s "cyrup has neither the artifact nor the identity" block;
`inspect.rs`'s `runner_release_verdict` doc and its module header; the
`ActiveAsyncCapacityReleaseEvidence::process_proof` note ("cyrup has no process-terminal artifact at
all"); `claim.rs`'s `rollbackBeforeRunnerProceed` paragraph; `process_terminal/id.rs`'s "the pair
§D3 substituted for this identity"; `active_async_capacity/tests.rs`'s module header and the
`a_completed_run_with_a_dead_runner_pid_releases_its_slot` premise; and
`cyrup-it/tests/subagents/main.rs`'s file count (39 → 41, which was already wrong by one before this
step).

### Two findings the port forced

1. **A gutting mutation of the verdict's own id-match came back GREEN, and that is the finding.**
   `read_process_terminal` already runs the sidecar through `validate_proof` with the reader's
   expectation, so a proof belonging to another runner degrades to `unknown` at the READ. The
   verdict-level re-match (pi `:232-234`) is therefore belt-and-braces, and the mutation that
   actually exposes the mis-port is at the read — passing `ProofExpectation::none()`. M-C2 is
   recorded as inert; M-C2b replaces it and goes red. (Step 1 recorded the same class of finding for
   pi `:277`.)
2. **The `spawning` window opens at the DISPATCH, not at the spawn**, and pinning it needed a
   deterministic case the integration test cannot give: `Spawning`→`Running` is a race against a
   real child's startup. `a_dispatch_reports_spawning_before_the_child_exists` drives `run_single`
   with a binary that does not exist, so `SpawnedChild::spawn` fails, no `Launched` is ever emitted,
   and the channel holds exactly one observation, every time.

### Reachability tests (18 new)

`background/session_lease/tests.rs` (10) — the four staleness rungs; the eight-way rename-claim
race; 0700/0600; both conflict sentences byte-for-byte with and without `parentSessionId`, plus the
unreadable-owner sentence; `update_writer`/`release` refusing after the lease was retaken, and the
real holder's release being acknowledged; the write→validate round trip including the writer keys
appearing and disappearing; and the `runtime:` start-identity fallback with its this-process-only
guard.

`background/reconcile.rs` (2) — the recycled pid answering `Dead`, its three counter-rungs
(`expected` absent, current identity absent, `Unknown` never upgraded) and a probe that must not be
consulted at all for a confirmed-dead pid; plus the production spelling over this live process.

`background/active_async_capacity/tests.rs` (5) — the proof rung firing BEFORE a pid probe that
PANICS if called; a foreign instance's proof not releasing; the `not-started` carve-out and its
empty-error counter-rung; and the fallback ladder releasing a recycled runner pid while retaining an
unchanged one.

`background/runner_main/executor.rs` (1) — the `spawning` window, above.

`crates/cyrup-it/tests/subagents/session_lease_revival_integration.rs` (3, real runner, real
children) — the lease held for a whole run and released at the close with an acknowledged release on
the candidate and an `observed` proof whose `canonicalSession.freeAtObservation` is true; **a second
revival refused with pi's sentence, against an incumbent whose pid is genuinely alive on this
genuine hostname** (VL-S3's whole claim); and a real child's pid read out of `owner.json` mid-run.

### Gutting mutations — 20, each RED, each restored byte-for-byte

| # | gutted | test that went RED |
|---|---|---|
| M-L1 | staleness rung 1 (hostname) | `lease_on_a_foreign_hostname_is_never_stale` |
| M-L2 | staleness rung 3 (`spawning`) | `lease_whose_writer_is_spawning_is_never_stale_even_with_a_dead_owner` |
| M-L3 | the start-identity disjunct (bare liveness) | `lease_alive_at_a_different_start_identity_is_stale` + `check_pid_identity_reports_dead_for_a_recycled_pid` + `the_fallback_ladder_releases_a_recycled_runner_pid` + `a_reused_pid_makes_the_lock_stale_even_while_the_pid_is_alive` |
| M-L4 | staleness rung 4b (writer pid) | `lease_running_writer_requires_both_pids_gone` |
| M-L5 | the rename-claim → `create_dir_all` | `a_second_acquirer_loses_the_rename_claim` |
| M-L6 | the 0700/0600 modes | `lease_directory_is_0700_and_owner_json_is_0600` |
| M-L7 | paraphrased the sentence's parent clause | `conflict_sentences_are_byte_identical_to_upstream` |
| M-L8 | `release`'s token re-check | `a_handle_whose_lease_was_retaken_refuses_to_write_or_release` |
| M-L9 | left the writer keys behind on the way down | `the_record_this_build_writes_is_the_record_its_validator_reads` |
| M-L10 | the `runtime:` start-identity rung | `a_procless_owner_still_records_a_stable_runtime_identity` |
| M-L11 | tombstone named after the CONTENDER | `lease_alive_at_a_different_start_identity_is_stale` |
| M-C1 | the proof rung moved BELOW the pid ladder | `a_matching_observed_proof_releases_the_slot_before_the_pid_is_ever_probed` |
| M-C2b | the proof read WITHOUT the reader's expectation | `a_proof_from_another_runner_instance_does_not_release_the_slot` |
| M-C3 | the early-failure carve-out | `a_pre_startup_failure_releases_on_the_not_started_carve_out` |
| M-C4 | the carve-out's non-empty-error test | `a_not_started_proof_without_an_error_does_not_release` |
| M-C5 | the fallback ladder back to bare `kill(pid, 0)` | `the_fallback_ladder_releases_a_recycled_runner_pid` |
| M-P7 | the dispatch never opens the `spawning` window | `a_dispatch_reports_spawning_before_the_child_exists` |
| M-I1 | the runner never releases its lease | IT `a_revival_run_holds_the_lease_for_its_whole_run_and_releases_it_at_the_close` |
| M-I2 | the runner ignores a refused acquire | IT `a_second_revival_of_one_session_file_is_refused_with_upstreams_sentence` |
| M-I3 | the writer channel never installed on a step's sink | IT `the_lease_records_the_running_child_as_its_writer_mid_run` |

(M-C2 — dropping the VERDICT's own id-match — came back GREEN and was replaced by M-C2b. That
inertness is the first finding recorded above.)

### Gates

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
  (`SessionLeaseError::Conflict`'s payload is boxed as `SessionLeaseConflict` for
  `clippy::result_large_err`; the format string still names every field, so the sentence stays
  readable in one place.)
- `cargo nextest run --workspace --features test-fixtures` —
  **`Summary [  93.869s] 10734 tests run: 10734 passed, 9 skipped`** (step-1 baseline 10716 + 18).
- `cargo nextest run -p cyrup-it --features it` —
  **`Summary [ 312.813s] 574 tests run: 574 passed, 0 skipped`** (step-1 baseline 571 + 3).

### For step 3

- **§5's remaining delta pass is untouched and unclaimed.** `run_lifecycle_debug.rs`'s whole "there
  is no sidecar and no overlay" block and its `RunnerLiveness` delta, `extension/executor/status.rs`'s
  producer, `active_run_index.rs:56-59` / `:359-363` (P16), `registration/doctor.rs:1019-1027`,
  `debug_run_lifecycle_integration.rs:17-19`'s sidecar note, and the ledger rows (`PARITY-GAPS.md` VL-S3/VL-S4,
  `00-residual-ledger.md`, `09-cyrup-ext-subagents.md` SUBA-023 including the stale
  `TerminationOutcome` sentence at `PARITY-GAPS.md:1453`, `09a-…-drift.md:3414`,
  `03-cyrup-session.md:936`) are all still standing. P15 and P16 are step 3's.
- **`spawn/signal.rs`'s SIGNAL-NAME half needs nothing** — §6 of the augment, re-verified (the
  file's process-GROUP half is a different half of this same batch and did change; see
  `[FIX — round 1]`): `TerminationOutcome` is at
  `:112-127` and already carries `signal_name`. The in-scope work there is deleting the stale ledger
  sentence, which is step 3's.
- **No residual is filed.** Nothing in this step was narrowed, and no dependency was found missing.

---

## [EXEC — premises]

**Step 3 of 3 — the eighteen false-premise items, `debug.run`'s real reader, and the ledger.**
VL-S3 and VL-S4 are CLOSED with evidence, SUBA-023's residual is discharged, and every in-tree
premise this batch falsified is DELETED rather than softened. One hollow advertisement the
previous steps left behind is now paid for by a real emitter (see "The finding").

### `debug.run` now reads what the batch writes (P15)

`background/run_lifecycle_debug.rs`'s 40-line module doc — *"cyrup has neither the sidecar nor the
overlay, neither a reader nor a writer"*, its `grep` claim, and *"upstream's overlay keys would be
dropped by serde on read"* — is **deleted**, with `RunnerLiveness` (the type AND its
`[CYRUP-DELTA]`), `PROCESS_TERMINAL_NOT_RECORDED`, `RunLifecycleDebug::runner` and the pid-probe
test that pinned them. In their place:

- `format_process_terminal` — pi `run-status.ts:47-50`, all three terms: the state, the `(reason)`
  parenthetical only the `unknown` arm has, the ` · runner <id>` tail, and `"missing"` for an
  absent record.
- `debug_process_terminal` — pi `:52-58`, returning the `{sidecar, overlay}` pair read against
  ONE expectation (`:53`: this run's id, and the runner instance the status itself names).
- The three lines in upstream's own positions: `Process terminal file:` between
  `Workflow receipt:` and `Session:` (`:95`), then `Status process terminal:` /
  `Sidecar process terminal:` after `Mode:` (`:102-103`).
- `Capacity runner: <runnerProcessInstanceId>` at pi's `:69`, which the old comment said cyrup
  could not print because it minted no instance id. It does now. `Capacity runner pid:` stays
  beside it as cyrup's own line, because cyrup's runner is detached and has a pid upstream's
  in-process child does not.

**Why the pair is not a duplicate**, stated in the module doc because a later reader will be
tempted to drop one: an `observed` sidecar under a `pending` overlay says the proof landed and the
status write behind it did not; a `pending` sidecar with no overlay says the runner never reached
its close. That disagreement is the diagnosis.

**`[CYRUP-DELTA]` — the overlay's sanitize already happened one layer lower.** Upstream's
`AsyncStatus.processTerminal` is untyped JSON, so `:56` runs `sanitizeProcessTerminal` at read
time. cyrup's `RunStatus::process_terminal` is a typed field whose own decoder IS that call. What
a serde field decoder cannot do is `:53`'s expectation — it is handed no access to the sibling
`runId` — so the one check upstream makes here that the decoder could not is re-applied over the
decoded value, with upstream's own refusal sentence as the diagnostic. A `status.json` copied from
another run degrades instead of being reported as this run's.

### The active-run index gets its second disjunct back (P16)

`read_live_active_run_ids`'s staleness rung reads the proof FIRST and falls to the 24-hour age
threshold second (pi `async-status.ts:574-576`). The module-doc bullet that recorded the
`readProcessTerminal` disjunct as unappliable is deleted; the rung's own note now says what each
disjunct answers. The consequence is user-visible: a run that closed one second ago stops being
reported as live by every listing that goes through this reader, instead of being held for a day.

### The ledger

- **`PARITY-GAPS.md` VL-S3 and VL-S4** — struck, dated, bodies kept as history per this
  directory's id-retention rule, with per-bullet evidence (the modules, the production call sites,
  the two `--features it` proofs, and what each substitute became).
- **`PARITY-GAPS.md:1453`'s `TerminationOutcome` sentence is DELETED, not carried into the
  closure.** It was false before this batch began: `spawn/signal.rs:112-127` has carried
  `signal_name` since sweep 1, `signal_name(i32)` is `:136-158`, `signal_name_of(&ExitStatus)` is
  `:162-170`, and `09-cyrup-ext-subagents.md:617` recorded the observation as REFUTED. **The
  SIGNAL-NAME half of `spawn/signal.rs` was untouched** (`:112-127`, `:136-158`, `:162-170`),
  because nothing needed to be — the seed's "that mapping is IN SCOPE" was the one premise the
  augment had already refuted, and the in-scope work was this deletion. The FILE is not untouched:
  the owned-process-tree half of this same batch added `ProcessGroupTerminal` and
  `verify_process_group_terminated` there (+207 lines), consumed at `exec/attempt_runner.rs:652`,
  and that call is what fills each candidate writer's `ProcessTreeTerminal`.
- **`09-cyrup-ext-subagents.md` SUBA-023** — CLOSED; its `Kind` cell corrected to `not-ported`,
  the correction the row's own body had owed itself; `:575`'s open-residual list, `:1235`'s
  kind-correction paragraph, `:1243` and SUBA-005's `debug.run` note all corrected.
- **`00-residual-ledger.md`** — row 12 struck with the closure, `:256-257`'s still-open list
  annotated, and item 5 of the "what this costs" section (*"a second subsystem now runs on a
  workaround for this row"*) DISCHARGED with the reason the pid ladder survives underneath.
- **`09a-…-drift.md:3414`** — struck, and its own finding (cyrup's process-GROUP kill path is
  stronger than upstream's single-pid kills) is now the reason the owned-process-tree half landed
  ABOVE upstream rather than at it.
- **`03-cyrup-session.md:936`** — the clause *"cyrup has no lease machinery on either path"* is
  false as written and is corrected. **That row stays open**: the new lease guards pi-subagents'
  async session files, not `cyrup-session`'s store, and nothing in `cyrup-session` acquires it.
- **`registration/doctor.rs`** — the operator-facing `release:` line becomes upstream's real
  sentence (`doctor.ts:198`) with ONE added clause for cyrup's pid fallback, and the
  `[CYRUP-DELTA]` says why an operator needs to know a slot can come back on a pid verdict.

### The finding: an advertised topic that nothing published

`extension/rpc/ping.rs`'s own module doc states the rule — *"a key advertised and not implemented
is a lie the client will act on"* — and `watch/observer.rs`'s states the corollary — *"a topic
constant belongs to whatever publishes on it, so an advertisement can never name a topic nothing
emits."* P17 had added `events.processTerminal = "subagent:process-terminal"`, and
`grep -rn 'SUBAGENT_PROCESS_TERMINAL_EVENT'` found the constant, the advertisement and the test —
**and no emitter**. A client subscribing to it would have waited forever.

So the emitter is built: `ProcessTerminalAnnouncingCompletionObserver`
(`background/watch/observer.rs`), pi `emitProcessTerminalEvent`
(`async-execution.ts:666-672`), registered in the production completion composite
(`extension/executor/notices.rs`) after the async-complete announcer. The constant moves to the
emitter's module, beside `SUBAGENT_ASYNC_COMPLETE_EVENT`, as that module's own rule requires.

**`[CYRUP-DELTA]` — the emit site moves from the launcher to the completion fan-out.** Upstream
emits from the PARENT, right after `finalizeProcessTerminal` returns on the runner's `close`
event. cyrup's runner is genuinely detached (R-SA-078), so it finalizes its own proof and has no
bus; the parent's first in-process edge after that write is the completion watcher. The delta is
the LATENCY, bounded below by that watcher's poll cadence — the same delta `CompletionBus` already
documents — never the payload: what is published is the sidecar the runner wrote, verbatim.

**A `pending` sidecar publishes nothing**, and that is upstream's shape too: its emit is downstream
of a `finalizeProcessTerminal` that a killed runner never ran. The absence of the event is itself
the crash signal, and the capacity rung's pid ladder is what speaks for that run instead.

### Reachability tests (8 new, 2 rewritten)

`background/run_lifecycle_debug.rs` (6 new, replacing the pid-probe test) — the three
`formatProcessTerminal` terms and `"missing"`; the three lines in place with an `observed` sidecar
under a `pending` overlay; both lines `missing` for a run that never reached
`initialize_process_terminal`; upstream's full line ORDER over a single-mode status; and the two
expectation tests — a sidecar from another run and an overlay from another run each degrade to
`unknown (proof-write-failed)` and the degraded record is attributed to the run the READER
expected (pi `:186`).

`background/active_run_index.rs` (1) — an `observed` proof releases the marker on the next read
with no backdating at all, and the `pending` counter-rung in the same test releases nothing.

`registration/doctor.rs` (1) — the `release:` sentence names the proof before the pid fallback.
Pinned because it is a SENTENCE, not a computation: nothing else would notice it drifting away
from the ladder it describes.

`src/tests/rpc_bridge_integration.rs` (1) — a proof reaches the inter-extension bus through the
PRODUCTION `install_completion_watcher`, with two runs published and exactly one announced.

`cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs` (rewritten) — over a real detached
spawn: the three lines, the minted instance on both records and on the capacity owner, **and the
EXISTENCE on disk of the file the first line names**. The old negative assertion (*"the dump must
not name a sidecar this build never writes"*) is inverted into that existence check, which is the
same honesty rule pointing the other way.

### Gutting mutations — 11, each RED, each restored byte-for-byte

| # | gutted | test that went RED |
|---|---|---|
| M-D1 | `format_process_terminal` drops the reason and runner terms | `format_process_terminal_renders_upstreams_three_terms` |
| M-D2 | the dump stops naming the sidecar file (pi `:95`) | `the_dump_names_the_sidecar_and_prints_both_records` |
| M-D3 | the overlay is reported without pi `:53`'s run-id check | `an_overlay_from_another_run_degrades_instead_of_being_reported` |
| M-D4 | the sidecar is read with `ProofExpectation::none()` | `a_sidecar_from_another_run_degrades_instead_of_being_reported` |
| M-X1 | the index loses the observed-proof disjunct (age only) | `an_observed_proof_releases_the_marker_before_the_age_rung` |
| M-X2 | the index releases on ANY proof, not only `observed` | `an_observed_proof_releases_the_marker_before_the_age_rung` |
| M-R1 | the announcer is never registered in the composite | `a_process_terminal_proof_is_announced_on_the_inter_extension_bus` |
| M-R2 | the announcer publishes the launch's `pending` placeholder too | `a_process_terminal_proof_is_announced_on_the_inter_extension_bus` |
| M-C6 | the doctor sentence names the pid fallback before the proof | `the_capacity_block_states_the_proof_rung_before_the_pid_fallback` |
| M-I4 | the dump stops printing the capacity owner's instance (pi `:69`) | IT `debug_run_prints_the_real_sidecar_overlay_pair_and_capacity_slot` |
| M-I5 | the `debug.run` producer stops reading the pair | IT `debug_run_prints_the_real_sidecar_overlay_pair_and_capacity_slot` |

M-R1 is the production-reachability mutation for the emitter and M-I5 for the dump's reader:
neither test can pass without the production wiring it names.

### Gates

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` —
  **`Summary [  95.531s] 10742 tests run: 10742 passed, 9 skipped`** (step-2 baseline 10734 + 8).
- `cargo nextest run -p cyrup-it --features it` — **`Summary [ 300.597s] 574 tests run: 574 passed, 0 skipped`**
  (step-2 baseline 574; this step rewrote one IT test and added none).

### No residual is filed

Nothing was narrowed and no dependency was found missing. The one thing that looked like a
residual — the advertised-but-unpublished `subagent:process-terminal` topic — was not filed,
because it was built.
