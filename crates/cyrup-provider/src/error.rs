//! Error taxonomy (arch-01 §8 / func-01 R-01-017).
//!
//! Two error channels, by design (arch-00 §3.1):
//! - *Introspection paths* (auth resolution, refresh) return `Result<_, ProviderError>` carrying the
//!   func-01 R-01-017 taxonomy codes.
//! - *Request/stream paths* never return `Err`: every failure is converted to a terminal
//!   [`StreamEvent::Error`] whose `AssistantMessage` carries `stopReason ∈ {error, aborted}`
//!   (func-01 R-01-018/044/045). [`ProviderError::into_error_event`] performs that conversion.

use crate::stream::StreamEvent;
use cyrup_core::{ApiId, AssistantMessage, ProviderId, StopReason};

/// Boxed source error for opaque underlying causes.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// Authentication / credential-store failure taxonomy (func-01 R-01-017).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// OAuth refresh failed; the stored credential is preserved for re-login (R-01-013). Code `oauth`.
    #[error("oauth refresh failed for {provider}")]
    OAuth {
        provider: ProviderId,
        #[source]
        cause: BoxErr,
    },
    /// Credential-store read/write failure. Code `auth`.
    #[error("credential store failure for {provider}")]
    Store {
        provider: ProviderId,
        #[source]
        cause: BoxErr,
    },
    /// API-key auth resolution failure. Code `auth`.
    #[error("api key auth failed for {provider}")]
    ApiKey {
        provider: ProviderId,
        #[source]
        cause: BoxErr,
    },
}

impl AuthError {
    /// The func-01 R-01-017 taxonomy code.
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::OAuth { .. } => "oauth",
            AuthError::Store { .. } | AuthError::ApiKey { .. } => "auth",
        }
    }

    pub fn oauth(provider: ProviderId, cause: impl Into<BoxErr>) -> Self {
        AuthError::OAuth {
            provider,
            cause: cause.into(),
        }
    }
    pub fn store(provider: ProviderId, cause: impl Into<BoxErr>) -> Self {
        AuthError::Store {
            provider,
            cause: cause.into(),
        }
    }
    pub fn api_key(provider: ProviderId, cause: impl Into<BoxErr>) -> Self {
        AuthError::ApiKey {
            provider,
            cause: cause.into(),
        }
    }
}

