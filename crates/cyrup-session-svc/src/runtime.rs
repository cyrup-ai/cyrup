//! `AgentSessionRuntime` — owns the active [`AgentSession`] + its cwd-bound services and replaces
//! them atomically on `new_session`/`switch_session`/`fork`/`dispose` (arch-11 §3.4; Pi
//! `agent-session-runtime.ts:74-398`). Every replacement follows the same protocol: emit a *before*
//! extension event that may veto, tear down the current session (`session_shutdown` + dispose),
//! invalidate prior subscriptions (terminal `SessionReplaced`, R-11-021), rebuild the next session
//! via the stored [`SessionFactory`] (recreating cwd-bound services), apply it, bump the generation
//! `watch`, and emit `session_start`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::{HostEvent, Reduced};
use cyrup_session::manager::SessionManager;
use tokio::sync::{watch, RwLock};

use crate::builder::SessionTarget;
use crate::error::SessionServiceError;
use crate::factory::SessionFactory;
use crate::session::{AgentSession, ForkPosition};

/// The result of a replacement op (Pi `{cancelled}`, agent-session-runtime.ts:200).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwitchResult {
    /// `true` when a `session_before_*` handler vetoed the replacement (the session is unchanged).
    pub cancelled: bool,
}

/// The result of an entry-anchored runtime fork (Pi `{cancelled, selectedText}`).
#[derive(Clone, Debug, Default)]
pub struct RuntimeForkResult {
    pub cancelled: bool,
    pub selected_text: Option<String>,
}

struct RuntimeInner {
    session: Arc<AgentSession>,
    generation: u64,
}

/// Owns the active session + rebuilds it on every cwd/session switch (arch-11 §3.4).
pub struct AgentSessionRuntime {
    factory: Arc<SessionFactory>,
    inner: RwLock<RuntimeInner>,
    gen_tx: watch::Sender<u64>,
}

impl AgentSessionRuntime {
    /// Build the initial runtime from a `factory` + initial `target` (Pi `createAgentSessionRuntime`,
    /// agent-session-runtime.ts:406).
    pub async fn create(
        factory: Arc<SessionFactory>,
        target: SessionTarget,
    ) -> Result<Self, SessionServiceError> {
        let session = Arc::new(factory.build(target, None).await?);
        let (gen_tx, _rx) = watch::channel(0);
        Ok(Self { factory, inner: RwLock::new(RuntimeInner { session, generation: 0 }), gen_tx })
    }

    /// The active session (cheap `Arc` clone). Re-read after any replacement.
    pub async fn session(&self) -> Arc<AgentSession> {
        self.inner.read().await.session.clone()
    }

    /// The current replacement generation (bumped on every successful switch/fork/new).
    pub async fn generation(&self) -> u64 {
        self.inner.read().await.generation
    }

    /// The active session's model-restore fallback warning, re-surfaced at the runtime tier (Pi
    /// `AgentSessionRuntime.modelFallbackMessage` getter, agent-session-runtime.ts:113). `None` when
    /// the resumed model resolved cleanly.
    pub async fn model_fallback_message(&self) -> Option<String> {
        self.inner.read().await.session.model_fallback_message().map(str::to_string)
    }

    /// Observe replacement generations; a change means the active session was swapped and any held
    /// subscription is stale (R-11-021).
    pub fn watch_generation(&self) -> watch::Receiver<u64> {
        self.gen_tx.subscribe()
    }

    /// Whether a `session_before_*` handler vetoes the replacement (Pi `emitBeforeSwitch`/Fork).
    async fn vetoed(&self, current: &AgentSession, event: HostEvent) -> bool {
        let cancel = CancelToken::new();
        let reduced =
            current.ext_host().dispatcher().dispatch_block_mutate(event, &cancel).await;
        matches!(reduced, Reduced::Blocked { .. })
    }

    /// Tear down `current`, invalidate its subscriptions, install `next`, bump the generation, and
    /// emit `session_start` on the new session (the shared replacement tail).
    async fn install(
        &self,
        next: Arc<AgentSession>,
        reason: &str,
        previous_session_file: Option<String>,
    ) {
        // Tear down the outgoing session: `session_shutdown` to its streams + extensions.
        let new_gen = {
            let g = self.inner.read().await;
            g.generation + 1
        };
        {
            let current = self.session().await;
            current.dispose(reason).await;
            // Invalidate prior subscriptions with a terminal `SessionReplaced` (R-11-021).
            current.notify_replaced(new_gen).await;
        }
        // Install + bump generation.
        {
            let mut g = self.inner.write().await;
            g.session = next.clone();
            g.generation = new_gen;
        }
        let _ = self.gen_tx.send(new_gen);
        // Announce the new session.
        next.emit_session_start(reason, previous_session_file).await;
    }

