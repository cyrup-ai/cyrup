---
stage: qa
status: completed
updated: 2026-08-27
---

# Make The Scrollbar Interactive — Hit-Test, Hover And Thumb Drag

> **ADR-0005 work unit B-7** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-4 (mouse), B-5 (scrollbar drawn) · **Effort:** M
## Objective

ratatui's `Scrollbar` **draws** the thumb; it does not respond to a pointer. Everything a user does
to a scrollbar is application work.

## Upstream reference

[`tui-alt-screen.ts:526-604`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts) — hit-test, hover
state and thumb drag.

## Current state in cyrup-tui

Nothing: the scrollbar does not exist until B-5, and mouse events are discarded until B-4.

## Subtasks

1. Hit-test a mouse position against the scrollbar column and the thumb's current extent.
2. Hover state — the thumb's appearance changes under the pointer.
3. Drag: on press within the thumb, record the grab offset; on motion, map pointer row to scroll
   offset preserving that offset so the thumb does not jump under the cursor; on release, end.
4. A press in the **trough** (outside the thumb) pages toward the click, matching `:526-604`.
5. Cancel an in-progress drag on focus loss, using the `?1004h` focus reporting B-4 enables.

## Acceptance criteria

- Grabbing the thumb mid-way and dragging does not snap the thumb's centre to the pointer.
- Trough clicks page rather than jumping to an absolute position.
- Hover is visually distinct from idle.
- A drag interrupted by focus loss ends cleanly and leaves no captured state.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
