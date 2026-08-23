---
title: Rewritten Concurrency Test Doc Misstates Which Lock It Covers
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 07:54
---

# The concurrency test's doc comment overstates its parallelism, inverts its coverage, and cites a deleted file

## Problem

`two_concurrent_overrides_on_one_settings_file_both_survive` in
[`crates/cyrup-ext-subagents/src/discovery/settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs)
was converted from `std::thread::scope` + two OS threads to `tokio::spawn` on a two-worker runtime,
and five lines of prose (`:265-269`) were added to justify it.

**The test body is sound and must not be touched** — see [§6](#6-detection-power-prior-measurement).
It detects a removed lock at least as often as the form it replaces. This is not a weakened or
tautological test. The defect is entirely in the doc comment, which carries three errors:

Current `:267-269`, verbatim:

```rust
    /// await it. Two workers keep the contention genuinely parallel, so this still exercises the
    /// real race; what it now also covers is the in-process layer of `FileLock`, which is the layer
    /// two writers in ONE process actually contend on.
```

1. **"keep the contention genuinely parallel"** — nothing forces the two tasks to overlap. Both go
   to the runtime's injection queue with no barrier; one worker may finish task 1 before task 2 is
   picked up. Overlap is probabilistic, exactly as it was with two OS threads. (`worker_threads = 2`
   *is* load-bearing, but for a different reason than the comment gives —
   [§5](#5-why-worker_threads--2-is-load-bearing-and-what-it-does-not-buy).)
2. **"what it now *also* covers is the in-process layer"** — "also" reads as *in addition to what it
   covered before*. The truth is the reverse: it now covers layer 1 **instead of** layer 2. The
   cross-process `flock` is taken uncontended here and its mutual exclusion is exercised nowhere
   in-process.
3. **`:261` cites `cyrup-config/src/settings.rs:1138`, a path that does not exist.** That module was
   split into the `crates/cyrup-config/src/settings/` directory; the `FileLock` + `write_atomic`
   pairing it points at now lives in `FileSettingsStore::with_lock`
   ([`crates/cyrup-config/src/settings/store.rs`](../../crates/cyrup-config/src/settings/store.rs)).
   The identical dead citation also sits in `lock_settings_file`'s doc at `:74`. See
   [§9](#9-the-settingsrs1138-citation-is-dead-in-two-places).

The correction must also land the fact the comment omits: this is a coverage **shift, not a loss**.
Layer-2 in-process contention became unreachable the moment the two-layer `FileLock` was put in
place — not when this test was rewritten. Nothing reachable was given up.

## Research

### 1. Citation audit against the tree as it stands

Three prior passes moved `crates/cyrup-config/src/lock.rs` substantially, and `settings.rs` became a
directory. **Every pointer in the previous revision of this spec was re-checked by reading the files
on disk.** Corrections applied throughout this document:

| Previous spec said | Reality now | How this spec cites it now |
|---|---|---|
| `lock.rs:59` is `pub async fn acquire` | `acquire` is at `lock.rs:103`, and its signature is `acquire(target: &Path, cancel: Option<&CancelToken>)` | by name: `FileLock::acquire` |
| `lock.rs:59-71` body calls `open_and_lock` via `spawn_blocking` | **`open_and_lock` no longer exists.** Layer 2 is now a NON-blocking `flock(LOCK_EX\|LOCK_NB)` (`open_and_try_lock` → `try_lock`) polled from async land with exponential backoff (`FIRST_RETRY` → `MAX_RETRY`) | by name: `open_and_try_lock`, `try_lock` |
| `lock.rs:91-93` — `Drop::drop` unlocks first | `impl Drop for FileLock` is at `lock.rs:194`; `FileExt::unlock(&self.file)` at `:196` is still its first statement | by name: the `Drop` impl for `FileLock` |
| `lock.rs:57` keys layer 1 on raw `lock_path_for(target)` | now `lock.rs:104`; still no `canonicalize` | by name: the `lock_path_for(target)` call at the head of `acquire` |
| `lock.rs:36-37` is `FileLock`'s "one task per path per process" doc | now `lock.rs:55-56` | by name: the `FileLock` type doc |
| `keyed_lock.rs:54-77` is `KeyedLocks::guard` | **still correct** | by name: `KeyedLocks::guard` |
| `settings_write.rs:80-89` is `lock_settings_file` | `:81-90` | by name: `lock_settings_file` |
| `settings_write.rs:98` is `write_settings_file` | `:100` | by name: `write_settings_file` |
| `settings_write.rs:44` is `read_settings_file_strict` | **still correct** | by name |
| `settings_write.rs:152` is `merge_builtin_agent_override` | **still correct** | by name |
| "four lines of prose at `:265-269`" | it is **five** lines | corrected |
| `cyrup-config/src/settings.rs:1138` | **file does not exist**; the pairing is `FileSettingsStore::with_lock` in `crates/cyrup-config/src/settings/store.rs` (`FileLock::acquire` at `:69`, `write_atomic` at `:84`) | by name |
| `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` is an open sibling | **landed**; it is in `.flux/done/2026-08-23-00-08/`, and the polled non-blocking layer 2 in `lock.rs` today is its result | [§10](#10-interactions) |

The **line-number citations that this spec still uses for `settings_write.rs`** (`:74-75`, `:261`,
`:263-269`, `:270-297`) were each read off disk during this pass and are correct as of
`settings_write.rs` at 445 lines (`wc -l`). They are given only as a locator — every edit below is anchored on
**exact, verified-unique text**, so the edits apply correctly even if those numbers drift again.

### 2. The `tokio::spawn` rewrite was forced by the async API

`FileLock::acquire` is `pub async fn`. Therefore `lock_settings_file` (`settings_write.rs:81-90`) is
`async`, and therefore `merge_builtin_agent_override` (`:152-169`) is `async`. A `std::thread::scope`
closure cannot `.await`, and `block_on` inside one is not an option. The `tokio::spawn` form is the
smallest honest conversion. **Keep it.**

### 3. Layer 1 gates layer 2 — `acquire` holds the async mutex across the whole layer-2 wait

`FileLock::acquire` (`crates/cyrup-config/src/lock.rs`), in order:

```rust
let lock_path = lock_path_for(target);
let token = cancel.unwrap_or(&NEVER_CANCELLED);
let in_process = CONFIG_LOCK_HANDLE.guard(lock_path.clone(), token).await.map_err(…)?;
// … spawn_blocking(open_and_try_lock) + a backoff retry loop over try_lock …
Ok(Self { _in_process: in_process, file })
```

`CONFIG_LOCK_HANDLE.guard` is [`KeyedLocks::guard`](../../crates/cyrup-core/src/keyed_lock.rs)
(`keyed_lock.rs:54-77`): one `tokio::sync::Mutex<()>` per key in a `DashMap`, awaited via
`lock_owned()`. One holder per key at a time. The guard is stored in the returned `FileLock` as
`_in_process` and lives until the whole read-modify-write finishes, so **layer 1 is held across
layer 2 for the guard's entire lifetime**.

Consequence for this test: both writers pass `dir/settings.json`, so both key on the identical
`dir/settings.json.lock` `PathBuf`. Writer B blocks in `guard()` — a pure userspace mutex wait, no
syscall — and only reaches `try_lock`'s `FileExt::try_lock` after writer A's `FileLock` has dropped,
which unlocks the `flock` as the first statement of the `Drop` impl. **The `flock` in this test is
always won on the first non-blocking attempt; the retry loop is never entered.**

### 4. There is no other in-process test of the `flock`

Neither [`lock.rs`](../../crates/cyrup-config/src/lock.rs) nor
[`keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) has a `mod tests`, and neither
`crates/cyrup-config/tests/` nor `crates/cyrup-core/tests/` exists. So the honest statement is
absolute, not hedged: after the two-layer split, no test in this workspace contends on the `flock`.
That is a property of the lock's design, not of this test — and the corrected comment should say so
rather than leaving a reader hunting for the coverage elsewhere.

