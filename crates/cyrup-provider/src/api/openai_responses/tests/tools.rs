//! Tool conversion and the DRIFT-001 message-anchored deferred-tool rendering.

use super::*;

#[test]
fn tools_convert_to_responses_function_tools() {
    let mut ctx = user_ctx("hi");
    ctx.tools = vec![ToolDef {
        name: "echo".into(),
        description: "echoes".into(),
        parameters: json!({"type": "object"}),
        constrained_sampling: None,
    }];
    let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "echo");
    // PROV-034: `getCompat` defaults `supportsStrictMode` to **false** on this route
    // (openai-responses.ts:72 @v0.83.0), and `convertResponsesTools` only assigns `strict` when
    // it is true (openai-responses-shared.ts:376-377) — so pi sends NO `strict` key here. The
    // previous `assert_eq!(…["strict"], false)` pinned behaviour pi does not have; it was a
    // test-defect, corrected rather than weakened.
    assert!(
        body["tools"][0].get("strict").is_none(),
        "no compat ⇒ no strict key: {}",
        body["tools"][0]
    );
}

/// PROV-034, opt-in half: with `supportsStrictMode: true` the key appears, carrying pi's
/// `defaultStrict` (`options?.strict === undefined ? false : options.strict` — `false` here).
#[test]
fn strict_is_emitted_only_when_the_model_opts_in() {
    let mut m = model();
    m.compat = Some(ModelCompat {
        supports_strict_mode: Some(true),
        ..Default::default()
    });
    let mut ctx = user_ctx("hi");
    ctx.tools = vec![ToolDef {
        name: "echo".into(),
        description: "e".into(),
        parameters: json!({"type": "object"}),
        constrained_sampling: None,
    }];
    let body = build_params(&m, &ctx, &StreamOptions::default(), None);
    assert_eq!(body["tools"][0]["strict"], false);
}

// -----------------------------------------------------------------------
// DRIFT-001: message-anchored tool loading, the openai-responses rendering
// (Pi `packages/ai/test/deferred-tools.test.ts`).
//
// Every assertion below reads the EMITTED WIRE JSON. The rule that makes this rendering the
// mirror image of the Anthropic one: a deferred tool is omitted from `body.tools` ENTIRELY and
// exists only inside the synthetic `tool_search_output`. Getting that backwards would ship the
// model tool calls whose schemas it never received.
// -----------------------------------------------------------------------

/// A real catalog model, so the default-OFF proof runs against shipped data, not a fixture.
fn catalog_model(id: &str) -> Model {
    crate::providers::openai::openai_models()
        .into_iter()
        .find(|m| m.id.as_str() == id)
        .unwrap_or_else(|| panic!("`{id}` missing from the embedded openai catalog"))
}

fn deferred_tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("The {name} tool"),
        parameters: json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        }),
        constrained_sampling: None,
    }
}

/// Pi `makeAssistantToolCall` — a FOREIGN (anthropic-authored) assistant turn, exactly as the
/// upstream test builds it.
fn assistant_tool_call(id: &str, name: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from(id),
            name: name.to_string(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
        })],
        provider: "anthropic".into(),
        model: "claude-opus-4-6".to_string(),
        api: "anthropic-messages".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 2,
    })
}

fn marked_tool_result(id: &str, added: &[&str]) -> Message {
    Message::ToolResult {
        tool_call_id: ToolCallId::from(id),
        tool_name: "base_tool".to_string(),
        content: vec![Content::text("done")],
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: added.iter().map(|s| (*s).to_string()).collect(),
        timestamp: 3,
    }
}

/// Pi `makeContext(tools, addedToolNames)` — user / assistant toolCall / toolResult / user.
fn deferred_ctx(tools: Vec<ToolDef>, added: &[&str]) -> Context {
    Context {
        system_prompt: None,
        messages: vec![
            Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 1,
            },
            assistant_tool_call("call_1", "base_tool"),
            marked_tool_result("call_1", added),
            Message::User {
                content: vec![Content::text("Hello")],
                timestamp: 4,
            },
        ],
        tools,
    }
}

fn input_types(body: &Value) -> Vec<&str> {
    body["input"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| i.get("type").and_then(Value::as_str).unwrap_or("role-item"))
                .collect()
        })
        .unwrap_or_default()
}

