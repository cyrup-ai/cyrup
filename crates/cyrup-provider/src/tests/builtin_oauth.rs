//! `OAuthAuth.isSubscription` and the provider `auth: { oauth: … }` clauses that carry it, read
//! through the entry point real callers use: `cyrup_provider::all_providers()` →
//! `Provider::provider_auth()` → `ProviderAuth::oauth`.
//!
//! That is exactly the chain `/login` walks (`cyrup-tui/src/app.rs:2006` calls `all_providers()`,
//! `:2013` reads `provider_auth()`, and `cyrup-config/src/login.rs:450` reads
//! `provider.auth.oauth`) and the chain the footer's ` (sub)` marker needs
//! (`isUsingSubscription` = `isUsingOAuth(id) && getProvider(id)?.auth.oauth?.isSubscription ===
//! true`, pi v0.84.1 `coding-agent/src/core/model-runtime.ts:462-464`).
//!
//! Upstream reference: pi v0.84.1 `ai/test/oauth-auth.test.ts:30-35`, *"identifies only
//! subscription-backed OAuth flows as subscriptions"*.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::all_providers;
use crate::auth::LoginOptions;
use std::sync::Arc;

fn oauth_by_id(id: &str) -> Option<Arc<dyn crate::auth::OAuthAuth>> {
    all_providers()
        .into_iter()
        .find(|p| p.id().as_str() == id)
        .and_then(|p| p.provider_auth().and_then(|a| a.oauth.clone()))
}

/// The built-in providers whose upstream definition wires `lazyOAuth` must offer an OAuth login
/// through `all_providers()` — otherwise `/login` can never present the row and the flow modules
/// are dead code.
#[test]
fn builtin_providers_expose_their_oauth_clause() {
    for (id, name) in [
        // `providers/anthropic.ts:50-54`
        ("anthropic", "Anthropic (Claude Pro/Max)"),
        // `providers/kimi-coding.ts:14-19`
        ("kimi-coding", "Kimi Code (subscription)"),
        // `providers/xai.ts:15-20`
        ("xai", "xAI (Grok/X subscription)"),
        // `providers/openrouter.ts:14-18`
        ("openrouter", "OpenRouter OAuth"),
        // PROV-118 — `providers/openai.ts:14-19` @v1.0.0 ("Sign in with ChatGPT").
        ("openai", "OpenAI (ChatGPT subscription)"),
        // PROV-080 — `providers/meta.ts:14-19` @f1b2e77f5 ("Sign in with Meta").
        ("meta", "Meta (Muse subscription)"),
    ] {
        let oauth = oauth_by_id(id).unwrap_or_else(|| panic!("{id} must expose an oauth strategy"));
        assert_eq!(oauth.name(), name, "{id} oauth display name");
    }
}

/// `isSubscription` is NARROWER than "authenticates with OAuth": OpenRouter signs in with OAuth
/// and still bills per token, so it must not be labelled a subscription. pi v0.84.0's coding-agent
/// changelog entry is the statement of intent: *"Fixed the footer showing `(sub)` for generic
/// OAuth/OpenID sign-ins without a known subscription"* (`coding-agent/CHANGELOG.md:155`).
#[test]
fn only_subscription_backed_oauth_reports_is_subscription() {
    for id in ["anthropic", "kimi-coding", "meta", "openai", "xai"] {
        let oauth = oauth_by_id(id).unwrap_or_else(|| panic!("{id} oauth"));
        assert!(
            oauth.is_subscription(),
            "{id} is subscription-backed upstream (isSubscription: true)"
        );
    }

    let openrouter = oauth_by_id("openrouter").expect("openrouter oauth");
    assert!(
        !openrouter.is_subscription(),
        "OpenRouter OAuth sets no isSubscription upstream (providers/openrouter.ts:14-18); \
         reporting it as a subscription would tell the user their metered usage is free"
    );
}

