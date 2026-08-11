//! X15 — the custom-ENTRY renderer surface, and the three-state [`RenderOutcome`] it needs.
//!
//! Upstream keeps TWO renderer surfaces over custom types and they draw different things when the
//! renderer misbehaves:
//!
//! * `CustomMessageComponent.rebuild` (`coding-agent/src/modes/interactive/components/
//!   custom-message.ts:82-84`) — `catch { /* Fall through to default rendering */ }`, so a throw is
//!   indistinguishable from "no renderer" BY DESIGN on that surface;
//! * `CustomEntryComponent.rebuild` (`.../custom-entry.ts:47-52`) — `catch` builds
//!   `Box(1, 1, customMessageBg)` + `theme.fg("error", `[${customType}] renderer failed: ${msg}`)`.
//!   `custom-entry.ts:50` is the ONLY occurrence of `renderer failed` anywhere in `pi/packages`.
//!
//! cyrup's `render_via` contained every fault as `warn!` + `None`, and `None` is also what "no
//! renderer is registered" returns — so the two were the same value and the ported failure box could
//! never be produced. These tests pin the distinction at the host seam, on the native arm; the guest
//! arm is `wasm_renderer_routing.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::ExtensionId;
use cyrup_ext::{
    ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi, NativeExtension,
    RenderOutcome,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: std::path::PathBuf::from(".") }
}

/// Registers an ENTRY renderer for `card` (draws), `boom` (panics — upstream's `throw`) and `quiet`
/// (returns `undefined`, upstream's `Component | undefined` opting out), plus a MESSAGE renderer for
/// `msg_boom` so the message surface's own collapse can be asserted against the same fault.
struct EntryExt;

#[async_trait::async_trait]
impl NativeExtension for EntryExt {
    fn id(&self) -> ExtensionId {
        "entries".into()
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.register_entry_renderer("card");
        api.register_entry_renderer("boom");
        api.register_entry_renderer("quiet");
        api.register_message_renderer("msg_boom");
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    fn render_entry(&self, custom_type: &str, entry: &Value) -> Option<Value> {
        match custom_type {
            "card" => Some(json!({ "widget": "text", "text": format!("card: {entry}") })),
            "boom" => panic!("entry renderer exploded"),
            _ => None,
        }
    }

    fn render_call(&self, key: &str, _call: &Value) -> Option<Value> {
        if key == "msg_boom" {
            panic!("message renderer exploded");
        }
        None
    }
}

async fn host() -> ExtensionHost {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(EntryExt)).await.expect("load native");
    host
}

/// The cheap pre-check upstream makes before constructing the component
/// (`if (!renderer) { return; }`, `interactive-mode.ts:3433-3435`), and the proof that the entry
/// table is DISJOINT from the message table — upstream stores them in two maps
/// (`extension.messageRenderers` / `extension.entryRenderers`, `extensions/types.ts:1703-1704`).
#[tokio::test]
async fn the_entry_renderer_table_is_disjoint_from_the_message_renderer_table() {
    let host = host().await;

    assert!(host.has_entry_renderer("card"), "registered on the entry surface");
    assert!(host.has_entry_renderer("boom"));
    assert!(!host.has_entry_renderer("nobody"), "no extension claims this type");

    // `registerMessageRenderer("msg_boom")` must NOT make `msg_boom` an ENTRY renderer, and
    // `registerEntryRenderer("card")` must NOT make `card` a MESSAGE renderer.
    assert!(!host.has_entry_renderer("msg_boom"), "a MESSAGE renderer is not an entry renderer");
    assert!(!host.has_message_renderer("card"), "an ENTRY renderer is not a message renderer");
    assert!(host.has_message_renderer("msg_boom"));
}

/// `custom-entry.ts:58-60` — the renderer returned a component.
#[tokio::test]
async fn a_rendering_entry_renderer_reports_its_output() {
    let host = host().await;
    let out = host.render_entry("card", &json!({ "n": 1 })).await;
    match out {
        RenderOutcome::Rendered(v) => {
            assert!(
                v["text"].as_str().unwrap_or("").contains("card: "),
                "the extension's own widget tree came back: {v}"
            );
        }
        other => panic!("expected Rendered, got {other:?}"),
    }
}

/// THE REGRESSION. `custom-entry.ts:47-52`: a renderer that throws is caught and drawn as
/// `[type] renderer failed: {message}`. The panic message must SURVIVE to the caller — before X15
/// `render_via` swallowed it into a bare `None`.
#[tokio::test]
async fn a_panicking_entry_renderer_reports_failed_and_keeps_the_message() {
    let host = host().await;
    let out = host.render_entry("boom", &json!({})).await;
    assert_eq!(
        out.failure(),
        Some("entry renderer exploded"),
        "the panic payload is upstream's `error.message` (`custom-entry.ts:48`), got {out:?}"
    );
    assert!(matches!(out, RenderOutcome::Failed(_)));
}

/// The two "nothing came back" cases upstream deliberately treats alike (`!component`,
/// `custom-entry.ts:54-56`, and `if (!renderer) return`, `interactive-mode.ts:3433-3435`).
///
/// This is the OTHER half of the regression: `None` must remain reachable and must NOT be confused
/// with a fault, or the failure box would start drawing for every unrendered entry.
#[tokio::test]
async fn no_renderer_and_a_renderer_that_draws_nothing_are_both_none_never_failed() {
    let host = host().await;

    let unclaimed = host.render_entry("nobody", &json!({})).await;
    assert_eq!(unclaimed, RenderOutcome::None, "no extension registered `nobody`");
    assert_eq!(unclaimed.failure(), None, "absence is not a fault");

    let opted_out = host.render_entry("quiet", &json!({})).await;
    assert_eq!(opted_out, RenderOutcome::None, "the renderer returned `undefined`");
    assert_eq!(opted_out.failure(), None, "opting out is not a fault");
}

/// The MESSAGE surface keeps its upstream behaviour: `custom-message.ts:82-84` catches the throw and
/// falls through to the default `[type] body` box, so `render_message_call` still answers `None`.
/// The fault is nevertheless AVAILABLE on the `_outcome` form — that availability is the whole point
/// of the change, and it is what stops a future consumer from having to re-derive it.
#[tokio::test]
async fn the_message_surface_still_collapses_a_fault_but_the_outcome_form_exposes_it() {
    let host = host().await;

    assert_eq!(
        host.render_message_call("msg_boom", &json!({})).await,
        None,
        "`custom-message.ts:82-84` falls through to the default box"
    );
    assert_eq!(
        host.render_message_call_outcome("msg_boom", &json!({})).await.failure(),
        Some("message renderer exploded"),
        "the fault survives to callers that need it"
    );
    // …and "no renderer at all" is still a different value from "the renderer threw".
    assert_eq!(
        host.render_message_call_outcome("unclaimed", &json!({})).await,
        RenderOutcome::None
    );
}

/// A faulting renderer must not take the host down with it (R-08-036) — the containment that was
/// already there and must survive the refactor. A second render after the panic still works.
#[tokio::test]
async fn a_faulting_renderer_is_contained_and_the_host_keeps_rendering() {
    let host = host().await;
    for _ in 0..3 {
        assert!(host.render_entry("boom", &json!({})).await.failure().is_some());
    }
    assert!(matches!(
        host.render_entry("card", &json!({ "after": true })).await,
        RenderOutcome::Rendered(_)
    ));
}
