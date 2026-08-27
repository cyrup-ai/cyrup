---
stage: qa
status: completed
updated: 2026-08-27
---

# Repaint The Document Into The Main Screen On Exit, And Delete The Interim 'Not Built Yet' Message

> **ADR-0005 work unit B-13** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-1, B-3, and everything that must work before the flag is honoured · **Effort:** M
## Objective

Leave the alt screen without the user's transcript vanishing — and remove the temporary refusal that
ADR-0005 §A-2 deliberately planted so it could be found and deleted here.

## Upstream reference

[`tui-alt-screen.ts:265-283`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts) — on stop **without**
`preserveScreen`, the last rendered document is written back into the main screen row by row
(`:323-329`); **with** `preserveScreen` (a mode switch, not an exit) it is not, because the incoming
renderer is about to paint.

## Current state in cyrup-tui — the tripwire

ADR-0005 §A-2 required the interim refusal to name the ADR so a grep would find it:

- [`crates/cyrup/src/main.rs:215`](../../crates/cyrup/src/main.rs) —
  `"--tui-mode fullscreen is not built yet in this release (ADR-0005); falling back to regular."`
- [`crates/cyrup/src/main.rs:209`](../../crates/cyrup/src/main.rs) — the comment above it says
  ADR-0005 §Decision A.2 "also fixes this wording".
- [`crates/cyrup/src/cli/enums.rs:77`](../../crates/cyrup/src/cli/enums.rs) —
  whose doc comment still reads "Not built yet":

  ```rust
  /// The alternate-screen renderer (`tui-alt-screen.ts` @v0.84.1). Not built yet — ADR-0005.
  ```
- [`crates/cyrup-it/tests/bin/tui_mode_flag.rs:131-139`](../../crates/cyrup-it/tests/bin/tui_mode_flag.rs)
  pins that exact string.

The CLI flag itself (`TuiMode::Fullscreen`, `enums.rs:78`) is **already implemented** — A-1 shipped.
Only the refusal remains.

## Subtasks

1. Implement exit repaint: on stop without `preserve_screen`, write the last rendered document back
   into the main screen; with `preserve_screen`, do not.
2. Delete the A-2 interim message and let `--tui-mode fullscreen` actually select the renderer.
3. Update the `enums.rs:77` doc comment — it must no longer say "Not built yet".
4. The integration test at `tui_mode_flag.rs:131-139` pins the removed string. Coordinate its
   retirement with the team that owns the test suite; **do not edit the test yourself** under this
   task's constraints. Flag it in the task's completion notes.

## Acceptance criteria

- Leaving fullscreen normally leaves the transcript visible in the main screen.
- Leaving via a mode switch (`preserve_screen`) does not double-paint.
- `grep -rn 'not built yet' crates/` returns nothing.
- `--tui-mode fullscreen` starts the alt-screen renderer instead of falling back.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
