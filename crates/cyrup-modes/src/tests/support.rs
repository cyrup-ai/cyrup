//! Shared fixtures for the whole crate-internal test tree: a tempdir project + agent dir, the
//! wired [`AgentSessionRuntime`] builders every case drives, the JSONL sink readers the assertions
//! parse with, and the in-memory duplex transport that stands in for real stdio.
//!
//! Lives at `tests::support` (not under `tests::modes`) because the `rpc_*` and `json_event`
//! siblings drive the same runtime over the same wire and each used to carry its own copy.

use std::path::PathBuf;
use std::sync::Arc;

use crate::run_rpc;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use serde_json::Value;
use tempfile::TempDir;

pub(super) struct Fixture {
    _tmp: TempDir,
    pub(super) cwd: PathBuf,
    pub(super) agent_dir: PathBuf,
}

pub(super) fn fixture() -> Fixture {
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

pub(super) fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project, no prompt
    cfg
}

/// [`base_config`] with the ambient extension tree switched OFF — the host-seam and
/// output-decoupling suites pin loop mechanics, so whatever extensions happen to be installed on
/// the developer's machine must not join the session and emit events into the wire under test.
pub(super) fn base_config_no_ext(fx: &Fixture) -> SessionConfig {
    let mut cfg = base_config(fx);
    cfg.no_extensions = true;
    cfg
}

/// A [`cyrup_session_svc::ProviderResolver`] that hands back an offline faux provider for any id —
/// stands in for the binary's `select_provider` seam so a cross-provider `set_model` can complete.
pub(super) struct AnyFauxResolver;

impl cyrup_session_svc::ProviderResolver for AnyFauxResolver {
    fn resolve(&self, _provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        Ok(Arc::new(FauxProvider::new()))
    }
}

/// The one place a test runtime is actually built: every per-file builder is a thin wrapper that
/// only differs in how it dresses the [`SessionFactory`] before handing it over.
pub(super) async fn create_runtime(
    factory: SessionFactory,
    target: SessionTarget,
) -> Arc<AgentSessionRuntime> {
    AgentSessionRuntime::create(Arc::new(factory), target)
        .await
        .expect("build runtime")
}

/// Build the multi-session runtime host the RPC adapter drives (Pi `rpc-mode.ts` `runtimeHost`).
///
/// Carries a [`AnyFauxResolver`] because the real host always carries one: `main.rs` hands every
/// `SessionFactory` a `BuiltinProviderResolver`. Any model command whose target model belongs to a
/// provider other than the installed one (`set_model` across providers, and `cycle_model` since it
/// walks `getAvailable()` across every configured provider) has to install that provider, and a
/// resolver-less host can only fail there — which says nothing about the RPC contract under test.
/// It matters here because these fixtures are NOT hermetic against the ambient environment: a
/// `TOGETHER_API_KEY` in the developer's shell makes `together` a configured provider and puts its
/// whole catalog in the available set, exactly as it would for a real user.
pub(super) async fn build_runtime(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let factory = SessionFactory::new(provider, cfg).provider_resolver(
        Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>
    );
    create_runtime(factory, target).await
}

/// [`build_runtime`] with the auth store's ambient env tier pinned EMPTY.
///
/// The doc above notes these fixtures are not hermetic against the ambient environment. For a test
/// that asserts on the *shape of the available model catalog* that is not a nuisance, it decides the
/// result: a host exporting `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` completes Bedrock's IAM
/// pair (`cyrup-config/src/env_keys.rs`), `has_auth` reports it configured, and its whole catalog
/// enters the available set AHEAD of anthropic — `Models::providers` is a `BTreeMap` keyed by
/// provider id, and `"amazon-bedrock" < "anthropic"`. `cycle_model` then steps onto a Bedrock model
/// and the assertion fails on a machine that has nothing to do with what is under test.
///
/// Pinning the tier makes "this provider has no credential" a property of the fixture rather than of
/// whoever is running it. Scrubbing the process environment would not do: these tests run in
/// parallel and would race each other.
pub(super) async fn build_runtime_hermetic_auth(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let auth = Arc::new(
        cyrup_config::AuthStore::at(fx.agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    );
    let factory =
        SessionFactory::new(provider, cfg)
            .provider_resolver(
                Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>
            )
            .auth(auth);
    create_runtime(factory, target).await
}

/// Build the RPC runtime with a native extension registered into every session it builds.
pub(super) async fn build_runtime_with_ext(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    ext: Arc<dyn cyrup_ext::NativeExtension>,
) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    create_runtime(
        SessionFactory::new(provider, cfg).with_native_extension(ext),
        target,
    )
    .await
}

