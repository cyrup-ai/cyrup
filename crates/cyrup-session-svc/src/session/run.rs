//! Prompting and the run driver — `prompt`/`steer`/`follow_up` in, an assembled agent run out.
//!
//! Pi `agent-session.ts` `prompt`/`_runAgentPrompt`/`_handlePostAgentRun`. Covers preflight
//! (`prepare`, the `input` extension event, queue routing), input expansion, run-message assembly
//! and the spawned post-run driver loop that owns retry / auto-compaction / queued continuations.

use std::sync::Arc;

use cyrup_agent::AgentMessage;
use cyrup_core::{AssistantMessage, Content, EventStream, Message};
use cyrup_ext::{HostEvent, InputEventSource, InputStreamingBehavior, Reduced};

use crate::error::SessionServiceError;
use crate::event::{
    AgentSessionEvent, InputSource, PromptAccepted, PromptOptions, StreamingBehavior, UserInput,
    core_message_to_agent,
};

use super::AgentSession;

/// The disposition of the `input` extension event (Pi `InputEventResult.action`, runner.ts:1100).
/// A `transform` outcome rewrites the in-flight [`UserInput`] in place (via `EventPatch::Input`) and
/// then reports `Continue`, exactly as Pi folds `currentText`/`currentImages` before continuing.
enum InputDisposition {
    /// A handler fully serviced the submission (`handled`); no run or queue follows.
    Handled,
    /// No handler claimed it; proceed with expansion + run/queue (text/images may have been
    /// rewritten by a `transform` handler already applied to the [`UserInput`]).
    Continue,
}

/// Collapse the host-side [`InputSource`] onto Pi's three handler-visible `InputSource` values
/// (`"interactive" | "rpc" | "extension"`, extensions/types.ts:789). cyrup's richer provenance
/// (`Cli`/`Stdin`/`Sdk`/`Tui`) all present as `interactive` to a handler, exactly as Pi's host
/// passes `"interactive"` for any non-rpc submission (agent-session.ts:1021).
fn input_event_source(source: InputSource) -> InputEventSource {
    match source {
        InputSource::Rpc => InputEventSource::Rpc,
        InputSource::Cli | InputSource::Stdin | InputSource::Sdk | InputSource::Tui => {
            InputEventSource::Interactive
        }
    }
}

/// Map the queue selector onto the handler-visible `streamingBehavior` (Pi `"steer" | "followUp"`).
fn input_streaming_behavior(behavior: StreamingBehavior) -> InputStreamingBehavior {
    match behavior {
        StreamingBehavior::Steer => InputStreamingBehavior::Steer,
        StreamingBehavior::FollowUp => InputStreamingBehavior::FollowUp,
    }
}

/// What [`AgentSession::prepare`] resolved a submission to (the shared `prompt` preflight outcome).
enum Prepared {
    /// Assembled run input to dispatch to the agent.
    Run(Vec<AgentMessage>),
    /// An `input` handler serviced it; nothing to run.
    Handled,
    /// The agent is streaming; the (expanded) submission is queued via the carried behavior.
    Queued(StreamingBehavior, UserInput),
}

impl AgentSession {
    /// Submit a user prompt and observe the run as a stream of [`AgentSessionEvent`] (R-11-005/007).
    ///
    /// The returned stream terminates after the run's `agent_end`. Errors only if the prompt could
    /// not be *accepted* (e.g. the agent is already streaming — use [`Self::steer`]/[`Self::follow_up`]).
    pub async fn prompt(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<EventStream<AgentSessionEvent>, SessionServiceError> {
        // AGENT-030: the session-level run latch, not the agent's per-run streaming flag — pi's
        // `prompt()` consults `this.isStreaming`, which IS `_isAgentRunActive`
        // (agent-session.ts:876-877 / :1159 @v0.83.0). See [`Self::is_run_active`].
        if self.is_run_active() {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        // Register the run-scoped subscription BEFORE starting the run so no event is missed.
        let stream = self.fanout.subscribe_run();
        match self.prepare(input.into(), PromptOptions::default()).await? {
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok(stream)
            }
            // An `input` handler serviced the submission (no run started); the stream stays idle.
            Prepared::Handled | Prepared::Queued(..) => Ok(stream),
        }
    }

    /// Submit a prompt, resolving only to the preflight acceptance (mirrors Pi). The run is observed
    /// via [`Self::subscribe`]. Used by adapters that manage their own persistent subscription.
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.prompt_with(input, PromptOptions::default()).await
    }

