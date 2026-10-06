//! The `tool_search` tool definition (`createToolSearchToolDefinition`,
//! `extensions/tool-search/tool.ts:169-247` @v1.0.1).
//!
//! # Production call path
//!
//! [`crate::ToolSearchExtension::init`] registers a [`ToolSearchTool`] bound to the session's
//! [`HostServices`]; the agent loop calls [`ToolSearchTool::execute`] when the model calls
//! `tool_search`.

use std::sync::Arc;

use cyrup_core::{
    CancelToken, Content, Tool, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink,
};
use cyrup_ext::HostServices;
use serde_json::{Value, json};

use crate::input::{ToolSearchInput, tool_search_schema};
use crate::search::{result_text, search_and_load};

/// `TOOL_SEARCH_TOOL_NAME` (`tool.ts:20`).
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";

/// `TOOL_SEARCH_DESCRIPTION` (`tool.ts:220`). It does not list the searchable tools or their
/// namespaces, so it stays the same while tools are registered, for example when MCP servers connect.
pub const TOOL_SEARCH_DESCRIPTION: &str = "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nSome of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`.";

/// `promptSnippet` (`tool.ts:229`).
const PROMPT_SNIPPET: &str = "Search for tools that are not loaded yet and load the matches";

/// The `tool_search` tool.
pub struct ToolSearchTool {
    /// The session whose tools it searches and loads. Without one, as in upstream's missing
    /// `options.tools`, the tool finds nothing.
    session: Option<Arc<dyn HostServices>>,
}

impl ToolSearchTool {
    #[must_use]
    pub fn new(session: Option<Arc<dyn HostServices>>) -> Self {
        Self { session }
    }
}

#[async_trait::async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        TOOL_SEARCH_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        tool_search_schema()
    }

    fn description(&self) -> &str {
        TOOL_SEARCH_DESCRIPTION
    }

    fn label(&self) -> Option<&str> {
        Some(TOOL_SEARCH_TOOL_NAME)
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(PROMPT_SNIPPET)
    }

    /// Searching is not something scripts need; it changes what the model sees (`tool.ts:231-232`).
    fn exposure(&self) -> ToolExposure {
        ToolExposure::ModelOnly
    }

    /// Registered inactive (`index.ts:15`): activate it with `--tools`, the `defaultTools` setting,
    /// or `set_active_tools_by_name`.
    fn default_active(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input = ToolSearchInput::parse(&params)
            .map_err(|refusal| ToolError::new(refusal.to_string()))?;
        let loaded = self
            .session
            .as_deref()
            .map(|session| search_and_load(session, &input))
            .unwrap_or_default();
        Ok(ToolResult {
            content: vec![Content::text(result_text(&loaded))],
            details: Some(json!({
                "loaded": loaded.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>()
            })),
            ..ToolResult::default()
        })
    }
}
