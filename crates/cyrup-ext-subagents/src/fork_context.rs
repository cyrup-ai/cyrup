//! Fork-context: the CANONICAL design (sole owner; arch-SA §6.6). `exec/`, `spawn/`, and
//! `background/` all call into this module's `ForkContextResolver`, never re-derive it.
//!
//! Fork-context is a plain, direct, synchronous Cargo dependency of this crate on
//! `cyrup-session` (never routed through the `cyrup-ext` capability system): branching happens
//! via `cyrup_session::SessionManager::create_branched_session` on a THROWAWAY handle opened
//! against the parent's persisted file (`SessionManager::open`) — the orchestrator's live,
//! in-memory session manager is never mutated in place by this call (R-SA-139/DI-SA-6).
//!
//! Three corrections against the architecture document's illustrative code, verified live
//! against the real `cyrup-session` source (`crates/cyrup-session/src/manager.rs`):
//!
//! 1. There is no `SessionManager::clone_at` method anywhere in `cyrup-session`. The real
//!    primitive is `create_branched_session(&mut self, leaf_id: &EntryId, layout: &SessionLayout)
//!    -> Result<Option<PathBuf>, SessionError>` (around `manager.rs:201`).
//! 2. There is no `SessionManager::persisted_path()` accessor. The real accessor is
//!    `session_file(&self) -> Option<&Path>`.
//! 3. `create_branched_session` returns `Ok(None)` (not an error) when the branch is created on
//!    an in-memory (never-persisted) session, AND when a persisted session's branched path has no
//!    assistant message yet (the write is deferred until the first assistant append, mirroring
//!    the parent's own deferred-flush semantics — see `manager.rs:294-303`). Both cases are
//!    "success but no path" from the primitive's point of view; this resolver treats a `None`
//!    result as `SubagentError::ForkFailed` since fork-context (R-SA-137/DI-SA-2) requires a
//!    concrete `session_file_path` to hand to the spawned child's `--session` argument — a
//!    fork-context resolution that produces no path is, by definition, not usable and MUST fail
//!    hard rather than silently downgrading to fresh context.
//!
//! Lineage provenance (R-SA-143) requires no additional code in this module:
//! `create_branched_session` itself records `parentSession` on the forked child's header
//! (`manager.rs:276-283`) whenever branching a persisted parent, which is the only case this
//! resolver ever reaches (the persisted-parent precondition is enforced below, before branching).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use cyrup_core::EntryId;
use cyrup_session::{SessionLayout, SessionManager};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SubagentError;

/// Whether a subagent run continues the parent conversation's session state (`Fork`) or starts
/// from a blank slate (`Fresh`). Canonical home: this module (arch-SA §6.6) — `discovery/types.rs`
/// (`AgentDefinition::default_context`), `exec/mod.rs` (`RunOptions::context`), and
/// `tui/mod.rs` (`SubagentProgressSnapshot::context`) all reference `crate::fork_context::ContextMode`
/// rather than re-declaring it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Start the subagent with no inherited conversation state (the default).
    #[default]
    Fresh,
    /// Branch the parent session's current leaf into a new, independent session file and hand
    /// that file's path to the child (R-SA-137/138/139).
    Fork,
}

/// The resolved outcome of a fork-context request: either `Fresh` (no session file), or `Fork`
/// with a concrete, on-disk session-file path ready to hand to a spawned child via `--session`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkContext {
    pub mode: ContextMode,
    pub session_file_path: Option<PathBuf>,
}

impl ForkContext {
    /// The `Fresh` outcome: no session file, nothing to branch.
    pub fn fresh() -> Self {
        Self {
            mode: ContextMode::Fresh,
            session_file_path: None,
        }
    }
}

