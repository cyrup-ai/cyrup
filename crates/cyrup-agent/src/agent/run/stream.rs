//! The LLM boundary (arch-02 §6.2): build the request, stream it, and always emit the assistant
//! `message_start`..`message_end` pair — including on abort — so the caller's closing sequence is
//! complete.

use std::sync::Arc;
use super::{RunCtx, RunFailure};
use crate::agent::message::{empty_assistant, errored_assistant};
use crate::event::{AgentEvent, AgentMessage};
use cyrup_core::{AssistantMessage, StopReason};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
use futures::StreamExt;

impl RunCtx {
    /// The LLM boundary (arch-02 §6.2). Always emits the assistant `message_start..message_end`
    /// (including on abort) so the caller's closing sequence is complete.
    ///
    /// AGENT-025 — a `transform_context` / `convert_to_llm` failure returns [`RunFailure`] instead
    /// of synthesizing its own errored assistant message. Pi awaits both hooks BARE
    /// (`packages/agent/src/agent-loop.ts:288-295`, identical offsets at both tags), so a rejection
    /// unwinds `streamAssistantResponse` → `runLoop` (`:193`) → `runAgentLoop` (`:116`) →
    /// `runWithLifecycle`'s catch (`agent.ts:489-490` @v0.83.0) → `handleRunFailure(error,
    /// signal.aborted)`, which picks `stopReason: aborted ? "aborted" : "error"` (`:504`) and emits
    /// `{ type: "agent_end", messages: [failureMessage] }` (`:511`) — the single synthetic message
    /// and nothing else. The throw at `agent-loop.ts:193` also means `newMessages.push(message)` at
    /// `:194` never runs, so upstream's accumulator never receives the failure either. cyrup used to
    /// hardcode `StopReason::Error` here and let `run_loop` fall through to the ordinary
    /// `Error|Aborted` branch, which emitted `agent_end` carrying the WHOLE run accumulator and
    /// never reported `aborted`.
    pub(super) async fn stream_assistant(&mut self) -> Result<AssistantMessage, RunFailure> {
        // The running baseline. `prepare_next_turn` overrides are STICKY: a returned
        // model/reasoning/context override is folded into the run's baseline (`self.model`,
        // `self.thinking_level`, and the live `state.messages`) in `run_loop`, so it persists for
        // ALL later turns in the run (Pi `config = {...config, model, reasoning}` /
        // `currentContext = snapshot.context ?? currentContext`, agent-loop.ts:226-239). A
        // non-reasoning model silently ignores the level (func-01 R-01-041).
        let model = self.model.clone();
        let effective_thinking = self.thinking_level;
        // Read the loop's OWN working copy (Pi `context.messages`, agent-loop.ts:283), NOT the live
        // `state.messages` Arc — a `prepare_next_turn` context override or a mid-run external
        // `set_messages` must not cross between the two.
        let base_messages = self.messages.clone();

        let transformed =
            match self.hooks.transform_context(base_messages, self.cancel.child()).await {
                Ok(m) => m,
                // Pi awaits `transformContext` bare (agent-loop.ts:288-292), so a throw unwinds to
                // `handleRunFailure`, whose `errorMessage` is the thrown value's own text
                // (`error instanceof Error ? error.message : String(error)`, agent.ts:504). Surface
                // `e.to_string()` — never a fixed label — or the hook's reason is lost outright.
                Err(e) => return Err(RunFailure(e.to_string())),
            };
        let llm = match self.hooks.convert_to_llm(&transformed).await {
            Ok(m) => m,
            // Same bare await for `convertToLlm` (agent-loop.ts:295) → same `handleRunFailure` text.
            Err(e) => return Err(RunFailure(e.to_string())),
        };

        // Dynamic key wins; fall back to the run's static key (Pi `... || config.apiKey`,
        // agent-loop.ts:301-302).
        // AGENT-032(b) — pi is `(config.getApiKey ? await config.getApiKey(...) : undefined) ||
        // config.apiKey` (`packages/agent/src/agent-loop.ts:306`, identical at both tags). `||` is
        // JS-FALSY, so a resolver that returns an EMPTY string falls through to the static key;
        // an `Option`-only fallback sent the empty key and the request 401'd.
        let api_key = match &self.key_resolver {
            Some(r) => r.get_api_key(&model.provider).await,
            None => None,
        }
        .filter(|k| !k.is_empty())
        .or_else(|| self.gen_config.api_key.clone());

        // Forward each tool's `description` to the model (Pi `Context.tools`, agent-loop.ts:289-296;
        // spec §4.3) — an empty description left the model unable to use the tool.
        let tool_defs: Vec<cyrup_provider::ToolDef> = self
            .tools
            .iter()
            .map(|t| cyrup_provider::ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters().clone(),
                // PROV-011: pi's `constrainedSampling` is a per-tool OPT-IN declared on the tool
                // definition (`extensions/types.ts:463` @v0.83.0); `undefined` and `false` behave
                // identically (`packages/ai/README.md:483`).
                //
                // Upstream the declaration is COPIED off the `ToolDefinition` onto the runtime
                // `AgentTool` by `wrapToolDefinition`
                // (`packages/coding-agent/src/core/tools/tool-definition-wrapper.ts:14` @v0.83.0),
                // and the loop then hands those same `AgentTool`s to the stream verbatim —
                // `tools: context.tools` (`packages/agent/src/agent-loop.ts:301` @v0.83.0) — so
                // whatever the tool declared is what `convertTools` sees. This `.map` IS that
                // hand-off, so it must read the declaration rather than erase it: hardcoding `None`
                // here made `cyrup_provider::utils::constrained_sampling` unreachable from the
                // agent loop, so no tool — extension-registered or WASM guest — could ever opt in,
                // and a `strict: "require"` declaration that upstream FAILS the request silently
                // degraded to an ordinary unconstrained tool call.
                constrained_sampling: t.constrained_sampling().cloned(),
            })
            .collect();

