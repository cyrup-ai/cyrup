---
stage: done
status: completed
updated: 2026-08-28
---

# Make `/tree` Page By A Screenful On PageUp/PageDown And ←/→, And Window Its Body So Paging Is Visible

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** divergent-behaviour · **Area:** Selectors, settings and dialogs

## Objective

`/tree` prints `←/→ page` in its own help row and then does nothing of the sort: PageUp/PageDown
crawl one row at a time and bare ←/→ are dead keys. Navigating a few hundred entries means holding
↑/↓. Upstream both key pairs jump a screenful.

## Upstream reference

[`tree-selector.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/tree-selector.ts):

- `:1018-1023` — the paging arm, which binds **two key ids per direction**:

  ```ts
  } else if (kb.matches(keyData, "tui.editor.cursorLeft") || kb.matches(keyData, "tui.select.pageUp")) {
      // Page up
      this.selectedIndex = Math.max(0, this.selectedIndex - this.maxVisibleLines);
  } else if (kb.matches(keyData, "tui.editor.cursorRight") || kb.matches(keyData, "tui.select.pageDown")) {
      // Page down
      this.selectedIndex = Math.min(this.filteredNodes.length - 1, this.selectedIndex + this.maxVisibleLines);
  }
  ```

  Note the clamps are asymmetric in form but not in effect: `max(0, …)` up, `min(len - 1, …)` down.
- `:1219` — `TREE_HELP_ITEMS[1]` is `{ keys: ["tui.editor.cursorLeft", "tui.editor.cursorRight"],
  label: "page" }`, i.e. the help cell cyrup already prints is upstream's and it is truthful there.
- `:1362` — the window size the paging jumps by:
  `const maxVisibleLines = Math.max(5, Math.floor(terminalHeight / 2));`, handed to the `TreeList`
  constructor at `:1364` and stored at `:136`.
- `:674-681` — the SAME `maxVisibleLines` windows the rendered body:
  `startIndex = max(0, min(selectedIndex - floor(maxVisibleLines / 2), filteredNodes.length -
  maxVisibleLines))`, `endIndex = min(startIndex + maxVisibleLines, filteredNodes.length)`. Paging
  and rendering share one number upstream, which is why a page jump is visible.

Both alt/ctrl+Left and alt/ctrl+Right remain the fold/unfold bindings (`app.tree.foldOrUp` /
`app.tree.unfoldOrDown`) — only the **bare** arrows page.

## Current state in cyrup-tui

- [`tree_selector.rs:1008-1016`](../../crates/cyrup-tui/src/tree_selector.rs) folds paging into
  single-row motion:

  ```rust
  Some(SelectAction::Up) | Some(SelectAction::PageUp) => { self.move_by(-1); … }
  Some(SelectAction::Down) | Some(SelectAction::PageDown) => { self.move_by(1); … }
  ```

  and `move_by` ([`:483-493`](../../crates/cyrup-tui/src/tree_selector.rs)) clamps `selected + delta`
  into `0..=len-1`, so a page action moves exactly one row.
- A bare `Left`/`Right` reaches nothing. `SelectKeymap::default`
  ([`keymap.rs:794-808`](../../crates/cyrup-tui/src/keymap.rs)) binds only `KeyCode::PageUp` /
  `PageDown` to `S::PageUp` / `S::PageDown` — no plain arrows. `TreeKeymap::default`
  ([`keymap.rs:1200-1229`](../../crates/cyrup-tui/src/keymap.rs)) binds only **alt**+Left/Right and
  **ctrl**+Left/Right to `FoldOrUp` / `UnfoldOrDown`. So a bare `KeyCode::Left` falls to the `None`
  arm at [`tree_selector.rs:1041`](../../crates/cyrup-tui/src/tree_selector.rs), is not a
  `KeyCode::Char`, and returns `SelectorOutcome::Ignored`.
- Meanwhile `help_text` ([`tree_selector.rs:890-891`](../../crates/cyrup-tui/src/tree_selector.rs))
  unconditionally pushes the literal `"←/→ page"` cell — an unwired control by the letter of batch
  3's detector.
- **The tree has no window at all.** `rows(width, theme)`
  ([`:746-749`](../../crates/cyrup-tui/src/tree_selector.rs)) iterates `self.visible_indices()`
  entire and emits a line per index; `render` ([`:942-981`](../../crates/cyrup-tui/src/tree_selector.rs))
  hands the whole `Vec<Line>` to a `Paragraph`, which clips at the bottom of the body rect; and
  `desired_height` ([`:930-940`](../../crates/cyrup-tui/src/tree_selector.rs)) asks for
  `visible_indices().len() + 6`. `grep -n 'max_visible\|scroll\|window\|viewport' tree_selector.rs`
  finds nothing. Consequently a selection below the clip line is **invisible**, which is why paging
  cannot be added as a pure keymap change.

**Correction to the survey's premise, which shortens the work:** the threading seam already exists.
`Selector::set_terminal_height(&mut self, rows: u16)`
([`selector/mod.rs:508`](../../crates/cyrup-tui/src/selector/mod.rs), a no-op default documented
"Called before `desired_height` on every frame") is invoked on the active selector every draw at
[`app/draw.rs:43-45`](../../crates/cyrup-tui/src/app/draw.rs). `ConfigSelector` is the worked
example — [`config_selector.rs:745-747`](../../crates/cyrup-tui/src/config_selector.rs) implements
it as `self.max_visible = Self::max_visible_for(rows)`. And the windowing arithmetic is already a
shared helper: `centered_window(selected, len, max)`
([`selector/mod.rs:217-223`](../../crates/cyrup-tui/src/selector/mod.rs)) is pi's `startIndex` /
`endIndex` formula verbatim — the same one `tree-selector.ts:674-681` uses. `SessionSelector` is the
in-crate paging pattern to copy ([`session_selector.rs:994-1003`](../../crates/cyrup-tui/src/session_selector.rs),
`self.selected.saturating_sub(self.max_visible)` / `(self.selected + self.max_visible).min(len - 1)`),
though note its `max_visible` is a fixed `10` ([`:229`](../../crates/cyrup-tui/src/session_selector.rs)),
not terminal-derived — the tree must use pi's `max(5, floor(terminal_height / 2))`.

## Subtasks

1. **Give `TreeSelector` a window size.** Add a `max_visible: usize` field to
   [`tree_selector.rs`](../../crates/cyrup-tui/src/tree_selector.rs) with pi's default for a 24-row
   terminal, and implement `Selector::set_terminal_height` on it as
   `max(5, terminal_height / 2)` (`tree-selector.ts:1362`), following
   [`config_selector.rs:745-747`](../../crates/cyrup-tui/src/config_selector.rs).
2. **Window the body.** In `rows()`
   ([`tree_selector.rs:746-749`](../../crates/cyrup-tui/src/tree_selector.rs)) slice
   `visible_indices()` through `centered_window(self.selected, len, self.max_visible)`
   ([`selector/mod.rs:217`](../../crates/cyrup-tui/src/selector/mod.rs)) instead of emitting every
   row, and cap the body term in `desired_height`
   ([`:930-940`](../../crates/cyrup-tui/src/tree_selector.rs)) at `max_visible` the way
   `ConfigSelector::desired_height` ([`config_selector.rs:749-757`](../../crates/cyrup-tui/src/config_selector.rs))
   does. Keep `is_sel` correct against the ABSOLUTE index after slicing — today `rows()` compares
   `row == self.selected` over an unsliced enumerate, which is only right at offset 0.
3. **Split paging out of the single-row arms** at
   [`tree_selector.rs:1008-1016`](../../crates/cyrup-tui/src/tree_selector.rs): `Up`/`Down` keep
   `move_by(±1)`; `PageUp`/`PageDown` become `move_by(±max_visible)`, whose existing clamp in
   `move_by` ([`:483-493`](../../crates/cyrup-tui/src/tree_selector.rs)) already matches pi's
   `max(0, …)` / `min(len - 1, …)`.
4. **Bind the bare arrows inside the tree.** pi resolves `tui.editor.cursorLeft` /
   `cursorRight` here, i.e. the editor tier's ids, not tree ids. Route a bare `KeyCode::Left` /
   `KeyCode::Right` (no CONTROL/ALT/SUPER modifier — those stay fold/unfold via
   [`keymap.rs:1207-1211`](../../crates/cyrup-tui/src/keymap.rs)) to the same page arms, in
   `TreeSelector::handle` ahead of the `None` fallthrough at
   [`:1041`](../../crates/cyrup-tui/src/tree_selector.rs). Do **not** add plain Left/Right to
   `SelectKeymap::default` ([`keymap.rs:794-808`](../../crates/cyrup-tui/src/keymap.rs)) — that
   table is shared by every selector, and upstream binds these two ids only in the tree.
5. **Leave `help_text` alone** ([`:890-891`](../../crates/cyrup-tui/src/tree_selector.rs)) — the
   `←/→ page` cell becomes truthful rather than needing removal.

## Acceptance criteria

- [ ] `TreeSelector` implements `Selector::set_terminal_height`, and `grep -n 'max_visible'
      crates/cyrup-tui/src/tree_selector.rs` shows it used by the page arms, by `rows()` and by
      `desired_height`
- [ ] The window size is `max(5, terminal_height / 2)` (pi `tree-selector.ts:1362`), not a constant
      and not the editor's `max(5, rows * 3/10)`
- [ ] PageUp and PageDown each move the highlight by the window size, clamped to `0` and `len - 1`
- [ ] A bare `Left` pages up and a bare `Right` pages down inside `/tree`
- [ ] `Alt+Left`, `Ctrl+Left`, `Alt+Right` and `Ctrl+Right` still fold/unfold — the modifier arms at
      [`keymap.rs:1207-1211`](../../crates/cyrup-tui/src/keymap.rs) are untouched
- [ ] `Up`/`Down` still move exactly one row
- [ ] `rows()` emits at most `max_visible` lines, and the highlighted entry is always among them
      after a page jump in either direction (i.e. paging is observable, not just internal)
- [ ] `SelectKeymap::default` ([`keymap.rs:794-808`](../../crates/cyrup-tui/src/keymap.rs)) still
      binds seven entries — no plain Left/Right added to the shared table
- [ ] The `←/→ page` help cell is still printed
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/selection_fidelity.rs` or
      `src/tests/tree_and_chrome.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
