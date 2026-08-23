---
title: Rewritten Concurrency Test Doc Misstates Which Lock It Covers
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:41
---

# The rewritten concurrency test's new prose overstates its parallelism and inverts its coverage

## Problem

`fe86c7f` rewrote `two_concurrent_overrides_on_one_settings_file_both_survive` in
[`crates/cyrup-ext-subagents/src/discovery/settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs)
from `std::thread::scope` + two OS threads to `tokio::spawn` on a two-worker runtime, and added four
lines of prose at `:265-269` to justify it.

**The rewritten test body is sound and must not be touched** — see [Research §5](#5-detection-power-measured):
it detects a removed lock at least as often as the form it replaces. This is not a weakened or
tautological test. The defect is entirely in the new prose, whose two factual claims are both wrong:

```rust
/// Two workers keep the contention genuinely parallel, so this still exercises the
/// real race; what it now also covers is the in-process layer of `FileLock`, which is the layer
/// two writers in ONE process actually contend on.
```

1. **"keep the contention genuinely parallel"** — nothing forces the two tasks to overlap. Both go
   to the runtime's injection queue with no barrier; one worker may finish task 1 before task 2 is
   picked up. Overlap is probabilistic, exactly as it was with two OS threads. (The `worker_threads
   = 2` *is* load-bearing, but for a different reason than the comment gives — [§4](#4-why-worker_threads--2-is-load-bearing-and-what-it-does-not-buy).)
2. **"what it now *also* covers is the in-process layer"** — "also" reads as *in addition to what it
   covered before*. The truth is the reverse: it now covers layer 1 **instead of** layer 2. The
   cross-process `flock` is taken uncontended here and its mutual exclusion is exercised nowhere
   in-process.

The correction must land the *third* fact the comment omits: this is a coverage **shift, not a
loss**. Layer-2 in-process contention became unreachable the moment the two-layer `FileLock` was put
in place — not when this test was rewritten. Nothing reachable was given up.

## Research

### 1. The rewrite was forced, and the comment's stated reason for it is the only correct part

Merge base (`git show 4902cddf:crates/cyrup-config/src/lock.rs`) — one layer, sync:

```rust
pub struct FileLock { file: File }
impl FileLock {
    pub fn acquire(target: &Path) -> Result<Self, ConfigError> { /* open + FileExt::lock */ }
}
```

HEAD — [`crates/cyrup-config/src/lock.rs:59`](../../crates/cyrup-config/src/lock.rs) is
`pub async fn acquire`, so [`lock_settings_file`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs)
(`settings_write.rs:80-89`) and therefore `merge_builtin_agent_override` (`:152`) are `async`. A
`std::thread::scope` closure cannot `.await`, and `block_on` inside one is not an option. The
`tokio::spawn` form is the smallest honest conversion. **Keep it.**

### 2. Layer 1 gates layer 2 — `acquire` holds the async mutex across the `flock`

[`crates/cyrup-config/src/lock.rs:59-71`](../../crates/cyrup-config/src/lock.rs):

```rust
let in_process = CONFIG_LOCK_HANDLE.guard(lock_path.clone(), token).await…?;
let target_owned = target.to_path_buf();
let file = tokio::task::spawn_blocking(move || open_and_lock(&target_owned, &lock_path)).await…?;
Ok(Self { _in_process: in_process, file })
```

`CONFIG_LOCK_HANDLE.guard` is [`KeyedLocks::guard`](../../crates/cyrup-core/src/keyed_lock.rs)
(`keyed_lock.rs:54-77`): one `tokio::sync::Mutex<()>` per key in a `DashMap`, awaited via
`lock_owned()`. One holder per key at a time. The guard is stored in the returned `FileLock` and
lives until the whole read-modify-write finishes, so **layer 1 is held across layer 2 for the
guard's entire lifetime**.

Consequence for this test: both writers pass `dir/settings.json`, so both key on the identical
`dir/settings.json.lock` `PathBuf`. Writer B blocks in `guard()` — a pure userspace mutex wait, no
syscall — and only reaches `open_and_lock`'s `FileExt::lock` after writer A's `FileLock` has
dropped, which (`lock.rs:91-93`) unlocks the `flock` as the first statement of `Drop::drop`. **The
`flock` in this test is always taken uncontended.**

### 3. There is no other in-process test of the `flock`

Neither [`lock.rs`](../../crates/cyrup-config/src/lock.rs) nor
[`keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) has a `mod tests`, and neither
`crates/cyrup-config/tests/` nor `crates/cyrup-core/tests/` exists. So the honest statement is
absolute, not hedged: after the two-layer split, no test in this workspace contends on the `flock`.
That is a property of the lock's design, not of this test — and the corrected comment should say so
rather than leaving a reader hunting for the coverage elsewhere.

