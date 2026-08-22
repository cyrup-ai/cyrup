---
stage: aug
status: done
updated: 2026-08-22 17:44
---

# Restore The Pre-Refactor rustdoc Baseline After The session.rs Split

## Description

The decomposition of `cyrup-session-svc/src/session.rs` into `src/session/` (21 files) is complete
and verified — every build gate green, 311 tests passing, public API unchanged. **None of that is
to be redone.**

One regression remains: the split moved doc comments into modules where the names they link to are
no longer in scope, so `cargo doc` gained warnings. Fix by bringing those names back into scope.
**Three `#[cfg(doc)]` imports and one path qualification. Four lines.**

## The measured regression — 5, not 7

`.flux/todo/CARGO_DOC_WARNINGS.md` carries a **pre-refactor baseline** taken at 06:00 today (this
session began at 16:09): `cyrup-session-svc` → **19** warnings under `cargo doc --workspace --no-deps`.

Measured now: **24**. The delta is **+5**. The earlier QA note of "+7" overcounted — see the
CompactionCancelGuard row below.

The arithmetic pins down the original set exactly. Files outside `src/session/` were never touched
and still carry 11 (`host_services.rs` 5, `runtime.rs` 3, `event.rs` 2, `builder.rs` 1), so the
original `session.rs` held `19 − 11 = 8`. Today `src/session/**` holds 13:

| Site | Warning now | In the single file | Verdict |
| --- | --- | --- | --- |
| `accessors.rs:145` | links to private `Self::push_active_tools` | same warning | pre-existing |
| `bash.rs:165` | links to private `Self::emit_user_bash_event` | same warning | pre-existing |
| `control.rs:83` | links to private `Self::apply_agent_state_op` | same warning | pre-existing |
| `model.rs:177` | links to private `Self::full_model_registry` | same warning | pre-existing |
| `queue.rs:107` | links to private `ABORT_SETTLE_TIMEOUT` | same warning | pre-existing |
| `transcript.rs:126` | links to private `host_services::tree_node_to_json` | same warning | pre-existing |
| `mod.rs:555` | **unresolved** `CompactionCancelGuard` | links to private item | **reclassified, not new** |
| `mod.rs:564` | **unresolved** `CompactionCancelGuard` | links to private item | **reclassified, not new** |
| `accessors.rs:258` | **unresolved** `ProviderSwap` | resolved cleanly | **NEW** |
| `types.rs:72` | **unresolved** `AgentSession::session_dag` | resolved cleanly | **NEW** |
| `types.rs:112` | **unresolved** `AgentSession::navigate_tree` | resolved cleanly | **NEW** |
| `types.rs:132` | **unresolved** `AgentSession::cycle_model` | resolved cleanly | **NEW** |
| `types.rs:141` | **unresolved** `AgentSession::bind_extensions_with` | resolved cleanly | **NEW** |

`6 pre-existing + 2 reclassified = 8` — the original count, to the unit. The 6 "links to private
item" rows are visibility-based: they fire identically whether the code sits in one file or twenty,
and they belong to [`CARGO_DOC_WARNINGS.md`](CARGO_DOC_WARNINGS.md), which owns that class
crate-wide (442 of them). **Leave all 6 alone.**

## The rule that decides every fix

rustdoc's notion of "private" is **reachability from the crate root, not declared visibility**.
Verified empirically (probe at [`tmp/docprobe2`](../../tmp/docprobe2)):

- A `pub` item in a *private* module that **is** re-exported by `lib.rs` counts as **public** —
  a link to it resolves silently.
- A `pub` item in a private module that is **not** re-exported counts as **private** — the link
  resolves but warns `links to private item`.

Which sorts the sites cleanly:

- `AgentSession` — `lib.rs:74` `pub use session::{AgentSession, …}` → reachable → **fix is free**.
- `ProviderSwap` — `lib.rs:70` `pub use provider_swap::{…, ProviderSwap}` → reachable → **free**.
- `CompactionCancelGuard` — `pub(super)`, never re-exported → unreachable → importing it turns
  `unresolved link` into `links to private item`, **which is exactly the class it had before the
  split**. That is the goal: restore the original state, not invent a new one.

## Why `#[cfg(doc)]` and not a plain `use`

None of the three names appears in *code* in the file that needs it (verified: `types.rs`
`AgentSession` = 0, `accessors.rs` `ProviderSwap` = 0, `mod.rs` `CompactionCancelGuard` = 0). rustc
does not count an intra-doc link as a use, so a plain `use` would trade a rustdoc warning for an
`unused_imports` warning — and `cargo check`/`clippy` over `src/session/**` are currently at **zero**.
That must not regress.

