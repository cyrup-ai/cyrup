//! Message injection — appending messages a run did not produce.
//!
//! Pi `sendCustomMessage`/`sendUserMessage`/`injectMessage` (agent-session.ts:1313-1339). Persists
//! a custom or user message into the session tree and either fans it out for display, stages it to
//! ride the next turn, or triggers a turn of its own.

use cyrup_agent::AgentMessage;
use cyrup_core::EntryId;
use cyrup_ext::host::HostServices;

use crate::error::SessionServiceError;
use crate::event::{AgentSessionEvent, PromptAccepted, StreamingBehavior, UserInput};

use super::run::InjectionOffer;
use super::{AgentSession, now_ms};

impl AgentSession {
    /// Persist a custom (non-LLM) message via the session tree (Pi `sendCustomMessage` durable path,
    /// agent-session.ts:1313). The agent transcript carries it as a `Custom` role for the next run.
    pub async fn append_custom_message(
        &self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
    ) -> Result<EntryId, SessionServiceError> {
        let id =
            self.manager
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
        // AGENT-030 — pi routes on `this.isStreaming`, which IS the session latch
        // `_isAgentRunActive` (agent-session.ts:900-901, consulted at :1190): a submission landing
        // in the post-`agent_end` gap queues onto the active loop instead of starting a second run.
        if self.is_run_active() {
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
            // Carried on the live message too, not just the durable arm below: the steer, follow-up
            // and next-turn arms surface through `message_end`, which is the renderer's surface.
            details: details.clone(),
            // SUBA-094 — same reason, and the same line of pi: `sendCustomMessage` puts `display` on
            // the ONE `appMessage` it builds (`agent-session.ts:1488-1496` @v0.84.4) and hands that
            // object to all five branches. Every arm below surfaces through `message_end`, so a
            // `display: false` message that took the steer / follow-up / next-turn route used to be
            // drawn anyway.
            display,
            timestamp: Some(ts),
        };
        match deliver_as {
            Some(DeliverAs::NextTurn) => {
                Self::lock(&self.pending_next_turn).push(msg);
            }
            // AGENT-030 — pi routes on `this.isStreaming`, the session latch `_isAgentRunActive`
            // (agent-session.ts:900-901, consulted at :1477): in the post-`agent_end` gap the
            // message queues onto the active loop instead of being appended between runs.
            _ if self.is_run_active() => match deliver_as {
                Some(DeliverAs::FollowUp) => self.agent.follow_up(msg),
                _ => self.agent.steer(msg),
            },
            _ => {
                self.manager.lock().await.append_custom_message(
                    custom_type,
                    content,
                    display,
                    details,
                )?;
                self.fanout_emit(AgentSessionEvent::MessageStart {
                    message: msg.clone(),
                })
                .await;
                self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg })
                    .await;
            }
        }
        Ok(())
    }

    /// Deliver one coalesced injection plan as ONE turn.
    ///
    /// Called only by the session's injection pump, which owns the batch and re-offers it on the
    /// next idle edge when this reports [`InjectionOffer::AgentBusy`]. That ownership is why this
    /// function has no queueing arm and no retry of its own: the two things the previous
    /// implementation did here — `agent.steer` onto an active run, and a `spawn_run` whose
    /// refusal was swallowed by the driver — are exactly the two ways an injected message used to
    /// disappear.
    ///
    /// # Errors
    ///
    /// A durable append failure, or an agent fault that waiting cannot fix. A busy agent is NOT an
    /// error: it is [`InjectionOffer::AgentBusy`].
    pub(super) async fn deliver_injection_inbox(
        &self,
        plan: InjectionPlan,
    ) -> Result<InjectionOffer, SessionServiceError> {
        // The no-turn members first: they are independent of the run latch, and persisting them
        // before a possible `AgentBusy` below means a re-offered batch never re-persists them.
        for msg in plan.durable {
            self.append_injected_message_durably(msg).await?;
        }
        if plan.turn.is_empty() {
            return Ok(InjectionOffer::Taken);
        }
        // Pi `_runAgentPrompt(appMessage)`: the turn's input IS the injected message(s). On
        // `Taken` the agent has claimed its latch with these messages already pushed onto the
        // run's transcript, so `message_end` — and therefore the durable persist — necessarily
        // follows; that is what makes acceptance a sufficient acknowledgement.
        self.run_injection(plan.turn).await
    }

    /// Persist an injected message that asked for no turn, and surface it (Pi's else-branch,
    /// agent-session.ts:1337-1370).
    async fn append_injected_message_durably(
        &self,
        msg: AgentMessage,
    ) -> Result<(), SessionServiceError> {
        let AgentMessage::Custom {
            kind,
            payload,
            details,
            display,
            ..
        } = &msg
        else {
            return Ok(());
        };
        self.manager.lock().await.append_custom_message(
            kind,
            payload.clone(),
            *display,
            details.clone(),
        )?;
        self.fanout_emit(AgentSessionEvent::MessageStart {
            message: msg.clone(),
        })
        .await;
        self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg })
            .await;
        Ok(())
    }

    /// Inject a host-originated message into the live session and optionally trigger an agent turn
    /// (Pi `sendCustomMessage(message, { triggerTurn })`, agent-session.ts:1337-1370). Backs the
    /// [`crate::host_services::LiveHostServices`] injection seam a background task drives
    /// (R-SA-101 / P-2) — the seam that surfaces a completed background result INTO the parent
    /// session's turn loop instead of stderr.
    ///
    /// # This is a producer, not a router
    ///
    /// It builds the message and hands it to the session's injection pump. It deliberately does
    /// NOT decide between steering an active run and starting a new one: that decision used to be
    /// made here, from a `is_run_active()` read that was not atomic with the act that followed,
    /// and both of its arms lost messages — a steer landing after the run loop's last drain point
    /// strands forever, and concurrent `spawn_run`s all but one lost `Agent::prompt`'s latch CAS
    /// and were warn-logged away. Scheduling belongs to the single consumer that owns the inbox.
    ///
    /// # Errors
    ///
    /// The plain-user-message arm propagates its prompt preflight. The custom arm fails only if
    /// the pump is unreachable, in which case nothing was queued.
    pub async fn inject_message(
        &self,
        content: String,
        custom_type: Option<String>,
        display: bool,
        details: Option<serde_json::Value>,
        trigger_turn: bool,
    ) -> Result<(), SessionServiceError> {
        let Some(kind) = custom_type else {
            // A plain user message: Pi `sendUserMessage` always triggers a turn (and steers/follows-up
            // while streaming). Boxed like the `SendUserMessage` control edge (`apply_pending_control`)
            // so the re-entry into the prompt path stays finitely sized (E0733). It takes a bare
            // string in pi, so it carries no `details` to drop.
            let _ = Box::pin(self.send_user_message(content, None)).await?;
            return Ok(());
        };
        self.services
            .host_services
            .inject_message(
                &content,
                Some(&kind),
                display,
                details.as_ref(),
                trigger_turn,
            )
            .map_err(SessionServiceError::InjectUnavailable)
    }
}

