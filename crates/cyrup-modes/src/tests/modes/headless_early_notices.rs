//! SEAM-160 — a headless session (`-p`, `--mode json`) holds no notice after its run.
//!
//! SEAM-157 made `LiveHostServices` keep the `Notify` effects said before the mode attaches its
//! effect drain, so the interactive TUI announces an MCP server that failed at once. `-p` and
//! `--mode json` never attach a drain, so the first thirty-two notices of such a session were held
//! for as long as it lived, with nobody ever to read them. The session's mode now declares that up
//! front (`SessionConfig::app_mode`, set from the CLI by `resolve_app_mode`), and a headless session
//! drops each notice as it is said, as pi's `noOpUIContext.notify` does.
//!
//! Observed through the public seam: after the run, a drain attached to the session's host services
//! would receive every held notice at once (`set_ui_effect_sink` delivers them first). It must
//! receive nothing. The CONTROL is an RPC session, whose drain is attached by `run_rpc` after the
//! notice was said: the notice still reaches the client.

use std::sync::Arc;

use super::support::{
    Fixture, base_config, create_runtime, fixture, read_json_line, spawn_rpc_duplex,
};
use crate::{PrintOptions, run_json, run_print};
use cyrup_config::AppMode;
use cyrup_core::StopReason;
use cyrup_ext::host::{HostServices, NotifyKind};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSessionRuntime, InputSource, SessionFactory, UiEffect, UserInput};
use tokio::io::AsyncWriteExt;

/// How many notices the session says before its mode runs — more than the thirty-two a draining
/// mode keeps, so a regression to "hold the first thirty-two" is visible as thirty-two.
const SAID: usize = 40;

async fn runtime_in(fx: &Fixture, mode: AppMode) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = base_config(fx);
    cfg.no_extensions = true;
    cfg.app_mode = mode;
    let target = cfg.target.clone();
    create_runtime(SessionFactory::new(provider, cfg), target).await
}

/// Say `SAID` notices on the active session, as a background extension (the MCP adapter connecting
/// its servers) does before the mode is running.
async fn say_early_notices(runtime: &AgentSessionRuntime) {
    let session = runtime.session().await;
    for n in 0..SAID {
        session
            .services()
            .host_services
            .notify(&format!("early notice {n}"), NotifyKind::Error);
    }
}

/// Attach a drain to the session the runtime ends on and collect what it is handed at once: the
/// notices the session was still holding.
async fn held_after_run(runtime: &AgentSessionRuntime) -> Vec<String> {
    let session = runtime.session().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    session.services().host_services.set_ui_effect_sink(tx);
    let mut held = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        if let UiEffect::Notify { message, .. } = effect {
            held.push(message);
        }
    }
    held
}

/// `-p`: `run_print` over a session built in [`AppMode::Print`].
#[tokio::test]
async fn a_print_session_holds_no_notice_after_its_run() {
    let fx = fixture();
    let runtime = runtime_in(&fx, AppMode::Print).await;
    say_early_notices(&runtime).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = run_print(
        &runtime,
        [UserInput::text("go", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");
    assert_eq!(code, 0, "the run itself succeeds");
    assert!(
        String::from_utf8_lossy(&out).contains("done"),
        "the run really ran"
    );

    let held = held_after_run(&runtime).await;
    assert!(
        held.is_empty(),
        "a -p session never attaches an effect drain, so it must hold no notice after its run; \
         it held {} of them: {held:?}",
        held.len()
    );
}

/// `--mode json`: `run_json` over a session built in [`AppMode::Json`].
#[tokio::test]
async fn a_json_session_holds_no_notice_after_its_run() {
    let fx = fixture();
    let runtime = runtime_in(&fx, AppMode::Json).await;
    say_early_notices(&runtime).await;

    let mut out: Vec<u8> = Vec::new();
    run_json(
        &runtime,
        [UserInput::text("go", InputSource::Cli)],
        &mut out,
    )
    .await
    .expect("json mode runs");
    assert!(
        String::from_utf8_lossy(&out).contains("agent_end"),
        "the run really ran"
    );

    let held = held_after_run(&runtime).await;
    assert!(
        held.is_empty(),
        "a --mode json session never attaches an effect drain, so it must hold no notice after \
         its run; it held {} of them: {held:?}",
        held.len()
    );
}

/// CONTROL: a session whose mode DOES attach a drain (`--mode rpc`) still gets the notice said
/// before the drain was attached (SEAM-157). `run_rpc` attaches it when it starts, after the notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rpc_session_still_receives_the_notice_said_before_its_drain() {
    let fx = fixture();
    let runtime = runtime_in(&fx, AppMode::Rpc).await;
    runtime
        .session()
        .await
        .services()
        .host_services
        .notify("said before the drain", NotifyKind::Error);

    let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);
    client_tx
        .write_all(b"{\"type\":\"get_state\",\"id\":\"boot\"}\n")
        .await
        .unwrap();
    let mut notice = None;
    loop {
        let line = read_json_line(&mut client_reader).await;
        if line["type"] == "extension_ui_request" && line["method"] == "notify" {
            notice = Some(line);
        } else if line["type"] == "response" && line["id"] == "boot" {
            break;
        }
    }
    let notice = notice.expect("the early notice reaches the RPC client before the boot answer");
    assert_eq!(notice["message"], "said before the drain", "{notice}");
    assert_eq!(notice["notifyType"], "error", "{notice}");

    drop(client_tx);
    rpc.await.expect("rpc loop exits at EOF");
}
