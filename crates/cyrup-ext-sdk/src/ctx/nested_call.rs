//! The `host-tool.execute-tool` / `callable-tools` WIT imports: pi's `ExtensionToolContext.executeTool`
//! and `.tools` (`core/extensions/types.ts:367-395` @v1.0.1), on the [`ToolCall`] a guest tool's
//! `execute` receives — the call whose id they are bound to.

use serde::Serialize;
use serde_json::Value;

use super::ToolCall;

/// Why [`ToolCall::execute_tool`] or [`ToolCall::tools`] could not be served at all. A tool that
/// FAILED is never one of these: it comes back as a [`ToolOutcome`] whose
/// [`ToolOutcome::is_error`] is set (pi: *"Never rejects for tool failures"*,
/// `extensions/types.ts:391-393` @v1.0.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NestedCallError {
    /// The arguments could not be encoded as JSON.
    Arguments(String),
    /// The host refused the call itself — `call-id` is not the call in flight or no session is
    /// attached. The text is the host's.
    Host(String),
    /// The host's answer is not the JSON shape `world.wit` promises.
    Answer(String),
}

impl std::fmt::Display for NestedCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arguments(e) => write!(f, "arguments are not JSON: {e}"),
            Self::Host(e) => f.write_str(e),
            Self::Answer(e) => write!(f, "the host's answer is not what world.wit promises: {e}"),
        }
    }
}

impl std::error::Error for NestedCallError {}

/// The options of [`ToolCall::execute_tool`] — pi `ExecuteToolOptions`
/// (`extensions/types.ts:368-373` @v1.0.4).
///
/// A guest suspended inside the call can neither run a closure nor fire an abort, so both of pi's
/// options cross in a different shape. `on_update` is invoked once per partial result AFTER the
/// call settles, in order (the same results reach `tool_execution_update` events live). Pi's
/// `signal` is the named signal [`Self::signal_id`] — the id namespace
/// [`crate::ctx::Ui::abort_signal`] writes, read once when the call starts, so only pi's "already
/// aborted" branch is reachable — and [`Self::timeout_ms`], the one mid-call abort a guest can ask
/// the host for. The nested call is always also cancelled with the calling tool.
#[derive(Default)]
pub struct ExecuteToolOptions {
    /// Receives each partial result of the nested tool, as pi's `AgentToolResult` JSON.
    pub on_update: Option<Box<dyn FnMut(Value)>>,
    /// A named signal that cancels the call when it was aborted before the call started. Set with
    /// [`Self::signal_id`].
    pub signal_id: Option<String>,
    /// Cancel the call this many milliseconds after it starts (pi: `AbortSignal.timeout(ms)` as
    /// `signal`). Set with [`Self::timeout_ms`].
    pub timeout_ms: Option<u32>,
}

impl ExecuteToolOptions {
    /// Cancel the call when the named signal was already aborted (builder-style).
    #[must_use]
    pub fn signal_id(mut self, id: impl Into<String>) -> Self {
        self.signal_id = Some(id.into());
        self
    }

    /// Cancel the call after `ms` milliseconds (builder-style).
    #[must_use]
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
}

/// The settled result of a nested call — pi `AgentToolResult` as carried by
/// `AgentToolCallOutcome.result` (`packages/agent/src/types.ts` @v1.0.1).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcomeResult {
    /// The content blocks the model would see.
    #[serde(default)]
    pub content: Value,
    /// The tool's own details, absent when it had none.
    #[serde(default)]
    pub details: Option<Value>,
    /// Usage the tool's execution reported.
    #[serde(default)]
    pub usage: Option<Value>,
    /// The machine-readable result of a tool that declares an output schema.
    #[serde(default)]
    pub structured_content: Option<Value>,
    /// The TOOL's own failure flag; the verdict is [`ToolOutcome::is_error`].
    #[serde(default)]
    pub is_error: bool,
    /// The tool's early-termination hint, when it gave one.
    #[serde(default)]
    pub terminate: Option<bool>,
    /// Names of tools this result introduced.
    #[serde(default)]
    pub added_tool_names: Vec<String>,
}

impl ToolOutcomeResult {
    /// The text blocks of [`Self::content`], joined with newlines.
    pub fn text(&self) -> String {
        self.content
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
}

/// What a nested call came back as — pi `AgentToolCallOutcome`
/// (`packages/agent/src/types.ts:449-453` @v1.0.1).
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    /// The call as it was issued; its `id` is `<calling id>/<n>`.
    pub tool_call: Value,
    /// The settled result.
    pub result: ToolOutcomeResult,
    /// The verdict: `true` for every failure class — unknown tool, a tool that is not callable, a
    /// validation failure, a blocked call, a tool that failed.
    pub is_error: bool,
}

