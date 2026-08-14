//! WASM UI-DIALOG CAPABILITY END-TO-END (L4 review §2.1 — the "interactive TUI has ZERO wiring for
//! extension confirm/input/select/editor dialogs" finding). Proves that a LIVE wasm guest's
//! `ui.{confirm,input,select,editor}` calls round-trip through the REAL session → `LiveHostServices`
//! → `ui_roundtrip` → `UiSink` mechanism `cyrup-tui`'s `App::run` now drives (the same mechanism
//! `crates/cyrup-modes/src/rpc.rs`'s `run_rpc` already used) — not a stub, not a canned answer, and
//! not a self-deadlock.
//!
//! `confirmdemo`/`inputdemo`/`selectdemo`/`editordemo` (`cyrup-ext-sdk/src/example.rs`) each open ONE
//! dialog of the matching kind, then open a SECOND `confirm` dialog whose PROMPT embeds the value just
//! received — proving the guest resumes with the REAL answer (not a default), across TWO SEQUENTIAL
//! synchronous host round trips from a SINGLE guest invocation. A scripted [`UiSink`] answers each
//! request with a DISTINCT, kind-specific value so a wrong/default value cannot pass by coincidence.
//!
//! The fixture is the bundled `cyrup-ext-sdk` demo extension, built to a `wasm32-wasip2` component.
//! Set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt component to skip the nested build.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, UiKind, UiReply, UiRequest};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// Wire a scripted [`UiSink`] onto `session` that answers every request immediately with a
/// kind-specific canned value (mirroring what `App::run`'s `ui_rx` arm / `run_rpc`'s `ui_rx` arm do
/// once a human/RPC-client answers) and records every `(kind, prompt)` pair it saw, in order.
fn wire_scripted_sink(session: &Arc<AgentSession>) -> Arc<Mutex<Vec<(UiKind, String)>>> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    session.services().host_services.set_ui_sink(tx);
    let seen: Arc<Mutex<Vec<(UiKind, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            seen2.lock().unwrap_or_else(|e| e.into_inner()).push((req.kind, req.prompt.clone()));
            let reply = match req.kind {
                UiKind::Confirm => UiReply::Confirm(true),
                UiKind::Input => UiReply::Text(Some("Ada Lovelace".to_string())),
                UiKind::Select => {
                    // Echo the LAST option string, distinguishing a real answer from any default.
                    let chosen = req
                        .options
                        .as_array()
                        .and_then(|a| a.last())
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    UiReply::Text(chosen)
                }
                UiKind::Editor => UiReply::Text(Some("edited by the scripted sink".to_string())),
            };
            let _ = req.reply.send(reply);
        }
    });
    seen
}

async fn build_session() -> (Arc<AgentSession>, Arc<cyrup_ext::host::LiveExtension>) {
    let bytes = bins::component_bytes();
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Leak the TempDir so its files outlive this function (the session/extension keep running).
    std::mem::forget(tmp);

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;

    let session = Arc::new(
        SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, cfg).build().await.unwrap(),
    );
    let ext = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes)
        .await
        .expect("load + init the live wasm extension");
    (session, ext)
}

/// `confirmdemo`: `ctx.ui().confirm_with("proceed?", "...", opts)` then a follow-up `confirm` whose
/// prompt embeds the received bool. Both round trips happen inside ONE guest invocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_confirm_dialog_receives_the_real_scripted_answer() {
    let (session, _ext) = build_session().await;
    let seen = wire_scripted_sink(&session);

    let _ = session.prompt("/confirmdemo").await.unwrap();
    session.wait_for_idle().await;

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "confirmdemo opens exactly two dialogs: {seen:?}");
    assert_eq!(seen[0].0, UiKind::Confirm);
    assert_eq!(seen[0].1, "proceed?");
    assert_eq!(
        seen[1],
        (UiKind::Confirm, "you answered: true".to_string()),
        "the guest's SECOND dialog embeds the REAL answer received from the FIRST, not a default: {seen:?}"
    );
}

