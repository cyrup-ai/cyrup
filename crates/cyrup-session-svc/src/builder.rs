//! `SessionConfig` + `SessionBuilder` — assemble an [`AgentSession`] from the real subsystems
//! (arch-11 §3.3). One async `build()` resolves settings + trust + auth + model (cyrup-config),
//! discovers resources (cyrup-resources), builds the tool registry with isolation decorators +
//! permission policy (cyrup-tools), opens/creates the session tree and wires compaction
//! (cyrup-session arch-04/05), assembles the system prompt + context store (arch-06), builds the
//! extension host with native built-ins and attaches BOTH ext seams to the agent (cyrup-ext), and
//! resolves the provider into the agent loop (cyrup-provider).

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_agent::{Agent, ProviderStreamFn};
use cyrup_core::{CancelToken, ModelRef, RunCancel, ThinkingLevel};
use cyrup_config::{
    decide_trust, has_trust_requiring_resources, AppMode, AuthStore, InMemorySettingsStore,
    ModelResolver, Settings, SettingsManager, SettingsStore, TrustInputs, TrustOutcome,
};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, NativeExtension};
use cyrup_provider::{Model, Provider};
use cyrup_resources::{discover, DiscoveryConfig, SkillPointer};
use cyrup_session::manager::{NewSessionOpts, SessionManager};
use cyrup_session::prompt::{
    ContextFileLoader, DocsPointers, PromptInputs, ResolvedOverride, SystemPromptBuilder,
    ToolPromptContribution,
};
use cyrup_session::SessionLayout;
use cyrup_tools::{
    Availability, Backend, PermissionPolicy, ProtectedFs, ShellConfig, ToolRegistry, ToolsOptions,
    TraversalFs,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::SessionServiceError;
use crate::event::core_message_to_agent;
use crate::services::AgentSessionServices;
use crate::session::AgentSession;
use crate::subscriber::{Fanout, SvcSubscriber};

/// Which session to start (arch-11 §3.3, `SessionStartEvent` analogue).
#[derive(Clone, Debug, Default)]
pub enum SessionTarget {
    /// A fresh session for the cwd.
    #[default]
    New,
    /// Resume an existing session file by path.
    Resume(PathBuf),
    /// Continue the most recent session for the cwd (or create one if none).
    Continue,
}

/// The declarative inputs the builder resolves into a wired session (arch-11 §3.3).
#[derive(Clone)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    /// Global agent dir (`~/.cyrup/agent`): global settings, auth, context files, resource roots.
    pub agent_dir: PathBuf,
    /// Home dir for trust-requiring-resource detection (defaults to `agent_dir`).
    pub home: PathBuf,
    /// Sessions root directory (defaults to `agent_dir/sessions`).
    pub session_dir: Option<PathBuf>,
    /// Runtime mode (drives non-prompting trust + the extension `ctx.mode`/`ctx.hasUI`).
    pub app_mode: AppMode,
    /// Model selection pattern (`provider/id[:level]`); `None` ⇒ settings default ⇒ first catalog.
    pub model_pattern: Option<String>,
    /// Thinking level override (`None` ⇒ pattern `:level` ⇒ settings default).
    pub thinking_level: Option<ThinkingLevel>,
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)).
    pub trust_override: Option<bool>,
    /// `--no-context-files` / `-nc`.
    pub no_context_files: bool,
    /// `--no-skills`.
    pub no_skills: bool,
    /// Full system-prompt replacement.
    pub system_prompt: Option<String>,
    /// Append text after the assembled prompt.
    pub append_system_prompt: Option<String>,
    /// Persist to disk (`false` ⇒ ephemeral in-memory session; print/json default, R-11-008).
    pub persist: bool,
    pub target: SessionTarget,
    /// Model-visible tool-set control.
    pub tool_availability: Availability,
    /// Opt-in permission policy gate (empty ⇒ YOLO default, R-12-001).
    pub permission_policy: PermissionPolicy,
    /// Wrap the fs backend in [`ProtectedFs`] (blocks writes to `.env`/`.git`/… R-12-006).
    pub protect_paths: bool,
    /// Wrap the fs backend in [`TraversalFs`] confined to `cwd` (R-03-006).
    pub confine_to_cwd: bool,
}

