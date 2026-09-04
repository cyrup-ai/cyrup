use super::*;

impl<B: Backend> App<B> {
    /// Fold an `AgentSessionEvent` into the UI state.
    ///
    /// Decodes tool names + error flag, model changes, queue depth, compaction, the live streaming
    /// **delta** text (`MessageUpdate` → [`Self::ingest_stream_event`] →
    /// [`TranscriptView::push_assistant_delta`](crate::transcript::TranscriptView::push_assistant_delta)),
    /// and the **terminal** assistant message (recovered via `StreamEvent::terminal_message()`, which
    /// yields a `&cyrup_core::AssistantMessage`). `cyrup-provider` is a direct dependency, so the
    /// token-by-token render (gap 1) is live, not deferred.
    ///
    /// TUI-092 F8 — by-reference, and therefore a `clone()`: the fold itself
    /// ([`Self::ingest_event_rendered_owned`]) consumes the event so the run loop can MOVE each
    /// payload. This entry point exists for the in-crate test call sites that hand it a borrowed
    /// literal; no production path reaches it, so no production path pays the clone.
    pub fn ingest_event(&mut self, ev: &AgentSessionEvent) {
        self.ingest_event_rendered_owned(
            ev.clone(),
            crate::transcript::Rendered::None,
            crate::transcript::Rendered::None,
        );
    }

    /// [`Self::ingest_event`], first giving the loaded extensions a chance to RENDER the event
    /// (EXT-006). This is the extension-aware fold — the interactive run loop reaches it through
    /// [`Self::ingest_event_with_extensions_owned`]; the sync [`Self::ingest_event`] is the
    /// no-extensions shorthand.
    ///
    /// Pi resolves a renderer at the point of display — `extensionRunner.getMessageRenderer(...)`
    /// for a custom message (interactive-mode.ts:3324-3336) and the per-tool `renderCall`/
    /// `renderResult` for a tool row (components/tool-execution.ts:81-112). cyrup's fold is sync
    /// (it mutates `&mut self` from a `select!` arm) while a guest renderer is an async wasm call,
    /// so the renderer runs FIRST and its text rides into the fold.
    ///
    /// TUI-092 F8 — the by-reference twin of [`Self::ingest_event_with_extensions_owned`], kept for
    /// the test call sites (in-crate and `cyrup-it`'s renderer-screen bin) that hand it a borrowed
    /// event. It pays a `clone()` the production run loop does not.
    pub async fn ingest_event_with_extensions(
        &mut self,
        ev: &AgentSessionEvent,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        self.ingest_event_with_extensions_owned(ev.clone(), ext_host)
            .await;
    }

    /// [`Self::ingest_event_with_extensions`] taking the event BY VALUE (TUI-092 F8) — the
    /// production shape, and the one that lets the fold MOVE each payload into the transcript.
    ///
    /// Both renderer lookups still run FIRST and off a borrow, exactly as before: `&ev` is live
    /// until the fold consumes it on the last statement, so nothing about the render-then-fold
    /// order changes.
    pub async fn ingest_event_with_extensions_owned(
        &mut self,
        ev: AgentSessionEvent,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        // EXT-006 — the display inputs the renderer runs under, read from the LIVE view. They are
        // recorded on the result (`run_renderer`), so a later toggle or theme switch can tell that
        // this text is stale and ask for it again.
        let opts = self.render_options();
        let rendered = extension_render(ext_host, &ev, &opts).await;
        // X15 — the custom-ENTRY renderer is a SECOND, disjoint lookup (Pi keeps
        // `messageRenderers` and `entryRenderers` as separate maps, types.ts:1703-1704, and
        // `addCustomEntryToChat` resolves the entry one at `interactive-mode.ts:3432`). It rides in
        // the same way and for the same reason: the fold is sync, the guest call is async.
        let entry = match &ev {
            AgentSessionEvent::EntryAppended { entry } => {
                let custom_type = custom_entry_type(entry);
                extension_render_entry(ext_host, &custom_type, entry, &opts).await
            }
            _ => crate::transcript::Rendered::None,
        };
        // EXT-019 — the commit frontier BEFORE the fold, so the transform pass below walks exactly
        // the entries this event produces. Nothing drains between the two reads: `drain_committed`
        // runs inside `App::draw`, one run-loop arm later.
        let first_pending = self.state.transcript.pending_len();
        self.ingest_event_rendered_owned(ev, rendered, entry);
        self.apply_markdown_transformers(ext_host, first_pending)
            .await;
    }

