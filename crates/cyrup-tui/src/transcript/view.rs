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
}
