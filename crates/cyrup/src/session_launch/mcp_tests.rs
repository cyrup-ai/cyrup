//! The MCP surface of a launched session, over the real launch wiring and a real stdio server.
//!
//! `build_factory` attaches the MCP extension, `codemode` and `tool_search` exactly as every mode
//! does; the server is a small `sh` script speaking MCP over stdio (the shape `cyrup-mcp`'s own
//! live tests use), so nothing about the connection, the discovery or the call is faked. The model
//! is scripted: its one reply is a `codemode` call whose script is the thing under test.
//!
//! What each test pins, and the pi behaviour behind it (`extensions/mcp/index.ts`,
//! `extensions/mcp/tools.ts` and `core/agent-session.ts` @v1.0.4):
//!
//! * a script waits for a server that is still starting, then sees its tools (`pi.on("tool_call")`);
//! * every MCP tool resolves to the server's `CallToolResult` (`convertMcpResult`);
//! * `--tools` / `--exclude-tools` entries are patterns, and an allowlist that names no MCP tool
//!   leaves them registered (`_isAllowedTool`, `_isActivatable`);
//! * `--no-mcp` (`disabledBuiltinExtensions: ["mcp"]`);
//! * `codemode.mode: only` hides the tools from the model and lists them in the description.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_core::StopReason;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig};
use serde_json::{Value, json};

use super::tests::{factory_with, last_tool_result};

/// A stdio MCP server as an `sh` script. `$1` is the number of seconds it takes to start answering
/// (a first `npx` download). It serves four tools:
///
/// * `echo` answers `echoed:<text>`;
/// * `pic` answers a text block and an image block;
/// * `boom` answers `isError: true`;
/// * `data` answers `structuredContent`.
///
/// Free of `${`, `$env:` and `{env:`: the adapter interpolates `args`, and any of those would let
/// it rewrite the script.
const SERVER: &str = r#"
sleep "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fx","version":"1"}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},{"name":"pic","description":"a picture","inputSchema":{"type":"object","properties":{}}},{"name":"boom","description":"fails","inputSchema":{"type":"object","properties":{}}},{"name":"data","description":"structured","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      case "$line" in
        *'"name":"pic"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"a picture"},{"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}]}}\n' "$id"
          ;;
        *'"name":"boom"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"it broke"}],"isError":true}}\n' "$id"
          ;;
        *'"name":"data"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"rows"}],"structuredContent":{"rows":2}}}\n' "$id"
          ;;
        *)
          text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed:%s"}]}}\n' "$id" "$text"
          ;;
      esac
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

