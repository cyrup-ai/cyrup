//! EXT-087 — a send queued from an EVENT handler actually starts a run, exactly once, at the
//! turn boundary.
//!
//! **Upstream.** `ctx.sendMessage` and `ctx.sendUserMessage` are
//! `assertActive(); runtime.sendMessage(...)` / `assertActive(); runtime.sendUserMessage(...)`
//! (`core/extensions/loader.ts:351-354` and `:356-358` @v0.87.1) — no tier check of any kind,
//! callable from every event handler. CITATION CORRECTION, recorded beside the original: the row
//! cites `loader.ts:304-311 @v0.83.0`; at the pinned v0.87.1 the `:304-311` region is other
//! `assertActive()` call sites, so a pass re-deriving from the row's line numbers at the new pin
//! lands on the wrong methods and could wrongly conclude the gate exists upstream.
//!
//! pi v0.87.0 added a DEFERRAL for the one case that cannot run inline: `_isEmittingAgentSettled`
//! (`core/agent-session.ts:378`) and `_deferredSettledActions` (`:379`), because
//! `pi.sendUserMessage` reaches `this.prompt()` SYNCHRONOUSLY inside the `agent_settled` dispatch.
//! `prompt()`'s entry pushes a closure instead when the flag is set (`:1607-1608`), and
//! `_emitAgentSettled` takes the queue exactly once and awaits each action after its `finally` has
//! cleared the flag (`:873-885`).
//!
//! **cyrup.** The send crosses a wasm import onto the control queue, so the QUEUE is already the
//! deferral — what was missing was a drain that runs at that point. `settle_run` now runs the full
//! `apply_pending_control` after `emit_agent_settled` has returned and after the idle latch drops.
//!
//! **What each assertion is for.** The op REACHING the queue proves nothing: `HostServices::control`
//! returned `Ok` before this change too (a native extension never had a tier gate at all — see
//! `cyrup-ext/src/native.rs`'s note). So every assertion below is on the observable effect — a
//! second run happening, the user message in the transcript, and above all the COUNT, because a
//! drain that fires twice is as wrong as one that never fires.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_ext::{
    ControlOp, EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use serde_json::json;
use std::sync::Mutex;
use tempfile::TempDir;

/// A native built-in whose `agent_settled` handler queues ONE `SendUserMessage`, through the same
/// `HostServices::control` seam a wasm guest's `control.send-user-message` import reaches
/// (`cyrup-ext/src/host/live.rs`).
///
/// It fires on the FIRST settle only. An extension that sent on every settle would loop forever —
/// upstream too — so the latch is the test's own bound, not a claim about the mechanism.
#[derive(Default)]
struct SettleSender {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    /// Settles observed, which is also the number of RUNS: `agent_settled` fires exactly once per
    /// run however many `agent_end`s it took (`tests/agent_settled.rs`).
    settles: Arc<AtomicUsize>,
    /// Whether the one send has already been queued.
    sent: Arc<AtomicBool>,
    /// Whether the handler was re-entered while it was still running — the deadlock shape the
    /// store-free drain point exists to prevent.
    reentered: Arc<AtomicBool>,
    in_handler: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl NativeExtension for SettleSender {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ext-087-settle-sender")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentSettled]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::AgentSettled) {
            return HookOutcome::Noop;
        }
        if self.in_handler.swap(true, Ordering::SeqCst) {
            self.reentered.store(true, Ordering::SeqCst);
        }
        self.settles.fetch_add(1, Ordering::SeqCst);
        if !self.sent.swap(true, Ordering::SeqCst) {
            let svc = self.services.lock().ok().and_then(|g| g.clone());
            if let Some(svc) = svc {
                // The whole point of the row: this is an EVENT-tier caller. Pre-fix the wasm import
                // refused it outright with "deadlock guard: session-mutating control op from an
                // event handler"; post-fix it queues, and the post-settle drain applies it.
                let _ = svc.control(ControlOp::SendUserMessage {
                    content: "next".to_string(),
                    opts: json!({}),
                });
            }
        }
        self.in_handler.store(false, Ordering::SeqCst);
        HookOutcome::Noop
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

/// THE proof. An `agent_settled` handler queues one `sendUserMessage`; a second run starts, the
/// text reaches the transcript as a USER message, and it happens exactly once.
///
/// **RED without the change** on the count assertion: pre-fix `settle_run` had no drain at all, so
/// the queued op sat on `LiveHostServices`'s control channel until some later COMMAND happened to
/// run `apply_pending_control`. The mid-loop `apply_pending_agent_control` drains in
/// `drive_accepted_run` see it, do not handle it, and re-queue it (`control.rs`'s
/// `other => Some(other)` arm) — which is exactly the half-fix this test is built to catch, because
/// from the guest's side it looks identical to success: `control()` returns `Ok` either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_from_an_agent_settled_handler_starts_exactly_one_run() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Two scripted turns: the user's prompt, and the run the settled handler's send starts. A third
    // is deliberately NOT scripted — if the drain double-fired, the extra run would show up in the
    // call count rather than being absorbed.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);

