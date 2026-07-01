//! LIVE assembled-host dispatch of the previously-DEAD mutating seams (gap-08 #1-#5). Builds the
//! `cyrup-ext-sdk` bundled extension to a real `wasm32-wasip2` COMPONENT, loads it through the host,
//! and drives the production facade `emit_*` methods — the seams the assembled host actually calls —
//! asserting the guest's block / transform / inject / replace results flow back across the boundary.
//!
//! This is the discipline the audit demanded: NOT a hand-built event handed to `dispatch_*`, but the
//! real `ExtensionHost::emit_before_agent_start` / `emit_input` / `emit_message_end` /
//! `emit_before_provider_request` / `emit_user_bash` entry points reaching the live `.wasm` guest.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{CancelToken, Content, Message};
use cyrup_ext::{
    CompactionReduction, DenyServices, EventKind, Extension, ExtMode, ExtensionHost, HostConfig,
    InputEventSource, InputReduction, TreeReduction, UserBashReduction,
};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Build (or locate) the demo guest component.
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build_dir = std::env::temp_dir().join("cyrup-ext-fixture-target");
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&build_dir)
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");
    let wasm = build_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_guest_dispatches_the_mutating_seams_end_to_end() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    // init declared subscriptions for all five seams (the guest's `on_*` handlers).
    for kind in [
        EventKind::BeforeAgentStart,
        EventKind::Input,
        EventKind::MessageEnd,
        EventKind::BeforeProviderRequest,
        EventKind::UserBash,
    ] {
        assert!(ext.subscriptions().contains(kind), "guest subscribed to {kind:?}");
    }

    let cancel = CancelToken::new();

    // 1) before_agent_start (gap-08 #1): the guest injects a message AND replaces the system prompt.
    let reduction = host
        .emit_before_agent_start("go", json!([]), "BASE", json!({}), &cancel)
        .await
        .expect("before_agent_start reduction is Some (the guest modified it)");
    assert_eq!(
        reduction.system_prompt.as_deref(),
        Some("INJECTED:BASE"),
        "guest replaced the system prompt across the live boundary"
    );
    assert_eq!(reduction.injected.len(), 1, "guest injected exactly one message");
    match &reduction.injected[0] {
        Message::User { content, .. } => match content.first() {
            Some(Content::Text { text, .. }) => assert_eq!(text, "injected by demo"),
            other => panic!("unexpected injected content: {other:?}"),
        },
        other => panic!("unexpected injected message: {other:?}"),
    }
    // A non-matching prompt yields no modification (Pi returns undefined).
    assert!(
        host.emit_before_agent_start("idle", json!([]), "BASE", json!({}), &cancel).await.is_none(),
        "unmatched prompt => no before_agent_start reduction"
    );

    // 2) input (gap-08 #2): block / transform / continue, all via the live guest.
    match host.emit_input("secret", vec![], InputEventSource::Interactive, None, &cancel).await {
        InputReduction::Blocked { reason, by } => {
            assert_eq!(by.to_string(), "demo");
            assert!(reason.as_deref().unwrap_or("").contains("input blocked"), "got {reason:?}");
        }
        other => panic!("expected input block, got {other:?}"),
    }
    match host.emit_input("up:hello", vec![], InputEventSource::Interactive, None, &cancel).await {
        InputReduction::Transform { text, .. } => assert_eq!(text, "HELLO"),
        other => panic!("expected input transform, got {other:?}"),
    }
    assert!(
        matches!(
            host.emit_input("plain", vec![], InputEventSource::Interactive, None, &cancel).await,
            InputReduction::Continue
        ),
        "an unmatched input continues unchanged"
    );

    // 3) message_end (gap-08 #3): the guest returns a same-role replacement.
    let original = Message::User { content: vec![Content::text("redact me")], timestamp: 0 };
    let replaced = host
        .emit_message_end(original, &cancel)
        .await
        .expect("message_end returned a replacement");
    match &replaced {
        Message::User { content, .. } => match content.first() {
            Some(Content::Text { text, .. }) => assert_eq!(text, "[redacted]"),
            other => panic!("unexpected replacement content: {other:?}"),
        },
        other => panic!("unexpected replacement message: {other:?}"),
    }
    // An unmatched message is left unmodified (None).
    let untouched = Message::User { content: vec![Content::text("keep me")], timestamp: 0 };
    assert!(host.emit_message_end(untouched, &cancel).await.is_none(), "unmatched => no replacement");

    // 4) before_provider_request (gap-08 #4): the guest tags the payload (whole-payload replacement).
    let out = host.emit_before_provider_request(json!({ "model": "x" }), &cancel).await;
    assert_eq!(out["demoTag"], json!(true), "guest replaced the provider payload");
    assert_eq!(out["model"], json!("x"), "the guest preserved the original field");

    // 5) user_bash (gap-08 #5): block a destructive command, proceed otherwise.
    match host.emit_user_bash("rm -rf /", &cancel).await {
        UserBashReduction::Blocked { by, .. } => assert_eq!(by.to_string(), "demo"),
        other => panic!("expected user_bash block, got {other:?}"),
    }
    assert!(
        matches!(host.emit_user_bash("ls", &cancel).await, UserBashReduction::Continue),
        "a safe user_bash command proceeds"
    );

    // 6) session_before_compact (L4 gap #5): the guest READS the typed `CompactionPreparation` off
    // the event and returns a custom-summary override that flows back across the live wasm boundary
    // with the new WIT arity (preparation-json + branch-entries-json + reason + willRetry).
    assert!(
        ext.subscriptions().contains(EventKind::SessionBeforeCompact),
        "guest subscribed to SessionBeforeCompact"
    );
    match host
        .emit_session_before_compact(
            json!({ "firstKeptEntryId": "e7", "tokensBefore": 10 }),
            json!([]),
            None,
            "manual",
            false,
            &cancel,
        )
        .await
    {
        CompactionReduction::Override(v) => {
            // The guest derived the summary from the preparation it read — proof the typed payload
            // crossed the seam, not a fabricated constant.
            assert_eq!(
                v["summary"], "demo-summary[manual|firstKept=e7]",
                "the guest read the preparation and returned a derived summary override: {v}"
            );
        }
        other => panic!("expected a compaction override, got {other:?}"),
    }

    // 7) session_before_tree (L4 gap #5): the guest reads the typed `TreePreparation` and overrides
    // the branch-summary label.
    assert!(
        ext.subscriptions().contains(EventKind::SessionBeforeTree),
        "guest subscribed to SessionBeforeTree"
    );
    match host
        .emit_session_before_tree(json!({ "targetId": "t9", "userWantsSummary": true }), &cancel)
        .await
    {
        TreeReduction::Override(v) => {
            assert_eq!(
                v["label"], "demo-tree-label[t9]",
                "the guest read the tree preparation and returned a derived label override: {v}"
            );
        }
        other => panic!("expected a tree override, got {other:?}"),
    }
}
