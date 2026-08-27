//! **The LIVE path**, end to end, through the one seam a model actually uses.
//!
//! [`super::activation`] proves the *cached* half of MCP-001: a session built with a fixture
//! `mcp.json` offers the `mcp` gateway tool, and the description it offers was built from that file
//! — with no server contacted, both fixture `command`s pointing at `/nonexistent/…`. It stops
//! exactly where the live path begins, because `SessionBuilder::build()` does not dispatch
//! `SessionStart`; only [`cyrup_session_svc::AgentSession::bind_extensions`] does.
//!
//! This file starts there. It writes an `mcp.json` naming a **real stdio MCP server**, builds the
//! same production-shaped session, calls `bind_extensions()` — and then drives a real turn in which
//! the model issues an `mcp({...})` tool call and the answer comes back off the live runtime.
//!
//! # The chain each test walks, and why every link is unfakeable here
//!
//! 1. `bind_extensions()` → `HostEvent::SessionStart` → `McpExtension::on_session_start`, which
//!    takes MCP-015's [`cyrup_mcp::runtime::ContextSnapshot`] from the dispatch ctx and calls
//!    `start_initialization`.
//! 2. `start_initialization` memoises the build **and spawns its driver** — a `Shared` nobody polls
//!    never runs, and `on_session_start` cannot await it (the native dispatch budget is 5 s and
//!    *drops* the handler future on expiry, which would cancel the subprocess handshakes).
//! 3. `initialize_mcp` spawns the child, completes the MCP `initialize` handshake and records the
//!    connection. The tempdir has no `mcp-cache.json`, so its cold-cache arm sets `bootstrap_all`
//!    and the startup pass connects every enabled server once.
//! 4. The commit tail publishes the state, installs the runtime env (the generation's
//!    `ProxyCtx`), installs MCP-214's dispatcher into the `ToolDispatch` slot every tool registered
//!    at `init` closed over, installs the surface-sync listener and re-syncs the surface.
//! 5. A model-issued `mcp({...})` reaches `ProxyTool::execute`, which finds a **non-empty** dispatch
//!    slot, calls `McpDispatch::call_proxy`, which reads the committed `ProxyCtx` live and routes
//!    into the nine modes against the real `McpState`.
//!
//! Break any one of those and [`a_model_issued_mcp_call_is_answered_by_the_live_runtime`] goes red
//! with `details.error == "not_initialized"` — the answer both `ProxyTool::execute`'s empty-slot arm
//! and the dispatcher's `NotInitialized` gate give, and the single observable that separates "the
//! tool registered" from "the tool works".
//!
//! [`the_gateway_answers_its_stub_without_a_session_start`] is that claim's negative control, and it
//! is here because an end-to-end assertion nobody has watched fail is not evidence. It builds the
//! identical session, withholds `bind_extensions()` and nothing else, and asserts the same tool with
//! the same arguments answers `not_initialized`. One call is the whole difference between the stub
//! and a report on a running child process.
//!
//! # What each test proves, and the two names that are not the same name
//!
//! The fixture server answers `tools/list` with one tool, `echo`, and answers `tools/call` with
//! `echoed:<text>`. Both now reach the model, by both routes:
//!
//! * [`the_live_surface_carries_the_servers_discovered_catalog`] — the connect issues `tools/list`,
//!   the result lands in `state.tool_metadata["fixture"]` and in `mcp-cache.json`, and
//!   `fixture_echo` is registered as a first-class tool in the array the agent hands the model.
//! * [`a_model_issued_direct_tool_call_returns_the_servers_own_result`] — the model calls
//!   `fixture_echo` and the transcript carries `echoed:pong`, off the running child.
//! * [`a_model_issued_gateway_call_returns_the_servers_own_result`] — the same result through the
//!   `mcp({tool: …})` gateway, which is the route a server gets with no `directTools` opt-in.
//!
//! **The catalog name is the prefixed name.** A tool the server calls `echo` is `fixture_echo`
//! everywhere the model can see it, because the default `toolPrefix` is `server`
//! (`cyrup_mcp::config::ToolPrefix::Server`) and both `ToolMetadata::name` and
//! `DirectToolSpec::prefixed_name` are `format_tool_name(tool, server, prefix)`. Every resolver in
//! `proxy/call.rs` compares against `ToolMetadata::name`, so `mcp({tool: "fixture_echo"})` resolves
//! and `mcp({tool: "echo"})` does not — it answers `tool_not_found` with
//! `suggestions: ["fixture_echo"]`, which is upstream's own behaviour and not a residual.
//! [`the_gateway_resolves_the_catalog_name_not_the_wire_name`] pins both halves of that, because a
//! reader who only saw the passing call could reasonably assume either name works.
//!
//! The un-prefixing back to `echo` happens exactly once, downstream, where the invocation reads
//! `tool_meta.original_name` — which is what the fixture server sees on the wire, and what
//! `details.tool` reports back.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{Message, StopReason};
use cyrup_mcp::McpExtension;
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxProvider, FauxResponseStep,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use serde_json::Value;
use tempfile::TempDir;

/// The gateway tool `register_surface` registers (`cyrup_mcp::registration::PROXY_TOOL_NAME`).
const PROXY_TOOL: &str = "mcp";

