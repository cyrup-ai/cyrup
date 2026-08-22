---
stage: aug
status: done
updated: 2026-08-22 17:44
---

# Decompose `cyrup-permission-system/src/extension.rs` Into Submodules

## Description

[`crates/cyrup-permission-system/src/extension.rs`](../../crates/cyrup-permission-system/src/extension.rs)
is **4,681 lines** — by far the largest file in the crate (next largest:
[`forwarding.rs`](../../crates/cyrup-permission-system/src/forwarding.rs) at 1,662). It is not one
concern; it is **twelve** concerns plus a 1,735-line inline `mod tests`, all sharing one file because
they all touch one struct.

Split it into `src/extension/` submodules grouped by logical concern. **Pure relocation — zero
behaviour change, zero public-API change, zero doc-comment rewriting.**

## Current shape (measured)

| Lines | Region | Contents |
| ----- | ------ | -------- |
| 1–59 | module doc | The `//!` wiring narrative (WIRING / LIVE HUMAN DIALOG / CHILD→PARENT FORWARDING / FOUR SUPPLEMENTARY LAYERS) |
| 61–92 | imports | `std`, `cyrup_core`, `cyrup_ext`, 15 × `crate::…` |
| 93–144 | consts | `EXTENSION_ID`, `PERMISSION_SYSTEM_COMMAND`, `PERMISSION_REQUEST_EVENT_CHANNEL`, `NO_EVENT_BACKEND_ERROR`, `COMMAND_YOLO_CONTROL_SOURCE`, `YOLO_PERSIST_FALLBACK_ERROR`, `PERMISSIONS_JSON_SCHEMA`, `PERMISSIONS_EXAMPLE_CONFIG` |
| 143–154 | command text | `COMMAND_USAGE`, `on_off` |
| 156–162 | path consts | `POLICY_FILE`, `CONFIG_DIR`, `CONFIG_FILE`, `PROJECT_AGENT_SUBDIR` |
| 164–218 | env contract | `CHILD_ENV_VAR`, `SUBAGENT_ENV_HINT_KEYS`, `INSTALL_ENV_VAR`, `POLICY_AGENT_DIR_ENV_KEY` |
| 220–238 | path helper | `policy_agent_dir` |
| 240–298 | warnings | `WarningSink` + `impl` + `manager_with_warnings` |
| 300–409 | **the struct** | `pub struct PermissionSystemExtension` — 20 fields, each with a load-bearing doc block |
| 411–464 | runtime api | `PublishedRuntimeApi` + `impl PermissionSystemRuntimeApi` + `impl` |
| 466–2372 | **`impl PermissionSystemExtension`** | 49 methods, ~1,900 lines |
| 2374–2418 | gate helpers | `dedup_details`, `GateCall<'a>`, `decision_state_str`, `source_str` |
| 2441–2446 | lock helper | `pub(crate) fn guard` |
| 2448–2462 | env probe | `resolve_agent_name_from_env` |
| 2464–2739 | **`impl NativeExtension`** | `id`, `is_ambient`, `set_host_services`, `init`, `execute_command`, `on_event` |
| 2740–2800 | env probes | `is_subagent_child`, `has_subagent_env_hint`, `env_truthy` |
| 2801–2944 | install/factory | `dir_has_entry`, `is_installed`, `permission_extension_for_env` |
| 2946–4681 | **`mod tests`** | 35 `#[test]`/`#[tokio::test]` fns + 19 fixtures/fakes/locks |

The 49 methods of the one `impl` block, in file order:

```
new  new_with_config  new_forwarding_parent  new_forwarding_parent_with_config
new_forwarding_child  new_forwarding_child_with_config                          → construction
manager_paths_for  config_path_for  resolved_config_path_for  logs_dir_for       → paths
load_config  refresh_config_and_manager  refresh_extension_config
invalidate_agent_start_cache  report_config_warning  sync_status_when_possible
save_extension_config  config_controller                                         → config lifecycle
yolo_mode  set_yolo_mode  toggle_yolo_mode  publish_runtime_api
retract_runtime_api                                                              → yolo + runtime api
emit_permission_request_event  emit_permission_state_event                       → event bus
run_permission_system_command  notify_save_failure  open_settings_overlay
render_settings  config_path_line                                                → slash command
from_parts  from_parts_full  into_shared  with_agent_name                        → construction
write_debug_entry  write_review_entry  review_permission_decision
permission_decision_scope                                                        → audit trail
decide  registered_tool_names  resolve_skill_read  resolve_external_directory     → the gate
resolve_prompt_decision  forget_prompt_decision  prompt_decision  resolve_ask
apply_decision                                                                   → ask/prompt
on_before_agent_start  shape_agent_start_prompt  should_expose_tool              → context hygiene
maybe_start_forwarding_watcher  stop_forwarding_watcher  has_live_forwarding_watcher
live_watcher_task_count  publish_parent_session_anchor                           → forwarding watcher
```