/// The full subscription set reachable from `all_providers()`. SEVEN upstream OAuth flows carry
/// `isSubscription: true` and `providers/all.rs` registers all seven providers — `meta`
/// (`providers/meta.ts:16` @f1b2e77f5) joined with PROV-080 —
/// `anthropic` (`providers/anthropic.ts:52`), `kimi-coding` (`providers/kimi-coding.ts:16`),
/// `xai` (`providers/xai.ts:17`), `openai-codex` (`providers/openai-codex.ts:15`),
/// `github-copilot` (`providers/github-copilot.ts:16`) and — PROV-118 — `openai`
/// (`providers/openai.ts:16` @v1.0.0). Every other built-in is api-key only or metered OAuth, so
/// nothing else may claim a subscription.
///
/// `openai` and `openai-codex` are now BOTH subscription-backed and both reach a ChatGPT plan;
/// that is upstream's state, and `"OpenAI Codex (legacy)"` (`providers/openai-codex.ts:10`) is how
/// `/login` tells them apart.
#[test]
fn no_other_builtin_provider_claims_a_subscription() {
    let subscription_ids: Vec<String> = all_providers()
        .into_iter()
        .filter(|p| {
            p.provider_auth()
                .and_then(|a| a.oauth.as_ref())
                .is_some_and(|o| o.is_subscription())
        })
        .map(|p| p.id().as_str().to_string())
        .collect();

    let mut sorted = subscription_ids.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "anthropic".to_string(),
            "github-copilot".to_string(),
            "kimi-coding".to_string(),
            "meta".to_string(),
            "openai".to_string(),
            "openai-codex".to_string(),
            "xai".to_string()
        ],
        "the subscription set is exactly the built-in providers upstream marks isSubscription: true"
    );
}

/// The API-key strategy is untouched by the OAuth wiring: adding `oauth` must not remove the env
/// key row `/login` offers, nor the ambient resolution path.
#[test]
fn wiring_oauth_keeps_the_api_key_strategy() {
    for id in [
        "anthropic",
        "kimi-coding",
        "meta",
        "openai",
        "xai",
        "openrouter",
    ] {
        let provider = all_providers()
            .into_iter()
            .find(|p| p.id().as_str() == id)
            .unwrap_or_else(|| panic!("{id} provider"));
        let auth = provider.provider_auth().expect("provider auth");
        assert!(
            auth.api_key.is_some(),
            "{id} keeps its envApiKeyAuth strategy alongside oauth"
        );
    }
}

/// The runtime halves must not drift from the strategies that are actually wired.
///
/// `GitHubCopilotOAuth` and `OpenAiCodexOAuth` are no longer what [`crate::all_providers`] carries
/// — `GitHubCopilotLogin` / `OpenAiCodexOAuthFlow` are, and they only *delegate* `refresh`/
/// `to_auth` to the runtime halves. Two `is_subscription` bodies now stand for one upstream
/// `isSubscription: true`, so this pins that they answer alike: editing the runtime half alone (the
/// mistake the old "this impl is the one the provider's `ProviderAuth` actually carries" doc
/// invited) changes nothing `/login` observes, and this test says so out loud.
#[test]
fn runtime_oauth_halves_agree_with_the_wired_strategies() {
    for (id, runtime) in [
        (
            "github-copilot",
            Arc::new(crate::providers::github_copilot::GitHubCopilotOAuth::new())
                as Arc<dyn crate::auth::OAuthAuth>,
        ),
        (
            "openai-codex",
            Arc::new(crate::providers::openai_codex::OpenAiCodexOAuth::new())
                as Arc<dyn crate::auth::OAuthAuth>,
        ),
    ] {
        let wired = oauth_by_id(id).unwrap_or_else(|| panic!("{id} oauth"));
        assert_eq!(
            wired.is_subscription(),
            runtime.is_subscription(),
            "{id}: the wired strategy and the delegated runtime half disagree on isSubscription"
        );
        assert_eq!(
            wired.name(),
            runtime.name(),
            "{id}: the wired strategy and the delegated runtime half disagree on name"
        );
    }
}

