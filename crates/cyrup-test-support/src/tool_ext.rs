//! Synthetic tool-injection for the harness (Pi `HarnessOptions.tools` + `baseToolsOverride`,
//! test-harness.ts:331,333,402; suite/harness.ts:67,107,183).
//!
//! Pi registers custom `AgentTool`s (and replaces built-in `read`/`bash`/`edit`/`write`) directly on
//! the `Agent`/`AgentSession`. cyrup's `SessionBuilder` has no `with_tool` hook, but a native
//! extension registers tools through [`InitApi::register_tool`], which *overrides a built-in of the
//! same name at the registry* (R-08-012). So a custom tool named `read` replaces the built-in `read`
//! — exactly Pi's `baseToolsOverride` semantics — while a uniquely-named tool is additive (Pi
//! `tools`). [`ToolExtension`] is the harness's tool-injection vehicle.
//!
//! [`SyntheticTool`] is a minimal, deterministic [`Tool`] for tool-dispatch/permission tests: it
//! records its invocations and returns a fixed text result.

use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExtensionId, TerminateHint, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};

/// A native extension that registers a fixed set of [`Tool`]s (Pi harness `tools` /
/// `baseToolsOverride` injection). Loaded via [`crate::harness::HarnessOptions::tools`].
pub struct ToolExtension {
    id: ExtensionId,
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolExtension {
    /// An extension named `test-tools` that registers `tools` (built-in names override built-ins).
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self {
            id: ExtensionId::from("test-tools"),
            tools,
        }
    }

    /// An extension with a caller-chosen id (for loading several disjoint tool sets).
    pub fn with_id(id: impl Into<String>, tools: Vec<Arc<dyn Tool>>) -> Self {
        Self {
            id: ExtensionId::from(id.into()),
            tools,
        }
    }
}

#[async_trait::async_trait]
impl NativeExtension for ToolExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for tool in &self.tools {
            api.register_tool(tool.clone());
        }
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A record of one [`SyntheticTool`] invocation (call id + raw params).
#[derive(Clone, Debug)]
pub struct SyntheticCall {
    pub call_id: String,
    pub params: serde_json::Value,
}

/// A deterministic, inspectable [`Tool`] for harness tool-dispatch tests. Accepts any object params
/// (open `{}` schema), records every call, and returns a fixed text result.
pub struct SyntheticTool {
    name: String,
    parameters: serde_json::Value,
    result_text: String,
    calls: Arc<Mutex<Vec<SyntheticCall>>>,
}

impl SyntheticTool {
    /// A tool named `name` that returns `result_text` and records its calls.
    pub fn new(name: impl Into<String>, result_text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            result_text: result_text.into(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A shared handle to this tool's recorded invocations.
    pub fn calls_handle(&self) -> Arc<Mutex<Vec<SyntheticCall>>> {
        self.calls.clone()
    }

    /// Snapshot of the recorded invocations.
    pub fn calls(&self) -> Vec<SyntheticCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait::async_trait]
impl Tool for SyntheticTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(SyntheticCall {
                call_id: call_id.to_string(),
                params,
            });
        Ok(ToolResult {
            content: vec![Content::text(self.result_text.clone())],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}
