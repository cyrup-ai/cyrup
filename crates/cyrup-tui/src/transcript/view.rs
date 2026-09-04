use super::*;

impl TranscriptView {
    /// An empty transcript.
    pub fn new() -> Self {
        // Pi's `outputPad` defaults to 1 (settings-manager.ts:1186), `terminal.showImages` to
        // `true` and `terminal.imageWidthCells` to 60 (settings-manager.ts:1047-1066) — none of
        // which a derived `Default` would give, so seed them explicitly.
        TranscriptView {
            output_pad: 1,
            show_images: true,
            graphical_images: true,
            image_width_cells: DEFAULT_IMAGE_WIDTH_CELLS,
            // `markdown.mermaid` is deliberately absent: the derived `Default` is already
            // `MermaidRenderingMode::Streaming`, Pi's own default (settings-manager.ts:61).
            ..TranscriptView::default()
        }
    }

    /// The horizontal output padding (`outputPad`, columns) applied to user/assistant messages — read
    /// by the shell to pass into [`entry_lines`] when flushing committed entries to scrollback.
    pub fn output_pad(&self) -> usize {
        self.output_pad
    }

    /// Set the horizontal output padding live (Pi `onOutputPadChange` → `this.outputPad = padding`,
    /// interactive-mode.ts:4127-4136). The `/settings` "Output padding" row drives this; the live region
    /// re-renders with the new indent on the next draw, and subsequently-committed messages flush with
    /// it (already-scrolled-off native scrollback is immutable, an accepted consequence of the
    /// content-sized-viewport architecture).
    pub fn set_output_pad(&mut self, pad: usize) {
        self.bump_render_generation();
        self.output_pad = pad;
    }

    /// Set `terminal.showImages` (Pi `ToolExecutionComponent.showImages`): rasterize a tool result's
    /// `image` blocks inline, or fall back to Pi's `[Image: …]` text stand-in.
    pub fn set_show_images(&mut self, show: bool) {
        self.bump_render_generation();
        self.show_images = show;
    }

    /// Whether inline tool-result images are on (read by the shell when flushing committed entries).
    pub fn show_images(&self) -> bool {
        self.show_images
    }

    /// Set `markdown.mermaid` live (Pi's transformer reads
    /// `getMode: () => this.settingsManager.getMermaidRenderingMode()` on every render,
    /// `interactive-mode.ts:484-486`, so upstream the `/settings` flip is live by construction).
    /// cyrup caches the mode here, so the row has to push into it.
    pub fn set_mermaid_mode(&mut self, mode: cyrup_config::MermaidRenderingMode) {
        // Required: the render cache is keyed on `(render_generation, width, theme.generation)`,
        // so without the bump a cycled row would not repaint the live region.
        self.bump_render_generation();
        self.mermaid_mode = mode;
    }

    /// The live `markdown.mermaid` mode (read by the shell when flushing committed entries and by
    /// the alternate screen's document key, so both renderers agree with the live region).
    pub fn mermaid_mode(&self) -> cyrup_config::MermaidRenderingMode {
        self.mermaid_mode
    }

    /// Set whether the terminal has a real image protocol (TUI-N01; Pi's `getCapabilities().images`
    /// gate at `tool-execution.ts:331`). Off ⇒ a tool result's `image` blocks take the same
    /// `[Image: …]` text branch `showImages: false` takes, rather than rasterizing anyway.
    pub fn set_graphical_images(&mut self, graphical: bool) {
        self.bump_render_generation();
        self.graphical_images = graphical;
    }

    /// Whether the terminal has a real image protocol (read by the shell when flushing committed
    /// entries, so a committed block and the live one it scrolled up from agree).
    pub fn graphical_images(&self) -> bool {
        self.graphical_images
    }

    /// Set whether the terminal forwards OSC-8 hyperlinks (TUI-020; pi's
    /// `getCapabilities().hyperlinks`, `terminal-image.ts:130-143`). Off ⇒ a `read`/`write`/`edit`/
    /// `ls` header path renders exactly as it does today, with no escape and no ` (url)` suffix —
    /// pi's own `if (!getCapabilities().hyperlinks) return styledText` early return
    /// (`render-utils.ts:20`).
    ///
    /// Bumps the render generation for the same reason [`Self::set_graphical_images`] does: a cache
    /// built before `App::detect_image_support` ran must be discarded, and the marker ids baked into
    /// the cached spans belong to the cached href table.
    pub fn set_hyperlinks(&mut self, hyperlinks: bool) {
        self.bump_render_generation();
        self.hyperlinks = hyperlinks;
    }

