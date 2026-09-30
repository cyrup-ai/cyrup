//! ICOM-063 — a `deliverAs: "steer"` injection made from a `turn_end` handler is on the running
//! agent's steering queue by the time `HostServices::inject_message_steer` returns.
//!
//! The agent loop awaits `turn_end` handlers and then polls steering; for a text-only turn that
//! poll is the run's last. Pi's `sendCustomMessage(msg, { deliverAs: "steer" })` on a streaming
//! session is a synchronous `agent.steer(appMessage)` (`agent-session.ts:1949-1954` @v0.87.1), so a
//! steer from that handler always reaches the poll — pi-intercom's `busyDelivery: "human-first"`
//! turn-boundary release (`index.ts:1814-1822` @v0.14.0) depends on it. cyrup's live host used to
//! hand the steer to the injection pump, a separate task, so it was not yet queued on return and
//! could miss the run. The end-to-end proof over the real broker is `cyrup-it --test intercom
//! inbound_live_session::busy_delivery_human_first::`.
//!
//! The runtime is single-threaded ON PURPOSE: there, a steer left to the pump cannot be queued
//! before the handler's next `.await`, so the regression is red every time, not under load only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::{AgentSession, InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use tempfile::TempDir;

const NOTE: &str = "turn-boundary-note";

/// On the FIRST `turn_end`, steer a note and record whether it was queued when the call returned.
struct TurnEndSteerer {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    session: Arc<OnceLock<Weak<AgentSession>>>,
    fired: AtomicBool,
    queued_on_return: AtomicBool,
}

#[async_trait::async_trait]
impl NativeExtension for TurnEndSteerer {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("turn-end-steerer")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::TurnEnd]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::TurnEnd { .. }) && !self.fired.swap(true, Ordering::SeqCst) {
            let services = self.services.lock().unwrap().clone().unwrap();
            services
                .inject_message_steer("peer note", Some(NOTE), true, None)
                .unwrap();
            // No `.await` between the call and this read: only a steer made inside the call counts.
            let queued = self
                .session
                .get()
                .and_then(Weak::upgrade)
                .is_some_and(|s| s.has_queued_messages());
            self.queued_on_return.store(queued, Ordering::SeqCst);
        }
        HookOutcome::Noop
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_steer_from_a_turn_end_handler_is_queued_before_the_call_returns() {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("answer to the note")], StopReason::Stop),
    ]);
    let slot = Arc::new(OnceLock::new());
    let ext = Arc::new(TurnEndSteerer {
        services: Mutex::new(None),
        session: slot.clone(),
        fired: AtomicBool::new(false),
        queued_on_return: AtomicBool::new(false),
    });
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    slot.set(Arc::downgrade(&session)).unwrap();

    let _stream = session
        .prompt(UserInput::text("human task", InputSource::Sdk))
        .await
        .expect("prompt");
    tokio::time::timeout(Duration::from_secs(30), session.wait_for_idle())
        .await
        .expect("the session settles");

    assert!(ext.fired.load(Ordering::SeqCst), "turn_end was dispatched");
    assert!(
        ext.queued_on_return.load(Ordering::SeqCst),
        "the steer was not on the agent's steering queue when `inject_message_steer` returned, \
         so it could miss the loop's next steering poll and with it the run"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "the steered note continued the SAME run with one more model call"
    );
    assert!(!session.has_queued_messages());
}
