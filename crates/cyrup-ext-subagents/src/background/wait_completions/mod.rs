//! Terminal completions for `wait` — the projector, the consumed-payload store, and the
//! three-rung resolution that keeps a completion visible even after its payload has been consumed
//! and deleted.
//!
//! Ports pi `runs/background/wait-completions.ts` (213 LOC).
//!
//! # Why the store exists
//!
//! The completion watcher deletes a payload once its delivery receipt lands
//! (`watch/install.rs`'s delete-last). A `wait` resolving immediately afterwards has no file left
//! to read at either of the two paths [`collect_wait_completions`] otherwise tries: the promoted
//! payload is gone and the session index that pointed at it is retired in the same operation. pi
//! closes exactly this gap by recording the projected completion at the moment it is observed,
//! before the unlink (`result-watcher.ts:428-435`). [`WaitCompletionStore`] is that record, wired
//! onto the same [`crate::background::watch::CompletionObserver`] seam the completion bus already
//! uses — see its own doc for the registration-order argument.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs       facade + this narrative; no logic
//! project.rs   WaitCompletion, WaitCompletionChild, CompletionUsage, CompletionProjectionError,
//!              to_wait_completion, project_structured_output, completion_usage
//! record.rs    WaitCompletionStore (+ its CompletionObserver impl)
//! collect.rs   collect_wait_completions — the three-rung resolution a wait reads through
//! ```
//!
//! # Tolerant by policy, strict at one seam
//!
//! Every payload here may have been written by a different build: [`to_wait_completion`] parses
//! raw [`serde_json::Value`], not a typed [`crate::background::ResultFile`], and degrades a
//! malformed individual field to absence rather than failing the whole read — a projector over
//! foreign JSON must stay tolerant. The one place tolerance stops is a corrupt or mismatched
//! `workflowChildren`, whose rejection propagates as [`CompletionProjectionError`].

mod collect;
mod project;
mod record;

pub use collect::collect_wait_completions;
pub use project::{
    CompletionProjectionError, CompletionUsage, WaitCompletion, WaitCompletionChild,
    completion_usage, project_structured_output, to_wait_completion,
};
pub use record::WaitCompletionStore;
