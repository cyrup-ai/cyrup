//! PROV-091 — Anthropic managed mid-conversation effort, a case-for-case translation of pi
//! `packages/ai/test/anthropic-mid-conversation-effort.test.ts` @v0.87.1.
//!
//! Upstream a managed-effort model does NOT carry one request-level `output_config.effort`. Instead
//! the effort of every recorded turn is replayed as its own
//! `{role:"system",content:[],output_config:{effort}}` message immediately before that turn, and one
//! final marker carries the effort THIS request runs at. That makes the request prefix a byte-exact
//! function of the transcript, so Anthropic's cache prefix survives a mid-conversation effort change
//! — and `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` lets the server drop a
//! stale thinking block instead of answering 400 forever.
//!
//! TWO upstream cases are deliberately NOT translated, rather than faked:
//!
//! * `generates exact model and transport gates` asserts `getModel("anthropic","claude-fable-5-1")`
//!   and `getModel("openrouter","anthropic/claude-fable-5.1")` carry `compat.supportsMidConvoEffort`
//!   and `thinkingLevelMap.off === null`. That is pure GENERATED catalog data
//!   (`generate-models.ts:592-604`, `:811-818`, `:1199-1207`); cyrup's catalog is frozen at
//!   `b0c2a90e` (PROV-071) and contains no `claude-fable-5-1` / `claude-opus-5` row at all, so there
//!   is nothing local to assert. The flag therefore reaches cyrup today only through a catalog
//!   overlay or a user `models.json` — which is exactly what [`managed_model`] below is.
//! * `packages/ai/test/anthropic-thinking-binding-e2e.test.ts` is a live-API test behind
//!   `skipIf(!ANTHROPIC_API_KEY)`. It is upstream's proof of the CONSEQUENCE: with
//!   `prefix_mismatch_behavior: "error"` a deleted `providerThinkingLevel` returns
//!   `stopReason: "error"` and an `errorMessage` containing "Invalid `signature`". Nothing offline
//!   can stand in for that, and a local test pretending to reach Anthropic would certify nothing.

use super::*;
use crate::api::anthropic_messages::messages::is_anthropic_effort;
use crate::api::anthropic_messages::params::managed_active_effort;

/// Pi's `managedModel()` fixture (`anthropic-mid-conversation-effort.test.ts:23-36`).
///
/// One addition, and only one: an `xhigh` entry in `thinking_level_map`. Pi's `capture()` hands
/// `stream()` a RAW `effort`, bypassing `mapThinkingLevelToEffort` entirely; cyrup has a single entry
/// point, so the only way to ask for the native `xhigh` rung is the `thinkingLevelMap` entry that is
/// itself how pi's generator grants a model native `xhigh` (see `params.rs` on the `High | Xhigh |
/// Max => "high"` fallback). Every other key is upstream's, `off: null` included — which the
/// generator pins on every managed row (`generate-models.ts:816`).
fn managed_model() -> Model {
    Model {
        id: "claude-fable-5-1".into(),
        name: "Claude Fable 5.1".into(),
        thinking_level_map: Some(
            [
                ("off", None),
                ("minimal", Some("low")),
                ("low", Some("low")),
                ("medium", Some("medium")),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
                ("max", Some("max")),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect(),
        ),
        compat: Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            supports_mid_convo_effort: Some(true),
            ..Default::default()
        }),
        ..model()
    }
}

/// The same model with the managed flag removed — pi's `model.compat = { forceAdaptiveThinking: true }`
/// in `leaves unsupported models on top-level effort`. This is the shape EVERY model cyrup ships
/// today has, so it is the non-regression control.
fn unmanaged_adaptive_model() -> Model {
    Model {
        compat: Some(ModelCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        }),
        ..managed_model()
    }
}

fn opts(level: ModelThinkingLevel) -> StreamOptions {
    StreamOptions {
        reasoning: level,
        cache_retention: Some(CacheRetention::None),
        ..Default::default()
    }
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User {
        content: vec![Content::text(text)],
        timestamp,
    }
}

/// Pi's `assistant(model, level)` (`:38-60`): a thinking block with a signature plus a text block,
/// so `build_assistant` yields a non-empty block list and the turn is actually recorded.
fn assistant(model: &Model, provider: &str, level: Option<&str>) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![
            Content::Thinking {
                thinking: "reasoning".into(),
                thinking_signature: Some("signature".into()),
                redacted: false,
            },
            Content::text("answer"),
        ],
        provider: ProviderId::from(provider),
        model: model.id.as_str().to_string(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: level.map(str::to_string),
        thinking_level: None,
        end_turn: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 1,
    })
}

