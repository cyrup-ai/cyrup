---
stage: qa
status: completed
updated: 2026-08-27
---

# Add The Flash Stack, And Fork /copy To Flash In Fullscreen While Keeping The Status Line In Regular

> **ADR-0005 work unit B-11** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-2 (seam exposes `flash`) · **Effort:** S/M
## Objective

Transient overlay notices. In fullscreen there is no status line to write to, so pi flashes instead —
and `/copy` is the case where the two modes must diverge on purpose.

## Upstream reference

- [`components/alt-screen-flash.ts`](../../tmp/pi/packages/tui/src/components/alt-screen-flash.ts)
  — the flash stack, 1000 ms default duration.
- `interactive-mode.ts:5957-5962` — the `/copy` fork: flash `Copied!` in fullscreen, status line in
  regular.

## Current state in cyrup-tui

`flash` is one of the five operations on the B-2 seam. Inline it is a no-op (the status line already
serves this purpose); the alt-screen renderer implements it.

## Subtasks

1. A flash stack on the alt-screen renderer: queued messages, each with a duration, default 1000 ms,
   rendered as an overlay.
2. Implement `flash` on the alt-screen renderer; leave the inline implementation a no-op.
3. Fork `/copy`: flash `Copied!` when fullscreen, keep the existing status-line path when regular.

## Acceptance criteria

- A flash appears and clears itself after its duration with no further input.
- Two flashes in quick succession stack rather than the second replacing the first mid-display.
- `/copy` in regular mode produces exactly the status-line output it does today — unchanged.
- `/copy` in fullscreen produces a `Copied!` flash and no status-line write.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
