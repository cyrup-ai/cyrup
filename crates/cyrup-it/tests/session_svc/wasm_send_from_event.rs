//! EXT-087 LIVE-COMPONENT PROOF — a real `wasm32-wasip2` guest whose `agent_settled` handler calls
//! `send-user-message`, inside an assembled `AgentSession`.
//!
//! This is the half a stub cannot prove. The row's other tests
//! (`cyrup-session-svc/src/tests/ext_087_send_from_event.rs`) drive a NATIVE extension, and a native
//! extension never had a tier gate at all — it queues through `HostServices::control` directly. The
//! gate being removed lives on the WASM import (`cyrup-ext/src/host/live.rs::send_user_message`),
//! so only a live guest crossing that import can show it is gone.
//!
//! **Upstream.** `sendUserMessage(content, options): void { assertActive();
//! runtime.sendUserMessage(content, options); }` (`core/extensions/loader.ts:356-358` @v0.87.1) —
//! no tier check, callable from every handler. pi v0.87.0 added a DEFERRAL for the settled case
//! only, because its `sendUserMessage` reaches `prompt()` synchronously inside the dispatch:
//! `_isEmittingAgentSettled` (`core/agent-session.ts:378`), `_deferredSettledActions` (`:379`),
//! spliced and awaited once the emit's `finally` has cleared the flag (`:873-885`). cyrup's send
//! crosses onto the control queue, so the queue is the deferral and what it needed was the drain
//! point — `AgentSession::settle_run`.
//!
//! Modelled on `gap11_event_tier_verify.rs`, which is the live-component proof for the GAP-11
//! exemption this row extends.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

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
    cfg.model_pattern = Some("faux-1".to_string());
    cfg
}

/// THE verification: a live guest's `agent_settled` handler sends a user message; it is accepted
/// (not refused by the deadlock guard), it starts EXACTLY ONE run, and that run's dispatch does not
/// re-enter the handler that queued it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_guest_sends_a_user_message_from_its_agent_settled_handler() {
    let bytes = bins::component_bytes();
    let fx = fixture();

    let faux = Arc::new(FauxProvider::new());
    // Three scripted turns for what should be TWO real ones: the user's prompt and the run the
    // settled handler's send starts. The spare is deliberate — if the drain double-fired, the extra
    // run would consume it and show up as a call count of 3 rather than being swallowed.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);

    // BIND the session (`into_shared`), as production does: only a bound session spawns the
    // post-run driver whose `settle_run` hosts the post-settle drain.
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // Arm the guest's one-shot settled send. The fixture component is shared by this whole suite,
    // so the handler is inert until a test asks for it — see `cyrup-ext-sdk/src/example/hooks.rs`.
    let _ = session.prompt("/armsend").await.unwrap();
    session.wait_for_idle().await;

    // ONE real turn. Its settle fires `agent_settled`, whose handler crosses the WIT import.
    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;

    // (a) NO error string reached the guest. Pre-fix the import answered
    //     "deadlock guard: session-mutating control op from an event handler", which the guest
    //     notifies verbatim — so this assertion is the one that fails against the old live.rs.
    let notes = ext.guest().notifications();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("demo: settled send queued")),
        "the guest's event-tier `send-user-message` was ACCEPTED across the wasm import; \
         notes: {notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("settled send refused")),
        "no deadlock-guard refusal may surface to the guest — upstream's `sendUserMessage` has no \
         tier check at all (`core/extensions/loader.ts:356-358` @v0.87.1); notes: {notes:?}"
    );

    // (b) The send actually STARTED A RUN, and the text reached the transcript as a user message —
    //     pi's `sendUserMessage` is a thin wrapper over `prompt` (`core/agent-session.ts:2030-2035`).
    //     Accepting the call without running it is the half-fix this asserts against: it looks
    //     identical from the guest's side.
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
        user_texts
            .iter()
            .any(|t| t.contains("from the settled handler")),
        "the queued send reached the session as a user message: {user_texts:?}"
    );

    // (c) EXACTLY ONE new run. `/armsend` short-circuits as a command and streams nothing, so the
    //     provider turns are the user's "hello" plus the one the send started. Upstream gets this
    //     from `_deferredSettledActions.splice(0)` taking the queue once (`:881`); cyrup gets it
    //     from `take_pending_control` being take-once and `settle_run` running once per run.
    assert_eq!(
        faux.call_count(),
        2,
        "one run for the prompt and one for the send — not three (drained twice)"
    );

    // (d) The settled handler was not re-entered by the new run's own dispatch: it is armed for a
    //     SINGLE firing, so a re-entrant settle would have found the latch already cleared and
    //     notified nothing more. Exactly one "queued" note is therefore the re-entrancy check, and
    //     it is why the drain must sit where every `LiveExtension.inner` store guard is released —
    //     getting that wrong deadlocks rather than failing an assertion.
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.contains("demo: settled send queued"))
            .count(),
        1,
        "the one-shot send fired once; notes: {notes:?}"
    );
}