fn ctx(messages: Vec<Message>) -> Context {
    Context {
        system_prompt: None,
        messages,
        tools: Vec::new(),
    }
}

fn wire_messages(body: &Value) -> Vec<Value> {
    body["messages"].as_array().expect("messages").clone()
}

/// Pi's `effortMessages(payload)` (`:83-85`).
fn effort_markers(body: &Value) -> Vec<Value> {
    wire_messages(body)
        .into_iter()
        .filter(|m| m["role"] == "system")
        .collect()
}

/// A minimal `message_start`/`message_delta`/`message_stop` transcript, decoded with the SAME
/// `managed_active_effort` the request body used — pi seeds `output.providerThinkingLevel` and the
/// trailing marker from one expression (`anthropic-messages.ts:521`, `:1064`).
async fn decode_terminal(m: &Model, o: &StreamOptions) -> AssistantMessage {
    let raw = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"model\":\"claude-fable-5-1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    let (sink, mut rx) = channel(64);
    let api = ApiId::from(API_ID);
    let frames = decode_sse_bytes_flushing_at_eof(raw.as_bytes().to_vec());
    let m2 = m.clone();
    let api2 = api.clone();
    let level = managed_active_effort(m, o);
    let task = tokio::spawn(async move {
        decode_stream(frames, &m2, &api2, &sink, false, &[], level).await;
    });
    let mut last = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Done { message, .. } = ev {
            last = Some(message);
        }
    }
    task.await.unwrap();
    (*last.expect("a done terminal")).clone()
}

/// Pi `reconstructs an exact historical marker prefix and appends the current marker` (`:88-118`).
///
/// The prefix property is the whole point: turn two's `messages` must START with turn one's array,
/// byte for byte, so Anthropic's cached prefix is still a prefix after the user changes effort.
#[test]
fn reconstructs_an_exact_historical_marker_prefix_and_appends_the_current_marker() {
    let m = managed_model();
    let first = build_body(
        &m,
        &ctx(vec![user("one", 1)]),
        &opts(ModelThinkingLevel::Low),
    );
    let second = build_body(
        &m,
        &ctx(vec![
            user("one", 1),
            assistant(&m, "anthropic", Some("low")),
            user("two", 2),
        ]),
        &opts(ModelThinkingLevel::High),
    );

    let first_messages = wire_messages(&first);
    assert_eq!(
        first_messages,
        vec![
            json!({ "role": "user", "content": "one" }),
            json!({ "role": "system", "content": [], "output_config": { "effort": "low" } }),
        ]
    );

    let second_messages = wire_messages(&second);
    assert_eq!(
        second_messages[..first_messages.len()],
        first_messages[..],
        "turn two must be prefixed byte-for-byte by turn one"
    );
    assert_eq!(
        second_messages.last(),
        Some(&json!({ "role": "system", "content": [], "output_config": { "effort": "high" } }))
    );

    // Request-level `output_config` is HARDCODED "high" on BOTH turns (pi `:1159`) — even the one
    // made at effort "low". The per-turn value travels only in the trailing marker.
    assert_eq!(first["output_config"], json!({ "effort": "high" }));
    assert_eq!(second["output_config"], json!({ "effort": "high" }));
    assert_eq!(
        second["thinking"],
        json!({
            "type": "adaptive",
            "display": "summarized",
            "block_binding": { "prefix_mismatch_behavior": "drop_block" },
        })
    );
}

/// Pi `preserves native effort %s` (`:120-125`), for every rung the Anthropic effort ladder has.
#[tokio::test]
async fn preserves_native_effort_at_every_rung() {
    let m = managed_model();
    for (level, effort) in [
        (ModelThinkingLevel::Low, "low"),
        (ModelThinkingLevel::Medium, "medium"),
        (ModelThinkingLevel::High, "high"),
        (ModelThinkingLevel::Xhigh, "xhigh"),
        (ModelThinkingLevel::Max, "max"),
    ] {
        let o = opts(level);
        let body = build_body(&m, &ctx(vec![user("one", 1)]), &o);
        assert_eq!(
            effort_markers(&body),
            vec![json!({ "role": "system", "content": [], "output_config": { "effort": effort } })],
            "{effort}: exactly one marker, carrying the active effort"
        );
        let message = decode_terminal(&m, &o).await;
        assert_eq!(
            message.provider_thinking_level.as_deref(),
            Some(effort),
            "{effort}: the decoded turn records what the request asked for"
        );
    }
}

