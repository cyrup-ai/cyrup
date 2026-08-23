---
title: Models Store Rationale Still Rests On A Sync Read Latest
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:44
---

# The omitted reload coalescer is now a real gap, not a stale comment

## Verdict — priority raised LOW → MEDIUM

The intake filed this as two stale rationales with "no behavioural defect". **That is wrong for the
first one.** The comment at
[`models_store.rs:145-146`](../../crates/cyrup-config/src/models_store.rs) is the stated reason an
upstream mechanism was deliberately omitted, and the async conversion did not merely falsify its
premise — it opened exactly the behaviour the omitted mechanism prevents.

Measured below: a concurrent fan-out of N readers over one `FileModelsStore` performed **1** full
read-and-parse of `models-store.json` at the merge base and performs **N** on this branch. The
regression is deterministic, not a race — every caller in the fan-out reloads, every time.

The second rationale ([`:288-292`](../../crates/cyrup-config/src/models_store.rs)) really is only a
stale comment, and this task fixes it in the same pass because it is four lines away.

The filename keeps its `LOW-` prefix; the frontmatter `priority` is the authority. Do not rename it
as part of this change.

> Line numbers throughout are as of this branch's `crates/cyrup-config/src/models_store.rs`
> (773 lines). Other queued tasks edit the same file — see [Interaction with the other queued
> tasks](#interaction-with-the-other-queued-tasks) — so re-anchor on the quoted text, not the number.

## Problem

### What the comment claims

[`models_store.rs:145-146`](../../crates/cyrup-config/src/models_store.rs):

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1), minus the in-flight-reload coalescer:
/// cyrup's reader is synchronous file I/O under a `FileLock`, so there is no promise to share.
```

The argument has two halves, and both mattered:

1. *There is no promise to share* — a Rust `fn` returning a value has no future object a second
   caller could join. Structurally true at the merge base.
2. *Therefore nothing is lost by omitting the coalescer* — the implied conclusion, and the only
   reason the omission needed no follow-up.

Half 1 is what this branch deleted. Half 2 was never independently true; it rode on half 1.

### The premise at the merge base

At `4902cddf`, `read_latest` was a plain `fn` and `FileLock::acquire` was fully synchronous —
`open` plus a blocking `FileExt::lock`, no in-process layer, no `spawn_blocking`
(`git show 4902cddf:crates/cyrup-config/src/lock.rs`, lines 18-33):

```rust
fn read_latest(&self) -> Result<OrderedObject, crate::error::ConfigError> {
    let revision = file_revision(&self.path);
    if revision.is_some() && /* … */ revision == state.revision { return Ok(state.data.clone()); }
    let _guard = crate::lock::FileLock::acquire(&self.path)?;
    let data = self.read_all();
    self.update_read_state(&data, file_revision(&self.path));
    Ok(data)
}
```

The whole interval from *"the revision check missed"* to *"the fresh revision is stamped"* was a
straight-line synchronous region inside one poll. A task that entered it ran it to completion before
the executor could poll anything else on that thread. That is the real, unstated content of the
comment: not "there is no promise", but **"the reload window is not observable"**.

### What the branch changed

[`models_store.rs:234-254`](../../crates/cyrup-config/src/models_store.rs) is now `async fn`, and
[`FileLock::acquire`](../../crates/cyrup-config/src/lock.rs) at `lock.rs:57-69` awaits twice — the
per-path keyed async mutex, then `tokio::task::spawn_blocking`. `spawn_blocking(..).await` is an
unconditional yield: the `JoinHandle` is never `Ready` on its first poll.

So the reload window now **contains a yield point**, and three properties of the surrounding code
turn that into duplicated work:

- The revision short-circuit runs **before** the lock is taken (`:239-244`).
- It is **never re-evaluated after** the lock is acquired (`:248-252`).
- `update_read_state` — the only thing that publishes the fresh revision — runs **after** the lock
  is held and the file re-read (`:252`).

Therefore any task polled inside the window observes the *stale* revision, misses the short-circuit,
and is irrevocably committed to its own full `read_to_string` + `serde_json::from_str` of the whole
file, which it performs after queuing on layer 1 of the lock.

**The in-process `FileLock` layer does not close the window** — it is the thing that makes the
duplicated work *serial* rather than parallel. The suggestion in the intake that it "may already
close it" is checked and refuted: the short-circuit is strictly before `FileLock::acquire`, so by
the time layer 1 admits the second task the decision to reload is already made and there is nothing
left that would reconsider it.

### The caller that makes it bite

[`RemoteCatalog::refresh_providers`](../../crates/cyrup-provider/src/remote_catalog.rs) (`:521-538`)
fans out over every configured provider with `futures::future::join_all`, and each
`refresh_once` (`:592-598`) opens with `self.store.read(provider_id, None).await` against the **same**
`FileModelsStore`:

```rust
let futures = provider_ids.iter().map(|id| { /* … */ this.refresh_provider(&id, options).await /* … */ });
futures::future::join_all(futures).await
```

Nothing on that path breaks the fan-out into separate tasks:
[`RefreshDedup::run`](../../crates/cyrup-provider/src/utils/refresh.rs) (`:93-146`) memoises
**per provider id**, so N distinct providers get N distinct memos, and it polls its `Shared` future
inline (`shared.await`) rather than spawning. All N `read()` calls are therefore polled in order by
one task, and all N reach `read_latest` before any of them stamps a revision.

Two entry points hit it:

- [`spawn_model_catalog_refresh_with`](../../crates/cyrup/src/provider.rs) (`:172-198`) — the
  background refresh on every interactive/rpc start.
- [`refresh_model_catalogs_with`](../../crates/cyrup/src/provider.rs) (`:249-283`) —
  `cyrup update --models`, inside a 15s total budget.

N is the number of credential-resolvable providers. It is also amplified in steady state: every
branch of `refresh_once` ends in a `store.write(..)` (`:618-708`), and `write` calls
`update_read_state(&all, None)` (`:328`), so the revision is `None` again and the *next* fan-out
misses from a clean slate too.

`load_overlay` (`:482-496`) is a sequential `for` loop, so it is unaffected: its first iteration
stamps and the remaining 30 short-circuit. The defect is specific to the concurrent path.

## Measured evidence

A standalone reproduction (`tmp/exp`, tokio 1.53, edition 2024) models the three shapes exactly:
the pre-lock revision check against a constant file revision, layer 1 as a `tokio::sync::Mutex`,
layer 2 as a `spawn_blocking` around a std mutex, a counter in `read_all`, and the drivers wrapped
in `join_all` the way `refresh_providers` wraps them.

```
single worker thread    workers=1  N=8   merge-base(sync)=1  branch(async)=8   with-double-check=1
multi-thread            workers=4  N=8   merge-base(sync)=1  branch(async)=8   with-double-check=1
multi-thread            workers=8  N=31  merge-base(sync)=1  branch(async)=31  with-double-check=1
multi-thread            workers=8  N=3   merge-base(sync)=1  branch(async)=3   with-double-check=1
```

Three things this settles:

1. **The merge base really did coalesce**, and did so for a reason that survives extra worker
   threads: `join_all` polls its children inline in one task, so the first child's synchronous
   `read_latest` completes and stamps before the second child is ever polled. `1`, not "usually 1".
2. **The branch really does not.** The count is exactly N at every width — a deterministic
   regression, not a timing window that a fast machine might miss.
3. **A revision re-check under the lock restores it**, without a coalescer and without touching
   `lock.rs`.

The cost per redundant reload is a full `read_to_string` plus a `serde_json::from_str` of the entire
provider catalog, plus a `spawn_blocking` hop and an `flock`/`unlock` pair. The `flock` half also
leaks outward: the cross-process lock is taken and released N times in sequence, so a peer
`cyrup update --models` in another terminal waits out N serialized reloads instead of one.

## Required change — one file, four hunks

`./crates/cyrup-config/src/models_store.rs`. **Nothing else.** Do not touch `lock.rs`,
`keyed_lock.rs` or `remote_catalog.rs`.

### Hunk 1 — extract the short-circuit so it can be used twice (new fn, after `read_all` at `:231`)

Insert between the closing brace of `read_all` (`:231`) and the `/// `readLatest`` doc comment
(`:233`):

