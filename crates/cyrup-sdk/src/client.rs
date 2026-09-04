//! [`Cyrup`] + [`CyrupBuilder`] — the embedder entry point.
//!
//! `Cyrup::builder()` configures the cross-session knobs and then `build_session(provider, config)`
//! assembles a wired [`Session`] over the [`cyrup_session_svc::SessionBuilder`] seam. The builder
//! adds **no behaviour**; it is a thin, stable construction surface.
//!
//! Advanced wiring — native built-in extensions, a custom credential store, settings overrides —
//! lives on the underlying [`SessionBuilder`]. Those APIs take types from `cyrup-ext`/`cyrup-config`,
//! which embedders pull in directly; reach them via [`CyrupBuilder::customize`] without this crate
//! re-exporting every internal type.

use std::sync::Arc;

use cyrup_agent::{ApiKeyResolver, StreamFn};
use cyrup_provider::{CreateModelsOptions, Provider};
use cyrup_session_svc::{ContextFile, SessionBuilder, SessionConfig, SkillPointer};

use crate::error::{SdkError, SdkResult};
use crate::handle::Session;

/// Zero-config provider construction — the packaged convenience Pi's `createAgentSession()` provides
/// via `ModelRegistry.create(authStorage)` + `findInitialModel` (sdk.ts:174-221).
///
/// Builds the full built-in provider catalog with **env-based auth** ([`cyrup_provider::default_models`]
/// over the default [`CreateModelsOptions`], whose `EnvAuthContext` reads real process-env credentials)
/// and returns the provider that owns `model_pattern` — a `provider/model` pattern selects that
/// provider; a bare provider id selects it directly. No credentials are wired by hand: an embedder can
/// go from a model string to a [`build_session`](CyrupBuilder::build_session)-ready provider without
/// constructing providers or an auth store itself.
///
/// # Examples
/// ```no_run
/// # fn demo() -> cyrup_sdk::SdkResult<()> {
/// let provider = cyrup_sdk::zero_config_provider("anthropic/claude-opus-4-8")?;
/// # let _ = provider;
/// # Ok(()) }
/// ```
///
/// # Errors
/// [`SdkError::Provider`] when `model_pattern`'s provider segment names no built-in provider.
pub fn zero_config_provider(model_pattern: &str) -> SdkResult<Arc<dyn Provider>> {
    let provider_id = model_pattern.split('/').next().unwrap_or(model_pattern);
    let models = cyrup_provider::default_models(CreateModelsOptions::default());
    models.get_provider(provider_id).ok_or_else(|| {
        let mut available: Vec<String> = models
            .get_providers()
            .iter()
            .map(|p| p.id().as_str().to_string())
            .collect();
        available.sort();
        SdkError::Provider(format!(
            "no built-in provider '{provider_id}' (from model pattern '{model_pattern}'); \
             available: {}",
            available.join(", ")
        ))
    })
}

/// A customization applied to the underlying [`SessionBuilder`] before `build`.
type Customizer = Box<dyn FnOnce(SessionBuilder) -> SessionBuilder + Send>;

/// The SDK entry point. Call [`Cyrup::builder`] to start configuring an embedding.
///
/// # Examples
/// ```no_run
/// # use std::sync::Arc;
/// # async fn demo(
/// #     provider: Arc<dyn cyrup_provider::Provider>,
/// #     config: cyrup_sdk::SessionConfig,
/// # ) -> cyrup_sdk::SdkResult<()> {
/// use cyrup_sdk::Cyrup;
///
/// let session = Cyrup::builder().build_session(provider, config).await?;
/// let answer = session.run("hello").await?;
/// println!("{answer}");
/// # Ok(()) }
/// ```
pub struct Cyrup;

impl Cyrup {
    /// Start a [`CyrupBuilder`] with embedder-friendly defaults.
    ///
    /// # Examples
    /// ```
    /// let _builder = cyrup_sdk::Cyrup::builder();
    /// ```
    #[must_use]
    pub fn builder() -> CyrupBuilder {
        CyrupBuilder::default()
    }
}

