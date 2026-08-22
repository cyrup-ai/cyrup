---
stage: aug
status: done
updated: 2026-08-22 16:23
---

# Decompose cyrup-session-svc src/session.rs Into Submodules

## Description

`crates/cyrup-session-svc/src/session.rs` is **6297 lines** — the largest file in the workspace
(next largest in the same crate is `host_services.rs` at 3050, `builder.rs` at 2767). It holds one
`pub struct AgentSession` plus **205 methods** spread across five `impl AgentSession` blocks, two
`Drop` guards, two host-service adapter structs, and ~20 free helper functions.

Split it into `src/session/mod.rs` + one submodule per concern. **Pure mechanical move.** No logic
edits, no signature changes, no new abstractions.

## Why this is a low-risk refactor (the enabling Rust rule)

Rust privacy is **module-scoped, and an item is visible in its defining module *and all
descendants***. Therefore:

- The private fields of `AgentSession` (declared in `session/mod.rs`) are **readable from every
  `session/*.rs` child module** with zero visibility changes. No field needs `pub(crate)`.
- Inherent methods marked `pub` / `pub(crate)` stay reachable from anywhere that can name the type,
  **regardless of which module the `impl` block sits in**. So every existing public and
  `pub(crate)` method moves verbatim.
- Only **private (`fn` with no visibility) methods called from a *sibling* submodule** need
  widening — to `pub(super)`, which resolves to "visible in `session` and all its descendants",
  i.e. all siblings, and **not** to `crate::builder`/`crate::runtime`/etc. That preserves the exact
  current encapsulation. The complete list is enumerated below — it is 12 items.

## Current-state analysis

Baseline verified: `cargo check -p cyrup-session-svc --lib` → **exit 0** before any change.

### The file already documents its own concern boundaries

[`src/session.rs`](../../crates/cyrup-session-svc/src/session.rs) carries banner comments that
were clearly written as section dividers. These **are** the module boundaries — the decomposition
does not have to invent a taxonomy, only honour the one already in the file:

| Line | Banner |
| --- | --- |
| 688 | `// ---- subscriptions ----` |
| 695 | `// ---- prompting ----` |
| 1545 | `// ---- compaction ----` |
| 1933 | `// ---- fork / branch ----` |
| 2472 | `// ---- naming / export ----` |
| 2720 | `// ---- lifecycle ----` |
| 2973 | `// ---- model control ----` |
| 3565 | `// ---- thinking control ----` |
| 3695 | `// ---- steering / follow-up mode ----` |
| 3719 | `// ---- read access ----` |
| 4022 | `// ---- state views ----` |
| 4397 | `// ---- model cycling ----` |
| 4579 | `// ---- facade accessors ----` |
| 4646 | `// ==== retry subsystem ====` |
| 4785 | `// ==== auto-compaction subsystem ====` |
| 5344 | `// ==== immediate-bash seam ====` |
| 5620 | `// ==== dynamic tools ====` |

Three banners cover more than one concern and get split (see the module table):
`// ---- naming / export ----` (2472) also contains `slash_command_catalog` + the JSON/DAG
introspection getters; `// ---- model control ----` (2973) also contains the extension control-op
drain from 3319; `// ---- state views ----` (4022) also contains `fanout_emit`,
`spawn_event_pump` and the four message-injection methods.

### Structural inventory