/// PROV-029 — `/login` must REACH the ported flows.
///
/// `github-copilot` and `openai-codex` wired the *runtime half* of their upstream OAuth object
/// (`refresh` + `to_auth` only), so `OAuthAuth::login` fell through to the trait default and
/// `/login` reported `LoginUnsupported` for two providers whose device-code / PKCE flows are fully
/// ported and tested. Both upstream definitions wire a login —
/// `lazyOAuth({ name: "GitHub Copilot", load: loadGitHubCopilotOAuth })`
/// (`providers/github-copilot.ts:16`) and
/// `lazyOAuth({ name: "OpenAI (ChatGPT Plus/Pro)", load: loadOpenAICodexOAuth })`
/// (`providers/openai-codex.ts:13`).
///
/// The probe cancels at the flow's FIRST prompt (Copilot's enterprise-domain text prompt,
/// `oauth/github-copilot.ts:330-334`; Codex's login-method select, `oauth/openai-codex.ts:496-506`),
/// so it never touches the network — it only proves the default was overridden.
#[tokio::test]
async fn copilot_and_codex_logins_are_reachable_from_all_providers() {
    for (id, name) in [
        ("github-copilot", "GitHub Copilot"),
        ("openai-codex", "OpenAI (ChatGPT Plus/Pro)"),
    ] {
        let oauth = oauth_by_id(id).unwrap_or_else(|| panic!("{id} must expose an oauth strategy"));
        assert_eq!(oauth.name(), name, "{id} oauth display name");

        let interaction = crate::auth::oauth::ScriptedInteraction::new(vec![Err(
            crate::auth::oauth::OAuthError::Cancelled,
        )]);
        let error = match oauth.login(&interaction, &LoginOptions::default()).await {
            Ok(_) => panic!("{id}: the cancelled probe must not produce a credential"),
            Err(error) => error,
        };
        assert!(
            !matches!(
                error,
                crate::auth::oauth::OAuthError::LoginUnsupported { .. }
            ),
            "{id}: /login reached the trait default instead of the ported flow ({error})"
        );
        assert!(
            !interaction.prompts().is_empty(),
            "{id}: the ported flow must have asked the user something before failing"
        );
    }
}

/// PROV-118 — `/login` must REACH the new ChatGPT flow on the plain `openai` provider, and must
/// offer it under upstream's selector label (`loginLabel: "Sign in with ChatGPT"`,
/// `providers/openai.ts:17`), which is the only thing distinguishing it from the legacy Codex row.
///
/// The probe supplies no device id, so the flow fails at its first statement
/// (`oauth/openai-chatgpt.ts:237`) before binding a port or opening a browser. That the failure is
/// the device-id message and NOT `LoginUnsupported` is the proof that the trait default was
/// overridden.
///
/// The app half — upstream's `{ getDeviceId: () => this.settingsManager.getOrCreateDeviceId() }`
/// (`modes/interactive/interactive-mode.ts:6262` → `core/settings-manager.ts:1175-1182`, a
/// persisted `deviceId` created with `randomUUID()` on first use) — has LANDED: `cyrup-config`'s
/// `login::login` now takes a `SettingsManager` and builds `LoginOptions::with_device_id` from the
/// global layer (`cyrup_config::settings::installation`), so `/login openai` reaches the flow. That
/// is asserted where it belongs, against the login path, in
/// `cyrup-config/src/tests/installation_id.rs`.
///
/// These assertions deliberately did NOT change: this probe calls the strategy directly and
/// supplies no device id, so refusing before any authorization starts is still the correct
/// behaviour and still the proof that the trait default was overridden.
#[tokio::test]
async fn the_openai_chatgpt_login_is_reachable_from_all_providers() {
    let oauth = oauth_by_id("openai").expect("openai must expose an oauth strategy");
    assert_eq!(oauth.name(), "OpenAI (ChatGPT subscription)");
    assert_eq!(oauth.login_label(), Some("Sign in with ChatGPT"));
    assert!(oauth.is_subscription());

    let interaction = crate::auth::oauth::ScriptedInteraction::new(Vec::new());
    let error = match oauth.login(&interaction, &LoginOptions::default()).await {
        Ok(_) => panic!("the probe must not produce a credential"),
        Err(error) => error,
    };
    assert!(
        !matches!(
            error,
            crate::auth::oauth::OAuthError::LoginUnsupported { .. }
        ),
        "/login reached the trait default instead of the ported flow ({error})"
    );
    assert_eq!(
        error.to_string(),
        "Sign in with ChatGPT requires a device ID (UUID) for this installation"
    );
    assert!(
        interaction.prompts().is_empty() && interaction.events().is_empty(),
        "the device-id check runs before any authorization starts"
    );
}