```rust
    /// The snapshot when the file has not moved since it was stamped — pi's
    /// `getFileRevision(this.path) === readState.revision` (models-store.ts:86-87 @v0.84.1).
    ///
    /// `None` means "reload", and covers all three of upstream's misses: the file cannot be stat'd,
    /// the revision differs, or no revision has been stamped yet (a fresh store, or a `write`/
    /// `delete` that cleared it). A poisoned `read_state` also reloads rather than propagating.
    fn current_snapshot(&self) -> Option<OrderedObject> {
        let revision = file_revision(&self.path)?;
        let state = self.read_state.read().ok()?;
        (state.revision.as_deref() == Some(revision.as_str())).then(|| state.data.clone())
    }
```

This is the existing predicate verbatim in meaning: `file_revision(..)?` is the old
`revision.is_some()` guard, `.read().ok()?` is the old `let Ok(state) = ..` arm falling through to
the reload, and the equality is the old `revision == state.revision`. Keep `then` — not `then_some`
— the `clone` must stay lazy.

### Hunk 2 — `read_latest` (`:233-254`)

**Find (verbatim, the whole fn including its doc comment):**

```rust
    /// `readLatest` (models-store.ts:81-108 @v0.84.1): answer from the snapshot when the file's
    /// revision is unchanged, otherwise reload under the cross-process lock and re-stamp it.
    async fn read_latest(
        &self,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<OrderedObject, crate::error::ConfigError> {
        let revision = file_revision(&self.path);
        if revision.is_some()
            && let Ok(state) = self.read_state.read()
            && revision == state.revision
        {
            return Ok(state.data.clone());
        }
        // The lock is advisory and cross-process: a concurrent `cyrup update --models` in another
        // terminal must not be observed mid-rename. A lock we could not take is not a degraded
        // read — it is an unserialized one, so it reaches the caller rather than being `.ok()`-ed.
        let _guard =
            crate::lock::FileLock::acquire(&self.path, options.and_then(|o| o.signal.as_ref()))
                .await?;
        let data = self.read_all();
        self.update_read_state(&data, file_revision(&self.path));
        Ok(data)
    }
```

