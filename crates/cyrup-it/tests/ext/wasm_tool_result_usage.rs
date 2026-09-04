//! EXT-028: LIVE coverage for the RE-SIGNED `events.on-tool-result` export.
//!
//! `f777e44` widened that export with a trailing `usage-json: option<string>` (Pi
//! `ToolResultEventBase.usage`, extensions/types.ts:919-921) in both `world.wit` copies. Until now
//! nothing registered a `tool_result` handler in the SDK demo extension, so the widened argument had
//! never crossed a real wasm boundary — the re-signing was covered only by host-side unit tests that
//! never instantiate a component.
//!
//! This builds the `cyrup-ext-sdk` demo to a real `wasm32-wasip2` COMPONENT, loads it, and drives the
//! production `dispatch_block_mutate` seam with a `HostEvent::ToolResult` carrying a real
//! `cyrup_core::Usage`, asserting both directions of the widened export:
//!   * the guest OBSERVES the usage it was handed (and observes its absence as `none`), and
//!   * the guest PATCHES it back — Pi `ToolResultEventResult.usage` (types.ts:1085-1090) replaces
//!     the recorded usage wholesale — with a value DERIVED from what it received.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::fixture;

use cyrup_core::{CancelToken, Content, Usage};
use cyrup_ext::{DenyServices, EventKind, Extension, ExtensionHost, HostEvent, Reduced};
use serde_json::json;
use std::sync::Arc;

fn probe_usage() -> Usage {
    Usage {
        input: 5,
        output: 7,
        total_tokens: 12,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_guest_reads_and_patches_the_re_signed_tool_result_usage() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    assert!(
        ext.subscriptions().contains(EventKind::ToolResult),
        "the demo guest declares a tool_result subscription"
    );

    let cancel = CancelToken::new();

    // 1) A tool that DID report usage: the guest must see the real numbers on the wire.
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolResult {
                call_id: "tr1".into(),
                name: "usage_probe".into(),
                input: json!({ "q": "x" }),
                content: vec![Content::text("ok")],
                details: None,
                is_error: false,
                usage: Some(probe_usage()),
                terminate: cyrup_core::TerminateHint::Unspecified,
            },
            &cancel,
        )
        .await;

    let notes = ext.guest().notifications();
    let seen = notes
        .iter()
        .find(|n| n.contains("tool_result usage_probe usage="))
        .unwrap_or_else(|| panic!("the guest's tool_result handler ran: {notes:?}"));
    assert!(
        seen.contains("\"input\":5") && seen.contains("\"output\":7"),
        "the guest read the REAL usage off the re-signed export, not a default: {seen}"
    );

    // 2) ...and its patch, derived from that payload, replaced the event's usage (output doubled).
    match reduced {
        Reduced::Pass(ev) => match *ev {
            HostEvent::ToolResult { usage, .. } => {
                let usage = usage.expect("the guest's usage patch is applied, not dropped");
                assert_eq!(
                    usage.output, 14,
                    "the guest doubled the `output` it received"
                );
                assert_eq!(
                    usage.input, 5,
                    "the rest of the echoed payload survived the round trip"
                );
            }
            other => panic!("expected a ToolResult event back, got {other:?}"),
        },
        other => panic!("tool_result is not blocked by the demo guest, got {other:?}"),
    }

    // 3) An ordinary tool reports NO usage: `option<string>` must arrive as absent, not as an empty
    //    string or a zeroed `Usage` (Pi's `usage` is `undefined` for every ordinary tool).
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolResult {
                call_id: "tr2".into(),
                name: "read".into(),
                input: json!({ "path": "x" }),
                content: vec![Content::text("file body")],
                details: None,
                is_error: false,
                usage: None,
                terminate: cyrup_core::TerminateHint::Unspecified,
            },
            &cancel,
        )
        .await;

    let notes = ext.guest().notifications();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("tool_result read usage=none")),
        "an absent usage-json reaches the guest as absent: {notes:?}"
    );
    match reduced {
        Reduced::Pass(ev) => match *ev {
            HostEvent::ToolResult { usage, .. } => {
                assert!(usage.is_none(), "the guest left an unreported usage absent");
            }
            other => panic!("expected a ToolResult event back, got {other:?}"),
        },
        other => panic!("tool_result is not blocked by the demo guest, got {other:?}"),
    }
}
