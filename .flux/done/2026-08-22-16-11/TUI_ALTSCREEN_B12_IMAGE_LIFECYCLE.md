---
stage: qa
status: completed
updated: 2026-08-27
---

# Manage The Inline-Image Lifecycle While The Alternate Screen Owns Every Cell

> **ADR-0005 work unit B-12** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-3 (enter/leave), B-5 (scroll offset drives eviction) · **Effort:** M/L
## Objective

When you own every cell, inline images must be placed, tracked and garbage-collected as they scroll
out of view. This interacts with cyrup's existing `ratatui-image` path and its capability detection,
and it is the unit most likely to be underestimated.

## Upstream reference

[`tui-alt-screen.ts`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts) `:220-226` and `:285-350`:

- `:267-269` — **suppress iterm2 images while the alt screen is active** (`setCapabilities({ ...capabilities, images: null })`), and restore the saved capabilities on exit (`:330-331`).
- `deleteKittyImages` (`:336`) — delete kitty placements on stop.
- `prepareKittyScreen` (`:340-395`) — re-place visible images and **evict** placements that scrolled
  out of view, under pi's caps: **16 images / 32 MB transmitted / 64 MB decoded** (`:58-60`).

## Current state in cyrup-tui

- `ratatui-image` is a workspace dependency, wired for Kitty/iTerm2/sixel per
  [`Cargo.toml:91-94`](../../crates/cyrup-tui/Cargo.toml).
- Capability detection exists; there is no alt-screen-aware suppression or restore, and no placement
  registry or eviction of any kind.

## Subtasks

1. On alt-screen enter, save the current capabilities and suppress iterm2 images; restore on exit.
2. A placement registry keyed by image id, tracking transmitted and decoded bytes.
3. On each render, re-place images inside the viewport and evict those outside it.
4. Enforce the three caps (16 / 32 MB / 64 MB), evicting least-recently-visible first.
5. Delete all kitty placements on stop, before B-3's teardown sequence.

## Acceptance criteria

- With iterm2 detected, no iterm2 image is emitted while the alt screen is active, and the original
  capability set is restored byte-identically on exit.
- Scrolling an image out of view and back re-places it without leaking the prior placement.
- Exceeding any of the three caps evicts rather than growing.
- Stopping the renderer leaves no kitty placement behind.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
