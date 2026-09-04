//! The demo extension's guest TOOLS: `demo_echo` (streams a partial-output chunk and draws its own
//! rows) and `signal_probe` (reports the cancellation signal's state).
//!
//! `demo_late` is NOT here — it is registered from a live `session_start` handler in
//! [`super::hooks`], which is the point of that demo.

use crate::{ExtensionApi, ToolCall, ToolDescriptor, ToolOutput};
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
