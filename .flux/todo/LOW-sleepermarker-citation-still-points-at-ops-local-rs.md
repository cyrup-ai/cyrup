---
title: Sleepermarker Citation Still Points At Ops Local Rs
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:49
---

# Retarget every `ops/local.rs` citation in the workspace at the submodule that now holds the thing

## Problem

This branch deleted `crates/cyrup-tools/src/ops/local.rs` (1,912 lines) and split it into
`crates/cyrup-tools/src/ops/local/`. The path no longer exists. **16 textual citations of it survive
across 8 source files in 4 crates**, every one of them prose that the branch did not touch and did
not need to touch — it broke them by deleting the file they name.

A reader who follows any of them lands on nothing. The originating report was the narrowest instance
(the forked-sleeper rule's doc pointing at `ops/local.rs` for `SleeperMarker`, which is now
`ops/local/tests/mod.rs:81`). Three separate reviewers filed three overlapping slices of the same
class. **This task is the collection point: one pass, all 16, no trail.**

Nothing behavioural is at stake. The `fixture_scripts_that_fork_a_sleeper_record_its_pid_so_the_fixture_can_reap_it`
rule still fires correctly; `SleeperMarker` still exists; the `ops::local` Rust module path is still
valid and every `pub use` in `ops/local/mod.rs` still resolves. Only the file paths written in prose
are dead.

## Where things went

Verified against HEAD, not assumed:

| construct cited | now lives in |
|---|---|
| `build_command`, `build_argv_command`, `cmd.current_dir` | `crates/cyrup-tools/src/ops/local/command.rs` |
| `LocalFs::access`, `LocalFs::walk`, `write_in_place` | `crates/cyrup-tools/src/ops/local/fs.rs` |
| `LocalProc::exec`, `LocalProc::exec_argv`, the SIGTERM/grace/SIGKILL loop | `crates/cyrup-tools/src/ops/local/proc.rs` |
| `kill_process_tree`, `send_sigkill_tree`, `terminate_pid`, `kill_pid` | `crates/cyrup-tools/src/ops/local/signal.rs` |
| `TRACKED_DETACHED_CHILD_PIDS`, `kill_tracked_detached_children` | `crates/cyrup-tools/src/ops/local/tracking.rs` |
| `KillTreeOnDrop` | `crates/cyrup-tools/src/ops/local/guard.rs` |
| `SleeperMarker`, `pid_exists`, `exec_spec`, `argv` | `crates/cyrup-tools/src/ops/local/tests/mod.rs` |
| the `echo $!` fixtures | `crates/cyrup-tools/src/ops/local/tests/{exec,exec_argv,tracking}.rs` |

## The required path — one rule, applied to all 14 sites

**Replace the dead file token with the exact `ops/local/<file>.rs` that holds the cited construct,
preserving whatever path prefix the site already writes, and DELETE any `:NNN` line suffix that
pointed into the old file — the surrounding prose names the item instead.**

Concretely, three mechanical operations:

1. `ops/local.rs` → `ops/local/<file>.rs`; `cyrup-tools/src/ops/local.rs` →
   `cyrup-tools/src/ops/local/<file>.rs`; `crates/cyrup-tools/src/ops/local.rs` →
   `crates/cyrup-tools/src/ops/local/<file>.rs`. Do not add or remove a crate prefix that is
   already there or already absent — that is a separate editorial question and not this task's.
2. Every absolute line number that pointed into `ops/local.rs` is dropped. Where the number carried
   real information (exactly one site does: the 16-line span at `no_inherited_harness_stdio.rs:52`),
   restate it as a **count**, which cannot drift.
3. **No new line number into `ops/local/**` is introduced.** Not at any site, not "corrected", not
   "re-derived".

Rule 3 is not fastidiousness, it is the measured failure mode — see the next section.

## Judgement calls, answered

### 1. The line numbers were already stale at the merge base. Do they get fixed anyway?

**Answered: they get DELETED, not recomputed — and that decision is what rule 3 encodes.**

All five line-numbered citations into `ops/local.rs` were checked against `4902cddf`'s copy of the
file. **Five out of five were already wrong before this branch existed:**