/// Configures construction inputs, then builds a [`Session`] per provider + [`SessionConfig`].
///
/// A minimal embedding is just `Cyrup::builder().build_session(provider, config)`. For native
/// extensions / custom auth / settings stores, use [`CyrupBuilder::customize`] to reach the wrapped
/// [`SessionBuilder`].
///
/// # Examples
/// ```no_run
/// # use std::sync::Arc;
/// # async fn demo(
/// #     provider: Arc<dyn cyrup_provider::Provider>,
/// #     config: cyrup_sdk::SessionConfig,
/// # ) -> cyrup_sdk::SdkResult<()> {
/// // `customize` reaches the underlying SessionBuilder for advanced wiring, e.g.
/// // `b.with_native_extension(ext)` / `b.auth(store)` / `b.cli_settings(settings)`.
/// let session = cyrup_sdk::Cyrup::builder()
///     .customize(|b| b) // pass-through; real use calls a SessionBuilder method
///     .build_session(provider, config)
///     .await?;
/// # let _ = session;
/// # Ok(()) }
/// ```
#[derive(Default)]
pub struct CyrupBuilder {
    customizers: Vec<Customizer>,
}

impl CyrupBuilder {
    /// Apply a transformation to the underlying [`SessionBuilder`] just before it is built.
    ///
    /// This is the escape hatch for any [`SessionBuilder`] method whose argument types come from an
    /// internal crate (e.g. `with_native_extension`, `auth`, `settings_store`, `cli_settings`).
    /// Customizers run in registration order.
    ///
    /// # Examples
    /// ```
    /// // A pass-through customizer; real wiring calls a `SessionBuilder` method on `b`
    /// // (e.g. `with_native_extension`, `auth`, `settings_store`, `cli_settings`).
    /// let _builder = cyrup_sdk::Cyrup::builder().customize(|b| b);
    /// ```
    #[must_use]
    pub fn customize(
        mut self,
        f: impl FnOnce(SessionBuilder) -> SessionBuilder + Send + 'static,
    ) -> Self {
        self.customizers.push(Box::new(f));
        self
    }

    /// Inject a custom transport (Pi `AgentOptions.streamFn`, sdk.ts:301; the `ProxyStreamFn`
    /// proxy-closure example, proxy.ts:92-98). The agent streams through `stream_fn` instead of the
    /// provider-backed default — bring your own proxy / HTTP transport. Pass a
    /// [`crate::ProxyStreamFn`] to route every call through an auth-managing proxy server.
    ///
    /// # Examples
    /// ```no_run
    /// # use std::sync::Arc;
    /// use cyrup_sdk::{Cyrup, ProxyStreamFn, StreamFn};
    /// let proxy: Arc<dyn StreamFn> =
    ///     Arc::new(ProxyStreamFn::new("https://genai.example.com", "auth-token"));
    /// let _builder = Cyrup::builder().stream_fn(proxy);
    /// ```
    #[must_use]
    pub fn stream_fn(self, stream_fn: Arc<dyn StreamFn>) -> Self {
        self.customize(move |b| b.stream_fn(stream_fn))
    }

    /// Inject a dynamic API-key resolver (Pi per-request key resolution). Consulted on every turn;
    /// its result overrides any static configured key.
    #[must_use]
    pub fn key_resolver(self, resolver: Arc<dyn ApiKeyResolver>) -> Self {
        self.customize(move |b| b.key_resolver(resolver))
    }

    /// Inject/transform synthetic skills (Pi `DefaultResourceLoader.skillsOverride`,
    /// resource-loader.ts:143). The closure transforms the discovered [`SkillPointer`] set that feeds
    /// the system prompt, letting an embedder add in-memory skills not backed by files on disk.
    ///
    /// # Examples
    /// ```no_run
    /// use cyrup_sdk::{Cyrup, SkillPointer};
    /// let _builder = Cyrup::builder().skills_override(|mut skills: Vec<SkillPointer>| {
    ///     skills.push(SkillPointer {
    ///         name: "deploy".into(),
    ///         description: Some("How to ship a release".into()),
    ///         path: "/virtual/deploy/SKILL.md".into(),
    ///         disable_model_invocation: false,
    ///     });
    ///     skills
    /// });
    /// ```
    #[must_use]
    pub fn skills_override(
        self,
        f: impl FnOnce(Vec<SkillPointer>) -> Vec<SkillPointer> + Send + 'static,
    ) -> Self {
        self.customize(move |b| b.skills_override(f))
    }

    /// Inject/transform synthetic context (`AGENTS.md`/`CLAUDE.md`) files (Pi
    /// `DefaultResourceLoader.agentsFilesOverride`, resource-loader.ts:155). The closure transforms
    /// the discovered [`ContextFile`] set the system prompt reads.
    #[must_use]
    pub fn context_files_override(
        self,
        f: impl FnOnce(Vec<ContextFile>) -> Vec<ContextFile> + Send + 'static,
    ) -> Self {
        self.customize(move |b| b.context_files_override(f))
    }

