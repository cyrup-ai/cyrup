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

/// A streamed progress update (func-02 R-02-023).
#[derive(Clone, Debug, Default)]
pub struct ToolUpdate {
    pub content: Vec<Content>,
    pub details: Option<serde_json::Value>,
}

/// Sink the runtime hands to a tool to stream progress. The runtime ignores updates after the
/// tool's execution settles (func-02 R-02-023).
pub type ToolUpdateSink = Box<dyn FnMut(ToolUpdate) + Send + 'static>;

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
    async fn execute(
        &self,
        call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError>;
}
