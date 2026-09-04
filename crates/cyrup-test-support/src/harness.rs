//! Headless [`AgentSession`] harness (Pi `createHarness`/`createHarnessWithExtensions`,
//! test-harness.ts:432-446; registration-based `suite/harness.ts`).
//!
//! Builds a fully-wired [`AgentSession`] over the scripted faux provider in a temp dir, drives turns
//! deterministically, captures every emitted [`AgentSessionEvent`] for assertions, and exposes the
//! faux call state + tool-gating knobs + native-extension loading. Cleanup is RAII (temp dir removed
//! when the [`Harness`] drops — cyrup's analogue of Pi's `cleanup()`/`afterEach`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cyrup_config::{AppMode, AuthStore, Settings};
use cyrup_core::{Content, Message, ProviderId, Tool};
use cyrup_ext::NativeExtension;
use cyrup_provider::faux::FauxModelDefinition;
use cyrup_provider::{Model, Provider};
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig, SessionServiceError,
};
use cyrup_tools::{Availability, PermissionPolicy};
use futures::StreamExt;

use crate::response::{
    FauxResponse, faux_model, faux_model_from_def, faux_model_with_context_window,
};
use crate::scripted::{
    FauxStreamFnState, ScriptedProvider, create_faux_stream_fn_queued,
    create_faux_stream_fn_with_models,
};
use crate::tempdir::TestTempDir;
use crate::tool_ext::ToolExtension;

/// Failure constructing or driving a [`Harness`].
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("temp dir: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] SessionServiceError),
}

/// Declarative harness inputs (Pi `HarnessOptions`, test-harness.ts:320-339 + suite/harness.ts:63-74),
/// adapted to cyrup's `SessionConfig` surface.
pub struct HarnessOptions {
    /// Response sequence for the scripted provider. Default: a single `"ok"` (Pi default,
    /// test-harness.ts:372).
    pub responses: Vec<FauxResponse>,
    /// Full system-prompt replacement. Default: `"You are a test assistant."` (Pi default,
    /// test-harness.ts:378).
    pub system_prompt: Option<String>,
    /// Text appended after the assembled system prompt.
    pub append_system_prompt: Option<String>,
    /// Model selection pattern (`provider/id[:level]`). Default: first catalog entry (the faux model).
    pub model_pattern: Option<String>,
    /// Declarative multi-model catalog the scripted provider advertises (Pi `models?:
    /// FauxModelDefinition[]`, suite/harness.ts:64). Empty ⇒ the single default faux model. When set,
    /// the first definition is the default and the rest are reachable via [`Harness::get_model`] /
    /// `model_pattern`, driving Pi's dynamic-provider / multi-model suites through the harness.
    pub models: Vec<FauxModelDefinition>,
    /// An entirely arbitrary [`Model`] to advertise (Pi `createHarness({ model })`,
    /// test-harness.ts:324,369-370) — a different api/provider/modalities than the faux default.
    /// Highest precedence: overrides both [`Self::models`] and [`Self::context_window`].
    pub model: Option<Model>,
    /// Use Pi's **queue-consuming** faux flavour (Pi `registerFauxProvider`/`suite/harness.ts`):
    /// responses are consumed in order, and once exhausted further turns stream the `"No more faux
    /// responses queued"` error terminal. Default `false` ⇒ the cycling `createFauxStreamFn` flavour
    /// (test-harness.ts). Enables [`Harness::append_responses`]/[`Harness::pending_count`].
    pub queue_responses: bool,
    /// Persist the session to disk under the temp dir. Default: `false` (in-memory; Pi
    /// `SessionManager.inMemory()`, test-harness.ts:384).
    pub persist: bool,
    /// Runtime mode (drives trust + extension `ctx.mode`/`ctx.hasUI`). Default: [`AppMode::Print`].
    pub app_mode: AppMode,
    /// Opt-in permission policy gate. Default: empty (YOLO).
    pub permission_policy: PermissionPolicy,
    /// Only these tool names are model-visible (Pi `allowedToolNames`, suite/harness.ts:70).
    pub allowed_tool_names: Option<Vec<String>>,
    /// All tools except these are model-visible (Pi `excludedToolNames`, suite/harness.ts:71).
    pub excluded_tool_names: Option<Vec<String>>,
    /// The tools active at session start (Pi `initialActiveToolNames`, suite/harness.ts:68,184).
    /// cyrup has no toggleable active-set distinct from the visibility gate, so when neither
    /// `allowed_tool_names` nor `excluded_tool_names` is set this seeds the visible (active) set
    /// ([CYRUP-DELTA]: Pi's separate `allowed`/`initialActive` gates collapse onto one selector).
    pub initial_active_tool_names: Option<Vec<String>>,
    /// CLI-scoped settings overrides — retry, compaction, etc. (Pi `HarnessOptions.settings`,
    /// test-harness.ts:327; suite/harness.ts:65). Deep-merged at highest precedence by the builder.
    pub settings: Settings,
    /// Model context-window override (Pi `HarnessOptions.contextWindow`, test-harness.ts:324,370).
    /// `None` ⇒ the faux model's default 128000. Makes compaction-threshold tests reproducible.
    pub context_window: Option<u64>,
    /// Custom tools registered on the session; a built-in name (`read`/`bash`/`edit`/`write`)
    /// overrides that built-in (Pi `tools` + `baseToolsOverride`, test-harness.ts:331,333,402).
    pub tools: Vec<Arc<dyn Tool>>,
    /// Wire a working faux credential for the provider (Pi `withConfiguredAuth`, default `true`,
    /// suite/harness.ts:73,108). `false` builds the unauthenticated path (no stored/runtime key).
    pub with_configured_auth: bool,
    /// Inline native extensions loaded into the session (Pi `extensionFactories`,
    /// test-harness.ts:338; cyrup's native-built-in analogue).
    pub native_extensions: Vec<Arc<dyn NativeExtension>>,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            responses: vec![FauxResponse::text("ok")],
            system_prompt: Some("You are a test assistant.".to_string()),
            append_system_prompt: None,
            model_pattern: None,
            models: Vec::new(),
            model: None,
            queue_responses: false,
            persist: false,
            app_mode: AppMode::Print,
            permission_policy: PermissionPolicy::new(),
            allowed_tool_names: None,
            excluded_tool_names: None,
            initial_active_tool_names: None,
            settings: Settings::new(),
            context_window: None,
            tools: Vec::new(),
            with_configured_auth: true,
            native_extensions: Vec::new(),
        }
    }
}

