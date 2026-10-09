//! PROV-133 — the native mid-conversation tool-change shape: a fixed request-level tool list plus
//! the deferred placeholder, `tool_addition` / `tool_removal` blocks carrying definitions by value,
//! and the `inline-tools-2026-09-15` beta.
//!
//! The row's Verify line asks for *"a run in which a tool is added, removed and redefined
//! mid-run: assert the blocks emitted, the unchanged request-level `tools`, and the beta header"*,
//! which is what [`a_mid_run_add_remove_and_redefine_emits_the_native_blocks`] does.
//!
//! These tests are only reachable because of AGENT-039: the gate's third term is the INITIAL system
//! message's `toolsAdded`, and while the request path stripped every declaration it was always
//! empty, so this whole path was dead code.

use super::*;
use crate::api::anthropic_messages::params::build_params;
use crate::utils::provider_plumbing::EnvSource;
use cyrup_core::SystemMessage;

/// A model that declares both mid-conversation capabilities by its id, through the runtime defaults
/// PROV-083a ported (`claude-opus-5-5` matches upstream's anchored regex).
fn capable_model() -> Model {
    Model {
        id: "claude-opus-5-5".into(),
        provider: "anthropic".into(),
        ..model()
    }
}

fn system(tools_added: &[&str], tools_removed: &[&str], text: &str) -> Message {
    Message::System(SystemMessage {
        content: if text.is_empty() {
            Vec::new()
        } else {
            vec![Content::text(text)]
        },
        tools_added: tools_added.iter().map(|n| tool_def(n)).collect(),
        tools_removed: tools_removed
            .iter()
            .map(|n| cyrup_core::ToolReference::new(*n))
            .collect(),
        timestamp: 0,
        ..SystemMessage::default()
    })
}

fn params_for(model: &Model, messages: Vec<Message>, tools: Vec<ToolDef>) -> Value {
    let ctx = Context {
        system_prompt: None,
        messages,
        tools,
    };
    build_params(
        model,
        &ctx,
        &StreamOptions::default(),
        EnvSource::default(),
        false,
    )
    .expect("params build")
}

