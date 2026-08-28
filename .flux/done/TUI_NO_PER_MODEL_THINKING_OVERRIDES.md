---
stage: done
status: completed
updated: 2026-08-28
---

# Add Per-Model Default Thinking Levels And The Multi-Step Submenu Engine They Need

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** low · **Effort:** large · Area: Selectors, settings and dialogs

## Objective

A user should be able to say "this cheap model defaults to `off`, this reasoning model defaults to
`high`" and have it persist. Today cyrup has one global default thinking level plus a per-session
override and nowhere to express a per-model preference — `/settings` has no
"Default thinking level per model" row, there is no `modelThinkingLevels` settings key, and the
`/settings` submenu shape cannot express a flow whose second step depends on the first.

## Dependency

**SETTINGS_SUBMENU_DOES_NOT_RETURN must land first.** pi's per-model flow is constructed with
`{ loop: true }` (`settings-selector.ts:667`), which returns to step 0 after each completion; with no
parent-restore there is nothing to loop back to.

## Upstream reference

**The row** —
[`settings-selector.ts:571-670`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts):
`id: "model-thinking"`, `label: "Default thinking level per model"`, description
`` `Override the default thinking level for specific models. ${cycleThinkingKey} cycles in-session.` ``,
`currentValue: modelThinkingOverridesSummary(currentModelThinkingLevels)` — which is `"none"` at
count 0 and `` `${count} configured` `` otherwise (`:182-186`).

Its `submenu` builds a two-step `SteppedSubmenu`:

- **step `model`** (`:578-611`) — title `Per-Model Thinking Level`, description
  `Select a model to configure`, `searchable: true`, `layout: MODEL_PICKER_LAYOUT`. Options are
  `config.availableDefaultModels` sorted **current model first, then default model, then by
  provider** (`:582-591`), each row's `description` showing the existing override; `preselect` is
  `currentModelKey ?? currentDefaultModelKey`. An empty catalog produces the single disabled-looking
  row `No models available` / `Log in to a provider or configure an API key first` (`:600-606`).
- **step `level`** (`:612-639`) — title `` `Thinking Level for ${modelDisplayLabel(model)}` ``,
  options are `getSupportedThinkingLevels(model)` when `model.reasoning` else just `["off"]`, each
  described by `THINKING_DESCRIPTIONS[level]`, plus a `(clear override)` row
  (`CLEAR_OVERRIDE_VALUE = "__clear__"`, `:172`) whose description is
  `` `Revert to global default (${config.thinkingLevel})` `` — appended **only** when an override
  already exists for that model. `preselect` is the current override.
- **`onComplete`** (`:647-663`) calls `onModelThinkingLevelRemove(provider, id)` for the clear value
  and `onModelThinkingLevelChange(provider, id, level)` otherwise, updating the local map so the
  summary refreshes, then `done(summary())` with `{ loop: true }`.

**The engine** —
[`settings-submenu.ts:143-250`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/settings-submenu.ts):
`SteppedSubmenuStep { key, title, description, options, preselect?, searchable?, layout? }` where
title/description/options/preselect all receive the accumulated `context: Record<string, string>`;
`SteppedSubmenuOptions { startAtStep?, initialContext?, loop? }`; `buildStep` (`:186-241`) renders a
`Step ${i+1}/${total} · ` prefix when `total > 1`, stores the chosen value under `step.key`, advances
or (at the last step) calls `onComplete` and then either loops back to step 0 with a cleared context
or cancels. Esc at step > 0 **deletes that step's key and walks back one step**; Esc at step 0
cancels.

**The settings key** —
[`settings-manager.ts:99`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts) declares
`modelThinkingLevels?: Record<string, ThinkingLevel>` *"keyed by `provider/modelId`"*, with
`getModelThinkingLevel` (`:791-793`), `getAllModelThinkingLevels` (`:795-797`),
`setModelThinkingLevel` (`:799-806`) and `removeModelThinkingLevel` (`:808-816`) — all writing the
**global** layer, and `remove` deleting the whole map once it is empty.
[`model-resolver.ts:629`](../../tmp/pi/packages/coding-agent/src/core/model-resolver.ts) carries it
through resolution and reads `modelThinkingLevels?.[`${provider}/${id}`]` at `:666` (scoped model) and
`:679` (default model).

## Current state in cyrup-tui

All three legs are absent.

- **No row.** [`app/settings_rows.rs`](../../crates/cyrup-tui/src/app/settings_rows.rs) contains
  exactly three `SettingRow::submenu` entries — `theme` (`:50`), `warnings` (`:197`) and `thinking`
  (`:204`) — and no per-model entry. (`src/tests/settings_trust_selectors.rs:398-402`
  `the_settings_grid_offers_the_warnings_and_thinking_submenus` asserts only that those two rows are
  *present*, not that the set is closed, so adding a row does not break it.)
- **No engine.** [`settings_selector.rs:104-116`](../../crates/cyrup-tui/src/settings_selector.rs)
  `SettingRow::submenu` carries a single `submenu_id: String`, and the dispatch at
  [`app/selectors.rs:273-300`](../../crates/cyrup-tui/src/app/selectors.rs) matches the three literal
  ids `"theme"` / `"thinking"` / `"warnings"` and no-ops otherwise. `grep -rn
  'SteppedSubmenu\|stepped\|start_at_step\|initial_context' crates/cyrup-tui/src` returns nothing.
  The nearest thing to a multi-step flow is the hardcoded `BranchSummary` ->
  `BranchSummaryInstructions` pair, whose Cancel arm at
  [`app/selectors.rs:252-266`](../../crates/cyrup-tui/src/app/selectors.rs) hand-rolls an Escape that
  walks back one stage — the same idea, wired for one flow instead of generalized.
