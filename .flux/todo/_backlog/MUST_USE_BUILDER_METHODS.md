---
stage: new
status: done
updated: 2026-08-22 19:42
---

# Add #[must_use] To The Ten cyrup-tui Builder Methods Clippy Now Flags

> Found while re-verifying `EDITOR_MODULE_DECOMPOSE` after rebasing onto current `main`.
> **Priority:** low · **Effort:** small

## Description

`cargo clippy -p cyrup-tui --all-targets` reports **11 warnings** on current `main`, up from
the **1** that held for most of this session (`escape_reassembly.rs:972`, "can be more
succinctly written as a byte str"). The 10 new ones are all
`clippy::must_use_candidate` — *"missing `#[must_use]` attribute on a method returning
`Self`"* — on builder-style methods:

| file:line | |
|---|---|
| `crates/cyrup-tui/src/selector.rs:619` | |
| `crates/cyrup-tui/src/selector.rs:626` | |
| `crates/cyrup-tui/src/selector.rs:632` | |
| `crates/cyrup-tui/src/selector.rs:658` | |
| `crates/cyrup-tui/src/theme.rs:266` | |
| `crates/cyrup-tui/src/theme.rs:277` | |
| `crates/cyrup-tui/src/settings_selector.rs:645` | |
| `crates/cyrup-tui/src/settings_selector.rs:658` | |
| `crates/cyrup-tui/src/text_input.rs:69` | |
| `crates/cyrup-tui/src/select_list.rs:107` | |
| `crates/cyrup-tui/src/app/backend.rs:11` | |

(Eleven rows, ten `must_use` plus the pre-existing `escape_reassembly.rs:972`.)

**These are not caused by any branch work.** They appeared when `main` absorbed PRs #40–#50 —
a fleet of sibling crate decompositions — and none of the flagged files was touched by the
`transcript` or `editor` splits. Verified: zero clippy findings cite
`crates/cyrup-tui/src/transcript/` or `crates/cyrup-tui/src/editor/`.

The lint is a real, if minor, correctness signal: a builder method returning `Self` whose
result is discarded is a silent no-op at the call site. `#[must_use]` makes that a warning.

## Required implementation

Add `#[must_use]` above each of the ten methods. Read each one first — the attribute is only
correct where the method genuinely returns a *new or transformed* `Self` and has no useful
side effect. If any of the ten mutates through `&mut self` and returns `Self` for chaining
only, note it rather than annotating blindly.

Where a method already carries a doc comment, the attribute goes between the doc and the
signature, matching the crate's existing style (`grep -rn '#\[must_use\]' crates/cyrup-tui/src`
for precedent).

Do not add a blanket `#![allow(clippy::must_use_candidate)]`. The lint is off by default in
most configurations and is firing here deliberately.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-tui --all-targets` reports exactly **1** cyrup-tui warning — the pre-existing `escape_reassembly.rs:972` byte-str one
- [ ] All ten sites carry `#[must_use]`, or any site skipped is justified in the commit message with the reason it is not a must-use candidate
- [ ] No `#![allow(clippy::must_use_candidate)]` was added anywhere
- [ ] `cargo build -p cyrup-tui` emits 0 warnings and `cargo test -p cyrup-tui` passes with the test count unchanged

## Evidence

Measured on `origin/main` at `9f20268` with the editor branch rebased on top:
`cyrup-tui (lib) generated 11 warnings`, `cyrup-tui (lib test) generated 12 warnings
(11 duplicates)`. Locations enumerated above via
`grep -oE 'crates/cyrup-tui/src/[A-Za-z0-9_/]+\.rs:[0-9]+'` over the clippy log.
Zero findings cite `src/editor/` or `src/transcript/`. Session baseline before main's
PRs #40–#50 landed: 1 cyrup-tui warning.
