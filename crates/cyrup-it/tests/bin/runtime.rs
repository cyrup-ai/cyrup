//! SDK multi-session runtime + run-mode re-export tests (arch-11 §2.3/§3.4; R-11-019/020/021).
//!
//! Build the runtime + drive a mode using ONLY `cyrup_sdk` re-exports — the embedder never depends
//! on `cyrup-session-svc`/`cyrup-modes` directly. Exercises: constructing a [`SessionFactory`] over a
//! scripted [`FauxProvider`], creating an [`AgentSessionRuntime`], swapping the active session via
//! `new_session` (and observing the generation bump + the terminal `SessionReplaced`, R-11-021), and
//! driving the re-exported `run_rpc` / `run_print` helpers over the seam.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_sdk::{
    AgentSessionEvent, AgentSessionRuntime, PrintOptions, SessionConfig, SessionFactory, run_print,
    run_rpc,
};
use futures::StreamExt;
use tempfile::TempDir;

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Build the runtime using only SDK re-exports (the construction primitive R-11-019).
async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = config(fx);
    let target = cfg.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, cfg));
    AgentSessionRuntime::create(factory, target)
        .await
        .expect("build runtime")
}

/// The runtime swaps the active session on `new_session`: the generation bumps and the prior
/// subscription is invalidated with a terminal `SessionReplaced` (R-11-021).
#[tokio::test]
async fn runtime_new_session_swaps_and_invalidates_subscriptions() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let first = runtime.session().await;
    let first_id = first.session_id().as_str().to_string();
    let mut events = first.subscribe();
    assert_eq!(runtime.generation().await, 0);

    let result = runtime.new_session().await.expect("new session");
    assert!(!result.cancelled, "no extension vetoed the swap");
    assert_eq!(
        runtime.generation().await,
        1,
        "generation must bump on swap"
    );

    // The held subscription receives the terminal `SessionReplaced` and then ends (R-11-021).
    let mut saw_replaced = false;
    while let Some(ev) = events.next().await {
        if matches!(ev, AgentSessionEvent::SessionReplaced { generation } if generation == 1) {
            saw_replaced = true;
        }
    }
    assert!(
        saw_replaced,
        "prior subscription must see the SessionReplaced terminal"
    );

    let second = runtime.session().await;
    assert_ne!(
        second.session_id().as_str(),
        first_id,
        "a fresh session is active"
    );
}

/// The re-exported `run_rpc` helper drives the runtime host end-to-end from the SDK surface.
#[tokio::test]
async fn sdk_run_rpc_drives_the_runtime_host() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    let input = concat!(
        r#"{"type":"get_state","id":"1"}"#,
        "\n",
        r#"{"type":"new_session","id":"2"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc runs");

    // A correlated response per command (parsed via the re-exported serde stack would need a json
    // dep; substring assertions over the protocol bytes suffice for the re-export smoke test).
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains(r#""command":"get_state""#),
        "get_state response present:\n{text}"
    );
    assert!(
        text.contains(r#""command":"new_session""#) && text.contains(r#""cancelled":false"#),
        "new_session swapped without veto:\n{text}"
    );
    // The swap actually happened on the runtime host.
    assert_eq!(
        runtime.generation().await,
        1,
        "new_session bumped the runtime generation"
    );
}

/// The re-exported `run_print` helper runs a one-shot prompt over the active session.
#[tokio::test]
async fn sdk_run_print_over_the_active_session() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("hello from print")],
        cyrup_sdk::core::StopReason::Stop,
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        std::iter::once(cyrup_sdk::UserInput::text(
            "hi",
            cyrup_sdk::InputSource::Cli,
        )),
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print runs");
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("hello from print"),
        "print emitted the final text:\n{text}"
    );
}
