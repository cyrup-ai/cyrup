---
stage: done
status: completed
updated: 2026-08-28
---

# Make A `/settings` Submenu Return To The Settings List Instead Of Closing The Dialog

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** divergent-behaviour · **Area:** Selectors, settings and dialogs

## Objective

In `/settings`, pressing Enter on **Theme**, **Thinking level** or **Warnings** and then picking a
value — or pressing Esc — currently drops the user all the way back to the prompt. Upstream returns
them to the settings list with the cursor still on the row they opened. Changing two settings in a
row costs one `/settings` in pi and three in cyrup. The submenus themselves already exist and work;
what is missing is the return path.

## Upstream reference

[`packages/tui/src/components/settings-list.ts`](../../tmp/pi/packages/tui/src/components/settings-list.ts).
A submenu is a **child** of the settings list, not a replacement for it — the list is never torn
down:

- `:50-52` — the three fields: `submenuComponent: Component | null`, `submenuItemIndex: number |
  null`, `navigateAfterClose: string | null`.
- `:96-99` — `render(width)` opens with "If submenu is active, render it instead":
  `if (this.submenuComponent) return this.submenuComponent.render(width);`
- `:184-187` — `handleInput` forwards **every** key to `this.submenuComponent` while it exists and
  returns; the list's own key handling is unreachable meanwhile.
- `:212-236` — `activateItem()`: for a row with a `submenu` factory it stores
  `submenuItemIndex = this.selectedIndex` and builds the child with a
  `done(selectedValue?, { navigateTo? })` callback that applies the value through
  `this.onChange(item.id, selectedValue)` and then calls `closeSubmenu()`. (A row with `values`
  instead of `submenu` just cycles in place, `:231-235`.)
- `:242-256` — `closeSubmenu()`: null the child, then either jump to `navigateAfterClose` via
  `selectItem(id)` + an immediate `activateItem()` (`:244-250`), or restore
  `this.selectedIndex = this.submenuItemIndex` (`:251-255`).
- `:83-89` — `selectItem(id)` moves the cursor to the row with that id.

Note the fuzzy filter is untouched by any of this (`applyFilter`, `:258-261`), so the search query
the user typed to find the row is still in force when the submenu closes.

Consumers that depend on the return:
[`settings-selector.ts:671-691`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts)
(the value-cycling rows) and the `theme` row's `ThemeSubmenu`, which **nests further** — a
`ThemeSubmenu` opens its own `SelectSubmenu` for the single/automatic modes
(`settings-selector.ts:283-330`), so depth > 1 is real upstream, not hypothetical.

**One thing not to build:** `navigateTo` has **no producer anywhere in this clone** —
`grep -rn "navigateTo" packages/ --include=*.ts` returns only the five lines in `settings-list.ts`
that declare and consume it. It is an unexercised seam. Port the plain restore-the-row half; treat
the navigate-and-auto-activate half as optional.

## Current state in cyrup-tui

`SelectorOutcome::OpenSubmenu` is implemented as a **replacement**, and both exits tear the slot
down.

- [`app/selectors.rs:273-301`](../../crates/cyrup-tui/src/app/selectors.rs) — the `OpenSubmenu` arm,
  whose own comment says "replace the settings selector with the nested picker":
  `"theme" => self.open_selector(SelectorKind::Theme)` (`:275`),
  `"thinking" => self.open_selector(SelectorKind::Thinking)` (`:280`), and `"warnings"` building a
  one-row `SettingsSelector` into `open_boxed_selector(SelectorKind::Settings, inner)` (`:288-297`).
- Every one of those constructors ends by **overwriting** the slot with no record of what it
  displaced: `open_selector` at [`selectors.rs:7-46`](../../crates/cyrup-tui/src/app/selectors.rs)
  (`:45` `self.state.selector = Some(ActiveSelector { … })`), `open_data_selector` (`:85`),
  `open_model_selector` (`:109`), `open_boxed_selector` (`:147`).
- Both exits nil the slot: the `Confirm` arm runs `confirm_selector` then `close_selector(false)`
  ([`selectors.rs:189-190`](../../crates/cyrup-tui/src/app/selectors.rs)); the `Cancel` arm runs
  `close_selector(true)` (`:250`). `close_selector`
  ([`selectors.rs:390-397`](../../crates/cyrup-tui/src/app/selectors.rs)) does
  `self.state.selector.take()` and restores only the editor text (and the previewed theme on
  cancel).
- [`app/state.rs:444-451`](../../crates/cyrup-tui/src/app/state.rs) — `ActiveSelector` has exactly
  four fields (`kind`, `inner`, `saved_editor`, `restore_theme`). There is **no parent slot and no
  stack to pop**.
- Out-of-band re-open does not happen either: the `C::OpenSelector(SelectorKind::Settings)` arm
  ([`app/execute.rs:178-193`](../../crates/cyrup-tui/src/app/execute.rs)) is reached only from
  `/settings` ([`app/submit.rs:61`](../../crates/cyrup-tui/src/app/submit.rs)), and the
  `C::ApplySetting` arm ([`app/execute_misc.rs:174-267`](../../crates/cyrup-tui/src/app/execute_misc.rs))
  persists and returns without touching `state.selector`.
