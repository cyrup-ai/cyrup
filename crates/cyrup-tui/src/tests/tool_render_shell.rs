//! EXT-024 — `ToolDefinition.renderShell` reaches the tool ROW (pi
//! `packages/coding-agent/src/core/extensions/types.ts:467` @v0.84.4: "Controls whether
//! ToolExecutionComponent renders the standard colored shell or the tool renders its own framing").
//!
//! Pi resolves it per component from the session's definition registry —
//! `getRenderShell()` is `toolDefinition.renderShell ?? builtInToolDefinition.renderShell ??
//! "default"` (`modes/interactive/components/tool-execution.ts:108-116`), the definition being
//! `this.session.getToolDefinition(toolName)` (`interactive-mode.ts:2067-2069`, handed to the
//! constructor at `:3348`) — and branches the whole block on it: `"self"` mounts a bare
//! `Container` in place of the `Box(1, 1, bgFn)` (`:76`), `updateDisplay` skips `setBgFn` for it
//! (`:275-277`), and `render()` emits `[""] + selfRenderContainer.render(width)` with no padding
//! and no tint (`:237-259`).
//!
//! cyrup carried the value all the way to `Tool::render_kind` — the WIT field, the SDK lowering,
//! `WasmTool`, the built-in `edit`, every MCP tool in compact mode — and drew the tinted
//! `Box(1, 1)` for every one of them: `grep -rn render_kind crates/cyrup-tui` was empty. These
//! tests drive the PRODUCTION lookup (`App::refresh_known_tool_definitions` /
//! `App::ingest_session_event_owned` over a real `AgentSession` whose registry holds a tool that
//! declares `SelfRendered`) and assert on the rendered CELLS: the self-rendered row carries no
//! `toolPendingBg` and starts at column 0; the default one keeps the shell.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use crate::{App, UiTheme};
use cyrup_core::{
    CancelToken, TerminateHint, Tool, ToolCallId, ToolError, ToolRenderKind, ToolResult,
    ToolUpdateSink,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, SessionConfig, SessionFactory,
    SessionTarget,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use serde_json::json;
use tempfile::TempDir;

/// A custom tool (pi `customTools`) whose only distinguishing trait is its declared shell.
struct FramedTool {
    name: &'static str,
    kind: ToolRenderKind,
    params: serde_json::Value,
}

impl FramedTool {
    fn new(name: &'static str, kind: ToolRenderKind) -> Self {
        Self {
            name,
            kind,
            params: json!({ "type": "object", "properties": {} }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for FramedTool {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "a tool that declares its own render shell"
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.kind
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _args: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text("ok")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

/// A real session whose definition registry holds one tool per shell kind.
async fn session_with_framed_tools() -> (TempDir, Arc<AgentSession>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    config.custom_tools = vec![
        Arc::new(FramedTool::new("self_framed", ToolRenderKind::SelfRendered)),
        Arc::new(FramedTool::new("shell_framed", ToolRenderKind::Default)),
    ];
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(SessionFactory::new(provider, config));
    let rt = AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    let session = rt.session().await;
    (tmp, session)
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
}

/// The row that carries `needle`, as `(column of its first cell, backgrounds of every cell)`.
fn row_with(app: &App<TestBackend>, needle: &str) -> (usize, Vec<Color>) {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    for y in 0..area.height {
        let mut text = String::new();
        let mut bgs = Vec::new();
        for x in 0..area.width {
            let cell = buf.cell((x, y)).unwrap();
            text.push_str(cell.symbol());
            bgs.push(cell.bg);
        }
        if let Some(col) = text.find(needle) {
            return (col, bgs);
        }
    }
    panic!("no row carries {needle:?}");
}

/// pi's `toolPendingBg` — the tint the default shell paints and the self shell must not.
fn pending_bg() -> Color {
    UiTheme::dark()
        .backgrounds()
        .tool_pending
        .expect("the dark theme defines toolPendingBg")
}

/// The definition's `renderShell: "self"` drops the shell: no `toolPendingBg` anywhere on the
/// row, and the call header starts at column 0 (a bare `Container`, not a `Box(1, 1)`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_rendered_definition_omits_the_default_shell() {
    let (_tmp, session) = session_with_framed_tools().await;
    let mut app = app();
    app.refresh_known_tool_definitions(&session);

    app.ingest_session_event_owned(
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call-self"),
            tool_name: "self_framed".into(),
            args: json!({ "x": 1 }),
        },
        &session,
    )
    .await;
    app.draw().unwrap();

    let (col, bgs) = row_with(&app, "self_framed");
    assert_eq!(
        col, 0,
        "a self-rendered row is a bare Container: no Box paddingX (tool-execution.ts:76,237-246)"
    );
    assert!(
        !bgs.contains(&pending_bg()),
        "a self-rendered row carries none of the shell's toolPendingBg (tool-execution.ts:275-277)"
    );
}

/// The control: a definition with no `renderShell` keeps pi's default — the tinted, padded shell.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_definition_keeps_the_state_tinted_shell() {
    let (_tmp, session) = session_with_framed_tools().await;
    let mut app = app();
    app.refresh_known_tool_definitions(&session);

    app.ingest_session_event_owned(
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call-shell"),
            tool_name: "shell_framed".into(),
            args: json!({ "x": 1 }),
        },
        &session,
    )
    .await;
    app.draw().unwrap();

    let (col, bgs) = row_with(&app, "shell_framed");
    assert_eq!(
        col, 1,
        "the default shell is a Box(1, 1): one column of padding"
    );
    assert!(
        bgs.iter().all(|bg| *bg == pending_bg()),
        "the default shell paints toolPendingBg across the whole row"
    );
}

/// The LIVE per-tool-start lookup (not only the bind-time refresh) answers the shell too: an app
/// that never refreshed its memo still resolves the definition off the session when the start
/// event passes through `ingest_session_event_owned`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_tool_start_lookup_resolves_the_shell_without_a_bind_refresh() {
    let (_tmp, session) = session_with_framed_tools().await;
    let mut app = app();

    app.ingest_session_event_owned(
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call-self-2"),
            tool_name: "self_framed".into(),
            args: json!({}),
        },
        &session,
    )
    .await;
    app.draw().unwrap();

    let (col, bgs) = row_with(&app, "self_framed");
    assert_eq!(col, 0);
    assert!(!bgs.contains(&pending_bg()));
}
