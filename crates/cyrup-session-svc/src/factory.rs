//! `SessionFactory` — the reusable per-cwd session builder the runtime invokes on every switch
//! (arch-11 §3.3/§3.4; Pi `CreateAgentSessionRuntimeFactory`, agent-session-runtime.ts:33). Unlike
//! the one-shot consuming [`crate::SessionBuilder`], a factory stores the process-global construction
//! inputs (provider, settings store, auth, native extensions, CLI settings, base config) and
//! produces a FRESH [`AgentSession`] for a given target/cwd each time it is called — exactly what
//! `newSession`/`switchSession`/`fork`/`import` need to rebuild cwd-bound services on replacement.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::{AuthStore, InMemorySettingsStore, Settings, SettingsStore};
use cyrup_ext::NativeExtension;
use cyrup_provider::Provider;
use cyrup_session::manager::SessionManager;

use crate::builder::{SessionBuilder, SessionConfig, SessionTarget};
use crate::error::SessionServiceError;
use crate::session::AgentSession;

/// A reusable factory that rebuilds an [`AgentSession`] per cwd-switch (arch-11 §3.4).
pub struct SessionFactory {
    provider: Arc<dyn Provider>,
    base_config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth: Option<Arc<AuthStore>>,
    native_extensions: Vec<Arc<dyn NativeExtension>>,
    cli_settings: Settings,
}

impl SessionFactory {
    /// Start a factory over the resolved `provider` and a base `config` (its `target`/`cwd` are
    /// overridden per build).
    pub fn new(provider: Arc<dyn Provider>, config: SessionConfig) -> Self {
        Self {
            provider,
            base_config: config,
            settings_store: Arc::new(InMemorySettingsStore::new()),
            auth: None,
            native_extensions: Vec::new(),
            cli_settings: Settings::new(),
        }
    }

    /// Override the settings store (default: in-memory).
    #[must_use]
    pub fn settings_store(mut self, store: Arc<dyn SettingsStore>) -> Self {
        self.settings_store = store;
        self
    }

    /// Override the credential store.
    #[must_use]
    pub fn auth(mut self, auth: Arc<AuthStore>) -> Self {
        self.auth = Some(auth);
        self
    }

    /// CLI-scoped settings overrides (highest precedence).
    #[must_use]
    pub fn cli_settings(mut self, settings: Settings) -> Self {
        self.cli_settings = settings;
        self
    }

    /// Register a native built-in extension (re-`init`-ed into each freshly built session).
    #[must_use]
    pub fn with_native_extension(mut self, ext: Arc<dyn NativeExtension>) -> Self {
        self.native_extensions.push(ext);
        self
    }

    /// The base cwd this factory was configured with.
    pub fn cwd(&self) -> &std::path::Path {
        &self.base_config.cwd
    }

    /// The sessions root directory this factory writes to (`session_dir` override, else
    /// `agent_dir/sessions`). Used by the runtime `import_from_jsonl` op to copy the source file in.
    pub(crate) fn session_dir(&self) -> PathBuf {
        self.base_config
            .session_dir
            .clone()
            .unwrap_or_else(|| self.base_config.agent_dir.join("sessions"))
    }

    /// Build a fresh [`AgentSession`] for `target`, optionally rebinding to a new `cwd` (arch-11
    /// §3.4 — recreating cwd-bound services for the effective cwd).
    pub async fn build(
        &self,
        target: SessionTarget,
        cwd: Option<PathBuf>,
    ) -> Result<AgentSession, SessionServiceError> {
        let mut cfg = self.base_config.clone();
        cfg.target = target;
        if let Some(c) = cwd {
            cfg.cwd = c;
        }
        let mut builder = SessionBuilder::new(self.provider.clone(), cfg)
            .settings_store(self.settings_store.clone())
            .cli_settings(self.cli_settings.clone());
        if let Some(auth) = &self.auth {
            builder = builder.auth(auth.clone());
        }
        for ext in &self.native_extensions {
            builder = builder.with_native_extension(ext.clone());
        }
        builder.build().await
    }

    /// Build a fresh [`AgentSession`] around a caller-supplied, already-constructed
    /// [`SessionManager`] (Pi `createAgentSessionFromServices` with an explicit `sessionManager`,
    /// agent-session-services.ts:187). Used by the runtime fork path: the branched manager is handed
    /// over directly (avoiding a reopen-by-path while its file write is still deferred on disk).
    pub(crate) async fn build_from_manager(
        &self,
        manager: SessionManager,
    ) -> Result<AgentSession, SessionServiceError> {
        let mut cfg = self.base_config.clone();
        cfg.cwd = manager.cwd().to_path_buf();
        let mut builder = SessionBuilder::new(self.provider.clone(), cfg)
            .settings_store(self.settings_store.clone())
            .cli_settings(self.cli_settings.clone())
            .with_manager(manager);
        if let Some(auth) = &self.auth {
            builder = builder.auth(auth.clone());
        }
        for ext in &self.native_extensions {
            builder = builder.with_native_extension(ext.clone());
        }
        builder.build().await
    }
}
