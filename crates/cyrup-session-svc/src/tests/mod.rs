//! Crate-internal test modules (relocated from `tests/` so the whole crate's tests
//! build and run as ONE binary instead of one process per file).

mod abort_settles;
mod added_tool_names_producer;
mod agent_settled;
mod agent_settled_deferral;
mod agent_transcript_raw_seed;
mod attribution_follows_model;
mod base_system_prompt;
mod bash_only_skills;
mod bash_session_env_wiring;
mod before_provider_headers_emitter;
mod before_session_invalidate;
mod build_containment_and_flag_diagnostics;
mod cmdhint01_argument_hint;
mod codemode;
mod compact_refusals;
mod compaction_model_overrides;
mod compaction_tokens_after;
mod context_usage_abort;
mod context_usage_branch;
mod control_ops;
mod ctx_state_and_abort;
mod custom_tool_render;
mod delete_session_file_trash;
mod dispose_invalidates;
mod export_branch_jsonl;
mod export_headless_theme;
mod export_html;
mod ext_077_user_bash_fails_closed;
mod ext_083_send_user_message_options;
mod ext_087_send_from_event;
mod fork_non_persisted;
mod fork_parent_and_unsaved_guard;
mod get_commands_source_info;
mod host_flag_value;
mod inject_message_details;
mod inject_message_display;
mod inject_pump_redrain;
mod install_noop;
mod integration;
mod late_seams;
mod live_provider_auth;
mod live_provider_host_wiring;
mod live_providers;
mod mid_run_tool_anchoring;
mod model_refresh_reaches_live_providers;
mod model_runtime_snapshot;
mod modelless_launch;
mod native_host_services;
mod native_slash_command_output;
mod nested_tool_calls;
mod nested_tool_context;
mod post_login_catalog_refresh;
mod production_provider_wiring;
mod project_trust_extension;
mod prompt_transcript;
mod provider_refresh;
mod read_image_auto_resize;
mod read_model_vision;
mod remote_catalog_overlay;
mod replay_custom_entries;
mod round10_medium;
mod round2;
mod round3;
mod round4;
mod round5;
mod round6;
mod round7;
mod round8_postrun;
mod round9_l5res;
mod same_path_mutation_order;
mod session_branch_dir;
mod session_dag;
mod session_list_dir;
mod session_start_lifecycle;
mod session_stats_shape;
mod settings_resolve;
mod startup_timings;
mod summarization_retry_events;
mod thinking_level_on_model_switch;
mod tool_exposure;
mod tool_result_structured_content;
mod tool_search;
mod tool_transcript;
mod tool_usage_extension_seam;
mod transport_setting;
mod tree_branch_summary_cap;
mod turn_end_steer;
mod type_driven_boundaries;
mod ui_prompt_events;
mod virtual_model_extension;
mod virtual_model_limits;
mod virtual_model_restore;
mod virtual_model_routing;

/// The system prompt a request carries, as the provider renders it: the replay of the transcript's
/// system messages (`getCurrentSystemPrompt` over `normalizeContext`). The agent holds no prompt of
/// its own since CODE-014, so the request's `Context::system_prompt` field says nothing; the prompt
/// is the `sections` of the system rows.
pub(crate) fn rendered_prompt(ctx: &cyrup_provider::Context) -> String {
    cyrup_provider::get_current_system_prompt(cyrup_provider::normalize_context(ctx).messages())
}
