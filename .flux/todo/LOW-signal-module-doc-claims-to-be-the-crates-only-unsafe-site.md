---
title: Signal Module Doc Claims To Be The Crates Only Unsafe Site
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:40
---

# `signal.rs`'s header calls the file "the crate's `unsafe` leaf" — three sibling files also hold `unsafe`

## Description

The twelve-file split of `ops/local.rs` gave every new file a fresh module header. The one written
for [`crates/cyrup-tools/src/ops/local/signal.rs:1`](../../crates/cyrup-tools/src/ops/local/signal.rs)
opens:

```rust
//! The raw unix kill primitives, and the crate's `unsafe` leaf.
```

That sentence is new — it did not exist in any form before the split — and it is false. `signal.rs`
holds five of the crate's eight non-test `unsafe` blocks; the other three live in `command.rs`,
`fs.rs` and `guard.rs`, all siblings in the same directory.

It is false in the one direction that matters for a crate whose entire audit posture is
`#![deny(unsafe_code)]` ([`lib.rs:16`](../../crates/cyrup-tools/src/lib.rs)) plus per-item
`#[allow(unsafe_code)]`: it tells a reader that `signal.rs` is *the* place to review `unsafe`. A
reviewer who greps for "the crate's `unsafe`" lands there, reviews five blocks, and closes the file
believing the surface is covered. The block most likely to be missed that way is
[`guard.rs:84`](../../crates/cyrup-tools/src/ops/local/guard.rs) — a `killpg(SIGKILL)` against a
whole process group, fired from a `Drop` impl that can run on an arbitrary unwinding or
cancellation path.

The split's own [`mod.rs:11-15`](../../crates/cyrup-tools/src/ops/local/mod.rs) states this
correctly and completely. So the branch leaves two module docs in the same directory contradicting
each other about where the crate's `unsafe` lives.

Where the sentence came from: the pre-split header
(`git show 4902cddf:crates/cyrup-tools/src/ops/local.rs`, lines 18-21) said *"The only `unsafe` in
the crate lives here…"* — true, because "here" was the whole 1,912-line file. Narrowing "here" onto
`signal.rs` without narrowing the claim's scope is exactly what broke it.

## Evidence — the complete `unsafe` inventory (verified, not quoted)

Enumerated directly from the source at HEAD of `claude/cyrup-tools-largest-file-rjs3yg`, not taken
from either doc comment. Every `unsafe` block in `crates/cyrup-tools/src/` is under
`ops/local/`; `grep -rn "unsafe {" crates/cyrup-tools/src/` returns exactly nine hits.

**Non-test (8):**

| # | File | Line | Enclosing item | Call |
|---|------|------|----------------|------|
| 1 | [`command.rs`](../../crates/cyrup-tools/src/ops/local/command.rs) | 43 | `build_command` | `std_cmd.pre_exec(\|\| { libc::setsid(); Ok(()) })` |
| 2 | [`signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs) | 29 | `kill_process_tree` | `libc::killpg(pid, SIGKILL)` |
| 3 | [`signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs) | 31 | `kill_process_tree` | `libc::kill(pid, SIGKILL)` (ESRCH fallback) |
| 4 | [`signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs) | 66 | `send_sigkill_tree` | `libc::killpg(pid, SIGKILL)` |
| 5 | [`signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs) | 109 | `terminate_pid` | `libc::kill(pid, SIGTERM)` |
| 6 | [`signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs) | 134 | `kill_pid` | `libc::kill(pid, SIGKILL)` |
| 7 | [`fs.rs`](../../crates/cyrup-tools/src/ops/local/fs.rs) | 142 | `LocalFs::access` | `libc::access(c_path.as_ptr(), amode)` |
| 8 | [`guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs) | 84 | `<KillTreeOnDrop as Drop>::drop` | `libc::killpg(pgid, SIGKILL)` |

**Test-only (1), for completeness:**

| # | File | Line | Enclosing item | Call |
|---|------|------|----------------|------|
| 9 | [`tests/mod.rs`](../../crates/cyrup-tools/src/ops/local/tests/mod.rs) | 136 | `pid_exists` (`#[cfg(test)]`) | `libc::kill(pid, 0)` liveness probe |

