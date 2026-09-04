//! `sampling-handler.ts` — `sampling/createMessage`.
//!
//! A server asks the agent to run a completion; the human approves the request and, separately, the
//! response; the server gets back a single text block.
//! [`McpClientHandler`](crate::runtime::McpClientHandler)'s `create_message` — its `rmcp`
//! `ClientHandler` impl — is the entry point, and it answers `METHOD_NOT_FOUND` unless
//! [`crate::server_manager::McpServerManager::set_sampling_config`] has installed a hook — which
//! [`crate::runtime::initialize_mcp`] does, gated on `settings.sampling(has_ui)`.
//!
//! # The one deliberate divergence
//!
//! [`SamplingOptions::models`] is `cyrup-provider`, not [`cyrup_ext::HostServices::models`]. The host
//! API is a bare `Value` whose only implementations are the trait default (`json!([])`) and a test
//! recorder — no live host implements it — while `cyrup-provider` is the literal upstream mechanism:
//! `sampling-handler.ts:1` imports `complete` from the AI package directly, bypassing the host API.
//!
//! `apiKey`/`headers` from the candidate probe are **not** threaded into [`cyrup_provider::StreamOptions`].
//! [`cyrup_provider::Models::complete`] re-resolves auth itself through the same `resolve_provider_auth`,
//! so [`cyrup_provider::Models::get_auth`] here is the *probe* — which is all `getApiKeyAndHeaders` is
//! used for on the candidate loop — and passing the key twice would be the only way to make the two
//! disagree.
//!
//! `params.metadata` (`sampling-handler.ts:80`) is dropped: it has no `StreamOptions` counterpart.
//! `StreamOptions::sampling_params` is a different thing — provider sampling knobs, not MCP request
//! metadata — and using it would send a server's opaque bag to the provider as request parameters.

// MCP-455 landed in `owner.rs` beside `McpDialog`, which it shares with MCP-232's tool-approval
// gate, because this module did not exist yet. Re-exported rather than moved: every path already
// written against `crate::owner::confirm_sampling` stays valid, which is the same technique
// `crate::state` uses for its forward declarations.
pub use crate::owner::{
    MESSAGE_TEXT_UNKNOWN_BLOCK, SAMPLING_REQUEST_APPROVAL_TITLE, SAMPLING_REQUEST_DECLINED,
    SAMPLING_REQUIRES_INTERACTIVE_APPROVAL, SAMPLING_RESPONSE_APPROVAL_TITLE, SamplingApproval,
    confirm_sampling, format_request_approval, format_response_approval,
};

use std::sync::Arc;

use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, Message, ProviderId, StopReason, Usage,
};
use cyrup_provider::{Context, Model, Models, StreamOptions};
// SEP-2577 deprecates the whole sampling surface upstream; cyrup still ports it because servers in
// the wild still send it. Suppressed on the import, the same way `crate::runtime` does.
#[allow(deprecated)]
use rmcp::model::{
    ContextInclusion, CreateMessageRequestParams, CreateMessageResult, ErrorData, ModelPreferences,
    Role, SamplingMessage, SamplingMessageContentBlock,
};

use crate::abort::throw_if_aborted;
use crate::errors::McpError;

/// `sampling-handler.ts:44`. **No throw site, and this is structural, not an omission.**
/// `CreateMessageRequestParams` has no `task` field: task augmentation is the
/// `io.modelcontextprotocol/tasks` extension, which this client never declares, so a conforming
/// server cannot send one and a non-conforming one has its `task` key dropped at deserialisation.
/// Written down so the day rmcp models it the arm is one `if` with the right text already here —
/// the same treatment [`MESSAGE_TEXT_UNKNOWN_BLOCK`] gets.
pub const SAMPLING_TASKS_UNSUPPORTED: &str = "MCP sampling tasks are not supported";
/// `sampling-handler.ts:47`.
pub const SAMPLING_CONTEXT_UNSUPPORTED: &str = "MCP sampling context inclusion is not supported";
/// `sampling-handler.ts:50`.
pub const SAMPLING_TOOLS_UNSUPPORTED: &str = "MCP sampling tool use is not supported";
/// `sampling-handler.ts:53`.
pub const SAMPLING_TOOL_CHOICE_UNSUPPORTED: &str = "MCP sampling tool choice is not supported";
/// `sampling-handler.ts:56`.
pub const SAMPLING_STOP_SEQUENCES_UNSUPPORTED: &str =
    "MCP sampling stop sequences are not supported";
