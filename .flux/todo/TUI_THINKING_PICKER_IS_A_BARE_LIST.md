---
stage: todo
status: pending
updated: 2026-08-27
---

# Give The Thinking Picker Its Real 0.84.3 Envelope, And Bind Its Levels To The Active Model

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** low · **Kind:** partial-behaviour · **Area:** Selectors, settings and dialogs

## Objective

The thinking picker opens as an unlabelled seven-row list: no "Thinking Level" heading, no reminder
of the in-session cycle key, no type-to-filter, no key hints — and it offers levels the active model
may not support (`xhigh`, `max` on a non-reasoning or limited model). Upstream it is a full envelope
with a search input, and its level set is a constructor argument fed per-model. The port's classification
of it as a bare list rests on a **stale reading** of upstream that the source in this clone
contradicts.

## Upstream reference

`packages/coding-agent/package.json` reports **0.84.3**.
[`thinking-selector.ts:71-100`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/thinking-selector.ts)
— the component's children, in order:

```
DynamicBorder
Spacer(1)
Text("Thinking Level")
Spacer(1)
Text(`${keyDisplayText("app.thinking.cycle")} cycles thinking levels in-session`)
Spacer(1)
Input                              // :86-88, search; onSubmit forwards "\r" to the list
Spacer(1)
SelectList
Spacer(1)
Text(dim, "  Enter to select · Ctrl+S to set as default · Esc to cancel")   // :93
DynamicBorder
```

- `:113-124` routes non-navigation keys to the search input and re-runs `fuzzyFilter` over
  `` `${label} ${description}` `` (`applyFilter`, `:105-115`).
- The level set is a **constructor argument** `availableLevels: ThinkingLevel[]` (`:60`), sourced
  per-model from `this.session.getAvailableThinkingLevels()`
  ([`interactive-mode.ts:4792`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts))
  / `getSupportedThinkingLevels(model)`
  ([`settings-selector.ts:621-624`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts)).

## Current state in cyrup-tui

### The chrome is switched off by three stale predicates

- [`selector/list.rs:206-225`](../../crates/cyrup-tui/src/selector/list.rs) — `ListSelector::thinking(current)`
  builds a hardcoded `const LEVELS: [(&str, &str); 7]` (`:210-218`) into
  `ListSelector::new(rows, 7, selected, false)` (`:224`). `new` sets `title: None`
  (`list.rs:52`), so even `SelectorKind::title()`'s `"Thinking Level"`
  ([`selector/mod.rs:317`](../../crates/cyrup-tui/src/selector/mod.rs)) is never drawn for this kind.
- [`app/selectors.rs:14-20`](../../crates/cyrup-tui/src/app/selectors.rs) wraps it in
  `.with_upstream_chrome(SelectorKind::Thinking, &self.state.select_keymap)`.
- [`selector/list.rs:153-164`](../../crates/cyrup-tui/src/selector/list.rs) — `with_upstream_chrome`
  is a **no-op** for `Thinking`, because all three predicates exclude it:
  `draws_hint_row` ([`selector/mod.rs:370-378`](../../crates/cyrup-tui/src/selector/mod.rs)),
  `insets_rows` (`:396-398`), `envelope_spacers` (`:432-441`). Each carries a doc comment asserting
  `ThinkingSelectorComponent` is "`DynamicBorder` + `SelectList` + `DynamicBorder` and nothing else"
  (see also `selector/list.rs:151-152` and `app/selectors.rs:10-12`). That is **false** of
  `thinking-selector.ts:71-100` in this clone.
- `crates/cyrup-tui/tests/dialog_envelope_spacers.rs:150-157` pins the stale bare-list reading.

### The data side is computed but never consumed

`cyrup_provider::collection::get_supported_thinking_levels(model)`
([`crates/cyrup-provider/src/collection.rs:808`](../../crates/cyrup-provider/src/collection.rs)) is
the equivalent of pi's per-model computation — it returns `vec![ModelThinkingLevel::Off]` for a
non-reasoning model — and the TUI never calls it.

### The layout this needs already exists next door

