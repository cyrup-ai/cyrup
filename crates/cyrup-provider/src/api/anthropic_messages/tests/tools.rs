//! Tool encoding and constrained sampling.

use super::*;

// PROV-011 — `resolveJsonSchemaStrictSampling` on the Anthropic route
// (`anthropic-messages.ts:1298` @v0.83.0, `supportsStrictTools` read at `:183`).
#[test]
fn constrained_sampling_drives_anthropic_strict_tools() {
    use crate::api::compat::AnthropicMessagesCompat;
    use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling};
    use crate::utils::constrained_sampling::ConstrainedSamplingError;

    let strict_tool = |strict| ToolDef {
        name: "Edit".into(),
        description: "edit a file".into(),
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false,
        }),
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema { strict },
        )),
    };

    // (a) No `supportsStrictTools` (the default) + `prefer` ⇒ degrade silently: no `strict`
    // key, and the legacy three-key `input_schema` — byte-identical to a plain tool.
    let mut ctx = user_ctx("hi");
    ctx.tools = vec![strict_tool(StrictSampling::Prefer)];
    let body = build_body(&model(), &ctx, &StreamOptions::default());
    assert!(body["tools"][0].get("strict").is_none());
    assert!(
        body["tools"][0]["input_schema"]
            .get("additionalProperties")
            .is_none(),
        "the non-strict branch sends only pi's legacy type/properties/required subset"
    );

    // (b) `supportsStrictTools` ⇒ `strict: true` AND the whole schema, with pi's legacy subset
    // spread over it so `type`/`properties`/`required` still win.
    let mut m = model();
    m.compat = Some(AnthropicMessagesCompat {
        supports_strict_tools: Some(true),
        ..Default::default()
    });
    let body = build_body(&m, &ctx, &StreamOptions::default());
    assert_eq!(body["tools"][0]["strict"], json!(true));
    assert_eq!(
        body["tools"][0]["input_schema"]["additionalProperties"],
        json!(false)
    );
    assert_eq!(body["tools"][0]["input_schema"]["type"], json!("object"));
    // The EXACT key set of a strict tool. pi's object literal is written
    // `name, description, eager_input_streaming?, strict?, input_schema, defer_loading?,
    // cache_control?` (`anthropic-messages.ts:1313-1321` @v0.83.0), but a JSON object is
    // unordered and this workspace's `serde_json` has no `preserve_order` feature, so `Map` is
    // a `BTreeMap` and emission order is lexicographic — only the key SET is observable on the
    // wire. pi asserts key sets the same way, `.sort()`ed
    // (`packages/ai/test/bedrock-error-metadata.test.ts:117`). Equality on the whole vector
    // still pins it exactly: no missing key, no extra key.
    //
    // `cache_control` IS part of that set here. `StreamOptions::default()` leaves
    // `cacheRetention` unset, which resolves to "short" (`:49-57`), so `getCacheControl`
    // returns `{ type: "ephemeral" }` (`:59-73`); `supportsCacheControlOnTools` defaults to
    // TRUE (`:180`) so `buildParams` forwards it to `convertTools` (`:1014`), which stamps the
    // LAST tool — `index === tools.length - 1` (`:1320`) — and this context has exactly one.
    let keys: Vec<&str> = body["tools"][0]
        .as_object()
        .expect("tool is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "cache_control",
            "description",
            "eager_input_streaming",
            "input_schema",
            "name",
            "strict"
        ]
    );
    // Its VALUE is the plain ephemeral marker for "short" retention. `build_body` passes no
    // env overlay, so `resolve_cache_retention` falls through to the ambient
    // `PI_CACHE_RETENTION` (`:371-378`, pi `:49-57`); pin the overlay so an exported "long" in
    // the developer's shell cannot swap in the 1h-ttl variant — that branch is pinned
    // separately by `tools_encode_eager_streaming_and_cache_control`.
    let short = ProviderEnv::from([("PI_CACHE_RETENTION".into(), "short".into())]);
    let pinned = build_params(&m, &ctx, &StreamOptions::default(), Some(&short), false)
        .expect("supports_strict_tools satisfies the `prefer` tool");
    assert_eq!(
        pinned["tools"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );

    // (c) `require` on a model without strict tools fails the whole turn, with pi's text.
    ctx.tools = vec![strict_tool(StrictSampling::Require)];
    assert_eq!(
        build_params(&model(), &ctx, &StreamOptions::default(), None, false),
        Err(ConstrainedSamplingError(
            "Tool \"Edit\" requires JSON-schema constrained sampling, but strict tools are unsupported."
                .to_string()
        ))
    );
}

#[test]
fn tools_encode_eager_streaming_and_cache_control() {
    let mut ctx = user_ctx("use a tool");
    ctx.tools = vec![ToolDef {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        constrained_sampling: None,
    }];
    let m = model();
    let opts = StreamOptions {
        cache_retention: Some(CacheRetention::Long),
        ..Default::default()
    };
    let body = build_body(&m, &ctx, &opts);
    let tool = &body["tools"][0];
    assert_eq!(tool["name"], "read");
    assert_eq!(tool["eager_input_streaming"], true);
    assert_eq!(tool["input_schema"]["type"], "object");
    assert_eq!(tool["input_schema"]["required"][0], "path");
    // long retention => cache_control with 1h ttl on the last tool.
    assert_eq!(tool["cache_control"]["type"], "ephemeral");
    assert_eq!(tool["cache_control"]["ttl"], "1h");
}

#[test]
fn fine_grained_beta_when_eager_unsupported() {
    let mut m = model();
    m.compat = Some(ModelCompat {
        supports_eager_tool_input_streaming: Some(false),
        ..Default::default()
    });
    let mut ctx = user_ctx("x");
    ctx.tools = vec![ToolDef {
        name: "read".to_string(),
        description: "d".to_string(),
        parameters: json!({}),
        constrained_sampling: None,
    }];
    let auth = auth_with(None);
    let headers = build_headers(&m, &ctx, &auth, &StreamOptions::default(), false);
    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(
        beta.contains(FINE_GRAINED_TOOL_STREAMING_BETA),
        "got: {beta}"
    );
    // tools omit eager_input_streaming when unsupported.
    let body = build_body(&m, &ctx, &StreamOptions::default());
    assert!(body["tools"][0].get("eager_input_streaming").is_none());
}