**Replace with:**

```rust
    /// `readLatest` (models-store.ts:81-108 @v0.84.1): answer from the snapshot when the file's
    /// revision is unchanged, otherwise reload under the cross-process lock and re-stamp it.
    ///
    /// Checked TWICE, and the second check is load-bearing rather than defensive — see the comment
    /// at the re-check below.
    async fn read_latest(
        &self,
        options: Option<&ModelsStoreOperationOptions>,
    ) -> Result<OrderedObject, crate::error::ConfigError> {
        if let Some(data) = self.current_snapshot() {
            return Ok(data);
        }
        // The lock is advisory and cross-process: a concurrent `cyrup update --models` in another
        // terminal must not be observed mid-rename. A lock we could not take is not a degraded
        // read — it is an unserialized one, so it reaches the caller rather than being `.ok()`-ed.
        let _guard =
            crate::lock::FileLock::acquire(&self.path, options.and_then(|o| o.signal.as_ref()))
                .await?;
        // Re-check under the lock. This stands in for pi's in-flight-reload coalescer
        // (models-store.ts:15-19), and it is not an optimisation: `acquire` awaits, so every other
        // reader polled between the check above and this line saw the OLD revision and queued
        // behind us. Whoever reached here first has already re-read the file and stamped its
        // revision, so re-parsing it now would be pure duplicated work — one full
        // `read_to_string` + `from_str` of the whole catalog per waiter. Upstream shares the
        // reload's promise; a future borrowing `&self` cannot be handed out, so the equivalent
        // here is to observe the result the winner published. `refresh_providers`
        // (`cyrup-provider/src/remote_catalog.rs:521-538`) fans out one `read` per configured
        // provider through `join_all`, which is exactly this case.
        if let Some(data) = self.current_snapshot() {
            return Ok(data);
        }
        let data = self.read_all();
        self.update_read_state(&data, file_revision(&self.path));
        Ok(data)
    }
```