/// `sampling-handler.ts:234`.
pub const SAMPLING_CALL_FAILED: &str = "MCP sampling model call failed";
/// `sampling-handler.ts:237`.
pub const SAMPLING_CALL_ABORTED: &str = "MCP sampling model call was aborted";
/// `sampling-handler.ts:251`.
pub const SAMPLING_RESULT_EMPTY: &str = "MCP sampling result did not contain text content";
/// `sampling-handler.ts:174`.
pub const NO_CONFIGURED_AUTH: &str = "No configured auth for MCP sampling model";
/// `sampling-handler.ts:176` — cyrup renames pi to cyrup, as every user-facing string in this port
/// does.
pub const NO_MODEL_AVAILABLE: &str = "No cyrup model is available for MCP sampling";
/// **No upstream counterpart, and that is a type difference rather than an omission.**
/// `getApiKeyAndHeaders` answers `{ok: false, error}` or a credential — two states.
/// [`cyrup_provider::Models::get_auth`] has a third, `Ok(None)`, meaning "the provider is unknown or
/// unconfigured". It is recorded as a skip carrying this detail rather than folded into the `Err`
/// arm, so the exhaustion message still distinguishes *auth failed* from *no auth configured*.
pub const NO_CONFIGURED_AUTH_DETAIL: &str = "no configured auth";

/// `sampling-handler.ts:222` / `:229` — the two per-role templates, which differ by one word.
const USER_BLOCK_TEMPLATE: &str = "MCP sampling {kind} content is not supported";
const ASSISTANT_BLOCK_TEMPLATE: &str = "MCP sampling assistant {kind} content is not supported";
/// `sampling-handler.ts:244`.
const RESULT_BLOCK_TEMPLATE: &str = "MCP sampling result {kind} content is not supported";
/// `sampling-handler.ts:209-211` — literal sentinels, not descriptions. Do not "improve" them: a
/// session that records a sampling round trip diverges from pi's bytes if they change.
const SAMPLING_SYNTHETIC_API: &str = "mcp-sampling";
const SAMPLING_SYNTHETIC_PROVIDER: &str = "mcp";
const SAMPLING_SYNTHETIC_MODEL: &str = "sampling-request";

/// Every upstream `throw` in this module becomes a JSON-RPC `-32603`, which is what the TS SDK turns
/// an uncaught handler rejection into. The three `-32602` cases belong to elicitation, not here.
fn internal_msg(message: &str) -> ErrorData {
    ErrorData::internal_error(message.to_string(), None)
}

fn internal(error: McpError) -> ErrorData {
    internal_msg(&error.to_string())
}

fn result_block_unsupported(kind: &str) -> String {
    RESULT_BLOCK_TEMPLATE.replace("{kind}", kind)
}

/// `cyrup_core`'s own helper is `pub(crate)`, so this is a second spelling of the same two lines
/// rather than a dependency edge added for a timestamp.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// `SamplingHandlerOptions` (`sampling-handler.ts:18-25`), minus `serverName`, which is per request.
pub struct SamplingOptions {
    /// `options.autoApprove` — `settings.samplingAutoApprove === true`.
    pub auto_approve: bool,
    /// `ctx.hasUI`, carried explicitly. See [`SamplingApproval::has_ui`] for why this cannot be
    /// inferred from a `false` out of `HostServices::confirm`.
    pub has_ui: bool,
    /// `options.ui` — resolved LIVE, per dialog, through the generation's fenced handle.
    pub session: Arc<crate::runtime::SessionSlot>,
    /// `options.modelRegistry`. See the module doc for why this is `cyrup-provider`.
    pub models: Arc<Models>,
    /// The generation owner. `owner.token()` is the `getSignal()` fallback for a stopped runtime.
    pub owner: Arc<crate::owner::McpRuntimeOwner>,
}