/// Resolves `context: "fork"` requests into a concrete, branched session-file path — the sole
/// owner of fork-context logic in this crate (arch-SA §6.6). Every subsystem that needs
/// fork-context (`exec/` foreground execution, `spawn/` the OS-subprocess boundary, `background/`
/// hand-off to the detached runner) calls [`ForkContextResolver::resolve`]; none re-derive any
/// part of this algorithm.
///
/// Per-index caching (`cached`) makes `resolve` idempotent across repeated calls for the same
/// batch-step index — required by the eager whole-batch validation algorithm (R-SA-137, §6.6):
/// `SubagentExecutor::plan_batch()` resolves every step's `ForkContext` up front, and a later
/// re-resolution for the same index (e.g. a retry path) MUST return the same branched session
/// file rather than creating a second, divergent branch.
pub struct ForkContextResolver {
    /// The orchestrator's LIVE session manager — read from only to obtain the current leaf id
    /// and confirm persistence. Branching itself NEVER touches this handle's in-memory state
    /// (R-SA-139/DI-SA-6); see [`ForkContextResolver::resolve`]'s throwaway-handle step.
    manager: Arc<AsyncMutex<SessionManager>>,
    layout: SessionLayout,
    cached: StdMutex<HashMap<u32, PathBuf>>,
}

impl ForkContextResolver {
    /// Construct a resolver over the orchestrator's live session manager and the session-file
    /// layout to branch new sessions into (normally the same layout the parent session itself
    /// was created/opened with, so the branched file lands alongside its parent).
    pub fn new(manager: Arc<AsyncMutex<SessionManager>>, layout: SessionLayout) -> Self {
        Self {
            manager,
            layout,
            cached: StdMutex::new(HashMap::new()),
        }
    }