### Hunk 3 — the `ModelsFileReadState` doc (`:145-146`)

**Find (verbatim, 2 lines):**

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1), minus the in-flight-reload coalescer:
/// cyrup's reader is synchronous file I/O under a `FileLock`, so there is no promise to share.
```

**Replace with (5 lines):**

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1). Upstream additionally memoises the
/// in-flight reload so concurrent `readLatest` callers await one promise. A future borrowing
/// `&self` cannot be handed out, so [`FileModelsStore::read_latest`] reaches the same property from
/// the other end: it re-checks this revision AFTER taking the lock, and a caller that queued behind
/// a reload returns the snapshot that reload stamped rather than re-parsing the file itself.
```

### Hunk 4 — the second abort check's rationale (`:288-292`)

**Find (verbatim, 5 lines):**

```rust
        // … and once after it returns, before the entry is handed back (`:121`). Upstream's second
        // check catches an abort raised while the shared reload was in flight; here `read_latest`
        // is synchronous, so it catches an abort raised while this task was descheduled across the
        // `async fn`'s poll. Both are the same guarantee: nothing is returned to a caller that has
        // already given up.
```

**Replace with (5 lines):**

```rust
        // … and once after it returns, before the entry is handed back (`:121`). Upstream's second
        // check catches an abort raised while the shared reload was in flight, and so does this
        // one: `read_latest` awaits `FileLock::acquire`, so the reload is genuinely in flight
        // across the two checks and a caller can give up inside it. Nothing is returned to a
        // caller that has already given up.
```

The code is already correct here and was correct before; only the justification was understated.
This is the "purely stale comment" half of the original finding — kept in this task because it is
four lines from hunk 2 and shares its premise.

## Why this shape and not the others

- **A true in-flight coalescer** (`futures::future::Shared` over the reload). Cannot be written
  without restructuring ownership: `ModelsStore::read` takes `&self`, `FileModelsStore` is held as
  `Arc<dyn ModelsStore>`, and `Shared<BoxFuture<'static, _>>` needs a `'static` future — there is no
  `Arc<Self>` in scope to build one from. It would also have to memoise a `Result` whose error is
  not `Clone`, i.e. the whole `SharedOut`/`MemoClear`/generation apparatus that
  [`RefreshDedup`](../../crates/cyrup-provider/src/utils/refresh.rs) already needed for the
  network refresh. Sixty lines of machinery to save what the re-check saves in three.
- **Re-checking between layer 1 and layer 2** of `FileLock`, so a waiter skips the `spawn_blocking`
  and the `flock` too. Strictly better on paper, but it requires a new `FileLock` API and puts
  models-store policy inside the lock primitive. The saving is a syscall pair against a whole-file
  JSON parse; not worth the coupling, and `lock.rs` is being rewritten by
  [`HIGH-dropped-acquire-future-detaches-blocking-flock-task`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md).
- **A second models-store-private async gate in front of `FileLock::acquire`.** Nests a lock inside
  a lock keyed on the same path, for no gain over the re-check.