/// One merge group: the messages that will become a single [`AgentMessage`].
///
/// A named struct rather than the four-element tuple this started as — `(Option<String>, bool,
/// Vec<String>, Option<Value>)` has two fields that are trivially transposable and two more whose
/// meaning is positional only.
struct MergeGroup {
    custom_type: Option<String>,
    display: bool,
    trigger_turn: bool,
    bodies: Vec<String>,
    details: Option<serde_json::Value>,
}

impl MergeGroup {
    fn new(message: &crate::host_services::InjectMessage) -> Self {
        Self {
            custom_type: message.custom_type.clone(),
            display: message.display,
            trigger_turn: message.trigger_turn,
            bodies: vec![message.content.clone()],
            details: message.details.clone(),
        }
    }

    /// Fold another message of the same kind in: bodies accumulate, the two flags OR (pi's
    /// `items.some(..)`), and the first non-empty `details` wins — concatenating two opaque
    /// renderer payloads would produce a value no renderer declared.
    fn absorb(&mut self, message: &crate::host_services::InjectMessage) {
        self.display = self.display || message.display;
        self.trigger_turn = self.trigger_turn || message.trigger_turn;
        self.bodies.push(message.content.clone());
        if self.details.is_none() {
            self.details = message.details.clone();
        }
    }
}

