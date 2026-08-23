---
title: Sleepermarker Citation Still Points At Ops Local Rs
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:20
---

# Retarget every `ops/local.rs` citation under `crates/` at the submodule that now holds the thing

## Problem

`crates/cyrup-tools/src/ops/local.rs` no longer exists. It was split into the directory
`crates/cyrup-tools/src/ops/local/` (`command.rs`, `fs.rs`, `guard.rs`, `mod.rs`, `proc.rs`,
`signal.rs`, `tracking.rs`, `tests/`). **16 textual citations of the dead path survive across 9
source files in 4 crates** — every one of them prose that the split did not touch and did not need
to touch; it broke them by deleting the file they name.

A reader who follows any of them lands on nothing. The originating report was the narrowest instance
(the forked-sleeper rule's doc pointing at `ops/local.rs` for `SleeperMarker`, which now lives in
`ops/local/tests/mod.rs`). Three separate reviewers filed three overlapping slices of the same
class. **This task is the collection point: one pass, all 16, no trail.**

Nothing behavioural is at stake. The
`fixture_scripts_that_fork_a_sleeper_record_its_pid_so_the_fixture_can_reap_it` rule still fires
correctly; `SleeperMarker` still exists; the `ops::local` **Rust** module path is still valid and
every `pub use` in `ops/local/mod.rs` still resolves. Only the file paths written in prose are dead.

## Re-verification, 2026-08-23

Everything below was re-checked against the tree on disk at the time of this augmentation. Two
claims from the previous augmentation were **wrong and have been corrected**; they are called out
inline so nobody re-derives them from the stale version:

| claim in the previous pass | truth on disk now |
|---|---|
| `docs/` has 35 hits (21/7/5/1/1) | **32** hits of `local\.rs` in 5 files (20/6/4/1/1); **28** of them are the exact string `ops/local.rs`. Disposition is unchanged: leave all of them. |
| `.flux/todo/` + `.flux/done/` hits are 5 files | **6** files, plus `.flux/SCOPE.md` = 7 under `.flux/`. Disposition unchanged: not touched. |
| the two sibling doc tasks "contain no `local.rs` string" | Both **do** mention `ops/local.rs` in their own prose. That is irrelevant — see *Out of scope*, where the real (unchanged) conclusion is restated correctly. |

All 14 anchor strings below were re-read from disk and each was confirmed to occur **exactly once**
in its file. All target files and directories were confirmed to exist. No line number in this spec
is used as an edit anchor; every anchor is verbatim text.

## Where things went — by name, not by line

| construct cited | now lives in |
|---|---|
| `build_command`, `build_argv_command`, the `cmd.current_dir` guard, `setsid` | `crates/cyrup-tools/src/ops/local/command.rs` |
| `LocalFs::access`, `LocalFs::walk`, `write_in_place` | `crates/cyrup-tools/src/ops/local/fs.rs` |
| `LocalProc::exec`, `LocalProc::exec_argv`, the SIGTERM/grace/SIGKILL loop | `crates/cyrup-tools/src/ops/local/proc.rs` |
| `kill_process_tree`, `send_sigkill_tree`, `terminate_pid`, `kill_pid`, `killpg` | `crates/cyrup-tools/src/ops/local/signal.rs` |
| `TRACKED_DETACHED_CHILD_PIDS`, `kill_tracked_detached_children` | `crates/cyrup-tools/src/ops/local/tracking.rs` |
| `KillTreeOnDrop` | `crates/cyrup-tools/src/ops/local/guard.rs` |
| `SleeperMarker`, `pid_exists`, `exec_spec`, `argv` | `crates/cyrup-tools/src/ops/local/tests/mod.rs` |
| the `echo $!` fixtures | `crates/cyrup-tools/src/ops/local/tests/` (`exec.rs`, `exec_argv.rs`, `tracking.rs`, and the `SleeperMarker` script builder in `mod.rs`) |

## The required rule

**Replace the dead file token with the exact `ops/local/<file>.rs` that holds the cited construct,
preserving whatever path prefix the site already writes, and DELETE any `:NNN` line suffix that
pointed into the old file — the surrounding prose names the item instead.**

Three mechanical consequences:

