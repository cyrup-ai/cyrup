//! cyrup-core — the shared substrate (see `spec/architecture/arch-00-overview-and-workspace.md` §3).
//!
//! Holds the load-bearing types every subsystem reuses: id newtypes, the message/content model,
//! the single `EventStream<T>` streaming primitive, the `CancelToken`/`RunCancel` cancellation
//! model, the runtime-facing `Tool` trait, and the cross-cutting error vocabulary. No I/O, no
//! tokio tasks of its own.
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use std::sync::Arc;

pub mod cancel;
pub mod constrained_sampling;
pub mod diagnostics;
pub mod error;
pub mod event_stream;
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
    finalizing_channel, EventStream, Finalizing, FinalizingSink, FinalizingStream,
};
pub use message::{
    AssistantMessage, Content, Cost, DeferredHandle, Message, ModelThinkingLevel, StopReason,
    TextPhase, TextSignatureV1, ThinkingLevel, ToolCall, Usage, UNRESOLVED_API,
};
pub use tool::{
    ExecMode, Tool, ToolError, ToolRenderKind, ToolResult, ToolUpdate, ToolUpdateSink,
};

/// Newtype ids — never raw `String` (arch-00 §3.3). `Arc<str>`-backed for cheap clones.
//
// LINT BLIND SPOT — keep this body free of fallible operations.
// The workspace no-panic policy (root Cargo.toml [workspace.lints.clippy]: `unwrap_used`,
// `expect_used`, `panic`, `indexing_slicing` = deny) does NOT reach inside `macro_rules!`
// expansions for three of its four lints. Verified on rustc 1.98.0 / clippy 0.1.98 by probing
// this macro body: `unwrap()`, `expect()` and `panic!` produced NO diagnostic at any of the
// eight `str_id!` invocations below, while the identical code in an ordinary `impl` block in
// this same file produced all three errors. Only `indexing_slicing` fired from the expansion
// (once per invocation). The gap is in clippy's from-expansion filtering, not in this crate's
// configuration — do not try to "fix" it by raising or re-configuring the deny levels.
// Consequence: code added here is effectively unlinted for panics, so anything fallible must
// be written to be infallible by construction and reviewed by hand.
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
