//! Request encoding: the client API key (Pi `getClientApiKey`, `openai-responses.ts:58-63`
//! @v1.0.1) **and what kind of token it is** (`isChatGPTSignIn`, `:36-47` @v1.0.1).

use super::headers::header_present;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::StreamOptions;

/// The base URL that, paired with provider `openai`, means "this request goes to OpenAI itself".
/// Kept in step with [`crate::providers::openai::OPENAI_BASE_URL`] and
/// [`crate::auth::oauth::openai_chatgpt::RESOURCE`] — upstream spells the same literal in all
/// three places (`openai-responses.ts:43`, `providers/openai.ts:11`, `openai-chatgpt.ts:21`).
const OPENAI_REAL_BASE_URL: &str = "https://api.openai.com/v1";

/// The prefix every OpenAI **api key** carries (`openai-responses.ts:37-38`, `:45`).
const OPENAI_API_KEY_PREFIX: &str = "sk-";

/// Which kind of credential this Responses request carries — pi `isChatGPTSignIn`
/// (`openai-responses.ts:36-47` @v1.0.1) as a **value** instead of a predicate.
///
/// PROV-118. The distinction is not cosmetic: an api key and a "Sign in with ChatGPT" access token
/// have **different legal request fields**. Four fields that an api key may send make a
/// token-sharing request fail, so the classification has to reach the body builder on every
/// request. Upstream's shape is `isChatGPTSignIn(model, apiKey)`, a predicate a caller must
/// remember to ask — and `buildParams` is the only one of pi's four body-building paths that does.
///
/// Per `docs/RUST-DESIGN-REVIEW.md`:
///
/// * **enum, not newtype or typestate.** The decision table's "state that must be *inspected
///   dynamically* at runtime" row: which credential a request carries is decided per request by
///   auth resolution, and both kinds flow through the same `ApiImpl::run`. There is no illegal
///   *sequence* to prevent, so typestate would be ceremony — and the compiler cannot know the kind
///   at the relevant program point anyway, because it comes out of a credential store.
/// * **the enum alone is not the guarantee.** A plain `ResponsesTokenKind` parameter can still be
///   passed the wrong variant. What makes it hold is [`ResponsesCredential`]: the key and its
///   classification are produced **together**, at the one boundary that sees the key, with private
///   fields and no public constructor, so a caller cannot obtain the key for the request without
///   also obtaining the answer to "may this request carry the four fields?". That is the "newtype
///   at a boundary, construct through a fallible parser, keep it intact downstream" rule applied
///   to a pair rather than a scalar.
/// * **guarantee gained**: it is not expressible to build a Responses body without having
///   classified the credential, and the classification cannot disagree with the key that is sent,
///   because they are the same value.
/// * **guarantee NOT gained**: this does not prove the *rule* is right. That the four fields are
///   exactly the rejected set, and that `sk-` is the discriminator, are upstream facts pinned by
///   tests, not by types. Nor does it stop a future `ApiImpl` resolving a key by another route;
///   [`ResponsesCredential::resolve`] is the only constructor, which is what keeps that honest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesTokenKind {
    /// Anything that is not a ChatGPT sign-in token: an `sk-`-prefixed OpenAI key, a gateway or
    /// Copilot key, the literal `"unused"` standing in for a bearer header, or any credential at
    /// all on a base URL that is not OpenAI's own. Sends the full request body.
    ApiKey,
    /// A "Sign in with ChatGPT" access token (`auth/oauth/openai_chatgpt`) bound for
    /// `https://api.openai.com/v1`. Upstream's comment: "OpenAI API keys start with `sk-`; a
    /// different credential sent directly to OpenAI is a Sign in with ChatGPT access token"
    /// (`:36-39`).
    ChatGptSignIn,
}

impl ResponsesTokenKind {
    /// `isChatGPTSignIn(model, apiKey)` (`openai-responses.ts:40-47`): provider `openai`, the real
    /// OpenAI base URL, and a credential that does not start with `sk-`.
    ///
    /// Pure — a function of (provider, base url, credential) alone, so every arm is assertable
    /// without a request. Upstream's `apiKey !== undefined` arm is the `Option`: no credential at
    /// all is not a sign-in token.
    fn classify(model: &Model, api_key: Option<&str>) -> Self {
        let is_sign_in = model.provider.as_str() == "openai"
            && model.base_url == OPENAI_REAL_BASE_URL
            && api_key.is_some_and(|key| !key.starts_with(OPENAI_API_KEY_PREFIX));
        if is_sign_in {
            Self::ChatGptSignIn
        } else {
            Self::ApiKey
        }
    }