1. `ops/local.rs` → `ops/local/<file>.rs`; `cyrup-tools/src/ops/local.rs` →
   `cyrup-tools/src/ops/local/<file>.rs`; `crates/cyrup-tools/src/ops/local.rs` →
   `crates/cyrup-tools/src/ops/local/<file>.rs`. **Do not add or remove a crate prefix that is
   already there or already absent** — that is a separate editorial question and not this task's.
2. Every absolute line number that pointed into `ops/local.rs` is dropped. Where the number carried
   real information (exactly one site: the span in `no_inherited_harness_stdio.rs`), restate it as a
   **count**, which does not drift when unrelated lines are inserted above it.
3. **No new line number into `ops/local/**` is introduced.** Not at any site, not "corrected", not
   "re-derived". There are currently **zero** such numbered citations under `crates/`; the change
   must keep it at zero.

Rule 3 is the required path, not a preference — see the next section.

## Judgement calls, answered

### 1. The line numbers were already stale before the split. Do they get fixed anyway?

**Answered: they get DELETED, not recomputed. That decision is what rule 3 encodes.**

All five line-numbered citations into `ops/local.rs` were checked against the pre-split copy of the
file in an earlier pass, and **five out of five were already wrong before the split existed** —
`no_inherited_harness_stdio.rs`'s `:275`/`:290`, `read.rs`'s `:113`, `spawn/signal.rs`'s `:898,904`
and `:458-459`. Each pointed 38–76 lines short of its construct. The uniform negative drift is the
tell: each pair was accurate when written and then walked as `local.rs` grew above it.

Nothing in the repo catches this. There is no citation-checking rule
(`crates/cyrup-tools/src/tests/` has source-scanning rules for stdio pinning and fixture reaping,
none for doc paths), there is no `CLAUDE.md` in the workspace, and this queue has itself shipped
three passes that shifted line numbers under other tasks' citations.

So: re-deriving five fresh numbers reinstalls a mechanism with a demonstrated 100% failure rate.
Naming the item instead is checkable with one `grep -rn` and survives the file growing. **The
numbers are therefore in scope — they go — but they are NOT "fixed" in the sense of being made to
point somewhere new.** They are removed as a class, and the removal is part of the same edit that
fixes the path, so scoping them out would mean deliberately retyping a line while leaving a
known-wrong number inside it.

### 2. Bare mentions with no line number — are they "still accurate as a module reference"?

**Answered: no, none of them are, and all of them are in scope.**

`ops/local.rs` is written as a *file path*, not a Rust path. The Rust module reference is
`ops::local` / `cyrup_tools::ops::local`, and those are all still correct — `ops/local/mod.rs`
re-exports `LocalFs`, `LocalProc`, `kill_pid`, `kill_process_tree`, `terminate_pid`, and the
`tracking` items. Two sites (edits 10 and 11, both in `spawn/signal.rs`) write the Rust path AND a
file parenthetical; **the Rust half is accurate and must not be touched** — only the parenthetical
is dead.

What separates the bare mentions from the numbered ones is the size of the edit, not whether they
are broken:

- **Cheap (12 of the 14)** — one or two path tokens; nothing else moves.
- **Forced prose change (2 of the 14)** — edits 9 and 14: the retarget makes an adjacent clause
  self-refuting, so the clause moves with it. Each is justified in place below.

No prose that is still accurate is churned. In particular the module doc in `ops/local/mod.rs`
already describes the split correctly and is not touched, and no `ops::local` Rust path anywhere is
rewritten.

### 3. `docs/` and `spec/`

- `spec/` — **zero hits.** Nothing to do.
- `docs/` — 32 hits of `local\.rs` across 5 files: `gap-analysis/04-cyrup-tools.md` (20),
  `gap-analysis/08-cyrup-session-svc-and-modes.md` (6), `gap-analysis/12-upstream-drift-pi-core.md`
  (4), `gap-analysis/REPRO-LOG.md` (1), `PARITY-PLAN.md` (1).

**Required: leave all 32 alone. Do not touch `docs/` in this task.** Every one of them is a dated
audit record that states the commit it was read at — the gap-analysis files say "Read first-hand at
cyrup HEAD `04c1ba2`", and `PARITY-PLAN.md` says "as regenerated 2026-08-12 ... against cyrup HEAD
`04c1ba2`". Their line numbers are evidence of what a specific auditor saw at a specific tree;
rewriting them destroys the audit trail and makes a claim about the current tree that the record was
never making. `PARITY-PLAN.md`'s hit is the same class — a batch-ordering argument ("two batches
must not hold that file") that was true of the tree it was written against. When those documents are
next regenerated they will be re-read at a new HEAD and will pick up the new paths naturally.

