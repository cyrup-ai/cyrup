//! `AgentSessionRuntime` — owns the active [`AgentSession`] + its cwd-bound services and replaces
//! them atomically on `new_session`/`switch_session`/`fork`/`dispose` (arch-11 §3.4; Pi
//! `agent-session-runtime.ts:74-398`). Every replacement follows the same protocol: emit a *before*
//! extension event that may veto, tear down the current session (`session_shutdown` + dispose),
//! invalidate prior subscriptions (terminal `SessionReplaced`, R-11-021), rebuild the next session
//! via the stored [`SessionFactory`] (recreating cwd-bound services), apply it, bump the generation
//! `watch`, and emit `session_start`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use cyrup_core::CancelToken;
use cyrup_ext::{HostEvent, Reduced};
use cyrup_session::manager::SessionManager;
use serde_json::Value;
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

/// The RUNTIME-tier half of a loaded extension's `control` capability — the sink an
/// [`AgentSession`] routes `new-session`/`switch`/`fork`/`reload` to (SEAM-003).
///
/// This is cyrup's [`ExtensionCommandContextActions`] (Pi `extensions/types.ts:1652-1672`). Pi
/// binds it with REAL implementations in every host — `modes/rpc/rpc-mode.ts:321-346` and
/// `modes/print-mode.ts:75-95` both pass a `commandContextActions` bag wired to
/// `runtimeHost.newSession`/`fork`/`switchSession` and `session.reload` — stores it at
/// `agent-session.ts:2236-2238` and installs it from `_applyExtensionBindings` (:2308-2310) via
/// `runner.bindCommandContext(...)`, so `ctx.newSession()` inside a command handler executes
/// INLINE. cyrup used to queue the op onto `LiveHostServices`'s control channel and then discard
/// the drained vector, so every one of these was a no-op end to end.
///
/// The session-LOCAL ops (`navigateTree`/`waitForIdle`/`sendMessage`) deliberately do NOT live here:
/// they need no runtime host and are applied in place by
/// [`AgentSession::apply_pending_control`].
///
/// Pi documents the resulting context staleness (`extensions/loader.ts:206-208`: "Do not use a
/// captured pi or command ctx after `ctx.newSession()`, `ctx.fork()`, `ctx.switchSession()`, or
/// `ctx.reload()`") — cyrup inherits it, since each of these disposes the session that queued the op.
#[async_trait::async_trait]
pub trait RuntimeActions: Send + Sync {
    /// Pi `ctx.newSession(options)`. `opts` is the raw guest bag (`{parentSession, withSession}`).
    async fn new_session(&self, opts: &Value) -> Result<(), SessionServiceError>;
    /// Pi `ctx.switchSession(sessionPath, options)`.
    async fn switch_session(&self, session_id: &str, opts: &Value)
        -> Result<(), SessionServiceError>;
    /// Pi `ctx.fork(entryId, {position, withSession})`.
    async fn fork(&self, entry_id: &str, opts: &Value) -> Result<(), SessionServiceError>;
    /// Pi `ctx.reload()`.
    async fn reload(&self) -> Result<(), SessionServiceError>;
}

/// The [`RuntimeActions`] implementation backed by a live [`AgentSessionRuntime`].
///
/// Holds a `Weak` so the runtime → session → actions → runtime chain is not a reference cycle: the
/// runtime owns the active session, the session holds this sink, and this sink must therefore NOT
/// own the runtime. An op arriving after the runtime is dropped degrades to a typed
/// [`SessionServiceError::NoRuntimeHost`], never a panic.
struct RuntimeHostActions(Weak<AgentSessionRuntime>);

impl RuntimeHostActions {
    fn runtime(&self, op: &'static str) -> Result<Arc<AgentSessionRuntime>, SessionServiceError> {
        self.0.upgrade().ok_or(SessionServiceError::NoRuntimeHost(op))
    }
}