/// The one server the fixture `mcp.json` configures. Short and greppable: it appears in the
/// `mcp({})` status text the model gets back, which is where the live assertions read it from.
const SERVER: &str = "fixture";

/// The tool that server advertises over `tools/list` and answers over `tools/call`, under the name
/// the SERVER uses. This is the wire name: it appears in the `tools/call` request the child
/// receives and in `details.tool` on the way back, and nowhere the model can see.
const REMOTE_TOOL: &str = "echo";

/// The same tool under the name the MODEL sees — `format_tool_name("echo", "fixture",
/// ToolPrefix::Server)`.
///
/// Written out rather than `format!`-ed so it can be a `const` and so the value a reader has to
/// match against the assertions below is literally in the source.
/// [`the_catalog_name_is_the_prefixed_name`] checks it against the derivation, so the two cannot
/// drift.
const DIRECT_TOOL: &str = "fixture_echo";

/// The fixture's answer to `tools/call`, verbatim: `echoed:` + the `text` argument. Nothing on this
/// side of the pipe can produce this string — it is the child process's own bytes.
const SERVER_ANSWER: &str = "echoed:pong";

/// A real stdio MCP server as an `sh` script — the same fixture runtime `runtime.rs`'s `TINY_MCP`
/// and `server_manager.rs`'s child-process tests already use, so it adds no host dependency to this
/// suite.
///
/// Two marker files, and the pair is the point:
///
/// * `$1` is truncated on the script's **first line**, before it reads a byte of JSON-RPC, so its
///   existence proves a child process really ran and nothing more.
/// * `$2` is truncated when the client sends `notifications/initialized`, which an MCP client sends
///   **only after it has accepted a valid `initialize` result**. Its existence therefore proves the
///   handshake round-tripped — a fact no amount of connection bookkeeping on this side could fake,
///   and the precondition for everything after it: discovery issues `tools/list` on the same
///   session this notification closes, so a `$2` that never appears means every catalog assertion
///   in this file would be asserting on a cache rather than on a server.
///
/// It **echoes back the protocol version the client asked for** rather than naming one. That keeps
/// this file free of an `rmcp` dependency (`cyrup-it` has none, by design) *and* makes the fixture
/// behave like a real server, which negotiates rather than dictates.
///
/// Deliberately free of `${`, `$env:` and `{env:`: `args` are passed through the adapter's
/// `interpolateEnvVars`, and any of those three would let the interpolator rewrite the script
/// itself.
const LIVE_MCP: &str = r#"
: > "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"},"instructions":"the fixture server speaks"}}\n' "$id" "$pv"
      ;;
    *'"method":"notifications/initialized"'*) : > "$2" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed:%s"}],"isError":false}}\n' "$id" "$text"
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    /// `$1` — truncated by [`LIVE_MCP`]'s first line. A child process really ran.
    started: PathBuf,
    /// `$2` — truncated when the client sends `notifications/initialized`. The MCP handshake really
    /// completed, from the server's side of the pipe.
    handshook: PathBuf,
}

/// A temp `<agent_dir>` carrying an `mcp.json` at the USER rung (`McpDirs::user_config`) that names
/// the live fixture server, plus an empty project cwd.
///
/// No `"lifecycle"` key and no `"disabled"`: the entry is an ordinary enabled server, which is what
/// makes the cold-cache bootstrap connect it. `initialize_mcp` finds no `mcp-cache.json` in this
/// tempdir, sets `bootstrap_all`, and the startup pass connects **every** enabled server once —
/// so the connect below needs no `eager` marking to provoke it.
///
/// `"directTools": true` is the one deliberate addition, and it is not a convenience: direct tools
/// are OPT-IN. `registration::resolve_tool_filter` returns `ToolFilter::Off` for a server with
/// neither `directTools` on its entry nor `settings.directTools`, so without this key a connected
/// server contributes its catalog to `state.tool_metadata` and to the `mcp` gateway's own
/// description — but registers no `fixture_echo`, and the model reaches the server only through
/// `mcp({tool: …})`. Both routes matter and this file proves both, so the fixture enables the
/// half that has to be asked for; [`the_gateway_is_the_route_without_the_direct_tools_opt_in`]
/// builds the same server WITHOUT the key and holds that boundary in place.
fn fixture() -> Fixture {
    fixture_with(true)
}

/// [`fixture`], with `"directTools"` under the caller's control.
fn fixture_with(direct_tools: bool) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let started = tmp.path().join("fixture-started");
    let handshook = tmp.path().join("fixture-handshook");

    let config = serde_json::json!({
        "mcpServers": {
            SERVER: {
                "command": "sh",
                "args": [
                    "-c",
                    LIVE_MCP,
                    "sh",
                    started.to_string_lossy(),
                    handshook.to_string_lossy(),
                ],
                "directTools": direct_tools,
            }
        }
    });
    std::fs::write(agent_dir.join("mcp.json"), serde_json::to_string_pretty(&config).unwrap())
        .unwrap();

    Fixture { _tmp: tmp, cwd, agent_dir, started, handshook }
}