    /// `omitUnsupportedFields` (`openai-responses.ts:329`). Upstream's comment at `:328`: "Sign in
    /// with ChatGPT rejects these request fields."
    ///
    /// The four fields are `prompt_cache_retention` (`:335`), `prompt_cache_options` (`:336`),
    /// `max_output_tokens` (`:340-341`) and `temperature` (`:344-345`). `prompt_cache_key`
    /// (`:334`) and `service_tier` (`:349`) are **not** omitted — the set is exactly these four.
    pub(super) fn omit_unsupported_fields(self) -> bool {
        matches!(self, Self::ChatGptSignIn)
    }
}

/// The resolved request credential **and** its kind, produced together.
///
/// `Debug` is hand-written and redacts the key; see [`crate::auth::types::Credential`] for the same
/// rule on the stored side.
#[derive(Clone)]
pub(super) struct ResponsesCredential {
    api_key: String,
    kind: ResponsesTokenKind,
}

impl std::fmt::Debug for ResponsesCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponsesCredential")
            .field("api_key", &crate::auth::types::REDACTED)
            .field("kind", &self.kind)
            .finish()
    }
}

impl ResponsesCredential {
    /// Pi `getClientApiKey` (`openai-responses.ts:58-63`) + the WireProvider-resolved key, and the
    /// `isChatGPTSignIn` classification of whatever came out.
    ///
    /// A resolved key wins; otherwise an `authorization`/`cf-aig-authorization` header (from the
    /// auth or opts overlay) lets the key be the literal `"unused"`; otherwise `None` and the
    /// caller errors. The **only** constructor, by design: see [`ResponsesTokenKind`].
    /// **The classification reads the resolved credential, not the `"unused"` stand-in.** Upstream
    /// is explicit about this: `getClientApiKey(model.provider, options?.apiKey, options?.headers)`
    /// (`:195`) produces the string the client sends, but `isChatGPTSignIn(model, options?.apiKey)`
    /// (`:329`) tests `options.apiKey` — which is `undefined` when only a bearer header was
    /// supplied. A header-only request therefore sends the full body, and
    /// [`AuthResult::auth`]'s `api_key` is cyrup's `options.apiKey`.
    pub(super) fn resolve(model: &Model, auth: &AuthResult, opts: &StreamOptions) -> Option<Self> {
        let kind = ResponsesTokenKind::classify(model, auth.auth.api_key.as_deref());
        let api_key = resolve_api_key(auth, opts)?;
        Some(Self { api_key, kind })
    }

    pub(super) fn api_key(&self) -> &str {
        &self.api_key
    }

    pub(super) fn kind(&self) -> ResponsesTokenKind {
        self.kind
    }
}