/// Pi `defaults omitted effort to high and still enables drop_block` (`:127-136`).
///
/// This is the case that pins the managed thinking branch being OUTSIDE `if model.reasoning` and
/// outside the thinking-enabled check: reasoning is OFF here, and upstream still emits adaptive
/// thinking with `drop_block` plus a `"high"` marker, because the generator pins
/// `thinkingLevelMap.off = null` on every model it grants the flag to.
#[tokio::test]
async fn defaults_omitted_effort_to_high_and_still_enables_drop_block() {
    let m = managed_model();
    let o = opts(ModelThinkingLevel::Off);
    let body = build_body(&m, &ctx(vec![user("one", 1)]), &o);

    assert_eq!(
        wire_messages(&body).last(),
        Some(&json!({ "role": "system", "content": [], "output_config": { "effort": "high" } }))
    );
    assert_eq!(
        body["thinking"]["block_binding"]["prefix_mismatch_behavior"],
        json!("drop_block")
    );
    // NOT `{"type":"disabled"}`: the `off_is_not_null` branch is unreachable for a managed model.
    assert_eq!(body["thinking"]["type"], json!("adaptive"));
    assert_eq!(
        decode_terminal(&m, &o)
            .await
            .provider_thinking_level
            .as_deref(),
        Some("high")
    );
}

/// Pi `does not invent markers for legacy or other-provider assistants` (`:138-148`).
///
/// A turn with no recorded level, and a turn recorded under a different provider, are BOTH skipped —
/// replaying an effort cyrup did not observe would change the prefix and defeat the whole mechanism.
#[test]
fn does_not_invent_markers_for_legacy_or_other_provider_assistants() {
    let m = managed_model();
    let body = build_body(
        &m,
        &ctx(vec![
            user("one", 1),
            assistant(&m, "anthropic", None),
            user("two", 2),
            assistant(&m, "other-provider", Some("low")),
            user("three", 3),
        ]),
        &opts(ModelThinkingLevel::Medium),
    );
    assert_eq!(
        effort_markers(&body),
        vec![json!({ "role": "system", "content": [], "output_config": { "effort": "medium" } })]
    );
}

/// PROV-091, cyrup's own half of the four-condition guard: a turn recorded on a DIFFERENT api is
/// skipped too (pi `msg.api === "anthropic-messages"`, `:1373`). Upstream's fixture cannot express
/// this because its `assistant()` hardcodes the api; cyrup's `AssistantMessage.api` is a required
/// field, so a cross-api transcript is representable and must be handled.
#[test]
fn does_not_invent_markers_for_another_apis_assistant() {
    let m = managed_model();
    let Message::Assistant(mut am) = assistant(&m, "anthropic", Some("low")) else {
        panic!("fixture builds an assistant turn");
    };
    am.api = ApiId::from(crate::known_api::OPENAI_COMPLETIONS);
    let body = build_body(
        &m,
        &ctx(vec![user("one", 1), Message::Assistant(am), user("two", 2)]),
        &opts(ModelThinkingLevel::Medium),
    );
    assert_eq!(
        effort_markers(&body),
        vec![json!({ "role": "system", "content": [], "output_config": { "effort": "medium" } })]
    );
}

/// Pi `leaves unsupported models on top-level effort` (`:150-157`) — the non-regression control for
/// every model cyrup ships today.
#[tokio::test]
async fn leaves_unsupported_models_on_top_level_effort() {
    let m = unmanaged_adaptive_model();
    let o = opts(ModelThinkingLevel::Low);
    let body = build_body(&m, &ctx(vec![user("one", 1)]), &o);

    assert_eq!(
        wire_messages(&body),
        vec![json!({ "role": "user", "content": "one" })],
        "no marker may appear on a model that did not declare the flag"
    );
    assert_eq!(body["output_config"], json!({ "effort": "low" }));
    assert_eq!(
        body["thinking"],
        json!({ "type": "adaptive", "display": "summarized" }),
        "no block_binding without the flag"
    );
    assert_eq!(
        decode_terminal(&m, &o).await.provider_thinking_level,
        None,
        "pi leaves providerThinkingLevel undefined for an unmanaged model"
    );
}

/// PROV-091 — the temperature exclusion (pi adds `model.compat?.supportsMidConvoEffort !== true` to
/// the gate at `:1103-1111`). Reasoning is OFF, so `thinking_enabled` is false and the pre-PROV-091
/// two-term gate would have sent `temperature` to a model that always runs adaptive thinking.
#[test]
fn a_managed_model_never_receives_temperature() {
    let o = StreamOptions {
        temperature: Some(0.5),
        ..opts(ModelThinkingLevel::Off)
    };
    let managed = build_body(&managed_model(), &ctx(vec![user("one", 1)]), &o);
    assert!(
        managed.get("temperature").is_none(),
        "managed model must not receive temperature, got {managed:#}"
    );
    // The control: the same options on the same model without the flag still send it.
    let unmanaged = build_body(&unmanaged_adaptive_model(), &ctx(vec![user("one", 1)]), &o);
    assert_eq!(unmanaged["temperature"], json!(0.5));
}