    /// The display inputs a renderer runs under RIGHT NOW (EXT-006) — the `(options, theme)` half
    /// of every upstream renderer signature, read live off the view.
    ///
    /// * `expanded` — `this.toolOutputExpanded`, which `setToolsExpanded` re-broadcasts to every
    ///   child on every toggle (`modes/interactive/interactive-mode.ts:4032-4048` @v0.84.4) and
    ///   `:3437` seeds into a freshly added `CustomEntryComponent`;
    /// * `output_pad` — `MessageRenderOptions.outputPad` (`extensions/types.ts:1198` @v0.84.4), the
    ///   `outputPad` setting, which `/settings` can move mid-session;
    /// * `theme` — the ACTIVE theme's name. See [`cyrup_ext::RenderOptions`] for why a name and not
    ///   the palette.
    ///
    /// `is_partial` is left `false` here because it is a property of a tool ROW, not of the frame;
    /// [`crate::transcript::TranscriptView::stale_extension_renders`] overrides it per row.
    pub(crate) fn render_options(&self) -> cyrup_ext::RenderOptions {
        cyrup_ext::RenderOptions::new(
            self.state.transcript.tool_expanded(),
            u32::try_from(self.state.transcript.output_pad()).unwrap_or(u32::MAX),
            Some(self.state.theme.name.clone()),
        )
    }

    /// Re-invoke every extension renderer whose output was produced under display inputs that no
    /// longer hold (EXT-006) — the toggle/theme half of the item.
    ///
    /// # Why this exists at all
    /// Upstream re-invokes a renderer from the DRAW path: `MessageRenderer = (message, options,
    /// theme) => Component | undefined` (`core/extensions/types.ts:1213-1217` @v0.84.4) is called
    /// per paint, so `Ctrl+O` and a `/theme` switch reach a component that was pushed under the old
    /// values. cyrup's draw path is sync (`App::draw` cannot `await`) and a guest renderer is an
    /// async wasm call, so the render is done once off the event path and its text written into the
    /// transcript. Without this pass that text was BAKED: every built-in row around it responded to
    /// the toggle and the extension's row did not.
    ///
    /// Same seam, and the same reason, as [`Self::apply_markdown_transformers`]: the decision is a
    /// pure comparison the view makes ([`crate::transcript::TranscriptView::stale_extension_renders`]);
    /// this is only the shell that awaits the guest and writes the answer back.
    ///
    /// # Scope
    /// Rows that are still addressable — the active tool runs and the committed-but-unflushed
    /// entries. A row already flushed to native scrollback (R-ARCH-TUI-003) cannot be repainted by
    /// anyone, which is the same boundary `set_pending_markdown` works within.
    pub(crate) async fn refresh_extension_renders(
        &mut self,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        let live = self.render_options();
        // Snapshotted rather than borrowed: each render is awaited, and a borrow into the
        // transcript cannot straddle that await while `self` is also the receiver.
        let stale = self.state.transcript.stale_extension_renders(&live);
        for item in stale {
            let rendered = super::extension_render_impl::run_renderer(
                ext_host,
                item.next.surface,
                item.next.key.clone(),
                item.next.payload.clone(),
                &item.next.under,
            )
            .await;
            // Only a fresh TEXT replaces the old one. A renderer that now answers `None` (or
            // faults, or wedges) leaves the previous text in place rather than blanking a row that
            // was drawing a moment ago — upstream's components keep their last child until a
            // rebuild produces a new one (`custom-message.ts:66-81`).
            if let crate::transcript::Rendered::Text(text) = rendered {
                self.state.transcript.set_extension_render(item.slot, text);
            }
        }
    }

