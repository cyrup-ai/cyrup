//! EXT-006 — a registered renderer must actually be CALLED, resolved from the key alone.
//!
//! Pi has two distinct renderer surfaces:
//!   1. per-TOOL `ToolDefinition.renderCall`/`renderResult` (`coding-agent/src/core/extensions/
//!      types.ts:472-481`), resolved by `modes/interactive/components/tool-execution.ts:81-112`
//!      (`getCallRenderer`/`getResultRenderer`/`hasRendererDefinition`), and
//!   2. per-custom-type `registerMessageRenderer(customType, renderer)` (`types.ts:1284`), resolved
//!      by `extensions/runner.ts:579-587 getMessageRenderer` — FIRST extension in load order wins.
//!
//! In cyrup both were dead END TO END: `register-message-renderer` wrote to a per-guest
//! `Mutex<Vec<String>>` nothing consumed; `ToolDescriptor.has_renderer` was set and read nowhere;
//! and `LiveExtension::render_call`/`render_result` had exactly one caller — a unit test holding
//! the `LiveExtension` handle directly. There was NO path from a tool name or a custom type to the
//! extension that renders it.
//!
//! These tests therefore never touch the `LiveExtension` handle: they ask the HOST to render by
//! name/type and assert the guest's own text came back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::fixture;

use cyrup_ext::DenyServices;
use serde_json::json;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_routes_a_tool_row_to_the_guest_that_renders_it() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");

    // The cheap pre-check a UI makes before paying for a guest round trip (Pi
    // `hasRendererDefinition`). `demo_echo` declares `has_renderer`; `signal_probe` does not.
    assert!(
        host.has_tool_renderer("demo_echo"),
        "demo_echo declared a renderer"
    );
    assert!(
        !host.has_tool_renderer("signal_probe"),
        "signal_probe declared none"
    );
    assert!(
        !host.has_tool_renderer("bash"),
        "a built-in with no extension renderer"
    );

    // Render by TOOL NAME only — no `LiveExtension` handle in sight.
    let call = host
        .render_tool_call("demo_echo", &json!({ "text": "hi" }))
        .await
        .expect("the host resolved and CALLED the guest's tool-call renderer");
    // The guest returns a serialized WIDGET TREE (`cyrup_ext_sdk::widget`), the wire analog of the
    // `pi-tui` Component a Pi renderer returns — here a `Container` of two `Text` rows. The host
    // facade hands it back verbatim; `cyrup-tui` is what flattens it to rows (see
    // `cyrup-tui/tests/wasm_renderer_screen.rs` for the end-to-end draw).
    assert_eq!(call["widget"], json!("container"));
    assert!(
        call["children"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("guest-rendered echo call"),
        "the rendered widget came from the GUEST: {call}"
    );

    let result = host
        .render_tool_result("demo_echo", &json!({ "content": "echo: hi" }))
        .await
        .expect("the host resolved and CALLED the guest's tool-result renderer");
    assert!(
        result["text"]
            .as_str()
            .unwrap_or("")
            .contains("guest-rendered echo result"),
        "the rendered widget came from the GUEST: {result}"
    );

    // A tool nobody renders falls back to the host's own framing (`None`), never an error.
    assert!(
        host.render_tool_call("signal_probe", &json!({}))
            .await
            .is_none()
    );
    assert!(host.render_tool_result("bash", &json!({})).await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_routes_a_custom_message_to_its_registered_renderer() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");

    assert!(
        host.has_message_renderer("demo"),
        "the guest registered a renderer for `demo`"
    );
    assert!(!host.has_message_renderer("nope"));

    let widget = host
        .render_message_call("demo", &json!({ "x": 1 }))
        .await
        .expect("the host resolved and CALLED the guest's message renderer");
    assert!(
        widget["text"].as_str().unwrap_or("").contains("demo call"),
        "the rendered widget came from the GUEST: {widget}"
    );

    assert!(
        host.render_message_call("nope", &json!({})).await.is_none(),
        "an unregistered custom type falls back to the host's framing"
    );
}

/// First registration wins, matching Pi's load-order `getMessageRenderer` loop (runner.ts:579-587).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_extension_in_load_order_owns_a_custom_type() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo-first".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load 1");
    host.load_wasm("demo-second".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load 2");

    assert_eq!(
        host.registry()
            .message_renderer_owner("demo")
            .expect("registry")
            .map(|i| i.to_string()),
        Some("demo-first".to_string()),
        "the FIRST extension to register a custom type keeps it"
    );
}

// ---------------------------------------------------------------------------
// X15 — the custom-ENTRY surface, GUEST arm. The native arm is `entry_renderer.rs`.
// ---------------------------------------------------------------------------

/// A guest that registered `register-entry-renderer` is reachable by custom type, and its output is
/// its own (Pi `getEntryRenderer(entry.customType)`, `extensions/runner.ts:593-600`, resolved by
/// `addCustomEntryToChat`, `interactive-mode.ts:3431-3436`).
///
/// The entry table must stay DISJOINT from the message table: the demo guest registers `demo` as a
/// MESSAGE renderer and `demo_card` as an ENTRY renderer, exactly as upstream keeps
/// `extension.messageRenderers` and `extension.entryRenderers` apart (`types.ts:1703-1704`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_routes_a_custom_entry_to_its_registered_guest_renderer() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");

    assert!(
        host.has_entry_renderer("demo_card"),
        "the guest registered an ENTRY renderer"
    );
    assert!(
        !host.has_entry_renderer("demo"),
        "`demo` is a MESSAGE renderer, not an entry one"
    );
    assert!(
        !host.has_message_renderer("demo_card"),
        "and not the other way round either"
    );
    assert!(!host.has_entry_renderer("nope"));

    match host.render_entry("demo_card", &json!({ "n": 7 })).await {
        cyrup_ext::RenderOutcome::Rendered(v) => assert!(
            v["text"]
                .as_str()
                .unwrap_or("")
                .contains("guest-rendered entry card"),
            "the rendered widget came from the GUEST: {v}"
        ),
        other => panic!("expected the guest's output, got {other:?}"),
    }

    // "No renderer claims this type" stays `None` — never `Failed`, or the failure box would draw
    // for every unrendered entry (`interactive-mode.ts:3433-3435` draws nothing at all).
    assert_eq!(
        host.render_entry("nope", &json!({})).await,
        cyrup_ext::RenderOutcome::None
    );
}

/// THE REGRESSION, guest arm. A guest entry renderer that FAULTS (a panic lowers to a wasm trap) is
/// reported as `RenderOutcome::Failed`, not as the bare `None` that `render_via` used to collapse
/// every fault into. `None` also means "no renderer", so the two were the same value and Pi's
/// `[type] renderer failed: …` box (`custom-entry.ts:47-52`) could never be produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_faulting_guest_entry_renderer_reports_failed_not_none() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");

    assert!(
        host.has_entry_renderer("demo_boom"),
        "the faulting renderer IS registered"
    );

    let out = host.render_entry("demo_boom", &json!({})).await;
    assert!(
        out.failure().is_some(),
        "a trapping guest renderer is a FAULT, distinct from `None` — got {out:?}"
    );
    assert_ne!(
        out,
        cyrup_ext::RenderOutcome::None,
        "collapsing this to `None` is exactly the bug: it is indistinguishable from \
         `has_entry_renderer == false`"
    );
}
