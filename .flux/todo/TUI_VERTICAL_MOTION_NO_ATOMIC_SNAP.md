---
stage: todo
status: pending
updated: 2026-08-27
---

# Port `moveToVisualLine`'s Atomic-Segment Snap And Its Non-Last-Segment Column Clamp Into The Three Vertical Movers

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** divergent-behaviour · **Area:** Editor, input, keys and autocomplete

## Objective

Up / Down / PageUp / PageDown can currently park the block cursor **inside** a `[paste #N …]`
marker on a wrapped line. The next Backspace then shreds the marker into leftover literal text and
silently drops the pasted content from what is sent to the model — the marker's registry entry is
stranded and `expanded_text` can no longer resolve it. Horizontal motion and deletion are already
marker-atomic; vertical motion is the one hole left, and it is the one that loses user data.

## Upstream reference

[`packages/tui/src/components/editor.ts`](../../tmp/pi/packages/tui/src/components/editor.ts) —
`moveToVisualLine(visualLines, currentVisualLine, targetVisualLine)` at **`:1387-1466`** is the
single primitive behind both vertical entry points: `moveCursor` (`:1801-1813`, the Up/Down arm) and
`pageScroll` (`:1866-1878`). Four things it does that cyrup does not:

1. **Pre-snap column re-resolution** (`:1396-1404`). If `snappedFromCursorCol !== null`, the source
   visual column is derived from the *pre-snap* column resolved through
   `findVisualLineAt(visualLines, currentVL.logicalLine, this.snappedFromCursorCol)` (`:1774-1789`),
   not from the live `cursorCol`. Comment: "This gives the correct visual column even after a resize
   reshuffles VLs."
2. **The non-last-segment clamp** (`:1406-1416`). A visual segment is "last" when it is the final
   entry or the next entry belongs to a different logical line. `sourceMaxVisualCol` /
   `targetMaxVisualCol` are `currentVL.length` / `targetVL.length` only for a last segment, and
   `Math.max(0, length - 1)` otherwise — the wrap-boundary column belongs to the NEXT visual row.
3. **The seven-case sticky-column table**, `computeVerticalMoveColumn(currentVisualCol,
   sourceMaxVisualCol, targetMaxVisualCol)` at `:1489-1518`, documented as a table at `:1470-1488`
   over P (preferred set) / S (cursor mid-source-line) / T (target shorter than current) / U (target
   shorter than preferred). It **clears** `preferredVisualCol` in three of the seven cases (`:1505`,
   `:1516`) and only sets it in two (`:1500`).
4. **The atomic-segment snap** (`:1425-1466`). After placing `cursorCol`, it re-segments the target
   logical line with the marker-aware segmenter (`this.segment(logicalLine, "grapheme")`) and, for
   any segment longer than one grapheme the new column falls inside:
   - if the segment **started on an earlier visual line and we are moving down** (`isContinuation &&
     isMovingDown`, `:1436-1453`), it skips every remaining continuation visual row of that segment
     and re-enters `moveToVisualLine` on the first row past it;
   - otherwise it **snaps back** to `seg.index`, stashing the pre-snap column in
     `snappedFromCursorCol` (`:1455-1461`), "so the cursor never lands in the middle of a
     multi-grapheme unit" and "gets highlighted";
   - falling off the end of the loop clears `snappedFromCursorCol` (`:1465`).

`setCursorCol` (`:1377-1381`) — used by every NON-vertical motion — clears **both**
`preferredVisualCol` and `snappedFromCursorCol`.

## Current state in cyrup-tui

The marker-aware segmenter is **already ported and correct**, and is already wired into horizontal
motion and deletion:

