use super::*;

impl TranscriptView {
    /// Invalidate the render cache. Called by every mutator on the bump list below; the next
    /// [`cached_render`](Self::cached_render) misses on the generation key and recomputes once.
    /// `wrapping_add`: a plain `+ 1` is a debug-build overflow panic (denied lints aside) after
    /// 2^64 bumps; wrapping can only alias a cache entry built 2^64 generations ago, which no
    /// session survives to observe.
    pub(super) fn bump_render_generation(&mut self) {
        self.render_generation = self.render_generation.wrapping_add(1);
    }

    /// Invalidate the render cache for a timer-driven repaint (TUI-092 F2): the live `!`/`!!`
    /// block's spinner glyph ([`BashExecution::render_lines`] → `started.elapsed()`,
    /// bash.rs:204) and a running bash tool's `Elapsed …` footer (`render_bash_result`,
    /// transcript/tool_builtin.rs) are computed from wall-clock time INSIDE `lines()`, so a frame that
    /// mutates nothing must still re-materialise while time-derived content is live. Called by
    /// the run loop's spinner tick (gated on `bash_running()`) and elapsed tick (gated on
    /// `has_running_elapsed_tool()`) — never on content-quiet frames, which stay free.
    pub fn bump_render_tick(&mut self) {
        self.bump_render_generation();
    }

    /// The materialised active region, recomputed only when (generation, width, theme) changed.
    /// The borrows are strictly sequential — check, build, assign, lend — so this is plain NLL,
    /// no Polonius case, and the final reborrow needs no `unwrap`/`expect` (no-panic policy):
    /// `render_cache` is a value, not an `Option`.
    pub(super) fn cached_render(&mut self, width: usize, theme: &UiTheme) -> &RenderCache {
        let stale = self.render_cache.generation != self.render_generation
            || self.render_cache.width != width
            || self.render_cache.theme_generation != theme.generation;
        if stale {
            // TUI-020 — the href table `tool_path_span` fills while `lines` runs. Built with the
            // lines and stored with them, because the marker ids the spans carry index THIS table.
            let links = crate::osc::LinkSink::new();
            let lines = self.lines_with(width, theme, Some(&links)); // the current body, unchanged
            // Measure WRAPPED display rows, not logical lines: `markdown::render` emits ONE
            // un-wrapped `Line` per prose paragraph, so counting `lines.len()` under-counts a long
            // streaming paragraph. `wrapped_height` measures with the SAME word-wrap `render`
            // applies (`Paragraph::line_count`). (Moved verbatim from the old `content_height`
            // body, transcript.rs:1138-1143.)
            let wrapped_height = wrapped_height(&lines, width);
            self.render_cache = RenderCache {
                generation: self.render_generation,
                width,
                theme_generation: theme.generation,
                lines,
                wrapped_height,
                links,
            };
        }
        &self.render_cache
    }

    /// The number of visual lines the active turn occupies at `width` — the message region's content
    /// height, used to **content-size** the inline viewport (ADR-0001 commitment #1, audit #1) so the
    /// empty turn never balloons into a void. `0` when nothing is streaming.
    pub fn content_height(&mut self, width: usize, theme: &UiTheme) -> usize {
        self.cached_render(width, theme).wrapped_height
    }