    /// Whether the terminal forwards OSC-8 hyperlinks (read by the shell when flushing committed
    /// entries, so a committed header and the live one it scrolled up from agree).
    pub fn hyperlinks(&self) -> bool {
        self.hyperlinks
    }

    /// Set `terminal.imageWidthCells` (Pi `maxWidthCells`): the cell width an inline image is
    /// clamped to. `0` is coerced to 1 so a degenerate setting cannot produce a zero-width raster.
    pub fn set_image_width_cells(&mut self, cells: u16) {
        self.bump_render_generation();
        self.image_width_cells = cells.max(1);
    }

    /// The inline-image cell-width clamp (read by the shell when flushing committed entries).
    pub fn image_width_cells(&self) -> u16 {
        self.image_width_cells
    }

    /// Committed entries not yet flushed to scrollback (test/inspection access).
    pub fn pending(&self) -> &[Entry] {
        &self.pending
    }

    /// The current streaming partial, if a turn is in flight.
    pub fn streaming(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// True while an assistant turn is actively streaming **or** a tool/bash run is live in the viewport.
    pub fn has_active(&self) -> bool {
        self.streaming.is_some()
            || self.thinking.is_some()
            || !self.active_tools.is_empty()
            || self.bash.is_some()
    }

    /// Take every committed entry, leaving the pending buffer empty — and, when retention is on,
    /// ALSO keep a clone of each in the retained document (ADR-0005 §Decision B-1).
    ///
    /// The returned `Vec` is identical in both modes. The **inline** renderer spends it by rendering
    /// those entries into native scrollback exactly once (R-ARCH-TUI-003), which is why they are not
    /// shown again in the inline viewport. That is how the inline renderer disposes of a drain — it
    /// is not a property of the transcript and not a limit on the crate: with
    /// [`Self::set_retain_document`] on, the same entries stay in [`Self::document()`], which is
    /// what the **alternate-screen** renderer scrolls. Upstream keeps its message components alive
    /// in `chatContainer` in both modes and simply wraps `documentContainer` in a `ScrollView`
    /// (`interactive-mode.ts:918`, mounted as the fullscreen layout root at `:933-936` @v0.84.3),
    /// so it needs no such flag.
    ///
    /// With retention OFF — the default, and every regular-mode session — this is the pre-ADR body
    /// exactly: the same `|=` onto `chat_flushed`, the same single `std::mem::take`, the same return
    /// value, with one `bool` test added in front of the new branch. Retention is also the only
    /// place the document grows, so [`MAX_RETAINED_ENTRIES`] is enforced here (via
    /// [`Self::trim_document`]) and nowhere else.
    pub fn drain_committed(&mut self) -> Vec<Entry> {
        self.chat_flushed |= !self.pending.is_empty();
        let committed = std::mem::take(&mut self.pending);
        if self.retain_document {
            self.document.extend_from_slice(&committed);
            self.trim_document();
        }
        committed
    }

    /// Enforce [`MAX_RETAINED_ENTRIES`] on the retained document, dropping the OLDEST entries first
    /// and adding however many it dropped to [`Self::retained_dropped()`] — ADR-0005 §Decision B-1.
    ///
    /// Called only from [`Self::drain_committed`], the document's only growth point, so the bound
    /// cannot be exceeded between frames. There is no pi counterpart: `chatContainer` lives for the
    /// process, so upstream never trims — see [`MAX_RETAINED_ENTRIES`] for why cyrup must.
    fn trim_document(&mut self) {
        let excess = self.document.len().saturating_sub(MAX_RETAINED_ENTRIES);
        if excess == 0 {
            return;
        }
        self.document.drain(..excess);
        self.retained_dropped = self
            .retained_dropped
            .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    }

    /// The retained document: every committed [`Entry`] drained while retention was on, in commit
    /// order, front-trimmed to [`MAX_RETAINED_ENTRIES`] (ADR-0005 §Decision B-1).
    ///
    /// cyrup's stand-in for the `documentContainer` pi's alt screen wraps in a `ScrollView`
    /// (`interactive-mode.ts:918`, `:933-936` @v0.84.3). Empty in every regular-mode session, and
    /// the inline path must not read it — the inline renderer consumes the `Vec`
    /// [`Self::drain_committed`] returns and flushes that to native scrollback (R-ARCH-TUI-003),
    /// which this accessor does not change.
    pub fn document(&self) -> &[Entry] {
        &self.document
    }

    /// Turn retention on or off — ADR-0005 §Decision B-1. `false` is the default and the inline
    /// mode's behaviour; `true` is what gives the alternate-screen renderer something to scroll.
    ///
    /// **Set once, at the composition root, for the session's life** (the `retain_document` field's
    /// documentation states the rule and why). Because cyrup's flag is a filter over drains and
    /// upstream's `chatContainer` is not, turning retention OFF and back ON would splice two
    /// non-adjacent runs of history together with no gap marker and no
    /// [`Self::retained_dropped()`] movement, silently invalidating every row index a renderer
    /// holds. ADR-0005 §B-14's live mode switch therefore leaves this flag alone.
    ///
    /// Deliberately does NOT bump the render generation: the cache this crate keys on that counter
    /// materialises the ACTIVE region (`lines()`), and retention changes nothing a live frame paints.
    pub fn set_retain_document(&mut self, retain: bool) {
        self.retain_document = retain;
    }

    /// Whether drains are retained in [`Self::document()`] (ADR-0005 §Decision B-1).
    pub fn retain_document(&self) -> bool {
        self.retain_document
    }

    /// How many entries have been removed from the FRONT of [`Self::document()`] over the session's
    /// life, by the [`MAX_RETAINED_ENTRIES`] bound or by [`Self::clear_document`] — monotonic, never
    /// reset (ADR-0005 §Decision B-1).
    ///
    /// A renderer records the value it last rebuilt its rows against and shifts its scroll position
    /// by the delta, so a trim scrolls history off the top instead of silently re-aiming the
    /// viewport at unrelated rows. Upstream has no counterpart because it never drops.
    pub fn retained_dropped(&self) -> u64 {
        self.retained_dropped
    }

    /// Drop the retained document — the counterpart of upstream clearing `chatContainer`.
    ///
    /// **Bumps [`Self::retained_dropped()`] by the number of entries removed**, because that counter
    /// is the ONLY signal a renderer has that its cached row offsets moved (ADR-0005 §Decision B-1).
    /// A clear that left it untouched would leave a renderer's recorded value equal, its row rebuild
    /// shifting by zero, and its scroll position pointing into an emptied document — the exact
    /// silent mis-scroll the counter exists to prevent. The counter therefore means "entries removed
    /// from the front over the session's life", not "entries the bound evicted".
    pub fn clear_document(&mut self) {
        let dropped = self.document.len();
        self.document.clear();
        self.retained_dropped = self
            .retained_dropped
            .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
    }

    /// `this.chatContainer.children.length > 0` (`interactive-mode.ts:3500`).
    ///
    /// cyrup's analogue of the chat container is the whole entry stream: everything ever committed —
    /// still in `pending` or already flushed by [`drain_committed`](Self::drain_committed) — plus the
    /// live components the viewport owns while a turn runs. Upstream keeps those live components
    /// (`AssistantMessageComponent`, each `ToolExecutionComponent`, the `!` bash block) in
    /// `chatContainer` too, so they count.
    pub(super) fn chat_has_children(&self) -> bool {
        self.chat_flushed
            || !self.pending.is_empty()
            || self.streaming.is_some()
            || self.thinking.is_some()
            || !self.active_tools.is_empty()
            || self.bash.is_some()
    }

    /// Set the live `app.tools.expand` key label every `… to expand` hint resolves through — Pi's
    /// `keyText("app.tools.expand")` (`keybinding-hints.ts:34-36`), which reads the keymap on every
    /// render. cyrup's transcript holds no keymap, so the app pushes the resolved label here
    /// whenever bindings change (X9). `None` restores cyrup's default binding label.
    pub fn set_expand_hint(&mut self, label: Option<String>) {
        self.bump_render_generation();
        self.expand_hint = label;
    }

    /// The label [`Self::set_expand_hint`] stored, or [`EXPAND_KEY`].
    pub fn expand_key(&self) -> &str {
        self.expand_hint.as_deref().unwrap_or(EXPAND_KEY)
    }

    /// Point the tool renderers at the SESSION's working directory — Pi `ToolRenderContext.cwd`
    /// (`tool-execution.ts:126`), which `read`'s compact classification resolves its path against
    /// (`read.ts:336`). `None` falls back to the process cwd.
    pub fn set_cwd(&mut self, cwd: Option<std::path::PathBuf>) {
        self.bump_render_generation();
        self.cwd = cwd;
    }

    /// The cwd [`Self::set_cwd`] stored.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }

    /// How many entries are committed-but-unflushed right now.
    ///
    /// The extension markdown-transform pass (`App::apply_markdown_transformers`, `app/events.rs`)
    /// snapshots this BEFORE folding an event and walks from it afterwards, so it transforms exactly
    /// the entries that fold produced. Nothing drains between the two reads — the drain happens in
    /// `App::draw`, one run-loop arm later.
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The pending entries at or after `from` that carry one of pi's three markdown message bodies,
    /// as `(index, messageType, text)`.
    ///
    /// The filter IS the port: upstream attaches `createMarkdownTransform` to the `Markdown` child
    /// of `UserMessageComponent` (`user-message.ts:53`) and to the two of `AssistantMessageComponent`
    /// (`assistant-message.ts:112`, `:157-161`), and to nothing else — a `[skill]` block, a custom
    /// message, a `/changelog` block and a tool row all render markdown that no transformer ever
    /// sees. So the three variants below are the whole list, and a new [`Entry`] variant does not
    /// join it by default.
    pub(crate) fn pending_markdown(
        &self,
        from: usize,
    ) -> impl Iterator<Item = (usize, crate::markdown::MessageType, &str)> + '_ {
        use crate::markdown::MessageType;
        self.pending
            .iter()
            .enumerate()
            .skip(from)
            .filter_map(|(i, entry)| match entry {
                Entry::User { text, .. } => Some((i, MessageType::User, text.as_str())),
                Entry::Assistant(text) => Some((i, MessageType::Assistant, text.as_str())),
                Entry::Thinking { text, .. } => {
                    Some((i, MessageType::AssistantThinking, text.as_str()))
                }
                _ => None,
            })
    }