impl SamplingOptions {
    /// `getSignal: () => owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal`
    /// (`init.ts:131-133`).
    ///
    /// **Mechanism divergence, recorded.** Upstream composes two `AbortSignal`s with
    /// `AbortSignal.any`; here a child token is composed by [`crate::abort::combine`] and passed down
    /// as `StreamOptions.cancel` rather than polled inside the completion. The observable behaviour —
    /// an in-flight sampling call dies when the turn is cancelled or the session reloads — is the same.
    ///
    /// **Called twice on purpose.** [`handle_sampling_request`] reads it once at entry and
    /// [`resolve_sampling_model`] reads it again inside the probe loop, exactly as upstream does
    /// (`sampling-handler.ts:40` and `:157`). A token captured once at entry diverges the moment the
    /// turn rolls over mid-request, which is precisely the case the second read exists for.
    #[must_use]
    pub fn signal(&self) -> CancelToken {
        if !self.owner.is_active() {
            return self.owner.token();
        }
        crate::abort::combine(&self.owner.token(), None)
    }

    /// `getCurrentModel: () => owner.isActive() ? ctx.model : undefined` (`init.ts:130`).
    #[must_use]
    pub fn current_model(&self) -> Option<String> {
        if !self.owner.is_active() {
            return None;
        }
        self.session.current_model()
    }

    /// `confirmSampling`'s three inputs, rebuilt per dialog so a generation that stopped between the
    /// request gate and the response gate is inert at the second one.
    fn approval(&self) -> SamplingApproval {
        SamplingApproval {
            auto_approve: self.auto_approve,
            has_ui: self.has_ui,
            dialog: self.session.dialog(),
        }
    }
}

/// `handleSamplingRequest(options, request)` (`sampling-handler.ts:35-93`).
#[allow(deprecated)] // SEP-2577; the same suppression, for the same reason, as `crate::runtime`.
pub async fn handle_sampling_request(
    options: &SamplingOptions,
    server: &str,
    params: CreateMessageRequestParams,
) -> Result<CreateMessageResult, ErrorData> {
    let signal = options.signal();
    throw_if_aborted(&signal, None).map_err(internal)?;

    // Guards 1-5, in upstream's order, so the FIRST violated one is reported. `params.task`
    // (guard 0) is unrepresentable — see [`SAMPLING_TASKS_UNSUPPORTED`].
    if params
        .include_context
        .is_some_and(|inclusion| inclusion != ContextInclusion::None)
    {
        return Err(internal_msg(SAMPLING_CONTEXT_UNSUPPORTED));
    }
    if params.tools.as_ref().is_some_and(|tools| !tools.is_empty()) {
        return Err(internal_msg(SAMPLING_TOOLS_UNSUPPORTED));
    }
    if params.tool_choice.is_some() {
        return Err(internal_msg(SAMPLING_TOOL_CHOICE_UNSUPPORTED));
    }
    if params
        .stop_sequences
        .as_ref()
        .is_some_and(|stops| !stops.is_empty())
    {
        return Err(internal_msg(SAMPLING_STOP_SEQUENCES_UNSUPPORTED));
    }

    // Guard 6 rides inside the conversion, exactly as upstream's does.
    let messages = params
        .messages
        .iter()
        .map(convert_sampling_message)
        .collect::<Result<Vec<Message>, ErrorData>>()?;

    let resolved = resolve_sampling_model(options, params.model_preferences.as_ref()).await?;
    throw_if_aborted(&signal, None).map_err(internal)?;

    confirm_sampling(
        &options.approval(),
        SAMPLING_REQUEST_APPROVAL_TITLE,
        &format_request_approval(
            server,
            &format!("{}/{}", resolved.provider.as_str(), resolved.id.as_str()),
            params.system_prompt.as_deref(),
            &messages,
        ),
    )
    .await
    .map_err(internal)?;
    throw_if_aborted(&signal, None).map_err(internal)?;

    let context = Context {
        system_prompt: params.system_prompt.clone(),
        messages,
        tools: Vec::new(),
    };
    let stream_options = StreamOptions {
        cancel: Some(signal.clone()),
        // `maxTokens: params.maxTokens` — passed through UNMODIFIED and UNCLAMPED. rmcp types it
        // `u32`; widening is lossless.
        max_tokens: Some(u64::from(params.max_tokens)),
        temperature: params.temperature,
        ..Default::default()
    };
    let assistant = options
        .models
        .complete(&resolved, &context, &stream_options)
        .await;

    let converted = convert_assistant_result(&assistant)?;
    throw_if_aborted(&signal, None).map_err(internal)?;
    confirm_sampling(
        &options.approval(),
        SAMPLING_RESPONSE_APPROVAL_TITLE,
        &format_response_approval(server, &converted),
    )
    .await
    .map_err(internal)?;
    Ok(converted)
}