#[async_trait::async_trait]
impl RuntimeActions for RuntimeHostActions {
    async fn new_session(&self, opts: &Value) -> Result<(), SessionServiceError> {
        let rt = self.runtime("new_session")?;
        let options = NewSessionOptions {
            parent_session: opts
                .get("parentSession")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        rt.new_session_with(options).await.map(|_| ())
    }

    async fn switch_session(
        &self,
        session_id: &str,
        opts: &Value,
    ) -> Result<(), SessionServiceError> {
        let rt = self.runtime("switch_session")?;
        let options = SwitchSessionOptions {
            cwd_override: opts.get("cwdOverride").and_then(Value::as_str).map(PathBuf::from),
        };
        rt.switch_session_with(PathBuf::from(session_id), options).await.map(|_| ())
    }

    async fn fork(&self, entry_id: &str, opts: &Value) -> Result<(), SessionServiceError> {
        let rt = self.runtime("fork")?;
        // Pi `fork(entryId, {position: "before" | "at"})` (types.ts:1661-1664); the default is
        // "before" (`const position = options?.position ?? "before"`, agent-session-runtime.ts:266).
        let position = match opts.get("position").and_then(Value::as_str) {
            Some("at") => ForkPosition::At,
            _ => ForkPosition::Before,
        };
        rt.fork(cyrup_core::EntryId::from(entry_id), position).await.map(|_| ())
    }

    async fn reload(&self) -> Result<(), SessionServiceError> {
        self.runtime("reload")?.reload(None).await
    }
}

/// Owns the active session + rebuilds it on every cwd/session switch (arch-11 §3.4).
pub struct AgentSessionRuntime {
    factory: Arc<SessionFactory>,
    inner: RwLock<RuntimeInner>,
    gen_tx: watch::Sender<u64>,
    /// The [`RuntimeActions`] sink installed onto EVERY session this runtime owns — the initial one
    /// in [`Self::create`] and each replacement in [`Self::install_inner`] (SEAM-003). Built once,
    /// in `create`, because it needs a `Weak<Self>` that only exists after the `Arc` is minted.
    actions: OnceLock<Arc<dyn RuntimeActions>>,
}

impl AgentSessionRuntime {
    /// Build the initial runtime from a `factory` + initial `target` (Pi `createAgentSessionRuntime`,
    /// agent-session-runtime.ts:406).
    ///
    /// The initial session is ANNOUNCED here (`session_start{reason:"startup"}`) — the mirror image
    /// of the `session_shutdown` [`Self::dispose`] emits at teardown. Pi's hosts do this by calling
    /// `session.bindExtensions()` right after `createAgentSessionRuntime` (rpc-mode.ts:318 via
    /// `rebindSession`, interactive-mode.ts:1698), which emits the session's `_sessionStartEvent`;
    /// for an initial runtime `main.ts:674` passes no `sessionStartEvent`, so it defaults to
    /// `{type:"session_start", reason:"startup"}` (agent-session.ts:389). Replacements are announced
    /// instead by [`Self::install_inner`] with their own reason.
    ///
    /// This is the convenience shape for a host with NOTHING to configure between build and
    /// announcement. A host that applies post-build CLI configuration first (`--name`, `--models`,
    /// `--thinking`) must use [`Self::create_unannounced`] and let its dispatch entry point
    /// announce, or the announcement races ahead of the configuration — see that constructor's
    /// SEAM-033 note.
    pub async fn create(
        factory: Arc<SessionFactory>,
        target: SessionTarget,
    ) -> Result<Arc<Self>, SessionServiceError> {
        let this = Self::create_unannounced(factory, target).await?;
        this.session().await.bind_extensions().await;
        Ok(this)
    }

