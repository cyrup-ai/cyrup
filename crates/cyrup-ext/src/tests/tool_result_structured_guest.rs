//! `tool_result` and the tool's structured content, at the WASM tier: a real component
//! ([`super::wat_guest`]) loaded through `ExtensionHost::load_wasm_with_caps`, driven through the
//! host's own dispatcher and hooks.
//!
//! Two crossings are under test. INBOUND, the world's `events.on-tool-result` gained a trailing
//! `structured-content-json: option<string>` (pi `ToolResultEventBase.structuredContent`,
//! `extensions/types.ts:1238` @v1.0.1), `none` when the tool returned none. OUTBOUND, a guest's
//! mutate JSON may carry `structuredContent` (`ToolResultEventResult.structuredContent`, `:1445`),
//! and the host folds it with pi's drop rule: replacing `content` without it drops it
//! (`runner.ts:1194-1198`). The tier-independent cases are in
//! [`super::tool_result_structured_content`].
#![cfg(feature = "wasm-host")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_agent::{AfterOutcome, AfterOverride, AfterToolCall, AgentContextView};
use cyrup_core::{CancelToken, Content, TerminateHint, ToolCallId};
use serde_json::{Value, json};

use crate::manifest::Capabilities;
use crate::tests::wat_guest::{
    Lowered, REGISTRATION_FLAG_AND_UNSUBSCRIBE, UI_STATUS_AND_SELECT, WatGuest, wat_str,
};
use crate::{EventKind, ExtMode, ExtensionHost, HostConfig, HostEvent};

const SUBSCRIBE: Lowered = Lowered {
    core_name: "subscribe",
    component_func: "$subscribe",
    core_sig: "(param i32 i32)",
    needs_realloc: false,
};
const SET_STATUS: Lowered = Lowered {
    core_name: "set_status",
    component_func: "$set-status",
    core_sig: "(param i32 i32 i32 i32 i32)",
    needs_realloc: false,
};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

fn le32(v: u32) -> String {
    v.to_le_bytes()
        .iter()
        .map(|b| format!("\\{b:02x}"))
        .collect()
}

fn text(s: &str) -> Vec<Content> {
    vec![Content::text(s)]
}

fn stats() -> Value {
    json!({ "files": 2, "names": ["a", "b"] })
}

/// The `init` body subscribing to `tool_result` (kind byte at 16384).
const INIT: &str = "    (func (export \"init\") (result i32) \
                    (call $subscribe (i32.const 16384) (i32.const 1)) i32.const 16)";

/// A guest whose `on-tool-result` answers `reply` as `hook-outcome::mutate`, or `noop` when `None`.
/// The export's 21 flat parameters arrive as ONE pointer to a memory record, so the body ignores
/// them.
fn replying_guest(reply: Option<&Value>) -> Vec<u8> {
    let mut data = vec![(16384, format!("\\{:02x}", EventKind::ToolResult as u8))];
    let result = match reply {
        None => "i32.const 16".to_string(),
        Some(reply) => {
            let payload = reply.to_string();
            // hook-outcome::mutate (case 2), the payload string at +4.
            data.push((
                16400,
                format!(
                    "\\02\\00\\00\\00{}{}",
                    le32(16416),
                    le32(payload.len() as u32)
                ),
            ));
            data.push((16416, wat_str(&payload)));
            "i32.const 16400".to_string()
        }
    };
    WatGuest {
        component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
        lowered: vec![SUBSCRIBE],
        overrides: vec![
            ("init", INIT.to_string()),
            (
                "on-tool-result",
                format!("    (func (export \"on-tool-result\") (param i32) (result i32) {result})"),
            ),
        ],
        data,
    }
    .build()
}

/// A guest that reports the `structured-content-json` it was handed as a status segment: the
/// trailing `option<string>` sits at byte 72 of the parameter record (call-id, name, input and
/// content at 0..32, is-error at 32, then four 12-byte options: details 36, usage 48, parent 60,
/// structured content 72).
fn echoing_guest() -> Vec<u8> {
    WatGuest {
        component: format!("{REGISTRATION_FLAG_AND_UNSUBSCRIBE}{UI_STATUS_AND_SELECT}"),
        lowered: vec![SUBSCRIBE, SET_STATUS],
        overrides: vec![
            ("init", INIT.to_string()),
            (
                "on-tool-result",
                "    (func (export \"on-tool-result\") (param i32) (result i32) \
                 (call $set_status (i32.const 16400) (i32.const 10) \
                 (i32.load offset=72 (local.get 0)) \
                 (i32.load offset=76 (local.get 0)) \
                 (i32.load offset=80 (local.get 0))) \
                 i32.const 16)"
                    .to_string(),
            ),
        ],
        data: vec![
            (16384, format!("\\{:02x}", EventKind::ToolResult as u8)),
            (16400, "structured".to_string()),
        ],
    }
    .build()
}

