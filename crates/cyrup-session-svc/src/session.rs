//! `AgentSession` — the single integration seam every front-end consumes (func-11 R-11-023).
//!
//! Wires the agent loop + tools + session persistence + config + resources + extensions behind one
//! async API: start/resume, prompt (→ an `EventStream<AgentSessionEvent>`), steer/follow-up,
//! interrupt, compaction, fork/branch + branch-summary, switch model — with durable persistence
//! across every turn. No mode reaches behaviour that does not flow through this object.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_agent::Agent;
use cyrup_core::{
    AssistantMessage, CancelToken, EntryId, EventStream, Message, ModelId, ModelRef, ProviderId,
    SessionId,
};
use cyrup_provider::Provider;
use cyrup_session::compaction::{
    BranchSummarySettings, CompactionReason, CompactionSettings, Compactor, NoHooks,
};
use cyrup_session::context::SessionContext;
use cyrup_session::manager::SessionManager;
use tokio::sync::Mutex as AsyncMutex;

use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{AgentSessionEvent, PromptAccepted, StreamingBehavior, UserInput};
use crate::services::AgentSessionServices;
use crate::subscriber::Fanout;

/// The integration seam (arch-11 §3.1). Cheaply shareable via `Arc`; every method is `&self`.
pub struct AgentSession {
    agent: Arc<Agent>,
    manager: Arc<AsyncMutex<SessionManager>>,
    fanout: Arc<Fanout>,
    provider: Arc<dyn Provider>,
    services: AgentSessionServices,
    /// The active model address (mutated by `set_model`).
    model: Mutex<ModelRef>,
    /// The resolved summarization/compaction model (kept in lockstep with `model`).
    compaction_model: Mutex<cyrup_provider::Model>,
    compaction_settings: CompactionSettings,
    branch_summary_settings: BranchSummarySettings,
    /// Long-lived token handed to the extension subscriber (distinct from per-run cancellation).
    session_cancel: CancelToken,
    session_id: SessionId,
}

