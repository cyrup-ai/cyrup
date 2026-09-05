//! The demo extension's EVENT handlers — its subscriptions to the host's event seams. (The
//! `demo:bus` subscription is NOT one of these: an inter-extension bus topic is not a host event,
//! and it lives in [`super::wiring`] with the rest of the bus surface.)
//!
//! Installed first by [`super::build`], so these are the demo's `tool_call`, `tool_result`,
//! `agent_start`, `agent_settled`, `session_start`, `before_agent_start`, `input`, `message_end`,
//! `thinking_level_select`, `model_select`, `before_provider_request`, `user_bash`,
//! `session_before_compact` and `session_before_tree` subscriptions.

use crate::{ExtensionApi, NotifyKind, Outcome, ToolCall, ToolDescriptor, ToolOutput};
use serde_json::json;

pub(super) fn install(api: &mut ExtensionApi) {
    // Permission gate (R-08-010): block any `bash` tool call with a reason.
    api.on_tool_call(|ev, ctx| {
        // EXT-005: `abort`/`shutdown` are BASE-context ops in Pi ("Available in all contexts",
        // extensions/types.ts:339,344) — so an EVENT handler like this gate may call them. Both are
        // deliberately NOT command-tier-gated host-side.
        if ev.name == "abortme" {
            match ctx.abort() {
                Ok(()) => ctx
                    .ui()
                    .notify("demo: abort requested from a tool_call handler"),
                Err(e) => ctx.ui().notify(&format!("demo: abort rejected: {e}")),
            }
            return Outcome::block("aborting the run");
        }
        if ev.name == "shutdownme" {
            match ctx.shutdown() {
                Ok(()) => ctx
                    .ui()
                    .notify("demo: shutdown requested from a tool_call handler"),
                Err(e) => ctx.ui().notify(&format!("demo: shutdown rejected: {e}")),
            }
            return Outcome::block("shutting down");
        }
        if ev.name == "bash" {
            // An `error`-severity notification (Pi `notify(msg, "error")`, types.ts:142 @v0.83.0).
            ctx.ui()
                .notify_with("permission-gate: blocked a bash call", NotifyKind::Error);
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
        let received = ev
            .usage
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "none".to_string());
        ctx.ui()
            .notify(&format!("demo: tool_result {} usage={received}", ev.name));
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
        // (loader.ts:369-372 / runner.ts:336, no tier gate) and it TAKES EFFECT. cyrup now QUEUES the
        // op and applies it at the store-free turn-boundary drain (never rejects it), so the guest
        // observes `Ok(())` and the level changes on the subsequent turn — matching Pi.
        match ctx.models().set_thinking_level("minimal") {
            Ok(()) => ctx.ui().notify("thinking level set from agent_start"),
            Err(e) => ctx.ui().notify(&format!("thinking level rejected: {e}")),
        }
    });

    // SEAM-005: `agent_settled` (Pi `on("agent_settled", …)`, `extensions/types.ts:1217` @v0.83.0; EXT-036 corrected `:1225`, which is `tool_execution_end`). Fires ONCE
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
        ctx.ui()
            .notify(&format!("demo: session_start ({})", ev.reason));
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
                let text = call
                    .params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(ToolOutput::text(format!("late: {text}")))
            },
        );
    });

    // --- the previously-dead mutating seams, now driven by the assembled host (gap-08 #1-#5) ---

    // before_agent_start (gap-08 #1): when the prompt is exactly "go", inject a marker message AND
    // replace the system prompt. Injected messages accumulate across handlers (Pi runner.ts:1014).
    api.on_before_agent_start(|ev, _ctx| {
        if ev.prompt == "go" {
            Outcome::before_agent_start(crate::BeforeAgentStartResult {
                message: Some(
                    json!({ "role": "user", "content": "injected by demo", "timestamp": 0 }),
                ),
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
            Outcome::replace_message(
                json!({ "role": "user", "content": "[redacted]", "timestamp": 0 }),
            )
        } else if role == Some("user") && text == Some("gap11switch") {
            // GAP-11 INDEPENDENT VERIFICATION: call BOTH set_model and set_thinking_level from this
            // EVENT handler (on_message_end fires DURING the run, while the wasm store is held). Pi
            // allows both from any handler and they take effect (loader.ts:342-354). The host must
            // QUEUE each op (never reject/drop it) and apply it at the store-free turn-boundary drain,
            // so the SUBSEQUENT turn uses the new model/level.
            //
            // `Models::set_model` is callable from any tier (EXT-074 / GAP-11), so the event-tier
            // call goes through the typed SDK wrapper, which encodes the ref with `impl Serialize`
            // for the host's serde_json parse (live.rs `set_model`). The WIT import returns void
            // — fire-and-forget — so `Ok(())` means only that the op reached the host; the guest
            // observes the EFFECT on the subsequent turn.
            let _ = ctx
                .models()
                .set_model(json!({ "provider": "faux", "model": "faux-2" }));
            ctx.ui().notify("gap11: set_model called from message_end");
            match ctx.models().set_thinking_level("high") {
                Ok(()) => ctx
                    .ui()
                    .notify("gap11: set_thinking_level ok from message_end"),
                Err(e) => ctx.ui().notify(&format!(
                    "gap11: set_thinking_level err from message_end: {e}"
                )),
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
        ctx.ui()
            .notify(&format!("tls re-emit reached guest: {}", ev.level));
    });
    api.on_model_select(|ev, ctx| {
        ctx.ui()
            .notify(&format!("model_select re-emit reached guest: {}", ev.model));
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

    // user_bash (gap-08 #5): block a destructive `!rm -rf` invocation; REDIRECT a `remote:` one to
    // this extension's own bash backend (DRIFT-004 — pi's `UserBashEventResult.operations`, the
    // shape every shipped upstream example uses: `examples/extensions/ssh.ts:203-206` returns
    // `{ operations }` and nothing else); otherwise proceed.
    api.on_user_bash(|ev, _ctx| {
        if ev.command.contains("rm -rf") {
            Outcome::block("user_bash blocked by demo")
        } else if ev.command.starts_with("remote:") {
            Outcome::handled(json!({ "operations": true }))
        } else {
            Outcome::noop()
        }
    });

    // The backend the handler above redirects to (DRIFT-004). Declared once at `init`
    // (`registration.register-bash-operations`) and reached per command through the
    // `bash-operations-exec` export: pi `BashOperations.exec(command, cwd, {onData, signal, …})`,
    // `core/tools/bash.ts:71-80` @v0.84.4. It streams through `write` (pi's `onData`) rather than
    // returning its output, because upstream's backend has no return channel for output at all —
    // only the exit code — and stops early when the host cancels (pi's `signal.aborted`).
    api.register_bash_operations(|cmd: &crate::BashCommand| {
        if cmd.is_cancelled() {
            return Ok(None); // pi `exitCode: null` — killed before it started.
        }
        if cmd.command.contains("boom") {
            // pi's `throw`: a backend FAILURE, which the host must not report as a command that
            // ran and produced nothing (`core/bash-executor.ts:154`).
            return Err("demo backend refused to run".to_string());
        }
        cmd.write(format!("[demo-backend] {} in {}\n", cmd.command, cmd.cwd).as_bytes());
        // Prove the VALUE half of pi's options bag crossed too (`timeout?`, `env?`).
        cmd.write(
            format!(
                "[demo-backend] env={} timeout={:?}\n",
                cmd.env.len(),
                cmd.timeout_ms
            )
            .as_bytes(),
        );
        Ok(Some(0))
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
        let target = ev
            .preparation
            .get("targetId")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        Outcome::tree_override(crate::SessionBeforeTreeResult {
            label: Some(format!("demo-tree-label[{target}]")),
            ..Default::default()
        })
    });
}
