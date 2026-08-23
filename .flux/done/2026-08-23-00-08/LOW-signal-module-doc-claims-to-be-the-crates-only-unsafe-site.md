---
title: Signal Module Doc Claims To Be The Crates Only Unsafe Site
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:22
---

# `signal.rs`'s header calls the file "the crate's `unsafe` leaf" — three sibling files also hold `unsafe`

## Description

The twelve-file split of `ops/local.rs` gave every new file a fresh module header. The one written
for `crates/cyrup-tools/src/ops/local/signal.rs` opens with its first line:

```rust
//! The raw unix kill primitives, and the crate's `unsafe` leaf.
```

That sentence is false. `signal.rs` holds five of the crate's eight non-test `unsafe` blocks; the
other three live in `command.rs`, `fs.rs` and `guard.rs`, all siblings in the same directory
(`crates/cyrup-tools/src/ops/local/`).

It is false in the one direction that matters for a crate whose entire audit posture is
`#![deny(unsafe_code)]` (the inner attribute immediately following the crate-root doc comment in
`crates/cyrup-tools/src/lib.rs`) plus per-item `#[allow(unsafe_code)]`: it tells a reader that
`signal.rs` is *the* place to review `unsafe`. A reviewer who greps for "the crate's `unsafe`"
lands there, reviews five blocks, and closes the file believing the surface is covered. The block
most likely to be missed that way is the one inside `<KillTreeOnDrop as Drop>::drop` in
`crates/cyrup-tools/src/ops/local/guard.rs` — a `killpg(SIGKILL)` against a whole process group,
fired from a `Drop` impl that can run on an arbitrary unwinding or cancellation path.

The sibling module doc in `crates/cyrup-tools/src/ops/local/mod.rs` — the header paragraph that
begins ``//! The only `unsafe` in the crate lives in this module,`` — states this correctly and
completely. So the tree currently holds two module docs in the same directory contradicting each
other about where the crate's `unsafe` lives.

Where the sentence came from: the pre-split `ops/local.rs` header said *"The only `unsafe` in the
crate lives here…"* — true, because "here" was the whole 1,912-line file. Narrowing "here" onto
`signal.rs` without narrowing the claim's scope is exactly what broke it. (Historical note only.
Do not consult git to confirm it; nothing in this task depends on it.)

## Evidence — the complete `unsafe` inventory

Re-verified against the current working tree during augmentation (2026-08-23), enumerated from the
source rather than from either doc comment. Every `unsafe` block in `crates/cyrup-tools/src/` is
under `ops/local/`; `grep -rn 'unsafe {' crates/cyrup-tools/src/ --include=*.rs` returns exactly
nine hits.

**Non-test (8):**

| # | File (under `crates/cyrup-tools/src/ops/local/`) | Line | Enclosing item | Call |
|---|------|------|----------------|------|
| 1 | `command.rs` | 43 | `build_command` | `std_cmd.pre_exec(\|\| { libc::setsid(); Ok(()) })` |
| 2 | `signal.rs` | 29 | `kill_process_tree` | `libc::killpg(pid, SIGKILL)` |
| 3 | `signal.rs` | 31 | `kill_process_tree` | `libc::kill(pid, SIGKILL)` (ESRCH fallback) |
| 4 | `signal.rs` | 66 | `send_sigkill_tree` | `libc::killpg(pid, SIGKILL)` |
| 5 | `signal.rs` | 109 | `terminate_pid` | `libc::kill(pid, SIGTERM)` |
| 6 | `signal.rs` | 134 | `kill_pid` | `libc::kill(pid, SIGKILL)` |
| 7 | `fs.rs` | 142 | `LocalFs::access` | `libc::access(c_path.as_ptr(), amode)` |
| 8 | `guard.rs` | 84 | `<KillTreeOnDrop as Drop>::drop` | `libc::killpg(pgid, SIGKILL)` |

**Test-only (1), for completeness:**

| # | File | Line | Enclosing item | Call |
|---|------|------|----------------|------|
| 9 | `ops/local/tests/mod.rs` | 136 | `pid_exists` (`#[cfg(test)]`) | `libc::kill(pid, 0)` liveness probe |

