//! The `auth: { oauth: … }` clause of pi's built-in provider definitions.
//!
//! Ports the `lazyOAuth({ … })` expressions that pi v0.83.0/v0.84.4 puts on six built-in
//! providers:
//!
//! | provider | pi v0.84.4 source | `isSubscription` |
//! |---|---|---|
//! | `anthropic` | `ai/src/providers/anthropic.ts:50-54` | **true** (`:52`) |
//! | `kimi-coding` | `ai/src/providers/kimi-coding.ts:14-19` | **true** (`:16`) |
//! | `xai` | `ai/src/providers/xai.ts:15-20` | **true** (`:17`) |
//! | `openrouter` | `ai/src/providers/openrouter.ts:14-18` | absent — metered, not a plan |
//! | `openrouter` (images) | `ai/src/providers/openrouter-images.ts:13-17` | absent |
//! | `radius` | `ai/src/providers/radius.ts:32` | absent (`oauth/radius.ts:357-361`) |
//!
//! `github-copilot` (`providers/github-copilot.ts:16`) and `openai-codex`
//! (`providers/openai-codex.ts:15`) wire their own OAuth inside
//! [`super::github_copilot`]/[`super::openai_codex`] and are not routed through here.
//!
//! `radius` (PROV-014) is parameterised by a gateway: `lazyOAuth({ name, load: () =>
//! loadRadiusOAuth({ name, gateway }) })` closes over the provider's `name` and normalized
//! `gateway` (`radius.ts:21-23`). The arm here is the BUILT-IN instance — `"Radius"` on
//! `DEFAULT_RADIUS_GATEWAY` — and [`super::radius::radius_auth`] is the parameterised form a
//! `models.json` provider with `"oauth": "radius"` uses.
//!
//! **Mechanism divergence from `lazyOAuth`.** Upstream defers loading the flow module because a
//! *variable* dynamic `import()` is what keeps Node-only code (`node:http` callback servers,
//! `node:crypto` PKCE) out of a browser bundle — see the note at the head of
//! [`crate::auth::oauth::load`]. Rust links statically, so there is nothing to defer: every flow
//! constructor here is a field assignment with no I/O, and the eager value additionally makes
//! `name`/`login_label`/`is_subscription` readable without a fallible load, which is exactly what
//! upstream's eager `name`/`isSubscription`/`loginLabel` copy on the lazy wrapper
//! (`ai/src/auth/helpers.ts:52-54`) exists to provide.

use crate::auth::OAuthAuth;
use crate::auth::oauth::anthropic::AnthropicOAuth;
use crate::auth::oauth::kimi_coding::KimiCodingOAuth;
use crate::auth::oauth::load::RadiusOptions;
use crate::auth::oauth::openrouter::OpenRouterOAuth;
use crate::auth::oauth::radius::RadiusOAuth;
use crate::auth::oauth::xai::XaiOAuth;
use std::sync::Arc;

/// The OAuth login strategy pi's built-in provider definition carries for `provider_id`, or `None`
/// for the providers whose `auth` clause has no `oauth` member.
pub fn builtin_provider_oauth(provider_id: &str) -> Option<Arc<dyn OAuthAuth>> {
    match provider_id {
        // `lazyOAuth({ name: "Anthropic (Claude Pro/Max)", isSubscription: true, … })`
        // (`providers/anthropic.ts:50-54`).
        "anthropic" => Some(Arc::new(AnthropicOAuth::new())),
        // `lazyOAuth({ name: "Kimi Code (subscription)", isSubscription: true, loginLabel: … })`
        // (`providers/kimi-coding.ts:14-19`).
        "kimi-coding" => Some(Arc::new(KimiCodingOAuth::new())),
        // `lazyOAuth({ name: "xAI (Grok/X subscription)", isSubscription: true, loginLabel: … })`
        // (`providers/xai.ts:15-20`).
        "xai" => Some(Arc::new(XaiOAuth::new())),
        // `lazyOAuth({ name: "OpenRouter OAuth", loginLabel: … })` — note the deliberate absence
        // of `isSubscription` (`providers/openrouter.ts:14-18`): OpenRouter OAuth still bills per
        // token, so it must NOT be labelled a subscription.
        "openrouter" => Some(Arc::new(OpenRouterOAuth::new())),
        // `lazyOAuth({ name, load: () => loadRadiusOAuth({ name, gateway }) })`
        // (`providers/radius.ts:32` @v0.84.4) with the built-in's defaults (`:21-23`). No
        // `isSubscription`: Radius bills per token (`oauth/radius.ts:357-361` sets none).
        "radius" => Some(Arc::new(RadiusOAuth::new(RadiusOptions {
            name: super::radius::RADIUS_PROVIDER_NAME.to_string(),
            gateway: super::radius::DEFAULT_RADIUS_GATEWAY.to_string(),
        }))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn only_the_five_built_ins_carry_oauth() {
        for id in ["anthropic", "kimi-coding", "xai", "openrouter", "radius"] {
            assert!(
                builtin_provider_oauth(id).is_some(),
                "{id} wires lazyOAuth upstream"
            );
        }
        for id in [
            "openai",
            "google",
            "groq",
            "deepseek",
            "minimax",
            "zai",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
        ] {
            assert!(
                builtin_provider_oauth(id).is_none(),
                "{id}'s upstream auth clause has no oauth member"
            );
        }
    }

    /// `ai/test/oauth-auth.test.ts:30-35` — "identifies only subscription-backed OAuth flows as
    /// subscriptions", asserted through the provider clause that actually reaches a user.
    #[test]
    fn subscription_split_matches_upstream() {
        for id in ["anthropic", "kimi-coding", "xai"] {
            let oauth = builtin_provider_oauth(id).expect("oauth");
            assert!(oauth.is_subscription(), "{id} is subscription-backed");
        }
        let openrouter = builtin_provider_oauth("openrouter").expect("oauth");
        assert!(
            !openrouter.is_subscription(),
            "OpenRouter OAuth is metered, not a subscription (providers/openrouter.ts:14-18 sets no isSubscription)"
        );
        let radius = builtin_provider_oauth("radius").expect("oauth");
        assert!(
            !radius.is_subscription(),
            "Radius OAuth is metered, not a subscription (oauth/radius.ts:357-361 sets no isSubscription)"
        );
        // `radius.ts:22` — the flow signs in under the provider's display name.
        assert_eq!(radius.name(), "Radius");
    }
}