Distribution of the eight non-test blocks: **`signal.rs` 5, `command.rs` 1, `fs.rs` 1,
`guard.rs` 1.** Five of eight is a plurality, not exclusivity.

### Two things to get right about the count

- **`tracking.rs` contains no `unsafe` at all.** The review note "six in signal/tracking/guard"
  arrives at the right number by the wrong route: 5 (`signal.rs`) + 0 (`tracking.rs`) +
  1 (`guard.rs`) = 6. `tracking.rs` reaches the syscall indirectly — `tracking.rs:11` is
  `use super::signal::kill_process_tree;`. Do not add `tracking` to any list of `unsafe` sites.
- **The `#[allow(unsafe_code)]` attributes index *items*, not *blocks*.** There are 8 allows
  (`command.rs:13`, `signal.rs:23/59/102/129`, `fs.rs:123`, `guard.rs:78`, `tests/mod.rs:133`) but
  9 `unsafe` blocks, because `kill_process_tree` carries two blocks (lines 29 and 31) under a
  single allow. The allow list is a reliable index of which *functions* need audit; it is not a
  block count.

### The other two claims in the crate

Repo-wide there are exactly three statements about where `unsafe` lives:

1. **`signal.rs:1`** — wrong, introduced by this branch. This task fixes it.
2. **[`mod.rs:11-15`](../../crates/cyrup-tools/src/ops/local/mod.rs)** — correct and complete.
   Checked item by item against the table above: `setsid`/`killpg` in `command::build_command` and
   `signal::send_sigkill_tree`, `kill_process_tree`, `guard::KillTreeOnDrop`'s `Drop`, the
   single-pid `kill(2)` calls (`terminate_pid`/`kill_pid`), and the `access(2)` probe in
   `LocalFs` — all 8 non-test blocks, no phantom entries. It is in fact *more* complete than the
   pre-split header, which omitted the `access(2)` probe. **This is the authoritative list. Leave
   it exactly as it is.**
3. **[`lib.rs:14-15`](../../crates/cyrup-tools/src/lib.rs)** — *"The only `unsafe` in the crate is
   the isolated unix process-group code in [`ops::local`]."* **Correct as scoped; leave it.** The
   locational claim is what it asserts, and it holds: every one of the nine blocks is under
   `crates/cyrup-tools/src/ops/local/`. The link resolves (`pub mod local;` at
   `crates/cyrup-tools/src/ops/mod.rs:9`). And it is not this branch's text — it is byte-identical
   at the merge base (`git show 4902cddf:crates/cyrup-tools/src/lib.rs | sed -n '14,15p'`); the
   branch's only `lib.rs` diff is `use` reordering under edition-2024 import style.
   One nuance recorded but deliberately **not** acted on: the phrase "unix process-group code"
   undersells the `access(2)` probe (a filesystem permission check, not process-group) and the
   single-pid `kill(2)` calls. That imprecision predates the branch, is a separate editorial call
   about a crate-root summary line, and is out of scope here — see *Out Of Scope*.

## Required Change

One file, one line replaced by two, in
[`crates/cyrup-tools/src/ops/local/signal.rs`](../../crates/cyrup-tools/src/ops/local/signal.rs).

Replace line 1 exactly:

```rust
//! The raw unix kill primitives, and the crate's `unsafe` leaf.
```

with exactly:

```rust
//! The raw unix kill primitives — the bulk of the crate's `unsafe`, but not all of it:
//! [`super`]'s module doc owns the full inventory.
```

Lines 2-9 of the header (the blank `//!`, the two-shapes paragraph, and the two link-reference
definitions for `LocalProc::exec` / `LocalProc::exec_argv`) are **unchanged**. The header goes from
9 `//!` lines to 10. Nothing below the header is touched.

### Why this wording

- **Leads with what the module is.** `signal.rs` is the raw unix kill primitives; that stays the
  first thing a reader learns, which is the part of the original sentence that was worth keeping.
- **Drops the exclusivity, keeps the useful signal.** "The bulk … but not all of it" is true today
  (5 of 8) and stays true as long as `signal.rs` holds the plurality. It carries no hard count, so
  it cannot rot the way "five of eight" or a duplicated file list would.
