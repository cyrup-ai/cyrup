---
title: Models Store Rationale Still Rests On A Sync Read Latest
priority: MEDIUM
stage: qa
status: completed
updated: 2026-08-23 08:34
---

# The omitted reload coalescer is a real gap, not a stale comment

## 0. Citation audit — this pass re-verified every pointer the previous revision carried

Three earlier passes edited `crates/cyrup-config/src/lock.rs` and
`crates/cyrup-config/src/models_store.rs`, and main separately decomposed `crates/cyrup/src/main.rs`
into `bootstrap.rs` / `prelaunch.rs` / `interactive.rs` / `actions.rs` / `session_launch.rs` /
`predispatch.rs`. Every `file:line` below was re-read against the working tree on
**2026-08-23 06:51**. `models_store.rs` is now **791** lines, not 773.

**Wherever a pointer can be a name, this revision uses the name.** The surviving numbers are only
those a reader cannot resolve otherwise, and each one is re-derived below.

| Previous revision said | Verdict | Corrected to |
| --- | --- | --- |
| `models_store.rs` is 773 lines | **STALE** | **791** lines |
| `models_store.rs:145-146` — the `ModelsFileReadState` doc | **STALE** | `:148-149`; anchored by name as **the doc comment on `struct ModelsFileReadState`** |
| `models_store.rs:234-254` — `read_latest` | **STALE** | `:250-272`; anchored by name as **`FileModelsStore::read_latest`** |
| `models_store.rs:239-244` / `:248-252` / `:252` — parts of `read_latest` | **STALE** | dropped; the whole fn is quoted verbatim in Hunk B instead |
| `models_store.rs:231` — end of `read_all` | **STALE** | `:248`; anchored as **the closing brace of `FileModelsStore::read_all`** |
| `models_store.rs:288-292` — the second abort check's rationale | **STALE** | `:306-310`, inside **`impl ModelsStore for FileModelsStore`'s `read`** |
| `models_store.rs:328` — `write`'s `update_read_state(&all, None)` | **STALE** | `:345` (and `delete`'s at `:363`); anchored by method name |
| `models_store.rs:427` / `:500` / `:639` / `:736` — the four tests | **ALL STALE** | `:445`, `:518`, `:657`, `:754`; anchored by test-fn name below |
| `lock.rs:57-69` — `FileLock::acquire` awaits twice | **STALE** | `acquire` is `:103-150`; the two awaits are the `CONFIG_LOCK_HANDLE.guard(..).await` at `:106-108` and the `spawn_blocking(..).await` at `:116-117` |
| `remote_catalog.rs:521-538` — `refresh_providers` | **STALE** | `:522-540`; anchored as **`RemoteCatalog::refresh_providers`** |
| `remote_catalog.rs:592-598` — `refresh_once`'s store read | **STALE** | `refresh_once` is `:555`; its `self.store.read(provider_id, None).await` is `:564` |
| `remote_catalog.rs:618-708` — `refresh_once`'s writes | **STALE** | four `self.store.write(..)` calls at `:623`, `:640`, `:660`, `:701` |
| `remote_catalog.rs:482-496` — `load_overlay` | **NEARLY** | `:482-498`; anchored as **`RemoteCatalog::load_overlay`** |
| `refresh.rs:93-146` — `RefreshDedup::run` | **CORRECT** | kept, anchored as **`RefreshDedup::run`** |
| `cyrup/src/provider.rs:172-198` — `spawn_model_catalog_refresh_with` | **NEARLY** | `:172-199`; anchored by name |
| `cyrup/src/provider.rs:249-283` — `refresh_model_catalogs_with` | **NEARLY** | `:249-286`; anchored by name |
| `git show 4902cddf:crates/cyrup-config/src/lock.rs` as the merge-base evidence | **UNUSABLE** | no git command may be run for this task; §2 now derives the same conclusion from the tree as it stands |
| the `tmp/exp` reproduction and its measured table | **GONE** — `tmp/exp` no longer exists | replaced by §3, a derivation checkable by reading `futures-util` 0.3.32 and the three cyrup functions |
| interaction table: `MEDIUM-lock-cancellation-…` edits `models_store.rs` `:20-24` / `:193-196` | **LANDED, NOT QUEUED** | it is in `.flux/done/2026-08-23-00-08/`; its module doc and `store_err` (`:194-214`) are already in the tree |
| interaction table: `LOW-public-api-changes-…` edits signatures in this file | **WRONG** | its single required change is in `crates/cyrup-config/src/settings/store.rs`. It does not touch `models_store.rs` |
| `HIGH-dropped-acquire-future-detaches-blocking-flock-task` is queued and will cite `models_store.rs:293` | **LANDED** | it is in `.flux/done/2026-08-23-00-08/`. The live citation it left is `lock.rs:97`, which reads `models_store.rs:311` and is **correct today** — see §6 |
| pi citations `models-store.ts:15-19 / 81-108 / 86-87 / 121 @v0.84.1` | **NOT RE-VERIFIABLE HERE** | the local checkout `tmp/pi` is **v0.83.0** (45-line `models-store.ts`, no `readLatest`, no `ModelsFileReadState`). Every hunk below therefore **carries the existing in-tree citations through unchanged and introduces no new upstream line number** |

