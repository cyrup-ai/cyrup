---
stage: qa
status: completed
updated: 2026-08-27
---

# Add The Scroll View Over The Retained Document, With The fullscreenScrollbar Modes

> **ADR-0005 work unit B-5** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-1 (retention), B-2 (seam) — and see the A-3 prerequisite below · **Effort:** M/L
## Objective

Make the retained document scrollable and draw the thumb. This is the unit that turns B-1's buffer
into something a user can actually move through.

## Upstream reference

- [`components/scroll-view.ts:4-78`](../../tmp/pi/packages/tui/src/components/scroll-view.ts) —
  the view itself; `follow: end`; the **1000 ms transient-hide timer** at `:46`, `:65-70`.
- Root built at `interactive-mode.ts:869-885`; the setting applied at `:1895`.
- Overscroll chaining across nested scroll views: `tui-alt-screen.ts:489-501`.

Scrollbar modes: `always` reserves the rightmost column permanently; `auto` shows it only while
content exceeds the viewport **and** activity is recent (1000 ms); `hidden` never.

## Current state in cyrup-tui

- `ratatui::widgets::Scrollbar` + `ScrollbarState` are available in ratatui 0.30.2 and render the
  thumb. cyrup uses neither today.
- [`theme.rs:1032-1037`](../../crates/cyrup-tui/src/theme.rs) — `scrollbarThumb` **already resolves
  correctly**, with pi's `?? selectedBg` fallback. The theme half is ported; only the painter is
  missing, as that file's own comment says.
- The `auto`/`always`/`hidden` selection and the 1000 ms timer are application state either way —
  ratatui provides the drawing, not the policy.

## ⚠ Unmet prerequisite (ADR-0005 §Decision A-3)

`grep -rn 'tui_mode\|fullscreen_scrollbar' crates/cyrup-config/src` returns **nothing**. The
settings keys `tuiMode` (default `regular`) and `fullscreenScrollbar` (default `auto`) were
scheduled under A-3 and are **not implemented**. Either land A-3 first or thread a default through
and file the settings work; do not invent a third config surface.

## Subtasks

1. A scroll view over B-1's retained document: scroll offset, viewport height, `follow: end`
   (stick to the bottom until the user scrolls away, then hold position).
2. `scroll_by` / `scroll_to_top` / `scroll_to_bottom` on the B-2 seam drive it.
3. Overscroll chaining per `:489-501`.
4. Draw with `ratatui::widgets::Scrollbar`, styled from the already-resolved `scrollbarThumb`.
5. Implement the three modes and the 1000 ms transient-hide timer for `auto`.

## Acceptance criteria

- With content shorter than the viewport, `auto` never shows a thumb and `always` still reserves the
  column.
- Under `auto`, the thumb appears on scroll activity and disappears 1000 ms after the last one.
- `follow: end` keeps new output visible until the user scrolls up, and does not yank them back down.
- `hidden` never reserves the column.
- The thumb colour comes from `theme.rs`'s existing `scrollbarThumb` resolution — no new theme key.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
