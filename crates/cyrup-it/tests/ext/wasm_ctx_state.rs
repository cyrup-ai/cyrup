//! EXT-005 — `ctx.abort()`, `ctx.shutdown()` and the base-context state accessors.
//!
//! Pi puts `isIdle()`, `hasPendingMessages()`, `isProjectTrusted()`, `getSystemPrompt()`, `abort()`
//! and `shutdown()` on the BASE `ExtensionContext` (`coding-agent/src/core/extensions/types.ts:
//! 329-346`) — "Available in all contexts" — deliberately NOT on the command-only
//! `ExtensionCommandContextActions` (`types.ts:1641-1672`). They are implemented at
//! `agent-session.ts:2405-2436`, and `shutdown()`'s runner entry point is `runner.ts:656-662`.
//!
//! cyrup had NO representation of any of them: `ControlOp` had no `Abort`/`Shutdown`, the
//! `HostServices` trait had no state accessors, and `grep shutdown wit/world.wit` found only the
//! INBOUND `on-session-shutdown`. These tests drive a real guest across the wasm boundary and
//! assert the HOST observed the call — not that the guest's binding returned `Ok`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::fixture;

use cyrup_core::CancelToken;
use cyrup_ext::{CannedResponses, ControlOp, HostEvent, RecordingServices, Reduced};
use serde_json::json;
use std::sync::Arc;

fn services() -> Arc<RecordingServices> {
    Arc::new(RecordingServices::new(CannedResponses {
        // Deliberately the OPPOSITE of every `HostCtxRich::default()` / trait-default answer, so a
        // passing assertion cannot be satisfied by the old hard-coded values.
        is_idle: false,
        has_pending_messages: true,
        is_project_trusted: true,
        system_prompt: Some("SYSTEM-PROMPT-FROM-HOST".into()),
        ..Default::default()
    }))
}

/// `ctx.abort()` from an EVENT-tier handler reaches the host backend. Pi allows abort from any
/// context; cyrup's `control.*` tier gate must NOT apply to it (every other `control` op is
/// command-only and would be rejected with a deadlock error here).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_from_an_event_handler_reaches_the_host() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let svc = services();
    host.load_wasm("demo".into(), &bytes, svc.clone()).await.expect("load + init");

    let cancel = CancelToken::new();
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "tc-abort".into(),
                name: "abortme".into(),
                input: json!({}),
            },
            &cancel,
        )
        .await;
    assert!(matches!(reduced, Reduced::Blocked { .. }), "the guest handler ran");

    assert!(
        svc.control_ops().iter().any(|op| matches!(op, ControlOp::Abort)),
        "an event-tier ctx.abort() must reach the host control sink, got {:?}",
        svc.control_ops()
    );
    // ...and the guest saw a success, not the deadlock rejection every other control op gets.
    assert!(
        host.registry().tool_renderer_owner("demo_echo").is_ok(),
        "registry stays healthy"
    );
    let notes = svc.notify_calls();
    assert!(
        notes.iter().any(|(m, _)| m.contains("abort requested from a tool_call handler")),
        "the guest observed abort() succeeding: {notes:?}"
    );
}

/// `ctx.shutdown()` likewise, from the same event tier (Pi types.ts:344 / runner.ts:656-662).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_from_an_event_handler_reaches_the_host() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let svc = services();
    host.load_wasm("demo".into(), &bytes, svc.clone()).await.expect("load + init");

    let cancel = CancelToken::new();
    let _ = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "tc-shutdown".into(),
                name: "shutdownme".into(),
                input: json!({}),
            },
            &cancel,
        )
        .await;

    assert!(
        svc.control_ops().iter().any(|op| matches!(op, ControlOp::Shutdown)),
        "an event-tier ctx.shutdown() must reach the host control sink, got {:?}",
        svc.control_ops()
    );
    let notes = svc.notify_calls();
    assert!(
        notes.iter().any(|(m, _)| m.contains("shutdown requested from a tool_call handler")),
        "the guest observed shutdown() succeeding: {notes:?}"
    );
}

/// The four base-context state accessors are served from the LIVE backend. The guest's `/ctxstate`
/// command reports what it read; every value here differs from the defaults, so the assertion can
/// only pass if the answer genuinely crossed the boundary from `HostServices`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guest_reads_live_ctx_state_from_the_host() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let svc = services();
    let ext = host.load_wasm("demo".into(), &bytes, svc).await.expect("load + init");

    let cancel = CancelToken::new();
    let out = ext.execute_command("ctxstate", "", &cancel).await.expect("ctxstate runs");
    assert_eq!(
        out.as_deref(),
        Some("idle=false pending=true trusted=true prompt=SYSTEM-PROMPT-FROM-HOST"),
        "the guest read the host's live ctx state"
    );
}
