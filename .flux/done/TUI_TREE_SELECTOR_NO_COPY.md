---
stage: done
status: completed
updated: 2026-08-28
---

# Make `app.message.copy` Work Inside `/tree` And Add The Missing `copy` Help Cell

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** medium · **Effort:** small · Area: Selectors, settings and dialogs

## Objective

Pressing the copy key (stock `Ctrl+X`) while `/tree` is open should copy the highlighted entry — a
bash command, a message body, a compaction summary — to the clipboard, as it does in pi. Today the
key does nothing at all inside `/tree`, and the `/tree` help row silently omits pi's `copy` cell.

## Upstream reference

In [`tree-selector.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/tree-selector.ts):

- `:125` — `public onCopy?: (text: string | undefined) => void;`
- `:627-630` — `copySelected() { const node = this.getSelectedNode(); this.onCopy?.(node ?
  this.getEntryCopyText(node) : undefined); }`
- `:896-922` — `getEntryCopyText(node)` switches on the entry:
  - `message` + `role === "bashExecution"` -> `entry.message.command`
  - `message` with `content` -> `extractFullContent(content)`; when that is empty **and** the role is
    `assistant`, fall back to `entry.message.errorMessage`
  - `custom_message` -> `extractFullContent(entry.content)`
  - `compaction` / `branch_summary` -> `entry.summary`
  - finally `return text?.trim() ? text : undefined` — whitespace-only becomes `undefined`
- `:883-892` — `extractFullContent` concatenates the `text` blocks of a content array (a bare string
  is returned as-is). **Correction to the audit note:** `formatToolCall` (`:938-994`) is *not* on
  the copy path — it is only reached from the row **label** at `:799`. Do not port tool-call
  rendering into the copy text.
- `:1029-1030` — `else if (kb.matches(keyData, "app.message.copy")) { this.copySelected(); }`, i.e.
  the one non-`app.tree.*` binding pi's tree consumes.
- `:1217-1235` — `TREE_HELP_ITEMS` places `{ keys: ["app.message.copy"], label: "copy" }`
  **between** the `branch` cell and the `label` cell.
- `:1364` — the wrapper forwards `this.treeList.onCopy = (text) => this.onCopy?.(text)`.
- `interactive-mode.ts:5297-5308` — the consumer: an empty/undefined text shows the error
  `Selected entry has no text to copy`; otherwise `copyToClipboard(text)` and the status
  `Copied selected message to clipboard`, with a clipboard failure surfaced through `showError`.

## Current state in cyrup-tui

- [`tree_selector.rs:984-1063`](../../crates/cyrup-tui/src/tree_selector.rs) `handle` covers the 11
  `TreeAction` variants, then `SelectAction` Up/Down/PageUp/PageDown/Confirm/Cancel, then a `None`
  arm handling `Backspace` and printable chars — and explicitly returns `SelectorOutcome::Ignored`
  for anything carrying CONTROL/ALT/SUPER (`:1049-1062`). There is nothing corresponding to
  `getEntryCopyText` in the file; `grep -ni 'copy\|clipboard' tree_selector.rs` matches only three
  `#[derive(Clone, Copy, …)]` lines (`:25`, `:92`, `:221`).
- [`keymap.rs:1137-1180`](../../crates/cyrup-tui/src/keymap.rs) `enum TreeAction` has 11 variants and
  no copy variant; `TreeAction::from_id` (`:1165+`) maps only `app.tree.*` ids.
- [`tree_selector.rs:890-928`](../../crates/cyrup-tui/src/tree_selector.rs) `help_text` builds
  move / page / branch / label / label-time / filters / cycle — pi's list minus the `copy` cell.
- The global copy action **is** ported but is unreachable here:
  [`app/input.rs:355-357`](../../crates/cyrup-tui/src/app/input.rs) maps `Action::MessageCopy` ->
  `AppCommand::Copy`, but `app/input.rs:38-40` returns `self.handle_selector_key(key)` before the
  global keymap whenever `state.selector.is_some()`.

## Subtasks

1. **Route the binding.** Either add a `TreeAction::Copy` to
   [`keymap.rs:1137`](../../crates/cyrup-tui/src/keymap.rs) resolving `app.message.copy` (keeping the
   default chord aligned with the global `Action::MessageCopy` binding so a user rebind moves both),
   or let `Action::MessageCopy` fall through the selector-first branch at
   [`app/input.rs:38-40`](../../crates/cyrup-tui/src/app/input.rs) for `SelectorKind::Tree`. Prefer
   the `TreeAction` route — it keeps the selector self-contained and matches how the other 11 tree
   bindings are resolved.
2. **Port `getEntryCopyText`** into [`tree_selector.rs`](../../crates/cyrup-tui/src/tree_selector.rs)
   over cyrup's own tree-node entry type, following `tree-selector.ts:896-922` exactly, including the
   assistant `errorMessage` fallback and the trim-to-`None` at the end. Reuse or add a
   text-block-concatenating helper equivalent to `extractFullContent` (`:883-892`).
3. **Wire the outcome** so the key produces the clipboard write and the two user-visible messages
   from `interactive-mode.ts:5297-5308`: `Selected entry has no text to copy` on `None`,
   `Copied selected message to clipboard` on success, and the clipboard error text on failure. Route
   through the existing `AppCommand::Copy` path where it fits rather than adding a second clipboard
   call site.
4. **Add the `copy` cell** to `help_text` at
   [`tree_selector.rs:890-928`](../../crates/cyrup-tui/src/tree_selector.rs), positioned between
   `branch` and `label` as in `TREE_HELP_ITEMS`.

## Acceptance criteria

- [ ] `grep -n 'app.message.copy' crates/cyrup-tui/src/keymap.rs crates/cyrup-tui/src/tree_selector.rs`
      returns a resolution reachable while `/tree` owns the input slot
- [ ] `tree_selector.rs` contains a per-entry copy-text function covering all five upstream branches:
      bashExecution command, text-block content, assistant `errorMessage` fallback, `custom_message`
      content, and `compaction` / `branch_summary` summary
- [ ] That function returns `None` for a whitespace-only result (pi's `text?.trim() ? text :
      undefined`)
- [ ] It does **not** render tool calls — `formatToolCall` is not on pi's copy path
- [ ] Copying an entry with no text produces `Selected entry has no text to copy`; a successful copy
      produces `Copied selected message to clipboard`
- [ ] `help_text` emits a `copy` cell between the `branch` and `label` cells; running `/tree` shows
      it in the hint row
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/tree_selector.rs` or
      `src/tests/tree_and_chrome.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