## 1. Verdict — priority raised LOW → MEDIUM

The intake filed this as two stale rationales with "no behavioural defect". **That is wrong for the
first one.** The doc comment on `struct ModelsFileReadState` is the stated reason an upstream
mechanism was deliberately omitted, and the async conversion did not merely falsify its premise — it
opened exactly the behaviour the omitted mechanism prevents.

A concurrent fan-out of N readers over one `FileModelsStore` performs **N** full read-and-parses of
`models-store.json` where a synchronous `read_latest` performed **1**. The regression is
deterministic, not a race: every caller in the fan-out reloads, every time. §3 derives this from the
code rather than asserting it.

The second rationale (the comment after `read_latest` returns, inside `read`) really is only a stale
comment, and this task fixes it in the same pass because it is four lines away and shares the
premise.

The filename keeps its `LOW-` prefix; the frontmatter `priority` is the authority. Do not rename the
file.

## 2. Problem

### 2.1 What the comment claims

`crates/cyrup-config/src/models_store.rs:148-149`, the doc comment on `struct ModelsFileReadState`:

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1), minus the in-flight-reload coalescer:
/// cyrup's reader is synchronous file I/O under a `FileLock`, so there is no promise to share.
```

The argument has two halves, and both mattered:

1. *There is no promise to share* — a Rust `fn` returning a value has no future object a second
   caller could join.
2. *Therefore nothing is lost by omitting the coalescer* — the implied conclusion, and the only
   reason the omission needed no follow-up.

Half 1 is what the async conversion deleted. Half 2 was never independently true; it rode on half 1.

### 2.2 The premise the comment was written against

The comment describes a `read_latest` that was a plain `fn` over a fully synchronous
`FileLock::acquire`. Under that shape the whole interval from *"the revision check missed"* to
*"the fresh revision is stamped"* was a straight-line synchronous region inside one poll: a task that
entered it ran it to completion before the executor could poll anything else on that thread. That is
the real, unstated content of the comment — not "there is no promise", but **"the reload window is
not observable"**.

> No git command is used to establish this. It is not needed: the claim under test is about the
> shape the comment *describes* ("cyrup's reader is synchronous file I/O"), and the tree refutes that
> description on its own — `read_latest` is `async fn` and `FileLock::acquire` is `pub async fn`.

### 2.3 What the tree does now

`FileModelsStore::read_latest` is an `async fn`, and `crate::lock::FileLock::acquire`
(`lock.rs:103-150`) awaits twice before it returns a guard:

- `CONFIG_LOCK_HANDLE.guard(lock_path.clone(), token).await` (`lock.rs:106-108`) — the per-path keyed
  async mutex, layer 1.
- `tokio::task::spawn_blocking(move || open_and_try_lock(..)).await` (`lock.rs:116`) — layer 2.
  `spawn_blocking(..).await` is an unconditional yield: the `JoinHandle` is never `Ready` on its
  first poll.

So the reload window now **contains a yield point**, and three properties of `read_latest` turn that
into duplicated work:

- The revision short-circuit runs **before** the lock is taken.
- It is **never re-evaluated after** the lock is acquired.
- `update_read_state` — the only thing that publishes the fresh revision — runs **after** the lock is
  held and the file re-read.

Therefore any task polled inside the window observes the *stale* revision, misses the short-circuit,
and is irrevocably committed to its own full `read_to_string` + `serde_json::from_str` of the whole
file, which it performs after queuing on layer 1 of the lock.

**The in-process `FileLock` layer does not close the window** — it is the thing that makes the
duplicated work *serial* rather than parallel. The intake's suggestion that it "may already close it"
is refuted: the short-circuit is strictly before `FileLock::acquire`, so by the time layer 1 admits
the second task the decision to reload is already made and nothing left reconsiders it.

### 2.4 The caller that makes it bite

`RemoteCatalog::refresh_providers` (`crates/cyrup-provider/src/remote_catalog.rs:522`) fans out over
every configured provider with `futures::future::join_all` (`:535`), and each `refresh_once` (`:555`)
opens with `self.store.read(provider_id, None).await` (`:564`) against the **same**
`FileModelsStore` — `RemoteCatalog` holds one `store: Arc<dyn ModelsStore>` (`:420`).

Nothing on that path breaks the fan-out into separate tasks: `RefreshDedup::run`
(`crates/cyrup-provider/src/utils/refresh.rs:93`) memoises **per provider id** — the memo is reached
through `RemoteCatalog::dedup_for(provider_id)` (`remote_catalog.rs:542`), so N distinct providers get
N distinct memos — and it polls its `Shared` future inline (`shared.await`, `refresh.rs:106` and
`:146`) rather than spawning. All N `read()` calls are therefore polled in order by one task, and all
N reach `read_latest` before any of them stamps a revision.

Two entry points hit it:

- `spawn_model_catalog_refresh_with` (`crates/cyrup/src/provider.rs:172`) — the background refresh on
  every interactive/rpc start. It makes **one** `tokio::spawn`, and the whole `join_all` fan-out runs
  inside that single task.
- `refresh_model_catalogs_with` (`crates/cyrup/src/provider.rs:249`) — `cyrup update --models`,
  inside the 15 s `MODELS_REFRESH_TIMEOUT` budget.

N is the number of credential-resolvable providers. It is also amplified in steady state: each of the
four terminal branches of `refresh_once` ends in `self.store.write(..)` (`remote_catalog.rs:623`,
`:640`, `:660`, `:701`), and `FileModelsStore::write` calls `self.update_read_state(&all, None)`
(`models_store.rs:345`; `delete`'s is `:363`), so the revision is `None` again and the *next* fan-out
misses from a clean slate too.

`RemoteCatalog::load_overlay` (`remote_catalog.rs:482-498`) is a sequential `for` loop, so it is
unaffected: its first iteration stamps and the rest short-circuit. The defect is specific to the
concurrent path.

## 3. Why the count is exactly N, and exactly 1 after the fix

The previous revision cited a `tmp/exp` reproduction. **That directory no longer exists**, so this
revision replaces the measurement with a derivation every step of which is checkable by reading
source that is on this machine.

**Step 1 — `join_all` never spawns; it polls its children inline, in order.** `futures` is pinned to
**0.3.32** in `Cargo.lock`, and
`~/.cargo/registry/src/*/futures-util-0.3.32/src/future/join_all.rs` has two shapes, chosen at
`:121-124` by `SMALL = 30` (`:35`): `JoinAllKind::Small` polls `for elem in iter_pin_mut(elems)` on
every poll (`:139-145`), and `JoinAllKind::Big` delegates to a `FuturesOrdered` (`:157`). Neither
spawns. One task therefore drives all N children.

**Step 2 — every child's first poll reaches `read_latest`'s check and then suspends before stamping.**
Child *k*'s first poll runs `refresh_provider` → `RefreshDedup::run` → `refresh_once` →
`store.read(..)` → `read_latest`; the revision check misses (the store was just written, or nothing
has stamped yet); it reaches `FileLock::acquire`, whose `spawn_blocking(..).await` is an unconditional
suspension. Control returns to `JoinAll::poll`, which polls child *k+1*. So all N children observe the
same stale revision before any of them stamps one. Hence **N** `read_all` calls — deterministic at
every worker count, because the ordering is a property of `JoinAll::poll`, not of the scheduler.

**Step 3 — the re-check collapses it to 1.** With the second check in place, the winner acquires the
lock, calls `read_all`, and calls `update_read_state(&data, file_revision(&self.path))` **while still
holding `_guard`** — the guard is a `let _guard` binding dropped at the end of the function body,
after the stamp. Every other child is queued on layer 1 of that same guard, so when it is admitted the
fresh revision is already published; its re-check hits and it returns the winner's snapshot without
touching the file. **1** `read_all`, matching the synchronous shape the comment described.

The cost this removes, per redundant reload: a full `read_to_string` plus a `serde_json::from_str` of
the entire provider catalog, plus a `spawn_blocking` hop and an `flock`/`unlock` pair. The `flock` half
also leaks outward — the cross-process lock is taken and released N times in sequence, so a peer
`cyrup update --models` in another terminal waits out N serialized reloads instead of one.

## 4. Required change — one file, three hunks

`crates/cyrup-config/src/models_store.rs`. **Nothing else in the repository is modified.** Do not
touch `lock.rs`, `keyed_lock.rs`, `error.rs` or `remote_catalog.rs`.

Each hunk gives a single-line anchor whose match count **must be asserted as 1** before editing, then
the verbatim FIND text and the verbatim REPLACE text. All three FIND blocks were matched against the
file on disk at 2026-08-23 06:51 and each occurs **exactly once**. Apply by text, never by line
number.

### Hunk A — the `ModelsFileReadState` doc

Assert first (must print `1`):

```
grep -Fc 'minus the in-flight-reload coalescer' crates/cyrup-config/src/models_store.rs
```

**FIND** (exactly these two lines, currently `:148-149`):

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1), minus the in-flight-reload coalescer:
/// cyrup's reader is synchronous file I/O under a `FileLock`, so there is no promise to share.
```

**REPLACE WITH** (five lines):

```rust
/// Pi `ModelsFileReadState` (models-store.ts:15-19 @v0.84.1). Upstream additionally memoises the
/// in-flight reload so concurrent `readLatest` callers await one promise. A future borrowing
/// `&self` cannot be handed out, so [`FileModelsStore::read_latest`] reaches the same property from
/// the other end: it re-checks this revision AFTER taking the lock, and a caller that queued behind
/// a reload returns the snapshot that reload stamped rather than re-parsing the file itself.
```

`[`FileModelsStore::read_latest`]` is a link into a private inherent method. That is deliberate and
safe here: the workspace pins `rustdoc::private_intra_doc_links = "allow"` and
`rustdoc::broken_intra_doc_links = "deny"` (root `Cargo.toml`, `[workspace.lints.rustdoc]`), and the
same file already links `[`FileModelsStore::read`]` from `read_all`'s doc.

### Hunk B — `current_snapshot` + `read_latest`

This one hunk replaces `FileModelsStore::read_latest` (doc comment and body, currently `:250-272`) and
introduces the extracted predicate immediately above it, between the closing brace of
`FileModelsStore::read_all` and the `readLatest` doc.

Assert first (each must print `1`):

```
grep -Fc '        let revision = file_revision(&self.path);' crates/cyrup-config/src/models_store.rs
grep -c 'fn current_snapshot' crates/cyrup-config/src/models_store.rs   # must print 0 — no collision
```

**FIND** (the whole function including its doc comment, 23 lines):

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

**REPLACE WITH** (47 lines):

```rust
    /// The snapshot, when the file has not moved since it was stamped — the same
    /// `getFileRevision(this.path) === readState.revision` predicate the `read_state` field doc
    /// above cites.
    ///
    /// `None` means "reload", and covers all three of upstream's misses: the file cannot be stat'd,
    /// the revision differs, or no revision has been stamped yet (a fresh store, or a `write`/
    /// `delete` that cleared it). A poisoned `read_state` also reloads rather than propagating.
    fn current_snapshot(&self) -> Option<OrderedObject> {
        let revision = file_revision(&self.path)?;
        let state = self.read_state.read().ok()?;
        (state.revision.as_deref() == Some(revision.as_str())).then(|| state.data.clone())
    }

    /// `readLatest` (models-store.ts:81-108 @v0.84.1): answer from the snapshot when the file's
    /// revision is unchanged, otherwise reload under the cross-process lock and re-stamp it.
    ///
    /// The snapshot is checked TWICE. The second check is load-bearing, not defensive — see the
    /// comment at the re-check below.
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
        // Re-check under the lock. This stands in for pi's in-flight-reload coalescer, and it is
        // not an optimisation: `acquire` awaits, so every other reader polled between the check
        // above and this line saw the OLD revision and queued behind us. Whoever reached here
        // first has already re-read the file and stamped its revision BEFORE releasing the guard,
        // so re-parsing now would be pure duplicated work — one full `read_to_string` +
        // `from_str` of the whole catalog per waiter. Upstream shares the reload's promise; a
        // future borrowing `&self` cannot be handed out, so the equivalent here is to observe the
        // result the winner published. `RemoteCatalog::refresh_providers` fans out one `read` per
        // configured provider through `join_all`, which is exactly this case.
        if let Some(data) = self.current_snapshot() {
            return Ok(data);
        }
        let data = self.read_all();
        self.update_read_state(&data, file_revision(&self.path));
        Ok(data)
    }
```

Notes that are requirements, not suggestions:

- `current_snapshot` is the existing predicate verbatim in meaning: `file_revision(..)?` is the old
  `revision.is_some()` guard, `.read().ok()?` is the old `let Ok(state) = ..` arm falling through to
  the reload, and the equality is the old `revision == state.revision` with the `Option<String>`
  comparison spelled as `Option<&str>`.
- Keep `then`, **not** `then_some` — the `clone` must stay lazy. `clippy::unnecessary_lazy_evaluations`
  does not fire on a closure whose body is a method call.
- The second check must sit **after** the `let _guard` binding and **before** `self.read_all()`.
  Getting that order wrong is the whole defect.
- The comment inside `read_latest` names `RemoteCatalog::refresh_providers` and does **not** carry a
  `remote_catalog.rs:NNN` pointer. This is deliberate: a cross-file line number in this crate has
  already rotted once (see §6), and the sibling spec that owns `lock.rs` is deleting the last one.

### Hunk C — the second abort check's rationale

Assert first (must print `1`):

```
grep -Fc '        // is synchronous, so it catches an abort raised while this task was descheduled across the' crates/cyrup-config/src/models_store.rs
```

**FIND** (five lines, currently `:306-310`, inside `impl ModelsStore for FileModelsStore`'s `read`):

```rust
        // … and once after it returns, before the entry is handed back (`:121`). Upstream's second
        // check catches an abort raised while the shared reload was in flight; here `read_latest`
        // is synchronous, so it catches an abort raised while this task was descheduled across the
        // `async fn`'s poll. Both are the same guarantee: nothing is returned to a caller that has
        // already given up.
```

**REPLACE WITH** (five lines):

```rust
        // … and once after it returns, before the entry is handed back (`:121`). Upstream's second
        // check catches an abort raised while the shared reload was in flight, and so does this
        // one: `read_latest` awaits `FileLock::acquire`, so the reload is genuinely in flight
        // across the two checks and a caller can give up inside it. Nothing is returned to a
        // caller that has already given up.
```

The code is already correct here and was correct before; only the justification was understated. This
is the "purely stale comment" half of the original finding, kept in this task because it is four lines
from Hunk B and shares its premise.

### 4.1 Dry-run facts (measured on a scratch copy, 2026-08-23 06:51)

All three hunks were applied to a copy of the current file under `tmp/` and the result inspected:

- Each FIND block matched **exactly once**.
- `crates/cyrup-config/src/models_store.rs` goes from **791** to **818** lines: **39 lines added,
  12 removed**.
- `rustfmt --edition 2024 --check` is silent on both the pre-change file and the post-change file, so
  the change is rustfmt-clean and reformats nothing else.
- No added line exceeds **100 characters** — and none exceeds 100 *bytes* either, so the file's 16
  pre-existing over-100-byte comment lines (all of them em-dash artefacts) stay the only ones.
- After the change: `current_snapshot` appears **3** times (one definition, two call sites),
  `is synchronous` appears **0** times, `let revision = file_revision` appears **1** time (inside
  `current_snapshot`), and `crate::lock::FileLock::acquire(` still appears **3** times (`read_latest`,
  `write`, `delete`).

## 5. Why this shape and not the others

- **A true in-flight coalescer** (`futures::future::Shared` over the reload). Cannot be written
  without restructuring ownership: `ModelsStore::read` takes `&self`, `FileModelsStore` is held as
  `Arc<dyn ModelsStore>` by `RemoteCatalog`, and `Shared<BoxFuture<'static, _>>` needs a `'static`
  future — there is no `Arc<Self>` in scope to build one from. It would also have to memoise a
  `Result` whose error is not `Clone`, i.e. the whole `SharedFut`/`MemoClear`/generation apparatus
  that `RefreshDedup` (`cyrup-provider/src/utils/refresh.rs`) already needed for the network refresh.
  Sixty lines of machinery to save what the re-check saves in three.
- **Re-checking between layer 1 and layer 2** of `FileLock`, so a waiter skips the `spawn_blocking`
  and the `flock` too. Strictly better on paper, but it requires a new `FileLock` API and puts
  models-store policy inside the lock primitive. The saving is a syscall pair against a whole-file
  JSON parse; not worth the coupling, and `lock.rs` has two open specs of its own
  (`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`,
  `LOW-spawn-blocking-join-error-reported-as-lock-contention.md`).
- **A second models-store-private async gate in front of `FileLock::acquire`.** Nests a lock inside a
  lock keyed on the same path, for no gain over the re-check.
- **Reverting `read_latest` to `fn`.** Not available: `FileLock::acquire` is `pub async fn` and stays
  that way — `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md` lists "do not change
  the lock mechanism" under its explicit non-scope, and the landed
  `HIGH-dropped-acquire-future-detaches-blocking-flock-task` depends on the async retry loop.
- **Comment-only fix (the original LOW filing).** Rejected on §3: it would document an O(N) regression
  as intentional.

## 6. Ordering with the other open specs

**No other queued task edits `crates/cyrup-config/src/models_store.rs`.** Re-checked file by file:

| Task | Where its edits land | Overlap with this task |
| --- | --- | --- |
| `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md` | three doc edits in `lock.rs` only | none |
| `LOW-spawn-blocking-join-error-reported-as-lock-contention.md` | `lock.rs` + `error.rs`; states "**No `models_store.rs` edit is needed or permitted**" | none |
| `LOW-public-api-changes-beyond-the-async-keyword.md` | one rustdoc hunk in `settings/store.rs` | none |
| `MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md` | **already landed** (`.flux/done/2026-08-23-00-08/`) — `store_err` at `models_store.rs:194-214` | none |
| `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` | **already landed** (`.flux/done/2026-08-23-00-08/`) | none |

**One live cross-reference.** `lock.rs:97` reads:

```rust
    /// (`models_store.rs:311` re-checks after the read; `write`/`delete` deliberately check only
```

`:311` is the second `ModelsStoreOperationOptions::throw_if_aborted(options)?` in
`impl ModelsStore for FileModelsStore`'s `read`, and it is **correct today**. This task's hunks add 27
net lines above it, moving it to `:338`.

The required resolution is **not** to renumber it here. `MEDIUM-cancel-token-…`'s EDIT 3 replaces that
whole two-line block with a name (`` `FileModelsStore`'s `read` re-checks after `read_latest` returns
``) and its definition of done asserts that no `models_store.rs:` pointer survives anywhere in
`lock.rs`. Its FIND text does not contain a line number from this file's *body*, so landing this task
first does not break its match. **Preferred order: land `MEDIUM-cancel-token-…` first.** If this task
lands first anyway, do not touch `lock.rs` — record that `lock.rs:97` is transiently stale and that
the sibling deletes the number outright.

## 7. Out of scope

- **`lock.rs`, `keyed_lock.rs`, `error.rs`.** No change. The re-check lives entirely in the
  models-store layer.
- **`remote_catalog.rs`.** `refresh_providers`' fan-out is correct and is the caller this fix serves;
  do not serialize it, and do not add a store-level memo there.
- **The `read_all`-then-`file_revision` ordering** at the end of `read_latest`. A non-cooperating
  writer between those two calls stamps a revision that does not describe the data — pre-existing,
  unrelated to the async conversion, and mitigated by the same advisory lock every cyrup writer takes.
  Do not widen this task to cover it.
- **`write` / `delete`.** Their `update_read_state(&all, None)` (`:345`, `:363`) is upstream's
  two-argument call and stays exactly as it is. They must keep reloading unconditionally, and they
  must keep checking the signal only *before* the acquire.
- **`load_overlay`'s sequential loop.** Already coalesces by construction.
- **Tests, benchmarks and documentation files.** Another team owns those. Add no test, change no
  existing test, create no `.md`. The four existing revision tests must keep passing unmodified —
  §8 item 6 checks that by inspection, not by writing anything.
- **The filename's `LOW-` prefix.** Frontmatter carries the priority; do not rename the file.

## 8. Definition of done

No test is written or modified, no benchmark is added, no documentation file is created, and no git
command is run. Every item is checked by reading the file or by one `grep`.

1. **The three hunks are present byte-for-byte**, applied in `crates/cyrup-config/src/models_store.rs`
   and nowhere else. `crates/cyrup-config/src/lock.rs`, `keyed_lock.rs`, `error.rs` and
   `crates/cyrup-provider/src/remote_catalog.rs` are byte-identical to their pre-change state.
2. **`grep -Fc 'is synchronous' crates/cyrup-config/src/models_store.rs`** prints `0`.
3. **`grep -c 'current_snapshot' crates/cyrup-config/src/models_store.rs`** prints `3` — one
   definition, two call sites, both inside `read_latest`.
4. **Ordering, read directly in the source.** `read_latest` contains exactly one
   `crate::lock::FileLock::acquire(`; the second `if let Some(data) = self.current_snapshot()` appears
   **after** the `let _guard` binding and **before** `self.read_all()`; and `update_read_state` is
   still called before the function returns, i.e. while `_guard` is alive.
5. **The old let-chain is gone.**
   `grep -Fc '        let revision = file_revision(&self.path);'` prints `1`, and the single remaining
   occurrence is the first line of `current_snapshot`, not of `read_latest`.
   `grep -Fc 'revision.is_some()'` prints `0`.
6. **The four existing revision tests still hold, by inspection of the new path** (do not edit them):
   - `read_answers_from_the_snapshot_until_the_file_revision_changes` (currently `:445`) — the
     planted-snapshot read hits the FIRST check, because only `read_state.data` was changed and the
     file's revision still equals the stamp; the post-rewrite read misses both checks (`state.revision`
     is the old stamp, the file's is new) and reloads to `"v2"`.
   - `a_deleted_file_reloads_rather_than_serving_the_stale_snapshot` (`:518`) — `file_revision` is
     `None`, so `current_snapshot`'s `?` returns `None` at both checks and the reload always wins.
   - `a_missing_or_corrupt_file_reads_as_no_overlay_and_never_errors` (`:657`) — same, plus
     `read_all`'s `unwrap_or_default`.
   - `cfg042_an_aborted_signal_is_refused_before_the_file_is_touched` (`:754`) — unaffected; the first
     `throw_if_aborted` still precedes `read_latest`.
7. **File shape matches the dry run.** `wc -l crates/cyrup-config/src/models_store.rs` prints `818`
   (was `791`), and `awk 'length > 100' crates/cyrup-config/src/models_store.rs | wc -l` still prints
   `16` — the pre-existing count, i.e. no added line is over 100 characters.
8. **Formatting holds.** `cargo fmt -p cyrup-config -- --check` reports no diff, or equivalently
   `rustfmt --edition 2024 --check crates/cyrup-config/src/models_store.rs` prints nothing.
9. **It compiles clean.** `cargo check -p cyrup-config` succeeds with no new warning, and
   `cargo clippy -p cyrup-config` adds none — in particular no `clippy::unnecessary_lazy_evaluations`
   on `current_snapshot`'s `then`, and no `rustdoc::broken_intra_doc_links` from Hunk A's
   `[`FileModelsStore::read_latest`]`.
10. **The regression is closed, argued from the code.** Trace one fan-out of N `read()` calls through
    `join_all` → `RefreshDedup::run` → `refresh_once` → `FileModelsStore::read` → `read_latest` and
    confirm in the source that the second check is reached by every caller that queued on
    `FileLock`'s layer 1, and that the winner's `update_read_state` runs before `_guard` drops. One
    `read_all` per fan-out, not N. No harness is built for this.

---

## 9. QA review — 2026-08-23 08:34 — PASS (9/10)

Reviewed against the tree, not against the spec's own claims. No source file was modified by QA.

**Defect fixed.** `FileModelsStore::read_latest` (`crates/cyrup-config/src/models_store.rs:271`) now
double-checks the revision: the second `if let Some(data) = self.current_snapshot()` sits AFTER the
`let _guard = crate::lock::FileLock::acquire(..).await?` binding and BEFORE `self.read_all()`, and
`update_read_state(&data, file_revision(&self.path))` still runs while `_guard` is alive. That is
exactly the ordering the O(N)-reload regression required.

**Every load-bearing claim in the new comments was verified against source, not taken on trust:**

- `FileLock::acquire` (`lock.rs:120`) really does await twice — `CONFIG_LOCK_HANDLE.guard(..).await`
  (layer 1, per-path keyed async mutex) and `spawn_blocking(..).await` — so the reload window really
  does contain a yield point and waiters really do queue on layer 1 of the same key.
- `RemoteCatalog::refresh_providers` (`remote_catalog.rs:522`) really does fan out with
  `futures::future::join_all` (`:535`), and `refresh_once` (`:555`) really does open with
  `self.store.read(provider_id, None).await` (`:564`) against the single `store: Arc<dyn ModelsStore>`
  (`:420`). The comment's "which is exactly this case" is true.
- `current_snapshot` is semantically identical to the deleted let-chain: `file_revision(..)?` is the
  old `revision.is_some()` guard, `.read().ok()?` is the old fall-through on a poisoned lock, and the
  `Option<&str>` equality is the old `Option<String>` comparison. `then` (lazy) was kept as required.
- Hunk C's new justification is true: `read_latest` awaits, so an abort genuinely can arrive between
  the two `throw_if_aborted` calls.
- Upstream `models-store.ts` claims were carried through unchanged. The local `tmp/pi` checkout is
  v0.83.0 (45-line `models-store.ts`, no `readLatest`), so they are not re-verifiable here — the spec
  said so honestly and introduced no new upstream line number. Confirmed: no new `@v0.84.1` pointer.

**Definition of done, measured:** 818 lines (was 791) ✅ · `is synchronous` → 0 ✅ ·
`current_snapshot` → 3 ✅ · `revision.is_some()` → 0 ✅ · `crate::lock::FileLock::acquire(` → 3 ✅ ·
`awk 'length > 100' | wc -l` → 16 ✅ · `rustfmt --edition 2024 --check` silent ✅ ·
`cargo clippy -p cyrup-config --all-targets` — zero diagnostics naming `models_store.rs`, no
`unnecessary_lazy_evaluations` ✅ · `cargo doc -p cyrup-config --no-deps --document-private-items` —
no `broken_intra_doc_links` from Hunk A's `[FileModelsStore::read_latest]` ✅ ·
`cargo test -p cyrup-config --lib models_store` — 11 passed, 0 failed, tests unmodified ✅ ·
`write`/`delete` still `update_read_state(&all, None)` (`:372`, `:390`) ✅ · `lock.rs`,
`keyed_lock.rs`, `error.rs`, `remote_catalog.rs` carry no edit from this task ✅.

**Two nits, neither blocking:**

1. DoD item 5's own grep is wrong, not the code. `grep -Fc '        let revision = file_revision(&self.path);'`
   prints `0`, not `1`, because the surviving occurrence inside `current_snapshot` ends in `)?;`. The
   intent of the check — the old let-chain is gone — is satisfied (`revision.is_some()` → 0).
2. `lock.rs:114` still reads ``(`models_store.rs:311` re-checks after the read; …)`` and `:311` is now
   `fn write_all`. This task predicted the move to `:338` (confirmed: the second `throw_if_aborted` is
   at `:338`), explicitly forbade renumbering it here, and handed it to
   `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`, which is still in
   `.flux/todo/`, has already re-audited it as load-bearing, and whose EDIT 3 FIND text still matches
   `lock.rs:114-115` byte-for-byte. Tracked handoff, not a dropped pointer — but it must land.

**Rating 9/10.** One point off only for nit 2: the change knowingly leaves a false cross-file pointer
live in `lock.rs` for the duration of the sibling task, and for nit 1's unrunnable DoD grep.
