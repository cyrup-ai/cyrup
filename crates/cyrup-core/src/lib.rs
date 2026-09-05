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
pub mod json;
pub mod keyed_lock;
pub mod lazy_args;
pub mod message;
pub mod shared_str;
pub mod tool;

pub use cancel::{CancelToken, RunCancel};
pub use constrained_sampling::{
    ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, StrictSampling,
    experimental_tool_sampling, experimental_tool_sampling_from,
};
pub use diagnostics::{
    AssistantMessageDiagnostic, DiagnosticCode, DiagnosticErrorInfo,
    append_assistant_message_diagnostic, create_assistant_message_diagnostic,
    create_assistant_message_diagnostic_from, extract_diagnostic_error, format_thrown_value,
};
pub use error::CoreError;
pub use event_stream::{Finalizing, FinalizingSink, FinalizingStream, finalizing_channel};
pub use keyed_lock::{Cancelled, KeyedAcquire, KeyedGuard, KeyedLockMap, KeyedLocks};
pub use lazy_args::LazyArgs;
pub use message::{
    AssistantMessage, Content, Cost, DeferredHandle, Message, ModelThinkingLevel, StopReason,
    TextPhase, TextSignatureV1, ThinkingLevel, ToolCall, UNRESOLVED_API, Usage,
};
pub use shared_str::SharedStr;
pub use tool::{
    ExecMode, TerminateHint, Tool, ToolError, ToolRenderKind, ToolResult, ToolUpdate,
    ToolUpdateSink,
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

    /// `serde_json/preserve_order` is on for the whole workspace, and stays on.
    ///
    /// The workspace `Cargo.toml` declares it on `serde_json` deliberately rather than inheriting
    /// it from `agent-client-protocol`'s non-optional edge, because two `cyrup-mcp` units compute a
    /// byte count that must match `JSON.stringify`'s and a `BTreeMap`-backed value sorts keys and
    /// changes it (`docs/gap-analysis/13h-mcp-tui.md`, twice). Left implicit, the ordering would
    /// depend on the shape of the build — `-p cyrup-mcp` alone would sort, `--workspace` would not.
    ///
    /// This test fails if the feature is ever dropped: the keys below are inserted in an order that
    /// is NOT alphabetical, so a `BTreeMap`-backed `serde_json::Map` reorders them. It asserts the
    /// map's behaviour rather than any one caller's output, so it keeps holding as callers change.
    #[test]
    fn preserve_order_is_declared_workspace_wide() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"type":"send","to":"peer","message":"hi","id":"m1"}"#)
                .expect("decodes");
        let keys: Vec<&str> = v
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["type", "to", "message", "id"],
            "serde_json::Map is not preserving insertion order — the workspace \
             `serde_json/preserve_order` feature has been dropped. See the comment on \
             `serde_json` in the workspace Cargo.toml before changing this."
        );
    }

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
        let b = ModelRef {
            provider: "anthropic".into(),
            api: None,
            model: "x".into(),
        };
        assert_ne!(a, b);
    }
}
