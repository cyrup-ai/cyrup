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
pub fn wrap_registered_tool(
    tool: Arc<dyn Tool>,
    active: Arc<dyn ActiveToolNames>,
) -> Arc<dyn Tool> {
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
    Some(
        after
            .iter()
            .filter(|n| !before_set.contains(n.as_str()))
            .cloned()
            .collect(),
    )
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
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.inner.prompt_guidelines()
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.inner.render_kind()
    }
    /// PROV-011 — upstream `wrapRegisteredTool` is a SPREAD of the already-wrapped tool
    /// (`return { ...tool, execute }`, `core/extensions/wrapper.ts:21-22` @v0.83.0), so every field
    /// `wrapToolDefinition` copied — `constrainedSampling` among them
    /// (`core/tools/tool-definition-wrapper.ts:14`) — survives this wrapper by construction.
    /// Rust has no spread: each surface method must be delegated by hand, and this one was the
    /// method the hand-written list missed.
    ///
    /// Everything the agent runs reaches it through this wrapper — the built-ins in `base` as well
    /// as the extension-registered and WASM-guest tools ([`crate::ExtensionHost::active_tools`],
    /// and upstream `wrapRegisteredTools(allCustomTools…)` +
    /// `wrapRegisteredTools(baseToolDefinitions…)` at agent-session.ts:2694-2702 @v0.84.2) — so the
    /// missing delegation silently dropped EVERY declaration one frame after it was read. For a
    /// guest tool that was the whole opt-in path, dead on arrival:
    /// `WasmTool::constrained_sampling` (host/live.rs) lifted the declaration off the descriptor
    /// and this wrapper discarded it. Since pi `7915cdac` @v0.84.2 it is also the path the four
    /// coding built-ins depend on: `read`, `edit`, `write` and the shared `ShellTool` engine —
    /// pi's `createShellToolDefinition`, so `powershell` inherits it — each return
    /// [`cyrup_core::experimental_tool_sampling`], which is `Some` only under
    /// `CYRUP_EXPERIMENTAL=1`/`PI_EXPERIMENTAL=1` and `None` otherwise.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        self.inner.constrained_sampling()
    }
    async fn prepare_arguments(&self, args: Value) -> Value {
        self.inner.prepare_arguments(args).await
    }
    fn render_call(&self, args: &Value) -> Option<String> {
        self.inner.render_call(args)
    }
    fn render_result(&self, result: &Value) -> Option<String> {
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
        let mut result = self
            .inner
            .execute(call_id, params, cancel, on_update)
            .await?;
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

    /// TOOL-024 — every surface method carries a DISTINCT non-default value.
    ///
    /// This fixture used to override only `name`, `parameters` and `execute`, so nine of the
    /// eleven assertions in `every_surface_method_delegates` compared a default against a default:
    /// deleting `RegisteredTool::prompt_snippet` (or `label`, or `render_kind`, or
    /// `prepare_arguments`) left the test GREEN, because the trait default the wrapper inherited
    /// equals the trait default the inner tool inherited. `prompt_guidelines` also owns a
    /// `Vec<String>` rather than a `&'static [&str]` — the exact shape `WasmTool` has, which is
    /// what forced TOOL-021's signature widening.
    struct Fixed {
        params: Value,
        result: Result<Vec<String>, ()>,
        guidelines: Vec<String>,
        constrained: cyrup_core::ConstrainedSampling,
    }

    #[async_trait::async_trait]
    impl Tool for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn parameters(&self) -> &Value {
            &self.params
        }
        fn description(&self) -> &str {
            "the fixed tool's description"
        }
        fn label(&self) -> Option<&str> {
            Some("Fixed Label")
        }
        fn prompt_snippet(&self) -> Option<&str> {
            Some("fixed prompt snippet")
        }
        fn prompt_guidelines(&self) -> Vec<&str> {
            self.guidelines.iter().map(String::as_str).collect()
        }
        fn render_kind(&self) -> ToolRenderKind {
            ToolRenderKind::SelfRendered
        }
        /// PROV-011 — a DISTINCT non-default declaration, for the reason this fixture exists: the
        /// trait default is `None`, so a fixture that left it defaulted would let
        /// `every_surface_method_delegates` compare `None` against `None` and stay green with the
        /// wrapper's delegation deleted.
        fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
            Some(&self.constrained)
        }
        fn execution_mode(&self) -> ExecMode {
            ExecMode::Sequential
        }
        fn render_call(&self, args: &Value) -> Option<String> {
            Some(format!("call:{args}"))
        }
        fn render_result(&self, result: &Value) -> Option<String> {
            Some(format!(
                "result:{}",
                result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map_or(0, Vec::len)
            ))
        }
        /// A MUTATING shim: an identity default would be indistinguishable from a dropped
        /// delegation (which is exactly how EXT-023 stayed invisible on the guest side).
        async fn prepare_arguments(&self, mut args: Value) -> Value {
            if let Some(o) = args.as_object_mut() {
                o.insert("prepared".to_string(), Value::Bool(true));
            }
            args
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
            guidelines: vec![
                "use fixed sparingly".to_string(),
                "fixed is not read".to_string(),
            ],
            constrained: cyrup_core::ConstrainedSampling::Config(
                cyrup_core::ConstrainedSamplingConfig::JsonSchema {
                    strict: cyrup_core::StrictSampling::Require,
                },
            ),
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
        assert_eq!(
            run(&w).await.unwrap().added_tool_names,
            vec!["late".to_string()]
        );
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
        let inner: Arc<dyn Tool> = Arc::new(Fixed {
            params: serde_json::json!({}),
            result: Err(()),
            guidelines: Vec::new(),
            // pi's explicit opt-OUT literal (`constrainedSampling: false`,
            // `packages/ai/README.md:483` @v0.83.0) — behaves as omitted.
            constrained: cyrup_core::ConstrainedSampling::Disabled(false),
        });
        let w = wrap_registered_tool(inner, active);
        assert!(run(&w).await.is_err());
    }

    /// TOOL-024 — `wrapRegisteredTool` returns a tool that is INDISTINGUISHABLE from the wrapped
    /// one on every non-`execute` surface (pi `wrapper.ts:22-35` wraps only `execute` and spreads
    /// the rest: `{...tool, execute: instrumented}`).
    ///
    /// Every value below is asserted against a LITERAL as well as against the inner tool, because
    /// `assert_eq!(w.x(), inner.x())` alone is vacuous whenever both sides fall through to the
    /// same trait default — which was true for nine of these eleven before this fixture grew real
    /// metadata. A deleted delegation now fails on the literal even if it "agrees" with the inner.
    #[tokio::test]
    async fn every_surface_method_delegates() {
        let active = ScriptedActive::new(vec![]);
        let inner = tool(vec![]);
        let w = wrap_registered_tool(inner.clone(), active);

        assert_eq!(w.name(), inner.name());
        assert_eq!(w.name(), "fixed");
        assert_eq!(w.description(), inner.description());
        assert_eq!(w.description(), "the fixed tool's description");
        assert_eq!(w.label(), inner.label());
        assert_eq!(w.label(), Some("Fixed Label"));
        assert_eq!(w.prompt_snippet(), inner.prompt_snippet());
        assert_eq!(w.prompt_snippet(), Some("fixed prompt snippet"));
        // TOOL-021: the inner tool's guidelines are OWNED `String`s, so this delegation is only
        // expressible since `Tool::prompt_guidelines` returns `Vec<&str>`.
        assert_eq!(w.prompt_guidelines(), inner.prompt_guidelines());
        assert_eq!(
            w.prompt_guidelines(),
            vec!["use fixed sparingly", "fixed is not read"]
        );
        assert_eq!(w.render_kind(), inner.render_kind());
        assert_eq!(w.render_kind(), ToolRenderKind::SelfRendered);
        // PROV-011 — upstream this survives because `wrapRegisteredTool` SPREADS the wrapped tool
        // (`core/extensions/wrapper.ts:21-22` @v0.83.0); in Rust it survives only because the
        // delegation is written out. Assert PRESENCE on the inner tool first, so a fixture that
        // ever loses its declaration fails loudly rather than passing this vacuously.
        assert!(
            inner.constrained_sampling().is_some(),
            "fixture must declare it"
        );
        assert_eq!(w.constrained_sampling(), inner.constrained_sampling());
        assert!(matches!(
            w.constrained_sampling()
                .and_then(cyrup_core::ConstrainedSampling::config),
            Some(cyrup_core::ConstrainedSamplingConfig::JsonSchema {
                strict: cyrup_core::StrictSampling::Require
            })
        ));
        assert_eq!(w.execution_mode(), inner.execution_mode());
        assert_eq!(w.execution_mode(), ExecMode::Sequential);
        assert_eq!(w.parameters(), inner.parameters());
        assert_eq!(
            w.render_call(&serde_json::json!({})),
            inner.render_call(&serde_json::json!({}))
        );
        assert_eq!(
            w.render_call(&serde_json::json!({})),
            Some("call:{}".to_string())
        );
        assert_eq!(
            w.render_result(&serde_json::json!({"content": []})),
            Some("result:0".to_string())
        );
        assert_eq!(
            w.render_result(&serde_json::json!({"content": [{"type": "text"}]})),
            Some("result:1".to_string()),
            "the wrapper delegates the payload through unchanged, not a default"
        );

        // A MUTATING `prepare_arguments`: the identity default cannot satisfy this.
        let args = serde_json::json!({"z": 1});
        assert_eq!(
            w.prepare_arguments(args.clone()).await,
            serde_json::json!({"z": 1, "prepared": true})
        );
    }
}
