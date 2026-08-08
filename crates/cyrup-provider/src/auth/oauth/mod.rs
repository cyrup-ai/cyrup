//! OAuth login substrate — the shared core every per-provider login flow is built on
//! (arch-01 §3.7 / func-01 §7).
//!
//! Ports pi v0.83.0 `packages/ai/src/auth/oauth/`:
//!
//! | this module | pi source |
//! |---|---|
//! | [`pkce`] | `pkce.ts` (PKCE verifier/challenge) |
//! | [`load`] | `load.ts` (flow-module loading / bundled registration) |
//! | [`device_code`] | `device-code.ts` (RFC 8628 device-code poll loop) |
//! | [`page`] | `oauth-page.ts` (the browser-facing callback pages) |
//! | [`callback`] | the loopback callback listener shared verbatim by `openrouter.ts:135-243`, `anthropic.ts:97-170`, `openai-codex.ts:334-375` and `radius.ts:170-212` |
//! | [`interaction`] | `../types.ts:119-187` (`AuthPrompt`/`AuthEvent`/`AuthInteraction`) |
//! | [`query`] | `URLSearchParams` parse/serialize, which pi gets from the platform |
//! | [`sha256`], [`random`] | `crypto.subtle.digest` / `crypto.getRandomValues`, which pi gets from Web Crypto |
//!
//! ## Mechanism divergences (language-forced; behaviour is unchanged)
//!
//! * pi calls the ambient Web Crypto API. Rust has no ambient crypto and this crate's manifest
//!   carries no hashing/RNG dependency, so [`sha256`] is a self-contained SHA-256 (verified
//!   against the FIPS 180-4 vectors) and [`random`] reads the OS CSPRNG directly.
//! * pi's callback server is `node:http`. This crate depends on `tokio` without the `net`
//!   feature, so [`callback`] runs a `std::net::TcpListener` accept loop on a dedicated OS
//!   thread and re-enters the async world through `tokio::runtime::Handle::block_on`. Ports,
//!   paths, status codes, response bodies and settle/claim semantics are the upstream ones.
//! * pi's `generatePKCE` is `async` only because `crypto.subtle.digest` returns a promise; the
//!   Rust equivalent is synchronous.

pub mod anthropic;
pub mod callback;
pub mod device_code;
pub mod github_copilot;
pub mod interaction;
pub mod kimi_coding;
pub mod load;
pub mod openai_codex;
pub mod openrouter;
pub mod page;
pub mod pkce;
pub mod query;
pub mod radius;
pub mod random;
pub mod sha256;
pub mod xai;

pub use anthropic::*;
pub use callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig, DEFAULT_CALLBACK_HOST, callback_host,
};
pub use device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
pub use interaction::{
    AuthEvent, AuthInfoLink, AuthInteraction, AuthPrompt, AuthPromptKind, AuthSelectOption,
    ScriptedInteraction,
};
pub use load::{
    OAuthFlowFactory, OAuthFlowId, OAuthFlowLoaders, RadiusFlowFactory, RadiusOptions,
    load_anthropic_oauth, load_github_copilot_oauth, load_kimi_coding_oauth,
    load_openai_codex_oauth, load_openrouter_oauth, load_radius_oauth, load_xai_oauth,
    register_bundled_oauth_flow_loaders,
};
pub use page::{oauth_error_html, oauth_success_html};
pub use pkce::{Pkce, base64url_encode, generate_pkce, pkce_challenge};
pub use query::{encode_query, parse_query};
pub use random::{random_bytes, random_token, random_uuid_v4};

use crate::error::AuthError;
use cyrup_core::ProviderId;

/// `CANCEL_MESSAGE` (`device-code.ts:1`); the same literal is thrown by every callback flow on
/// abort (`openrouter.ts:220`, `anthropic.ts:275`).
pub const CANCEL_MESSAGE: &str = "Login cancelled";
/// `TIMEOUT_MESSAGE` (`device-code.ts:2`).
pub const TIMEOUT_MESSAGE: &str = "Device flow timed out";
/// `SLOW_DOWN_TIMEOUT_MESSAGE` (`device-code.ts:3-4`), verbatim.
pub const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";