/// `SessionConfig::new` already sets `home = agent_dir`, so the session and the adapter agree on one
/// temp home with no env mutation anywhere (docs/TEST-ARCHITECTURE.md §4 R2).
fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The adapter exactly as `crates/cyrup/src/main.rs` attaches it — `McpExtension::with_config(dirs,
/// None)` through `into_arc`, which is `mcp_extension_for_env`'s entire body — with only the home
/// pinned.
///
/// Returned as the concrete `Arc<McpExtension>` **before** the coercion, unlike
/// [`super::activation`]'s `adapter`, because every test here has to poll the extension's own
/// generation slots (`state()`, `proxy_ctx()`, `dispatch()`) to know when the SPAWNED build has
/// committed. The session gets the same `Arc`, coerced at the attach.
fn adapter(fx: &Fixture) -> Arc<McpExtension> {
    let dirs = cyrup_mcp::dirs::McpDirs::new(fx.agent_dir.clone(), fx.cwd.clone());
    McpExtension::with_config(dirs, None).with_home(fx.agent_dir.clone()).into_arc()
}

/// Build the session, attach the adapter, and fire the one event that starts everything.
///
/// `bind_extensions()` is the load-bearing call, not an incidental one: `SessionBuilder::build()`
/// does **not** dispatch `SessionStart` (only `AgentSessionRuntime::create` does, through this same
/// method), so without it `on_session_start` never runs, `start_initialization` is never reached,
/// and no server is ever contacted — which is precisely the state [`super::activation`] asserts.
/// `bind = false` builds the identical session and deliberately withholds that one call — the
/// negative control [`the_gateway_answers_its_stub_without_a_session_start`] needs, and the reason
/// this parameter exists rather than a second copy of the builder.
async fn start_session(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    bind: bool,
) -> (AgentSession, Arc<McpExtension>) {
    let ext = adapter(fx);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(fx))
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    if bind {
        session.bind_extensions().await;
    }
    (session, ext)
}

