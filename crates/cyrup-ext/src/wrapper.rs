//! The registered-tool wrapper (arch-08 §3.1) — a 1:1 port of Pi's
//! `coding-agent/src/core/extensions/wrapper.ts` `wrapRegisteredTool`.
//!
//! Upstream, a tool NEVER sets `addedToolNames` itself. Pi wraps every tool that lands in the
//! session's `_toolRegistry` — the built-ins as well as the extension-registered ones
//! (`wrapRegisteredTools(allCustomTools, runner)` + `wrapRegisteredTools(baseToolDefinitions…)`,
//! agent-session.ts:2506-2515) — and the wrapper snapshots `runner.getActiveTools()` on BOTH sides
//! of `execute`. When the change is purely ADDITIVE it folds the difference onto the result as
//! `addedToolNames` (`AgentToolResult.addedToolNames`, agent/src/types.ts:362-363, upstream
//! `3d8f7435`). That derived value is the message anchor a provider adapter with native deferred
//! tool loading reads back off the transcript (`splitDeferredTools`).
//!
//! Two properties of the upstream algorithm are load-bearing and reproduced exactly:
//!
//! * **Additive-only.** If ANY name present before is missing after, the wrapper returns the result
//!   untouched (`if (!activeBefore.every((name) => activeAfter.includes(name))) return result`).
//!   A removal invalidates the model's cached tool definitions wholesale, so anchoring the
//!   *additions* of such a change would place definitions against a cache that is about to be
//!   wiped anyway.
//! * **Union, order-preserving, deduped.** A tool that DID set the field keeps its own entries
//!   first (`[...new Set([...(result.addedToolNames ?? []), ...addedToolNames])]`).

use cyrup_core::{
    CancelToken, ExecMode, Tool, ToolCallId, ToolError, ToolRenderKind, ToolResult, ToolUpdateSink,
};
use serde_json::Value;
use std::sync::Arc;

/// The live active-tool-name source the wrapper diffs against (Pi `ExtensionRunner.getActiveTools()`,
/// extensions/runner.ts:664-667, which binds straight through to the session's `getActiveToolNames`,
/// agent-session.ts:813 — i.e. `agent.state.tools`).
///
/// `None` means "no live agent is attached", which is the case for the default host and for the
/// window between building the tool set and attaching the session's dynamic-tool view. The wrapper
/// treats `None` as "cannot tell" and stamps nothing — never as "the set shrank".
pub trait ActiveToolNames: Send + Sync {
    fn active_tool_names(&self) -> Option<Vec<String>>;
}

/// A tool wrapped for `addedToolNames` derivation (Pi `wrapRegisteredTool`). Every `Tool` surface
/// method delegates verbatim; only `execute` is instrumented.
pub struct RegisteredTool {
    inner: Arc<dyn Tool>,
    active: Arc<dyn ActiveToolNames>,
}

impl RegisteredTool {
    pub fn new(inner: Arc<dyn Tool>, active: Arc<dyn ActiveToolNames>) -> Self {
        Self { inner, active }
    }

    /// The wrapped tool, for callers that need the raw handle back.
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.inner
    }
}

/// Wrap `tool` so its execution derives `addedToolNames` (Pi `wrapRegisteredTool`).
pub fn wrap_registered_tool(tool: Arc<dyn Tool>, active: Arc<dyn ActiveToolNames>) -> Arc<dyn Tool> {
    Arc::new(RegisteredTool::new(tool, active))
}

/// The names present in `after` but not in `before`, in `after` order — or `None` when the change
/// was NOT purely additive (Pi's `activeBefore.every(...)` bail-out).
fn additive_delta(before: &[String], after: &[String]) -> Option<Vec<String>> {
    let after_set: std::collections::HashSet<&str> = after.iter().map(String::as_str).collect();
    if !before.iter().all(|n| after_set.contains(n.as_str())) {
        return None;
    }
    let before_set: std::collections::HashSet<&str> = before.iter().map(String::as_str).collect();
    Some(after.iter().filter(|n| !before_set.contains(n.as_str())).cloned().collect())
}

/// Union `derived` onto `existing`, preserving order and dropping duplicates (Pi's
/// `[...new Set([...(result.addedToolNames ?? []), ...addedToolNames])]`).
fn union_in_order(existing: &[String], derived: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(existing.len() + derived.len());
    for n in existing.iter().cloned().chain(derived) {
        if seen.insert(n.clone()) {
            out.push(n);
        }
    }
    out
}