The line numbers above are informational only — **no edit in this task is addressed by line
number.** The one edit is addressed by exact text match (see *Required Change*). Enclosing item
names are the durable pointers.

Distribution of the eight non-test blocks: **`signal.rs` 5, `command.rs` 1, `fs.rs` 1,
`guard.rs` 1.** Five of eight is a plurality, not exclusivity.

### Two things to get right about the count

- **`tracking.rs` contains no `unsafe` at all** — `grep -c unsafe` on it returns 0. Any review note
  of the form "six in signal/tracking/guard" arrives at the right number by the wrong route:
  5 (`signal.rs`) + 0 (`tracking.rs`) + 1 (`guard.rs`) = 6. `tracking.rs` reaches the syscall
  indirectly, via its `use super::signal::kill_process_tree;` import. Do not add `tracking` to any
  list of `unsafe` sites.
- **The `#[allow(unsafe_code)]` attributes index *items*, not *blocks*.** There are 8 allows
  (one in `command.rs`, four in `signal.rs` — on `kill_process_tree`, `send_sigkill_tree`,
  `terminate_pid`, `kill_pid` — one on `LocalFs::access` in `fs.rs`, one on the `impl Drop for
  KillTreeOnDrop` in `guard.rs`, one on `pid_exists` in `tests/mod.rs`) but 9 `unsafe` blocks,
  because `kill_process_tree` carries two blocks under a single allow. The allow list is a reliable
  index of which *functions* need audit; it is not a block count.

### The other two claims in the crate

Repo-wide there are exactly three statements about where `unsafe` lives:

1. **`signal.rs`'s first line** — wrong. This task fixes it, and only it.
2. **The header paragraph in `crates/cyrup-tools/src/ops/local/mod.rs` beginning
   ``//! The only `unsafe` in the crate lives in this module,``** — correct and complete.
   Re-checked item by item against the table above: `setsid`/`killpg` in `command::build_command`
   and
   `signal::send_sigkill_tree`, `kill_process_tree`, `guard::KillTreeOnDrop`'s `Drop`, the
   single-pid `kill(2)` calls (`terminate_pid`/`kill_pid`), and the `access(2)` probe in
   `LocalFs` — all 8 non-test blocks, no phantom entries. "this module" scopes to `ops::local`,
   which is where all nine blocks live, so the claim holds. **This is the authoritative list.
   Leave it exactly as it is.**
3. **The crate-root claim at the end of `crates/cyrup-tools/src/lib.rs`'s doc comment** — *"The
   only `unsafe` in the crate is the isolated unix process-group code in [`ops::local`]."*
   **Correct as scoped; leave it.** The locational claim is what it asserts, and it holds: every
   one of the nine blocks is under `crates/cyrup-tools/src/ops/local/`. The intra-doc link resolves
   (`ops` is `pub mod ops;` in `lib.rs`, and `local` is `pub mod local;` in
   `crates/cyrup-tools/src/ops/mod.rs`). One nuance recorded but deliberately **not** acted on: the
   phrase "unix process-group code" undersells the `access(2)` probe (a filesystem permission
   check, not process-group) and the single-pid `kill(2)` calls. That imprecision is a separate
   editorial call about a crate-root summary line and is out of scope here — see *Out Of Scope*.

## Required Change

Exactly one file is modified: `crates/cyrup-tools/src/ops/local/signal.rs`. One line is replaced
by two. Nothing else in the repository changes.

### Step 1 — assert the anchor is unique (required precondition)

Run, from the repo root:

```
grep -cF "//! The raw unix kill primitives, and the crate's \`unsafe\` leaf." crates/cyrup-tools/src/ops/local/signal.rs
```

This **must print `1`**. Verified during augmentation: it is 1 in `signal.rs`, and the substring
`The raw unix kill primitives` occurs exactly once across all of `crates/**/*.rs`. If it prints
anything other than `1`, stop and re-scope the task — do not guess at a line number.

### Step 2 — the edit

Replace this exact text (a whole line, 64 characters, pure ASCII — no non-ASCII bytes anywhere in
it, in particular the apostrophe in `crate's` is ASCII `U+0027`):

```
//! The raw unix kill primitives, and the crate's `unsafe` leaf.
```

