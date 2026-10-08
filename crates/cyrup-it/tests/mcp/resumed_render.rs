//! **TUI-146's verify paragraph, end to end** (`docs/gap-analysis/07-cyrup-tui.md`, the `TUI-146`
//! row): replay a history that calls an MCP tool which is not registered, and the transcript shows
//! a bounded call and result.
//!
//! The case is a resumed session whose MCP server has not connected. The adapter registers its
//! direct tools from `mcp-cache.json` at `init`, but only from a VALID entry; an entry whose config
//! hash changed, whose server-declared `ttlMs` ran out, or whose listing was `cacheScope: "private"`
//! registers nothing until the server connects. The history drawn on `/resume` can still call those
//! tools. pi renders such a call with the MCP renderers, through `pi.registerToolRenderer`
//! (`coding-agent/src/extensions/mcp/index.ts:383-386` @ce950d78f, `11449730c`, #10285), and keeps
//! the unbounded `formatToolExecution` only for a tool nothing knows
//! (`modes/interactive/components/tool-execution.ts:396-407` @ce950d78f). Both halves are asserted
//! here against a real session, the production adapter, and the TUI's replay walk.
//!
//! The fixture uses a `private` entry because it is the one stale shape the adapter never tries to
//! rediscover at startup (`runtime.rs`, the `needs_discovery` split), so nothing is spawned; the
//! `lazy` lifecycle and the `/nonexistent` command make that doubly sure.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{ApiId, AssistantMessage, Content, Message, ProviderId, StopReason, ToolCall};
use cyrup_mcp::McpExtension;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::agent_message::AgentMessage;
use cyrup_session_svc::{ReplayItem, SessionBuilder, SessionConfig};
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::json;
use tempfile::TempDir;

/// The server whose cached tool the history calls; `notes_search` is its `server`-prefixed name.
const SERVER: &str = "notes";
const MCP_TOOL: &str = "notes_search";
/// A name nothing registers and nothing renders: pi's generic unknown-tool case.
const UNKNOWN_TOOL: &str = "nobody_knows_this";
/// Output lines in each replayed result — far past every collapsed preview.
const OUTPUT_LINES: usize = 120;

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
    std::fs::write(
        agent_dir.join("mcp.json"),
        json!({
            "mcpServers": {
                SERVER: {
                    "command": "/nonexistent/tui-146-never-spawned",
                    "lifecycle": "lazy",
                    "directTools": true
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        agent_dir.join("mcp-cache.json"),
        json!({
            "version": cyrup_mcp::registration::METADATA_CACHE_VERSION,
            "servers": {
                SERVER: {
                    "configHash": "0".repeat(64),
                    "cachedAt": 1,
                    "cacheScope": "private",
                    "tools": [{ "name": "search", "description": "Search the notes." }]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn assistant_calling(calls: &[(&str, &str)], query: &str) -> ReplayItem {
    let mut assistant = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::ToolUse,
        String::new(),
    );
    assistant.error_message = None;
    assistant.content = calls
        .iter()
        .map(|(id, name)| {
            Content::ToolCall(ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: json!({ "query": query })
                    .as_object()
                    .cloned()
                    .unwrap()
                    .into(),
                thought_signature: None,
                namespace: None,
            })
        })
        .collect();
    ReplayItem::Message(Box::new(AgentMessage::Core(Message::Assistant(assistant))))
}

fn result(id: &str, name: &str, prefix: &str) -> ReplayItem {
    let text = (0..OUTPUT_LINES)
        .map(|i| format!("{prefix} line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    ReplayItem::Message(Box::new(AgentMessage::Core(Message::ToolResult {
        tool_call_id: id.into(),
        tool_name: name.into(),
        content: vec![Content::Text {
            text: text.into(),
            text_signature: None,
        }],
        is_error: false,
        details: None,
        timestamp: 0,
        usage: None,
        added_tool_names: Vec::new(),
        nested_calls: None,
    })))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_call_to_an_unconnected_mcp_tool_replays_bounded() {
    let fx = fixture();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let dirs = cyrup_mcp::dirs::McpDirs::new(fx.agent_dir.clone(), fx.cwd.clone());
    let adapter = McpExtension::with_config(dirs, None)
        .with_home(fx.agent_dir.clone())
        .into_arc() as Arc<dyn cyrup_ext::NativeExtension>;
    let faux = Arc::new(FauxProvider::new()) as Arc<dyn Provider>;
    let session = SessionBuilder::new(faux, cfg)
        .with_native_extension(adapter)
        .build()
        .await
        .unwrap();

    // The precondition that makes this TUI-146 rather than an ordinary MCP call: the tool is NOT
    // registered — its server has not connected and its cache entry is not valid.
    let names: Vec<String> = session.all_tools().into_iter().map(|t| t.name).collect();
    assert!(
        !names.iter().any(|n| n == MCP_TOOL),
        "the stale entry registers no tool; got: {names:?}"
    );

    let long_query = "q".repeat(4_000);
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.replay_items_with_extensions(
        &[
            assistant_calling(
                &[("call-mcp", MCP_TOOL), ("call-x", UNKNOWN_TOOL)],
                &long_query,
            ),
            result("call-mcp", MCP_TOOL, "mcp"),
            result("call-x", UNKNOWN_TOOL, "unknown"),
        ],
        session.ext_host(),
    )
    .await;
    app.draw().unwrap();
    let text = app.scrollback_text();

    // The MCP call: drawn by the adapter's renderers, so the arguments are capped and the output
    // is a collapsed preview.
    assert!(text.contains(MCP_TOOL), "{text}");
    assert!(
        text.contains("mcp line 0"),
        "the preview starts the output:\n{text}"
    );
    assert!(
        !text.contains(&format!("mcp line {}", OUTPUT_LINES - 1)),
        "the MCP result is collapsed, not the whole output:\n{text}"
    );
    let mcp_query_chars = text
        .lines()
        .take_while(|line| !line.contains(UNKNOWN_TOOL))
        .map(|line| line.matches('q').count())
        .sum::<usize>();
    // `format_mcp_direct_tool_call_lines` caps the argument JSON at
    // `DEFAULT_MAX_CALL_INPUT_CHARS`; the `"query": "` prefix and the `…` tail eat into that cap,
    // so the drawn `q`s stay at or under it, far short of the 4,000 the call carried.
    assert!(
        mcp_query_chars <= cyrup_mcp::renderers::DEFAULT_MAX_CALL_INPUT_CHARS,
        "the MCP call's arguments are bounded ({mcp_query_chars} chars drawn):\n{text}"
    );
    // And the unknown tool's arguments, by contrast, are drawn whole.
    let unknown_query_chars = text
        .lines()
        .skip_while(|line| !line.contains(UNKNOWN_TOOL))
        .map(|line| line.matches('q').count())
        .sum::<usize>();
    assert!(
        unknown_query_chars >= long_query.len(),
        "{unknown_query_chars} chars drawn:\n{text}"
    );

    // The unknown tool keeps pi's generic shape: every line of its output is committed.
    assert!(
        text.contains(&format!("unknown line {}", OUTPUT_LINES - 1)),
        "an unknown tool still renders through the generic, unbounded shape:\n{text}"
    );
}
