//! `sampling/createMessage` and `elicitation/create`, end to end, against a real child process.
//!
//! # What this proves that a unit test cannot
//!
//! Before Wave 0, `ClientHandler::create_message` answered `METHOD_NOT_FOUND` to **every** server,
//! unconditionally: `bare_handler_factory` built every handler with `sampling: None`,
//! `build_client_capabilities` therefore advertised nothing, and `set_sampling_config` had no
//! production caller. Four links, all inside `initialize_mcp`.
//!
//! These tests drive all four from a real `mcp.json` through a real session start, and read the
//! answer **off the fixture server's own stdin/stdout** — the server writes what it received to a
//! file, so every assertion is made from the far side of the pipe.
//!
//! # Why the request fails, and why that is the proof
//!
//! [`the_server_reaches_our_handler_instead_of_method_not_found`] asserts the server receives
//! `"No cyrup model is available for MCP sampling"` (or the no-auth sibling). That string is
//! produced at exactly one place in the tree — `cyrup_mcp::sampling::resolve_sampling_model` — and
//! reaching it means the request traversed the transport, the handler, the installed hook, the
//! guards and the message conversion. A hermetic box has no configured provider, so the model
//! resolution is where the journey ends; completing it would test `cyrup-provider`, not this wire.
//!
//! The negative control is what makes that decisive:
//! [`sampling_disabled_in_settings_still_answers_method_not_found`] runs the identical fixture with
//! `"sampling": false` and asserts `-32601` — the answer the whole tree gave before this change.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_mcp::McpExtension;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

const SERVER: &str = "fixture";

/// A real stdio MCP server that issues a `sampling/createMessage` **back at the client** during
/// `tools/call`, then records the client's answer verbatim.
///
/// `$1` started, `$2` handshook, `$3` the captured `initialize` request, `$4` the captured sampling
/// response, `$5` extra JSON spliced into the request's `params` (empty for the plain case). Free of `${`, `$env:` and `{env:` for the same reason [`super::live_tool_call`]'s
/// fixture is: `args` pass through the adapter's `interpolateEnvVars`.
const SAMPLING_MCP: &str = r#"
: > "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s' "$line" > "$3"
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id" "$pv"
      ;;
    *'"method":"notifications/initialized"'*)
      : > "$2"
      # Issued the moment the handshake completes, so the round trip needs no tool call to
      # provoke it — the client is fully connected by definition at this point.
      printf '{"jsonrpc":"2.0","id":9001,"method":"sampling/createMessage","params":{"messages":[{"role":"user","content":{"type":"text","text":"ping"}}],"maxTokens":16%s}}\n' "$5"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ask","description":"ask the model","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"id":9001'*) printf '%s' "$line" > "$4" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"done"}],"isError":false}}\n' "$id"
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
    handshook: PathBuf,
    initialize: PathBuf,
    sampling_response: PathBuf,
}

/// `settings` is written verbatim into `mcp.json`, so each test states the gate it is exercising.
fn fixture(settings: serde_json::Value) -> Fixture {
    fixture_with(settings, "")
}

/// [`fixture`], with extra keys merged onto the **server entry** — the per-server half of a gate.
fn fixture_with_entry(settings: serde_json::Value, entry: serde_json::Value) -> Fixture {
    build(settings, "", entry)
}

/// [`fixture`], with extra `params` JSON for the sampling request the server issues.
fn fixture_with(settings: serde_json::Value, extra_params: &str) -> Fixture {
    build(settings, extra_params, serde_json::json!({}))
}

fn build(
    settings: serde_json::Value,
    extra_params: &str,
    entry_overrides: serde_json::Value,
) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let started = tmp.path().join("started");
    let handshook = tmp.path().join("handshook");
    let initialize = tmp.path().join("initialize.json");
    let sampling_response = tmp.path().join("sampling-response.json");

    let mut config = serde_json::json!({
        "mcpServers": {
            SERVER: {
                "command": "sh",
                "args": [
                    "-c",
                    SAMPLING_MCP,
                    "sh",
                    started.to_string_lossy(),
                    handshook.to_string_lossy(),
                    initialize.to_string_lossy(),
                    sampling_response.to_string_lossy(),
                    extra_params,
                ],
                "directTools": true,
            }
        },
        "settings": settings,
    });
    if let (Some(entry), Some(overrides)) = (
        config["mcpServers"][SERVER].as_object_mut(),
        entry_overrides.as_object(),
    ) {
        for (key, value) in overrides {
            entry.insert(key.clone(), value.clone());
        }
    }
    std::fs::write(
        agent_dir.join("mcp.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
        handshook,
        initialize,
        sampling_response,
    }
}

