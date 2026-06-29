//! A tiny reference extension authored with this SDK (arch-08 §11; the analog of Pi's
//! `examples/extensions/permission-gate.ts` + a dynamic tool). Building this crate to
//! `wasm32-wasip2` produces a loadable `cyrup:ext` COMPONENT whose `init` registers everything
//! below; the host loads it and dispatches real events to it (the arch-08b live E2E proof).
//!
//! It demonstrates: a `tool_call` permission gate (block), a notify hook (`agent_start`), and a
//! dynamically-registered streaming tool (`demo_echo`).

use crate::{
    AutocompleteItem, AutocompleteSuggestions, CommandDescriptor, DialogOptions, ExtensionApi,
    MessageRenderer, NewSessionOptions, NotifyKind, OAuthProvider, Outcome, ProviderConfig,
    ProviderHandlers, ReplacedSessionContext, ToolCall, ToolDescriptor, ToolOutput,
};
use serde_json::{json, Value};

/// A trivial custom renderer for the demo's `custom_type` (Pi `renderCall`/`renderResult`).
struct DemoRenderer;
impl MessageRenderer for DemoRenderer {
    fn render_call(&self, call: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(json!({ "widget": "text", "text": format!("demo call: {call}") }))
    }
    fn render_result(&self, result: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(json!({ "widget": "text", "text": format!("demo result: {result}") }))
    }
}