    /// Run the extension-registered markdown transformers over everything the fold just put on
    /// screen — pi's `createMarkdownTransform(messageType, isStreaming, transformers)`
    /// (`components/markdown-transform.ts:3-10`), which upstream attaches to the `Markdown` child of
    /// each of its three message components: `user-message.ts:53` (`"user"`, `false`),
    /// `assistant-message.ts:112` (`"assistant"`, `this.isStreaming`) and `:157-161`
    /// (`"assistant-thinking"`).
    ///
    /// # Why the seam is here
    /// This reuses, verbatim, the reason the two renderer lookups above it run before the fold
    /// (`ingest_event_with_extensions_owned`, and the note at the top of this file): cyrup's fold is
    /// sync — it mutates `&mut self` from a `select!` arm — while a guest call is async. Upstream
    /// has no such constraint and applies the transform as the first statement of
    /// `Markdown.render()`, on every frame (`markdown.ts:285`); cyrup's markdown renderer is reached
    /// from `App::draw`, which cannot `await`. So the fold runs ONCE per body, here, and the result
    /// is written back into the transcript. The consequence — a resize does not re-run transformers
    /// over already-committed text — is recorded on `crate::markdown::render_message`.
    ///
    /// `from` is the pending index the caller snapshotted before the entries were pushed; the live
    /// partials are always re-offered, because a delta lengthened them.
    ///
    /// **No fold or containment logic lives here.** Ordering the transformers, feeding each one the
    /// previous one's output and CONTAINING a faulting one are all
    /// [`cyrup_ext::ExtensionHost::transform_markdown`]'s job (facade.rs), which is the single
    /// implementation of `applyMarkdownTransformers` (`markdown-transform.ts:12-28`).
    pub(crate) async fn apply_markdown_transformers(
        &mut self,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
        from: usize,
    ) {
        // The sync pre-check the `has_*_renderer` gates in `extension_render` use, for the same
        // reason: this runs once per event — including once per streamed delta — and a session with
        // no markdown transformer must pay one rwlock read rather than an async hop, and must leave
        // every rendered line byte-identical.
        if !ext_host.has_markdown_transformers() {
            return;
        }
        // `availableWidth` (`core/extensions/types.ts:1204`) is `markdown.ts:284`'s `contentWidth`:
        // the width the renderer itself works in, `max(1, width - paddingX * 2)`, where `paddingX`
        // for all three message components is `outputPad`. That is the same expression
        // `transcript/render.rs` and `transcript/cache.rs` already feed `render_message` as `width`
        // — the only difference is that they have the frame width and this has the last drawn one
        // ([`AppState::term_cols`]).
        let pad = u32::try_from(self.state.transcript.output_pad()).unwrap_or(u32::MAX);
        let available_width = u32::from(self.state.term_cols)
            .saturating_sub(pad.saturating_mul(2))
            .max(1);
        // Snapshotted rather than borrowed: the guest call is awaited, and a `&mut String` pointing
        // into the transcript cannot straddle that await while `self` is also the receiver.
        let committed: Vec<(usize, crate::markdown::MessageType, String)> = self
            .state
            .transcript
            .pending_markdown(from)
            .map(|(index, kind, text)| (index, kind, text.to_string()))
            .collect();
        for (index, kind, text) in committed {
            // `isStreaming: false`: a committed entry is by definition no longer streaming — the
            // turn that produced it has ended (`assistant-message.ts:111`, whose `this.isStreaming`
            // is false for every finalized message).
            let out = ext_host
                .transform_markdown(&text, kind.as_pi_str(), false, available_width)
                .await;
            if out != text {
                self.state.transcript.set_pending_markdown(index, out);
            }
        }
        // The two LIVE partials, with `isStreaming: true` — pi's streaming
        // `AssistantMessageComponent` is the same component with the flag set (`:111`), and it
        // carries BOTH the answer text and the reasoning run (`:156-162`).
        if let Some(raw) = self.state.transcript.thinking().map(str::to_string) {
            let out = ext_host
                .transform_markdown(
                    &raw,
                    crate::markdown::MessageType::AssistantThinking.as_pi_str(),
                    true,
                    available_width,
                )
                .await;
            self.state
                .transcript
                .set_thinking_display((out != raw).then_some(out));
        }
        if let Some(raw) = self.state.transcript.streaming().map(str::to_string) {
            let out = ext_host
                .transform_markdown(
                    &raw,
                    crate::markdown::MessageType::Assistant.as_pi_str(),
                    true,
                    available_width,
                )
                .await;
            self.state
                .transcript
                .set_streaming_display((out != raw).then_some(out));
        }
    }

    /// The run loop's per-event fold: [`Self::ingest_event_with_extensions`], then the footer's
    /// session-derived context segment ([`Self::refresh_context_usage`]). The loop itself calls the
    /// by-value [`Self::ingest_session_event_owned`]; this is the shape of both.
    ///
    /// This is the whole of what pi's `render()` does for the footer for free — it calls
    /// `this.session.getContextUsage()` on every frame (`footer.ts:108`). cyrup's fold is sync and
    /// cannot `await` the session, so the refresh is hoisted here, to the one place that both holds
    /// the session and already runs per event.
    ///
    /// TUI-092 F8 — the by-reference twin of [`Self::ingest_session_event_owned`], kept for the
    /// tests that drive this seam with a borrowed event; the run loop calls the owned one.
    pub async fn ingest_session_event(
        &mut self,
        ev: &AgentSessionEvent,
        session: &Arc<AgentSession>,
    ) {
        self.ingest_session_event_owned(ev.clone(), session).await;
    }