/// Wait for the SPAWNED build to commit, the fixture server to be live, AND the commit tail to have
/// finished publishing the surface — bounded.
///
/// Polling rather than a handshake because that is what the design is: `start_initialization`
/// returns immediately having spawned its driver, so there is no future here to await. The
/// conditions are checked together on purpose — a committed state whose connection map is still
/// empty means the commit tail ran before the startup connect pass finished, and asserting on that
/// state would be a race.
///
/// `init_task().is_none()` is the third condition and it is what makes the surface assertions
/// deterministic rather than *usually* right. `ext.state()` is published by commit step 2 and the
/// discovered surface by step 6 (`sync_tool_surface`); the memo is cleared by step 8, last of all.
/// Waiting only on the state would let a poller on another worker thread observe the window in
/// between and assert that a server which HAS been discovered has contributed no tools yet — a
/// flake that reads exactly like the bug this file exists to catch.
///
/// Panics naming `init_task` if nothing settles, because a build that never commits is exactly the
/// symptom of a memo that nobody polls.
async fn await_live_connection(ext: &Arc<McpExtension>) -> Arc<cyrup_mcp::state::McpState> {
    let deadline = Duration::from_secs(30);
    let poll = async {
        loop {
            if let Some(state) = ext.state()
                && state.manager.get_connection(SERVER).is_some_and(|connection| {
                    connection.status() == cyrup_mcp::lifecycle::ConnectionStatus::Connected
                })
                && ext.proxy_ctx().is_some()
                && ext.init_task().is_none()
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(deadline, poll).await.unwrap_or_else(|_| {
        panic!(
            "the session start never produced a connected `{SERVER}`: either `on_session_start` \
             did not call `start_initialization`, or the memoised `init_task` was never polled by \
             a spawned driver, or the commit tail dropped the build as stale"
        )
    })
}

/// Wait, bounded, for a file the fixture server creates.
async fn await_file(path: &std::path::Path, why: &str) {
    let poll = async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(10), poll).await.is_err() {
        panic!("{why} — `{}` never appeared", path.display());
    }
}

/// Script one `mcp(<args>)` tool call followed by a closing text turn, drive it, and return the
/// gateway's `ToolResult` as the transcript recorded it.
///
/// Reading the answer out of `session.messages()` rather than calling the dispatcher directly is
/// the point: it is the model's own path — provider → agent loop → tool registry → `ProxyTool` →
/// `ToolDispatch` → `McpDispatch` — and every link of it has to be live for a result to appear.
async fn call_gateway(fx: &Fixture, args: Value) -> (Arc<McpExtension>, GatewayAnswer) {
    call_tool(fx, PROXY_TOOL, args).await
}

/// [`call_gateway`] for any tool on the model's surface — the gateway, or one of the
/// `<server>_<tool>` direct tools discovery registered.
///
/// One helper for both because they are one path: `registration::register_surface` registers
/// `ProxyTool` and `DirectTool` through the same sink, over the same `ToolDispatch` slot, and both
/// reach `McpDispatch`. The only difference is which arm of `McpToolDispatch` the call lands in.
async fn call_tool(fx: &Fixture, tool: &str, args: Value) -> (Arc<McpExtension>, GatewayAnswer) {
    let (ext, answer) = drive_tool_call(fx, tool, args, true).await;

    // THE DISCRIMINATOR, asserted for every LIVE call this helper makes. `not_initialized` is what
    // `ProxyTool::execute` answers from its empty-slot arm and what `McpDispatch`'s gate answers
    // when nothing is committed — so seeing anything else is the proof that the dispatcher was
    // installed by the commit tail AND found this generation's `ProxyCtx`.
    // [`the_gateway_answers_its_stub_without_a_session_start`] is the negative control that keeps
    // this assertion honest: the same tool, the same args, no `SessionStart` — and that answer IS
    // `not_initialized`.
    assert_ne!(
        answer.detail("error").and_then(Value::as_str),
        Some("not_initialized"),
        "the gateway answered its uninitialized stub — the dispatcher never reached a live \
         runtime: {answer:?}"
    );

    (ext, answer)
}

/// [`call_tool`] without the live-runtime assertion, and with `SessionStart` optional.
///
/// `bind = false` is the negative control's path: everything else about the session is identical,
/// so a difference in the answer can only be the `SessionStart` chain.
async fn drive_tool_call(
    fx: &Fixture,
    tool: &str,
    args: Value,
    bind: bool,
) -> (Arc<McpExtension>, GatewayAnswer) {
    // A factory step rather than a static message, for one reason: it records `ctx.tools` — the
    // tool array the AGENT handed the model on that turn, read off the provider request itself.
    //
    // That is the only view that answers the question these tests ask. `session.active_tool_names()`
    // is `DynamicToolState`'s name list, which is the INPUT to the rebuild rather than its result;
    // the extension host's registry is a third view again, and a tool can sit in it while being
    // absent from the array the model was shown. Discovery's whole payoff is a name in `ctx.tools`,
    // so `ctx.tools` is what gets asserted.
    //
    // Both turns are captured because the interesting one is turn 2 in general and turn 1 here:
    // `await_live_connection` below waits out the commit tail, so the connect, the discovery and
    // `sync_tool_surface` have all happened before the first request is built.
    let offered: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let tool = tool.to_string();
    let scripted = tool.clone();

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        {
            let offered = Arc::clone(&offered);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                offered.lock().unwrap().push(ctx.tools.iter().map(|t| t.name.clone()).collect());
                faux_assistant_message(
                    vec![faux_tool_call(&scripted, args.clone())],
                    StopReason::ToolUse,
                )
            })
        },
        {
            let offered = Arc::clone(&offered);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                offered.lock().unwrap().push(ctx.tools.iter().map(|t| t.name.clone()).collect());
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            })
        },
    ]);

    let (session, ext) = start_session(fx, faux, bind).await;
    if bind {
        let _state = await_live_connection(&ext).await;
    }

    // The stream is dropped on purpose: `wait_for_idle` is what settles the turn, and
    // `late_tools.rs` drives a turn the same way.
    let _stream = session.prompt("use the mcp surface").await.expect("prompt accepted");
    session.wait_for_idle().await;

    let offered = offered.lock().unwrap().clone();
    assert!(!offered.is_empty(), "the agent drove at least one real turn against the provider");
    assert!(
        offered[0].iter().any(|name| *name == tool),
        "the AGENT offered `{tool}` to the model, or the scripted call below would be answered by \
         nothing: {:?}",
        offered[0]
    );

    let answer = session
        .messages()
        .await
        .into_iter()
        .find_map(|message| match message {
            Message::ToolResult { tool_name, content, is_error, details, .. }
                if tool_name == tool =>
            {
                Some(GatewayAnswer {
                    text: content
                        .iter()
                        .filter_map(|block| match block {
                            cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    is_error,
                    details,
                    offered: offered.clone(),
                })
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the scripted `{tool}` call must land a tool result in the transcript"));

    (ext, answer)
}

/// One `mcp({...})` answer as the transcript holds it, plus the tool arrays the agent handed the
/// model on each turn of the run that produced it.
#[derive(Debug)]
struct GatewayAnswer {
    text: String,
    is_error: bool,
    details: Option<Value>,
    /// `ctx.tools` per turn — the MODEL-VISIBLE surface, captured from the provider request itself
    /// rather than from any registry view.
    ///
    /// Kept per-turn rather than collapsed to a set because the two turns straddle the tool call,
    /// and a surface rebuild that dropped a tool BETWEEN them would be invisible to a union.
    /// `was_offered` answers "at all"; iterating `offered` directly answers "on every turn", and
    /// [`the_live_surface_carries_the_servers_discovered_catalog`] needs the second one.
    offered: Vec<Vec<String>>,
}

impl GatewayAnswer {
    fn detail(&self, key: &str) -> Option<&Value> {
        self.details.as_ref()?.get(key)
    }

    /// Every tool name the model was offered across the run, deduplicated by presence.
    fn was_offered(&self, name: &str) -> bool {
        self.offered.iter().any(|turn| turn.iter().any(|tool| tool == name))
    }
}

// ================================================================================================
// 1 · The connect.
// ================================================================================================

/// **A server listed in `mcp.json` connects because a session started**, and the commit tail put
/// everything the model needs in place.
///
/// Deliberately separate from the call test below so that a failure says *which* half broke: a red
/// here is the runtime, a red there is the dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_server_connects_when_the_session_starts() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);

    let (session, ext) = start_session(&fx, faux, true).await;
    let state = await_live_connection(&ext).await;

    // (1) A real child process ran. The marker is written by the script's very first line, before
    //     it reads a single byte of JSON-RPC, so it cannot be a side effect of the handshake.
    assert!(fx.started.exists(), "a real child process ran");

    // (2) The MCP handshake really completed, observed from the SERVER's side of the pipe: a
    //     client sends `notifications/initialized` only after accepting a valid `initialize`
    //     result. A `Connected` status recorded without a real handshake — an optimistic write, a
    //     stubbed factory — cannot produce this file.
    //
    //     Polled rather than read once: the notification is a buffered write on the transport that
    //     `serve_client_with_lifecycle_and_ct` performs *after* the `initialize` response it
    //     reports success on, so "connected" and "the server has seen the notification" are two
    //     different instants.
    await_file(
        &fx.handshook,
        "the client sent `notifications/initialized`, so `initialize` round-tripped",
    )
    .await;

    // (3) The cold-cache bootstrap ran, which is what set `bootstrap_all` and made this an
    //     unconditional startup connect rather than a lazy one.
    assert!(
        fx.agent_dir.join("mcp-cache.json").exists(),
        "the cold-cache bootstrap writes the metadata cache before the startup pass"
    );

    // (4) The commit tail: the proxy context, the dispatcher and the surface listener. Each is a
    //     separate link and each has its own failure mode — no context means every call answers
    //     `not_initialized`; no dispatcher means the same from the other side; no listener means a
    //     later `tools/list_changed` or `mcp({connect})` never reaches the model.
    assert!(ext.proxy_ctx().is_some(), "`install_runtime_env` ran");
    assert!(
        ext.dispatch().is_some_and(|slot| slot.is_installed()),
        "MCP-214's executor was installed into the slot every tool registered at `init` shares"
    );
    assert!(
        state.on_tool_metadata_updated.lock().unwrap().is_some(),
        "`install_surface_sync` ran"
    );

    // (5) The memo is cleared, and only after the surface sync — a caller arriving during the sync
    //     window should join a settled build rather than be told there is none.
    assert!(ext.init_task().is_none(), "`initPromise = null` ran at the end of the commit tail");

    // (6) Teardown, through the production path: `dispose` dispatches
    //     `HostEvent::SessionShutdown` to the extension, which routes the outgoing generation
    //     through `shutdown_previous_generation` — begin_stop + shutdown_state + shutdown_oauth.
    //     That drain is what closes the transport, which drops the child's stdin and reaps it.
    session.dispose("quit").await;
    assert!(
        ext.state().is_none() && ext.proxy_ctx().is_none() && ext.owner().is_none(),
        "the shutdown handler takes every generation slot, or the next session starts on top of \
         a live one"
    );
    assert!(
        state.manager.get_connection(SERVER).is_none(),
        "`shutdown_previous_generation` really drains the generation's children — the graceful \
         shutdown closes the connection and removes it from the map"
    );
}

// ================================================================================================
// 2 · The call. THE deliverable's assertion.
// ================================================================================================

/// **A model-issued `mcp({...})` call is answered by the LIVE runtime.**
///
/// The provider is scripted to emit one `mcp({})` tool call. `{}` selects the ninth arm of the
/// gateway router — `execute_status` — which is the mode whose entire answer is read from the
/// generation's live state: `ctx.env.get_connection(name)` goes through `RuntimeEnv` to
/// `McpState.manager` to the real child process's connection record.
///
/// So the assertions below cannot be produced by anything short of the full chain. In particular
/// `connectedCount == 1` and the `✓ fixture` glyph line are *the running subprocess*, reported back
/// to the model through the tool-result path: registration alone yields `not_initialized`, a
/// committed state with no dispatcher yields `not_initialized`, and a dispatcher over a dead or
/// missing `ProxyCtx` yields `not_initialized`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_issued_mcp_call_is_answered_by_the_live_runtime() {
    let fx = fixture();
    let (_ext, answer) = call_gateway(&fx, serde_json::json!({})).await;

    // The gateway answered as a mode, not as an init envelope. The three init envelopes
    // (`not_initialized`, `init_timeout`, `init_failed`) carry no `mode` key at all; every one of
    // the nine modes does.
    assert_eq!(
        answer.detail("mode").and_then(Value::as_str),
        Some("status"),
        "the call reached the nine-arm router's status mode: {answer:?}"
    );
    assert!(!answer.is_error, "a status answer is not an error: {answer:?}");

    // THE LIVE FACTS. `execute_status` computes each row's status from
    // `ctx.env.get_connection(name)`, so `"connected"` here is the real child process's connection
    // record, read through the production `ProxyEnv`.
    assert_eq!(
        answer.detail("connectedCount"),
        Some(&serde_json::json!(1)),
        "the model was told the fixture server is connected: {answer:?}"
    );
    let servers = answer.detail("servers").and_then(Value::as_array).cloned().unwrap_or_default();
    let row = servers
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(SERVER))
        .unwrap_or_else(|| panic!("the status names the configured server: {answer:?}"));
    assert_eq!(
        row.get("status").and_then(Value::as_str),
        Some("connected"),
        "…and reports it CONNECTED, which is a fact about the running child: {answer:?}"
    );

    // The text half is what the model actually reads. `✓` is `execute_status`'s literal connected
    // glyph; `○ … (not connected)` is what a session that registered the tool but never started a
    // runtime would have produced.
    assert!(
        answer.text.contains(&format!("✓ {SERVER}")),
        "the model's text answer carries the connected glyph: {answer:?}"
    );
}

// ================================================================================================
// 3 · The negative control.
// ================================================================================================

/// **The negative control**, without which the test above proves nothing.
///
/// The same fixture, the same session, the same scripted `mcp({})` call — and `bind_extensions()`
/// is never called, so `HostEvent::SessionStart` is never dispatched, `on_session_start` never
/// runs, `start_initialization` is never reached and the `ToolDispatch` slot every registered tool
/// closed over stays empty.
///
/// The gateway then answers `ProxyTool::execute`'s `None` arm: a **successful** `ToolResult`
/// carrying `details.error = "not_initialized"`, never an `Err` — `registration::not_initialized_result`,
/// which is a deliberate divergence from `cyrup-core`'s "tools signal failure with `Err`" because
/// an `Err` would lose the `details` that `error-signal.ts`'s `toolErrorOverride` reads.
///
/// This is what makes the live assertions load-bearing rather than decorative: the difference
/// between this test and the one above is exactly one call, and the answer the model gets changes
/// from a stub to the real state of a running child process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_answers_its_stub_without_a_session_start() {
    let fx = fixture();
    let (ext, answer) = drive_tool_call(&fx, PROXY_TOOL, serde_json::json!({}), false).await;

    assert_eq!(
        answer.detail("error").and_then(Value::as_str),
        Some("not_initialized"),
        "with no `SessionStart` the gateway must answer its uninitialized stub: {answer:?}"
    );
    assert_eq!(answer.text, "MCP not initialized");
    assert!(
        answer.detail("mode").is_none(),
        "the init envelopes carry no `mode` key — only the nine modes do: {answer:?}"
    );

    // …and nothing was built or contacted. Each of these is the direct cause of the answer above.
    assert!(ext.state().is_none(), "no generation was ever built");
    assert!(ext.proxy_ctx().is_none(), "no runtime env was installed");
    assert!(
        ext.dispatch().is_some_and(|slot| !slot.is_installed()),
        "the tools registered at `init` share a dispatch slot that nothing filled"
    );
    assert!(!fx.started.exists(), "no child process was spawned");
}

