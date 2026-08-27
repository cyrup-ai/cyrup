---
stage: qa
status: completed
updated: 2026-08-27
---

# Emit pi's Multiplexer-Aware Mouse Sequences Directly — Do NOT Use crossterm's EnableMouseCapture

> **ADR-0005 work unit B-4** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-3 (terminal setup) · **Effort:** S — but the detail below is the whole task
## Objective

Turn mouse reporting on. The convenience API is a trap here, and this is the one place in the entire
fullscreen feature where ADR-0005 found that the obvious call is **wrong**.

## Upstream reference

[`tui-alt-screen.ts:48-49`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts), selected at `:236-247`.
pi emits:

| context | sequence |
|---|---|
| under a multiplexer | `?1000h ?1002h ?1004h ?1006h` |
| everywhere else | `?1000h ?1002h ?1003h ?1004h ?1006h` |

Multiplexer detection: `TMUX` / `ZELLIJ` / `STY` set, or `TERM` starting `tmux`/`screen`.

## Current state in cyrup-tui

- Mouse events are already parsed by crossterm and then **discarded**:
  [`app/input_reader.rs:433`](../../crates/cyrup-tui/src/app/input_reader.rs) is
  `Event::Mouse(_) => None`.
- No mouse enable sequence is emitted anywhere. `into_stdout` pushes `EnableBracketedPaste` +
  `PushKeyboardEnhancementFlags`, no `EnableFocusChange`, no mouse capture.

## Why `EnableMouseCapture` must not be used

`crossterm::event::EnableMouseCapture` (crossterm 0.29.0, `src/event.rs:321-336`) emits
`?1000h ?1002h ?1003h ?1015h ?1006h` **unconditionally**. Three differences from pi, each a real
defect:

1. It always turns on **any-motion tracking (`?1003h`)**. pi deliberately does not under
   tmux/zellij/screen — forwarding every pointer movement makes multiplexers lag.
2. It adds rxvt `?1015h`, which pi does not.
3. **It never enables focus reporting (`?1004h`), and pi's alt-screen input handler depends on
   `FOCUS_OUT` to cancel an in-progress selection** (`tui-alt-screen.ts:386-403`). Without it, a
   drag that leaves the window never ends.

crossterm `Command`s are plain ANSI writers, so emitting the literals is a `queue!` of escapes —
not a fork, not unsafe, not a workaround.

## Subtasks

1. Write a multiplexer probe over `TMUX`/`ZELLIJ`/`STY` and the `TERM` prefix check.
2. Emit the correct literal enable sequence for the detected context on alt-screen enter, and the
   matching disable sequence on leave (paired with B-3's teardown ordering).
3. Enable focus reporting and route `Event::FocusLost` to the seam so B-8 can cancel a live drag.
4. Stop discarding `Event::Mouse` when the alt-screen renderer is active; route it to the renderer.
   The inline path keeps discarding it.

## Acceptance criteria

- `grep -rn 'EnableMouseCapture' crates/cyrup-tui/src` returns nothing.
- Under a simulated `TMUX` env the emitted bytes contain `?1002h` and **not** `?1003h`; without it
  they contain `?1003h`.
- `?1004h` is emitted in both cases, and `?1015h` in neither.
- Disable sequences are the exact inverse and fire before `LeaveAlternateScreen`.
- `Event::Mouse` still returns `None` on the inline path.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
