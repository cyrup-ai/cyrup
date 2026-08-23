---
title: Guard Doc Points At A Select Loop Below That Is Not There
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:41
---

# Repoint the two `ops/local/` doc references that the `local.rs` split left dangling

Splitting `crates/cyrup-tools/src/ops/local.rs` (1,912 lines) into twelve files under
`crates/cyrup-tools/src/ops/local/` broke every doc reference that pointed at a location by file
POSITION rather than by NAME. Two such references exist in the new tree and both were correct at
the merge base `4902cddf`:

| Site | Says | Referent now lives in |
|---|---|---|
| [`ops/local/guard.rs:18`](../../crates/cyrup-tools/src/ops/local/guard.rs) | "the `select!` loop below" | [`ops/local/proc.rs:177`](../../crates/cyrup-tools/src/ops/local/proc.rs) |
| [`ops/local/tests/mod.rs:49`](../../crates/cyrup-tools/src/ops/local/tests/mod.rs) | "The three fixtures below" | [`ops/local/tests/exec_argv.rs`](../../crates/cyrup-tools/src/ops/local/tests/exec_argv.rs) `:93`, `:128`, `:162` |

This task fixes both, plus one one-character follow-on inside the same `guard.rs` doc comment.
Documentation only — no behaviour, no signature, no test changes.

## Why the `guard.rs` one is load-bearing, not cosmetic

`KillTreeOnDrop`'s doc comment is the only written justification for a destructor that issues
`killpg(pgid, SIGKILL)` — an `unsafe` group-wide signal fired from `Drop`. The argument runs:

1. Pi's abort/timeout handling hangs off an `async` function that ALWAYS settles, so upstream
   cannot leave the shell's process GROUP alive.
2. A Rust future has no such guarantee: it can be dropped at any `.await`, **abandoning the
   `select!` loop whose `send_sigkill_tree` arms are the non-drop equivalent**.
3. Therefore the "runs no matter how we leave" slot must be `Drop`.
4. And `kill_on_drop(true)` is not a substitute, because it kills one pid, not the group.

Step 2 is the hinge, and it is the step whose pointer now resolves to nothing: there is no
`select!` anywhere in `guard.rs`. A reader who cannot find the loop cannot check step 2, and the
paragraph that immediately follows exists to refute exactly the conclusion they will reach
instead ("this is redundant with `kill_on_drop`").

The correction must therefore keep the whole argument intact and change only the pointer. The same
doc already demonstrates the right style two paragraphs later — "Declared AFTER `child` in `exec`"
names the function instead of leaning on file position.

