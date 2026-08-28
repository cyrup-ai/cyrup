use super::*;

use crate::transcript::ImageOpts;

/// The per-paint bag every entry render needs and no [`crate::Entry`] can carry on itself — Pi's
/// `ToolRenderContext` (`tool-execution.ts:116-135`), built from the live [`AppState`].
///
/// One definition, two renderers. The inline commit flush and the alternate screen's document build
/// must produce the *same rows for the same entry* — that is the whole premise of ADR-0005 §B-5's
/// bridge (`altscreen/document.rs`: "there is deliberately no second rendering path") — and the
/// surest way to keep two call sites agreeing on nine fields is not to have two.
///
/// Every field is read LIVE rather than frozen onto a message, which is upstream's own rule for the
/// three that used to drift: `setToolsExpanded` re-broadcasts to every `chatContainer` child on each
/// toggle (`interactive-mode.ts:4032-4046`), and so do `setHiddenThinkingLabel` (`:2118-2129`) and
/// the image capability (`tool-execution.ts:331`). A committed block that scrolled up must not
/// disagree with the one still on screen it was scrolled up from.
fn image_opts<'a>(
    state: &'a AppState,
    // TUI-020 — the sink `tool_path_span` registers hrefs into while `entry_lines` runs, emitted as
    // OSC-8 once the cells exist. It is a per-flush local, so it cannot be read off `state`; the
    // fullscreen path passes `None`, which is the same value every caller passed before the
    // alternate screen existed.
    links: Option<&'a crate::osc::LinkSink>,
) -> ImageOpts<'a> {
    ImageOpts {
        show: state.transcript.show_images(),
        // TUI-N01 — the same capability both paths read, so a block that scrolled up cannot
        // disagree with the one still on screen.
        graphical: state.transcript.graphical_images(),
        width_cells: state.transcript.image_width_cells(),
        // X9/X7 — the live `app.tools.expand` label and the SESSION cwd (not the process's), which
        // is what `read`'s compact classification resolves its path against (`read.ts:336`).
        expand_key: state.transcript.expand_key(),
        cwd: state.transcript.cwd(),
        // TUI-020 — the same OSC-8 capability the live render reads, for the same reason
        // `graphical` is read here: a header that scrolled up must not disagree with the one still
        // on screen.
        hyperlinks: state.transcript.hyperlinks(),
        links,
        // X14 — the LIVE `this.toolOutputExpanded` (`interactive-mode.ts:442`).
        tools_expanded: state.transcript.tool_expanded(),
        // TUI-030 — the LIVE `setHiddenThinkingLabel` override.
        hidden_thinking_label: Some(state.transcript.hidden_thinking_label()),
        // The LIVE `markdown.mermaid` mode, for the same reason: upstream's transformer re-reads
        // `getMermaidRenderingMode()` on every render (`interactive-mode.ts:484-486`), so a row
        // cycled mid-session must reach both the inline flush and the alternate screen through the
        // one shared builder.
        mermaid: state.transcript.mermaid_mode(),
    }
}

