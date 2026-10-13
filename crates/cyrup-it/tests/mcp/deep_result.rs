//! **MCP-637, measured on a real session**: how much deeper a tool result's session entry nests than
//! the MCP reply that carried it, and whether the deepest reply cyrup reads comes back on resume.
//!
//! The transport reads a reply nested at most 127 levels (`cyrup_mcp::depth_guard::MAX_READ_DEPTH`,
//! serde_json's limit) and refuses a deeper one at once. What it reads goes on: the raw
//! `CallToolResult` is recorded in the tool result's `details.mcpResult`, and the session loader
//! (`cyrup-session` `manager/load.rs`) reads each line with the same serde_json limit and skips a
//! line it cannot parse. A real direct call through a real turn, and the session file read back,
//! measure both ends:
//!
//! * `fits` answers with a reply nested 125 levels, whose result nests 124 — the deepest
//!   `details.mcpResult` kept verbatim (`cyrup_mcp::renderers::MCP_RESULT_MAX_DEPTH`). Its session
//!   line nests 127: **the entry is 2 levels deeper than the reply** (the line, `message` and
//!   `details` hold the result, which the reply holds under its line and `result`), and 3 deeper
//!   than the result (`cyrup_mcp::renderers::SESSION_DETAILS_DEPTH`).
//! * `edge` answers with a reply nested 127 levels. Kept verbatim, its line nested 129, and the
//!   session loader skipped it on resume: the tool result vanished. Its `details.mcpResult` is now a
//!   summary that says how deep the result was, and the line reloads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_mcp::McpExtension;
use cyrup_mcp::depth_guard::{MAX_READ_DEPTH, nesting_depth};
use cyrup_mcp::renderers::{MCP_RESULT_MAX_DEPTH, SESSION_DETAILS_DEPTH};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, faux_assistant_message, faux_text, faux_tool_call_with_id,
};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

const SERVER: &str = "deep";

/// A stdio server with two tools: `tools/call` for `fits` answers with the result in the file `$2`,
/// for anything else with the result in `$3`, verbatim.
const DEEP_MCP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"deep","version":"1"}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"fits","inputSchema":{"type":"object"}},{"name":"edge","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*'"name":"fits"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":' "$id"; tr -d '\n' < "$1"; printf '}\n'
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":' "$id"; tr -d '\n' < "$2"; printf '}\n'
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

/// A `CallToolResult` nested `depth` levels: a text block, and `structuredContent` `{"v":[[…1…]]}`.
fn result_nested(label: &str, depth: usize) -> String {
    let arrays = depth - 2;
    let result = format!(
        r#"{{"content":[{{"type":"text","text":"{label}"}}],"structuredContent":{{"v":{}1{}}}}}"#,
        "[".repeat(arrays),
        "]".repeat(arrays)
    );
    assert_eq!(nesting_depth(result.as_bytes()), depth);
    result
}

/// The session line holding the tool result of `call_id`.
fn result_line<'a>(lines: &'a [String], call_id: &str) -> &'a str {
    lines
        .iter()
        .find(|line| {
            line.contains(r#""role":"toolResult""#)
                && line.contains(&format!(r#""toolCallId":"{call_id}""#))
        })
        .unwrap_or_else(|| panic!("the session file records the result of {call_id}"))
}

fn write_reply(dir: &Path, name: &str, result: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, result).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_as_deep_as_cyrup_reads_is_persisted_and_reloaded() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    // A reply is its line and `result` around the result: 1 + the result's nesting.
    let fits = result_nested("fits", MCP_RESULT_MAX_DEPTH);
    let edge = result_nested("edge", MAX_READ_DEPTH - 1);
    let fits_file = write_reply(tmp.path(), "fits.json", &fits);
    let edge_file = write_reply(tmp.path(), "edge.json", &edge);
    std::fs::write(
        agent_dir.join("mcp.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {
                SERVER: {
                    "command": "sh",
                    "args": [
                        "-c",
                        DEEP_MCP,
                        "sh",
                        fits_file.to_string_lossy(),
                        edge_file.to_string_lossy(),
                    ],
                    "directTools": true,
                    "lifecycle": "keep-alive",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![
                faux_tool_call_with_id(
                    "deep_fits",
                    serde_json::json!({}),
                    Some("call-fits".to_string()),
                ),
                faux_tool_call_with_id(
                    "deep_edge",
                    serde_json::json!({}),
                    Some("call-edge".to_string()),
                ),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let ext = McpExtension::with_config(
        cyrup_mcp::dirs::McpDirs::new(agent_dir.clone(), cwd.clone()),
        None,
    )
    .with_home(agent_dir.clone())
    .into_arc();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config)
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    session.bind_extensions().await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while ext.init_task().is_some() || ext.state().is_none() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the build commits");

    let _stream = session.prompt("call both").await.expect("prompt accepted");
    session.wait_for_idle().await;

    let path = session
        .session_file()
        .await
        .expect("the session is on disk");
    let lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();

    // The measurement: a kept result's entry nests 2 deeper than its reply, 3 deeper than itself.
    let fits_reply = 1 + MCP_RESULT_MAX_DEPTH;
    let fits_line = nesting_depth(result_line(&lines, "call-fits").as_bytes());
    assert_eq!(
        fits_line,
        fits_reply + 2,
        "the entry nests 2 deeper than the reply"
    );
    assert_eq!(fits_line, MCP_RESULT_MAX_DEPTH + SESSION_DETAILS_DEPTH);
    assert_eq!(
        fits_line, MAX_READ_DEPTH,
        "the deepest line the loader reads"
    );

    // The deepest reply the transport reads: summarized, so its line is one the loader reads.
    let edge_line = result_line(&lines, "call-edge");
    assert!(
        nesting_depth(edge_line.as_bytes()) <= MAX_READ_DEPTH,
        "the 127-level reply's entry nests {} levels, past what the session loader reads",
        nesting_depth(edge_line.as_bytes())
    );
    assert!(
        edge_line.contains(&format!(r#""nestingDepth":{}"#, MAX_READ_DEPTH - 1)),
        "the summary says how deep the result was: {}",
        edge_line.chars().take(600).collect::<String>()
    );

    // Reloaded as a resume reads it: both results are there.
    let reloaded = cyrup_session::SessionManager::open(&path).expect("the session reloads");
    let entries = serde_json::to_string(reloaded.entries()).unwrap();
    for call_id in ["call-fits", "call-edge"] {
        assert!(
            entries.contains(&format!(r#""toolCallId":"{call_id}""#)),
            "the result of {call_id} vanished from the session on reload"
        );
    }
}