| Span | Contents |
| --- | --- |
| 1–48 | module doc + `use` block |
| 49–77 | `WAIT_IDLE_CONTROL_TIMEOUT`, `ABORT_SETTLE_TIMEOUT`, `control_op_name` |
| 79–208 | public types: `ForkPosition`, `ForkOutcome`, `ForkAnchor`, `SessionDagKind`, `SessionDagNode`, `NavigateTreeOptions`, `NavigateTreeOutcome`, `ScopedModel`, `ModelCycleResult` |
| 210–269 | private: `InputDisposition`, `input_event_source`, `input_streaming_behavior`, `parse_model_ref`, `Prepared` |
| 271–323 | `pub(crate) struct SessionExtras`, `pub(crate) struct SessionHandle` + its impl |
| 325–490 | `pub struct AgentSession` (all fields, heavily documented) |
| 492–4644 | `impl AgentSession` — the 4100-line block, banner-divided |
| 4646–4783 | `impl AgentSession` — retry subsystem |
| 4785–5228 | `impl AgentSession` — auto-compaction subsystem |
| 5230–5290 | `impl Drop for AgentSession` (+ its 55-line rationale doc) |
| 5292–5342 | `struct CompactionCancelGuard` + impls |
| 5344–5618 | `struct BashCancelGuard` + `impl AgentSession` — immediate-bash seam |
| 5620–5775 | `impl AgentSession` — dynamic tools |
| 5776–5832 | `SessionActivityHandle`, `SessionCatalogHandle` + their `crate::host_services` trait impls |
| 5834–6297 | free helpers (`strip_frontmatter`, `agent_user_text`, `BeforeCompactOutcome`, compaction value/override/reason helpers, `BindOptions`, `DeleteMethod`, `delete_session_file_at`, `rename_session_file_at`, `trash_args`, `pi_hint_char_boundary`, `now_ms`, `same_file`, `flatten_dag_node`, `dag_display`, `join_text`, `user_message_text`, `custom_message_text`, `branch_summary_entry_of`, `user_message_anchor`, `branch_layout`, `fork_anchor`) |

### External surface that must not move

[`src/lib.rs:74-78`](../../crates/cyrup-session-svc/src/lib.rs) re-exports exactly:

```rust
pub use session::{
    AgentSession, BindOptions, DeleteMethod, ForkAnchor, ForkOutcome, ForkPosition, ModelCycleResult,
    NavigateTreeOptions, NavigateTreeOutcome, ScopedModel, SessionDagKind, SessionDagNode,
    delete_session_file_at, rename_session_file_at,
};
```

`lib.rs` is **not edited**. `session/mod.rs` must re-export all 14 names so this `use` keeps resolving.

Intra-crate paths that must also keep resolving (found by grepping `crate::session::` across the crate):

| Path | Named by |
| --- | --- |
| `crate::session::AgentSession` | [`builder.rs:42`](../../crates/cyrup-session-svc/src/builder.rs), [`factory.rs:21`](../../crates/cyrup-session-svc/src/factory.rs), [`runtime.rs:21`](../../crates/cyrup-session-svc/src/runtime.rs) |
| `crate::session::SessionHandle` | [`subscriber.rs:21`](../../crates/cyrup-session-svc/src/subscriber.rs), [`builder.rs:1460`](../../crates/cyrup-session-svc/src/builder.rs), [`hooks.rs:138,147`](../../crates/cyrup-session-svc/src/hooks.rs) |
| `crate::session::SessionExtras` | [`builder.rs:1713`](../../crates/cyrup-session-svc/src/builder.rs) |
| `crate::session::{ForkPosition}` | [`runtime.rs:21`](../../crates/cyrup-session-svc/src/runtime.rs) |
| `crate::session::{ForkAnchor, ForkOutcome, ForkPosition, NavigateTreeOptions, NavigateTreeOutcome}` | [`command.rs:17`](../../crates/cyrup-session-svc/src/command.rs) |
| `crate::session::trash_args` | [`tests/delete_session_file_trash.rs:34`](../../crates/cyrup-session-svc/src/tests/delete_session_file_trash.rs) |

### Layout convention in this workspace

`mod.rs` + `pub mod` children + `pub use` re-exports is the dominant style (30 `mod.rs` files vs 3
sibling-file modules). The closest precedent is
[`cyrup-session/src/compaction/mod.rs`](../../crates/cyrup-session/src/compaction/mod.rs): a doc
header naming each submodule's role, a `pub mod` list, then grouped `pub use` re-exports. **Follow
that shape**, except the children here are `mod` (private), not `pub mod` — the only names that
leave `session` are the 14 in the `lib.rs` list plus the `pub(crate)` items above.

