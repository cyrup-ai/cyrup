//! **The MCP SDK's 60 s default, live** (MCP-633 (a)).
//!
//! A real session, a real child process and the real clock: a server that starts and never answers
//! `initialize`, configured with no `requestTimeoutMs` anywhere, fails its connect at the 60 s
//! default the session applies — not before it, and not long after. The unit pins hold the same
//! bound on a paused clock against the connection builder alone
//! (`cyrup-mcp` `runtime.rs`, `a_wedged_stdio_server_with_no_timeout_configured_…`); this is the
//! default as the startup pass of a session applies it, from `mcp.json` to `failure_messages`.
//!
//! It costs a minute of wall time, and it is the only test here that does.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::StopReason;
use cyrup_mcp::McpExtension;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

const SERVER: &str = "silent";

/// The SDK default, written out: a change to the default, or to the budget the startup pass
/// applies, moves the connect off this and fails the test.
const SDK_DEFAULT: Duration = Duration::from_secs(60);

/// How late the failure may land past the default: the spawn, the commit tail and the 50 ms poll.
/// Far below any other bound a connect could end at.
const SLACK: Duration = Duration::from_secs(15);

/// A stdio server that starts, says so, and never reads its stdin. Its `initialize` sits unread in
/// the pipe, which is the case nothing below the MCP request timeout can end.
const NEVER_ANSWERS: &str = r#"
: > "$1"
exec sleep 600
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_never_answers_initialize_fails_its_connect_at_the_sdk_default() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let started: PathBuf = tmp.path().join("silent-started");
    std::fs::write(
        agent_dir.join("mcp.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {
                SERVER: {
                    "command": "sh",
                    "args": ["-c", NEVER_ANSWERS, "sh", started.to_string_lossy()],
                    "lifecycle": "keep-alive",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
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

    // The clock starts before the session does: the request timer starts later still, when
    // `initialize` is sent, so what is measured here can only overstate the budget.
    let clock = Instant::now();
    session.bind_extensions().await;

    let failure = tokio::time::timeout(SDK_DEFAULT + SLACK, async {
        loop {
            if let Some(state) = ext.state()
                && ext.init_task().is_none()
                && let Some(message) = state.failure_messages.lock().unwrap().get(SERVER).cloned()
            {
                return message;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "no connect failure within {:?}: the default is not bounding `initialize`",
            SDK_DEFAULT + SLACK
        )
    });
    let elapsed = clock.elapsed();

    assert!(started.exists(), "the server really started");
    assert!(
        failure.contains(cyrup_mcp::runtime::HANDSHAKE_TIMED_OUT),
        "the connect failed on its request timeout: {failure}"
    );
    assert!(
        elapsed >= SDK_DEFAULT,
        "the connect gave up after {elapsed:?}, before the SDK default"
    );
    assert!(
        elapsed < SDK_DEFAULT + SLACK,
        "the connect gave up after {elapsed:?}, long past the SDK default"
    );
    session.dispose("quit").await;
}