## Why this is safe in Rust (the three mechanics the whole plan rests on)

1. **Inherent `impl` blocks may be split across any modules of the defining crate.** `impl
   PermissionSystemExtension { … }` in `extension/decide.rs` adds methods to the type declared in
   `extension/mod.rs`. Call sites (`ext.decide(…)`, `PermissionSystemExtension::is_installed`) are
   **unchanged** — inherent method resolution does not care which module the `impl` lives in. No
   re-export, no trait, no import needed at any caller.
2. **A private field is visible in its defining module *and every descendant*.** Keeping `pub struct
   PermissionSystemExtension` in `extension/mod.rs` means every `extension::*` submodule can read
   and write its private fields with **no visibility annotations on the 20 fields at all**. This is
   why the struct does not move.
3. **A private *method* is private to the module holding its `impl` block.** This is the one real
   mechanical change: a method defined in `extension::decide` and called from `extension::native`
   is `E0624: method is private`. Fix: `fn foo` → `pub(super) fn foo`. `pub(super)` from
   `extension::decide` means "visible in `extension` and its descendants" — it is **not** reachable
   from `crate::` root or from outside the crate, so **the public API does not widen**.

## Target layout

Create `src/extension/` and delete `src/extension.rs`. Follow the crate's existing
`mod.rs` convention ([`src/sanitize/mod.rs`](../../crates/cyrup-permission-system/src/sanitize/mod.rs),
[`src/tests/mod.rs`](../../crates/cyrup-permission-system/src/tests/mod.rs)) — **not** the
`extension.rs` + `extension/` sibling form.

| File | Moves from (line ranges) | Contents | ~lines |
| ---- | ------------------------ | -------- | ------ |
| `mod.rs` | 1–59, 300–409, 2441–2446 | The `//!` doc verbatim, `mod` declarations, the re-export block, `pub struct PermissionSystemExtension`, `pub(crate) fn guard` | 215 |
| `consts.rs` | 93–142 | Extension id, command name, event channel, the two error strings, `COMMAND_YOLO_CONTROL_SOURCE`, the two `include_str!` artifacts | 52 |
| `env.rs` | 164–218, 2448–2462, 2740–2800 | The env-var contract (`CHILD_ENV_VAR`, `SUBAGENT_ENV_HINT_KEYS`, `INSTALL_ENV_VAR`, `POLICY_AGENT_DIR_ENV_KEY`) + every probe that reads it (`is_subagent_child`, `has_subagent_env_hint`, `env_truthy`, `resolve_agent_name_from_env`) | 130 |
| `paths.rs` | 156–162, 220–238, 563–638 | Where things live on disk: `POLICY_FILE`/`CONFIG_DIR`/`CONFIG_FILE`/`PROJECT_AGENT_SUBDIR`, `policy_agent_dir`, `manager_paths_for`, `config_path_for`, `resolved_config_path_for`, `logs_dir_for` | 110 |
| `warnings.rs` | 240–298 | `WarningSink` + `manager_with_warnings` | 60 |
| `construct.rs` | 466–562, 1214–1321, 1401–1407 | `new*`, `new_forwarding_parent*`, `new_forwarding_child*`, `from_parts`, `from_parts_full`, `into_shared`, `with_agent_name` | 213 |
| `config.rs` | 639–811 | `load_config`, `refresh_config_and_manager`, `refresh_extension_config`, `invalidate_agent_start_cache`, `report_config_warning`, `sync_status_when_possible`, `save_extension_config`, `config_controller` | 174 |
| `yolo.rs` | 411–464, 812–942 | `PublishedRuntimeApi` (+ both its impls), `yolo_mode`, `set_yolo_mode`, `toggle_yolo_mode`, `publish_runtime_api`, `retract_runtime_api` | 185 |
| `events.rs` | 943–1037 | `emit_permission_request_event`, `emit_permission_state_event` | 94 |
| `command.rs` | 143–154, 1038–1213 | `COMMAND_USAGE`, `on_off`, `run_permission_system_command`, `notify_save_failure`, `open_settings_overlay`, `render_settings`, `config_path_line` | 187 |
| `audit.rs` | 1322–1400, 2419–2440 | `write_debug_entry`, `write_review_entry`, `review_permission_decision`, `permission_decision_scope`, `decision_state_str`, `source_str` | 103 |
| `decide.rs` | 1408–1751, 2374–2418 | The `before_tool_call` gate: `decide`, `registered_tool_names`, `resolve_skill_read`, `resolve_external_directory`, `GateCall<'a>`, `dedup_details` | 389 |
| `prompt.rs` | 1752–2054 | The ask tier: `resolve_prompt_decision`, `forget_prompt_decision`, `prompt_decision`, `resolve_ask`, `apply_decision` | 303 |
| `agent_start.rs` | 2055–2253 | `on_before_agent_start`, `shape_agent_start_prompt`, `should_expose_tool` | 199 |
| `watcher.rs` | 2254–2371 | `maybe_start_forwarding_watcher`, `stop_forwarding_watcher`, `has_live_forwarding_watcher`, `live_watcher_task_count`, `publish_parent_session_anchor` | 119 |
| `native.rs` | 2464–2739 | `impl NativeExtension for PermissionSystemExtension` — the whole trait impl, untouched | 277 |
| `install.rs` | 2801–2944 | `dir_has_entry`, `pub fn is_installed`, `pub fn permission_extension_for_env` | 144 |
| `tests/` | 2946–4681 | see **Test split** below | 1,735 |