/// What a run recorded: the tools each provider request declared, with their descriptions.
type Requests = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// A provider whose first reply is a `codemode` call running `script` and whose second ends the
/// turn, recording what each request declared.
fn scripted(requests: &Requests, script: &str) -> Arc<FauxProvider> {
    let replies = [Some(script.to_string()), None];
    let steps = replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(
                    ctx.tools
                        .iter()
                        .map(|tool| (tool.name.clone(), tool.description.clone()))
                        .collect(),
                );
                match &reply {
                    Some(code) => faux_assistant_message(
                        vec![faux_tool_call(
                            "codemode".to_string(),
                            json!({ "code": code }),
                        )],
                        StopReason::ToolUse,
                    ),
                    None => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
                }
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

/// One run of a launched session against the `fx` server.
struct Run {
    runtime: Arc<AgentSessionRuntime>,
    /// What the script returned (`JSON.stringify(...)`), parsed.
    answer: Value,
    /// The tool names each request declared.
    declared: Vec<Vec<String>>,
    /// The `codemode` description in each request that declared the tool.
    codemode_descriptions: Vec<String>,
    _tmp: tempfile::TempDir,
}

/// What a test chooses about the world: the server's entry beyond its command, `settings.json`,
/// the seconds the server takes to start, and the launch flags.
struct World {
    delay: u32,
    entry: Value,
    settings: Value,
}

impl World {
    fn eager(delay: u32) -> Self {
        Self {
            delay,
            entry: json!({ "directTools": true }),
            settings: json!({}),
        }
    }
}

/// Launch a session in `world` (flags applied by `configure`), let the model run `script`, and
/// return what it saw.
async fn run(world: World, script: &str, configure: impl FnOnce(&mut SessionConfig)) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut entry = json!({
        "command": "sh",
        "args": ["-c", SERVER, "sh", world.delay.to_string()],
        // A resident connection: a plain `lazy` server is discovered and then closed.
        "lifecycle": "keep-alive",
    });
    if let (Some(base), Some(extra)) = (entry.as_object_mut(), world.entry.as_object()) {
        base.extend(extra.clone());
    }
    std::fs::write(
        agent_dir.join("mcp.json"),
        json!({ "mcpServers": { "fx": entry } }).to_string(),
    )
    .unwrap();
    std::fs::write(agent_dir.join("settings.json"), world.settings.to_string()).unwrap();

    let requests: Requests = Arc::default();
    let provider = scripted(&requests, script);
    let (factory, target) = factory_with(provider, &agent_dir, &cwd, false, configure);
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    let session = runtime.session().await;
    let _ = session.prompt("run the script").await.unwrap();
    session.wait_for_idle().await;

    let (content, _) = last_tool_result(&session, "codemode").await;
    let text = content
        .iter()
        .filter_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let start = text
        .find('{')
        .unwrap_or_else(|| panic!("no JSON answer in {text:?}"));
    let end = text
        .rfind('}')
        .unwrap_or_else(|| panic!("no JSON answer in {text:?}"));
    let answer: Value = serde_json::from_str(&text[start..=end])
        .unwrap_or_else(|e| panic!("script answer is not JSON ({e}): {text:?}"));

    let requests = requests.lock().unwrap().clone();
    let codemode_descriptions = requests
        .iter()
        .filter_map(|tools| {
            tools
                .iter()
                .find(|(name, _)| name == "codemode")
                .map(|(_, description)| description.clone())
        })
        .collect();
    let declared = requests
        .into_iter()
        .map(|tools| tools.into_iter().map(|(name, _)| name).collect())
        .collect();
    Run {
        runtime,
        answer,
        declared,
        codemode_descriptions,
        _tmp: tmp,
    }
}

/// Activate `codemode` the way a user does, with the `defaultTools` setting.
fn with_codemode() -> Value {
    json!({ "defaultTools": ["+codemode"] })
}

/// A script that lists the server's tools the way a model finds them, and says whether `tools`
/// offers each one.
const LIST_SCRIPT: &str = r#"
    const listed = ALL_TOOLS.map((t) => t.name).filter((n) => n.startsWith("fx_")).sort();
    const offered = {};
    for (const name of ["fx_echo", "fx_pic", "fx_boom", "fx_data"]) offered[name] = name in tools;
    return JSON.stringify({ listed, offered });
"#;

/// A first script runs while a cold server is still starting, as in `cyrup -p`: the tool does not
/// exist yet, and nothing the model does inside that call can change it. The script waits for the
/// server it names (`pi.on("tool_call")` @v1.0.4), then finds its tools.
///
/// The server takes longer than the 5 s the dispatcher gives the `input` handler to wait for the
/// build, which is the only wait a turn had before: that one is cut, and the script's is not.
/// Killing mutation: `tool_call` is not subscribed — the script runs ahead of the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_script_waits_for_a_slow_mcp_server_and_then_sees_its_tools() {
    let run = run(
        World {
            settings: with_codemode(),
            ..World::eager(7)
        },
        LIST_SCRIPT,
        |_| {},
    )
    .await;
    assert_eq!(
        run.answer["listed"],
        json!(["fx_boom", "fx_data", "fx_echo", "fx_pic"]),
        "ALL_TOOLS lacks the server that was still starting: {}",
        run.answer
    );
    assert_eq!(
        run.answer["offered"]["fx_echo"],
        json!(true),
        "{}",
        run.answer
    );
}

