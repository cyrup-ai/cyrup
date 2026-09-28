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
    // Gemini 2.x reasoning model with reasoning off → thinkingBudget: 0 (pi
    // `getDisabledGoogleThinkingConfig` first arm, google-shared.ts:103; pinned upstream by
    // "disables Gemini 2.5 thinking when reasoning is omitted").
    let m = model_with("gemini-2.5-flash", true);
    let body = build_body(&m, &user_ctx("x"), &StreamOptions::default());
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        0
    );
    // A level model whose catalog row STILL SUPPORTS `off` also disables with a zero budget
    // (google-shared.ts:105-106) — an empty map keeps `off` supported.
    let m3_off_ok = model_with("gemini-3-pro-preview", true);
    let body = build_body(&m3_off_ok, &user_ctx("x"), &StreamOptions::default());
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({ "thinkingBudget": 0 })
    );
    // A level model whose row marks `off` unsupported sends the lowest rung it still supports as a
    // level (google-shared.ts:108-110).
    let m3 = model_with_map(
        "gemini-3-pro-preview",
        &[("off", None), ("minimal", None), ("low", Some("LOW"))],
    );
    let body = build_body(&m3, &user_ctx("x"), &StreamOptions::default());
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "LOW"
    );
}

/// `thinkingLevelMap` — not a hard-coded per-family table — decides what each pi rung sends (Pi
/// `resolveGoogleThinkingLevel`, google-shared.ts:48-65). Each case below is one of pi's own
/// `test/google-thinking-level-map.test.ts` cases @v0.87.1, kept separate so a regression names
/// exactly which rung broke.
///
/// (1) native medium survives on a Gemini-3 Pro row that maps it ("preserves native medium effort
/// for Gemini 3.1 Pro").
#[test]
fn mapped_medium_survives_on_gemini_3_pro() {
    let m = model_with_map("gemini-3-pro-preview", &[("medium", Some("MEDIUM"))]);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Medium,
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    assert_eq!(
        tc,
        json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" })
    );
}

/// (2) an extended rung maps DOWN to a real Google level ("maps Google Generative AI xhigh to a
/// supported level").
#[test]
fn mapped_xhigh_reaches_a_real_google_level() {
    let m = model_with_map("gemini-3-pro-preview", &[("xhigh", Some("HIGH"))]);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Xhigh,
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    assert_eq!(
        tc,
        json!({ "includeThoughts": true, "thinkingLevel": "HIGH" })
    );
}

/// (3) the token-budget table is keyed on the RESOLVED level, not the requested rung ("honors
/// uppercase provider values" + "uses mapped levels for token budgets").
#[test]
fn token_budget_is_keyed_on_the_resolved_level() {
    let m = model_with_map("gemini-2.5-pro", &[("low", Some("HIGH"))]);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::Low,
        ..Default::default()
    };
    let tc =
        build_body(&m, &user_ctx("think"), &opts)["generationConfig"]["thinkingConfig"].clone();
    assert_eq!(
        tc,
        json!({ "includeThoughts": true, "thinkingBudget": 32768 })
    );
}

/// (4) reasoning omitted on a level row that bars `off`/`minimal` → the lowest supported rung
/// ("uses the lowest supported level when reasoning is omitted", pi issue #9455).
#[test]
fn reasoning_off_uses_the_lowest_supported_level() {
    let m = model_with_map(
        "gemini-3-flash-preview",
        &[("off", None), ("minimal", None), ("low", Some("LOW"))],
    );
    let tc = build_body(&m, &user_ctx("x"), &StreamOptions::default())["generationConfig"]
        ["thinkingConfig"]
        .clone();
    assert_eq!(tc, json!({ "thinkingLevel": "LOW" }));
}

/// A mapping that names no Google level fails the turn before any HTTP, with pi's exact message
/// (`[CYRUP-DELTA]` for google-shared.ts:61-63's throw).
#[test]
fn unmappable_thinking_level_fails_the_request() {
    let m = model_with_map("gemini-3-pro-preview", &[("high", Some("ULTRA"))]);
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let err = build_params(&m, &user_ctx("think"), &opts).expect_err("ULTRA is not a Google level");
    assert_eq!(
        err,
        GoogleParamsError::UnsupportedThinkingLevel(
            "Unsupported Google thinking level mapping for google/gemini-3-pro-preview: high -> ULTRA"
                .to_string()
        )
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
