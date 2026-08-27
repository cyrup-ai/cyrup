---
stage: qa
status: completed
updated: 2026-08-27
---

# Text Selection, Clipboard Copy, OSC-8 Link Activation And Right-Click Paste

> **ADR-0005 work unit B-8** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-4 (mouse + focus reporting) · **Effort:** L — ~350 upstream lines, the largest unit after B-1
## Objective

**Capturing the mouse takes the terminal's own selection away from the user.** A renderer that
captures the mouse therefore owes the user a replacement. ratatui provides none of this.

This is not an optional polish unit — without it, turning on fullscreen mode is a net regression for
anyone who copies text out of their terminal.

## Upstream reference

[`tui-alt-screen.ts`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts) `:514-524` and `:605-963`:

- character / word / line granularity selected by **click count**
- drag to extend, with **edge auto-scroll** when the pointer leaves the viewport
- highlight rendering over the selected range
- clipboard copy
- **OSC-8 URL activation on click**
- right-click paste
- `FOCUS_OUT` cancels an in-progress selection (`:386-403`) — this is why B-4 must enable `?1004h`

## Current state in cyrup-tui

- `arboard` is **already a dependency and already drives clipboard reads** —
  [`app/event_extract.rs:95-97`](../../crates/cyrup-tui/src/app/event_extract.rs) documents it as
  "the faithful Rust analog" for pi's clipboard path. Reuse it; do not add a second clipboard crate.
- No selection model, no highlight rendering, no OSC-8 handling exists.

## Subtasks

1. A selection model over B-1's retained document: anchor, focus, and granularity
   (char / word / line by click count).
2. Press / drag / release handling, with edge auto-scroll while dragging beyond the viewport.
3. Highlight the selected range at render time. Selection is view state — it must not mutate entries.
4. Copy to clipboard through the existing `arboard` path.
5. OSC-8 hyperlink hit-test and activation on click.
6. Right-click paste.
7. Cancel on `FOCUS_OUT`.

## Acceptance criteria

- Double-click selects a word, triple-click a line, single-click-drag a character range.
- Dragging past the top or bottom edge auto-scrolls and keeps extending.
- The copied string matches the visible selected text, including across wrapped rows.
- Clicking an OSC-8 link activates its target; clicking plain text does not.
- Focus loss mid-drag clears the selection state rather than leaving it armed.
- No new clipboard dependency is introduced.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
