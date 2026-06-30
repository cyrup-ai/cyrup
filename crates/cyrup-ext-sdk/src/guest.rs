//! The wasm32 guest binding layer (arch-08 §4.1). Generates the `cyrup:ext` bindings with
//! `wit-bindgen` (with `pub_export_macro` so a downstream author crate can call `export!`), then
//! provides the routing free functions the [`crate::export_extension!`] macro wires the world's
//! exports to: `init`, the 30 lifecycle hooks, `execute-tool`, `execute-command`/
//! `get-argument-completions`, and `render-call`/`render-result`. Compiled ONLY for `wasm32`.
//!
//! The glue (the `Guest`/`events::Guest` impls + `export!`) is emitted by the macro — in THIS crate
//! for the bundled [`crate::example`], or in a third-party author's crate — so a one-line
//! `export_extension!(my_factory)` yields a loadable `cyrup:ext` component. The macro delegates every
//! export to the helpers here, so the routing logic lives once.

#![allow(clippy::all)]

use crate::api::{ExtensionApi, RawOutcome};
use crate::ctx::ToolCall;
use core::cell::RefCell;
use serde_json::Value;

pub mod bindings {
    #![allow(clippy::all, dead_code, unused)]
    wit_bindgen::generate!({
        world: "extension",
        path: "wit",
        // Make `bindings::export!` reachable from a downstream author crate (the macro path).
        pub_export_macro: true,
    });
}

use bindings::cyrup::ext::{registration, types};

thread_local! {
    static API: RefCell<Option<ExtensionApi>> = const { RefCell::new(None) };
}

/// The world's `init` body (R-08-001): build the author's [`ExtensionApi`] from `factory`, flush its
/// registrations through the host imports, declare the subscription bitset, and stash the api for the
/// event/tool/command exports. Called by the macro-generated `Guest::init`.
pub fn run_init(factory: fn() -> ExtensionApi) -> Result<(), String> {
    let api = factory();
    push_registrations(&api);
    API.with(|c| *c.borrow_mut() = Some(api));
    Ok(())
}

/// Flush the author's declared registrations through the host imports + declare the subscription set.
fn push_registrations(api: &ExtensionApi) {
    registration::subscribe(&api.subscription_kinds());

    for t in api.tools() {
        let d = &t.descriptor;
        let wit = types::ToolDescriptor {
            name: d.name.clone(),
            label: d.label.clone(),
            description: d.description.clone(),
            parameters_json: d.parameters.to_string(),
            exec_mode: d.execution_mode.map(|m| match m {
                crate::descriptor::ExecMode::Parallel => types::ExecMode::Parallel,
                crate::descriptor::ExecMode::Sequential => types::ExecMode::Sequential,
            }),
            prompt_snippet: d.prompt_snippet.clone(),
            prompt_guidelines: d.prompt_guidelines.clone(),
            has_renderer: d.has_renderer,
        };
        registration::register_tool(&wit);
    }
    for (name, cmd) in &api.commands {
        let desc_json = serde_json::to_string(&cmd.descriptor).unwrap_or_else(|_| "{}".into());
        registration::register_command(name, &desc_json);
    }
    for s in &api.shortcuts {
        registration::register_shortcut(&s.key, &s.description);
    }
    for (name, spec) in &api.flags {
        let spec_json = serde_json::to_string(spec).unwrap_or_else(|_| "{}".into());
        registration::register_flag(name, &spec_json);
    }
    for (id, config) in &api.providers {
        let config_json = serde_json::to_string(config).unwrap_or_else(|_| "{}".into());
        registration::register_provider(id, &config_json);
    }
    for r in &api.renderers {
        registration::register_message_renderer(&r.custom_type);
    }
    for command in &api.autocomplete {
        registration::add_autocomplete(command);
    }
    // One `add-autocomplete-provider` per stacked global provider (Pi addAutocompleteProvider).
    for _ in 0..api.autocomplete_provider_count() {
        registration::add_autocomplete_provider();
    }
}

/// Run the registered handler for `kind` with ordered string args; returns the lowered outcome.
fn dispatch(kind: u8, args: &[&str]) -> RawOutcome {
    let ctx = crate::ctx::Ctx::new();
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.dispatch(kind, args, &ctx),
        None => RawOutcome::Noop,
    })
}

fn to_wit(o: RawOutcome) -> types::HookOutcome {
    match o {
        RawOutcome::Noop => types::HookOutcome::Noop,
        RawOutcome::Block(r) => types::HookOutcome::Block(r),
        RawOutcome::Mutate(s) => types::HookOutcome::Mutate(s),
        RawOutcome::Handled(s) => types::HookOutcome::Handled(s),
    }
}

/// Stringify a bool for the ordered-arg seam.
pub fn b(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}

/// A block/mutate/handled hook export: dispatch + lower to the WIT outcome (macro entry point).
pub fn hook(kind: u8, args: &[&str]) -> types::HookOutcome {
    to_wit(dispatch(kind, args))
}

/// A notify-only hook export: dispatch, return ignored (macro entry point).
pub fn notify(kind: u8, args: &[&str]) {
    dispatch(kind, args);
}