    /// Resolve one batch-step's requested context mode into a concrete [`ForkContext`].
    ///
    /// Fails fast (R-SA-137/DI-SA-2): NEVER falls back to `Fresh` when `Fork` was requested and
    /// branching cannot proceed. Callers (notably `exec::plan_batch`) MUST resolve every step in
    /// a batch before spawning any child process for that batch, so a later step's fork failure
    /// is discovered before any earlier step's subprocess has started.
    pub async fn resolve(
        &self,
        requested: ContextMode,
        index: u32,
    ) -> Result<ForkContext, SubagentError> {
        if requested != ContextMode::Fork {
            return Ok(ForkContext::fresh());
        }

        if let Some(cached_path) = self
            .cached
            .lock()
            .map_err(|_| SubagentError::ForkFailed)?
            .get(&index)
        {
            return Ok(ForkContext {
                mode: ContextMode::Fork,
                session_file_path: Some(cached_path.clone()),
            });
        }

        // Read only the current leaf id and the persisted-file path from the LIVE parent
        // manager; the guard is dropped at the end of this block, before any branching happens,
        // so branching never observes (or mutates) the live manager's in-memory state
        // (R-SA-139/DI-SA-6).
        let (leaf, persisted_path) = {
            let guard = self.manager.lock().await;
            if !guard.is_persisted() {
                return Err(SubagentError::ForkRequiresPersistedParent);
            }
            let leaf: EntryId = guard
                .leaf_id()
                .cloned()
                .ok_or(SubagentError::ForkRequiresLeaf)?;
            // `is_persisted()` was true above, so `session_file()` is expected to be `Some`; a
            // `None` here would mean the store reports itself persisted without a backing path,
            // an internal inconsistency this resolver treats identically to "not persisted"
            // rather than panicking or indexing into an absent value.
            let persisted_path = guard
                .session_file()
                .ok_or(SubagentError::ForkRequiresPersistedParent)?
                .to_path_buf();
            (leaf, persisted_path)
        };

        // Open a THROWAWAY handle on the parent's PERSISTED file on disk — never the live
        // manager. This is a brand-new `SessionManager` instance, used exactly once for this one
        // branch call, then dropped; it never becomes "the" session manager for anything, and the
        // orchestrator's own live manager is left completely untouched (R-SA-139/DI-SA-6).
        let mut throwaway = SessionManager::open(&persisted_path)?;
        let branched_path = throwaway
            .create_branched_session(&leaf, &self.layout)?
            .ok_or(SubagentError::ForkFailed)?;
        drop(throwaway);

        self.cached
            .lock()
            .map_err(|_| SubagentError::ForkFailed)?
            .insert(index, branched_path.clone());

        Ok(ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(branched_path),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::Path;

    use cyrup_core::{AssistantMessage, Content, Message, StopReason, Usage};
    use cyrup_session::NewSessionOpts;

    use super::*;

    fn layout(root: &Path, cwd: &Path) -> SessionLayout {
        SessionLayout::new(root.to_path_buf(), cwd.to_path_buf())
    }

    fn user(s: &str) -> Message {
        Message::User {
            content: vec![Content::text(s)],
            timestamp: 0,
        }
    }

    fn assistant(s: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![Content::text(s)],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
    }

    /// A real, persisted parent session (tempdir-backed, on-disk JSONL — never mocked) branches
    /// successfully: `resolve(Fork, _)` produces a genuine new session file on disk, distinct from
    /// the parent's file, and the parent's own live manager and its on-disk file are both left
    /// completely untouched (R-SA-139/DI-SA-6).
    #[tokio::test]
    async fn fork_resolve_produces_a_real_new_session_file_without_mutating_the_live_parent() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/fork-context-test");
        let lay = layout(root.path(), &cwd);

        let mut parent = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        parent.append_message(user("hello")).expect("append user");
        parent
            .append_message(assistant("hi there"))
            .expect("append assistant");
        let parent_path = parent
            .session_file()
            .expect("parent persisted")
            .to_path_buf();
        let parent_leaf_before = parent.leaf_id().cloned().expect("parent has a leaf");
        let parent_entry_count_before = parent.entries().len();

        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay.clone());

        let resolved = resolver
            .resolve(ContextMode::Fork, 0)
            .await
            .expect("fork resolves");
        assert_eq!(resolved.mode, ContextMode::Fork);
        let forked_path = resolved
            .session_file_path
            .expect("fork produces a session file path");

        // A real, distinct file exists on disk with real JSONL content.
        assert!(
            forked_path.exists(),
            "branched session file must exist on disk"
        );
        assert_ne!(
            forked_path, parent_path,
            "branched file must differ from the parent's file"
        );
        let forked_contents = std::fs::read_to_string(&forked_path).expect("read forked file");
        assert!(
            forked_contents.lines().count() >= 2,
            "forked file must contain a real header + entries"
        );

        // Lineage provenance (R-SA-143): the forked file's header records the parent's path.
        let reopened = SessionManager::open(&forked_path).expect("reopen forked session");
        assert_eq!(
            reopened.header().parent_session.as_deref(),
            Some(parent_path.to_string_lossy().as_ref()),
            "forked session header must record parentSession provenance"
        );

        // The live parent handle held by this resolver was NEVER mutated in place: same leaf,
        // same entry count, same file identity (R-SA-139/DI-SA-6).
        let guard = manager.lock().await;
        assert_eq!(
            guard.leaf_id().cloned(),
            Some(parent_leaf_before),
            "live parent leaf unchanged"
        );
        assert_eq!(
            guard.entries().len(),
            parent_entry_count_before,
            "live parent entries unchanged"
        );
        assert_eq!(
            guard.session_file().map(|p| p.to_path_buf()),
            Some(parent_path.clone()),
            "live parent still points at its own original file"
        );
        drop(guard);

        // On-disk parent file is untouched: reopening it independently yields the same content.
        let parent_reopened = SessionManager::open(&parent_path).expect("reopen parent file");
        assert_eq!(parent_reopened.entries().len(), parent_entry_count_before);
    }

    /// `ContextMode::Fresh` never touches the session manager or filesystem at all.
    #[tokio::test]
    async fn fresh_resolve_never_touches_the_session_manager() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/fresh-context-test");
        let lay = layout(root.path(), &cwd);