| citation | claimed line at merge base | what was actually there | where the construct really was | drift |
|---|---|---|---|---|
| `no_inherited_harness_stdio.rs:52` → `:275` | `Command::new` in `build_command` | `}` | `:318` | −43 |
| `no_inherited_harness_stdio.rs:52` → `:290` | `.stderr(...)` | a doc line about Pi's grace timer | `:333` | −43 |
| `read.rs:134` → `:113` | `LocalFs::access`'s message | `path: &Path,` (a param of another fn) | `:189` | −76 |
| `spawn/signal.rs:346` → `:898,904` | the grace-wait gate | `child.stderr.take()` / `match timeout` | `:941,943` (and `:947,949`) | −43/−39 |
| `spawn/signal.rs:454` → `:458-459` | `kill_process_tree`'s `not(unix)` arm | `drain_and_kill`'s doc comment | `:496-506` | −38 |

The uniform negative drift is the tell: each pair was accurate when written and then walked as
`local.rs` grew above it. Nothing in the repo catches this — there is no citation-checking test
(`crates/cyrup-tools/src/tests/` has source-scanning rules for stdio pinning and fixture reaping,
none for doc paths), and no `CLAUDE.md`.

So: re-deriving five fresh numbers reinstalls a mechanism with a demonstrated 100% failure rate.
Naming the item instead is checkable in one `grep -rn` and survives the file growing. **These are
therefore in scope — the numbers go — but they are NOT "fixed" in the sense of being made to point
somewhere new.** They are removed as a class, and the fix is the same edit that fixes the path, so
scoping them out would mean deliberately leaving a known-wrong number inside a line being retyped.

### 2. Bare mentions with no line number — are they "still accurate as a module reference"?

**Answered: no, none of them are, and all 11 are in scope — but the distinction is real and it
changes how much prose moves.**

`ops/local.rs` is written as a *file path*, not a Rust path. The Rust module reference is
`ops::local` / `cyrup_tools::ops::local`, and those are all still correct — `ops/local/mod.rs`
re-exports `LocalFs`, `LocalProc`, `kill_pid`, `kill_process_tree`, `terminate_pid`,
`kill_tracked_detached_children`, `track_detached_child_pid`, `untrack_detached_child_pid`. Two
sites (`spawn/signal.rs:345`, `:454`) write the Rust path AND a file parenthetical; the Rust half
is accurate and **must not be touched** — only the parenthetical is dead.

What separates the bare mentions from the numbered ones is the size of the edit, not whether they
are broken:

- **Cheap (9 sites)** — one path token, nothing else moves. Sites 2-8, 10, 11.
- **Forced prose change (2 sites)** — the retarget makes an adjacent clause self-refuting, so the
  clause moves with it. Sites 9 and 14; both are called out below with the reason.

No prose that is still accurate is churned. In particular the module-doc sentence in
`ops/local/mod.rs` already describes the split correctly and is not touched, and no `ops::local`
Rust path anywhere is rewritten.

### 3. `docs/` and `spec/`

- `spec/` — **zero hits.** Nothing to do.
- `docs/` — 35 hits across 5 files: `gap-analysis/04-cyrup-tools.md` (21),
  `gap-analysis/08-cyrup-session-svc-and-modes.md` (7), `gap-analysis/12-upstream-drift-pi-core.md`
  (5), `gap-analysis/REPRO-LOG.md` (1), `PARITY-PLAN.md` (1).

**Recommendation: leave all 35 alone. Do not touch `docs/` in this task.** Every one of them is a
dated audit record that states the commit it was read at — the gap-analysis files say "Read
first-hand at cyrup HEAD `04c1ba2`", and `PARITY-PLAN.md` says "as regenerated 2026-08-12 ...
against cyrup HEAD `04c1ba2`". Their line numbers are evidence of what a specific auditor saw at a
specific tree; rewriting them destroys the audit trail and makes a claim about HEAD that the record
was never making. `PARITY-PLAN.md:764` is the same class — it is a batch-ordering argument
("two batches must not hold that file") that was true of the tree it was written against.

The one thing worth doing when those documents are next regenerated, and NOT here: the regeneration
pass re-reads at a new HEAD and will pick up the new paths naturally.

`.flux/todo/` and `.flux/done/` hits (5 files) are workflow records and are likewise not touched.

## The sites

14 sites, 16 occurrences, 8 files, 4 crates. Line numbers are HEAD as of this augmentation; if the
file has moved under you, match on the verbatim text.

