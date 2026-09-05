//! The [`export_extension!`](crate::export_extension) macro (arch-08 §2.2/§3.6; Pi's "the author's
//! module IS the extension" pattern). This module is that macro's authoring guide and nothing else:
//! the macro is `#[macro_export]`ed, so it is documented at the CRATE ROOT and this page lists no
//! items of its own. An external author depends on this crate, vendors the shared `wit/` dir, and
//! writes:
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
//! the wasm guest glue — the world's `init` + `events` (all 33 hooks + `execute-tool` +
//! `execute-command`/`get-argument-completions` + `render-call`/`render-result`) exports + the
//! `export!` invocation — each delegating to the routing helpers in `crate::guest`. The
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
                ) -> ::core::result::Result<
                    bindings::cyrup::ext::types::ToolOutput,
                    ::std::string::String,
                > {
                    $crate::guest::run_tool(name, call_id, params_json)
                }
                fn execute_command(
                    name: ::std::string::String,
                    args: ::std::string::String,
                ) -> ::core::result::Result<
                    ::core::option::Option<::std::string::String>,
                    ::std::string::String,
                > {
                    $crate::guest::run_command(name, args)
                }
                fn get_argument_completions(
                    name: ::std::string::String,
                    prefix: ::std::string::String,
                ) -> ::std::vec::Vec<::std::string::String> {
                    $crate::guest::completions(name, prefix)
                }
                // EXT-023 / TOOL-022 — `prepareArguments` (pi `ToolDefinition.prepareArguments?`,
                // extensions/types.ts:468 @v0.83.0). Called ONLY for a descriptor that set the
                // `prepare-arguments` flag, and only BEFORE schema validation.
                fn prepare_arguments(
                    name: ::std::string::String,
                    args_json: ::std::string::String,
                ) -> ::core::option::Option<::std::string::String> {
                    $crate::guest::prepare_arguments(name, args_json)
                }
                fn execute_shortcut(
                    key: ::std::string::String,
                ) -> ::core::result::Result<(), ::std::string::String> {
                    $crate::guest::run_shortcut(key)
                }
                fn render_call(
                    custom_type: ::std::string::String,
                    call_json: ::std::string::String,
                    opts_json: ::std::string::String,
                ) -> ::core::option::Option<::std::string::String> {
                    $crate::guest::render_call(custom_type, call_json, opts_json)
                }
                fn render_result(
                    custom_type: ::std::string::String,
                    result_json: ::std::string::String,
                    opts_json: ::std::string::String,
                ) -> ::core::option::Option<::std::string::String> {
                    $crate::guest::render_result(custom_type, result_json, opts_json)
                }
                fn transform_markdown(
                    markdown: ::std::string::String,
                    ctx_json: ::std::string::String,
                ) -> ::std::string::String {
                    $crate::guest::transform_markdown(markdown, ctx_json)
                }
                // DRIFT-004 — the guest half of `UserBashEventResult.operations`. Called only on
                // a guest that declared `registration.register-bash-operations`.
                fn bash_operations_exec(
                    call_id: ::std::string::String,
                    command: ::std::string::String,
                    cwd: ::std::string::String,
                    opts_json: ::std::string::String,
                ) -> ::core::result::Result<::core::option::Option<i32>, ::std::string::String>
                {
                    $crate::guest::bash_operations_exec(call_id, command, cwd, opts_json)
                }
                fn on_terminal_input(
                    data: ::std::string::String,
                ) -> ::core::option::Option<
                    $crate::guest::bindings::exports::cyrup::ext::events::TerminalInputResult,
                > {
                    $crate::guest::on_terminal_input(data).map(|r| {
                        $crate::guest::bindings::exports::cyrup::ext::events::TerminalInputResult {
                            consume: r.consume,
                            data: r.data,
                        }
                    })
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
                        id,
                        stream_id,
                        model_json,
                        context_json,
                        options_json,
                    )
                }
                fn autocomplete_suggest(
                    base_json: ::std::string::String,
                    query_json: ::std::string::String,
                ) -> ::std::string::String {
                    $crate::guest::autocomplete_suggest(base_json, query_json)
                }
                fn with_session(
                    callback_id: ::std::string::String,
                ) -> ::core::result::Result<(), ::std::string::String> {
                    $crate::guest::with_session(callback_id)
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
                    input_json: ::std::string::String,
                    content_json: ::std::string::String,
                    is_error: bool,
                    details_json: ::core::option::Option<::std::string::String>,
                    usage_json: ::core::option::Option<::std::string::String>,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(
                        1,
                        &[
                            &call_id,
                            &name,
                            &input_json,
                            &content_json,
                            $crate::guest::b(is_error),
                            details_json.as_deref().unwrap_or(""),
                            usage_json.as_deref().unwrap_or(""),
                        ],
                    )
                }
                fn on_context(
                    messages_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(2, &[&messages_json])
                }
                fn on_message_end(
                    message_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
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
                fn on_input(
                    text: ::std::string::String,
                    images_json: ::std::string::String,
                    source: ::std::string::String,
                    streaming_behavior: ::core::option::Option<::std::string::String>,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(
                        18,
                        &[
                            &text,
                            &images_json,
                            &source,
                            streaming_behavior.as_deref().unwrap_or(""),
                        ],
                    )
                }
                fn on_user_bash(
                    command: ::std::string::String,
                    exclude_from_context: bool,
                    cwd: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(
                        19,
                        &[&command, $crate::guest::b(exclude_from_context), &cwd],
                    )
                }
                fn on_before_provider_request(
                    payload_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(20, &[&payload_json])
                }
                // EXT-009 / PROV-042 — `before_provider_headers` (pi `BeforeProviderHeadersEvent`,
                // extensions/types.ts:686-689 @v0.83.0; runner `emitBeforeProviderHeaders`,
                // core/extensions/runner.ts:1049-1065). Kind 31.
                fn on_before_provider_headers(
                    headers_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(31, &[&headers_json])
                }
                // EXT-016 — `cwd` + `reason` (pi extensions/types.ts:544-548 @v0.83.0).
                fn on_resources_discover(
                    cwd: ::std::string::String,
                    reason: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(5, &[&cwd, &reason])
                }
                fn on_project_trust(
                    cwd: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(6, &[&cwd])
                }
                fn on_session_before_switch(
                    reason: ::std::string::String,
                    target_session_file: ::core::option::Option<::std::string::String>,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(
                        24,
                        &[&reason, target_session_file.as_deref().unwrap_or("")],
                    )
                }
                fn on_session_before_fork(
                    entry_id: ::std::string::String,
                    position: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(25, &[&entry_id, &position])
                }
                fn on_session_before_compact(
                    preparation_json: ::std::string::String,
                    branch_entries_json: ::std::string::String,
                    custom_instructions: ::core::option::Option<::std::string::String>,
                    reason: ::std::string::String,
                    will_retry: bool,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(
                        26,
                        &[
                            &preparation_json,
                            &branch_entries_json,
                            custom_instructions.as_deref().unwrap_or(""),
                            &reason,
                            $crate::guest::b(will_retry),
                        ],
                    )
                }
                fn on_session_before_tree(
                    preparation_json: ::std::string::String,
                ) -> bindings::cyrup::ext::types::HookOutcome {
                    $crate::guest::hook(28, &[&preparation_json])
                }

                // --- notify-only hooks ---
                fn on_agent_start() {
                    $crate::guest::notify(7, &[]);
                }
                fn on_agent_end(messages_json: ::std::string::String) {
                    $crate::guest::notify(8, &[&messages_json]);
                }
                fn on_agent_settled() {
                    $crate::guest::notify(30, &[]);
                }
                fn on_turn_start(turn_index: u32, timestamp: u64) {
                    $crate::guest::notify(9, &[&turn_index.to_string(), &timestamp.to_string()]);
                }
                fn on_turn_end(
                    turn_index: u32,
                    message_json: ::std::string::String,
                    tool_results_json: ::std::string::String,
                ) {
                    $crate::guest::notify(
                        10,
                        &[&turn_index.to_string(), &message_json, &tool_results_json],
                    );
                }
                fn on_message_start(message_json: ::std::string::String) {
                    $crate::guest::notify(11, &[&message_json]);
                }
                fn on_message_update(
                    message_json: ::std::string::String,
                    delta_json: ::std::string::String,
                ) {
                    $crate::guest::notify(12, &[&message_json, &delta_json]);
                }
                fn on_tool_execution_start(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    args_json: ::std::string::String,
                ) {
                    $crate::guest::notify(13, &[&call_id, &name, &args_json]);
                }
                fn on_tool_execution_update(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    args_json: ::std::string::String,
                    chunk_json: ::std::string::String,
                ) {
                    $crate::guest::notify(14, &[&call_id, &name, &args_json, &chunk_json]);
                }
                fn on_tool_execution_end(
                    call_id: ::std::string::String,
                    name: ::std::string::String,
                    result_json: ::std::string::String,
                    is_error: bool,
                ) {
                    $crate::guest::notify(
                        15,
                        &[&call_id, &name, &result_json, $crate::guest::b(is_error)],
                    );
                }
                fn on_session_start(
                    reason: ::std::string::String,
                    previous_session_file: ::core::option::Option<::std::string::String>,
                ) {
                    $crate::guest::notify(
                        16,
                        &[&reason, previous_session_file.as_deref().unwrap_or("")],
                    );
                }
                fn on_session_shutdown(
                    reason: ::std::string::String,
                    target_session_file: ::core::option::Option<::std::string::String>,
                ) {
                    $crate::guest::notify(
                        17,
                        &[&reason, target_session_file.as_deref().unwrap_or("")],
                    );
                }
                // EXT-011 — `session_info_changed` (pi `SessionInfoChangedEvent`,
                // extensions/types.ts:571-575 @v0.83.0). Kind 32; notify-only.
                fn on_session_info_changed(name: ::core::option::Option<::std::string::String>) {
                    $crate::guest::notify(32, &[name.as_deref().unwrap_or("")]);
                }
                fn on_after_provider_response(status: u32, headers_json: ::std::string::String) {
                    $crate::guest::notify(21, &[&status.to_string(), &headers_json]);
                }
                fn on_model_select(
                    model_json: ::std::string::String,
                    previous_model_json: ::core::option::Option<::std::string::String>,
                    source: ::std::string::String,
                ) {
                    $crate::guest::notify(
                        22,
                        &[
                            &model_json,
                            previous_model_json.as_deref().unwrap_or(""),
                            &source,
                        ],
                    );
                }
                fn on_thinking_level_select(
                    level: ::std::string::String,
                    previous_level: ::core::option::Option<::std::string::String>,
                ) {
                    $crate::guest::notify(23, &[&level, previous_level.as_deref().unwrap_or("")]);
                }
                fn on_session_compact(
                    compaction_entry_json: ::std::string::String,
                    from_extension: bool,
                    reason: ::std::string::String,
                    will_retry: bool,
                ) {
                    $crate::guest::notify(
                        27,
                        &[
                            &compaction_entry_json,
                            $crate::guest::b(from_extension),
                            &reason,
                            $crate::guest::b(will_retry),
                        ],
                    );
                }
                fn on_session_tree(tree_json: ::std::string::String) {
                    $crate::guest::notify(29, &[&tree_json]);
                }

                // --- inter-extension event bus delivery (gap-08 §5.3) ---
                fn bus_deliver(topic: ::std::string::String, payload_json: ::std::string::String) {
                    $crate::guest::bus_deliver(topic, payload_json)
                }
            }

    bindings::export!(__CyrupExtComponent with_types_in bindings);
        };
    };
}
