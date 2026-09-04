//! The thinking lowering and the direct `GoogleOptions.thinking` override.

use super::*;

#[test]
fn thinking_budget_for_gemini_2_5() {
    let m = model_with("gemini-2.5-pro", true);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("think"), &opts);
    let tc = &body["generationConfig"]["thinkingConfig"];
    assert_eq!(tc["includeThoughts"], true);
    assert_eq!(tc["thinkingBudget"], 32768);
    assert!(tc.get("thinkingLevel").is_none());
}

#[test]
fn thinking_level_for_gemini_3_pro() {
    let m = model_with("gemini-3-pro-preview", true);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let body = build_body(&m, &user_ctx("think"), &opts);
    let tc = &body["generationConfig"]["thinkingConfig"];
    assert_eq!(tc["includeThoughts"], true);
    assert_eq!(tc["thinkingLevel"], "HIGH");
    assert!(tc.get("thinkingBudget").is_none());
}

#[test]
fn disabled_thinking_when_reasoning_off() {
    // Gemini 2.x reasoning model with reasoning off → thinkingBudget: 0.
    let m = model_with("gemini-2.5-flash", true);
    let body = build_body(&m, &user_ctx("x"), &StreamOptions::default());
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        0
    );
    // Gemini 3 pro reasoning model with reasoning off → thinkingLevel: LOW.
    let m3 = model_with("gemini-3-pro-preview", true);
    let body = build_body(&m3, &user_ctx("x"), &StreamOptions::default());
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "LOW"
    );
}

/// Byte-diff vs Pi `buildParams` (google-generative-ai.ts:373-384): a direct
/// `GoogleOptions.thinking` override is read verbatim, bypassing the unified-`reasoning` lowering.
/// `level` wins over `budgetTokens`; `enabled:false` lowers to the disabled config; the override
/// can DISABLE thinking on a request whose unified `reasoning` is High (proving the override path,
/// not the lowering, drove the bytes).
#[test]
fn google_thinking_override_threads_budget_and_level() {
    // 1. budgetTokens override on a Gemini-2.x model: thinkingBudget = the supplied value
    //    (-1 dynamic), NOT the `getGoogleBudget`-computed 32768.
    let m = model_with("gemini-2.5-pro", true);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
            thinking: Some(GoogleThinking {
                enabled: true,
                budget_tokens: Some(-1),
                level: None,
            }),
        })),
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    // Pi: { includeThoughts: true, thinkingBudget: -1 }.
    assert_eq!(tc, json!({ "includeThoughts": true, "thinkingBudget": -1 }));

    // 2. level wins over budgetTokens (Pi reads `level` first, google-generative-ai.ts:376-381).
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Low,
        api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
            thinking: Some(GoogleThinking {
                enabled: true,
                budget_tokens: Some(9999),
                level: Some(GoogleThinkingLevel::Medium),
            }),
        })),
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    // Pi: { includeThoughts: true, thinkingLevel: "MEDIUM" } (no thinkingBudget).
    assert_eq!(
        tc,
        json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" })
    );

    // 3. enabled:false override DISABLES thinking even though unified reasoning is High → the
    //    model's disabled config (Pi `model.reasoning && options.thinking && !enabled`,
    //    google-generative-ai.ts:383-384). For gemini-2.5 that is `{ thinkingBudget: 0 }`.
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        api_options: Some(crate::stream::ApiStreamOptions::Google(GoogleOptions {
            thinking: Some(GoogleThinking {
                enabled: false,
                budget_tokens: None,
                level: None,
            }),
        })),
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    assert_eq!(tc, json!({ "thinkingBudget": 0 }));

    // 4. without the override, the unified `reasoning` lowering still drives the bytes (32768),
    //    proving (1)/(2) came from the override path.
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    assert_eq!(tc["thinkingBudget"], 32768);
}