fn tool_names(body: &Value) -> Vec<&str> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn loads_an_openai_responses_tool_through_client_tool_search() {
    // Pi "loads an OpenAI Responses tool through client tool search".
    let ctx = deferred_ctx(
        vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
        &["late_tool"],
    );
    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );

    // (1) The deferred tool is GONE from `body.tools` — not merely flagged there.
    assert_eq!(tool_names(&body), ["base_tool"]);
    assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
    assert!(
        body["tools"][0].get("defer_loading").is_none(),
        "an immediate tool must not carry defer_loading"
    );

    // (2) The pair is injected IMMEDIATELY AFTER the function_call_output at that index.
    assert_eq!(
        input_types(&body),
        [
            "role-item",            // user "Hello"
            "function_call",        // assistant tool call
            "function_call_output", // the tool result
            "tool_search_call",     // <- injected here
            "tool_search_output",
            "role-item", // trailing user "Hello"
        ]
    );

    let items = body["input"].as_array().unwrap();
    let call = &items[3];
    let out = &items[4];

    // (3) The exact `tool_search_call` shape, including the hash INPUT: the full toolCallId,
    // a COMMA-joined name list for the hash and a SPACE-joined one for the query.
    assert_eq!(
        *call,
        json!({
            "type": "tool_search_call",
            "call_id": "pi_tool_load_1co5fstaye10s",
            "execution": "client",
            "status": "completed",
            "arguments": { "query": "late_tool", "limit": 1 },
        })
    );
    assert_eq!(
        call["call_id"],
        json!(format!("pi_tool_load_{}", short_hash("call_1:late_tool"))),
    );

    // (4) The exact `tool_search_output` shape: same call id, and the definition carries
    // `defer_loading: true`.
    //
    // PROV-034: there is **no `strict` key**. pi builds its function-tool literal without one
    // and only then does `if (supportsStrictMode) functionTool.strict = constrainedStrict ??
    // defaultStrict` (`openai-responses-shared.ts:365-378` @v0.83.0), and this api resolves
    // `supportsStrictMode: model.compat?.supportsStrictMode ?? false` (`openai-responses.ts:72`).
    // The embedded `gpt-5.4` row carries `compat: { supportsToolSearch: true }` and nothing
    // else, so the flag is false and the key is absent — where cyrup used to hard-code
    // `"strict": false` onto every tool of every request.
    assert_eq!(
        *out,
        json!({
            "type": "tool_search_output",
            "call_id": "pi_tool_load_1co5fstaye10s",
            "execution": "client",
            "status": "completed",
            "tools": [{
                "type": "function",
                "name": "late_tool",
                "description": "The late_tool tool",
                "parameters": {
                    "type": "object",
                    "properties": { "value": { "type": "string" } },
                    "required": ["value"],
                },
                "defer_loading": true,
            }],
        })
    );
    // Stated positively as well, so the absence is an assertion rather than a gap in the
    // literal above: `strict` must be missing on BOTH sides of the split — the searched-for
    // definition and the immediate `body.tools` prefix — because the same
    // `supportsStrictMode: false` governs both call sites.
    assert!(
        out["tools"][0].get("strict").is_none(),
        "supportsStrictMode is false for gpt-5.4, so pi emits no `strict` key at all"
    );
    assert!(
        body["tools"][0].get("strict").is_none(),
        "the immediate tool prefix must not carry `strict` either"
    );

    // (5) Nothing was displaced or lost: the tool result's own output is untouched.
    assert_eq!(
        items[2],
        json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "done",
        })
    );
}

#[test]
fn unsupported_openai_models_use_the_normal_tool_list() {
    // Pi it.each(["gpt-5.2", "gpt-5.4-nano", "gpt-5.5-pro"]) — a version PREFIX is not the
    // gate; the enabled set is an exact id list.
    for id in ["gpt-5.2", "gpt-5.4-nano", "gpt-5.5-pro"] {
        let ctx = deferred_ctx(
            vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
            &["late_tool"],
        );
        let body = build_params(&catalog_model(id), &ctx, &StreamOptions::default(), None);
        assert_eq!(tool_names(&body), ["base_tool", "late_tool"], "model {id}");
        assert!(
            !input_types(&body).contains(&"tool_search_output"),
            "model {id} must not emit a tool_search_output"
        );
        assert!(
            !input_types(&body).contains(&"tool_search_call"),
            "model {id} must not emit a tool_search_call"
        );
    }
}