/// A login-flow failure.
///
/// pi throws bare `Error`s whose *messages* are the contract (the TUI prints them, and
/// `device-code.ts` distinguishes its two timeouts only by message). This enum preserves those
/// messages exactly via [`std::fmt::Display`] while letting Rust callers match on the cause.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// The user (or the enclosing run) aborted the login — `throw new Error(CANCEL_MESSAGE)`
    /// (`device-code.ts:66`).
    #[error("{CANCEL_MESSAGE}")]
    Cancelled,

    /// The device-code deadline passed with no `slow_down` response (`device-code.ts:97`).
    #[error("{TIMEOUT_MESSAGE}")]
    DeviceFlowTimeout,

    /// The device-code deadline passed after at least one `slow_down` (`device-code.ts:97`).
    #[error("{SLOW_DOWN_TIMEOUT_MESSAGE}")]
    DeviceFlowSlowDownTimeout,

    /// A flow-supplied timeout, e.g. `"OpenRouter OAuth login timed out"` (`openrouter.ts:223`).
    #[error("{message}")]
    Timeout { message: String },

    /// A `{ status: "failed", message }` poll result (`device-code.ts:73`) or any other
    /// flow-level failure whose message is the contract.
    #[error("{0}")]
    Failed(String),

    /// The callback listener could not bind or accept.
    ///
    /// pi surfaces the raw `node:http` `error` event (`openrouter.ts:216`); Rust names the
    /// `std::io::Error` so callers can distinguish `AddrInUse` (another `cyrup login` is
    /// already listening on the flow's fixed port) from the rest.
    #[error("oauth callback server failed on {address}: {source}")]
    Listen {
        address: String,
        #[source]
        source: std::io::Error,
    },

    /// No OS entropy source is reachable, so no PKCE verifier can be generated.
    ///
    /// pi cannot hit this: `crypto.getRandomValues` is ambient in Node and the browser.
    #[error("no OS entropy source available for OAuth PKCE: {0}")]
    Entropy(String),

    /// A flow module was requested that nothing registered.
    ///
    /// pi reaches its flows through a dynamic `import()` (`load.ts:9-12`), which Rust has no
    /// equivalent of; an unregistered flow is the Rust shape of an import that resolves to
    /// nothing. See [`load`].
    #[error("OAuth flow \"{flow}\" is not registered")]
    FlowUnavailable { flow: &'static str },

    /// The strategy has no interactive login. pi models this by making `login` a required member
    /// of `OAuthAuth` (`../types.ts:196`) that only real flows implement.
    #[error("{name} does not support interactive OAuth login")]
    LoginUnsupported { name: String },
}

impl OAuthError {
    /// Fold a login failure into the crate-wide taxonomy (func-01 R-01-017, code `oauth`).
    pub fn into_auth_error(self, provider: impl Into<ProviderId>) -> AuthError {
        AuthError::OAuth {
            provider: provider.into(),
            cause: Box::new(self),
        }
    }
}

/// Build the canonical stored OAuth credential (`OAuthCredential`, `../types.ts:31-33`).
///
/// `expires` is Unix **milliseconds** — the `auth.json` contract pi writes as
/// `Date.now() + expires_in * 1000` (`anthropic.ts:225`); see [`crate::auth::types::Credential`].
pub fn oauth_credential(
    access: impl Into<String>,
    refresh: impl Into<String>,
    expires: i64,
) -> crate::auth::types::Credential {
    crate::auth::types::Credential::Oauth {
        refresh: refresh.into(),
        access: access.into(),
        expires,
        ext: serde_json::Map::new(),
    }
}

