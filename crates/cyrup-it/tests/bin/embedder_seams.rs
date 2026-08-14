//! Embedder-seam tests (gap-10 majors 1/2/3 + gap-03 ProxyStreamFn wiring).
//!
//! Drives an assembled [`cyrup_sdk::Session`] to prove the four closed seams work end-to-end,
//! matching Pi's SDK contract:
//!
//!  1. **Custom transport injection** (Pi `AgentOptions.streamFn`, sdk.ts:301): an embedder-supplied
//!     [`cyrup_sdk::StreamFn`] serves a live turn *instead of* the provider — proven by scripting the
//!     injected transport and the provider with different text and asserting the injected text wins.
//!  2. **`ProxyStreamFn` wired into a live Agent turn** (gap-03 #4 — previously never wired anywhere):
//!     a real [`cyrup_sdk::ProxyStreamFn`] streams a turn over the wire against a local SSE server
//!     speaking Pi's proxy protocol (proxy.ts:36-57), and the proxied text reaches the assistant.
//!  3. **Dynamic key resolution** (Pi per-request key resolver): an injected
//!     [`cyrup_sdk::ApiKeyResolver`] is consulted on the live turn.
//!  4. **Resource-override closures** (Pi `DefaultResourceLoader.skillsOverride`/`agentsFilesOverride`,
//!     resource-loader.ts:143,155): synthetic in-memory skills + context files injected by an embedder
//!     appear in the assembled system prompt.
//!
//! No network / tokens: the transport is either a scripted [`FauxProvider`]-backed spy or a local
//! `std::net` SSE server on a loopback port.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cyrup_agent::{ProviderStreamFn, StreamFn};
use cyrup_core::{EventStream, ModelRef, ProviderId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use cyrup_sdk::{ApiKeyResolver, ContextFile, ContextScope, Cyrup, ProxyStreamFn, Session, SessionConfig, SkillPointer};
use tempfile::TempDir;

// ----------------------------------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------------------------------

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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project
    cfg
}

/// A [`FauxProvider`] scripted with a single one-shot assistant text reply.
fn scripted_provider(text: &str) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text(text)], StopReason::Stop)]);
    faux
}

// ----------------------------------------------------------------------------------------------
// 1. Custom transport injection: the injected StreamFn serves the turn, not the provider.
// ----------------------------------------------------------------------------------------------

/// A [`StreamFn`] that records each call, then delegates to a scripted provider-backed transport.
struct RecordingStreamFn {
    inner: ProviderStreamFn,
    hits: Arc<AtomicUsize>,
}

impl StreamFn for RecordingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        self.inner.stream(model, ctx, opts)
    }
}

/// Pi `AgentOptions.streamFn` (sdk.ts:301): a caller-supplied transport replaces the provider's.
/// The provider arg is scripted with one text, the injected transport with another — the injected
/// text must win, and the spy must have been invoked exactly once.
#[tokio::test]
async fn injected_stream_fn_serves_the_turn_not_the_provider() {
    let fx = fixture();

    // The provider that resolves the model catalog — scripted with a reply we must NOT see.
    let provider: Arc<dyn Provider> = scripted_provider("REPLY FROM THE PROVIDER (must not appear)");

    // The injected transport — scripted with the reply we MUST see.
    let injected_backing: Arc<dyn Provider> = scripted_provider("reply from the injected transport");
    let hits = Arc::new(AtomicUsize::new(0));
    let injected: Arc<dyn StreamFn> = Arc::new(RecordingStreamFn {
        inner: ProviderStreamFn::new(injected_backing),
        hits: hits.clone(),
    });

    let session: Session = Cyrup::builder()
        .stream_fn(injected)
        .build_session(provider, config(&fx))
        .await
        .expect("build session with injected transport");

    let text = session.run("hello").await.expect("run completes");

    assert_eq!(
        text, "reply from the injected transport",
        "the injected StreamFn must serve the turn, not the provider"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the injected transport must have run exactly once");
}

// ----------------------------------------------------------------------------------------------
// 2. ProxyStreamFn streams a live turn over the wire against a local SSE server (Pi proxy protocol).
// ----------------------------------------------------------------------------------------------

/// Spawn a one-shot loopback SSE server that answers `POST /api/stream` with Pi-shaped proxy frames
/// (proxy.ts:36-57), then closes. Returns the base URL (`http://127.0.0.1:<port>`). The server runs
/// on a plain `std::net` OS thread — no `tokio` net feature needed.
fn spawn_proxy_server(frames: Vec<String>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the request headers (best-effort); the small POST body can stay in the socket.
            let mut buf = [0u8; 4096];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut body = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for f in &frames {
                body.push_str("data: ");
                body.push_str(f);
                body.push_str("\n\n");
            }
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
            // Let the client drain the body before the socket closes.
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });
    url
}

