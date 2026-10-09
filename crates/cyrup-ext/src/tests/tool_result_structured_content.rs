//! `tool_result` handlers and the tool's structured content, at the native tier.
//!
//! pi's `ToolResultEventResult` has five patchable fields (`core/extensions/types.ts:1442-1448`
//! @v1.0.1): `content`, `details`, `structuredContent`, `isError` and `usage`. Its doc says
//! "Omitted fields stay as they are, except that replacing `content` without returning
//! `structuredContent` drops the structured content, because it may no longer match. Return it
//! along with `content` to keep it", and `emitToolResult` (`runner.ts:1183-1241`) is what applies
//! it: handlers chain in load order, each seeing what the ones before it left.
//!
//! These drive the production `ExtHooks::after_tool_call` over real [`NativeExtension`]s and read
//! the [`AfterOverride`] the agent folds. In that override a `content` without a
//! `structured_content` IS the drop (pi's `afterResult.content ? undefined : result.structuredContent`,
//! `agent-loop.ts:879-880`), so each case asserts the pair, not one field.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_agent::{AfterOutcome, AfterOverride, AfterToolCall, AgentContextView};
use cyrup_core::{CancelToken, Content, ExtensionId, TerminateHint, ToolCallId};
use serde_json::{Value, json};

use crate::{
    EventKind, EventPatch, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent,
    InitApi, NativeExtension,
};

/// What a handler sees of the event and answers with.
type Handler = Arc<dyn Fn(&HostEvent) -> HookOutcome + Send + Sync>;

/// A `tool_result` handler, recording the structured content it was handed.
struct Handlers {
    id: &'static str,
    handler: Handler,
    seen: Arc<Mutex<Vec<Option<Value>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for Handlers {
    fn id(&self) -> ExtensionId {
        self.id.into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolResult {
            structured_content, ..
        } = ev
        {
            self.seen.lock().unwrap().push(structured_content.clone());
        }
        (self.handler)(ev)
    }
}

/// A handler's patch, with every field unset.
fn patch() -> EventPatch {
    EventPatch::ToolResult {
        content: None,
        details: None,
        structured_content: None,
        is_error: None,
        usage: None,
        terminate: None,
    }
}

fn with(f: impl FnOnce(&mut Fields)) -> EventPatch {
    let mut fields = Fields::default();
    f(&mut fields);
    EventPatch::ToolResult {
        content: fields.content,
        details: fields.details,
        structured_content: fields.structured_content.map(Box::new),
        is_error: None,
        usage: None,
        terminate: None,
    }
}

#[derive(Default)]
struct Fields {
    content: Option<Vec<Content>>,
    details: Option<Value>,
    structured_content: Option<Value>,
}

fn text(s: &str) -> Vec<Content> {
    vec![Content::text(s)]
}

fn stats() -> Value {
    json!({ "files": 2, "names": ["a", "b"] })
}

/// Load the handlers in order and run one finished call of a tool that returned `content` and
/// `structured`, the way the agent loop does. Returns the outcome and what each handler observed.
async fn run(
    handlers: Vec<Handler>,
    content: &[Content],
    structured: Option<&Value>,
) -> (AfterOutcome, Vec<Arc<Mutex<Vec<Option<Value>>>>>) {
    let host = ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    });
    let ids = ["h1", "h2", "h3"];
    let mut seen = Vec::new();
    for (handler, id) in handlers.into_iter().zip(ids) {
        let log = Arc::new(Mutex::new(Vec::new()));
        seen.push(Arc::clone(&log));
        host.load_native(Arc::new(Handlers {
            id,
            handler,
            seen: log,
        }))
        .await
        .unwrap();
    }
    let id: ToolCallId = "call".into();
    let args = json!({});
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
        duration_ms: None,
    };
    let ctx = AfterToolCall {
        tool_name: "stats",
        tool_call_id: &id,
        args: &args,
        content,
        details: None,
        structured_content: structured,
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
    let outcome = host.hooks().after_tool_call(ctx, CancelToken::new()).await;
    (outcome, seen)
}

fn handler(f: impl Fn(&HostEvent) -> EventPatch + Send + Sync + 'static) -> Handler {
    Arc::new(move |ev| HookOutcome::Mutate(f(ev)))
}

