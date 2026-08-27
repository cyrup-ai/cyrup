use super::*;

/// The **fallback** `app.tools.expand` label, used only when the caller supplied no live one —
/// cyrup's own default binding (`keymap.rs:378`, `Key::ctrl('o')`).
///
/// X9: this used to be the label itself, hard-coded at every hint site, so a user who rebound
/// `app.tools.expand` still read `ctrl+o` on screen. Pi resolves the binding at render time
/// (`keyText(keybinding)` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-36`); the live
/// label now rides in on [`ImageOpts::expand_key`].
pub(super) const EXPAND_KEY: &str = "ctrl+o";

/// Render one tool execution into styled lines by dispatching on the tool name to its Pi-specific
/// `renderCall`/`renderResult` (`tool-execution.ts` composes each built-in's renderers, not a generic
/// one-liner): edit → a self-diff (`edit.ts:390`), bash → an output tail + truncation + `Took …`
/// (`bash.ts:440`), read → a line-range header + a hidden-until-expanded body (`read.ts:329/339`),
/// write → a content preview (`write.ts:227`), grep/find/ls → a match/entry list with limit notices
/// (`grep.ts:370`, `find.ts:359`, `ls.ts:210`). The whole block is tinted by execution state
/// (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`, tool-execution.ts:253-258) — the bg is the state
/// affordance (Pi has no gear/check glyph), preceded by an untinted blank (the component's `Spacer(1)`,
/// tool-execution.ts:63).
pub(crate) fn tool_lines(
    run: &ToolRun,
    expanded: bool,
    width: usize,
    theme: &UiTheme,
    images: ImageOpts,
) -> Vec<Line<'static>> {
    let mut block: Vec<Line<'static>> = Vec::new();
    // EXT-006: an extension that registered a renderer for THIS tool name owns the block (Pi
    // prefers the extension's `renderCall`/`renderResult` over the built-in's,
    // tool-execution.ts:81-112). Checked before the built-in dispatch so an extension can also
    // override how a BUILT-IN tool draws, exactly as Pi's definition-registry override does.
    if run.rendered_call.is_some() || run.rendered_result.is_some() {
        render_extension(run, expanded, theme, &mut block);
    } else {
        match run.name.as_str() {
            "read" => render_read(run, expanded, theme, images, &mut block),
            "write" => render_write(run, expanded, theme, images, &mut block),
            "edit" => render_edit(run, theme, images, &mut block),
            "bash" => render_bash(run, expanded, theme, images.expand_key, "$", &mut block),
            "powershell" => render_bash(run, expanded, theme, images.expand_key, "PS>", &mut block),
            "grep" => render_grep(run, expanded, theme, images.expand_key, &mut block),
            "find" => render_find(run, expanded, theme, images.expand_key, &mut block),
            "ls" => render_ls(run, expanded, theme, images, &mut block),
            _ => render_generic(run, theme, &mut block),
        }
    }
    // `image` content blocks (`tool-execution.ts:330-350`). Pi adds a real `Image` component per
    // block when `caps.images && showImages`, and otherwise `getTextOutput` appends the
    // `imageFallback` indicator to the text body (render-utils.ts:49-59). The two cases split around
    // `finalize_block` because a half-block raster must NOT get the tool block's background tint
    // patched over its cells — matching Pi, whose images are siblings of the tool box, not children.
    // TUI-N01 — the gate must consult the terminal's image CAPABILITY, not just `showImages` and
    // decodability. Upstream is `const caps = getCapabilities(); … if (caps.images && this.showImages
    // && img.data && img.mimeType)` (`components/tool-execution.ts:331-334` @v0.83.0): no protocol
    // means no `Image` child at all, and `getTextOutput` supplies the one-line `imageFallback`. On a
    // plain xterm, the Linux console, CI or a pipe, a `read` of a screenshot used to dump ~20-30 rows
    // of coloured `▀` into scrollback where pi prints one `[Image: …]` line.
    let inline = images.graphical
        && images.show
        && !run.images.is_empty()
        && run.images.iter().all(|i| i.block.is_some());
    if !inline {
        push_image_fallbacks(run, theme, &mut block);
    }
    // The block is state-tinted (bg-only); a leading untinted blank stands in for the component Spacer.
    //
    // X8 — `edit` is the one tool whose tint is NOT the shared `done`/`is_error` one. Pi gives it
    // `getEditHeaderBg(component.preview, component.settledError)` (`edit.ts:239-253`, applied at
    // `:262`), which tests the PREVIEW first and never looks at `done`: a preview diff computed from
    // the streamed arguments greens the block while the call is still pending, and a preview that
    // failed reds it.
    let bg = if run.name == "edit" && run.rendered_call.is_none() && run.rendered_result.is_none() {
        theme.edit_bg_style(Style::default(), edit_header_preview(run), run.is_error)
    } else {
        theme.tool_bg_style(Style::default(), run.done, run.is_error)
    };
    let mut out = vec![Line::default()];
    out.extend(finalize_block(block, width, bg));
    if inline {
        out.extend(image_raster_lines(run, width, images.width_cells));
    }
    out
}

