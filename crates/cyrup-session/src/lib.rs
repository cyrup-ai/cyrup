//! cyrup-session — sessions & branching (arch-04; conformance: func-04).
//!
//! An append-only JSONL session file whose entries form a tree (`id`/`parentId`); the current
//! position is a **leaf** and the LLM context is built by walking leaf→root. This layer owns the
//! file format + versioning/migration, the in-memory tree (id→entry / parent→children indexes),
//! context building (including consuming a `CompactionEntry`), the session-manager operations
//! (create/open/continue/fork/clone/ephemeral), atomic line-append + crash recovery, listing /
//! selection, and JSONL export/import.
//!
//! The defining invariant (DI-9): the on-disk record is **lossless** — branch navigation and
//! compaction prune only the *built context*, never the file.
//!
//! Compaction *generation* (arch-05) lives in [`compaction`]; system-prompt & context assembly
//! (arch-06) lives in [`prompt`].
#![forbid(unsafe_code)]

pub mod agent_message;
pub mod compaction;
pub mod context;
pub mod entry;
pub mod error;
pub mod git_paths;
pub mod header;
pub mod ids;
pub mod layout;
pub mod listing;
pub mod manager;
pub mod migrate;
pub mod prompt;
pub mod store;

pub use agent_message::{
    AgentMessage, BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage,
    CustomRoleMessage, MessageRole, convert_to_llm,
};
pub use compaction::{
    BranchSummarySettings, CompactionError, CompactionHooks, CompactionReason, CompactionSettings,
    Compactor, NoHooks, Summarizer, serialize_conversation,
};
pub use context::SessionContext;
pub use entry::{Entry, EntryBase, KnownEntry};
pub use error::SessionError;
pub use git_paths::{GitPaths, canonicalize_path, find_git_paths, resolve_path};
pub use header::{CURRENT_VERSION, SessionHeader};
pub use layout::{SessionLayout, SessionsRoot, encode_cwd};
pub use listing::{
    SessionInfo, SessionListProgress, SessionSelector, list, list_all, list_all_in_dir,
    list_all_with_progress, list_in_dir, newest_session, resolve,
};
pub use manager::{NewSessionOpts, SessionManager, TreeNode};
pub use prompt::{
    BeforeAgentStartHook, BeforeAgentStartInput, BeforeAgentStartOutput, ContextDiagnostic,
    ContextError, ContextFile, ContextFileLoader, ContextScope, ContextSnapshot, ContextStore,
    DEFAULT_SELECTED_TOOLS, DocsPointers, PromptInputs, ResolvedOverride, SkillPointer,
    SystemPromptBuilder, ToolPromptContribution, TrustQuery, apply_before_agent_start,
};
pub use store::{DiskStore, MemStore, SessionStore, flush_session_writes};

#[cfg(test)]
mod tests;