### 4. Why `worker_threads = 2` IS load-bearing, and what it does not buy

The comment's "genuinely parallel" is wrong, but two workers are still necessary — for the RED case,
which is the case the test exists to catch:

- **Red** (the lock removed): `merge_builtin_agent_override`'s body is `read_settings_file_strict` +
  map edits + `write_settings_file`, all synchronous (`settings_write.rs:44`, `:98`). With the
  `FileLock` gone the `async fn` contains **no `.await` at all**, so each task completes in a single
  poll. On a single worker the two tasks would be strictly serialized and the lost update could
  never occur — the test would be green against broken code. Two workers restore true parallelism
  and the race with it.
- **Green** (as shipped): `acquire`'s `spawn_blocking(...).await` is a yield point, so the tasks
  interleave even on one worker — worker count is irrelevant to the passing path.

What two workers do **not** buy is guaranteed overlap. Both tasks are pushed to the injection queue
from the test's `block_on` thread with no barrier; a single worker may steal both. Overlap is likely,
never certain — the same probabilistic property the `thread::scope` version had.

### 5. Detection power, measured

`merge_builtin_agent_override`'s read-modify-write reproduced with the lock removed, run in both
forms, counting how often the lost update actually happens:

| run | NEW `tokio::spawn`, 2 workers | OLD `std::thread::scope` |
|---|---|---|
| n=400, quiet box, the two forms run back to back | 44 / 400 | 42 / 400 |
| n=600, loaded box, forms interleaved per iteration | 585 / 600 | 460 / 600 |
| n=400, loaded box, forms interleaved per iteration | 103 / 400 | 103 / 400 |
| n=400, loaded box, forms interleaved per iteration | 367 / 400 | 247 / 400 |

(Interleaving alternates the two forms inside one loop so both see the same machine load; the
absolute rate swings with load, the relative ordering does not.) The new form is never the weaker
detector. Both are flaky race detectors — already true of the thread version at merge base, not
introduced here.

### 6. The coverage shift belongs to the lock change, not to this test

`fe86c7f` is a single commit carrying two logically separate changes: the two-layer `FileLock` (a
rebase of [`CFGLOCK_2`](_backlog/CFGLOCK_2.md), originally `c3d0cdf`) and the async propagation that
follows from it, of which this test rewrite is one line item. The moment layer 1 went in front of
layer 2, **no** single-process caller of `FileLock::acquire` naming one path could reach a contended
`flock` — however its test was written. Rewriting the test to `tokio::spawn` did not remove that
coverage; it had already ceased to exist. The comment must attribute the shift correctly, or a
maintainer reads it as this diff having traded coverage away.

### 7. Caveat that keeps the wording honest: keys are not canonicalized

`lock.rs:57` keys layer 1 on the raw `lock_path_for(target)` with no `canonicalize`, so today two
tasks naming the *same file* by *different spellings* (`a/settings.json` vs `./a/settings.json`)
would take different layer-1 keys and genuinely contend on the `flock`. That is the defect tracked
in [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md),
not a testing strategy. This is why the replacement text says **"one task per path per process"** —
the same phrasing `FileLock`'s own type doc uses (`lock.rs:36-37`) — rather than "one task per
file": it is exactly true now, and stays exactly true if that task lands and canonicalizes the key.

## Decision: comment-only. The test body is untouched.

