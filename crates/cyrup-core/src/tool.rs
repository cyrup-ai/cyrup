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
///
/// Mirrors Pi's `AgentToolResult<T>` (agent/src/types.ts:354-368). Every field past
/// `content`/`details` is optional upstream, so each Rust analogue has a `Default` that means
/// "absent"; build with `..Default::default()` so a later widening stays source-compatible.
#[derive(Clone, Debug, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
    /// Usage from the tool execution itself, if available. NOT part of main LLM context accounting
    /// (Pi `AgentToolResult.usage`, types.ts:360-361, upstream `2fd38684`). Reaches the transcript
    /// as `ToolResultMessage.usage` and is patchable by `after_tool_call` (Pi
    /// `AfterToolCallResult.usage`, types.ts:83-84).
    pub usage: Option<crate::message::Usage>,
    /// Names of tools introduced by this result and available from this transcript point onward
    /// (Pi `AgentToolResult.addedToolNames`, types.ts:362-363, upstream `3d8f7435`).
    ///
    /// This does NOT by itself change the active tool set — that is driven by the runtime's tool
    /// list. It is a cache-placement record telling a provider adapter with native deferred tool
    /// loading WHERE in the transcript a tool definition first becomes available; adapters without
    /// that capability ignore it and use the normal tool list. Empty = absent on the wire.
    pub added_tool_names: Vec<String>,
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
    /// empty contributes nothing (today's behavior).
    ///
    /// TOOL-021 / EXT-007: the return type is `Vec<&str>`, not `&[&str]`. A borrowed slice of
    /// `&'static str` can only be produced by a tool whose guidelines are a compile-time constant
    /// array — which every BUILT-IN is, and which a WASM guest tool can never be: its descriptor
    /// owns a `Vec<String>` decoded from the component (`cyrup-ext/wit/world.wit`
    /// `prompt-guidelines`, copied at `host/live.rs:121` and stored at `registry.rs:27`). So the
    /// data crossed the ABI, reached the host, and had no reader — a guest declaring
    /// `promptGuidelines` silently contributed nothing to the system prompt. Borrowing `&str` from
    /// `&self` keeps the zero-copy property for the built-ins (`SLICE.to_vec()` copies pointers,
    /// not strings) while making the owned case expressible at all.
    fn prompt_guidelines(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Whether the runtime draws the standard tool shell or the tool renders its own framing (Pi
    /// `ToolDefinition.renderShell`, extensions/types.ts:448-449). Defaulted to
    /// [`ToolRenderKind::Default`] (today's behavior).
    fn render_kind(&self) -> ToolRenderKind {
        ToolRenderKind::Default
    }

    /// Per-tool opt-in to provider-side constrained sampling (Pi
    /// `ToolDefinition.constrainedSampling`, extensions/types.ts:463 @v0.83.0).
    ///
    /// PROV-011 / EXT-024. Upstream the declaration is copied verbatim from the `ToolDefinition`
    /// onto the runtime `AgentTool` by `wrapToolDefinition`
    /// (`packages/coding-agent/src/core/tools/tool-definition-wrapper.ts:14` @v0.83.0, and back at
    /// `:42`), reaches the loop as `Context.tools[].constrainedSampling`, and is resolved by
    /// `packages/ai/src/api/constrained-sampling.ts`. This accessor is that copy's Rust
    /// counterpart: the runtime `Tool` is where the loop reads it from.
    ///
    /// Default `None` = the field is absent, which upstream is indistinguishable from `false`
    /// (`ConstrainedSampling::Disabled`). No pi built-in tool declares it — the three hits of
    /// `git grep constrainedSampling v0.83.0 -- packages/coding-agent/src packages/agent/src` are
    /// the field declaration and the two wrapper copies — so every built-in correctly keeps the
    /// default.
    fn constrained_sampling(&self) -> Option<&crate::ConstrainedSampling> {
        None
    }

    /// Compatibility shim to normalize raw tool-call arguments before schema validation (Pi
    /// `ToolDefinition.prepareArguments`, extensions/types.ts:451-452). Default: identity
    /// passthrough — the arguments are returned unchanged (today's behavior).
    async fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        args
    }

    /// Custom rendering of a tool *call* for the UI (Pi `ToolDefinition.renderCall`,
    /// `core/extensions/types.ts:488-489` @v0.84.2). The returned string is the rendered
    /// representation; `None` = the runtime uses its standard call framing.
    ///
    /// **This is a live contribution point** — see [`Tool::render_result`] for the resolution
    /// order it participates in. It is the tier upstream reaches for a tool that is neither a
    /// built-in nor extension-registered: pi's resolver is
    /// `toolDefinition.renderCall ?? builtInToolDefinition.renderCall`
    /// (`modes/interactive/components/tool-execution.ts:84-91` @v0.84.2), where `toolDefinition` is
    /// `session.getToolDefinition(name)` (`interactive-mode.ts:1996-1998`) reading
    /// `_toolDefinitions` — a map built from the built-in table OVERLAID with `allCustomTools`,
    /// which is `extensionRunner.getAllRegisteredTools()` **plus `this._customTools`**, the SDK
    /// tools handed to `createAgentSession({customTools})` (`core/agent-session.ts:2471-2495`).
    /// So upstream a plain SDK tool object supplies its own renderer through the very same map an
    /// extension's does, and this method is that seam in cyrup.
    ///
    /// Implementors do not register anything: `SessionBuilder` hands every configured custom tool
    /// to `ExtensionHost::register_native_tool_renderer`, and the host consults it when no
    /// extension owns a renderer for the name.
    fn render_call(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Custom rendering of a tool *result* for the UI (Pi `ToolDefinition.renderResult`,
    /// `core/extensions/types.ts:491-497` @v0.84.2). The returned string is the rendered
    /// representation; `None` = the runtime uses its standard result framing.
    ///
    /// # Resolution order
    ///
    /// Upstream is `toolDefinition.renderResult ?? builtInToolDefinition.renderResult`
    /// (`modes/interactive/components/tool-execution.ts:94-101`), i.e. a tool's OWN renderer wins
    /// over the built-in table keyed by name. cyrup resolves the same three tiers in the same
    /// order, split across two crates because the built-in tier draws `ratatui` lines rather than
    /// returning a string:
    ///
    /// 1. an extension that registered a renderer for this tool NAME
    ///    (`ExtensionHost::render_tool_result_outcome` → `registry.tool_renderer_owner`);
    /// 2. **this method**, via the host's native-tool table
    ///    (`ExtensionHost::register_native_tool_renderer`) — tiers 1 and 2 are one map upstream
    ///    (`allCustomTools`, `core/agent-session.ts:2472-2478`);
    /// 3. the built-in per-name dispatch in `cyrup_tui::transcript::tool_lines`.
    ///
    /// # Why `&Value` and not `&ToolResult`
    ///
    /// CYRUP-DELTA vs the typed `AgentToolResult` upstream's `renderResult` receives. The renderer
    /// is resolved at the point of DISPLAY, and by then the result has crossed the session-event
    /// boundary as `AgentSessionEvent::ToolExecutionEnd { result: serde_json::Value, .. }`
    /// (`cyrup-session-svc/src/event.rs:143-148`) — [`ToolResult`] is not `Deserialize`, so no
    /// typed value survives to the seam. Taking the `Value` the seam actually carries is what
    /// makes this method reachable; taking `&ToolResult` is what kept it dead.
    ///
    /// RESIDUAL, shared with tier 1 and NOT introduced here: upstream also passes
    /// `ToolRenderResultOptions` (`expanded`, `isPartial`), the theme and a `ToolRenderContext`.
    /// cyrup renders once as the event is folded into the transcript rather than at draw time, so
    /// neither tier has an expansion state to pass; the extension tier
    /// (`ExtensionHost::render_tool_result`) has always had the same shape.
    fn render_result(&self, _result: &serde_json::Value) -> Option<String> {
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
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
        assert!(t.constrained_sampling().is_none());
        assert_eq!(t.render_call(&serde_json::json!({"a": 1})), None);
        assert_eq!(t.render_result(&serde_json::json!({"content": []})), None);
        // prepare_arguments is an identity passthrough.
        let args = serde_json::json!({"x": [1, 2, 3]});
        assert_eq!(t.prepare_arguments(args.clone()).await, args);
        // The pre-existing defaults are unchanged.
        assert_eq!(t.execution_mode(), ExecMode::Parallel);
    }

    /// PROV-011: a tool that DOES declare `constrainedSampling` must be able to hand it to the
    /// loop. Without this the `is_none()` assertion above is vacuous — it would hold for a trait
    /// surface that had no way to say yes.
    struct OptingTool {
        params: serde_json::Value,
        cs: crate::ConstrainedSampling,
    }

    #[async_trait::async_trait]
    impl Tool for OptingTool {
        fn name(&self) -> &str {
            "opting"
        }
        fn parameters(&self) -> &serde_json::Value {
            &self.params
        }
        fn constrained_sampling(&self) -> Option<&crate::ConstrainedSampling> {
            Some(&self.cs)
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

    #[test]
    fn a_tool_can_opt_in_to_constrained_sampling() {
        use crate::constrained_sampling::{
            ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants,
        };
        let t = OptingTool {
            params: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            }),
            cs: ConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".into()),
                    openai_regex: None,
                },
            }),
        };
        let declared = t.constrained_sampling().expect("the tool declared it");
        match declared.config() {
            Some(ConstrainedSamplingConfig::Grammar { variants }) => {
                assert_eq!(variants.openai_lark.as_deref(), Some("start: /[a-z]+/"));
            }
            other => panic!("expected a grammar config, got {other:?}"),
        }
    }
}
