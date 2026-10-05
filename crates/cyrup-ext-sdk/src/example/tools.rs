//! The demo extension's guest TOOLS: `demo_echo` (streams a partial-output chunk and draws its own
//! rows), `signal_probe` (reports the cancellation signal's state) and `nested_probe` (calls another
//! tool through `ToolCall::execute_tool`).
//!
//! `demo_late` is NOT here — it is registered from a live `session_start` handler in
//! [`super::hooks`], which is the point of that demo.

use crate::{ExecuteToolOptions, ExtensionApi, ToolCall, ToolDescriptor, ToolOutput};
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
                    "collect": { "type": "boolean" }
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
            Ok(
                match call.execute_tool(&tool, args, ExecuteToolOptions { on_update }) {
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
                },
            )
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
}
