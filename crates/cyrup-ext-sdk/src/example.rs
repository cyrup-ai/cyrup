//! A tiny reference extension authored with this SDK (arch-08 §11; the analog of Pi's
//! `examples/extensions/permission-gate.ts` + a dynamic tool). Building this crate to
//! `wasm32-wasip2` produces a loadable `cyrup:ext` COMPONENT whose `init` registers everything
//! below; the host loads it and dispatches real events to it (the arch-08b live E2E proof).
//!
//! It demonstrates: a `tool_call` permission gate (block), a notify hook (`agent_start`), and a
//! dynamically-registered streaming tool (`demo_echo`).

use crate::{
    AutocompleteItem, AutocompleteSuggestions, CommandDescriptor, DialogOptions, ExecOptions,
    ExtensionApi, HttpRequest, MessageRenderer, NewSessionOptions, NotifyKind, OAuthProvider,
    Outcome, ProcSpawnOptions, ProviderConfig, ProviderHandlers, ReplacedSessionContext, ToolCall,
    ToolDescriptor, ToolOutput,
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

    // --- the previously-dead mutating seams, now driven by the assembled host (gap-08 #1-#5) ---

    // before_agent_start (gap-08 #1): when the prompt is exactly "go", inject a marker message AND
    // replace the system prompt. Injected messages accumulate across handlers (Pi runner.ts:1014).
    api.on_before_agent_start(|ev, _ctx| {
        if ev.prompt == "go" {
            Outcome::before_agent_start(crate::BeforeAgentStartResult {
                message: Some(json!({ "role": "user", "content": "injected by demo", "timestamp": 0 })),
                system_prompt: Some(format!("INJECTED:{}", ev.system_prompt)),
            })
        } else {
            Outcome::noop()
        }
    });

    // input (gap-08 #2): block "secret", uppercase-transform an "up:" prefix, else continue (Pi
    // `InputEventResult` `{action:"transform"|"handled"}` / block, runner.ts:1108-1131).
    api.on_input(|ev, _ctx| {
        if ev.text == "secret" {
            Outcome::block("input blocked by demo")
        } else if let Some(rest) = ev.text.strip_prefix("up:") {
            Outcome::mutate(json!({ "action": "transform", "text": rest.to_uppercase() }))
        } else {
            Outcome::noop()
        }
    });

    // message_end (gap-08 #3): redact a user message whose text is "redact me", preserving the role
    // (the host rejects a role change, Pi runner.ts:785).
    api.on_message_end(|ev, _ctx| {
        let role = ev.message.get("role").and_then(|r| r.as_str());
        let text = ev.message.get("content").and_then(|c| c.as_str());
        if role == Some("user") && text == Some("redact me") {
            Outcome::replace_message(json!({ "role": "user", "content": "[redacted]", "timestamp": 0 }))
        } else {
            Outcome::noop()
        }
    });

    // before_provider_request (gap-08 #4): tag the outbound payload — Pi replaces the payload
    // wholesale with the handler's return value (runner.ts:962).
    api.on_before_provider_request(|ev, _ctx| {
        let mut payload = ev.payload.clone();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("demoTag".into(), json!(true));
        }
        Outcome::mutate(payload)
    });

    // user_bash (gap-08 #5): block a destructive `!rm -rf` invocation; otherwise proceed.
    api.on_user_bash(|ev, _ctx| {
        if ev.command.contains("rm -rf") {
            Outcome::block("user_bash blocked by demo")
        } else {
            Outcome::noop()
        }
    });

    // session_before_compact (L4 gap #5): READ the computed typed preparation and return a custom
    // summary override (Pi `SessionBeforeCompactResult.compaction`, agent-session.ts:1672-1693). The
    // override's summary lands in the appended compaction entry (marked `fromExtension`). The demo
    // derives the summary from the preparation so the test proves the typed payload crossed the seam.
    api.on_session_before_compact(|ev, _ctx| {
        // `preparation.firstKeptEntryId` is a real field of Pi's `CompactionPreparation`.
        let first_kept = ev
            .preparation
            .get("firstKeptEntryId")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        Outcome::compaction_override(crate::SessionBeforeCompactResult {
            summary: format!("demo-summary[{}|firstKept={first_kept}]", ev.reason),
            ..Default::default()
        })
    });

    // session_before_tree (L4 gap #5): READ the TreePreparation and override the branch-summary label
    // (Pi `SessionBeforeTreeResult.label`). Proves the typed tree preparation crossed the seam.
    api.on_session_before_tree(|ev, _ctx| {
        let target = ev.preparation.get("targetId").and_then(|v| v.as_str()).unwrap_or("?");
        Outcome::tree_override(crate::SessionBeforeTreeResult {
            label: Some(format!("demo-tree-label[{target}]")),
            ..Default::default()
        })
    });

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

    // A command exercising the state-mutation seams (R-08-026; Pi `appendEntry`/`setSessionName`/
    // `setLabel`, agent-session.ts:2265-2279): append a custom entry to the live tree, rename the
    // session, then label the just-appended entry. Proves each no-op FIRES against the real session.
    api.register_command(
        "statedemo",
        CommandDescriptor::new("Append a custom entry, rename the session, label the entry (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let id = ctx.ctx().session().append_entry("demoNote", json!({ "note": "from guest" }))?;
            ctx.ctx().session().set_session_name("renamed-by-guest");
            ctx.ctx().session().set_label(&id, "guest-label");
            Ok(Some(format!("appended {id}")))
        },
    );

    // A command exercising the capability-scoped exec grant (arch-08 exec; Pi `pi.exec` →
    // `execCommand`, exec.ts:34-46): run `echo hi` as a DIRECT argv (shell:false) and surface the
    // REAL captured stdout + `killed` flag. When the host has NOT granted exec (untrusted ⇒
    // `DenyServices`) the call errors and we notify the denial reason instead — proving the same
    // seam gates both ways.
    api.register_command(
        "execdemo",
        CommandDescriptor::new("Run `echo hi` via the exec capability and report stdout (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| match ctx.ctx().exec(
            "echo",
            &["hi"],
            &ExecOptions::default(),
        ) {
            Ok(r) => {
                ctx.ui().notify(&format!("exec stdout: {}", r.stdout.trim_end()));
                Ok(Some(format!("exec code {} killed {}", r.code, r.killed)))
            }
            Err(e) => {
                ctx.ui().notify(&format!("exec denied: {e}"));
                Ok(Some(format!("exec denied: {e}")))
            }
        },
    );

    // A command exercising the capability-scoped http-client grant (arch-08 §3.2 draft;
    // pi-mcp-adapter-port.md §3.2): GET `args` (the target URL) and surface the REAL captured status
    // + body. When the host has NOT granted http-client (untrusted ⇒ `DenyServices`) the call errors
    // and we notify the denial reason instead — the same seam gates both ways (mirrors `execdemo`).
    api.register_command(
        "httpdemo",
        CommandDescriptor::new(
            "GET a URL via the http-client capability and report status+body (demo).",
        ),
        |args: &str, ctx: &crate::CommandCtx| match ctx
            .ctx()
            .http_request(&HttpRequest::get(args.trim()))
        {
            Ok(r) => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                ctx.ui().notify(&format!("http status: {} body: {}", r.status, body));
                Ok(Some(format!("http status {} body {}", r.status, body)))
            }
            Err(e) => {
                ctx.ui().notify(&format!("http denied: {e}"));
                Ok(Some(format!("http denied: {e}")))
            }
        },
    );

    // A command exercising the streaming half of the http-client grant (`request-stream` /
    // `poll-stream-chunk`): open a stream to `args`, immediately surface the initiating response's
    // status+headers (closes L4 §2.3 — available BEFORE and INDEPENDENT of draining any chunk, off
    // the SAME round trip `request-stream` used to open the body, exactly what the real consumer this
    // backs — the MCP SDK's `StreamableHTTPClientTransport` — reads off its one `fetch()` response),
    // then poll every chunk to EOF and surface the real chunk count + concatenated body — proving the
    // host-owned stream registry (arch-08 §5.2's request/poll bridge) delivers real bytes across the
    // wasm boundary, in order.
    api.register_command(
        "httpstreamdemo",
        CommandDescriptor::new(
            "Stream a URL via the http-client capability and report status+headers+chunks+body (demo).",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let opened = match ctx.ctx().http_request_stream(&HttpRequest::get(args.trim())) {
                Ok(o) => o,
                Err(e) => {
                    ctx.ui().notify(&format!("http stream denied: {e}"));
                    return Ok(Some(format!("http stream denied: {e}")));
                }
            };
            let content_type = opened
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            // Notified BEFORE any chunk is polled: proves status/headers are independent of the body.
            ctx.ui().notify(&format!(
                "http stream opened status: {} content-type: {content_type}",
                opened.status
            ));
            let handle = opened.handle;
            let mut body = Vec::new();
            let mut chunks = 0u32;
            loop {
                match ctx.ctx().http_poll_stream_chunk(handle) {
                    Ok(Some(chunk)) => {
                        chunks += 1;
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        ctx.ui().notify(&format!("http stream poll error: {e}"));
                        break;
                    }
                }
            }
            ctx.ctx().http_close_stream(handle);
            let body = String::from_utf8_lossy(&body).into_owned();
            ctx.ui().notify(&format!("http stream chunks: {chunks} body: {body}"));
            Ok(Some(format!(
                "http stream status {} content-type {content_type} chunks {chunks} body {body}",
                opened.status
            )))
        },
    );

    // Commands exercising the capability-scoped `proc` grant (arch-08 §5.2 request/poll bridge;
    // pi-mcp-adapter-port.md §3.1): a long-lived, duplex-pipe child, distinct from the bounded
    // `execdemo` one-shot. Split into separate commands (rather than one big demo like
    // `execdemo`/`httpdemo`) so a HOST-side test can drive each step as its own top-level
    // `session.prompt(...)` round trip — proving stdin/stdout stay live across genuinely separate
    // calls, not just an internal loop within one guest invocation — and interleave real OS-level
    // process checks between `procspawn` and `prockill`.
    //
    // `procspawn` runs a marker-tagged shell read-echo loop (`sh -c 'while IFS= read -r line; do
    // printf "echo:%s\n" "$line"; done' <marker>`) — a genuine long-lived duplex process (not a
    // one-shot), with the marker as a trailing shell positional arg so a host-side `pgrep -f
    // <marker>` can find (and later confirm the disappearance of) the exact real OS process.
    api.register_command(
        "procspawn",
        CommandDescriptor::new(
            "Spawn a marker-tagged shell read-echo loop via the proc capability (demo). \
             args: <marker>",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let marker = args.trim();
            match ctx.ctx().proc_spawn(
                "sh",
                &["-c", "while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done", marker],
                &ProcSpawnOptions::default(),
            ) {
                Ok(handle) => {
                    ctx.ui().notify(&format!("proc spawned handle:{handle}"));
                    Ok(Some(format!("proc spawned handle:{handle}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc denied: {e}"));
                    Ok(Some(format!("proc denied: {e}")))
                }
            }
        },
    );

    // `procspawnexit` (no args): spawns a child that exits ON ITS OWN shortly after starting
    // (`sh -c "sleep 0.1; exit 7"`) — no `kill` involved — so a host-side test can prove `poll-exit`
    // reports the REAL natural exit code, not just a `kill`-driven one.
    api.register_command(
        "procspawnexit",
        CommandDescriptor::new("Spawn a proc that exits on its own with code 7 (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            match ctx.ctx().proc_spawn(
                "sh",
                &["-c", "sleep 0.1; exit 7"],
                &ProcSpawnOptions::default(),
            ) {
                Ok(handle) => {
                    ctx.ui().notify(&format!("proc spawned handle:{handle}"));
                    Ok(Some(format!("proc spawned handle:{handle}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc denied: {e}"));
                    Ok(Some(format!("proc denied: {e}")))
                }
            }
        },
    );

    // `procwrite <handle> <text>`: write `<text>\n` to the child's REAL stdin.
    api.register_command(
        "procwrite",
        CommandDescriptor::new("Write a line to a spawned proc's stdin (demo). args: <handle> <text>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let mut it = args.trim().splitn(2, ' ');
            let handle: u32 = it.next().unwrap_or_default().parse().unwrap_or(0);
            let text = it.next().unwrap_or_default();
            let mut line = text.to_string();
            line.push('\n');
            match ctx.ctx().proc_write_stdin(handle, line.as_bytes()) {
                Ok(n) => {
                    ctx.ui().notify(&format!("proc wrote handle:{handle} bytes:{n}"));
                    Ok(Some(format!("proc wrote {n} bytes")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("proc write denied: {e}"));
                    Ok(Some(format!("proc write denied: {e}")))
                }
            }
        },
    );

    // `procreadpoll <handle> <needle>`: poll REAL stdout — across MULTIPLE `read-stdout` calls in a
    // bounded loop (empty = no data yet, never treated as EOF) — until the accumulated bytes
    // contain `<needle>`, proving the pipe is genuinely live, not a captured one-shot.
    api.register_command(
        "procreadpoll",
        CommandDescriptor::new(
            "Poll a spawned proc's stdout until a needle appears (demo). args: <handle> <needle>",
        ),
        |args: &str, ctx: &crate::CommandCtx| {
            let mut it = args.trim().splitn(2, ' ');
            let handle: u32 = it.next().unwrap_or_default().parse().unwrap_or(0);
            let needle = it.next().unwrap_or_default().as_bytes();
            let mut acc: Vec<u8> = Vec::new();
            let mut seen = false;
            for _ in 0..20_000u32 {
                match ctx.ctx().proc_read_stdout(handle, 4096) {
                    Ok(chunk) => acc.extend_from_slice(&chunk),
                    Err(e) => {
                        ctx.ui().notify(&format!("proc read denied: {e}"));
                        return Ok(Some(format!("proc read denied: {e}")));
                    }
                }
                if !needle.is_empty() && acc.windows(needle.len()).any(|w| w == needle) {
                    seen = true;
                    break;
                }
            }
            let acc_text = String::from_utf8_lossy(&acc).into_owned();
            ctx.ui().notify(&format!("proc read handle:{handle} seen:{seen} acc:{acc_text}"));
            Ok(Some(format!("proc read seen:{seen}")))
        },
    );

    // `procpollexit <handle>`: a single non-blocking `poll-exit` (none = still running).
    api.register_command(
        "procpollexit",
        CommandDescriptor::new("Poll a spawned proc's exit status once (demo). args: <handle>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let handle: u32 = args.trim().parse().unwrap_or(0);
            let code = ctx.ctx().proc_poll_exit(handle);
            ctx.ui().notify(&format!("proc pollexit handle:{handle} code:{code:?}"));
            Ok(Some(format!("proc pollexit code:{code:?}")))
        },
    );

    // `prockill <handle>`: terminate the child (SIGTERM then SIGKILL after a grace period,
    // host-side policy) and report both the kill outcome and the exit status observed right after.
    api.register_command(
        "prockill",
        CommandDescriptor::new("Kill a spawned proc (demo). args: <handle>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let handle: u32 = args.trim().parse().unwrap_or(0);
            let kill_result = ctx.ctx().proc_kill(handle);
            let code = ctx.ctx().proc_poll_exit(handle);
            ctx.ui().notify(&format!(
                "proc kill handle:{handle} ok:{} code:{code:?}",
                kill_result.is_ok()
            ));
            match kill_result {
                Ok(()) => Ok(Some(format!("proc killed code:{code:?}"))),
                Err(e) => Ok(Some(format!("proc kill denied: {e}"))),
            }
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
                // Full Pi model shape (sdk gap #26): reasoning/input modalities/cost/contextWindow/maxTokens.
                reasoning: true,
                input: vec!["text".into(), "image".into()],
                cost: crate::ModelCost { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
                context_window: Some(200000),
                max_tokens: Some(8192),
                ..Default::default()
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