/// One tool a nested call can reach — the parts of pi's `AgentTool` that
/// `ExtensionToolContext.tools` (`extensions/types.ts:385` @v1.0.1) is read for.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallableTool {
    /// The name [`ToolCall::execute_tool`] takes.
    pub name: String,
    /// The tool's display label.
    #[serde(default)]
    pub label: Option<String>,
    /// What the tool does.
    #[serde(default)]
    pub description: String,
    /// JSON Schema of its arguments.
    #[serde(default)]
    pub parameters: Value,
    /// JSON Schema of its `structuredContent`, for a tool that declares one.
    #[serde(default)]
    pub output_schema: Option<Value>,
    /// Pi's exposure literal: `direct`, `model-only`, `codemode`, `deferred` or `hidden`.
    #[serde(default)]
    pub exposure: String,
    /// The group the tool belongs to.
    #[serde(default)]
    pub namespace: Option<ToolNamespaceInfo>,
}

/// A tool's group (pi `ToolNamespace`, `extensions/types.ts:527` @v1.0.1) as
/// [`CallableTool::namespace`] reports it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ToolNamespaceInfo {
    /// For example `mcp__docs`.
    pub name: String,
    /// The short summary shown once with the group.
    #[serde(default)]
    pub description: Option<String>,
    /// Longer usage guidance.
    #[serde(default)]
    pub instructions: Option<String>,
}

impl ToolCall {
    /// Run another tool from inside this tool's `execute` — pi `ctx.executeTool(name, args,
    /// options?)` (`extensions/types.ts:394` @v1.0.1), through the same validation, hooks and
    /// permission checks as a model-issued call.
    ///
    /// The call gets the id `<call_id>/<n>`; its `tool_call`, `tool_result` and `tool_execution_*`
    /// events carry [`Self::call_id`] as `parent_tool_call_id`; it never appears in the transcript,
    /// and a bounded record of it is kept as `nestedCalls` on this call's result message.
    ///
    /// `Err` only when the call itself could not be made (see [`NestedCallError`]); a tool that
    /// failed is `Ok` with [`ToolOutcome::is_error`] set, and so is a call to a tool of this same
    /// extension: an instance runs one call at a time and this one is suspended in the call, so the
    /// host answers that call with an error outcome that names the cause instead of running it.
    pub fn execute_tool(
        &self,
        name: &str,
        args: impl Serialize,
        options: ExecuteToolOptions,
    ) -> Result<ToolOutcome, NestedCallError> {
        let args_json =
            serde_json::to_string(&args).map_err(|e| NestedCallError::Arguments(e.to_string()))?;
        let ExecuteToolOptions {
            on_update,
            signal_id,
            timeout_ms,
        } = options;
        #[cfg(target_arch = "wasm32")]
        {
            let mut on_update = on_update;
            let (outcome_json, partials) =
                crate::guest::bindings::cyrup::ext::host_tool::execute_tool(
                    &self.call_id,
                    name,
                    &args_json,
                    &crate::guest::bindings::cyrup::ext::host_tool::ExecuteOptions {
                        collect_updates: on_update.is_some(),
                        signal_id,
                        timeout_ms,
                    },
                )
                .map_err(NestedCallError::Host)?;
            if let Some(on_update) = on_update.as_mut() {
                for partial in partials {
                    on_update(serde_json::from_str(&partial).unwrap_or(Value::Null));
                }
            }
            return serde_json::from_str(&outcome_json)
                .map_err(|e| NestedCallError::Answer(e.to_string()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, args_json, on_update, signal_id, timeout_ms);
            Err(NestedCallError::Host(
                "execute_tool unavailable on host target".into(),
            ))
        }
    }

    /// The tools [`Self::execute_tool`] can call — pi `ctx.tools`
    /// (`extensions/types.ts:385` @v1.0.1).
    pub fn tools(&self) -> Result<Vec<CallableTool>, NestedCallError> {
        #[cfg(target_arch = "wasm32")]
        {
            let json = crate::guest::bindings::cyrup::ext::host_tool::callable_tools(&self.call_id)
                .map_err(NestedCallError::Host)?;
            return serde_json::from_str(&json).map_err(|e| NestedCallError::Answer(e.to_string()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        Err(NestedCallError::Host(
            "tools unavailable on host target".into(),
        ))
    }
}
