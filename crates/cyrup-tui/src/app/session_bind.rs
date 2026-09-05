use super::*;

impl<B: Backend> App<B> {
    pub fn rebind_session(&mut self) {
        // Extension-owned surfaces first (pi `resetExtensionUI`, `interactive-mode.ts:1974-2003`).
        //
        // pi registers this on the runtime's `beforeSessionInvalidate` so it runs while the OLD
        // session is still alive. cyrup calls it here instead, and the difference is safe for a
        // specific reason: pi's hook is positioned early because a JS closure over `this` can also
        // reach into `oldSession.extensionRunner` (its own ordering test asserts exactly that),
        // whereas this function touches NOTHING but local UI state. There is no old-host resource
        // to race. The Rust hook cannot capture `&mut App` in an `Arc<dyn Fn()>` anyway — see
        // `AgentSessionRuntime::set_before_session_invalidate`, which exists as a library surface
        // for embedders that need the earlier position.
        self.reset_extension_ui();
        self.state.transcript = TranscriptView::new();
        self.state.selector = None;
        self.state.overlays.clear();
        self.state.status.set_streaming(false);
        // The queue belongs to the OUTGOING session: its steering/follow-up lists were emitted by a
        // `queue_update` from a session that is gone, and its compaction queue would be delivered
        // into the new one. Clearing them clears the rendered region, which is the whole point —
        // this used to be `status.set_queued(0)`, which zeroed a counter with no render site and
        // left `pending_messages` drawing the previous session's `Steering: …` rows above the
        // editor for the rest of the process (TUI-016 / ADR-0009 item 3).
        self.state.session_queue = (Vec::new(), Vec::new());
        self.state.compaction_queue.clear();
        self.rebuild_pending_messages();
        self.state.indicator.idle();
        // The new session starts idle, so drop the prior turn's grow-only height floor; the next
        // `draw` collapses the viewport to the compact idle region (void-fix).
        self.live_floor = 0;
        // Tone is per-command (`/new` alone gets pi's accent `handleClearCommand` receipt;
        // `/resume`/`/fork`/`/reload`/`/import` and a runtime-side swap with no caption keep
        // today's dim `showStatus` styling).
        match self.state.pending_swap_status.take() {
            Some(SwapCaption::Receipt(text)) => self.state.transcript.push_receipt(text),
            Some(SwapCaption::Status(text)) => self.state.transcript.push_status(text),
            None => self.state.transcript.push_status("session replaced"),
        }
    }

