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
use crate::host_services::{InjectAck, InjectRequest};

/// A no-turn injection the pump handed to a live run's steering queue, kept with its ack until the
/// idle edge settles whether the run drained it (ICOM-035).
#[derive(Debug)]
pub(super) struct SteeredInjection {
    /// The exact message steered — compared by value to find it in the agent's queue again.
    pub(super) message: AgentMessage,
    /// Answered once its fate is settled.
    pub(super) ack: InjectAck,
}

/// Whether a no-turn append waits out a running compaction ([`AgentSession::append_no_turn_messages`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactionGate {
    /// The injection pump: a compaction is busy (SEAM-125), so report
    /// [`InjectionOffer::AgentBusy`] and let the next idle edge re-offer.
    Wait,
    /// `send_custom_message`: append now, as pi's `_appendCustomMessage` does.
    Ignore,
}

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
            // ICOM-068 — pi's not-streaming, no-trigger arm is `_appendCustomMessage`: the tree
            // append AND `_refreshFinalizedContext()`, which re-seeds `agent.state.messages` from the
            // session projection (`agent-session.ts:1968-1982`, `:730-736` @v0.87.1), so the model
            // sees the message on the next prompt. This arm used to do the tree append alone: the
            // message was drawn and persisted and never sent to the model until a resume, fork or
            // compaction re-seeded the transcript from the tree.
            _ => {
                match self
                    .append_no_turn_messages(vec![msg.clone()], &[], CompactionGate::Ignore)
                    .await?
                {
                    InjectionOffer::Taken => {}
                    // A run claimed the latch between the routing read above and the append's
                    // critical section. It is streaming now, so this is pi's streaming arm.
                    InjectionOffer::AgentBusy => self.agent.steer(msg),
                }
            }
        }
        Ok(())
    }

    /// ICOM-035 — hand every no-turn custom injection in `inbox` to the live run's steering queue.
    ///
    /// pi routes `sendCustomMessage(msg, { deliverAs: "steer" })` to `this.agent.steer(appMessage)`
    /// while `isStreaming` (`agent-session.ts:1949-1954` @v0.87.1), which is how pi-intercom's busy
    /// delivery (`index.ts:1221-1246` @v0.14.0) reaches the running model at its next steering
    /// boundary, in the SAME run. The pump used to hold these until the run was over and then append
    /// them with no turn, so a supervisor's redirect arrived after the work it was redirecting.
    ///
    /// Steering is not the end of the pump's responsibility: a steer that lands after the run
    /// loop's last `poll_steering` is left in the agent's queue with the session idle — the stranding
    /// the pump exists to prevent. Each steered message therefore moves into `steered` together
    /// with its ack, and [`Self::append_no_turn_messages`] settles its fate at the idle edge: drained
    /// by the run (delivered), or still queued (taken back and appended then).
    ///
    /// Only messages injected with pi's `deliverAs: "steer"`
    /// ([`cyrup_ext::host::HostServices::inject_message_steer`]) steer. A turn-triggering one keeps
    /// waiting for the idle edge (the pump's documented turn-boundary contract), and so does a
    /// plain no-turn message: pi defers `{ triggerTurn: false }` on a streaming session instead of
    /// steering it (`_pendingCustomMessages`, `agent-session.ts:1962-1967`), because a steer the
    /// model answers can extend the run — which a notice that asked for no turn must never do.
    pub(super) fn steer_injections_onto_live_run(
        &self,
        inbox: &mut Vec<InjectRequest>,
        steered: &mut Vec<SteeredInjection>,
    ) {
        if inbox.is_empty() || !self.is_run_active() {
            return;
        }
        let mut kept = Vec::with_capacity(inbox.len());
        for req in inbox.drain(..) {
            let InjectRequest { message, ack } = req;
            let Some(kind) = message
                .custom_type
                .clone()
                .filter(|_| message.steer && !message.trigger_turn)
            else {
                kept.push(InjectRequest { message, ack });
                continue;
            };
            let msg = AgentMessage::Custom {
                kind,
                payload: serde_json::Value::String(message.content),
                details: message.details,
                display: message.display,
                timestamp: Some(now_ms()),
            };
            self.agent.steer(msg.clone());
            steered.push(SteeredInjection { message: msg, ack });
        }
        *inbox = kept;
    }

    /// Append no-turn custom messages to the session tree AND the agent's transcript, as ONE
    /// critical section, and surface them — pi's `_appendCustomMessage` (`agent-session.ts:1968-1982`
    /// @v0.87.1: `appendCustomMessageEntry` → `_refreshFinalizedContext()` → `message_start`/`_end`).
    ///
    /// `steered` are the pump's messages already handed to a run ([`Self::steer_injections_onto_live_run`]):
    /// any of them still sitting in the agent's steering queue landed past that run's last poll, so
    /// they are taken back and appended here, AHEAD of `messages` (they arrived first). The ones the
    /// run drained are already in its transcript and tree and are left alone.
    ///
    /// # Why one critical section (ICOM-068)
    ///
    /// The tree append runs under the manager lock, which a starting run's `message_end` persist
    /// also needs, and the transcript push goes through [`cyrup_agent::Agent::edit_transcript`],
    /// which reads the run latch under the state lock a run's claim snapshots under. So a racing
    /// prompt either sees neither (the edit refused: [`InjectionOffer::AgentBusy`], nothing written,
    /// any taken-back steer restored to the queue head for that run to drain) or both, in the same
    /// order in the tree and in its request.
    ///
    /// # Errors
    ///
    /// A tree append failure. The transcript push for the messages the tree did not take is rolled
    /// back first, so the two never disagree about a message that failed.
    pub(super) async fn append_no_turn_messages(
        &self,
        messages: Vec<AgentMessage>,
        steered: &[SteeredInjection],
        compaction: CompactionGate,
    ) -> Result<InjectionOffer, SessionServiceError> {
        // Only a custom message has a no-turn delivery (pi's `sendMessage`); anything else here is a
        // producer bug that was always dropped, and stays dropped rather than guessed at.
        let messages: Vec<AgentMessage> = messages
            .into_iter()
            .filter(|m| matches!(m, AgentMessage::Custom { .. }))
            .collect();
        if messages.is_empty() && steered.is_empty() {
            return Ok(InjectionOffer::Taken);
        }
        let mut manager = self.manager.lock().await;
        // Re-read under the lock: the caller's idle observation is already stale. A manual
        // compaction is honoured by the pump (it can discard what is appended under it — pi-intercom
        // holds for exactly that reason, `index.ts:1336`) and ignored by the public
        // `send_custom_message`, whose pi counterpart appends regardless.
        if self.is_run_active()
            || (matches!(compaction, CompactionGate::Wait) && self.is_compacting())
        {
            return Ok(InjectionOffer::AgentBusy);
        }
        let mut owned: Vec<&AgentMessage> = steered.iter().map(|s| &s.message).collect();
        let stranded = self.agent.take_steering_where(|queued| {
            match owned.iter().position(|mine| *mine == queued) {
                Some(index) => {
                    owned.remove(index);
                    true
                }
                None => false,
            }
        });
        let mut batch = stranded.clone();
        batch.extend(messages);
        if batch.is_empty() {
            // Every steer was drained by its run: delivered, nothing left to append.
            return Ok(InjectionOffer::Taken);
        }
        let Ok(len_after) = self.agent.edit_transcript(|transcript| {
            transcript.extend(batch.iter().cloned());
            transcript.len()
        }) else {
            // A run claimed the latch after the read above. Its snapshot is taken after this edit
            // would have been, so hand it the stranded steers back to drain.
            self.agent.restore_steering_front(stranded);
            return Ok(InjectionOffer::AgentBusy);
        };
        for (index, msg) in batch.iter().enumerate() {
            let AgentMessage::Custom {
                kind,
                payload,
                details,
                display,
                ..
            } = msg
            else {
                continue;
            };
            if let Err(fault) =
                manager.append_custom_message(kind, payload.clone(), *display, details.clone())
            {
                let unpersisted = batch.len() - index;
                // Best effort: refused only if a run has claimed since, in which case it already
                // snapshotted the messages and there is nothing coherent left to undo.
                let _ = self.agent.edit_transcript(|transcript| {
                    if transcript.len() == len_after {
                        transcript.truncate(len_after - unpersisted);
                    }
                });
                return Err(fault.into());
            }
        }
        drop(manager);
        for msg in batch {
            self.fanout_emit(AgentSessionEvent::MessageStart {
                message: msg.clone(),
            })
            .await;
            self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg })
                .await;
        }
        Ok(InjectionOffer::Taken)
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
    }
}

