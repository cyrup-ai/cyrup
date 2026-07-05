//! LIVE guest COMPONENT proof for GAP-11 (`set_thinking_level` from an event handler). Builds the
//! `cyrup-ext-sdk` demo extension to a `wasm32-wasip2` COMPONENT, loads it through the host with a
//! [`RecordingServices`] backend (which GRANTS + records the `control` capability), and drives
//! `set_thinking_level` from BOTH tiers across the real Wasmtime boundary:
//!
//!   * event tier  (`agent_start` hook)  — Pi allows it from any handler, so cyrup QUEUES it,
//!   * command tier (`/thinkdemo` command) — always allowed.
//!
//! Pi allows `setThinkingLevel` from any handler (`loader.ts:352-354` / `runner.ts:330`, no tier
//! gate) and it takes effect. cyrup now matches: from EITHER tier the call is QUEUED as a
//! `SetThinkingLevel` control op (a synchronous mpsc push that touches no wasm store) and the guest
//! observes `Ok(())`. The op is later applied at the store-free turn-boundary drain
//! (`AgentSession::apply_pending_control`), where its `thinking_level_select` re-emit is a fresh
//! top-level guest call — never a re-entry into the suspended single-instance store, so the old
//! R-08-008 deadlock the command-tier gate guarded against is dissolved by deferral. This test pins
//! that new contract at the wasm boundary: the event-tier call is no longer rejected — it is QUEUED
//! (a recorded control op) and the guest surfaces the `Ok` success via `notify`, never a deadlock
//! error and never a silent nothing. (The full assembled-session drive that observes the level
//! change TAKE EFFECT on the next turn is the session-svc E2E; here the backend only records.)
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::CancelToken;
use cyrup_ext::host::{CannedResponses, ControlOp, RecordingServices};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, HostEvent};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Build (or locate) the demo guest component. Mirrors `wasm_component.rs`.
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build_dir = std::env::temp_dir().join("cyrup-ext-fixture-target");
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

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

/// Count `SetThinkingLevel(level)` control ops the host actually applied.
fn thinking_ops(rec: &RecordingServices) -> Vec<String> {
    rec.control_ops()
        .into_iter()
        .filter_map(|op| match op {
            ControlOp::SetThinkingLevel(level) => Some(level),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_thinking_level_is_queued_from_both_tiers_across_the_wasm_boundary() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");

    let recording = Arc::new(RecordingServices::new(CannedResponses::default()));
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, recording.clone())
        .await
        .expect("load + init the live wasm extension");

    let cancel = CancelToken::new();

    // --- EVENT tier: agent_start -> the guest calls ctx.models().set_thinking_level("minimal") ---
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;

    // GAP-11: the host must QUEUE the op from an event handler (Pi allows it from any handler), NOT
    // reject it. The `SetThinkingLevel("minimal")` control op reached the RecordingServices backend.
    assert_eq!(
        thinking_ops(&recording),
        vec!["minimal".to_string()],
        "event-tier set_thinking_level must be QUEUED (not rejected); recorded ops: {:?}",
        recording.control_ops()
    );

    // ...and the guest must observe `Ok(())` — the success path — NOT a deadlock error. The demo's
    // `agent_start` notifies "thinking level set from agent_start" on `Ok`. (Pre-fix this call
    // returned an honest deadlock `Err` and the guest notified "thinking level rejected".)
    let notes = ext.guest().notifications();
    assert!(
        notes.iter().any(|n| n.contains("thinking level set from agent_start")),
        "event-tier set_thinking_level must surface Ok to the guest, not a deadlock error; \
         notifications: {notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("thinking level rejected")),
        "event-tier set_thinking_level must NOT surface a rejection error anymore; \
         notifications: {notes:?}"
    );

    // --- COMMAND tier: /thinkdemo high -> the guest calls set_thinking_level at command tier ---
    let out = ext.execute_command("thinkdemo", "high", &cancel).await.expect("thinkdemo runs");
    assert_eq!(out.as_deref(), Some("thinking level set: high"));

    // Both tiers queued their op, in order: the event-tier "minimal" then the command-tier "high".
    assert_eq!(
        thinking_ops(&recording),
        vec!["minimal".to_string(), "high".to_string()],
        "both tiers queue a SetThinkingLevel op, in order; recorded ops: {:?}",
        recording.control_ops()
    );
    // ...and the command observed Ok (took effect), surfaced via notify.
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("thinking level set: high")),
        "command-tier set_thinking_level success must be observable; notifications: {:?}",
        ext.guest().notifications()
    );
}
