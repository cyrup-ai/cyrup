//! The turn driver (Pi `runLoop`): steering/follow-up injection, the assistant turn, the tool
//! batch, and the two post-turn hooks whose overrides are folded back into the run baseline.

use super::{RunCtx, RunFailure};
use crate::agent::message::tool_calls;
use crate::event::{AgentEvent, AgentMessage};
use crate::hooks::{AgentContextView, PostTurn};
use cyrup_core::StopReason;

impl RunCtx {
    pub(super) async fn run_loop(&mut self, mut turn_started: bool) -> Result<(), RunFailure> {
        // Pi polls steering at the very top (agent-loop.ts:167), but a continue-from-assistant run
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
                if turn_started {
                    turn_started = false;
                } else {
                    self.emit(AgentEvent::TurnStart).await?;
                }
                for m in std::mem::take(&mut pending) {
                    self.emit(AgentEvent::MessageStart { message: m.clone() }).await?;
                    self.emit(AgentEvent::MessageEnd { message: m.clone() }).await?;
                    // Pi pushes each injected steering/follow-up message onto the loop's working copy
                    // (`currentContext.messages.push`, agent-loop.ts:186).
                    self.messages.push(m.clone());
                    self.new_messages.push(m);
                }

                let asst = self.stream_assistant().await?;
                // Pi's `streamAssistantResponse` leaves the final assistant message in the loop's
                // working copy (`currentContext.messages`, agent-loop.ts:346/348/361/363); mirror that
                // before tool execution / the post-turn hooks read the context.
                self.messages.push(AgentMessage::Assistant(asst.clone()));
                self.new_messages.push(AgentMessage::Assistant(asst.clone()));

                if matches!(asst.stop_reason, StopReason::Error | StopReason::Aborted) {
                    self.emit(AgentEvent::TurnEnd {
                        message: AgentMessage::Assistant(asst),
                        tool_results: Vec::new(),
                    })
                    .await?;
                    self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await?;
                    return Ok(());
                }

                let calls = tool_calls(&asst);
                let mut tool_results = Vec::new();
                has_more_tools = false;
                if !calls.is_empty() {
                    // A `length` stop means the output was cut off by the token limit, so every
                    // tool call in the message may carry truncated arguments. Fail them all
                    // instead of executing potentially borked calls (Pi agent-loop.ts:207-216).
                    let batch = if matches!(asst.stop_reason, StopReason::Length) {
                        self.fail_truncated_tool_calls(&calls).await?
                    } else {
                        self.execute_tool_calls(&asst, &calls).await?
                    };
                    tool_results = batch.messages;
                    // `terminate` ends only TOOL-driven continuation (the whole batch must set it,
                    // `shouldTerminateToolBatch`, agent-loop.ts:210,544-546); queued steering /
                    // follow-up still flow through the post-turn path below.
                    has_more_tools = !batch.terminate;
                    for r in &tool_results {
                        // Pi pushes each tool result onto the loop's working copy
                        // (`currentContext.messages.push(result)`, agent-loop.ts:213).
                        self.messages.push(AgentMessage::ToolResult(r.clone()));
                        self.new_messages.push(AgentMessage::ToolResult(r.clone()));
                    }
                }

                self.emit(AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(asst.clone()),
                    tool_results: tool_results.clone(),
                })
                .await?;
                self.turn_index += 1;

                // NOTE: there is NO early return on terminate. Pi still runs the post-turn path —
                // `prepareNextTurn`, `shouldStopAfterTurn`, then the steering poll — and continues
                // if any steering / follow-up is queued (agent-loop.ts:210,218-262). So a terminating
                // turn still fires both post-turn hooks and still drains any queued steering /
                // follow-up; absent a queue the inner loop simply exits (`has_more_tools` is false)
                // and the run ends via the normal `agent_end` below.