/// `inputdemo`: `ctx.ui().input_with("name?", ...)` then a follow-up `confirm` embedding the typed
/// text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_input_dialog_receives_the_real_scripted_answer() {
    let (session, _ext) = build_session().await;
    let seen = wire_scripted_sink(&session);

    let _ = session.prompt("/inputdemo").await.unwrap();
    session.wait_for_idle().await;

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "inputdemo opens exactly two dialogs: {seen:?}");
    assert_eq!(seen[0].0, UiKind::Input);
    assert_eq!(
        seen[1],
        (UiKind::Confirm, "you typed: Some(\"Ada Lovelace\")".to_string()),
        "the guest's SECOND dialog embeds the REAL typed text, not a default: {seen:?}"
    );
}

/// `selectdemo`: `ctx.ui().select("pick one", &["alpha","beta","gamma"])` then a follow-up `confirm`
/// embedding the chosen option STRING (not an index).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_select_dialog_receives_the_real_scripted_answer() {
    let (session, _ext) = build_session().await;
    let seen = wire_scripted_sink(&session);

    let _ = session.prompt("/selectdemo").await.unwrap();
    session.wait_for_idle().await;

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "selectdemo opens exactly two dialogs: {seen:?}");
    assert_eq!(seen[0].0, UiKind::Select);
    assert_eq!(
        seen[1],
        (UiKind::Confirm, "you picked: gamma".to_string()),
        "the guest's SECOND dialog embeds the chosen STRING (the scripted sink's last option), not an \
         index or a default: {seen:?}"
    );
}

/// `editordemo`: `ctx.ui().editor("edit demo", "seed text from the guest")` then a follow-up
/// `confirm` embedding the edited text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_editor_dialog_receives_the_real_scripted_answer() {
    let (session, _ext) = build_session().await;
    let seen = wire_scripted_sink(&session);

    let _ = session.prompt("/editordemo").await.unwrap();
    session.wait_for_idle().await;

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "editordemo opens exactly two dialogs: {seen:?}");
    assert_eq!(seen[0].0, UiKind::Editor);
    // L4 review §2 (editor title fix): `req.prompt` is now genuinely the guest's `title` argument
    // (Pi `editor(title, prefill)`, types.ts:216; world.wit:267) — a REAL wasm guest call, not the
    // hardcoded `""` the pre-fix wire request sent. The seed text arrives separately on
    // `req.message`, unit-proven distinctly in `host_services.rs`'s
    // `ui_grant_round_trips_through_a_scripted_sink`.
    assert_eq!(seen[0].1, "edit demo", "the LIVE wasm guest's real editor title arrived, not \"\"");
    assert_eq!(
        seen[1],
        (UiKind::Confirm, "edited: edited by the scripted sink".to_string()),
        "the guest's SECOND dialog embeds the edited text, not the seed or a default: {seen:?}"
    );
}

/// The `ctrl+t` shortcut (R-08-017) opens a `confirm` dialog then a follow-up `confirm` embedding the
/// answer — driven through `ExtensionHost::run_shortcut` (the path `App::run`'s `AppAction::
/// ExtensionShortcut` arm now SPAWNS rather than awaits inline, to avoid self-deadlocking against its
/// own `ui_rx` servicing). Proves TWO SEQUENTIAL synchronous round trips complete correctly from a
/// shortcut handler too, not just a command handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_shortcut_confirm_dialog_receives_the_real_scripted_answer() {
    let (session, _ext) = build_session().await;
    let seen = wire_scripted_sink(&session);

    let cancel = cyrup_core::CancelToken::new();
    session.services().ext_host.run_shortcut("ctrl+t", &cancel).await.expect("shortcut runs");

    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.len(), 2, "the ctrl+t shortcut opens exactly two dialogs: {seen:?}");
    assert_eq!(seen[0], (UiKind::Confirm, "shortcut confirm — proceed?".to_string()));
    assert_eq!(
        seen[1],
        (UiKind::Confirm, "shortcut confirmed: true".to_string()),
        "the shortcut's SECOND dialog embeds the REAL answer received from the FIRST: {seen:?}"
    );
}