/// Parse the produced sink bytes into one `serde_json::Value` per non-empty LF-delimited line.
pub(super) fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    let text = String::from_utf8(bytes.to_vec()).expect("utf8 output");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each line is valid json"))
        .collect()
}

pub(super) fn type_of(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

/// [`type_of`] with a self-describing default, for the json-event suite's `match kind(line)` arms
/// where a type-less record should read as `<none>` rather than as an empty string.
pub(super) fn kind(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("<none>")
}

/// Read one non-empty JSONL record from an async reader (test helper for the interactive RPC flow).
pub(super) async fn read_json_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.expect("read a line");
        assert!(n > 0, "unexpected EOF while awaiting a json line");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed).expect("valid json line");
    }
}

/// The in-memory bidirectional transport that stands in for real stdio: returns the client's write
/// half, a buffered reader over the server's output, and the join handle of the loop itself. Drop
/// the write half to signal EOF, then `await` the handle.
pub(super) fn spawn_rpc_duplex(
    runtime: Arc<AgentSessionRuntime>,
) -> (
    tokio::io::DuplexStream,
    tokio::io::BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let (client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let handle = tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(server_rx);
        let mut writer = server_tx;
        run_rpc(&runtime, reader, &mut writer)
            .await
            .expect("rpc mode runs");
    });
    (client_tx, tokio::io::BufReader::new(client_rx), handle)
}

/// Run [`run_rpc`] as a client that keeps its input OPEN until `done` says it has seen what it was
/// waiting for, then closes it — the only way to get a run or a command to completion since
/// closing the input aborts them (SEAM-154, pi's `process.stdin.on("end")` → `shutdown()`,
/// rpc-mode.ts:802-805). `input` is written up front; the result is every line the host wrote, in
/// order, up to its return, as the bytes the wire carried.
pub(super) async fn run_rpc_until_bytes(
    runtime: &AgentSessionRuntime,
    input: &str,
    done: impl Fn(&[Value]) -> bool + Send + 'static,
) -> Vec<u8> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let (mut client_tx, server_rx) = tokio::io::duplex(1 << 20);
    let (mut server_tx, client_rx) = tokio::io::duplex(1 << 20);
    client_tx.write_all(input.as_bytes()).await.unwrap();
    let collector = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(client_rx).lines();
        let mut seen: Vec<Value> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut input_open = Some(client_tx);
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.trim().is_empty() {
                continue;
            }
            seen.push(serde_json::from_str(&line).expect("each line is valid json"));
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
            if input_open.is_some() && done(&seen) {
                input_open = None;
            }
        }
        bytes
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        run_rpc(
            runtime,
            tokio::io::BufReader::new(server_rx),
            &mut server_tx,
        ),
    )
    .await
    .expect("run_rpc returns once the client has what it waited for and closes the input")
    .expect("rpc mode runs");
    drop(server_tx);
    collector.await.unwrap()
}

/// [`run_rpc_until_bytes`], parsed one value per line.
pub(super) async fn run_rpc_until(
    runtime: &AgentSessionRuntime,
    input: &str,
    done: impl Fn(&[Value]) -> bool + Send + 'static,
) -> Vec<Value> {
    parse_lines(&run_rpc_until_bytes(runtime, input, done).await)
}

/// Whether the host has written `agent_settled`: the whole run is done (SEAM-005).
pub(super) fn has_settled(lines: &[Value]) -> bool {
    lines.iter().any(|l| type_of(l) == "agent_settled")
}

/// Whether the host has written the `response` to the command with `id`.
pub(super) fn has_response(lines: &[Value], id: &str) -> bool {
    lines
        .iter()
        .any(|l| type_of(l) == "response" && l["id"] == id)
}