    /// Submit a prompt with per-call [`PromptOptions`] (Pi `prompt(text, options)`,
    /// agent-session.ts:998). Closes the in-`prompt` `streamingBehavior` seam (gap `#13`): while the
    /// agent is streaming, the (template-expanded) text is queued via steer/follow-up per
    /// `streaming_behavior` instead of being rejected, exactly as Pi does at agent-session.ts:1043-
    /// 1056. The `Result` itself is the `preflightResult` callback (`Ok` = accepted, `Err` = the
    /// preflight throw). An `input` extension handler may fully service the submission, yielding
    /// [`PromptAccepted::Handled`].
    pub async fn prompt_with(
        &self,
        input: impl Into<UserInput>,
        options: PromptOptions,
    ) -> Result<PromptAccepted, SessionServiceError> {
        match self.prepare(input.into(), options).await? {
            Prepared::Handled => Ok(PromptAccepted::Handled),
            Prepared::Queued(behavior, ui) => match behavior {
                StreamingBehavior::FollowUp => self.follow_up(ui).await,
                StreamingBehavior::Steer => self.steer(ui).await,
            },
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok(PromptAccepted::Started)
            }
        }
    }

    /// Dispatch an assembled run. A BOUND session (via [`Self::into_shared`]) spawns the post-run
    /// driver task so auto-retry / post-run auto-compaction / queued continuations actually fire from
    /// the completed turn (Pi `_runAgentPrompt`, agent-session.ts:973-985). An unbound by-value session
    /// keeps the legacy behavior: start the run and let the subscriber terminate the run-scoped streams
    /// on `agent_end` (the post-run loop does not run).
    pub(super) async fn spawn_run(&self, messages: Vec<AgentMessage>) -> Result<(), SessionServiceError> {
        match self.handle.get() {
            Some(this) => {
                // Flag the loop active BEFORE returning so an immediate `wait_for_idle` waits for the
                // WHOLE loop, not just the first `agent_end`.
                let _ = self.driver_tx.send(true);
                tokio::spawn(async move { this.drive_run(messages).await });
                Ok(())
            }
            None => {
                self.agent.prompt(messages).await?;
                Ok(())
            }
        }
    }

    /// The post-run execution loop (Pi `_runAgentPrompt` + `_handlePostAgentRun`,
    /// agent-session.ts:973-1022). Runs the prompt, then — for as long as the post-run handler asks —
    /// drives `agent.continue()` for an auto-retry, a threshold/overflow auto-compaction, or an
    /// `agent_end`-queued continuation. Spawned by [`Self::spawn_run`] on a bound session.
    async fn drive_run(self: Arc<Self>, messages: Vec<AgentMessage>) {
        if let Ok(handle) = self.agent.prompt(messages).await {
            let _ = handle.finished().await;
            // GAP-11: apply the event-tier control ops (set_model / set_thinking_level) a guest queued
            // from `on_message_end` / a mid-turn tool hook / `on_agent_end`. This runs at a STORE-FREE
            // point — the whole run's ordered subscriber dispatch has returned, so every
            // `LiveExtension.inner` store guard is released and the drain's `thinking_level_select` /
            // `model_select` re-emit is a fresh top-level guest call, never a re-entry into the
            // suspended event-hook store (see live.rs `set_thinking_level`). This is the "before the
            // next turn" point the control queue promises, so the SUBSEQUENT `continue_run` (and the
            // next user turn) reads the new `agent.model` / `thinking_level`. Uses the `Send`-safe
            // focused drain (not the full `apply_pending_control`) because this future is spawned:
            // only SetModel/SetThinkingLevel can reach the queue from an event handler.
            self.apply_pending_agent_control().await;
            while self.handle_post_agent_run().await {
                match self.agent.continue_run().await {
                    Ok(h) => {
                        let _ = h.finished().await;
                        // Same store-free turn-boundary drain after each continuation settles.
                        self.apply_pending_agent_control().await;
                    }
                    Err(_) => break,
                }
            }
        }
        // Pi `_runAgentPrompt`'s `finally` opens with `this._systemPromptOverride = undefined;`
        // (agent-session.ts:1069 @v0.83.0), BEFORE the bash flush and the settle emit — a
        // `before_agent_start` replacement is scoped to its own run and must not survive into the
        // next one (DRIFT-033).
        *crate::sync::lock(&self.system_prompt_override) = None;
        // Pi `finally` (agent-session.ts:982-984): flush deferred bash messages from this turn.
        self.flush_pending_bash_messages().await;
        // SEAM-005: the run has FULLY settled — the post-run loop above is done, so no retry,
        // compaction or queued continuation will follow. This is exactly Pi's `_emitAgentSettled()`
        // call site: the `finally` of `_runAgentPrompt` (agent-session.ts:1063-1072), AFTER
        // `_flushPendingBashMessages()` and BEFORE the idle wait resolves.
        self.emit_agent_settled().await;
        // Terminate the run-scoped subscriptions returned by `prompt` now the whole loop has
        // settled. Ordered AFTER the settle emit so a run-scoped subscriber (what `prompt` hands
        // back) actually observes `agent_settled` as its last event.
        self.fanout.end_run();
        // Pi's `_resolveIdleWaitIfIdle()` runs in `_emitAgentSettled`'s own `finally` — i.e. the
        // idle wait releases only after the event has been delivered. `driver_tx` is cyrup's idle
        // latch, so it drops last.
        let _ = self.driver_tx.send(false);
    }

    /// Emit `agent_settled` (Pi `_emitAgentSettled`, agent-session.ts:581-588) — to the EXTENSION
    /// RUNNER first, then to the session subscribers, matching Pi's order exactly
    /// (`await this._extensionRunner.emit(...)` then `this._emit(...)`).
    ///
    /// Fires once per RUN, not once per agent loop: a turn that auto-retries produces two
    /// `agent_end`s and exactly one `agent_settled`. That is the whole reason the event exists —
    /// `agent_end` cannot tell a consumer whether more work is coming, which is why Pi's RPC host
    /// checks its shutdown request here and nowhere else (rpc-mode.ts:355-358).
    pub(crate) async fn emit_agent_settled(&self) {
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::AgentSettled, &cancel)
            .await;
        self.fanout_emit(AgentSessionEvent::AgentSettled).await;
    }

    /// Decide whether the just-finished run needs a continuation (Pi `_handlePostAgentRun`,
    /// agent-session.ts:986-1013): retry a transient error after backoff, close a spent retry
    /// sequence, run a post-run threshold/overflow compaction, or continue for `agent_end`-queued
    /// messages. Returns `true` when the driver should `agent.continue()`.
    async fn handle_post_agent_run(&self) -> bool {
        let Some(msg) = crate::sync::lock(&self.last_assistant).take() else { return false };
        // Retryable transient error → backoff + continue (Pi :991-993).
        if self.is_retryable_error(&msg) && self.prepare_retry(&msg).await {
            return true;
        }
        // A terminal error with a spent / non-retryable budget closes the retry sequence (Pi :995-1003).
        if msg.stop_reason == cyrup_core::StopReason::Error && self.retry_attempt() > 0 {
            let attempt = std::mem::replace(&mut *crate::sync::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: msg.error_message.clone(),
            })
            .await;
        }
        // Threshold / overflow post-run compaction → continue (Pi :1005-1007).
        if self.check_compaction(&msg, true).await.unwrap_or(false) {
            return true;
        }
        // Messages queued by `agent_end` extension handlers need a continuation (Pi :1009-1012).
        self.agent.has_queued_messages()
    }

    /// The persist+fan-out subscriber's `message_start` handler for a USER message (Pi
    /// `_handleAgentEvent` head, agent-session.ts:514-535): reset the overflow-recovery latch and, when
    /// the message text matches a queued steer/follow-up mirror entry, drop it and emit `queue_update`
    /// as the agent drains the queue.
    pub(crate) async fn on_user_message_start(&self, message: &AgentMessage) {
        *crate::sync::lock(&self.overflow_recovery_attempted) = false;
        let Some(text) = agent_user_text(message) else { return };
        let mut drained = false;
        {
            let mut steer = crate::sync::lock(&self.steering_messages);
            if let Some(pos) = steer.iter().position(|m| *m == text) {
                steer.remove(pos);
                drained = true;
            }
        }
        if !drained {
            let mut fu = crate::sync::lock(&self.follow_up_messages);
            if let Some(pos) = fu.iter().position(|m| *m == text) {
                fu.remove(pos);
                drained = true;
            }
        }
        if drained {
            self.emit_queue_update().await;
        }
    }

    /// The subscriber's `message_end` handler for an ASSISTANT message (Pi `_handleAgentEvent` tail,
    /// agent-session.ts:562-577): track the last assistant message (drives the post-run loop) and — on
    /// a non-error response — clear the overflow latch and reset the retry counter, emitting
    /// `auto_retry_end{success:true}` if a retry sequence was in flight.
    pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
        *crate::sync::lock(&self.last_assistant) = Some(assistant.clone());
        if assistant.stop_reason == cyrup_core::StopReason::Error {
            return;
        }
        *crate::sync::lock(&self.overflow_recovery_attempted) = false;
        let attempt = {
            let mut at = crate::sync::lock(&self.retry_attempt);
            let v = *at;
            if v > 0 {
                *at = 0;
            }
            v
        };
        if attempt > 0 {
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt,
                final_error: None,
            })
            .await;
        }
    }

    /// The shared preflight Pi's `prompt` performs before either running or queueing
    /// (agent-session.ts:1003-1142): emit the `input` extension event (which may fully service the
    /// submission), then — if the agent is streaming — expand templates and route to the steer/
    /// follow-up queue per `streaming_behavior` (erroring when none is given), else assemble the run
    /// input. Returns the disposition the caller acts on.
    async fn prepare(
        &self,
        mut ui: UserInput,
        options: PromptOptions,
    ) -> Result<Prepared, SessionServiceError> {
        // AGENT-030: pi's whole preflight reads `this.isStreaming` == `_isAgentRunActive`
        // (agent-session.ts:1022 for the `input` event's `streamingBehavior`, `:1159` for the
        // queue routing) — the latch that spans `_handlePostAgentRun` and every `agent.continue()`,
        // not a per-run flag. See [`Self::is_run_active`].
        let streaming = self.is_run_active();
        // 0. Slash extension-command exec FIRST (Pi `_tryExecuteExtensionCommand`,
        //    agent-session.ts:1004-1013): for `expandPromptTemplates && text.startsWith("/")`, if a
        //    registered command name matches, run its handler and short-circuit (no prompt sent).
        //    Matches Pi's order: tried BEFORE the `input` event + before skill/template expansion.
        if ui.expand_templates
            && ui.text.starts_with('/')
            && self.try_execute_extension_command(&ui.text).await
        {
            return Ok(Prepared::Handled);
        }
        // 1. `input` extension event, emitted BEFORE expansion (Pi agent-session.ts:1015-1033). A
        //    handler that returns `handled` fully services the submission — no run, no queue; a
        //    `transform` handler rewrites `ui` (text/images) in place before continuing. The handler
        //    sees `streamingBehavior` only while streaming (Pi `this.isStreaming ? ... : undefined`,
        //    agent-session.ts:1022).
        let handler_behavior = if streaming { options.streaming_behavior } else { None };
        if matches!(
            self.emit_input_event(&mut ui, handler_behavior).await,
            InputDisposition::Handled
        ) {
            return Ok(Prepared::Handled);
        }
        // GAP-11: apply any event-tier control op (set_model / set_thinking_level) an `on_input`
        // handler just queued, at this STORE-FREE point — `emit_input_event` has returned, releasing
        // every `LiveExtension.inner` guard, so the drain's re-emit is a fresh top-level guest call
        // (never a re-entry). This makes an `on_input` `setModel`/`setThinkingLevel` take effect on
        // the turn now being assembled, matching Pi, whose synchronous `on_input` mutation lands
        // before the dispatched turn (agent-session.ts:1015-1033). The focused drain never re-enters
        // `prepare` (unlike the full `apply_pending_control`'s `SendUserMessage` arm), keeping this
        // hot path free of the boxed async-recursion edge.
        self.apply_pending_agent_control().await;
        // 2. While streaming, expand then queue per `streamingBehavior` (Pi agent-session.ts:1043-
        //    1056). Without a behavior the submission is rejected (Pi throws at :1044).
        if streaming {
            let behavior = options
                .streaming_behavior
                .ok_or(SessionServiceError::StreamingNeedsBehavior)?;
            let mut queued = ui;
            if queued.expand_templates {
                queued.text = self.expand_input_text(&queued.text);
            }
            return Ok(Prepared::Queued(behavior, queued));
        }
        // 3. Not streaming: run the full pre-send sequence + assemble the run input.
        Ok(Prepared::Run(self.prepare_and_assemble(ui).await?))
    }

    /// Emit the `input` extension event (Pi `emitInput`, runner.ts:1095). A handler may fully
    /// service the submission (`HookOutcome::Handled`/`Block` ⇒ [`InputDisposition::Handled`]) or
    /// *transform* it (`HookOutcome::Mutate(EventPatch::Input{..})`, Pi `action:"transform"`,
    /// runner.ts:1116-1119): the folded text/images flow back into `ui` and the submission continues
    /// with the rewritten content (Pi agent-session.ts:1029-1032).
    async fn emit_input_event(
        &self,
        ui: &mut UserInput,
        streaming_behavior: Option<StreamingBehavior>,
    ) -> InputDisposition {
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::Input) {
            return InputDisposition::Continue;
        }
        let cancel = self.session_cancel.child_token();
        // Deliver the `source` (Pi `InputEvent.source`, agent-session.ts:1021) + the in-flight
        // `streamingBehavior` (`undefined` when idle, :1022) so a handler can branch on
        // interactive-vs-queued / steer-vs-follow-up before deciding (#13c).
        let event = HostEvent::Input {
            text: ui.text.clone(),
            images: ui.images.clone(),
            source: input_event_source(ui.source),
            streaming_behavior: streaming_behavior.map(input_streaming_behavior),
        };
        let reduced = self
            .services
            .ext_host
            .dispatcher()
            .dispatch_block_mutate(event, &cancel)
            .await;
        match reduced {
            Reduced::Handled(_) | Reduced::Blocked { .. } => InputDisposition::Handled,
            // Apply any `transform` the handler chain folded into the event (Pi
            // agent-session.ts:1029-1032: `currentText`/`currentImages` adopt the result).
            Reduced::Pass(ev) => {
                if let HostEvent::Input { text, images, .. } = *ev {
                    ui.text = text;
                    ui.images = images;
                }
                InputDisposition::Continue
            }
        }
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
        // 3. Model + auth precheck. Pi validates the MODEL first —
        // `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
        // (agent-session.ts:1177-1180) — and only then the credential (`:1182-1195`). This is the
        // first turn of a modelless first run (SEAM-075): the answer is the `/login` → `/model`
        // instruction, surfaced as an error on the turn, never a process exit.
        {
            let model = crate::sync::lock(&self.compaction_model)
                .clone()
                .ok_or(SessionServiceError::NoModelSelected)?;
            // PROV-037 — pi's refusal is a THREE-branch decision, not one
            // (`agent-session.ts:1182-1195` @v0.83.0):
            //
            //   const hasConfiguredAuth =
            //       this._modelRuntime.hasConfiguredAuth(this.model.provider) ||
            //       (await this._modelRuntime.checkAuth(this.model.provider)) !== undefined;
            //   if (!hasConfiguredAuth) {
            //       if (this._modelRuntime.isUsingOAuth(...)) throw new Error(`Authentication failed…`);
            //       throw new Error(formatNoApiKeyFoundMessage(this.model.provider));
            //   }
            //
            // cyrup consulted only the cached `has_configured_auth` and reported its own
            // `no configured auth for model: p/m`. Two consequences, both user-visible: a provider
            // whose credential is present but outside the cached configured set was refused where
            // pi re-checks and PROCEEDS, and an expired OAuth token produced a message that named
            // neither the provider nor `/login`.
            if !self.has_configured_auth(&model) && !self.recheck_provider_auth(&model).await {
                let provider = model.provider.as_str();
                return Err(SessionServiceError::AuthPreflightRefused(
                    if self.provider_is_oauth_backed(&model.provider).await {
                        crate::auth_guidance::format_oauth_reauthenticate_message(provider)
                    } else {
                        crate::auth_guidance::format_no_api_key_found_message(provider)
                    },
                ));
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
        let pending: Vec<AgentMessage> = std::mem::take(&mut *crate::sync::lock(&self.pending_next_turn));

        // The LIVE base (Pi reads the mutable field: `this._baseSystemPrompt`, agent-session.ts:1228
        // into `emitBeforeAgentStart`, :1252 for the reset) — NOT the frozen builder-assembled
        // `services.system_prompt`, which predates every `set_active_tools_by_name` /
        // `refresh_extension_tools` rebuild this session performed.
        let base = self.base_system_prompt();
        // Fast path: no extension listens for `before_agent_start` — keep the assembled base prompt.
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::BeforeAgentStart)
        {
            // No handler ran, so there is nothing to override with — pi's `else` branch
            // (agent-session.ts:1251 @v0.83.0) clears the slot for exactly this reason, and a stale
            // override from a PREVIOUS run must not leak into this one.
            *crate::sync::lock(&self.system_prompt_override) = None;
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
        // Pi `setActiveTools` (pi-permission-system index.ts:2155): a `before_agent_start` handler may
        // have RESTRICTED the active tool set via `HostServices::set_active_tools` (the permission
        // companion's `shouldExposeTool` shaping), which stages a `(tools, prompt)` push. Drain + apply
        // it IN-TURN here — before `spawn_run` — so the restriction shapes THIS turn (turn 1), not the
        // next turn boundary where `apply_pending_agent_control` would otherwise pick it up. Apply ONLY
        // the restricted tool ARRAY; the `DynamicToolState`-rebuilt prompt is DISCARDED so it cannot
        // clobber the handler's own sanitized system prompt applied just below (pi's `setActiveTools`
        // and its returned `systemPrompt` are independent). Draining it here also leaves
        // `pending_active_tools` empty for the later `apply_pending_agent_control` drains, so the
        // restriction is applied exactly once.
        if let Some((tools, _rebuilt_prompt)) =
            self.services.host_services.take_pending_active_tools()
        {
            self.agent.set_tools(tools).await;
        }
        if let Reduced::Pass(ev) = reduced
            && let HostEvent::BeforeAgentStart { system_prompt, injected, .. } = *ev
        {
            // Apply the (possibly handler-replaced / sanitized) system prompt; reset to base
            // otherwise. Pi's two branches are `if (result?.systemPrompt !== undefined) {
            // this._systemPromptOverride = result.systemPrompt; this.agent.state.systemPrompt =
            // result.systemPrompt; } else { this._systemPromptOverride = undefined;
            // this.agent.state.systemPrompt = this._baseSystemPrompt; }` (agent-session.ts:1246-1252
            // @v0.83.0) — the OVERRIDE SLOT is written on both, which is what makes the turn-boundary
            // refresh able to re-push `override ?? base` without clobbering this sanitization
            // (DRIFT-033).
            //
            // CYRUP-DELTA on the discriminator only: pi distinguishes "handler returned no prompt"
            // (`undefined`) from "handler returned one"; cyrup's `HostEvent::BeforeAgentStart`
            // carries the prompt as a mutated-in-place `String`, so a handler that returns the base
            // verbatim is indistinguishable from one that returns nothing. Equality with `base` is
            // therefore read as "no override", which agrees with pi on the resulting prompt for
            // every input and differs only in which slot holds the identical text.
            if system_prompt == base {
                *crate::sync::lock(&self.system_prompt_override) = None;
                self.agent.set_system_prompt(base.clone()).await;
            } else {
                *crate::sync::lock(&self.system_prompt_override) = Some(system_prompt.clone());
                self.agent.set_system_prompt(system_prompt).await;
            }
            messages.extend(injected.iter().map(core_message_to_agent));
        } else {
            // Blocked/Handled (no Pi analogue here): keep the base prompt, no injection.
            *crate::sync::lock(&self.system_prompt_override) = None;
            self.agent.set_system_prompt(base.clone()).await;
        }
        messages
    }

    /// Await full settlement of the in-flight run AND its post-run loop (R-11-005). On a bound session
    /// the agent goes briefly idle BETWEEN a completed turn and a retry/compaction continuation, so
    /// this first awaits the post-run driver (`driver_tx` is `true` for the whole loop) and only then
    /// the agent — otherwise a one-shot caller would resume mid-loop.
    pub async fn wait_for_idle(&self) {
        let mut rx = self.driver_tx.subscribe();
        while *rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        self.agent.wait_for_idle().await;
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9). Mirrors the
    /// text into the facade queue + emits `queue_update` (Pi `_queueSteer`, agent-session.ts:1249).
    pub async fn steer(&self, input: impl Into<UserInput>) -> Result<PromptAccepted, SessionServiceError> {
        let mut ui = input.into();
        // Pi agent-session.ts:1242-1252: error on an extension command, then expand skill/template
        // BEFORE queueing — the queued text and the mirror must carry the expanded content.
        if ui.expand_templates {
            self.throw_if_extension_command(&ui.text)?;
            ui.text = self.expand_input_text(&ui.text);
        }
        crate::sync::lock(&self.steering_messages).push(ui.text.clone());
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
        let mut ui = input.into();
        // Pi agent-session.ts:1262-1272: error on an extension command, then expand skill/template
        // BEFORE queueing.
        if ui.expand_templates {
            self.throw_if_extension_command(&ui.text)?;
            ui.text = self.expand_input_text(&ui.text);
        }
        crate::sync::lock(&self.follow_up_messages).push(ui.text.clone());
        self.agent.follow_up(ui.into_agent_message());
        self.emit_queue_update().await;
        Ok(PromptAccepted::Queued(StreamingBehavior::FollowUp))
    }

    /// Error if `text` is a registered extension command (Pi `_throwIfExtensionCommand`,
    /// agent-session.ts:1312-1321): extension commands cannot be queued via `steer`/`follow_up`.
    /// Only `/`-prefixed text is checked; the registry covers native + wasm commands.
    fn throw_if_extension_command(&self, text: &str) -> Result<(), SessionServiceError> {
        let Some(body) = text.strip_prefix('/') else { return Ok(()) };
        let name = body.split_once(' ').map_or(body, |(n, _)| n);
        if self.services.ext_host.registry().has_command(name).unwrap_or(false) {
            return Err(SessionServiceError::ExtensionCommandNotQueueable(name.to_string()));
        }
        Ok(())
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

/// The concatenated text of a `user` agent message, or `None` for any other role (Pi
/// `_getUserMessageText`, agent-session.ts:589-595). Used to match a streaming user message against
/// the facade steer/follow-up queue mirrors so they drain in lockstep with the agent.
fn agent_user_text(m: &AgentMessage) -> Option<String> {
    match m {
        AgentMessage::User { content, .. } => Some(
            content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}