- [`editor/motion.rs:30-37`](../../crates/cyrup-tui/src/editor/motion.rs) `marker_grapheme_boundaries`
  — grapheme boundaries with every valid paste marker merged into one cluster. Its own doc says
  "Without the merge the caret can be parked INSIDE a `[paste #N …]` marker, where the next keystroke
  silently destroys it (TUI-043's cursor-motion half)."
- Consumers: `prev_grapheme` / `next_grapheme` (`motion.rs:41-52`) → `move_left` / `move_right`
  (`:6-22`), word motion, and `backspace` / `delete`
  ([`editor/edit.rs:48-110`](../../crates/cyrup-tui/src/editor/edit.rs)).

The three vertical movers never consult it. All three are the same five lines:

| mover | file:line | the placement line |
|---|---|---|
| `move_up_visual` | [`editor/wrap.rs:77-92`](../../crates/cyrup-tui/src/editor/wrap.rs) | `:90` `self.col = target.start + goal.min(target.len);` |
| `move_down_visual` | [`editor/wrap.rs:96-111`](../../crates/cyrup-tui/src/editor/wrap.rs) | `:109` same |
| `page_scroll` | [`editor/wrap.rs:131-146`](../../crates/cyrup-tui/src/editor/wrap.rs) | `:143` `self.col = t.start + goal.min(t.len);` |

Each seeds `goal` from `self.preferred_visual_col.unwrap_or(self.col - here.start)` and then
unconditionally **sets** `preferred_visual_col = Some(goal)` (`:82`, `:101`, `:138`). There is no
marker lookup, no snapped-column field on `InputEditor`
([`editor/mod.rs:177-180`](../../crates/cyrup-tui/src/editor/mod.rs) has `preferred_visual_col` and
nothing else), no `len - 1` clamp for a non-last visual segment, and no case that ever clears the
preferred column — the only clear in the crate is `reset_preferred_col` (`wrap.rs:159-161`), called
by non-vertical motion.

**Why the corruption follows.** `prev_grapheme` from a mid-marker column returns the marker START
(`motion.rs:41-44`, `rfind(|&b| b < col)` over boundaries that exclude the marker interior). The
`deleted_marker` guard in [`edit.rs:57-62`](../../crates/cyrup-tui/src/editor/edit.rs) then filters
on `end == self.col`, which **fails** for a mid-marker caret, so `drop_paste` never runs; `:65-70`
drains `start..col` — a *prefix* of the marker. Result: an orphan marker tail in the buffer and a
stranded registry entry `expanded_text`
([`editor/paste.rs:43-63`](../../crates/cyrup-tui/src/editor/paste.rs)) can no longer resolve.

**The wrapper cannot defend against this — by design.**
[`wrap.rs:260-266`](../../crates/cyrup-tui/src/editor/wrap.rs) carries a `[CYRUP-DELTA]` stating
"cyrup's segments are plain extended grapheme clusters — never composite", which is exactly what
lets a marker be split across visual rows. The snap inside the mover is therefore the only defence,
and it is absent.

**Preserve this deliberate divergence.** cyrup's `move_up_visual` at the first visual line falls
through to line-start (`wrap.rs:83-87`) and `move_down_visual` at the last falls through to line-end
(`:102-106`), per spec/tui/03 §5.1; pi's `moveCursor` simply does not move (`editor.ts:1808-1812`
guards the target index). That difference is intentional and is not part of this task.

## Subtasks

Two independent halves. (a) is the corruption fix and should land first; (b) is caret-placement
fidelity and can ship separately.

### (a) Atomic-segment snap

1. **Add the snapped-column field.** `snapped_from_col: Option<usize>` on `InputEditor`
   ([`editor/mod.rs:177-180`](../../crates/cyrup-tui/src/editor/mod.rs)), doc-cited to
   `editor.ts:336`.
