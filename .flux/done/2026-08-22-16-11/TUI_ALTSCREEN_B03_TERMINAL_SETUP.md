---
stage: qa
status: completed
updated: 2026-08-27
---

# Enter And Leave The Alternate Screen Around A Fullscreen Viewport, With pi's Exact Teardown

> **ADR-0005 work unit B-3** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-2 (renderer seam) · **Effort:** S/M
## Objective

Stand up the alt-screen terminal itself: enter, configure, and — critically — tear down in pi's
order so a mode switch or a crash does not leave the user's terminal wedged.

## Upstream reference

[`packages/tui/src/tui-alt-screen.ts`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts)
`:44-54`, `:236-250`, `:252-262`, `:265-288` — enter/leave alt screen, autowrap off/on, cursor
hide, and **synchronized output (`?2026h`/`?2026l`) around teardown**.

## Current state in cyrup-tui

- **The primitive already exists in this crate.**
  [`startup_selector.rs:20`](../../crates/cyrup-tui/src/startup_selector.rs) imports
  `EnterAlternateScreen`/`LeaveAlternateScreen` and `:77` executes it around the pre-session wizard,
  with `StartupTerminalRestore` as a `Drop` guard armed the instant raw mode is on.
- `Viewport::Fullscreen` is ratatui's default and is **simpler** than the inline viewport cyrup
  already maintains — the inline path needs `RebuildBackend`/reanchor machinery precisely because
  `Viewport::Inline` is immutable after construction.
- No autowrap, cursor-hide or synchronized-output sequences are emitted anywhere today.

## Subtasks

1. Construct the alt-screen renderer over a default (`Fullscreen`) viewport terminal.
2. On enter: `EnterAlternateScreen`, autowrap off (`?7l`), cursor hide.
3. On leave: mirror `:265-288` — wrap teardown in `?2026h`/`?2026l`, restore autowrap (`?7h`),
   restore the cursor, `LeaveAlternateScreen`.
4. Follow `startup_selector.rs`'s `Drop`-guard idiom so every exit path — including a panic unwind or
   `?` early return — restores the terminal. This is the existing house pattern; do not invent another.

## Acceptance criteria

- Entering and leaving restores the terminal to its prior state, including autowrap and cursor
  visibility, on the normal path and on an early-return/unwind path.
- Teardown is bracketed by `?2026h`/`?2026l`.
- The restore is driven by a `Drop` guard, not by explicit calls at each return site.
- `preserve_screen` is threaded as a parameter even though B-13 is what consumes it.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