/// `convertSamplingMessage` (`sampling-handler.ts:196-216`).
///
/// `SamplingMessage::content`'s `Single`/`Multiple` already models upstream's
/// `Array.isArray(content) ? content : [content]`, so the normalisation is `into_vec`.
#[allow(deprecated)]
fn convert_sampling_message(message: &SamplingMessage) -> Result<Message, ErrorData> {
    let blocks = message.content.clone().into_vec();
    let timestamp = now_millis();
    match message.role {
        Role::User => Ok(Message::User {
            content: blocks
                .iter()
                .map(|block| convert_text_block(block, USER_BLOCK_TEMPLATE))
                .collect::<Result<_, _>>()?,
            timestamp,
        }),
        // `api: "mcp-sampling"`, `provider: "mcp"`, `model: "sampling-request"`, `zeroUsage()`,
        // `stopReason: "stop"` — literal sentinels, not descriptions. Do not "improve" them.
        Role::Assistant => Ok(Message::Assistant(AssistantMessage {
            content: blocks
                .iter()
                .map(|block| convert_text_block(block, ASSISTANT_BLOCK_TEMPLATE))
                .collect::<Result<_, _>>()?,
            provider: ProviderId::from(SAMPLING_SYNTHETIC_PROVIDER),
            model: SAMPLING_SYNTHETIC_MODEL.to_string(),
            api: ApiId::from(SAMPLING_SYNTHETIC_API),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp,
        })),
    }
}

/// `convertUserContent` / `convertAssistantContent` (`sampling-handler.ts:218-231`), which differ
/// only by their template.
///
/// `{kind}` is the **wire discriminant**, read back off the serialised block through
/// [`crate::owner::sampling_block_type`] — reused rather than re-written, because
/// `SamplingMessageContentBlock` is `#[non_exhaustive]` and a hand-written match would have to
/// invent a name for a variant it has never seen.
#[allow(deprecated)]
fn convert_text_block(
    block: &SamplingMessageContentBlock,
    template: &str,
) -> Result<Content, ErrorData> {
    match block {
        SamplingMessageContentBlock::Text(text) => Ok(Content::Text {
            text: text.text.clone().into(),
            text_signature: None,
        }),
        other => Err(internal_msg(
            &template.replace("{kind}", &crate::owner::sampling_block_type(other)),
        )),
    }
}