        // An UNPERSISTED in-memory manager: if `resolve` incorrectly touched it for a `Fresh`
        // request, this would panic/error since it has no leaf and is not persisted.
        let manager = SessionManager::in_memory(&cwd, NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let resolved = resolver
            .resolve(ContextMode::Fresh, 0)
            .await
            .expect("fresh always resolves");
        assert_eq!(resolved, ForkContext::fresh());
    }

    /// Fail-hard behavior (R-SA-137/DI-SA-2): an UNPERSISTED parent session (no file ever
    /// written) requesting `Fork` MUST return `ForkRequiresPersistedParent` — never silently
    /// downgrade to `Fresh`, and never reach `create_branched_session` at all (verified here by
    /// the fact that no session file appears anywhere under `root` after the call).
    #[tokio::test]
    async fn fork_on_unpersisted_parent_fails_hard_without_calling_create_branched_session() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/unpersisted-test");
        let lay = layout(root.path(), &cwd);

        // In-memory session: never persisted, `is_persisted()` is false.
        let manager = SessionManager::in_memory(&cwd, NewSessionOpts::default())
            .expect("create in-memory session");
        assert!(
            !manager.is_persisted(),
            "precondition: in-memory session is not persisted"
        );

        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let err = resolver
            .resolve(ContextMode::Fork, 0)
            .await
            .expect_err("fork against an unpersisted parent must fail hard");
        assert!(
            matches!(err, SubagentError::ForkRequiresPersistedParent),
            "expected ForkRequiresPersistedParent, got: {err:?}"
        );

        // No session file was ever created anywhere under the sessions root — proof that
        // `create_branched_session` (and even `SessionManager::open`) was never reached.
        let any_files_created = std::fs::read_dir(root.path())
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(
            !any_files_created,
            "no filesystem state should be created when fork fails hard pre-branch"
        );
    }

    /// Fail-hard behavior (R-SA-137/DI-SA-2): a PERSISTED parent session with NO resolvable leaf
    /// (a freshly-created session with zero appended messages — `leaf_id()` is `None`) requesting
    /// `Fork` MUST return `ForkRequiresLeaf` — never silently downgrade to `Fresh`.
    #[tokio::test]
    async fn fork_with_no_resolvable_leaf_fails_hard() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/no-leaf-test");
        let lay = layout(root.path(), &cwd);

        // A brand-new session with zero entries has no leaf, regardless of persistence.
        let manager = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        assert!(
            manager.leaf_id().is_none(),
            "precondition: fresh session has no leaf"
        );

        let manager = Arc::new(AsyncMutex::new(manager));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let err = resolver
            .resolve(ContextMode::Fork, 0)
            .await
            .expect_err("fork against a leafless parent must fail hard");
        // A leafless session is also not-yet-persisted in this implementation (the file write is
        // deferred until the first assistant message), so either fail-hard variant is acceptable
        // here as long as it is NOT a silent Fresh downgrade and NOT a success.
        assert!(
            matches!(
                err,
                SubagentError::ForkRequiresLeaf | SubagentError::ForkRequiresPersistedParent
            ),
            "expected a fail-hard fork error, got: {err:?}"
        );
    }

    /// Repeated resolution for the SAME batch-step index returns the SAME branched session file
    /// (idempotent caching), rather than creating a second, divergent branch on every call.
    #[tokio::test]
    async fn resolve_is_idempotent_per_index() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/idempotent-test");
        let lay = layout(root.path(), &cwd);

        let mut parent = SessionManager::create(&cwd, &lay, NewSessionOpts::default())
            .expect("create parent session");
        parent.append_message(user("hello")).expect("append user");
        parent
            .append_message(assistant("hi there"))
            .expect("append assistant");

        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(Arc::clone(&manager), lay);

        let first = resolver
            .resolve(ContextMode::Fork, 7)
            .await
            .expect("first resolve");
        let second = resolver
            .resolve(ContextMode::Fork, 7)
            .await
            .expect("second resolve");
        assert_eq!(first.session_file_path, second.session_file_path);
    }
}