    /// Start a fresh session in the same cwd (Pi `newSession`, agent-session-runtime.ts:223).
    pub async fn new_session(&self) -> Result<SwitchResult, SessionServiceError> {
        let current = self.session().await;
        if self.vetoed(&current, HostEvent::SessionBeforeSwitch { target_id: String::new() }).await {
            return Ok(SwitchResult { cancelled: true });
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        drop(current);
        let next = Arc::new(self.factory.build(SessionTarget::New, None).await?);
        self.install(next, "new", previous).await;
        Ok(SwitchResult { cancelled: false })
    }

    /// Resume a session file, recreating cwd-bound services for its cwd (Pi `switchSession`,
    /// agent-session-runtime.ts:193). Asserts the resumed session's cwd still exists (#42,
    /// `MissingSessionCwd`).
    pub async fn switch_session(
        &self,
        path: impl Into<PathBuf>,
    ) -> Result<SwitchResult, SessionServiceError> {
        let path = path.into();
        let current = self.session().await;
        let target_id = path.display().to_string();
        if self.vetoed(&current, HostEvent::SessionBeforeSwitch { target_id }).await {
            return Ok(SwitchResult { cancelled: true });
        }
        // Pre-flight: peek the resumed session's cwd and assert it still exists BEFORE teardown.
        let cwd = {
            let mgr = SessionManager::open(&path)?;
            mgr.cwd().to_path_buf()
        };
        if !cwd.exists() {
            return Err(SessionServiceError::MissingSessionCwd(cwd.display().to_string()));
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        drop(current);
        let next =
            Arc::new(self.factory.build(SessionTarget::Resume(path), Some(cwd)).await?);
        self.install(next, "resume", previous).await;
        Ok(SwitchResult { cancelled: false })
    }

    /// Fork at an entry, then switch the runtime to the new branched session (Pi `fork`,
    /// agent-session-runtime.ts:259). `position:"before"` returns the anchor's `selected_text`.
    pub async fn fork(
        &self,
        entry: cyrup_core::EntryId,
        position: ForkPosition,
    ) -> Result<RuntimeForkResult, SessionServiceError> {
        let current = self.session().await;
        let veto =
            self.vetoed(&current, HostEvent::SessionBeforeFork { entry_id: entry.to_string() }).await;
        if veto {
            return Ok(RuntimeForkResult { cancelled: true, selected_text: None });
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        let session_file = current.session_file().await;
        let cwd = current.services().cwd.clone();
        drop(current);

        // Mirror Pi (agent-session-runtime.ts:287-324): for a persisted session, open a throwaway
        // manager from the current file, branch it in place at the resolved leaf, and hand THAT
        // manager object to the factory (its on-disk write may still be deferred). For an in-memory
        // session, or a "fork before the first message", fall back to a fresh session.
        let (next, selected_text) = match session_file {
            Some(file) => {
                let mut mgr = SessionManager::open(&file)?;
                let (target_leaf, selected_text) =
                    crate::session::fork_anchor(&mgr, &entry, position)?;
                match target_leaf {
                    Some(leaf) => {
                        let root =
                            file.parent().map(Path::to_path_buf).unwrap_or_else(|| cwd.clone());
                        let layout = cyrup_session::SessionLayout::new(root, cwd);
                        mgr.create_branched_session(&leaf, &layout)?;
                        (Arc::new(self.factory.build_from_manager(mgr).await?), selected_text)
                    }
                    // Fork before the first message: a brand-new session.
                    None => (Arc::new(self.factory.build(SessionTarget::New, None).await?), selected_text),
                }
            }
            None => (Arc::new(self.factory.build(SessionTarget::New, None).await?), None),
        };
        self.install(next, "fork", previous).await;
        Ok(RuntimeForkResult { cancelled: false, selected_text })
    }

    /// Import a session JSONL file and switch the runtime to it (Pi `importFromJsonl`,
    /// agent-session-runtime.ts:353-388). The file is copied into the factory's sessions dir, the
    /// `session_before_switch` veto is offered, the imported session's cwd is asserted to exist, and
    /// the runtime is replaced via the standard teardown→install protocol. Errors with
    /// [`SessionServiceError::ImportFileNotFound`] when the source path does not exist.
    pub async fn import_from_jsonl(
        &self,
        input_path: impl Into<PathBuf>,
        cwd_override: Option<PathBuf>,
    ) -> Result<SwitchResult, SessionServiceError> {
        let resolved = input_path.into();
        if !resolved.exists() {
            return Err(SessionServiceError::ImportFileNotFound(resolved.display().to_string()));
        }
        let session_dir = self.factory.session_dir();
        std::fs::create_dir_all(&session_dir)
            .map_err(|e| SessionServiceError::Io(e.to_string()))?;
        let file_name = resolved
            .file_name()
            .ok_or_else(|| SessionServiceError::ImportFileNotFound(resolved.display().to_string()))?;
        let destination = session_dir.join(file_name);

        let current = self.session().await;
        let target_id = destination.display().to_string();
        if self.vetoed(&current, HostEvent::SessionBeforeSwitch { target_id }).await {
            return Ok(SwitchResult { cancelled: true });
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        drop(current);

        // Copy the source into the sessions dir (skip when it is already the destination).
        if std::fs::canonicalize(&destination).ok() != std::fs::canonicalize(&resolved).ok() {
            std::fs::copy(&resolved, &destination)
                .map_err(|e| SessionServiceError::Io(e.to_string()))?;
        }

        // Resolve the imported session's cwd and assert it still exists BEFORE teardown (#42).
        let cwd = match cwd_override {
            Some(c) => c,
            None => SessionManager::open(&destination)?.cwd().to_path_buf(),
        };
        if !cwd.exists() {
            return Err(SessionServiceError::MissingSessionCwd(cwd.display().to_string()));
        }
        let next =
            Arc::new(self.factory.build(SessionTarget::Resume(destination), Some(cwd)).await?);
        self.install(next, "resume", previous).await;
        Ok(SwitchResult { cancelled: false })
    }

    /// Dispose the runtime (Pi `dispose`, agent-session-runtime.ts:390): `session_shutdown{quit}` +
    /// dispose the active session. The runtime is unusable afterward.
    pub async fn dispose(&self) {
        self.session().await.dispose("quit").await;
    }

    /// The factory's base cwd.
    pub fn cwd(&self) -> &Path {
        self.factory.cwd()
    }
}