#[test]
fn explicit_compat_override_wins_in_both_directions() {
    // Pi "uses the normal tool list when OpenAI tool search is explicitly disabled" — an
    // enabled catalog model behind a proxy provider, turned off by hand.
    let mut off = catalog_model("gpt-5.4");
    off.provider = "openai-proxy".into();
    off.compat = Some(crate::api::compat::ModelCompat {
        supports_tool_search: Some(false),
        ..Default::default()
    });
    let ctx = deferred_ctx(
        vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
        &["late_tool"],
    );
    let body = build_params(&off, &ctx, &StreamOptions::default(), None);
    assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
    assert!(!input_types(&body).contains(&"tool_search_output"));

    // And the override turns it ON for a provider the catalog never enables — the gate is
    // `compat.supportsToolSearch ?? false`, with no provider predicate (Pi
    // openai-responses.ts:74).
    let mut on = model(); // provider "openai", id "gpt-5" — off by default
    let body = build_params(&on, &ctx, &StreamOptions::default(), None);
    assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
    on.compat = Some(crate::api::compat::ModelCompat {
        supports_tool_search: Some(true),
        ..Default::default()
    });
    let body = build_params(&on, &ctx, &StreamOptions::default(), None);
    assert_eq!(tool_names(&body), ["base_tool"]);
    assert!(input_types(&body).contains(&"tool_search_output"));
}

#[test]
fn an_all_deferred_request_omits_the_tools_key_entirely() {
    // There is NO safety valve on this path (Pi openai-responses.ts:301 guards on
    // `immediate.length > 0`; the promote-everything-back rule is Anthropic-only). The body
    // ships with no `tools` key at all and the definition lives purely in the transcript.
    let ctx = deferred_ctx(vec![deferred_tool("late_tool")], &["late_tool"]);
    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );
    assert!(
        body.get("tools").is_none(),
        "expected no `tools` key, got {}",
        body["tools"]
    );
    let items = body["input"].as_array().unwrap();
    assert_eq!(items[4]["tools"][0]["name"], "late_tool");
    assert_eq!(items[4]["tools"][0]["defer_loading"], true);
}

#[test]
fn a_deferred_name_is_loaded_at_most_once_per_request() {
    // `loadedToolNames` is declared once per conversion (Pi openai-responses-shared.ts:143),
    // so a second marker for the same tool anchors nothing.
    let mut ctx = deferred_ctx(
        vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
        &["late_tool"],
    );
    ctx.messages
        .insert(3, assistant_tool_call("call_2", "base_tool"));
    ctx.messages
        .insert(4, marked_tool_result("call_2", &["late_tool"]));
    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );
    let types = input_types(&body);
    assert_eq!(
        types.iter().filter(|t| **t == "tool_search_call").count(),
        1
    );
    assert_eq!(
        types.iter().filter(|t| **t == "tool_search_output").count(),
        1
    );
    // Both tool results still emitted their own output — nothing was swallowed.
    assert_eq!(
        types
            .iter()
            .filter(|t| **t == "function_call_output")
            .count(),
        2
    );
}

#[test]
fn the_search_call_id_hashes_the_full_tool_call_id_and_comma_joined_names() {
    // Pi `shortHash(`${msg.toolCallId}:${names.join(",")}`)` — the FULL id, INCLUDING the
    // `|item_id` suffix that `function_call_output.call_id` drops.
    let mut ctx = deferred_ctx(
        vec![
            deferred_tool("base_tool"),
            deferred_tool("late_tool"),
            deferred_tool("later_tool"),
        ],
        &["late_tool", "later_tool"],
    );
    // A same-provider assistant turn so the `|item_id` survives normalization verbatim.
    ctx.messages[1] = Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from("call_1|fc_item_1"),
            name: "base_tool".to_string(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
        })],
        provider: "openai".into(),
        model: "gpt-5.4".to_string(),
        api: API_ID.into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 2,
    });
    ctx.messages[2] = marked_tool_result("call_1|fc_item_1", &["late_tool", "later_tool"]);

    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );
    let items = body["input"].as_array().unwrap();
    // The output item drops the suffix …
    assert_eq!(items[2]["call_id"], "call_1");
    // … while the search id hashes the id WITH it.
    // Literal, cross-checked against Pi's JS `shortHash` for the exact input
    // `call_1|fc_item_1:late_tool,later_tool` — dropping the `|fc_item_1` suffix or
    // space-joining the names both change this string.
    assert_eq!(items[3]["call_id"], "pi_tool_load_1u3lpgp10pr00m");
    assert_eq!(
        items[3]["call_id"],
        json!(format!(
            "pi_tool_load_{}",
            short_hash("call_1|fc_item_1:late_tool,later_tool")
        ))
    );
    // Comma for the hash, SPACE for the query; `limit` is the count.
    assert_eq!(items[3]["arguments"]["query"], "late_tool later_tool");
    assert_eq!(items[3]["arguments"]["limit"], 2);
    assert_eq!(items[4]["tools"][0]["name"], "late_tool");
    assert_eq!(items[4]["tools"][1]["name"], "later_tool");
}