    /// Build the styled lines the inline viewport renders: **only** the active streaming partial,
    /// rendered as markdown (spec/tui/06 §8). Committed entries live in native scrollback (see
    /// [`drain_committed`](Self::drain_committed)).
    ///
    /// Pi renders the in-flight assistant message **inline** with no surrounding box/title
    /// (`assistant-message.ts:84-93`) and with **no** streaming caret — the only caret in the TUI is
    /// the editor's reverse-video cell (`editor.ts:545-564`), and `git grep "▌" v0.84.1 --
    /// packages/` finds a single hit, the pupil of an eye in
    /// `examples/extensions/custom-header.ts:22` (X1). The buffer is run through
    /// [`trim_partial_closing_fence`](crate::markdown::trim_partial_closing_fence) so a streaming code
    /// fence does not flicker open/closed (`markdown.ts:25-48`).
    ///
    /// Test-only since TUI-020: the production caller is [`Self::cached_render`], which owns an
    /// href sink and therefore goes straight to [`Self::lines_with`]. The tests that only read the
    /// text back keep this two-argument shape.
    #[cfg(test)]
    pub(super) fn lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        self.lines_with(width, theme, None)
    }

    /// The styled lines of the active region, with the OSC-8 href sink the caller owns (TUI-020) —
    /// the body [`Self::cached_render`] materialises. `None` renders the same lines with no link
    /// marked: the shape every caller that does not own the resulting `Buffer` wants, and the shape
    /// the test-only `lines()` wrapper above keeps for the tests that only read text back.
    pub(super) fn lines_with(
        &self,
        width: usize,
        theme: &UiTheme,
        links: Option<&crate::osc::LinkSink>,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // The live reasoning block renders ABOVE the answer text, the order Pi's content walk
        // produces for a reasoning model (thinking blocks precede the text blocks of the turn) —
        // `assistant-message.ts:115-166`.
        // L3 — `assistant-message.ts:100-102`'s single `Spacer(1)`, emitted once for the whole
        // message when `hasVisibleContent`. The live view is the same component mid-stream, so the
        // blank belongs to whichever of the two blocks comes first.
        //
        // `hasVisibleContent` (`:96-98`) is
        // `content.some(c => (c.type === "text" && c.text.trim()) || (c.type === "thinking" &&
        // c.thinking.trim()))` — a **trimmed** test on both legs, and `:107` gates the text block's
        // `Markdown` child on the same `content.text.trim()`. So an assistant turn that has streamed
        // nothing but whitespace is not visible content: no blank, and no body either.
        let thinking_visible = self.thinking.as_ref().is_some_and(|t| !t.trim().is_empty());
        let stream_visible = self.streaming.as_ref().is_some_and(|s| !s.trim().is_empty());
        if thinking_visible || stream_visible {
            lines.push(Line::default());
        }
        // `:129-131` — `if (thinkingBlocks.length === 0) { continue; }` runs BEFORE the
        // `hideThinkingBlock` branch, and `:122-125` only collects blocks whose `.trim()` is
        // non-empty. So a reasoning run of nothing but whitespace renders nothing at all, not even
        // the `Thinking...` label. `commit_thinking` already applies that test; the live leg did
        // not, so a stray whitespace `ThinkingDelta` put a label (and its blank) on screen.
        // No mermaid context is plumbed through `thinking_lines`, and that is still right: pi's
        // MERMAID gate returns the markdown untouched whenever
        // `context.messageType === "assistant-thinking"` (`mermaid.ts:65`), so for that transformer
        // the thinking path — here and in `transcript/message.rs` — is an unconditional
        // pass-through and a context there would be dead weight.
        //
        // The GENERAL, extension-supplied transformers do run on it (`"assistant-thinking"`,
        // `assistant-message.ts:156-162`) — but not from here: they are applied at push time by
        // `App::apply_markdown_transformers` (`app/events.rs`) and land in `thinking_display`, which
        // is what this renders when the pass has produced anything. Falling back to the raw buffer
        // keeps the frame between a delta and the next pass drawing untransformed text rather than
        // none.
        let thinking_body = self.thinking_display.as_deref().or(self.thinking.as_deref());
        if let Some(thinking) = thinking_body.filter(|_| thinking_visible) {
            let mut td = thinking_lines(
                thinking,
                self.hide_thinking,
                width.saturating_sub(self.output_pad * 2),
                theme,
                self.hidden_thinking_label(),
            );
            if !td.is_empty() {
                pad_lines(&mut td, self.output_pad);
                lines.extend(td);
                // Pi's `hasVisibleContentAfter` spacer (`:134-137`): a blank only when more visible
                // assistant content follows — and "visible" is the same trimmed test (`:137`).
                if stream_visible {
                    lines.push(Line::default());
                }
            }
        }
        // Same `*_display`-then-raw fallback as the reasoning block above: the extension
        // transformers' `is_streaming = true` output (`assistant-message.ts:111`) when the pass has
        // produced one, the accumulator otherwise. `stream_visible` deliberately stays keyed on the
        // RAW buffer — upstream's `hasVisibleContent` tests `content.text.trim()`, the untransformed
        // message content (`assistant-message.ts:96-98`), so a transformer cannot conjure or erase
        // the leading `Spacer(1)`.
        let stream_body = self.streaming_display.as_deref().or(self.streaming.as_deref());
        if let Some(partial) = stream_body.filter(|_| stream_visible) {
            // X1 — no role label and no `▌` caret. `assistant-message.ts:104-114` adds exactly one
            // child per text block, `new Markdown(content.text.trim(), this.outputPad, 0, …)`; the
            // only caret in the whole TUI is the editor's reverse-video cell (`editor.ts:545-564`).
            // `git grep "▌" v0.84.1 -- packages/` finds one hit, and it is the pupil of an eye in
            // `examples/extensions/custom-header.ts:22`.
            //
            // M9 — the width follows from that: `markdown.ts:284` `contentWidth = width -
            // paddingX * 2`, i.e. `width - outputPad * 2`, where the old `width - (11 + outputPad)`
            // was budgeting for the deleted `"assistant: "`.
            let body = crate::markdown::trim_partial_closing_fence(partial);
            // The ONLY `is_streaming: true` markdown site — `assistant-message.ts:112` passes
            // `createMarkdownTransform("assistant", this.isStreaming, …)`, and `isStreaming` is
            // true exactly while the turn streams. It is what makes `markdown.mermaid = "final"`
            // hold the raw fence here and draw the diagram once the turn commits (`mermaid.ts:66`).
            let mut md = crate::markdown::render_message(
                &body,
                width.saturating_sub(self.output_pad * 2).max(1),
                theme,
                None,
                crate::markdown::MermaidContext::new(
                    self.mermaid_mode,
                    crate::markdown::MessageType::Assistant,
                    true,
                ),
            );
            if md.is_empty() {
                md.push(Line::default());
            }
            pad_lines(&mut md, self.output_pad);
            lines.extend(md);
        }
        // Live tool executions render below the streaming partial, honoring the expand flag so
        // `Ctrl+O` toggles their result body in the viewport before the turn commits.
        for run in &self.active_tools {
            lines.extend(tool_lines(
                run,
                self.tool_expanded,
                width,
                theme,
                ImageOpts {
                    show: self.show_images,
                    graphical: self.graphical_images,
                    width_cells: self.image_width_cells,
                    expand_key: self.expand_key(),
                    cwd: self.cwd.as_deref(),
                    hyperlinks: self.hyperlinks,
                    links,
                    tools_expanded: self.tool_expanded,
                    // A tool block draws no reasoning, so this is inert here — carried only so the
                    // bag has one construction shape.
                    hidden_thinking_label: None,
                    // Likewise inert: a tool block is not one of pi's three markdown message
                    // bodies, so no mermaid transformer reaches it upstream.
                    mermaid: self.mermaid_mode,
                },
            ));
        }
        // The live `!`/`!!` bash block renders last (`bash-execution.ts` sits in the message region).
        if let Some(b) = &self.bash {
            lines.extend(b.render_lines(
                width,
                theme,
                self.bash_cancel_hint.as_deref(),
                self.bash_expand_hint.as_deref(),
            ));
        }
        lines
    }
}

impl Component for TranscriptView {
    /// Render the active turn **inline** (no box/title — `assistant-message.ts:84-93`, spec/tui/01 §3):
    /// the streaming partial is a wrapped `Paragraph` filling the region, tail-anchored so the newest
    /// text stays visible as it grows (spec/tui/01 §3 overflow).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let width = area.width as usize;
        let (total, lines) = {
            let cache = self.cached_render(width, theme);
            (cache.lines.len(), cache.lines.clone())
        };
        // Auto-scroll: keep the tail (newest text) visible when content exceeds the region height,
        // minus any user page-up offset (clamped so it can never scroll past the top).
        let inner_h = area.height as usize;
        let max_scroll = total.saturating_sub(inner_h);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        let scroll = max_scroll.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16;
        let para = Paragraph::new(lines)
            .style(theme.base_style())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, area);
        // TUI-020 — same ordering rule as the scrollback flush: inject only once the cells exist.
        // `ForcedWidth` keeps `Buffer::diff_iter` advancing one column per escaped cell, so the
        // frame-to-frame diff that drives the live viewport stays aligned.
        crate::osc::inject(frame.buffer_mut(), &self.render_cache.links);
    }
}