2. **Clear it wherever pi's `setCursorCol` clears it.** `reset_preferred_col`
   ([`wrap.rs:159-161`](../../crates/cyrup-tui/src/editor/wrap.rs)) is cyrup's analogue of
   `editor.ts:1377-1381`; it must now clear both fields. Verify every existing caller still wants
   both cleared (they are all non-vertical motions/edits, which is exactly pi's rule).
3. **Extract one shared placement primitive** in
   [`editor/wrap.rs`](../../crates/cyrup-tui/src/editor/wrap.rs) — cyrup's `moveToVisualLine` — and
   route all three movers (`:77`, `:96`, `:131`) through it, replacing the three duplicated
   placement blocks. Upstream shares one function for exactly this reason (`editor.ts:1383-1385`,
   "Shared by moveCursor() and pageScroll()").
4. **Implement the snap** inside that primitive, following `editor.ts:1425-1466`: re-segment the
   target logical line via `marker_grapheme_boundaries`
   ([`motion.rs:30-37`](../../crates/cyrup-tui/src/editor/motion.rs)) — or the `marker_spans` it is
   built from ([`paste.rs:91`](../../crates/cyrup-tui/src/editor/paste.rs)) — and for a
   multi-grapheme segment the new column lands inside:
   - continuation-and-moving-down → skip every remaining continuation visual row of that segment and
     re-enter the primitive on the first row past it (`:1437-1453`);
   - otherwise snap `col` back to the segment start and stash the pre-snap column in
     `snapped_from_col` (`:1455-1461`).
   Falling through with no snap clears `snapped_from_col` (`:1465`). The re-entry must be bounded
   (each hop strictly increases the target row) so a pathological line cannot loop.
5. **Consume the stash** at the top of the primitive: when `snapped_from_col` is `Some`, derive the
   source visual column from it via a `findVisualLineAt` equivalent over the current visual-line map
   (`editor.ts:1396-1404`, `:1774-1789`) instead of from the live `col`. `current_visual_line`
   ([`wrap.rs:60-72`](../../crates/cyrup-tui/src/editor/wrap.rs)) is the nearest existing helper but
   resolves the LIVE cursor only — it needs a `(line, col)` form, matching pi's split between
   `findVisualLineAt` and `findCurrentVisualLine` (`:1794-1799`).

### (b) Column clamp and the sticky-column table

6. **Add the last-segment predicate and the clamp** (`editor.ts:1406-1416`) in the shared primitive:
   a visual segment is last when it is the final map entry or the next entry has a different
   `logical`; `max_visual_col` is `len` for a last segment and `len.saturating_sub(1)` otherwise.
   Apply it to BOTH source and target. This replaces today's `goal.min(target.len)`
   ([`wrap.rs:90`](../../crates/cyrup-tui/src/editor/wrap.rs), `:109`, `:143`), which permits
   `col == target.start + target.len` on a non-last segment — the wrap boundary, which belongs to
   the next visual row.
7. **Port `computeVerticalMoveColumn`** (`editor.ts:1489-1518`) as a private function in
   [`editor/wrap.rs`](../../crates/cyrup-tui/src/editor/wrap.rs), all seven cases including the
   three that CLEAR `preferred_visual_col`, and call it from the shared primitive in place of the
   current unconditional `preferred_visual_col = Some(goal)` at `:82` / `:101` / `:138`. Carry
   upstream's P/S/T/U table into the doc comment.

## Acceptance criteria

- [ ] `crates/cyrup-tui/src/editor/wrap.rs` contains **one** placement primitive; `move_up_visual`,
      `move_down_visual` and `page_scroll` each call it and no longer contain a `self.col = … +
      goal.min(…)` expression of their own
- [ ] `InputEditor` carries a snapped-column field, and `reset_preferred_col` clears it as well as
      `preferred_visual_col`
- [ ] On a wrapped logical line containing a `[paste #1 +N lines]` marker, no sequence of
      Up/Down/PageUp/PageDown can leave `col` strictly between a marker's start and end — the caret
      is always at the marker start or outside it
- [ ] From a caret snapped to a marker start, the NEXT vertical move resolves its source visual
      column from the stashed pre-snap column, not from the snapped column
- [ ] Moving DOWN into a marker that began on an earlier visual row lands on the first visual row
      past the marker's end, not on a continuation row (`editor.ts:1437-1453`)
- [ ] `Backspace` at a caret reached by vertical motion over a marker deletes the WHOLE marker and
      runs `drop_paste`, leaving no orphan marker text and no stranded registry entry — i.e. the
      `end == self.col` filter at `edit.rs:57-62` is now always satisfiable
- [ ] For a NON-last visual segment of a logical line, the vertical movers never place `col` at
      `segment.start + segment.len`; for a last segment they still may (`editor.ts:1409-1416`)
- [ ] `crates/cyrup-tui/src/editor/wrap.rs` contains a seven-case sticky-column function whose
      cases 1, 3 and 6 set `preferred_visual_col` to `None`
- [ ] The two deliberate fall-throughs still hold: Up at the first visual line moves to column 0,
      Down at the last moves to end-of-line (`wrap.rs:83-87`, `:102-106`)
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/editor.rs`,
      `src/tests/editor_fidelity.rs` or `src/tests/editor_page_actions.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
