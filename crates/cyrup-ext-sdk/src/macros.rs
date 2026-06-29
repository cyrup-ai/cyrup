//! The `export_extension!` macro (arch-08 §2.2/§3.6; Pi's "the author's module IS the extension"
//! pattern). An external author depends on this crate, vendors the shared `wit/` dir, and writes:
//!
//! ```ignore
//! use cyrup_ext_sdk::prelude::*;
//!
//! fn build() -> ExtensionApi {
//!     let mut api = ExtensionApi::new();
//!     api.on_tool_call(|ev, _ctx| if ev.name == "bash" { Outcome::block("no") } else { Outcome::noop() });
//!     api
//! }
//!
//! cyrup_ext_sdk::export_extension!(build);
//! ```
//!
//! `cargo build --target wasm32-wasip2` then yields a loadable `cyrup:ext` COMPONENT. The macro emits
//! the wasm guest glue — the world's `init` + `events` (all 30 hooks + `execute-tool` +
//! `execute-command`/`get-argument-completions` + `render-call`/`render-result`) exports + the
//! `export!` invocation — each delegating to the routing helpers in [`crate::guest`]. The
//! `wit_bindgen::generate!` (with `pub_export_macro`) runs once in this crate; the downstream author's
//! cdylib reaches `bindings::export!` by path, so a one-line invocation produces a working component.
//!
//! This crate's own bundled [`crate::example`] is exported through this same macro (in `lib.rs`),
//! so the live end-to-end test exercises the macro-generated glue — not a hand-written copy.