Nothing lands over ~400 lines; the median file is ~180.

## Mechanical procedure

### Step 1 — carve the source

Move each item **with its preceding `///` doc block and any attributes** (`#[must_use]`,
`#[derive(…)]`, `#[async_trait]`). Line 2842 is `#[must_use]` sitting above `pub fn is_installed` at
2843; the same pattern recurs at 812, 1213, 1400. Cutting on a bare item line silently drops an
attribute.

**Do not reword, reflow, reformat, or "improve" a single doc comment, comment, or line of code.**
Every `///` in this file cites a pi source line (`index.ts:1586-1592`) or a ticket (`PERM-005`,
`R-PERM-010`) — that provenance is the crate's review record. The diff must read as pure relocation.

Each submodule that holds methods opens its own `impl PermissionSystemExtension { … }` block. Each
carries only the imports it needs — `src/lib.rs`'s `#![deny(...)]` plus the workspace lints will not
flag unused imports, but `cargo check` warns, so trim per file rather than copying the 30-line
import header everywhere.

### Step 2 — `mod.rs`

```rust
//! <lines 1-59 verbatim>

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::HostServices;

use crate::agent_start_cache::AgentStartCache;
use crate::ask::AskChannel;
use crate::dedup::DedupCache;
use crate::forwarding;
use crate::manager::PermissionManager;
use crate::skill::SkillPromptEntry;
use crate::stores::SessionApprovalStore;

mod agent_start;
mod audit;
mod command;
mod config;
mod construct;
mod consts;
mod decide;
mod env;
mod events;
mod install;
mod native;
mod paths;
mod prompt;
mod warnings;
mod watcher;
mod yolo;

#[cfg(test)]
mod tests;

// Re-exported so `crate::extension::…` and `cyrup_permission_system::extension::…` keep resolving
// exactly as they did when this was one file — `src/lib.rs:104-108` re-exports six of these,
// `tests/shipped_artifacts.rs:19` imports two directly, and `logging.rs:47` / `status.rs:12` take
// `EXTENSION_ID`. It is ALSO what makes `use super::*` inside `tests/` reach items that now live in
// sibling modules.
pub use consts::{
    COMMAND_YOLO_CONTROL_SOURCE, EXTENSION_ID, PERMISSIONS_EXAMPLE_CONFIG, PERMISSIONS_JSON_SCHEMA,
    PERMISSION_REQUEST_EVENT_CHANNEL, PERMISSION_SYSTEM_COMMAND,
};
pub use env::{CHILD_ENV_VAR, INSTALL_ENV_VAR, POLICY_AGENT_DIR_ENV_KEY, SUBAGENT_ENV_HINT_KEYS};
pub use install::{is_installed, permission_extension_for_env};

// <lines 300-409 verbatim: the struct + its 20 documented fields>

// <lines 2441-2446 verbatim: pub(crate) fn guard>
```

