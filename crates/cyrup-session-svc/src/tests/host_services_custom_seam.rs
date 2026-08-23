//! TUI-030 — the `custom` seam: the four working-indicator verbs reaching (or silently dropping
//! without) the mode's [`crate::host_services::UiEffect`] drain, and `custom()` driving a guest
//! spec through the overlay renderer or declining without one.
//!
//! One of the five files the inline `mod tests` in `host_services.rs` became when that file was
//! split into `src/host_services/`; this is the section its `TUI-030 / the `custom` seam` banner
//! opened. Shares [`super::host_services_core::svc_with`] with its siblings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_ext::host::HostServices;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use serde_json::{json, Value};

use crate::host_services::{OverlayRequest, UiEffect};

use super::host_services_core::svc_with;

/// TUI-030 — the four working-indicator verbs must reach the mode's effect drain.
///
/// **PRE-FIX this test fails on its first assertion**: `LiveHostServices` overrode none of the
/// four, so each call took the `HostServices` trait's empty default body, `emit_ui_effect` was
/// never reached, and `drained` came back EMPTY — `assert_eq!(got.len(), 4)` saw `0`. The test
/// deliberately drives the four `HostServices` methods on the LIVE backend (not the `UiEffect`
/// enum, and not a shared helper) precisely because that is the seam that was dead: a test that
/// constructed the four variants by hand would pass against the unfixed tree.
#[tokio::test]
async fn the_working_indicator_family_reaches_the_effect_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
    svc.set_ui_effect_sink(tx);

    svc.set_working_message(Some("indexing the repo"));
    svc.set_working_visible(false);
    svc.set_working_indicator(Some(&json!({"frames": ["-", "\\", "|", "/"], "intervalMs": 120})));
    svc.set_hidden_thinking_label(Some("redacted"));

    let mut got = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        got.push(effect);
    }
    assert_eq!(got.len(), 4, "all four verbs must emit; got {got:?}");
    assert_eq!(
        got[0],
        UiEffect::SetWorkingMessage { message: Some("indexing the repo".to_string()) }
    );
    assert_eq!(got[1], UiEffect::SetWorkingVisible { visible: false });
    assert_eq!(
        got[2],
        UiEffect::SetWorkingIndicator {
            options: Some(json!({"frames": ["-", "\\", "|", "/"], "intervalMs": 120}))
        },
        "the whole `WorkingIndicatorOptions` bag rides through, not just the frames"
    );
    assert_eq!(
        got[3],
        UiEffect::SetHiddenThinkingLabel { label: Some("redacted".to_string()) }
    );

    // `None` is upstream's no-argument call ("restore the default") and must be DISTINGUISHABLE
    // from "never called" — it is a value on the wire, not an absence.
    svc.set_working_message(None);
    svc.set_hidden_thinking_label(None);
    svc.set_working_indicator(None);
    assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetWorkingMessage { message: None }));
    assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetHiddenThinkingLabel { label: None }));
    assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetWorkingIndicator { options: None }));
}

/// MIRROR: with NO effect sink (headless print/json) the four silently drop and — critically —
/// never block, which is Pi's `noOpUIContext` (`core/extensions/runner.ts:242-245` @v0.84.2,
/// four `() => {}` bodies). A single-thread runtime proves no `block_in_place` is reached.
#[test]
fn the_working_indicator_family_is_a_silent_no_op_without_a_sink() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    svc.set_working_message(Some("nobody is listening"));
    svc.set_working_visible(true);
    svc.set_working_indicator(None);
    svc.set_hidden_thinking_label(Some("x"));
}

/// SEAM — `custom` must reach a real interactive surface for a WASM guest.
///
/// **PRE-FIX this test fails on its first assertion**: `LiveHostServices` did not override
/// `custom` at all, so it took the trait default `None`. The scripted renderer below would
/// never receive an `OverlayRequest` (`took_it` stays `false`) and the returned value would be
/// `None` instead of the chosen option id. Nothing else in the tree could have made it pass:
/// `open_overlay` — the NATIVE tier's route, which was always implemented — is only reached
/// here because the fix routes the guest's spec onto it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_drives_a_guest_spec_through_the_overlay_renderer() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = Arc::new(svc_with(provider));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OverlayRequest>();
    svc.set_overlay_sink(tx);

    // The scripted renderer: paint once (proving the spec really became a driveable component),
    // press Down then Enter (the human choosing the SECOND row), then tear the modal down.
    let painted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let painted2 = Arc::clone(&painted);
    tokio::spawn(async move {
        while let Some(mut req) = rx.recv().await {
            let rows = req.overlay.render(60, 24);
            *crate::sync::lock(&painted2) =
                rows.iter().map(cyrup_ext::host::OverlayLine::plain_text).collect();
            req.overlay.handle_key(cyrup_ext::host::OverlayKey::plain(
                cyrup_ext::host::OverlayKeyCode::Down,
            ));
            req.overlay.handle_key(cyrup_ext::host::OverlayKey::plain(
                cyrup_ext::host::OverlayKeyCode::Enter,
            ));
            // Dropping `req.overlay` here would be the renderer closing the modal; the caller
            // is released by the `done` one-shot, exactly as the TUI's run loop releases it.
            let _ = req.done.send(());
        }
    });

    let s = Arc::clone(&svc);
    let picked = tokio::task::spawn_blocking(move || {
        s.custom(&json!({
            "title": "Pick a target",
            "lines": ["two hosts are reachable"],
            "options": ["staging", {"id": "prod", "label": "production (careful)"}],
        }))
    })
    .await
    .expect("the custom task");

    assert_eq!(
        picked.as_deref(),
        Some("prod"),
        "the chosen row's id comes back to the guest, not its label"
    );
    let rows = crate::sync::lock(&painted).clone();
    assert_eq!(
        rows,
        vec![
            "Pick a target".to_string(),
            "two hosts are reachable".to_string(),
            "> staging".to_string(),
            "  production (careful)".to_string(),
        ],
        "the guest's spec really rendered — title, body, then the options with a gutter marker"
    );
}

/// MIRROR: with NO overlay renderer (headless print/json, and RPC — whose wire cannot stream
/// keystrokes into a host component) `custom` answers `None` WITHOUT blocking, which is pi's own
/// RPC body verbatim (`async custom() { return undefined as never }`,
/// `modes/rpc/rpc-mode.ts:228-231` @v0.84.2). A single-thread runtime proves the non-blocking part: a
/// `block_in_place` would panic here.
#[test]
fn custom_declines_without_an_overlay_renderer_and_without_blocking() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    assert_eq!(svc.custom(&json!({"title": "hi", "options": ["a"]})), None);
    // …and an empty/garbage spec is declined even WITH a renderer, rather than opening a blank
    // modal a human has to dismiss.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<OverlayRequest>();
    svc.set_overlay_sink(tx);
    assert_eq!(svc.custom(&json!({})), None);
    assert_eq!(svc.custom(&Value::Null), None);
}