impl<B: Backend> App<B> {
    /// Render one frame: first flush newly-committed entries to native scrollback (R-ARCH-TUI-003),
    /// then draw the active region into the inline viewport (pure: `state -> frame`).
    pub fn draw(&mut self) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // SEAM-T01/T02 — republish the extension-visible editor buffer and active theme name before
        // anything is painted, so a guest reading either sees the state this frame is about to
        // show. One call site because `draw` is the one every run-loop arm that can have changed
        // them passes through (see [`Self::publish_extension_readbacks`]).
        self.publish_extension_readbacks();
        // ADR-0005 §B-14 — the renderer fork, and it is the WHOLE fork: `draw` is the single frame
        // path every run-loop arm ends in, so routing here is what makes the alternate screen the
        // live renderer rather than a second one painting over the first. Regular mode is `None`
        // and everything below this line is untouched.
        if self.altscreen.is_some() {
            return self.draw_fullscreen();
        }
        // Content-size the inline viewport to the live region (active turn + band + slot + footer),
        // recomputed every frame as content grows/shrinks (ADR-0001 #1, audit #1). The viewport is
        // rebuilt only when its height actually changes so steady-state frames keep their cell-diff.
        // Resize **before** flushing so the committed `insert_before` lines scroll above the
        // correctly-anchored viewport (the active turn's height is unaffected by the flush).
        let size = self.terminal.backend().size().ok();
        // TUI-039 — pi's `$LINES` / `$COLUMNS` step sits between the ioctl and the constant
        // (`tui.ts:1730-1736`). cyrup's own last resort here stays the live viewport height rather
        // than pi's bare `24`, since it is a strictly better guess when one is available.
        let term_h = size
            .map(|s| s.height)
            .or_else(env_rows)
            .unwrap_or(self.viewport_height)
            .max(1);
        let term_w = size.map(|s| s.width).unwrap_or_else(fallback_columns);
        // Publish the SCREEN height before anything measures: the editor's row budget is
        // `max(5, floor(terminalRows * 0.3))` against the terminal, not the live region
        // (`editor.ts:499-501`; see [`AppState::term_rows`]). A selector that windows its own body
        // gets the same number through `Selector::set_terminal_height`, which is documented as
        // "called before `desired_height` on every frame" and, until now, was called only by the
        // standalone `startup_selector` loop — so the in-app `/config` grid and the `ui.editor`
        // dialog (E12) both sized themselves against a default they were never told to update.
        self.state.term_rows = term_h;
        // E17: the editor caps ITSELF at `max(5, floor(terminalRows * 0.3))` from inside
        // `render` (`editor.ts:499-501`), so it needs the screen height too — `region_constraints`
        // reserving the right number of rows is not the same thing as the component knowing its own
        // budget.
        self.state.editor.set_terminal_height(term_h);
        if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_terminal_height(term_h);
        }
        let raw = live_region_height(&mut self.state, term_w, term_h);
        // Grow-only hysteresis GATED on the turn being active. `status.streaming` is set on
        // `AgentStart` and cleared on `AgentEnd`, so it spans the WHOLE multi-step turn including the
        // gaps between tools (it is NOT `transcript.has_active()`, which flickers false between tools
        // and would re-trigger per-tool reconstruction); `has_bash()` covers a live `!`/`!!` run.
        // While active, the viewport pins at its high-water (capped to the terminal height so a
        // resize-shrink still reduces it) and stops tracking per-tool content churn — so
        // `resize_viewport`/`reanchor_inline` fire only on genuine geometry changes, killing the
        // per-tool FLICKER. Idle: drop the floor and size to the live content so the region collapses
        // to the compact editor/footer (void-fix).
        let turn_active = self.state.status.streaming || self.state.transcript.has_bash();
        // TUI-090 — a commit pending flush means content has LEFT the live region (a finished tool, the
        // finalized assistant text). If the floor is still pinned above the remaining content, the
        // viewport is stale-full and `insert_before` sends the flush straight to native scrollback
        // invisibly (ratatui-core inline.rs:66-67). Release the floor to the REMAINING content height on
        // exactly the frames that will flush, so the shrink (resize_viewport, which runs before
        // flush_committed below precisely so the insert lands above the correctly-anchored viewport)
        // puts the flushed lines ON the screen, directly above the live tail. Between commits
        // the floor stays grow-only — no per-tool-event reconstruction (the FLICKER fix is preserved);
        // the release costs one shrink per COMMIT, which is the frame that visually requires it.
        let flush_pending = !self.state.transcript.pending().is_empty();
        let desired = if turn_active {
            if flush_pending && raw < self.live_floor {
                self.live_floor = raw;
            }
            self.live_floor = self.live_floor.max(raw).min(term_h);
            self.live_floor
        } else {
            self.live_floor = 0;
            raw
        };
        if desired != self.viewport_height {
            // TUI-093 — NOT `?`. A frame at the previous height is a cosmetic defect; propagating
            // here unwinds ~40 `draw_synchronized()?` call sites out of `App::run` and ENDS THE
            // SESSION (main.rs `anyhow!("tui: {e}")` → `eprintln!("cyrup: {err:#}")`).
            // `viewport_height` is committed only on success, so the next frame retries the same
            // reconstruction rather than getting stuck believing it already happened.
            match self.resize_viewport(desired) {
                Ok(()) => self.viewport_height = desired,
                Err(e) => self.state.transcript.push_status(format!("viewport resize failed: {e}")),
            }
        }
        self.flush_committed()?;
        let App { terminal, state, .. } = self;
        terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Render one frame on the ADR-0005 §B-3 alternate screen — the fullscreen half of
    /// [`Self::draw`], reached only while [`App::switch_tui_mode`] has one installed.
    ///
    /// # What this deliberately does NOT do
    /// **`flush_committed`.** `Terminal::insert_before` writes into native scrollback, which is the
    /// one place a user inside the alternate screen cannot look at — the lines would land behind
    /// the screen they are staring at and, worse, `insert_before` on a viewport that takes up the
    /// whole screen goes straight to the scrollback buffer with no visible frame at all
    /// (ratatui-core `inline.rs:66-67`). Upstream has the same split: `TuiAltScreen` never writes
    /// to the main screen while it is up, and puts the conversation there on the way out instead
    /// (`tui-alt-screen.ts:322-327`, ADR-0005 §B-13).
    ///
    /// **`resize_viewport`.** The inline `Terminal` is not the one being painted and must not emit
    /// a single escape while the alternate screen owns the cells; it is put back by
    /// [`App::restore_main_screen_render_state`], which forces the rebuild on the first frame after
    /// the switch by seeding `viewport_height` to `0`.
    ///
    /// # What it must still do
    /// **Drain.** ADR-0005 §B-1's retained document — the only thing the alternate screen has to
    /// paint — grows exclusively inside [`crate::TranscriptView::drain_committed`]
    /// (`transcript/view.rs:110-116`), so the drain has to happen on this path too. The returned
    /// `Vec` is dropped rather than rendered: with retention on, the same entries are already in
    /// [`crate::TranscriptView::document`], which is what [`crate::AltScreen::sync_document`] walks.
    ///
    /// # Known residual: no chrome
    /// The frame is the scrolled document, the selection highlight, the scrollbar and the flash
    /// overlay — [`crate::AltScreen::draw`]'s z-order, upstream's `:1290`. The editor, the status
    /// band, the selector slot, the footer and the attachment strip are **not** painted: they are
    /// upstream's layout root (`interactive-mode.ts:933-936`), reached through
    /// [`crate::ViewportRenderer::set_layout_root`], and cyrup has no [`crate::Component`] that
    /// paints them from [`crate::AppState`] without the renderer owning it (`altscreen/mod.rs`,
    /// rule 2). Nothing calls `set_layout_root` today, so a fullscreen session shows the transcript
    /// and nothing else.
    fn draw_fullscreen(&mut self) -> Result<(), TuiError> {
        // Dropped, not rendered: with retention on the same entries are already in
        // `TranscriptView::document`, which is what `sync_document` walks below.
        drop(self.state.transcript.drain_committed());
        // Destructured for the disjoint borrows: `sync_document` reads the transcript and the theme
        // while the renderer it hands them to is mutably borrowed out of the same `self` — the
        // shape `app/draw.rs:89` and `altscreen/mod.rs`'s rule 1 both already use.
        let App { altscreen, state, .. } = self;
        let Some(alt) = altscreen.as_mut() else { return Ok(()) };
        alt.sync_document(&state.transcript, &state.theme, image_opts(state, None));
        // §B-12's strip, built from the same two `AppState` fields the inline path renders from, so
        // an attachment cannot look different across a mode switch.
        let strip = (!state.pending_images.is_empty()).then(|| crate::altscreen::Strip {
            renderer: &state.image_renderer,
            blocks: &state.pending_images,
            theme: &state.theme,
            show_images: state.transcript.show_images(),
            width_cells: state.transcript.image_width_cells(),
        });
        alt.draw(strip)
    }

    /// Rebuild the terminal with a new inline-viewport `height` over a fresh handle to the same
    /// backend (ratatui's inline height is immutable after construction; audit #1). The cursor anchor
    /// is preserved by [`RebuildBackend::rebuild`], so the re-placed viewport stays where it was.
    fn resize_viewport(&mut self, height: u16) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // Erase the CURRENT inline region and re-anchor the cursor BEFORE reconstructing at the new
        // height, so the reservation's `append_lines` scrolls BLANKS rather than the prior frame's
        // chrome. On a real terminal this is the whole difference between a clean regrow and the
        // hint-bar/editor-rule/footer STACKING the audit hit; a no-op on fresh-grid backends
        // (`TestBackend`), which start each `rebuild` from a blank buffer and can never stack.
        let size = self.terminal.backend().size().ok();
        let term_h = size.map(|s| s.height).unwrap_or(height).max(1);
        let old_h = self.viewport_height;
        self.terminal.backend_mut().reanchor_inline(term_h, old_h, height);

        let backend = self.terminal.backend().rebuild();
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        self.terminal = terminal;
        Ok(())
    }

    /// Move every newly-committed transcript entry into native scrollback via `Terminal::insert_before`
    /// **exactly once** (R-ARCH-TUI-003 / R-10-002), and — only in test/inspection builds
    /// (`scrollback-accumulator`, TUI-092 F1) — recording the same lines in the `scrollback`
    /// accumulator. After this the inline viewport only renders the active streaming turn,
    /// the editor, and the status line. A no-op when nothing was committed since the last flush.
    fn flush_committed(&mut self) -> Result<(), TuiError> {
        let committed = self.state.transcript.drain_committed();
        if committed.is_empty() {
            return Ok(());
        }
        // Content width for markdown wrapping: the live terminal width (R-ARCH-TUI-005), fallback 80.
        let width = self
            .terminal
            .backend()
            .size()
            .map(|s| s.width)
            .unwrap_or_else(|_| fallback_columns()) as usize;
        let output_pad = self.state.transcript.output_pad();
        // Committed tool-result images keep rendering — a half-block raster is ordinary cells, so it
        // survives `insert_before` into native scrollback (see `ImageBlock::halfblock_lines`).
        // TUI-020 — the hrefs `tool_path_span` registers while `entry_lines` runs, emitted as
        // OSC-8 once the cells exist. Built per flush; empty on a hyperlink-incapable terminal, in
        // which case `osc::inject` returns on its first line.
        let links = crate::osc::LinkSink::new();
        // Built by [`image_opts`], the one definition both renderers share; the reasons the nine
        // fields are read live are recorded there.
        let images = image_opts(&self.state, Some(&links));
        let lines: Vec<Line<'static>> = committed
            .iter()
            .flat_map(|e| entry_lines(e, &self.state.theme, width, output_pad, images))
            .collect();
        #[cfg(any(test, feature = "scrollback-accumulator"))]
        self.state.scrollback.extend(lines.iter().cloned());
        let style = self.state.theme.base_style();
        // Size the scrollback slot to the WRAPPED display-row count (not `lines.len()`) and render
        // WITH `.wrap()`: `entry_lines` emits one un-wrapped `Line` per prose paragraph, so a long
        // committed answer must wrap to width and reserve its wrapped height — otherwise
        // `insert_before` clips it to a single row and the full text is lost from native scrollback
        // (the PROSE-WRAP truncation; R-ARCH-TUI-003/-005, spec/tui/01 §3 overflow).
        let height = crate::transcript::wrapped_height(&lines, width).min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).wrap(Wrap { trim: false }).render(buf.area, buf);
                // AFTER the wrap: the escape must not be present while `Paragraph` measures
                // columns, and the marked cells do not exist until it has written them.
                crate::osc::inject(buf, &links);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }
}