## Required implementation

### Step 0 — move the file, preserving history

```bash
cd crates/cyrup-session-svc
mkdir -p src/session
git mv src/session.rs src/session/mod.rs
```

Then carve submodules out of `src/session/mod.rs`. Do **not** create the new files by retyping —
cut the exact line spans and paste them, so every doc comment (the Pi-parity commentary is the
crate's primary design record) survives byte-for-byte.

### Step 1 — the module table

Twenty submodules. Every row is a straight cut from the spans above.

| File | Moves in | Approx. lines |
| --- | --- | --- |
| `session/mod.rs` | module doc + `use` block; `SessionExtras`; `SessionHandle` + impl; `struct AgentSession` (all fields); the construction/core impl **492–694** (`from_parts`, `install_runtime_actions`, `shutdown_requested`, `into_shared`, `is_idle`, `is_run_active`, `lock`, `subscribe`); `fanout_emit` + `spawn_event_pump` (**4236–4275**); `impl Drop for AgentSession` + its doc (**5230–5290**); `now_ms`; `mod` list + re-exports | ~620 |
| `session/types.rs` | **79–208** public types + `BindOptions` (5919–5936) + `DeleteMethod` + impl (5938–5959) | ~210 |
| `session/run.rs` | **695–1039** and **1196–1544** minus the queue/abort tail: `prompt`, `prompt_accepted`, `prompt_with`, `spawn_run`, `drive_run`, `emit_agent_settled`, `handle_post_agent_run`, `on_user_message_start`, `on_assistant_message_end`, `prepare`, `emit_input_event`, `prepare_and_assemble`, `expand_input_text`, `expand_skill_command`, `last_assistant_message`, `assemble_run_messages`, `wait_for_idle`, `steer`, `follow_up`, `throw_if_extension_command`; private `InputDisposition`, `Prepared`, `input_event_source`, `input_streaming_behavior`, `strip_frontmatter`, `agent_user_text` | ~620 |
| `session/commands.rs` | **1040–1195** (`try_execute_extension_command`, `surface_command_outcome`, both `#[cfg]` arms of `try_execute_wasm_command`) + `slash_command_catalog` (**2562–2676**) | ~275 |
| `session/queue.rs` | **1452–1544** (`steering_messages`, `follow_up_messages`, `pending_message_count`, `clear_queue`, `drain_queue`, `emit_queue_update`, `abort`, `abort_and_settle`) + **3695–3718** (`steering_mode`, `follow_up_mode`, `set_steering_mode`, `set_follow_up_mode`) + the `ABORT_SETTLE_TIMEOUT` const (**58**, used only by `abort_and_settle` at 1542, and named by its intra-doc link) | ~130 |
| `session/compaction.rs` | **1545–1932** (`compact`, `emit_before_compact`, `abort_compaction`, `abort_branch_summary`) + `CompactionCancelGuard` (**5292–5342**) + `BeforeCompactOutcome`, `compaction_preparation_value`, `parse_compaction_override`, `compaction_reason_str` (**5866–5922**) | ~500 |
| `session/auto_compaction.rs` | **4785–5228** (`is_compacting`, `auto_compaction_enabled`, `set_auto_compaction_enabled`, `check_compaction`, `latest_compaction_ts`, `run_auto_compaction`, `effective_compaction_settings`) | ~445 |
| `session/retry.rs` | **4646–4783** whole banner block incl. `drop_trailing_assistant` | ~140 |
| `session/forking.rs` | **1933–2471** (`branch`, `branch_with_summary`, `navigate_tree`, `generate_branch_summary_with_instructions`, `republish_session_identity`, `fork`, `clone_at`, `fork_at_entry`, `user_messages_for_forking`) + `fork_anchor_live`, `branch_live_manager` (**2834–2883**) + `branch_summary_entry_of`, `user_message_anchor`, `branch_layout`, `fork_anchor` (**6228–6297**) | ~660 |
| `session/model.rs` | **2973–3318** (`set_model`, `set_model_resolved`, `install_owning_provider`, `has_configured_auth`, `provider_has_configured_auth`, `recheck_provider_auth`, `provider_is_oauth_backed`, `full_model_catalog`, `full_model_registry`, `available_model_catalog`, `attribution_headers`, `headers_for_model_ref`, `emit_model_select`, `set_model_id`) + **4397–4578** (`scoped_models`, `set_scoped_models`, `cycle_model`, `cycle_scoped_model`, `cycle_available_model`, `apply_model_change`) | ~530 |
| `session/thinking.rs` | **3565–3694** (`thinking_level`, `available_thinking_levels`, `supports_thinking`, `set_thinking_level`, `set_transport`, `cycle_thinking_level`) | ~130 |
| `session/control.rs` | **3319–3564** (`apply_pending_control`, `control_navigate`, `control_send_message`, `report_control_failure`, `apply_agent_state_op`, `apply_pending_agent_control`) + `control_op_name` (**61–77**), `parse_model_ref` (sole call site 3506) and the `WAIT_IDLE_CONTROL_TIMEOUT` const (**50**, sole call site 3373) | ~270 |
| `session/lifecycle.rs` | **2720–2833** and **2884–2972**: `dispose`, `dispose_with`, `take_manager`, `notify_replaced`, `bind_extensions`, `bind_extensions_with`, `emit_session_start` | ~200 |
| `session/transcript.rs` | **2472–2561** (`session_name`, `set_session_name`, `export_to_jsonl`, `export_to_html`) + **2677–2719** (`entries_json`, `tree_json`, `session_dag`) + DAG/text helpers `flatten_dag_node`, `dag_display`, `join_text`, `user_message_text`, `custom_message_text` (**6080–6227**) | ~290 |
| `session/accessors.rs` | **3719–3835** and **3912–4021** (`model`, `model_fallback_message`, `is_streaming`, `session_id`, `session_header`, `claim_json_header`, `session_file`, `services`, `extension_flag_values`, `trust_store_path`, `project_trust_options`, `saved_trust_decision`, `write_project_trust`, `persist_setting`, `system_prompt`, `base_system_prompt`, `system_prompt_override`, `effective_system_prompt`, `current_system_prompt`, `context`, `messages`, `raw_context_messages`, `leaf_id`, `read_model_vision`, `agent_headers`, `agent_messages`, `last_assistant_text`) + **4579–4643** facade accessors (`prompt_templates`, `model_catalog`, `resources`, `ext_host`, `has_extension_handlers`, `load_wasm_extension`) | ~300 |
| `session/files.rs` | **3836–3911** (`sessions_root`, `session_dir`, `list_sessions`, `delete_session_file`, `rename_session_file`, `manager_path`) + **5960–6079** (`delete_session_file_at`, `rename_session_file_at`, `trash_args`, `pi_hint_char_boundary`, `same_file`) | ~200 |
| `session/stats.rs` | **4022–4235** (`session_stats`, `usage_cost_breakdown`, `cache_waste`, `stats_context_usage`, `has_post_compaction_usage`, `context_usage`, `state_view`) | ~215 |
| `session/inject.rs` | **4276–4396** (`append_custom_message`, `send_user_message`, `send_custom_message`, `inject_message`) | ~120 |
| `session/bash.rs` | **5344–5618** — `BashCancelGuard` + `execute_bash`, `execute_bash_with_user_event`, `emit_user_bash_event`, `record_bash_result`, `next_bash_cancel_id`, `abort_bash`, `is_bash_running`, `has_pending_bash_messages`, `flush_pending_bash_messages`, `append_bash_message` | ~275 |
| `session/tools.rs` | **5620–5775** — `active_tool_names`, `all_tools`, `tool_definition`, `push_active_tools`, `refresh_extension_tools`, `next_turn_tools`, `next_turn_model_baseline`, `set_active_tools_by_name`, `register_custom_tools` | ~156 |
| `session/adapters.rs` | **5776–5832** — `SessionActivityHandle`, `SessionCatalogHandle` and their `crate::host_services::{SessionActivity, SessionCatalog}` impls | ~57 |

`session/bash.rs` and `session/tools.rs` do **not** collide with the crate-level
`src/bash.rs` / `src/tools.rs`: those are named absolutely as `crate::bash::…` / `crate::tools::…`
inside the moved code, which resolves past the sibling module. Keep the existing `use
crate::bash::{…}` / `use crate::tools::{…}` lines verbatim.

### Step 2 — the twelve visibility widenings (the ONLY code edits)

Every one of these is a private `fn` whose call sites land in a different submodule after the
split. Change **only** the visibility keyword — never the name, signature, body or doc.

| Item | Now | Becomes | Lives in | Called from |
| --- | --- | --- | --- | --- |
| `lock` | `fn` | `pub(super) fn` | `mod.rs` | everywhere (~100 sites) |
| `fanout_emit` | `async fn` | `pub(super) async fn` | `mod.rs` | run, queue, compaction, auto_compaction, forking, transcript, lifecycle, control, thinking, inject |
| `spawn_event_pump` | `fn` | `pub(super) fn` | `mod.rs` | compaction 1603, forking 1973/2304, auto_compaction 4970, bash 5445 |
| `now_ms` | `fn` | `pub(super) fn` | `mod.rs` | control 3465, inject 4321/4377, bash 5555 |
| `spawn_run` | `async fn` | `pub(super) async fn` | `run.rs` | control 3467, inject 4384 |
| `emit_queue_update` | `async fn` | `pub(super) async fn` | `queue.rs` | run 898 |
| `emit_before_compact` | `async fn` | `pub(super) async fn` | `compaction.rs` | auto_compaction 5004 |
| `compaction_reason_str` | `fn` | `pub(super) fn` | `compaction.rs` | auto_compaction 5127 |
| `drop_trailing_assistant` | `async fn` | `pub(super) async fn` | `retry.rs` | auto_compaction 4868 |
| `push_active_tools` | `async fn` | `pub(super) async fn` | `tools.rs` | control 3340, 3551 |
| `user_message_text` | `fn` | `pub(super) fn` | `transcript.rs` | forking 2058, 2060, 2468 |
| `custom_message_text` | `fn` | `pub(super) fn` | `transcript.rs` | forking 2063 |

Everything else keeps its exact current visibility:

- `pub(crate)` methods — `from_parts`, `emit_agent_settled`, `on_user_message_start`,
  `on_assistant_message_end`, `surface_command_outcome`, `fork_anchor_live`,
  `branch_live_manager`, `take_manager`, `refresh_extension_tools`, `next_turn_tools`,
  `next_turn_model_baseline` — stay `pub(crate)`. Inherent-method visibility does not depend on the
  module the `impl` sits in, so no re-export is needed for any of them.
- Private helpers whose call sites land in the **same** submodule stay private:
  `strip_frontmatter`, `agent_user_text`, `input_event_source`, `input_streaming_behavior`,
  `control_op_name`, `parse_model_ref`, `manager_path`, `same_file`, `pi_hint_char_boundary`,
  `effective_compaction_settings`, `latest_compaction_ts`, `install_owning_provider`,
  `emit_model_select`, `apply_model_change`, `branch_summary_entry_of`, `user_message_anchor`,
  `flatten_dag_node`, `dag_display`, `join_text`, `has_post_compaction_usage`,
  `emit_user_bash_event`, `next_bash_cancel_id`, `append_bash_message`, `report_control_failure`,
  `control_navigate`, `control_send_message`, `apply_agent_state_op`, `apply_pending_agent_control`,
  `handle_post_agent_run`, `drive_run`, `prepare`, `emit_input_event`, `prepare_and_assemble`,
  `expand_input_text`, `expand_skill_command`, `last_assistant_message`, `assemble_run_messages`,
  `throw_if_extension_command`, `try_execute_extension_command`, `try_execute_wasm_command`,
  `republish_session_identity`, `generate_branch_summary_with_instructions`,
  `full_model_registry`, `headers_for_model_ref`, `provider_has_configured_auth`,
  `recheck_provider_auth`, `provider_is_oauth_backed`, `cycle_scoped_model`,
  `cycle_available_model`, `run_auto_compaction`.
- `pub(crate) fn trash_args`, `pub(crate) fn branch_layout`, `pub(crate) fn fork_anchor` keep
  `pub(crate)` **and** get a `pub(crate) use` re-export in `mod.rs` (see below) — `trash_args`
  because a test names `crate::session::trash_args`, the other two for path stability.

### Step 3 — `session/mod.rs` skeleton

Keep the existing 6-line module doc at the top, then extend it with the submodule roster (the
[`cyrup-session/src/compaction/mod.rs`](../../crates/cyrup-session/src/compaction/mod.rs) pattern).
The `mod` declarations are **private**; only the re-exports leave the module.

```rust
//! `AgentSession` — the single integration seam every front-end consumes (func-11 R-11-023).
//!
//! … (existing 6-line doc, unchanged) …
//!
//! ## Layout
//! The seam is one struct with ~205 methods; the `impl` blocks are split by concern:
//! [`run`] (prompt/dispatch/post-run driver), [`commands`] (slash + wasm command execution),
//! [`queue`] (steering/follow-up + abort), [`compaction`] / [`auto_compaction`] / [`retry`],
//! [`forking`] (branch/fork/tree), [`model`] / [`thinking`] (model + reasoning control),
//! [`control`] (the extension control-op drain), [`lifecycle`] (dispose/bind/announce),
//! [`transcript`] (naming/export/JSON+DAG views), [`accessors`], [`files`], [`stats`],
//! [`inject`], [`bash`], [`tools`], [`adapters`], [`types`].
//! The struct, its fields, construction and the shared primitives (`lock`, `fanout_emit`,
//! `spawn_event_pump`, `now_ms`, `Drop`) stay here.

mod accessors;
mod adapters;
mod auto_compaction;
mod bash;
mod commands;
mod compaction;
mod control;
mod files;
mod forking;
mod inject;
mod lifecycle;
mod model;
mod queue;
mod retry;
mod run;
mod stats;
mod thinking;
mod tools;
mod transcript;
mod types;

// The public seam surface `crate::lib` re-exports (lib.rs:74-78) — unchanged names, so that
// `pub use session::{…}` keeps resolving without touching lib.rs.
pub use types::{
    BindOptions, DeleteMethod, ForkAnchor, ForkOutcome, ForkPosition, ModelCycleResult,
    NavigateTreeOptions, NavigateTreeOutcome, ScopedModel, SessionDagKind, SessionDagNode,
};
pub use files::{delete_session_file_at, rename_session_file_at};

// Crate-internal paths other modules (and one test) name through `crate::session::…`.
pub(crate) use files::trash_args;
pub(crate) use forking::{branch_layout, fork_anchor};

use …; // the existing `use` block, trimmed to what mod.rs itself still references

pub(crate) struct SessionExtras { /* … verbatim … */ }
pub(crate) struct SessionHandle { /* … verbatim … */ }
impl SessionHandle { /* … verbatim … */ }

/// The integration seam (arch-11 §3.1). Cheaply shareable via `Arc`; every method is `&self`.
pub struct AgentSession { /* … all fields verbatim, still private … */ }

impl AgentSession {
    // construction + core: from_parts, install_runtime_actions, shutdown_requested,
    // into_shared, is_idle, is_run_active, subscribe — verbatim.

    /// Lock a `std::sync::Mutex` ignoring poisoning (no panic; arch-00 no-panic).
    pub(super) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) async fn fanout_emit(&self, ev: AgentSessionEvent) { /* verbatim */ }
    pub(super) fn spawn_event_pump(&self, /* … */) -> tokio::task::JoinHandle<()> { /* verbatim */ }
}

impl Drop for AgentSession { /* … verbatim, doc included … */ }

pub(super) fn now_ms() -> i64 { /* verbatim */ }
```

### Step 4 — every submodule follows this shape

```rust
//! <one-line concern statement, in the voice of the banner comment it replaces>
//!
//! <the banner's Pi-reference line, e.g. "Pi `agent-session.ts:2582-2684`. The out-of-loop
//!  bash RPC path.">

use std::sync::Arc;

use cyrup_core::…;                 // only what THIS file needs
use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::AgentSession;           // plus any sibling item this file calls
use super::types::ForkPosition;

impl AgentSession {
    // … the moved methods, verbatim …
}
```

Rules for the `use` blocks — this is where a mechanical split usually goes wrong:

1. Do **not** copy the 40-line header `use` block into every file. Give each submodule only the
   imports its own body resolves, then let `cargo check` name the leftovers.
2. Unused imports left in `mod.rs` will surface as `unused_imports` warnings — remove them from
   `mod.rs` as they appear; that is the one place where deleting a line is expected.
3. Sibling items are reached as `super::<module>::<item>` or via a `use super::…;` at the top —
   whichever keeps the call site byte-identical to what it is today (prefer the `use`, so the
   moved bodies need no edits at all).
4. Preserve `#[cfg(feature = "wasm-host")]` / `#[cfg(not(feature = "wasm-host"))]` on both
   `try_execute_wasm_command` arms in `commands.rs`, and on `load_wasm_extension` in
   `accessors.rs`. Both feature arms must still compile.

### Step 5 — what must NOT change

- No method renamed, no signature altered, no body edited, no `#[allow]` added or removed.
- No doc comment reworded, reflowed or dropped. The Pi-parity commentary (`[CYRUP-DELTA]`, the
  `DRIFT-0xx` / `SEAM-0xx` references, the `Drop` and `CompactionCancelGuard` rationale essays) is
  the crate's design record — it moves with its item, unchanged.
- No struct field visibility widened.
- `src/lib.rs` is not touched.
- Do **not** run `cargo fmt` over the crate — there is no `rustfmt.toml`, so a repo-wide format
  would rewrite lines the refactor never touched and bury the move in noise. Match the surrounding
  style by hand where a `use` block is new.

## Definition of done

- [ ] `crates/cyrup-session-svc/src/session.rs` no longer exists; `src/session/mod.rs` plus the 20
      submodules above do, created via `git mv` so history follows.
- [ ] No file under `src/session/` exceeds ~700 lines.
- [ ] `cargo check -p cyrup-session-svc --lib` passes (baseline was exit 0).
- [ ] `cargo check -p cyrup-session-svc --lib --no-default-features` passes — proves the
      `#[cfg(not(feature = "wasm-host"))]` arm still compiles after the `commands.rs` split.
- [ ] `cargo check -p cyrup-session-svc --all-targets` passes — proves `crate::session::trash_args`
      still resolves for `src/tests/delete_session_file_trash.rs`.
- [ ] `cargo clippy -p cyrup-session-svc --all-targets` reports no new warnings (workspace denies
      `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`).
- [ ] `git diff -M --stat` shows the change as renames + moves: the only non-move hunks are the 12
      visibility keywords, the new per-file `use` headers, the new module docs, and the `mod` +
      re-export block in `mod.rs`.
