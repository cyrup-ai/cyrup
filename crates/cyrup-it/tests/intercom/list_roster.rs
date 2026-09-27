//! ICOM-066 — `list` / `list-cwd` results carry `details.roster`, and the production renderer
//! collapses them to one line (pi-intercom v0.14.0 `a0cc5a1`, #127; `index.ts:2280`, `:2334`,
//! `:2730-2737`).
//!
//! Driven end to end: a real `cyrup-intercom-broker` process, three sessions registered over its
//! socket, the production `intercom` tool's `list` / `list-cwd` actions, and the result they return
//! handed to the production `IntercomExtension`'s `render_result_under` — the hook the TUI
//! re-invokes whenever the expand toggle moves. The model-visible text is asserted unchanged: the
//! collapse is display-only upstream.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate};
use cyrup_ext::{NativeExtension, RenderOptions};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::IntercomClient;

use super::common::{Broker, registration};

fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            Content::Text { text, .. } => text.to_string(),
            _ => String::new(),
        })
        .collect()
}

/// The `{content, details}` payload the host hands a tool renderer
/// (`cyrup-tui` `tool_result_payload`, pi `tool-execution.ts:307-308`).
fn payload(result: &ToolResult) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text_of(result) }],
        "details": result.details,
    })
}

fn rendered(ext: &IntercomExtension, result: &ToolResult, expanded: bool) -> String {
    let opts = RenderOptions {
        expanded,
        ..Default::default()
    };
    ext.render_result_under("intercom", &payload(result), &opts)
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("the intercom tool renders its own result")
}

async fn connect(broker: &Broker, name: &str, cwd: &str) -> Arc<IntercomClient> {
    let mut reg = registration(name);
    reg.cwd = cwd.to_string();
    Arc::new(
        IntercomClient::connect(&broker.socket, reg, Some(format!("{name}-session")))
            .await
            .expect("connects"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_and_list_cwd_collapse_to_a_one_line_roster() {
    let broker = Broker::start().await;
    let me = connect(&broker, "me", "/tmp/work").await;
    let _peer = connect(&broker, "peer", "/tmp/work").await;
    let _away = connect(&broker, "away", "/tmp/elsewhere").await;

    let state = Arc::new(SharedIntercomState::new(
        IntercomConfig::default(),
        600_000,
        PathBuf::from("/tmp/work"),
    ));
    state.set_client(Some(me));
    let tool = IntercomTool::new(state);
    let ext = IntercomExtension::new(
        PathBuf::from("/nonexistent-agent-dir"),
        PathBuf::from("/tmp/work"),
        IntercomConfig::default(),
        None,
    )
    .expect("build the intercom extension");
    let sink = || -> Box<dyn FnMut(ToolUpdate) + Send + 'static> { Box::new(|_| {}) };

    // `list`: `{ peers: otherSessions.length, total: sessions.length }` — no `cwd`.
    let list = tool
        .execute(
            ToolCallId::from("list"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            sink(),
        )
        .await
        .expect("list succeeds");
    assert_eq!(
        list.details,
        Some(serde_json::json!({ "roster": { "peers": 2, "total": 3 } }))
    );
    assert_eq!(
        rendered(&ext, &list, false),
        "✓ 2 other sessions (3 connected)"
    );
    // Expanded, and for the model, the full roster is still there.
    let full = rendered(&ext, &list, true);
    assert!(
        full.contains("Other sessions:") && full.contains("• away"),
        "{full}"
    );
    assert!(
        text_of(&list).contains("• peer"),
        "the model's text is unchanged"
    );

    // `list-cwd`: the RESOLVED filter cwd rides along, so the one line names the directory.
    let list_cwd = tool
        .execute(
            ToolCallId::from("list-cwd"),
            serde_json::json!({ "action": "list-cwd" }),
            CancelToken::new(),
            sink(),
        )
        .await
        .expect("list-cwd succeeds");
    assert_eq!(
        list_cwd.details,
        Some(serde_json::json!({ "roster": { "peers": 1, "total": 3, "cwd": "/tmp/work" } }))
    );
    assert_eq!(
        rendered(&ext, &list_cwd, false),
        "✓ 1 other session in /tmp/work (3 connected)"
    );

    let nobody = tool
        .execute(
            ToolCallId::from("list-cwd-empty"),
            serde_json::json!({ "action": "list-cwd", "cwd": "/tmp/nowhere" }),
            CancelToken::new(),
            sink(),
        )
        .await
        .expect("list-cwd succeeds");
    assert_eq!(
        rendered(&ext, &nobody, false),
        "✓ no other sessions in /tmp/nowhere (3 connected)"
    );
}