    /// Replace the markdown body of the pending entry at `index` with a transformer's output.
    ///
    /// Rewritten IN PLACE rather than kept beside the source, because these three bodies are
    /// display-only: `transcript/render.rs` is the only reader of each, and the alternate screen's
    /// prompt navigation keys off the entry VARIANT, not its text
    /// (`altscreen/prompt_nav.rs`). No other subsystem re-derives anything from them, so there is
    /// nothing a parallel `display` field would protect. (The LIVE partials are the opposite case
    /// and do get one — see [`Self::set_streaming_display`].)
    ///
    /// A `None` from `get_mut`, or an index that has since been filled by another variant, is a
    /// silent no-op: the pass reads and writes within one `&mut self` borrow of the app, so it
    /// cannot actually happen, and the alternative is an index panic the workspace lints forbid.
    pub(crate) fn set_pending_markdown(&mut self, index: usize, text: String) {
        let Some(entry) = self.pending.get_mut(index) else {
            return;
        };
        match entry {
            Entry::User { text: slot, .. } => *slot = text,
            Entry::Assistant(slot) => *slot = text,
            Entry::Thinking { text: slot, .. } => *slot = text,
            _ => return,
        }
        // `render_generation` contract (see the field's docs in `transcript/mod.rs`): every `&mut
        // self` mutator that changes what `lines()` emits bumps it. Without this the cache would
        // keep serving the pre-transform lines for the rest of the turn.
        self.bump_render_generation();
    }

    /// Publish what the extension markdown transformers made of the live assistant partial, or
    /// `None` to fall back to the raw buffer — see the `streaming_display` field in
    /// `transcript/mod.rs` for why this is a second buffer and not a rewrite of the accumulator.
    ///
    /// A no-op when the value is unchanged — including the common `None` → `None` — so an event
    /// that streams nothing does not invalidate the render cache on a session with a transformer
    /// loaded.
    pub(crate) fn set_streaming_display(&mut self, text: Option<String>) {
        if self.streaming_display == text {
            return;
        }
        self.bump_render_generation();
        self.streaming_display = text;
    }

    /// [`Self::set_streaming_display`] for the live reasoning partial (pi's `"assistant-thinking"`
    /// transform, `assistant-message.ts:156-162`).
    pub(crate) fn set_thinking_display(&mut self, text: Option<String>) {
        if self.thinking_display == text {
            return;
        }
        self.bump_render_generation();
        self.thinking_display = text;
    }
}
