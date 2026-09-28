//! Endpoint, headers and beta-token resolution.

use super::*;

#[test]
fn url_appends_v1_messages() {
    assert_eq!(
        messages_url("https://api.anthropic.com"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        messages_url("https://api.anthropic.com/"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        messages_url("https://api.anthropic.com/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn api_key_headers_and_version() {
    let m = model();
    let auth = auth_with(Some("sk-ant-api03-xxx"));
    let headers = build_headers(
        &m,
        &Context::default(),
        &auth,
        &StreamOptions::default(),
        false,
    );
    assert_eq!(
        headers.get("x-api-key").and_then(|v| v.clone()).as_deref(),
        Some("sk-ant-api03-xxx")
    );
    assert_eq!(
        headers
            .get("anthropic-version")
            .and_then(|v| v.clone())
            .as_deref(),
        Some(ANTHROPIC_VERSION)
    );
    // interleaved-thinking beta is sent for non-adaptive reasoning models.
    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(beta.contains(INTERLEAVED_THINKING_BETA));
}

/// PROV-056 — the wire half. The catalog now carries `forceAdaptiveThinking: true` on every
/// `kimi-coding` row (pi `ai/scripts/generate-models.ts:1861-1864` @v0.83.0), and this asserts
/// what that flag actually changes on the request, driven by the SHIPPED catalog rather than a
/// synthetic model — because the defect was never in this file's logic, which was already a
/// faithful port, but in the data that reaches it.
///
/// Two divergences per request, on all three models, which is every model the provider has:
/// cyrup sent a budget-based `thinking` block where pi sends `{type: "adaptive"}`
/// (pi `anthropic-messages.ts:1033`), and it sent the `interleaved-thinking-2025-05-14` beta
/// that pi suppresses for adaptive models (`:858`,
/// `needsInterleavedBeta = interleavedThinking && model.compat?.forceAdaptiveThinking !== true`).
/// Pre-fix both assertions are RED for all three rows.
#[test]
fn kimi_coding_catalog_rows_send_adaptive_thinking_and_no_interleaved_beta() {
    let auth = auth_with(Some("sk-ant-api03-xxx"));
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        ..Default::default()
    };
    let models = crate::providers::anthropic::anthropic_fleet_spec("kimi-coding")
        .expect("kimi-coding fleet spec")
        .models();
    assert_eq!(models.len(), 5, "every kimi-coding row must be covered");

    for m in &models {
        let body = build_body(m, &user_ctx("think"), &opts);
        assert_eq!(
            body["thinking"]["type"],
            "adaptive",
            "kimi-coding/{} sends a non-adaptive thinking block to an upstream pi flags as \
             requiring the adaptive format",
            m.id.as_str()
        );

        let beta = build_headers(m, &Context::default(), &auth, &opts, false)
            .get("anthropic-beta")
            .and_then(|v| v.clone())
            .unwrap_or_default();
        assert!(
            !beta.contains(INTERLEAVED_THINKING_BETA),
            "kimi-coding/{} sent the interleaved-thinking beta pi suppresses for adaptive \
             models; beta header was {beta:?}",
            m.id.as_str()
        );
    }
}

#[test]
fn interleaved_thinking_per_api_option_suppresses_beta() {
    // Pi `options?.interleavedThinking ?? true` (anthropic-messages.ts:520): an explicit
    // `false` drops the interleaved-thinking beta header that is otherwise sent by default.
    let m = model();
    let auth = auth_with(Some("sk-ant-api03-xxx"));

    let default_headers = build_headers(
        &m,
        &Context::default(),
        &auth,
        &StreamOptions::default(),
        false,
    );
    let default_beta = default_headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(
        default_beta.contains(INTERLEAVED_THINKING_BETA),
        "beta on by default"
    );

    let opts = StreamOptions {
        api_options: Some(crate::stream::ApiStreamOptions::Anthropic(
            AnthropicOptions {
                interleaved_thinking: Some(false),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    let headers = build_headers(&m, &Context::default(), &auth, &opts, false);
    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(
        !beta.contains(INTERLEAVED_THINKING_BETA),
        "explicit false suppresses the beta"
    );
}

/// PROV-027/PROV-028 — the Copilot branch of `createClient`
/// (anthropic-messages.ts:867-888) plus the dynamic Copilot headers computed at `:525-531`.
///
/// A real Copilot token is a `tid=…;exp=…;proxy-ep=…` claim string with no `sk-ant-oat`
/// marker, so the `isOAuthToken` sniff cyrup used to key off cannot select Bearer for it: every
/// request on Copilot's 9 anthropic-messages rows went out as `x-api-key` and was rejected.
#[test]
fn copilot_uses_bearer_and_dynamic_headers_not_x_api_key() {
    const COPILOT_TOKEN: &str =
        "tid=abc123;exp=1789000000;proxy-ep=proxy.individual.githubcopilot.com;sku=copilot";

    let mut m = model();
    m.provider = "github-copilot".into();
    m.headers = Some(std::collections::BTreeMap::from([(
        "Editor-Version".to_string(),
        Some("vscode/1.107.0".to_string()),
    )]));
    let auth = auth_with(Some(COPILOT_TOKEN));

    // The sniff alone never selects Bearer for this token — that is the whole bug.
    assert!(!is_oauth_token(COPILOT_TOKEN));
    // …but the provider branch does, and it also keeps `isOAuthToken` false for `buildParams`
    // (anthropic-messages.ts:887).
    assert!(!resolve_is_oauth(&m, &auth));

    let ctx = user_ctx("hi");
    let headers = build_headers(&m, &ctx, &auth, &StreamOptions::default(), false);

    assert_eq!(
        headers
            .get("authorization")
            .and_then(|v| v.clone())
            .as_deref(),
        Some(format!("Bearer {COPILOT_TOKEN}").as_str()),
        "Copilot takes `authToken`, not `apiKey` (anthropic-messages.ts:870-871)"
    );
    assert!(
        !headers.contains_key("x-api-key"),
        "the api-key branch must not run for Copilot"
    );
    // Selective betas: none of the Claude-Code/OAuth identity is sent.
    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(!beta.contains("claude-code-20250219"), "got: {beta}");
    assert!(!beta.contains("oauth-2025-04-20"), "got: {beta}");
    assert!(!headers.contains_key("x-app"));

    // PROV-028: the dynamic headers, on top of the static `model.headers` identity.
    assert_eq!(
        headers
            .get("X-Initiator")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("user"),
        "the last turn is a user turn (github-copilot-headers.ts:5-8)"
    );
    assert_eq!(
        headers
            .get("Openai-Intent")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("conversation-edits")
    );
    assert!(
        !headers.contains_key("Copilot-Vision-Request"),
        "no image in this turn"
    );
    assert_eq!(
        headers
            .get("Editor-Version")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("vscode/1.107.0"),
        "the static model.headers identity still merges"
    );

    // An agent-loop follow-up (last turn is a toolResult carrying an image) flips both.
    let mut agent_ctx = ctx.clone();
    agent_ctx.messages.push(Message::ToolResult {
        tool_call_id: cyrup_core::ToolCallId::from("call_1"),
        tool_name: "screenshot".to_string(),
        content: vec![Content::Image {
            data: "aGk=".to_string(),
            mime_type: "image/png".to_string(),
        }],
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: Vec::new(),
        timestamp: 0,
    });
    let headers = build_headers(&m, &agent_ctx, &auth, &StreamOptions::default(), false);
    assert_eq!(
        headers
            .get("X-Initiator")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("agent")
    );
    assert_eq!(
        headers
            .get("Copilot-Vision-Request")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("true")
    );

    // Non-Copilot anthropic providers are untouched by any of this.
    let plain = build_headers(
        &model(),
        &ctx,
        &auth_with(Some("sk-ant-api03-xxx")),
        &StreamOptions::default(),
        false,
    );
    assert_eq!(
        plain.get("x-api-key").and_then(|v| v.clone()).as_deref(),
        Some("sk-ant-api03-xxx")
    );
    assert!(!plain.contains_key("X-Initiator"));
}

#[test]
fn oauth_headers_use_bearer_and_identity() {
    let m = model();
    let auth = auth_with(Some("sk-ant-oat01-yyy"));
    let is_oauth = is_oauth_token(auth.auth.api_key.as_deref().unwrap());
    assert!(is_oauth);
    let headers = build_headers(
        &m,
        &Context::default(),
        &auth,
        &StreamOptions::default(),
        is_oauth,
    );
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|v| v.clone())
            .as_deref(),
        Some("Bearer sk-ant-oat01-yyy")
    );
    assert!(!headers.contains_key("x-api-key"));
    let beta = headers
        .get("anthropic-beta")
        .and_then(|v| v.clone())
        .unwrap_or_default();
    assert!(beta.contains("claude-code-20250219"));
    assert!(beta.contains("oauth-2025-04-20"));
    assert_eq!(
        headers.get("x-app").and_then(|v| v.clone()).as_deref(),
        Some("cli")
    );
    // PROV-077: pi `claudeCodeVersion` is `2.1.280` at v0.87.1 (`anthropic-messages.ts:87`).
    assert_eq!(
        headers.get("user-agent").and_then(|v| v.clone()).as_deref(),
        Some("claude-cli/2.1.280")
    );
}

#[test]
fn oauth_remaps_tool_names_to_claude_code() {
    let mut ctx = user_ctx("x");
    ctx.tools = vec![ToolDef {
        name: "bash".to_string(),
        description: "run".to_string(),
        parameters: json!({}),
        constrained_sampling: None,
    }];
    let m = model();
    // build_params with is_oauth=true via direct call.
    let body = build_params(
        &m,
        &ctx,
        &StreamOptions::default(),
        EnvSource::default(),
        true,
    )
    .unwrap();
    assert_eq!(body["tools"][0]["name"], "Bash");
}

/// PROV-086 — pi `getAnthropicCompat` (`anthropic-messages.ts:211-212` @v0.87.1; `bbb61e34a`,
/// v0.86.0, #9102): an OpenRouter model on `anthropic-messages` sends session affinity by default,
/// in OpenRouter's form (`x-session-id`, `:967`), while a first-party model still sends nothing
/// unless its compat opts in, and then `x-session-affinity`.
#[test]
fn openrouter_anthropic_sends_x_session_id_by_default() {
    let opts = StreamOptions {
        session_id: Some("sess-9".into()),
        ..Default::default()
    };
    let auth = auth_with(Some("sk-or-v1-xxx"));
    let headers_for = |m: &Model| build_headers(m, &Context::default(), &auth, &opts, false);

    let mut router = model();
    router.provider = "openrouter".into();
    router.base_url = "https://openrouter.ai/api".to_string();
    let h = headers_for(&router);
    assert_eq!(h.get("x-session-id"), Some(&Some("sess-9".to_string())));
    assert!(!h.contains_key("x-session-affinity"));

    router.compat = Some(crate::api::compat::AnthropicMessagesCompat {
        send_session_affinity_headers: Some(false),
        ..Default::default()
    });
    assert!(!headers_for(&router).contains_key("x-session-id"));

    let first_party = headers_for(&model());
    assert!(!first_party.contains_key("x-session-id"));
    assert!(!first_party.contains_key("x-session-affinity"));

    let mut opted_in = model();
    opted_in.compat = Some(crate::api::compat::AnthropicMessagesCompat {
        send_session_affinity_headers: Some(true),
        ..Default::default()
    });
    let h = headers_for(&opted_in);
    assert_eq!(
        h.get("x-session-affinity"),
        Some(&Some("sess-9".to_string()))
    );
    assert!(!h.contains_key("x-session-id"));
}

/// PROV-095 — pi `mergeClientHeaders` (`anthropic-messages.ts:294-296` @v0.87.1, #8305): a default
/// client User-Agent under every other header source, on the API-key and Copilot branches. The
/// OAuth branch's own `claude-cli/…` identity still wins over it.
#[test]
fn default_user_agent_sits_under_the_overlays() {
    let auth = auth_with(Some("sk-ant-api03-xxx"));
    crate::utils::user_agent::assert_default_user_agent_under_overlays(|overlay| {
        let opts = StreamOptions {
            headers: overlay,
            ..Default::default()
        };
        build_headers(&model(), &Context::default(), &auth, &opts, false)
    });
    let mut copilot = model();
    copilot.provider = "github-copilot".into();
    crate::utils::user_agent::assert_default_user_agent_under_overlays(|overlay| {
        let opts = StreamOptions {
            headers: overlay,
            ..Default::default()
        };
        build_headers(&copilot, &Context::default(), &auth, &opts, false)
    });

    let oauth = build_headers(
        &model(),
        &Context::default(),
        &auth_with(Some("sk-ant-oat01-yyy")),
        &StreamOptions::default(),
        true,
    );
    let agents: Vec<_> = oauth
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
        .collect();
    assert_eq!(agents.len(), 1, "{oauth:?}");
    assert_eq!(
        agents[0].1.as_deref(),
        Some(format!("claude-cli/{CLAUDE_CODE_VERSION}").as_str())
    );
}