- The crate does contain one hand-rolled two-stage flow —
  `BranchSummary` → `BranchSummaryInstructions`
  ([`selectors.rs:253-267`](../../crates/cyrup-tui/src/app/selectors.rs)), which re-opens a named
  successor from the Cancel arm — proving the idea is expressible, but it is hardcoded for that one
  pair and does not restore any parent state.
- [`settings_selector.rs`](../../crates/cyrup-tui/src/settings_selector.rs) has no field analogous
  to `SettingsList.submenuComponent`; it emits `OpenSubmenu(id)` and forgets.

## Subtasks

1. **Give `ActiveSelector` a parent slot.** Add `parent: Option<Box<ActiveSelector>>` to
   [`app/state.rs:444-451`](../../crates/cyrup-tui/src/app/state.rs) (a boxed single link, not a
   `Vec`, so nesting depth is naturally unbounded and each frame carries its own `saved_editor` /
   `restore_theme`). Every existing constructor initialises it to `None`, so no current call site
   changes behaviour.
2. **Add a push-style opener** next to `open_boxed_selector`
   ([`selectors.rs:147`](../../crates/cyrup-tui/src/app/selectors.rs)) that takes the current
   `ActiveSelector` out of the slot and stores it as the new one's `parent`, instead of dropping it.
   It must NOT re-snapshot the editor — the parent already holds the original `saved_editor`, and
   the editor was swapped out when the parent opened.
3. **Route the three `OpenSubmenu` ids through it** at
   [`selectors.rs:273-301`](../../crates/cyrup-tui/src/app/selectors.rs) so `theme`, `thinking` and
   `warnings` nest rather than replace. Keep the defensive no-op `_ => {}` arm.
4. **Pop on exit.** In `close_selector`
   ([`selectors.rs:390-397`](../../crates/cyrup-tui/src/app/selectors.rs)): when the closing
   selector has a `parent`, restore it into the slot and **skip** the editor restore (the parent
   still owns the input slot; only the outermost close restores the editor text). Cancel's theme
   restore must still run for the closing frame — that is what makes Esc out of the theme picker
   undo the live preview while leaving `/settings` open, as
   [`selectors.rs:392-394`](../../crates/cyrup-tui/src/app/selectors.rs) already does.
5. **Restore the parent's cursor row** (pi `closeSubmenu`, `settings-list.ts:251-255`). The parent's
   `selectedIndex` lives inside its boxed `dyn Selector`, so it is preserved for free by keeping the
   component alive — confirm that by reading
   [`settings_selector.rs`](../../crates/cyrup-tui/src/settings_selector.rs)'s selection state, and
   only add an explicit re-select if the component rebuilds its rows on re-entry. The fuzzy/search
   query must likewise survive (pi never re-filters on close).
6. **Reflect a changed value on the parent row.** Upstream's `done()` calls `onChange(item.id,
   selectedValue)` **before** `closeSubmenu` (`settings-list.ts:222-225`), so the row the user
   returns to already shows the new value. cyrup's `Confirm` path runs `confirm_selector`
   ([`selectors.rs:316-345`](../../crates/cyrup-tui/src/app/selectors.rs)) which emits
   `AppCommand::ApplySetting` — verify the parent `SettingsSelector`'s displayed value is refreshed
   from the same state the row was built from, and refresh it if not.
7. **Do not build `navigateTo`** unless it falls out for free — it has no upstream producer (see the
   Upstream reference). If it is added, mirror `settings-list.ts:244-250` exactly, including the
   auto-`activateItem()`.

## Acceptance criteria

- [ ] `ActiveSelector` ([`app/state.rs:444-451`](../../crates/cyrup-tui/src/app/state.rs)) carries a
      parent link, and `grep -n 'parent' crates/cyrup-tui/src/app/selectors.rs` shows it being both
      set on open and consumed on close
- [ ] Opening `/settings`, pressing Enter on **Theme**, choosing a theme with Enter → the settings
      list is on screen again with the cursor on the Theme row; the prompt is not reached
- [ ] Same for **Thinking level** and **Warnings**, on both the Enter and the Esc exit
- [ ] Esc out of the theme submenu restores the pre-preview theme AND leaves `/settings` open (both,
      not either)
- [ ] Esc from the settings list itself (no parent) still closes the slot and restores the editor
      text, exactly as today
- [ ] The editor text is restored **once**, on the outermost close — not on each submenu close
- [ ] A search query typed in `/settings` before opening a submenu is still in force after it closes
- [ ] Nesting deeper than one level works: a submenu opened from a submenu pops back one level per
      close (pi's `ThemeSubmenu` → `SelectSubmenu`, `settings-selector.ts:283-330`)
- [ ] After changing a value in a submenu, the parent row shows the new value without reopening
      `/settings`
- [ ] The `BranchSummary` → `BranchSummaryInstructions` flow
      ([`selectors.rs:253-267`](../../crates/cyrup-tui/src/app/selectors.rs)) is unchanged
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/settings_trust_selectors.rs` or
      `src/tests/selector_wiring.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