/// PROV-120 — `/login anthropic` must REACH the method selector and, through it, the headless
/// copy-code login, via the same `all_providers()` → `provider_auth().oauth` chain `/login` walks
/// (not just the concrete `AnthropicOAuth` the module tests drive).
///
/// Upstream `anthropicOAuth.login` (`oauth/anthropic.ts:282-299` @v1.1.0) asks
/// `"Select Anthropic login method:"` first, offering `browser` then `copy_code`
/// (pi `test/anthropic-oauth.test.ts:121-129`). The first probe cancels there, so nothing else
/// runs. The second picks `copy_code` and cancels at the paste prompt: by then the flow has shown
/// the authorize URL with Anthropic's hosted redirect (`COPY_CODE_REDIRECT_URI`, `:23`) and asked
/// for `code#state` (`:218-223`). Neither probe binds a port or touches the network — the
/// copy-code flow starts no listener and the token exchange is never reached.
#[tokio::test]
async fn the_anthropic_login_reaches_the_method_selector_and_copy_code_flow() {
    use crate::auth::oauth::{
        AuthEvent, AuthPromptKind, AuthSelectOption, OAuthError, ScriptedInteraction,
    };

    let oauth = oauth_by_id("anthropic").expect("anthropic must expose an oauth strategy");
    assert_eq!(oauth.name(), "Anthropic (Claude Pro/Max)");

    // 1. The selector is the very first thing `/login anthropic` shows.
    let interaction = ScriptedInteraction::new(vec![Err(OAuthError::Cancelled)]);
    let error = match oauth.login(&interaction, &LoginOptions::default()).await {
        Ok(_) => panic!("the cancelled probe must not produce a credential"),
        Err(error) => error,
    };
    assert!(
        matches!(error, OAuthError::Cancelled),
        "/login anthropic must stop at the selector, not reach the trait default ({error})"
    );
    let prompts = interaction.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert_eq!(prompts[0].kind, Some(AuthPromptKind::Select));
    assert_eq!(prompts[0].message, "Select Anthropic login method:");
    assert_eq!(
        prompts[0].options,
        vec![
            AuthSelectOption {
                id: "browser".to_string(),
                label: "Browser login (default)".to_string(),
                description: None,
            },
            AuthSelectOption {
                id: "copy_code".to_string(),
                label: "Copy code login (headless)".to_string(),
                description: None,
            },
        ]
    );
    assert!(
        interaction.events().is_empty(),
        "no authorize URL before a method is chosen"
    );

    // 2. Choosing `copy_code` runs the headless flow: authorize URL with the hosted redirect,
    //    then one `code#state` paste prompt.
    let interaction = ScriptedInteraction::new(vec![
        Ok("copy_code".to_string()),
        Err(OAuthError::Cancelled),
    ]);
    let error = match oauth.login(&interaction, &LoginOptions::default()).await {
        Ok(_) => panic!("the cancelled probe must not produce a credential"),
        Err(error) => error,
    };
    assert!(matches!(error, OAuthError::Cancelled), "{error}");

    let events = interaction.events();
    let url = match events.first() {
        Some(AuthEvent::AuthUrl { url, instructions }) => {
            assert_eq!(
                instructions.as_deref(),
                Some(
                    "Complete login in your browser, then copy the code Anthropic shows and paste it here."
                )
            );
            url.clone()
        }
        other => panic!("expected the authorize URL first, got {other:?}"),
    };
    assert!(
        url.starts_with("https://claude.ai/oauth/authorize?"),
        "{url}"
    );
    assert!(
        url.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"),
        "the copy-code login must redirect to Anthropic's hosted code page: {url}"
    );
    assert_eq!(
        events.len(),
        1,
        "no exchange after a cancelled paste: {events:?}"
    );

    let prompts = interaction.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert_eq!(prompts[1].kind, Some(AuthPromptKind::ManualCode));
    assert_eq!(
        prompts[1].message,
        "Paste the code Anthropic shows after you sign in:"
    );
    assert_eq!(prompts[1].placeholder.as_deref(), Some("code#state"));
}