impl HarnessOptions {
    /// Options scripted with the given responses (everything else default).
    pub fn with_responses(responses: Vec<FauxResponse>) -> Self {
        Self {
            responses,
            ..Default::default()
        }
    }
}

/// A wired session + scripted provider + captured events (Pi `Harness`, test-harness.ts:341-356).
pub struct Harness {
    session: AgentSession,
    provider: Arc<ScriptedProvider>,
    faux_state: Arc<Mutex<FauxStreamFnState>>,
    events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    // RAII temp dir: removed on drop (Pi `cleanup()`).
    _temp: TestTempDir,
}

impl Harness {
    /// The wired session.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// The scripted provider (reconfigure responses mid-test).
    pub fn provider(&self) -> &Arc<ScriptedProvider> {
        &self.provider
    }

    /// Replace the scripted response sequence (Pi `setResponses`, suite/harness.ts:203).
    pub fn set_responses(&self, responses: Vec<FauxResponse>) {
        self.provider.set_responses(responses);
    }

    /// Append to the scripted response sequence (Pi `appendResponses`, suite/harness.ts:204). Most
    /// meaningful with [`HarnessOptions::queue_responses`], where it extends the consumable queue.
    pub fn append_responses(&self, responses: Vec<FauxResponse>) {
        self.provider.append_responses(responses);
    }

    /// The number of pending (not-yet-consumed) responses (Pi `getPendingResponseCount`,
    /// suite/harness.ts:205).
    pub fn pending_count(&self) -> usize {
        self.provider.pending_count()
    }

    /// The full model catalog the harness advertises (Pi `harness.models`, suite/harness.ts:82,201).
    /// The first entry is the default; the rest are reachable via [`Self::get_model`].
    pub fn models(&self) -> &[Model] {
        self.provider.models()
    }

    /// The default model (Pi `harness.getModel()` with no id, suite/harness.ts:83,202).
    pub fn model(&self) -> Option<&Model> {
        self.provider.models().first()
    }

    /// Look a model up by id (Pi `harness.getModel(modelId)`, suite/harness.ts:84,202); `None` when
    /// no advertised model has that id.
    pub fn get_model(&self, id: &str) -> Option<&Model> {
        self.provider.get_model(id)
    }