/// `Date.now()` in Unix milliseconds — the clock every flow stamps `expires` with.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    use crate::auth::types::{Credential, ModelAuth};
    use crate::auth::{OAuthAuth, ProviderAuth};
    use std::sync::Arc;

    #[test]
    fn error_messages_match_upstream_literals() {
        assert_eq!(OAuthError::Cancelled.to_string(), "Login cancelled");
        assert_eq!(
            OAuthError::DeviceFlowTimeout.to_string(),
            "Device flow timed out"
        );
        assert_eq!(
            OAuthError::DeviceFlowSlowDownTimeout.to_string(),
            "Device flow timed out after one or more slow_down responses. This is often caused by \
             clock drift in WSL or VM environments. Please sync or restart the VM clock and try \
             again."
        );
        assert_eq!(
            OAuthError::Failed("Device flow failed: access_denied".into()).to_string(),
            "Device flow failed: access_denied"
        );
    }

    #[test]
    fn into_auth_error_uses_the_oauth_taxonomy_code() {
        let err = OAuthError::Cancelled.into_auth_error("anthropic");
        assert_eq!(err.code(), "oauth");
        assert!(err.to_string().contains("anthropic"));
    }

    #[test]
    fn oauth_credential_serializes_as_pi_auth_json() {
        let cred = oauth_credential("acc", "ref", 1_700_000_000_000);
        let json = serde_json::to_value(&cred).unwrap();
        assert_eq!(json["type"], "oauth");
        assert_eq!(json["access"], "acc");
        assert_eq!(json["refresh"], "ref");
        assert_eq!(json["expires"], 1_700_000_000_000i64);
    }

    /// A minimal `OAuthAuth` that does nothing but inherit the trait defaults. Proves the login
    /// surface added to [`crate::auth::OAuthAuth`] is real and does not break existing
    /// implementors (`../types.ts:189-206`).
    struct NoLoginFlow;

    #[async_trait::async_trait]
    impl OAuthAuth for NoLoginFlow {
        fn name(&self) -> &str {
            "Test OAuth"
        }
        async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
            Ok(cred.clone())
        }
        async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, AuthError> {
            Ok(ModelAuth::default())
        }
    }

    /// A complete redirect-based login built only out of this module, reached through the real
    /// [`crate::auth::OAuthAuth::login`] entry point: PKCE, a loopback callback listener, the
    /// authorize URL, the success page, and a stored credential. It is the `openrouter.ts` /
    /// `radius.ts` shape with the token endpoint replaced by a local derivation, so no test ever
    /// touches the network.
    mod demo_flow {
        #![allow(
            clippy::unwrap_used,
            clippy::expect_used,
            clippy::panic,
            clippy::indexing_slicing
        )]

        use super::super::*;
        use crate::auth::types::{Credential, ModelAuth};
        use crate::auth::{AuthError, OAuthAuth};
        use std::sync::Arc;
        use std::time::Duration;

        struct DemoHandler {
            state: String,
        }

        #[async_trait::async_trait]
        impl callback::CallbackHandler for DemoHandler {
            type Value = String;

            async fn handle(
                &self,
                request: CallbackRequest,
                _control: CallbackControl,
            ) -> CallbackOutcome<String> {
                if request.param("state") != Some(self.state.as_str()) {
                    return CallbackOutcome::Continue {
                        reply: CallbackReply::error(400, "OAuth state mismatch.", None),
                    };
                }
                match request.param("code") {
                    Some(code) => CallbackOutcome::Complete {
                        reply: CallbackReply::success("Signed in. You may now close this page."),
                        value: code.to_string(),
                    },
                    None => CallbackOutcome::Continue {
                        reply: CallbackReply::error(400, "Missing authorization code.", None),
                    },
                }
            }
        }

        struct DemoFlow;

        #[async_trait::async_trait]
        impl OAuthAuth for DemoFlow {
            fn name(&self) -> &str {
                "Demo OAuth"
            }

            fn login_label(&self) -> Option<&str> {
                Some("Sign in with Demo")
            }

            async fn login(
                &self,
                interaction: &dyn AuthInteraction,
            ) -> Result<Credential, OAuthError> {
                let pkce = generate_pkce()?;
                let state = random_token(16)?;
                let server = CallbackServer::start(
                    CallbackServerConfig::ephemeral("/callback")
                        .with_interaction(interaction)
                        .with_timeout(Duration::from_secs(10), "Demo OAuth login timed out"),
                    DemoHandler {
                        state: state.clone(),
                    },
                )
                .await?;

                let authorize = format!(
                    "https://demo.invalid/authorize?{}",
                    encode_query([
                        ("client_id", "demo"),
                        ("redirect_uri", server.redirect_uri()),
                        ("state", state.as_str()),
                        ("code_challenge", pkce.challenge.as_str()),
                        ("code_challenge_method", "S256"),
                    ])
                );
                interaction.notify(AuthEvent::AuthUrl {
                    url: authorize,
                    instructions: None,
                });

                let code = server
                    .wait()
                    .await?
                    .ok_or_else(|| OAuthError::Failed("Missing authorization code".to_string()))?;
                // Stands in for the token endpoint: a real flow POSTs `code` + `code_verifier`.
                Ok(oauth_credential(
                    format!("access-for-{code}"),
                    format!("refresh-for-{}", pkce.verifier),
                    now_ms() + 3_600_000,
                ))
            }

            async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
                Ok(cred.clone())
            }

            async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
                let api_key = match cred {
                    Credential::Oauth { access, .. } => Some(access.clone()),
                    Credential::ApiKey { key, .. } => key.clone(),
                };
                Ok(ModelAuth {
                    api_key,
                    ..Default::default()
                })
            }
        }

        /// Pull `redirect_uri` back out of the authorize URL the flow published.
        fn redirect_uri_of(authorize: &str) -> String {
            let (_, query) = authorize
                .split_once('?')
                .expect("authorize URL has a query");
            parse_query(query)
                .into_iter()
                .find(|(k, _)| k == "redirect_uri")
                .map(|(_, v)| v)
                .expect("authorize URL carries redirect_uri")
        }

        fn browser_get(url: &str) -> String {
            let rest = url.strip_prefix("http://").expect("http URL");
            let (authority, path) = rest.split_once('/').expect("URL has a path");
            let mut stream =
                std::net::TcpStream::connect(authority).expect("connect to loopback listener");
            use std::io::{Read, Write};
            stream
                .write_all(
                    format!(
                        "GET /{path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .expect("write request");
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            response
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn login_through_the_oauth_auth_trait_yields_a_credential() {
            let flow = Arc::new(DemoFlow);
            let interaction = Arc::new(ScriptedInteraction::new(Vec::new()));

            let login = {
                let flow = Arc::clone(&flow);
                let interaction = Arc::clone(&interaction);
                tokio::spawn(async move { flow.login(interaction.as_ref()).await })
            };

            // Wait for the flow to publish its authorize URL, exactly as a TUI would.
            let authorize = loop {
                let found = interaction
                    .events()
                    .into_iter()
                    .find_map(|event| match event {
                        AuthEvent::AuthUrl { url, .. } => Some(url),
                        _ => None,
                    });
                if let Some(url) = found {
                    break url;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };

            assert!(authorize.contains("code_challenge_method=S256"));
            let redirect_uri = redirect_uri_of(&authorize);
            assert!(
                redirect_uri.starts_with("http://127.0.0.1:"),
                "listener must be loopback: {redirect_uri}"
            );
            let state = parse_query(authorize.split_once('?').unwrap().1)
                .into_iter()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v)
                .unwrap();

            let response = tokio::task::spawn_blocking(move || {
                browser_get(&format!("{redirect_uri}?code=auth-code-9&state={state}"))
            })
            .await
            .unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
            assert!(response.contains("<title>Authentication successful</title>"));

            let credential = login.await.unwrap().expect("login succeeds");
            match &credential {
                Credential::Oauth {
                    access,
                    refresh,
                    expires,
                    ..
                } => {
                    assert_eq!(access, "access-for-auth-code-9");
                    assert!(refresh.starts_with("refresh-for-"));
                    assert!(*expires > now_ms(), "expires is a future Unix-ms deadline");
                }
                Credential::ApiKey { .. } => panic!("login must produce an OAuth credential"),
            }

            // And the credential is usable through the same strategy object.
            let auth = flow.to_auth(&credential).await.unwrap();
            assert_eq!(auth.api_key.as_deref(), Some("access-for-auth-code-9"));
            assert_eq!(flow.login_label(), Some("Sign in with Demo"));
        }

        /// The same flow, aborted: `login` surfaces upstream's `"Login cancelled"` rather than
        /// hanging on the listener.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn aborted_login_reports_login_cancelled() {
            let token = cyrup_core::CancelToken::new();
            let interaction =
                Arc::new(ScriptedInteraction::new(Vec::new()).with_cancel(token.clone()));
            let login = {
                let interaction = Arc::clone(&interaction);
                tokio::spawn(async move { DemoFlow.login(interaction.as_ref()).await })
            };
            tokio::time::sleep(Duration::from_millis(30)).await;
            token.cancel();
            let err = match login.await.unwrap() {
                Ok(_) => panic!("an aborted login must not produce a credential"),
                Err(error) => error,
            };
            assert_eq!(err.to_string(), "Login cancelled");
        }
    }

    #[tokio::test]
    async fn default_login_reports_unsupported_and_login_label_is_none() {
        let flow = NoLoginFlow;
        assert!(flow.login_label().is_none());
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow.login(&interaction).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Test OAuth does not support interactive OAuth login"
        );
        // And it is still usable as the provider-level strategy object.
        let auth = ProviderAuth::with_oauth(Arc::new(NoLoginFlow));
        assert!(auth.is_configured());
    }
}
