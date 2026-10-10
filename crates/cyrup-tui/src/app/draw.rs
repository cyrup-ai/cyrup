use super::*;

use ratatui::layout::Rect;

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
        // The inline flush draws a committed reasoning run as it committed: the rows go into the
        // terminal's scrollback and are the terminal's from then on. The retained document the
        // alternate screen repaints sets the live flag itself (`altscreen::document`).
        thinking: crate::transcript::ThinkingHiding::AsCommitted,
    }
}

/// The rows pi's `headerContainer` contributes to the document (`interactive-mode.ts:622-628`
/// @v1.0.0), fitted to the content `width`. They are the first rows of the fullscreen document, so
/// they scroll away with the conversation; the inline renderer, which has no scrolled document,
/// paints the same two things in bands of its own ([`render_impl::paint_header`],
/// [`render_impl::paint_startup_hints`]).
///
/// Pi builds the header once and keeps it for the session. With the built-in header enabled
/// ([`StartupHeader`], pi's `shouldShowStartupHeader`) it is `[Spacer(1), builtInHeader,
/// Spacer(1)]` (`:1061-1065`), and `setExtensionHeader` swaps a custom header in for the
/// `builtInHeader` child only (`:2515-2531`), so an extension's header keeps the spacers around it.
/// With the built-in header disabled (`quietStartup`) the container holds just an empty `Text`
/// (`:1067-1068`), which a custom header replaces: no spacers.
///
/// The startup block's own framing blanks are part of the block ([`chrome::compact_hint_lines`]).
fn document_header(state: &AppState, width: usize) -> Vec<Line<'static>> {
    let builtin = state.startup_header.is_shown();
    let lines: Vec<Line<'static>> = match state.extension_header.as_deref() {
        Some(content) => {
            let mut lines: Vec<Line<'static>> = content
                .lines()
                .map(|l| Line::from(Span::styled(l.to_string(), state.theme.base_style())))
                .collect();
            if builtin {
                lines.insert(0, Line::default());
                lines.push(Line::default());
            }
            lines
        }
        None if builtin => crate::chrome::compact_hint_lines(
            &state.theme,
            &state.keymap,
            state.editor.keymap_ref(),
            u16::try_from(width).unwrap_or(u16::MAX),
            state.startup_header.details(),
            // The same `getStartupExpansionState()` disjunction the pinned-viewport renderer uses
            // (`interactive-mode.ts:1418-1420`), so the scrolling-document path cannot disagree
            // with it about how tall the header is.
            state.verbose_startup || state.transcript.tool_expanded(),
        ),
        None => Vec::new(),
    };
    // One `Line` is one display row in the document, so a row wider than the viewport is reflowed
    // here rather than truncated by the painter.
    crate::transcript::wrap_all_owned(lines, width.max(1))
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
        // Publish the SCREEN width in the same breath, and for a related reason: the
        // `availableWidth` of `MarkdownTransformContext` (`core/extensions/types.ts:1204`) is read
        // off the render path but CONSUMED at push/commit time, by
        // [`Self::apply_markdown_transformers`] — pi's transform runs inside `Markdown.render()` and
        // is simply handed the live width (`markdown.ts:284-285`), cyrup's cannot. The last drawn
        // width is the honest value there, so this is the frame that records it. See
        // [`AppState::term_cols`].
        self.state.term_cols = term_w;
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
        // CFG-063 — pi's full-redraw reasons, first match wins (`tui-main-screen.ts:330-358`
        // @v0.87.1): the first frame, a terminal width change, a terminal height change, then a
        // live-region height change — each of which repaints the whole inline region rather than
        // diffing it (see `render_debug`'s module doc).
        let full_redraw = render_debug::full_redraw_reason(
            std::mem::replace(
                &mut self.last_frame_size,
                (i32::from(term_w), i32::from(term_h)),
            ),
            (term_w, term_h),
            self.viewport_height,
            desired,
        );
        if let Some(reason) = full_redraw.as_deref() {
            self.render_debug
                .log_redraw(reason, self.viewport_height, desired, term_h);
        }
        if desired != self.viewport_height {
            // TUI-093 — NOT `?`. A frame at the previous height is a cosmetic defect; propagating
            // here unwinds ~40 `draw_synchronized()?` call sites out of `App::run` and ENDS THE
            // SESSION (main.rs `anyhow!("tui: {e}")` → `eprintln!("cyrup: {err:#}")`).
            // `viewport_height` is committed only on success, so the next frame retries the same
            // reconstruction rather than getting stuck believing it already happened.
            match self.resize_viewport(desired) {
                Ok(()) => self.viewport_height = desired,
                Err(e) => self
                    .state
                    .transcript
                    .push_status(format!("viewport resize failed: {e}")),
            }
        }
        let committed_rows = self.flush_committed()?;
        let App {
            terminal,
            state,
            render_debug: debug,
            ..
        } = self;
        let completed = terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        if debug.frame_dump_dir.is_some() {
            let new_lines = render_debug::buffer_lines(completed.buffer);
            let viewport_top = completed.area.y;
            // pi dumps only on the differential path — a full redraw returns before the dump —
            // but `previousLines` moves either way (`fullRender` sets it too).
            if full_redraw.is_none() {
                debug.dump_frame(&render_debug::FrameDump {
                    viewport_top,
                    height: term_h,
                    width: term_w,
                    live_region_height: self.viewport_height,
                    live_floor: self.live_floor,
                    committed_rows,
                    new_lines: &new_lines,
                    previous_lines: &self.debug_previous_lines,
                });
            }
            self.debug_previous_lines = new_lines;
        }
        Ok(())
    }

    /// pi `resetRenderState()` (`tui-main-screen.ts:158-166` @v0.87.1), what `requestRender(true)`
    /// runs before the next frame: drop every piece of diff state so that frame repaints from
    /// scratch. ratatui's diff lives in the `Terminal`'s back buffer, so the reset is
    /// `terminal.clear()`; the render-debug state is reset with it — upstream's `previousWidth =
    /// -1` is what makes that frame log `terminal width changed (-1 -> <width>)` rather than pass as
    /// a diffed frame (CFG-063).
    pub(crate) fn reset_render_state(&mut self) {
        let _ = self.terminal.clear();
        self.last_frame_size = render_debug::RESET_FRAME;
        self.debug_previous_lines.clear();
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
    /// # The frame
    /// The scrolled document (committed entries, then the in-flight turn) over a dock, both laid out
    /// from one [`Regions`] — the same rectangles the inline renderer paints into, so the editor, the
    /// selectors, the band, the widgets and the footer are the very components inline mode draws, in
    /// the same rows, with the document taking whatever the dock leaves. The renderer asks
    /// [`FullscreenChrome`] for that layout and hands the frame back to it for the dock and the
    /// overlays (`altscreen::Chrome`).
    fn draw_fullscreen(&mut self) -> Result<(), TuiError> {
        // Dropped, not rendered: with retention on the same entries are already in
        // `TranscriptView::document`, which is what `sync_document` walks below.
        drop(self.state.transcript.drain_committed());
        {
            // Destructured for the disjoint borrows, as everywhere the renderer and the state are
            // used together (`altscreen/mod.rs`'s rule 1).
            let App {
                altscreen, state, ..
            } = self;
            let Some(alt) = altscreen.as_mut() else {
                return Ok(());
            };
            // Publish the SCREEN geometry before anything measures, as the inline path does: the
            // editor caps itself at `max(5, floor(terminalRows * 0.3))` and a windowed selector
            // sizes against the terminal, not against the rows the dock happens to be given.
            let screen = alt.area();
            state.term_rows = screen.height;
            state.term_cols = screen.width;
            state.editor.set_terminal_height(screen.height);
            if let Some(active) = state.selector.as_mut() {
                active.inner.set_terminal_height(screen.height);
            }
        }
        self.sync_fullscreen_document();
        let App {
            altscreen, state, ..
        } = self;
        let Some(alt) = altscreen.as_mut() else {
            return Ok(());
        };
        alt.draw_with(&mut FullscreenChrome { state })
    }

    /// Bring the alternate screen's retained document up to date with the transcript, the header
    /// and the in-flight turn — [`crate::AltScreen::sync_document`] with everything it takes read
    /// off [`AppState`].
    ///
    /// Called by every frame, and by the input path right after it changes what the document is
    /// made of (a click that toggles an entry), so that the very next hit test is made against the
    /// document the user is about to see rather than the one they just changed. It costs a rebuild
    /// only when [`crate::AltScreen::sync_document`]'s key moved, so the frame that follows finds
    /// nothing left to do.
    pub(crate) fn sync_fullscreen_document(&mut self) {
        let App {
            altscreen, state, ..
        } = self;
        let Some(alt) = altscreen.as_mut() else {
            return;
        };
        let width = usize::from(alt.content_width());
        let header = document_header(state, width);
        let live = state.transcript.live_rows(width, &state.theme);
        alt.sync_document(
            &state.transcript,
            &state.theme,
            image_opts(state, None),
            &header,
            &live,
        );
    }

    /// Rebuild the terminal with a new inline-viewport `height` over a fresh handle to the same
    /// backend (ratatui's inline height is immutable after construction; audit #1). The cursor anchor
    /// is preserved by [`RebuildBackend::rebuild`], so the re-placed viewport stays where it was.
    /// Rebuild the inline viewport at `height`.
    ///
    /// # `[CYRUP-DELTA]` this is pi's `clearOnShrink`, unconditionally (TUI-182)
    ///
    /// pi's renderer diffs into a fixed-height screen and keeps a high-water mark of rendered rows;
    /// `terminal.clearOnShrink` makes a shrink below that mark take `fullRender(true)` instead of the
    /// incremental path, which is what re-pins the editor and footer to the bottom
    /// (`tui-main-screen.ts:356` @v1.1.0). pi defaults it OFF, because its `fullRender` repaints the
    /// whole visible screen — the changelog's "may cause some flicker due to redraws".
    ///
    /// cyrup needs no setting for it. [`App::draw`] content-sizes the live region every frame, so any
    /// shrink changes `desired` and lands here, and this function erases the region and constructs a
    /// new `Terminal` — a full repaint that re-pins by construction. cyrup is therefore permanently
    /// in pi's `clearOnShrink = true` mode, and it is the CHEAP end of that trade: committed
    /// transcript entries have already gone to native scrollback through `insert_before`
    /// ([`Self::flush_committed`]), so what repaints is the live region, never the document pi would
    /// redraw. Honouring `false` would mean not resizing on shrink, i.e. abandoning the
    /// content-sized viewport (ADR-0001 #1) to reintroduce a defect pi offers a switch to escape.
    ///
    /// The setting's other upstream role — reserving the idle status container
    /// (`interactive-mode.ts:2075-2078`) — died with the 2-row band in `TUI-103`. `cyrup-config`
    /// still parses the key so a pi `settings.json` loads and the schema dump still carries it;
    /// nothing in the TUI reads it.
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
        self.terminal
            .backend_mut()
            .reanchor_inline(term_h, old_h, height);

        let backend = self.terminal.backend().rebuild();
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height.max(1)),
            },
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
    ///
    /// Returns how many scrollback rows were inserted (the frame dump's `committedRows`).
    fn flush_committed(&mut self) -> Result<u16, TuiError> {
        let committed = self.state.transcript.drain_committed();
        if committed.is_empty() {
            return Ok(0);
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
        // Size the scrollback slot to the WRAPPED display-row count, not `lines.len()`: a long
        // committed answer must reserve its wrapped height or `insert_before` clips it and the
        // full text is lost from native scrollback (the PROSE-WRAP truncation;
        // R-ARCH-TUI-003/-005, spec/tui/01 §3 overflow).
        //
        // [PERF-005 §3.5] This used to say `entry_lines` emits one un-wrapped `Line` per prose
        // paragraph. That has not been true since `MdRenderer::finish` started wrapping to width
        // (`markdown/walk.rs:835-838`) — most rows arrive already fitting, which is exactly why
        // `wrap_all_owned` MOVES them instead of cloning. The rows that still need wrapping are
        // the ones the inner wrap cannot bound (deeply nested quoted lists at a narrow pane) and
        // the ones that never went through markdown at all (`tool_lines`,
        // `BashExecution::render_lines`).
        //
        // Wrap ONCE, here, and blit the result: this used to measure with `wrapped_height`
        // (a `to_vec` plus a full ratatui `line_count` pass) and then wrap a SECOND time inside
        // `Paragraph::render(.wrap(..))`. `rows.len()` is the same height for free, and it is now
        // the same oracle the live viewport uses (PERF-005 §3.0).
        let rows = crate::transcript::wrap_all_owned(lines, width.max(1));
        let height = rows.len().min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(rows).style(style).render(buf.area, buf);
                // AFTER the rows are written: the escape must not be present while `Paragraph`
                // measures columns, and the marked cells do not exist until it has written them.
                crate::osc::inject(buf, &links);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(height)
    }
}