/// Emit the wasm32 guest exports for an extension whose factory is `$factory` (a `fn() ->
/// ExtensionApi`). No-op on non-wasm targets so the author's crate still builds/tests on the host.
#[macro_export]
macro_rules! export_extension {
    ($factory:path) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            use $crate::guest::bindings;

            struct __CyrupExtComponent;

            impl bindings::Guest for __CyrupExtComponent {
                fn init() -> ::core::result::Result<(), ::std::string::String> {
                    $crate::guest::run_init($factory)
                }
            }

            impl bindings::exports::cyrup::ext::events::Guest for __CyrupExtComponent {
                // --- guest tool / command / renderer execution ---
                fn execute_tool(
                    name: ::std::string::String,
                    call_id: ::std::string::String,
                    params_json: ::std::string::String,
                ) -> ::core::result::Result<bindings::cyrup::ext::types::ToolOutput, ::std::string::String>
                {
                    $crate::guest::run_tool(name, call_id, params_json)
                }
                fn execute_command(
                    name: ::std::string::String,
                    args: ::std::string::String,
                ) -> ::core::result::Result<::core::option::Option<::std::string::String>, ::std::string::String>
                {
                    $crate::guest::run_command(name, args)
                }
                fn get_argument_completions(
                    name: ::std::string::String,
                    prefix: ::std::string::String,
                ) -> ::std::vec::Vec<::std::string::String> {
                    $crate::guest::completions(name, prefix)
                }
                fn render_call(
                    custom_type: ::std::string::String,
                    call_json: ::std::string::String,
                ) -> ::core::option::Option<::std::string::String> {
                    $crate::guest::render_call(custom_type, call_json)
                }
                fn render_result(
                    custom_type: ::std::string::String,
                    result_json: ::std::string::String,
                ) -> ::core::option::Option<::std::string::String> {
                    $crate::guest::render_result(custom_type, result_json)
                }

                // --- provider OAuth + streamSimple + autocomplete stacking ---
                fn provider_login(
                    id: ::std::string::String,
                ) -> ::core::result::Result<::std::string::String, ::std::string::String> {
                    $crate::guest::provider_login(id)
                }
                fn provider_refresh_token(
                    id: ::std::string::String,
                    credentials_json: ::std::string::String,
                ) -> ::core::result::Result<::std::string::String, ::std::string::String> {
                    $crate::guest::provider_refresh_token(id, credentials_json)
                }
                fn provider_get_api_key(
                    id: ::std::string::String,
                    credentials_json: ::std::string::String,
                ) -> ::core::result::Result<::std::string::String, ::std::string::String> {
                    $crate::guest::provider_get_api_key(id, credentials_json)
                }
                fn provider_modify_models(
                    id: ::std::string::String,
                    models_json: ::std::string::String,
                    credentials_json: ::std::string::String,
                ) -> ::core::result::Result<::std::string::String, ::std::string::String> {
                    $crate::guest::provider_modify_models(id, models_json, credentials_json)
                }
                fn provider_stream_simple(
                    id: ::std::string::String,
                    stream_id: ::std::string::String,
                    model_json: ::std::string::String,
                    context_json: ::std::string::String,
                    options_json: ::std::string::String,
                ) -> ::core::result::Result<(), ::std::string::String> {
                    $crate::guest::provider_stream_simple(
                        id, stream_id, model_json, context_json, options_json,
                    )
                }
                fn autocomplete_suggest(
                    base_json: ::std::string::String,
                    query_json: ::std::string::String,
                ) -> ::std::string::String {
                    $crate::guest::autocomplete_suggest(base_json, query_json)
                }

                // --- block/mutate/handled hooks ---
                fn on_tool_call(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    input_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(0, &[&call_id, &name, &input_json])
                }
                fn on_tool_result(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    content_json: ::std::string::String,
                    is_error: bool,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(1, &[&call_id, &name, &content_json, $crate::guest::b(is_error)])
                }
                fn on_context(messages_json: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(2, &[&messages_json])
                }
                fn on_message_end(message_json: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(3, &[&message_json])
                }
                fn on_before_agent_start(
                    prompt: ::std::string::String,
                    images_json: ::std::string::String,
                    system_prompt: ::std::string::String,
                    options_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(4, &[&prompt, &images_json, &system_prompt, &options_json])
                }
                fn on_input(text: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(18, &[&text])
                }
                fn on_user_bash(
                    command: ::std::string::String,
                    operations_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(19, &[&command, &operations_json])
                }
                fn on_before_provider_request(payload_json: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(20, &[&payload_json])
                }
                fn on_resources_discover() -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(5, &[])
                }
                fn on_project_trust() -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(6, &[])
                }
                fn on_session_before_switch(target_id: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(24, &[&target_id])
                }
                fn on_session_before_fork(entry_id: ::std::string::String) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(25, &[&entry_id])
                }
                fn on_session_before_compact() -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(26, &[])
                }
                fn on_session_before_tree() -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(28, &[])
                }

                // --- notify-only hooks ---
                fn on_agent_start() {
                    $crate::guest::notify(7, &[]);
                }
                fn on_agent_end(messages_json: ::std::string::String) {
                    $crate::guest::notify(8, &[&messages_json]);
                }
                fn on_turn_start(turn_index: u32) {
                    $crate::guest::notify(9, &[&turn_index.to_string()]);
                }
                fn on_turn_end(turn_index: u32, message_json: ::std::string::String) {
                    $crate::guest::notify(10, &[&turn_index.to_string(), &message_json]);
                }
                fn on_message_start(role: ::std::string::String) {
                    $crate::guest::notify(11, &[&role]);
                }
                fn on_message_update(delta_json: ::std::string::String) {
                    $crate::guest::notify(12, &[&delta_json]);
                }
                fn on_tool_exec_start(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    args_json: ::std::string::String,
                ) {
                    $crate::guest::notify(13, &[&call_id, &name, &args_json]);
                }
                fn on_tool_exec_update(call_id: ::std::string::String, chunk_json: ::std::string::String) {
                    $crate::guest::notify(14, &[&call_id, &chunk_json]);
                }
                fn on_tool_exec_end(
                    call_id: ::std::string::String,
                    result_json: ::std::string::String,
                    is_error: bool,
                ) {
                    $crate::guest::notify(15, &[&call_id, &result_json, $crate::guest::b(is_error)]);
                }
                fn on_session_start(reason: ::std::string::String) {
                    $crate::guest::notify(16, &[&reason]);
                }
                fn on_session_shutdown(reason: ::std::string::String) {
                    $crate::guest::notify(17, &[&reason]);
                }
                fn on_after_provider_response(status: u32, headers_json: ::std::string::String) {
                    $crate::guest::notify(21, &[&status.to_string(), &headers_json]);
                }
                fn on_model_select(model_json: ::std::string::String) {
                    $crate::guest::notify(22, &[&model_json]);
                }
                fn on_thinking_level_select(level: ::std::string::String) {
                    $crate::guest::notify(23, &[&level]);
                }
                fn on_session_compact(summary: ::std::string::String) {
                    $crate::guest::notify(27, &[&summary]);
                }
                fn on_session_tree(tree_json: ::std::string::String) {
                    $crate::guest::notify(29, &[&tree_json]);
                }
            }

            bindings::export!(__CyrupExtComponent with_types_in bindings);
        };
    };
}
