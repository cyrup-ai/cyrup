# Active scope: the 22 code-review findings

`todo/` holds exactly the 22 tasks produced by the code review of branch
`claude/cyrup-tools-largest-file-rjs3yg` (findings vs merge base `4902cddf`).

Main's 92-task backlog arrived with the rebase and is parked in `todo/_backlog/`.
A non-recursive `todo/*.md` glob — which every flux command uses — does not see it,
so `/aug N`, `/exec N` and `/qa N` operate on these 22 and nothing else.

To restore the full queue:

    mv .flux/todo/_backlog/*.md .flux/todo/ && rmdir .flux/todo/_backlog

## The 22

| Priority | Task |
| --- | --- |
| HIGH | `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` |
| MEDIUM | `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md` |
| MEDIUM | `MEDIUM-deferred-config-write-loses-toggles-on-err-and-replays-snapshots.md` |
| MEDIUM | `MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md` |
| MEDIUM | `MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md` |
| MEDIUM | `MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md` |
| MEDIUM | `MEDIUM-proactiveskillsinput-doc-still-calls-the-dispatch-sync.md` |
| MEDIUM | `MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md` |
| MEDIUM | `MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md` |
| LOW | `LOW-awaitless-test-promoted-to-tokio-test-by-the-automation.md` |
| LOW | `LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md` |
| LOW | `LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md` |
| LOW | `LOW-guard-doc-points-at-a-select-loop-below-that-is-not-there.md` |
| LOW | `LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md` |
| LOW | `LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md` |
| LOW | `LOW-models-store-rationale-still-rests-on-a-sync-read-latest.md` |
| LOW | `LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md` |
| LOW | `LOW-public-api-changes-beyond-the-async-keyword.md` |
| LOW | `LOW-rewritten-concurrency-test-doc-misstates-which-lock-it-covers.md` |
| LOW | `LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md` |
| LOW | `LOW-sleepermarker-citation-still-points-at-ops-local-rs.md` |
| LOW | `LOW-spawn-blocking-join-error-reported-as-lock-contention.md` |

## Sequencing note

These are not uniform work.

- **`HIGH-dropped-acquire-future-detaches-blocking-flock-task`** and
  **`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait`** are the same
  question — what cancellation governs inside `FileLock::acquire`. Fixing either in
  isolation risks contradicting the other; they should be reasoned about together.
- **`MEDIUM-trustpromptfn-public-break-and-its-misstated-cost`** already carries a
  correction to its own first-proposed fix: taking the arguments by value does NOT
  remove the per-invocation clone, because the clone comes from the return type.
- Six tasks are one-line comment corrections and batch cheaply.
- **`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines`** warns against a
  workspace-wide `cargo fmt`. No crate outside `cyrup-tools` is rustfmt-clean at HEAD,
  so a blanket run reformats whole packages. Format only the touched files.

---

# Exec ordering (resolved 2026-08-23, after augmentation)

## Conflict resolved: HIGH vs MEDIUM-cancel-token

The two specs came back with **mutually exclusive designs** for `FileLock::acquire`, each
declaring the other subsumed. Resolved in favour of the **HIGH**: non-blocking `try_lock`
retried from async land. `MEDIUM-cancel-token` is **void by its own written terms** and
becomes a verification-only close; a resolution block is recorded at the top of that file.

Deciding evidence — tokio's own doc for `spawn_blocking`
(`tokio-1.52.3/src/task/blocking.rs:106-120`): *"runtime shutdown will wait indefinitely for
all started `spawn_blocking` to finish running."* The MEDIUM's design parks a pool thread
inside `flock(2)` when the caller goes away, so **process exit hangs** until a foreign peer
releases. A started `spawn_blocking` task cannot be aborted at all, so no guard placement
fixes it.

`CONFIG_LOCK_CONTENTION.md`'s anti-polling rule rested on *"JS has no better primitive.
Rust does."* False for `flock(2)`: no timeout variant, no epoll/`AsyncFd` readiness, no
io_uring lock opcode, no inotify on release. The rule binds everywhere it was true; not here.

## Hard ordering edges (violating these corrupts a later task's premise)

| Must run first | Then | Why |
| --- | --- | --- |
| `HIGH-dropped-acquire` | `MEDIUM-cancel-token` | Subsumption; the latter verifies and closes |
| `HIGH-dropped-acquire` | `LOW-spawn-blocking-join-error` | HIGH creates a **second** join site; the fix must cover both |
| `MEDIUM-map-field-doc` | `LOW-branch-rustfmt` | Its DoD #7 requires the two `cyrup-tools/src/lock.rs` fmt diffs to still exist |
| `LOW-keyed-lock-map-alias` | `LOW-mutationguard-alias` | Newtype the map first; the guard alias is the lesser leak |
| *everything else* | `LOW-branch-rustfmt` | It formats 7 files; any later edit re-dirties them. **Run it last.** |

## Mutual-exclusion sets (never co-run in the same batch of 2)

- **`cyrup-config/src/lock.rs`** — `HIGH-dropped-acquire`, `MEDIUM-cancel-token`,
  `MEDIUM-filelock-drop-order`, `LOW-config-lock-path-resolution`,
  `LOW-spawn-blocking-join-error`
- **`cyrup-tools/src/lock.rs` + `cyrup-core/src/keyed_lock.rs`** — `MEDIUM-map-field-doc`,
  `LOW-mutationguard-alias`, `LOW-keyedlocks-doc-clone`, `LOW-keyed-lock-map-alias`
- **`cyrup-ext-subagents/src/discovery/management.rs`** — `LOW-awaitless-test`,
  `MEDIUM-proactiveskillsinput-doc`

The two lock sets are in different crates and touch disjoint files, so pairing one from each
per batch is safe and keeps the queue moving.

## Scope escalations found during augmentation

- `MEDIUM-deferred-config-write` overturned the smaller fix: `panic = "abort"`
  (`Cargo.toml:284`) makes `catch_unwind` buffer protection inert in every shipped build.
  Prescribes `run_startup_selector` → `impl AsyncFnMut` across six call sites. **Accepted.**
  Adopting the crate's existing `crossterm_input_stream` for the pre-launch read was evaluated
  on a challenge and **excluded for cause**, not for size. That reader thread carries the
  TUI-092 wedge watchdog: `Escalation::on_press` hard-exits the process (`exit(130)`) after
  three Ctrl+C/Ctrl+D chords arrive while `INPUT_SERVICED` has not moved, and `INPUT_SERVICED`
  is bumped only by `App::run`'s input arm, which a pre-launch modal does not have. Since
  `app.session.delete` is **Ctrl+D** (`keymap.rs:1061`), deleting three sessions from the
  `--resume` picker at human pace would kill the process. It also drops keystrokes: the reader
  polls at 100 ms and a key pressed while the thread sits in `event::read()` on unmount is
  consumed and lost. The blocking read stays, but is now a **required doc comment** rather than
  an undocumented surprise. A scoped follow-up is named in that file.

  The concern that prompted the challenge — a blocking read starving the runtime inside
  `SessionBuilder::build` — does not hold here: verified zero production `tokio::spawn` on the
  path from process start to `builder.rs:684` (none before `main.rs:810`, none in `builder.rs`
  or `factory.rs`), and `spawn_abort_on_signal` runs at `main.rs:804`, after `build()` returns.
  The prompt parks a worker with nothing else to run, and does so identically today.
- `LOW-models-store-rationale` was re-rated **MEDIUM** by its augmentation: not a stale
  comment but a measured behavioural gap (N readers each doing a full parse where the merge
  base coalesced to 1). Filename keeps its `LOW-` prefix.

## Late-augmentation edges (found after the table above was written)

`LOW-keyed-lock-map-alias` newtypes `KeyedLockMap`, which invalidates prescriptions in two
sibling specs. It supplied compatible replacements for both; exec must use them, not the
originals:

| Sibling | What breaks | Fix |
| --- | --- | --- |
| `LOW-keyedlocks-doc-clone` | Its prescribed `Self { map: Arc::clone(&self.map) }` **will not compile** against the newtype | Use the body given in `LOW-keyed-lock-map-alias` |
| `MEDIUM-map-field-doc` (already `done`) | Its comment clause about handing out "the raw `DashMap`" becomes **false** | Apply that task's conditional one-clause replacement |

This makes `LOW-keyed-lock-map-alias` the **head** of the `cyrup-tools`/`keyed_lock` chain,
already required by the earlier edge against `LOW-mutationguard-alias`.

`LOW-public-api-changes` resolves to close-as-recorded plus one rustdoc hunk, and settles a
question `MEDIUM-with-lock` delegated to it: `SettingsStore::read` **stays sync** (it takes no
lock in either impl, so there is no suspension point; async would cascade to eight sync
`load` call sites; upstream is sync on both halves).

## Rebase onto main (2026-08-23)

Rebased onto `origin/main` `353bb93`, which carried four new commits including a
decomposition of `crates/cyrup/src/main.rs` (2402 deletions) mirroring our own
`ops/local.rs` split. That commit moved all three of our `main.rs` edit sites into new
modules, so the single conflict was resolved by taking main's decomposition and
re-applying our async propagation where the code now lives:

| our edit | now applied in |
| --- | --- |
| `run_first_time_setup(..).await?` | `bootstrap.rs:163`, and `maybe_run_first_time_setup` became `pub async fn` (its caller `main.rs:278` is already inside `async fn run()`) |
| `trust_prompt_callback` boxed-future form | `prelaunch.rs:235` |
| `persist_setting(..).await` | `interactive.rs:194` |

`main` also added six `MCP_*` task files to `.flux/todo/`. They are **main's work, not part
of the 22**, and have been parked in `_backlog/` alongside the other 92 so the active queue
stays at exactly 22. Restore them the same way as the rest of the backlog.

### Spec citation drift from the rebase

All 22 specs were augmented against the pre-rebase tree. `main`'s four commits touched files
13 specs cite, so exec must treat line numbers as hints and match on verbatim text — which
most specs already instruct. Two needed real repair, done:

- **`MEDIUM-trustpromptfn-public-break`** cited `main.rs:1529-1547` and `:1536` four times.
  `trust_prompt_callback` now lives at `prelaunch.rs:229-247`, clone at `:236`. Retargeted.
- **`LOW-branch-rustfmt`** derived its "7 files / 37 hunks" set by running `rustfmt --check`
  against the old tree, and `main.rs`, `management.rs` and `builder.rs` have all changed
  since. **Re-run the derivation script embedded in that spec before acting on the file
  list** — do not trust the enumerated 7.