    /// Seed the transcript from a session's persisted conversation — Pi's `renderInitialMessages()`
    /// → `renderSessionEntries(buildContextEntries(), {updateFooter, populateHistory})`
    /// (interactive-mode.ts:3548-3562) and the `rebuildChatFromMessages()` used after a compaction
    /// or a tree/fork navigation (`:3599-3601`, `:1737-1742`).
    ///
    /// Without this a `/resume`, `/fork`, `/import`, `--resume` or `--continue` shows an EMPTY view
    /// even though the session file holds the whole conversation, because
    /// [`rebind_session`](Self::rebind_session) starts the new session from a fresh
    /// [`TranscriptView`].
    ///
    /// **Feed it [`AgentSession::raw_context_messages`], never `AgentSession::messages()`.** The
    /// latter is the LLM boundary (`convertToLlm`, `messages.ts:148-195`): it has already rendered a
    /// compaction summary, a branch summary, an extension `custom` message and a `!` bash execution
    /// down to `user` messages carrying wrapper prose ("The conversation history before this point
    /// was compacted into the following summary: …"), which would replay as the *user* having typed
    /// that text — and would seed it into the editor's Up-arrow history. Pi feeds the RAW projection
    /// for exactly this reason: `renderSessionEntries` maps entries through
    /// `sessionEntryToContextMessages` (interactive-mode.ts:3506-3516) whose roles are still
    /// `compactionSummary`/`branchSummary`/`custom`/`bashExecution`, and `addMessageToChat`
    /// (`:3308-3350`) routes each to its own component.
    ///
    /// The port follows Pi's `renderSessionItems` walk (`:3415-3497`) + `addMessageToChat`
    /// (`:3308-3413`):
    /// * `user` → the user block (a `<skill …>` submission still splits into its `[skill]`
    ///   invocation + the trailing message, via [`TranscriptView::push_user`]) and, like Pi's
    ///   `populateHistory`, the prompt is pushed into the editor's Up-arrow history;
    /// * `assistant` → the reasoning section, the answer markdown, a live tool block per `toolCall`
    ///   content, and the not-finished-cleanly notice ([`stop_reason_notice`]);
    /// * `toolResult` → attached to the matching open tool block by tool name, then the finished
    ///   leading run is committed so tools land between the assistant turns that bracket them
    ///   rather than all at the end;
    /// * `bashExecution` → a committed bash block (`BashExecutionComponent`, `:3310-3322`),
    ///   dim-bordered for a `!!` (`excludeFromContext`) run;
    /// * `custom` → the labeled extension block, **only when `display`** (`:3323-3336`);
    /// * `compactionSummary` / `branchSummary` → their own summary blocks (`:3337-3350`).
    ///
    /// **Divergence from pi — UNPORTED (the `ADR-0001` it once cited does not exist; see CLAUDE.md)**: Pi calls `chatContainer.clear()` before replaying, which
    /// wipes the previous session off the screen. cyrup's committed entries live in the terminal's
    /// native scrollback (`insert_before`) and cannot be erased, so after a mid-session `/resume`
    /// the previous conversation stays visible ABOVE the replayed one. The replay itself needs no
    /// re-render: it starts from an empty transcript and flushes forward normally.
    ///
    /// X11 — this is the NO-EXTENSIONS shorthand. Pi resolves an extension's registered message
    /// renderer on the replay walk too (`const renderer = this.session.extensionRunner
    /// .getMessageRenderer(message.customType)`, `interactive-mode.ts:3471`, inside the same
    /// `case "custom"` the `display` gate at `:3470` guards), exactly as it does on the live
    /// `addMessageToChat` path. Call [`Self::replay_items_with_extensions`] wherever a host is in
    /// hand — every production `/resume`, `/fork`, `/import` and `--continue` does — or a resumed
    /// session silently loses extension rendering that the live session had.
    ///
    /// It is also the NO-NOTICES shorthand: a bare message list carries neither of the derived
    /// notices pi re-injects on a rebuild, so a caller that wants them asks the session for the
    /// full stream ([`cyrup_session_svc::AgentSession::replay_items`]) instead.
    pub fn replay_session(&mut self, messages: &[cyrup_session_svc::agent_message::AgentMessage]) {
        self.replay_session_rendered(&as_replay_items(messages), &ReplayRenders::default());
    }

    /// TUI-N04 — the second statement of Pi's `renderInitialMessages()`, immediately after the
    /// replay (`interactive-mode.ts:3485`), body at `:3496-3514` @v0.83.0:
    ///
    /// ```ts
    /// private renderProjectTrustWarningIfNeeded(): void {
    ///     if (this.settingsManager.isProjectTrusted() || !hasTrustRequiringProjectResources(this.sessionManager.getCwd())) {
    ///         return;
    ///     }
    ///     if (this.chatContainer.children.length > 0) this.chatContainer.addChild(new Spacer(1));
    ///     this.chatContainer.addChild(new Text(theme.fg("warning",
    ///         `This project is not trusted. Project ${CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.`), 1, 0));
    /// }
    /// ```
    ///
    /// Both halves of the predicate already existed in cyrup and neither had a reader on this path:
    /// `AgentSessionServices::project_trusted` (`services.rs:104`, the same field the `/trust`
    /// dialog reads at [`Self::open_selector`]) and
    /// [`cyrup_config::trust::has_trust_requiring_resources`] (`trust.rs:201`, the same scan
    /// `AgentSessionBuilder` runs at `builder.rs:597` to decide whether trust is even in question).
    ///
    /// **The string is rebranded, not reworded**: `.cyrup` for pi's `CONFIG_DIR_NAME` (the directory
    /// `has_trust_requiring_resources` actually probes, `trust.rs:211`) and `cyrup` for `pi`.
    ///
    /// **[CYRUP-DELTA]** — pi gates its leading `Spacer(1)` on `chatContainer.children.length > 0`
    /// (`:3502`), so on a *completely* empty transcript the warning is the first row with no blank
    /// above it. `Entry::Warning` emits its leading blank unconditionally
    /// (`transcript.rs`'s `Entry::Warning` arm, matching `showWarning`), so cyrup shows one extra
    /// blank line in that one case. Reproducing the gate would mean a second warning entry kind
    /// whose only difference is a blank line; recorded rather than taken.
    pub fn render_project_trust_warning_if_needed(&mut self, session: &Arc<AgentSession>) {
        let services = session.services();
        if services.project_trusted
            || !cyrup_config::trust::has_trust_requiring_resources(&services.cwd, &services.home)
        {
            return;
        }
        // No `Warning: ` prefix: pi's trust banner is a RAW `Text` in the warning colour (`:3505`),
        // not a `showWarning` call, so — unlike `interactive-mode.ts:3884-3888`'s
        // `Warning: ${warningMessage}` — there is no prefix to carry (TUI-062).
        self.state
            .transcript
            .push_warning(PROJECT_UNTRUSTED_WARNING);
    }