`guard` stays in `mod.rs` rather than a one-function `util.rs`: it is 3 lines,
[`config_modal.rs`](../../crates/cyrup-permission-system/src/config_modal.rs) reaches it as
`crate::extension::guard` at six call sites, and putting it here means that path needs no re-export.

### Step 3 — visibility (the only edit to code bodies)

Exact set, derived from the call-site map. Everything else keeps the visibility it has today.

**Inherent methods: `fn` → `pub(super) fn`**

| Method | new module | called from |
| ------ | ---------- | ----------- |
| `manager_paths_for` | `paths` | `construct` (×3), `config` |
| `logs_dir_for` | `paths` | `construct` |
| `load_config` | `config` | `construct` (×3) |
| `refresh_config_and_manager` | `config` | `native` (×2) |
| `refresh_extension_config` | `config` | `native` |
| `invalidate_agent_start_cache` | `config` | `native` |
| `sync_status_when_possible` | `config` | `yolo` |
| `publish_runtime_api` | `yolo` | `native` |
| `retract_runtime_api` | `yolo` | `native` |
| `emit_permission_state_event` | `events` | `prompt` (×3) |
| `run_permission_system_command` | `command` | `native` |
| `write_debug_entry` | `audit` | `config`, `yolo` (×2), `events`, `native` (×2) |
| `write_review_entry` | `audit` | `decide` (×5) |
| `review_permission_decision` | `audit` | `decide`, `prompt` (×6) |
| `permission_decision_scope` | `audit` | `decide`, `prompt` (×2) |
| `decide` | `decide` | `native` |
| `prompt_decision` | `prompt` | `decide` (×2) |
| `resolve_ask` | `prompt` | `decide` |
| `on_before_agent_start` | `agent_start` | `native` |
| `maybe_start_forwarding_watcher` | `watcher` | `native` (×4) |
| `stop_forwarding_watcher` | `watcher` | `native` |
| `publish_parent_session_anchor` | `watcher` | `native` |
| `has_live_forwarding_watcher` | `watcher` | `tests::watcher` |
| `live_watcher_task_count` | `watcher` | `tests::watcher` |

**Free items: `fn`/`const`/`struct` → `pub(super)`**

| Item | new module | called from |
| ---- | ---------- | ----------- |
| `policy_agent_dir` | `paths` | `paths`, `command`, `install` |
| `POLICY_FILE` | `paths` | `paths`, `command`, `install` |
| `PROJECT_AGENT_SUBDIR` | `paths` | `paths`, `install` |
| `manager_with_warnings` | `warnings` | `config`, `construct` |
| `WarningSink` + `new`/`notify`/`reset` | `warnings` | `mod` (field type), `construct`, `config`, `native` |
| `decision_state_str` | `audit` | `decide`, `prompt` (×3) |
| `source_str` | `audit` | `decide` |
| `dedup_details` | `decide` | `decide`, `prompt` (×2) |
| `env_truthy` | `env` | `install` |
| `is_subagent_child` | `env` | `prompt` |
| `has_subagent_env_hint` | `env` | `env`, `tests::env` |
| `resolve_agent_name_from_env` | `env` | `construct` (×2) |
| `NO_EVENT_BACKEND_ERROR`, `YOLO_PERSIST_FALLBACK_ERROR` | `consts` | `events`, `yolo` |

Stay module-private (single-module callers): `report_config_warning`, `emit_permission_request_event`,
`notify_save_failure`, `open_settings_overlay`, `render_settings`, `config_path_line`, `on_off`,
`COMMAND_USAGE`, `from_parts_full`, `registered_tool_names`, `resolve_skill_read`,
`resolve_external_directory`, `GateCall`, `resolve_prompt_decision`, `forget_prompt_decision`,
`apply_decision`, `shape_agent_start_prompt`, `should_expose_tool`, `dir_has_entry`,
`PublishedRuntimeApi` (+ `gone`), `WarningSink`'s two fields.

