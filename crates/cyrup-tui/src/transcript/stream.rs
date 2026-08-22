use super::*;

impl TranscriptView {
    /// Append a user message. When `text` is a `<skill …>` block (a `/skill:name` expansion), it is
    /// split into a collapsible `[skill]` invocation message plus the trailing user message, exactly
    /// as Pi renders the `user` role (`parseSkillBlock` → `SkillInvocationMessageComponent` +
    /// `UserMessageComponent`, interactive-mode.ts:3112-3132). Plain text falls through to a single
    /// user entry.
    ///
    /// The leading `Spacer(1)` is gated on `this.chatContainer.children.length > 0` (`:3500`), so
    /// the first message of a fresh session gets none; the answer is frozen into the entry because
    /// the render happens later, after `drain_committed` has already emptied `pending`. The user
    /// message that trails a skill block is added at `:3513-3521` with its own **unconditional**
    /// spacer, so it always carries one.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.bump_render_generation();
        let text = text.into();
        let lead_spacer = self.chat_has_children();
        if let Some(block) = parse_skill_block(&text) {
            self.pending.push(Entry::SkillInvocation {
                name: block.name,
                content: block.content,
                lead_spacer,
            });
            if let Some(user_message) = block.user_message {
                self.pending.push(Entry::User { text: user_message, lead_spacer: true });
            }
        } else {
            self.pending.push(Entry::User { text, lead_spacer });
        }
        // A fresh prompt jumps the active region back to the tail (spec/tui/07 auto-scroll).
        self.scroll_offset = 0;
    }

    /// Page the active region up by `page` visual lines (`PageUp`): reveal earlier streamed/tool/bash
    /// output. Clamped against the content height at render time.
    pub fn page_up(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(page.max(1));
    }

    /// Page the active region down by `page` visual lines (`PageDown`); `0` is the pinned tail.
    pub fn page_down(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(page.max(1));
    }

    /// The current page-scroll offset from the tail (test/inspection access).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Append a chunk of assistant text to the in-flight streaming buffer (R-10-028 streaming).
    pub fn push_assistant_delta(&mut self, delta: &str) {
        self.bump_render_generation();
        match &mut self.streaming {
            Some(buf) => buf.push_str(delta),
            None => self.streaming = Some(delta.to_string()),
        }
    }

    /// Finalize the assistant turn. If `text` is given it replaces the streaming buffer (e.g. the
    /// authoritative terminal message); otherwise the accumulated streaming buffer is committed.
    ///
    /// L3 — the gate is **whitespace-only**, not empty. Pi renders a text block at all only when
    /// `content.text.trim()` is truthy (`assistant-message.ts:107`) and counts it toward
    /// `hasVisibleContent` by the same trimmed test (`:96-98`), so a message of nothing but spaces
    /// produces no `Markdown` child and no leading `Spacer(1)`. Testing `!t.is_empty()` let `"   "`
    /// through and gave it a blank upstream never emits.
    pub fn commit_assistant(&mut self, text: Option<String>) {
        self.bump_render_generation();
        let final_text = text.or_else(|| self.streaming.take());
        self.streaming = None;
        if let Some(t) = final_text
            && !t.trim().is_empty()
        {
            self.pending.push(Entry::Assistant(t));
        }
    }

    /// Drop any in-flight streaming partial without committing (abort, R-10-030). Drops the live
    /// reasoning buffer too — an aborted turn shows neither its partial answer nor its partial
    /// thinking.
    pub fn discard_streaming(&mut self) {
        self.bump_render_generation();
        self.streaming = None;
        self.thinking = None;
    }

    /// Append a streamed chunk of assistant **reasoning** to the in-flight thinking buffer
    /// (`StreamEvent::ThinkingDelta`, provider `stream.rs:413`). Pi renders the thinking blocks of a
    /// turn as their own section (`assistant-message.ts:115-166`), so the buffer is kept apart from
    /// the answer text.
    pub fn push_thinking_delta(&mut self, delta: &str) {
        self.bump_render_generation();
        match &mut self.thinking {
            Some(buf) => buf.push_str(delta),
            None => self.thinking = Some(delta.to_string()),
        }
    }

    /// The current reasoning partial, if a turn is thinking (test/inspection access).
    pub fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
    }

    /// Finalize the turn's reasoning. `text` (the authoritative `thinking` blocks of the terminal
    /// message, coalesced by [`thinking_text`]) replaces the streamed buffer when given; otherwise
    /// the accumulated buffer commits. Whitespace-only reasoning commits nothing, exactly as Pi
    /// skips a run whose trimmed blocks are all empty (`assistant-message.ts:128-130`).
    ///
    /// The `hideThinkingBlock` choice is frozen into the entry here — see [`Entry::Thinking`].
    pub fn commit_thinking(&mut self, text: Option<String>) {
        self.bump_render_generation();
        let final_text = text.or_else(|| self.thinking.take());
        self.thinking = None;
        if let Some(t) = final_text
            && !t.trim().is_empty()
        {
            self.pending.push(Entry::Thinking { text: t, hidden: self.hide_thinking });
        }
    }

    /// Set `hideThinkingBlock` live (Pi `setHideThinkingBlock`, assistant-message.ts:57-62). Affects
    /// the live reasoning block and every entry committed afterwards; already-flushed scrollback is
    /// immutable (see [`Entry::Thinking`]).
    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.bump_render_generation();
        self.hide_thinking = hide;
    }

    /// Whether the reasoning body is collapsed to the `Thinking...` label (test/inspection access).
    pub fn hide_thinking_block(&self) -> bool {
        self.hide_thinking
    }

    /// Pi `setHiddenThinkingLabel(label?)` (`extensions/types.ts:167` @v0.83.0; the interactive body
    /// is `interactive-mode.ts:2118-2129` @v0.84.2, which assigns `label ?? this.defaultHiddenThinkingLabel`
    /// and re-broadcasts to every already-mounted assistant component). `None` restores
    /// [`HIDDEN_THINKING_LABEL`]. See [`Self::hidden_thinking_label`] for why this is paint-time
    /// state rather than a value frozen at commit.
    pub fn set_hidden_thinking_label(&mut self, label: Option<String>) {
        self.bump_render_generation();
        self.hidden_thinking_label = label;
    }

    /// The label a collapsed reasoning block currently renders — the extension's override, else
    /// [`HIDDEN_THINKING_LABEL`]. Read by the shell when flushing committed entries to scrollback,
    /// so a pending entry and the live block cannot disagree.
    pub fn hidden_thinking_label(&self) -> &str {
        self.hidden_thinking_label.as_deref().unwrap_or(HIDDEN_THINKING_LABEL)
    }
}