[`settings_selector.rs:259`](../../crates/cyrup-tui/src/settings_selector.rs) — `SettingsSelector::lines`
already composes an `input_line_spans` search box + filtered rows + a hint line in exactly the shape
this picker wants.

## Subtasks

### (a) Chrome

1. **`crates/cyrup-tui/src/selector/mod.rs`** — add `SelectorKind::Thinking` to `draws_hint_row`
   (`:370-378`) and `envelope_spacers` (`:432-441`), and to `insets_rows` (`:396-398`) if the row
   inset applies; correct the three doc comments, which currently cite `thinking-selector.ts:42-69`
   as a bare border/list/border component, to the 0.84.3 shape at `:71-100`.
2. **`crates/cyrup-tui/src/selector/list.rs`** — give the thinking picker a title (upstream's
   `Text("Thinking Level")`, `:75`) rather than leaving `title: None` at `:52`, and correct the
   `with_upstream_chrome` doc at `:151-152`.
3. **`crates/cyrup-tui/src/selector/list.rs` (or a composed selector modelled on
   `settings_selector.rs:259`)** — add the in-session cycle hint row
   `` `${keyDisplayText("app.thinking.cycle")} cycles thinking levels in-session` ``
   (`thinking-selector.ts:79-83`), resolving the key label from the live keymap rather than
   hardcoding it.
4. Same file — add a fuzzy **search input** over `` `${label} ${description}` ``
   (`thinking-selector.ts:86-88`, `applyFilter` `:105-115`), with `Enter` in the input forwarding to
   the list as upstream's `onSubmit` does.
5. Same file — add the bottom hint row
   `"  Enter to select · Ctrl+S to set as default · Esc to cancel"` (`:93`) and the four `Spacer(1)`
   rows of the envelope.
6. **`crates/cyrup-tui/src/app/selectors.rs:10-12`** — update the comment that repeats the stale
   claim.
7. **`crates/cyrup-tui/tests/dialog_envelope_spacers.rs:150-157`** — this pins the bare-list reading
   and will fail; retarget it to the 0.84.3 shape as part of the change (the suite is another team's,
   but a test asserting the corrected-away behaviour cannot be left asserting it).

### (b) Data

8. **`crates/cyrup-tui/src/selector/list.rs:206-225`** — replace the hardcoded 7-level table
   (`:210-218`) with a caller-supplied level set, mirroring upstream's `availableLevels` constructor
   argument (`thinking-selector.ts:60`). Keep the descriptions table for labelling the levels that
   are supplied.
9. **`crates/cyrup-tui/src/app/selectors.rs:14-20`** — thread the active model's supported set from
   `cyrup_provider::collection::get_supported_thinking_levels`
   (`crates/cyrup-provider/src/collection.rs:808`) through `App::open_selector` into
   `ListSelector::thinking`.

## Acceptance criteria

- [ ] `SelectorKind::Thinking` appears in the `matches!` sets of `draws_hint_row` and
      `envelope_spacers` in `crates/cyrup-tui/src/selector/mod.rs`.
- [ ] The rendered thinking picker contains a `"Thinking Level"` title row.
- [ ] It contains a row reading `<key> cycles thinking levels in-session`, with `<key>` resolved from
      the live keymap binding for the thinking-cycle action, not a literal.
- [ ] It contains a search input, and typing a non-navigation character filters the rows by a fuzzy
      match over label **and** description.
- [ ] It contains the row `Enter to select · Ctrl+S to set as default · Esc to cancel`.
- [ ] `crates/cyrup-tui/src/selector/list.rs` no longer contains
      `const LEVELS: [(&str, &str); 7]` as the source of the picker's rows.
- [ ] `grep -rn "get_supported_thinking_levels" crates/cyrup-tui/src` returns at least one call site.
- [ ] For a model whose `reasoning` flag is false, the picker shows exactly one row (`off`).
- [ ] No doc comment in `crates/cyrup-tui/src` still describes `ThinkingSelectorComponent` as
      `DynamicBorder` + `SelectList` + `DynamicBorder` and nothing else.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