Already `pub`/`pub(crate)` — **do not change**: `config_path_for`, `resolved_config_path_for`
(both `pub(crate)`, reached from `config_modal.rs`), `guard` (`pub(crate)`), `save_extension_config`,
`config_controller`, `yolo_mode`, `set_yolo_mode`, `toggle_yolo_mode`, `from_parts`, `into_shared`,
`with_agent_name`, `new*`, `is_installed`, `permission_extension_for_env`, every `pub const`.

### Step 4 — test split

`src/extension/tests/` — one `#![allow(clippy::unwrap_used, clippy::expect_used,
clippy::indexing_slicing)]` at the top of `tests/mod.rs` covers every submodule (a lint level on a
`mod` item propagates into its out-of-line body), so the inner attribute at line 2947 is written once,
not nine times. Each submodule opens with `use super::super::*;` — or, equivalently and more legibly,
`use crate::extension::*;` — which reaches the private items of `extension` because
`extension::tests::x` is a **descendant** of `extension`, plus `use super::support::*;`.

| File | Tests (by name) | ~lines |
| ---- | --------------- | ------ |
| `mod.rs` | module doc + `mod` declarations + the `#![allow]` | 40 |
| `support.rs` | `write_file`, `event_ctx`, `init_ext`, `with_config_env_lock`, `bash_call`, `without_install_env`, `before_agent_start`, `ui_ctx`, `headless_ctx`, `FakeRegistry`, `NotifyRecorder`, `LifecycleRecorder` — every fixture with callers in **two or more** groups, all `pub(super)` | 250 |
| `install.rs` | `not_installed_without_policy_or_env_returns_none`, `installed_when_policy_file_present`, `agent_markdown_frontmatter_alone_installs_the_gate`, `project_scoped_agent_markdown_also_installs_the_gate`, `the_policy_agent_dir_override_moves_both_the_probe_and_the_engine`, `a_blank_policy_agent_dir_override_is_not_an_override`, `auto_materialized_config_does_not_latch_the_gate_on`, `enabled_false_attaches_nothing_even_with_a_policy_file_present`, `a_legacy_three_key_config_template_still_reads_as_pristine`, `the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch`, `attaching_the_gate_loads_the_extension_config_exactly_once` + `with_config_path_override` (sole caller is here) | 380 |
| `config_reload.rs` | `resources_discover_reloads_config_and_invalidates_skill_cache`, `session_start_rebuilds_manager_from_current_session_cwd`, `before_agent_start_re_reads_config_json`, `a_resources_discover_reload_re_syncs_the_yolo_pill`, `reload_surfaces_write_lifecycle_reload_debug_entries`, `malformed_policy_and_config_files_notify_the_host`, `a_config_warning_re_arms_after_a_clean_load_clears_the_memo`, `the_env_locked_body_does_not_carry_the_guard_across_its_awaits` + `lifecycle_reload_entries` (sole caller is here) | 430 |
| `gate.rs` | `registry_gate_fails_closed_with_no_attached_registry`, `ask_fails_fast_without_ui_subagent_or_yolo`, `permission_decision_scope_trims_the_first_three_and_not_the_last_two` | 110 |
| `agent_start.rs` | `repeated_before_agent_start_applies_the_active_tool_set_once`, `a_mid_session_policy_edit_re_applies_the_shaped_tool_set` | 80 |
| `env.rs` | `subagent_env_hint_ors_any_non_empty_value_across_all_three_keys`, `subagent_env_hint_keys_match_the_spawn_overlay_contract` | 75 |
| `watcher.rs` | `parent_role_publishes_and_clears_the_process_parent_session_anchor`, `a_subagent_child_never_publishes_or_clears_the_parent_session_anchor`, `a_fresh_extension_holds_no_watcher_config_handles`, `repeated_hooks_yield_exactly_one_forwarding_watcher`, `a_later_hook_arms_the_watcher_a_headless_session_start_could_not`, `a_detaching_ui_tears_the_forwarding_watcher_down`, `the_running_watcher_shares_the_extensions_live_config` + `ANCHOR_REGISTER_LOCK`, `AnchorHost`, `WatcherHost`, `parent_ext` | 230 |
| `events.rs` | `init_publishes_the_yolo_control_surface_and_shutdown_retracts_it`, `a_gated_request_is_published_on_the_permission_request_channel` + `RecordingBus` | 110 |