`.flux/` hits (7 files: `SCOPE.md`, one under `.flux/done/`, five under `.flux/todo/` including this
one) are workflow records and are likewise not touched.

## The 14 edits

Each edit is a literal replacement. **Match on the verbatim "Find" text — do not use line numbers to
locate it.** Every "Find" block was confirmed to occur exactly once in its file at augmentation
time; before applying an edit, assert the match count is 1 and stop if it is not.

Where a multi-line block is shown, the whole block is one contiguous match including the newlines
between its lines.

---

### 1 — `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs` (3 lines → 3)

Find (occurs once):
```
/// builders interleave long citation comments with the builder calls (`build_command` spans
/// `Command::new` at `ops/local.rs:275` to `.stderr(...)` at `:290`), and the point of the rule is
/// to catch a spawn that names NOTHING, not to police formatting.
```
Replace with:
```
/// builders interleave long citation comments with the builder calls (`build_command` spans 16
/// lines from `Command::new` to `.stderr(...)` in `ops/local/command.rs`), and the point of the
/// rule is to catch a spawn that names NOTHING, not to police formatting.
```
The count is re-verified on disk: in `ops/local/command.rs`, `build_command`'s
`std::process::Command::new(&spec.shell.program)` through its `std_cmd.stderr(...)` is a 16-line
span inclusive — the identical span the dead citation described. `build_argv_command`'s equivalent
span is 22 lines. Both remain comfortably inside `WINDOW_LINES = 60`, so the constant's
justification in this very doc comment is unchanged.

---

### 2 — `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs` (1 → 1)