    /// [`Self::ingest_session_event`] taking the event BY VALUE (TUI-092 F8) — what the run loop's
    /// events arm calls, since it owns every event it dequeues.
    pub async fn ingest_session_event_owned(
        &mut self,
        ev: AgentSessionEvent,
        session: &Arc<AgentSession>,
    ) {
        let ext_host = session.services().ext_host.clone();
        // TUI-092 F8 — read the event-kind predicate BEFORE the fold consumes `ev`. It is the same
        // pure `matches!` it always was (`event_extract.rs:10`); hoisting it changes the borrow and
        // nothing else, because the refresh it gates still runs where it did — after the fold and
        // after the compaction flush.
        let usage_may_have_moved = context_usage_may_have_moved(&ev);
        // Pi resolves `getToolDefinition(toolName)` per tool-execution component — the registry is
        // handed to the render context at `interactive-mode.ts:3413` and read as
        // `hasRendererDefinition()` at `tool-execution.ts:103-105` and as `getRenderShell()` at
        // `:108-116` (EXT-024). cyrup's fold is sync and holds no session, so the one lock-guarded
        // registry lookup is hoisted here, the same place (and for the same reason)
        // `refresh_context_usage` is. Memoized in [`AppState::known_tool_definitions`] so a
        // repeated tool never re-locks, and read BEFORE the fold consumes `ev`.
        if let AgentSessionEvent::ToolExecutionStart { tool_name, .. } = &ev
            && !self.state.known_tool_definitions.contains_key(tool_name)
            && let Some(definition) = session.tool_definition(tool_name)
        {
            self.state
                .known_tool_definitions
                .insert(tool_name.clone(), definition.render_kind);
        }
        self.ingest_event_with_extensions_owned(ev, &ext_host).await;
        // EXT-006 — the events arm's half of the same seam `dispatch_run_action` ends with: an
        // extension handler can move the display inputs from an EVENT (`ui.set-tools-expanded`,
        // `ui.theme-set`), which never passes through the input arm.
        self.refresh_extension_renders(&ext_host).await;
        // TUI-031 — `flushCompactionQueue` (`interactive-mode.ts:4036-4110` @v0.83.0), the last
        // statement of pi's `compaction_end` arm. Runs here because it needs the session; the sync
        // `ingest_event` half only raises the flag.
        if std::mem::take(&mut self.state.compaction_flush_pending) {
            for msg in self.take_compaction_queue() {
                // `if (isExtensionCommand) prompt(text) else if (mode === "followUp")
                // followUp(text) else steer(text)` (`:4055-4062`). Delivered in queue order.
                let ui = UserInput::text(msg.text.clone(), InputSource::Tui);
                if is_extension_command(session, &msg.text) {
                    let _ = session.prompt_accepted(ui).await;
                } else if msg.follow_up {
                    let _ = session.follow_up(ui).await;
                } else {
                    let _ = session.steer(ui).await;
                }
            }
        }
        // pi `maybeShowCacheMissNotice(this.streamingMessage)`, the last statement of its
        // `message_end` arm before the streaming slot closes (`interactive-mode.ts:3311`). It
        // lands here rather than in the sync fold for the same reason the compaction flush above
        // does: the scan is an async session call and `ingest_event` holds no session. Position is
        // still pi's — on the SAME event, so the notice precedes any tool row of the next event.
        if std::mem::take(&mut self.state.cache_miss_check_pending)
            && let Some(miss) = session.last_cache_miss().await
        {
            self.state.transcript.push_cache_miss_notice(&miss);
        }
        // `autoCompactionEnabled` is a plain `bool` read with no session walk behind it, and
        // upstream's THIRD `setAutoCompactEnabled` call site is a settings toggle rather than a turn
        // event (`interactive-mode.ts:4417-4419`), so it must not ride the six-event predicate that
        // gates the (much more expensive) context recompute below.
        self.refresh_auto_compact(session);
        if usage_may_have_moved {
            self.refresh_context_usage(session).await;
        }
    }