    /// Snapshot of the faux call state (call count + captured contexts; Pi `harness.faux`).
    pub fn faux(&self) -> FauxStreamFnState {
        self.faux_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Drive one prompt turn to completion, returning that run's events (also appended to the
    /// cumulative [`Self::events`]). The run-scoped stream terminates after `agent_end`, so capture
    /// is deterministic and race-free.
    pub async fn run(
        &self,
        text: impl Into<String>,
    ) -> Result<Vec<AgentSessionEvent>, HarnessError> {
        let mut stream = self.session.prompt(text.into()).await?;
        let mut run_events = Vec::new();
        while let Some(ev) = stream.next().await {
            run_events.push(ev.clone());
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(ev);
        }
        Ok(run_events)
    }

    /// All events captured across every [`Self::run`], in order (Pi `harness.events`).
    pub fn events(&self) -> Vec<AgentSessionEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Filter captured events by their `kind` discriminant (Pi `eventsOfType<T>`,
    /// test-harness.ts:424). cyrup keys on the snake_case `type` tag string.
    pub fn events_of_kind(&self, kind: &str) -> Vec<AgentSessionEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.kind() == kind)
            .collect()
    }

    /// The persisted user-message texts on the current branch (Pi `getUserTexts`,
    /// suite/harness.ts:51-55).
    pub async fn user_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .await
            .iter()
            .filter(|m| matches!(m, Message::User { .. }))
            .map(message_text)
            .collect()
    }

    /// The persisted assistant-message texts on the current branch (Pi `getAssistantTexts`,
    /// suite/harness.ts:57-61).
    pub async fn assistant_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .await
            .iter()
            .filter(|m| matches!(m, Message::Assistant(_)))
            .map(message_text)
            .collect()
    }
}

