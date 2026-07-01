//! WASM SLASH-COMMAND END-TO-END (residual §07 / arch-08b headline for the facade). Proves that a
//! slash command REGISTERED BY A LIVE WASM GUEST executes through the REAL run path — Pi
//! `_tryExecuteExtensionCommand` (agent-session.ts:1148-1172), reached from `prompt` →
//! `prepare` (agent-session.ts:1006-1013). Not a native stub, not a hand-called facade: we
//! build a real `wasm32-wasip2` COMPONENT, load it through the session's host with the session's
//! own `LiveHostServices` injected (the arch-08 §5.6 seam), then drive `AgentSession::prompt("/greet
//! world")` and assert the GUEST handler ran across the WIT boundary (its `ctx.ui().notify(...)`
//! recorded host-side) AND that the slash command short-circuited the prompt (no user message sent).
//!
//! The fixture is the bundled `cyrup-ext-sdk` demo extension (its `example.rs` registers the
//! `/greet` command), built to a component via `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`
//! (wasm32-wasip2 emits a component directly). Set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt
//! component to skip the nested build.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// Build (or locate) the demo guest component (mirrors cyrup-ext/tests/wasm_component.rs).
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // A dedicated target dir so this nested build never contends with the outer workspace lock.
    let build_dir = std::env::temp_dir().join("cyrup-session-svc-fixture-target");
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&build_dir)
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");

    let wasm = build_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    // Disable on-disk extension auto-discovery so ONLY the explicitly-loaded guest is present.
    cfg.no_extensions = true;
    cfg
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// THE headline proof: a live wasm guest registers `/greet`; driving `prompt("/greet world")`
/// through the real `prepare` → `_tryExecuteExtensionCommand` path runs the GUEST handler across
/// the WIT boundary and short-circuits the prompt (no user message sent to the model).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_slash_command_executes_through_the_run_path() {
    let wasm_path = fixture_component();
    let bytes = std::fs::read(&wasm_path).expect("read fixture component bytes");

    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();

    // Load the guest COMPONENT through the session's host, injecting the session's OWN
    // LiveHostServices (arch-08 §5.6 — the same backend `apply_pending_control` drains).
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` registered `/greet` (cyrup-ext-sdk example.rs).
    assert!(
        session.services().ext_host.registry().command_names().unwrap().iter().any(|n| n == "greet"),
        "the guest-registered `/greet` command is in the host command registry"
    );
    // Nothing has invoked the handler yet.
    assert!(
        !ext.guest().notifications().iter().any(|n| n.contains("greet command ran")),
        "guest handler has not run before the prompt"
    );

    // Drive the command through the REAL public entry point (prompt → prepare →
    // _tryExecuteExtensionCommand → the live guest's `execute-command` export).
    let _ = session.prompt("/greet world").await.unwrap();
    session.wait_for_idle().await;

    // The GUEST handler ran across the wasm boundary: its `ctx.ui().notify("greet command ran")`
    // was recorded host-side in the live extension's guest state.
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("greet command ran")),
        "the wasm guest command handler executed end-to-end: {:?}",
        ext.guest().notifications()
    );

    // The slash command short-circuited the prompt: no `/greet` user message was sent/persisted
    // (Pi `_tryExecuteExtensionCommand` returns `true` ⇒ the prompt is consumed).
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/greet")),
        "the wasm slash command was consumed — no user message went to the model"
    );

    // A `/unknown` command (no guest or native owner) is NOT consumed: it falls through to a
    // normal prompt (Pi `getCommand` returns undefined ⇒ false, agent-session.ts:1184).
    let _ = session.prompt("/unknown please run").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("/unknown please run")),
        "an unmatched slash command falls through to normal prompt handling"
    );
}

/// Guard: the fixture path resolves to a real file (the nested build actually produced a component).
#[test]
fn fixture_component_exists() {
    let p = fixture_component();
    assert!(Path::new(&p).exists(), "fixture component missing at {}", p.display());
}