Find (occurs once):
```
/// (`.config/nextest.toml:42`). See `ops/local.rs`'s `SleeperMarker` for the measurement.
```
Replace with:
```
/// (`.config/nextest.toml:42`). See `ops/local/tests/mod.rs`'s `SleeperMarker` for the measurement.
```
`SleeperMarker` is confirmed to be declared in `ops/local/tests/mod.rs`. (The `.config/nextest.toml`
citation is outside this task's rule and is left exactly as written.)

---

### 3 — `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs` (1 → 1)

Find (occurs once):
```
/// `ops/local.rs` uses; requiring it is what stops a new fixture reintroducing the shape.
```
Replace with:
```
/// `ops/local/tests/` uses; requiring it is what stops a new fixture reintroducing the shape.
```
A directory, not a file, and that is deliberate: the `echo $!` fixtures are spread across
`ops/local/tests/exec.rs`, `ops/local/tests/exec_argv.rs` (two of them),
`ops/local/tests/tracking.rs`, and the `SleeperMarker` script builder in `ops/local/tests/mod.rs`.
"every already-correct fixture in `ops/local/tests/`" is the accurate claim; no single file is.

---

### 4 — `crates/cyrup-tools/src/ops/shell.rs` (1 → 1, two occurrences on the line)

Find (occurs once):
```
        // `ops/local.rs`, the `ops/local.rs` fixtures) already pins all three.
```
Replace with:
```
        // `ops/local/command.rs`, the `ops/local/tests/` fixtures) already pins all three.
```
The preceding line names `build_command`/`build_argv_command`, which are in `command.rs`; the
fixtures are the `ops/local/tests/` ones from edit 3.

---

### 5 — `crates/cyrup-tools/src/tools/find.rs` (1 → 1)

Find (occurs once):
```
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local.rs).
```
Replace with:
```
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local/fs.rs).
```
`LocalFs::walk`, its `spawn_blocking` producer, and the `tx.blocking_send(item).is_err()` break are
all confirmed present in `ops/local/fs.rs`.

---

### 6 — `crates/cyrup-tools/src/tools/edit.rs` (2 lines → 2)

Find (occurs once):
```
        // (ops/local.rs) precisely so this arm can recover it; the access mode matches on both
        // sides (edit.ts:96 `R_OK | W_OK`, ops/local.rs `libc::R_OK | libc::W_OK`).
```
Replace with:
```
        // (ops/local/fs.rs) precisely so this arm can recover it; the access mode matches on both
        // sides (edit.ts:96 `R_OK | W_OK`, ops/local/fs.rs `libc::R_OK | libc::W_OK`).
```
Both claims re-verified in `ops/local/fs.rs`: `LocalFs::access`'s `Access::ReadWrite` arm evaluates
`libc::R_OK | libc::W_OK`, and the failure path calls `error::io_errno`.

---

### 7 — `crates/cyrup-tools/src/tools/read.rs` (1 → 1)

Find (occurs once):
```
        // `"{resolved path}: {io error}"` (ops/local.rs:113), so propagating is enough.
```
Replace with:
```
        // `"{resolved path}: {io error}"` (ops/local/fs.rs), so propagating is enough.
```
Path retargeted and the stale `:113` dropped. The quoted message shape is separately imprecise —
see *Out of scope*; **do not change it here.**

---

### 8 — `crates/cyrup-tools/src/tools/bash.rs` (1 → 1)

Find (occurs once):
```
        // circuits before `getShellConfig` is ever reached). `LocalProc::exec` (ops/local.rs) still
```
Replace with:
```
        // circuits before `getShellConfig` is ever reached). `LocalProc::exec` (ops/local/proc.rs) still
```
The defensive pre-spawn `cancel.is_cancelled()` re-check this sentence names is confirmed inside
`LocalProc::exec` in `ops/local/proc.rs`. The replacement line is 105 columns — intended, see
*Line width*.

---

### 9 — `crates/cyrup/src/signals.rs` (2 lines → 2) — forced prose fix

Find (occurs once):
```
//! `crates/cyrup-tools/src/ops/local.rs`, sitting beside the `setsid` and `killpg` primitives it
//! needs; `LocalProc::exec` enrolls its shell at spawn and — this is the
```
Replace with:
```
//! `crates/cyrup-tools/src/ops/local/tracking.rs`, a sibling module of the `setsid` and `killpg`
//! primitives it needs; `LocalProc::exec` enrolls its shell at spawn and — this is the
```
**Why the wording must move.** "sitting beside" asserted same-file adjacency, which the split ended.
Re-verified on disk: `TRACKED_DETACHED_CHILD_PIDS` is declared in `ops/local/tracking.rs`, the
`libc::setsid()` call is in `ops/local/command.rs`, and the `libc::killpg` calls are in
`ops/local/signal.rs`. They are siblings in one module directory, not neighbours in one file.
Retargeting the path while leaving "sitting beside" would state something the retargeted path itself
disproves. `ops/local/mod.rs`'s own module doc already describes the arrangement as a split by
concern, so this wording matches the crate's.

---

### 10 — `crates/cyrup-ext-subagents/src/spawn/signal.rs` (1 line → 2)

Find (occurs once):
```
/// on the returned bool exactly this way (`ops/local.rs:898,904`).
```
Replace with:
```
/// on the returned bool exactly this way (`LocalProc::exec_argv`'s cancel and timeout arms in
/// `ops/local/proc.rs`).
```
There are **two** such gates, not one — the `cancel.cancelled()` arm and the `timeout_fut` arm of
`exec_argv`'s `tokio::select!`, each pairing a `terminate_pid(...)` call with a
`let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };`. That plurality is a
second reason the numeric form was a poor fit. The line immediately after the anchor is a bare
`///`, so growing this from one line to two cascades nothing.

**Leave the `cyrup_tools::ops::local::terminate_pid` Rust path on the preceding line exactly as it
is** — it still resolves.

---

### 11 — `crates/cyrup-ext-subagents/src/spawn/signal.rs` (1 → 1)

Find (occurs once):
```
/// `cyrup_tools::ops::local::kill_process_tree`'s `not(unix)` arm (`ops/local.rs:458-459`).
```
Replace with:
```
/// `cyrup_tools::ops::local::kill_process_tree`'s `not(unix)` arm (`ops/local/signal.rs`).
```
Re-verified: `kill_process_tree`'s `#[cfg(not(unix))]` block, with its
`.args(["/F", "/T", "/PID", &pid.to_string()])`, is in `ops/local/signal.rs`. The Rust path is
correct and stays.

---

### 12 — `crates/cyrup-session-svc/src/host_services.rs` (3 lines → 3)

Find (occurs once):
```
/// firing still goes through the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// (`cyrup-tools/src/ops/local.rs`) as an explicit guest timeout — not an ungraceful `kill_on_drop`
/// SIGKILL from abandoning the future outright.
```
Replace with:
```
/// firing still goes through the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// (`cyrup-tools/src/ops/local/proc.rs`) as an explicit guest timeout — not an ungraceful
/// `kill_on_drop` SIGKILL from abandoning the future outright.
```
Re-wrapped because the wrap is free here: the third line ends the doc comment and the next line is
`const DEFAULT_EXEC_TIMEOUT`, so nothing cascades. All three replacement lines are under 100
columns.

---

### 13 — `crates/cyrup-session-svc/src/host_services.rs` (1 → 1)

Find (occurs once):
```
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local.rs`). Deliberately does NOT honor a
```
Replace with:
```
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local/proc.rs`). Deliberately does NOT honor a
```
The replacement line is 106 columns — intended, see *Line width*.

