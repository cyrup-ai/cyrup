//! `AgentSession` — the single integration seam every front-end consumes (func-11 R-11-023).
//!
//! Wires the agent loop + tools + session persistence + config + resources + extensions behind one
//! async API: start/resume, prompt (→ an `EventStream<AgentSessionEvent>`), steer/follow-up,
//! interrupt, compaction, fork/branch + branch-summary, switch model — with durable persistence
//! across every turn. No mode reaches behaviour that does not flow through this object.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_agent::{Agent, AgentMessage};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, EntryId, EventStream, Message, ModelId,
    ModelRef, ModelThinkingLevel, ProviderId, SessionId,
};
use cyrup_ext::{HostEvent, Reduced};
use cyrup_provider::{is_context_overflow, is_retryable_assistant_error, Model, Provider};
use cyrup_session::compaction::{
    BranchSummarySettings, CompactionReason, CompactionSettings, Compactor, NoHooks,
};
use cyrup_session::context::SessionContext;
use cyrup_session::manager::SessionManager;
use cyrup_tools::{ProcOps, ShellConfig};
use tokio::sync::Mutex as AsyncMutex;

use crate::bash::{bash_message_payload, run_bash, BashOptions, BashResult};
use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{
    core_message_to_agent, AgentSessionEvent, PromptAccepted, StreamingBehavior, UserInput,
};
use crate::services::AgentSessionServices;
use crate::subscriber::Fanout;
use crate::tools::{DynamicToolState, ToolInfo};

/// Where a fork anchors relative to the selected entry (Pi `fork(entryId, {position})`,
/// agent-session-runtime.ts:259). `Before` anchors at the selected *user* message's parent and
/// extracts its text (for re-editing); `At` anchors at the selected entry itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

/// The outcome of an entry-anchored fork (Pi returns `{cancelled, selectedText}`,
/// agent-session-runtime.ts:262).
#[derive(Clone, Debug, Default)]
pub struct ForkOutcome {
    /// The new branched session id (the forked file's session id), if a new file was created.
    pub session_id: Option<SessionId>,
    /// For `position:"before"`, the selected user message's text (so a UI can pre-fill the editor).
    pub selected_text: Option<String>,
}

/// A single user message anchor for the `/tree`/`/fork` pickers (Pi `getUserMessagesForForking`,
/// agent-session.ts:2901).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkAnchor {
    pub entry_id: EntryId,
    pub text: String,
}

/// A scoped model in the `cycle_model` set (Pi `{model, thinkingLevel?}`, agent-session.ts:870). An
/// explicit `thinking_level` overrides the session level when cycled to; `None` inherits it.
#[derive(Clone, Debug)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// The typed result of [`AgentSession::cycle_model`] (Pi `ModelCycleResult`, agent-session.ts:1471).
/// `is_scoped` distinguishes the scoped-set path from the full-catalog path.
#[derive(Clone, Debug)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    pub is_scoped: bool,
}

