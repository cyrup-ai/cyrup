//! A tiny reference extension authored with this SDK (arch-08 §11; the analog of Pi's
//! `examples/extensions/permission-gate.ts` + a dynamic tool). Building this crate to
//! `wasm32-wasip2` produces a loadable `cyrup:ext` COMPONENT whose `init` registers everything
//! below; the host loads it and dispatches real events to it (the arch-08b live E2E proof).
//!
//! It demonstrates: a `tool_call` permission gate (block), a notify hook (`agent_start`), and a
//! dynamically-registered streaming tool (`demo_echo`).

use crate::{
    AutocompleteItem, AutocompleteSuggestions, CommandDescriptor, DialogOptions, ExecOptions,
    ExtensionApi, FlagSpec, HttpRequest, MessageRenderer, NewSessionOptions, NotifyKind,
    OAuthProvider, Outcome, ProcSpawnOptions, ProviderConfig, ProviderHandlers,
    ReplacedSessionContext, ToolCall, ToolDescriptor, ToolOutput,
};
use serde_json::{json, Value};

/// A trivial custom renderer for the demo's `custom_type` (Pi `renderCall`/`renderResult`).
///
/// The return is a SERIALIZED WIDGET TREE — cyrup's wire analog of the `pi-tui` `Component` a Pi
/// renderer returns. Build it with [`crate::widget`] rather than a hand-written `json!`: the host
/// draws only the documented vocabulary as rows and falls back to pretty-printed JSON for anything
/// else, so a typo here is a JSON blob in the user's transcript.
struct DemoRenderer;
impl MessageRenderer for DemoRenderer {
    fn render_call(&self, call: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::text(format!("demo call: {call}")))
    }
    fn render_result(&self, result: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::text(format!("demo result: {result}")))
    }
}

/// The per-TOOL renderer for `demo_echo` (Pi `ToolDefinition.renderCall`/`renderResult`,
/// extensions/types.ts:472-481). Registered under the TOOL NAME, which is the key the host routes
/// a tool row by (EXT-006).
///
/// The call side returns a MULTI-NODE tree (Pi renderers routinely return a `Container` of a header
/// plus detail rows) so the demo exercises the host flattener's stacking, not just the degenerate
/// single-`Text` case.
struct DemoToolRenderer;
impl MessageRenderer for DemoToolRenderer {
    fn render_call(&self, call: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::stack([
            crate::widget::text(format!("guest-rendered echo call: {call}")),
            crate::widget::text("(drawn by the demo extension)"),
        ]))
    }
    fn render_result(&self, result: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::text(format!("guest-rendered echo result: {result}")))
    }
}

/// A custom-ENTRY renderer (Pi `registerEntryRenderer`, extensions/types.ts:1295). Entries are
/// TUI-only durable state appended with `append_entry`; they never enter LLM context. An entry
/// crosses the boundary on `render-call`, so the renderer only implements that half.
struct DemoEntryRenderer;
impl MessageRenderer for DemoEntryRenderer {
    fn render_call(&self, entry: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        Some(crate::widget::text(format!("guest-rendered entry card: {entry}")))
    }
}

/// An entry renderer that deliberately FAULTS, so the guest half of X15's failure box has something
/// to prove itself against. Upstream's analog is a renderer that `throw`s
/// (`custom-entry.ts:47-52`); a guest panic lowers to a wasm trap, which the host contains as
/// `RenderOutcome::Failed` instead of the silent `None` it used to report.
///
/// `unreachable!` rather than `panic!`: the workspace denies `clippy::panic`, and the trap is
/// identical either way.
struct FaultingEntryRenderer;
impl MessageRenderer for FaultingEntryRenderer {
    fn render_call(&self, _entry: &Value, _ctx: &crate::Ctx) -> Option<Value> {
        unreachable!("demo_boom: this entry renderer always faults (X15 fixture)")
    }
}