    /// Assemble a wired [`Session`] over the given `provider` and `config`.
    ///
    /// Resolves settings + trust + auth + model, discovers resources, builds tools, opens/creates
    /// the session tree, assembles the system prompt, and loads any extensions registered via
    /// [`CyrupBuilder::customize`] — all via [`cyrup_session_svc::SessionBuilder`].
    ///
    /// The returned session is **bound**: SEAM-026/SEAM-001 — this calls
    /// [`cyrup_session_svc::AgentSession::bind_extensions`], which announces
    /// `session_start{reason:"startup"}` to every loaded extension, exactly as pi's hosts do right
    /// after constructing a session (`bindExtensions()` from print-mode.ts:73, rpc-mode.ts:318 and
    /// interactive-mode.ts:1698, whose tail emits `_sessionStartEvent`, agent-session.ts:389+2250).
    /// Before this the documented one-call SDK path never bound at all, so anything initialising on
    /// that hook — audit loggers, intercom identity registration, permission policy load — silently
    /// no-op'd for embedders.
    ///
    /// Pair every built session with [`Session::close`], the `session_shutdown` mirror image.
    ///
    /// Runtime-tier control ops (`ctx.newSession()`/`fork()`/`switchSession()`/`reload()`) still
    /// need a host that can REPLACE the session, which a single `Session` by construction cannot be;
    /// they fail with `NoRuntimeHost` here. An embedder that needs them builds a
    /// [`cyrup_session_svc::SessionFactory`] + [`cyrup_session_svc::AgentSessionRuntime`] instead
    /// (both re-exported from this crate) — that is the same tier pi's own hosts sit on. This path
    /// cannot construct one for you because a customizer is `FnOnce(SessionBuilder) -> SessionBuilder`
    /// and a factory must re-apply it on every replacement.
    ///
    /// # Errors
    /// Returns [`crate::SdkError`] if the underlying facade build fails (e.g. an unknown model
    /// pattern, an empty provider catalog, or a failing extension `init`).
    ///
    /// # Examples
    /// ```no_run
    /// # use std::sync::Arc;
    /// # async fn demo(
    /// #     provider: Arc<dyn cyrup_provider::Provider>,
    /// #     config: cyrup_sdk::SessionConfig,
    /// # ) -> cyrup_sdk::SdkResult<()> {
    /// let session = cyrup_sdk::Cyrup::builder().build_session(provider, config).await?;
    /// // … drive the session …
    /// session.close().await;
    /// # Ok(()) }
    /// ```
    pub async fn build_session(
        self,
        provider: Arc<dyn Provider>,
        config: SessionConfig,
    ) -> SdkResult<Session> {
        let mut builder = SessionBuilder::new(provider, config);
        for customize in self.customizers {
            builder = customize(builder);
        }
        let session = Session::new(builder.build().await?);
        // SEAM-026/SEAM-001: announce the session to its extensions. Runs AFTER `Session::new`'s
        // `into_shared()` so the self-handle (post-run driver) is already bound when a
        // `session_start` handler runs — the same order `AgentSessionRuntime::create` uses.
        session.agent_session().bind_extensions().await;
        Ok(session)
    }

    /// Assemble a session, constructing the provider automatically from `config.model_pattern` via
    /// [`zero_config_provider`] (env-based auth, zero credential wiring) — the packaged zero-config
    /// path Pi's `createAgentSession()` provides via `ModelRegistry.create` + `findInitialModel`
    /// (sdk.ts:166-221). `config.model_pattern` also selects the model within that provider.
    ///
    /// # Errors
    /// [`SdkError::Provider`] when `config.model_pattern` is `None` or names no built-in provider;
    /// otherwise the underlying facade build error (unknown model id, empty catalog, extension init).
    ///
    /// # Examples
    /// ```no_run
    /// # async fn demo() -> cyrup_sdk::SdkResult<()> {
    /// use cyrup_sdk::{Cyrup, SessionConfig};
    /// let mut config = SessionConfig::new(".", "/home/me/.cyrup/agent");
    /// config.model_pattern = Some("anthropic/claude-opus-4-8".into());
    /// let session = Cyrup::builder().build_session_auto(config).await?;
    /// # let _ = session;
    /// # Ok(()) }
    /// ```
    pub async fn build_session_auto(self, config: SessionConfig) -> SdkResult<Session> {
        let pattern = config.model_pattern.clone().ok_or_else(|| {
            SdkError::Provider(
                "build_session_auto requires config.model_pattern to name a provider \
                 (e.g. \"anthropic/claude-opus-4-8\")"
                    .to_string(),
            )
        })?;
        let provider = zero_config_provider(&pattern)?;
        self.build_session(provider, config).await
    }
}
