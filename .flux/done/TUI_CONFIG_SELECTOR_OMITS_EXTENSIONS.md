---
stage: done
status: completed
updated: 2026-08-28
---

# Make `cyrup config` Able To Enable/Disable Extensions: Discovery, Override Honoring, Then The Row Kind

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** medium · **Kind:** missing-feature · **Area:** Selectors, settings and dialogs

## Objective

`cyrup config` shows Skills, Prompts and Themes but no Extensions group, so the only way to disable
a misbehaving extension — or force-load one for a single project — is hand-editing the `extensions`
array in `settings.json`. Upstream, Extensions is the **first** group in every scope. Closing this
needs more than a fourth enum variant: nothing currently enumerates loose extensions as
config rows, and the `-pattern` disable half of the `extensions` array is honored nowhere.

## Upstream reference

[`packages/coding-agent/src/cli/config-selector.ts`](../../tmp/pi/packages/coding-agent/src/cli/config-selector.ts):

- `:25` `type ResourceType = "extensions" | "skills" | "prompts" | "themes"`; `:31` `RESOURCE_TYPES`;
  `:33-38` `RESOURCE_TYPE_LABELS` with `extensions: "Extensions"`.
- `:143-146` `addToGroup(resolved.extensions, "extensions")` is the **first** call.
- `:160` `typeOrder = { extensions: 0, skills: 1, prompts: 2, themes: 3 }` — Extensions renders first
  in every group.
- `:126-129` the extensions-specific display name: `parentFolder/fileName` when the parent directory
  is not literally `extensions`.
- `:462-503` `toggleTopLevelResource` writes `+pattern` / `-pattern` into the `extensions` settings
  array via `setExtensionPaths` / `setProjectExtensionPaths`.
- The auto-discovery that produces those rows is
  [`package-manager.ts:2417-2424`](../../tmp/pi/packages/coding-agent/src/core/package-manager.ts)
  `collectAutoExtensionEntries`.

## Current state in cyrup-tui

### The editor knows only three kinds

- [`config_selector.rs:114-121`](../../crates/cyrup-tui/src/config_selector.rs) —
  `pub enum ConfigKind { Skills, Prompts, Themes }`. `key()` (`:123-131`), `label()` (`:133-141`) and
  `order()` (`:143-150`) cover the same three. `grep -ni extension config_selector.rs` returns
  **zero** hits.
- The module doc (`:1-19`) scopes out only the **package tier** (pi's `togglePackageResource`,
  `config-selector.ts:505-562`). It never mentions extensions — they were dropped silently.
- [`crates/cyrup/src/subcommands.rs:825`](../../crates/cyrup/src/subcommands.rs) — the only
  production caller — iterates `[ConfigKind::Skills, ConfigKind::Prompts, ConfigKind::Themes]`.
  `settings_array` (`:913-921`) maps those three to `skill_paths()` / `prompt_template_paths()` /
  `theme_paths()`. The row collector at `:1017-1070` walks
  `universe.registry.skills` / `.prompts` / `.themes` only, via `loose_pattern` (`:1081`).
- The write target already exists and is already plumbed: `Settings::extension_paths()`
  ([`crates/cyrup-config/src/settings/layer.rs:169-171`](../../crates/cyrup-config/src/settings/layer.rs))
  is passed as `ResourceOverrides.extensions` at
  [`subcommands.rs:996,1002`](../../crates/cyrup/src/subcommands.rs) — and is unused by this editor.

### Two prerequisites the row work depends on

1. **No loose-extension discovery in `cyrup-resources`.**
   [`discovery/blocking.rs:134-186`](../../crates/cyrup-resources/src/discovery/blocking.rs)
   `collect_global_loose` scans skills/prompts/themes roots only; there is no analogue of
   `collectAutoExtensionEntries`. `ResolvedPaths::ext_crate_paths`
   ([`discovery/mod.rs:260`](../../crates/cyrup-resources/src/discovery/mod.rs), written at
   `blocking.rs:79`) therefore holds package/settings-declared roots only — there would be no
   top-level rows to list. Loose extension discovery **does** exist, but in a different crate and
   with a different shape: [`crates/cyrup-ext/src/loader.rs:119`](../../crates/cyrup-ext/src/loader.rs)
   `discover(&DiscoveryRoots)` (`:65-72`: `<cwd>/.cyrup/extensions`, `<agentDir>/extensions`, plus
   configured paths), with `scan_dir` at `:237`.
