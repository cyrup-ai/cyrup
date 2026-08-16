use super::*;

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
            self.resize_viewport(desired)?;
            self.viewport_height = desired;
        }
        self.flush_committed()?;
        let App { terminal, state, .. } = self;
        terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
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
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            // TUI-N01 — the committed path reads the same capability the live one does, so a block
            // that scrolled up cannot disagree with the one still on screen.
            graphical: self.state.transcript.graphical_images(),
            width_cells: self.state.transcript.image_width_cells(),
            // X9/X7 — the same live `app.tools.expand` label and session cwd the in-viewport render
            // uses, so a committed block's hints and compact `read` header do not disagree with the
            // live one they were just scrolled up from.
            expand_key: self.state.transcript.expand_key(),
            cwd: self.state.transcript.cwd(),
            // X14 — the LIVE `this.toolOutputExpanded`. Upstream never freezes an expansion onto a
            // message: `setToolsExpanded` walks `chatContainer.children` and re-broadcasts to every
            // expandable child (`interactive-mode.ts:4032-4046`), so a branch/compaction summary
            // pushed while collapsed still opens when `Ctrl+O` is pressed before it paints.
            tools_expanded: self.state.transcript.tool_expanded(),
            // TUI-030 — the LIVE `setHiddenThinkingLabel` override, for the same reason
            // `tools_expanded` is read live here: a reasoning block flushed to scrollback must not
            // disagree with the one still on screen it was scrolled up from.
            hidden_thinking_label: Some(self.state.transcript.hidden_thinking_label()),
        };
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
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }
}