/// The build-time inputs the facade threads into [`AgentSession::from_parts`] for the subsystems
/// added beyond the core seam (retry/auto-compaction/bash/dynamic-tools/attribution). Grouped to
/// keep the constructor signature bounded.
pub(crate) struct SessionExtras {
    pub telemetry_enabled: bool,
    pub compaction_settings: CompactionSettings,
    pub branch_summary_settings: BranchSummarySettings,
    pub auto_compaction_enabled: bool,
    pub auto_retry_enabled: bool,
    pub retry_max_retries: u32,
    pub retry_base_delay_ms: u64,
    pub proc: Arc<dyn ProcOps>,
    pub shell: ShellConfig,
    pub dynamic_tools: DynamicToolState,
}

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
    /// Facade-side mirror of the steering queue text (Pi `_steeringMessages`, agent-session.ts:476)
    /// for `queue_update` emission + introspection; the authoritative queue lives in the agent.
    steering_messages: Mutex<Vec<String>>,
    /// Facade-side mirror of the follow-up queue text (Pi `_followUpMessages`, agent-session.ts:477).
    follow_up_messages: Mutex<Vec<String>>,
    /// Warning surfaced when a resumed session's saved model could not be restored (Pi
    /// `modelFallbackMessage`, sdk.ts:91/192). `None` when the model resolved cleanly.
    model_fallback_message: Option<String>,
    /// Cancel handle for an in-flight manual compaction (Pi `_compactionAbortController`,
    /// agent-session.ts:1654); set while [`Self::compact`] runs, cleared in its `finally`.
    compaction_cancel: Mutex<Option<CancelToken>>,
    /// Cancel handle for an in-flight branch summarization (Pi `_branchSummaryAbortController`,
    /// agent-session.ts:1796).
    branch_summary_cancel: Mutex<Option<CancelToken>>,
    /// Messages staged to ride the NEXT prompt turn (Pi `_pendingNextTurnMessages`,
    /// agent-session.ts:1339); drained into the run by [`Self::assemble_run_messages`].
    pending_next_turn: Mutex<Vec<AgentMessage>>,
    /// Models available for `cycle_model` (Pi `_scopedModels`, agent-session.ts:870).
    scoped_models: Mutex<Vec<ScopedModel>>,
    /// Facade mirror of the agent's steering-queue mode (the agent exposes only a setter; Pi reads
    /// `agent.steeringMode`, agent-session.ts:845).
    steering_mode: Mutex<cyrup_agent::QueueMode>,
    /// Facade mirror of the agent's follow-up-queue mode (Pi `agent.followUpMode`, :850).
    follow_up_mode: Mutex<cyrup_agent::QueueMode>,
    /// Whether provider install-telemetry is on (gates default attribution headers, Pi sdk.ts:323).
    telemetry_enabled: bool,
    // ---- retry subsystem (Pi agent-session.ts:778,2484-2572) ----
    /// Current retry attempt (0 when not retrying; Pi `_retryAttempt`).
    retry_attempt: Mutex<u32>,
    /// Cancel handle for the in-flight backoff sleep (Pi `_retryAbortController`).
    retry_cancel: Mutex<Option<CancelToken>>,
    /// Runtime override of the settings `retry.enabled` toggle (Pi `setAutoRetryEnabled`).
    auto_retry_override: Mutex<Option<bool>>,
    /// `retry.enabled` default sourced from settings at build time.
    retry_enabled_default: bool,
    retry_max_retries: u32,
    retry_base_delay_ms: u64,
    // ---- auto-compaction (Pi agent-session.ts:831,1811-1905,2078-2086) ----
    /// Runtime override of the settings `compaction.enabled` toggle (Pi `setAutoCompactionEnabled`).
    auto_compaction_override: Mutex<Option<bool>>,
    auto_compaction_enabled_default: bool,
    /// Cancel handle for an in-flight auto-compaction (Pi `_autoCompactionAbortController`).
    auto_compaction_cancel: Mutex<Option<CancelToken>>,
    /// Set once after an overflow auto-compaction so a second overflow does not loop (Pi
    /// `_overflowRecoveryAttempted`, agent-session.ts:1859).
    overflow_recovery_attempted: Mutex<bool>,
    // ---- immediate-bash seam (Pi agent-session.ts:2582-2684) ----
    proc: Arc<dyn ProcOps>,
    shell: ShellConfig,
    /// Cancel handle for an in-flight `execute_bash` (Pi `_bashAbortController`).
    bash_cancel: Mutex<Option<CancelToken>>,
    /// Bash messages deferred while a run streams, flushed after the turn (Pi `_pendingBashMessages`).
    pending_bash: Mutex<Vec<AgentMessage>>,
    // ---- dynamic tools (Pi agent-session.ts:786-828,2304) ----
    dynamic_tools: Mutex<DynamicToolState>,
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
        model_fallback_message: Option<String>,
        extras: SessionExtras,
    ) -> Self {
        let compaction_model = services.model.clone();
        // Seed the queue-mode mirrors from the resolved settings (the builder wired the same modes
        // into the agent), so the getters report the live mode without an agent-side getter.
        let eff = services.settings.effective();
        let steering_mode = crate::builder::parse_queue_mode(&eff.steering_mode());
        let follow_up_mode = crate::builder::parse_queue_mode(&eff.follow_up_mode());
        Self {
            agent,
            manager,
            fanout,
            provider,
            services,
            model: Mutex::new(model),
            compaction_model: Mutex::new(compaction_model),
            compaction_settings: extras.compaction_settings,
            branch_summary_settings: extras.branch_summary_settings,
            session_cancel,
            session_id,
            steering_messages: Mutex::new(Vec::new()),
            follow_up_messages: Mutex::new(Vec::new()),
            model_fallback_message,
            compaction_cancel: Mutex::new(None),
            branch_summary_cancel: Mutex::new(None),
            pending_next_turn: Mutex::new(Vec::new()),
            scoped_models: Mutex::new(Vec::new()),
            steering_mode: Mutex::new(steering_mode),
            follow_up_mode: Mutex::new(follow_up_mode),
            telemetry_enabled: extras.telemetry_enabled,
            retry_attempt: Mutex::new(0),
            retry_cancel: Mutex::new(None),
            auto_retry_override: Mutex::new(None),
            retry_enabled_default: extras.auto_retry_enabled,
            retry_max_retries: extras.retry_max_retries,
            retry_base_delay_ms: extras.retry_base_delay_ms,
            auto_compaction_override: Mutex::new(None),
            auto_compaction_enabled_default: extras.auto_compaction_enabled,
            auto_compaction_cancel: Mutex::new(None),
            overflow_recovery_attempted: Mutex::new(false),
            proc: extras.proc,
            shell: extras.shell,
            bash_cancel: Mutex::new(None),
            pending_bash: Mutex::new(Vec::new()),
            dynamic_tools: Mutex::new(extras.dynamic_tools),
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
        let messages = self.prepare_and_assemble(input.into()).await?;
        self.agent.prompt(messages).await?;
        Ok(stream)
    }

    /// Submit a prompt, resolving only to the preflight acceptance (mirrors Pi). The run is observed
    /// via [`Self::subscribe`]. Used by adapters that manage their own persistent subscription.
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        if self.is_streaming().await {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        let messages = self.prepare_and_assemble(input.into()).await?;
        self.agent.prompt(messages).await?;
        Ok(PromptAccepted::Started)
    }

    /// Run the pre-send sequence Pi's `prompt` performs before dispatching the run
    /// (agent-session.ts:1037-1083): expand skill/prompt-template commands, flush any pending bash
    /// messages, run the `hasConfiguredAuth` precheck, and perform the pre-send compaction check
    /// (which catches an aborted last response). Then assemble the run input (`before_agent_start`
    /// hook + ordering). Returns the assembled run messages. Errors before any persistence on an auth
    /// miss (Pi `_getRequiredRequestAuth` throw → `preflightResult?.(false)`).
    async fn prepare_and_assemble(
        &self,
        mut input: UserInput,
    ) -> Result<Vec<AgentMessage>, SessionServiceError> {
        // 1. Skill (`/skill:name`) + prompt-template (`/name args`) expansion (agent-session.ts:1037).
        if input.expand_templates {
            input.text = self.expand_input_text(&input.text);
        }
        // 2. Flush deferred bash messages so ordering is intact (agent-session.ts:1058).
        self.flush_pending_bash_messages().await;
        // 3. Auth precheck: the active model must have configured auth (agent-session.ts:1062-1075).
        {
            let model = Self::lock(&self.compaction_model).clone();
            if !self.has_configured_auth(&model) {
                return Err(SessionServiceError::NoConfiguredAuth(format!(
                    "{}/{}",
                    model.provider.as_str(),
                    model.id.as_str()
                )));
            }
        }
        // 4. Pre-send compaction check on the last assistant turn (agent-session.ts:1080-1083).
        if self.auto_compaction_enabled()
            && let Some(last) = self.last_assistant_message().await
        {
            let _ = self.check_compaction(&last, false).await?;
        }
        // 5. Assemble (before_agent_start hook + ordering).
        Ok(self.assemble_run_messages(input).await)
    }

    /// Expand a `/skill:name args` command to the skill block + args, or a `/name args` prompt
    /// template, leaving any other text unchanged (Pi `_expandSkillCommand` + `expandPromptTemplate`,
    /// agent-session.ts:1174-1204,1037-1041).
    fn expand_input_text(&self, text: &str) -> String {
        let expanded = self.expand_skill_command(text);
        let templates: Vec<_> = self.prompt_templates().winners().collect();
        cyrup_resources::expand_prompt_template(&expanded, templates)
    }

    /// `/skill:name args` → the skill block (Pi `_expandSkillCommand`, agent-session.ts:1174). Unknown
    /// skills / read failures pass the text through unchanged.
    fn expand_skill_command(&self, text: &str) -> String {
        let Some(rest) = text.strip_prefix("/skill:") else { return text.to_string() };
        let (name, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        let Some(skill) = self.services.resources.skills.winners().find(|s| s.name == name) else {
            return text.to_string();
        };
        let Ok(content) = std::fs::read_to_string(&skill.skill_md) else {
            return text.to_string();
        };
        let body = strip_frontmatter(&content).trim().to_string();
        let block = format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name,
            skill.skill_md.display(),
            skill.dir.display(),
            body
        );
        if args.is_empty() {
            block
        } else {
            format!("{block}\n\n{args}")
        }
    }

    /// The most recent assistant message on the current branch as a full [`AssistantMessage`] (for
    /// the compaction/retry checks), or `None`.
    async fn last_assistant_message(&self) -> Option<AssistantMessage> {
        self.messages().await.into_iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
    }

    /// Run the `before_agent_start` extension hook and assemble the run's input messages (R-06-014;
    /// Pi agent-session.ts:1105-1131). The hook chain may (a) **replace** the system prompt — applied
    /// to the agent before the run, and reset to the assembled base when no handler replaced it — and
    /// (b) **inject** additional messages, which are appended after the user message. Without this the
    /// assembled prompt was never offered to extensions (the gap the facade closes).
    async fn assemble_run_messages(&self, input: UserInput) -> Vec<AgentMessage> {
        let user_text = input.text.clone();
        let images = input.images.clone();
        let user_msg = input.into_agent_message();
        // Drain any messages staged for this turn (Pi `_pendingNextTurnMessages`,
        // agent-session.ts:1099-1103); they are injected AFTER the user message in the run input.
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_next_turn));

        let base = &self.services.system_prompt;
        // Fast path: no extension listens for `before_agent_start` — keep the assembled base prompt.
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::BeforeAgentStart)
        {
            let mut messages = vec![user_msg];
            messages.extend(pending);
            return messages;
        }

        let event = HostEvent::BeforeAgentStart {
            prompt: user_text,
            images: serde_json::to_value(&images).unwrap_or(serde_json::Value::Null),
            system_prompt: base.clone(),
            options: serde_json::Value::Null,
            injected: Vec::new(),
        };
        let cancel = self.session_cancel.child_token();
        let reduced = self.services.ext_host.dispatcher().dispatch_block_mutate(event, &cancel).await;

        let mut messages = vec![user_msg];
        messages.extend(pending);
        if let Reduced::Pass(ev) = reduced
            && let HostEvent::BeforeAgentStart { system_prompt, injected, .. } = *ev
        {
            // Apply the (possibly handler-replaced) system prompt; reset to base otherwise.
            if &system_prompt == base {
                self.agent.set_system_prompt(base.clone()).await;
            } else {
                self.agent.set_system_prompt(system_prompt).await;
            }
            messages.extend(injected.iter().map(core_message_to_agent));
        } else {
            // Blocked/Handled (no Pi analogue here): keep the base prompt, no injection.
            self.agent.set_system_prompt(base.clone()).await;
        }
        messages
    }

    /// Await full settlement of the in-flight run (`agent_end`). For print/one-shot modes (R-11-005).
    pub async fn wait_for_idle(&self) {
        self.agent.wait_for_idle().await;
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9). Mirrors the
    /// text into the facade queue + emits `queue_update` (Pi `_queueSteer`, agent-session.ts:1249).
    pub async fn steer(&self, input: impl Into<UserInput>) -> Result<PromptAccepted, SessionServiceError> {
        let ui = input.into();
        Self::lock(&self.steering_messages).push(ui.text.clone());
        self.agent.steer(ui.into_agent_message());
        self.emit_queue_update().await;
        Ok(PromptAccepted::Queued(StreamingBehavior::Steer))
    }

    /// Enqueue a follow-up message (delivered after the agent goes idle, func-02 §9). Mirrors the
    /// text into the facade queue + emits `queue_update` (Pi `_queueFollowUp`, agent-session.ts:1266).
    pub async fn follow_up(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        let ui = input.into();
        Self::lock(&self.follow_up_messages).push(ui.text.clone());
        self.agent.follow_up(ui.into_agent_message());
        self.emit_queue_update().await;
        Ok(PromptAccepted::Queued(StreamingBehavior::FollowUp))
    }

    /// The pending steering messages, in order (Pi `getSteeringMessages`, agent-session.ts:1408).
    pub fn steering_messages(&self) -> Vec<String> {
        Self::lock(&self.steering_messages).clone()
    }

    /// The pending follow-up messages, in order (Pi `getFollowUpMessages`, agent-session.ts:1412).
    pub fn follow_up_messages(&self) -> Vec<String> {
        Self::lock(&self.follow_up_messages).clone()
    }

    /// Total queued (steering + follow-up) message count (Pi `pendingMessageCount`,
    /// agent-session.ts:1393).
    pub fn pending_message_count(&self) -> usize {
        Self::lock(&self.steering_messages).len() + Self::lock(&self.follow_up_messages).len()
    }

    /// Clear both queues (Pi `clearQueue`, agent-session.ts:1416): drains the agent's authoritative
    /// queues and the facade mirrors, then emits `queue_update`.
    pub async fn clear_queue(&self) {
        self.agent.clear_all_queues();
        Self::lock(&self.steering_messages).clear();
        Self::lock(&self.follow_up_messages).clear();
        self.emit_queue_update().await;
    }

    /// Emit a `queue_update` snapshot of both facade queues (Pi `_emitQueueUpdate`,
    /// agent-session.ts:1382).
    async fn emit_queue_update(&self) {
        let steering = Self::lock(&self.steering_messages).clone();
        let follow_up = Self::lock(&self.follow_up_messages).clone();
        self.fanout_emit(AgentSessionEvent::QueueUpdate { steering, follow_up }).await;
    }

    /// Interrupt the active run (idempotent, R-11-018 / func-02 R-02-045).
    pub fn abort(&self) {
        self.agent.abort();
    }

    // ------------------------------------------------------------------- compaction ----

    /// Trigger a compaction of the current branch (R-11-014 `compact`; Pi `compact`,
    /// agent-session.ts:1647-1788). Aborts any active run first, emits
    /// `compaction_start`/`compaction_end`, offers the extension `session_before_compact` veto hook,
    /// appends a `CompactionEntry`, and notifies `session_compact`. Returns the
    /// [`CompactionResult`], or `None` when there was nothing to compact or a handler cancelled.
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<Option<crate::state::CompactionResult>, SessionServiceError> {
        let reason = CompactionReason::Manual;
        // Disconnect/abort dance: stop the active run before compacting (agent-session.ts:1648-1649).
        self.abort();
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.compaction_cancel) = Some(cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        // session_before_compact ext hook: a handler may veto (cancel) the compaction
        // (agent-session.ts:1672-1693).
        if !self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::SessionBeforeCompact)
        {
            let reduced = self
                .services
                .ext_host
                .dispatcher()
                .dispatch_block_mutate(HostEvent::SessionBeforeCompact, &cancel)
                .await;
            if matches!(reduced, Reduced::Blocked { .. }) {
                *Self::lock(&self.compaction_cancel) = None;
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: true }).await;
                return Ok(None);
            }
        }

        let model = { Self::lock(&self.compaction_model).clone() };
        let summarizer = DynSummarizer::new(self.provider.clone(), model.clone());
        let compactor = Compactor::new(summarizer, NoHooks);

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
        // Estimate the rebuilt context size for the result payload (Pi `estimateMessagesTokens`).
        let estimated_tokens_after: u64 = guard
            .build_context()
            .messages
            .iter()
            .map(cyrup_provider::estimate_message_tokens)
            .sum();
        drop(guard);
        *Self::lock(&self.compaction_cancel) = None;

        match result {
            Ok(Some(entry)) => {
                let cr = crate::state::CompactionResult {
                    summary: entry.summary.clone(),
                    first_kept_entry_id: entry.first_kept_entry_id.to_string(),
                    tokens_before: entry.tokens_before,
                    estimated_tokens_after,
                    details: entry.details.clone(),
                };
                // session_compact ext notify (agent-session.ts:1740-1747).
                let notify_cancel = self.session_cancel.child_token();
                self.services
                    .ext_host
                    .dispatcher()
                    .dispatch_notify(
                        &HostEvent::SessionCompact { summary: cr.summary.clone() },
                        &notify_cancel,
                    )
                    .await;
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: false }).await;
                Ok(Some(cr))
            }
            Ok(None) => {
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: false }).await;
                Ok(None)
            }
            Err(e) => {
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted }).await;
                Err(e.into())
            }
        }
    }

    /// Cancel an in-flight manual/auto compaction (Pi `abortCompaction`, agent-session.ts:1788).
    pub fn abort_compaction(&self) {
        if let Some(c) = Self::lock(&self.compaction_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Cancel an in-flight branch summarization (Pi `abortBranchSummary`, agent-session.ts:1796).
    pub fn abort_branch_summary(&self) {
        if let Some(c) = Self::lock(&self.branch_summary_cancel).as_ref() {
            c.cancel();
        }
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
        *Self::lock(&self.branch_summary_cancel) = Some(cancel.clone());

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
            .await;
        drop(guard);
        *Self::lock(&self.branch_summary_cancel) = None;
        Ok(entry_opt?.map(|e| e.summary))
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

    /// Clone the session at an explicit entry (or the current leaf when `None`) into a new file,
    /// WITHOUT switching the active session to it (arch-11 `clone_at`; distinct from `fork`, which
    /// switches). Returns the new branched session id. Unlike `fork_at_entry`'s `before` anchoring,
    /// `clone_at` anchors the branch leaf at the selected entry itself (the full path up to and
    /// including it is cloned).
    pub async fn clone_at(&self, entry: Option<EntryId>) -> Result<SessionId, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let leaf = match entry {
            Some(e) => {
                guard
                    .entry(&e)
                    .ok_or_else(|| SessionServiceError::InvalidForkEntry(e.to_string()))?;
                e
            }
            None => guard.leaf_id().cloned().ok_or_else(|| {
                cyrup_session::SessionError::EmptyFork(
                    guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
                )
            })?,
        };
        let root = guard.session_file().and_then(Path::parent).map(Path::to_path_buf);
        let layout = match root {
            Some(dir) => cyrup_session::SessionLayout::new(dir, guard.cwd().to_path_buf()),
            None => cyrup_session::SessionLayout::for_cwd(guard.cwd().to_path_buf()),
        };
        guard.create_branched_session(&leaf, &layout)?;
        Ok(guard.session_id().clone())
    }

    /// Entry-anchored fork (Pi `fork(entryId, {position})`, agent-session-runtime.ts:259-344). For
    /// `position:"before"` the anchor must be a *user* message; the new branch leaf is that message's
    /// parent and its text is returned as `selected_text` (so a UI can re-edit it). For
    /// `position:"at"` the new branch leaf is the selected entry itself. A persisted session forks
    /// into a new file via `createBranchedSession(leafId)`; an anchor with no parent (forking before
    /// the very first message) yields a fresh empty session.
    pub async fn fork_at_entry(
        &self,
        entry: &EntryId,
        position: ForkPosition,
    ) -> Result<ForkOutcome, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let (target_leaf, selected_text) = fork_anchor(&guard, entry, position)?;

        match target_leaf {
            Some(leaf) => {
                let root = guard.session_file().and_then(Path::parent).map(Path::to_path_buf);
                let layout = match root {
                    Some(dir) => cyrup_session::SessionLayout::new(dir, guard.cwd().to_path_buf()),
                    None => cyrup_session::SessionLayout::for_cwd(guard.cwd().to_path_buf()),
                };
                guard.create_branched_session(&leaf, &layout)?;
                let id = guard.session_id().clone();
                Ok(ForkOutcome { session_id: Some(id), selected_text })
            }
            // Forking before the first user message: nothing to branch from.
            None => Ok(ForkOutcome { session_id: None, selected_text }),
        }
    }

    /// Enumerate the user-message fork anchors on the current branch (Pi `getUserMessagesForForking`,
    /// agent-session.ts:2901) — each `{entry_id, text}` is a candidate the `/fork`/`/tree` UI offers.
    pub async fn user_messages_for_forking(&self) -> Vec<ForkAnchor> {
        let guard = self.manager.lock().await;
        let leaf = guard.leaf_id().cloned();
        guard
            .branch_path(leaf.as_ref())
            .into_iter()
            .filter_map(|e| user_message_text(e).map(|text| ForkAnchor { entry_id: e.id(), text }))
            .collect()
    }

    // --------------------------------------------------------------- naming / export ----

    /// The session's display name, if set (Pi `sessionName` getter, agent-session.ts:865).
    pub async fn session_name(&self) -> Option<String> {
        self.manager.lock().await.session_name()
    }

    /// Set the session's display name, persisting a `session_info` entry (Pi `setSessionName`,
    /// agent-session.ts:2690).
    pub async fn set_session_name(&self, name: &str) -> Result<(), SessionServiceError> {
        self.manager.lock().await.append_session_info(name)?;
        Ok(())
    }

    /// Export the current session tree as JSONL (Pi `exportToJsonl`, agent-session.ts:3052). With a
    /// `path` the bytes are written there; otherwise the JSONL text is returned.
    pub async fn export_to_jsonl(
        &self,
        path: Option<&Path>,
    ) -> Result<Option<String>, SessionServiceError> {
        let guard = self.manager.lock().await;
        let mut buf: Vec<u8> = Vec::new();
        guard.export_jsonl(&mut buf)?;
        drop(guard);
        let text = String::from_utf8_lossy(&buf).into_owned();
        match path {
            Some(p) => {
                std::fs::write(p, text).map_err(|e| SessionServiceError::Io(e.to_string()))?;
                Ok(None)
            }
            None => Ok(Some(text)),
        }
    }

    // ------------------------------------------------------------------- lifecycle ----

    /// Dispose the session (Pi `AgentSession.dispose` via runtime `dispose`,
    /// agent-session-runtime.ts:390): abort any in-flight run, emit `session_shutdown`, and cancel
    /// the long-lived session token so the extension subscriber unwinds.
    pub async fn dispose(&self, reason: &str) {
        self.abort();
        self.fanout_emit(AgentSessionEvent::SessionShutdown { reason: reason.to_string() }).await;
        // Notify extensions, then release the long-lived token.
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::SessionShutdown { reason: reason.to_string() }, &cancel)
            .await;
        self.session_cancel.cancel();
    }

    /// Invalidate every live subscription on replacement (R-11-021): emit the terminal
    /// `SessionReplaced{generation}` and drop all senders so consumers re-subscribe.
    pub async fn notify_replaced(&self, generation: u64) {
        self.fanout.invalidate(generation).await;
    }

    /// Announce this (freshly-installed) session to its subscribers + extensions (Pi `session_start`,
    /// agent-session-runtime.ts:215). `reason` ∈ `new`/`resume`/`fork`/`reload`.
    pub async fn emit_session_start(&self, reason: &str, previous_session_file: Option<String>) {
        self.fanout_emit(AgentSessionEvent::SessionStart {
            reason: reason.to_string(),
            previous_session_file,
        })
        .await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::SessionStart { reason: reason.to_string() }, &cancel)
            .await;
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
        self.set_model_resolved(resolved).await
    }

    /// Switch to a resolved [`Model`] (Pi `setModel(Model)`, agent-session.ts:1448-1463), running the
    /// `hasConfiguredAuth` precheck first (our auth proxy: the model must be in the live provider
    /// catalog). Updates the agent + compaction model + attribution headers + host-services view and
    /// records a `model_change` entry.
    pub async fn set_model_resolved(&self, model: Model) -> Result<ModelRef, SessionServiceError> {
        if !self.has_configured_auth(&model) {
            return Err(SessionServiceError::NoConfiguredAuth(format!(
                "{}/{}",
                model.provider.as_str(),
                model.id.as_str()
            )));
        }
        let previous = Self::lock(&self.model).clone();
        self.apply_model_change(&model, &previous, "set", None).await?;
        Ok(ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        })
    }

    /// Whether the model has usable auth (Pi `modelRegistry.hasConfiguredAuth`, agent-session.ts:1449).
    /// cyrup's auth proxy (gap doc §3): a model the injected provider exposes in its catalog is
    /// usable — the provider already resolved its credentials when it was constructed.
    pub fn has_configured_auth(&self, model: &Model) -> bool {
        self.provider
            .models()
            .iter()
            .any(|m| m.provider == model.provider && m.id == model.id)
    }

    /// The provider-attribution + session-affinity headers this session attaches to provider requests
    /// for `model` (Pi `mergeProviderAttributionHeaders`, sdk.ts:323; #20). Computed from the merge
    /// function + the session's telemetry flag + id. The builder threads the resolved model's headers
    /// onto the agent at construction; this getter lets callers inspect/recompute them per model.
    pub fn attribution_headers(&self, model: &Model) -> Option<cyrup_provider::HeaderMap> {
        crate::attribution::merge_provider_attribution_headers(
            model,
            self.telemetry_enabled,
            Some(&self.session_id),
            &[],
        )
    }

    /// Emit the `model_select` extension event when the model actually changes (Pi `_emitModelSelect`,
    /// agent-session.ts:1429-1440). `source` ∈ `set`/`cycle`/`restore`. No-op when the model is
    /// unchanged (Pi `modelsAreEqual` guard).
    async fn emit_model_select(
        &self,
        next: &cyrup_provider::Model,
        previous: &ModelRef,
        source: &str,
    ) {
        // `modelsAreEqual`: same provider + id.
        if previous.provider == next.provider && previous.model == next.id {
            return;
        }
        let cancel = self.session_cancel.child_token();
        let model_val = serde_json::json!({
            "provider": next.provider.as_str(),
            "id": next.id.as_str(),
            "previousModel": { "provider": previous.provider.as_str(), "id": previous.model.as_str() },
            "source": source,
        });
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::ModelSelect { model: model_val }, &cancel)
            .await;
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

    // --------------------------------------------------------------- thinking control ----

    /// The agent's current thinking level (Pi `thinkingLevel` getter, agent-session.ts:763).
    pub async fn thinking_level(&self) -> ModelThinkingLevel {
        self.agent.snapshot().await.thinking_level
    }

    /// The thinking levels the active model supports (Pi `getAvailableThinkingLevels`,
    /// agent-session.ts:1576). A non-reasoning model supports only `off`.
    pub fn available_thinking_levels(&self) -> Vec<ModelThinkingLevel> {
        let model = { Self::lock(&self.compaction_model).clone() };
        cyrup_provider::get_supported_thinking_levels(&model)
    }

    /// Whether the active model supports reasoning/thinking (Pi `supportsThinking`,
    /// agent-session.ts:1585).
    pub fn supports_thinking(&self) -> bool {
        Self::lock(&self.compaction_model).reasoning
    }

    /// Set the thinking level, clamping to the model's capabilities, persisting a
    /// `thinking_level_change` entry and emitting the `thinking_level_select` ext event + the
    /// facade event — but only when the effective level actually changes (Pi `setThinkingLevel`,
    /// agent-session.ts:1541-1572).
    pub async fn set_thinking_level(
        &self,
        level: ModelThinkingLevel,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model = { Self::lock(&self.compaction_model).clone() };
        let effective = cyrup_provider::clamp_thinking_level(&model, level);
        let previous = self.agent.snapshot().await.thinking_level;
        self.agent.set_thinking_level(effective).await;
        if effective == previous {
            return Ok(effective);
        }
        let level_str = crate::builder::thinking_level_to_str(effective);
        self.manager.lock().await.append_thinking_level_change(&level_str)?;
        self.services.host_services.update_model(
            Self::lock(&self.model).clone(),
            model.context_window,
            Some(level_str.clone()),
        );
        self.fanout_emit(AgentSessionEvent::ThinkingLevelChanged { level: level_str.clone() }).await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::ThinkingLevelSelect { level: level_str }, &cancel)
            .await;
        Ok(effective)
    }

    /// Cycle to the next thinking level (Pi `cycleThinkingLevel`, agent-session.ts:1551). Returns
    /// `None` when the model does not support thinking.
    pub async fn cycle_thinking_level(&self) -> Result<Option<ModelThinkingLevel>, SessionServiceError> {
        if !self.supports_thinking() {
            return Ok(None);
        }
        let levels = self.available_thinking_levels();
        if levels.is_empty() {
            return Ok(None);
        }
        let current = self.thinking_level().await;
        let idx = levels.iter().position(|l| *l == current).unwrap_or(0);
        let Some(&next) = levels.get((idx + 1) % levels.len()) else {
            return Ok(None);
        };
        Ok(Some(self.set_thinking_level(next).await?))
    }

    // ----------------------------------------------------- steering / follow-up mode ----

    /// The agent's current steering mode (Pi `steeringMode` getter, agent-session.ts:845).
    pub fn steering_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.steering_mode)
    }

    /// The agent's current follow-up mode (Pi `followUpMode` getter, agent-session.ts:850).
    pub fn follow_up_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.follow_up_mode)
    }

    /// Set the steering-message delivery mode (Pi `setSteeringMode`, agent-session.ts:1631).
    pub fn set_steering_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_steering_mode(mode);
        *Self::lock(&self.steering_mode) = mode;
    }

    /// Set the follow-up-message delivery mode (Pi `setFollowUpMode`, agent-session.ts:1640).
    pub fn set_follow_up_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_follow_up_mode(mode);
        *Self::lock(&self.follow_up_mode) = mode;
    }

    // ----------------------------------------------------------------- read access ----

    /// The current model address.
    pub fn model(&self) -> ModelRef {
        Self::lock(&self.model).clone()
    }

    /// The model-restore fallback warning, if the resumed session's saved model was unavailable
    /// (Pi `modelFallbackMessage`, sdk.ts:91).
    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
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

    /// The assembled *base* system prompt for this session (arch-06). Stable across the session.
    pub fn system_prompt(&self) -> &str {
        &self.services.system_prompt
    }

    /// The agent's *current* system prompt — equal to the base unless a `before_agent_start` handler
    /// replaced it for the in-flight run (Pi `agent.state.systemPrompt`, agent-session.ts:1127).
    pub async fn current_system_prompt(&self) -> String {
        self.agent.snapshot().await.system_prompt
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

    // -------------------------------------------------------------------- state views ----

    /// Aggregate transcript stats for the current branch (Pi `getSessionStats`,
    /// agent-session.ts:2932; RPC `get_session_stats`).
    pub async fn session_stats(&self) -> crate::state::SessionStats {
        crate::state::SessionStats::from_messages(&self.messages().await)
    }

    /// Context-window occupancy from the last assistant turn (Pi `getContextUsage`,
    /// agent-session.ts:2977).
    pub async fn context_usage(&self) -> crate::state::ContextUsage {
        let messages = self.messages().await;
        let last = messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        });
        let window = { Self::lock(&self.compaction_model).context_window };
        crate::state::ContextUsage::from_last_assistant(last, window)
    }

    /// A serializable snapshot of the session for RPC `get_state` (Pi `state` getter,
    /// agent-session.ts:753).
    pub async fn state_view(&self) -> crate::state::SessionStateView {
        let messages = self.messages().await;
        let stats = crate::state::SessionStats::from_messages(&messages);
        let last = messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        });
        let window = { Self::lock(&self.compaction_model).context_window };
        let context_usage = crate::state::ContextUsage::from_last_assistant(last, window);
        let model = Self::lock(&self.model).clone();
        crate::state::SessionStateView {
            session_id: self.session_id.to_string(),
            cwd: self.services.cwd.display().to_string(),
            provider: model.provider.to_string(),
            model: model.model.to_string(),
            session_name: self.session_name().await,
            is_streaming: self.is_streaming().await,
            message_count: messages.len(),
            pending_message_count: self.pending_message_count(),
            stats,
            context_usage,
        }
    }

    async fn fanout_emit(&self, ev: AgentSessionEvent) {
        // Reuse the same fan-out the agent subscriber feeds; session-level events interleave with
        // agent events on the live streams.
        self.fanout.emit_external(ev).await;
    }

    /// Persist a custom (non-LLM) message via the session tree (Pi `sendCustomMessage` durable path,
    /// agent-session.ts:1313). The agent transcript carries it as a `Custom` role for the next run.
    pub async fn append_custom_message(
        &self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
    ) -> Result<EntryId, SessionServiceError> {
        let id = self
            .manager
            .lock()
            .await
            .append_custom_message(custom_type, content, display, None)?;
        Ok(id)
    }

    /// Send a user message that always triggers a turn (Pi `sendUserMessage`, agent-session.ts:1351).
    /// While the agent is streaming, the message is queued per `deliver_as` (steer / follow-up)
    /// instead of starting a new run.
    pub async fn send_user_message(
        &self,
        input: impl Into<UserInput>,
        deliver_as: Option<StreamingBehavior>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        let ui = input.into();
        if self.is_streaming().await {
            return match deliver_as {
                Some(StreamingBehavior::FollowUp) => self.follow_up(ui).await,
                _ => self.steer(ui).await,
            };
        }
        self.prompt_accepted(ui).await
    }

    /// Send a custom (non-LLM) message with delivery timing (Pi `sendCustomMessage`,
    /// agent-session.ts:1307-1338). `nextTurn` stages the message to ride the next prompt; `steer`/
    /// `followUp` queue onto the active run while streaming; otherwise the message is persisted and
    /// surfaced via `message_start`/`message_end`.
    pub async fn send_custom_message(
        &self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
        deliver_as: Option<crate::event::DeliverAs>,
    ) -> Result<(), SessionServiceError> {
        use crate::event::DeliverAs;
        let ts = now_ms();
        let msg = AgentMessage::Custom {
            kind: custom_type.to_string(),
            payload: content.clone(),
            timestamp: Some(ts),
        };
        match deliver_as {
            Some(DeliverAs::NextTurn) => {
                Self::lock(&self.pending_next_turn).push(msg);
            }
            _ if self.is_streaming().await => match deliver_as {
                Some(DeliverAs::FollowUp) => self.agent.follow_up(msg),
                _ => self.agent.steer(msg),
            },
            _ => {
                self.manager
                    .lock()
                    .await
                    .append_custom_message(custom_type, content, display, details)?;
                self.fanout_emit(AgentSessionEvent::MessageStart { message: msg.clone() }).await;
                self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg }).await;
            }
        }
        Ok(())
    }

    // --------------------------------------------------------------- model cycling ----

    /// The models available for `cycle_model` (Pi `scopedModels` getter, agent-session.ts:870).
    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        Self::lock(&self.scoped_models).clone()
    }

    /// Replace the scoped-model cycle set (Pi `setScopedModels`, agent-session.ts:875).
    pub fn set_scoped_models(&self, models: Vec<ScopedModel>) {
        *Self::lock(&self.scoped_models) = models;
    }

    /// Cycle to the next/previous model (Pi `cycleModel`, agent-session.ts:1471-1539). Cycles over
    /// the scoped set when one is configured (filtered to models with configured auth), else the full
    /// provider catalog. Returns a typed [`ModelCycleResult`] distinguishing the scoped vs available
    /// path + the restored thinking level, or `None` when there is one-or-fewer candidate. Applies
    /// the model + re-clamps/restores the thinking level, persists a `model_change`, and emits
    /// `model_changed` + the `model_select` ext event.
    pub async fn cycle_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let scoped = Self::lock(&self.scoped_models).clone();
        if scoped.is_empty() {
            self.cycle_available_model(forward).await
        } else {
            self.cycle_scoped_model(forward, &scoped).await
        }
    }

    /// Cycle over the scoped set, honoring per-model thinking levels (Pi `_cycleScopedModel`,
    /// agent-session.ts:1479-1510).
    async fn cycle_scoped_model(
        &self,
        forward: bool,
        scoped: &[ScopedModel],
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates: Vec<&ScopedModel> =
            scoped.iter().filter(|s| self.has_configured_auth(&s.model)).collect();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            .position(|s| s.model.provider == current.provider && s.model.id == current.model)
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).copied() else {
            return Ok(None);
        };
        // Explicit scoped thinking level overrides; `None` inherits the current session level.
        let explicit = next.thinking_level;
        let new_level = self
            .apply_model_change(&next.model, &current, "cycle", explicit)
            .await?;
        Ok(Some(ModelCycleResult { model: next.model.clone(), thinking_level: new_level, is_scoped: true }))
    }

    /// Cycle over the full provider catalog (Pi `_cycleAvailableModel`, agent-session.ts:1512-1538).
    async fn cycle_available_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates = self.provider.models().to_vec();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            .position(|m| m.provider == current.provider && m.id == current.model)
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).cloned() else {
            return Ok(None);
        };
        let new_level = self.apply_model_change(&next, &current, "cycle", None).await?;
        Ok(Some(ModelCycleResult { model: next, thinking_level: new_level, is_scoped: false }))
    }

    /// Apply a resolved model change: push to the agent, re-derive headers, persist, re-clamp/restore
    /// the thinking level, emit `model_changed` + `model_select`. Returns the new thinking level.
    /// Shared by [`Self::set_model_resolved`] and the cycle paths.
    async fn apply_model_change(
        &self,
        next: &Model,
        previous: &ModelRef,
        source: &str,
        explicit_thinking: Option<ModelThinkingLevel>,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model_ref = ModelRef {
            provider: next.provider.clone(),
            api: Some(next.api.clone()),
            model: next.id.clone(),
        };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = model_ref.clone();
        *Self::lock(&self.compaction_model) = next.clone();
        self.services.host_services.update_model(model_ref, next.context_window, None);
        self.manager.lock().await.append_model_change(next.provider.clone(), next.id.clone())?;
        // Re-clamp the thinking level for the new model (explicit override or current session level).
        let level = match explicit_thinking {
            Some(l) => l,
            None => self.thinking_level().await,
        };
        let new_level = self.set_thinking_level(level).await?;
        self.fanout_emit(AgentSessionEvent::ModelChanged {
            provider: next.provider.to_string(),
            model: next.id.to_string(),
        })
        .await;
        self.emit_model_select(next, previous, source).await;
        Ok(new_level)
    }

    // ------------------------------------------------------------- facade accessors ----

    /// The file-based prompt templates discovered for this session (Pi `promptTemplates` getter,
    /// agent-session.ts:880).
    pub fn prompt_templates(
        &self,
    ) -> &cyrup_resources::ResourceSet<cyrup_resources::PromptTemplate> {
        &self.services.resources.prompts
    }

    /// The live provider model catalog (Pi `modelRegistry` getter, agent-session.ts:1412).
    pub fn model_catalog(&self) -> &[cyrup_provider::Model] {
        self.provider.models()
    }

    /// The session-scoped resource registry (Pi `resourceLoader` getter, agent-session.ts:363).
    pub fn resources(&self) -> &Arc<cyrup_resources::ResourceRegistry> {
        &self.services.resources
    }

    /// Read-only handle to the extension host (Pi `extensionRunner` getter, agent-session.ts:3142).
    pub fn ext_host(&self) -> &Arc<cyrup_ext::ExtensionHost> {
        &self.services.ext_host
    }

    /// Whether any loaded extension handles `kind` (Pi `hasExtensionHandlers`, agent-session.ts:3135).
    pub fn has_extension_handlers(&self, kind: cyrup_ext::EventKind) -> bool {
        !self.services.ext_host.dispatcher().no_subscribers(kind)
    }
}