    let ext = Arc::new(SettleSender::default());
    let settles = ext.settles.clone();
    let reentered = ext.reentered.clone();

    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(ext as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();

    let _stream = session
        .prompt(UserInput::text("hello", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    // ONE `wait_for_idle`, deliberately, and it is an assertion in its own right: it must cover the
    // run the settled handler's send starts, not just the user's.
    //
    // pi resolves the idle wait with `_resolveIdleWaitIfIdle()` AFTER the deferred settled actions
    // and only if the session is actually idle (`core/agent-session.ts:881-890` @v0.87.1), so a
    // deferred send keeps the wait pending. cyrup's latch drops in `settle_run` BELOW the drain and
    // only when `run_starts` shows nothing was started. Written the obvious way, this line caught
    // the earlier ordering — with the latch dropping above the drain, `wait_for_idle` returned in
    // the gap before `spawn_run` re-raised it and the transcript below still read `["hello"]`.
    session.wait_for_idle().await;

    let messages = session.messages().await;
    let user_texts: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|t| t.contains("next")),
        "the send queued from `agent_settled` reached the session as a user message — pi's \
         `sendUserMessage` is a thin wrapper over `prompt` (`core/agent-session.ts:2030-2035` \
         @v0.87.1): {user_texts:?}"
    );

    // EXACTLY ONE new run. Upstream gets this from `_deferredSettledActions.splice(0)` taking the
    // queue once (`core/agent-session.ts:881`); cyrup gets it from `take_pending_control` being
    // take-once and `settle_run` running once per run. A drain in the continuation loop AND one
    // after the settle would double-fire and show up here as three.
    assert_eq!(
        settles.load(Ordering::SeqCst),
        2,
        "the original run plus exactly one started by the send — not zero (never drained) and not \
         three (drained twice)"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "two provider turns, matching the two runs"
    );

    // The drain point is STORE-FREE: `emit_agent_settled` has fully returned before it runs, so the
    // new run's own dispatch cannot re-enter the handler that queued it. Getting this wrong
    // deadlocks rather than fails, so the latch is checked explicitly.
    assert!(
        !reentered.load(Ordering::SeqCst),
        "the `agent_settled` handler was re-entered during its own dispatch"
    );
}

/// EXT-087 — a COMPILE-TIME assertion, and the one that records a finding rather than a behaviour.
///
/// `apply_agent_state_op`'s doc used to justify excluding the `send_user_message`/`compact` arms by
/// saying their prompt-path futures are `!Send`, so a caller needing a `Send` future — the spawned
/// post-run driver — could not use the full drain. That claim was FALSE, and it was the only thing
/// standing between this row and its fix, so it was checked empirically rather than reasoned about:
/// an `assert_send` probe over `apply_pending_control()`'s future compiles, while a deliberate
/// `Rc`-holding negative control beside it does not.
///
/// The `Box::pin` at the send arm is for E0733 — a finitely sized future across the recursive
/// `apply_pending_control -> prompt_with -> prepare -> try_execute_extension_command ->
/// apply_pending_control` cycle, which is what that arm's own comment says. Boxing a concrete
/// future PRESERVES `Send`; a recursive async cycle can defeat auto-trait INFERENCE without the
/// future being `!Send`.
///
/// **RED without the change:** this does not compile if `apply_pending_control()`'s future ever
/// stops being `Send`, which would also break `settle_run` inside the spawned driver. It is here so
/// that breakage is a named failure rather than a wall of E0277 from `tokio::spawn`.
#[test]
fn ext_087_post_settle_drain_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    fn probe(session: Arc<crate::AgentSession>) {
        assert_send(async move { session.apply_pending_control().await });
    }
    let _ = probe as fn(Arc<crate::AgentSession>);
}

