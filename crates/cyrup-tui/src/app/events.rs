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
        self.ingest_event_rendered_owned(ev.clone(), None, crate::transcript::Rendered::None);
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
        self.ingest_event_with_extensions_owned(ev.clone(), ext_host).await;
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
        let rendered = extension_render(ext_host, &ev).await;
        // X15 — the custom-ENTRY renderer is a SECOND, disjoint lookup (Pi keeps
        // `messageRenderers` and `entryRenderers` as separate maps, types.ts:1703-1704, and
        // `addCustomEntryToChat` resolves the entry one at `interactive-mode.ts:3432`). It rides in
        // the same way and for the same reason: the fold is sync, the guest call is async.
        let entry = match &ev {
            AgentSessionEvent::EntryAppended { entry } => {
                let custom_type = custom_entry_type(entry);
                extension_render_entry(ext_host, &custom_type, entry).await
            }
            _ => crate::transcript::Rendered::None,
        };
        self.ingest_event_rendered_owned(ev, rendered, entry);
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
        self.ingest_event_with_extensions_owned(ev, &ext_host).await;
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
            None => self.state.status.set_context_usage(
                None,
                None,
                session.auto_compaction_enabled(),
            ),
        }
    }

    /// `this.footer.setAutoCompactEnabled(this.session.autoCompactionEnabled)` — the ` (auto)`
    /// suffix on the footer's context segment, on its own (`interactive-mode.ts:572`, `:1902`,
    /// `:4418`).
    ///
    /// Sync and cheap: `auto_compaction_enabled()` is an override-or-default `bool` read, no session
    /// walk. Any future auto-compaction toggle in cyrup's settings selector calls THIS.
    pub fn refresh_auto_compact(&mut self, session: &Arc<AgentSession>) {
        self.state.status.set_auto_compact(session.auto_compaction_enabled());
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
    }
}
