//! EXT-086 — a LIVE wasm guest makes a model call through the ASSEMBLED session's providers: pi
//! `ctx.modelRegistry.complete()` / `streamSimple()` (`core/model-registry.ts` @v1.0.4).
//!
//! The row's own Verify line, end to end: *"A guest calls `models.complete` against the faux
//! provider and receives its text; a guest without the capability is refused."* — plus the half the
//! row does not name but the feature is: a guest that consumes a STREAM chunk by chunk to its end.
//!
//! Every test drives the production path: the demo guest's slash command (`prompt` → the guest's
//! `execute-command` export) → `ctx.models().complete/stream_simple(…)` → the WIT `models.*` imports
//! → `LiveHostServices::model_stream` → the session backend `AgentSession::into_shared` attached →
//! the session's faux provider. The session is SHARED, as every production session is, so the
//! backend under test is the real one and not the by-value fallback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::Capabilities;
use cyrup_ext::host::LiveExtension;
use cyrup_ext_sdk::example::model_calls::{COMPLETE_COMMAND, STREAM_COMMAND};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxConfig, FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use tempfile::TempDir;

use crate::support::bins;

/// The reply the faux provider scripts for the guest's call. Long enough that faux cuts it into
/// several 12–20-character deltas.
const REPLY: &str = "a reply from the session's faux provider, long enough to arrive in pieces";

/// A SHARED, trusted session on `faux`, with the demo guest loaded under `caps`.
async fn session_with_guest(
    faux: Arc<FauxProvider>,
    caps: &Capabilities,
) -> (Arc<AgentSession>, Arc<LiveExtension>) {
    let tmp = TempDir::new().expect("tempdir");
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");
    std::fs::create_dir_all(&agent_dir).expect("mkdir agent_dir");
    // Outlive the session (test-process-lifetime scratch dir), as the sibling `wasm_*` tests do.
    std::mem::forget(tmp);
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build session")
        .into_shared();
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bins::component_bytes(), caps)
        .await
        .expect("load + init the demo guest");
    (session, ext)
}

fn scripted(faux: FauxProvider) -> Arc<FauxProvider> {
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text(REPLY)],
        StopReason::Stop,
    )]);
    Arc::new(faux)
}

/// The guest's `ui.notify` that starts with `prefix`.
fn notification(ext: &LiveExtension, prefix: &str) -> String {
    let all = ext.guest().notifications();
    all.iter()
        .find(|n| n.starts_with(prefix))
        .cloned()
        .unwrap_or_else(|| panic!("no `{prefix}…` notification: {all:?}"))
}

/// THE Verify line, first half: the guest calls `models.complete` against the faux provider and
/// receives its text — through the real session, with exactly one provider request. Red against a
/// `complete` import that answers without calling the backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_completes_a_prompt_through_the_sessions_faux_provider() {
    let faux = scripted(FauxProvider::new());
    let address = format!("faux/{}", faux.model().id.as_str());
    let (session, ext) = session_with_guest(Arc::clone(&faux), &Capabilities::host_granted()).await;

    let _ = session
        .prompt(format!("/{COMPLETE_COMMAND} {address} say something"))
        .await
        .unwrap();
    session.wait_for_idle().await;

    assert_eq!(notification(&ext, "model "), format!("model text: {REPLY}"));
    assert_eq!(
        faux.call_count(),
        1,
        "the guest's call was not exactly one provider request"
    );
}

/// THE Verify line, second half: a guest whose manifest does not declare `modelCalls` is refused
/// with the host's own words, and the provider is never asked. Red against an import that skips the
/// `capabilities.modelCalls` gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_without_the_model_calls_capability_is_refused() {
    let faux = scripted(FauxProvider::new());
    let address = format!("faux/{}", faux.model().id.as_str());
    let caps = Capabilities {
        model_calls: false,
        ..Capabilities::host_granted()
    };
    let (session, ext) = session_with_guest(Arc::clone(&faux), &caps).await;

    let _ = session
        .prompt(format!("/{COMPLETE_COMMAND} {address} say something"))
        .await
        .unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        notification(&ext, "model "),
        format!("model denied: {}", cyrup_ext::DENIED_MODEL_CALLS)
    );
    assert_eq!(
        faux.call_count(),
        0,
        "a refused guest still reached the provider"
    );
}

/// A guest consumes a `streamSimple` stream CHUNK BY CHUNK to its end: the faux provider is paced so
/// its deltas arrive over time, and the guest's drain loop must see them across SEVERAL polls, then
/// exactly one `done` terminal, then end-of-stream — with the deltas reassembling the reply. Red
/// against a `poll-stream` that buffers to the terminal before answering (one batch), and against
/// one that reports end-of-stream before the terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_consumes_a_stream_chunk_by_chunk_to_its_end() {
    // ~75 characters at 3-5 tokens (12-20 chars) per delta and 20 tokens a second: several deltas,
    // spread over roughly a second — several of the host's ≤1s polls.
    let faux = scripted(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(20.0),
        ..FauxConfig::default()
    }));
    let address = format!("faux/{}", faux.model().id.as_str());
    let (session, ext) = session_with_guest(Arc::clone(&faux), &Capabilities::host_granted()).await;

    let _ = session
        .prompt(format!("/{STREAM_COMMAND} {address} stream something"))
        .await
        .unwrap();
    session.wait_for_idle().await;

    // "model stream polls: <p> deltas: <n> terminal: <type> text: <joined deltas>"
    let report = notification(&ext, "model stream");
    let rest = report
        .strip_prefix("model stream polls: ")
        .unwrap_or_else(|| panic!("the stream failed: {report}"));
    let (polls, rest) = rest.split_once(" deltas: ").unwrap();
    let (deltas, rest) = rest.split_once(" terminal: ").unwrap();
    let (terminal, text) = rest.split_once(" text: ").unwrap();
    let (polls, deltas): (u32, u32) = (polls.parse().unwrap(), deltas.parse().unwrap());

    assert_eq!(terminal, "done", "{report}");
    assert_eq!(
        text, REPLY,
        "the deltas did not reassemble the reply: {report}"
    );
    assert!(deltas > 1, "one delta is not a stream: {report}");
    assert!(
        polls > 1,
        "every event arrived in one poll, so nothing was consumed incrementally: {report}"
    );
    assert_eq!(faux.call_count(), 1);
}