/// Pi `sends the effort and binding beta headers` (`:159-193`).
#[test]
fn sends_the_effort_and_binding_beta_headers() {
    let beta = build_headers(
        &managed_model(),
        &Context::default(),
        &auth_with(Some("test-key")),
        &opts(ModelThinkingLevel::Off),
        false,
    )
    .get("anthropic-beta")
    .and_then(|v| v.clone())
    .unwrap_or_default();

    assert!(
        beta.contains(MID_CONVERSATION_OUTPUT_CONFIG_BETA),
        "missing the mid-conversation-output-config beta in {beta:?}"
    );
    assert!(
        beta.contains(THINKING_BINDING_CONTROLS_BETA),
        "missing the thinking-binding-controls beta in {beta:?}"
    );

    // And neither is sent for a model that did not declare the flag.
    let unmanaged = build_headers(
        &unmanaged_adaptive_model(),
        &Context::default(),
        &auth_with(Some("test-key")),
        &opts(ModelThinkingLevel::Off),
        false,
    )
    .get("anthropic-beta")
    .and_then(|v| v.clone())
    .unwrap_or_default();
    assert!(!unmanaged.contains(MID_CONVERSATION_OUTPUT_CONFIG_BETA));
    assert!(!unmanaged.contains(THINKING_BINDING_CONTROLS_BETA));
}

/// Pi `isAnthropicEffort` (`anthropic-messages.ts:1430-1432`). `minimal` is NOT an Anthropic effort:
/// it is a `ThinkingLevel` rung that lowers to `low`, so a persisted `minimal` can only come from a
/// foreign writer and must never be replayed as a marker.
#[test]
fn is_anthropic_effort_accepts_only_the_five_real_rungs() {
    for ok in ["low", "medium", "high", "xhigh", "max"] {
        assert!(is_anthropic_effort(Some(ok)), "{ok} is an Anthropic effort");
    }
    for bad in ["minimal", "off", "", "HIGH", "ultra"] {
        assert!(
            !is_anthropic_effort(Some(bad)),
            "{bad} must not be replayed as a marker"
        );
    }
    assert!(!is_anthropic_effort(None));
}

/// PROV-091 — `minimal` end to end: the level a managed model CAN be asked for, but which is never a
/// marker value. `mapThinkingLevelToEffort` lowers it to `low` on the way out (pi `:845-847`), so
/// the active marker reads `low` and a persisted `minimal` on a history turn is ignored.
#[test]
fn minimal_lowers_to_low_and_is_never_replayed() {
    let m = managed_model();
    assert_eq!(
        managed_active_effort(&m, &opts(ModelThinkingLevel::Minimal)).as_deref(),
        Some("low")
    );
    let body = build_body(
        &m,
        &ctx(vec![
            user("one", 1),
            assistant(&m, "anthropic", Some("minimal")),
            user("two", 2),
        ]),
        &opts(ModelThinkingLevel::Minimal),
    );
    assert_eq!(
        effort_markers(&body),
        vec![json!({ "role": "system", "content": [], "output_config": { "effort": "low" } })]
    );
}

/// PROV-091 — the marker must not steal the cache breakpoint. `insertThinkingLevelMessages` runs
/// AFTER `convertMessages` applied `cache_control` (pi `:1341-1425` then `:1069`), so the breakpoint
/// stays on the last real user block even though a `system` marker is appended after it.
#[test]
fn the_trailing_marker_does_not_take_the_cache_breakpoint() {
    let m = managed_model();
    let body = build_params(
        &m,
        &ctx(vec![user("one", 1)]),
        &StreamOptions {
            reasoning: ModelThinkingLevel::High,
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        },
        EnvSource::default(),
        false,
    )
    .expect("fixture declares no unsatisfiable constrained sampling");

    let messages = wire_messages(&body);
    assert_eq!(messages.len(), 2, "user turn + trailing marker: {body:#}");
    assert_eq!(
        messages[0]["content"][0]["cache_control"],
        json!({ "type": "ephemeral" }),
        "the breakpoint stays on the user block"
    );
    assert_eq!(
        messages[1],
        json!({ "role": "system", "content": [], "output_config": { "effort": "high" } }),
        "the marker is untouched"
    );
}