impl AgentSession {
    /// Build from the assembled parts (called by [`crate::SessionBuilder::build`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        agent: Arc<Agent>,
        manager: Arc<AsyncMutex<SessionManager>>,
        fanout: Arc<Fanout>,
        provider: Arc<dyn Provider>,
        services: AgentSessionServices,
        model: ModelRef,
        session_cancel: CancelToken,
        session_id: SessionId,
    ) -> Self {
        let compaction_model = services.model.clone();
        Self {
            agent,
            manager,
            fanout,
            provider,
            services,
            model: Mutex::new(model),
            compaction_model: Mutex::new(compaction_model),
            compaction_settings: CompactionSettings::default(),
            branch_summary_settings: BranchSummarySettings::default(),
            session_cancel,
            session_id,
        }
    }

    /// Lock a `std::sync::Mutex` ignoring poisoning (no panic; arch-00 no-panic).
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -------------------------------------------------------------- subscriptions ----

    /// A long-lived event subscription (TUI / SDK observer) — lives until the stream is dropped.
    pub fn subscribe(&self) -> EventStream<AgentSessionEvent> {
        self.fanout.subscribe_persistent()
    }

    // ------------------------------------------------------------------- prompting ----

    /// Submit a user prompt and observe the run as a stream of [`AgentSessionEvent`] (R-11-005/007).
    ///
    /// The returned stream terminates after the run's `agent_end`. Errors only if the prompt could
    /// not be *accepted* (e.g. the agent is already streaming — use [`Self::steer`]/[`Self::follow_up`]).
    pub async fn prompt(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<EventStream<AgentSessionEvent>, SessionServiceError> {
        if self.is_streaming().await {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        // Register the run-scoped subscription BEFORE starting the run so no event is missed.
        let stream = self.fanout.subscribe_run();
        let msg = input.into().into_agent_message();
        self.agent.prompt(msg).await?;
        Ok(stream)
    }

    /// Submit a prompt, resolving only to the preflight acceptance (mirrors Pi). The run is observed
    /// via [`Self::subscribe`]. Used by adapters that manage their own persistent subscription.
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        let ui = input.into();
        if self.is_streaming().await {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        self.agent.prompt(ui.into_agent_message()).await?;
        Ok(PromptAccepted::Started)
    }

    /// Await full settlement of the in-flight run (`agent_end`). For print/one-shot modes (R-11-005).
    pub async fn wait_for_idle(&self) {
        self.agent.wait_for_idle().await;
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9).
    pub async fn steer(&self, input: impl Into<UserInput>) -> Result<PromptAccepted, SessionServiceError> {
        self.agent.steer(input.into().into_agent_message());
        Ok(PromptAccepted::Queued(StreamingBehavior::Steer))
    }

    /// Enqueue a follow-up message (delivered after the agent goes idle, func-02 §9).
    pub async fn follow_up(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.agent.follow_up(input.into().into_agent_message());
        Ok(PromptAccepted::Queued(StreamingBehavior::FollowUp))
    }

    /// Interrupt the active run (idempotent, R-11-018 / func-02 R-02-045).
    pub fn abort(&self) {
        self.agent.abort();
    }

    // ------------------------------------------------------------------- compaction ----

    /// Trigger a compaction of the current branch (R-11-014 `compact`). Emits
    /// `compaction_start`/`compaction_end` and appends a `CompactionEntry` to the session tree.
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<bool, SessionServiceError> {
        let reason = CompactionReason::Manual;
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        let model = { Self::lock(&self.compaction_model).clone() };
        let summarizer = DynSummarizer::new(self.provider.clone(), model.clone());
        let compactor = Compactor::new(summarizer, NoHooks);
        let cancel = self.session_cancel.child_token();

        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction(
                &mut guard,
                &model,
                &self.compaction_settings,
                reason,
                custom_instructions,
                false,
                cancel,
            )
            .await;
        drop(guard);

        let (compacted, aborted) = match &result {
            Ok(Some(_)) => (true, false),
            Ok(None) => (false, false),
            Err(_) => (false, true),
        };
        self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted }).await;
        result?;
        Ok(compacted)
    }

    // --------------------------------------------------------------- fork / branch ----

    /// Navigate the session leaf to `entry` (no file mutation; R-04-023).
    pub async fn branch(&self, entry: EntryId) -> Result<(), SessionServiceError> {
        self.manager.lock().await.branch(&entry)?;
        Ok(())
    }

    /// Navigate to `entry`, recording a branch-summary of the abandoned branch (R-04-024/R-05-016).
    /// Returns the summary text, if one was produced.
    pub async fn branch_with_summary(
        &self,
        entry: EntryId,
        user_wants_summary: bool,
    ) -> Result<Option<String>, SessionServiceError> {
        let model = { Self::lock(&self.compaction_model).clone() };
        let summarizer = DynSummarizer::new(self.provider.clone(), model.clone());
        let compactor = Compactor::new(summarizer, NoHooks);
        let cancel = self.session_cancel.child_token();

        let mut guard = self.manager.lock().await;
        let old_leaf = guard.leaf_id().cloned();
        let entry_opt = compactor
            .run_branch_summary(
                &mut guard,
                &model,
                entry,
                old_leaf,
                user_wants_summary,
                &self.branch_summary_settings,
                cancel,
            )
            .await?;
        Ok(entry_opt.map(|e| e.summary))
    }

    /// Fork the current persisted session into a new file under the same cwd (R-04-020/021).
    pub async fn fork(&self) -> Result<SessionId, SessionServiceError> {
        // A fork clones the active path through the current leaf into a new file.
        let mut guard = self.manager.lock().await;
        let root = guard.session_file().and_then(Path::parent).map(Path::to_path_buf);
        let layout = match root {
            Some(dir) => cyrup_session::SessionLayout::new(dir, guard.cwd().to_path_buf()),
            None => cyrup_session::SessionLayout::for_cwd(guard.cwd().to_path_buf()),
        };
        // Pi forks at an explicit leaf and mutates the manager in place
        // (`createBranchedSession(leafId)`, session-manager.ts:1292-1392). Fork-at-current-position
        // passes the current leaf; an empty session has nothing to fork.
        let leaf = guard.leaf_id().cloned().ok_or_else(|| {
            cyrup_session::SessionError::EmptyFork(
                guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
            )
        })?;
        guard.create_branched_session(&leaf, &layout)?;
        let id = guard.session_id().clone();
        Ok(id)
    }

    // --------------------------------------------------------------- model control ----

    /// Switch the active model by pattern (`provider/id[:level]`), updating the agent, the
    /// compaction model, and recording a model-change entry (R-11-014 `set_model`).
    pub async fn set_model(&self, pattern: &str) -> Result<ModelRef, SessionServiceError> {
        let resolved = {
            let available = self.provider.models();
            let resolver = cyrup_config::ModelResolver::new(available);
            let parsed = resolver.parse_pattern(pattern, true);
            parsed.model.ok_or_else(|| SessionServiceError::ModelNotFound(pattern.to_string()))?
        };
        let model_ref = ModelRef {
            provider: resolved.provider.clone(),
            api: Some(resolved.api.clone()),
            model: resolved.id.clone(),
        };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = model_ref.clone();
        *Self::lock(&self.compaction_model) = resolved.clone();

        // Record the change durably.
        self.manager
            .lock()
            .await
            .append_model_change(resolved.provider.clone(), resolved.id.clone())?;

        self.fanout_emit(AgentSessionEvent::ModelChanged {
            provider: resolved.provider.to_string(),
            model: resolved.id.to_string(),
        })
        .await;
        Ok(model_ref)
    }

    /// Set the active model directly from a provider+id pair (no pattern matching).
    pub async fn set_model_id(
        &self,
        provider: ProviderId,
        model: ModelId,
    ) -> Result<(), SessionServiceError> {
        let model_ref = ModelRef { provider: provider.clone(), api: None, model: model.clone() };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = model_ref;
        self.manager.lock().await.append_model_change(provider, model)?;
        Ok(())
    }

    // ----------------------------------------------------------------- read access ----

    /// The current model address.
    pub fn model(&self) -> ModelRef {
        Self::lock(&self.model).clone()
    }

    /// Whether a run is currently streaming.
    pub async fn is_streaming(&self) -> bool {
        self.agent.snapshot().await.is_streaming
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The on-disk session file, if this session is persisted.
    pub async fn session_file(&self) -> Option<std::path::PathBuf> {
        self.manager.lock().await.session_file().map(Path::to_path_buf)
    }

    /// The cwd-bound services this session wired (settings/auth/resources/ext host/model/prompt).
    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    /// The assembled system prompt for this session (arch-06).
    pub fn system_prompt(&self) -> &str {
        &self.services.system_prompt
    }

    /// The current LLM context built from the session tree (leaf→root, R-04-011).
    pub async fn context(&self) -> SessionContext {
        self.manager.lock().await.build_context()
    }

    /// The persisted transcript messages on the current branch (R-11-014 `get_messages`).
    pub async fn messages(&self) -> Vec<Message> {
        self.manager.lock().await.build_context().messages
    }

    /// The agent's current in-memory transcript (includes the streaming partial).
    pub async fn agent_messages(&self) -> Vec<cyrup_agent::AgentMessage> {
        self.agent.snapshot().await.messages
    }

    /// The most recent assistant message text on the current branch (print-mode helper).
    pub async fn last_assistant_text(&self) -> Option<String> {
        self.messages().await.into_iter().rev().find_map(|m| match m {
            Message::Assistant(AssistantMessage { content, .. }) => {
                let text: String = content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() { None } else { Some(text) }
            }
            _ => None,
        })
    }

    async fn fanout_emit(&self, ev: AgentSessionEvent) {
        // Reuse the same fan-out the agent subscriber feeds; session-level events interleave with
        // agent events on the live streams.
        self.fanout.emit_external(ev).await;
    }
}