// ================================================================================================
// 4 · THE DELIVERABLE. A configured server's own tool, called by the model, answered by the server.
// ================================================================================================

/// **The deliverable's own sentence, run: the model calls a configured server's tool and the
/// server's real result comes back.**
///
/// The model issues `mcp({ tool: "fixture_echo", server: "fixture", args: { text: "pong" } })`.
/// That is arm 3 of the gateway router, `execute_call`, and every phase of it runs against the live
/// generation: phase 1 resolves `fixture_echo` in `state.tool_metadata["fixture"]` — which is there
/// only because the connect issued `tools/list` against the child and stored what came back — phase
/// 8 clears approval, and the invocation sends `tools/call` down the same stdio pipe the handshake
/// used and hands the child's answer to the model.
///
/// `echoed:pong` is the assertion that cannot be faked from this side. The string is composed by
/// the `sh` fixture out of the `text` argument it parsed off the wire; nothing in `cyrup-mcp`, in
/// the session, or in this file can produce it without a round trip to a running child process.
///
/// The three `details` keys are the trip in the other direction:
///
/// * `mode: "call"` — the dispatcher routed into `execute_call` rather than answering an init
///   envelope (`not_initialized`, `init_timeout` and `init_failed` carry no `mode` key at all).
/// * `server: "fixture"` — the hint pinned the server, so no scan ran.
/// * `tool: "echo"` — the **wire** name. The model asked for `fixture_echo`; what reached the child
///   is `tool_meta.original_name`, which is the un-prefixing happening exactly once and in the one
///   place it belongs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_issued_gateway_call_returns_the_servers_own_result() {
    let fx = fixture();
    let (_ext, answer) = call_gateway(
        &fx,
        serde_json::json!({ "tool": DIRECT_TOOL, "server": SERVER, "args": { "text": "pong" } }),
    )
    .await;

    // THE ANSWER. The fixture's own bytes, in the transcript the model reads.
    assert!(
        answer.text.contains(SERVER_ANSWER),
        "the server's real `tools/call` result reached the model: {answer:?}"
    );
    assert!(!answer.is_error, "a successful tool call is not an error: {answer:?}");
    assert_eq!(
        answer.detail("error"),
        None,
        "…and carries no error code at all — not `tool_not_found`, not `call_failed`: {answer:?}"
    );

    // It got there through the live call mode, against the named server.
    assert_eq!(
        answer.detail("mode").and_then(Value::as_str),
        Some("call"),
        "the call reached `execute_call` against the committed `ProxyCtx`: {answer:?}"
    );
    assert_eq!(answer.detail("server").and_then(Value::as_str), Some(SERVER));
    assert_eq!(
        answer.detail("tool").and_then(Value::as_str),
        Some(REMOTE_TOOL),
        "the WIRE name reached the child — the catalog name is un-prefixed exactly once, at the \
         invocation: {answer:?}"
    );

    // `details.mcpResult` is the raw MCP payload, kept beside the rendered text. Asserted because
    // it is the half a UI reads, and because it shows the `isError: false` the server itself sent
    // rather than a flag this side inferred.
    let raw = answer.detail("mcpResult").unwrap_or_else(|| panic!("the raw MCP result: {answer:?}"));
    assert_eq!(raw.get("isError"), Some(&serde_json::json!(false)), "{answer:?}");
    assert_eq!(
        raw.pointer("/content/0/text").and_then(Value::as_str),
        Some(SERVER_ANSWER),
        "the raw payload carries the server's content block verbatim: {answer:?}"
    );
}

