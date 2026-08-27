//! `functionDeclarations` encoding and the `functionCallingConfig` mode.

use super::*;

#[test]
fn tools_encode_function_declarations_and_tool_config() {
    let mut ctx = user_ctx("use a tool");
    ctx.tools = vec![ToolDef {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        constrained_sampling: None,
    }];
    let m = model_with("gemini-2.0-flash", false);
    let opts = StreamOptions {
        tool_choice: Some(ToolChoice::Required),
        ..Default::default()
    };
    let body = build_body(&m, &ctx, &opts);
    let decl = &body["tools"][0]["functionDeclarations"][0];
    assert_eq!(decl["name"], "read");
    assert_eq!(
        decl["parametersJsonSchema"]["properties"]["path"]["type"],
        "string"
    );
    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
}

// PROV-011 — `resolveGoogleFunctionCallingMode` (`google-shared.ts:311-324` @v0.83.0). The
// Google leg is the only route where a strict tool buys a SERVER-side guarantee (`VALIDATED`)
// rather than a hint, and cyrup could never emit it because the mode was mapped from
// `tool_choice` alone.
#[test]
fn strict_tools_select_validated_function_calling_mode() {
    use crate::context::{ConstrainedSampling, ConstrainedSamplingConfig, StrictSampling};

    let tool = |constrained| ToolDef {
        name: "calc".into(),
        description: "calculate".into(),
        parameters: json!({
            "type": "object",
            "properties": { "expr": { "type": "string" } },
            "required": ["expr"],
        }),
        constrained_sampling: constrained,
    };
    let strict = || {
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema {
                strict: StrictSampling::Prefer,
            },
        ))
    };

    // Gemini 3 supports strict sampling ⇒ VALIDATED, with no explicit tool choice.
    let m3 = model_with("gemini-3-pro-preview", true);
    let mut ctx = user_ctx("x");
    ctx.tools = vec![tool(strict())];
    let body = build_body(&m3, &ctx, &StreamOptions::default());
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"]["mode"],
        json!("VALIDATED")
    );

    // Gemini 2.5 does not (`supportsGoogleStrictToolSampling` is major >= 3), and `prefer`
    // degrades silently ⇒ no toolConfig at all.
    let m2 = model_with("gemini-2.5-pro", true);
    let body = build_body(&m2, &ctx, &StreamOptions::default());
    assert!(body.get("toolConfig").is_none());

    // An explicit `none`/`any` choice wins over VALIDATED (`google-shared.ts:317-319`)…
    let opts = StreamOptions {
        tool_choice: Some(ToolChoice::None),
        ..Default::default()
    };
    let body = build_body(&m3, &ctx, &opts);
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"]["mode"],
        json!("NONE")
    );
    // …but `auto` does not.
    let opts = StreamOptions {
        tool_choice: Some(ToolChoice::Auto),
        ..Default::default()
    };
    let body = build_body(&m3, &ctx, &opts);
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"]["mode"],
        json!("VALIDATED")
    );

    // A plain tool on the same model still maps from tool_choice only.
    ctx.tools = vec![tool(None)];
    let body = build_body(&m3, &ctx, &opts);
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"]["mode"],
        json!("AUTO")
    );
    let body = build_body(&m3, &ctx, &StreamOptions::default());
    assert!(body.get("toolConfig").is_none());
}
