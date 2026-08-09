//! The `auth: { oauth: … }` clause of pi's built-in provider definitions.
//!
//! Ports the `lazyOAuth({ … })` expressions that pi v0.83.0/v0.84.1 puts on five built-in
//! providers:
//!
//! | provider | pi v0.84.1 source | `isSubscription` |
//! |---|---|---|
//! | `anthropic` | `ai/src/providers/anthropic.ts:50-54` | **true** (`:52`) |
//! | `kimi-coding` | `ai/src/providers/kimi-coding.ts:14-19` | **true** (`:16`) |
//! | `xai` | `ai/src/providers/xai.ts:15-20` | **true** (`:17`) |
//! | `openrouter` | `ai/src/providers/openrouter.ts:14-18` | absent — metered, not a plan |
//! | `openrouter` (images) | `ai/src/providers/openrouter-images.ts:13-17` | absent |
//!
//! `github-copilot` (`providers/github-copilot.ts:16`) and `openai-codex`
//! (`providers/openai-codex.ts:15`) wire their own OAuth inside
//! [`super::github_copilot`]/[`super::openai_codex`] and are not routed through here.
//! `radius` is parameterised by a gateway and has no built-in provider in cyrup.
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
use crate::auth::oauth::openrouter::OpenRouterOAuth;
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
    fn only_the_four_built_ins_carry_oauth() {
        for id in ["anthropic", "kimi-coding", "xai", "openrouter"] {
            assert!(
                builtin_provider_oauth(id).is_some(),
                "{id} wires lazyOAuth upstream"
            );
        }
        for id in ["openai", "google", "groq", "deepseek", "minimax", "zai"] {
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
    }
}
