//! LIVE guest COMPONENT proof for parity gap #12 (`set_thinking_level` dead-but-advertised silent
//! no-op). Builds the `cyrup-ext-sdk` demo extension to a `wasm32-wasip2` COMPONENT, loads it
//! through the host with a [`RecordingServices`] backend (which GRANTS + records the command-tier
//! `control` capability), and drives `set_thinking_level` from BOTH tiers across the real Wasmtime
//! boundary:
//!
//!   * event tier  (`agent_start` hook)  — the deadlock rule forbids the command-tier `control` op,
//!   * command tier (`/thinkdemo` command) — the deadlock rule permits it.
//!
//! Pi allows `setThinkingLevel` from any handler (`runner.ts:330`, no tier gate); cyrup routes it
//! through the command-tier `control` path (arch-08 §6.3 deadlock rule — Pi's `setThinkingLevel`
//! re-emits `thinking_level_select`, `agent-session.ts:1588`, a re-entrant dispatch a single-
//! instance wasm store cannot legally make from a suspended event handler). This test pins the
//! contract: command tier takes effect (a recorded `SetThinkingLevel` control op); event tier is
//! rejected OBSERVABLY (a `deadlock guard: …` error the guest surfaces via `notify`), never a
//! silent nothing.
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
async fn set_thinking_level_is_tier_honest_across_the_wasm_boundary() {
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

    // The host must NOT have applied the op from an event handler (deadlock rule): no
    // `SetThinkingLevel` control op reached the RecordingServices backend.
    assert!(
        thinking_ops(&recording).is_empty(),
        "event-tier set_thinking_level must NOT be applied (deadlock rule); recorded ops: {:?}",
        recording.control_ops()
    );

    // ...and the rejection must be OBSERVABLE to the guest (parity gap #12): after the fix, the
    // event handler surfaces the deadlock error via notify. (On pre-fix code this notification does
    // NOT exist — the call silently no-ops — so this assertion is what fails without the fix.)
    let notes = ext.guest().notifications();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("thinking level rejected") && n.contains("deadlock guard")),
        "event-tier set_thinking_level rejection must be surfaced to the guest with the deadlock \
         reason, not silent; notifications: {notes:?}"
    );

    // --- COMMAND tier: /thinkdemo high -> the guest calls set_thinking_level at command tier ---
    let out = ext.execute_command("thinkdemo", "high", &cancel).await.expect("thinkdemo runs");
    assert_eq!(out.as_deref(), Some("thinking level set: high"));

    // The host applied it at command tier: exactly one `SetThinkingLevel("high")` op recorded.
    assert_eq!(
        thinking_ops(&recording),
        vec!["high".to_string()],
        "command-tier set_thinking_level must be applied exactly once; recorded ops: {:?}",
        recording.control_ops()
    );
    // ...and the command observed Ok (took effect), surfaced via notify.
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("thinking level set: high")),
        "command-tier set_thinking_level success must be observable; notifications: {:?}",
        ext.guest().notifications()
    );
}
