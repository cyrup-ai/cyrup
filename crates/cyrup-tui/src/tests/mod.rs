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
//!
//! # Where a new test goes
//!
//! The crate keeps tests in three places, and the choice between them is forced by what the test
//! needs to reach — not by taste. The rule is already written down in the leaves; it is restated
//! here because the leaves are exactly where a reader adding a file does not look first.
//!
//! - **Inline `#[cfg(test)] mod tests` beside the code** — when the test needs private items or
//!   private fields, or must sit next to a process-global `static` and the lock that serializes it.
//!   Thirty-three production files do this. `src/app/backend.rs:230-234` states the case: the
//!   module's own tests reach members no sibling file can name.
//! - **`src/transcript/tests/`** — the transcript module's own private-access tests, seven files
//!   declared at `src/transcript/mod.rs:50-51`. `src/transcript/tests/mod.rs:1-3` states why they
//!   are a directory of the module rather than files here: they are inside `transcript`, so they
//!   see its private surface, which a test under `src/tests/` does not.
//! - **`src/tests/` (here)** — App-level tests that drive an `App<TestBackend>` through its public
//!   surface and assert on rendered output. Everything here can be written against `crate::`
//!   exports plus [`harness`]; if a test cannot, it belongs in one of the two locations above.
//!
//! [`harness`] carries the shared buffer scrapes ([`harness::buf_text`] and friends), the key-event
//! constructors, and the cross-file `caps_lock`. It also pins the trailing-newline convention that
//! the hand-rolled per-file copies had already broken — read its module doc before adding another
//! scrape helper.

mod alt_screen;
mod app_global_actions;
mod assembled_render;
mod auth_selector;
mod autocomplete;
mod bash_elapsed;
mod bash_live_run;
mod bash_overlay;
mod cell_size_query;
mod chrome;
mod clipboard;
mod color_mode_assembled;
mod command_exec;
mod commands;
mod compaction_status;
mod confirm_as_default_dispatch;
mod dialog_envelope_spacers;
mod diff;
mod edit_preview;
mod editor;
mod editor_fidelity;
mod editor_min_height;
mod editor_page_actions;
mod escalation;
mod escape_chain;
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
mod extension_theme_and_editor_readback;
mod extension_ui_effects;
mod extension_ui_reset_on_swap;
mod extension_working_indicator;
mod external_editor;
mod footer_chrome_fidelity;
mod footer_git_branch;
mod footer_subscription;
mod fork_selector;
mod fullscreen_scrollback;
mod fullscreen_settings;
pub(crate) mod harness;
mod image;
mod image_capabilities;
mod import_confirm;
mod inline_stacking;
mod input_pipeline;
mod input_prompt;
mod keybindings;
mod keymap;
mod live_floor;
mod login_flow;
mod markdown;
mod model_selector_assembled;
mod native_shift_enter;
mod package_update_notice;
mod pending_messages;
mod project_trust_banner;
mod reload_implicit_trust;
mod render;
mod render_cache_tick;
mod resize_viewport_failure;
mod rich_messages;
mod run_loop_cancel_bias;
mod run_loop_draw_coalescing;
mod run_loop_input_priority;
mod run_loop_swap_arm_reachable;
mod runtime_swap;
mod scoped_models;
mod selection_fidelity;
mod selector;
mod selector_cursor;
mod selector_wiring;
mod session_replay;
mod settings_inert_keys;
mod settings_trust_selectors;
mod share_url;
mod sigint_double_tap;
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
mod tool_render_shell;
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