`#[cfg(doc)]` is the idiom for a doc-only import: the item exists while rustdoc runs and is absent
from every other build. Both halves were measured on the probe, not assumed:

```
$ cargo doc  --no-deps   # types.rs link resolves, no warning emitted for it
$ cargo clippy --all-targets   # no unused-import warning — cfg(doc) is invisible here
```

The workspace has no `#[cfg(doc)]` precedent yet, so each import carries a one-line comment saying
why it exists — otherwise the next reader deletes it as dead.

## Required changes — four edits

### 1. `src/session/types.rs` — resolves 4 warnings

Insert directly after the module doc, **before** the existing `use cyrup_core::{…}` line:

```rust
// Doc-only: the types below document themselves against the seam that produces them, but none of
// them names `AgentSession` in code. `cfg(doc)` keeps the intra-doc links resolvable without an
// `unused_imports` warning in a normal build.
#[cfg(doc)]
use super::AgentSession;
```

### 2. `src/session/accessors.rs` — resolves 1 warning

`model_catalog`'s doc (line 258) links `[`ProviderSwap`]`. Add below the existing
`use super::AgentSession;` at the end of the import block:

```rust
// Doc-only — see `types.rs`. `model_catalog`'s doc names the swappable provider; the original
// `session.rs` had it in scope via `use crate::provider_swap::ProviderSwap` (session.rs:41).
#[cfg(doc)]
use crate::provider_swap::ProviderSwap;
```

### 3. `src/session/mod.rs` — restores 2 warnings to their original class

The `Drop for AgentSession` rationale (lines 555 and 564) names the guard twice. Add beside the
existing `#[cfg(test)] pub(crate) use files::trash_args;` re-export:

```rust
// Doc-only: the `Drop` rationale below is written against the guard that plays the same role for
// the compaction cancel slot. `CompactionCancelGuard` is `pub(super)` and never re-exported, so
// rustdoc still reports `links to private item` — which is the warning this link carried before
// the split, and is `CARGO_DOC_WARNINGS.md`'s to resolve, not this task's.
#[cfg(doc)]
use compaction::CompactionCancelGuard;
```

### 4. `src/session/model.rs` — latent, fix while here

Its module doc links bare `[`ProviderSwap`]`. It does not warn today only because private module
docs are never link-checked, so it is a trap that springs the moment anyone makes the module public.
This doc was authored by the refactor, so the no-rewording rule does not cover it — qualify the path
rather than adding a fourth import:

```
//! Resolving a `/model` pattern, installing the owning provider into the live
//! [`crate::ProviderSwap`], the configured-auth checks that gate a candidate, …
```

Leave `mod.rs`'s twenty `[`run`]`/`[`commands`]`/… module links and `types.rs`'s `[`crate`]` alone —
those targets are in scope and resolve.

## Do NOT

- Touch any of the 6 pre-existing `links to private item` warnings.
- Reword any doc comment carried over from `session.rs` — the Pi-parity commentary is the crate's
  design record. Items 1–3 add imports; only item 4 edits a doc, and that doc is new.
- Add `#[allow(unused_imports)]` anywhere — `#[cfg(doc)]` makes it unnecessary.
- Move code between modules, change any visibility, or run `cargo fmt`.

## Definition of done

```bash
# 1. exactly 8 warnings under src/session/ — the pre-refactor count, and the same 6 + 2 classes
cargo doc --workspace --no-deps --message-format short 2>&1 | grep -c 'src/session/.*warning'   # 8

# 2. no `unresolved link` left in src/session/
cargo doc --workspace --no-deps --message-format short 2>&1 \
  | grep 'src/session/' | grep -c 'unresolved link'                                              # 0

# 3. crate-wide back to the CARGO_DOC_WARNINGS.md baseline
cargo doc --workspace --no-deps --message-format short 2>&1 \
  | grep -c 'cyrup-session-svc/src/.*warning'                                                    # 19

# 4. the cfg(doc) imports cost nothing in a normal build — still zero under src/session/
cargo check  -p cyrup-session-svc --all-targets --message-format short 2>&1 | grep -c 'src/session/'  # 0
cargo clippy -p cyrup-session-svc --all-targets --message-format short 2>&1 | grep -c 'src/session/'  # 0

# 5. nothing else moved
cargo test -p cyrup-session-svc      # 311 passed, 0 failed
```

## Not actionable — recorded so it is not re-litigated

The original plan's `git mv` step and its `git diff -M --stat shows renames` bullet cannot hold for a
1→21 split: rename detection needs one successor ≥50% similar to the original, and the largest piece
here is 11% of it. `git mv` would have produced byte-identical results. `git log --follow` and
`git blame` stop at the split commit for the new files — inherent to splitting a file, not a defect.