| # | site | must point at | change |
|---|---|---|---|
| 1 | `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs:51-53` | `ops/local/command.rs` | path + drop 2 line numbers, span → count |
| 2 | `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs:178` | `ops/local/tests/mod.rs` | path only |
| 3 | `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs:182` | `ops/local/tests/` | path only |
| 4 | `crates/cyrup-tools/src/ops/shell.rs:430` | `ops/local/command.rs` + `ops/local/tests/` | 2 paths on one line |
| 5 | `crates/cyrup-tools/src/tools/find.rs:164` | `ops/local/fs.rs` | path only |
| 6 | `crates/cyrup-tools/src/tools/edit.rs:237,238` | `ops/local/fs.rs` | 2 paths |
| 7 | `crates/cyrup-tools/src/tools/read.rs:134` | `ops/local/fs.rs` | path + drop line number |
| 8 | `crates/cyrup-tools/src/tools/bash.rs:288` | `ops/local/proc.rs` | path only |
| 9 | `crates/cyrup/src/signals.rs:39-40` | `ops/local/tracking.rs` | path + forced prose fix |
| 10 | `crates/cyrup-ext-subagents/src/spawn/signal.rs:346` | `ops/local/proc.rs` | path + drop line numbers |
| 11 | `crates/cyrup-ext-subagents/src/spawn/signal.rs:454` | `ops/local/signal.rs` | path + drop line numbers |
| 12 | `crates/cyrup-session-svc/src/host_services.rs:50-52` | `.../ops/local/proc.rs` | path only |
| 13 | `crates/cyrup-session-svc/src/host_services.rs:1401` | `.../ops/local/proc.rs` | path only |
| 14 | `crates/cyrup-session-svc/src/host_services.rs:1416-1417` | `.../ops/local/command.rs` | path + forced prose fix |

### 1 — `no_inherited_harness_stdio.rs:51-53` (replace 3 lines with 3)

Before:
```
/// builders interleave long citation comments with the builder calls (`build_command` spans
/// `Command::new` at `ops/local.rs:275` to `.stderr(...)` at `:290`), and the point of the rule is
/// to catch a spawn that names NOTHING, not to police formatting.
```
After:
```
/// builders interleave long citation comments with the builder calls (`build_command` spans 16
/// lines from `Command::new` to `.stderr(...)` in `ops/local/command.rs`), and the point of the
/// rule is to catch a spawn that names NOTHING, not to police formatting.
```
The count is verified: `command.rs:15` (`Command::new`) to `:30` (`.stderr(...)`) is 16 lines
inclusive, the identical span the dead citation described. `build_argv_command`'s is 22
(`:65`–`:86`); both remain comfortably inside `WINDOW_LINES = 60`, so the constant's justification
is unchanged.

### 2 — `no_inherited_harness_stdio.rs:178` (1 → 1)

Before:
```
/// (`.config/nextest.toml:42`). See `ops/local.rs`'s `SleeperMarker` for the measurement.
```
After:
```
/// (`.config/nextest.toml:42`). See `ops/local/tests/mod.rs`'s `SleeperMarker` for the measurement.
```

### 3 — `no_inherited_harness_stdio.rs:182` (1 → 1)

Before:
```
/// `ops/local.rs` uses; requiring it is what stops a new fixture reintroducing the shape.
```
After:
```
/// `ops/local/tests/` uses; requiring it is what stops a new fixture reintroducing the shape.
```
Directory, not a file: the `echo $!` fixtures are spread over `tests/exec.rs:33`,
`tests/exec_argv.rs:311,372`, `tests/tracking.rs:147` and the `SleeperMarker` helper in
`tests/mod.rs:101`. "every already-correct fixture in `ops/local/tests/`" is the accurate claim.

### 4 — `ops/shell.rs:430` (1 → 1, two occurrences)

Before:
```
        // `ops/local.rs`, the `ops/local.rs` fixtures) already pins all three.
```
After:
```
        // `ops/local/command.rs`, the `ops/local/tests/` fixtures) already pins all three.
```

### 5 — `tools/find.rs:164` (1 → 1)

Before:
```
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local.rs).
```
After:
```
            // `LocalFs::walk`'s producer task breaks on the send error (ops/local/fs.rs).
```
The producer task and its `tx.blocking_send(item).is_err()` break are at `fs.rs:212` and `:236`.

### 6 — `tools/edit.rs:237-238` (2 → 2, one occurrence each)

Before:
```
        // (ops/local.rs) precisely so this arm can recover it; the access mode matches on both
        // sides (edit.ts:96 `R_OK | W_OK`, ops/local.rs `libc::R_OK | libc::W_OK`).
```
After:
```
        // (ops/local/fs.rs) precisely so this arm can recover it; the access mode matches on both
        // sides (edit.ts:96 `R_OK | W_OK`, ops/local/fs.rs `libc::R_OK | libc::W_OK`).
```
`libc::R_OK | libc::W_OK` is at `fs.rs:138`; the `error::io_errno` call is at `:149`.

