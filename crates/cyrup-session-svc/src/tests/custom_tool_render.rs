//! A custom (SDK) tool's OWN `render_call`/`render_result` must reach the transcript.
//!
//! Upstream a tool contributes rendering through the same map an extension does. The resolver is
//! `toolDefinition.renderCall ?? builtInToolDefinition.renderCall`
//! (`modes/interactive/components/tool-execution.ts:84-101` @v0.84.2), and `toolDefinition` is
//! `session.getToolDefinition(name)` (`modes/interactive/interactive-mode.ts:1996-1998`) reading
//! `_toolDefinitions` — the built-in table overlaid by a loop over `allCustomTools`, which is
//! `extensionRunner.getAllRegisteredTools()` spread FOLLOWED BY `this._customTools`, the SDK tools
//! passed to `createAgentSession({customTools})` (`core/agent-session.ts:2471-2495`).
//!
//! cyrup ported the extension half of that map (`ExtensionRegistry::tool_renderer_owner`) and the
//! built-in half (`cyrup_tui::transcript::tool_lines`'s per-name dispatch) but not the SDK half:
//! `Tool::render_call`/`Tool::render_result` had NO reader anywhere in `crates/`, so a custom tool
//! that supplied its own rendering had it silently discarded and drew the generic shell.
//!
//! # What these tests see PRE-FIX
//!
//! Both assert against the seam production uses, not against the new helper. `cyrup_tui::app::
//! extension_render` gates on `ExtensionHost::has_tool_renderer(name)` and early-returns `None`
//! when it is false, then calls `render_tool_call_outcome`/`render_tool_result_outcome`.
//!
//! Pre-fix, `has_tool_renderer` consulted the extension registry ALONE and `render_*_outcome`
//! resolved through `tool_renderer_owner` alone; a custom tool owns no extension, so both answered
//! "nothing renders this". `custom_tool_renders_its_own_call` therefore fails pre-fix on its first
//! assertion (`has_tool_renderer` → `false`) and, with that assertion deleted, still fails on
//! [`cyrup_ext::RenderOutcome::None`]. It is the load-bearing red one: every symbol it touches
//! existed pre-fix with these exact signatures, so it compiles against the unfixed tree and fails
//! there on behaviour.
//!
//! `custom_tool_renders_its_own_result` fails for the same reason but is weaker evidence: pre-fix
//! `render_result` took `&ToolResult`, so `RenderingTool` would not have compiled unchanged.
//!
//! A test that called `RenderingTool::render_call` directly would pass against the unfixed tree,
//! which is exactly the trap here — the method always worked, nothing ever called it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::{Content, Tool, ToolError, ToolResult, ToolUpdateSink};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// A custom tool that supplies BOTH renderers, each echoing something only it could know — so a
/// passing assertion cannot be satisfied by the generic shell or by the built-in dispatch.
struct RenderingTool {
    params: serde_json::Value,
}

