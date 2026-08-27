---
stage: todo
status: pending
updated: 2026-08-27
---

# Extract A Shared Single-Line `Input` Editing Surface For Every Selector Search Field

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** high · **Effort:** large · Area: Editor, input, keys and autocomplete

## Objective

Every search box in the TUI — `/model`, `/resume` (and its rename field), `/settings`, `/config`,
`/tree`, `/scoped-models`, the OAuth picker, the login dialog and the `ui.input` extension dialog —
should behave like the Emacs-grade line editor pi gives them. Today the caret can move but nothing
else can: word motion, the kill ring, undo and paste are all unreachable from a search field, so
repairing a typo in the middle of a query still means backspacing everything after it, and a query
longer than the field is clipped instead of scrolled.

## Scope correction — read this before starting

The audit headline ("only insert+backspace: no cursor motion") is **wrong on cursor motion** and the
task is scoped to what actually remains.

[`text_input.rs:196-233`](../../crates/cyrup-tui/src/text_input.rs) already implements `Left`
(`:211`), `Right` (`:215`), `Home` (`:219`), `End` (`:223`) and forward `Delete` (`:207`), backed by
`cursor_left`/`cursor_right` at `:131-141` and covered by the test
`backspace_and_cursor_motion` at `:402`.
[`login_dialog.rs:558-590`](../../crates/cyrup-tui/src/login_dialog.rs) has the identical five.
**Do not re-add those.**

## Upstream reference

[`packages/tui/src/components/input.ts`](../../tmp/pi/packages/tui/src/components/input.ts) is a
full single-line editor and is the search field of every selector — `new Input()` at
`model-selector.ts:117`, `session-selector.ts:332` and `:718` (rename), `config-selector.ts:263`,
`thinking-selector.ts:84`, `oauth-selector.ts:76`, `scoped-models-selector.ts:139`,
`settings-list.ts:70`, `settings-submenu.ts:71`, `tree-selector.ts:1289`, `login-dialog.ts:55`,
`alt-screen-search.ts:106`, `extension-input.ts:63`.

`handleInput` (`:48-211`) routes **through `getKeybindings()`**, not a fixed key table, and
implements the parts this task is about:

| pi behaviour | `input.ts` lines | stock chord |
| --- | --- | --- |
| bracketed paste, newline-stripped + tab-expanded | `:54-84`, `:362-372` | — |
| `tui.editor.undo` | `:95-98`, `:338-348` | `Ctrl+-` |
| `deleteWordBackward` / `deleteWordForward` | `:117-125`, `:268-307` | `Ctrl+W`, `Alt+D` |
| `deleteToLineStart` / `deleteToLineEnd` | `:127-135`, `:249-266` | `Ctrl+U`, `Ctrl+K` |
| kill-ring `yank` / `yankPop` | `:138-145`, `:309-336` | `Ctrl+Y`, `Alt+Y` |
| `cursorWordLeft` / `cursorWordRight` via `findWordBackward`/`findWordForward` | `:182-190`, `:350-360` | `Alt+B`/`Alt+F`, `Alt+Left`/`Alt+Right` |
| grapheme-granular char delete/motion | `:107-115`, `:224-247`, `:148-168` | `Backspace`, `Delete`, `Left`/`Right` |
| typing-coalesced undo snapshots | `:213-222` | — |

`render` (`:378-446`) also **horizontally scrolls** a value wider than the field, keeping the caret
centred (`:391-422`, re-read in the clone): when `visibleWidth(value) >= availableWidth` it reserves
one column for a caret at the end, then picks `startCol` = 0 (caret near start), `totalWidth -
scrollWidth` (caret near end), or `cursorCol - halfWidth` (caret in the middle), and slices with
`sliceByColumn`.

## Current state in cyrup-tui

**The nearest Rust is [`text_input.rs`](../../crates/cyrup-tui/src/text_input.rs)**, whose doc
explicitly ports only `Input.render`. The `handleInput` half was reduced to a fixed
`match key.code` (`:200-232`) with no keybinding resolution: an unmatched key returns
`SelectorOutcome::Ignored`.

