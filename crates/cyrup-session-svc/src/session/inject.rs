//! Message injection — appending messages a run did not produce.
//!
//! Pi `sendCustomMessage`/`sendUserMessage`/`injectMessage` (agent-session.ts:1313-1339). Persists
//! a custom or user message into the session tree and either fans it out for display, stages it to
//! ride the next turn, or triggers a turn of its own.

use cyrup_agent::AgentMessage;
use cyrup_core::EntryId;

use crate::error::SessionServiceError;
use crate::event::{AgentSessionEvent, PromptAccepted, StreamingBehavior, UserInput};

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
            // Carried on the live message too, not just the durable arm below: the steer, follow-up
            // and next-turn arms surface through `message_end`, which is the renderer's surface.
            details: details.clone(),
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

    /// Inject a host-originated message into the live session and optionally trigger an agent turn
    /// (Pi `sendCustomMessage(message, { triggerTurn })`, agent-session.ts:1337-1370). Backs the
    /// late-bound [`crate::host_services::LiveHostServices`] inject sink a background task drives
    /// (R-SA-101 / P-2) — the seam that surfaces a completed background result INTO the parent
    /// session's turn loop instead of stderr. Reproduces Pi's three cases:
    ///
    /// * **`custom_type = None`** — a plain user message: Pi `sendUserMessage`, which ALWAYS triggers a
    ///   turn (steer/follow-up while streaming). `display`/`trigger_turn` don't apply to a user message.
    /// * **`Some(kind)` while streaming** — queue the custom message onto the active run (Pi `steer`).
    /// * **`Some(kind)`, idle, `trigger_turn`** — run a fresh turn OVER the custom message (Pi
    ///   `_runAgentPrompt(appMessage)`, `spawn_run(vec![msg])`) — the `triggerTurn` branch cyrup's
    ///   `send_custom_message` lacked.
    /// * **`Some(kind)`, idle, no `trigger_turn`** — persist + surface durably (Pi's else-branch).
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
        let msg = AgentMessage::Custom {
            kind: kind.clone(),
            payload: serde_json::Value::String(content.clone()),
            details: details.clone(),
            timestamp: Some(now_ms()),
        };
        if self.is_streaming().await {
            // Pi: while streaming, queue onto the active run (steer).
            self.agent.steer(msg);
        } else if trigger_turn {
            // Pi `_runAgentPrompt(appMessage)`: run a turn whose input IS the injected message.
            self.spawn_run(vec![msg]).await?;
        } else {
            // Pi else-branch: append durably + surface via message_start/message_end.
            self.manager
                .lock()
                .await
                .append_custom_message(&kind, serde_json::Value::String(content), display, details)?;
            self.fanout_emit(AgentSessionEvent::MessageStart { message: msg.clone() }).await;
            self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg }).await;
        }
        Ok(())
    }
}