                // Post-turn hook context: the completed assistant message, this turn's tool results,
                // the live context (system prompt + tools + full transcript), and the new-message
                // accumulator (Pi `ShouldStopAfterTurnContext`/`PrepareNextTurnContext`,
                // types.ts:116-138).
                let ctx_messages = self.messages.clone();
                let prep = {
                    let ctx = PostTurn {
                        messages: &self.new_messages,
                        turn_index: self.turn_index,
                        message: &asst,
                        tool_results: &tool_results,
                        context: AgentContextView {
                            system_prompt: &self.system_prompt,
                            messages: &ctx_messages,
                            tools: &self.tools,
                        },
                    };
                    self.hooks.prepare_next_turn(ctx, self.cancel.child()).await
                };
                match prep {
                    Ok(Some(u)) => {
                        // Overrides are STICKY: Pi reassigns the running `config`/`currentContext`
                        // so a model / reasoning / context override returned once becomes the new
                        // baseline for EVERY later turn in the run (agent-loop.ts:226-239), not a
                        // one-shot. We fold each provided field into the run baseline here.
                        if let Some(m) = u.model {
                            self.model = m;
                        }
                        if let Some(t) = u.thinking_level {
                            self.thinking_level = t;
                        }
                        if let Some(ctx) = u.context {
                            // `currentContext = snapshot.context ?? currentContext`
                            // (agent-loop.ts:228): the override replaces ONLY the loop's working copy.
                            // The agent's observable `state.messages` keeps growing via the reducer, so
                            // the override never leaks into `agent.state.messages` (Pi keeps the two
                            // arrays distinct, agent.ts:519-522). Subsequent turns append onto the
                            // override here.
                            self.messages = ctx;
                        }
                        // The tool array and system prompt travel inside Pi's `context` on the same
                        // return (`{...previousContext, systemPrompt, tools:
                        // this.agent.state.tools.slice()}`, agent-session.ts:530-534) and are just as
                        // sticky. Folding them here is what lets a tool that becomes active MID-RUN
                        // be called on the very next turn — the precondition an `addedToolNames`
                        // anchor asserts (DRIFT-001) and what EXT-004's late registration needs to
                        // reach the model before the run ends.
                        if let Some(tools) = u.tools {
                            self.tools = tools;
                        }
                        if let Some(prompt) = u.system_prompt {
                            self.system_prompt = prompt;
                        }
                    }
                    Ok(None) => {}
                    // A THROWING `prepareNextTurn` is not caught by `runLoop` (agent-loop.ts:231 has
                    // no try/catch): the rejection escapes into `runWithLifecycle`'s catch
                    // (agent.ts:489-490) and lands in `handleRunFailure` — a synthetic errored
                    // assistant message plus the FULL closing quartet, not a bare `agent_end`.
                    Err(e) => return Err(RunFailure(e.to_string())),
                }

                // Pi passes the UPDATED `currentContext` to `shouldStopAfterTurn` (it runs AFTER the
                // `prepareNextTurn` reassignment, agent-loop.ts:241-251), so re-snapshot the (possibly
                // overridden) transcript for this hook's context view.
                let ctx_messages_after = self.messages.clone();
                let stop = {
                    let ctx = PostTurn {
                        messages: &self.new_messages,
                        turn_index: self.turn_index,
                        message: &asst,
                        tool_results: &tool_results,
                        context: AgentContextView {
                            system_prompt: &self.system_prompt,
                            messages: &ctx_messages_after,
                            tools: &self.tools,
                        },
                    };
                    self.hooks.should_stop_after_turn(ctx, self.cancel.child()).await
                };
                match stop {
                    Ok(true) => {
                        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await?;
                        return Ok(());
                    }
                    Ok(false) => {}
                    // Same as `prepareNextTurn` above: `shouldStopAfterTurn` is awaited bare
                    // (agent-loop.ts:246-252), so a throw escapes to `handleRunFailure` rather than
                    // ending the run with the ordinary `agent_end` of the `Ok(true)` arm.
                    Err(e) => return Err(RunFailure(e.to_string())),
                }

                pending = self.poll_steering();
            }

            let follow = self.poll_follow_up();
            if !follow.is_empty() {
                pending = follow;
                continue;
            }
            break;
        }
        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await?;
        Ok(())
    }
}
