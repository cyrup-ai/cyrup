//! Unit tests for `cyrup-tui`.
//!
//! These were previously one integration-test **binary each** under `crates/cyrup-tui/tests/`.
//! Cargo compiles and links every file there as its own process, which dominated the workspace
//! test wall-clock; the assertions are unchanged, only their compilation unit moved. Declared from
//! `lib.rs` as `#[cfg(test)] mod tests;`, so the gate lives there and these files carry none.
//!
//! Two files stay under `tests/` because they need process isolation, not just a library:
//! `wasm_renderer_screen.rs` (a nested `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`
//! producing a component the host loads) and `experimental_marker.rs` (it mutates the process
//! environment with `std::env::set_var`, which this crate's `#![forbid(unsafe_code)]` rules out
//! in `src/` — and whose soundness argument depends on being a one-test binary).

mod app_global_actions;
mod assembled_render;
mod auth_selector;
mod autocomplete;
mod bash_elapsed;
mod bash_overlay;
mod cell_size_query;
mod chrome;
mod clipboard;
mod color_mode_assembled;
mod command_exec;
mod commands;
mod compaction_status;
mod dialog_envelope_spacers;
mod diff;
mod edit_preview;
mod editor;
mod escape_chain;
mod editor_fidelity;
mod editor_min_height;
mod editor_page_actions;
mod escape_restores_queue;
mod export;
mod extension_dialog_countdown;
mod extension_dialog_wrapping;
mod extension_editor_dialog;
mod extension_error_surfacing;
mod extension_renderers;
mod extension_select_empty_options;
mod extension_shortcut;
mod extension_shutdown;
mod extension_ui_effects;
mod extension_ui_reset_on_swap;
mod footer_chrome_fidelity;
mod footer_git_branch;
mod footer_subscription;
mod fork_selector;
mod image;
mod image_capabilities;
mod inline_stacking;
mod input_prompt;
mod keybindings;
mod keymap;
mod login_flow;
mod markdown;
mod model_selector_assembled;
mod native_shift_enter;
mod package_update_notice;
mod pending_messages;
mod project_trust_banner;
mod render;
mod rich_messages;
mod run_loop_cancel_bias;
mod runtime_swap;
mod scoped_models;
mod selection_fidelity;
mod selector;
mod selector_cursor;
mod selector_wiring;
mod session_replay;
mod settings_inert_keys;
mod settings_trust_selectors;
mod startup_resources_panel;
mod status_indicator;
mod stop_reason;
mod terminal_progress;
mod terminal_theme_query;
mod terminal_title;
mod theme_controller_assembled;
mod theme_fidelity;
mod thinking;
mod tool_render;
mod tool_result_images;
mod tool_result_sanitize;
mod tool_result_usage_totals;
mod transcript_expand_wiring;
mod transport_live_apply;
mod tree_and_chrome;
mod tree_branch_summary;
mod tree_label_timestamp;
mod tree_selector;
mod turn_interleaving;