// ============================================================================ retry subsystem ====
// Pi `agent-session.ts:778,561,2484-2572`. The agent layer drives provider-level retry
// (`max_retries`/`max_retry_delay_ms`); this is the SESSION-level retry-after-agent-end policy:
// when the final assistant turn carries a transient (retryable) error, the facade waits an
// exponential backoff and continues the agent, up to `retry.maxRetries`.
impl AgentSession {
    /// Current retry attempt (0 when not retrying; Pi `retryAttempt` getter, agent-session.ts:778).
    pub fn retry_attempt(&self) -> u32 {
        *Self::lock(&self.retry_attempt)
    }

    /// Whether a retry backoff is in flight (Pi `isRetrying` getter, agent-session.ts:2553).
    pub fn is_retrying(&self) -> bool {
        Self::lock(&self.retry_cancel).is_some()
    }

    /// Whether auto-retry is enabled (runtime override, else the settings default; Pi
    /// `autoRetryEnabled`, agent-session.ts:2558).
    pub fn auto_retry_enabled(&self) -> bool {
        Self::lock(&self.auto_retry_override).unwrap_or(self.retry_enabled_default)
    }

    /// Toggle auto-retry (Pi `setAutoRetryEnabled`, agent-session.ts:2565). Facade-side override of
    /// the settings `retry.enabled` value (settings persistence lives one layer down).
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_retry_override) = Some(enabled);
    }

    /// Cancel an in-flight retry backoff (Pi `abortRetry`, agent-session.ts:2548).
    pub fn abort_retry(&self) {
        if let Some(c) = Self::lock(&self.retry_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Whether an assistant error is retryable (Pi `_isRetryableError`, agent-session.ts:2484).
    /// Context-overflow is handled by compaction, never retry.
    pub fn is_retryable_error(&self, message: &AssistantMessage) -> bool {
        let window = { Some(Self::lock(&self.compaction_model).context_window) };
        if is_context_overflow(message, window) {
            return false;
        }
        is_retryable_assistant_error(message)
    }

    /// Whether the run that just ended will retry (Pi `_willRetryAfterAgentEnd`, agent-session.ts:561).
    /// True iff auto-retry is enabled, the budget is not exhausted, and the last assistant message is
    /// a retryable error.
    pub fn will_retry_after_agent_end(&self, messages: &[AgentMessage]) -> bool {
        if !self.auto_retry_enabled() || self.retry_attempt() >= self.retry_max_retries {
            return false;
        }
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(self.is_retryable_error(a)),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Prepare a retryable error for continuation with exponential backoff (Pi `_prepareRetry`,
    /// agent-session.ts:2495-2543). Returns `true` when the caller should continue the agent after
    /// the (abortable) backoff, `false` when retry is disabled, the budget is exhausted, or the wait
    /// was cancelled. Drops the trailing error message from the agent transcript before continuing.
    pub async fn prepare_retry(&self, message: &AssistantMessage) -> bool {
        if !self.auto_retry_enabled() {
            return false;
        }
        {
            let mut attempt = Self::lock(&self.retry_attempt);
            *attempt += 1;
            if *attempt > self.retry_max_retries {
                *attempt -= 1;
                return false;
            }
        }
        let attempt = self.retry_attempt();
        let delay_ms = self
            .retry_base_delay_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
        self.fanout_emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: self.retry_max_retries,
            delay_ms,
            error_message: message.error_message.clone().unwrap_or_else(|| "Unknown error".into()),
        })
        .await;
        // Drop the trailing error message from the agent transcript (kept in session for history).
        self.drop_trailing_assistant().await;
        // Abortable exponential backoff.
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.retry_cancel) = Some(cancel.clone());
        let slept = cancel
            .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(delay_ms)))
            .await
            .is_some();
        *Self::lock(&self.retry_cancel) = None;
        if !slept {
            let attempt = std::mem::replace(&mut *Self::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".into()),
            })
            .await;
            return false;
        }
        true
    }

    /// Drop the trailing assistant message from the agent transcript (used by retry/overflow paths).
    async fn drop_trailing_assistant(&self) {
        let mut msgs = self.agent.snapshot().await.messages;
        if matches!(msgs.last(), Some(AgentMessage::Assistant(_))) {
            msgs.pop();
            self.agent.set_messages(msgs).await;
        }
    }
}