---

### 14 — `crates/cyrup-session-svc/src/host_services.rs` (2 lines → 3) — forced prose fix

Find (occurs once):
```
        // than reaching `LocalProc::exec_argv`'s unconditional `cmd.current_dir(cwd)`
        // (`cyrup-tools/src/ops/local.rs`) with an empty path and hard-failing the spawn.
```
Replace with:
```
        // than relying on `build_argv_command`'s defense-in-depth empty-`cwd` skip
        // (`cyrup-tools/src/ops/local/command.rs`), which is what stops an empty path
        // reaching `cmd.current_dir(cwd)` and hard-failing the spawn.
```
**Why the wording must move.** `cmd.current_dir(cwd)` is not unconditional, and has not been since
before the split. Re-verified on disk: in `ops/local/command.rs`, `build_argv_command` guards the
call with `if !spec.cwd.as_os_str().is_empty()`, and the comment directly above that guard names
`cyrup-session-svc::host_services::exec` as the caller whose fold makes the guard defense in depth.
The two comments are a matched pair and one of them has the wrong word. That was already true before
the split, so it is not the split's defect — but the retarget sends the reader to the file where the
guard sits three lines from the cited call, which turns a quiet inaccuracy into a citation that
refutes itself on arrival. Fixing the path without fixing the word is not an option; both move. The
line after the anchor is code (`let cwd = opts`), so the extra line cascades nothing.

## Line width

Two replacements exceed 100 columns: edit 8 at 105 and edit 13 at 106 (they were at 100 and 101
before). **Leave them.** There is no `rustfmt.toml` or `.rustfmt.toml` anywhere in the workspace
(confirmed), so `max_width` is the default 100 and `wrap_comments` is off — rustfmt does not reflow
comments and no gate flags these. Both files already carry longer comment lines: `host_services.rs`
has 280 lines over 100 columns (longest 144), including four inside this very paragraph; `bash.rs`
has 22 (longest 115). Re-wrapping either paragraph to hold 100 cascades through five to eighteen
lines of densely packed prose for no reviewable gain, and would collide with the separate rustfmt
task, which is about code lines rustfmt actually rewrites. Everywhere a wrap was free it was taken
(edits 1, 10, 12, 14); all other replacement lines are ≤100.

## Deliberately out of scope

Seen, judged, deliberately not done:

- **All 32 `docs/` hits.** Dated audit records. See judgement call 3.
- **All `.flux/` hits** (`SCOPE.md`, `.flux/done/**`, `.flux/todo/**`). Workflow records, and
  outside this task's write permission anyway. This includes the sibling tasks' own prose mentions
  of `ops/local.rs` — those are descriptions of the split, not citations that a reader follows.
- **`read.rs`'s quoted message shape** (edit 7). It says `LocalFs::access` builds
  `"{resolved path}: {io error}"`, but `error::io_errno` in `crates/cyrup-tools/src/error.rs` builds
  `"{code}: {context}: {display}"` — the leading errno token is missing from the quote. Pre-existing
  and unrelated to the split, and the sentences immediately above it separately and correctly state
  that the propagated text carries both the errno CODE and the resolved path, so no reader is misled
  about behaviour. Not filed; fix it only if you are already editing that paragraph for another
  reason.
