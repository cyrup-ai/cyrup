---
stage: todo
status: pending
updated: 2026-08-27
---

# Add Ctrl+S "Set As Default" And The `· default` Marker To `/model` And The Thinking Picker

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** missing-feature · **Area:** Selectors, settings and dialogs

## Objective

In `/model` and the thinking picker, Ctrl+S does nothing. A model or reasoning level can only be set
for the current session, never saved as the default, so the choice is lost on restart — and neither
picker shows which entry currently **is** the default. Upstream, Enter selects for the session and
Ctrl+S selects *and* persists, with the default row badged `· default` and sorted to the top.

## Upstream reference

Both pickers ported only the select/cancel half of pi's confirm surface. The persisting half:

### `/model` — [`model-selector.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/model-selector.ts)

- `:401-407` — the key arm, guarded on the callback's presence:

  ```ts
  // Ctrl+S — select and save as default
  else if (matchesKey(keyData, "ctrl+s") && this.onSelectAsDefaultCallback) {
      const selectedModel = this.filteredModels[this.selectedIndex];
      if (selectedModel) { this.dispose(); this.onSelectAsDefaultCallback(selectedModel.model); }
  }
  ```

- `:138-141` — the hint row is swapped when the callback exists:
  `"  Enter to select · Ctrl+S to set as default · Esc to cancel"`.
- `:252-254` — `isDefaultModel(model)` is `provider` **and** `id` equality against the
  `DefaultModelReference` passed in at construction.
- `:316-317` — the row badge: `const defaultBadge = isDefault ? theme.fg("muted", " · default") : ""`.
- `:225-240` — `sortModels`: **current model first, default model second**, then by provider
  (`localeCompare`).
- `:256-259` / `:270-290` — `isDefaultSearch(query)` is `"default".startsWith(query.trim()
  .toLowerCase())` on a non-empty query; when true, `filterModels` hoists every default row to the
  front of the fuzzy result and de-duplicates the rest by `provider\0id`. The fuzzy haystack itself
  also gets `" default"` appended for a default row (`:276`).

### Thinking picker — [`thinking-selector.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/thinking-selector.ts)

- `:122-125` — `if (matchesKey(keyData, "ctrl+s") && this.onSelectAsDefault) { … this.onSelectAsDefault(item.value as ThinkingLevel); }`
- `:73` — the description suffix:
  `level === defaultThinkingLevel ? \`${LEVEL_DESCRIPTIONS[level]} · default\` : LEVEL_DESCRIPTIONS[level]`
- `:94` — the same `Enter to select · Ctrl+S to set as default · Esc to cancel` hint row.

### What the callbacks do — [`interactive-mode.ts`](../../tmp/pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)

Both are wired in this clone, so the hint rows are live, not dead code:

- `:4790-4801` — the thinking picker gets `(level) => selectLevel(level, false)` for Enter and
  `(level) => selectLevel(level, true)` for Ctrl+S, plus
  `this.settingsManager.getDefaultThinkingLevel() ?? DEFAULT_THINKING_LEVEL` as the badge source.
  `selectThinkingLevel(level, persist)` (`:4773-4782`) calls
  `session.setThinkingLevel(level, { persist })` and shows
  `` `Default thinking level: ${level}` `` when persisting, `` `Thinking level: ${level}` `` otherwise.
- `:4973-4984` — the model picker gets `(model) => selectModel(model, false)` for Enter and
  `(model) => selectModel(model, true)` for Ctrl+S, plus
  `defaultProvider && defaultModel ? { provider, id } : undefined` as the badge source.
  `selectModel` (`:4956-4971`) calls `session.setModel(model, { persist })` and shows
  `` `Default model: ${model.provider}/${model.id}` `` when persisting, `` `Model: ${model.id}` `` otherwise.
- The persist itself: [`agent-session.ts:1645-1648`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)
  calls `settingsManager.setDefaultModelAndProvider(provider, id)` — which writes **both**
  `defaultProvider` and `defaultModel` ([`settings-manager.ts:736-743`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts))
  — and then `_addPersistedDefaultToNonEmptyScope(model)` (`:1658-1670`): if the session's scoped-model
  list is **non-empty** and does not already contain the model, the model is appended to it and, when
  `enabledModels` is configured, appended there too. `setThinkingLevel(level, { persist })`
  (`:1780-1784`) calls `settingsManager.setDefaultThinkingLevel(level)`.

## Current state in cyrup-tui