    /// Reload [`AppState::known_tool_definitions`] from the bound session's tool registry — Pi's
    /// `definitionRegistry`, rebuilt over the builtins plus every registered and custom tool
    /// (`agent-session.ts:2659-2676`) and read per component as `getToolDefinition(name)` (`:806`,
    /// handed to the render context at `:3413`).
    ///
    /// Called wherever a session is bound or swapped in, BEFORE the replay walk: a `/resume`d
    /// conversation's tool calls never pass through [`Self::ingest_session_event_owned`], so the
    /// per-tool-start lookup there cannot answer for them and they would all fall back to
    /// `formatToolExecution`'s full argument dump.
    pub fn refresh_known_tool_definitions(&mut self, session: &Arc<AgentSession>) {
        // The definition's `renderShell` rides along (EXT-024): `getRenderShell()` reads it off the
        // same `getToolDefinition(name)` answer (`tool-execution.ts:108-116`).
        self.state.known_tool_definitions = session
            .all_tools()
            .into_iter()
            .map(|t| (t.name, t.render_kind))
            .collect();
    }

    /// [`Self::replay_session`], first resolving each displayed `custom` message's registered
    /// extension renderer (EXT-006; Pi `getMessageRenderer(message.customType)`,
    /// `interactive-mode.ts:3471`).
    ///
    /// The renderer lookup is an async guest call with a timeout while the replay walk is sync, so
    /// — exactly like [`Self::ingest_event_with_extensions`] on the live path — every renderer runs
    /// FIRST and its text rides into the walk, keyed by the message's index.
    pub async fn replay_session_with_extensions(
        &mut self,
        messages: &[cyrup_session_svc::agent_message::AgentMessage],
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        self.replay_items_with_extensions(&as_replay_items(messages), ext_host)
            .await;
    }