fn tool_result(structured: Option<Value>) -> HostEvent {
    HostEvent::ToolResult {
        call_id: ToolCallId::from("call-1"),
        name: "stats".into(),
        input: json!({}),
        content: text("2 files"),
        details: None,
        structured_content: structured,
        is_error: false,
        usage: None,
        terminate: TerminateHint::Unspecified,
    }
}

async fn host_with(component: &[u8]) -> (ExtensionHost, Arc<crate::host::LiveExtension>) {
    let host = ExtensionHost::with_wasm(cfg()).unwrap();
    let live = host
        .load_wasm_with_caps(
            "structured-guest".into(),
            component,
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
    (host, live)
}

/// The guest's reply, folded the way the session does: one finished call of a tool that returned
/// `2 files` and `{files: 2, names: [a, b]}`.
async fn after(host: &ExtensionHost) -> AfterOutcome {
    let id: ToolCallId = "call-1".into();
    let args = json!({});
    let content = text("2 files");
    let structured = stats();
    let call = cyrup_core::ToolCall {
        id: id.clone(),
        name: "stats".to_string(),
        arguments: serde_json::Map::new().into(),
        thought_signature: None,
        namespace: None,
    };
    let message = cyrup_core::AssistantMessage {
        content: vec![Content::ToolCall(call.clone())],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    let ctx = AfterToolCall {
        tool_name: "stats",
        tool_call_id: &id,
        args: &args,
        content: &content,
        details: None,
        structured_content: Some(&structured),
        usage: None,
        is_error: false,
        terminate: TerminateHint::Unspecified,
        assistant_message: &message,
        tool_call: &call,
        context: AgentContextView {
            system_prompt: "",
            messages: &[],
            tools: &[],
        },
    };
    host.hooks().after_tool_call(ctx, CancelToken::new()).await
}

fn overridden(outcome: AfterOutcome) -> Box<AfterOverride> {
    match outcome {
        AfterOutcome::Override(over) => over,
        AfterOutcome::Keep => panic!("expected an override, the guest changed nothing"),
        AfterOutcome::Failed(e) => panic!("expected an override, the hook failed: {e}"),
    }
}

/// INBOUND: the structured content crosses as `structured-content-json`, and its absence as `none`.
#[tokio::test]
async fn a_guest_receives_the_structured_content_and_none_for_a_tool_without_one() {
    let (host, live) = host_with(&echoing_guest()).await;
    let cancel = CancelToken::new();

    host.dispatcher()
        .dispatch_block_mutate(tool_result(Some(stats())), &cancel)
        .await;
    host.dispatcher()
        .dispatch_block_mutate(tool_result(None), &cancel)
        .await;
    host.dispatcher()
        .dispatch_block_mutate(tool_result(Some(Value::Null)), &cancel)
        .await;

    assert_eq!(
        live.guest().statuses(),
        vec![
            ("structured".to_string(), Some(stats().to_string())),
            ("structured".to_string(), None),
            ("structured".to_string(), Some("null".to_string())),
        ],
        "the JSON the tool returned, `none` for no structured content, and a JSON null as a value"
    );
}

/// OUTBOUND, upstream `keeps structured content that tool_result handlers replace along with the
/// content`: a guest's mutate carrying both lands both.
#[tokio::test]
async fn a_guest_replaces_the_structured_content_along_with_the_content() {
    let zero = json!({ "files": 0, "names": [] });
    let (host, _live) = host_with(&replying_guest(Some(&json!({
        "content": text("0 files"),
        "structuredContent": zero,
    }))))
    .await;

    let over = overridden(after(&host).await);
    assert_eq!(over.content, Some(text("0 files")));
    assert_eq!(over.structured_content, Some(zero));
}

/// OUTBOUND: a guest's mutate with `content` alone drops the structured content.
#[tokio::test]
async fn a_guest_replacing_content_alone_drops_the_structured_content() {
    let (host, _live) = host_with(&replying_guest(Some(
        &json!({ "content": text("redacted") }),
    )))
    .await;

    let over = overridden(after(&host).await);
    assert_eq!(over.content, Some(text("redacted")));
    assert_eq!(over.structured_content, None);
}

/// OUTBOUND: `structuredContent` alone replaces it and leaves the content.
#[tokio::test]
async fn a_guest_replacing_only_the_structured_content_leaves_the_content() {
    let replacement = json!({ "files": 9 });
    let (host, _live) = host_with(&replying_guest(Some(
        &json!({ "structuredContent": replacement }),
    )))
    .await;

    let over = overridden(after(&host).await);
    assert_eq!(over.content, None);
    assert_eq!(over.structured_content, Some(replacement));
}

/// A guest that answers noop changes nothing.
#[tokio::test]
async fn a_guest_that_answers_noop_keeps_the_result() {
    let (host, _live) = host_with(&replying_guest(None)).await;
    assert!(matches!(after(&host).await, AfterOutcome::Keep));
}