/// `convertAssistantResult` (`sampling-handler.ts:232-260`) + `mapStopReason` (`:262-267`).
#[allow(deprecated)]
fn convert_assistant_result(message: &AssistantMessage) -> Result<CreateMessageResult, ErrorData> {
    match message.stop_reason {
        StopReason::Error => {
            return Err(internal_msg(
                message
                    .error_message
                    .as_deref()
                    .unwrap_or(SAMPLING_CALL_FAILED),
            ));
        }
        StopReason::Aborted => {
            return Err(internal_msg(
                message
                    .error_message
                    .as_deref()
                    .unwrap_or(SAMPLING_CALL_ABORTED),
            ));
        }
        _ => {}
    }

    let mut parts: Vec<&str> = Vec::new();
    for block in &message.content {
        match block {
            Content::Text { text, .. } => parts.push(text),
            // `if (block.type === "thinking") return undefined` — dropped, not an error.
            Content::Thinking { .. } => {}
            Content::Image { .. } => return Err(internal_msg(&result_block_unsupported("image"))),
            Content::ToolCall(_) => {
                return Err(internal_msg(&result_block_unsupported("toolCall")));
            }
        }
    }
    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() {
        return Err(internal_msg(SAMPLING_RESULT_EMPTY));
    }

    // `CreateMessageResult::STOP_REASON_*` are exactly `mapStopReason`'s outputs. The remaining
    // arms are spelled out rather than caught by a wildcard: `StopReason` deliberately carries no
    // `#[serde(other)]`, and `Error`/`Aborted` have already returned above, so an exhaustive match
    // is what makes a NEW variant a compile error here rather than a silent passthrough.
    let stop_reason = match message.stop_reason {
        StopReason::Stop => CreateMessageResult::STOP_REASON_END_TURN,
        StopReason::Length => CreateMessageResult::STOP_REASON_END_MAX_TOKEN,
        StopReason::ToolUse => CreateMessageResult::STOP_REASON_TOOL_USE,
        // `return reason` — every other spelling passes through verbatim, in the enum's own
        // `rename_all = "camelCase"` wire spelling.
        StopReason::Pending => "pending",
        StopReason::Deferred => "deferred",
        StopReason::Error | StopReason::Aborted => unreachable!("returned above"),
    };
    // `SamplingMessage` is `#[non_exhaustive]`; `assistant_text` is rmcp's own constructor for
    // exactly this shape — role `Assistant`, one text block — so it stays correct if the struct
    // grows a field.
    Ok(CreateMessageResult::new(
        SamplingMessage::assistant_text(text),
        format!("{}/{}", message.provider.as_str(), message.model),
    )
    .with_stop_reason(stop_reason))
}

/// `resolveSamplingModel`'s candidate assembly (`sampling-handler.ts:135-154`) with
/// `addSamplingCandidate` (`:179-183`) inlined as the dedupe.
///
/// Order is behaviour, not taste: hints in the server's order (each hint scanning the whole registry
/// in registry order), then the session's current model, then everything else.
#[must_use]
pub fn sampling_candidates(
    available: &[Model],
    hints: &[String],
    current: Option<&Model>,
) -> Vec<Model> {
    let mut candidates: Vec<Model> = Vec::new();
    fn push(candidates: &mut Vec<Model>, model: &Model) {
        if !candidates
            .iter()
            .any(|seen| seen.provider == model.provider && seen.id == model.id)
        {
            candidates.push(model.clone());
        }
    }

    for hint in hints {
        let needle = hint.trim().to_lowercase();
        // `if (!normalizedHint) continue;` — an empty or whitespace-only hint matches nothing rather
        // than everything, which is what a bare `.contains("")` would do.
        if needle.is_empty() {
            continue;
        }
        for model in available {
            let haystacks = [
                format!("{}/{}", model.provider.as_str(), model.id.as_str()),
                model.id.as_str().to_string(),
                model.name.clone(),
            ];
            // Plain lowercase substring. NOT fuzzy matching.
            if haystacks
                .iter()
                .any(|name| name.to_lowercase().contains(&needle))
            {
                push(&mut candidates, model);
            }
        }
    }
    if let Some(current) = current {
        push(&mut candidates, current);
    }
    for model in available {
        push(&mut candidates, model);
    }
    candidates
}