/// EXT-087, the MID-RUN half — a send queued from a handler that fires DURING a run joins THAT
/// run; it does not start a second one.
///
/// **Upstream.** `pi.sendUserMessage` runs synchronously from the handler into `prompt`
/// (`core/agent-session.ts:2006-2035` @v0.87.1), whose streaming branch is:
///
/// ```ts
/// if (this.isStreaming) {
///     if (!options?.streamingBehavior) { throw new Error("Agent is already processing. …"); }
///     if (options.streamingBehavior === "followUp") { await this._queueFollowUp(expandedText, currentImages); }
///     else { await this._queueSteer(expandedText, currentImages); }
///     preflightResult?.(true);
///     return;
/// }
/// ```
/// (`:1653-1666`). `pi.sendMessage` branches the same way at `:1949-1954`
/// (`this.agent.followUp(appMessage)` / `this.agent.steer(appMessage)`). NEITHER starts a run: the
/// message joins the run that is already going, and the whole thing settles ONCE.
///
/// **RED without the change.** `apply_pending_agent_control` — the drain that runs at
/// `drive_accepted_run`'s `handle.finished()` point, i.e. while the run latch is still raised — used
/// to re-queue both sends unconditionally, carrying them to the POST-SETTLE drain. That is a second
/// run: a second `agent_start`/`agent_end`/`agent_settled`, and a `deliverAs` that was parsed and
/// then could not matter because nothing was streaming by the time it was read. So the pre-change
/// tree answers `settles == 2` here, and the post-change tree answers `1`. Reverting
/// `control.rs::apply_pending_agent_control` to the re-queue-everything form fails this on that
/// assertion — proven by doing exactly that.
///
/// The transcript assertion is the second half: "one settle" alone would also pass if the send were
/// silently dropped, so the queued text must be shown to have reached the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_from_a_mid_run_handler_joins_the_current_run_instead_of_starting_a_second() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Two scripted turns for ONE run: the user's prompt, and the continuation the follow-up drives.
    // A third is deliberately unscripted — a second RUN would consume it and show as a call count
    // of 3 rather than being absorbed.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("third")], StopReason::Stop),
    ]);

    let ext = Arc::new(MidRunSender::default());
    let settles = ext.settles.clone();

    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(ext as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();

    let _stream = session
        .prompt(UserInput::text("hello", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;

    let messages = session.messages().await;
    let user_texts: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|t| t.contains("mid-run follow-up")),
        "the send queued from the mid-run `agent_end` handler reached the session as a user \
         message: {user_texts:?}"
    );

    // THE assertion. `agent_settled` fires exactly once per RUN however many `agent_end`s it took
    // (`tests/agent_settled.rs`), so this counts RUNS. Upstream's follow-up joins the live run, so
    // there is exactly one.
    assert_eq!(
        settles.load(Ordering::SeqCst),
        1,
        "the mid-run send was delivered by the CURRENT run (pi `_queueFollowUp`, \
         `core/agent-session.ts:1661` @v0.87.1) — a count of 2 is the pre-change behaviour, where \
         the op was re-queued to the post-settle drain and started a second run"
    );
    // Two provider turns inside that one run: the user's prompt and the queued follow-up's
    // continuation (`handle_post_agent_run`'s `agent.has_queued_messages()`, pi `:1009-1012`).
    assert_eq!(
        faux.call_count(),
        2,
        "two turns of ONE run — three would mean a second run started"
    );
}

/// A native built-in whose `agent_end` handler — a handler that fires DURING the run, before the
/// continuation loop has decided anything — queues one `SendUserMessage` carrying
/// `deliverAs: "followUp"`, through the same `HostServices::control` seam a wasm guest's
/// `control.send-user-message` import reaches.
///
/// One-shot, for the same reason [`SettleSender`] is: a handler that sent on every `agent_end`
/// would never let the run end, upstream too.
#[derive(Default)]
struct MidRunSender {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    /// Settles observed == RUNS observed. This is the number the row turns on.
    settles: Arc<AtomicUsize>,
    sent: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl NativeExtension for MidRunSender {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ext-087-mid-run-sender")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentEnd, EventKind::AgentSettled]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::AgentSettled => {
                self.settles.fetch_add(1, Ordering::SeqCst);
            }
            HostEvent::AgentEnd { .. } if !self.sent.swap(true, Ordering::SeqCst) => {
                let svc = self.services.lock().ok().and_then(|g| g.clone());
                if let Some(svc) = svc {
                    let _ = svc.control(ControlOp::SendUserMessage {
                        content: "mid-run follow-up".to_string(),
                        opts: json!({ "deliverAs": "followUp" }),
                    });
                }
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}