2. **The `-pattern` disable half is honored nowhere.**
   [`discovery/scan.rs:245-256`](../../crates/cyrup-resources/src/discovery/scan.rs) treats the
   `extensions` array purely as a *positive* listing (`resolve_local_entries`), unlike
   skills/prompts/themes which get an `override_enabled(...)` retain pass
   (`blocking.rs:165,173,183` global; `:585,630,639,649` project). `cyrup-ext/src/loader.rs` applies
   no override filter at all.

## Subtasks

1. **`crates/cyrup-resources/src/discovery/blocking.rs`** — add loose-extension enumeration
   alongside `collect_global_loose` (`:134-186`) and its project counterpart, mirroring pi's
   `collectAutoExtensionEntries` (`package-manager.ts:2417-2424`). Reuse the root set
   `cyrup-ext/src/loader.rs:65-72` already names rather than inventing a second convention.
2. **`crates/cyrup-resources/src/discovery/scan.rs:245-256`** — make the `extensions` array honor
   `-pattern` as well as `+pattern`, applying the same `override_enabled(...)` retain pass the other
   three resource kinds get in `blocking.rs`.
3. **`crates/cyrup-ext/src/loader.rs`** — apply the resolved override filter so a `-pattern`
   disable is respected at load time, not only in the config listing.
4. **`crates/cyrup-tui/src/config_selector.rs`** — add `ConfigKind::Extensions`; `key()` → `"extensions"`,
   `label()` → `"Extensions"`, and `order()` → `0`, shifting Skills/Prompts/Themes to 1/2/3
   (`config-selector.ts:160`). Update the module doc (`:1-19`) to state that the top-level extension
   tier is now covered and only the package tier is out of scope.
5. **`crates/cyrup-tui/src/config_selector.rs`** — implement the extensions-specific display name
   (`config-selector.ts:126-129`): `parentFolder/fileName` when the parent directory is not literally
   `extensions`, plain file name otherwise.
6. **`crates/cyrup/src/subcommands.rs`** — add `ConfigKind::Extensions` to the seed loop at `:825`,
   map it in `settings_array` (`:913-921`) to `layer.extension_paths()`, and extend the row collector
   at `:1017-1070` to walk the new loose-extension registry entries through the existing
   `loose_pattern` (`:1081`) `+`/`-` pattern machinery.

## Acceptance criteria

- [ ] `crates/cyrup-tui/src/config_selector.rs` declares a fourth `ConfigKind` variant whose `key()`
      is `"extensions"`, `label()` is `"Extensions"` and `order()` is `0`; the other three orders are
      `1`/`2`/`3`.
- [ ] `crates/cyrup/src/subcommands.rs:825`'s array contains four kinds, and `settings_array` has an
      `Extensions => layer.extension_paths()` arm.
- [ ] The row collector emits `ConfigKind::Extensions` rows for loose extensions discovered under
      `<cwd>/.cyrup/extensions` and `<agentDir>/extensions`.
- [ ] `crates/cyrup-resources/src/discovery/scan.rs` applies an `override_enabled(...)`-style retain
      pass to the `extensions` entries, so an `-<pattern>` entry removes an otherwise-discovered
      extension.
- [ ] Toggling an extension row off writes `-<pattern>` into the `extensions` array of the selected
      scope's settings layer, and toggling on writes `+<pattern>` — the same shape the skills rows
      already produce.
- [ ] An extension disabled through `cyrup config` is not loaded by `cyrup-ext/src/loader.rs` on the
      next run.
- [ ] The Extensions group renders **above** Skills in every scope group.
- [ ] `cargo build -p cyrup-tui -p cyrup -p cyrup-resources -p cyrup-ext` → 0 warnings.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