/// The key a message merges under, or `None` when it must stay its own message.
///
/// A plain user message (`custom_type: None`) keeps its identity: it is a different kind of turn
/// input. So does a custom message that carries `details` — its renderer payload describes THAT
/// message only (an intercom card is the sender, the envelope and the body of one message), so a
/// merge could neither concatenate two payloads into a value no renderer declared nor keep one
/// without drawing every other member's body under the first member's card. Upstream never has to
/// choose: each `pi.sendMessage` is its own message. The subagent completion batches this merging
/// exists for carry no `details`, so they merge exactly as before.
fn merge_key(message: &crate::host_services::InjectMessage) -> Option<&str> {
    if message.details.is_some() {
        return None;
    }
    message.custom_type.as_deref()
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

/// Split an inbox into the requests whose merge group asks for NO turn and those whose group does,
/// by exactly [`merge_injection_batch`]'s grouping rule (by `custom_type`, `trigger_turn` OR'd across
/// the group; a `custom_type: None` member is its own group) — so merging each half yields the
/// `durable` and the `turn` half of merging the whole, in the same order.
///
/// The pump needs the split at the REQUEST level: the no-turn half is delivered (and its acks
/// answered) before the turn half is offered to the agent, and when that offer comes back
/// [`InjectionOffer::AgentBusy`] only the turn half may be kept for the next idle edge. Re-merging
/// the whole inbox there persisted the no-turn members a second time.
pub(super) fn split_by_group_trigger(
    inbox: Vec<InjectRequest>,
) -> (Vec<InjectRequest>, Vec<InjectRequest>) {
    let group_triggers = |key: &str| {
        inbox
            .iter()
            .filter(|req| merge_key(&req.message) == Some(key))
            .any(|req| req.message.trigger_turn)
    };
    let triggers: Vec<bool> = inbox
        .iter()
        .map(|req| match merge_key(&req.message) {
            Some(key) => group_triggers(key),
            None => req.message.trigger_turn,
        })
        .collect();
    let mut durable = Vec::new();
    let mut turn = Vec::new();
    for (req, triggers) in inbox.into_iter().zip(triggers) {
        if triggers {
            turn.push(req);
        } else {
            durable.push(req);
        }
    }
    (durable, turn)
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
/// Members are merged by `custom_type` ([`merge_key`]) — in practice every member of a
/// background-completion batch is a `subagent-notify` — with the bodies joined by a blank line and `display`/`trigger_turn`
/// OR'd across the group (so one loud member wakes a turn for its quiet siblings), which is pi's own
/// batching (`sendCompletion` builds one message
/// from an array of completion details and ORs their `triggerTurn`, `notify.ts:399-412` @v0.64.0).
///
/// A `custom_type: None` member is never merged: a plain user message is a different kind of turn
/// input and must keep its own identity. Nor is a member that carries `details` (see
/// [`merge_key`]).
pub(super) fn merge_injection_batch(
    inbox: &[crate::host_services::InjectRequest],
) -> InjectionPlan {
    // Insertion-ordered grouping: the orchestrator reads the blocks in completion order, so a
    // hash-ordered merge would scramble a fan-out's results.
    let mut groups: Vec<MergeGroup> = Vec::new();
    for req in inbox {
        let message = &req.message;
        // A group formed by a message that carries `details` has `details: Some` and so is never
        // joined: `merge_key` is `None` for it, and only same-key, detail-free groups match.
        let existing = merge_key(message).and_then(|key| {
            groups.iter().position(|group| {
                group.details.is_none() && group.custom_type.as_deref() == Some(key)
            })
        });
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! SUBA-017 — `merge_injection_batch` is the only thing between a fan-out's N completions and
    //! N turns, and it had no test. Each test names the mutation it kills.

    use super::{InjectionPlan, merge_injection_batch, split_by_group_trigger};
    use crate::host_services::{InjectAck, InjectMessage, InjectRequest};
    use cyrup_agent::AgentMessage;

    fn req(kind: Option<&str>, body: &str, display: bool, trigger_turn: bool) -> InjectRequest {
        InjectRequest {
            message: InjectMessage {
                content: body.to_string(),
                custom_type: kind.map(str::to_string),
                display,
                details: None,
                trigger_turn,
                steer: false,
            },
            ack: InjectAck::detached(),
        }
    }

    /// `(kind, body, display)` of a planned custom message; a user message reads as kind `"<user>"`.
    fn shape(msg: &AgentMessage) -> (String, String, bool) {
        match msg {
            AgentMessage::Custom {
                kind,
                payload,
                display,
                ..
            } => (
                kind.clone(),
                payload.as_str().unwrap_or_default().to_string(),
                *display,
            ),
            AgentMessage::User { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<String>();
                ("<user>".to_string(), text, true)
            }
            other => panic!("unexpected planned message {other:?}"),
        }
    }

    fn shapes(plan_half: &[AgentMessage]) -> Vec<(String, String, bool)> {
        plan_half.iter().map(shape).collect()
    }

    /// Same-type members become ONE message whose bodies keep arrival order, and groups keep the
    /// order of their first member. Kills: a hash-ordered grouping (group or body order scrambles),
    /// and "no merge" (three `subagent-notify` turn messages instead of one).
    #[test]
    fn merge_injection_batch_merges_same_type_in_order() {
        let inbox = vec![
            req(Some("subagent-notify"), "c1", false, true),
            req(Some("watchdog-warning"), "w1", true, true),
            req(Some("subagent-notify"), "c2", false, true),
            req(Some("subagent-notify"), "c3", false, true),
        ];
        let InjectionPlan { turn, durable } = merge_injection_batch(&inbox);
        assert!(durable.is_empty(), "every member asked for a turn");
        assert_eq!(
            shapes(&turn),
            vec![
                ("subagent-notify".into(), "c1\n\nc2\n\nc3".into(), false),
                ("watchdog-warning".into(), "w1".into(), true),
            ],
            "one message per type, bodies in arrival order, groups in first-arrival order"
        );
    }

    /// `display` and `trigger_turn` OR across a group: one displayed member displays the group, and
    /// one turn-triggering member carries its quiet siblings into the turn (pi `items.some(..)`).
    /// Kills: AND instead of OR on either flag (the group would be hidden, or land in `durable`).
    #[test]
    fn a_groups_flags_are_ored_across_its_members() {
        let inbox = vec![
            req(Some("subagent-notify"), "quiet", false, false),
            req(Some("subagent-notify"), "loud", true, true),
        ];
        let InjectionPlan { turn, durable } = merge_injection_batch(&inbox);
        assert!(
            durable.is_empty(),
            "the loud member pulls the group into the turn"
        );
        assert_eq!(
            shapes(&turn),
            vec![("subagent-notify".into(), "quiet\n\nloud".into(), true)]
        );
    }

    /// A group none of whose members asked for a turn is persisted, never run. Kills: routing on
    /// anything but the group's OR'd `trigger_turn` (e.g. every group into `turn`).
    #[test]
    fn a_group_with_no_turn_request_is_durable_only() {
        let inbox = vec![
            req(Some("status-note"), "a", true, false),
            req(Some("subagent-notify"), "b", false, true),
            req(Some("status-note"), "c", false, false),
        ];
        let InjectionPlan { turn, durable } = merge_injection_batch(&inbox);
        assert_eq!(
            shapes(&turn),
            vec![("subagent-notify".into(), "b".into(), false)]
        );
        assert_eq!(
            shapes(&durable),
            vec![("status-note".into(), "a\n\nc".into(), true)]
        );
    }

    /// A plain user message (`custom_type: None`) keeps its own identity: two of them are two turn
    /// inputs, never one merged body. Kills: dropping the `custom_type.is_some()` guard, which
    /// would merge every `None` into one user message.
    #[test]
    fn plain_user_messages_are_never_merged() {
        let inbox = vec![
            req(None, "first", true, true),
            req(None, "second", true, true),
        ];
        let InjectionPlan { turn, durable } = merge_injection_batch(&inbox);
        assert!(durable.is_empty());
        assert_eq!(
            shapes(&turn),
            vec![
                ("<user>".into(), "first".into(), true),
                ("<user>".into(), "second".into(), true),
            ]
        );
    }

    /// The pump's request-level split agrees with the message-level merge: a no-turn member whose
    /// group has a turn-triggering sibling rides the turn, a group with none is durable, a plain
    /// user message is its own group, and each half keeps arrival order. Kills: splitting per
    /// request instead of per group (the quiet `subagent-notify` sibling would land in `durable`
    /// and be appended as well as run).
    #[test]
    fn split_by_group_trigger_matches_the_merge_grouping() {
        let inbox = vec![
            req(Some("subagent-notify"), "quiet", false, false),
            req(Some("intercom_message"), "note-1", true, false),
            req(None, "user-quiet", true, false),
            req(Some("subagent-notify"), "loud", true, true),
            req(Some("intercom_message"), "note-2", true, false),
        ];
        let (durable, turn) = split_by_group_trigger(inbox);
        let bodies = |v: &[crate::host_services::InjectRequest]| -> Vec<String> {
            v.iter().map(|r| r.message.content.clone()).collect()
        };
        assert_eq!(bodies(&durable), vec!["note-1", "user-quiet", "note-2"]);
        assert_eq!(bodies(&turn), vec!["quiet", "loud"]);
        let InjectionPlan { turn: planned, .. } = merge_injection_batch(&turn);
        assert_eq!(
            shapes(&planned),
            vec![("subagent-notify".into(), "quiet\n\nloud".into(), true)]
        );
    }

    /// A custom message that carries `details` (a renderer payload such as an intercom card) is
    /// never merged: two held intercom messages flushed together stay two messages, each with its
    /// own card, and a detail-free sibling of the same type does not absorb into either. Kills:
    /// merging on `custom_type` alone (one message, the second card's payload silently dropped).
    #[test]
    fn a_message_with_details_keeps_its_own_identity() {
        let with_details = |body: &str| {
            let mut r = req(Some("intercom_message"), body, true, true);
            r.message.details = Some(serde_json::json!({ "card": body }));
            r
        };
        let inbox = vec![
            with_details("m1"),
            with_details("m2"),
            req(Some("intercom_message"), "plain", true, true),
        ];
        let InjectionPlan { turn, durable } = merge_injection_batch(&inbox);
        assert!(durable.is_empty());
        assert_eq!(
            shapes(&turn),
            vec![
                ("intercom_message".into(), "m1".into(), true),
                ("intercom_message".into(), "m2".into(), true),
                ("intercom_message".into(), "plain".into(), true),
            ]
        );
        let details: Vec<_> = turn
            .iter()
            .map(|m| match m {
                AgentMessage::Custom { details, .. } => details.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(
            details,
            vec![
                Some(serde_json::json!({ "card": "m1" })),
                Some(serde_json::json!({ "card": "m2" })),
                None
            ]
        );
    }
}