### 7 — `tools/read.rs:134` (1 → 1)

Before:
```
        // `"{resolved path}: {io error}"` (ops/local.rs:113), so propagating is enough.
```
After:
```
        // `"{resolved path}: {io error}"` (ops/local/fs.rs), so propagating is enough.
```

### 8 — `tools/bash.rs:288` (1 → 1)

Before:
```
        // circuits before `getShellConfig` is ever reached). `LocalProc::exec` (ops/local.rs) still
```
After:
```
        // circuits before `getShellConfig` is ever reached). `LocalProc::exec` (ops/local/proc.rs) still
```
The defensive pre-spawn re-check this names is `proc.rs:110`.

### 9 — `cyrup/src/signals.rs:39-40` (2 → 2) — forced prose fix

Before:
```
//! `crates/cyrup-tools/src/ops/local.rs`, sitting beside the `setsid` and `killpg` primitives it
//! needs; `LocalProc::exec` enrolls its shell at spawn and — this is the
```
After:
```
//! `crates/cyrup-tools/src/ops/local/tracking.rs`, a sibling module of the `setsid` and `killpg`
//! primitives it needs; `LocalProc::exec` enrolls its shell at spawn and — this is the
```
**Why the wording moves.** "sitting beside" asserted same-file adjacency, which the split ended:
the registry is now `tracking.rs:34`, `setsid` is in `command.rs:45`, and `killpg` is in
`signal.rs:29`. They are siblings in one module directory, not neighbours in one file. Leaving
"sitting beside" while retargeting the path would state something the retargeted path itself
disproves. `ops/local/mod.rs`'s own module doc already describes the arrangement this way, so this
wording matches the crate's.

### 10 — `spawn/signal.rs:346` (1 → 2)

Before:
```
/// on the returned bool exactly this way (`ops/local.rs:898,904`).
```
After:
```
/// on the returned bool exactly this way (`LocalProc::exec_argv`'s cancel and timeout arms in
/// `ops/local/proc.rs`).
```
There are two such gates, not one — the cancel arm (`proc.rs:312,314`) and the timeout arm
(`:318,320`) — which is a second reason the numeric form was a poor fit. Line 347 is a bare `///`,
so the extra line does not cascade. **Leave `cyrup_tools::ops::local::terminate_pid` on line 345
exactly as it is:** that Rust path still resolves.

### 11 — `spawn/signal.rs:454` (1 → 1)

Before:
```
/// `cyrup_tools::ops::local::kill_process_tree`'s `not(unix)` arm (`ops/local.rs:458-459`).
```
After:
```
/// `cyrup_tools::ops::local::kill_process_tree`'s `not(unix)` arm (`ops/local/signal.rs`).
```
The arm is `signal.rs:36-48`, its `["/F", "/T", "/PID", …]` argv at `:42`. Again, the Rust path is
correct and stays.

### 12 — `host_services.rs:50-52` (3 → 3)

Before:
```
/// firing still goes through the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// (`cyrup-tools/src/ops/local.rs`) as an explicit guest timeout — not an ungraceful `kill_on_drop`
/// SIGKILL from abandoning the future outright.
```
After:
```
/// firing still goes through the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// (`cyrup-tools/src/ops/local/proc.rs`) as an explicit guest timeout — not an ungraceful
/// `kill_on_drop` SIGKILL from abandoning the future outright.
```
Re-wrapped only because the wrap is free here (line 52 ends the doc comment, nothing cascades).

### 13 — `host_services.rs:1401` (1 → 1)

Before:
```
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local.rs`). Deliberately does NOT honor a
```
After:
```
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local/proc.rs`). Deliberately does NOT honor a
```

### 14 — `host_services.rs:1416-1417` (2 → 3) — forced prose fix

Before:
```
        // than reaching `LocalProc::exec_argv`'s unconditional `cmd.current_dir(cwd)`
        // (`cyrup-tools/src/ops/local.rs`) with an empty path and hard-failing the spawn.
```
After:
```
        // than relying on `build_argv_command`'s defense-in-depth empty-`cwd` skip
        // (`cyrup-tools/src/ops/local/command.rs`), which is what stops an empty path
        // reaching `cmd.current_dir(cwd)` and hard-failing the spawn.
```
**Why the wording moves.** `cmd.current_dir(cwd)` is not unconditional and has not been since
before the merge base: `build_argv_command` guards it with `if !spec.cwd.as_os_str().is_empty()`
(`command.rs:77`), and the ten-line comment above that guard (`command.rs:67-76`) names
`cyrup-session-svc::host_services::exec` as the caller whose fold makes the guard defense in depth.
The two comments are a matched pair and one of them has the wrong word. That was already true at
`4902cddf`, so it is not this branch's defect — but the retarget sends the reader to the file where
the guard is three lines from the cited call, which turns a quiet inaccuracy into a citation that
refutes itself on arrival. Fixing the path without fixing the word is not an option; both move.
Line 1418 is code, so the extra line does not cascade.