Secondary effect worth knowing before editing: `guard.rs:27` ("the guard is disarmed only AFTER
**the loop** has observed `child.wait()`") and `guard.rs:35` ("a statement placed after **the**
`select!` **loop**") are bare anaphors whose only antecedent in the file is the broken line 18.
Fixing line 18 by NAMING the loop restores the antecedent for both — which is why line 27 needs no
edit of its own, and why line 35 only needs one word.

## Evidence (verified at HEAD of `claude/cyrup-tools-largest-file-rjs3yg`)

`select!` in `guard.rs` — two prose mentions, zero loops:

```
crates/cyrup-tools/src/ops/local/guard.rs:18: /// `select!` loop below without running a single one of its `send_sigkill_tree` arms.
crates/cyrup-tools/src/ops/local/guard.rs:35: /// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting
```

Where the loops actually are:

```
crates/cyrup-tools/src/ops/local/proc.rs:177:            tokio::select! {   # LocalProc::exec
crates/cyrup-tools/src/ops/local/proc.rs:301:            tokio::select! {   # LocalProc::exec_argv
```

The `SleeperMarker` doc and the fixtures it points at:

```
crates/cyrup-tools/src/ops/local/tests/mod.rs:49:  /// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by
crates/cyrup-tools/src/ops/local/tests/mod.rs:81:  struct SleeperMarker {

crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:95:    let sleeper = SleeperMarker::new("gracefulterm");
crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:130:   let sleeper = SleeperMarker::new("argvtimeoutkill");
crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:164:   let sleeper = SleeperMarker::new("argvcancelkill");
```

`grep -c 'SleeperMarker::new' tests/mod.rs` is `0` — "below" points past the end of the file.

Both were accurate before the split. In `4902cddf`'s single `local.rs`:

```
572:  struct KillTreeOnDrop {          807:  tokio::select! {     # below ✔
1070: /// The three fixtures below     1399/1428/1459: SleeperMarker::new(...)   # below ✔
```

## The edits

Three exact replacements. Nothing else in either file changes.

### Edit 1 — `crates/cyrup-tools/src/ops/local/guard.rs`, lines 17-18

Replace exactly (2 lines):

```rust
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon the
/// `select!` loop below without running a single one of its `send_sigkill_tree` arms.
```

with exactly (3 lines):

```rust
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon
/// [`LocalProc::exec`]'s `select!` loop (in [`super::proc`]) without running a single one of its
/// `send_sigkill_tree` arms.
```

Line 16 above it (`/// call. A Rust future has no such guarantee — …`) is unchanged; the trailing
`the` moves off line 17 onto the new line 18, so the sentence still reads as one clause.

Why this exact form:

- **``[`LocalProc::exec`]`` needs no new link target.** Its reference definition already sits at the
  bottom of this same doc comment (`guard.rs:42`,
  `` /// [`LocalProc::exec`]: crate::ops::ProcOps::exec ``), placed there for the "Declared AFTER
  `child` in `exec`" sentence. Reusing it is what makes the two cross-references consistent.
- **``[`super::proc`]`` is the module the loop moved to** and resolves from `guard.rs`
  (`super` == `crate::ops::local`). The same relative form is already used and proven in this very
  file: `guard.rs:2` links `` [`super::tracking`] `` from the module doc, and `guard.rs:103-104`
  link `pub(super)` items by full path. No rustdoc lint fires — `KillTreeOnDrop` is `pub(super)`,
  so `rustdoc::private_intra_doc_links` (which only fires from PUBLIC items) does not apply, and
  the crate configures no rustdoc lints (`crates/cyrup-tools/src/lib.rs` denies `unsafe_code`
  only; `Cargo.toml`'s `[workspace.lints.clippy]` lists no rustdoc entries).
- **Naming the function AND the module** is deliberate. The module alone would not say which of
  `proc.rs`'s two `select!` loops is meant, and `exec_argv`'s loop (`proc.rs:301`) is explicitly
  NOT the one this guard protects — `exec_argv` never `setsid`s and never `killpg`s
  (`command.rs:85-89`), so `KillTreeOnDrop` is not armed for it at all.
- Longest new line is 97 columns, inside the file's existing range (`guard.rs` already carries
  lines of 101, 102 and 104 columns).

### Edit 2 — `crates/cyrup-tools/src/ops/local/guard.rs`, line 35 (one word)

Replace exactly:

```rust
/// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting
```

with exactly:

```rust
/// "runs no matter how we leave" is `Drop`, not a statement placed after that `select!` loop. Putting
```

`the` → `that`. This mention describes a hypothetical alternative implementation, not a location,
so it did not break outright — but after Edit 1 the doc names a specific loop, and `that` binds
this sentence to it instead of leaving a definite article that a reader may still try to resolve
inside `guard.rs`. One character; no re-wrap, no ripple into lines 36-40. The line goes from 101 to
102 columns, which is not a rustfmt concern: the repo has no `rustfmt.toml` / `.rustfmt.toml` and
rustfmt's `wrap_comments` defaults to `false`, so comment width is never reformatted (this is why
the surrounding 101/102/104-column comment lines already survive a clean `cargo fmt`).

### Edit 3 — `crates/cyrup-tools/src/ops/local/tests/mod.rs`, lines 49-51

Replace exactly (3 lines):

```rust
/// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by
/// DESIGN — and until this helper landed, none of them reaped it, so the process outlived the
/// whole test binary by up to a second.
```

with exactly (3 lines):

```rust
/// prove exactly that. The three fixtures in `exec_argv.rs` that use this helper therefore
/// leave a live `sleep 1` behind by DESIGN — and until this helper landed, none of them reaped
/// it, so the process outlived the whole test binary by up to a second.
```

Why this exact form:

- The three fixtures are `exec_argv_timeout_preserves_the_real_code_of_a_graceful_sigterm_handler`
  (`exec_argv.rs:93`), `exec_argv_timeout_escalates_to_sigkill_when_the_child_ignores_sigterm`
  (`:128`) and `exec_argv_cancel_escalates_to_sigkill_when_the_child_ignores_sigterm` (`:162`).
  They are deliberately NOT enumerated in the comment: "the fixtures in `exec_argv.rs` that use
  this helper" stays true when a fourth is added, and is resolvable in one grep
  (`SleeperMarker::new`), whereas a hard list would be the next thing to go stale.
- **A filename, not an intra-doc link, on purpose.** This whole module is behind `#[cfg(test)]`
  (`ops/local/mod.rs:32-33`), so rustdoc never compiles it and a `` [`exec_argv`] `` link would be
  decorative and permanently unverified. A backticked filename matches how the neighbouring test
  docs already cite code.
- The count "three" is preserved and is still correct (three `SleeperMarker::new` call sites, all
  in `exec_argv.rs`); `exec_argv_kill_signals_only_the_single_pid_never_the_process_group`, named
  by hand one sentence earlier, is not one of them and stays untouched.
- Same three-line shape, longest new line 97 columns (the file's current maximum is 99).

## Sweep: everything else in `ops/local/` is intra-file and correct

`grep -rniE '\b(below|above|earlier in this file|later in this file|further (down|up)|the loop that follows|the following|shown below|listed above|top of|bottom of)\b'` over
`crates/cyrup-tools/src/ops/local/` returns 22 hits. Every one other than the two fixed above was
checked against its referent and resolves inside its own file — do NOT "fix" them:

- `proc.rs:103` "the cwd check below" → `proc.rs:112`. `proc.rs:105` "the mid-run cancel branch
  below" → the `exec` `select!` at `:177`. Both same file, both still below.
- `proc.rs:244, 283, 287, 289, 290, 294, 324, 326, 327, 335, 337, 345, 399` — all within `exec` /
  `exec_argv`, and the "identical loop/block above" pairs point from `exec_argv` back to `exec`,
  which is above it in the same file. Correct.
- `command.rs:89` "see the doc comment above" → `build_argv_command`'s own doc comment, directly
  above it in `command.rs`. Correct.
- `tests/exec.rs:32`, `tests/exec_argv.rs:231, 363, 400`, `tests/tracking.rs:118` — all
  intra-function references inside a single test body. Correct.
- `tests/mod.rs:58` "some fd above 2" — a numeric fd comparison, not a location. Correct.

Two further hits are *hypothetical-alternative* phrasings, not locations, and are deliberately
out of scope: `tests/tracking.rs:120-121` ("not a statement after the `select!` loop that a dropped
future never reaches") reads as a description of the rejected design inside an assertion message
and has no file-position claim. Leave it.

## Do not touch

- **`guard.rs:30`** — "Declared AFTER `child` in `exec` so Rust's reverse-declaration drop order
  runs this guard while that ownership still holds." This is correct and
  [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  explicitly marks it **"Do not touch"** as the correct exemplar it contrasts against. Do not
  reword or re-wrap it while editing lines 17-18 and 35 of the same comment.
- **`guard.rs:27`** — "the guard is disarmed only AFTER the loop has observed `child.wait()`".
  Edit 1 supplies its antecedent; no edit needed.
- **Any `.rs` file outside `crates/cyrup-tools/src/ops/local/`.** The dead-path citations to
  `ops/local.rs` in `src/tests/no_inherited_harness_stdio.rs:52, 178, 182` belong to
  [`LOW-sleepermarker-citation-still-points-at-ops-local-rs.md`](./LOW-sleepermarker-citation-still-points-at-ops-local-rs.md).
  The remaining `ops/local.rs` citations (`ops/shell.rs:430`, `tools/find.rs:164`,
  `tools/edit.rs:237-238`, `tools/read.rs:134`, `tools/bash.rs:288`) are the same dead-path class
  and are **not** claimed by this task — do not fold them in here, and do not fix them silently.
- **`signal.rs`'s module doc** — owned by
  [`LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md`](./LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md).
- No `cargo fmt` run, workspace-wide or otherwise. These are comment-only edits and rustfmt does
  not reformat comments; a blanket run is what
  [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  is already about.

## Definition of done

1. `crates/cyrup-tools/src/ops/local/guard.rs` contains the Edit 1 and Edit 2 text verbatim.
2. `crates/cyrup-tools/src/ops/local/tests/mod.rs` contains the Edit 3 text verbatim.
3. `grep -rn 'loop below\|fixtures below' crates/cyrup-tools/src/ops/local/` returns nothing.
4. `grep -rn 'select!' crates/cyrup-tools/src/ops/local/guard.rs` returns exactly two lines: the
   new line 18, which names where the loop lives (``[`LocalProc::exec`]`` / ``[`super::proc`]``),
   and line 35, which refers back to that naming (``that `select!` loop``).
5. `git diff --stat` for this task shows exactly two files changed — `guard.rs` `+4 −3` (edit 1 is
   `+3 −2`, edit 2 is `+1 −1`) and `tests/mod.rs` `+3 −3`, for a total of `+7 −6`. Confirm that no
   other file appears and that every changed line begins with `///`.
6. `git diff -U0 -- crates/cyrup-tools/src/ops/local/ | grep '^[+-]' | grep -v '^[+-][+-]'` shows
   only `///` comment lines — no code line, no `#[` attribute, no blank-line churn.
7. All four numbered steps of the `KillTreeOnDrop` argument (JS-settles → Rust-can-drop →
   `Drop`-is-the-slot → `kill_on_drop`-is-not-a-substitute) are still present and in order, and
   the `DRIFT-043` / `docs/gap-analysis/12-upstream-drift-pi-core.md` citation at `guard.rs:23-24`
   is untouched.
8. Not required and explicitly out of scope for this task: `cargo test`, `cargo check`,
   `cargo clippy`, `cargo doc`, `cargo fmt`.

## Files

- [`crates/cyrup-tools/src/ops/local/guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs) — edits 1 and 2
- [`crates/cyrup-tools/src/ops/local/tests/mod.rs`](../../crates/cyrup-tools/src/ops/local/tests/mod.rs) — edit 3
- [`crates/cyrup-tools/src/ops/local/proc.rs`](../../crates/cyrup-tools/src/ops/local/proc.rs) — referent only, read-only
- [`crates/cyrup-tools/src/ops/local/tests/exec_argv.rs`](../../crates/cyrup-tools/src/ops/local/tests/exec_argv.rs) — referent only, read-only