/// Every MCP tool resolves to the server's `CallToolResult` in a script, eager ones included
/// (`convertMcpResult` + `outputSchema: createMcpResultSchema(...)` @v1.0.4): the image block is
/// there, `isError` resolves instead of rejecting, `structuredContent` comes through.
/// Killing mutation: an eager tool declares no output schema — a script gets its text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_mcp_tool_resolves_to_the_servers_call_tool_result() {
    let run = run(
        World {
            settings: with_codemode(),
            ..World::eager(0)
        },
        r#"
        const out = {};
        out.echo = await tools.fx_echo({ text: "hi" });
        out.pic = await tools.fx_pic({});
        try { out.boom = await tools.fx_boom({}); } catch (e) { out.boomThrew = String(e); }
        out.data = await tools.fx_data({});
        return JSON.stringify(out);
        "#,
        |_| {},
    )
    .await;
    assert_eq!(
        run.answer["echo"],
        json!({ "content": [{ "type": "text", "text": "echoed:hi" }] })
    );
    assert_eq!(
        run.answer["pic"]["content"],
        json!([
            { "type": "text", "text": "a picture" },
            { "type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png" }
        ]),
        "the image block reaches the script"
    );
    assert_eq!(
        run.answer["boom"],
        json!({ "content": [{ "type": "text", "text": "it broke" }], "isError": true }),
        "a server `isError` result resolves: {}",
        run.answer
    );
    assert!(run.answer.get("boomThrew").is_none(), "{}", run.answer);
    assert_eq!(
        run.answer["data"]["structuredContent"],
        json!({ "rows": 2 })
    );
}

/// `--tools` and `--exclude-tools` entries are patterns, and an allowlist with no `mcp__` entry
/// leaves MCP tools registered but not declared (`_isAllowedTool`, `_isActivatable` @v1.0.4): a
/// `directTools` (eager, `direct`) tool the list does not name is neither declared nor callable,
/// one it matches is both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tools_pattern_activates_the_mcp_tools_it_matches() {
    let run = run(World::eager(0), LIST_SCRIPT, |cfg| {
        cfg.tools = Some(vec!["codemode".into(), "fx_e*".into()]);
    })
    .await;
    assert_eq!(
        run.answer["offered"]["fx_echo"],
        json!(true),
        "{}",
        run.answer
    );
    assert_eq!(
        run.answer["offered"]["fx_pic"],
        json!(false),
        "kept without being named, so inactive, and a `direct` tool is callable only while active: {}",
        run.answer
    );
    // The server registers its tools while the script waits, so the request after the call is the
    // first that can declare them.
    let last = run.declared.last().expect("a request");
    assert!(last.contains(&"fx_echo".to_string()), "{last:?}");
    assert!(!last.contains(&"fx_pic".to_string()), "{last:?}");
}

/// A search-mode (`deferred`) server's tools stay callable from scripts under an allowlist that
/// names no MCP tool, and are never declared: the allowlist is for what the MODEL sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowlist_without_an_mcp_entry_leaves_search_mode_tools_to_scripts() {
    let run = run(
        World {
            entry: json!({ "directTools": "search" }),
            ..World::eager(0)
        },
        r#"
        const echo = "fx_echo" in tools ? await tools.fx_echo({ text: "hi" }) : null;
        return JSON.stringify({ echo });
        "#,
        |cfg| cfg.tools = Some(vec!["codemode".into()]),
    )
    .await;
    assert_eq!(
        run.answer["echo"],
        json!({ "content": [{ "type": "text", "text": "echoed:hi" }] }),
        "{}",
        run.answer
    );
    for request in &run.declared {
        assert!(
            request.iter().all(|name| !name.starts_with("fx_")),
            "a search-mode tool was declared: {request:?}"
        );
    }
}

