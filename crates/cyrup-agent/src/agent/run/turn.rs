//! The turn driver (Pi `runLoop`, `agent-loop.ts:162-320` @v0.87.1): steering/follow-up injection,
//! the per-request and per-turn hooks whose overrides are folded back into the run baseline, the
//! assistant turn, the tool batch, and `finishTurn`'s end/continue decision.

use super::{RunCtx, RunFailure};
use crate::agent::message::tool_calls;
use crate::event::{AgentEvent, AgentMessage, ToolResultMessage};
use crate::hooks::{AgentContextView, PostTurn, PrepareRequestCtx, RequestUpdate, TurnDecision};
use cyrup_core::{AssistantMessage, StopReason};
use std::sync::Arc;

/// The turn the next `prepare_next_turn` describes (Pi `lastCompletedTurn`, `agent-loop.ts:172`).
/// Its `context` / `newMessages` are the loop's live state, so only the turn's own parts are kept.
struct CompletedTurn {
    message: Arc<AssistantMessage>,
    tool_results: Vec<ToolResultMessage>,
}

impl RunCtx {
    pub(super) async fn run_loop(&mut self) -> Result<(), RunFailure> {
        let mut last_completed: Option<CompletedTurn> = None;
        // Pi `explicitContinuation` (`agent-loop.ts:173`): a `finishTurn` `continue` that no natural
        // request (tool results, steering, follow-up) has satisfied yet.
        let mut explicit_continuation = false;
        // Pi polls steering at the very top (agent-loop.ts:175), but a continue-from-assistant run
        // already drained one steering message and passes it as the prompt; `skipInitialSteeringPoll`
        // makes this first poll return `[]` so the next queued steering message is not drained a turn
        // too early under `one-at-a-time` (agent.ts:351,440-446).
        let mut pending = if self.skip_initial_steering_poll {
            self.skip_initial_steering_poll = false;
            Vec::new()
        } else {
            self.poll_steering()
        };
        loop {
            let mut has_more_tools = true;
            while has_more_tools || !pending.is_empty() {
                // The run entry already emitted the first turn's `turn_start`; every later turn is
                // prepared first (Pi `if (lastCompletedTurn)`, agent-loop.ts:184-205).
                let mut prepared = Vec::new();
                if let Some(turn) = &last_completed {
                    if let Some(update) = self.prepare_next_turn(turn).await? {
                        let (request, messages) = update.into_parts();
                        self.apply_update(request);
                        prepared = messages;
                    }
                    // AGENT-036 — "Preparation can be long-running (for example, compaction). Pick
                    // up steering queued while it ran. Only poll again if the earlier poll returned
                    // nothing; otherwise one-at-a-time mode would deliver two messages in this
                    // turn." (agent-loop.ts:200-205)
                    if pending.is_empty() {
                        pending = self.poll_steering();
                    }
                    self.emit(AgentEvent::TurnStart).await?;
                }

                // Prepared messages first, then the queued ones (`[...preparedMessages,
                // ...pendingMessages]`, agent-loop.ts:210).
                for m in prepared.into_iter().chain(std::mem::take(&mut pending)) {
                    self.emit(AgentEvent::MessageStart { message: m.clone() })
                        .await?;
                    self.emit(AgentEvent::MessageEnd { message: m.clone() })
                        .await?;
                    // Pi pushes each injected message onto the loop's working copy
                    // (`currentContext.messages.push`, agent-loop.ts:213).
                    let m = Arc::new(m);
                    self.messages.push(Arc::clone(&m));
                    self.new_messages.push(m);
                }

                self.prepare_request().await?;

                // The turn's assistant message is shared by the two working transcripts and the
                // `turn_end` event, so build the handle ONCE and clone the pointer. Wrapping at
                // each use (`Arc::new(asst.clone())`) allocates a fresh `Arc` and deep-copies the
                // message into it, which defeats the sharing at the moment it is created.
                let asst = self.stream_assistant().await?;
                // Pi's `streamAssistantResponse` leaves the final assistant message in the loop's
                // working copy (`currentContext.messages`); mirror that before tool execution /
                // `finish_turn` read the context.
                let asst_msg = Arc::new(AgentMessage::Assistant(Arc::clone(&asst)));
                self.messages.push(Arc::clone(&asst_msg));
                self.new_messages.push(asst_msg);

                if matches!(asst.stop_reason, StopReason::Error | StopReason::Aborted) {
                    // `finishTurn` still runs, before `turn_end`, but its decision is ignored: an
                    // errored or aborted response is a hard exit (agent-loop.ts:244-254).
                    self.turn_index += 1;
                    self.finish_turn(&asst, &[]).await?;
                    self.emit(AgentEvent::TurnEnd {
                        message: AgentMessage::Assistant(Arc::clone(&asst)),
                        tool_results: Vec::new(),
                    })
                    .await?;
                    self.emit(AgentEvent::AgentEnd {
                        messages: self.new_messages.clone(),
                    })
                    .await?;
                    return Ok(());
                }

                let calls = tool_calls(&asst);
                let mut tool_results = Vec::new();
                has_more_tools = false;
                if !calls.is_empty() {
                    // A `length` stop means the output was cut off by the token limit, so every
                    // tool call in the message may carry truncated arguments. Fail them all
                    // instead of executing potentially borked calls (Pi agent-loop.ts:263-269).
                    let batch = if matches!(asst.stop_reason, StopReason::Length) {
                        self.fail_truncated_tool_calls(&calls).await?
                    } else {
                        self.execute_tool_calls(&asst, &calls).await?
                    };
                    tool_results = batch.messages;
                    // `terminate` ends only TOOL-driven continuation (the whole batch must set it,
                    // `shouldTerminateToolBatch`); queued steering / follow-up and a `finish_turn`
                    // `Continue` still flow through the post-turn path below.
                    has_more_tools = !batch.terminate;
                    for r in &tool_results {
                        // Pi pushes each tool result onto the loop's working copy
                        // (`currentContext.messages.push(result)`, agent-loop.ts:273-276).
                        let r = Arc::new(AgentMessage::ToolResult(r.clone()));
                        self.messages.push(Arc::clone(&r));
                        self.new_messages.push(r);
                    }
                }

                // `finishTurn` sees the finalized turn — its tool results already appended — and
                // decides BEFORE subscribers observe `turn_end`; the decision is applied after it
                // (agent-loop.ts:279-291). So a stop decision is taken on the context as the turn
                // left it, never on a later `prepare_next_turn` override.
                self.turn_index += 1;
                let decision = self.finish_turn(&asst, &tool_results).await?;
                self.emit(AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(Arc::clone(&asst)),
                    tool_results: tool_results.clone(),
                })
                .await?;

                if decision == Some(TurnDecision::End) {
                    // "`{ action: "end" }` ends the run without polling queues or preparing another
                    // request" (types.ts `AgentLoopConfig.finishTurn` @v0.87.1).
                    self.emit(AgentEvent::AgentEnd {
                        messages: self.new_messages.clone(),
                    })
                    .await?;
                    return Ok(());
                }

                explicit_continuation = decision == Some(TurnDecision::Continue);
                pending = self.poll_steering();
                // A natural request satisfies the continuation and adds no extra one
                // (agent-loop.ts:293-297).
                if has_more_tools || !pending.is_empty() {
                    explicit_continuation = false;
                }
                last_completed = Some(CompletedTurn {
                    message: asst,
                    tool_results,
                });
            }

            let follow = self.poll_follow_up();
            if !follow.is_empty() {
                explicit_continuation = false;
                pending = follow;
                continue;
            }
            // "No natural request was selected, so fulfill the continuation decision with one
            // context-only turn." (agent-loop.ts:309-313)
            if explicit_continuation {
                explicit_continuation = false;
                continue;
            }
            break;
        }
        self.emit(AgentEvent::AgentEnd {
            messages: self.new_messages.clone(),
        })
        .await?;
        Ok(())
    }

    /// The live context view every hook is handed (Pi `currentContext`).
    fn context_view(&self) -> AgentContextView<'_> {
        AgentContextView {
            system_prompt: &self.system_prompt,
            messages: &self.messages,
            tools: &self.tools,
        }
    }

    /// Pi `config.finishTurn?.(lastCompletedTurn, signal)`, awaited bare: a throw escapes into
    /// `runWithLifecycle`'s catch (agent.ts:489-490) and lands in `handleRunFailure` — a synthetic
    /// errored assistant message plus the FULL closing quartet, not a bare `agent_end`.
    async fn finish_turn(
        &self,
        message: &AssistantMessage,
        tool_results: &[ToolResultMessage],
    ) -> Result<Option<TurnDecision>, RunFailure> {
        let ctx = PostTurn {
            messages: &self.new_messages,
            turn_index: self.turn_index,
            message,
            tool_results,
            context: self.context_view(),
        };
        self.hooks
            .finish_turn(ctx, self.cancel.child())
            .await
            .map_err(|e| RunFailure(e.to_string()))
    }

    /// Pi `config.prepareNextTurn?.(lastCompletedTurn)` — also a bare await (agent-loop.ts:185).
    async fn prepare_next_turn(
        &self,
        turn: &CompletedTurn,
    ) -> Result<Option<crate::hooks::TurnUpdate>, RunFailure> {
        let ctx = PostTurn {
            messages: &self.new_messages,
            turn_index: self.turn_index,
            message: &turn.message,
            tool_results: &turn.tool_results,
            context: self.context_view(),
        };
        self.hooks
            .prepare_next_turn(ctx, self.cancel.child())
            .await
            .map_err(|e| RunFailure(e.to_string()))
    }

    /// Pi `config.prepareRequest?.({ context, model, thinkingLevel }, signal)` before every provider
    /// request (agent-loop.ts:218-238), awaited bare like the other loop hooks.
    async fn prepare_request(&mut self) -> Result<(), RunFailure> {
        let update = {
            let ctx = PrepareRequestCtx {
                context: self.context_view(),
                model: &self.model,
                thinking_level: self.thinking_level,
            };
            self.hooks
                .prepare_request(ctx, self.cancel.child())
                .await
                .map_err(|e| RunFailure(e.to_string()))?
        };
        if let Some(update) = update {
            self.apply_update(update);
        }
        Ok(())
    }

    /// Fold a `prepare_next_turn` / `prepare_request` override into the run baseline.
    ///
    /// Overrides are STICKY: Pi reassigns the running `config`/`currentContext` so a model /
    /// reasoning / context override returned once becomes the new baseline for EVERY later request
    /// in the run (agent-loop.ts:186-198, :228-237), not a one-shot.
    fn apply_update(&mut self, update: RequestUpdate) {
        let RequestUpdate {
            context,
            model,
            thinking_level,
            tools,
            system_prompt,
        } = update;
        if let Some(m) = model {
            self.model = m;
        }
        if let Some(t) = thinking_level {
            self.thinking_level = t;
        }
        if let Some(ctx) = context {
            // `currentContext = snapshot.context ?? currentContext`: the override replaces ONLY the
            // loop's working copy. The agent's observable `state.messages` keeps growing via the
            // reducer, so the override never leaks into `agent.state.messages` (Pi keeps the two
            // arrays distinct, agent.ts:519-522). Subsequent turns append onto the override here.
            self.messages = ctx;
        }
        // The tool array and system prompt travel inside Pi's `context` on the same return
        // (`{...previousContext, systemPrompt, tools: this.agent.state.tools.slice()}`,
        // agent-session.ts:530-534) and are just as sticky. Folding them here is what lets a tool
        // that becomes active MID-RUN be called on the very next turn — the precondition an
        // `addedToolNames` anchor asserts (DRIFT-001) and what EXT-004's late registration needs
        // to reach the model before the run ends.
        if let Some(tools) = tools {
            self.tools = tools;
        }
        if let Some(prompt) = system_prompt {
            self.system_prompt = prompt;
        }
    }
}
