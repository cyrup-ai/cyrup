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

/// A diagnostic collected while building the active session (Pi `AgentSessionRuntimeDiagnostic`,
/// agent-session-services.ts:78). Surfaced to the host so it can warn the user about a degraded
/// build (e.g. a resumed model that could not be restored). Most diagnostics originate from
/// extension provider-registration (`#23`, outer-layer-L6) and so the list is empty until that
/// lands; the model-restore fallback is the one source available at this tier today.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostic {
    /// `"warning"` | `"error"` (Pi `severity`).
    pub severity: String,
    /// Human-readable message.
    pub message: String,
    /// What produced the diagnostic (`"model"`, `"provider"`, …), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Option bag for [`AgentSessionRuntime::new_session_with`] (Pi `newSession` options,
/// runtime.ts:223). The `withSession`/`setup` host callbacks are L6-fed (see gap `#26`/`#27`); the
/// data-carrying `parent_session` is honored here.
#[derive(Clone, Debug, Default)]
pub struct NewSessionOptions {
    /// Record this file as the new session's parent (Pi `parentSession`, runtime.ts:224).
    pub parent_session: Option<String>,
}

/// Option bag for [`AgentSessionRuntime::switch_session_with`] (Pi `switchSession` options,
/// runtime.ts:195). The `withSession`/`projectTrustContextFactory` host callbacks are L6-fed (see
/// gap `#27`); the data-carrying `cwd_override` is honored here.
#[derive(Clone, Debug, Default)]
pub struct SwitchSessionOptions {
    /// Override the cwd the resumed session binds to (Pi `cwdOverride`, runtime.ts:196). When
    /// `None`, the cwd is derived from the session file (the prior behavior).
    pub cwd_override: Option<PathBuf>,
}

struct RuntimeInner {
    session: Arc<AgentSession>,
    generation: u64,
    diagnostics: Vec<RuntimeDiagnostic>,
}

