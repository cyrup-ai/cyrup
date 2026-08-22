---
stage: qa
status: completed
updated: 2026-08-22 21:05
---

# Decompose settings.rs Into A src/settings/ Module Tree

## Description

[`crates/cyrup-config/src/settings.rs`](../../crates/cyrup-config/src/settings.rs) is **3,089
lines** — by far the largest file left in the crate now that `model.rs` was split in PR #38
(next largest: `auth.rs` 954, `login.rs` 1720 but cohesive, `config_value.rs` 820). Lines
`1739-3089` (1,351) are a single `#[cfg(test)] mod tests`, so ~1,738 lines of production code hold
ten value types, two store implementations, a trait, a manager, and five free functions.

### The seams are already clean

`grep -n '^pub struct \|^pub enum \|^pub fn \|^fn \|^impl \|^pub trait \|^#\[cfg(test)\]'` gives
five contiguous, non-interleaved regions:

1. **Scope + value types** `:18-211` — `SettingsScope`:18, `DefaultProjectTrust`:26, `MermaidRenderingMode`:37 (+`impl`:45), `ThinkingBudgets`:59, `Warnings`:73, `ProviderRetrySettings`:80, `BranchSummarySettings`:88, `CompactionSettings`:96, `RetrySettings`:105, `PackageSource`:118 (+`impl`:138)
2. **The `Settings` layer type** `:214-401` — `pub struct Settings`:214, `impl Settings`:218, `fn strip_global_only`:392
3. **Migration + merge free functions** `:403-534` — `migrate_settings`:403, `random_uuid_v4`:469, `deep_merge`:516
4. **The `EffectiveSettings` merged view** `:536-1151` — `EffectiveSettings`:536, `impl`:540, `parse_http_idle_timeout_ms`:1117, `expand_tilde`:1148
5. **Stores + manager** `:1153-1737` — `pub trait SettingsStore`:1153, `FileSettingsStore`:1167 (+`impl`:1172, `impl SettingsStore`:1188), `InMemorySettingsStore`:1218 (+`impl`:1223, `impl SettingsStore`:1242), `SettingsManager`:1273 (+`impl`:1291), `set_value_at_path`:1712

### Churn outside the crate is zero — if the facade is complete

`src/lib.rs:72-77` re-exports 16 items, so external callers naming `cyrup_config::X` are untouched.

**But four more `pub` types live in settings.rs and are NOT re-exported at the crate root** —
`ThinkingBudgets`:59, `Warnings`:73, `ProviderRetrySettings`:80, `BranchSummarySettings`:88
(verified: each is `pub struct` in settings.rs and appears 0 times in `lib.rs`). They are reachable
only as `cyrup_config::settings::X`. The new `src/settings/mod.rs` facade must therefore re-export
**20** items, not 16, to keep the `settings::` path intact.

### Fold in the duplicated string-list helper while you are here

Two byte-identical `Vec<String>` extractors differ only in which field they read:

- `Settings::layer_string_list` — `settings.rs:322-333`, reads `self.obj`, private
- `EffectiveSettings::string_list` — `settings.rs:1041-1052`, reads `self.merged` (which *is* a `Settings`)

Because `EffectiveSettings.merged: Settings`, `self.string_list(k)` is exactly
`self.merged.layer_string_list(k)`. Four getter pairs prove the duplication is mechanical
(EffectiveSettings vs Settings): `extension_paths` 1075/353, `skill_paths` 1080/336,
`prompt_template_paths` 1086/341, `theme_paths` 1092/346. Deleting `EffectiveSettings::string_list`
and delegating costs exactly one visibility widening — `layer_string_list` becomes `pub(crate)`.

### Precedent

PR #38 split `model.rs` (3,791 lines) the same way: private submodules behind a complete `mod.rs`
re-export facade, so `cyrup_config::model::X` stayed the one public path and no
`cyrup_config::model::<submodule>::X` path was minted. Follow that shape.

**Out of scope:** `src/model/` (already decomposed). This is a pure code move — no behavior change,
no new tests.

## Acceptance Criteria

- [ ] `src/settings.rs` no longer exists; `src/settings/mod.rs` plus submodules replace it, and no submodule exceeds ~900 lines
- [ ] `src/settings/mod.rs` re-exports all 20 public items so both `cyrup_config::X` and `cyrup_config::settings::X` resolve exactly as before — including `ThinkingBudgets`, `Warnings`, `ProviderRetrySettings`, `BranchSummarySettings`
- [ ] `src/lib.rs:72-77`'s `pub use settings::{...}` block is unchanged (same 16 names)
- [ ] `grep -rn 'fn string_list' crates/cyrup-config/src/settings/` returns no match; the four `EffectiveSettings` getters delegate to `Settings::layer_string_list`, now `pub(crate)`
- [ ] `cargo build --workspace` succeeds with no changes to any file outside `crates/cyrup-config/src/`
- [ ] `cargo test -p cyrup-config` shows the same pass/fail set as before the move

## Outcome — completed

Landed in `749cbb9`, merged to `main` via #46 (squashed into `7e221a3`).

`settings.rs` (3,089 lines) became `src/settings/`: seven production submodules — `types` 179,
`layer` 212, `migrate` 72, `merge` 23, `effective` 621, `store` 127, `manager` 538 — plus a
three-way test split and a 37-line `mod.rs`. Largest file 621, under the ~900 cap.

All six acceptance criteria met. The facade re-exports **20** items as required, the count having
been re-derived from the real file rather than trusted: the 16 in `lib.rs` plus `ThinkingBudgets`,
`Warnings`, `ProviderRetrySettings` and `BranchSummarySettings`, which are reachable only as
`cyrup_config::settings::X` and would otherwise have silently left the public API. `src/lib.rs` is
byte-identical. `grep -rn 'fn string_list' src/settings/` returns nothing — the four
`EffectiveSettings` getters delegate to `Settings::layer_string_list`, now `pub(crate)`.

The move was verified verbatim rather than assumed: `types`, `migrate`, `merge`, `store` and
`manager` diff byte-identical against their original line ranges; `layer` differs only in the ten
intended lines (nine visibility widenings and one doc-link rewrite).

Every line number and count cited in this task file checked out against the real file.