- **`/model` has no Ctrl+S path and no default notion.**
  [`model_selector.rs:461-521`](../../crates/cyrup-tui/src/model_selector.rs) `handle` covers Tab →
  scope (`:462-465`), wrapping Up/Down (`:466-484`), PageUp/PageDown (`:485-495`), `Confirm` →
  `SelectorOutcome::Confirm(format!("{}/{}", m.provider, m.id))` (`:496-503`), `Cancel` (`:504`), then
  a `KeyCode::Char(c)` search arm explicitly guarded by
  `!key.modifiers.contains(KeyModifiers::CONTROL)` (`:507-508`) — so Ctrl+S falls through to
  `SelectorOutcome::Ignored` (`:518`). `body_lines`
  ([`:304-360`](../../crates/cyrup-tui/src/model_selector.rs)) draws the `→` cursor, the provider
  badge and a `✓` on the ACTIVE model (`:324`) but no `· default`; there is no default-aware arm in
  the filter or the sort, and `ModelEntry`
  ([`:33`](../../crates/cyrup-tui/src/model_selector.rs)) carries only "is this the currently-active
  model".
- **The thinking picker is a fixed seven-row `ListSelector`.**
  [`selector/list.rs:206-225`](../../crates/cyrup-tui/src/selector/list.rs) `ListSelector::thinking`
  builds `(level, level, Some(desc))` rows from a `const LEVELS` table and hands them to
  `ListSelector::new`; it is opened at
  [`app/selectors.rs:14-20`](../../crates/cyrup-tui/src/app/selectors.rs) with no extra key handling.
- Crate-wide, no set-as-default path exists: `Key::ctrl('s')` appears only as `ModelsAction::Save`
  ([`keymap.rs:1011`](../../crates/cyrup-tui/src/keymap.rs)) and `SessionSortAction::ToggleSort`
  ([`keymap.rs:1096`](../../crates/cyrup-tui/src/keymap.rs)).
- **The persist mechanism it needs already exists.** `confirm_selector`'s Theme arm
  ([`app/selectors.rs:330-336`](../../crates/cyrup-tui/src/app/selectors.rs)) returns
  `AppCommand::ApplySetting { id, value }`, and the `C::ApplySetting` handler
  ([`app/execute_misc.rs:174-267`](../../crates/cyrup-tui/src/app/execute_misc.rs)) ends in
  `session.persist_setting(SettingsScope::Global, &id, json)` (`:264`) over an arbitrary dotted id
  and pushes the `{id} → {value}` status. The keys are already readable:
  [`cyrup-config/src/settings/effective.rs:33-39`](../../crates/cyrup-config/src/settings/effective.rs)
  `default_provider()` / `default_model()` and `:54-58` `default_thinking_level()`.
- The **session** setters have no persist flag and should not grow one:
  [`cyrup-session-svc/src/session/model.rs:27-42`](../../crates/cyrup-session-svc/src/session/model.rs)
  `set_model(pattern)` and
  [`session/thinking.rs:50-…`](../../crates/cyrup-session-svc/src/session/thinking.rs)
  `set_thinking_level(level)` are the session half only. Persistence rides `ApplySetting`, which is
  cyrup's existing split.
- Today's Enter paths, for reference:
  [`app/execute.rs:281-286`](../../crates/cyrup-tui/src/app/execute.rs)
  (`C::ConfirmSelection { kind: Model, value }` → `session.set_model(&value)`, status
  `model → {value}`) and the `SelectorKind::Thinking` arm of `confirm_selector`
  ([`app/selectors.rs:337-345`](../../crates/cyrup-tui/src/app/selectors.rs)).

The hint ROW for the thinking picker is **not** this task — it lands with
[`TUI_THINKING_PICKER_IS_A_BARE_LIST.md`](TUI_THINKING_PICKER_IS_A_BARE_LIST.md), which ports that
picker's whole envelope including the `Enter to select · Ctrl+S to set as default · Esc to cancel`
line. Coordinate so the line is written once.

## Subtasks

1. **Carry the default into `/model`.** Extend `ModelEntry`
   ([`model_selector.rs:33`](../../crates/cyrup-tui/src/model_selector.rs)) with an `is_default`
   flag, and populate it where the picker's rows are built — `handle_model_command`
   ([`app/selectors.rs:122`](../../crates/cyrup-tui/src/app/selectors.rs)) — from
   `session.services().settings.effective().default_provider()` + `.default_model()`, matching on
   **both** (pi `isDefaultModel`, `model-selector.ts:252-254`). A missing pair means no row is
   default.
2. **Badge the row.** In `body_lines`
   ([`model_selector.rs:304-360`](../../crates/cyrup-tui/src/model_selector.rs)) append
   `" · default"` in the muted style after the existing `✓`/provider spans, per
   `model-selector.ts:316-317`.
3. **Sort default second.** Wherever `/model` orders its rows (the current-model-first ordering the
   module doc at [`:5`](../../crates/cyrup-tui/src/model_selector.rs) describes), insert pi's second
   key — default before the provider `localeCompare` — per `model-selector.ts:225-240`.