/// Collect the build-time diagnostics for `session` (Pi `result.diagnostics`,
/// agent-session-services.ts:176). Today the only tier-available source is the model-restore
/// fallback; extension provider-registration diagnostics (`#23`) join here once that lands.
fn collect_diagnostics(session: &AgentSession) -> Vec<RuntimeDiagnostic> {
    let mut out = Vec::new();
    if let Some(msg) = session.model_fallback_message() {
        out.push(RuntimeDiagnostic {
            severity: "warning".to_string(),
            message: msg.to_string(),
            source: Some("model".to_string()),
        });
    }
    out
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
        let session = factory.build(target, None).await?.into_shared();
        let diagnostics = collect_diagnostics(&session);
        let (gen_tx, _rx) = watch::channel(0);
        Ok(Self {
            factory,
            inner: RwLock::new(RuntimeInner { session, generation: 0, diagnostics }),
            gen_tx,
        })
    }

    /// The active session (cheap `Arc` clone). Re-read after any replacement.
    pub async fn session(&self) -> Arc<AgentSession> {
        self.inner.read().await.session.clone()
    }

    /// The current replacement generation (bumped on every successful switch/fork/new).
    pub async fn generation(&self) -> u64 {
        self.inner.read().await.generation
    }

    /// The build-time diagnostics for the active session (Pi `diagnostics` getter, runtime.ts:109).
    /// Recomputed on every replacement. Empty when the session built cleanly.
    pub async fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        self.inner.read().await.diagnostics.clone()
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
        self.install_inner(next, reason, previous_session_file, None).await;
    }

    /// The replacement tail with an optional host pre-start hook run after install but before
    /// `session_start` is emitted (Pi `beforeSessionStart`, runtime.ts:2470).
    async fn install_inner(
        &self,
        next: Arc<AgentSession>,
        reason: &str,
        previous_session_file: Option<String>,
        before_start: Option<Box<dyn FnOnce() + Send>>,
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
        // Install + bump generation; recompute the build diagnostics for the new session.
        let diagnostics = collect_diagnostics(&next);
        {
            let mut g = self.inner.write().await;
            g.session = next.clone();
            g.generation = new_gen;
            g.diagnostics = diagnostics;
        }
        let _ = self.gen_tx.send(new_gen);
        // Host pre-start hook (reload only): runs after the new session is installed but before
        // its `session_start` fans out (Pi `beforeSessionStart`, runtime.ts:2470).
        if let Some(hook) = before_start {
            hook();
        }
        // Announce the new session.
        next.emit_session_start(reason, previous_session_file).await;
    }

    /// Start a fresh session in the same cwd (Pi `newSession`, agent-session-runtime.ts:223).
    pub async fn new_session(&self) -> Result<SwitchResult, SessionServiceError> {
        self.new_session_with(NewSessionOptions::default()).await
    }

    /// Start a fresh session in the same cwd, honoring the [`NewSessionOptions`] bag (Pi `newSession`
    /// options, runtime.ts:223-257). `parent_session` is recorded on the new session file.
    pub async fn new_session_with(
        &self,
        options: NewSessionOptions,
    ) -> Result<SwitchResult, SessionServiceError> {
        let current = self.session().await;
        if self.vetoed(&current, HostEvent::SessionBeforeSwitch { target_id: String::new() }).await {
            return Ok(SwitchResult { cancelled: true });
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        drop(current);
        let next = self
            .factory
            .build_with_parent(SessionTarget::New, None, options.parent_session)
            .await?
            .into_shared();
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
        self.switch_session_with(path, SwitchSessionOptions::default()).await
    }

    /// Resume a session file, honoring the [`SwitchSessionOptions`] bag (Pi `switchSession` options,
    /// runtime.ts:193-220). A `cwd_override` rebinds the resumed session to a caller-supplied cwd
    /// instead of the one derived from the file.
    pub async fn switch_session_with(
        &self,
        path: impl Into<PathBuf>,
        options: SwitchSessionOptions,
    ) -> Result<SwitchResult, SessionServiceError> {
        let path = path.into();
        let current = self.session().await;
        let target_id = path.display().to_string();
        if self.vetoed(&current, HostEvent::SessionBeforeSwitch { target_id }).await {
            return Ok(SwitchResult { cancelled: true });
        }
        // Pre-flight: resolve the effective cwd (override wins, else derived from the file) and
        // assert it still exists BEFORE teardown (Pi `assertSessionCwdExists`, runtime.ts:208).
        let cwd = match options.cwd_override {
            Some(c) => c,
            None => SessionManager::open(&path)?.cwd().to_path_buf(),
        };
        if !cwd.exists() {
            return Err(SessionServiceError::MissingSessionCwd(cwd.display().to_string()));
        }
        let previous = current.session_file().await.map(|p| p.display().to_string());
        drop(current);
        let next =
            self.factory.build(SessionTarget::Resume(path), Some(cwd)).await?.into_shared();
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
                        // Reuse the current session file's OWN directory literally (Pi
                        // `createBranchedSession`'s `this.sessionDir` reuse, session-manager.ts:1343)
                        // — it is already fully resolved, so re-encoding it would nest the branch one
                        // level too deep (gap-analysis 05, Finding 1). Falls back to the cwd itself
                        // only if the file somehow has no parent.
                        let root =
                            file.parent().map(Path::to_path_buf).unwrap_or_else(|| cwd.clone());
                        let layout = cyrup_session::SessionLayout::literal(root, cwd);
                        mgr.create_branched_session(&leaf, &layout)?;
                        (self.factory.build_from_manager(mgr).await?.into_shared(), selected_text)
                    }
                    // Fork before the first message: a brand-new session.
                    None => {
                        (self.factory.build(SessionTarget::New, None).await?.into_shared(), selected_text)
                    }
                }
            }
            None => (self.factory.build(SessionTarget::New, None).await?.into_shared(), None),
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
        let next = self
            .factory
            .build(SessionTarget::Resume(destination), Some(cwd))
            .await?
            .into_shared();
        self.install(next, "resume", previous).await;
        Ok(SwitchResult { cancelled: false })
    }

    /// Reload the active session in place (Pi `reload`, agent-session.ts:2451): re-emit
    /// `session_shutdown{reload}`, rebuild the session via the factory — which re-loads settings,
    /// re-discovers resources, re-derives the system prompt, and resets the provider — then re-emit
    /// `session_start{reload}`. A persisted session is rebuilt by re-opening its file (preserving the
    /// transcript); an in-memory session has no file to re-open and is rebuilt fresh. `before_start`
    /// runs after the rebuild is installed but before `session_start` fans out (Pi
    /// `options.beforeSessionStart`, agent-session.ts:2470). The generation bumps (held subscriptions
    /// re-subscribe, R-11-021).
    pub async fn reload(
        &self,
        before_start: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<(), SessionServiceError> {
        let current = self.session().await;
        let previous = current.session_file().await.map(|p| p.display().to_string());
        // Derive the current target so the rebuild re-opens the SAME session (a persisted file is
        // resumed; an ephemeral session has nothing to re-open).
        let target = match current.session_file().await {
            Some(file) => SessionTarget::Resume(file),
            None => SessionTarget::New,
        };
        let cwd = current.services().cwd.clone();
        drop(current);
        let next = self.factory.build(target, Some(cwd)).await?.into_shared();
        self.install_inner(next, "reload", previous, before_start).await;
        Ok(())
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
