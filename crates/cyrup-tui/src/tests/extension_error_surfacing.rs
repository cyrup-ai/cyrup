//! TUI-S02 — a CONTAINED extension fault must be visible in the interactive TUI, not only over RPC.
//!
//! Pi's interactive mode passes a fault listener when it binds extensions:
//!
//! ```text
//! onError: (error) => {
//!     this.showExtensionError(error.extensionPath, error.error, error.stack);
//! },
//! ```
//!
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1700-1701` at v0.83.0), and
//! `showExtensionError` (`:2545-2560`) writes `Extension "${extensionPath}" error: ${error}` into
//! the chat container in the `error` colour.
//!
//! cyrup registered such a listener in RPC mode ONLY (`crates/cyrup-modes/src/rpc.rs`'s
//! `error_listener`, installed in `run_rpc` and re-installed in `rebind_session`). The interactive
//! TUI registered none, so `Dispatcher::report` (`crates/cyrup-ext/src/dispatch.rs`) fell through to
//! a bare `tracing::warn!` — meaning a guest handler that faulted, and was therefore SKIPPED (fail
//! open) or turned into a BLOCK (fail closed, R-08-036 / EXT-001), produced nothing at all on screen
//! in cyrup's DEFAULT mode while an RPC client attached to the same session saw an
//! `extension_error` line.
//!
//! These tests drive the production seams end to end: a REAL `ExtensionHost` with a REAL faulting
//! `NativeExtension`, the REAL `App::install_error_listener` that `App::run` (and its session-swap
//! arm) call, a REAL dispatch that contains the fault, and the REAL `App::show_extension_error` the
//! run loop's `ext_error_rx` drain arm calls — asserting on the terminal cells the user sees.
//! `App::run` itself needs a real terminal event source, so the loop is not spun here; every
//! function it calls on this path is.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::{
    EventKind, ExtError, ExtMode, ExtensionError, ExtensionHost, HookOutcome, HostConfig, HostCtx,
    HostEvent, InitApi, NativeExtension,
};
use crate::{App, UiTheme};
use ratatui::backend::TestBackend;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: std::path::PathBuf::from(".") }
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

/// Everything the user could possibly see: the flushed scrollback plus the live screen cells.
fn rendered(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let mut out = app.scrollback_text();
    out.push('\n');
    let buf = app.terminal().backend().buffer().clone();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// A `tool_call` gate that faults instead of deciding. The dispatcher contains the panic
/// (R-08-036), turns the call into a fail-closed block, and reports it to every registered error
/// listener — the exact fault shape Pi's `onError` exists to surface.
struct BrokenGate;

#[async_trait::async_trait]
impl NativeExtension for BrokenGate {
    fn id(&self) -> ExtensionId {
        "broken-gate".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        panic!("gate exploded before it could decide");
    }
}

/// Load `BrokenGate` on a real host and make it fault once, returning the receiver the TUI run loop
/// drains. `install` selects whether `App::install_error_listener` — the production call — ran.
async fn fault_once(install: bool) -> UnboundedReceiver<ExtensionError> {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(BrokenGate)).await.unwrap();

    let (tx, rx) = unbounded_channel::<ExtensionError>();
    if install {
        App::<TestBackend>::install_error_listener(&host, tx);
    } else {
        drop(tx);
    }

    // A real dispatch on the real seam `cyrup-permission-system` subscribes (R-08-010).
    let _ = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            &CancelToken::new(),
        )
        .await;
    rx
}

/// Characterizes the hole the TUI used to sit in: the fault IS contained and reported, but with no
/// listener registered `Dispatcher::report` has nobody to tell, so nothing can reach a UI.
#[tokio::test]
async fn without_a_listener_a_contained_fault_reaches_no_ui() {
    let mut rx = fault_once(false).await;
    assert!(
        rx.try_recv().is_err(),
        "no listener installed ⇒ the fault stays inside `tracing`, which no TUI user reads"
    );
}

/// The whole path, end to end: a real guest faults, the real listener `App::run` installs forwards
/// it, the real drain arm renders it, and the user SEES Pi's copy on the terminal.
#[tokio::test]
async fn a_contained_fault_is_drawn_into_the_transcript() {
    let mut rx = fault_once(true).await;

    let err = rx.try_recv().expect(
        "TUI-S02: `App::install_error_listener` must deliver the contained fault to the run loop",
    );
    assert_eq!(err.extension.as_str(), "broken-gate", "attributed to the faulting extension");
    assert_eq!(err.event, "tool_call", "carries the event kind the fault happened during");

    let mut app = app();
    app.show_extension_error(&err);

    let text = rendered(&mut app);
    // Pi `showExtensionError`: `Extension "${extensionPath}" error: ${error}`
    // (`interactive-mode.ts:2545-2546`).
    assert!(
        text.contains("Extension \"broken-gate\" error:"),
        "the fault must be visible in the DEFAULT mode, with Pi's copy: {text}"
    );
    assert!(
        text.contains("gate exploded before it could decide"),
        "the underlying message must reach the user, not just the extension name: {text}"
    );
}

/// A second fault on the SAME host is delivered too — Pi's `onError` is a per-fault callback, not a
/// one-shot. Drains to the second item rather than assuming a count, so the test does not depend on
/// how many hooks a single dispatch happens to fan out to.
#[tokio::test]
async fn every_fault_is_delivered_not_just_the_first() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(BrokenGate)).await.unwrap();
    let (tx, mut rx) = unbounded_channel::<ExtensionError>();
    App::<TestBackend>::install_error_listener(&host, tx);

    for call_id in ["t1", "t2"] {
        let _ = host
            .dispatcher()
            .dispatch_block_mutate(
                HostEvent::ToolCall {
                    call_id: call_id.into(),
                    name: "bash".into(),
                    input: serde_json::json!({}),
                },
                &CancelToken::new(),
            )
            .await;
    }

    let mut app = app();
    let mut seen = 0usize;
    while let Ok(err) = rx.try_recv() {
        app.show_extension_error(&err);
        seen += 1;
    }
    assert!(seen >= 2, "both dispatches' faults must be reported, got {seen}");
    let text = rendered(&mut app);
    assert!(
        text.matches("Extension \"broken-gate\" error:").count() >= 2,
        "each contained fault gets its own transcript line: {text}"
    );
}