### 5. Why `worker_threads = 2` IS load-bearing, and what it does not buy

The comment's "genuinely parallel" is wrong, but two workers are still necessary — for the RED case,
which is the case the test exists to catch:

- **Red** (the lock removed): `merge_builtin_agent_override`'s body is `read_settings_file_strict`
  (`settings_write.rs:44`, sync) + `serde_json` map edits + `write_settings_file` (`:100`, sync).
  With the `let _lock = lock_settings_file(path).await?;` line gone the `async fn` contains **no
  `.await` at all**, so each task completes in a single poll. On a single worker the two tasks would
  be strictly serialized and the lost update could never occur — the test would be green against
  broken code. Two workers restore true parallelism and the race with it.
- **Green** (as shipped): `acquire`'s `spawn_blocking(…).await` is a yield point, so the tasks
  interleave even on one worker — worker count is irrelevant to the passing path.

What two workers do **not** buy is guaranteed overlap. Both tasks are pushed to the injection queue
from the test's `block_on` thread with no barrier; a single worker may steal both. Overlap is likely,
never certain — the same probabilistic property the `thread::scope` version had.

### 6. Detection power (prior measurement)

`merge_builtin_agent_override`'s read-modify-write reproduced with the lock removed, run in both
forms, counting how often the lost update actually happens:

