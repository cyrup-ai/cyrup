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
    // Pi's `builtInToolDefinition` lookup, asked ONCE so both resolutions below read the same
    // answer (`tool-execution.ts:84-101`).
    let builtin = builtin_kind(&run.name);
    // `hasRendererDefinition()` — `builtInToolDefinition !== undefined || toolDefinition !==
    // undefined` (`tool-execution.ts:103-105`). A registered RENDERER can only have come from a
    // definition, so either rendered side implies one; the rest is what the session's own
    // `getToolDefinition(name)` answered when the run started ([`ToolRun::has_definition`]).
    let has_definition =
        run.definition.is_some() || run.rendered_call.is_some() || run.rendered_result.is_some();
    if builtin.is_none() && !has_definition {
        // The `else` of `hasRendererDefinition()`: the unbounded `formatToolExecution()`
        // (`tool-execution.ts:330-333`). Nothing at all is known about this tool name.
        render_generic(run, theme, &mut block);
    } else {
        // EXT-006 / `updateDisplay` (`tool-execution.ts:272-330`): the call side and the result
        // side are resolved SEPARATELY, each preferring the extension's renderer, then the
        // built-in's, then the matching fallback. Resolving them together is what used to leave an
        // extension that registered only `renderCall` with no body at all, and what sent every
        // defined-but-unrendered tool through `formatToolExecution`.
        match &run.rendered_call {
            Some(call) => render_extension_call(call, theme, &mut block),
            None => match builtin {
                Some(kind) => render_builtin_call(kind, run, expanded, theme, images, &mut block),
                // `createCallFallback()` (`:137-139`, selected at `:281-283`).
                None => render_call_fallback(run, theme, &mut block),
            },
        }
        match &run.rendered_result {
            Some(result) => render_extension_result(result, run, expanded, theme, &mut block),
            None => match builtin {
                Some(kind) => render_builtin_result(kind, run, expanded, theme, images, &mut block),
                // `createResultFallback()` (`:141-155`, selected at `:298-304`). Upstream reaches
                // it only inside `if (this.result)` (`:295`); the fallback makes the same check.
                None => render_result_fallback(run, expanded, theme, images.expand_key, &mut block),
            },
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
    // `applyLineResets` (`tui.ts:1160-1168`) — pi normalizes every finished row of the frame, and
    // separately every child of the tool-execution component is a `Text`/`Box`/`Markdown`
    // (`tool-execution.ts:153-155`), all three of which expand tabs for themselves (`text.ts:61`,
    // `markdown.ts:298`). cyrup's per-tool renderers push bare `Line`s instead of components, so
    // this is where both of those upstream layers land: without it a literal tab in grep/ls/bash or
    // extension output is DELETED by ratatui's control-grapheme filter rather than rendered.
    // Idempotent over the `read`/`write` rows that already went through `replace_tabs`, and applied
    // before the raster rows are appended below so an image line is never walked (`tui.ts:1163`'s
    // `isImageLine` guard).
    for line in &mut block {
        normalize_line(line);
    }
    // EXT-024 — `getRenderShell()` (`tool-execution.ts:108-116` @v0.84.4):
    //
    // ```ts
    // if (!this.builtInToolDefinition) return this.toolDefinition?.renderShell ?? "default";
    // if (!this.toolDefinition) return this.builtInToolDefinition.renderShell ?? "default";
    // return this.toolDefinition.renderShell ?? this.builtInToolDefinition.renderShell ?? "default";
    // ```
    //
    // `run.definition` is the session registry's answer (`getToolDefinition(name)`), which in
    // cyrup already merges the built-ins with every custom and extension tool, so the first tier
    // is one read. The second tier — `builtInToolDefinition.renderShell` — is reached only when
    // no registry was asked (the id-less constructors, a result whose start was missed), and the
    // built-in table answers it: `edit` is the one built-in that declares `renderShell: "self"`
    // (`core/tools/edit.ts:330`; `cyrup-tools/src/tools/edit.rs` `render_kind`), every other
    // built-in leaves it unset.
    let shell = match run.definition {
        Some(kind) => kind,
        None => match builtin {
            Some(Builtin::Edit) => ToolRenderKind::SelfRendered,
            _ => ToolRenderKind::Default,
        },
    };
    // `if (this.hasRendererDefinition() && this.getRenderShell() === "self")` (`:237`) — the
    // `hasRendererDefinition()` half is implied: `shell` can only be `SelfRendered` through a
    // definition or the built-in table, each of which satisfies it.
    if shell == ToolRenderKind::SelfRendered {
        return self_rendered_lines(run, builtin, block, inline, width, theme, images);
    }
    // The DEFAULT shell: `this.contentBox = new Box(1, 1, bgFn)` (`:71`), tinted by execution
    // state (`updateDisplay`, `:265-269` — `toolPendingBg` while partial, else `toolErrorBg` /
    // `toolSuccessBg`), preceded by the untinted `Spacer(1)` the constructor adds (`:66`).
    let bg = theme.tool_bg_style(Style::default(), run.done, run.is_error);
    let mut out = vec![Line::default()];
    out.extend(finalize_block(block, width, bg));
    if inline {
        out.extend(image_raster_lines(run, width, images.width_cells));
    }
    out
}

/// The `renderShell: "self"` block — `ToolExecutionComponent.render()`'s first branch
/// (`tool-execution.ts:237-259` @v0.84.4):
///
/// ```ts
/// if (this.hasRendererDefinition() && this.getRenderShell() === "self") {
///     const contentLines = this.selfRenderContainer.render(width);
///     if (contentLines.length === 0 && this.imageComponents.length === 0) return [];
///     const lines: string[] = [];
///     if (contentLines.length > 0) { lines.push(""); lines.push(...contentLines); }
///     for (…) { lines.push(...spacer.render(width)); lines.push(...imageComponent.render(width)); }
///     return lines;
/// }
/// ```
///
/// `selfRenderContainer` is a bare `Container` (`:73`), so `updateDisplay`'s `if (renderContainer
/// instanceof Box) renderContainer.setBgFn(bgFn)` (`:275-277`) skips it: **no** padding column,
/// **no** state tint, and no blank row when the renderers drew nothing. The tool owns its framing.
///
/// The one built-in that declares `"self"` is `edit`, and its framing is the `EditCallRenderComponent`
/// itself — a `Box(1, 1)` whose fill is `getEditHeaderBg(preview, settledError)` (`edit.ts:258-273`,
/// applied at `:281`), which tests the PREVIEW first and never looks at `done`: a diff computed from
/// the streamed arguments greens the block while the call is still pending, and a preview that
/// failed reds it (X8). cyrup's built-in `edit` renderers push bare rows, so that component's box
/// is drawn here, around them — the same rows the shell tail used to paint under the `edit`
/// special case, now attributed to the renderer that owns them. An extension renderer that takes
/// `edit` over (`rendered_call`/`rendered_result`) replaces the component, box and all, exactly as
/// `toolDefinition.renderCall ?? builtInToolDefinition.renderCall` (`:84-92`) would.
fn self_rendered_lines(
    run: &ToolRun,
    builtin: Option<Builtin>,
    block: Vec<Line<'static>>,
    inline: bool,
    width: usize,
    theme: &UiTheme,
    images: ImageOpts,
) -> Vec<Line<'static>> {
    let own_edit_component = builtin == Some(Builtin::Edit)
        && run.rendered_call.is_none()
        && run.rendered_result.is_none();
    let content: Vec<Line<'static>> = if own_edit_component {
        let bg = theme.edit_bg_style(Style::default(), edit_header_preview(run), run.is_error);
        box_lines(block, width, 1, 1, bg)
    } else {
        // `Container.render(width)`: each child at the full width, wrapped as `Text` wraps
        // (`text.ts:60-87`), with nothing added around it.
        block.iter().flat_map(|l| wrap_line(l, width)).collect()
    };
    if content.is_empty() && !inline {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(content.len() + 1);
    if !content.is_empty() {
        out.push(Line::default());
        out.extend(content);
    }
    if inline {
        // `image_raster_lines` emits the per-image `Spacer(1)` + raster pair (`:248-257`).
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
    /// The LIVE `markdown.mermaid` mode (Pi's transformer closure re-reads
    /// `getMermaidRenderingMode()` on every render, `interactive-mode.ts:484-486`), for the same
    /// reason `tools_expanded` and `hidden_thinking_label` are here: a `/settings` flip must reach
    /// the committed entries this bag renders, not only the live region. Defaults to
    /// [`cyrup_config::MermaidRenderingMode::Streaming`], Pi's documented default
    /// (`settings-manager.ts:61`).
    pub mermaid: cyrup_config::MermaidRenderingMode,
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
            mermaid: cyrup_config::MermaidRenderingMode::default(),
        }
    }
}
