---
stage: qa
status: completed
updated: 2026-08-27
---

# Route Wheel Events To The Scroll View, With Overscroll Chaining

> **ADR-0005 work unit B-6** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-4 (mouse events arriving), B-5 (scroll view) · **Effort:** S
## Objective

Make the mouse wheel scroll the transcript.

## Upstream reference

[`tui-alt-screen.ts:462-501`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts) — wheel handling and
the overscroll chaining rule at `:489-501`: a nested scroll view that cannot move further passes the
remainder to its parent rather than swallowing it.

## Current state in cyrup-tui

Wheel events arrive as `Event::Mouse` and are dropped at
[`app/input_reader.rs:433`](../../crates/cyrup-tui/src/app/input_reader.rs). B-4 stops the drop; this
unit gives them a destination.

## Subtasks

1. Map `MouseEventKind::ScrollUp`/`ScrollDown` to `scroll_by` on the seam.
2. Implement the chaining rule from `:489-501` — hit-test to the innermost scroll view under the
   pointer, and pass the unconsumed delta outward when it is already at its limit.
3. Ensure a wheel event marks scroll activity so B-5's `auto` thumb appears.

## Acceptance criteria

- Wheel up/down moves the transcript by pi's per-notch amount.
- A nested scroll view at its top passes further up-scroll to the parent instead of absorbing it.
- Wheel activity triggers the `auto` scrollbar's visibility window.
- Wheel events on the inline path remain ignored.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
