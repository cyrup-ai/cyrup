---
stage: qa
status: completed
updated: 2026-08-27
---

# Add The Eight tui.altScreen Keybindings, Including The Editor-Shadowing Rule

> **ADR-0005 work unit B-9** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-5 (scroll operations to bind to) · **Effort:** M
## Objective

Bind the scroll surface to keys, with pi's exact ids, defaults and — the part that is easy to lose —
its **shadowing** rule.

## Upstream reference

[`packages/tui/src/keybindings.ts:44-52`](../../tmp/pi/packages/tui/src/keybindings.ts) and
`:153-179` @v0.84.1:

| id | default keys | description |
|---|---|---|
| `tui.altScreen.pageUp` | `pageUp` | Scroll viewport up one page |
| `tui.altScreen.pageDown` | `pageDown` | Scroll viewport down one page |
| `tui.altScreen.halfPageUp` | *(none — `[]`)* | Scroll viewport up half a page |
| `tui.altScreen.halfPageDown` | *(none — `[]`)* | Scroll viewport down half a page |
| `tui.altScreen.previousPrompt` | `ctrl+shift+up` | Jump to previous semantic prompt |
| `tui.altScreen.nextPrompt` | `ctrl+shift+down` | Jump to next semantic prompt |
| `tui.altScreen.top` | `home` | Scroll viewport to top |
| `tui.altScreen.bottom` | `end` | Scroll viewport to bottom |

**Three rules that are behaviour, not decoration:**

1. `keybindings.ts:153` — "These intentionally shadow the unmodified editor bindings in fullscreen
   mode" is **normative**. In fullscreen, `pageUp`/`pageDown`/`home`/`end` scroll the viewport
   instead of moving the editor caret. In regular they keep hitting `tui.editor.pageUp`/`pageDown`.
2. `halfPageUp`/`halfPageDown` ship **unbound** and must still appear in the keybindings surface so
   a user can bind them.
3. Page scroll is `viewport_height - 4`, floor 1 (`PAGE_SCROLL_OVERLAP = 4`,
   `tui-alt-screen.ts:57`, `:425`, `:431`). Half-page is `floor(viewport_height / 2)`, floor 1.

## Current state in cyrup-tui

cyrup's editor ids live at [`keymap.rs:144-149`](../../crates/cyrup-tui/src/keymap.rs) and `:182-183`.
No `altScreen` ids exist.

## Subtasks

1. Register the eight ids with their defaults — two of them with an empty default binding.
2. Implement the shadowing rule as a mode-conditional lookup, so regular mode is untouched.
3. Implement both scroll amounts with their floors.
4. `previousPrompt`/`nextPrompt` dispatch to B-10; wire the ids here even if B-10 lands later.

## Acceptance criteria

- All eight ids resolve, and the two half-page ids resolve with no default keys bound.
- In fullscreen, `pageUp` scrolls the viewport; in regular the same key still moves the editor caret.
- Page scroll on a 30-row viewport moves 26 rows; on a 3-row viewport it moves 1, not -1.
- The two half-page ids appear in whatever surface lists bindings to the user.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