/// An allowlist entry for MCP tools (`mcp__*`) makes the list decide for them: the server's tools,
/// which it does not match, are dropped from the registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_mcp_entry_in_the_allowlist_drops_the_mcp_tools_it_does_not_match() {
    let run = run(
        World {
            entry: json!({ "directTools": "search" }),
            ..World::eager(0)
        },
        LIST_SCRIPT,
        |cfg| cfg.tools = Some(vec!["codemode".into(), "mcp__other__*".into()]),
    )
    .await;
    assert_eq!(run.answer["listed"], json!([]), "{}", run.answer);
    let session = run.runtime.session().await;
    assert!(
        !session
            .all_tools()
            .iter()
            .any(|row| row.name.starts_with("fx_")),
        "registered under an allowlist that decides for MCP tools"
    );
}

/// `--exclude-tools` applies to MCP tools, patterns included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exclude_tools_patterns_remove_mcp_tools() {
    let run = run(
        World {
            settings: with_codemode(),
            ..World::eager(0)
        },
        LIST_SCRIPT,
        |cfg| cfg.exclude_tools = vec!["fx_p*".into(), "fx_boom".into()],
    )
    .await;
    assert_eq!(
        run.answer["listed"],
        json!(["fx_data", "fx_echo"]),
        "{}",
        run.answer
    );
}

/// `--no-mcp` (`disabledBuiltinExtensions: ["mcp"]`, `main.ts` @v1.0.4): the built-in is not
/// loaded, no server connects and a script finds no MCP tool. The same launch without the flag is
/// the control: [`a_first_script_waits_for_a_slow_mcp_server_and_then_sees_its_tools`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_mcp_loads_no_mcp_extension_and_no_server_connects() {
    let run = run(
        World {
            settings: with_codemode(),
            ..World::eager(0)
        },
        LIST_SCRIPT,
        |cfg| {
            cfg.disabled_builtin_extensions
                .push(cyrup_mcp::EXTENSION_ID.to_string());
        },
    )
    .await;
    assert_eq!(run.answer["listed"], json!([]), "{}", run.answer);
    let session = run.runtime.session().await;
    let loaded: Vec<String> = session
        .services()
        .ext_host
        .loaded_ids()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        !loaded.iter().any(|id| id == cyrup_mcp::EXTENSION_ID),
        "the MCP extension is loaded under --no-mcp: {loaded:?}"
    );
    assert!(
        loaded.iter().any(|id| id == "codemode"),
        "only MCP is off, not the other built-ins: {loaded:?}"
    );
    assert!(
        !session.all_tools().iter().any(|row| row.name == "mcp"),
        "the `mcp` gateway is registered"
    );
}

/// `codemode.mode: only` with MCP tools, end to end through the real session builder: the model is
/// never shown the server's tools, the `codemode` description lists them once they are registered,
/// and a script calls them. The first script names the server, so it waits for it; the description
/// the NEXT request carries is the one that lists the tools.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_only_mode_mcp_tools_are_listed_for_scripts_and_hidden_from_the_model() {
    let run = run(
        World {
            settings: json!({ "defaultTools": ["+codemode"], "codemode": { "mode": "only" } }),
            ..World::eager(1)
        },
        r#"
        const echo = await tools.fx_echo({ text: "only" });
        return JSON.stringify({ echo });
        "#,
        |_| {},
    )
    .await;
    assert_eq!(
        run.answer["echo"],
        json!({ "content": [{ "type": "text", "text": "echoed:only" }] }),
        "{}",
        run.answer
    );
    for request in &run.declared {
        assert!(
            request.iter().all(|name| !name.starts_with("fx_")),
            "`only` mode declared an MCP tool to the model: {request:?}"
        );
        assert!(request.contains(&"codemode".to_string()), "{request:?}");
    }
    let last = run
        .codemode_descriptions
        .last()
        .expect("a codemode description");
    assert!(
        last.contains("fx_echo"),
        "the description lists the tool the model reaches through scripts: {last}"
    );
}