- **`.flux/todo/LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md`** and
  **`.flux/todo/LOW-guard-doc-points-at-a-select-loop-below-that-is-not-there.md`.** Both cover doc
  defects *inside* `ops/local/signal.rs` and `ops/local/guard.rs`. Neither of those two files
  contains any of the 14 anchors above, so there is **no overlap and no ordering constraint** in
  either direction. (The previous augmentation justified this with a claim about the string
  `local.rs` that is factually wrong — both task files do mention it in their own prose. The
  conclusion stands for the reason given here instead.)
- **Re-deriving correct line numbers for any of the five numeric citations.** See judgement call 1.
- **Tests, benchmarks, and new documentation.** Another team owns those. This task adds none, and
  the definition of done below requires none.

## Definition of done

All checks are read-only commands run from the repo root. No test is written or run, no build is
run, and no `git` command is used at any point.

1. **No dead path left in code.**
   `grep -rn 'local\.rs' crates/` returns **zero** hits.
   (It returns exactly the 15 lines listed in this spec before the change.)

2. **The dead path survives only where it was deliberately left.**
   `grep -rn 'ops/local\.rs' . --exclude-dir=target --exclude-dir=.git` returns hits **only** under
   `docs/` and `.flux/`.

3. **Every path written by the change resolves.** One command:
   `ls crates/cyrup-tools/src/ops/local/command.rs crates/cyrup-tools/src/ops/local/fs.rs crates/cyrup-tools/src/ops/local/proc.rs crates/cyrup-tools/src/ops/local/signal.rs crates/cyrup-tools/src/ops/local/tracking.rs crates/cyrup-tools/src/ops/local/tests/mod.rs crates/cyrup-tools/src/ops/local/tests/`
   exits 0 with no "No such file" line.

4. **No line number anywhere points into `ops/local/**`.**
   `grep -rn 'ops/local/[a-z_]*\.rs:[0-9]' crates/` returns zero hits, and so does
   `grep -rn 'ops/local/tests/[a-z_]*\.rs:[0-9]' crates/`. Both return zero **before** the change
   too, so this is a "kept at zero" check, not a "brought to zero" one.

5. **All 14 replacements are present verbatim.** For each edit, the "Replace with" block occurs
   exactly once in its file and the "Find" block occurs zero times. Check with `grep -c` on a
   distinctive substring of each, e.g.
   `grep -c 'ops/local/tests/mod.rs' crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs`
   returns 1.

6. **Comments only.** Every changed line begins (after leading whitespace) with `//`, `///`, or
   `//!`. Confirm by eye on the 9 files: no statement, no signature, no attribute, no `use`, and no
   change to `WINDOW_LINES = 60` or any other constant.

7. **The two Rust paths in `crates/cyrup-ext-subagents/src/spawn/signal.rs` are untouched.**
   `grep -c 'cyrup_tools::ops::local::terminate_pid' crates/cyrup-ext-subagents/src/spawn/signal.rs`
   and `grep -c 'cyrup_tools::ops::local::kill_process_tree' crates/cyrup-ext-subagents/src/spawn/signal.rs`
   each still return their pre-change count; only the file parentheticals beside them moved.

8. **Only the two sanctioned prose changes happened.** Edits 9 and 14 carry their new wording;
   `grep -rn 'sitting beside' crates/cyrup/src/signals.rs` and
   `grep -rn "unconditional \`cmd.current_dir" crates/cyrup-session-svc/src/host_services.rs` both
   return zero. No other sentence in the 9 files is reworded.

9. **Nothing outside these 9 files changed.** The files touched are exactly:
   `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs`,
   `crates/cyrup-tools/src/ops/shell.rs`, `crates/cyrup-tools/src/tools/find.rs`,
   `crates/cyrup-tools/src/tools/edit.rs`, `crates/cyrup-tools/src/tools/read.rs`,
   `crates/cyrup-tools/src/tools/bash.rs`, `crates/cyrup/src/signals.rs`,
   `crates/cyrup-ext-subagents/src/spawn/signal.rs`,
   `crates/cyrup-session-svc/src/host_services.rs`.
   That is **9** files, in 4 crates. (Older prose in this task said "8 source files"; it
   miscounted — the list above is authoritative and was re-derived from the tree.)