/// **The same result through the DIRECT tool** — `fixture_echo` called by name, with no gateway in
/// the middle.
///
/// This is the route a user notices, because `fixture_echo` is a tool in the model's array with the
/// server's own description and the server's own JSON schema: the model calls it the way it calls
/// `read`. `McpDispatch::call_direct` is the arm that serves it, and it reaches the same
/// `execute_call` with the server pinned and `ApprovalOrigin::for_direct_tool` supplied.
///
/// Worth its own test rather than folded into the one above because the two arms take different
/// paths to the same executor, and exactly one of them was wrong: `call_direct` passed
/// `spec.original_name` where `execute_call` matches `ToolMetadata::name`, so under the default
/// `toolPrefix: "server"` every direct tool answered `tool_not_found` — while the gateway, whose
/// caller supplies the catalog name directly, worked. A single test through the gateway would have
/// reported the feature green with every direct tool broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_issued_direct_tool_call_returns_the_servers_own_result() {
    let fx = fixture();
    let (_ext, answer) =
        call_tool(&fx, DIRECT_TOOL, serde_json::json!({ "text": "pong" })).await;

    assert!(
        answer.text.contains(SERVER_ANSWER),
        "the server's real `tools/call` result reached the model through the direct tool: \
         {answer:?}"
    );
    assert!(!answer.is_error, "{answer:?}");
    assert_eq!(answer.detail("error"), None, "{answer:?}");
    assert_eq!(answer.detail("server").and_then(Value::as_str), Some(SERVER));
    assert_eq!(
        answer.detail("tool").and_then(Value::as_str),
        Some(REMOTE_TOOL),
        "the direct tool's own `original_name` is what the child was asked for: {answer:?}"
    );
}

