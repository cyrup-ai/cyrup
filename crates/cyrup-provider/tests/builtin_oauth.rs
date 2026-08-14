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

use cyrup_provider::all_providers;
use std::sync::Arc;

fn oauth_by_id(id: &str) -> Option<Arc<dyn cyrup_provider::auth::OAuthAuth>> {
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
    for id in ["anthropic", "kimi-coding", "xai"] {
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

/// The full subscription set reachable from `all_providers()`. Five upstream OAuth flows carry
/// `isSubscription: true` and `providers/all.rs` registers all five providers:
/// `anthropic` (`providers/anthropic.ts:52`), `kimi-coding` (`providers/kimi-coding.ts:16`),
/// `xai` (`providers/xai.ts:17`), `openai-codex` (`providers/openai-codex.ts:15`) and
/// `github-copilot` (`providers/github-copilot.ts:16`). Every other built-in is api-key only
/// or metered OAuth, so nothing else may claim a subscription.
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
    for id in ["anthropic", "kimi-coding", "xai", "openrouter"] {
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

        let interaction = cyrup_provider::auth::oauth::ScriptedInteraction::new(vec![Err(
            cyrup_provider::auth::oauth::OAuthError::Cancelled,
        )]);
        let error = match oauth.login(&interaction).await {
            Ok(_) => panic!("{id}: the cancelled probe must not produce a credential"),
            Err(error) => error,
        };
        assert!(
            !matches!(
                error,
                cyrup_provider::auth::oauth::OAuthError::LoginUnsupported { .. }
            ),
            "{id}: /login reached the trait default instead of the ported flow ({error})"
        );
        assert!(
            !interaction.prompts().is_empty(),
            "{id}: the ported flow must have asked the user something before failing"
        );
    }
}
