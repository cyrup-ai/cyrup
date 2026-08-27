---
stage: new
status: pending
priority: LOW
tool: all
source: aug follow-up from the OSC-8 regression-test task
updated: 2026-08-27 14:50
---

# `osc::inject`'s `ForcedWidth(1)` understates a wide head grapheme

`crates/cyrup-tui/src/osc.rs` defines `UNIT_WIDTH = CellDiffOption::ForcedWidth(1)`
and applies it to the head and tail cells of every marked run. When the head cell
holds a **wide** grapheme — a two-column CJK character, an emoji — the forced
value understates its real width by one column for the diff cursor.

Verified against ratatui-core 0.1.2: `diff_iter` advances by `cell_width()`
(`buffer/diff.rs:133-140`) and `Cell::cell_width` returns the forced value once
set (`buffer/cell.rs:309-317`).

## Reachable trigger

A path component that is itself wide, e.g. `read プロジェクト/x.rs`, puts a
two-column grapheme in the head cell of the linked run.

## Why it was not fixed in the test task

Closing this means editing `osc.rs`, which the OSC-8 regression-test task's own
Definition of Done forbids — that task adds tests only. A test written there
would either assert the defect or fail on landing, so it was correctly recorded
rather than forced.

## Parity action

Capture `cell.symbol().cell_width()` (trait `CellWidth`, ratatui-core
`buffer/cell_width.rs:19-24`) **before** prepending the escape, and force that
value instead of the constant `1`. The tail cell needs the same treatment.

Anchor by symbol — `UNIT_WIDTH`, `inject` — not by line number.

## Definition of done

1. A linked header whose path contains a two-column grapheme renders with
   columns unmoved relative to the unlinked render.
2. `strip_ansi(linked) == plain` byte-for-byte for that case.
3. `CellDiffOption::Skip` is still not used anywhere.
4. Existing OSC-8 behaviour for all-narrow paths is unchanged.