impl SessionConfig {
    /// A config rooted at `cwd` with `agent_dir`, all defaults sensible for an SDK embedder.
    pub fn new(cwd: impl Into<PathBuf>, agent_dir: impl Into<PathBuf>) -> Self {
        let agent_dir = agent_dir.into();
        Self {
            cwd: cwd.into(),
            home: agent_dir.clone(),
            agent_dir,
            session_dir: None,
            app_mode: AppMode::Print,
            model_pattern: None,
            thinking_level: None,
            trust_override: None,
            no_context_files: false,
            no_skills: false,
            system_prompt: None,
            append_system_prompt: None,
            persist: true,
            target: SessionTarget::New,
            tool_availability: Availability::All,
            permission_policy: PermissionPolicy::new(),
            protect_paths: true,
            confine_to_cwd: false,
        }
    }
}

/// Assembles an [`AgentSession`] from a [`SessionConfig`] + injected provider/services (arch-11).
pub struct SessionBuilder {
    provider: Arc<dyn Provider>,
    config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth: Option<Arc<AuthStore>>,
    native_extensions: Vec<Arc<dyn NativeExtension>>,
    cli_settings: Settings,
}

impl SessionBuilder {
    /// Start a builder over the resolved `provider` and a `config`.
    pub fn new(provider: Arc<dyn Provider>, config: SessionConfig) -> Self {
        Self {
            provider,
            config,
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

    /// Override the credential store (default: `auth.json` under the agent dir).
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

    /// Register a native built-in extension (loaded in order; both seams wired to the agent).
    #[must_use]
    pub fn with_native_extension(mut self, ext: Arc<dyn NativeExtension>) -> Self {
        self.native_extensions.push(ext);
        self
    }

    /// Assemble the wired [`AgentSession`] (arch-11 §3.3). Async: discovery + context load + native
    /// extension `init` run here.
    pub async fn build(self) -> Result<AgentSession, SessionServiceError> {
        let cfg = self.config;
        let cwd = cfg.cwd.clone();
        let cancel = RunCancel::new();

        // ---- 1. settings + trust (cyrup-config) ------------------------------------------------
        // Load global first (project untrusted) to read defaultProjectTrust, then decide trust.
        let mut settings = SettingsManager::load(
            self.settings_store.clone(),
            self.cli_settings.clone(),
            false,
        );
        let default_trust = settings.effective().default_project_trust();
        let has_resources = has_trust_requiring_resources(&cwd, &cfg.home);
        let outcome = decide_trust(TrustInputs {
            has_resources,
            trust_override: cfg.trust_override,
            saved: None,
            default_trust,
            mode: cfg.app_mode,
            prompt_choice: None,
        });
        let trusted = matches!(outcome, TrustOutcome::Trusted);
        settings.set_project_trusted(trusted);

        // ---- 2. auth (cyrup-config) ------------------------------------------------------------
        let auth = self
            .auth
            .unwrap_or_else(|| Arc::new(AuthStore::at(cfg.agent_dir.join("auth.json"))));

        // ---- 3. model resolution (cyrup-config + cyrup-provider) -------------------------------
        let (resolved_model, model_ref, thinking) = resolve_model(&*self.provider, &cfg, &settings)?;

        // ---- 4. tools + isolation + policy (cyrup-tools) --------------------------------------
        let shell = ShellConfig::detect();
        let base = Backend::local(shell);
        let mut fs = base.fs.clone();
        if cfg.confine_to_cwd {
            fs = Arc::new(TraversalFs::new(fs, cwd.clone()));
        }
        if cfg.protect_paths {
            fs = Arc::new(ProtectedFs::with_defaults(fs));
        }
        let backend = Backend { fs, proc: base.proc.clone() };
        let registry = ToolRegistry::with_builtins(cwd.clone(), backend, ToolsOptions::default());
        let base_tools = registry.visible(&cfg.tool_availability);
        let read_available = base_tools.iter().any(|t| t.name() == "read");

        // ---- 5. resources discovery (cyrup-resources) -----------------------------------------
        let mut disc = DiscoveryConfig::new(cwd.clone(), cfg.agent_dir.clone());
        disc.trusted_project = trusted;
        disc.enable_skills = !cfg.no_skills;
        let report = discover(&disc, cancel.token()).await?;
        let resources = Arc::new(report.registry);
        // Read-gated skill pointers (R-06-010): only when the `read` tool is available.
        let skills: Vec<SkillPointer> = if read_available && !cfg.no_skills {
            resources.skills.winners().map(|s| s.pointer()).collect()
        } else {
            Vec::new()
        };

        // ---- 6. context store + system prompt (cyrup-session arch-06) -------------------------
        let loader = ContextFileLoader::new(
            cwd.clone(),
            cfg.agent_dir.clone(),
            trusted,
            cfg.no_context_files,
        );
        let context_store = Arc::new(cyrup_session::prompt::ContextStore::new());
        context_store
            .reload(&cancel, loader, Arc::from(skills), ResolvedOverride::default())
            .await?;
        let snapshot = context_store.snapshot();

        let selected_tools: Vec<Arc<str>> =
            base_tools.iter().map(|t| Arc::from(t.name())).collect();
        let tool_contributions: Vec<ToolPromptContribution> =
            base_tools.iter().map(|t| tool_contribution(t.name())).collect();

        let prompt_inputs = PromptInputs {
            custom_prompt: cfg.system_prompt.clone().map(Arc::from),
            selected_tools,
            tool_contributions,
            prompt_guidelines: Vec::new(),
            append_system_prompt: cfg.append_system_prompt.clone().map(Arc::from),
            cwd: cwd.clone(),
            context_files: snapshot.context_files.clone(),
            skills: snapshot.skills.clone(),
            docs: DocsPointers::default(),
            today: today(),
        };
        let system_prompt = SystemPromptBuilder::new().build(&prompt_inputs);

        // ---- 7. session tree (cyrup-session arch-04) ------------------------------------------
        let sessions_root =
            cfg.session_dir.clone().unwrap_or_else(|| cfg.agent_dir.join("sessions"));
        let layout = SessionLayout::new(sessions_root, cwd.clone());
        let manager = match &cfg.target {
            SessionTarget::New => {
                if cfg.persist {
                    SessionManager::create(&cwd, &layout, NewSessionOpts::default())?
                } else {
                    SessionManager::in_memory(&cwd, NewSessionOpts::default())
                }
            }
            SessionTarget::Resume(path) => SessionManager::open(path)?,
            SessionTarget::Continue => SessionManager::continue_recent(&cwd, &layout)?,
        };
        let session_id = manager.session_id().clone();
        // Seed the agent transcript from the resumed branch (R-04-011).
        let seed: Vec<cyrup_agent::AgentMessage> =
            manager.build_context().messages.iter().map(core_message_to_agent).collect();

        // ---- 8. extension host + both seams (cyrup-ext) ---------------------------------------
        let (mode, has_ui) = ext_mode(cfg.app_mode);
        let host = ExtensionHost::new(HostConfig { mode, has_ui, cwd: cwd.clone() });
        for ext in self.native_extensions {
            host.load_native(ext).await?;
        }
        let ext_host = Arc::new(host);
        let active_tools = ext_host.active_tools(&base_tools)?;
        let session_cancel = CancelToken::new();
        let ext_subscriber = ext_host.subscriber(session_cancel.clone());
        let ext_hooks = ext_host.hooks();

        // ---- 9. agent loop: provider + tools + composed hooks + both seams --------------------
        let policy_hooks = Arc::new(crate::hooks::PolicyHooks::new(
            cfg.permission_policy.clone(),
            ext_hooks,
            has_ui,
        ));
        let agent = Agent::builder(
            model_ref.clone(),
            Arc::new(ProviderStreamFn::new(self.provider.clone())),
        )
        .system_prompt(system_prompt.clone())
        .thinking_level(thinking)
        .tools(active_tools)
        .messages(seed)
        .hooks(policy_hooks)
        .session_id(session_id.clone())
        .build();

        let manager = Arc::new(AsyncMutex::new(manager));
        let fanout = Arc::new(Fanout::new());

        // Attach the extension notify seam, then the facade's persist+fan-out subscriber.
        agent.subscribe(ext_subscriber);
        agent.subscribe(Arc::new(SvcSubscriber::new(fanout.clone(), manager.clone())));
        let agent = Arc::new(agent);

        // ---- 10. assemble the session --------------------------------------------------------
        let services = AgentSessionServices {
            cwd,
            settings,
            project_trusted: trusted,
            auth,
            resources,
            context: context_store,
            ext_host,
            model: resolved_model,
            system_prompt,
        };

        Ok(AgentSession::from_parts(
            agent,
            manager,
            fanout,
            self.provider,
            services,
            model_ref,
            session_cancel,
            session_id,
        ))
    }
}

/// Resolve `(Model, ModelRef, ThinkingLevel)` from the pattern / settings / catalog (R-07-019).
fn resolve_model(
    provider: &dyn Provider,
    cfg: &SessionConfig,
    settings: &SettingsManager,
) -> Result<(Model, ModelRef, ThinkingLevel), SessionServiceError> {
    let available = provider.models();
    if available.is_empty() {
        return Err(SessionServiceError::NoModels(provider.id().to_string()));
    }
    let resolver = ModelResolver::new(available);
    let pattern = cfg.model_pattern.clone().or_else(|| settings.effective().default_model());

    let (model, parsed_thinking) = match pattern {
        Some(pat) => {
            let parsed = resolver.parse_pattern(&pat, true);
            match parsed.model {
                Some(m) => (m, parsed.thinking_level),
                None => return Err(SessionServiceError::ModelNotFound(pat)),
            }
        }
        None => {
            // First catalog entry (checked non-empty above).
            let m = available.first().cloned().ok_or_else(|| {
                SessionServiceError::NoModels(provider.id().to_string())
            })?;
            (m, None)
        }
    };
    let thinking = cfg
        .thinking_level
        .or(parsed_thinking)
        .unwrap_or_else(|| settings.effective().default_thinking_level());
    let model_ref = ModelRef {
        provider: model.provider.clone(),
        api: Some(model.api.clone()),
        model: model.id.clone(),
    };
    Ok((model, model_ref, thinking))
}

/// Synthesize a one-line prompt snippet for a built-in tool so the "Available tools" section is
/// populated (arch-06 R-06-012). Mirrors the built-ins' own `prompt_snippet` intent.
fn tool_contribution(name: &str) -> ToolPromptContribution {
    let snippet = match name {
        "read" => "Read a file from the workspace",
        "write" => "Write a file to the workspace",
        "edit" => "Edit a file with a find/replace",
        "bash" => "Run a shell command",
        "grep" => "Search file contents",
        "find" => "Find files by glob",
        "ls" => "List a directory",
        _ => "Tool",
    };
    ToolPromptContribution::snippet(Arc::<str>::from(name), Arc::<str>::from(snippet))
}

/// Map the runtime mode to the extension `(ExtMode, has_ui)` (R-11-002).
fn ext_mode(mode: AppMode) -> (ExtMode, bool) {
    match mode {
        AppMode::Interactive => (ExtMode::Tui, true),
        AppMode::Rpc => (ExtMode::Rpc, true),
        AppMode::Json => (ExtMode::Json, false),
        AppMode::Print => (ExtMode::Print, false),
    }
}

/// Today's date (UTC) for the prompt footer; falls back to the epoch on a clock fault.
fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}