/// One coalesced batch, split by what it needs from the session.
///
/// A named struct rather than the `(Vec<AgentMessage>, Vec<AgentMessage>)` an earlier draft used:
/// both halves have the SAME type, so a transposed destructuring compiles cleanly and would run
/// the durable-only messages as turn input while persisting the turn input twice.
#[derive(Debug, Default)]
pub(super) struct InjectionPlan {
    /// Messages whose delivery is a turn (`trigger_turn`), merged per group.
    pub(super) turn: Vec<AgentMessage>,
    /// Messages that asked for no turn: persisted and surfaced, never run.
    pub(super) durable: Vec<AgentMessage>,
}

/// Split one coalesced inbox into an [`InjectionPlan`].
///
/// # The functional core of the injection pump
///
/// Pure: no I/O, no session, no latch, no channel. Everything the pump does that is worth
/// reasoning about in isolation — how a fan-out's three completions become one turn, in what
/// order, and which of them still need their own durable append — happens here, against explicit
/// inputs.
///
/// Members are merged by `(custom_type, display, trigger_turn)` — in practice every member of a
/// background-completion batch is a `subagent-notify` — with the bodies joined by a blank line and
/// `display`/`trigger_turn` OR'd, which is pi's own batching (`sendCompletion` builds one message
/// from an array of completion details and ORs their `triggerTurn`, `notify.ts:399-412` @v0.64.0).
///
/// A `custom_type: None` member is never merged: a plain user message is a different kind of turn
/// input and must keep its own identity.
pub(super) fn merge_injection_batch(
    inbox: &[crate::host_services::InjectRequest],
) -> InjectionPlan {
    // Insertion-ordered grouping: the orchestrator reads the blocks in completion order, so a
    // hash-ordered merge would scramble a fan-out's results.
    let mut groups: Vec<MergeGroup> = Vec::new();
    for req in inbox {
        let message = &req.message;
        let existing = message
            .custom_type
            .is_some()
            .then(|| {
                groups
                    .iter()
                    .position(|group| group.custom_type == message.custom_type)
            })
            .flatten();
        match existing.and_then(|index| groups.get_mut(index)) {
            Some(group) => group.absorb(message),
            None => groups.push(MergeGroup::new(message)),
        }
    }

    let mut plan = InjectionPlan::default();
    for group in groups {
        let MergeGroup {
            custom_type,
            display,
            trigger_turn,
            bodies,
            details,
        } = group;
        let content = bodies.join("\n\n");
        let msg = match custom_type {
            Some(kind) => AgentMessage::Custom {
                kind,
                payload: serde_json::Value::String(content),
                details,
                // SUBA-094 — pi's `_runAgentPrompt(appMessage)` (`agent-session.ts:1505` @v0.84.4)
                // runs the turn over the SAME object that carries `display`, and the interactive
                // host gates drawing on `message.display` (`interactive-mode.ts:3609`), so a
                // background completion whose predicate said `display: false` (`pi-subagents
                // notify.ts:402` @v0.64.0) reaches the model and not the screen.
                display,
                timestamp: Some(now_ms()),
            },
            None => AgentMessage::user_text(content),
        };
        if trigger_turn {
            plan.turn.push(msg);
        } else {
            plan.durable.push(msg);
        }
    }
    plan
}