fn tool_names(params: &Value) -> Vec<String> {
    params["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The blocks of every `role: "system"` message on the wire, in order.
fn system_blocks(params: &Value) -> Vec<Value> {
    params["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m["role"] == "system")
                .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// The row's Verify line, end to end: `search` is active from the start, `write` is added mid-run,
/// `search` is removed, and `read` is added and then REDEFINED under the same name.
#[test]
fn a_mid_run_add_remove_and_redefine_emits_the_native_blocks() {
    let params = params_for(
        &capable_model(),
        vec![
            // The initial system message: the prompt and the starting tools.
            system(&["search", "read"], &[], "be brief"),
            Message::User {
                content: vec![Content::text("one")],
                timestamp: 0,
            },
            tc_assistant(&[]),
            // Mid-run: add `write`, remove `search`, and REDEFINE `read`.
            system(&["write", "read"], &["search", "read"], "tools changed"),
            Message::User {
                content: vec![Content::text("two")],
                timestamp: 0,
            },
            tc_assistant(&[]),
        ],
        vec![tool_def("write"), tool_def("read")],
    );

    // 1. The request-level list is the INITIAL tools plus the placeholder — it does NOT follow the
    //    tool set, which is the whole point: the cached prefix survives every change.
    assert_eq!(
        tool_names(&params),
        ["search", "read", "__cyrup_deferred_placeholder__"],
        "the fixed list is the initial tools plus the placeholder"
    );
    let placeholder = &params["tools"][2];
    assert_eq!(
        placeholder["defer_loading"], true,
        "the placeholder is deferred so it is never active"
    );

    // 2. The later system message becomes text, then removals, then additions.
    let blocks = system_blocks(&params);
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| b["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        ["text", "tool_removal", "tool_addition", "tool_addition"],
        "blocks: {blocks:#?}"
    );

    // 3. `read` was removed AND re-added, so no removal is emitted for it — a definition under the
    //    same name replaces the old one. Only `search` is withdrawn.
    assert_eq!(blocks[1]["tool"]["type"], "tool_reference");
    assert_eq!(
        blocks[1]["tool"]["name"], "search",
        "a redefined name must NOT also be removed"
    );

    // 4. Each addition carries the definition BY VALUE.
    let added: Vec<&str> = blocks[2..]
        .iter()
        .map(|b| b["tool"]["definition"]["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(added, ["write", "read"]);
    assert_eq!(blocks[2]["tool"]["type"], "tool_definition");
    assert!(
        blocks[2]["tool"]["definition"]["input_schema"].is_object(),
        "a definition by value carries its schema: {:#?}",
        blocks[2]
    );

    // 5. The beta that the shape requires.
    let headers = build_headers(
        &capable_model(),
        &Context {
            system_prompt: None,
            messages: vec![system(&["search"], &[], "be brief")],
            tools: vec![tool_def("search")],
        },
        &auth_with(Some("sk-ant-api03-xxx")),
        &StreamOptions::default(),
        false,
    );
    let betas = headers
        .get("anthropic-beta")
        .and_then(|v| v.as_deref())
        .unwrap_or_default();
    assert!(
        betas.contains("inline-tools-2026-09-15"),
        "the native shape must send the inline-tools beta, got: {betas}"
    );
}

/// The initial system message is the PROMPT, not a turn: it must not also be emitted as a block
/// message, or every tool would be declared twice on the first request.
#[test]
fn the_initial_system_message_is_not_emitted_as_blocks() {
    let params = params_for(
        &capable_model(),
        vec![
            system(&["search"], &[], "be brief"),
            Message::User {
                content: vec![Content::text("one")],
                timestamp: 0,
            },
        ],
        vec![tool_def("search")],
    );
    assert!(
        system_blocks(&params).is_empty(),
        "the initial system message produced blocks: {:#?}",
        params["messages"]
    );
    assert_eq!(
        tool_names(&params),
        ["search", "__cyrup_deferred_placeholder__"]
    );
}

/// Without an initial active tool the gate is false and nothing changes — pi's third term, which
/// exists because Anthropic rejects a tool list where every tool is deferred.
#[test]
fn no_initial_tool_means_no_native_shape() {
    let params = params_for(
        &capable_model(),
        vec![
            system(&[], &[], "be brief"),
            Message::User {
                content: vec![Content::text("one")],
                timestamp: 0,
            },
            tc_assistant(&[]),
            system(&["write"], &[], ""),
        ],
        vec![tool_def("write")],
    );
    assert!(
        !tool_names(&params).contains(&"__cyrup_deferred_placeholder__".to_string()),
        "no placeholder without an anchoring initial tool: {:?}",
        tool_names(&params)
    );
    assert!(
        system_blocks(&params).is_empty(),
        "no blocks without the native shape"
    );
}

/// A model that does not declare the capabilities keeps the pre-existing behaviour exactly: the
/// request-level list follows the tool set and later system messages are dropped.
#[test]
fn a_model_without_the_capabilities_keeps_the_old_shape() {
    let incapable = Model {
        id: "claude-3-5-sonnet-20241022".into(),
        provider: "anthropic".into(),
        ..model()
    };
    let params = params_for(
        &incapable,
        vec![
            system(&["search"], &[], "be brief"),
            Message::User {
                content: vec![Content::text("one")],
                timestamp: 0,
            },
            tc_assistant(&[]),
            system(&["write"], &["search"], "changed"),
        ],
        vec![tool_def("write")],
    );
    assert_eq!(
        tool_names(&params),
        ["write"],
        "the list follows ctx.tools when the native shape is off"
    );
    assert!(
        system_blocks(&params).is_empty(),
        "a model that cannot express the blocks must not be sent them"
    );

    let headers = build_headers(
        &incapable,
        &Context {
            system_prompt: None,
            messages: vec![system(&["search"], &[], "be brief")],
            tools: vec![tool_def("search")],
        },
        &auth_with(Some("sk-ant-api03-xxx")),
        &StreamOptions::default(),
        false,
    );
    let betas = headers
        .get("anthropic-beta")
        .and_then(|v| v.as_deref())
        .unwrap_or_default();
    assert!(
        !betas.contains("inline-tools"),
        "the beta must not ship without the shape: {betas}"
    );
}