fn overridden(outcome: AfterOutcome) -> Box<AfterOverride> {
    match outcome {
        AfterOutcome::Override(over) => over,
        AfterOutcome::Keep => panic!("expected an override, the handlers changed nothing"),
        AfterOutcome::Failed(e) => panic!("expected an override, the hook failed: {e}"),
    }
}

/// Upstream `keeps structured content that tool_result handlers replace along with the content`
/// (`agent-session-codemode.test.ts`): the first handler replaces both, a later one that only
/// touches `details` keeps what it set.
#[tokio::test]
async fn keeps_structured_content_that_tool_result_handlers_replace_along_with_the_content() {
    let zero = json!({ "files": 0, "names": [] });
    let replaced = zero.clone();
    let (outcome, seen) = run(
        vec![
            handler(move |_| {
                with(|f| {
                    f.content = Some(text("0 files"));
                    f.structured_content = Some(replaced.clone());
                })
            }),
            handler(|_| with(|f| f.details = Some(json!({ "audited": true })))),
        ],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.content, Some(text("0 files")));
    assert_eq!(over.structured_content, Some(zero.clone()));
    assert_eq!(over.details, Some(json!({ "audited": true })));
    // The second handler saw the replacement, not the tool's own value.
    assert_eq!(*seen[0].lock().unwrap(), vec![Some(stats())]);
    assert_eq!(*seen[1].lock().unwrap(), vec![Some(zero)]);
}

/// Upstream's content-only case (`routes nested calls through extension hooks`: "Replacing
/// content without replacing structured content drops the structured result"). The override
/// carries the new `content` and no `structured_content`, which the agent folds to a drop.
#[tokio::test]
async fn replacing_content_without_structured_content_drops_it() {
    let (outcome, _) = run(
        vec![handler(|_| with(|f| f.content = Some(text("redacted"))))],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.content, Some(text("redacted")));
    assert_eq!(over.structured_content, None);
}