    /// [`Self::replay_session_with_extensions`] over the FULL replay stream — the messages plus the
    /// derived cache-miss and compaction-cost notices [`AgentSession::replay_items`] re-derives
    /// (pi `renderSessionEntries`, `interactive-mode.ts:3781-3796`, whose items are exactly this
    /// union).
    ///
    /// Every production replay — `--resume`/`--continue` at boot and `/resume`, `/fork`, `/import`
    /// on a swap — calls THIS; the two `AgentMessage`-shaped entry points above are the shorthands
    /// for a caller that has messages and nothing to re-derive from.
    pub async fn replay_items_with_extensions(
        &mut self,
        items: &[cyrup_session_svc::ReplayItem],
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        use cyrup_core::{Content, Message};
        use cyrup_session_svc::agent_message::AgentMessage;
        use serde_json::Value;
        let mut rendered = ReplayRenders::default();
        // EXT-006 — the display inputs every renderer in this walk is invoked under, read ONCE
        // here from the live view rather than defaulted, so a replay lands in the same expansion
        // and theme the live turn would have. They are recorded on each render, so the first
        // toggle after the replay re-invokes exactly as it does for a live row.
        let opts = self.render_options();
        // MESSAGE-relative, matching the walk below: `rendered.messages` is keyed by the index a
        // message has among messages, not among items, so an interleaved notice cannot shift a
        // custom message's renderer onto its neighbour.
        for (i, message) in replay_messages(items).enumerate() {
            match message {
                AgentMessage::Custom(c) => {
                    // `if (message.display)` (`:3470`) gates the whole arm, lookup included.
                    if !c.display {
                        continue;
                    }
                    let payload = serde_json::to_value(message).unwrap_or(Value::Null);
                    // Carried WHOLE. `has_content()` is the "did a renderer claim this" question
                    // the old `if let Some(text)` was asking, and it stays true for a LIVE
                    // component, which `Rendered::into_text()` would have dropped.
                    let r =
                        extension_render_message(ext_host, &c.custom_type, &payload, &opts).await;
                    if r.has_content() {
                        rendered.messages.insert(i, r);
                    }
                }
                // EXT-041 — the TOOL surface. Pi's replay builds a `ToolExecutionComponent` for
                // every replayed `toolCall` with `this.getRegisteredToolDefinition(content.name)`
                // (`interactive-mode.ts:3729-3741` @v0.84.4) — the definition that carries the
                // extension's `renderCall`/`renderResult` — exactly as the live
                // `tool_execution_start` arm does (`:3340-3351`). The custom-message arm above is
                // this arm's sibling, not a superset: a `/resume` used to draw every
                // extension-rendered tool row with the built-in framing the live turn had replaced.
                // Keyed by TOOL-CALL id, which is how pi files the component
                // (`renderedPendingTools.set(content.id, component)`, `:3760`) and how the result
                // finds it back (`:3770`) — two `bash` calls in one turn are indistinguishable by
                // name.
                AgentMessage::Core(Message::Assistant(m)) => {
                    for call in m.content.iter().filter_map(|c| match c {
                        Content::ToolCall(call) => Some(call),
                        _ => None,
                    }) {
                        let args = Value::Object((*call.arguments).clone());
                        // A tool ROW is a string surface (see the live fold): `into_text` flattens
                        // exactly as `events_fold.rs` does, and a fault already collapsed to `None`.
                        if let Some(text) =
                            extension_render_tool_call(ext_host, &call.name, &args, &opts)
                                .await
                                .into_text()
                        {
                            rendered
                                .tool_calls
                                .insert(call.id.as_str().to_string(), text);
                        }
                    }
                }
                AgentMessage::Core(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    details,
                    ..
                }) => {
                    // The SAME `{content, details}` value the walk hands the built-in renderer,
                    // which is also what pi's `ToolExecutionComponent` passes its `renderResult`
                    // (`tool-execution.ts:307-308`: `{ content: this.result.content, details:
                    // this.result.details }`).
                    let result = tool_result_payload(content, details.as_ref());
                    if let Some(text) =
                        extension_render_tool_result(ext_host, tool_name, &result, &opts)
                            .await
                            .into_text()
                    {
                        rendered
                            .tool_results
                            .insert(tool_call_id.as_str().to_string(), text);
                    }
                }
                _ => {}
            }
        }
        // EXT-041 — the custom-ENTRY surface, a SECOND and disjoint lookup exactly as it is on the
        // live path (pi keeps `messageRenderers` and `entryRenderers` as separate maps,
        // `extensions/types.ts:1766`/`:1768` @v0.84.4, and `addCustomEntryToChat` resolves the entry one,
        // `interactive-mode.ts:3571`). Its own loop because it is keyed by ITEM index, not by the
        // message index the loop above walks. The three-state outcome is carried WHOLE: a faulting
        // renderer draws pi's failure box (`custom-entry.ts:47-52`), which is not the same answer
        // as "no renderer claimed this type".
        for (i, item) in items.iter().enumerate() {
            if let cyrup_session_svc::ReplayItem::CustomEntry(entry) = item {
                let ty = custom_entry_type(entry);
                let r = extension_render_entry(ext_host, &ty, entry, &opts).await;
                if r.has_content() {
                    rendered.entries.insert(i, r);
                }
            }
        }
        // EXT-019 — the commit frontier before the walk, for the transform pass below.
        let first_pending = self.state.transcript.pending_len();
        self.replay_session_rendered(items, &rendered);
        // A replayed conversation does NOT route through `ingest_event_with_extensions_owned`, so
        // it would otherwise render untransformed while a live one renders transformed — the same
        // trap `refresh_known_tool_definitions` documents for the per-tool-start lookup above.
        // Upstream cannot have it: pi rebuilds real `UserMessage`/`AssistantMessage` components on
        // replay (`renderSessionEntries`, `interactive-mode.ts:3781-3796`), each with its own
        // `createMarkdownTransform`.
        self.apply_markdown_transformers(ext_host, first_pending)
            .await;
    }

    /// The replay walk itself — pi `renderSessionItems` (`interactive-mode.ts:3705-3775`), over the
    /// same union of items: raw-context messages interleaved with the re-derived cache-miss and
    /// compaction-cost notices.
    ///
    /// `rendered.messages` maps a MESSAGE index (not an item index) to the text an extension's
    /// registered renderer produced for it (X11); `rendered.tool_calls`/`.tool_results` map a
    /// TOOL-CALL id to the extension's call header / result body (EXT-041). An absent entry draws
    /// the built-in framing, which is Pi's `getMessageRenderer(...) === undefined` outcome for a
    /// message and `getCallRenderer()`/`getResultRenderer()` resolving to the built-in definition
    /// (`tool-execution.ts:84-101`) for a tool row.
    fn replay_session_rendered(
        &mut self,
        items: &[cyrup_session_svc::ReplayItem],
        rendered: &ReplayRenders,
    ) {
        use cyrup_core::{Content, Message};
        use cyrup_session_svc::ReplayItem;
        use cyrup_session_svc::agent_message::AgentMessage;
        use serde_json::Value;
        // The MESSAGE index `rendered` is keyed by — advanced only on a message, so the notices
        // interleaved by [`AgentSession::replay_items`] cannot desynchronise it.
        let mut next_index = 0usize;
        for (item_index, item) in items.iter().enumerate() {
            let message = match item {
                ReplayItem::Message(m) => m.as_ref(),
                // pi re-injects the miss notice after the assistant message that paid for it
                // (`interactive-mode.ts:3753-3755`); the gate is its `getShowCacheMissNotices()`
                // at the collection site (`:3694`).
                ReplayItem::CacheMiss(miss) => {
                    if self.state.show_cache_miss_notices {
                        self.state.transcript.push_cache_miss_notice(miss);
                    }
                    continue;
                }
                // pi's synthesised `compaction_cost` item, dispatched by `renderSessionItems`
                // (`:3705-3709`) into `addCompactionCostNotice`, which re-reads the same gate.
                ReplayItem::CompactionCost { kind, usage } => {
                    if self.state.show_cache_miss_notices {
                        self.state.transcript.push_compaction_cost_notice(
                            match kind {
                                cyrup_session_svc::CompactionCostKind::Compaction => {
                                    crate::transcript::CompactionCostKind::Compaction
                                }
                                cyrup_session_svc::CompactionCostKind::BranchSummary => {
                                    crate::transcript::CompactionCostKind::BranchSummary
                                }
                            },
                            usage,
                        );
                    }
                    continue;
                }
                // EXT-041 — pi's `if (isCustomSessionEntry(item)) this.addCustomEntryToChat(item)`
                // (`interactive-mode.ts:3717-3719`), the SAME method its live `entry_appended` arm
                // calls (`:3217-3218`). The three-state outcome the pre-pass resolved decides what
                // draws, exactly as it does live.
                ReplayItem::CustomEntry(entry) => {
                    let ty = custom_entry_type(entry);
                    let r = rendered
                        .entries
                        .get(&item_index)
                        .cloned()
                        .unwrap_or(crate::transcript::Rendered::None);
                    self.push_custom_entry(ty, r);
                    continue;
                }
            };
            let index = next_index;
            next_index += 1;
            match message {
                AgentMessage::Core(Message::User { content, .. }) => {
                    let text = content_text(content);
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.state.transcript.push_user(text.clone());
                    // Pi `populateHistory` (interactive-mode.ts:3387): replayed prompts are
                    // recallable with Up, so a resumed session can re-run its own last message.
                    self.state.editor.push_history(&text);
                }
                AgentMessage::Core(Message::Assistant(m)) => {
                    let thinking = thinking_text(&m.content);
                    if !thinking.is_empty() {
                        self.state.transcript.commit_thinking(Some(thinking));
                    }
                    let text = content_text(&m.content);
                    if !text.trim().is_empty() {
                        self.state.transcript.commit_assistant(Some(text));
                    }
                    for call in m.content.iter().filter_map(|c| match c {
                        Content::ToolCall(call) => Some(call),
                        _ => None,
                    }) {
                        // Pi files each replayed call component under `content.id`
                        // (`renderedPendingTools.set(content.id, component)`,
                        // interactive-mode.ts:3473) so the `toolResult` below resolves to the exact
                        // call that produced it — two `read`s in one turn are indistinguishable by
                        // name.
                        // `hasRendererDefinition()` (tool-execution.ts:103-105) and
                        // `getRenderShell()` (`:108-116`) for a replayed call: the bind refreshed
                        // the whole registry into [`AppState::known_tool_definitions`] just above,
                        // which is the only place this walk can ask — it holds messages, not a
                        // session.
                        let definition = self.state.known_tool_definitions.get(&call.name).copied();
                        self.state.transcript.push_tool_start_defined(
                            call.name.clone(),
                            Some(call.id.as_str().to_string()),
                            Value::Object((*call.arguments).clone()),
                            // EXT-041 — the extension's call header, resolved in the pre-pass
                            // above; `None` keeps the built-in per-tool dispatch, as on the live
                            // path.
                            rendered.tool_calls.get(call.id.as_str()).cloned(),
                            definition,
                        );
                    }
                    if let Some(notice) = stop_reason_notice(m) {
                        self.state.transcript.push_error(notice);
                    }
                }
                AgentMessage::Core(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    is_error,
                    details,
                    ..
                }) => {
                    // The shape every per-tool `renderResult` reads (`{content, details}`) — the
                    // built-in's and the extension's alike, so the pre-pass built the same value.
                    let result = tool_result_payload(content, details.as_ref());
                    // `renderedPendingTools.get(message.toolCallId)` (`:3483`) — an exact id lookup,
                    // never a name scan.
                    self.state.transcript.push_tool_end_rendered(
                        tool_name.clone(),
                        Some(tool_call_id.as_str()),
                        *is_error,
                        Some(result),
                        // EXT-041 — the extension's result body for THIS call id, or the built-in.
                        rendered.tool_results.get(tool_call_id.as_str()).cloned(),
                    );
                    // Keep call order in scrollback: commit the finished leading run now instead of
                    // deferring every tool of the whole replay to the end.
                    self.state.transcript.commit_finished_leading_tools();
                }
                AgentMessage::BashExecution(b) => {
                    self.state.transcript.push_bash_execution(
                        b.command.clone(),
                        b.exclude_from_context.unwrap_or(false),
                        &b.output,
                        b.exit_code.and_then(|c| i32::try_from(c).ok()),
                        b.cancelled,
                        // X13 — upstream replays both (`interactive-mode.ts:3460-3465`
                        // `message.truncated ? {truncated:true} : undefined, message.fullOutputPath`),
                        // which is what puts the `Output truncated. Full output: …` row back on a
                        // resumed session's `!` block.
                        b.truncated,
                        b.full_output_path.clone(),
                    );
                }
                AgentMessage::Custom(c) => {
                    // Pi renders a custom message only when it opted into display
                    // (`if (message.display)`, interactive-mode.ts:3470).
                    if c.display {
                        // X11 — `const renderer = this.session.extensionRunner.getMessageRenderer(
                        // message.customType); new CustomMessageComponent(message, renderer, …)`
                        // (`:3471-3477`). The replay arm is NOT a thinner variant of the live one:
                        // it performs the identical lookup, so a resumed session keeps the
                        // extension rendering the live session had. Absent an entry the built-in
                        // `[type] body` framing draws — `getMessageRenderer` returning `undefined`.
                        let rendered = rendered.messages.get(&index).cloned().unwrap_or_default();
                        self.state.transcript.push_custom_message_rendered(
                            c.custom_type.clone(),
                            custom_message_text(&c.content),
                            rendered,
                        );
                    }
                }
                AgentMessage::BranchSummary(b) => {
                    self.state.transcript.push_branch_summary(b.summary.clone());
                }
                AgentMessage::CompactionSummary(c) => {
                    self.state
                        .transcript
                        .push_compaction_summary(c.tokens_before, c.summary.clone());
                }
            }
        }
        // A tool call whose result never persisted (an interrupted turn) still commits, as-is.
        self.state.transcript.commit_tools();
    }

    /// Emit the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    /// interactive-mode.ts:1480-1690, called with `{force: false, showDiagnosticsWhenQuiet: true}`
    /// at `:1769`).
    ///
    /// TUI-006: without this, extension load failures, shadowed skills and missing prompt paths were
    /// entirely invisible in cyrup — the data existed (`AgentSessionServices::startup_diagnostics`)
    /// but nothing rendered it. Push it before the first draw so it lands at the top of scrollback,
    /// ahead of the conversation.
    pub fn push_loaded_resources(&mut self, report: &crate::startup::StartupReport) {
        self.state
            .transcript
            .push_loaded_resources(crate::startup::build_startup_lines(report));
    }

    /// Arm pi's `options.verbose` for [`Self::push_session_loaded_resources`] — the `--verbose`
    /// flag that overrides `quietStartup` for the listing (`interactive-mode.ts:1702` @v0.84.4).
    /// The host calls this once, before the first frame, exactly as it arms
    /// [`Self::set_auto_trust_on_reload_cwd`].
    pub fn set_verbose_startup(&mut self, verbose: bool) {
        self.state.verbose_startup = verbose;
    }

    /// Re-emit the loaded-resources / diagnostics panel for `session` (TUI-N02).
    ///
    /// pi calls `showLoadedResources({force: false, showDiagnosticsWhenQuiet: true})` from BOTH
    /// `bindCurrentSessionExtensions` (`interactive-mode.ts:1982` @v0.84.4, reached on boot AND on
    /// every session replacement via `rebindCurrentSession` → the runtime's `setRebindSession`
    /// hook, `:577`) and `handleReloadCommand` (`:5991-5994`, the identical options object). cyrup
    /// pushed it only from the boot path, so `/reload` — the command a user runs right after
    /// editing an extension, skill or prompt — reported `Reloaded …` and swallowed the very
    /// diagnostics the reload had just re-collected: a broken extension, a shadowed skill name, a
    /// prompt conflict. The data was rebuilt server-side by the session factory and discarded.
    ///
    /// The panel is NOT gated by swap reason, because pi's is not: its hook fires for `/new`,
    /// `/resume`, `/fork`, `/import` and `/reload` alike.
    ///
    /// pi re-renders into a dedicated `loadedResourcesContainer` that it `clear()`s first
    /// (`:1699`), a region pinned ABOVE `chatContainer` (`:594-596`); cyrup's committed entries live
    /// in the terminal's own scrollback and cannot be re-rendered, so the caller pushes this
    /// BEFORE the swap's replay to reproduce that stacking, and a second swap appends a second
    /// panel rather than replacing the first.
    pub fn push_session_loaded_resources(&mut self, session: &cyrup_session_svc::AgentSession) {
        let report =
            crate::startup::StartupReport::from_session(session, self.state.verbose_startup);
        self.push_loaded_resources(&report);
    }

    /// Put already-queued steering/follow-up text back into the editor — the buffer half of Pi's
    /// `restoreQueuedMessagesToEditor` (interactive-mode.ts:4064-4083). `queued` is
    /// `[...steering, ...followUp]` **already drained** from the session (Pi's `clearAllQueues()`
    /// at `:4065`, here [`AgentSession::drain_queue`]); this half is pure, so the run loop owns the
    /// async drain and the abort and the App owns what the user sees.
    ///
    /// The queued messages join with a blank line and are PREPENDED to whatever is already typed,
    /// with empty parts dropped (`:4074-4077` — `[queuedText, currentText].filter(t => t.trim())`).
    /// An empty queue leaves the editor untouched and returns `0`, which is how
    /// [`AppAction::Dequeue`] decides between Pi's two `handleDequeue` statuses (`:3834-3841`).
    /// The Esc path (`{abort: true}`) shows no status at all — Pi's escape branch never calls
    /// `showStatus`.
    pub fn restore_queued_to_editor(&mut self, queued: &[String]) -> usize {
        if queued.is_empty() {
            return 0;
        }
        let queued_text = queued.join("\n\n");
        let current = self.state.editor.text();
        let combined = [queued_text, current]
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.state.editor.set_text(&combined);
        queued.len()
    }
}