/// Extract the concatenated text of a message (Pi `getMessageText`, suite/harness.ts:34-49).
pub fn message_text(message: &Message) -> String {
    let content: &[Content] = match message {
        Message::User { content, .. } => content,
        Message::Assistant(a) => &a.content,
        Message::ToolResult { content, .. } => content,
    };
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn availability_for(options: &HarnessOptions) -> Availability {
    if let Some(allowed) = &options.allowed_tool_names {
        Availability::Allow(allowed.iter().cloned().collect::<HashSet<_>>())
    } else if let Some(initial) = &options.initial_active_tool_names {
        // [CYRUP-DELTA] cyrup has no toggleable active-set distinct from the visibility gate, so the
        // initial-active set seeds the visible (active) set when no explicit allow/exclude is given.
        Availability::Allow(initial.iter().cloned().collect::<HashSet<_>>())
    } else if let Some(excluded) = &options.excluded_tool_names {
        Availability::Exclude(excluded.iter().cloned().collect::<HashSet<_>>())
    } else {
        Availability::All
    }
}

fn build_config(cwd: std::path::PathBuf, options: &HarnessOptions) -> SessionConfig {
    let mut config = SessionConfig::new(cwd.clone(), cwd);
    config.app_mode = options.app_mode;
    config.persist = options.persist;
    config.system_prompt = options.system_prompt.clone();
    config.append_system_prompt = options.append_system_prompt.clone();
    config.model_pattern = options.model_pattern.clone();
    config.permission_policy = options.permission_policy.clone();
    config.tool_availability = availability_for(options);
    // Tests run in an isolated temp dir; trust it so resources/context load deterministically.
    config.trust_override = Some(true);
    config
}

/// Build a headless harness (Pi `createHarness`, test-harness.ts:432-439).
pub async fn create_harness(options: HarnessOptions) -> Result<Harness, HarnessError> {
    let temp = TestTempDir::new()?;
    let cwd = temp.path().to_path_buf();

    // The model catalog the scripted provider advertises. Precedence (highest first):
    //   1. an arbitrary `model` override (Pi `createHarness({ model })`, test-harness.ts:324,369);
    //   2. a multi-model `models` catalog (Pi `models?: FauxModelDefinition[]`, suite/harness.ts:64);
    //   3. the single default faux model, with an optional `contextWindow` override (Pi
    //      test-harness.ts:370: `{ ...baseModel, contextWindow }`).
    let models: Vec<Model> = if let Some(model) = options.model.clone() {
        vec![model]
    } else if !options.models.is_empty() {
        options.models.iter().map(faux_model_from_def).collect()
    } else {
        vec![match options.context_window {
            Some(cw) => faux_model_with_context_window(cw),
            None => faux_model(),
        }]
    };
    let model_provider = models
        .first()
        .map(|m| m.provider.clone())
        .unwrap_or_else(|| ProviderId::from("faux"));
    // Cycling (Pi `createFauxStreamFn`) vs queue-consuming (Pi `registerFauxProvider`) flavour.
    let (provider, faux_state) = if options.queue_responses {
        create_faux_stream_fn_queued(options.responses.clone(), models)
    } else {
        create_faux_stream_fn_with_models(options.responses.clone(), models)
    };
    let config = build_config(cwd.clone(), &options);

    // Auth (Pi `withConfiguredAuth`): seed a working faux runtime key, or leave the store empty for
    // the unauthenticated path (suite/harness.ts:108-136).
    let auth = Arc::new(AuthStore::at(cwd.join("auth.json")));
    if options.with_configured_auth {
        auth.set_runtime_api_key(model_provider, "faux-key".to_string());
    }

    let mut builder = SessionBuilder::new(provider.clone(), config)
        .cli_settings(options.settings.clone())
        .auth(auth);
    // Custom tools (Pi `tools` + `baseToolsOverride`) are injected via a synthetic native extension
    // whose `register_tool` overrides a built-in of the same name (R-08-012).
    if !options.tools.is_empty() {
        builder =
            builder.with_native_extension(Arc::new(ToolExtension::new(options.tools.clone())));
    }
    for ext in &options.native_extensions {
        builder = builder.with_native_extension(ext.clone());
    }
    let session = builder.build().await?;

    Ok(Harness {
        session,
        provider,
        faux_state,
        events: Arc::new(Mutex::new(Vec::new())),
        _temp: temp,
    })
}

/// A real-provider session + its temp dir (Pi `createTestSession`/`TestSessionContext`,
/// utilities.ts:172-177,234-278). Distinct from [`Harness`]: driven by a caller-supplied real
/// provider for e2e tests (no scripted faux, no event capture). Cleanup is RAII.
pub struct TestSession {
    /// The wired session.
    pub session: AgentSession,
    // RAII temp dir.
    _temp: TestTempDir,
}

impl TestSession {
    /// The wired session.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }
}

/// Options for [`create_test_session`] (Pi `TestSessionOptions`, utilities.ts:160-167).
#[derive(Clone, Debug, Default)]
pub struct TestSessionOptions {
    /// Persist to disk (Pi `inMemory` is the inverse; default in-memory).
    pub persist: bool,
    /// Custom system prompt (Pi `systemPrompt`).
    pub system_prompt: Option<String>,
    /// Model selection pattern (`provider/id[:level]`); `None` ⇒ first catalog entry.
    pub model_pattern: Option<String>,
}

/// Create an [`AgentSession`] over a caller-supplied real `provider` for e2e tests (Pi
/// `createTestSession`, utilities.ts:234-278). Built in a fresh temp dir with cyrup's coding tools
/// (via the session builder's built-in registry) and cleaned up on drop.
pub async fn create_test_session(
    provider: Arc<dyn cyrup_provider::Provider>,
    options: TestSessionOptions,
) -> Result<TestSession, HarnessError> {
    let temp = TestTempDir::new()?;
    let cwd = temp.path().to_path_buf();
    let mut config = SessionConfig::new(cwd.clone(), cwd);
    config.app_mode = AppMode::Print;
    config.persist = options.persist;
    config.system_prompt = options
        .system_prompt
        .or_else(|| Some("You are a helpful assistant. Be extremely concise.".to_string()));
    config.model_pattern = options.model_pattern;
    config.trust_override = Some(true);
    let session = SessionBuilder::new(provider, config).build().await?;
    Ok(TestSession {
        session,
        _temp: temp,
    })
}

/// Build a headless harness with inline native extensions (Pi `createHarnessWithExtensions`,
/// test-harness.ts:441-446). cyrup loads native built-ins through the session's extension host; the
/// extensions are supplied on [`HarnessOptions::native_extensions`].
pub async fn create_harness_with_extensions(
    options: HarnessOptions,
) -> Result<Harness, HarnessError> {
    create_harness(options).await
}