        // Forward the generation params + telemetry + reasoning level (Pi `AgentLoopConfig` →
        // `streamSimple`, agent-loop.ts:298-308 / agent.ts:421-447).
        let opts = StreamOptions {
            cancel: Some(self.cancel.child()),
            api_key,
            session_id: self.session_id.clone(),
            reasoning: effective_thinking,
            temperature: self.gen_config.temperature,
            max_tokens: self.gen_config.max_tokens,
            cache_retention: self.gen_config.cache_retention,
            // LIVE, not `gen_config`: pi rebuilds these inside `streamFn` for the model the request
            // is actually going to (`sdk.ts:318-327`), so a cross-provider `/model` switch must not
            // keep sending the previous provider's attribution headers. AGENT-029: resolve them
            // from `model` — the loop's OWN per-turn model, which a sticky `TurnUpdate::model`
            // override may have retargeted since run start — rather than from the latched
            // `state.headers` snapshot, which only the two session-level model-change paths write.
            headers: self.headers_for(&model),
            transport: self.gen_config.transport,
            max_retry_delay_ms: self.gen_config.max_retry_delay_ms,
            max_retries: self.gen_config.max_retries,
            thinking_budgets: self.gen_config.thinking_budgets,
            on_payload: self.gen_config.on_payload.clone(),
            on_response: self.gen_config.on_response.clone(),
            // Provider-scoped env overlay (e.g. the `httpProxy` setting) + request idle timeout (Pi
            // `applyHttpProxySettings`/`configureHttpDispatcher`, main.ts:744-745).
            env: self.gen_config.env.clone(),
            timeout_ms: self.gen_config.timeout_ms,
            // AGENT-S03 / AGENT-031 — pi's `AgentLoopConfig extends SimpleStreamOptions`
            // (`packages/agent/src/types.ts:271`) and `agent-loop.ts:308-312` spreads the WHOLE
            // config into `streamFunction`, so every `SimpleStreamOptions` field a caller sets is on
            // the wire by construction. Populating these explicitly (rather than leaving them to
            // `..Default::default()`) is what gives them a path out of the agent at all.
            metadata: self.gen_config.metadata.clone(),
            websocket_connect_timeout_ms: self.gen_config.websocket_connect_timeout_ms,
            ..Default::default()
        };
        let ctx = Context {
            system_prompt: Some(self.system_prompt.clone()),
            messages: llm,
            tools: tool_defs,
        };