| run | NEW `tokio::spawn`, 2 workers | OLD `std::thread::scope` |
|---|---|---|
| n=400, quiet box, the two forms run back to back | 44 / 400 | 42 / 400 |
| n=600, loaded box, forms interleaved per iteration | 585 / 600 | 460 / 600 |
| n=400, loaded box, forms interleaved per iteration | 103 / 400 | 103 / 400 |
| n=400, loaded box, forms interleaved per iteration | 367 / 400 | 247 / 400 |

**Provenance:** these numbers come from a scratch harness built during the original augmentation
pass; they were **not** re-run in this pass (building a scratch tokio project is not worth the disk
budget, and no source change since then affects the RED path being measured — with the lock removed
neither form touches `cyrup-config` at all). What matters for the decision below is not the absolute
rate but the ordering, and that is independently supported from source by §5: the new form's RED
path is genuinely parallel on two workers, so it cannot be structurally weaker than two OS threads.
Both are flaky race detectors — already true of the thread version before the rewrite, not
introduced by it.

### 7. The coverage shift belongs to the lock change, not to this test

The two-layer `FileLock` (a rebase of [`CFGLOCK_2`](_backlog/CFGLOCK_2.md)) and the async propagation
that follows from it are one change; this test rewrite is one line item of that propagation. The
moment layer 1 went in front of layer 2, **no** single-process caller of `FileLock::acquire` naming
one path could reach a contended `flock` — however its test was written. Rewriting the test to
`tokio::spawn` did not remove that coverage; it had already ceased to exist. The comment must
attribute the shift correctly, or a maintainer reads it as this change having traded coverage away.

### 8. Caveat that keeps the wording honest: keys are not canonicalized

