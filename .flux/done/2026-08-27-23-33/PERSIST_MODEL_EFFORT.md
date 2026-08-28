---
stage: qa
status: completed
updated: 2026-08-28 01:58
---

# Persist model + effort — complete

Pi's `Ctrl+S` set-as-default is ported for both the model picker and the thinking-level picker:
a new `SelectorOutcome::ConfirmDefault`, a `persist_hint`/`default_model` opt-in that gates both the
key and its footer, per-model thinking tiers through `cyrup-config` and `cyrup-session-svc`, and the
`/thinking` command and `/settings` submenu that reach them.

**Gates at completion:** 1,276 pass; `cargo check --workspace --all-targets`, crate-scoped clippy and
`cargo doc` all exit 0. Clippy's single `cyrup-tui` warning is the pre-existing byte-str lint at
`escape_reassembly.rs:972`, untouched by this task.

## What the review cycles caught

Three QA rounds, each closed:

1. **Unpinned confirm keys.** `Ctrl+S` and `Enter` had no test holding them apart. Fixed by four
   tests asserting the exact variant *and* value on both keys, in both pickers.
2. **Under-asserting negative tests.** `!matches!(…, ConfirmDefault(_))` also passes for
   `Confirm(value)` — an un-wired picker that began confirming a selection the user never asked for
   would have stayed green. Tightened to `assert_eq!(…, SelectorOutcome::Ignored)` across all three
   un-wired cases, and revert-proven in both directions: mutating both `None` arms to `Confirm`
   fails the tightened tests while the old loose form passes that identical regression.
3. **Comments that outran the evidence.** A test name and two comments claimed `Ctrl+S` "falls
   through to the search input" and types; it does not — `action_for` returns `None` and the `None`
   arm refuses control chars, so `handle` returns `Ignored` first. Renamed and rewritten. A later
   pass then corrected a clause of my own — "no default binding claims `ctrl+s`" is false of pi
   (`app.session.toggleSort`, `app.models.save`; `keybindings.ts:166,182`), narrowed to the
   `tui.editor.*` / `tui.input.submit` / `tui.select.cancel` ids `Input.handleInput` actually
   consults — and an off-by-one citation (`settings-manager.ts:805` → `:804`).

## Divergences recorded

- **`modelThinkingLevels` is written as a whole map**, never as a dotted key. `persist_setting`
  splits keys on `.` (`accessors.rs:135`) and 325 catalog model ids contain a dot, so a
  `modelThinkingLevels.{provider}/{id}` key would split mid-id and write a bogus nested object. Pi
  indexes its map directly with the composed string (`settings-manager.ts:804`) and has no such
  hazard; reading, modifying and writing the map back reaches the same end state, dot-safe.
- **Un-wired `Ctrl+S` returns `Ignored` one layer earlier than pi.** Pi hands the key to its search
  input, which then drops it as a C0 control char (`input.ts:203-209`). Cyrup refuses it at the
  `None` arm instead. Not a behavioural divergence — identical user-visible effect, different layer.
- **The `Enter` path keeps cyrup's existing `model → {value}` status line** rather than the task's
  `Model: {id}`; changing it was explicitly out of scope, and the persist path says
  `Default model: …` as specified, so the two remain distinguishable.