// ====================================================================== auto-compaction subsystem ====
// Pi `agent-session.ts:831,1811-1905,2078-2086`. The pre-send + post-run compaction trigger that
// keeps a long session inside its context window. Manual `compact` already exists; this adds the
// threshold/overflow auto-trigger + the enable toggle + `is_compacting`.
impl AgentSession {
    /// Whether any compaction (manual / auto / branch-summary) is running (Pi `isCompacting`,
    /// agent-session.ts:831).
    pub fn is_compacting(&self) -> bool {
        Self::lock(&self.compaction_cancel).is_some()
            || Self::lock(&self.auto_compaction_cancel).is_some()
            || Self::lock(&self.branch_summary_cancel).is_some()
    }

    /// Whether auto-compaction is enabled (runtime override, else the settings default; Pi
    /// `autoCompactionEnabled`, agent-session.ts:2086).
    pub fn auto_compaction_enabled(&self) -> bool {
        Self::lock(&self.auto_compaction_override).unwrap_or(self.auto_compaction_enabled_default)
    }

    /// Toggle auto-compaction (Pi `setAutoCompactionEnabled`, agent-session.ts:2078).
    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_compaction_override) = Some(enabled);
    }

    /// Check whether the given assistant turn requires compaction and run it (Pi `_checkCompaction`,
    /// agent-session.ts:1808-1898). Returns `true` when a compaction ran. `skip_aborted` skips a
    /// user-cancelled turn (post-run); the pre-send check passes `false` to catch aborted responses.
    pub async fn check_compaction(
        &self,
        assistant: &AssistantMessage,
        skip_aborted: bool,
    ) -> Result<bool, SessionServiceError> {
        if !self.auto_compaction_enabled() {
            return Ok(false);
        }
        if skip_aborted && assistant.stop_reason == cyrup_core::StopReason::Aborted {
            return Ok(false);
        }
        let model = { Self::lock(&self.compaction_model).clone() };
        let window = model.context_window;
        let same_model = {
            let cur = Self::lock(&self.model);
            assistant.provider == cur.provider && assistant.model.as_str() == cur.model.as_str()
        };

        // Case 1: overflow — a context-overflow error/usage on the SAME model compacts (no retry
        // for a completed answer; the overflow-recovery flag guards an infinite loop).
        if same_model && is_context_overflow(assistant, Some(window)) {
            let will_retry = assistant.stop_reason != cyrup_core::StopReason::Stop;
            if !will_retry {
                return self.run_auto_compaction(CompactionReason::Overflow).await;
            }
            if *Self::lock(&self.overflow_recovery_attempted) {
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    aborted: false,
                })
                .await;
                return Ok(false);
            }
            *Self::lock(&self.overflow_recovery_attempted) = true;
            self.drop_trailing_assistant().await;
            return self.run_auto_compaction(CompactionReason::Overflow).await;
        }

        // Case 2: threshold — the built context exceeds `window − reserve`.
        let guard = self.manager.lock().await;
        let path: Vec<cyrup_session::Entry> = guard.branch_path(None).into_iter().cloned().collect();
        drop(guard);
        let settings = self.effective_compaction_settings();
        let summarizer = DynSummarizer::new(self.provider.clone(), model.clone());
        let compactor = Compactor::new(summarizer, NoHooks);
        let window32 = u32::try_from(window).unwrap_or(u32::MAX);
        if compactor.should_compact(&path, window32, &settings) {
            return self.run_auto_compaction(CompactionReason::Threshold).await;
        }
        Ok(false)
    }

    /// Run an auto-compaction with its own abort controller + events (Pi `_runAutoCompaction`,
    /// agent-session.ts:1905-2076). Mirrors [`Self::compact`]'s dance but tagged with the auto
    /// `reason` and tracked under `auto_compaction_cancel` so `is_compacting`/`abort_compaction`
    /// observe it.
    async fn run_auto_compaction(
        &self,
        reason: CompactionReason,
    ) -> Result<bool, SessionServiceError> {
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.auto_compaction_cancel) = Some(cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        if !self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionBeforeCompact)
        {
            let reduced = self
                .services
                .ext_host
                .dispatcher()
                .dispatch_block_mutate(HostEvent::SessionBeforeCompact, &cancel)
                .await;
            if matches!(reduced, Reduced::Blocked { .. }) {
                *Self::lock(&self.auto_compaction_cancel) = None;
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: true }).await;
                return Ok(false);
            }
        }

        let model = { Self::lock(&self.compaction_model).clone() };
        let summarizer = DynSummarizer::new(self.provider.clone(), model.clone());
        let compactor = Compactor::new(summarizer, NoHooks);
        let settings = self.effective_compaction_settings();
        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction(&mut guard, &model, &settings, reason, None, false, cancel)
            .await;
        drop(guard);
        *Self::lock(&self.auto_compaction_cancel) = None;

        match result {
            Ok(Some(entry)) => {
                let notify_cancel = self.session_cancel.child_token();
                self.services
                    .ext_host
                    .dispatcher()
                    .dispatch_notify(
                        &HostEvent::SessionCompact { summary: entry.summary.clone() },
                        &notify_cancel,
                    )
                    .await;
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: false }).await;
                Ok(true)
            }
            Ok(None) => {
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted: false }).await;
                Ok(false)
            }
            Err(e) => {
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                self.fanout_emit(AgentSessionEvent::CompactionEnd { reason, aborted }).await;
                if aborted {
                    Ok(false)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// The effective compaction settings with the live `enabled` toggle applied.
    fn effective_compaction_settings(&self) -> CompactionSettings {
        CompactionSettings {
            enabled: self.auto_compaction_enabled(),
            reserve_tokens: self.compaction_settings.reserve_tokens,
            keep_recent_tokens: self.compaction_settings.keep_recent_tokens,
        }
    }
}

// =========================================================================== immediate-bash seam ====
// Pi `agent-session.ts:2582-2684`. The out-of-loop bash RPC path.
impl AgentSession {
    /// Execute a bash command out-of-band and record its result (Pi `executeBash`,
    /// agent-session.ts:2588). Streams combined output to `on_chunk`; the result is recorded into the
    /// transcript (or deferred while a run streams).
    pub async fn execute_bash(
        &self,
        command: &str,
        options: BashOptions,
        on_chunk: crate::bash::BashChunkSink,
    ) -> BashResult {
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.bash_cancel) = Some(cancel.clone());
        let cwd = self.services.cwd.clone();
        let result = run_bash(&self.proc, &self.shell, cwd, command.to_string(), cancel, on_chunk).await;
        *Self::lock(&self.bash_cancel) = None;
        self.record_bash_result(command, &result, options).await;
        result
    }

    /// Record a bash result into the transcript + session (Pi `recordBashResult`,
    /// agent-session.ts:2628). While a run streams, the message is deferred to avoid breaking
    /// tool_use/tool_result ordering and flushed after the turn.
    pub async fn record_bash_result(&self, command: &str, result: &BashResult, options: BashOptions) {
        let payload = bash_message_payload(command, result, options.exclude_from_context);
        let msg = AgentMessage::Custom {
            kind: "bashExecution".to_string(),
            payload: payload.clone(),
            timestamp: Some(now_ms()),
        };
        if self.is_streaming().await {
            Self::lock(&self.pending_bash).push(msg);
            return;
        }
        self.append_bash_message(msg, &payload).await;
    }

    /// Cancel a running bash command (Pi `abortBash`, agent-session.ts:2660).
    pub fn abort_bash(&self) {
        if let Some(c) = Self::lock(&self.bash_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Whether a bash command is running (Pi `isBashRunning`, agent-session.ts:2665).
    pub fn is_bash_running(&self) -> bool {
        Self::lock(&self.bash_cancel).is_some()
    }

    /// Whether deferred bash messages await flush (Pi `hasPendingBashMessages`, agent-session.ts:2670).
    pub fn has_pending_bash_messages(&self) -> bool {
        !Self::lock(&self.pending_bash).is_empty()
    }

    /// Flush deferred bash messages to the transcript + session (Pi `_flushPendingBashMessages`,
    /// agent-session.ts:2675). Called before a new prompt so ordering is intact.
    pub async fn flush_pending_bash_messages(&self) {
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_bash));
        for msg in pending {
            if let AgentMessage::Custom { payload, .. } = &msg {
                let payload = payload.clone();
                self.append_bash_message(msg, &payload).await;
            }
        }
    }

    /// Append a bash message to the agent transcript + persist it durably.
    async fn append_bash_message(&self, msg: AgentMessage, payload: &serde_json::Value) {
        let mut msgs = self.agent.snapshot().await.messages;
        msgs.push(msg);
        self.agent.set_messages(msgs).await;
        let _ = self
            .manager
            .lock()
            .await
            .append_custom_message("bashExecution", payload.clone(), true, None);
    }
}

