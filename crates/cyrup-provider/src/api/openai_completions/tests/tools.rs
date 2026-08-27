//! Request builder: tool schemas, `tool_choice`, deferred tools and strict sampling.

use super::*;

/// PROV-025 — `deferredToolsMode: "kimi"`.
///
/// BEFORE: `rg 'deferred_tools_mode|DeferredToolsMode' crates/` returned nothing, so a Kimi
/// model received the FULL tool schema set on every single turn and the prompt-prefix cache
/// churned on exactly the provider family upstream added the mode for.
///
/// Pi's mechanism, reproduced here clause for clause: the deferred tool is dropped from
/// `params.tools` (`openai-completions.ts:719-721` @v0.83.0) and emitted ONCE as a
/// `{role: "system", tools: [...]}` message directly after the tool-result run that introduced
/// it (`:1266-1276`).
#[test]
fn kimi_deferred_tools_move_from_the_tools_array_into_an_inline_system_message() {
    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("the {name} tool"),
            parameters: json!({ "type": "object", "properties": {} }),
            constrained_sampling: None,
        }
    }
    fn transcript() -> Vec<Message> {
        vec![
            Message::User { content: vec![Content::text("go")], timestamp: 0 },
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCall {
                    id: ToolCallId::from("c1"),
                    name: "early".into(),
                    arguments: Map::new(),
                    thought_signature: None,
                })],
                provider: "moonshotai".into(),
                model: "kimi-k2".into(),
                api: API_ID.into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                timestamp: 0,
            }),
            Message::ToolResult {
                tool_call_id: ToolCallId::from("c1"),
                tool_name: "early".into(),
                content: vec![Content::text("ok")],
                is_error: false,
                details: None,
                timestamp: 0,
                usage: None,
                // The anchor: this result introduced `late`.
                added_tool_names: vec!["late".to_string()],
            },
        ]
    }
    let ctx = Context {
        system_prompt: None,
        messages: transcript(),
        tools: vec![tool("early"), tool("late")],
    };

    // ---- WITHOUT the flag: today's behaviour, and the control that keeps the assertions
    // below from passing vacuously.
    let plain = build_body(&model(), &ctx, &StreamOptions::default());
    let plain_tools = plain.get("tools").and_then(Value::as_array).expect("tools array");
    assert_eq!(
        plain_tools.len(),
        2,
        "without deferredToolsMode both tools stay in the top-level array"
    );
    assert!(
        !plain
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages")
            .iter()
            .any(|m| m.get("tools").is_some()),
        "without the flag no message carries an inline `tools` key"
    );

    // ---- WITH `compat: {"deferredToolsMode": "kimi"}`.
    let mut kimi_model = model();
    kimi_model.compat = Some(ModelCompat {
        deferred_tools_mode: Some(DeferredToolsMode::Kimi),
        ..Default::default()
    });
    let body = build_body(&kimi_model, &ctx, &StreamOptions::default());

    // `late` is gone from the top-level array; `early` (never deferred) stays.
    let tools = body.get("tools").and_then(Value::as_array).expect("tools array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, vec!["early"], "a deferred tool must not be repeated in `tools`");

    // …and appears exactly once, inline, in a `{role: "system", tools: [...]}` message with no
    // `content` key, positioned after the tool-result run that introduced it.
    let messages = body.get("messages").and_then(Value::as_array).expect("messages");
    let inline_at = messages
        .iter()
        .position(|m| m.get("tools").is_some())
        .expect("an inline kimi tool message is emitted");
    let inline = &messages[inline_at];
    assert_eq!(inline.get("role").and_then(Value::as_str), Some("system"));
    assert!(
        inline.get("content").is_none(),
        "Kimi's tool system message omits the standard `content` field"
    );
    let inline_names: Vec<&str> = inline
        .get("tools")
        .and_then(Value::as_array)
        .expect("inline tools")
        .iter()
        .filter_map(|t| t.pointer("/function/name").and_then(Value::as_str))
        .collect();
    assert_eq!(inline_names, vec!["late"]);
    assert_eq!(
        messages[inline_at - 1].get("role").and_then(Value::as_str),
        Some("tool"),
        "the inline block follows the tool-result run that introduced the tool"
    );
    assert_eq!(
        messages.iter().filter(|m| m.get("tools").is_some()).count(),
        1,
        "the schema is emitted ONCE — repeating it is the churn the mode exists to avoid"
    );
}