### ⚠ The one way this split can break the suite

`cargo test` runs a crate's unit tests as **parallel threads in one process**. Several of these tests
mutate process env and are serialized by *shared statics*:

- `static ANCHOR_REGISTER_LOCK: tokio::sync::Mutex<()>` (line 4186) — taken at 4199, 4255 and inside
  `parent_ext` (4360). All four sites land in `tests/watcher.rs`, so the static moves there **once**.
- `crate::ext_config::env_lock()` — taken via `with_config_env_lock` (3195) and directly inside
  `without_install_env` (3346) and at 3087. These live in `support.rs`; the lock itself is already a
  crate-level static, so nothing changes.
- `crate::runtime_api::test_registry_lock()` (4575) — crate-level, unaffected.

**Declaring a second `ANCHOR_REGISTER_LOCK` (or a per-module copy of any lock static) silently
destroys the mutual exclusion and produces a flaky, env-race suite.** Every lock static must exist
exactly once and be shared by every caller. This is the single highest-risk step in the task.

The doc comment on
[`src/tests/mod.rs`](../../crates/cyrup-permission-system/src/tests/mod.rs) explains the same hazard
for the crate's other internal test tree and states the bar for what may live there — these tests do
**not** meet it (they mutate process env), so they stay under `extension/tests/` and must **not** be
relocated to `src/tests/`.

## Invariants (a violation means the refactor is wrong)

1. `src/lib.rs` is edited **only** if a re-export path breaks — the intent is that it needs **no**
   change at all. Its `pub use extension::{is_installed, permission_extension_for_env,
   PermissionSystemExtension, CHILD_ENV_VAR, EXTENSION_ID, INSTALL_ENV_VAR,
   PERMISSIONS_EXAMPLE_CONFIG, PERMISSIONS_JSON_SCHEMA, PERMISSION_REQUEST_EVENT_CHANNEL,
   PERMISSION_SYSTEM_COMMAND, POLICY_AGENT_DIR_ENV_KEY};` must keep compiling verbatim.
2. No other file in the crate changes. `logging.rs:47`, `status.rs:12`, `config_modal.rs:159-221`,
   `tests/shipped_artifacts.rs:19` all address `crate::extension::…` / `extension::…` paths that the
   `mod.rs` re-export block preserves.
3. No file outside the crate changes. `crates/cyrup/src/main.rs:728,950,1095`,
   `crates/cyrup-it/tests/permission/*`, `crates/cyrup-ext-subagents/*` consume only the `lib.rs`
   surface.
4. Not one behavioural line is edited: no reordering of statements, no changed match arms, no
   "while I'm here" fix. If a genuine bug is spotted, leave it and note it — a mixed diff makes the
   relocation unreviewable.
5. `git diff --stat` should be dominated by deletions from `extension.rs` and additions in
   `extension/*`; `git show --stat` line counts should net out to roughly zero plus the ~120 lines of
   new module headers, `mod` declarations and re-exports.

## Definition of done

```bash
cd /home/user/cyrup
cargo check -p cyrup-permission-system --all-targets   # clean, no new warnings
cargo clippy -p cyrup-permission-system --all-targets  # clean (workspace denies unwrap/expect/panic/indexing)
cargo test  -p cyrup-permission-system                 # same tests, same count, all green
cargo fmt -p cyrup-permission-system -- --check        # formatted
wc -l crates/cyrup-permission-system/src/extension/*.rs crates/cyrup-permission-system/src/extension/tests/*.rs
```

- `src/extension.rs` no longer exists; `src/extension/` holds 17 source modules + `tests/` with 9.
- Every file is under ~400 lines.
- The test count reported by `cargo test -p cyrup-permission-system` matches the pre-refactor count
  exactly (35 `#[test]`/`#[tokio::test]` attributes in the extension module) — a dropped test is the most likely silent failure of a split
  this size, so compare the counts, not just the pass/fail.
- `cargo check -p cyrup` still builds (the one workspace consumer of `permission_extension_for_env`).