/// pi drops on the REPLACEMENT, not on a difference: `handlerResult.content !== undefined`
/// (`runner.ts:1193`). A handler that hands back content equal to the tool's still drops it.
#[tokio::test]
async fn a_content_replacement_equal_to_the_original_still_drops_the_structured_content() {
    let (outcome, _) = run(
        vec![handler(|_| with(|f| f.content = Some(text("2 files"))))],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(
        over.content,
        Some(text("2 files")),
        "the replacement is sent even though it equals the original, so the agent drops"
    );
    assert_eq!(over.structured_content, None);
}

/// `structuredContent` on its own replaces it and leaves the content alone.
#[tokio::test]
async fn structured_content_alone_replaces_it_and_leaves_the_content() {
    let replacement = json!({ "files": 9 });
    let sent = replacement.clone();
    let (outcome, _) = run(
        vec![handler(move |_| {
            with(|f| f.structured_content = Some(sent.clone()))
        })],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.content, None);
    assert_eq!(over.structured_content, Some(replacement));
}

/// "Return it along with `content` to keep it": a handler that rewrites the text and hands back the
/// structured content it was given keeps it. The override must say so, because the agent drops a
/// replaced `content` whose `structured_content` is absent, and an unchanged value is not a diff.
#[tokio::test]
async fn returning_the_unchanged_structured_content_with_new_content_keeps_it() {
    let (outcome, _) = run(
        vec![handler(|ev| {
            let HostEvent::ToolResult {
                structured_content, ..
            } = ev
            else {
                panic!("a tool_result event");
            };
            with(|f| {
                f.content = Some(text("two files"));
                f.structured_content = structured_content.clone();
            })
        })],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.content, Some(text("two files")));
    assert_eq!(over.structured_content, Some(stats()));
}

/// A patch that touches neither field keeps both: `details` alone is not a content replacement.
#[tokio::test]
async fn a_details_only_patch_keeps_content_and_structured_content() {
    let (outcome, _) = run(
        vec![handler(|_| with(|f| f.details = Some(json!({ "n": 1 }))))],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.details, Some(json!({ "n": 1 })));
    assert_eq!(over.content, None);
    assert_eq!(over.structured_content, None);
}

/// Handlers chain: a later content-only handler drops what an earlier one set, and a later
/// structured-content handler puts one back (`runner.ts:1190-1207`).
#[tokio::test]
async fn handlers_chain_the_drop_and_the_replacement_in_load_order() {
    let first = json!({ "from": "first" });
    let sent = first.clone();
    let (dropped, seen) = run(
        vec![
            handler(move |_| {
                with(|f| {
                    f.content = Some(text("one"));
                    f.structured_content = Some(sent.clone());
                })
            }),
            handler(|_| with(|f| f.content = Some(text("two")))),
        ],
        &text("2 files"),
        Some(&stats()),
    )
    .await;
    let over = overridden(dropped);
    assert_eq!(over.content, Some(text("two")));
    assert_eq!(
        over.structured_content, None,
        "the second handler replaced the content alone"
    );
    assert_eq!(
        *seen[1].lock().unwrap(),
        vec![Some(first)],
        "the second handler saw the first one's structured content"
    );

    let late = json!({ "from": "third" });
    let sent = late.clone();
    let (restored, seen) = run(
        vec![
            handler(|_| with(|f| f.content = Some(text("one")))),
            handler(move |_| with(|f| f.structured_content = Some(sent.clone()))),
        ],
        &text("2 files"),
        Some(&stats()),
    )
    .await;
    let over = overridden(restored);
    assert_eq!(over.content, Some(text("one")));
    assert_eq!(over.structured_content, Some(late));
    assert_eq!(
        *seen[1].lock().unwrap(),
        vec![None],
        "the first handler's content replacement had dropped it before the second saw it"
    );
}

/// A tool that returned none has nothing to drop, and a handler can attach one.
#[tokio::test]
async fn a_tool_without_structured_content_can_be_given_one() {
    let (plain, _) = run(
        vec![handler(|_| with(|f| f.content = Some(text("redacted"))))],
        &text("ok"),
        None,
    )
    .await;
    let over = overridden(plain);
    assert_eq!(over.content, Some(text("redacted")));
    assert_eq!(over.structured_content, None);

    let attach = json!({ "attached": true });
    let sent = attach.clone();
    let (attached, seen) = run(
        vec![handler(move |_| {
            with(|f| f.structured_content = Some(sent.clone()))
        })],
        &text("ok"),
        None,
    )
    .await;
    let over = overridden(attached);
    assert_eq!(over.content, None);
    assert_eq!(over.structured_content, Some(attach));
    assert_eq!(
        *seen[0].lock().unwrap(),
        vec![None],
        "an absent structured content reaches the handler as absent"
    );
}

/// A handler that only observes changes nothing, with or without structured content.
#[tokio::test]
async fn observing_the_structured_content_changes_nothing() {
    let (outcome, seen) = run(
        vec![Arc::new(|_: &HostEvent| HookOutcome::Noop) as Handler],
        &text("2 files"),
        Some(&stats()),
    )
    .await;
    assert!(matches!(outcome, AfterOutcome::Keep));
    assert_eq!(*seen[0].lock().unwrap(), vec![Some(stats())]);
}

/// pi reads `null` like the agent loop does (`??` is nullish): a handler returning
/// `structuredContent: null` leaves the result with none, so a result that had one loses it. The
/// override says it as the drop (content present, structured content absent), with no `null`.
#[tokio::test]
async fn a_null_structured_content_leaves_the_result_without_one() {
    let (outcome, _) = run(
        vec![handler(|_| {
            with(|f| f.structured_content = Some(Value::Null))
        })],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(
        over.content,
        Some(text("2 files")),
        "the unchanged content is sent so the agent drops"
    );
    assert_eq!(over.structured_content, None);
}

/// The other four fields of the patch keep working beside the new one (`details`, `isError`,
/// `usage` are replace-not-merge; an omitted key keeps).
#[tokio::test]
async fn the_other_patch_fields_still_apply_beside_structured_content() {
    let (outcome, _) = run(
        vec![handler(|_| {
            let mut p = patch();
            if let EventPatch::ToolResult {
                is_error, usage, ..
            } = &mut p
            {
                *is_error = Some(true);
                *usage = Some(cyrup_core::Usage {
                    input: 3,
                    ..cyrup_core::Usage::default()
                });
            }
            p
        })],
        &text("2 files"),
        Some(&stats()),
    )
    .await;

    let over = overridden(outcome);
    assert_eq!(over.is_error, Some(true));
    assert_eq!(over.usage.map(|u| u.input), Some(3));
    assert_eq!(over.content, None);
    assert_eq!(over.structured_content, None);
}