with this exact text (two whole lines):

```
//! The raw unix kill primitives — the bulk of the crate's `unsafe`, but not all of it:
//! [`super`]'s module doc owns the full inventory.
```

Character-level requirements for the replacement, all verified during augmentation:

- The dash after `primitives` is an **em dash, U+2014** (`—`), not a hyphen and not an en dash.
  This is the file's established style: `signal.rs` already contains U+2014 on 15 lines — 12 in
  its own `///` doc comments (including those on `kill_process_tree`, `send_sigkill_tree`,
  `terminate_pid` and `kill_pid`) and 3 in `//` inline comments (including the `SAFETY:` comment
  inside `kill_process_tree`). *(Note: the existing `//!` header lines contain zero em dashes — do
  not use the header as the style reference; use the `///` comments.)*
- Both apostrophes (`crate's`, `` [`super`]'s ``) are ASCII `U+0027`, matching the line being
  replaced and matching `` [`super::proc`]'s `` already used in this header.
- The two new lines are 87 and 51 columns. The workspace has **no `rustfmt.toml`** (confirmed: none
  at the repo root or in any crate), so `max_width` is the default 100 and `wrap_comments` is off —
  rustfmt will not reflow them.

Everything else in the file is untouched. The remaining eight `//!` header lines (the blank `//!`,
the three-line "Two shapes" paragraph, the ``[`super::proc`]`` cross-reference, the blank `//!`, and
the two link-reference definitions for `LocalProc::exec` / `LocalProc::exec_argv`) shift down by
exactly one line and are otherwise byte-identical. The header goes from 9 `//!` lines to 10. The
file goes from 148 lines to 149. Nothing below the header is touched.

### Why this wording (this is the required wording, not a suggestion)

- **Leads with what the module is.** `signal.rs` is the raw unix kill primitives; that stays the
  first thing a reader learns, which is the part of the original sentence worth keeping.
- **Drops the exclusivity, keeps the useful signal.** "The bulk … but not all of it" is true today
  (5 of 8) and stays true as long as `signal.rs` holds the plurality. It carries no hard count, so
  it cannot rot the way "five of eight" or a duplicated file list would.
- **One authoritative inventory, not two.** It points at `super` rather than restating `mod.rs`'s
  enumeration. That is the whole point of the fix: the failure mode was two lists disagreeing, and
  a second list — even a correct one — would recreate it on the next `unsafe` change. For this
  reason, do **not** substitute a wording that names `command.rs`/`fs.rs`/`guard.rs` inline.
- **``[`super`]`` is established in this repo.** Seven existing uses across the workspace; in
  particular `crates/cyrup-permission-system/src/extension/consts.rs` uses the same
  "…[`super`]'s module doc" phrasing verbatim. From `crate::ops::local::signal` it resolves to
  `crate::ops::local`, which is `pub mod local;` in `ops/mod.rs`, so no
  `rustdoc::private_intra_doc_links`. `signal.rs` already links ``[`super::proc`]`` three times, so
  the relative-link style matches the file it is in.

## Out Of Scope

- Do **not** edit `crates/cyrup-tools/src/ops/local/mod.rs`. Its `unsafe` inventory paragraph is
  authoritative and was re-verified correct against the source. Adding to it, reordering it, or
  "syncing" it with `signal.rs` all defeat the fix.
- Do **not** edit `crates/cyrup-tools/src/lib.rs`. Its claim was checked and holds. The "unix
  process-group code" phrasing nuance is noted above and deliberately left alone.
- Do **not** touch any `.rs` file other than `signal.rs`, and inside `signal.rs` touch nothing
  except the single header line named above — not the `SAFETY:` comments, not the
  `#[allow(unsafe_code)]` attributes, not the per-function doc comments, not the `///` bodies.
- Do **not** add, remove, relocate, or reword any `unsafe` block, `#[allow(unsafe_code)]`, or
  `SAFETY:` comment. No behaviour change, no API change, no re-export change.
- Do **not** run a workspace-wide `cargo fmt`. This edit introduces no rustfmt violation.
- Do **not** write tests or benchmarks for this change, and do not author any new documentation
  beyond the two replacement lines. A different team owns tests, benchmarks and docs; there is
  nothing executable to assert about a doc comment anyway.
- **The comment correction is the deliverable.** It is not "documentation cleanup" to be dropped as
  low-value: the sentence actively misdirects an `unsafe` audit in a `#![deny(unsafe_code)]` crate.

## Definition of Done

Run every command below from the repo root (`/home/user/cyrup`). No git command is used or needed.

- [ ] `sed -n '1,2p' crates/cyrup-tools/src/ops/local/signal.rs` prints exactly these two lines:
      ``//! The raw unix kill primitives — the bulk of the crate's `unsafe`, but not all of it:``
      then ``//! [`super`]'s module doc owns the full inventory.``
- [ ] The second line is present verbatim exactly once — this command prints `1`:

      ```
      grep -cF "//! [\`super\`]'s module doc owns the full inventory." crates/cyrup-tools/src/ops/local/signal.rs
      ```

- [ ] `grep -rn 'leaf' crates/cyrup-tools/src/ --include=*.rs` returns nothing — that word appears
      exactly once in the crate today, in the sentence being replaced
- [ ] `grep -rn 'The raw unix kill primitives, and' crates/ --include=*.rs` returns nothing
- [ ] `grep -c '^//!' crates/cyrup-tools/src/ops/local/signal.rs` prints `10` (was 9)
- [ ] `wc -l < crates/cyrup-tools/src/ops/local/signal.rs` prints `149` (was 148)
- [ ] The rest of the header only shifted down by one line. Before editing, capture
      `sed -n '2,9p' crates/cyrup-tools/src/ops/local/signal.rs > tmp/signal-header-before.txt`;
      after editing, `sed -n '3,10p' crates/cyrup-tools/src/ops/local/signal.rs | diff - tmp/signal-header-before.txt`
      prints nothing. (`tmp/` is gitignored; delete the scratch file afterwards.)
- [ ] `awk 'NR<=10 && length > 100 { print NR": "length }' crates/cyrup-tools/src/ops/local/signal.rs`
      prints nothing
- [ ] The em dash really is U+2014:
      `sed -n '1p' crates/cyrup-tools/src/ops/local/signal.rs | grep -c $'—'` prints `1`
- [ ] No remaining text in `crates/cyrup-tools/src/` claims `signal.rs` is the sole or only
      `unsafe` site. This command must return **exactly one** hit, the pre-existing and correct
      `ops/local/mod.rs:11` inventory line — and no hit in `signal.rs`:

      ```
      grep -rn 'only \`unsafe\`' crates/cyrup-tools/src/ --include=*.rs
      ```

      (The `lib.rs` crate-root claim does not appear in this grep because its wording wraps across
      two `//!` lines — that is expected, not a miss.)
- [ ] Inventory unchanged crate-wide:
      `grep -rn 'unsafe {' crates/cyrup-tools/src/ --include=*.rs | wc -l` is still `9`, and
      `grep -rc 'allow(unsafe_code)' crates/cyrup-tools/src/ --include=*.rs | awk -F: '{s+=$2} END{print s}'`
      is still `8`
- [ ] `mod.rs` and `lib.rs` are untouched. Verify by content, not by git:
      `wc -l < crates/cyrup-tools/src/ops/local/mod.rs` still prints `60`,
      `wc -l < crates/cyrup-tools/src/lib.rs` still prints `56`, and
      `sed -n '11,15p' crates/cyrup-tools/src/ops/local/mod.rs` plus
      `sed -n '14,16p' crates/cyrup-tools/src/lib.rs` still read exactly as quoted in
      *The other two claims in the crate* above.
- [ ] `cargo doc -p cyrup-tools --no-deps --document-private-items` completes with no
      `rustdoc::broken_intra_doc_links` or `rustdoc::private_intra_doc_links` warning for
      `signal.rs`. **`--document-private-items` is required**: `signal` is declared
      `pub(crate) mod signal;` in `ops/local/mod.rs`, so a plain `cargo doc --no-deps` never
      renders this module doc and would silently prove nothing.
- [ ] `cargo check -p cyrup-tools` still succeeds (sanity only — a doc comment cannot break it)
