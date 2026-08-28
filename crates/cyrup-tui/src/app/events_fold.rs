use super::*;

impl<B: Backend> App<B> {
    /// `rendered` is what a custom-MESSAGE / tool-row renderer produced (already collapsed to
    /// `Option<String>`, since both surfaces swallow a renderer throw upstream). `entry` is the
    /// three-state custom-ENTRY outcome, which does NOT collapse — see [`extension_render_entry`].
    ///
    /// TUI-092 F8 — the fold consumes the event **by value**. The run loop's events arm owns every
    /// event it dequeues (`run_action.rs`'s `pending.pop_front()`), and the transcript APIs the
    /// payloads land in already take them by value — `push_tool_start_rendered(…, args: Value, …)`
    /// (`transcript.rs:783`), `push_tool_update(…, partial: Option<Value>)` (`:813`),
    /// `push_tool_end_rendered(…, result: Option<Value>, …)` (`:882`) — so a borrowed fold could
    /// only ever `clone()` its way across that seam: CPU per event proportional to payload size, on
    /// the path a chatty turn hits hardest (a `ToolExecutionEnd` carrying a large `result` JSON was
    /// copied in full on every tool completion, and it compounds with event rate). The arms below
    /// MOVE `args` / `partial_result` / `result` / `steering` / `follow_up` instead. The
    /// by-reference entry points ([`Self::ingest_event`], [`Self::ingest_event_with_extensions`])
    /// survive as thin `ev.clone()` wrappers for the in-crate test call sites, which then pay a
    /// clone no production path pays.
    pub(crate) fn ingest_event_rendered_owned(
        &mut self,
        ev: AgentSessionEvent,
        rendered: crate::transcript::Rendered,
        entry_rendered: crate::transcript::Rendered,
    ) {
        // The arms that need the WHOLE event again (the serde projections in `event_extract.rs`)
        // bind nothing, so `ev` is still fully initialised inside them and `&ev` is legal there;
        // the arms that consume a payload move exactly the fields they consume. Fold ORDER is
        // untouched — this is the same match, arm for arm and statement for statement, with the
        // single documented exception in `ToolExecutionStart` below.
        match ev {
            AgentSessionEvent::AgentStart => {
                // Pi `case "agent_start"` (`interactive-mode.ts:2865-2867`): the FIRST statement of
                // the arm, before the retry-handler restore and the working indicator, is
                // `if (getShowTerminalProgress()) this.ui.terminal.setProgress(true)`. The OSC write
                // is the run loop's (`flush_terminal_progress`), as for the OSC 0 title.
                self.state.terminal_progress.set(true);
                self.state.status.set_streaming(true);
                self.state.indicator.working();
            }
            AgentSessionEvent::AgentEnd { .. } => {
                // Pi `case "agent_end"` (`interactive-mode.ts:3057-3059`), again the arm's first
                // statement: `setProgress(false)`. `agent_end` — not `agent_settled` — is where Pi
                // clears, so a turn that goes on to auto-retry or run a queued continuation drops
                // the indicator and the next `agent_start` puts it back.
                self.state.terminal_progress.set(false);
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                // Reasoning commits BEFORE the answer text so the scrollback order matches Pi's
                // content walk (thinking section, then the assistant markdown).
                self.state.transcript.commit_thinking(None);
                self.state.transcript.commit_assistant(None);
                // `if (this.streamingComponent) { … this.streamingComponent = undefined; }`
                // (`interactive-mode.ts:3271-3275`) — a turn that ended without a `message_end`
                // (an abort mid-stream) must not leave the slot open for the next turn.
                self.state.streaming_assistant = false;
                // Commit the turn's live tool executions into scrollback (`tool-execution.ts` tools
                // persist through the turn, then scroll up as committed history).
                self.state.transcript.commit_tools();
            }
            // SEAM-005 `agent_settled` (Pi interactive-mode.ts:3137): the run has fully settled —
            // no retry, post-run compaction or queued continuation will follow. Pi's interactive
            // mode does exactly ONE thing here, `await this.checkShutdownRequested()`; the visual
            // teardown already happened on `agent_end` above. That shutdown check lives in the
            // async event-loop arm (`run`, the `events.next()` branch) rather than in this sync
            // fold, which cannot `await` or return control to the caller — so this arm is a
            // deliberate no-op, NOT a missing case.
            AgentSessionEvent::AgentSettled => {}
            AgentSessionEvent::TurnStart | AgentSessionEvent::TurnEnd { .. } => {}
            // Pi `case "message_start"` (`interactive-mode.ts:3121-3143`): an `assistant` message
            // opens a fresh `AssistantMessageComponent` and files it in `this.streamingComponent`
            // (`:3130-3139`). cyrup's transcript already owns the streaming buffers, so the only
            // thing this arm has to reproduce is the LIFETIME — the bit `message_end` reads to know
            // an assistant message is open and unfinalized (`:3182`).
            AgentSessionEvent::MessageStart { .. } => {
                match message_role_from_event(&ev).as_deref() {
                    Some("assistant") => self.state.streaming_assistant = true,
                    // Pi `:2915-2918`: `event.message.role === "user"` →
                    // `this.addMessageToChat(event.message)` then
                    // `this.updatePendingMessagesDisplay()`. **This is the only place a user bubble
                    // is written** (TUI-016 / TUI-052) — the submission path deliberately does not,
                    // because the session may queue it instead of sending it, and a queued message
                    // belongs to the pending region until the turn that carries it actually starts.
                    // The `queue_update` that drains the queue arrives around this event, so the row
                    // and the bubble hand off without ever both being on screen.
                    Some("user") => {
                        if let Some(text) = user_message_text_from_event(&ev)
                            && !text.trim().is_empty()
                        {
                            self.state.transcript.push_user(text);
                        }
                    }
                    _ => {}
                }
            }
            // Pi `case "message_end"` (`interactive-mode.ts:3180-3216`). This is where an assistant
            // message is FINALIZED — `this.streamingComponent.updateContent(this.streamingMessage,
            // false)` at `:3193`, then `this.streamingComponent = undefined` at `:3213`.
            //
            // It is not optional bookkeeping: it is what makes a turn INTERLEAVE. Each finished
            // assistant text commits here, before the tool calls it requested start; each
            // `ToolExecutionComponent` is then appended after it (`:3166`/`:3240`) and the next
            // step's text after those. Committing assistant text only at `agent_end` instead —
            // which is what cyrup did while this arm was empty — concatenated every step's text
            // into one block and pushed the whole turn's tools below it, because
            // `commit_finished_leading_tools` refuses to commit a tool ahead of uncommitted
            // assistant text (`transcript.rs:865-868`) and so never fired at all.
            AgentSessionEvent::MessageEnd { .. } => {
                // A tool that reported usage for its own execution spends real tokens, so the
                // cumulative footer totals must include it (`footer.ts:99-101`). This is the
                // `toolResult` branch and, like upstream, must NOT restate the `CH` segment —
                // assistant usage goes through `add_usage` in [`Self::finalize_assistant_message`].
                if let Some(u) = tool_result_usage_from_event(&ev) {
                    self.state.status.add_usage_totals(&u);
                }
                // `if (event.message.role === "user") break;` (`:3181`) plus the
                // `this.streamingComponent &&` guard (`:3182`): only an OPEN assistant message
                // finalizes here. The open bit is cleared by whichever path finalizes first, so a
                // producer that does deliver a terminal `StreamEvent::Done` inside `message_update`
                // cannot commit the same text twice.
                if self.state.streaming_assistant
                    && let Some(message) = assistant_message_from_event(&ev)
                {
                    self.finalize_assistant_message(&message);
                }
                // The `AgentMessage` type lives in `cyrup-agent` (a dev-dep here, not a direct dep), so
                // the `Custom` arm is detected via its serde projection (`tag = "role"`,
                // `rename_all_fields = camelCase`) rather than a direct match — no dependency ripple.
                if let Some((kind, body)) = custom_message_from_event(&ev) {
                    // EXT-006: `rendered` is the text the extension that registered a renderer for
                    // this custom type produced; absent one it is `None` and the default
                    // `[kind] body` framing draws (Pi `CustomMessageComponent`).
                    // `Rendered::None` is "no renderer claimed this type" — Pi's
                    // `getMessageRenderer(...) === undefined` (`interactive-mode.ts:3326`), which
                    // draws the default box. A renderer that FAULTED also lands here, matching
                    // `custom-message.ts:82-84`'s `catch { /* Fall through to default rendering */ }`.
                    // Carried through as the host produced it: `Rendered::Live` must reach the
                    // entry intact so `entry_lines` can re-render it per frame.
                    self.state.transcript.push_custom_message_rendered(kind, body, rendered.clone());
                }
            }
            AgentSessionEvent::MessageUpdate { assistant_message_event, .. } => {
                self.ingest_stream_event(&assistant_message_event);
            }
            AgentSessionEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                // Pi's `edit` renderCall fires `computeEditsDiff` the moment the streamed arguments
                // are complete (edit.ts:377-386) so the diff is on screen while the call is still
                // pending. `ToolExecutionStart` IS that moment here: cyrup emits it with the full
                // arguments and BEFORE `prepare`, i.e. before the `before_tool_call` permission gate
                // (`cyrup-agent/src/agent.rs:1181/1334`), so the preview is up for the whole time an
                // approval prompt is waiting — and nothing has been written yet.
                //
                // TUI-092 F8 — the diff is COMPUTED here, ahead of the row, because the push below
                // MOVES `args` into the transcript. Only the read+diff moved earlier, and it touches
                // no UI state ([`edit_preview`] reads the file and diffs it); the two TRANSCRIPT
                // mutations still run in their original order — `push_tool_start_rendered` and then
                // `set_edit_preview`, which resolves the row it attaches to by `call_id`
                // (`transcript.rs:852`) and so must still follow the push that creates it.
                let preview = if tool_name == "edit" {
                    let cwd = self.state.title_cwd.clone();
                    edit_preview(&args, &cwd)
                } else {
                    None
                };
                // Hand the raw call args to the transcript so each built-in renders its Pi-specific
                // `renderCall` header (path+range / `$ command` / `/pattern/` / …), not a generic
                // one-liner (transcript.rs `tool_lines` dispatch).
                // The `toolCallId` is what the matching `ToolExecutionEnd` is paired back by — Pi
                // files the component under it (`pendingTools.set(event.toolCallId, component)`,
                // interactive-mode.ts:3096). A turn that batches two `read`s cannot be resolved by
                // tool name.
                // EXT-006: an extension that declared a renderer for THIS tool supplies the call
                // header; `None` keeps the built-in per-tool dispatch.
                // `hasRendererDefinition()` (tool-execution.ts:103-105) — resolved off the live
                // `getToolDefinition` registry one frame up, in
                // [`Self::ingest_session_event_owned`], because this fold holds no session. It is
                // what decides whether an unrendered tool draws as a bold name + ten-line preview
                // or as `formatToolExecution`'s full argument dump.
                let has_definition = self.state.known_tool_definitions.contains(&tool_name);
                self.state.transcript.push_tool_start_defined(
                    tool_name,
                    Some(tool_call_id.as_str().to_string()),
                    args,
                    // A tool ROW is a string surface: it has no live-component tier, so the
                    // outcome is flattened here rather than carried.
                    rendered.clone().into_text(),
                    has_definition,
                );
                if let Some(preview) = preview {
                    self.state.transcript.set_edit_preview(Some(tool_call_id.as_str()), preview);
                }
            }
            AgentSessionEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. } => {
                // Pi: `this.pendingTools.get(event.toolCallId)` (interactive-mode.ts:3104).
                self.state
                    .transcript
                    .push_tool_update(Some(tool_call_id.as_str()), Some(partial_result));
            }
            AgentSessionEvent::ToolExecutionEnd { tool_call_id, tool_name, is_error, result } => {
                // The full `{content, details, terminate}` result flows through so `renderResult` can
                // reach each tool's `details` (edit `diff`, bash/read truncation, …), and the
                // `toolCallId` routes it to the run that made THIS call (`:3113`).
                self.state.transcript.push_tool_end_rendered(
                    tool_name,
                    Some(tool_call_id.as_str()),
                    is_error,
                    Some(result),
                    rendered.clone().into_text(),
                );
                // Progressively flush finished tools to native scrollback mid-turn so the inline
                // viewport holds only the running tail, not the whole turn's tool stack (the
                // SCREEN-FILL disaster). The finished tool leaves `active_tools` here; the
                // draw-after-every-event (`flush_committed` → `insert_before`) lands it above the
                // viewport on the very next frame, and `terminal.draw` renders the tail without it —
                // an atomic handoff, no duplicate/flash. Mirrors Pi's completed `tool-execution.ts`
                // components scrolling up into native history as the turn proceeds.
                self.state.transcript.commit_finished_leading_tools();
            }
            // Pi `case "queue_update"` (`interactive-mode.ts:2888-2891`): rebuild the
            // pending-messages region and re-render. TUI-016 — cyrup used to keep only the COUNT
            // (`status.set_queued`) and, since the fidelity pass deleted the `{n} queued` footer
            // segment, rendered it nowhere; the texts were dropped on the floor here.
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.session_queue = (steering, follow_up);
                // TUI-031 — the region shows the UNION of the session's queues and the compaction
                // queue, as `getAllQueuedMessages` does (`interactive-mode.ts:3942-3953`).
                self.rebuild_pending_messages();
            }
            AgentSessionEvent::CompactionStart { reason } => {
                // Pi `case "compaction_start"` (`interactive-mode.ts:3076-3078`): compaction is
                // also work the user waits on, so it arms the same indicator — including a manual
                // `/compact` outside any turn, which is the one progress window with no
                // `agent_start` around it.
                self.state.terminal_progress.set(true);
                // Pi's exact status copy (status-indicator.ts:80-82): a MANUAL `/compact` reads
                // "Compacting context…"; an automatic compaction reads "Auto-compacting…", prefixed
                // "Context overflow detected, " when the overflow path triggered it (item #9). The
                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
                let msg = match reason {
                    CompactionReason::Manual => "Compacting context...".to_string(),
                    CompactionReason::Overflow => {
                        "Context overflow detected, Auto-compacting...".to_string()
                    }
                    CompactionReason::Threshold => "Auto-compacting...".to_string(),
                };
                // X18 — the indicator is a BAND, not a message. `interactive-mode.ts:3075-3087`
                // (citation re-derived at v0.83.0) `case "compaction_start"` calls
                // `showStatusIndicator(new CompactionStatusIndicator(this.ui, event.reason))` at
                // `:3084` and nothing else; `StatusIndicator` extends
                // `Loader` (`status-indicator.ts:9-27`) and is mounted in the fixed status slot, so
                // it disappears the moment `clearStatusIndicator` runs. cyrup was ALSO pushing the
                // identical string into the transcript, which `insert_before` then froze into
                // scrollback as a permanent dim `• Compacting context...` row upstream never writes.
                self.state.indicator.set(IndicatorKind::Compaction, Some(msg));
                // Pi rebinds `defaultEditor.onEscape` to `abortCompaction` here (`:3080-3086`).
                self.state.compacting = true;
            }
            AgentSessionEvent::CompactionEnd { reason, result, aborted, error_message, .. } => {
                // Pi `case "compaction_end"` (`interactive-mode.ts:3090-3092`): clears
                // unconditionally, even when this was an AUTO-compaction inside a still-streaming
                // turn. Pi's own `agent_end` then re-clears; the visible effect is a brief gap in
                // the taskbar pulse, and matching it is why `TerminalProgress::set` does not
                // deduplicate repeated transitions.
                self.state.terminal_progress.set(false);
                // Back to working if the turn is still streaming, else idle.
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                // TUI-054 — this arm used to end in an unconditional
                // `push_status("compaction complete")`, discarding every field of the event. A
                // refusal ("Nothing to compact") and an outright provider failure (`http 400`) were
                // both followed on screen by a claim that the context had been compacted, which it
                // had not been; the user then reasons about their remaining window from a false
                // premise.
                //
                // Pi branches instead (`interactive-mode.ts:3089-3123` @v0.83.0) and never states
                // success in words: `aborted` ⇒ `showError("Compaction cancelled")` for a manual
                // compaction and `showStatus("Auto-compaction cancelled")` otherwise; `result` ⇒
                // the compaction-summary MESSAGE; `errorMessage` ⇒ `showError(...)` for manual and
                // an error-styled chat line otherwise.
                //
                // **[CYRUP-DELTA]** Pi's `/compact` handler renders nothing at all
                // (`handleCompactCommand`, `:6030-6038`: `await this.session.compact()` inside a
                // `try {} catch {}` whose comment is "Ignore, will be emitted as an event"), so the
                // event is upstream's ONLY renderer. cyrup's `/compact` returns a `CompactOutcome`
                // that [`App::apply_compact_outcome`] renders on the command path — the seam that
                // moved the compaction off the run loop. Both would fire for a manual compaction,
                // so this arm renders the automatic reasons only and leaves `Manual` to the command
                // path — which now renders pi's two manual branches verbatim and through
                // `showError`, so the residual this comment used to record (a manual abort reading
                // `compact error: …` where pi reads `Compaction cancelled`) is closed.
                if !matches!(reason, CompactionReason::Manual) {
                    if aborted {
                        self.state.transcript.push_status("Auto-compaction cancelled");
                    } else if let Some(msg) = error_message {
                        self.state.transcript.push_error(msg);
                    } else if let Some(res) = result {
                        let usage = res.usage.clone();
                        self.state
                            .transcript
                            .push_compaction_summary(res.tokens_before, res.summary);
                        // pi appends the cost notice right after the summary message on the same
                        // event (`interactive-mode.ts:3431-3437`: `if (event.result.usage)
                        // this.addCompactionCostNotice({kind: "compaction", usage})`). The gate is
                        // pi's own `getShowCacheMissNotices()`, read off the cached copy because
                        // this fold holds no session.
                        if self.state.show_cache_miss_notices
                            && let Some(u) = usage.as_ref()
                        {
                            self.state.transcript.push_compaction_cost_notice(
                                crate::transcript::CompactionCostKind::Compaction,
                                u,
                            );
                        }
                    }
                }
                // TUI-031 — `void this.flushCompactionQueue({ willRetry: event.willRetry })` is the
                // LAST statement of pi's `compaction_end` arm (`interactive-mode.ts:3103`), and it
                // runs on every outcome, aborted and failed included. `ingest_event` cannot await a
                // session call, so the drained batch rides out on `AppState` for the run loop's
                // `ingest_session_event` wrapper to dispatch — see
                // [`App::take_pending_compaction_flush`].
                self.state.compaction_flush_pending = true;
                // Pi restores the previous Escape handler here (`:3094-3097`).
                self.state.compacting = false;
            }
            AgentSessionEvent::AutoRetryStart { attempt, max_attempts, delay_ms, .. } => {
                // Pi's exact retry copy (status-indicator.ts:46-47): `Retrying (a/max) in Ns...`,
                // where N starts at `Math.ceil(delayMs / 1000)` and is then re-set every second by a
                // `CountdownTimer` (`:55-64`, `countdown-timer.ts:21-30`). `set_retry` owns that
                // countdown; formatting the message here would freeze N for the whole backoff. The
                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
                // X18 — band only, exactly as `interactive-mode.ts:3339-3347` `case
                // "auto_retry_start"`: `showStatusIndicator(new RetryStatusIndicator(...))`, no
                // chat write. The mirrored `• Retrying (1/3) in 30s...` row was cyrup-only, and
                // being a snapshot of a ticking countdown it froze at whatever second it was
                // pushed.
                self.state.indicator.set_retry(attempt, max_attempts, delay_ms);
            }
            AgentSessionEvent::SummarizationRetryScheduled {
                attempt,
                max_attempts,
                delay_ms,
                error_message,
            } => {
                // Pi `interactive-mode.ts:3222-3229`: surface the transient error, then swap the
                // compaction/branch indicator for the same `RetryStatusIndicator` the turn-level
                // auto-retry uses, so a compacting session shows a countdown rather than hanging.
                // `showError(event.errorMessage)` then `showStatusIndicator(new
                // RetryStatusIndicator(...))` (`interactive-mode.ts:3367-3374`) — the error goes to
                // the chat, the countdown stays in the band (X18).
                self.state.transcript.push_error(error_message);
                self.state.indicator.set_retry(attempt, max_attempts, delay_ms);
            }
            AgentSessionEvent::SummarizationRetryAttemptStart { source } => {
                // Pi `interactive-mode.ts:3231-3240`: clear the retry indicator and RECREATE the
                // underlying one from `source` — that is the only reason the event carries it.
                let (kind, msg) = match source {
                    SummarizationRetrySource::BranchSummary => {
                        (IndicatorKind::BranchSummary, "Summarizing branch...".to_string())
                    }
                    SummarizationRetrySource::Compaction { reason } => (
                        IndicatorKind::Compaction,
                        match reason {
                            CompactionReason::Manual => "Compacting context...".to_string(),
                            CompactionReason::Overflow => {
                                "Context overflow detected, Auto-compacting...".to_string()
                            }
                            CompactionReason::Threshold => "Auto-compacting...".to_string(),
                        },
                    ),
                };
                self.state.indicator.set(kind, Some(msg));
            }
            AgentSessionEvent::SummarizationRetryFinished => {
                // Pi `interactive-mode.ts:3242-3245`: `clearStatusIndicator("retry")` only — it is
                // a no-op unless the retry indicator is the live one, which it is exactly when the
                // loop ended DURING a backoff (exhausted / aborted). A loop that ended on a
                // successful retried call already restored its own indicator in the arm above.
                if self.state.indicator.kind() == IndicatorKind::Retry {
                    if self.state.status.streaming {
                        self.state.indicator.working();
                    } else {
                        self.state.indicator.idle();
                    }
                }
            }
            // Pi renders bash output from the execution callback, not from this event
            // (`interactive-mode.ts:3075-3077`: "The bash execution callback handles TUI output
            // rendering."). Kept as an explicit no-op so the parity is visible.
            AgentSessionEvent::BashExecutionUpdate { .. } => {}
            AgentSessionEvent::AutoRetryEnd { success, .. } => {
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                self.state
                    .transcript
                    .push_status(if success { "retry succeeded" } else { "retry ended" });
            }
            AgentSessionEvent::ModelChanged { provider, model } => {
                let label = format!("{provider}/{model}");
                self.state.status.set_model(label.clone());
                // Feed the provider into the footer right cluster (`(provider)` prefix, footer.ts:191).
                self.state.status.set_provider(Some(provider));
                // …and re-answer `usingSubscription` for the NEW provider (`footer.ts:139-141`).
                // pi gets this for free — `model_changed` calls `footer.invalidate()`
                // (`interactive-mode.ts:3070`) and the flag is recomputed inside `render()`. cyrup
                // must push it, or a `/model` switch from a subscription provider to a metered one
                // would keep printing ` (sub)` (and vice versa).
                self.refresh_subscription_marker();
                self.state.transcript.push_status(format!("model → {label}"));
            }
            AgentSessionEvent::ThinkingLevelChanged { level } => {
                // Pi's `thinking_level_changed` handler (interactive-mode.ts:2804-2807) only
                // `footer.invalidate()` + `updateEditorBorderColor()` — NO status line (the acting
                // command, e.g. Shift+Tab's `C::CycleThinking`, owns the status). Mirror the level into
                // the footer right cluster (`• {level}`, footer.ts:186-188), the editor's rule color
                // (spec/tui/03 §3.3), and the TUI's cached level so `/debug` reflects the authoritative
                // session state.
                self.state.thinking_level = level.clone();
                self.state.status.set_thinking_level(level.clone());
                self.state.editor.set_thinking_level(level);
            }
            AgentSessionEvent::SessionInfoChanged { name } => {
                // Pi `interactive-mode.ts:2784` mirrors the renamed session into the header/status.
                let label = name.clone().unwrap_or_default();
                self.state.transcript.push_status(format!("session renamed → {label}"));
                // Pi's `session_info_changed` arm (`interactive-mode.ts:2900-2903`) is
                // `updateTerminalTitle()` + `footer.invalidate()`: the new name reaches BOTH the
                // footer's location line (` • {name}`, footer.ts:116-130) and the window title. The
                // recomputed title is written by the crossterm run loop (see
                // [`Self::update_terminal_title`]).
                self.state.status.set_session_name(name);
                let _ = self.update_terminal_title();
            }
            AgentSessionEvent::EntryAppended { entry } => {
                // A loaded extension appended a custom (non-LLM) entry to the tree (Pi
                // `entry_appended`, agent-session.ts:140 → `addCustomEntryToChat(event.entry)`,
                // interactive-mode.ts:3105/3431-3450).
                let ty = custom_entry_type(&entry);
                // X15 — `addCustomEntryToChat` is entirely a renderer question:
                //
                // ```ts
                // const renderer = this.session.extensionRunner.getEntryRenderer(entry.customType);
                // if (!renderer) { return; }                      // :3433-3435 — draws NOTHING
                // const component = new CustomEntryComponent(entry, renderer);
                // component.setExpanded(this.toolOutputExpanded);
                // if (!component.hasContent()) { return; }        // :3438-3440 — also nothing
                // ```
                //
                // …and `CustomEntryComponent` is where a THROW becomes the failure box
                // (`custom-entry.ts:47-52`) rather than being dropped. `entry_rendered` carries
                // that three-state answer here.
                if entry_rendered.has_content() {
                    self.state.transcript.push_custom_message_rendered(
                        ty,
                        String::new(),
                        entry_rendered,
                    );
                } else {
                    // CYRUP-DELTA: with no renderer claiming the type upstream shows nothing at
                    // all, which leaves a `/statedemo`-style entry invisible. cyrup keeps its
                    // pre-existing one-line receipt for that case only — strictly additive over
                    // "nothing", and it never competes with a renderer, because a renderer that
                    // produced output (or faulted) took the branch above.
                    self.state.transcript.push_status(format!("entry appended → {ty}"));
                }
            }
            // pi routes `session_start`/`session_shutdown` to EXTENSIONS ONLY — declared
            // `extensions/types.ts:563`/`:632`, subscribed via `on("session_start", …)` at
            // `:1221`/`:1234`, emitted solely to `extensionRunner`
            // (`agent-session-runtime.ts:172`, `:400`; `agent-session.ts:2706`, `:2725`;
            // `extensions/runner.ts:189-196`). The interactive UI never receives them, so it
            // renders nothing — no test asserts these strings (verified). cyrup used to push a
            // status line for each; the `session shutdown (new)` banner a frozen `/new` left on
            // screen was that invention. Kept as documented no-op arms for match exhaustiveness.
            AgentSessionEvent::SessionStart { .. } | AgentSessionEvent::SessionShutdown { .. } => {}
            AgentSessionEvent::SessionReplaced { .. } => {
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
            }
        }
    }

    /// Fold one streaming `StreamEvent` (the `assistantMessageEvent` payload of a `MessageUpdate`,
    /// session-svc `event.rs:111`) into the transcript — the live token-by-token render (gap 1).
    ///
    /// `TextDelta { delta, .. }` (provider `stream.rs:306`) is appended to the in-flight streaming
    /// buffer via [`TranscriptView::push_assistant_delta`], so the viewport grows a character at a
    /// time exactly like Pi's interactive stream. A terminal event (`Done`/`Error`, recoverable via
    /// [`StreamEvent::terminal_message`]) replaces the partial with the authoritative
    /// `AssistantMessage` text and records its token usage in the footer.
    ///
    /// `ThinkingDelta { delta, .. }` (provider `stream.rs:413`) grows the separate live *reasoning*
    /// block via [`TranscriptView::push_thinking_delta`]; the terminal event commits the message's
    /// authoritative `thinking` blocks ([`thinking_text`]) ahead of the answer text, matching Pi's
    /// in-order content walk (`assistant-message.ts:115-166`). The remaining non-text frames
    /// (start/text-start/text-end/thinking-start/thinking-end/toolcall*) carry only the running
    /// `partial`, whose content already reaches us via the deltas + the terminal, so nothing is
    /// rendered for them.
    ///
    /// A terminal whose `stop_reason` is not a clean stop also appends Pi's error-styled
    /// incomplete/failed-turn notice ([`stop_reason_notice`], `assistant-message.ts:175-201`).
    fn ingest_stream_event(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_assistant_delta(delta);
                }
            }
            // Reasoning deltas (provider `stream.rs:413`) grow their own live block above the
            // answer text, exactly as Pi renders the turn's `thinking` content
            // (`assistant-message.ts:115-166`). `ThinkingStart`/`ThinkingEnd` carry no incremental
            // text of their own — the authoritative blocks arrive with the terminal message below.
            StreamEvent::ThinkingDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_thinking_delta(delta);
                }
            }
            // DEFENSIVE, not the live path. `cyrup-agent` `break 'consume`s the moment the stream
            // yields its terminal (`agent.rs:813-820`), so a terminal event is never re-emitted as
            // a `MessageUpdate` and this arm does not fire for a real turn — `MessageEnd` is where
            // an assistant message finalizes. It stays for any producer (an embedder, a replayed
            // transport) that does forward the terminal, and clears the open bit so `MessageEnd`
            // will not then commit the same text a second time. The `streaming_assistant` guard is
            // Pi's `if (this.streamingComponent && ...)` on `message_update`
            // (`interactive-mode.ts:3146`). That guard makes this a PARTIAL arm: when
            // `streaming_assistant` is false a matched `Done`/`Error` falls through to the `_ => {}`
            // below, which is intended — with no open assistant message there is nothing to
            // finalize — not an oversight.
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. }
                if self.state.streaming_assistant =>
            {
                self.finalize_assistant_message(message);
            }
            _ => {}
        }
    }

}