/// gap-03 #4: `ProxyStreamFn` — never wired into any live Agent before this change — now streams a
/// real turn end-to-end. The local server sends the proxy text; the provider is scripted with a
/// different reply that must NOT appear (proving the proxy transport served the turn).
#[tokio::test]
async fn proxy_stream_fn_streams_a_live_turn_over_the_wire() {
    let fx = fixture();

    let usage = r#"{"input":5,"output":7,"cacheRead":0,"cacheWrite":0,"totalTokens":12,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}"#;
    let frames = vec![
        r#"{"type":"start"}"#.to_string(),
        r#"{"type":"text_start","contentIndex":0}"#.to_string(),
        r#"{"type":"text_delta","contentIndex":0,"delta":"streamed via the proxy"}"#.to_string(),
        r#"{"type":"text_end","contentIndex":0}"#.to_string(),
        format!(r#"{{"type":"done","reason":"stop","usage":{usage}}}"#),
    ];
    let proxy_url = spawn_proxy_server(frames);

    let provider: Arc<dyn Provider> = scripted_provider("REPLY FROM THE PROVIDER (must not appear)");
    let proxy: Arc<dyn StreamFn> = Arc::new(ProxyStreamFn::new(proxy_url, "test-token"));

    let session = Cyrup::builder()
        .stream_fn(proxy)
        .build_session(provider, config(&fx))
        .await
        .expect("build session with ProxyStreamFn");

    let text = session.run("ping the proxy").await.expect("run completes over the proxy");
    assert_eq!(
        text, "streamed via the proxy",
        "ProxyStreamFn must stream the live turn over the wire"
    );
}

// ----------------------------------------------------------------------------------------------
// 3. Dynamic key resolution: the injected ApiKeyResolver is consulted on the live turn.
// ----------------------------------------------------------------------------------------------

struct RecordingKeyResolver {
    hits: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ApiKeyResolver for RecordingKeyResolver {
    async fn get_api_key(&self, _provider: &ProviderId) -> Option<String> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Some("resolved-key".to_string())
    }
}

/// Pi per-request key resolution: the injected resolver is consulted on every turn (agent.rs:599).
#[tokio::test]
async fn key_resolver_is_consulted_on_a_live_turn() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = scripted_provider("ok");
    let hits = Arc::new(AtomicUsize::new(0));
    let resolver: Arc<dyn ApiKeyResolver> =
        Arc::new(RecordingKeyResolver { hits: hits.clone() });

    let session = Cyrup::builder()
        .key_resolver(resolver)
        .build_session(provider, config(&fx))
        .await
        .expect("build session with key resolver");

    let _ = session.run("hello").await.expect("run completes");
    assert!(
        hits.load(Ordering::SeqCst) >= 1,
        "the injected ApiKeyResolver must be consulted on the live turn"
    );
}

// ----------------------------------------------------------------------------------------------
// 4. Resource-override closures: synthetic skills + context files appear in the system prompt.
// ----------------------------------------------------------------------------------------------

/// Pi `DefaultResourceLoader.skillsOverride`/`agentsFilesOverride` (resource-loader.ts:143,155):
/// synthetic, in-memory resources injected by an embedder surface in the assembled system prompt.
#[tokio::test]
async fn resource_overrides_appear_in_the_system_prompt() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = scripted_provider("ok");

    let session = Cyrup::builder()
        .skills_override(|mut skills: Vec<SkillPointer>| {
            skills.push(SkillPointer {
                name: "synthetic-deploy-skill".to_string(),
                description: Some("Injected by the embedder, not on disk".to_string()),
                path: PathBuf::from("/virtual/deploy/SKILL.md"),
                disable_model_invocation: false,
            });
            skills
        })
        .context_files_override(|mut files: Vec<ContextFile>| {
            files.push(ContextFile {
                path: PathBuf::from("/virtual/AGENTS.md"),
                content: Arc::from("SYNTHETIC-CONTEXT-MARKER: injected AGENTS.md content"),
                scope: ContextScope::Cwd,
            });
            files
        })
        .build_session(provider, config(&fx))
        .await
        .expect("build session with resource overrides");

    let prompt = session.system_prompt();

    // The synthetic skill pointer appears in the <available_skills> section (read tool is active).
    assert!(
        prompt.contains("synthetic-deploy-skill"),
        "injected skill name must appear in the system prompt:\n{prompt}"
    );
    assert!(
        prompt.contains("Injected by the embedder, not on disk"),
        "injected skill description must appear in the system prompt"
    );
    // The synthetic context file's content appears in the prompt body.
    assert!(
        prompt.contains("SYNTHETIC-CONTEXT-MARKER: injected AGENTS.md content"),
        "injected context-file content must appear in the system prompt:\n{prompt}"
    );
}

// ----------------------------------------------------------------------------------------------
// 5. Zero-config provider construction: resolve a built-in provider from a model pattern (env auth).
// ----------------------------------------------------------------------------------------------

/// Pi `createAgentSession()` zero-config provider path (sdk.ts:174-221): a model pattern resolves a
/// working built-in provider with no manual provider/auth wiring; an unknown provider errors.
#[test]
fn zero_config_provider_resolves_builtins_and_errors_on_unknown() {
    let anthropic =
        cyrup_sdk::zero_config_provider("anthropic/claude-opus-4-8").expect("anthropic is built-in");
    assert_eq!(anthropic.id().as_str(), "anthropic");

    // A bare provider id resolves too.
    let openai = cyrup_sdk::zero_config_provider("openai").expect("openai is built-in");
    assert_eq!(openai.id().as_str(), "openai");

    // An unknown provider yields a clear error listing what is available.
    let err = cyrup_sdk::zero_config_provider("nope/whatever").err().expect("unknown provider errors");
    let msg = err.to_string();
    assert!(msg.contains("no built-in provider 'nope'"), "unexpected error: {msg}");
}