// =============================================================================== dynamic tools ====
// Pi `agent-session.ts:786-828,2304`. Mid-session tool toggling + system-prompt rebuild.
impl AgentSession {
    /// Names of the currently-active tools (Pi `getActiveToolNames`, agent-session.ts:786).
    pub fn active_tool_names(&self) -> Vec<String> {
        Self::lock(&self.dynamic_tools).active_names()
    }

    /// All enable-able tools with metadata (Pi `getAllTools`, agent-session.ts:794).
    pub fn all_tools(&self) -> Vec<ToolInfo> {
        Self::lock(&self.dynamic_tools).all()
    }

    /// One tool's definition by name (Pi `getToolDefinition`, agent-session.ts:806).
    pub fn tool_definition(&self, name: &str) -> Option<ToolInfo> {
        Self::lock(&self.dynamic_tools).get(name)
    }

    /// Set the active tool set by name, rebuilding the base system prompt and re-pushing both the
    /// tool array and the prompt to the agent for the next turn (Pi `setActiveToolsByName`,
    /// agent-session.ts:812). Unknown names are ignored.
    pub async fn set_active_tools_by_name(&self, names: &[String]) {
        let (tools, prompt) = { Self::lock(&self.dynamic_tools).set_active(names) };
        self.agent.set_tools(tools).await;
        self.agent.set_system_prompt(prompt).await;
    }