    /// Re-read `getContextUsage()` off the live session into the footer (`footer.ts:106-111`), plus
    /// [`Self::refresh_auto_compact`].
    ///
    /// The three answers map straight onto [`StatusLine::set_context_usage`]; `percent` is a 0-100
    /// percentage session-side and a fraction footer-side, hence the `/ 100.0`.
    ///
    /// **The ` (auto)` suffix does not belong to this method alone.** Upstream sets it from three
    /// places — construction (`interactive-mode.ts:572`), a runtime-settings reapply (`:1902`) and
    /// the `/settings` auto-compaction toggle's `onAutoCompactChange` callback (`:4417-4419`) — and
    /// only the first two are turn-shaped. cyrup has no auto-compaction row in its settings selector
    /// today (`AgentSession::set_auto_compaction_enabled` is reached only from the RPC mode and the
    /// `SessionCommand` seam), so there is no toggle site to wire; when one is added it must call
    /// [`Self::refresh_auto_compact`] directly, exactly as `onAutoCompactChange` does. Until then the
    /// per-event refresh in [`Self::ingest_session_event`] picks up any out-of-band change.
    pub async fn refresh_context_usage(&mut self, session: &Arc<AgentSession>) {
        self.refresh_auto_compact(session);
        match session.stats_context_usage().await {
            Some(usage) => self.state.status.set_context_usage(
                usage.percent.map(|p| p / 100.0),
                Some(usage.context_window),
                session.auto_compaction_enabled(),
            ),
            None => {
                self.state
                    .status
                    .set_context_usage(None, None, session.auto_compaction_enabled())
            }
        }
    }

    /// `this.footer.setAutoCompactEnabled(this.session.autoCompactionEnabled)` — the ` (auto)`
    /// suffix on the footer's context segment, on its own (`interactive-mode.ts:572`, `:1902`,
    /// `:4418`).
    ///
    /// Sync and cheap: `auto_compaction_enabled()` is an override-or-default `bool` read, no session
    /// walk. Any future auto-compaction toggle in cyrup's settings selector calls THIS.
    pub fn refresh_auto_compact(&mut self, session: &Arc<AgentSession>) {
        self.state
            .status
            .set_auto_compact(session.auto_compaction_enabled());
    }

    /// Pi `message_end`'s finalization of an assistant message (`interactive-mode.ts:3183-3214`):
    /// the authoritative message replaces whatever streamed, and the streaming slot closes.
    ///
    /// Commits the reasoning FIRST — Pi walks the message content in order and `thinking` precedes
    /// the answer (`assistant-message.ts:115-166`) — preferring the final message's blocks over the
    /// streamed ones, since a redacted/summarised block only ever arrives terminally.
    pub(crate) fn finalize_assistant_message(&mut self, message: &cyrup_core::AssistantMessage) {
        // `this.streamingComponent = undefined` (`:3213`).
        self.state.streaming_assistant = false;
        let thinking = thinking_text(&message.content);
        if thinking.is_empty() {
            self.state.transcript.commit_thinking(None);
        } else {
            self.state.transcript.commit_thinking(Some(thinking));
        }
        let text = content_text(&message.content);
        if text.is_empty() {
            // Pure tool-use / empty terminal: keep any streamed partial; `AgentEnd` commits it.
            self.state.transcript.commit_assistant(None);
        } else {
            self.state.transcript.commit_assistant(Some(text));
        }
        let tokens = message.usage.total_tokens;
        if tokens > 0 {
            self.state.status.set_tokens(tokens);
        }
        // Accumulate the turn into the cumulative session footer totals (footer.ts:86-107).
        self.state.status.add_usage(&message.usage);
        // A turn that did not finish cleanly gets Pi's error-styled footer notice
        // (assistant-message.ts:175-201) — otherwise a 5xx, an abort or a max-token
        // truncation would end the turn with no explanation at all.
        if let Some(notice) = stop_reason_notice(message) {
            self.state.transcript.push_error(notice);
        }
        // pi runs `maybeShowCacheMissNotice` only on the clean branch of its `message_end` arm —
        // the `else` of `if (stopReason === "aborted" || stopReason === "error")`
        // (`interactive-mode.ts:3752`, and the same exclusion on its replay walk). The scan itself
        // needs the session, which this sync finalizer does not hold, so raise the flag and let
        // [`Self::ingest_session_event_owned`] settle it — the `compaction_flush_pending` shape.
        if self.state.show_cache_miss_notices
            && !matches!(
                message.stop_reason,
                cyrup_core::StopReason::Aborted | cyrup_core::StopReason::Error
            )
        {
            self.state.cache_miss_check_pending = true;
        }
    }
}
