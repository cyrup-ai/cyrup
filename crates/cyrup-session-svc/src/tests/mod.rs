//! Crate-internal test modules (relocated from `tests/` so the whole crate's tests
//! build and run as ONE binary instead of one process per file).

// Crate-wide test policy, in one place instead of once per file: inside `tests/` a panic IS
// the failure report, so the four workspace denies that forbid it are lifted here. Lint levels
// on a module are lexically scoped over the out-of-line child modules it declares, so this one
// attribute covers every leaf below.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

// One `mod` list, alphabetically sorted — `grep '^mod ' mod.rs | sort -c` is the check. Keep it
// that way when adding a leaf; declaration order carries no meaning for out-of-line modules.
mod abort_settles;
mod added_tool_names_producer;
mod agent_settled;
mod agent_transcript_raw_seed;
mod attribution_follows_model;
mod base_system_prompt;
mod bash_session_env_wiring;
mod before_session_invalidate;
mod build_containment_and_flag_diagnostics;
mod cmdhint01_argument_hint;
/// Shared fixtures (`Fixture`, `fixture()`, `base_config()`) that the leaves below reach through
/// `use super::common::…`.
mod common;
mod compact_refusals;
mod compaction_tokens_after;
mod control_ops;
mod ctx_state_and_abort;
mod custom_tool_render;
mod delete_session_file_trash;
mod dispose_invalidates;
mod extension_input_event;
mod fork_non_persisted;
mod fork_parent_and_unsaved_guard;
mod get_commands_source_info;
mod host_services_core;
mod host_services_custom_seam;
mod host_services_introspection;
mod host_services_oauth;
mod host_services_session_view;
mod install_noop;
mod integration;
mod late_seams;
mod mid_run_tool_anchoring;
mod model_and_thinking_control;
mod modelless_launch;
mod native_host_services;
mod native_slash_command_output;
mod navigate_tree;
mod project_trust_extension;
mod read_image_auto_resize;
mod read_model_vision;
mod remote_catalog_overlay;
mod retry_and_postrun_loop;
mod session_branch_dir;
mod session_dag;
mod session_list_dir;
mod session_start_lifecycle;
mod session_stats_shape;
mod settings_resolve;
mod summarization_retry_events;
mod tool_usage_extension_seam;
mod transport_setting;