/// Build the demo extension's [`ExtensionApi`]. Pure ergonomic-layer code — also unit-testable on
/// the host target.
pub fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();

    // Permission gate (R-08-010): block any `bash` tool call with a reason.
    api.on_tool_call(|ev, ctx| {
        // EXT-005: `abort`/`shutdown` are BASE-context ops in Pi ("Available in all contexts",
        // extensions/types.ts:339,344) — so an EVENT handler like this gate may call them. Both are
        // deliberately NOT command-tier-gated host-side.
        if ev.name == "abortme" {
            match ctx.abort() {
                Ok(()) => ctx.ui().notify("demo: abort requested from a tool_call handler"),
                Err(e) => ctx.ui().notify(&format!("demo: abort rejected: {e}")),
            }
            return Outcome::block("aborting the run");
        }
        if ev.name == "shutdownme" {
            match ctx.shutdown() {
                Ok(()) => ctx.ui().notify("demo: shutdown requested from a tool_call handler"),
                Err(e) => ctx.ui().notify(&format!("demo: shutdown rejected: {e}")),
            }
            return Outcome::block("shutting down");
        }
        if ev.name == "bash" {
            // An `error`-severity notification (Pi `notify(msg, "error")`, types.ts:135).
            ctx.ui().notify_with("permission-gate: blocked a bash call", NotifyKind::Error);
            Outcome::block("bash is disabled by the demo extension")
        } else {
            Outcome::noop()
        }
    });

    // EXT-028: `tool_result` carries the tool's OWN reported usage (Pi `ToolResultEventBase.usage`,
    // extensions/types.ts:919-921), which `f777e44` re-signed onto the `events.on-tool-result`
    // EXPORT as a trailing `usage-json: option<string>`. Nothing registered this handler until now,
    // so the widened export had never crossed a real wasm boundary.
    //
    // The handler proves BOTH directions in one round trip: it notifies the usage it RECEIVED (so a
    // host test can see the inbound arg arrived, and see `none` for the ordinary tool that reports
    // no usage), and for `usage_probe` it echoes that very payload back with `output` doubled —
    // Pi `ToolResultEventResult.usage` (types.ts:1085-1090) REPLACES the recorded usage wholesale.
    // Deriving the patch from the received value is what makes it a read proof rather than a
    // constant the guest could have invented.
    api.on_tool_result(|ev, ctx| {
        let received =
            ev.usage.as_ref().map(|u| u.to_string()).unwrap_or_else(|| "none".to_string());
        ctx.ui().notify(&format!("demo: tool_result {} usage={received}", ev.name));
        match ev.usage.clone() {
            Some(mut usage) if ev.name == "usage_probe" => {
                let doubled = usage.get("output").and_then(|v| v.as_u64()).unwrap_or(0) * 2;
                if let Some(obj) = usage.as_object_mut() {
                    obj.insert("output".into(), json!(doubled));
                }
                Outcome::mutate(json!({ "usage": usage }))
            }
            _ => Outcome::noop(),
        }
    });

    // Notify hook: announce activation when a run starts.
    api.on_agent_start(|ctx| {
        ctx.ui().notify("demo extension active");
        // GAP-11: set_thinking_level from an EVENT handler. Pi allows this from any handler
        // (loader.ts:352-354 / runner.ts:330, no tier gate) and it TAKES EFFECT. cyrup now QUEUES the
        // op and applies it at the store-free turn-boundary drain (never rejects it), so the guest
        // observes `Ok(())` and the level changes on the subsequent turn — matching Pi.
        match ctx.models().set_thinking_level("minimal") {
            Ok(()) => ctx.ui().notify("thinking level set from agent_start"),
            Err(e) => ctx.ui().notify(&format!("thinking level rejected: {e}")),
        }
    });

    // SEAM-005: `agent_settled` (Pi `on("agent_settled", …)`, extensions/types.ts:1225). Fires ONCE
    // per run, after every automatic retry / post-run compaction / queued continuation — unlike
    // `agent_start`/`agent_end` above, which fire once per agent loop. The distinct notification
    // text is what lets a host test prove the GUEST's handler ran across the WIT boundary.
    api.on_agent_settled(|ctx| {
        ctx.ui().notify("demo: agent settled");
    });

    // EXT-004: register a tool from a LIVE handler, after `init` — Pi's
    // `examples/extensions/dynamic-tools.ts` registers from exactly this event, and
    // `extensions/loader.ts:249-256` follows every `registerTool()` with `runtime.refreshTools()`.
    // The host must re-materialize the descriptor into an EXECUTABLE handle; before EXT-004 the
    // wrapping happened once right after `init`, so this tool could never be called.
    api.on_session_start(|ev, ctx| {
        ctx.ui().notify(&format!("demo: session_start ({})", ev.reason));
        ctx.register_tool(
            ToolDescriptor::new(
                "demo_late",
                json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }),
            )
            .description("Registered after init, from a session_start handler (demo)."),
            |call: ToolCall| {
                let text = call.params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Ok(ToolOutput::text(format!("late: {text}")))
            },
        );
    });

    // EXT-005: report the BASE-context state accessors (Pi types.ts:329-346) back as text so a test
    // can prove the guest reads the HOST's live answer rather than a hard-coded default.
    api.register_command(
        "ctxstate",
        CommandDescriptor::new("Report the base-context state accessors (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let base = ctx.ctx();
            Ok(Some(format!(
                "idle={} pending={} trusted={} prompt={}",
                base.is_idle(),
                base.has_pending_messages(),
                base.is_project_trusted(),
                base.system_prompt(),
            )))
        },
    );

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
    api.on_message_end(|ev, ctx| {
        let role = ev.message.get("role").and_then(|r| r.as_str());
        // cyrup serializes `UserMessage.content` as the array form `[{type:"text",text}]` (Pi's real
        // entry points always build the array; the bare-string shorthand is only read-tolerated,
        // never written — cyrup-core message.rs). Read the first text block's `text`, falling back to
        // the bare-string shorthand so this stays 1:1 with Pi's `string | Content[]` tolerance.
        let content = ev.message.get("content");
        let text = content.and_then(|c| c.as_str()).or_else(|| {
            content
                .and_then(|c| c.as_array())
                .and_then(|blocks| blocks.first())
                .and_then(|first| first.get("text"))
                .and_then(|t| t.as_str())
        });
        if role == Some("user") && text == Some("redact me") {
            Outcome::replace_message(json!({ "role": "user", "content": "[redacted]", "timestamp": 0 }))
        } else if role == Some("user") && text == Some("gap11switch") {
            // GAP-11 INDEPENDENT VERIFICATION: call BOTH set_model and set_thinking_level from this
            // EVENT handler (on_message_end fires DURING the run, while the wasm store is held). Pi
            // allows both from any handler and they take effect (loader.ts:342-354). The host must
            // QUEUE each op (never reject/drop it) and apply it at the store-free turn-boundary drain,
            // so the SUBSEQUENT turn uses the new model/level.
            //
            // The ergonomic SDK exposes `set_model` only on `CommandCtx`; to exercise the HOST's
            // event-tier `set_model` import we call the raw WIT binding directly (WIT `set-model`
            // returns void — fire-and-forget — so the guest observes the EFFECT, not a return value).
            // The host parses `model-json` with serde_json (live.rs `set_model`), so pass valid JSON
            // — the object form `{provider, model}` (parse_model_ref accepts it), exactly what the
            // SDK's `CommandCtx::set_model` encodes for a typed ref.
            #[cfg(target_arch = "wasm32")]
            crate::guest::bindings::cyrup::ext::models::set_model(
                r#"{"provider":"faux","model":"faux-2"}"#,
            );
            ctx.ui().notify("gap11: set_model called from message_end");
            match ctx.models().set_thinking_level("high") {
                Ok(()) => ctx.ui().notify("gap11: set_thinking_level ok from message_end"),
                Err(e) => ctx.ui().notify(&format!("gap11: set_thinking_level err from message_end: {e}")),
            }
            Outcome::noop()
        } else {
            Outcome::noop()
        }
    });

    // GAP-11 RE-ENTRANCY PROOF: subscribe to `thinking_level_select` (and `model_select`). When an
    // event-tier `set_thinking_level` is applied at the store-free turn-boundary drain, the host
    // RE-EMITS `thinking_level_select` back to the guest (agent-session.ts:1560-1567) — a FRESH
    // top-level guest call that re-enters the single-instance wasm store. This is EXACTLY the re-entry
    // the old command-tier gate guarded against: if the drain point were not store-free, this re-entry
    // would deadlock/hang. The handler notifies so a test can prove the re-emit reached the guest.
    api.on_thinking_level_select(|ev, ctx| {
        ctx.ui().notify(&format!("tls re-emit reached guest: {}", ev.level));
    });
    api.on_model_select(|ev, ctx| {
        ctx.ui().notify(&format!("model_select re-emit reached guest: {}", ev.model));
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
        .description("Echo the input text back (demo tool).")
        // EXT-006: this tool draws its OWN call/result rows (Pi `renderCall`/`renderResult`,
        // types.ts:472-481). The matching renderer is registered under the tool NAME below.
        .has_renderer(true),
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
            // Carries Pi's `CompactOptions.customInstructions` (types.ts:296-300) so the host-side
            // op is proved to arrive with its payload, not just to arrive.
            let _ = ctx.compact_with(&crate::CompactOptions {
                custom_instructions: Some("demo: keep the greeting".into()),
            });
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

    // Report the host's run mode + dialog capability back out as command output (Pi `ctx.mode` /
    // `ctx.hasUI`, extensions/types.ts:311,313). A guest guards terminal-only UI on these, so the
    // pair has to reflect the mode the HOST was actually configured with — which is what
    // `crates/cyrup-ext/tests/guest_host_mode.rs` reads this command's output to prove.
    api.register_command(
        "hostmode",
        CommandDescriptor::new("Report ctx.mode + ctx.hasUI (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let base = ctx.ctx();
            Ok(Some(format!("mode={} has_ui={}", base.mode().as_str(), base.has_ui())))
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

    // Two commands exercising the capability-scoped `ext-fs` grant (EXT-054/EXT-055): `/fswrite
    // <name> <text>` and `/fsread <name>`, both addressing paths relative to the project root, both
    // reporting the host's verbatim refusal when the manifest's `capabilities.fs` does not cover the
    // path. They are the fs analog of `execdemo`/`httpdemo`: the same "granted ⇒ real effect,
    // ungranted ⇒ typed denial" seam, in the one capability that had NO guest-reachable surface at
    // all until the SDK gained `Ctx::read_file`/`Ctx::write_file`.
    api.register_command(
        "fswrite",
        CommandDescriptor::new("Write `<name> <text>` through the ext-fs capability (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let (name, text) = args.trim().split_once(' ').unwrap_or((args.trim(), ""));
            match ctx.ctx().write_file(name, text.as_bytes()) {
                Ok(()) => {
                    ctx.ui().notify(&format!("fs wrote: {name}"));
                    Ok(Some(format!("fs wrote {name}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("fs write denied: {e}"));
                    Ok(Some(format!("fs write denied: {e}")))
                }
            }
        },
    );
    api.register_command(
        "fsread",
        CommandDescriptor::new("Read `<name>` through the ext-fs capability (demo)."),
        |args: &str, ctx: &crate::CommandCtx| match ctx.ctx().read_file(args.trim()) {
            Ok(bytes) => {
                let body = String::from_utf8_lossy(&bytes).into_owned();
                ctx.ui().notify(&format!("fs read: {body}"));
                Ok(Some(format!("fs read {body}")))
            }
            Err(e) => {
                ctx.ui().notify(&format!("fs read denied: {e}"));
                Ok(Some(format!("fs read denied: {e}")))
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

    // A COMMAND-tier counterpart to the event-tier set_thinking_level call above (parity gap #12):
    // at command tier the deadlock rule permits it, so the host APPLIES it (recorded as a
    // `SetThinkingLevel` control op) and the guest observes `Ok(())`.
    api.register_command(
        "thinkdemo",
        CommandDescriptor::new("Set the thinking level from a command (demo, parity gap #12)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let level = if args.trim().is_empty() { "high" } else { args.trim() };
            match ctx.models().set_thinking_level(level) {
                Ok(()) => {
                    ctx.ui().notify(&format!("thinking level set: {level}"));
                    Ok(Some(format!("thinking level set: {level}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("thinking level rejected: {e}"));
                    Ok(Some(format!("thinking level rejected: {e}")))
                }
            }
        },
    );

    // GAP-11 command-tier proof: a guest COMMAND calling set_model must still apply after the
    // event-tier gate was removed. Command tier was always permitted (R-08-008); this pins that the
    // fix did not regress it.
    api.register_command(
        "gap11setmodel",
        CommandDescriptor::new("Set the model from a command (GAP-11 command-tier proof)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let target = args.trim();
            match ctx.set_model(target) {
                Ok(()) => {
                    ctx.ui().notify(&format!("model set: {target}"));
                    Ok(Some(format!("model set: {target}")))
                }
                Err(e) => {
                    ctx.ui().notify(&format!("model rejected: {e}"));
                    Ok(Some(format!("model rejected: {e}")))
                }
            }
        },
    );

    // A custom-MESSAGE renderer (Pi `registerMessageRenderer(customType, renderer)`,
    // types.ts:1284) keyed by a custom message type.
    api.register_message_renderer("demo", DemoRenderer);
    // EXT-006: the per-TOOL renderer for `demo_echo` (whose descriptor declares `has_renderer`).
    // Keyed by the TOOL NAME — that is how the host routes a tool row back to the guest that draws
    // it (Pi `getCallRenderer`/`getResultRenderer`, tool-execution.ts:81-112).
    api.register_message_renderer("demo_echo", DemoToolRenderer);
    // X15 — the custom-ENTRY surface (Pi `registerEntryRenderer(customType, renderer)`,
    // types.ts:1295). `demo_card` draws; `demo_boom` deliberately FAULTS, which is the only way to
    // exercise the guest half of the failure box (`custom-entry.ts:47-52`) end to end. A guest
    // panic is a wasm trap, which the host reports as `RenderOutcome::Failed`.
    api.register_entry_renderer("demo_card", DemoEntryRenderer);
    api.register_entry_renderer("demo_boom", FaultingEntryRenderer);

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
                cost: crate::ModelCost {
                    input: 3.0,
                    output: 15.0,
                    cache_read: 0.3,
                    cache_write: 3.75,
                    tiers: None,
                },
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
            let ok = ctx.ui().confirm_with("proceed?", "unreachable body", &DialogOptions::signal("demo-dialog"));
            Ok(Some(format!("confirmed: {ok}")))
        },
    );

    // A command exercising `confirm`'s `message` body (Pi `confirm(title, message, opts)`,
    // rpc-types.ts:232; L4 review §2.6): threads live through `ctx.rs` -> WIT `confirm` -> the host
    // backend, distinct from the prompt/title (not dismissed, so the backend actually sees it).
    api.register_command(
        "confirmdemo",
        CommandDescriptor::new("Open a confirm dialog with a message body (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let ok = ctx.ui().confirm_with(
                "proceed?",
                "this is the message body, distinct from the title",
                &DialogOptions::default(),
            );
            ctx.ui().notify(&format!("confirmed: {ok}"));
            // Visible LIVE proof the guest received the REAL answer, not just that the dialog closed:
            // a second dialog whose PROMPT embeds the value just received — no dependency on `notify`/
            // `append_entry`'s host-side-only recording (a command handler's plain return value is
            // itself discarded too, Pi-faithfully, `session.rs` `try_execute_wasm_command`).
            let _ = ctx.ui().confirm(&format!("you answered: {ok}"));
            Ok(Some(format!("confirmed: {ok}")))
        },
    );

    // A command exercising `input`'s `placeholder` (Pi `input(title, placeholder, opts)`,
    // rpc-types.ts:233-240; L4 review §2.7): threads live through `ctx.rs` -> WIT `input` -> the host
    // backend, distinct from the prompt/title.
    api.register_command(
        "inputdemo",
        CommandDescriptor::new("Open an input dialog with a placeholder (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let answer =
                ctx.ui().input_with("name?", Some("e.g. Ada Lovelace"), &DialogOptions::default());
            ctx.ui().notify(&format!("input: {answer:?}"));
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received text in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&format!("you typed: {answer:?}"));
            Ok(Some(format!("input: {answer:?}")))
        },
    );

    // A command exercising `select` (Pi `select(title, options, opts): Promise<string|undefined>`,
    // types.ts:127; L4 review §2.1 interactive-TUI wiring): offers three options and surfaces the
    // chosen STRING (or "none" if dismissed).
    api.register_command(
        "selectdemo",
        CommandDescriptor::new("Open a select dialog over three options (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let chosen = ctx.ui().select("pick one", &["alpha", "beta", "gamma"]);
            let text = format!("selected: {}", chosen.as_deref().unwrap_or("none"));
            ctx.ui().notify(&text);
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received choice in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&format!("you picked: {}", chosen.as_deref().unwrap_or("none")));
            Ok(Some(text))
        },
    );

    // A command exercising `editor` (Pi's external-editor dialog; L4 review §2.1): seeds the buffer
    // with fixed text and surfaces the edited result (or "none" if cancelled/non-zero exit).
    api.register_command(
        "editordemo",
        CommandDescriptor::new("Open an external-editor dialog seeded with fixed text (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let edited = ctx.ui().editor("edit demo", "seed text from the guest");
            let text = format!("edited: {}", edited.as_deref().unwrap_or("none"));
            ctx.ui().notify(&text);
            // Visible LIVE proof (see `confirmdemo`'s comment): echo the received text in a follow-up
            // dialog's prompt.
            let _ = ctx.ui().confirm(&text);
            Ok(Some(text))
        },
    );

    // A keyboard shortcut (R-08-017) whose handler itself opens a SYNCHRONOUS `ui.confirm` dialog (L4
    // review §2.1): proves the run loop's `AppAction::ExtensionShortcut` no longer self-deadlocks now
    // that it is spawned rather than awaited inline (a shortcut handler blocking on `ui_roundtrip`
    // while the SAME task also owns `ui_rx` would otherwise hang forever).
    api.register_shortcut("ctrl+t", "Open a confirm dialog from a shortcut (demo)", |ctx: &crate::Ctx| {
        let ok = ctx.ui().confirm("shortcut confirm — proceed?");
        let text = format!("shortcut confirmed: {ok}");
        ctx.ui().notify(&text);
        // Visible LIVE proof (see `confirmdemo`'s comment) — also proves TWO SEQUENTIAL synchronous
        // `ui.*` round trips from the SAME spawned shortcut task both reach the run loop correctly.
        let _ = ctx.ui().confirm(&text);
        Ok(())
    });

    // A CLI-overridable flag (Pi `registerFlag`, loader.ts:257-266) + a command that READS it back
    // via `getFlag` (loader.ts:280-284). Registered with a static default; a `--demo-flag=<value>`
    // token captured on the command line (Pi `unknownFlags` → `applyExtensionFlagValues`,
    // agent-session-services.ts:84-113) must override that default so the value the guest reads here
    // is the CLI-supplied one, not the registered default (gap-08 §5.6).
    api.register_flag(
        "demo-flag",
        FlagSpec { r#type: "string".into(), default: Some(json!("off")), description: String::new() },
    );
    api.register_command(
        "flagdemo",
        CommandDescriptor::new("Report the resolved value of --demo-flag (demo, getFlag override)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let value = ctx
                .ctx()
                .get_flag("demo-flag")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            ctx.ui().notify(&format!("flag demo-flag = {value}"));
            Ok(Some(format!("flag demo-flag = {value}")))
        },
    );

    // Inter-extension event bus (Pi `pi.events.emit`/`pi.events.on`, event-bus.ts:12-32; gap-08 §5.3).
    // `/buspub <msg>` EMITS on the shared bus; the `on_bus` handler below (registered by EVERY loaded
    // instance of this extension) RECEIVES a topic another instance emitted and surfaces it via
    // `notify` — the observable proof a published event reached a subscribed handler cross-extension.
    api.on_bus("demo:bus", |topic: &str, payload: Value, ctx: &crate::Ctx| {
        let msg = payload.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        ctx.notify(&format!("bus recv {topic}: {msg}"));
    });
    api.register_command(
        "buspub",
        CommandDescriptor::new("Emit a message on the inter-extension event bus (demo). args: <msg>"),
        |args: &str, ctx: &crate::CommandCtx| {
            let msg = args.trim();
            ctx.ctx().emit("demo:bus", json!({ "msg": msg }));
            Ok(Some(format!("emitted demo:bus: {msg}")))
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
