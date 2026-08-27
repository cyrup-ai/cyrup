---
stage: qa
status: completed
updated: 2026-08-27
---

# Jump To Previous/Next Prompt By Entry Index — Not By Re-Parsing OSC 133

> **ADR-0005 work unit B-10** — [`docs/adr/ADR-0005-alt-screen-tui-mode.md`](../../docs/adr/ADR-0005-alt-screen-tui-mode.md) §Decision B.
> Status **accepted**: cyrup builds the alternate-screen TUI mode. This file is one of the fourteen
> units that ADR requires `TUI-019` to be decomposed into before batch 30b is scheduled.
> **Depends on:** B-1 (retained entries), B-5 (scroll), B-9 (the two ids) · **Effort:** M
## Objective

`ctrl+shift+up`/`ctrl+shift+down` jump between prompts. cyrup reaches the identical result by a
**deliberately different mechanism**, and ADR-0005 authorises the difference explicitly.

## Upstream reference

[`tui-alt-screen.ts:56`](../../tmp/pi/packages/tui/src/tui-alt-screen.ts), `:366-379`, and
`scrollToPrompt` at `:412-425` — pi scans **rendered lines** for `\x1b]133;A` (the OSC 133 prompt
mark) because its renderer only has lines to work with.

## The sanctioned mechanism difference (ADR-0005 §B-10)

> "**Allowed mechanism difference, zero behavioural cost:** pi scans rendered lines for `\x1b]133;A`
> because its renderer only has lines; cyrup retains structured `Entry`s (B-1) and must jump by
> `Entry::User` row index instead. Reason: cyrup emits no OSC 133 today, and manufacturing marks
> purely so they can be re-parsed is a strictly worse mechanism for the identical result."

`grep -rn ']133\|OSC133' crates/ --include='*.rs'` returns nothing — cyrup emits no prompt marks.
**Do not add OSC 133 emission to satisfy this task.**

## Current state in cyrup-tui

After B-1 the retained document is a `Vec<Entry>`; `Entry::User` is directly identifiable without
any text scanning. This is strictly more reliable than pi's regex over rendered output.

## Subtasks

1. Map each retained `Entry::User` to its first rendered row in the scroll document.
2. `previousPrompt`/`nextPrompt` select the first such row **strictly past the current scroll offset,
   in the search direction**, and scroll to it.
3. No-op when there is no match in that direction — do not wrap, do not clamp to an edge.

## Acceptance criteria

- The jump lands on the same row a pi user lands on for an equivalent transcript.
- The "first match strictly past the current `scrollTop`, in the search direction" rule holds —
  repeatedly pressing the same binding advances rather than sticking.
- With no match ahead, the binding does nothing at all.
- `grep -rn ']133' crates/cyrup-tui/src` still returns nothing.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic` and `indexing_slicing`; `cyrup-tui` also
  has `forbid(unsafe_code)` and `deny(clippy::string_slice)`.
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
- **Do not change the inline (`regular`) renderer's behaviour.** It stays the default; fullscreen is
  an additional mode, not a replacement (ADR-0005 §Decision B).