/// The per-frame render inputs a tool block needs that are not on the [`ToolRun`] itself — Pi's
/// `ToolRenderContext` (`extensions/types.ts`, built at `tool-execution.ts:116-135`), narrowed to
/// the three fields cyrup's built-ins actually read.
///
/// `show`/`width_cells` are `terminal.showImages` / `terminal.imageWidthCells` (Pi's
/// `maxWidthCells`). `expand_key` and `cwd` are `context.expanded`'s companions: the live
/// `app.tools.expand` label every `… to expand` hint resolves through (`keyText`,
/// `keybinding-hints.ts:34-36`) and `context.cwd`, which `read`'s compact classification resolves
/// its path against (`read.ts:336`, `resolveToCwd(rawPath, cwd)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageOpts<'a> {
    pub show: bool,
    /// Whether the terminal negotiated a real image protocol — Pi's `getCapabilities().images`
    /// (`tool-execution.ts:331`). TUI-N01: fed from `AppState::image_renderer.is_graphical()`.
    /// Defaults to `true` so a test constructing `ImageOpts::default()` still exercises the inline
    /// path, which is the branch the raster tests are about.
    pub graphical: bool,
    pub width_cells: u16,
    /// The live `app.tools.expand` label; [`EXPAND_KEY`] when the caller has no keymap in hand.
    pub expand_key: &'a str,
    /// Pi `ToolRenderContext.cwd` — the SESSION's working directory, not necessarily the process's.
    /// `None` falls back to the process cwd.
    pub cwd: Option<&'a std::path::Path>,
    /// `getCapabilities().hyperlinks` (`render-utils.ts:20`) — whether the controlling terminal
    /// forwards OSC-8. Threaded rather than read from `crate::image::hyperlinks_supported()` for
    /// the same reason `graphical` is (TUI-N01/TUI-N11): the global falls back to an env sniff, so
    /// reading it here would make every header assertion depend on the developer's `TERM_PROGRAM`.
    pub hyperlinks: bool,
    /// Where [`tool_path_span`] registers an href for [`crate::osc::inject`] to emit. `None` on any
    /// path that does not own the resulting `Buffer` — the link is then simply not marked.
    pub links: Option<&'a crate::osc::LinkSink>,
    /// X14 — the LIVE `this.toolOutputExpanded` (`interactive-mode.ts:442`), the flag `Ctrl+O` /
    /// `setToolsExpanded` drive. Upstream never stores an expansion on a message: it seeds each
    /// component from this field at construction (`:3486`, `:3493`) and re-broadcasts to every
    /// `chatContainer` child on each toggle (`:4032-4046`), so the value in force at PAINT time is
    /// what renders. The branch/compaction summary arms of [`entry_lines`] read it here for exactly
    /// that reason. Defaults to `false`, Pi's own initial value.
    pub tools_expanded: bool,
    /// The LIVE `this.hiddenThinkingLabel` (`interactive-mode.ts:436`), for exactly the reason
    /// `tools_expanded` above is here: upstream never freezes it onto a message, it re-broadcasts to
    /// every mounted assistant component on each `setHiddenThinkingLabel` (`:2118-2129` @v0.84.2), so the
    /// value in force at PAINT time is what renders. `None` ⇒ [`HIDDEN_THINKING_LABEL`].
    ///
    /// (This struct's name has been narrower than its contents since `expand_key`/`cwd`/
    /// `tools_expanded` joined it; it is the per-paint bag for everything an [`Entry`] cannot carry
    /// on itself.)
    pub hidden_thinking_label: Option<&'a str>,
}

impl Default for ImageOpts<'_> {
    fn default() -> Self {
        ImageOpts {
            show: true,
            graphical: true,
            width_cells: DEFAULT_IMAGE_WIDTH_CELLS,
            expand_key: EXPAND_KEY,
            cwd: None,
            // Pi's own conservative default (`terminal-image.ts:130-134`), which also keeps every
            // existing `ImageOpts::default()` construction on the plain-text branch.
            hyperlinks: false,
            links: None,
            tools_expanded: false,
            hidden_thinking_label: None,
        }
    }
}