#[test]
fn tool_choice_emitted_only_when_caller_sets_it() {
    let ctx = Context {
        tools: vec![ToolDef {
            name: "t".into(),
            description: "d".into(),
            parameters: json!({}),
            constrained_sampling: None,
        }],
        ..Default::default()
    };
    // No caller tool_choice => omitted even with tools present.
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    assert!(body.get("tool_choice").is_none());

    // Mode constraint => bare string.
    let opts = StreamOptions {
        tool_choice: Some(crate::stream::ToolChoice::Required),
        ..Default::default()
    };
    assert_eq!(build_body(&model(), &ctx, &opts)["tool_choice"], "required");

    // Named-function constraint => object.
    let opts = StreamOptions {
        tool_choice: Some(crate::stream::ToolChoice::Function { name: "t".into() }),
        ..Default::default()
    };
    assert_eq!(
        build_body(&model(), &ctx, &opts)["tool_choice"],
        json!({ "type": "function", "function": { "name": "t" } })
    );
}

#[test]
fn strict_tools_only_when_supported() {
    let ctx = Context {
        tools: vec![ToolDef {
            name: "t".into(),
            description: "d".into(),
            parameters: json!({ "type": "object" }),
            constrained_sampling: None,
        }],
        ..Default::default()
    };
    // Together => no strict.
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    assert!(body["tools"][0]["function"].get("strict").is_none());
    // OpenAI => strict: false.
    let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
    assert_eq!(body["tools"][0]["function"]["strict"], false);
}

// PROV-011 — `openai-completions.ts:1309`/`:1317` @v0.83.0: `strict` is
// `resolveJsonSchemaStrictSampling(tool, compat.supportsStrictMode) ?? false`, emitted only
// when the provider supports strict mode.
#[test]
fn constrained_sampling_drives_completions_strict_flag() {
    use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling};
    use crate::utils::constrained_sampling::ConstrainedSamplingError;

    let tool = |strict| ToolDef {
        name: "calc".into(),
        description: "calculate".into(),
        parameters: json!({"type": "object", "properties": {}, "required": []}),
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema { strict },
        )),
    };

    // openai detects `supportsStrictMode: true` ⇒ `strict: true` for a constrained tool…
    let mut ctx = Context {
        tools: vec![tool(StrictSampling::Prefer)],
        ..Default::default()
    };
    let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
    assert_eq!(body["tools"][0]["function"]["strict"], json!(true));

    // …and `false` for an unconstrained one, which is the pre-existing behaviour.
    ctx.tools = vec![ToolDef {
        name: "calc".into(),
        description: "calculate".into(),
        parameters: json!({"type": "object", "properties": {}, "required": []}),
        constrained_sampling: None,
    }];
    let body = build_body(&openai_model(), &ctx, &StreamOptions::default());
    assert_eq!(body["tools"][0]["function"]["strict"], json!(false));

    // `model()` is a `together` model, which detects `supportsStrictMode: false`: no `strict`
    // key at all, and a `require` tool fails the turn with pi's message.
    ctx.tools = vec![tool(StrictSampling::Prefer)];
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    assert!(body["tools"][0]["function"].get("strict").is_none());

    ctx.tools = vec![tool(StrictSampling::Require)];
    assert_eq!(
        build_body_with_env(&model(), &ctx, &StreamOptions::default(), None),
        Err(ConstrainedSamplingError(
            "Tool \"calc\" requires JSON-schema constrained sampling, but strict tools are unsupported."
                .to_string()
        ))
    );
}
