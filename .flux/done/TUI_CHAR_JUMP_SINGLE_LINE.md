---
stage: done
status: completed
updated: 2026-08-28
---

# Make Char-Jump Search The Whole Buffer, And Let A Control Key Fall Through When It Cancels Jump Mode

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** low · **Kind:** partial-behaviour · **Area:** Editor, input, keys and autocomplete

## Objective

In a multi-line prompt, `Ctrl+]` followed by a character does nothing whenever that character is not
on the caret's current line — upstream the caret jumps to it on a later line, and `Ctrl+Alt+]`
reaches earlier lines symmetrically. Separately, pressing a bound control key to bail out of jump
mode currently swallows that key instead of performing its action. Both are small omissions in an
otherwise faithful port.

## Upstream reference

[`packages/tui/src/components/editor.ts`](../../tmp/pi/packages/tui/src/components/editor.ts):

- `:2043-2065` `jumpToChar`. Its doc comment at `:2044` reads "**Multi-line search.** Case-sensitive.
  Skips the current cursor position." The body loops
  `for (let lineIdx = this.state.cursorLine; lineIdx !== end; lineIdx += step)` with
  `end = isForward ? lines.length : -1` (`:2050-2051`), searching the current line from
  `cursorCol ± 1` and every subsequent/preceding line **in full**
  (`searchFrom = isCurrentLine ? … : undefined`, `:2057-2059`), then sets **both**
  `state.cursorLine` and the column on a hit (`:2061-2064`).
- `:606-624` — while jump mode is armed, a **control** character cancels jump and then "falls through
  to normal handling" (`:622-623`), so e.g. `Ctrl+A` still moves to line start.

## Current state in cyrup-tui

- [`editor/motion.rs:242-263`](../../crates/cyrup-tui/src/editor/motion.rs) — `jump_to` is the port.
  Its doc at `:242-243` says "on the **current line**", and the body opens
  `let Some(line) = self.lines.get(self.row) else { return };`. Both arms (`JumpDir::Forward`
  `:246-252`, `JumpDir::Backward` `:254-260`) write only `self.col`; `self.row` is never assigned.
  The outer line loop of `jumpToChar` was simply omitted, collapsing a buffer-wide search to a
  within-line one. Nothing structural prevented it — `jump_to` takes `&mut self` and `self.row` is in
  scope and mutable.
- [`editor/keys.rs:9-17`](../../crates/cyrup-tui/src/editor/keys.rs) — the jump-mode arm:
  ```rust
  if let Some(dir) = self.jump.take() {
      if let KeyCode::Char(c) = ev.code
          && !ev.modifiers.contains(KeyModifiers::CONTROL) {
              self.jump_to(dir, c);
              return EditorOutcome::Edited;
          }
      return EditorOutcome::Edited; // any other key cancels jump
  }
  ```
  The trailing `return EditorOutcome::Edited` at `:16` swallows every non-printable key instead of
  falling through to the keymap/action path below (`:25` onward), unlike `editor.ts:622-624`.

## Subtasks

1. **`crates/cyrup-tui/src/editor/motion.rs:244-263`** — wrap the existing per-line search in the
   outer line loop from `editor.ts:2050-2059`: iterate from `self.row` toward `self.lines.len()`
   (forward) or toward `-1` (backward), searching the **current** line from `self.col ± 1` and every
   other line in full, and set **both** `self.row` and `self.col` on a hit. Preserve
   case-sensitivity and the skip-the-current-position rule.
2. **`crates/cyrup-tui/src/editor/motion.rs:242`** — update the doc comment: it currently says "on
   the current line" and must say multi-line, citing `editor.ts:2043-2065`.
3. **`crates/cyrup-tui/src/editor/keys.rs:16`** — when the key is not a printable char, clear jump
   mode (already done by the `self.jump.take()` at `:10`) and **fall through** to the normal
   keymap/action resolution at `:25` instead of returning `EditorOutcome::Edited`, matching
   `editor.ts:622-624`.
4. Confirm no caller of `jump_to` assumes the row is unchanged (it is called only from
   `editor/keys.rs:13`); if any viewport/scroll bookkeeping is keyed to a row change elsewhere in the
   editor, drive it the same way the other row-moving motions in `editor/motion.rs` do.

## Acceptance criteria

- [ ] `crates/cyrup-tui/src/editor/motion.rs::jump_to` assigns `self.row` on at least one path.
- [ ] `jump_to` no longer contains a single `let Some(line) = self.lines.get(self.row) else { return };`
      as its only line access — it iterates lines.
- [ ] With the buffer `["abc", "xdz"]`, caret at row 0 col 0, `jump_to(Forward, 'd')` leaves the
      caret at row 1, col 1.
- [ ] With the buffer `["abc", "xdz"]`, caret at row 1 col 2, `jump_to(Backward, 'b')` leaves the
      caret at row 0, col 1.
- [ ] A forward jump for a character that occurs nowhere in the buffer leaves both `row` and `col`
      unchanged.
- [ ] The doc comment on `jump_to` no longer says "on the current line".
- [ ] `crates/cyrup-tui/src/editor/keys.rs` has no unconditional `return EditorOutcome::Edited` at
      the end of the jump-mode block; a `Ctrl`-modified key pressed while jump mode is armed reaches
      the keymap resolution and performs its bound action.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- Tests ARE in scope. (A prior revision of this file claimed "another team owns the test suite"; that was unfounded — `git log` over `crates/cyrup-tui/src/tests/` shows only the two authors already working here. It cost the alt-screen renderer its entire suite.)
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