    /// Build the runtime WITHOUT announcing the initial session — pi's `createAgentSessionRuntime`
    /// verbatim (agent-session-runtime.ts:414-432: it constructs and returns, and never touches
    /// `bindExtensions`).
    ///
    /// SEAM-033 ordering: pi's HOST announces, and it does so only after `main.ts` has finished
    /// configuring the session — `sessionManager.appendSessionInfo(name)` at main.ts:650 and the
    /// `scopedModels` fold into `sessionOptions` at main.ts:742-750 both run strictly BEFORE
    /// `createAgentSessionRuntime` at main.ts:793, and the announcement itself lands later still, in
    /// `runPrintMode`'s `rebindSession()` → `session.bindExtensions(...)` (print-mode.ts:119 → :73 →
    /// agent-session.ts:2250). A host that must apply post-build configuration (`--name`,
    /// `--models`, `--thinking`) therefore builds with THIS constructor, configures, and lets the
    /// dispatch entry point announce; a host with nothing to configure first uses [`Self::create`].
    ///
    /// Announcing too early is observable: a `session_start` handler — the permission gate's policy
    /// refresh, intercom's registration, a subagent's background-run reset — reads session state
    /// that would still be unconfigured, and since print/json is the arm a spawned subagent child
    /// re-execs into, every subagent run inherits whatever it sees.
    ///
    /// SEAM-003: the runtime is returned as an `Arc` because it must hand every session it owns a
    /// `Weak`-backed [`RuntimeActions`] sink, and that `Weak` can only be taken from the `Arc`. The
    /// sink is installed here, BEFORE any announcement, so a `session_start` handler that
    /// immediately calls `ctx.newSession()`/`ctx.reload()` reaches a live host rather than a dead
    /// queue — Pi installs `commandContextActions` as an argument OF `bindExtensions`
    /// (rpc-mode.ts:342-346), so they are bound before the emit at its tail (agent-session.ts:2250).
    pub async fn create_unannounced(
        factory: Arc<SessionFactory>,
        target: SessionTarget,
    ) -> Result<Arc<Self>, SessionServiceError> {
        let session = factory.build(target, None).await?.into_shared();
        let diagnostics = collect_diagnostics(&session);
        let (gen_tx, _rx) = watch::channel(0);
        let this = Arc::new(Self {
            factory,
            inner: RwLock::new(RuntimeInner {
                session: session.clone(),
                generation: 0,
                diagnostics,
            }),
            gen_tx,
            actions: OnceLock::new(),
        });
        let actions: Arc<dyn RuntimeActions> = Arc::new(RuntimeHostActions(Arc::downgrade(&this)));
        let _ = this.actions.set(actions.clone());
        session.install_runtime_actions(actions);
        Ok(this)
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
        // SEAM-003: hand the REPLACEMENT session the same runtime sink before it is announced, so a
        // `ctx.newSession()` from a command on the new session works exactly as it did on the old
        // one. Pi re-runs `bindExtensions({commandContextActions})` on every rebind
        // (rpc-mode.ts:341-346 `rebindSession`), which is the same guarantee.
        if let Some(actions) = self.actions.get() {
            next.install_runtime_actions(actions.clone());
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

        // SEAM-009 — resolve (and VALIDATE) the anchor against the LIVE session manager BEFORE the
        // persisted/in-memory split, exactly as Pi does: `getEntry(entryId)` +
        // `throw new Error("Invalid entry ID for forking")` at agent-session-runtime.ts:275-283 sit
        // ABOVE the `isPersisted()` branch at :290. Resolving inside the persisted arm (as this used
        // to) meant an unsaved session accepted any entry id, valid or not, and branched at none of
        // them.
        let (target_leaf, selected_text) = current.fork_anchor_live(&entry, position).await?;

        // Mirror Pi (agent-session-runtime.ts:287-350): for a persisted session, open a throwaway
        // manager from the current file, branch it in place at the resolved leaf, and hand THAT
        // manager object to the factory (its on-disk write may still be deferred). For an in-memory
        // session, branch the LIVE manager and hand it over (Pi reuses `this.session.sessionManager`
        // verbatim, :333-341). Only a "fork before the first message" — no anchor at all — is a
        // brand-new empty session, on either path (Pi `newSession(...)`, :291/:335).
        let next = match (&target_leaf, session_file) {
            (Some(leaf), Some(file)) => {
                let mut mgr = SessionManager::open(&file)?;
                // Reuse the current session file's OWN directory literally (Pi
                // `createBranchedSession`'s `this.sessionDir` reuse, session-manager.ts:1343)
                // — it is already fully resolved, so re-encoding it would nest the branch one
                // level too deep (gap-analysis 05, Finding 1). Falls back to the cwd itself
                // only if the file somehow has no parent.
                let root = file.parent().map(Path::to_path_buf).unwrap_or_else(|| cwd.clone());
                let layout = cyrup_session::SessionLayout::literal(root, cwd);
                mgr.create_branched_session(leaf, &layout)?;
                self.factory.build_from_manager(mgr).await?.into_shared()
            }
            // SEAM-009: the non-persisted branch. The transcript lives ONLY in memory, so the live
            // manager is the sole copy — branch it and carry it into the forked session rather than
            // building an empty `SessionTarget::New` and losing the whole conversation.
            //
            // The three steps are ordered, and the order is load-bearing in a way Pi's is not. Pi
            // branches `this.session.sessionManager` in place and only THEN awaits
            // `teardownCurrent` (agent-session-runtime.ts:333-341); because the outgoing session
            // keeps pointing at that same object, everything the dying run appends while it settles
            // lands in the branched manager — i.e. in the fork. cyrup's `build_from_manager` takes
            // the manager BY VALUE, so moving it out early would leave the outgoing session writing
            // into a throwaway placeholder that is then dropped: an in-flight turn's final content
            // (Pi's "aborted turn including tool results", :167-169) would be lost outright, and a
            // non-persisted session has no file to recover it from. So: branch in place, settle the
            // outgoing run against the branched manager, and only then move it.
            (Some(leaf), None) => {
                current.branch_live_manager(leaf).await?;
                current.abort_and_settle().await;
                let mgr = current.take_manager().await?;
                self.factory.build_from_manager(mgr).await?.into_shared()
            }
            // Fork before the first message: a brand-new session (both persistence modes).
            (None, _) => self.factory.build(SessionTarget::New, None).await?.into_shared(),
        };
        drop(current);
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