#[async_trait::async_trait]
impl Tool for RegisteredTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn parameters(&self) -> &Value {
        self.inner.parameters()
    }
    fn execution_mode(&self) -> ExecMode {
        self.inner.execution_mode()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn label(&self) -> Option<&str> {
        self.inner.label()
    }
    fn prompt_snippet(&self) -> Option<&str> {
        self.inner.prompt_snippet()
    }
    fn prompt_guidelines(&self) -> &[&str] {
        self.inner.prompt_guidelines()
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.inner.render_kind()
    }
    async fn prepare_arguments(&self, args: Value) -> Value {
        self.inner.prepare_arguments(args).await
    }
    fn render_call(&self, args: &Value) -> Option<String> {
        self.inner.render_call(args)
    }
    fn render_result(&self, result: &ToolResult) -> Option<String> {
        self.inner.render_result(result)
    }

    /// Pi `wrapRegisteredTool`'s instrumented `execute` (wrapper.ts:22-35).
    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let before = self.active.active_tool_names();
        // A failing tool propagates unchanged — upstream the `await` throws past the diff entirely.
        let mut result = self.inner.execute(call_id, params, cancel, on_update).await?;
        let (Some(before), Some(after)) = (before, self.active.active_tool_names()) else {
            return Ok(result);
        };
        let Some(added) = additive_delta(&before, &after) else {
            return Ok(result); // not purely additive → leave the result alone
        };
        if added.is_empty() {
            return Ok(result);
        }
        result.added_tool_names = union_in_order(&result.added_tool_names, added);
        Ok(result)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_core::Content;
    use std::sync::Mutex;

    /// A scripted `getActiveTools`: pops one snapshot per call, so a test can script the
    /// before/after pair the wrapper observes around `execute`.
    struct ScriptedActive(Mutex<Vec<Option<Vec<String>>>>);

    impl ScriptedActive {
        fn new(steps: Vec<Option<Vec<&str>>>) -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                steps
                    .into_iter()
                    .map(|s| s.map(|v| v.into_iter().map(str::to_string).collect()))
                    .collect(),
            )))
        }
    }

    impl ActiveToolNames for ScriptedActive {
        fn active_tool_names(&self) -> Option<Vec<String>> {
            let mut g = self.0.lock().unwrap();
            if g.is_empty() {
                return None;
            }
            g.remove(0)
        }
    }

    struct Fixed {
        params: Value,
        result: Result<Vec<String>, ()>,
    }

    #[async_trait::async_trait]
    impl Tool for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn parameters(&self) -> &Value {
            &self.params
        }
        async fn execute(
            &self,
            _call_id: ToolCallId,
            _params: Value,
            _cancel: CancelToken,
            _on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            match &self.result {
                Ok(added) => Ok(ToolResult {
                    content: vec![Content::text("ok")],
                    added_tool_names: added.clone(),
                    ..Default::default()
                }),
                Err(()) => Err(ToolError::new("boom")),
            }
        }
    }

    fn tool(added: Vec<&str>) -> Arc<dyn Tool> {
        Arc::new(Fixed {
            params: serde_json::json!({}),
            result: Ok(added.into_iter().map(str::to_string).collect()),
        })
    }

    async fn run(t: &Arc<dyn Tool>) -> Result<ToolResult, ToolError> {
        t.execute(
            ToolCallId::from("c1"),
            serde_json::json!({}),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
    }

    #[tokio::test]
    async fn a_purely_additive_widening_is_stamped_onto_the_result() {
        let active = ScriptedActive::new(vec![Some(vec!["a"]), Some(vec!["a", "late"])]);
        let w = wrap_registered_tool(tool(vec![]), active);
        assert_eq!(run(&w).await.unwrap().added_tool_names, vec!["late".to_string()]);
    }

    #[tokio::test]
    async fn a_removal_bails_out_and_stamps_nothing() {
        // "a" disappeared: not purely additive, so upstream returns the result untouched even
        // though "late" was also added.
        let active = ScriptedActive::new(vec![Some(vec!["a", "b"]), Some(vec!["b", "late"])]);
        let w = wrap_registered_tool(tool(vec![]), active);
        assert!(run(&w).await.unwrap().added_tool_names.is_empty());
    }

    #[tokio::test]
    async fn an_unchanged_set_stamps_nothing() {
        let active = ScriptedActive::new(vec![Some(vec!["a"]), Some(vec!["a"])]);
        let w = wrap_registered_tool(tool(vec![]), active);
        assert!(run(&w).await.unwrap().added_tool_names.is_empty());
    }

    #[tokio::test]
    async fn the_tools_own_names_come_first_and_dedupe() {
        let active = ScriptedActive::new(vec![Some(vec!["a"]), Some(vec!["a", "x", "y"])]);
        let w = wrap_registered_tool(tool(vec!["x", "own"]), active);
        assert_eq!(
            run(&w).await.unwrap().added_tool_names,
            vec!["x".to_string(), "own".to_string(), "y".to_string()]
        );
    }

    #[tokio::test]
    async fn no_live_agent_stamps_nothing() {
        let active = ScriptedActive::new(vec![None, None]);
        let w = wrap_registered_tool(tool(vec![]), active);
        assert!(run(&w).await.unwrap().added_tool_names.is_empty());
    }

    #[tokio::test]
    async fn a_failing_tool_propagates_unchanged() {
        let active = ScriptedActive::new(vec![Some(vec!["a"]), Some(vec!["a", "late"])]);
        let inner: Arc<dyn Tool> =
            Arc::new(Fixed { params: serde_json::json!({}), result: Err(()) });
        let w = wrap_registered_tool(inner, active);
        assert!(run(&w).await.is_err());
    }

    #[tokio::test]
    async fn every_surface_method_delegates() {
        let active = ScriptedActive::new(vec![]);
        let inner = tool(vec![]);
        let w = wrap_registered_tool(inner.clone(), active);
        assert_eq!(w.name(), inner.name());
        assert_eq!(w.description(), inner.description());
        assert_eq!(w.label(), inner.label());
        assert_eq!(w.prompt_snippet(), inner.prompt_snippet());
        assert_eq!(w.prompt_guidelines(), inner.prompt_guidelines());
        assert_eq!(w.render_kind(), inner.render_kind());
        assert_eq!(w.execution_mode(), inner.execution_mode());
        assert_eq!(w.parameters(), inner.parameters());
        assert_eq!(w.render_call(&serde_json::json!({})), None);
        assert_eq!(w.render_result(&ToolResult::default()), None);
        let args = serde_json::json!({"z": 1});
        assert_eq!(w.prepare_arguments(args.clone()).await, args);
    }
}
