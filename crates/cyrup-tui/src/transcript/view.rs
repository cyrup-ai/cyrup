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


    /// Take every committed entry, leaving the pending buffer empty. The shell renders the returned
    /// entries into native scrollback exactly once (R-ARCH-TUI-003), so they are not shown again in
    /// the inline viewport.
    pub fn drain_committed(&mut self) -> Vec<Entry> {
        self.chat_flushed |= !self.pending.is_empty();
        std::mem::take(&mut self.pending)
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