/// Build the demo extension's [`ExtensionApi`]. Pure ergonomic-layer code — also unit-testable on
/// the host target.
pub fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();

    // Permission gate (R-08-010): block any `bash` tool call with a reason.
    api.on_tool_call(|ev, ctx| {
        if ev.name == "bash" {
            // An `error`-severity notification (Pi `notify(msg, "error")`, types.ts:135).
            ctx.ui().notify_with("permission-gate: blocked a bash call", NotifyKind::Error);
            Outcome::block("bash is disabled by the demo extension")
        } else {
            Outcome::noop()
        }
    });

    // Notify hook: announce activation when a run starts.
    api.on_agent_start(|ctx| ctx.ui().notify("demo extension active"));

    // A dynamically-registered tool (R-08-013/015): echoes its `text` argument, streaming a chunk.
    api.register_tool(
        ToolDescriptor::new(
            "demo_echo",
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }),
        )
        .description("Echo the input text back (demo tool)."),
        |call: ToolCall| {
            let text =
                call.params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // Stream a partial-output chunk (Pi onUpdate).
            call.emit_update(json!({ "content": [{ "type": "text", "text": "working..." }] }));
            Ok(ToolOutput::text(format!("echo: {text}")))
        },
    );

    // A slash command (R-08-016) with a dynamic argument completer: `/greet <name>` -> a greeting.
    api.register_command_with_completions(
        "greet",
        CommandDescriptor::new("Greet someone by name (demo command)."),
        |args: &str, ctx: &crate::CommandCtx| {
            ctx.ui().notify("greet command ran");
            // Address a keyed status segment (Pi `setStatus(key, text)`, types.ts:141), then clear
            // it (Pi `setStatus(key, undefined)`) — proves keyed set + clear over the boundary.
            ctx.ui().set_status("greet", Some("greeting…"));
            ctx.ui().clear_status("greet");
            // A COMMAND-tier control op (R-08-008): legal here, recorded by the host backend.
            let _ = ctx.compact();
            Ok(Some(format!("hello, {}!", args.trim())))
        },
        |prefix: &str| {
            ["world", "team", "everyone"]
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| c.to_string())
                .collect()
        },
    );

    // A custom message renderer (R-08-020) keyed by a custom tool type.
    api.register_message_renderer("demo", DemoRenderer);

    // A custom provider with OAuth + a custom `streamSimple` (Pi `registerProvider({oauth, streamSimple})`,
    // the `custom-provider-*` examples). The `login` flow drives the host `oauth` callbacks; the
    // stream pushes assistant-message events back across the `provider-stream` import.
    let oauth = OAuthProvider::new(
        "Demo SSO",
        |callbacks: &crate::OAuthCallbacks| {
            // Drive the interactive login (Pi onAuth + onPrompt).
            callbacks.on_auth("https://demo.example/oauth/authorize?x=1", None);
            let code = callbacks.on_prompt("Paste the callback code:", None, false)?;
            Ok(json!({ "refresh": "r-demo", "access": format!("a-{code}"), "expires": 0 }))
        },
        |creds: Value| {
            // Refresh: rotate the access token (Pi refreshToken).
            let refresh = creds.get("refresh").and_then(|v| v.as_str()).unwrap_or("r-demo");
            Ok(json!({ "refresh": refresh, "access": "a-refreshed", "expires": 0 }))
        },
        |creds: &Value| {
            // getApiKey: derive the key string from the credentials.
            Ok(creds.get("access").and_then(|v| v.as_str()).unwrap_or_default().to_string())
        },
    )
    .with_modify_models(|models: Value, _creds: &Value| Ok(models));

    let stream = |model: Value, _ctx: Value, _opts: Value, out: &crate::ProviderStream| {
        // Push two assistant-message events then end (Pi createAssistantMessageEventStream).
        let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("demo-model").to_string();
        out.emit(json!({ "type": "text", "text": format!("stream from {id}") }));
        out.emit(json!({ "type": "done" }));
        Ok(())
    };

    api.register_provider_with_handlers(
        "demo-oauth",
        ProviderConfig {
            name: "demo-oauth".into(),
            base_url: Some("https://demo.example".into()),
            api: Some("anthropic".into()),
            api_key: None,
            auth_header: None,
            headers: Default::default(),
            models: vec![crate::ProviderModelConfig {
                id: "demo-model".into(),
                name: Some("Demo Model".into()),
                context_window: Some(200000),
                max_output_tokens: Some(8192),
            }],
            oauth: None,
            has_stream_simple: false,
        },
        ProviderHandlers::new().with_oauth(oauth).with_stream_simple(stream),
    );

    // A global autocomplete provider (Pi `addAutocompleteProvider`): stack a "demo:" item on top of
    // whatever the wrapped ("current") provider produced.
    api.add_autocomplete_provider(
        |query: &crate::AutocompleteQuery, current: Option<&AutocompleteSuggestions>| {
            let mut items = current.map(|c| c.items.clone()).unwrap_or_default();
            items.push(AutocompleteItem::labelled("demo:run", "demo:run (extension)"));
            Some(AutocompleteSuggestions { items, prefix: query.current_line().to_string() })
        },
    );

    // A command exercising the active-tool restriction (Pi `setActiveTools`) + typed fork options.
    api.register_command(
        "planmode",
        CommandDescriptor::new("Restrict the active tools to read-only (demo plan mode)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.ctx().set_active_tools(&["read"]);
            let active = ctx.ctx().get_active_tools();
            Ok(Some(format!("active tools: {}", active.join(","))))
        },
    );

    // A tool that polls its cancellation `signal` (Pi `ToolDefinition.execute` `signal`, sdk gap #1):
    // a long tool would loop and bail when aborted; this demo just reports the current state.
    api.register_tool(
        ToolDescriptor::new("signal_probe", json!({ "type": "object", "properties": {} }))
            .description("Report whether the host has requested cancellation (demo signal)."),
        |call: ToolCall| Ok(ToolOutput::text(format!("aborted: {}", call.signal().is_aborted()))),
    );

    // A command exercising the programmatic dialog-dismiss signal (Pi `ExtensionUIDialogOptions.signal`,
    // sdk gap #2): abort the named signal, then a dialog bound to it returns cancelled (here `confirm`
    // -> false) even though the backend's canned answer is `true`.
    api.register_command(
        "signaldemo",
        CommandDescriptor::new("Dismiss a dialog via a named signal, then confirm (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.ui().abort_signal("demo-dialog");
            let ok = ctx.ui().confirm_with("proceed?", &DialogOptions::signal("demo-dialog"));
            Ok(Some(format!("confirmed: {ok}")))
        },
    );

    // A command exercising the `withSession` re-binding callback (Pi `ReplacedSessionContext`,
    // sdk gap #3): start a new session and move post-replacement work into the closure, which the host
    // invokes against the re-bound session after the command returns.
    api.register_command(
        "withsessiondemo",
        CommandDescriptor::new("Start a new session and notify on the re-bound session (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            ctx.new_session_with_callback(
                &NewSessionOptions::default(),
                |rsc: &ReplacedSessionContext| {
                    rsc.ui().notify("withSession ran on the replacement session");
                    Ok(())
                },
            )?;
            Ok(Some("new session scheduled".into()))
        },
    );

    api
}
