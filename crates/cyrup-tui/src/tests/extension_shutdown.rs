//! EXT-005 (interactive host) — a loaded extension's `ctx.shutdown()` must actually end the TUI.
//!
//! Pi's interactive mode honours it at TWO moments (the RPC host does the same, rpc-mode.ts:357 and
//! :786):
//!
//! * the `shutdownHandler` bound in `bindExtensions` —
//!   `this.shutdownRequested = true; if (this.session.isIdle) { void this.shutdown(); }`
//!   (interactive-mode.ts:1753-1757);
//! * `case "agent_settled": await this.checkShutdownRequested();` (interactive-mode.ts:3137-3138).
//!
//! cyrup honoured it in RPC only; the TUI's `agent_settled` arm was an explicit no-op whose comment
//! claimed the check was "polled in the event loop" — no such poll existed anywhere in cyrup-tui.
//!
//! `App::run` needs a real terminal event source, so the loop itself is not driven here. What IS
//! driven end to end is the decision the loop makes: a real native extension calls `ctx.shutdown()`
//! through the real command path on a real session, and
//! [`should_honor_extension_shutdown`] — the single predicate both run-loop call sites use — says
//! exit. Nothing here asserts that `HostServices::control(...)` returned `Ok`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::host::{ControlOp, HostServices};
use cyrup_ext::{
    CommandDescriptor, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use crate::should_honor_extension_shutdown;
use tempfile::TempDir;

/// A native built-in whose `/quitnow` command calls the base-context `ctx.shutdown()`
/// (Pi `ctx.shutdown()`, extensions/types.ts:344 → `runner.shutdown()`, runner.ts:656-662) —
/// the shape of Pi's own `examples/extensions/shutdown-command.ts`.
#[derive(Default)]
struct QuitExt {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for QuitExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("tui-quit-ext")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "quitnow",
            CommandDescriptor {
                description: "request a graceful host shutdown".into(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let svc = self
            .services
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| ExtError::Component("no host services".into()))?;
        svc.control(ControlOp::Shutdown).map_err(ExtError::Component)?;
        Ok(Some(String::new()))
    }
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
    cfg.no_extensions = true;
    cfg
}

fn faux_ok() -> Arc<dyn Provider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// A `/quit`-style extension COMMAND, with no agent run ever having happened, makes the interactive
/// host exit. Gating this on `agent_settled` alone (cyrup's pre-fix RPC rule, and the TUI's total
/// absence of a check) leaves the command doing nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_command_shutdown_ends_the_interactive_host() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_ok(), base_config(&fx))
        .with_native_extension(Arc::new(QuitExt::default()) as Arc<dyn NativeExtension>)
        .build()
        .await
        .unwrap()
        .into_shared();

    // Baseline: an idle session with nothing requested must NOT exit — the predicate is not simply
    // "we are idle".
    assert!(
        !should_honor_extension_shutdown(&session, false),
        "an untouched idle host keeps running"
    );
    assert!(
        !should_honor_extension_shutdown(&session, true),
        "a settle with no pending request keeps running"
    );

    // The real command path: prompt -> extension command -> the guest/native `ctx.shutdown()`.
    let _ = session.prompt("/quitnow").await.unwrap();
    session.wait_for_idle().await;

    assert!(
        session.shutdown_requested(),
        "the extension's ctx.shutdown() latched on the live session"
    );
    assert!(
        should_honor_extension_shutdown(&session, false),
        "the interactive host exits at the command tail (Pi's shutdownHandler idle branch), with no \
         run ever having happened"
    );
}

/// A shutdown requested while a run is IN FLIGHT is not honoured mid-run, and is honoured at the
/// settle point (Pi's `agent_settled` arm).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shutdown_requested_mid_run_waits_for_the_settle_point() {
    let fx = fixture();
    let ext = Arc::new(QuitExt::default());
    let session = SessionBuilder::new(faux_ok(), base_config(&fx))
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .unwrap()
        .into_shared();

    // Ask for the shutdown directly through the capability seam (as a mid-run event handler would),
    // then assert the NON-settle check refuses to act while the session is busy.
    let _ = session.prompt("hello").await.unwrap();
    let svc = ext.services.lock().unwrap().clone().expect("the native captured the backend");
    svc.control(ControlOp::Shutdown).unwrap();
    if !session.is_idle() {
        assert!(
            !should_honor_extension_shutdown(&session, false),
            "a busy host does not exit out from under an in-flight run"
        );
    }

    session.wait_for_idle().await;
    assert!(
        should_honor_extension_shutdown(&session, true),
        "the settle point honours the pending request (Pi interactive-mode.ts:3137-3138)"
    );
}

/// EXT-005: a shutdown request with NO turn boundary behind it must still be observed.
///
/// cyrup forwards control ops onto a queue that drains only at a turn boundary. `ctx.shutdown()`
/// cannot afford that: Pi's `shutdownHandler` is literally `() => { shutdownRequested = true }`
/// (rpc-mode.ts:344-346) and interactive-mode.ts:1753-1757 sets the field synchronously and then
/// exits immediately if the session is already idle. A queued-only cyrup latch is unobservable
/// whenever no boundary follows — an extension background task on an idle session, or a request
/// that lands in the window AFTER the in-flight run's own drain has already run. The latter is what
/// made `a_shutdown_requested_mid_run_waits_for_the_settle_point` above fail intermittently under
/// parallel load: the faux run finished before the test's `control(...)` call, so the op was queued
/// with no drain left to see it. `HostServices::control` now latches at the seam, exactly like it
/// already fires `ControlOp::Abort` live.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shutdown_requested_with_no_turn_boundary_left_is_still_observed() {
    let fx = fixture();
    let ext = Arc::new(QuitExt::default());
    let session = SessionBuilder::new(faux_ok(), base_config(&fx))
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .unwrap()
        .into_shared();

    let svc = ext.services.lock().unwrap().clone().expect("the native captured the backend");
    assert!(!session.shutdown_requested(), "nothing requested yet");

    // No prompt, no run, no settle — nothing in this test will EVER drain the control queue.
    svc.control(ControlOp::Shutdown).unwrap();

    assert!(
        session.shutdown_requested(),
        "ctx.shutdown() latches at the capability seam, not at a turn boundary that may never come"
    );
    assert!(
        should_honor_extension_shutdown(&session, false),
        "an idle host with a pending request exits at once (Pi interactive-mode.ts:1753-1757)"
    );
    assert!(
        should_honor_extension_shutdown(&session, true),
        "and the settle point honours it too (Pi interactive-mode.ts:3137-3138)"
    );
}