- **No settings key.** `grep -rni 'modelThinkingLevels|model_thinking_level' crates` matches only two
  unrelated test function names (`cyrup-provider/src/tests/thinking_max.rs:74`,
  `cyrup-core/src/message/thinking.rs:78`). cyrup-config has the *global* key only:
  [`settings/effective.rs:41-58`](../../crates/cyrup-config/src/settings/effective.rs)
  `default_thinking_level()`, whose one production consumer is
  [`cyrup-session-svc/src/builder.rs:1946-1972`](../../crates/cyrup-session-svc/src/builder.rs) —
  the resolution point a per-model override has to hook into, immediately before
  `clamp_thinking_level`.

## Subtasks

**The config layer must land first, or the TUI row has nothing to write.**

1. **cyrup-config** — add a `modelThinkingLevels` key (`Record<"provider/model", ThinkingLevel>`)
   with a get-one / get-all reader on
   [`settings/effective.rs`](../../crates/cyrup-config/src/settings/effective.rs) and set / remove
   writers on [`settings/manager.rs`](../../crates/cyrup-config/src/settings/manager.rs), writing the
   **global** scope (follow `set_mermaid_rendering_mode` at `manager.rs:362-376` for the
   nested-write-without-clobbering-siblings shape), and deleting the map entirely when the last entry
   is removed (`settings-manager.ts:811-813`).
2. **Honour it in resolution** — in
   [`cyrup-session-svc/src/builder.rs:1946-1972`](../../crates/cyrup-session-svc/src/builder.rs),
   consult the per-model override for the chosen model before falling back to
   `default_thinking_level()`, keeping the existing `DEFAULT_THINKING_LEVEL` fallback and the final
   `clamp_thinking_level` / modelless-forces-`Off` rules intact (pi `model-resolver.ts:660-680`).
3. **Generalize the submenu** — extend
   [`settings_selector.rs`](../../crates/cyrup-tui/src/settings_selector.rs)'s single `submenu_id`
   into a multi-step description (per-step title/description/options/preselect closures over an
   accumulated context, plus `searchable`, `start_at_step`, `initial_context` and `loop`), and teach
   the `SelectorOutcome::OpenSubmenu` dispatch at
   [`app/selectors.rs:273-300`](../../crates/cyrup-tui/src/app/selectors.rs) to drive it: advance on
   confirm, on Esc drop the current step's key and walk back one step, cancel only at step 0, and on
   completion either loop to step 0 with a cleared context or close. Render the `Step i/N · ` prefix
   when there is more than one step. The existing three literal ids must keep working unchanged.
4. **The row itself** — add `model-thinking` to
   [`app/settings_rows.rs`](../../crates/cyrup-tui/src/app/settings_rows.rs) with pi's label,
   description and `"none"` / `"N configured"` summary, the searchable model step sorted
   current -> default -> provider with each row's description showing its existing override, and the
   level step with `THINKING_DESCRIPTIONS`-equivalent text and the conditional `(clear override)`
   row. Persist through the existing `AppCommand::ApplySetting` path.

## Acceptance criteria

- [ ] `grep -rn "modelThinkingLevels" crates/cyrup-config/src` returns a reader and set/remove writers
      on the global scope; removing the last entry deletes the map rather than leaving `{}`
- [ ] `grep -rn "model_thinking" crates/cyrup-session-svc/src/builder.rs` shows the override consulted
      before `default_thinking_level()`, with `clamp_thinking_level` and the modelless-`Off` rule
      unchanged
- [ ] The submenu description in `settings_selector.rs` supports more than one step, with per-step
      option closures receiving the prior selections; `grep -n 'submenu_id' src/settings_selector.rs`
      no longer shows a bare single `String` as the only representation
- [ ] Esc on step 2 returns to step 1 with step 2's selection dropped; Esc on step 1 closes the flow;
      a `loop` flow returns to step 1 after completing step 2
- [ ] `theme`, `thinking` and `warnings` submenus behave exactly as before
- [ ] `/settings` shows a `Default thinking level per model` row reading `none` with no overrides and
      `N configured` with N
- [ ] The model step is searchable and sorted current model, then default model, then by provider;
      each row shows its existing override as the description; an empty catalog shows
      `No models available`
- [ ] The level step lists only the levels the model supports (`off` alone for a non-reasoning model)
      and shows `(clear override)` only when that model already has one
- [ ] Selecting `(clear override)` removes the entry and the parent row's summary decrements
- [ ] `cargo build --workspace --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui -p cyrup-config -p cyrup-session-svc --all-targets` — warning count
      not increased
- [ ] `cargo test --workspace` — no pre-existing test regresses, including
      `the_settings_grid_offers_the_warnings_and_thinking_submenus`
      (`crates/cyrup-tui/src/tests/settings_trust_selectors.rs:398-402`)

## Constraints

- Tests ARE in scope. (A prior revision of this file claimed "another team owns the test suite"; that was unfounded — `git log` over `crates/cyrup-tui/src/tests/` shows only the two authors already working here. It cost the alt-screen renderer its entire suite.)
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