## Line width

Two of the replacements exceed 100 columns: site 8 at 105 and site 13 at 106 (both were at 100/101
before). **Leave them.** There is no `rustfmt.toml` in the workspace, so `max_width` is the default
100 and `wrap_comments` is off — rustfmt does not reflow comments and no gate flags these. Both
files already carry longer comment lines: `host_services.rs` has 280 lines over 100 columns (max
144), including `:1399`, `:1403`, `:1405` and `:1410` inside this very paragraph; `bash.rs` has 22
(max 115). Re-wrapping either paragraph to hold 100 cascades through five to eighteen lines of
densely packed prose for no reviewable gain, and would collide with the separate rustfmt task,
which is about code lines rustfmt actually rewrites. Everywhere else a wrap was free it was taken
(sites 1, 10, 12, 14).

## Deliberately out of scope

Seen, judged, deliberately not done:

- **All 35 `docs/` hits.** Dated audit records. See judgement call 3.
- **`.flux/todo/*.md` and `.flux/done/*.md` hits.** Workflow records, and outside this task's write
  permission anyway.
- **`read.rs:134`'s quoted message shape.** It says `LocalFs::access` builds
  `"{resolved path}: {io error}"`, but `error::io_errno` (`error.rs:78-83`) builds
  `"{code}: {context}: {display}"` — the leading errno token is missing from the quote. Pre-existing
  at the merge base (`error.rs` is essentially untouched by this branch), and the surrounding
  sentences at `read.rs:126-133` separately and correctly state that the propagated text carries
  both the errno CODE and the resolved path, so no reader is misled about behaviour. Not filed; fix
  it if you are already editing that paragraph for another reason.
- **`.flux/todo/LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md`** and
  **`LOW-guard-doc-points-at-a-select-loop-below-that-is-not-there.md`** cover doc defects inside
  `ops/local/signal.rs` and `ops/local/guard.rs`. Neither contains the string `local.rs`, so there
  is no overlap with any of the 14 sites and no ordering constraint between the tasks.
- **Re-deriving correct line numbers for any of the five numeric citations.** See judgement call 1.

## Definition of done

1. `grep -rn 'local\.rs' crates/` returns **zero** hits.
2. `grep -rn 'ops/local\.rs' . --exclude-dir=target --exclude-dir=.git` returns hits **only** under
   `docs/` and `.flux/`.
3. Every path written by the change resolves to a file that exists: `ops/local/command.rs`,
   `ops/local/fs.rs`, `ops/local/proc.rs`, `ops/local/signal.rs`, `ops/local/tracking.rs`,
   `ops/local/tests/mod.rs`, and the directory `ops/local/tests/`. Verify with `ls`, one command.
4. **No line number anywhere in the workspace points into `ops/local/**`.** After the change,
   `grep -rn 'ops/local/[a-z_]*\.rs:[0-9]' crates/` returns zero hits.
5. `git diff` for this change touches **comment and doc-comment lines only** — no statement, no
   signature, no attribute, no `use`. Confirm by eye; the diff is ~20 lines across 8 files.
6. The two forced prose fixes (sites 9 and 14) are present, and no other prose is reworded.
7. `cyrup_tools::ops::local::terminate_pid` and `cyrup_tools::ops::local::kill_process_tree` in
   `spawn/signal.rs:345,454` are unchanged — only their file parentheticals moved.

## How this was verified

- Merge base `4902cddf8ce7d4723e41b4a7bf652361a584f905`'s `crates/cyrup-tools/src/ops/local.rs`
  extracted with `git show` (1,912 lines) and every cited line number read out of it directly.
- `git diff MERGE_BASE..HEAD -- <file> | grep '^[-+].*local\.rs'` is **empty for all 8 source
  files**: not one citation line was added or modified by this branch. Every one is pre-existing
  text the deletion invalidated.
- Current homes located by `grep -n` in each `ops/local/*.rs`, and every replacement line measured
  against 100 columns before being written into this spec.
- No `cargo` invocation of any kind was run.