/// **The catalog name is the prefixed name, and only the prefixed name resolves.**
///
/// Not a residual and not a workaround — this is `format_tool_name`'s contract, observed from the
/// model's side. `ToolMetadata::name` and `DirectToolSpec::prefixed_name` are the same expression,
/// and every resolver in `proxy/call.rs` (`get_tool_matches`, `get_enabled_tool_matches`, the
/// phase-4 prefix scan) compares against `ToolMetadata::name`. So `mcp({tool: "echo"})` is a call
/// for a tool that does not exist under that name anywhere the model can see, and the runtime says
/// so — while naming the one that does.
///
/// Pinned because the deliverable's prose says `tool: "echo"` and a reader who tried it would
/// otherwise conclude the feature is broken. It is not: it is prefixed, by default, on purpose, and
/// the answer tells you the name to use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_resolves_the_catalog_name_not_the_wire_name() {
    let fx = fixture();
    let (_ext, answer) = call_gateway(
        &fx,
        serde_json::json!({ "tool": REMOTE_TOOL, "server": SERVER, "args": { "text": "pong" } }),
    )
    .await;

    assert_eq!(
        answer.detail("mode").and_then(Value::as_str),
        Some("call"),
        "still the live call mode — this is a resolution answer, not an init envelope: {answer:?}"
    );
    assert_eq!(
        answer.detail("error").and_then(Value::as_str),
        Some("tool_not_found"),
        "the WIRE name is not a catalog name: {answer:?}"
    );
    assert_eq!(answer.detail("requestedTool").and_then(Value::as_str), Some(REMOTE_TOOL));
    assert_eq!(answer.detail("hintServer").and_then(Value::as_str), Some(SERVER));

    // …and the answer is USEFUL: it names the tool the model should have asked for, which is the
    // whole difference between this and the empty-catalog failure it replaced (that one suggested
    // nothing, because there was nothing to suggest).
    let suggestions =
        answer.detail("suggestions").and_then(Value::as_array).cloned().unwrap_or_default();
    assert!(
        suggestions.iter().any(|name| name.as_str() == Some(DIRECT_TOOL)),
        "the runtime names the catalog entry it DOES have: {answer:?}"
    );
    assert!(
        answer.text.contains(DIRECT_TOOL),
        "…in the text the model reads, too: {answer:?}"
    );
    assert!(!answer.is_error, "an MCP tool error is a successful result with details: {answer:?}");
}

/// [`DIRECT_TOOL`] really is what `format_tool_name` produces for this server and tool under the
/// default prefix mode — so the literal above cannot drift away from the thing it names.
#[test]
fn the_catalog_name_is_the_prefixed_name() {
    assert_eq!(
        cyrup_mcp::registration::format_tool_name(
            REMOTE_TOOL,
            SERVER,
            cyrup_mcp::config::ToolPrefix::Server,
        ),
        DIRECT_TOOL,
    );
}

// ================================================================================================
// 5 · The surface. What the connect actually put in front of the model.
// ================================================================================================