**(A) Rewrite the doc comment, keep the test exactly as it is. — CHOSEN.** §5 proves the body is the
equal-or-better detector, and §1 proves the `tokio::spawn` shape is forced by the async API. The
whole defect is text.

**(B) Additionally add a two-process `flock` test. — REJECTED, out of scope.** It would need a spawned
child binary or a `#[test]`-harness fork, i.e. new test infrastructure in `cyrup-config`, for a layer
this diff did not change. If it is ever wanted it belongs to `cyrup-config`, not to a subagents
settings-writer test. The corrected comment names the gap so the decision is visible.

**(C) Force overlap with a barrier so "genuinely parallel" becomes true. — REJECTED.** A barrier
between the two `merge_builtin_agent_override` calls cannot be placed inside them without editing
production code, and one placed before them only synchronizes the task starts, not the
read-modify-write windows. It would add machinery and change a body §5 shows is already the stronger
detector.

## Required change

One edit, in
[`crates/cyrup-ext-subagents/src/discovery/settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs).
Replace **lines 263-269** — the "Red before the fix" sentence plus the four new lines — with exactly:

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

- Line 263's `both threads` becomes `both writers`. It is the one word left in the paragraph still
  describing the pre-async form, it sits inside the hunk being edited anyway, and leaving it makes
  the corrected paragraph self-contradictory.
- Lines 253-262 (the SUBA-029 header and THE USER ACTION paragraph) are **not** touched. They are
  accurate and predate the branch.
- Longest new line is 100 columns; the file's existing `///` maximum is 102 and rustfmt does not
  reflow comments (`wrap_comments` defaults off, no `rustfmt.toml` in the repo), so this is
  format-stable.
- No intra-doc links are introduced — `FileLock` is deliberately in backticks, not brackets: this is
  a `#[cfg(test)]` module in `cyrup-ext-subagents`, and the surrounding comment already cites
  `cyrup-config` paths as plain text (`:261`).

## Definition of done

- [ ] `settings_write.rs` lines 263-269 replaced with the block above, verbatim.
- [ ] `git diff -- crates/cyrup-ext-subagents/src/discovery/settings_write.rs` shows a
      **comment-only** change confined to that range.
- [ ] The test body `:270-297` is byte-identical to what is on the branch now — same
      `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`, same `tokio::spawn` loop, same
      assertion. Reverting it to `std::thread::scope` would not compile and would be wrong anyway.
- [ ] No source file outside `settings_write.rs` is modified. In particular
      [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) and
      [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) are
      untouched by this task.
- [ ] `rustfmt --check --edition 2024 crates/cyrup-ext-subagents/src/discovery/settings_write.rs`
      is clean. Do **not** run a workspace-wide `cargo fmt`.
- [ ] No new tests, and no change to the number of tests in the module. The deliverable is the
      comment; there is no runtime behaviour to assert.

## Interactions

- [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  corrects the `FileLock` field comment that this test's new prose depends on for "the second writer
  reaches `flock(2)` only after the first has already released it". Both descriptions must agree;
  they do — the release ordering comes from the explicit `FileExt::unlock` in `Drop::drop`. No
  textual overlap: different crates, different files.
- [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md)
  is why the replacement says "per path per process" rather than "per file" — see §7. If that task
  lands, this comment stays correct unchanged.
- [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md)
  and [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
  rework `FileLock::acquire`'s internals. Neither changes the layer-1-before-layer-2 ordering this
  comment describes, so neither invalidates it — but if either ends up moving the `KeyedGuard` into
  the blocking closure, re-read this comment in that change.
- [`LOW-awaitless-test-promoted-to-tokio-test-by-the-automation.md`](./LOW-awaitless-test-promoted-to-tokio-test-by-the-automation.md)
  reverts a *different* test that was promoted to `#[tokio::test]` with no `.await`. This one has
  real awaits and a deliberate `worker_threads = 2`; it is **not** an instance of that pattern and
  must not be swept into that revert.
- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  also covers this crate; keep the file-scoped `rustfmt --check` above rather than a workspace pass.