        let mut stream = self.stream_fn.stream(&model, &ctx, &opts);
        let cancel_tok = self.cancel.token();
        let mut started = false;
        // The structured partial assistant message, kept in lockstep with the provider's per-event
        // `partial` snapshot (Pi `event.partial`, agent-loop.ts:313-340): distinct text / thinking /
        // toolCall content blocks (with signatures) and streaming tool-call args — NOT a single
        // collapsed text block. The provider exposes this via `StreamEvent::partial()` (stream.rs).
        // The running partial is held as a SHARED handle: refreshing it from each event, and
        // re-emitting it on `message_update`, were three deep copies of the whole message per
        // delta (PERF-001).
        let mut partial = Arc::new(empty_assistant(&model));
        let mut final_msg: Option<AssistantMessage> = None;

        'consume: loop {
            tokio::select! {
                biased;
                _ = cancel_tok.cancelled() => {
                    if !started {
                        self.emit(AgentEvent::MessageStart {
                            message: AgentMessage::Assistant(partial.clone()),
                        })
                        .await?;
                    }
                    // Pi returns the stream's own `result()` terminal on abort (agent-loop.ts:344),
                    // which carries the ACCUMULATED partial content with `stopReason:"aborted"` — NOT
                    // a fresh empty message. Reuse the structured partial we have been tracking and
                    // only stamp the terminal reason, so a subscriber/transcript sees the streamed
                    // text/thinking/tool-call blocks rather than `[]`. The terminal's `errorMessage`
                    // is Pi's uniform abort string `"Request was aborted"` — every provider throws
                    // `new Error("Request was aborted")` on `signal.aborted` and the catch sets
                    // `output.errorMessage = error.message` (anthropic-messages.ts:718,733-734; the
                    // faux provider's `createAbortedMessage` uses the same string, faux.ts:291-297) —
                    // NOT the bare `"aborted"`.
                    let mut aborted = (*partial).clone();
                    aborted.stop_reason = StopReason::Aborted;
                    aborted.error_message = Some("Request was aborted".to_string());
                    self.emit(AgentEvent::MessageEnd {
                        message: AgentMessage::Assistant(Arc::new(aborted.clone())),
                    })
                    .await?;
                    return Ok(aborted);
                }
                ev = stream.next() => {
                    let e = match ev {
                        None => break,
                        Some(e) => e,
                    };
                    // Refresh the structured partial from the event's own snapshot for every
                    // non-terminal event (Pi assigns `partialMessage = event.partial`).
                    if let Some(p) = e.partial() {
                        partial = p.clone();
                    }
                    match &e {
                        StreamEvent::Start { .. } => {
                            started = true;
                            self.emit(AgentEvent::MessageStart {
                                message: AgentMessage::Assistant(partial.clone()),
                            })
                            .await?;
                        }
                        // Pi RETURNS from `streamAssistantResponse` immediately on the `done`/`error`
                        // terminal (agent-loop.ts:342-355): it stops consuming the stream right here.
                        // Break out of the consume loop so a (non-conforming) post-terminal event can
                        // neither emit a stray `message_update` nor overwrite the final `partial`.
                        StreamEvent::Done { message, .. } => {
                            final_msg = Some((**message).clone());
                            break 'consume;
                        }
                        StreamEvent::Error { error, .. } => {
                            final_msg = Some((**error).clone());
                            break 'consume;
                        }
                        // Every other event is a content-block start/delta/end (text, thinking, OR
                        // tool-call): re-emit the refreshed partial on `message_update` (Pi emits
                        // `message_update` for all nine block events once the partial exists,
                        // agent-loop.ts:319-340).
                        _ => {
                            if started {
                                self.emit(AgentEvent::MessageUpdate {
                                    message: AgentMessage::Assistant(partial.clone()),
                                    assistant_message_event: Box::new(e.clone()),
                                })
                                .await?;
                            }
                        }
                    }
                }
            }
        }

        let final_msg = final_msg.unwrap_or_else(|| {
            errored_assistant(
                model.provider.clone(),
                model.model.as_str(),
                model.api.clone(),
                StopReason::Error,
                "stream ended without a terminal event",
            )
        });
        if !started {
            self.emit(AgentEvent::MessageStart {
                message: AgentMessage::Assistant(Arc::new(final_msg.clone())),
            })
            .await?;
        }
        self.emit(AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(Arc::new(final_msg.clone())),
        })
            .await?;
        Ok(final_msg)
    }
}
