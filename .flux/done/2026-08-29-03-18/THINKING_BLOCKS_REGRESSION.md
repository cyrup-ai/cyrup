---
stage: qa
status: completed
updated: 2026-08-29 05:17
---

# `/settings` — the `Thinking level` row goes stale when `Hide thinking` is toggled

One defect, one edit, one supporting refinement. Everything else from the visibility work is
complete and verified; the "Already complete" section lists it so it is not redone.

## The defect

`/settings` builds its row list **once** — [`app/execute.rs`](../../crates/cyrup-tui/src/app/execute.rs)
`:224-233` calls `settings_rows(session.services().settings.effective(), …)` and hands the result to
`SettingsSelector::new`. Cycling a row does **not** rebuild that list: `cycle_current` mutates only
its own `row.value` and emits `Apply`, and the chrome deliberately keeps the slot open
([`app/selectors.rs`](../../crates/cyrup-tui/src/app/selectors.rs) `:436-441`, "a `/settings` row
cycled in place: persist it live, keep the slot open").

So, in one dialog, with no way for the user to tell:

1. `/settings` opens. `hideThinkingBlock` is `false`, so the `Thinking level` row is built as `max`.
2. Enter (or Space) on `Hide thinking` cycles it to `true`. Reasoning is now suppressed.
3. The `Thinking level` row, one line away, **still reads `max`** with no marker.

Toggling back to `false` is equally wrong in the other direction: the `(hidden)` marker persists on
the level row for the rest of the dialog.

## Where the fix goes, and why that location is complete

**Both interactive writers of `hideThinkingBlock` converge on a single handler**, so one edit covers
every path that can change the flag while the list is open:

| writer | dispatches |
|---|---|
| `Ctrl+T` (`app.thinking.toggle`) | [`app/input.rs`](../../crates/cyrup-tui/src/app/input.rs) `:465` → `AppCommand::ApplySetting { id: "hideThinkingBlock", … }` |
| `/settings` row cycle | `Apply("id\u{1f}value")` → [`app/selectors.rs`](../../crates/cyrup-tui/src/app/selectors.rs) `:439` → the same `AppCommand::ApplySetting` |

Both land in `C::ApplySetting` at
[`app/execute_misc.rs`](../../crates/cyrup-tui/src/app/execute_misc.rs) `:645`, which already
special-cases this exact id at `:662`. The remaining two callers of `AppState::set_hide_thinking`
(`app/run_arms.rs:58`, `:237`) run at session bind and swap, and the row list is rebuilt from
`eff` every time `/settings` opens, so they cannot leave a stale row behind.

### This is pi's own mechanism, not a cyrup invention

pi keeps its rows current by writing through the item on change — `item.currentValue = selectedValue`
immediately before `this.onChange(...)` for a submenu return, and `item.currentValue = newValue`
before `onChange` in the cycle branch
([`tmp/pi/packages/tui/src/components/settings-list.ts`](../../tmp/pi/packages/tui/src/components/settings-list.ts)
`:222-225` and `:236-238`). cyrup mirrors that write-through as `App::set_settings_row_value`, and
**already uses it for exactly this purpose** at `execute_misc.rs:623`
(`self.set_settings_row_value("model-thinking", &summary)`), under a comment that states the rule:
*"The `/settings` list under this submenu keeps showing the summary it was built with unless it is
written through."*

pi has no sibling to refresh here only because pi's level row carries no marker — the `(hidden)`
marker is the cyrup delta, so keeping it current is cyrup's obligation, discharged with pi's
mechanism.

## Required change

### 1. `app/execute_misc.rs` — write the sibling row through

Replace the existing arm at `:662-664`:

```rust
                if id == "hideThinkingBlock" {
                    self.state.set_hide_thinking(value == "true");
                }
```

with:

```rust
                if id == "hideThinkingBlock" {
                    let hide = value == "true";
                    self.state.set_hide_thinking(hide);
                    // The `/settings` slot stays open and its rows are built ONCE from the
                    // effective settings (`app/execute.rs`), so the sibling `Thinking level` row
                    // would keep the marker state it was born with — reading as "reasoning is
                    // visible" one line under the switch that just suppressed it. Written through
                    // exactly as `model-thinking` is at `:623`, which is pi's own
                    // `item.currentValue = …` before `onChange` (`settings-list.ts:222-225`,
                    // `:236-238`). A no-op when no settings list is open, which is the `Ctrl+T`
                    // case.
                    let shown = thinking_row_value(&self.state.thinking_level, hide);
                    self.set_settings_row_value("thinking", &shown);
                }
```

`self.state.thinking_level` is the live level and is never empty (`AppState` seeds `"medium"`), so
the marker always attaches to a real level.

### 2. `app/mod.rs` — re-export the formatter so it reads like its sibling

`thinking_row_value` is currently reachable only by full path. Its sibling
`model_thinking_summary_for_count` is re-exported and called unqualified; match that. In the
`pub(crate) use settings_rows::{…}` block at
[`app/mod.rs`](../../crates/cyrup-tui/src/app/mod.rs) `:111-114`, add `thinking_row_value` to the
list, then drop the now-redundant `crate::app::settings_rows::` qualifier at its existing call site
in `app/selectors.rs` (`set_submenu_row_value`) so both callers spell it the same way.

## Definition of done

- With `/settings` open, cycling `Hide thinking` to `true` updates the `Thinking level` row to carry
  `(hidden)` **without the dialog being closed and reopened**; cycling back to `false` removes it.
- The same holds for `Ctrl+T` when a settings list happens to be open, and is a silent no-op when
  none is.
- Both callers of `thinking_row_value` spell it identically.
- `cargo build --workspace --all-targets` and `cargo clippy --workspace --all-targets` stay at 0,
  and the existing suite stays green.

## Already complete — do not redo

- `/thinking` picker states the hidden state and names the live `app.thinking.toggle` key.
  `desired_height` derives from `lines().len()`, so the two extra lines cannot clip the footer.
- Footer right cluster renders `• {level} (hidden)`, correctly skipped when the model does not
  support reasoning and when the level is `off`.
- `AppState::set_hide_thinking` is the single writer; no production caller bypasses it.
- `set_submenu_row_value` shares the formatter with the row builder, so a confirmed pick keeps the
  marker.
- The decorated value is display-only and can never be persisted: `SettingRow::submenu` sets
  `cycle: Vec::new()`, `cycle_current` returns `None` on an empty cycle before building the
  `"{id}\u{1f}{value}"` payload, and Enter on a submenu row returns `OpenSubmenu` first.
- Transcript rendering path untouched.

## Out of scope

- **Do not rebind `Ctrl+T`** — pi's documented default and an intentional toggle.
- **Do not touch the `Thinking...` label or the hidden-vs-body branch** — verified pi-faithful at
  `assistant-message.ts:139-143`.
- **Do not touch `Entry::Thinking`'s commit-time freeze** — `TUI-N06` owns it.
- **Plain Space flipping a global toggle with no confirmation** (`settings_selector.rs:497`; Space is
  unbound in `SelectKeymap`, `Hide thinking` is the 14th row) is a **user decision that has not been
  made**. Do not fold it in.

## Note on verification

The three surfaces are exercised through `TestBackend` only; nothing here has been seen in a real
terminal. The staleness above was invisible to the existing checks because they assert the formatter
and the row list as first built — never the transition — which is the shape this fix has to be
observed through.