/// Everything the extension-renderer pre-pass resolved for one replay, handed to the sync walk
/// ([`App::replay_session_rendered`]). Four surfaces, four keys, because pi resolves them at four
/// different places: a custom message by its position in the stream (`getMessageRenderer`,
/// `interactive-mode.ts:3610` @v0.84.4), a tool call by `content.id` (`:3760`), a tool result by
/// `message.toolCallId` (`:3770`) and a custom ENTRY by `entry.customType` at the walk itself
/// (`getEntryRenderer`, `:3571`). A key with no entry draws the built-in framing.
///
/// The two tool maps hold flattened text rather than [`crate::transcript::Rendered`] because a
/// tool row is a string surface — the live fold flattens with `into_text()` at the push
/// (`events_fold.rs`), and the replay must not carry a tier the row cannot draw. The message and
/// entry maps carry the whole [`crate::transcript::Rendered`]: a `Live` component draws, and — for
/// an entry — `Failed` is pi's own third outcome (`custom-entry.ts:47-52`), not an absence.
#[derive(Default)]
struct ReplayRenders {
    /// MESSAGE index → the custom-message renderer's output (X11 / EXT-006).
    messages: std::collections::HashMap<usize, crate::transcript::Rendered>,
    /// Tool-call id → the extension's `renderCall` text (EXT-041).
    tool_calls: std::collections::HashMap<String, crate::transcript::RenderedText>,
    /// Tool-call id → the extension's `renderResult` text (EXT-041).
    tool_results: std::collections::HashMap<String, crate::transcript::RenderedText>,
    /// ITEM index → the custom-ENTRY renderer's three-state outcome (EXT-041).
    ///
    /// Keyed by position in the ITEM stream, not by entry id: pi resolves this renderer inside the
    /// walk, holding the entry (`addCustomEntryToChat(item)`, `:3718`), so there is no upstream key
    /// to mirror — and the item's own position is the one address that exists whatever the
    /// serialized entry happens to carry.
    entries: std::collections::HashMap<usize, crate::transcript::Rendered>,
}