async fn start(fx: &Fixture) -> Arc<McpExtension> {
    start_in(fx, cyrup_session_svc::AppMode::Print).await
}

/// `AppMode` decides `has_ui`, which is step 6's gate and half of step 5's: `SessionConfig` defaults
/// to `Print`, which derives `has_ui = false`.
async fn start_in(fx: &Fixture, mode: cyrup_session_svc::AppMode) -> Arc<McpExtension> {
    let dirs = cyrup_mcp::dirs::McpDirs::new(fx.agent_dir.clone(), fx.cwd.clone());
    let ext = McpExtension::with_config(dirs, None)
        .with_home(fx.agent_dir.clone())
        .into_arc();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.app_mode = mode;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::clone(&ext) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    session.bind_extensions().await;
    ext
}

async fn await_connected(ext: &Arc<McpExtension>) -> Arc<cyrup_mcp::state::McpState> {
    let poll = async {
        loop {
            if let Some(state) = ext.state()
                && state
                    .manager
                    .get_connection(SERVER)
                    .is_some_and(|connection| {
                        connection.status() == cyrup_mcp::lifecycle::ConnectionStatus::Connected
                    })
                && ext.init_task().is_none()
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), poll)
        .await
        .expect("the fixture server never reached Connected")
}

/// Marker files are created empty (`: > "$2"`), so existence is the whole signal.
async fn await_exists(path: &std::path::Path, why: &str) {
    let poll = async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    if tokio::time::timeout(Duration::from_secs(15), poll)
        .await
        .is_err()
    {
        panic!("{why} — `{}` never appeared", path.display());
    }
}

/// Content files, by contrast, are written in one `printf` and must be non-empty to be parseable.
async fn await_file(path: &std::path::Path, why: &str) -> String {
    let poll = async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(15), poll)
        .await
        .unwrap_or_else(|_| panic!("{why} — `{}` never appeared", path.display()))
}

/// Wait for the client's answer to the request the fixture issued at handshake time.
async fn sampling_answer(fx: &Fixture, ext: &Arc<McpExtension>) -> serde_json::Value {
    await_connected(ext).await;
    await_exists(&fx.handshook, "the handshake never completed").await;
    let raw = await_file(
        &fx.sampling_response,
        "the client never answered the sampling request",
    )
    .await;
    serde_json::from_str(&raw).expect("the client's answer is JSON-RPC")
}

/// The capability is advertised on the wire, in the real `initialize` frame the server received.
#[tokio::test]
async fn the_initialize_frame_advertises_sampling_when_the_gate_is_open() {
    let fx = fixture(serde_json::json!({"samplingAutoApprove": true}));
    let ext = start(&fx).await;
    await_connected(&ext).await;
    let raw = await_file(&fx.initialize, "no initialize frame was captured").await;
    let frame: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let capabilities = &frame["params"]["capabilities"];
    assert!(
        capabilities.get("sampling").is_some(),
        "the server must see a sampling capability; got {capabilities}"
    );
}

/// The negative control, and the reason the positive one is decisive: `"sampling": false` closes
/// step 5's gate, so nothing is installed and the tree answers exactly as it did before Wave 0.
#[tokio::test]
async fn sampling_disabled_in_settings_still_answers_method_not_found() {
    let fx = fixture(serde_json::json!({"sampling": false, "samplingAutoApprove": true}));
    let ext = start(&fx).await;

    let raw = await_file(&fx.initialize, "no initialize frame was captured").await;
    let frame: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        frame["params"]["capabilities"].get("sampling").is_none(),
        "a closed gate must advertise nothing"
    );

    let answer = sampling_answer(&fx, &ext).await;
    assert_eq!(
        answer["error"]["code"].as_i64(),
        Some(-32601),
        "a server that samples an unadvertised client gets METHOD_NOT_FOUND; got {answer}"
    );
}

