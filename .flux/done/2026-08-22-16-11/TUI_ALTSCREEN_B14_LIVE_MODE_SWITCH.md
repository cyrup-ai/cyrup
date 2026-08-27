---
stage: qa
status: completed
updated: 2026-08-27
---

# Switch Renderers Live From /settings With No Restart, And Restore The Two Omitted Settings Rows

> **ADR-0005 work unit B-14** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-13 (renderer fully works) — and the A-3 settings prerequisite · **Effort:** L
## Objective

The last unit: change `tuiMode` in `/settings` and have the renderer swap underneath a running
session, preserving everything the user had.

## Upstream reference

- `switchTuiMode`,
  [`interactive-mode.ts:795-830`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)
  — swaps renderer while preserving focus, `clearOnShrink`, `onDebug`, main-screen render state,
  extension input listeners and theme bindings.
- The stable-reference `Proxy` at `:355-372` is what makes the swap invisible to holders of the old
  reference. cyrup's equivalent is the B-2 seam behind an indirection — not a `Proxy`.
- `/settings` rows: `settings-selector.ts:633-645`, fed from `interactive-mode.ts:4411-4412`.
  `Fullscreen scrollbar` is documented "no effect in regular mode".
- pi keeps every message component alive in **both** modes, which is why the identical component set
  can be handed to the new renderer (`:808-822`) — that is what B-1 makes possible here.

## ⚠ Unmet prerequisite (ADR-0005 §Decision A-3)

The settings keys do not exist: `grep -rn 'tui_mode\|fullscreen_scrollbar' crates/cyrup-config/src`
returns **nothing**. A-3 requires `tuiMode: TuiMode` and `fullscreenScrollbar: ScrollViewScrollbar`
in `crates/cyrup-config/src/settings.rs` with pi's defaults (`regular`, `auto`) and pi's degrading
getter — `settings.tuiMode === "fullscreen" ? "fullscreen" : "regular"`
(`settings-manager.ts:1129`), so any other value degrades to `regular` rather than erroring.
**Both keys must round-trip byte-faithfully**: a `settings.json` written by pi with
`tuiMode: "fullscreen"` must survive a cyrup read-modify-write untouched. This unit cannot complete
without A-3; land it first or file it.

## Why the rows are absent today

ADR-0005 §A-4 deliberately **omitted** the `TUI mode` and `Fullscreen scrollbar` rows from
`/settings` until the renderer existed — capability-gating a settings row is pi's own idiom
(`settings-selector.ts:490`, `:657`, `:676` omit the image rows with no image protocol). Shipping
them earlier would have shipped two lying controls. This unit re-adds them.

## Subtasks

1. Put the active renderer behind the B-2 seam with an indirection that survives replacement.
2. Implement the swap, preserving what `:795-830` preserves: focus, render state, extension input
   listeners, theme bindings.
3. Hand the retained document (B-1) to the incoming renderer rather than rebuilding it.
4. Re-add the two `/settings` rows, with `Fullscreen scrollbar` documented as having no effect in
   regular mode.

## Acceptance criteria

- Switching mode from `/settings` swaps the renderer with no restart and no lost transcript.
- Focus and theme survive the swap.
- Switching regular → fullscreen → regular returns to a working inline renderer with history intact.
- The two rows appear only once the renderer exists, and `Fullscreen scrollbar` states its no-op.
- A `settings.json` round-trip preserves an unknown `tuiMode` value rather than rewriting it.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
