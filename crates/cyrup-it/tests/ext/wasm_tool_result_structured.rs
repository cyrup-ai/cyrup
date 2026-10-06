//! LIVE coverage for the `structured-content-json` of the re-signed `events.on-tool-result`
//! export (world 0.16; pi `ToolResultEventBase.structuredContent` / `ToolResultEventResult.
//! structuredContent`, `core/extensions/types.ts:1238`, `:1445` @v1.0.1).
//!
//! The `cyrup-ext-sdk` demo is built to a real `wasm32-wasip2` COMPONENT, loaded, and driven
//! through the production `dispatch_block_mutate` seam with a `HostEvent::ToolResult` carrying
//! structured content. The demo's `tool_result` handler reads it through the SDK's
//! `ToolResultEvent::structured_content` and answers through `ToolResultPatch`-shaped JSON, so the
//! whole SDK path (macro lowering, `on_tool_result`, the mutate JSON, the host's `decode_patch`
//! and the fold) is crossed, not a hand-written component.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::fixture;

use cyrup_core::{CancelToken, Content};
use cyrup_ext::{DenyServices, ExtensionHost, HostEvent, Reduced};
use serde_json::{Value, json};
use std::sync::Arc;

fn tool_result(name: &str, structured: Option<Value>) -> HostEvent {
    HostEvent::ToolResult {
        call_id: "sc1".into(),
        name: name.into(),
        input: json!({}),
        content: vec![Content::text("2 files")],
        details: None,
        structured_content: structured,
        is_error: false,
        usage: None,
        terminate: cyrup_core::TerminateHint::Unspecified,
    }
}

fn settled(reduced: Reduced) -> (Vec<Content>, Option<Value>) {
    match reduced {
        Reduced::Pass(ev) => match *ev {
            HostEvent::ToolResult {
                content,
                structured_content,
                ..
            } => (content, structured_content),
            other => panic!("expected a ToolResult event back, got {other:?}"),
        },
        other => panic!("tool_result is not blocked by the demo guest, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_guest_reads_and_replaces_the_structured_content() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");
    let cancel = CancelToken::new();
    let received = json!({ "files": 2, "names": ["a", "b"] });

    // Replaced ALONG WITH the content: the guest doubled `files` in what it RECEIVED, and the
    // content it replaced does not drop it.
    let (content, structured) = settled(
        host.dispatcher()
            .dispatch_block_mutate(
                tool_result("structured_probe", Some(received.clone())),
                &cancel,
            )
            .await,
    );
    let notes = ext.guest().notifications();
    let seen = notes
        .iter()
        .find(|n| n.contains("tool_result structured_probe structured="))
        .unwrap_or_else(|| panic!("the guest's tool_result handler ran: {notes:?}"));
    assert!(
        seen.contains("\"files\":2") && seen.contains("\"names\":[\"a\",\"b\"]"),
        "the guest read the REAL structured content off the export: {seen}"
    );
    assert_eq!(content, vec![Content::text("rewritten by the guest")]);
    assert_eq!(
        structured,
        Some(json!({ "files": 4, "names": ["a", "b"] })),
        "the replacement the guest returned along with the content was kept"
    );

    // The content replaced ALONE: pi's drop rule discards the structured half (runner.ts:1194-1198).
    let (content, structured) = settled(
        host.dispatcher()
            .dispatch_block_mutate(tool_result("redacted_probe", Some(received)), &cancel)
            .await,
    );
    assert_eq!(content, vec![Content::text("rewritten by the guest")]);
    assert_eq!(
        structured, None,
        "a content-only patch drops the structured content"
    );

    // A tool with none: `option<string>` arrives as absent.
    let (_, structured) = settled(
        host.dispatcher()
            .dispatch_block_mutate(tool_result("structured_probe", None), &cancel)
            .await,
    );
    assert!(
        ext.guest()
            .notifications()
            .iter()
            .any(|n| n.contains("tool_result structured_probe structured=none")),
        "an absent structured-content-json reaches the guest as absent"
    );
    assert_eq!(structured, None);
}