- **Reverting `read_latest` to `fn`.** Not available: `FileLock::acquire` is async on this branch
  because [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
  and the `HIGH` task above both depend on it being so.
- **Comment-only fix (the original LOW filing).** Rejected on the evidence above: it would document
  an O(N) regression as intentional.

## Interaction with the other queued tasks

Three other queued tasks touch `models_store.rs`. All four sets of hunks are disjoint, but they
shift each other's line numbers — apply by matching the quoted text.

| Task | Its hunks in this file | Overlap |
| --- | --- | --- |
| [`MEDIUM-lock-cancellation-reported-as-model-source-not-aborted`](./MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md) | module doc `:20-24`, `store_err` `:193-196` | none |
| [`LOW-spawn-blocking-join-error-reported-as-lock-contention`](./LOW-spawn-blocking-join-error-reported-as-lock-contention.md) | `lock.rs` only | none |
| [`LOW-public-api-changes-beyond-the-async-keyword`](./LOW-public-api-changes-beyond-the-async-keyword.md) | signatures | none — `read_latest` and `current_snapshot` are both private |

One cross-reference to fix up if the `HIGH` task lands first: its new `acquire` rustdoc cites
"`models_store.rs:293` re-checks after the read". Hunks 1-3 add 27 lines above that point, so the
second `throw_if_aborted` moves from `:293` to `:320` (measured on the dry-run apply behind DoD item 6).
Whichever task lands second updates the number; do not change the other file's prose beyond the
digits.

## Out of scope

- **`lock.rs`, `keyed_lock.rs`.** No change. The re-check lives entirely in the models-store layer.
- **`remote_catalog.rs`.** `refresh_providers`' fan-out is correct and is the caller this fix
  serves; do not serialize it, and do not add a store-level memo there.
- **The `read_all`-then-`file_revision` ordering** at the end of `read_latest`. A non-cooperating
  writer between those two calls stamps a revision that does not describe the data — pre-existing,
  present at the merge base, unrelated to the async conversion, and mitigated by the same advisory
  lock every cyrup writer takes. Do not widen this task to cover it.
- **`write` / `delete`.** Their `update_read_state(&all, None)` is upstream's two-argument call and
  stays exactly as it is. They must keep reloading unconditionally.
- **`load_overlay`'s sequential loop.** Already coalesces by construction.
- **The filename's `LOW-` prefix.** Frontmatter carries the priority; do not `git mv`.

## Definition of Done

1. `crates/cyrup-config/src/models_store.rs` contains all four hunks byte-for-byte. No other file in
   the repository is modified.
2. `grep -n 'is synchronous' crates/cyrup-config/src/models_store.rs` returns nothing.
3. `grep -c 'current_snapshot' crates/cyrup-config/src/models_store.rs` returns `3` — one
   definition, two call sites in `read_latest`.
4. `read_latest` contains exactly one `FileLock::acquire`, and the `if let Some(data) =
   self.current_snapshot()` that follows it appears **after** the `let _guard` binding and
   **before** `self.read_all()`. Getting this order wrong is the whole defect.
5. The old `let revision = file_revision(&self.path);` / `revision.is_some() && let Ok(state) = ..`
   let-chain no longer appears in `read_latest`; `file_revision` is still called once at the end of
   the reload branch, for the stamp.
6. `git diff --stat` shows **one file changed, 39 insertions, 12 deletions** — the exact figures
   from a dry-run apply of all four hunks against the current file. Every changed line is inside
   `read_latest`, the new `current_snapshot`, the `ModelsFileReadState` doc, or the abort comment.
7. `rustfmt --edition 2024` is a no-op on the result. The file is rustfmt-clean before the change
   and the hunks above keep it so (verified on the dry-run apply); no added line exceeds 100
   columns, and the file's five pre-existing over-100 comment lines are untouched.
8. Behaviour of the four existing revision tests is unchanged, by inspection of the new path:
   - `read_answers_from_the_snapshot_until_the_file_revision_changes` (`:427`) — the planted-snapshot
     read hits the FIRST check (revision untouched); the post-rewrite read misses both (`state.revision`
     is the old stamp, the file's is new) and reloads to `"v2"`.
   - `a_deleted_file_reloads_rather_than_serving_the_stale_snapshot` (`:500`) — `file_revision`
     is `None`, so `current_snapshot` is `None` at both checks and the reload always wins.
   - `a_missing_or_corrupt_file_reads_as_no_overlay_and_never_errors` (`:639`) — same, plus
     `read_all`'s `unwrap_or_default`.
   - `cfg042_an_aborted_signal_is_refused_before_the_file_is_touched` (`:736`) — unaffected; the
     first `throw_if_aborted` still precedes `read_latest`.
9. The regression itself is gone: N concurrent `read()` calls on one `FileModelsStore`, driven by
   `join_all` with no intervening write, perform **one** `read_all`, matching the merge base and the
   `with-double-check` column in the table above.
