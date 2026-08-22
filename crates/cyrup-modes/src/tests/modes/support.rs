//! Shared fixtures for the mode-adapter suite: a tempdir project + agent dir, the wired
//! [`AgentSessionRuntime`] builders every case drives, the JSONL sink readers the assertions parse
//! with, and the in-memory duplex transport that stands in for real stdio.

use std::path::PathBuf;
use std::sync::Arc;

use crate::run_rpc;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory};
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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

pub(super) fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project, no prompt
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
pub(super) async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, cfg)
            .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn cyrup_session_svc::ProviderResolver>),
    );
    AgentSessionRuntime::create(factory, target).await.expect("build runtime")
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
    let factory = Arc::new(SessionFactory::new(provider, cfg).with_native_extension(ext));
    AgentSessionRuntime::create(factory, target).await.expect("build runtime")
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
        run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });
    (client_tx, tokio::io::BufReader::new(client_rx), handle)
}
