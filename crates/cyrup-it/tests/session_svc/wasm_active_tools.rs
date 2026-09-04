//! WASM ACTIVE-TOOL RESTRICTION END-TO-END (§07 G9 — the guest-facing `setActiveTools`/
//! `getActiveTools` binding). Proves that a LIVE wasm guest's `setActiveTools` genuinely restricts
//! the running agent's tool set on the next turn — Pi binds the guest's `setActiveTools`/
//! `getActiveTools` DIRECTLY to `setActiveToolsByName`/`getActiveToolNames`
//! (agent-session.ts:2281,2283 → 840-855,813-815), the SAME facade methods the host/CLI tool-toggle
//! uses; a guest's call has full, real effect.
//!
//! Before the fix the guest binding wrote to an INERT local `Mutex` in `cyrup-ext` and the
//! `HostServices` trait had no active-tools hook at all, so the restriction never reached the live
//! `AgentSession` — the agent kept streaming its full tool set. This test drives the demo guest's
//! `/planmode` command (which calls `pi.setActiveTools(["read"])`), then (1) asserts the facade's
//! `active_tool_names()` reflects the restriction and (2) drives a REAL turn through a `FauxProvider`
//! that captures the tool set the agent actually offered the model — proving only `read` survived.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// The tool sets an agent offered the model, one entry per turn (Pi request `tools`).
type CapturedTurns = Arc<Mutex<Vec<Vec<String>>>>;

/// A faux provider whose response factory captures the tool set the agent offered the model on each
/// turn into `captured`, so the test can observe the AGENT's real, effective tool set — not just the
/// facade's mirror.
fn faux_capturing_tools(captured: &CapturedTurns) -> Arc<FauxProvider> {
    let cap = captured.clone();
    let step = FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
        cap.lock()
            .unwrap()
            .push(ctx.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>());
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
    });
    let faux = Arc::new(FauxProvider::new());
    // A few identical steps so re-driven turns keep answering.
    faux.set_response_steps(vec![step.clone(), step.clone(), step]);
    faux
}

/// THE G9 proof: a live wasm guest's `/planmode` command calls `pi.setActiveTools(["read"])`; the
/// running agent's tool set is genuinely restricted to `["read"]` on the next turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_set_active_tools_restricts_the_live_agent() {
    let bytes = bins::component_bytes();
    let fx = fixture();
    let captured: CapturedTurns = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_capturing_tools(&captured);

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();
    let _ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // The guest registered `/planmode`.
    assert!(
        session
            .services()
            .ext_host
            .registry()
            .command_names()
            .unwrap()
            .iter()
            .any(|n| n == "planmode"),
        "the guest-registered `/planmode` command is in the host command registry"
    );

    // Baseline: the full built-in tool set is active (more than one tool, and `write` is present —
    // it is what `/planmode` will restrict AWAY).
    let baseline = session.active_tool_names();
    assert!(
        baseline.len() > 1,
        "more than one tool active by default: {baseline:?}"
    );
    assert!(
        baseline.contains(&"read".to_string()),
        "read active by default: {baseline:?}"
    );
    assert!(
        baseline.contains(&"write".to_string()),
        "write active by default: {baseline:?}"
    );

    // Drive the guest command through the REAL run path: prompt -> _tryExecuteExtensionCommand ->
    // execute-command -> the guest's `pi.setActiveTools(["read"])` -> apply_pending_control.
    let _ = session.prompt("/planmode").await.unwrap();
    session.wait_for_idle().await;

    // (1) The facade's active-tool view reflects the guest restriction (Pi getActiveToolNames reads
    //     the SAME state a guest setActiveTools mutates). BEFORE THE FIX this is still `baseline`.
    assert_eq!(
        session.active_tool_names(),
        vec!["read".to_string()],
        "the guest setActiveTools genuinely restricted the live session's active tool set"
    );
    assert!(
        session.tool_definition("write").is_some_and(|t| !t.active),
        "write is no longer active after the guest restriction"
    );

    // (2) THE LIVE PROOF: drive a real turn and observe the tool set the AGENT actually offered the
    //     model. Only `read` survived — the restriction reached the streaming agent, not just a mirror.
    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;

    let turns = captured.lock().unwrap().clone();
    assert!(
        !turns.is_empty(),
        "the agent drove at least one real turn against the provider"
    );
    let last = turns.last().unwrap();
    assert_eq!(
        last,
        &vec!["read".to_string()],
        "the agent offered the model ONLY the restricted `read` tool on the live turn: {turns:?}"
    );
}