impl RenderingTool {
    fn new() -> Self {
        Self {
            params: serde_json::json!({
                "type": "object",
                "properties": {"q": {"type": "string"}},
            }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for RenderingTool {
    fn name(&self) -> &str {
        "render_me"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "A tool that draws its own call and result"
    }
    fn render_call(&self, args: &serde_json::Value) -> Option<String> {
        Some(format!(
            "CALL<{}>",
            args.get("q").and_then(|v| v.as_str()).unwrap_or("")
        ))
    }
    fn render_result(&self, result: &serde_json::Value) -> Option<String> {
        Some(format!(
            "RESULT<{}>",
            result.get("marker").and_then(|v| v.as_str()).unwrap_or("")
        ))
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("done")],
            ..Default::default()
        })
    }
}

/// A custom tool WITHOUT renderers. Every custom tool is registered unconditionally (whether
/// `render_call` returns `Some` cannot be known without calling it), so what must hold is that this
/// one still RESOLVES to nothing — otherwise `tool_lines` would take its extension branch with an
/// empty block instead of the built-in/generic shell.
struct PlainTool {
    params: serde_json::Value,
}

impl PlainTool {
    fn new() -> Self {
        Self {
            params: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
}

#[async_trait::async_trait]
impl Tool for PlainTool {
    fn name(&self) -> &str {
        "plain_me"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

/// PRE-FIX: `has_tool_renderer("render_me")` is `false` (the extension registry is empty and was
/// the only table consulted), so this fails on the first assertion; with that assertion removed it
/// still fails, because `render_tool_call_outcome` resolved through `tool_renderer_owner` alone and
/// returned [`cyrup_ext::RenderOutcome::None`].
#[tokio::test]
async fn custom_tool_renders_its_own_call() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(RenderingTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();
    let host = session.ext_host();

    // The cheap sync gate `cyrup_tui::app::extension_render` early-returns on.
    assert!(
        host.has_tool_renderer("render_me"),
        "a custom tool that overrides render_call must be reported as having a renderer, or the \
         TUI never attempts to resolve one"
    );

    let out = host
        .render_tool_call_outcome("render_me", &serde_json::json!({"q": "hello"}))
        .await
        .into_option();
    assert_eq!(
        out,
        Some(serde_json::Value::String("CALL<hello>".to_string())),
        "the custom tool's own render_call must own the tool row"
    );
}

/// PRE-FIX: fails identically to the call-side test — and could not even be written against the
/// old `render_result(&ToolResult)` signature, since the render seam carries the serialized
/// `AgentSessionEvent::ToolExecutionEnd { result: Value }` and [`ToolResult`] is not `Deserialize`.
#[tokio::test]
async fn custom_tool_renders_its_own_result() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(RenderingTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();

    let out = session
        .ext_host()
        .render_tool_result_outcome("render_me", &serde_json::json!({"marker": "ok"}))
        .await
        .into_option();
    assert_eq!(
        out,
        Some(serde_json::Value::String("RESULT<ok>".to_string())),
        "the custom tool's own render_result must own the result row"
    );
}

/// The negative half: a custom tool taking both defaults must still resolve to NOTHING, so
/// `tool_lines` keeps drawing the built-in/generic shell, and a name this tier knows nothing about
/// must not be claimed.
///
/// This one does NOT go red pre-fix and is not offered as proof of the fix — it pins the blast
/// radius. `has_tool_renderer` is deliberately coarse here (true for any REGISTERED custom tool,
/// renderers or not) because upstream's gate is coarser still: `hasRendererDefinition()` is
/// `builtInToolDefinition !== undefined || toolDefinition !== undefined`
/// (`modes/interactive/components/tool-execution.ts:104-106` @v0.84.2) — true for every tool that
/// has a definition at all. The cost of the coarseness is one resolution that returns `None`, and
/// what must not regress is the OUTCOME, which is what this asserts.
#[tokio::test]
async fn tool_without_renderers_resolves_to_nothing() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(PlainTool::new()), Arc::new(RenderingTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();
    let host = session.ext_host();

    assert!(
        host.render_tool_call_outcome("plain_me", &serde_json::json!({}))
            .await
            .into_option()
            .is_none(),
        "a tool overriding neither method must leave the caller drawing the built-in shell"
    );
    assert!(
        host.render_tool_result_outcome("plain_me", &serde_json::json!({}))
            .await
            .into_option()
            .is_none(),
        "same on the result side"
    );
    assert!(
        !host.has_tool_renderer("read"),
        "a built-in tool is drawn by the per-name dispatch, not through this tier"
    );
    assert!(
        !host.has_tool_renderer("nonexistent"),
        "an unknown tool name is claimed by nothing"
    );
    assert!(
        host.render_tool_call_outcome("nonexistent", &serde_json::json!({}))
            .await
            .into_option()
            .is_none(),
        "and resolves to nothing"
    );
}