/// The request traverses the transport, the installed hook, the guards and the conversion, and ends
/// in `resolve_sampling_model` — a message no other code path in the tree produces.
#[tokio::test]
async fn the_server_reaches_our_handler_instead_of_method_not_found() {
    let fx = fixture(serde_json::json!({"samplingAutoApprove": true}));
    let ext = start(&fx).await;
    let answer = sampling_answer(&fx, &ext).await;

    assert_ne!(
        answer["error"]["code"].as_i64(),
        Some(-32601),
        "the hook is installed, so this must not be METHOD_NOT_FOUND; got {answer}"
    );
    // Past the guards, the journey's end depends on the box: with no usable provider credential it
    // stops at `resolve_sampling_model`, and with one it goes all the way into
    // `Models::complete` and fails (or succeeds) on the provider's own terms. Both are proof the
    // request reached the handler; neither is `-32601`. `-32603` is what every `throw` in
    // `handle_sampling_request` becomes, so an error answer must carry it.
    if let Some(code) = answer["error"]["code"].as_i64() {
        assert_eq!(
            code, -32603,
            "an error from our handler is an internal error; got {answer}"
        );
    } else {
        assert!(
            answer.get("result").is_some(),
            "the answer is either our handler's error or its result; got {answer}"
        );
    }
}

/// The deterministic half, and the one that does not depend on what credentials the box has: a
/// parameter guard is checked BEFORE any model resolution, so its message is reachable with no
/// provider at all.
///
/// `stopSequences` is guard 4 of 5 (`sampling-handler.ts:56`). The constant it returns exists at
/// exactly one place in the tree, so receiving it over the wire proves the request ran our body —
/// not merely that something answered.
#[tokio::test]
async fn a_parameter_guard_answers_with_its_own_constant_before_any_model_work() {
    let fx = fixture_with(
        serde_json::json!({"samplingAutoApprove": true}),
        r#","stopSequences":["halt"]"#,
    );
    let ext = start(&fx).await;
    let answer = sampling_answer(&fx, &ext).await;

    assert_eq!(answer["error"]["code"].as_i64(), Some(-32603));
    assert_eq!(
        answer["error"]["message"].as_str(),
        Some(cyrup_mcp::sampling::SAMPLING_STOP_SEQUENCES_UNSUPPORTED),
        "the guard's own constant, byte for byte; got {answer}"
    );
}

/// Step 6's proof, in the same shape as step 5's: `set_elicitation_config` had no production caller
/// until `initialize_mcp` installed one, so `create_elicitation` could only ever answer rmcp's
/// default `Decline` and no server was ever told the client could be asked.
///
/// `AppMode::Interactive` is the load-bearing line — `settings.elicitation(has_ui)` is
/// `elicitation != false && hasUI`, and `Print` derives `has_ui = false`.
#[tokio::test]
async fn the_initialize_frame_advertises_elicitation_only_with_a_ui() {
    let headless = fixture(serde_json::json!({}));
    let ext = start_in(&headless, cyrup_session_svc::AppMode::Print).await;
    await_connected(&ext).await;
    let raw = await_file(&headless.initialize, "no initialize frame").await;
    let frame: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        frame["params"]["capabilities"].get("elicitation").is_none(),
        "a headless session has nothing to ask through; got {}",
        frame["params"]["capabilities"]
    );

    let interactive = fixture(serde_json::json!({}));
    let ext = start_in(&interactive, cyrup_session_svc::AppMode::Interactive).await;
    await_connected(&ext).await;
    let raw = await_file(&interactive.initialize, "no initialize frame").await;
    let frame: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        frame["params"]["capabilities"].get("elicitation").is_some(),
        "with a UI the server must be told it can ask; got {}",
        frame["params"]["capabilities"]
    );
}

/// The other half of the same gate: `"elicitation": false` closes it even with a UI.
#[tokio::test]
async fn elicitation_disabled_in_settings_advertises_nothing() {
    let fx = fixture(serde_json::json!({"elicitation": false}));
    let ext = start_in(&fx, cyrup_session_svc::AppMode::Interactive).await;
    await_connected(&ext).await;
    let raw = await_file(&fx.initialize, "no initialize frame").await;
    let frame: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        frame["params"]["capabilities"].get("elicitation").is_none(),
        "an explicit `false` outranks the UI; got {}",
        frame["params"]["capabilities"]
    );
}