/// The `{content, details}` value a persisted `toolResult` message presents to a `renderResult`
/// — pi's `{ content: this.result.content, details: this.result.details }`
/// (`components/tool-execution.ts:307-308` @v0.84.4). Built ONCE per shape so the extension
/// renderer in the pre-pass and the built-in renderer in the walk see the same value.
fn tool_result_payload(
    content: &[cyrup_core::Content],
    details: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    result.insert(
        "content".to_string(),
        serde_json::to_value(content).unwrap_or(serde_json::Value::Null),
    );
    if let Some(d) = details {
        result.insert("details".to_string(), d.clone());
    }
    serde_json::Value::Object(result)
}

/// The messages of a replay stream, in order, with the derived notices skipped — the projection an
/// extension-renderer pre-pass and the `rendered` index map are both keyed against.
fn replay_messages(
    items: &[cyrup_session_svc::ReplayItem],
) -> impl Iterator<Item = &cyrup_session_svc::agent_message::AgentMessage> {
    items.iter().filter_map(|item| match item {
        cyrup_session_svc::ReplayItem::Message(m) => Some(m.as_ref()),
        _ => None,
    })
}

/// Lift a plain message list into a replay stream carrying no derived notices — what the two
/// `AgentMessage`-shaped replay entry points feed the walk. A caller that wants the notices asks
/// the session for them ([`cyrup_session_svc::AgentSession::replay_items`]).
fn as_replay_items(
    messages: &[cyrup_session_svc::agent_message::AgentMessage],
) -> Vec<cyrup_session_svc::ReplayItem> {
    messages
        .iter()
        .cloned()
        .map(|m| cyrup_session_svc::ReplayItem::Message(Box::new(m)))
        .collect()
}
