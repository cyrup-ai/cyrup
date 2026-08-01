//! The runtime-facing `Tool` trait (arch-00 §3.4; conformance: func-02 §4.3 / func-03 §11).
//!
//! Defined here in `cyrup-core`; built-in tools implement it in `cyrup-tools` (arch-03), and
//! extension tools implement it in `cyrup-ext` (arch-08). Tools signal failure by returning
//! `Err(ToolError)` (never error text — func-02 R-02-024).

use crate::cancel::CancelToken;
use crate::message::Content;
use crate::ToolCallId;

/// Per-tool execution mode (func-02 R-02-014).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExecMode {
    #[default]
    Parallel,
    Sequential,
}

/// A tool's final result (func-02 §4.3). `details` is app/extension metadata, NOT sent to the model.
#[derive(Clone, Debug, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
    /// Hint to stop the loop after this batch (func-02 §7.7); runtime-only, never persisted.
    pub terminate: bool,
}

/// A streamed progress update (func-02 R-02-023). Mirrors Pi's `AgentToolResult` (the
/// `partialResult` payload, types.ts:350-360): besides `content`/`details` a partial may carry the
/// optional early-termination hint `terminate`, which surfaces on the `tool_execution_update` event
/// (agent-loop.ts:641-653). `None` = the field is absent on the wire (Pi `terminate?: boolean`).
#[derive(Clone, Debug, Default)]
pub struct ToolUpdate {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
    /// Optional early-termination hint carried by the partial result (Pi `AgentToolResult.terminate`,
    /// types.ts:359). `None` omits the field from the emitted update, exactly as Pi omits an
    /// `undefined` `terminate`.
    pub terminate: Option<bool>,
}

/// Sink the runtime hands to a tool to stream progress. The runtime ignores updates after the
/// tool's execution settles (func-02 R-02-023).
pub type ToolUpdateSink = Box<dyn FnMut(ToolUpdate) + Send + 'static>;

/// How a tool's execution row is framed in the UI (Pi `ToolDefinition.renderShell`,
/// extensions/types.ts:448-449: `"default" | "self"`). `Default` = the runtime draws the standard
/// colored shell; `Selfish` = the tool renders its own framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToolRenderKind {
    /// The runtime renders the standard colored shell (Pi `"default"`).
    #[default]
    Default,
    /// The tool renders its own framing (Pi `"self"`).
    SelfRendered,
}

/// Tool failure (arch-03 §8). Re-exported by `cyrup-tools` as its `ToolError`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    /// JSON-Schema-compatible parameter schema (func-01 §10).
    fn parameters(&self) -> &serde_json::Value;
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Parallel
    }

    /// Description shown to the model (Pi `ToolDefinition.description`, extensions/types.ts:441).
    /// Defaulted to `""` so existing impls compile unchanged; per-tool values land in `cyrup-tools`.
    fn description(&self) -> &str {
        ""
    }

    /// Human-readable label for the UI (Pi `ToolDefinition.label`, extensions/types.ts:438-439).
    /// Default `None` = the runtime falls back to the tool `name` (today's behavior).
    fn label(&self) -> Option<&str> {
        None
    }

    /// One-line snippet for the "Available tools" section of the default system prompt (Pi
    /// `ToolDefinition.promptSnippet`, extensions/types.ts:442-443). Default `None` omits the tool
    /// from that section (today's behavior).
    fn prompt_snippet(&self) -> Option<&str> {
        None
    }

    /// Tool-specific guideline bullets for the "Guidelines" section of the default system prompt
    /// (Pi `ToolDefinition.promptGuidelines`, extensions/types.ts:444-446). Per func-03 R-03-039
    /// each string MUST name its tool so it stays meaningful once the tool is disabled. Default
    /// `&[]` contributes nothing (today's behavior).
    fn prompt_guidelines(&self) -> &[&str] {
        &[]
    }

    /// Whether the runtime draws the standard tool shell or the tool renders its own framing (Pi
    /// `ToolDefinition.renderShell`, extensions/types.ts:448-449). Defaulted to
    /// [`ToolRenderKind::Default`] (today's behavior).
    fn render_kind(&self) -> ToolRenderKind {
        ToolRenderKind::Default
    }

    /// Compatibility shim to normalize raw tool-call arguments before schema validation (Pi
    /// `ToolDefinition.prepareArguments`, extensions/types.ts:451-452). Default: identity
    /// passthrough — the arguments are returned unchanged (today's behavior).
    async fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        args
    }

    /// Custom rendering of a tool *call* for the UI (Pi `ToolDefinition.renderCall`,
    /// extensions/types.ts:472-473). The returned string is the rendered representation; `None` =
    /// the runtime uses its standard call framing (today's behavior).
    fn render_call(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Custom rendering of a tool *result* for the UI (Pi `ToolDefinition.renderResult`,
    /// extensions/types.ts:475-481). The returned string is the rendered representation; `None` =
    /// the runtime uses its standard result framing (today's behavior).
    fn render_result(&self, _result: &ToolResult) -> Option<String> {
        None
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A tool that overrides only the required methods, so every new surface method exercises its
    /// default — proving the additive trait surface preserves today's behavior.
    struct BareTool {
        params: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl Tool for BareTool {
        fn name(&self) -> &str {
            "bare"
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.params
        }
        async fn execute(
            &self,
            _call_id: ToolCallId,
            _params: serde_json::Value,
            _cancel: CancelToken,
            _on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::default())
        }
    }

    #[tokio::test]
    async fn defaulted_surface_preserves_behavior() {
        let t = BareTool { params: serde_json::json!({}) };
        assert_eq!(t.description(), "");
        assert_eq!(t.label(), None);
        assert_eq!(t.prompt_snippet(), None);
        assert!(t.prompt_guidelines().is_empty());
        assert_eq!(t.render_kind(), ToolRenderKind::Default);
        assert_eq!(t.render_call(&serde_json::json!({"a": 1})), None);
        assert_eq!(t.render_result(&ToolResult::default()), None);
        // prepare_arguments is an identity passthrough.
        let args = serde_json::json!({"x": [1, 2, 3]});
        assert_eq!(t.prepare_arguments(args.clone()).await, args);
        // The pre-existing defaults are unchanged.
        assert_eq!(t.execution_mode(), ExecMode::Parallel);
    }
}