`FileLock::acquire` keys layer 1 on the raw `lock_path_for(target)` with no `canonicalize`, so today
two tasks naming the *same file* by *different spellings* (`a/settings.json` vs `./a/settings.json`)
would take different layer-1 keys and genuinely contend on the `flock`. That is the defect tracked in
[`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md),
not a testing strategy. This is why the replacement text says **"one task per path per process"** —
the same phrasing `FileLock`'s own type doc uses (`lock.rs:55-56`) — rather than "one task per file":
it is exactly true now, and stays exactly true if that task lands and canonicalizes the key.

### 9. The `settings.rs:1138` citation is dead in two places

`crates/cyrup-config/src/settings.rs` **does not exist**. `crates/cyrup-config/src/settings/` is a
directory (`effective.rs`, `layer.rs`, `manager.rs`, `merge.rs`, `migrate.rs`, `mod.rs`, `store.rs`,
`types.rs`, `tests/`). The `FileLock` + `write_atomic` pairing the comment holds up as cyrup's own
bar is `FileSettingsStore::with_lock` in `crates/cyrup-config/src/settings/store.rs`
(`FileLock::acquire` at `:69`, `write_atomic` at `:84`).

The dead path appears twice in `settings_write.rs`, at `:74` and `:261` — nowhere else in the crate.
No other task in `.flux/todo/` owns either line (checked by grep across `todo/*.md`). Both are fixed
here: `:261` is inside the very doc comment this task rewrites, and `:74` is the same dead string in
the same file, so leaving it would knowingly ship a false citation this task already diagnosed.
`FileSettingsStore` is a unique name in the workspace (declared once, at
`crates/cyrup-config/src/settings/store.rs:28`), so naming it is rot-proof where the path was not.

### 10. Interactions

- [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  corrects the `FileLock` field comment that this test's new prose depends on for "the second writer
  reaches `flock(2)` only after the first has already released it". Both descriptions agree — the
  release ordering comes from the explicit `FileExt::unlock` that is the first statement of
  `Drop::drop`. No textual overlap: different crates, different files. **That task grows `lock.rs` by
  10 lines**, which is another reason nothing here cites `lock.rs` by line number in the replacement
  text.
- [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md)
  is why the replacement says "per path per process" rather than "per file" — see §8. If that task
  lands, this comment stays correct unchanged.
- `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` has **landed** (now in
  `.flux/done/2026-08-23-00-08/`). Its result is the polled non-blocking layer 2 that `lock.rs`
  carries today. It did not change the layer-1-before-layer-2 ordering this comment describes.
- [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
  is still filed under `todo/`, but its goal is already met in `lock.rs` today — cancellation reaches
  layer 2 through the `tokio::select!` in `acquire`'s retry loop — by a different mechanism than that
  spec prescribes (that spec's `AcquireTask`/`open_and_lock` shape no longer matches the source).
  Whatever becomes of it, it does not change the layer ordering, so it does not invalidate this
  comment.
- [`LOW-awaitless-test-promoted-to-tokio-test-by-the-automation.md`](./LOW-awaitless-test-promoted-to-tokio-test-by-the-automation.md)
  reverts a *different* test that was promoted to `#[tokio::test]` with no `.await`. This one has
  real awaits and a deliberate `worker_threads = 2`; it is **not** an instance of that pattern and
  must not be swept into that revert.
- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  owns the 16 pre-existing rustfmt hunks in this file (see §11). **Do not fix any of them here.**

### 11. `rustfmt --check` on this file is ALREADY failing — do not try to make it clean

Measured this pass:

- `rustfmt --check --edition 2024 crates/cyrup-ext-subagents/src/discovery/settings_write.rs`
  exits **1** today, with **16** hunks starting at lines
  `81, 111, 159, 246, 305, 312, 328, 341, 348, 355, 359, 376, 386, 414, 427, 437`.
  Not one of them touches a `///` line; they are all expression-layout hunks
  (`FileLock::acquire(path, None).await.map_err(…)`, `overrides_of`'s chain, and similar).
- The same command run on a scratch copy carrying **all three edits below** also exits 1 with the
  **same 16 hunks, byte-identical in content**, the only difference being that the ten hunks after
  the edit point are reported 13 lines lower. Verified by `diff` of the two `--check` outputs with
  the file path normalised: the only differing lines are the `Diff in F:<n>:` headers.

So the edits are provably **format-neutral**. The previous revision of this spec asserted
`rustfmt --check` "is clean" and made that a done-criterion; that was false and would have pushed the
implementer into unrelated reformatting. The criterion below is the correct one.

## Decision: comment-only. The test body is untouched.

**(A) Rewrite the doc comment, keep the test body exactly as it is. — CHOSEN, and the single
required path.** §6 shows the body is the equal-or-better detector, §5 shows why from source, and §2
shows the `tokio::spawn` shape is forced by the async API. The whole defect is text. The dead
`settings.rs:1138` citation is repaired in the same pass because it is the same class of defect, in
the same doc comment (`:261`) and the same file (`:74`), and no other task owns it.

**(B) Additionally add a two-process `flock` test. — REJECTED, out of scope.** It would need a
spawned child binary or a `#[test]`-harness fork, i.e. new test infrastructure in `cyrup-config`, for
a layer this change did not touch. If it is ever wanted it belongs to `cyrup-config`, not to a
subagents settings-writer test. The corrected comment names the gap so the decision stays visible.

**(C) Force overlap with a barrier so "genuinely parallel" becomes true. — REJECTED.** A barrier
between the two `merge_builtin_agent_override` calls cannot be placed inside them without editing
production code, and one placed before them only synchronizes the task starts, not the
read-modify-write windows. It would add machinery and change a body §5/§6 show is already the
stronger detector.

## Required change

Three edits, all in
[`crates/cyrup-ext-subagents/src/discovery/settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs).
**No other file is touched.** Each edit is given as exact find text → exact replacement text. Every
find string was verified against the file on disk this pass: **match count = 1** for each.

Edits 1 and 2 are line-count neutral (2→2 and 1→1 lines), so they can be applied in any order and the
`:263-269` locator for Edit 3 stays valid either way.

### Edit 1 — `lock_settings_file`'s doc (`:74-75`): dead path → the item's name

**Find** (exactly once; note the `—` is U+2014 EM DASH, the only non-ASCII character in either
block, at offset 30 of the first line; no leading indentation on either line):

```rust
/// missed is **cyrup's own** — `cyrup-config/src/settings.rs:1138` takes a `FileLock` plus
/// `write_atomic` for exactly this file.
```

**Replace with** (the `—` is again U+2014; the paragraph is re-wrapped across the same two lines):

```rust
/// missed is **cyrup's own** — `cyrup-config`'s `FileSettingsStore::with_lock` takes a `FileLock`
/// plus `write_atomic` for exactly this file.
```

Widths: 98 and 46 characters.

### Edit 2 — the test doc's bar citation (`:261`): dead path → the item's name

**Find** (exactly once; pure ASCII; four spaces of indentation):

```rust
    /// cyrup's own bar (`cyrup-config/src/settings.rs:1138`) is `FileLock` + `write_atomic`.
```

**Replace with**:

```rust
    /// cyrup's own bar (`FileSettingsStore::with_lock`) is `FileLock` + `write_atomic`.
```

Width: 88 characters. The crate is not repeated here because the preceding line already frames this
as cyrup's own bar and `FileSettingsStore` is unique workspace-wide; Edit 1 carries the crate name.

### Edit 3 — the test doc's body (`:263-269`): the two false claims

**Find** (exactly once; pure ASCII, no em dashes; four spaces of indentation on every line):

```rust
    /// Red before the fix: with the bare `fs::write` both threads read `{}` and the last write
    /// wins, so exactly ONE override survives.
    /// The writers are `tokio::spawn`ed onto a two-worker runtime rather than `std::thread::scope`d,
    /// because `merge_builtin_agent_override` is now `async` and a plain thread closure cannot
    /// await it. Two workers keep the contention genuinely parallel, so this still exercises the
    /// real race; what it now also covers is the in-process layer of `FileLock`, which is the layer
    /// two writers in ONE process actually contend on.
```

**Replace with** (20 lines; the three `—` are U+2014 EM DASH, on the "not certain", "NOT the
`flock`" and "was rewritten" lines; everything else is ASCII; four spaces of indentation on every
line):

```rust
    /// Red before the fix: with the bare `fs::write` both writers read `{}` and the last write
    /// wins, so exactly ONE override survives.
    ///
    /// The writers are `tokio::spawn`ed onto a two-worker runtime rather than `std::thread::scope`d
    /// because `merge_builtin_agent_override` is `async` since the two-layer `FileLock` landed, and
    /// a plain thread closure cannot await it. `worker_threads = 2` is load-bearing for the RED
    /// case specifically: with the lock removed this function's body is entirely synchronous, so on
    /// one worker each task would run to completion in a single poll and neither could read the
    /// other's stale document. Two workers make the overlap POSSIBLE, not certain — like the
    /// `thread::scope` form it replaces this is a probabilistic race detector, measured to fire at
    /// least as often as that form did.
    ///
    /// What it contends on is `FileLock`'s layer 1, the per-path async mutex — NOT the `flock`.
    /// Layer 1 admits one task per path per process, so the second writer reaches `flock(2)` only
    /// after the first has already released it. That is DIFFERENT coverage from the pre-split
    /// thread version, not less: layer 1 is the layer two writers in ONE process actually contend
    /// on, and it is what serializes the read-modify-write this test is about. In-process `flock`
    /// contention stopped being reachable when layer 1 was put in front of it, not when this test
    /// was rewritten — nothing reachable was given up here. Exercising the `flock` needs a second
    /// process, which no unit test in this crate is.
```

Notes on the text:

- Line `:263`'s `both threads` becomes `both writers`. It is the one word left in the paragraph still
  describing the pre-async form, it sits inside the block being replaced anyway, and leaving it makes
  the corrected paragraph self-contradictory.
- Lines `:253-260` (the SUBA-029 header and THE USER ACTION paragraph) and `:262` (a bare `///`) are
  **not** touched.
- No `lock.rs` line numbers appear in the replacement text, deliberately: `lock.rs` has already moved
  three times and a sibling task will move it again.
- No intra-doc links are introduced — `FileLock` and `FileSettingsStore::with_lock` are deliberately
  in backticks, not brackets. This is a `#[cfg(test)]` module in `cyrup-ext-subagents`; neither item
  is in scope here and a bracketed link would be a broken-intra-doc-link warning.
- The replacement grows the file from 445 to 458 lines by `wc -l` (+13, all from Edit 3). The test
  body shifts
  from `:270-297` to `:283-310` and is otherwise unchanged.

### Pre-verified formatting facts

- Longest replacement line is **100** characters (three lines hit it). The file's existing maximum
  `///` line is **102** (`:173`), so nothing regresses.
- There is no `rustfmt.toml` or `.rustfmt.toml` anywhere in the workspace, so `wrap_comments` is at
  its default `false`; rustfmt will not reflow any of these lines. Confirmed empirically: none of the
  16 `rustfmt --check` hunks on this file touches a `///` line, before or after the edits.
- All character counts above are Unicode code points, not bytes (each `—` is 3 UTF-8 bytes).

## Definition of done

No tests are written or run. No benchmarks. No new documentation beyond the three replacements
above. No `git` command is used for any check — every item below is verified by reading files and
running `rustfmt`/`diff`/`sed`/`sha256sum`.

Before editing, take a reference copy so the "comment-only" check needs no VCS:

```
cp crates/cyrup-ext-subagents/src/discovery/settings_write.rs /home/user/cyrup/tmp/settings_write.before.rs
```

Then:

- [ ] Each of the three find strings occurred **exactly once** before its replacement (assert the
      count; do not apply a replacement whose count is not 1).
- [ ] All three replacements are applied verbatim, including the U+2014 em dashes and the four-space
      indentation on Edits 2 and 3.
- [ ] `diff /home/user/cyrup/tmp/settings_write.before.rs crates/cyrup-ext-subagents/src/discovery/settings_write.rs`
      shows exactly three hunks, at `74-75`, `261`, and `263-269`, and **every** added and removed
      line in all three begins with `///` after its indentation. No non-comment line appears in the
      diff.
- [ ] `wc -l crates/cyrup-ext-subagents/src/discovery/settings_write.rs` reports **458** (it
      reports 445 today).
- [ ] The test body is byte-identical to what is on disk now:
      `sed -n '283,310p' crates/cyrup-ext-subagents/src/discovery/settings_write.rs | sha256sum`
      prints `9474117fd9ef1ea082d79d23afd3f231b8b65fb324ddcd0e9e1a05cbebfff227`.
      (That is the same hash `sed -n '270,297p' …` prints on the pre-edit file.) It still opens with
      `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` and still uses the `tokio::spawn`
      loop. Reverting it to `std::thread::scope` would not compile and would be wrong anyway.
- [ ] `grep -rn 'settings\.rs:1138' crates/` returns **nothing**.
- [ ] `grep -c 'genuinely parallel' crates/cyrup-ext-subagents/src/discovery/settings_write.rs`
      returns **0**.
- [ ] No file outside `crates/cyrup-ext-subagents/src/discovery/settings_write.rs` is modified. In
      particular [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs),
      [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) and
      [`crates/cyrup-config/src/settings/store.rs`](../../crates/cyrup-config/src/settings/store.rs)
      are untouched by this task.
- [ ] `rustfmt --check --edition 2024 crates/cyrup-ext-subagents/src/discovery/settings_write.rs`
      still exits 1 with **exactly 16 hunks**, now starting at lines
      `81, 111, 159, 246, 318, 325, 341, 354, 361, 368, 372, 389, 399, 427, 440, 450` — the same 16
      as before the edit, the last ten shifted by +13. **This is the pass condition; a "clean"
      result would mean unrelated reformatting was done and is a FAILURE.** Do **not** run
      `cargo fmt`, and do not fix any of those hunks — they belong to
      [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
- [ ] No test is added, removed, or renamed; the module still contains the same test functions it
      contains now. The deliverable is the comment; there is no runtime behaviour to assert.
- [ ] Delete the reference copy: `rm /home/user/cyrup/tmp/settings_write.before.rs`.