/// `resolveSamplingModel` (`sampling-handler.ts:130-177`).
#[allow(deprecated)]
async fn resolve_sampling_model(
    options: &SamplingOptions,
    preferences: Option<&ModelPreferences>,
) -> Result<Model, ErrorData> {
    let available = options.models.get_available(None).await;
    let hints: Vec<String> = preferences
        .and_then(|preferences| preferences.hints.as_ref())
        .map(|hints| hints.iter().filter_map(|hint| hint.name.clone()).collect())
        .unwrap_or_default();
    let current = options.current_model().and_then(|id| {
        available
            .iter()
            .find(|model| model.id.as_str() == id)
            .cloned()
    });
    let candidates = sampling_candidates(&available, &hints, current.as_ref());

    let mut errors: Vec<String> = Vec::new();
    // `const signal = options.getSignal();` — the SECOND read (`sampling-handler.ts:157`).
    let signal = options.signal();
    for model in candidates {
        throw_if_aborted(&signal, None).map_err(internal)?;
        let auth = options.models.get_auth(&model).await;
        throw_if_aborted(&signal, None).map_err(internal)?;
        match auth {
            // `auth.ok === false` — recorded and SKIPPED, never fatal on its own.
            Err(error) => errors.push(format!(
                "{}/{}: {error}",
                model.provider.as_str(),
                model.id.as_str()
            )),
            Ok(None) => errors.push(format!(
                "{}/{}: {NO_CONFIGURED_AUTH_DETAIL}",
                model.provider.as_str(),
                model.id.as_str()
            )),
            Ok(Some(_)) => return Ok(model),
        }
    }

    // The two exhaustion messages, and which one you get is observable.
    if errors.is_empty() {
        return Err(internal_msg(NO_MODEL_AVAILABLE));
    }
    Err(internal_msg(&format!(
        "{NO_CONFIGURED_AUTH}. {}",
        errors.join("; ")
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, deprecated)]
mod tests {
    use super::*;
    use cyrup_provider::{Modality, ModelCost};

    fn model(provider: &str, id: &str, name: &str) -> Model {
        Model {
            id: id.into(),
            name: name.to_string(),
            api: "test".into(),
            provider: provider.into(),
            base_url: String::new(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 100,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn names(models: &[Model]) -> Vec<String> {
        models
            .iter()
            .map(|model| format!("{}/{}", model.provider.as_str(), model.id.as_str()))
            .collect()
    }

    /// `sampling-handler.ts:135-154`. Order is behaviour: hints in the SERVER's order, each hint
    /// scanning the whole registry in registry order, then the current model, then the rest.
    #[test]
    fn candidate_order_is_hint_major_registry_minor() {
        let available = vec![
            model("a", "one", "First"),
            model("b", "two", "Second"),
            model("c", "three", "Third"),
        ];
        // Hint order is deliberately the reverse of registry order, so a implementation that
        // iterated the registry outermost would produce the other answer.
        let hints = vec!["three".to_string(), "one".to_string()];
        let got = sampling_candidates(&available, &hints, None);
        assert_eq!(names(&got), vec!["c/three", "a/one", "b/two"]);
    }

    /// `addSamplingCandidate` (`:179-183`) — first wins, and the current model does not jump a hint.
    #[test]
    fn a_hinted_model_keeps_its_place_when_it_is_also_the_current_model() {
        let available = vec![model("a", "one", "First"), model("b", "two", "Second")];
        let current = model("b", "two", "Second");
        let got = sampling_candidates(&available, &["two".to_string()], Some(&current));
        assert_eq!(
            names(&got),
            vec!["b/two", "a/one"],
            "no duplicate, no reorder"
        );
    }

    /// `if (!normalizedHint) continue;` — an empty or whitespace-only hint matches NOTHING, where a
    /// bare `.contains("")` would match everything and silently reorder the whole registry.
    #[test]
    fn an_empty_hint_matches_nothing_rather_than_everything() {
        let available = vec![model("a", "one", "First"), model("b", "two", "Second")];
        let got = sampling_candidates(&available, &["   ".to_string()], None);
        assert_eq!(
            names(&got),
            vec!["a/one", "b/two"],
            "registry order, unperturbed by a blank hint"
        );
    }

    /// The hint matches on `provider/id`, bare `id`, and `name` — and is a plain lowercase
    /// substring, NOT fuzzy.
    #[test]
    fn hints_match_all_three_names_case_insensitively() {
        let available = vec![model("anthropic", "opus", "Claude Opus")];
        for hint in ["ANTHROPIC/opus", "OPUS", "claude opus"] {
            assert_eq!(
                names(&sampling_candidates(&available, &[hint.to_string()], None)),
                vec!["anthropic/opus"],
                "hint {hint} should match"
            );
        }
        assert!(
            sampling_candidates(&available, &["oPuS-4".to_string()], None)
                .first()
                .is_some_and(|first| first.id.as_str() == "opus"),
            "a non-matching hint falls through to registry order rather than erroring"
        );
    }

    fn assistant(stop_reason: StopReason, content: Vec<Content>) -> AssistantMessage {
        AssistantMessage {
            content,
            provider: ProviderId::from("p"),
            model: "m".to_string(),
            api: ApiId::from("test"),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    fn text(text: &str) -> Content {
        Content::Text {
            text: text.into(),
            text_signature: None,
        }
    }

    /// `mapStopReason` (`:262-267`) — the three named arms, and the model string the server sees.
    #[test]
    fn stop_reasons_map_to_the_wire_spellings() {
        for (reason, expected) in [
            (StopReason::Stop, "endTurn"),
            (StopReason::Length, "maxTokens"),
            (StopReason::ToolUse, "toolUse"),
            (StopReason::Pending, "pending"),
            (StopReason::Deferred, "deferred"),
        ] {
            let result = convert_assistant_result(&assistant(reason, vec![text("hi")]))
                .expect("a text block converts");
            assert_eq!(result.stop_reason.as_deref(), Some(expected));
            assert_eq!(
                result.model, "p/m",
                "`provider/model`, as upstream sends it"
            );
        }
    }

    /// `if (block.type === "thinking") return undefined` — dropped, not an error; and the join is
    /// two newlines.
    #[test]
    fn thinking_is_dropped_and_text_blocks_are_joined() {
        let message = assistant(
            StopReason::Stop,
            vec![
                text("first"),
                Content::Thinking {
                    thinking: "ignored".into(),
                    thinking_signature: None,
                    redacted: false,
                },
                text("second"),
            ],
        );
        let result = convert_assistant_result(&message).expect("text survives");
        assert_eq!(
            result
                .message
                .content
                .first()
                .and_then(|block| block.as_text())
                .map(|block| block.text.as_str()),
            Some("first\n\nsecond")
        );
    }

    /// `:251` — a response that is only thinking has no text, and that is an error rather than an
    /// empty success the server would have to interpret.
    #[test]
    fn a_response_with_no_text_is_an_error() {
        let message = assistant(
            StopReason::Stop,
            vec![Content::Thinking {
                thinking: "only".into(),
                thinking_signature: None,
                redacted: false,
            }],
        );
        let error = convert_assistant_result(&message).expect_err("no text is fatal");
        assert!(
            error.message.contains(SAMPLING_RESULT_EMPTY),
            "got {}",
            error.message
        );
    }

    /// `:234` / `:237` — the provider's own message wins, and the constant is the fallback.
    #[test]
    fn error_and_aborted_carry_the_providers_message_when_it_has_one() {
        let mut failed = assistant(StopReason::Error, vec![]);
        failed.error_message = Some("upstream said no".to_string());
        assert!(
            convert_assistant_result(&failed)
                .expect_err("Error is fatal")
                .message
                .contains("upstream said no")
        );

        let bare = assistant(StopReason::Aborted, vec![]);
        assert!(
            convert_assistant_result(&bare)
                .expect_err("Aborted is fatal")
                .message
                .contains(SAMPLING_CALL_ABORTED)
        );
    }
}