/// L4 review §2.1 regression guard: a SECOND, INDEPENDENT command against the SAME live extension
/// after a completed dialog round trip ACTUALLY EXECUTES (not merely still `registered` — a Store
/// that already trapped once can stay listed in the command registry while every further call into
/// it silently no-ops; asserting on `ctx.ui().notify(...)` having fired proves the guest genuinely
/// ran, not just that its name is still known).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_extension_stays_usable_after_a_dialog_round_trip() {
    let (session, ext) = build_session().await;
    let _seen = wire_scripted_sink(&session);

    let _ = session.prompt("/confirmdemo").await.unwrap();
    session.wait_for_idle().await;

    // A second, unrelated dialog-free command still runs against the SAME extension instance.
    let before = ext.guest().notifications().len();
    let _ = session.prompt("/execdemo").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications()[before..].iter().any(|n| n.starts_with("exec stdout:")),
        "execdemo genuinely ran (not a silent no-op) after a completed dialog round trip: {:?}",
        ext.guest().notifications()
    );
}

/// Closes the CRITICAL, still-open finding that the WASM epoch budget
/// (`ExtensionHost::WASM_EPOCH_BUDGET_TICKS`, `crates/cyrup-ext/src/facade.rs`, ~5s) used to bound
/// the ENTIRE `ui.*` dialog wait: a human (here, a scripted sink standing in for one) taking longer
/// than the budget to answer left the deadline already expired by the time the guest resumed wasm
/// execution right after the blocking host call returned — tripping an epoch trap that permanently
/// wedges the instance (component-model reentrance bookkeeping never sees a clean completion).
/// Reproduces the audit's OWN exact repro methodology: a REAL ~6s-delayed reply (well past the ~5s
/// budget), driven through the actual `SessionBuilder`/`ExtensionHost::load_wasm` production path
/// (the real `WASM_EPOCH_BUDGET_TICKS`, not a test-shortened one) — `confirmdemo` uses
/// `DialogOptions::default()` (no `timeout_ms`), so `LiveHostServices::ui_roundtrip`'s OWN
/// independent host-side timeout race (the separate, already-covered Finding 2) never fires either,
/// isolating this test to the epoch mechanism alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_dialog_delayed_past_the_epoch_budget_does_not_wedge_the_extension() {
    let (session, ext) = build_session().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    session.services().host_services.set_ui_sink(tx);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            // A REAL delay past the ~5s `WASM_EPOCH_BUDGET_TICKS` budget — matches the audit's own
            // repro ("a 6s-delayed reply").
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            let reply = match req.kind {
                UiKind::Confirm => UiReply::Confirm(true),
                _ => UiReply::Text(None),
            };
            let _ = req.reply.send(reply);
        }
    });

    let _ = session.prompt("/confirmdemo").await.unwrap();
    session.wait_for_idle().await;

    // The delayed reply still resolved to the REAL answer, not a timeout/trap-induced default —
    // `confirmdemo` (`cyrup-ext-sdk/src/example.rs`) notifies `"confirmed: {ok}"` right after the
    // FIRST (delayed) dialog returns, before ever opening its second, nested confirm.
    let notifications = ext.guest().notifications();
    assert!(
        notifications.iter().any(|n| n == "confirmed: true"),
        "the delayed dialog resolved to the real scripted answer, not a default: {notifications:?}"
    );

    // THE headline proof: a later, unrelated, dialog-free command against the SAME instance still
    // genuinely runs. Before this fix, the epoch trap right after the delayed reply permanently
    // wedged the instance and this silently no-op'd.
    let before = ext.guest().notifications().len();
    let _ = session.prompt("/execdemo").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications()[before..].iter().any(|n| n.starts_with("exec stdout:")),
        "the extension survives a dialog delayed past the epoch budget — a later command still \
         genuinely runs, not a silent no-op: {:?}",
        ext.guest().notifications()
    );
}
