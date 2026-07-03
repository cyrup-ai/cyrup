//! [`SdkError`] — the embedder-facing error type (arch-11 §8).
//!
//! A thin, stable wrapper over [`cyrup_session_svc::SessionServiceError`]. The SDK is a
//! publication boundary, not new behaviour, so this type mostly forwards the facade's aggregate
//! error; embedders match on it (or its [`std::error::Error::source`]) without depending on every
//! internal crate.

/// The error returned by every fallible [`crate`] operation.
///
/// # Examples
/// ```
/// use cyrup_sdk::SdkError;
///
/// fn is_streaming_conflict(err: &SdkError) -> bool {
///     matches!(
///         err,
///         SdkError::Session(cyrup_sdk::SessionServiceError::StreamingNeedsBehavior)
///     )
/// }
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SdkError {
    /// An error from the wrapped [`cyrup_session_svc::AgentSession`] facade.
    #[error(transparent)]
    Session(#[from] cyrup_session_svc::SessionServiceError),
    /// Zero-config provider construction failed — the model pattern named no built-in provider
    /// ([`crate::zero_config_provider`]/[`crate::CyrupBuilder::build_session_auto`]).
    #[error("provider construction failed: {0}")]
    Provider(String),
}

/// A convenience result alias for SDK operations.
///
/// # Examples
/// ```
/// use cyrup_sdk::SdkResult;
///
/// fn label() -> SdkResult<&'static str> {
///     Ok("ready")
/// }
/// # assert_eq!(label().unwrap(), "ready");
/// ```
pub type SdkResult<T> = Result<T, SdkError>;