/// Pi `getClientApiKey` (`openai-responses.ts:58-63`), without the classification. Private so the
/// key cannot be obtained unclassified; [`ResponsesCredential::resolve`] is the way in.
fn resolve_api_key(auth: &AuthResult, opts: &StreamOptions) -> Option<String> {
    if let Some(key) = &auth.auth.api_key {
        return Some(key.clone());
    }
    let has = |name: &str| {
        header_present(auth.auth.headers.as_ref(), name)
            || header_present(opts.headers.as_ref(), name)
    };
    if has("authorization") || has("cf-aig-authorization") {
        return Some("unused".to_string());
    }
    None
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
    use crate::model::{Modality, ModelCost};

    fn model_with(provider: &str, base_url: &str) -> Model {
        Model {
            id: "gpt-5-mini".into(),
            name: "GPT-5 Mini".into(),
            api: crate::known_api::OPENAI_RESPONSES.into(),
            provider: provider.into(),
            base_url: base_url.to_string(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            input_limits: None,
            prompt_cache: None,
            context_window: 400_000,
            max_tokens: 128_000,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    /// `isChatGPTSignIn` (`openai-responses.ts:40-47`): all three conditions are required.
    #[test]
    fn only_a_non_sk_credential_on_openais_own_base_url_is_a_sign_in_token() {
        let openai = model_with("openai", "https://api.openai.com/v1");
        assert_eq!(
            ResponsesTokenKind::classify(&openai, Some("chatgpt-access-token")),
            ResponsesTokenKind::ChatGptSignIn
        );
        // an api key
        assert_eq!(
            ResponsesTokenKind::classify(&openai, Some("sk-proj-test")),
            ResponsesTokenKind::ApiKey
        );
        // no credential at all (`apiKey !== undefined`)
        assert_eq!(
            ResponsesTokenKind::classify(&openai, None),
            ResponsesTokenKind::ApiKey
        );
        // another OpenAI-compatible endpoint, same provider id
        let gateway = model_with("openai", "https://gateway.example.com/v1");
        assert_eq!(
            ResponsesTokenKind::classify(&gateway, Some("gateway-key")),
            ResponsesTokenKind::ApiKey
        );
        // another provider on OpenAI's URL
        let other = model_with("openai-codex", "https://api.openai.com/v1");
        assert_eq!(
            ResponsesTokenKind::classify(&other, Some("chatgpt-access-token")),
            ResponsesTokenKind::ApiKey
        );
    }

    #[test]
    fn only_a_sign_in_token_omits_the_unsupported_fields() {
        assert!(ResponsesTokenKind::ChatGptSignIn.omit_unsupported_fields());
        assert!(!ResponsesTokenKind::ApiKey.omit_unsupported_fields());
    }

    /// The pairing is what makes the enum hold: the key that goes on the wire and the kind the body
    /// builder reads are the same value.
    #[test]
    fn resolve_pairs_the_key_with_its_classification() {
        let openai = model_with("openai", "https://api.openai.com/v1");
        let auth = AuthResult::from_key("chatgpt-access-token", "OAuth");
        let cred = ResponsesCredential::resolve(&openai, &auth, &StreamOptions::default()).unwrap();
        assert_eq!(cred.api_key(), "chatgpt-access-token");
        assert_eq!(cred.kind(), ResponsesTokenKind::ChatGptSignIn);

        let auth = AuthResult::from_key("sk-proj-test", "env");
        let cred = ResponsesCredential::resolve(&openai, &auth, &StreamOptions::default()).unwrap();
        assert_eq!(cred.kind(), ResponsesTokenKind::ApiKey);
    }

    /// The `"unused"` stand-in (`openai-responses.ts:60-61`) must **not** be classified: upstream
    /// tests `options?.apiKey` (`:329`), which is `undefined` on a header-only request, so the
    /// body keeps its four fields. Classifying the stand-in instead — it does not start with
    /// `sk-` — would silently strip `max_output_tokens` and `temperature` from every
    /// bearer-header request to `api.openai.com`.
    #[test]
    fn a_bearer_header_request_to_openai_is_not_a_sign_in_token() {
        let openai = model_with("openai", "https://api.openai.com/v1");
        let mut headers = crate::HeaderMap::new();
        headers.insert("authorization".to_string(), Some("Bearer t".to_string()));
        let auth = AuthResult {
            auth: crate::auth::ModelAuth {
                api_key: None,
                headers: Some(headers),
                base_url: None,
            },
            env: None,
            source: Some("test".to_string()),
        };
        let cred = ResponsesCredential::resolve(&openai, &auth, &StreamOptions::default()).unwrap();
        assert_eq!(cred.api_key(), "unused");
        assert_eq!(cred.kind(), ResponsesTokenKind::ApiKey);
    }

    #[test]
    fn debug_does_not_render_the_key() {
        let openai = model_with("openai", "https://api.openai.com/v1");
        let auth = AuthResult::from_key("sk-SECRET", "env");
        let cred = ResponsesCredential::resolve(&openai, &auth, &StreamOptions::default()).unwrap();
        let rendered = format!("{cred:?}");
        assert!(
            !rendered.contains("SECRET"),
            "ResponsesCredential Debug leaked the key: {rendered}"
        );
        assert!(rendered.contains("ApiKey"));
    }
}