/// Provider-layer error taxonomy (func-01 R-01-017 + transport/decode for the request path).
///
/// Maps into [`cyrup_core::CoreError`] via the `Core` variant's `#[from]` so the shared
/// abort/serde/io vocabulary stays consistent (arch-00 §8).
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Unknown provider id. Code `provider`.
    #[error("unknown provider: {0}")]
    UnknownProvider(ProviderId),
    /// No API implementation registered for `model.api`. Code `stream`.
    #[error("no API implementation for {0}")]
    NoApiImpl(ApiId),
    /// Dynamic model-catalog refresh failed. Code `model_source`.
    #[error("model source refresh failed: {0}")]
    ModelSource(#[source] BoxErr),
    /// Non-2xx HTTP response from the vendor. Code `http`.
    ///
    /// `message` is the response body, trimmed and capped at
    /// [`MAX_PROVIDER_ERROR_BODY_CHARS`](crate::utils::error_body::MAX_PROVIDER_ERROR_BODY_CHARS)
    /// by the transport before it gets here — it reaches the transcript verbatim through
    /// [`into_error_message`](Self::into_error_message), so it must never be unbounded.
    #[error("http {status}: {message}")]
    Http { status: u16, message: String },
    /// A server-requested retry delay exceeded `StreamOptions.max_retry_delay_ms`, so the request
    /// failed immediately instead of sleeping (Pi `validateServerRetryDelayMs`,
    /// provider-retry.ts:36-48). Code `http`.
    ///
    /// Pi throws a bare `Error` here, and its catch block runs the result through
    /// `formatProviderError(normalizeProviderError(error))`, which — with no SDK status/body fields
    /// to probe — returns `error.message` unchanged. The `Display` is therefore the message alone,
    /// with no `"…: "` prefix, so `errorMessage` is byte-identical to Pi's. That wording is also
    /// what makes the turn-level classifier retry it: `"retry delay"` is one of
    /// [`crate::utils::retry`]'s retryable patterns (Pi `retry.ts:70-71`).
    #[error("{0}")]
    RetryDelay(String),
    /// Connection / TLS / request transport failure. Code `transport`.
    #[error("transport error: {0}")]
    Transport(#[source] BoxErr),
    /// Malformed SSE / vendor payload. Code `decode`.
    #[error("decode error: {0}")]
    Decode(String),
    /// The request was cancelled via the `CancelToken` (func-01 R-01-044). Code `aborted`.
    #[error("aborted")]
    Aborted,
    /// Auth/credential-store failure (carries its own R-01-017 code).
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// Shared core error (cancelled / serde / io).
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

impl ProviderError {
    /// The func-01 R-01-017 taxonomy code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            ProviderError::UnknownProvider(_) => "provider",
            ProviderError::NoApiImpl(_) => "stream",
            ProviderError::ModelSource(_) => "model_source",
            ProviderError::Http { .. } | ProviderError::RetryDelay(_) => "http",
            ProviderError::Transport(_) => "transport",
            ProviderError::Decode(_) => "decode",
            ProviderError::Aborted => "aborted",
            ProviderError::Auth(e) => e.code(),
            ProviderError::Core(_) => "core",
        }
    }

    /// `true` for the cancellation case (terminal `StopReason::Aborted` rather than `Error`).
    pub fn is_aborted(&self) -> bool {
        matches!(self, ProviderError::Aborted)
    }

    /// A best-effort clone preserving the variant + `Display`. [`ProviderError`] is intentionally not
    /// `Clone` (its boxed sources are not), so this reproduces an equivalent error — flattening
    /// opaque boxed causes to their message — for fanning a single deduplicated result out to
    /// concurrent awaiters (see [`crate::utils::refresh::RefreshDedup`]). The taxonomy [`code`] is
    /// preserved for every variant whose code is realizable without its original source; the two
    /// fully-opaque introspection variants (`Auth`/`Core`) collapse to `Decode` (they do not arise on
    /// the model-refresh fetch path).
    ///
    /// [`code`]: ProviderError::code
    #[must_use]
    pub fn reproduce(&self) -> ProviderError {
        match self {
            ProviderError::UnknownProvider(p) => ProviderError::UnknownProvider(p.clone()),
            ProviderError::NoApiImpl(a) => ProviderError::NoApiImpl(a.clone()),
            ProviderError::ModelSource(e) => ProviderError::ModelSource(e.to_string().into()),
            ProviderError::Http { status, message } => ProviderError::Http {
                status: *status,
                message: message.clone(),
            },
            ProviderError::RetryDelay(s) => ProviderError::RetryDelay(s.clone()),
            ProviderError::Transport(e) => ProviderError::Transport(e.to_string().into()),
            ProviderError::Decode(s) => ProviderError::Decode(s.clone()),
            ProviderError::Aborted => ProviderError::Aborted,
            ProviderError::Auth(e) => ProviderError::Decode(e.to_string()),
            ProviderError::Core(e) => ProviderError::Decode(e.to_string()),
        }
    }

    /// Build the terminal error `AssistantMessage` for this failure (func-01 R-01-045). Aborts use
    /// `StopReason::Aborted`; every other failure uses `StopReason::Error`. `api` is the producing
    /// wire-protocol id (Pi sets `output.api = model.api` even on the error path). The message is a
    /// valid, appendable assistant turn (func-01 R-01-046).
    pub fn into_error_message(
        &self,
        provider: ProviderId,
        model: &str,
        api: Option<ApiId>,
    ) -> AssistantMessage {
        let stop = if self.is_aborted() {
            StopReason::Aborted
        } else {
            StopReason::Error
        };
        AssistantMessage::errored(provider, model, api, stop, self.to_string())
    }

    /// Build the terminal [`StreamEvent::Error`] for this failure (func-01 R-01-018/045). This is the
    /// ONLY way request/stream failures reach a consumer — they are never thrown. The `reason`
    /// discriminant mirrors the message's `stop_reason` (Pi `{type:"error", reason, error}`).
    pub fn into_error_event(
        &self,
        provider: ProviderId,
        model: &str,
        api: Option<ApiId>,
    ) -> StreamEvent {
        let error = self.into_error_message(provider, model, api);
        // `into_error_message` always sets `stop_reason ∈ {error, aborted}`, so `terminal` routes to
        // the `error` terminal with the matching narrowed [`ErrorReason`].
        StreamEvent::terminal(error)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::stream::ErrorReason;

    #[test]
    fn taxonomy_codes() {
        assert_eq!(
            ProviderError::UnknownProvider("x".into()).code(),
            "provider"
        );
        assert_eq!(ProviderError::NoApiImpl("y".into()).code(), "stream");
        assert_eq!(
            ProviderError::Http {
                status: 500,
                message: "boom".into()
            }
            .code(),
            "http"
        );
        assert_eq!(ProviderError::Aborted.code(), "aborted");
        let ae = AuthError::oauth("p".into(), "nope");
        assert_eq!(ProviderError::from(ae).code(), "oauth");
    }

    #[test]
    fn aborted_maps_to_aborted_terminal() {
        let ev = ProviderError::Aborted.into_error_event("p".into(), "m", Some("test-api".into()));
        match ev {
            StreamEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.api.to_string(), "test-api");
                assert_eq!(error.error_message.as_deref(), Some("aborted"));
            }
            _ => panic!("expected error terminal"),
        }
    }

    #[test]
    fn http_maps_to_error_terminal() {
        let ev = ProviderError::Http {
            status: 503,
            message: "down".into(),
        }
        .into_error_event("p".into(), "m", None);
        match ev {
            StreamEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
            }
            _ => panic!("expected error terminal"),
        }
    }
}