#[test]
fn a_marked_name_absent_from_context_tools_anchors_nothing() {
    let ctx = deferred_ctx(vec![deferred_tool("base_tool")], &["ghost_tool"]);
    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );
    assert_eq!(tool_names(&body), ["base_tool"]);
    assert!(!input_types(&body).contains(&"tool_search_call"));
}

#[test]
fn a_tool_used_before_its_marker_stays_in_the_prefix() {
    // The model already called it, so hiding it from the prefix would be a lie about what it
    // could see (`splitDeferredTools` suppression rule).
    let mut ctx = deferred_ctx(
        vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
        &["late_tool"],
    );
    ctx.messages[1] = assistant_tool_call("call_1", "late_tool");
    let body = build_params(
        &catalog_model("gpt-5.4"),
        &ctx,
        &StreamOptions::default(),
        None,
    );
    assert_eq!(tool_names(&body), ["base_tool", "late_tool"]);
    assert!(!input_types(&body).contains(&"tool_search_output"));
}

#[test]
fn tool_search_is_off_for_every_openai_responses_model_but_the_seven() {
    // CONSTRAINT: default OFF is load-bearing. Turning this on for a model that has never seen
    // `tool_search_*` items changes its wire payload and would 400. Proven over the REAL
    // embedded catalogs, every provider, not a fixture.
    //
    // Pi's enabled set is baked into the generated catalog by
    // `ai/scripts/generate-models.ts:731-738` against `OPENAI_TOOL_SEARCH_MODEL_IDS` (:324-332);
    // cyrup carries the same data as `compat.supportsToolSearch` in
    // `providers/catalog/openai.json`. `openai-codex` contributes nothing — cyrup does not port
    // `openai-codex-responses`.
    const ENABLED: [&str; 7] = [
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-pro",
        "gpt-5.5",
        "gpt-5.6-luna",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
    ];

    let mut on: Vec<(String, String)> = Vec::new();
    let mut total = 0usize;
    for provider in crate::providers::all::all_providers() {
        for m in provider.models() {
            if m.api.as_str() != API_ID {
                continue;
            }
            total += 1;
            if get_responses_compat(m).supports_tool_search {
                on.push((
                    provider.id().as_str().to_string(),
                    m.id.as_str().to_string(),
                ));
            }
        }
    }

    assert!(
        total > 70,
        "expected the real catalogs to carry many openai-responses models, saw {total}"
    );
    let mut got: Vec<String> = on
        .iter()
        .filter(|(p, _)| p == "openai")
        .map(|(_, id)| id.clone())
        .collect();
    got.sort();
    let mut want: Vec<String> = ENABLED.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(got, want, "enabled set drifted from Pi's id list");
    // No OTHER provider may enable it — every reseller shipping these ids stays off.
    assert!(
        on.iter().all(|(p, _)| p == "openai"),
        "tool search leaked to non-openai providers: {on:?}"
    );
}

#[test]
fn a_disabled_model_emits_a_byte_identical_body_to_the_pre_drift_shape() {
    // The regression guard for constraint 3: with the flag off, the split is a pass-through and
    // `body.tools` is exactly `convert_responses_tools(ctx.tools, false)` — no `defer_loading`
    // key anywhere in the payload.
    let ctx = deferred_ctx(
        vec![deferred_tool("base_tool"), deferred_tool("late_tool")],
        &["late_tool"],
    );
    let body = build_params(&model(), &ctx, &StreamOptions::default(), None);
    assert_eq!(
        body["tools"],
        Value::Array(convert_responses_tools(
            &ctx.tools,
            ConvertResponsesToolsOptions {
                defer_loading: false,
                supports_strict_mode: get_responses_compat(&model()).supports_strict_mode,
                default_strict: Some(false),
            },
        )
        .unwrap())
    );
    assert!(
        !serde_json::to_string(&body)
            .unwrap()
            .contains("defer_loading"),
        "a disabled model must not mention defer_loading: {body}"
    );
}