/// Everything the alternate screen paints that is not the scrolled document: it adapts
/// [`AppState`] to the renderer's [`crate::altscreen::Chrome`] seam.
struct FullscreenChrome<'a> {
    state: &'a mut AppState,
}

impl crate::altscreen::Chrome for FullscreenChrome<'_> {
    fn layout(&mut self, screen: Rect) -> Rect {
        // The header is the first rows of the scrolled document here, not a band above it.
        let regions = Regions::compute_with(self.state, screen, HeaderPlacement::InDocument);
        self.state.regions = regions;
        regions.msg
    }

    fn strip(&self) -> Option<(crate::altscreen::Strip<'_>, Rect)> {
        let state = &*self.state;
        // §B-12's strip, built from the same two `AppState` fields the inline path renders from, so
        // an attachment cannot look different across a mode switch.
        (!state.pending_images.is_empty()).then(|| {
            (
                crate::altscreen::Strip {
                    renderer: &state.image_renderer,
                    blocks: &state.pending_images,
                    theme: &state.theme,
                    show_images: state.transcript.show_images(),
                    width_cells: state.transcript.image_width_cells(),
                },
                state.regions.images,
            )
        })
    }

    fn paint(&mut self, frame: &mut Frame) {
        let regions = self.state.regions;
        // Neither the extension header nor the startup hints are painted here: both are rows of
        // the scrolled document ([`document_header`]), drawn by the renderer with the rest of it.
        render_impl::paint_dock_without_images(frame, self.state, &regions);
        let screen = frame.area();
        render_impl::paint_overlays(frame, self.state, screen);
    }
}