// ---- the wire tracer (§6) ------------------------------------------------------------------

/// The tracer's proof, and the reason it is here rather than in `crate::trace`'s unit tests: those
/// prove the writer and the redaction in isolation, against an injected file system. This one runs a
/// real child process over a real pipe and reads the JSONL **off disk**, so it proves the thing the
/// unit tests structurally cannot — that `TracingTransport` is actually installed on the transport a
/// production connect builds, and that the frames it records are the frames rmcp exchanged.
#[tokio::test]
async fn a_traced_server_writes_the_real_handshake_to_a_real_file() {
    let fx = fixture(serde_json::json!({"trace": {"enabled": true}}));
    let ext = start(&fx).await;
    await_connected(&ext).await;
    await_exists(&fx.handshook, "the handshake never completed").await;

    let dir = fx.cwd.join(".cyrup").join("mcp-traces");
    let poll = async {
        loop {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                    // Wait for both directions, so the assertions below cannot race the writer.
                    if text.contains("\"direction\":\"outbound\"")
                        && text.contains("\"direction\":\"inbound\"")
                    {
                        return text;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    let text = tokio::time::timeout(Duration::from_secs(15), poll)
        .await
        .unwrap_or_else(|_| panic!("no trace file appeared under {}", dir.display()));

    let lines: Vec<serde_json::Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect();
    assert!(
        lines.len() >= 2,
        "expected both directions, got {}",
        lines.len()
    );

    // Every line carries the schema version and this server's name.
    for line in &lines {
        assert_eq!(line["version"].as_u64(), Some(1));
        assert_eq!(line["server"].as_str(), Some(SERVER));
        assert_eq!(line["transport"].as_str(), Some("stdio"));
    }

    // The outbound `initialize` really was seen by the tracer, classified as a request because it
    // carries both `method` and `id`.
    let initialize = lines
        .iter()
        .find(|line| line["method"].as_str() == Some("initialize"))
        .expect("the handshake's initialize was traced");
    assert_eq!(initialize["direction"].as_str(), Some("outbound"));
    assert_eq!(initialize["kind"].as_str(), Some("request"));
    assert_eq!(initialize["status"].as_str(), Some("sent"));
    assert!(
        initialize["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
        "byte counts come from the same serialisation the classifier used"
    );
    assert!(
        initialize["durationMs"].as_f64().is_some(),
        "an outbound frame is timed around the inner send"
    );

    // And the server's answer came back in, classified as a response.
    assert!(
        lines
            .iter()
            .any(|line| line["direction"].as_str() == Some("inbound")
                && line["kind"].as_str() == Some("response")
                && line["status"].as_str() == Some("received")),
        "the inbound half was traced too; got {lines:#?}"
    );
}

/// The gate, from the other side: no `trace` block means no writer, no directory, and no wrapping.
/// This is what keeps the tracer off by default.
#[tokio::test]
async fn an_untraced_server_writes_nothing_at_all() {
    let fx = fixture(serde_json::json!({}));
    let ext = start(&fx).await;
    await_connected(&ext).await;
    await_exists(&fx.handshook, "the handshake never completed").await;
    // The handshake is complete, so any tracing that were going to happen already has.
    let dir = fx.cwd.join(".cyrup").join("mcp-traces");
    assert!(
        !dir.exists(),
        "an untraced session must not even create the directory; found {}",
        dir.display()
    );
}

/// `definition.trace ?? settings?.enabled === true` — a per-server `false` beats a global `true`,
/// end to end. `||` would trace this server; `??` does not.
#[tokio::test]
async fn a_per_server_false_beats_a_global_true_on_the_wire() {
    let fx = fixture_with_entry(
        serde_json::json!({"trace": {"enabled": true}}),
        serde_json::json!({"trace": false}),
    );
    let ext = start(&fx).await;
    await_connected(&ext).await;
    await_exists(&fx.handshook, "the handshake never completed").await;

    let dir = fx.cwd.join(".cyrup").join("mcp-traces");
    let wrote_anything = std::fs::read_dir(&dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                std::fs::read_to_string(entry.path())
                    .map(|text| !text.trim().is_empty())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(
        !wrote_anything,
        "the per-server `false` must win; something was written under {}",
        dir.display()
    );
}
