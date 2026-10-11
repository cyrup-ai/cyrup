//! The demo extension's guest TOOLS: `demo_echo` (streams a partial-output chunk and draws its own
//! rows), `signal_probe` (reports the cancellation signal's state), `nested_probe` (calls another
//! tool through `ToolCall::execute_tool`), `structured_demo` (declares an output schema and
//! annotations, and returns structured content), `failing_demo` (reports a failure without
//! throwing) and `loadout_demo` (supplies a `prepareLoadout` hook).
//!
//! `demo_late` is NOT here — it is registered from a live `session_start` handler in
//! [`super::hooks`], which is the point of that demo.

use crate::{
    ExecuteToolOptions, ExtensionApi, ToolAnnotations, ToolCall, ToolDescriptor, ToolExec,
    ToolLoadout, ToolLoadoutChanges, ToolOutput,
};
use serde_json::json;

pub(super) fn install(api: &mut ExtensionApi) {
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
        // types.ts:489-497). The matching renderer is registered under the tool NAME below.
        .has_renderer(true),
        |call: ToolCall| {
            let text = call
                .params
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Stream a partial-output chunk (Pi onUpdate).
            call.emit_update(json!({ "content": [{ "type": "text", "text": "working..." }] }));
            Ok(ToolOutput::text(format!("echo: {text}")))
        },
    );

    // Pi `ctx.executeTool(name, args, options?)` and `ctx.tools` (`extensions/types.ts:383-395`
    // @v1.0.1) from a guest tool: runs the tool named by `tool` with `args` through the session and
    // reports what came back, or lists the callable tools when `list` is true. The nested call's
    // events reach this extension's own handlers with `parent_tool_call_id` set — except to this
    // instance, which is executing this very call — so the report is built from the outcome alone.
    api.register_tool(
        ToolDescriptor::new(
            "nested_probe",
            json!({
                "type": "object",
                "properties": {
                    "tool": { "type": "string" },
                    "args": { "type": "object" },
                    "list": { "type": "boolean" },
                    "collect": { "type": "boolean" },
                    "signal_id": { "type": "string" },
                    "abort_first": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                }
            }),
        )
        .description("Call another tool through the session (demo ctx.executeTool)."),
        |call: ToolCall| {
            if call.params.get("list").and_then(|v| v.as_bool()) == Some(true) {
                return Ok(match call.tools() {
                    Ok(tools) => ToolOutput::text(format!(
                        "tools: {}",
                        tools
                            .iter()
                            .map(|t| format!("{}[{}]", t.name, t.exposure))
                            .collect::<Vec<_>>()
                            .join(",")
                    )),
                    Err(e) => ToolOutput::text(format!("refused: {e}")),
                });
            }
            let tool = call
                .params
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("read")
                .to_string();
            let args = call
                .params
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let collected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let on_update: Option<Box<dyn FnMut(serde_json::Value)>> =
                if call.params.get("collect").and_then(|v| v.as_bool()) == Some(true) {
                    let sink = std::rc::Rc::clone(&collected);
                    Some(Box::new(move |partial| sink.borrow_mut().push(partial)))
                } else {
                    None
                };
            // pi `options.signal`: `abort_first` aborts a named signal before the call, `signal_id`
            // hands it to the call, and `timeout_ms` is the deadline a suspended guest can ask for.
            if let Some(id) = call.params.get("abort_first").and_then(|v| v.as_str()) {
                call.ctx.ui().abort_signal(id);
            }
            let mut options = ExecuteToolOptions {
                on_update,
                ..ExecuteToolOptions::default()
            };
            if let Some(id) = call.params.get("signal_id").and_then(|v| v.as_str()) {
                options = options.signal_id(id);
            }
            if let Some(ms) = call
                .params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .and_then(|ms| u32::try_from(ms).ok())
            {
                options = options.timeout_ms(ms);
            }
            Ok(match call.execute_tool(&tool, args, options) {
                Ok(outcome) => ToolOutput::text(format!(
                    "nested {} error={} partials={} :: {}",
                    outcome
                        .tool_call
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    outcome.is_error,
                    collected.borrow().len(),
                    outcome.result.text()
                )),
                Err(e) => ToolOutput::text(format!("refused: {e}")),
            })
        },
    );

    // A tool that polls its cancellation `signal` (Pi `ToolDefinition.execute` `signal`, sdk gap #1):
    // a long tool would loop and bail when aborted; this demo just reports the current state.
    api.register_tool(
        ToolDescriptor::new(
            "signal_probe",
            json!({ "type": "object", "properties": {} }),
        )
        .description("Report whether the host has requested cancellation (demo signal)."),
        |call: ToolCall| {
            Ok(ToolOutput::text(format!(
                "aborted: {}",
                call.signal().is_aborted()
            )))
        },
    );

    // A tool that declares an output schema and returns structured content (pi
    // `ToolDefinition.outputSchema` / `AgentToolResult.structuredContent`, `extensions/types.ts:592`
    // and `agent/src/types.ts:433` @v1.0.4), with annotations (`extensions/types.ts:603`). A codemode
    // script that calls it receives the structured value, not the text.
    api.register_tool(
        ToolDescriptor::new(
            "structured_demo",
            json!({
                "type": "object",
                "properties": { "n": { "type": "integer" } }
            }),
        )
        .description("Return a structured result (demo outputSchema).")
        .output_schema(json!({
            "type": "object",
            "properties": {
                "doubled": { "type": "integer" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["doubled"]
        }))
        .annotations(
            ToolAnnotations::new()
                .read_only(true)
                .idempotent(true)
                .open_world(false),
        ),
        |call: ToolCall| {
            let n = call.params.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(ToolOutput::text(format!("doubled {n}"))
                .with_structured_content(json!({ "doubled": n * 2, "tags": ["a", "b"] })))
        },
    );

    // A tool that reports a failure WITHOUT throwing (pi `AgentToolResult.isError`,
    // `agent/src/types.ts:440`): its structured content is kept for programmatic callers.
    api.register_tool(
        ToolDescriptor::new(
            "failing_demo",
            json!({ "type": "object", "properties": {} }),
        )
        .description("Report a failure without throwing (demo isError).")
        .output_schema(json!({ "type": "object" })),
        |_call: ToolCall| {
            Ok(ToolOutput::error("it failed on purpose")
                .with_structured_content(json!({ "reason": "on purpose" })))
        },
    );

    // A tool with a `prepareLoadout` hook (pi `ToolDefinition.prepareLoadout`,
    // `extensions/types.ts:617` @v1.0.4): it lists the callable tools in its own description and
    // leaves the declaration of `signal_probe` out of requests when that tool is declared.
    api.register_tool(
        ToolDescriptor::new(
            "loadout_demo",
            json!({ "type": "object", "properties": {} }),
        )
        .description("Orchestrates other tools (demo prepareLoadout).")
        .prepare_loadout(true),
        LoadoutDemo,
    );
}

/// The executor of `loadout_demo`: a tool that does nothing but shape the loadout.
struct LoadoutDemo;

impl ToolExec for LoadoutDemo {
    fn execute(&self, _call: ToolCall) -> Result<ToolOutput, String> {
        Ok(ToolOutput::text("loadout_demo ran"))
    }

    fn prepare_loadout(&self, loadout: &ToolLoadout) -> Option<ToolLoadoutChanges> {
        let callable = loadout
            .callable
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let mut changes = ToolLoadoutChanges::new().describe(
            "loadout_demo",
            format!("Orchestrates other tools. Callable: {callable}."),
        );
        if loadout.declared.iter().any(|t| t.name == "signal_probe") {
            changes = changes.hide("signal_probe");
        }
        Some(changes)
    }
}