- **One authoritative inventory, not two.** It points at `super` rather than restating `mod.rs`'s
  enumeration. That is the whole point of the fix: the failure mode here was two lists disagreeing,
  and a second list — even a correct one — would recreate it on the next `unsafe` change.
- **``[`super`]`` is established in this repo** (7 existing uses; e.g.
  `crates/cyrup-permission-system/src/extension/consts.rs:4` uses the same "…[`super`]'s module
  doc" phrasing). It resolves to `crate::ops::local`, which is `pub mod` (`ops/mod.rs:9`), so no
  `rustdoc::private_intra_doc_links`. `signal.rs` already links ``[`super::proc`]`` three times, so
  the relative-link style matches the file it is in.
- **No formatting risk.** The two lines are 87 and 51 columns; the repo has no `rustfmt.toml`, so
  `max_width` is the default 100 and `wrap_comments` is off — rustfmt will not reflow them.
- **Character-level match with the surrounding prose.** Em dash is U+2014 (as at `signal.rs:3-5`);
  the apostrophe in `crate's` stays ASCII `'`, matching the line being replaced.

## Out Of Scope

- Do **not** edit [`mod.rs`](../../crates/cyrup-tools/src/ops/local/mod.rs). Its lines 11-15 are the
  authoritative inventory and were verified correct against the source. Adding to it, reordering it,
  or "syncing" it with `signal.rs` all defeat the fix.
- Do **not** edit [`lib.rs`](../../crates/cyrup-tools/src/lib.rs). Its claim was checked and holds;
  it is also unchanged from the merge base, so touching it widens this branch's diff for no defect.
  The "unix process-group code" phrasing nuance is noted above and deliberately left alone.
- Do **not** touch any `.rs` file other than `signal.rs`, and do not touch anything in `signal.rs`
  below line 2 — not the `SAFETY:` comments, not the `#[allow(unsafe_code)]` attributes, not the
  per-function doc comments.
- Do **not** add, remove, relocate, or reword any `unsafe` block, `#[allow(unsafe_code)]`, or
  `SAFETY:` comment. No behaviour change, no API change, no re-export change.
- Do **not** run a workspace-wide `cargo fmt`. This edit introduces no rustfmt violation.
- Do **not** add tests. There is nothing executable to assert about a doc comment.
- **The comment correction is the deliverable.** It is not "documentation cleanup" to be dropped as
  low-value: the sentence actively misdirects an `unsafe` audit in a `#![deny(unsafe_code)]` crate.

## Definition of Done

- [ ] `sed -n '1,2p' crates/cyrup-tools/src/ops/local/signal.rs` prints exactly:
      ``//! The raw unix kill primitives — the bulk of the crate's `unsafe`, but not all of it:``
      then ``//! [`super`]'s module doc owns the full inventory.``
- [ ] `grep -rn "leaf" crates/cyrup-tools/src/ --include=*.rs` returns nothing — that word appears
      exactly once in the crate today, in the sentence being replaced
- [ ] No remaining text in `crates/cyrup-tools/src/` claims `signal.rs` is the sole or only
      `unsafe` site
- [ ] `grep -c '^//!' crates/cyrup-tools/src/ops/local/signal.rs` is **10** (was 9)
- [ ] `sed -n '3,9p' crates/cyrup-tools/src/ops/local/signal.rs` is byte-identical to the current
      `sed -n '2,9p'` output — the rest of the header only shifted down by one line
- [ ] `awk 'NR<=10 { if (length > 100) print NR": "length }' crates/cyrup-tools/src/ops/local/signal.rs`
      prints nothing
- [ ] `git diff --stat` for this task shows **one file changed, 2 insertions, 1 deletion**
- [ ] `git diff -- crates/cyrup-tools/src/ops/local/mod.rs crates/cyrup-tools/src/lib.rs` is empty
- [ ] Counts unchanged crate-wide: `grep -rc 'allow(unsafe_code)' crates/cyrup-tools/src/ --include=*.rs`
      still sums to **8**, and `grep -rn 'unsafe {' crates/cyrup-tools/src/ --include=*.rs | wc -l`
      is still **9**
- [ ] `cargo doc -p cyrup-tools --no-deps` resolves ``[`super`]`` with no
      `rustdoc::broken_intra_doc_links` warning *(verification step for whoever executes this task;
      not run during augmentation)*
