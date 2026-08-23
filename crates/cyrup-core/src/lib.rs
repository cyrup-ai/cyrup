//! cyrup-core — the shared substrate (see `spec/architecture/arch-00-overview-and-workspace.md` §3).
//!
//! Holds the load-bearing types every subsystem reuses: id newtypes, the message/content model,
//! the single `EventStream<T>` streaming primitive, the `CancelToken`/`RunCancel` cancellation
//! model, the runtime-facing `Tool` trait, and the cross-cutting error vocabulary. No I/O, no
//! tokio tasks of its own.
#![forbid(unsafe_code)]

use std::sync::Arc;

pub mod cancel;
pub mod constrained_sampling;
pub mod diagnostics;
pub mod error;
pub mod event_stream;
pub mod keyed_lock;
pub mod message;
pub mod tool;

pub use cancel::{CancelToken, RunCancel};
pub use constrained_sampling::{
    ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, StrictSampling,
};
pub use diagnostics::{
    append_assistant_message_diagnostic, create_assistant_message_diagnostic,
    create_assistant_message_diagnostic_from, extract_diagnostic_error, format_thrown_value,
    AssistantMessageDiagnostic, DiagnosticCode, DiagnosticErrorInfo,
};
pub use error::CoreError;
pub use event_stream::{
    finalizing_channel, Finalizing, FinalizingSink, FinalizingStream,
};
pub use keyed_lock::{Cancelled, KeyedGuard, KeyedLockMap, KeyedLocks};
pub use message::{
    AssistantMessage, Content, Cost, DeferredHandle, Message, ModelThinkingLevel, StopReason,
    TextPhase, TextSignatureV1, ThinkingLevel, ToolCall, Usage, UNRESOLVED_API,
};
pub use tool::{
    ExecMode, Tool, ToolError, ToolRenderKind, ToolResult, ToolUpdate, ToolUpdateSink,
};

/// The single streaming primitive used across provider, agent, and tools (arch-00 §3.1).
///
/// Transport failures are delivered AS terminal items of `T` (never in `Err`); see arch-00 §3.1
/// and func-01 R-01-018.
pub type EventStream<T> = std::pin::Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'static>>;

/// Newtype ids — never raw `String` (arch-00 §3.3). `Arc<str>`-backed for cheap clones.
macro_rules! str_id {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Arc<str>);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({:?})", stringify!($name), &self.0)
            }
        }
        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(Arc::from(s))
            }
        }
        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(Arc::from(s.as_str()))
            }
        }
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

str_id!(SessionId);
str_id!(EntryId);
str_id!(ToolCallId);
str_id!(ProviderId);
str_id!(ModelId);
str_id!(ExtensionId);
str_id!(ApiId); // wire-protocol id (anthropic-messages, openai-completions, …); reused by cyrup-provider.
str_id!(PackageId); // reused by cyrup-resources.

/// Provider+model address (arch-00 §3.3).
///
/// `api: Some(_)` is the handoff-equality key (func-01 R-01-029: same provider+api+model ⇒ no
/// thinking-block flattening); `api: None` is a session-local / user-facing selection whose `api`
/// is reconstructed at model-resolution time. Supersedes any per-doc `ModelRef2`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ModelRef {
    pub provider: ProviderId,
    pub api: Option<ApiId>,
    pub model: ModelId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrips_and_displays() {
        let p = ProviderId::from("anthropic");
        assert_eq!(p.as_str(), "anthropic");
        assert_eq!(p.to_string(), "anthropic");
    }

    #[test]
    fn model_ref_equality_distinguishes_api() {
        let a = ModelRef {
            provider: "anthropic".into(),
            api: Some("anthropic-messages".into()),
            model: "x".into(),
        };
        let b = ModelRef { provider: "anthropic".into(), api: None, model: "x".into() };
        assert_ne!(a, b);
    }
}
