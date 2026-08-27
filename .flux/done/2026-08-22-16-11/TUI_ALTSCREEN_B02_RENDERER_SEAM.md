---
stage: qa
status: completed
updated: 2026-08-27
---

# Introduce A Renderer Trait Both The Inline App And The Alt-Screen Renderer Satisfy

> **ADR-0005 work unit B-2** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** nothing — can proceed in parallel with B-1 · **Effort:** M
## Objective

Two renderers need one seam, so the shell can drive either without branching at every call site and
so B-14's live mode switch has something to swap.

## Upstream reference

pi's `ViewportTUI` interface,
[`packages/tui/src/tui.ts:322-330`](../../tmp/pi/packages/tui/src/tui.ts). `TuiAltScreen`
implements it (`tui-alt-screen.ts:167`: `export class TuiAltScreen extends TuiBase implements
ViewportTUI`), as does the main-screen renderer
([`tui-main-screen.ts`](../../tmp/pi/packages/tui/src/tui-main-screen.ts), 654 lines).

## Current state in cyrup-tui

- [`app/shell.rs:12-16`](../../crates/cyrup-tui/src/app/shell.rs) — `App::new` hard-codes
  `TerminalOptions { viewport: Viewport::Inline(height.max(1)) }`. There is one renderer and no seam.
- [`app/mod.rs:158`](../../crates/cyrup-tui/src/app/mod.rs) imports `Terminal, TerminalOptions, Viewport`
  directly into `App`.
- `App` is generic over `B: Backend` already, so the type machinery for a second implementation exists.

## Subtasks

1. Define the trait (name it for cyrup, not for pi) with the operations ADR-0005 §B-2 names:
   a `set_layout_root` equivalent, `scroll_by`, `scroll_to_top`, `scroll_to_bottom`, `flash`.
2. Implement it for the existing inline `App`. The scroll operations are no-ops inline — pi's main
   screen does not scroll either; the terminal does.
3. Keep `App::new` and the inline renderer the default construction path. Nothing else changes yet.

## Acceptance criteria

- The trait exists with all five operations and is implemented by the inline renderer.
- The inline renderer's rendered output is unchanged: existing render tests pass untouched.
- No call site outside the seam constructs a `Terminal` directly.
- The trait's doc comment cites `tui.ts:322-330` as its upstream.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