- **Stepping is by `char`, not grapheme.** `insert_char` (`:109`), `backspace` (`:114`),
  `delete_forward` (`:123`), `cursor_left` (`:131`), `cursor_right` (`:137`) all advance by
  `char::len_utf8`, so a ZWJ emoji or a combining sequence takes several presses.
- **The machinery all exists but is unreachable.** Kill ring
  ([`editor/kill_ring.rs`](../../crates/cyrup-tui/src/editor/kill_ring.rs) — `push_kill`, `yank`,
  `yank_pop`), undo ([`editor/undo.rs`](../../crates/cyrup-tui/src/editor/undo.rs) — `snapshot`,
  `push_undo_for` with pi's fish-style typing coalescing), and word motion
  ([`editor/motion.rs:127`](../../crates/cyrup-tui/src/editor/motion.rs) `word_left_target`) are all
  `pub(super) impl InputEditor`, i.e. private to the multi-line editor module. No `Input`-tier
  component can call them.
- **The keymap layer is already there.** [`keymap.rs:298`](../../crates/cyrup-tui/src/keymap.rs)
  `enum EditorAction` carries `CursorWordLeft` (`:303`), `DeleteWordBackward` (`:309`), `Yank`
  (`:313`), `YankPop` (`:314`), `Undo` (`:315`) and the rest, `EditorKeymap` is at `:1279` and
  `EditorKeymap::action_for` at `:1373`. Nothing resolves selector keys through it.
- **No horizontal scroll.**
  [`selector/mod.rs:66-88`](../../crates/cyrup-tui/src/selector/mod.rs) `search_input_spans` emits
  before/caret/after spans with **no window offset**, so an over-wide value is clipped by the
  `Paragraph` rather than scrolled with a centred caret.
- **Paste never reaches a selector.**
  [`app/input.rs:101-104`](../../crates/cyrup-tui/src/app/input.rs) returns `AppAction::None` for
  `InputEvent::Paste` whenever `state.selector.is_some()`, with the comment *"pure-list selectors
  ignore pastes (no embedded Input yet)"*.
- **Six selectors reimplement insert+backspace inline** rather than embedding a shared component,
  and a grep for `KeyCode::Left|Right|Home|End|Delete` across all six returns zero hits:
  [`model_selector.rs:505-518`](../../crates/cyrup-tui/src/model_selector.rs),
  [`session_selector.rs:1010-1022`](../../crates/cyrup-tui/src/session_selector.rs),
  [`config_selector.rs:826-845`](../../crates/cyrup-tui/src/config_selector.rs),
  [`settings_selector.rs:485-505`](../../crates/cyrup-tui/src/settings_selector.rs),
  [`tree_selector.rs:626-645`](../../crates/cyrup-tui/src/tree_selector.rs) (the inline label
  editor), [`oauth_selector.rs:333`](../../crates/cyrup-tui/src/oauth_selector.rs).

## Subtasks

1. **Lift the editing primitives out of `impl InputEditor`.** In
   [`editor/kill_ring.rs`](../../crates/cyrup-tui/src/editor/kill_ring.rs),
   [`editor/undo.rs`](../../crates/cyrup-tui/src/editor/undo.rs) and
   [`editor/motion.rs`](../../crates/cyrup-tui/src/editor/motion.rs), re-express the kill-ring
   accumulate/rotate rules, the coalescing undo-snapshot rule and `word_left_target`'s segment walk
   as functions over a `(&str, cursor)` pair (or a small shared struct), and have `InputEditor` call
   them. Behaviour must not change for the multi-line editor.
2. **Build the shared component.** Give
   [`text_input.rs`](../../crates/cyrup-tui/src/text_input.rs) a reusable `Input` type — buffer,
   cursor, kill ring, undo stack, scroll offset — whose key handler resolves through
   `EditorKeymap::action_for` ([`keymap.rs:1373`](../../crates/cyrup-tui/src/keymap.rs)) and covers
   `CursorWordLeft`/`CursorWordRight`, `DeleteWordBackward`/`DeleteWordForward`,
   `DeleteToLineStart`/`DeleteToLineEnd`, `Yank`/`YankPop` and `Undo`, falling back to the existing
   literal `Left`/`Right`/`Home`/`End`/`Delete`/`Backspace` arms. `TextInputSelector` becomes a thin
   wrapper over it.
3. **Grapheme granularity.** Replace the `char::len_utf8` stepping at
   [`text_input.rs:109-141`](../../crates/cyrup-tui/src/text_input.rs) with grapheme-cluster
   stepping (pi `deleteCharBackward`/`cursorLeft`, `input.ts:224-247`, `:148-168`). Use whatever
   grapheme facility the crate already depends on; do not add a new dependency without checking.
4. **Add a paste entry point** to the new `Input` (pi `input.ts:54-84`, `:362-372`: strip newlines,
   expand tabs), then route to it: change
   [`app/input.rs:101-104`](../../crates/cyrup-tui/src/app/input.rs) so `InputEvent::Paste` is
   offered to the focused selector before being dropped, and keep the `AppAction::None` fallback for
   selectors that own no input.
5. **Horizontal scroll.** Extend
   [`selector/mod.rs:66-88`](../../crates/cyrup-tui/src/selector/mod.rs) `search_input_spans` with an
   available-width parameter and pi's three-branch `startCol` computation (`input.ts:391-422`),
   including the one reserved column when the caret sits at the end. Keep the existing REVERSED
   caret span.
6. **Delegate the six selectors.** Replace the inline insert/backspace blocks in
   `model_selector.rs`, `session_selector.rs`, `config_selector.rs`, `settings_selector.rs`,
   `tree_selector.rs` (label editor) and `oauth_selector.rs` with the shared `Input`, preserving each
   one's post-edit hook (`on_query_changed`, `apply_filter`, `clear_folds` + `clamp_selection`, …).
7. **Update `login_dialog.rs`** to the same component so the credential fields gain the same surface
   and stop carrying their own `backspace`/`cursor_left`/`cursor_right` copies.

## Acceptance criteria

- [ ] A single type in [`text_input.rs`](../../crates/cyrup-tui/src/text_input.rs) owns buffer,
      cursor, kill ring, undo stack and scroll offset, and `grep -c 'fn backspace' src/text_input.rs
      src/login_dialog.rs src/model_selector.rs src/session_selector.rs src/oauth_selector.rs`
      reports the private per-selector copies gone
- [ ] That type's key handler calls `EditorKeymap::action_for` and handles at minimum
      `CursorWordLeft`, `CursorWordRight`, `DeleteWordBackward`, `DeleteWordForward`,
      `DeleteToLineStart`, `DeleteToLineEnd`, `Yank`, `YankPop`, `Undo`
- [ ] `grep -rn "impl InputEditor" src/editor/kill_ring.rs src/editor/undo.rs` shows those bodies
      delegating to shared free functions rather than owning the logic
- [ ] Cursor and delete stepping in `text_input.rs` advances by grapheme cluster, not
      `char::len_utf8`; `grep -n 'len_utf8' src/text_input.rs` returns nothing in the motion/delete
      paths
- [ ] `search_input_spans` in [`selector/mod.rs`](../../crates/cyrup-tui/src/selector/mod.rs) takes
      the field width and returns a windowed slice; a value wider than the field renders with the
      caret inside the window, matching `input.ts:391-422`'s three `startCol` branches
- [ ] [`app/input.rs`](../../crates/cyrup-tui/src/app/input.rs)'s `InputEvent::Paste` arm no longer
      returns `AppAction::None` unconditionally when a selector is open; the comment "no embedded
      Input yet" is gone
- [ ] `grep -n 'KeyCode::Char(c)' src/model_selector.rs src/session_selector.rs
      src/config_selector.rs src/settings_selector.rs src/oauth_selector.rs` shows no inline
      insert/backspace search-field handling left
- [ ] `Left`/`Right`/`Home`/`End`/`Delete` still work in `TextInputSelector` and `login_dialog.rs`
      (they already did — this task must not regress them)
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