/// `execute-tool` export body (R-08-015).
pub fn run_tool(
    name: String,
    call_id: String,
    params_json: String,
) -> Result<types::ToolOutput, String> {
    let params = serde_json::from_str(&params_json).unwrap_or(Value::Null);
    let call = ToolCall::new(call_id, params);
    let out = API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.execute_tool(&name, call),
        None => Err(format!("no such tool: {name}")),
    })?;
    Ok(types::ToolOutput {
        content_json: serde_json::to_string(&out.content).unwrap_or_else(|_| "[]".into()),
        details_json: out.details.map(|d| d.to_string()),
        is_error: out.is_error,
        terminate: out.terminate,
    })
}

/// `execute-command` export body (R-08-016).
pub fn run_command(name: String, args: String) -> Result<Option<String>, String> {
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.execute_command(&name, &args),
        None => Err(format!("no such command: {name}")),
    })
}

/// `execute-shortcut` export body (R-08-017; Pi `registerShortcut` handler, types.ts:1199-1205): run
/// the stored handler for `key` against a fresh [`crate::ctx::Ctx`]. An unknown key is an error.
pub fn run_shortcut(key: String) -> Result<(), String> {
    let ctx = crate::ctx::Ctx::new();
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.execute_shortcut(&key, &ctx),
        None => Err(format!("no such shortcut: {key}")),
    })
}

/// `get-argument-completions` export body (Pi `getArgumentCompletions`).
pub fn completions(name: String, prefix: String) -> Vec<String> {
    API.with(|c| {
        c.borrow().as_ref().map(|api| api.argument_completions(&name, &prefix)).unwrap_or_default()
    })
}

/// `render-call` export body (Pi `renderCall`).
pub fn render_call(custom_type: String, call_json: String) -> Option<String> {
    let call = serde_json::from_str(&call_json).unwrap_or(Value::Null);
    API.with(|c| {
        c.borrow().as_ref().and_then(|api| api.render_call(&custom_type, &call)).map(|v| v.to_string())
    })
}

/// `render-result` export body (Pi `renderResult`).
pub fn render_result(custom_type: String, result_json: String) -> Option<String> {
    let result = serde_json::from_str(&result_json).unwrap_or(Value::Null);
    API.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|api| api.render_result(&custom_type, &result))
            .map(|v| v.to_string())
    })
}

/// `provider-login` export body (Pi `oauth.login`): returns the credentials JSON to persist.
pub fn provider_login(id: String) -> Result<String, String> {
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.provider_login(&id).map(|creds| creds.to_string()),
        None => Err("extension not initialized".into()),
    })
}

/// `provider-refresh-token` export body (Pi `oauth.refreshToken`).
pub fn provider_refresh_token(id: String, credentials_json: String) -> Result<String, String> {
    let creds = serde_json::from_str(&credentials_json).unwrap_or(Value::Null);
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.provider_refresh_token(&id, creds).map(|c| c.to_string()),
        None => Err("extension not initialized".into()),
    })
}

/// `provider-get-api-key` export body (Pi `oauth.getApiKey`).
pub fn provider_get_api_key(id: String, credentials_json: String) -> Result<String, String> {
    let creds = serde_json::from_str(&credentials_json).unwrap_or(Value::Null);
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.provider_get_api_key(&id, &creds),
        None => Err("extension not initialized".into()),
    })
}

/// `provider-modify-models` export body (Pi optional `oauth.modifyModels`).
pub fn provider_modify_models(
    id: String,
    models_json: String,
    credentials_json: String,
) -> Result<String, String> {
    let models = serde_json::from_str(&models_json).unwrap_or(Value::Null);
    let creds = serde_json::from_str(&credentials_json).unwrap_or(Value::Null);
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.provider_modify_models(&id, models, &creds).map(|m| m.to_string()),
        None => Err("extension not initialized".into()),
    })
}

/// `provider-stream-simple` export body (Pi `streamSimple`): drive the guest's custom stream.
pub fn provider_stream_simple(
    id: String,
    stream_id: String,
    model_json: String,
    context_json: String,
    options_json: String,
) -> Result<(), String> {
    let model = serde_json::from_str(&model_json).unwrap_or(Value::Null);
    let context = serde_json::from_str(&context_json).unwrap_or(Value::Null);
    let options = serde_json::from_str(&options_json).unwrap_or(Value::Null);
    let stream = crate::provider::ProviderStream::new(stream_id);
    API.with(|c| match c.borrow().as_ref() {
        Some(api) => api.provider_stream_simple(&id, &stream, model, context, options),
        None => Err("extension not initialized".into()),
    })
}

/// `with-session` export body (Pi `ReplacedSessionContext` re-binding, sdk gap #3): run the stored
/// `withSession` closure for `callback_id` against a freshly-bound command-tier context. The host
/// calls this after re-binding the session that a `control.*` op replaced.
pub fn with_session(callback_id: String) -> Result<(), String> {
    crate::ctx::run_with_session(&callback_id)
}

/// `autocomplete-suggest` export body (Pi the `AutocompleteProviderFactory` chain): fold the stacked
/// providers over the host's built-in suggestions, returning the final serialized suggestions.
pub fn autocomplete_suggest(base_json: String, query_json: String) -> String {
    let base: Option<crate::autocomplete::AutocompleteSuggestions> =
        serde_json::from_str(&base_json).ok();
    let query: crate::autocomplete::AutocompleteQuery =
        serde_json::from_str(&query_json).unwrap_or_default();
    API.with(|c| {
        let folded = c.borrow().as_ref().and_then(|api| api.autocomplete_suggest(base, &query));
        serde_json::to_string(&folded.unwrap_or_default()).unwrap_or_else(|_| "{\"items\":[],\"prefix\":\"\"}".into())
    })
}