4. **Default-aware search.** In the filter used by
   [`model_selector.rs:134`](../../crates/cyrup-tui/src/model_selector.rs) `filtered()`, append
   `" default"` to a default row's fuzzy haystack, and when the trimmed lowercase query is a
   non-empty prefix of `"default"` hoist default rows to the front and de-duplicate the remainder by
   `provider` + `id` (`model-selector.ts:256-259`, `:270-290`).
5. **Add the Ctrl+S arm to `/model`.** In `handle`
   ([`model_selector.rs:461`](../../crates/cyrup-tui/src/model_selector.rs)), before the
   `KeyCode::Char` search arm at `:507`, match `Ctrl+S` and emit a distinguishable outcome carrying
   the same `"{provider}/{id}"` value the `Confirm` arm produces. `SelectorOutcome` has no
   "confirm-and-persist" variant today — either add one, or reuse `Apply(payload)`
   ([`app/selectors.rs:196`](../../crates/cyrup-tui/src/app/selectors.rs)), which already carries a
   `\u{1f}`-separated payload and keeps the slot open; prefer whichever leaves the picker CLOSING,
   since pi calls `this.dispose()` before the callback (`model-selector.ts:404`).
6. **Handle it in the app.** The Ctrl+S path must do what Enter does **and then** persist: run the
   existing `session.set_model(&value)` ([`app/execute.rs:281-286`](../../crates/cyrup-tui/src/app/execute.rs))
   and emit `AppCommand::ApplySetting` for `defaultProvider` and `defaultModel` — **both keys**, as
   `setDefaultModelAndProvider` writes both ([`settings-manager.ts:736-743`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts)).
   Status text is `` Default model: {provider}/{id} `` (pi `interactive-mode.ts:4964`), not the
   generic `{id} → {value}` the `ApplySetting` arm pushes — suppress or override that for this path.
7. **Port `_addPersistedDefaultToNonEmptyScope`** ([`agent-session.ts:1658-1670`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)):
   when persisting a default model and the session's scoped-model list is non-empty and lacks the
   model, append it to the scope and — if `enabledModels` is configured and does not already list
   `provider/id` case-insensitively — append it there too. Without this, saving a default outside the
   current scope makes the default unreachable on the next launch.
8. **Add the Ctrl+S arm to the thinking picker.** `ListSelector::thinking`
   ([`selector/list.rs:206-225`](../../crates/cyrup-tui/src/selector/list.rs)) is a shared component,
   so keep the key handling out of the generic `ListSelector` if it would leak into `show_images` /
   `theme` — gate it on the selector kind, or on an opt-in flag set only by `thinking`. The app-side
   effect is the existing session set plus `AppCommand::ApplySetting { id: "defaultThinkingLevel" }`,
   with the status `` Default thinking level: {level} `` (pi `interactive-mode.ts:4778`).
9. **Badge the thinking rows.** Suffix `" · default"` to the description of the row whose level
   equals `default_thinking_level()` (falling back to `cyrup_config::DEFAULT_THINKING_LEVEL` ([`crates/cyrup-config/src/lib.rs:50`](../../crates/cyrup-config/src/lib.rs)) when
   unset, as `interactive-mode.ts:4800` does), per `thinking-selector.ts:73`. That means
   `ListSelector::thinking` needs the default level as a second argument alongside `current`.

## Acceptance criteria

- [ ] Ctrl+S in `/model` closes the picker, switches the session to the highlighted model, AND
      persists `defaultProvider` + `defaultModel` to the global settings layer
- [ ] Ctrl+S in the thinking picker sets the session level AND persists `defaultThinkingLevel`
- [ ] The two status strings are `Default model: {provider}/{id}` and
      `Default thinking level: {level}` — not the generic `{id} → {value}`
- [ ] Enter in either picker still does **not** persist: `defaultModel` / `defaultThinkingLevel` are
      unchanged in settings after an Enter selection
- [ ] `/model` renders `· default` in the muted style on exactly the row matching BOTH the persisted
      `defaultProvider` and `defaultModel`; no row is badged when either key is unset
- [ ] With a current model and a different default model present, `/model`'s first row is the current
      model and the second is the default (`model-selector.ts:225-240`)
- [ ] Typing `d`, `de` … `default` in `/model` lists the default row(s) first, with no duplicate rows
- [ ] The thinking picker's default row's description ends in ` · default`
- [ ] Persisting a default model while the session has a non-empty scoped-model list adds that model
      to the scope, and to `enabledModels` when that setting is configured
      (`agent-session.ts:1658-1670`)
- [ ] `grep -n "ctrl('s')" crates/cyrup-tui/src/keymap.rs` still shows the `ModelsAction::Save` and
      `SessionSortAction::ToggleSort` bindings unchanged
- [ ] Ctrl+S does nothing new in the `show_images` and `theme` list selectors
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/model_selector_assembled.rs`,
      `src/tests/scoped_models.rs` or `src/tests/thinking.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
