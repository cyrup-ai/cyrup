---
title: Guard Doc Points At A Select Loop Below That Is Not There
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:24
---

# Repoint the two `ops/local/` doc references that the `local.rs` split left dangling

Splitting `crates/cyrup-tools/src/ops/local.rs` into the twelve files now under
`crates/cyrup-tools/src/ops/local/` broke every doc reference that pointed at a location by file
POSITION rather than by NAME. Exactly two such references survive in the current tree:

| Site | Says | Referent actually lives in |
|---|---|---|
| [`ops/local/guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs), `KillTreeOnDrop`'s doc comment | "the `select!` loop below" | [`ops/local/proc.rs`](../../crates/cyrup-tools/src/ops/local/proc.rs), inside `<LocalProc as ProcOps>::exec` |
| [`ops/local/tests/mod.rs`](../../crates/cyrup-tools/src/ops/local/tests/mod.rs), `SleeperMarker`'s doc comment | "The three fixtures below" | [`ops/local/tests/exec_argv.rs`](../../crates/cyrup-tools/src/ops/local/tests/exec_argv.rs), the three `SleeperMarker::new` call sites |

This task fixes both, plus one one-character follow-on inside the same `guard.rs` doc comment.
Documentation-comment text only — no behaviour, no signature, no test, no new documentation.

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

Secondary effect worth knowing before editing: the later sentences "the guard is disarmed only
AFTER **the loop** has observed `child.wait()`" and "a statement placed after **the** `select!`
**loop**" are bare anaphors whose only antecedent in the file is the broken first mention. Fixing
the first mention by NAMING the loop restores the antecedent for both — which is why the
`child.wait()` sentence needs no edit of its own, and why the "statement placed after" sentence
only needs one word.

## Evidence (re-verified against the current working tree, 2026-08-23)

`select!` in `guard.rs` — two prose mentions, zero loops:

```
crates/cyrup-tools/src/ops/local/guard.rs:18: /// `select!` loop below without running a single one of its `send_sigkill_tree` arms.
crates/cyrup-tools/src/ops/local/guard.rs:35: /// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting
```

Where the loops actually are — `proc.rs` has exactly two, one per `ProcOps` method
(`async fn exec` opens at `proc.rs:92`, `async fn exec_argv` at `proc.rs:252`):

```
crates/cyrup-tools/src/ops/local/proc.rs:177:            tokio::select! {   # inside LocalProc::exec
crates/cyrup-tools/src/ops/local/proc.rs:301:            tokio::select! {   # inside LocalProc::exec_argv
```

The `SleeperMarker` doc (`tests/mod.rs:49`, struct at `tests/mod.rs:81`) and the fixtures it
points at — all three in `exec_argv.rs`, none in `mod.rs`:

```
crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:93   async fn exec_argv_timeout_preserves_the_real_code_of_a_graceful_sigterm_handler
                                                  :95     let sleeper = SleeperMarker::new("gracefulterm");
crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:128  async fn exec_argv_timeout_escalates_to_sigkill_when_the_child_ignores_sigterm
                                                  :130    let sleeper = SleeperMarker::new("argvtimeoutkill");
crates/cyrup-tools/src/ops/local/tests/exec_argv.rs:162  async fn exec_argv_cancel_escalates_to_sigkill_when_the_child_ignores_sigterm
                                                  :164    let sleeper = SleeperMarker::new("argvcancelkill");
```

`grep -c 'SleeperMarker::new' crates/cyrup-tools/src/ops/local/tests/mod.rs` is `0` — "below"
points past the end of the file. `reapable_sleep_loop` / `reap` are likewise called only from
those same three fixtures (`exec_argv.rs:100/117`, `:136/143`, `:171/178`).

Both references were accurate before the split, when `KillTreeOnDrop`, the `select!` loop, the
`SleeperMarker` doc and the three fixtures all shared one 1,912-line `ops/local.rs`. That history
is context only; nothing below depends on it, and it is not something to re-verify.

## The edits

Three exact replacements. Each target string below was checked against the file on disk and
occurs **exactly once**. Nothing else in either file changes.

### Edit 1 — `crates/cyrup-tools/src/ops/local/guard.rs` (`KillTreeOnDrop` doc, currently lines 17-18)

Replace this exact text (2 lines, match count **1**):

```rust
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon the
/// `select!` loop below without running a single one of its `send_sigkill_tree` arms.
```

with this exact text (3 lines):

```rust
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon
/// [`LocalProc::exec`]'s `select!` loop (in [`super::proc`]) without running a single one of its
/// `send_sigkill_tree` arms.
```

The preceding line (`/// call. A Rust future has no such guarantee — it can be dropped at ANY
`.await`: a cancelled`) is unchanged; the trailing `the` moves off the first line onto the new
second line, so the sentence still reads as one clause.

Why this exact form — this is the required wording, not one option among several:

- **``[`LocalProc::exec`]`` needs no new link target.** Its reference definition already sits at
  the bottom of this same doc comment (`guard.rs:42`,
  `` /// [`LocalProc::exec`]: crate::ops::ProcOps::exec ``), placed there for the "Declared AFTER
  `child` in `exec`" sentence. Reusing it is what makes the two cross-references consistent.
- **``[`super::proc`]`` is the module the loop moved to** and resolves from `guard.rs`
  (`super` == `crate::ops::local`; `proc` is declared `pub(crate) mod proc;` in
  `ops/local/mod.rs`). The same relative form is already used and proven in this very file:
  the module doc at `guard.rs:2` links `` [`super::tracking`] ``, and the `#[cfg(not(unix))]`
  doc at `guard.rs:103-104` links `pub(super)` items by full path.
- **No rustdoc lint can fire on it.** `KillTreeOnDrop` is `pub(super)`, so
  `rustdoc::private_intra_doc_links` (which only fires from PUBLIC items) does not apply. The
  workspace declares a `[workspace.lints.rustdoc]` table in the root `Cargo.toml`, but that table
  is **empty** — it sets no rustdoc lint level at all — and `crates/cyrup-tools/Cargo.toml`
  inherits it verbatim via `[lints] workspace = true`. `crates/cyrup-tools/src/lib.rs:16` denies
  `unsafe_code` and nothing else.
- **Naming the function AND the module is deliberate.** The module alone would not say which of
  `proc.rs`'s two `select!` loops is meant, and `exec_argv`'s loop (`proc.rs:301`) is explicitly
  NOT the one this guard protects: `build_argv_command` (`command.rs:64`) installs no `setsid`
  and the code says so at `command.rs:87-90` ("Deliberately NO `setsid`/process-group setup
  here … must be signaled by single pid only, never `killpg`"), so `KillTreeOnDrop` is never
  armed for `exec_argv`.
- Longest new line is **97** columns, inside the file's existing range — `guard.rs` already
  carries lines of 101 (×5), 102 and 104 columns.

### Edit 2 — `crates/cyrup-tools/src/ops/local/guard.rs` (same doc comment, currently line 35)

Replace this exact text (match count **1**):

```rust
/// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting
```

with this exact text:

```rust
/// "runs no matter how we leave" is `Drop`, not a statement placed after that `select!` loop. Putting
```

`the` → `that`, one character. This mention describes a hypothetical alternative implementation,
not a location, so it did not break outright — but after Edit 1 the doc names a specific loop, and
`that` binds this sentence to it instead of leaving a definite article a reader may still try to
resolve inside `guard.rs`. No re-wrap, no ripple into the sentences that follow. The line goes
from 101 to 102 columns, which is not a rustfmt concern: the repo has no `rustfmt.toml` /
`.rustfmt.toml` at the workspace root or in `crates/cyrup-tools/`, and rustfmt's `wrap_comments`
defaults to `false`, so comment width is never reformatted — which is why the surrounding
101/102/104-column comment lines already survive a clean `cargo fmt`.

### Edit 3 — `crates/cyrup-tools/src/ops/local/tests/mod.rs` (`SleeperMarker` doc, currently lines 49-51)

Replace this exact text (3 lines, match count **1**; note the em dash `—` on the second line):

```rust
/// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by
/// DESIGN — and until this helper landed, none of them reaped it, so the process outlived the
/// whole test binary by up to a second.
```

with this exact text (3 lines, same em dash):

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
- **A backticked filename, not an intra-doc link, on purpose.** The whole `tests` module is behind
  `#[cfg(test)]` (`ops/local/mod.rs:32-33`), so rustdoc never compiles it and a `` [`exec_argv`] ``
  link would be decorative and permanently unverified. A backticked filename matches how the
  neighbouring test docs already cite code.
- The count "three" is preserved and is still correct — exactly three `SleeperMarker::new` call
  sites exist, all in `exec_argv.rs`. `exec_argv_kill_signals_only_the_single_pid_never_the_process_group`
  (`exec_argv.rs:301`), named by hand one sentence earlier, uses no `SleeperMarker`, is not one of
  the three, and stays untouched.
- Same three-line shape; longest new line is **95** columns (the file's current maximum is 99).

## Sweep: everything else in `ops/local/` is intra-file and correct

```
grep -rniE '\b(below|above|earlier in this file|later in this file|further (down|up)|the loop that follows|the following|shown below|listed above|top of|bottom of)\b' crates/cyrup-tools/src/ops/local/
```

returns **24** hits in the current tree. Every one other than the two fixed above was checked
against its referent and resolves inside its own file — do NOT "fix" them:

- `proc.rs:103` "the cwd check below" → the `tokio::fs::metadata(&spec.cwd)` guard at
  `proc.rs:115`. `proc.rs:105` "the mid-run cancel branch below" → the `exec` `select!` at
  `proc.rs:177`. Both same file, both still below.
- `proc.rs:244, 283, 287, 289, 290, 294, 324, 326, 327, 335, 337, 345, 399` — all within `exec` /
  `exec_argv`, and the "identical loop/block above" pairs point from `exec_argv` back to `exec`,
  which is above it in the same file. Correct.
- `command.rs:89` "see the doc comment above" → `build_argv_command`'s own doc comment, directly
  above it in `command.rs`. Correct.
- `tests/exec.rs:32`, `tests/exec_argv.rs:231, 363, 400`, `tests/tracking.rs:118` — all
  intra-function references inside a single test body. Correct.
- `tests/mod.rs:58` "some fd above 2" — a numeric fd comparison, not a location. Correct.

One further hit is a *hypothetical-alternative* phrasing, not a location, and is deliberately out
of scope: the assertion message at `tests/tracking.rs:122-124` ("its only faithful Rust home is
`Drop`, not a statement after the `select!` loop that a dropped future never reaches") describes
the rejected design and makes no file-position claim. Leave it.

## Do not touch

- **`guard.rs:30`** — "Declared AFTER `child` in `exec` so Rust's reverse-declaration drop order
  runs this guard while that ownership still holds." This is correct and
  [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  explicitly marks it **"Do not touch"** as the correct exemplar it contrasts against. Do not
  reword or re-wrap it while editing the same comment.
- **`guard.rs:27`** — "the guard is disarmed only AFTER the loop has observed `child.wait()`".
  Edit 1 supplies its antecedent; no edit needed.
- **`guard.rs:23-24`** — the `docs/gap-analysis/12-upstream-drift-pi-core.md` / `DRIFT-043`
  citation. Both the file and the `DRIFT-043` id exist; leave the sentence byte-identical.
- **Any `.rs` file outside `crates/cyrup-tools/src/ops/local/`.** The dead-path citations to
  `ops/local.rs` in `src/tests/no_inherited_harness_stdio.rs:52, 178, 182` belong to
  [`LOW-sleepermarker-citation-still-points-at-ops-local-rs.md`](./LOW-sleepermarker-citation-still-points-at-ops-local-rs.md).
  The remaining `ops/local.rs` citations — `ops/shell.rs:430`, `tools/find.rs:164`,
  `tools/edit.rs:237-238`, `tools/read.rs:134`, `tools/bash.rs:288`, and
  `crates/cyrup/src/signals.rs:39` — are the same dead-path class and are **not** claimed by this
  task. Do not fold them in here, and do not fix them silently.
- **`signal.rs`'s module doc** — owned by
  [`LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md`](./LOW-signal-module-doc-claims-to-be-the-crates-only-unsafe-site.md).
- No `cargo fmt` run, workspace-wide or otherwise. These are comment-only edits and rustfmt does
  not reformat comments; a blanket run is what
  [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  is already about.

## Definition of done

No tests, no benchmarks, no new documentation, and no git commands are required or wanted. Run
this one script from the repo root; done means it prints `ALL OK` and nothing else.

```bash
python3 - <<'PY'
import io, sys

G = 'crates/cyrup-tools/src/ops/local/guard.rs'
M = 'crates/cyrup-tools/src/ops/local/tests/mod.rs'
rd = lambda p: io.open(p, encoding='utf-8').read()
g, m = rd(G), rd(M)

OLD1 = ("/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon the\n"
        "/// `select!` loop below without running a single one of its `send_sigkill_tree` arms.\n")
NEW1 = ("/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon\n"
        "/// [`LocalProc::exec`]'s `select!` loop (in [`super::proc`]) without running a single one of its\n"
        "/// `send_sigkill_tree` arms.\n")
OLD2 = '/// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting\n'
NEW2 = '/// "runs no matter how we leave" is `Drop`, not a statement placed after that `select!` loop. Putting\n'
OLD3 = ("/// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by\n"
        "/// DESIGN — and until this helper landed, none of them reaped it, so the process outlived the\n"
        "/// whole test binary by up to a second.\n")
NEW3 = ("/// prove exactly that. The three fixtures in `exec_argv.rs` that use this helper therefore\n"
        "/// leave a live `sleep 1` behind by DESIGN — and until this helper landed, none of them reaped\n"
        "/// it, so the process outlived the whole test binary by up to a second.\n")

fail = []
def eq(label, got, want):
    if got != want:
        fail.append(f'{label}: got {got!r}, want {want!r}')

# 1. The three edits landed exactly once each, and no original text survives.
eq('edit1 applied',  g.count(NEW1), 1); eq('edit1 old gone',  g.count(OLD1), 0)
eq('edit2 applied',  g.count(NEW2), 1); eq('edit2 old gone',  g.count(OLD2), 0)
eq('edit3 applied',  m.count(NEW3), 1); eq('edit3 old gone',  m.count(OLD3), 0)

# 2. No dangling positional pointer is left anywhere under ops/local/.
import glob
for p in glob.glob('crates/cyrup-tools/src/ops/local/**/*.rs', recursive=True):
    t = rd(p)
    for bad in ('loop below', 'fixtures below'):
        if bad in t:
            fail.append(f'{p}: still contains {bad!r}')

# 3. guard.rs mentions `select!` exactly twice, and each one now names its referent.
sel = [l for l in g.splitlines() if 'select!' in l]
eq('guard select! mentions', len(sel), 2)
if len(sel) == 2:
    if '[`super::proc`]' not in sel[0] or '[`LocalProc::exec`]' not in sel[0]:
        fail.append('first select! mention does not name the loop')
    if 'that `select!` loop' not in sel[1]:
        fail.append('second select! mention does not refer back')

# 4. Only doc-comment text changed: every line of both files that is not a `///`
#    or `//!` comment must be unchanged, so the load-bearing code anchors are intact.
for label, text, anchors in [
    ('guard.rs', g, ['pub(super) struct KillTreeOnDrop {',
                     'libc::killpg(pgid as libc::pid_t, libc::SIGKILL);',
                     'pub(super) fn disarm(&mut self) {',
                     '/// [`LocalProc::exec`]: crate::ops::ProcOps::exec']),
    ('tests/mod.rs', m, ['struct SleeperMarker {',
                         'fn reapable_sleep_loop(&self, prefix: &str) -> String {',
                         'fn reap(&self) {']),
]:
    for a in anchors:
        if a not in text:
            fail.append(f'{label}: lost anchor {a!r}')

# 5. The four steps of the KillTreeOnDrop argument, in order, are still present.
order = ['ALWAYS settles',
         'A Rust future has no such guarantee',
         '`kill_on_drop(true)` is NOT a substitute',
         'is `Drop`, not a statement placed after that `select!` loop']
pos = [g.find(s) for s in order]
if -1 in pos or pos != sorted(pos):
    fail.append(f'KillTreeOnDrop argument steps missing or reordered: {pos}')

# 6. The DRIFT-043 citation is untouched.
if 'docs/gap-analysis/12-upstream-drift-pi-core.md' not in g or 'DRIFT-043' not in g:
    fail.append('DRIFT-043 citation was disturbed')

# 7. Out-of-scope dead-path citations owned by other tasks are still exactly as found.
for p, needle in [
    ('crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs', "See `ops/local.rs`'s `SleeperMarker`"),
    ('crates/cyrup-tools/src/ops/shell.rs', '`ops/local.rs`, the `ops/local.rs` fixtures'),
    ('crates/cyrup-tools/src/tools/read.rs', '(ops/local.rs:113)'),
    ('crates/cyrup/src/signals.rs', 'crates/cyrup-tools/src/ops/local.rs'),
]:
    if needle not in rd(p):
        fail.append(f'{p}: out-of-scope citation was modified')

print('\n'.join(fail) if fail else 'ALL OK')
sys.exit(1 if fail else 0)
PY
```

Explicitly out of scope, and not to be run as part of this task: `cargo test`, `cargo check`,
`cargo clippy`, `cargo doc`, `cargo fmt`, and any git invocation.

## Files

- [`crates/cyrup-tools/src/ops/local/guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs) — edits 1 and 2
- [`crates/cyrup-tools/src/ops/local/tests/mod.rs`](../../crates/cyrup-tools/src/ops/local/tests/mod.rs) — edit 3
- [`crates/cyrup-tools/src/ops/local/proc.rs`](../../crates/cyrup-tools/src/ops/local/proc.rs) — referent only, read-only
- [`crates/cyrup-tools/src/ops/local/tests/exec_argv.rs`](../../crates/cyrup-tools/src/ops/local/tests/exec_argv.rs) — referent only, read-only

## QA verdict (2026-08-23 08:24) — PASS, 9/10

All three edits are on disk exactly as specified and every factual claim in the new text was
re-verified against the real source, not taken on the comment's word:

- `guard.rs:17-19` now reads "abandon [`LocalProc::exec`]'s `select!` loop (in [`super::proc`])".
  TRUE: `impl ProcOps for LocalProc` is `proc.rs:91`, `async fn exec` opens at `proc.rs:92`, and its
  `tokio::select!` is `proc.rs:177` with two `send_sigkill_tree` arms (`proc.rs:180`, `:184`).
  `super` from `ops/local/guard.rs` is `crate::ops::local`, and `pub(crate) mod proc;` is
  `ops/local/mod.rs:28`.
- Naming `exec` rather than the module alone is correct, not over-specification: `KillTreeOnDrop::arm`
  has exactly one call site, `proc.rs:136`, inside `exec`. The second `select!` (`proc.rs:301`, in
  `exec_argv`) is never guarded, so the disambiguation is load-bearing.
- `guard.rs:36` `the` -> `that` landed; the anaphors at `guard.rs:28` ("the loop has observed
  `child.wait()`") now have a real antecedent. `guard.rs:31` ("Declared AFTER `child` in `exec`") and
  the DRIFT-043 citation are byte-identical.
- `tests/mod.rs:49-51` now says "The three fixtures in `exec_argv.rs` that use this helper". TRUE and
  exhaustive: `SleeperMarker::new` has exactly three call sites, all in `exec_argv.rs` (`:95`, `:130`,
  `:164`), each pairing `reapable_sleep_loop` with a `reap()` (`:100/117`, `:136/143`, `:171/178`).
  `exec_argv_kill_signals_only_the_single_pid_never_the_process_group` uses no `SleeperMarker`, so the
  sentence one clause earlier stays consistent.
- No `loop below` / `fixtures below` survives anywhere under `ops/local/`. Line widths (97 and 102)
  sit inside the file's pre-existing 97-104 range.

The Definition-of-done script passes checks 1-6. Check 7 (out-of-scope citations unchanged) reports
four "modified" files, and that is a false alarm from a stale expectation, not a scope violation: the
sibling tasks that own those citations landed first, so `no_inherited_harness_stdio.rs:52/178/182`,
`ops/shell.rs:430`, `tools/read.rs:134` and `crates/cyrup/src/signals.rs:39` now correctly cite
`ops/local/command.rs`, `ops/local/tests/mod.rs`, `ops/local/fs.rs` and `ops/local/tracking.rs`.

Point off for one thing, and it is about the plan's reasoning rather than the diff: the justification
claimed "the workspace `[workspace.lints.rustdoc]` table is empty". It is not — the root `Cargo.toml`
now sets `broken_intra_doc_links = "deny"` (with `--document-private-items` in `.cargo/config.toml`),
so a bad link here would have been a hard error. The two new links do resolve, verified by running
`cargo doc -p cyrup-tools --no-deps`, but that was luck rather than a checked premise.

### Separate pre-existing defect found while verifying — NOT this task's, do not fold in

`cargo doc -p cyrup-tools --no-deps` fails with one deny-level error, untouched by this task:

```
error: unresolved link to `crate::ops::local::tracking::TRACKED_DETACHED_CHILD_PIDS`
  --> crates/cyrup-tools/src/ops/local/guard.rs:44:38
```

`static TRACKED_DETACHED_CHILD_PIDS` (`ops/local/tracking.rs:34`) has no visibility modifier, so it is
private to `tracking` and genuinely not nameable from `guard`. Before the `local.rs` split both lived
in one module and the link resolved; the split broke it. This is the same class of split fallout this
task exists to clean up, but it is a link-target break rather than a positional pointer, it is outside
the task's stated scope and its DoD, and `RUSTDOC_MODULE_LINK_ERRORS.md` recorded `cargo doc` exiting 0
at 02:26 — so the workspace doc gate is red again and needs its own task.