    /// Register additional custom tools into the enable-able registry (Pi `customTools`, sdk.ts:71,384).
    pub fn register_custom_tools(&self, tools: Vec<Arc<dyn cyrup_core::Tool>>) {
        Self::lock(&self.dynamic_tools).register_custom(tools);
    }
}

/// Strip a leading `---\n…\n---` YAML frontmatter block (Pi `stripFrontmatter`); returns the body
/// after it, or the original text when no frontmatter is present.
fn strip_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")) else {
        return content;
    };
    // Find the closing `---` line.
    if let Some(idx) = rest.find("\n---") {
        let after = &rest[idx + 4..];
        after.strip_prefix('\n').or_else(|| after.strip_prefix("\r\n")).unwrap_or(after)
    } else {
        content
    }
}

/// Current wall-clock time in milliseconds (Pi `Date.now()`); 0 on a clock fault.
fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

/// The concatenated text of a core `user` message entry, or `None` for any other entry/role.
fn user_message_text(e: &cyrup_session::Entry) -> Option<String> {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};
    let Entry::Known(KnownEntry::Message { message, .. }) = e else { return None };
    let SessMsg::Core(Message::User { content, .. }) = message else { return None };
    let text: String = content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

/// For a `position:"before"` fork: require a user-message anchor and return `(parent_id, text)`.
fn user_message_anchor(e: &cyrup_session::Entry) -> Option<(Option<EntryId>, String)> {
    user_message_text(e).map(|text| (e.parent_id(), text))
}

/// Resolve the branch leaf + optional selected-text for an entry-anchored fork (Pi
/// agent-session-runtime.ts:268-284). Shared by [`AgentSession::fork_at_entry`] and the runtime's
/// throwaway-manager fork path so the anchor semantics stay identical.
pub(crate) fn fork_anchor(
    mgr: &SessionManager,
    entry: &EntryId,
    position: ForkPosition,
) -> Result<(Option<EntryId>, Option<String>), SessionServiceError> {
    let selected = mgr
        .entry(entry)
        .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
    match position {
        ForkPosition::At => Ok((Some(selected.id()), None)),
        ForkPosition::Before => {
            let (parent, text) = user_message_anchor(selected)
                .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
            Ok((parent, Some(text)))
        }
    }
}
