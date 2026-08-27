---
stage: qa
status: completed
updated: 2026-08-27
---

# Give TranscriptView A Retained Document Mode So Fullscreen Has Something To Scroll

> **ADR-0005 work unit B-1** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** nothing — this is the keystone unit · **Effort:** L
## Objective

Fullscreen needs a scrollable document. cyrup currently throws that document away: committed entries
are pushed to the terminal's native scrollback and dropped from memory, so there is nothing to
re-render. This is the single largest structural change in the whole feature, and every scrolling,
search, selection and prompt-jump unit below depends on it.

## Upstream reference

pi keeps every message component alive in `chatContainer` in **both** modes. That is why
`switchTuiMode` can hand the identical component set to the new renderer
([`interactive-mode.ts:808-822`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts))
and why the alt screen can simply wrap `documentContainer` in a `ScrollView` (`:869-885`).
There is no upstream counterpart to dropping history — the drop is cyrup-only.

## Current state in cyrup-tui

- [`transcript/view.rs:94-97`](../../crates/cyrup-tui/src/transcript/view.rs) — `drain_committed`
  is `std::mem::take(&mut self.pending)`. The entries are returned once, rendered to native
  scrollback, and gone.
- [`transcript/mod.rs:100-107`](../../crates/cyrup-tui/src/transcript/mod.rs) — the module doc states
  the contract: entries "are emitted to the terminal's native scrollback with
  `Terminal::insert_before` and never re-rendered inside the viewport" (R-ARCH-TUI-003).
- [`transcript/mod.rs:136`](../../crates/cyrup-tui/src/transcript/mod.rs) — "entries already flushed
  to the terminal's NATIVE scrollback via `insert_before` cannot be repainted at all (they are the
  terminal's cells now, not cyrup's)".
- `TranscriptView` holds `pending` + `streaming` + `thinking` + live tool/bash state, and nothing
  historical.

**This constraint is descriptive of the inline mode, not prescriptive for the crate.** ADR-0001
explicitly does **not** decide whether cyrup ships fullscreen — it defers that to ADR-0005, which
decided to build it.

## Subtasks

1. Add a retention flag to `TranscriptView` (set when `tui_mode == fullscreen`).
2. When retention is on, `drain_committed` still returns the entries for the caller, but
   `TranscriptView` **keeps** them in a retained `Vec<Entry>` document rather than taking them.
   When retention is off the current `mem::take` behaviour is byte-identical.
3. Expose the retained document to the renderer (a slice accessor); the inline path must not read it.
4. Repoint the R-ARCH-TUI-003 doc comments in `transcript/view.rs` and `transcript/mod.rs` to say the
   drop is the **inline mode's** strategy and name this unit as the fullscreen alternative. A future
   agent reading `drain_committed` must not conclude the feature is impossible.
5. Decide and document the retention bound (entry count or rendered-line cap) and where trimming
   happens. Unbounded growth over a long session is not acceptable.

## Acceptance criteria

- With `tui_mode` regular, `drain_committed` leaves `pending` empty — behaviour unchanged.
- With retention on, the retained document contains every committed entry in commit order after N
  drains, and the same entries are still returned to the caller each time.
- `transcript/view.rs` and `transcript/mod.rs` no longer state the drop as an unqualified
  crate-wide rule, and both name ADR-0005 §B-1.
- `grep -rn 'R-ARCH-TUI-003' crates/cyrup-tui/src` returns no comment that reads as a blanket
  impossibility claim.
- The retention bound is stated in a doc comment with the rule that enforces it.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