/// **A connected server's discovered catalog reaches the model** — the `tools/list` half of the
/// deliverable, asserted at all three places it has to appear.
///
/// The fixture answers `tools/list` with one tool, `echo`. That answer travels:
///
/// 1. into `state.tool_metadata["fixture"]`, which is what every resolver in `proxy/call.rs`
///    matches against;
/// 2. into the `mcp({})` status report the model reads, as `toolCount: 1`; and
/// 3. into the model's own tool array, as `fixture_echo` — a first-class tool beside `read` and
///    `bash`, carrying the server's description and the server's JSON schema.
///
/// (3) is the one that costs a user when it is missing, and it is also the one that cannot be
/// produced without (1): direct tools are resolved from `mcp-cache.json`, which the connect writes
/// from the same discovery result. A cache pre-seeded by hand would not prove it either — the
/// startup pass overwrites `state.tool_metadata[name]` with what the LIVE connect discovered, so a
/// stale or invented entry is replaced rather than trusted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_surface_carries_the_servers_discovered_catalog() {
    let fx = fixture();
    let (ext, answer) = call_gateway(&fx, serde_json::json!({})).await;

    // (2) The status report, which is `execute_status` reading the live generation.
    let servers = answer.detail("servers").and_then(Value::as_array).cloned().unwrap_or_default();
    let row = servers
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(SERVER))
        .unwrap_or_else(|| panic!("the status names the configured server: {answer:?}"));
    assert_eq!(row.get("status").and_then(Value::as_str), Some("connected"));
    assert_eq!(
        row.get("toolCount"),
        Some(&serde_json::json!(1)),
        "the connected server contributes the one tool its `tools/list` answered with: {answer:?}"
    );

    // (1) The same fact from the state's own side, so it cannot be read as a formatting quirk of
    //     `execute_status` — and by NAME, because the name is the part that has to be the prefixed
    //     one for any of the resolvers to find it.
    let state = ext.state().expect("the generation committed");
    let metadata = state.tool_metadata.lock().unwrap();
    let entries = metadata
        .get(SERVER)
        .unwrap_or_else(|| panic!("`state.tool_metadata[\"{SERVER}\"]` exists"));
    let names: Vec<&str> = entries.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, vec![DIRECT_TOOL], "the catalog holds the model-facing name");
    let echo = entries.first().expect("one entry");
    assert_eq!(echo.original_name, REMOTE_TOOL, "…beside the wire name the child answers to");
    assert_eq!(
        echo.description, "echo back",
        "…and the server's OWN description, which only `tools/list` could have supplied"
    );
    assert_eq!(
        echo.input_schema
            .as_ref()
            .and_then(|schema| schema.pointer("/properties/text/type"))
            .and_then(Value::as_str),
        Some("string"),
        "…and the server's OWN input schema, verbatim: {echo:?}"
    );
    drop(metadata);

    // (3) The MODEL-VISIBLE surface — captured from the provider request itself, not from a
    //     registry view. This is where discovery is either worth something or invisible.
    assert!(
        answer.was_offered(DIRECT_TOOL),
        "`{DIRECT_TOOL}` reached the model's tool array: {:?}",
        answer.offered
    );
    // The gateway is there THROUGHOUT, on the turn before the surface re-sync and the turn after.
    // Asserted because the sync that adds `fixture_echo` rewrites the agent's whole tool array, and
    // a rewrite that added the direct tool while dropping the gateway would leave a server with no
    // `mcp({connect})`, no `mcp({})` status and no route at all for a tool the filters excluded.
    assert!(
        answer.offered.iter().all(|turn| turn.iter().any(|tool| tool == PROXY_TOOL)),
        "the gateway is on the model's surface throughout: {:?}",
        answer.offered
    );
}

/// **Direct tools are opt-in; the gateway is not.** The same live server, with `"directTools"`
/// left out of `mcp.json`.
///
/// `registration::resolve_tool_filter` answers `ToolFilter::Off` for a server with neither
/// `directTools` on its entry nor `settings.directTools`, so `resolve_direct_tools` yields nothing
/// and no `fixture_echo` is registered. The catalog is still discovered — `toolCount` still says 1
/// — and the model can still reach the tool, through `mcp({tool: "fixture_echo"})`.
///
/// Here so that [`the_live_surface_carries_the_servers_discovered_catalog`] cannot be read as "a
/// connected server always contributes direct tools". It contributes them when asked, and the
/// gateway is what a server gets when it is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_is_the_route_without_the_direct_tools_opt_in() {
    let fx = fixture_with(false);
    let (ext, answer) = call_gateway(
        &fx,
        serde_json::json!({ "tool": DIRECT_TOOL, "server": SERVER, "args": { "text": "pong" } }),
    )
    .await;

    // Discovery ran all the same — this is not a server that failed to connect.
    let state = ext.state().expect("the generation committed");
    assert_eq!(
        state.tool_metadata.lock().unwrap().get(SERVER).map(Vec::len),
        Some(1),
        "the catalog is discovered whether or not direct tools were asked for"
    );
    assert!(
        ext.registered_direct_tools().lock().unwrap().is_empty(),
        "…and no direct tool was registered, because none was configured"
    );
    assert!(
        !answer.was_offered(DIRECT_TOOL),
        "`{DIRECT_TOOL}` is NOT on the model's surface without the opt-in: {:?}",
        answer.offered
    );

    // …and the gateway still delivers the server's own answer.
    assert!(
        answer.text.contains(SERVER_ANSWER),
        "the model reaches the tool through `mcp({{tool}})` instead: {answer:?}"
    );
    assert!(!answer.is_error, "{answer:?}");
    assert_eq!(answer.detail("error"), None, "{answer:?}");
}
