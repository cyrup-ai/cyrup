use super::*;

/// Pure render: lay out conversation / editor / status and render each component (`state -> frame`).
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let [
        header_h,
        msg_h,
        pending_h,
        band_h,
        images_h,
        wabove_h,
        slot_h,
        popup_h,
        wbelow_h,
        footer_h,
    ] = region_constraints(state, area.width, area.height);
    let _ = msg_h; // the message region absorbs the remainder via `Min(0)` below.
    let [
        header_area,
        msg_area,
        pending_area,
        band_area,
        images_area,
        wabove_area,
        slot_area,
        popup_area,
        wbelow_area,
        status_area,
    ] = Layout::vertical([
        // TUI-033 — `headerContainer` is docked above `chatContainer` (`interactive-mode.ts:709`).
        Constraint::Length(header_h),
        // `Min(0)` (not the old `Min(1)`): the empty turn must not balloon the viewport (audit #1).
        Constraint::Min(0),
        Constraint::Length(pending_h),
        Constraint::Length(band_h),
        Constraint::Length(images_h),
        // TUI-014 — `widgetContainerAbove`, immediately before `editorContainer` (`:715-716`).
        Constraint::Length(wabove_h),
        Constraint::Length(slot_h),
        Constraint::Length(popup_h),
        // TUI-014 — `widgetContainerBelow`, immediately after `editorContainer` (`:717`).
        Constraint::Length(wbelow_h),
        Constraint::Length(footer_h),
    ])
    .areas(area);
    if header_h > 0
        && let Some(content) = state.extension_header.as_deref()
    {
        let lines: Vec<Line<'static>> = content
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), state.theme.base_style())))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).style(state.theme.base_style()),
            header_area,
        );
    }
    state.transcript.render(frame, msg_area, &state.theme);
    // The compact startup-help block (`compactInstructions` + `compactOnboarding` + `onboarding`,
    // the startup `ExpandableText`'s collapsed body at interactive-mode.ts:936-957, framed by
    // `Spacer(1)` at `:960-962`) occupies the bottom rows of the otherwise-empty message area at
    // startup — just above the editor — sourced from the live keymap so rebinds reflect. It is
    // suppressed once a submission lands (`show_startup_hints` cleared) and while a selector owns
    // the slot, so it never shifts the editor/footer geometry. `render_compact_hints` degrades the
    // block from its edges inward when `rows` is short of the block's wrapped height, so the hint
    // bar itself survives down to a single row.
    if state.show_startup_hints
        && state.selector.is_none()
        && !state.transcript.has_active()
        && msg_area.height >= 1
    {
        let rows = crate::chrome::compact_hint_height(&state.theme, &state.keymap, msg_area.width)
            .min(msg_area.height);
        let hint_row = ratatui::layout::Rect {
            x: msg_area.x,
            y: msg_area.y.saturating_add(msg_area.height - rows),
            width: msg_area.width,
            height: rows,
        };
        crate::chrome::render_compact_hints(frame, hint_row, &state.theme, &state.keymap);
    }
    if pending_h > 0 {
        // `getAppKeyDisplay("app.message.dequeue")` (`interactive-mode.ts:3987`) — `keyDisplayText`,
        // so ALL bound keys joined with `/` and title-cased (`keybinding-hints.ts:29-40`).
        let dequeue = state
            .keymap
            .keys_label(Action::Dequeue)
            .map(|k| crate::chrome::format_key_text(&k, true));
        state
            .pending_messages
            .render(frame, pending_area, &state.theme, dequeue.as_deref());
    }
    if images_h > 0 {
        render_images(frame, images_area, state);
    }
    if band_h > 0 {
        // `(${keyText("app.interrupt")} to cancel)` (`status-indicator.ts:47,78,100`) — `keyText`,
        // so ALL bound keys joined with `/` (`keybinding-hints.ts:29-36`), not just the first.
        let cancel = state.keymap.keys_label(Action::Interrupt);
        state
            .indicator
            .render(frame, band_area, &state.theme, cancel.as_deref());
    }
    // Pi gates the hardware cursor globally — `showHardwareCursor` (`tui.ts:344,389-397`), fed from
    // the setting at `interactive-mode.ts:1721-1732` — and cyrup parks that flag on the editor
    // (`editor.rs:277`, "the ONLY component that asks for a cursor position is this editor"), which
    // was true only because the selector half had never been wired. Read before the borrow below.
    // …and only while the slot actually holds focus: a floating overlay draws OVER the live region
    // and captures input, so parking the cursor on a caret the user cannot type into would point at
    // the wrong thing. Pi ties the same decision to its own z-stack (`if (this.overlayStack.length
    // === 0) this.terminal.hideCursor()`, `tui.ts:656`).
    let show_hardware_cursor = state.editor.show_hardware_cursor() && state.overlays.is_empty();
    if let Some(active) = state.selector.as_mut() {
        active.inner.render(frame, slot_area, &state.theme);
        // The selector half of the hardware cursor. While a selector owns the input slot — an
        // extension `ui.input` dialog, `/model`, `/resume`'s search — Pi still positions the real
        // cursor at the typed character, because the focused `Input` inside the dialog emits
        // `CURSOR_MARKER` and `TUI.extractCursorPosition` finds it in the rendered output
        // (`tui.ts:1189-1207`, `input.ts:434`). Cyrup drew the reverse-video caret but left the
        // terminal cursor wherever the previous frame put it, which is what an IME composes
        // against and what a screen reader follows. [`crate::selector::caret_cell`] is the same
        // scan over the rendered CELLS; see its doc for why the reversed caret is the marker.
        if show_hardware_cursor {
            // Bound the buffer borrow to this statement so `set_cursor_position` can take `frame`.
            let caret = crate::selector::caret_cell(frame.buffer_mut(), slot_area);
            if let Some(pos) = caret {
                frame.set_cursor_position(pos);
            }
        }
    } else if let Some(loader) = state.loader.as_ref() {
        // A long inline op (e.g. `/share`'s gist creation) owns the slot with a `BorderedLoader`.
        loader.render(frame, slot_area, &state.theme, state.loader_tick);
    } else {
        state.editor.render(frame, slot_area, &state.theme);
        if let Some(ac) = state.editor.autocomplete() {
            // E14: the popup lives INSIDE the editor's padding frame. Pi renders it at
            // `contentWidth` (= `width - paddingX * 2`) and prefixes the same `leftPadding` every
            // text row gets (`editor.ts:591-597`), so with `editorPaddingX` 1–3 — the values
            // `/settings` cycles — the completions line up with the text they complete. cyrup drew
            // them into `popup_area` at full frame width, flush at column 0. No effect at the
            // default padding of 0, which is why it went unnoticed.
            let pad = state.editor.effective_padding(popup_area.width);
            let inner = ratatui::layout::Rect {
                x: popup_area.x.saturating_add(pad),
                y: popup_area.y,
                width: popup_area.width.saturating_sub(pad.saturating_mul(2)),
                height: popup_area.height,
            };
            let lines = ac.list.lines(inner.width, &state.theme);
            frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), inner);
        }
    }
    if wabove_h > 0 {
        render_extension_widgets(frame, wabove_area, state, false);
    }
    if wbelow_h > 0 {
        render_extension_widgets(frame, wbelow_area, state, true);
    }
    // TUI-033 — `setExtensionFooter` CLEARS `footerContainer` and adds the extension component in
    // place of the built-in footer, restoring the built-in when the factory is cleared
    // (`interactive-mode.ts:2245-2254`). So this is a swap, not an overlay.
    match state.extension_footer.as_deref() {
        Some(content) => {
            let lines: Vec<Line<'static>> = content
                .lines()
                .map(|l| Line::from(Span::styled(l.to_string(), state.theme.base_style())))
                .collect();
            frame.render_widget(
                Paragraph::new(lines).style(state.theme.base_style()),
                status_area,
            );
        }
        None => state.status.render(frame, status_area, &state.theme),
    }
    // Floating overlays draw last, on top of the live region, bottom→top (spec/tui/05 §2; arch-10
    // §6.4): each clears its own `Rect` then renders its box.
    for overlay in state.overlays.iter_mut() {
        overlay.render(frame, area, &state.theme);
    }
}

/// Render the attached-image strip inline above the editor (`components/image.ts`): stack each
/// [`ImageBlock`] at its natural cell height, drawing the real protocol when `show_images` is on and a
/// text placeholder when off (spec/tui/06 §6). Honors the live image protocol negotiated at startup.
/// TUI-039 — the terminal-geometry fallback is a **two-step** one upstream, not a constant:
/// `get columns() { return process.stdout.columns || Number(process.env.COLUMNS) || 80; }` and
/// `get rows() { return process.stdout.rows || Number(process.env.LINES) || 24; }`
/// (`packages/tui/src/tui.ts:1730-1736` @v0.83.0). Wherever the ioctl gives no size — a pipe, a CI
/// harness, some container PTY setups — cyrup pinned 80 columns and silently ignored a `COLUMNS=200`
/// the user or harness had set.
///
/// `Number("garbage")` is `NaN`, which is falsy, so pi falls through to the constant; a parse
/// failure, a zero and a negative all do the same here.
pub(crate) fn fallback_columns() -> u16 {
    env_geometry("COLUMNS").unwrap_or(80)
}

/// The `$LINES` half (`tui.ts:1734-1736`). Returned as an `Option` rather than defaulted to pi's
/// bare `24`, because cyrup's one caller has a strictly better last resort available — the live
/// inline-viewport height — and chains onto this.
pub(crate) fn env_rows() -> Option<u16> {
    env_geometry("LINES")
}

/// `Number(process.env.X) || …` — a positive integer, else `None`.
pub(crate) fn env_geometry(var: &str) -> Option<u16> {
    std::env::var(var)
        .ok()?
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|n| *n > 0)
}

/// Pi's `isExtensionCommand(text)` (`interactive-mode.ts:4022-4030` @v0.83.0): a leading `/`, the
/// word up to the first space, looked up in the extension runner's command registry. An extension
/// command is executed immediately even during a compaction — it is UI work, not a turn — which is
/// why the compaction queue skips it. TUI-031.
pub(crate) fn is_extension_command(session: &AgentSession, text: &str) -> bool {
    let Some(body) = text.strip_prefix('/') else {
        return false;
    };
    let name = body.split_once(' ').map_or(body, |(n, _)| n);
    session
        .services()
        .ext_host
        .registry()
        .has_command(name)
        .unwrap_or(false)
}

/// Draw one placement's extension widgets, in mount order — Pi's `renderWidgets` re-adds every
/// entry of the matching map to its container (`interactive-mode.ts:1920-1960`). Each row is a
/// `Text(line, 1, 0)`, i.e. `paddingX` 1. TUI-014.
pub(crate) fn render_extension_widgets(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    below: bool,
) {
    let lines: Vec<Line<'static>> = state
        .extension_widgets
        .iter()
        .filter(|w| w.below == below)
        .flat_map(|w| w.lines.iter())
        .map(|l| Line::from(Span::styled(format!(" {l}"), state.theme.base_style())))
        .collect();
    frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), area);
}

pub(crate) fn render_images(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    // TUI-017 — Pi's width rule for the attachment strip is
    // `Math.max(1, Math.min(width - 2, this.options.maxWidthCells ?? 60))`
    // (`packages/tui/src/components/image.ts:65` @v0.83.0), where `maxWidthCells` comes from
    // `terminal.imageWidthCells`. cyrup passed the raw `area.width` with no cap at all, so on a wide
    // terminal the raster was unbounded where Pi stops at 60 cells.
    let max_cells = state.transcript.image_width_cells().max(1);
    let width = area.width.saturating_sub(2).min(max_cells).max(1);
    for block in &state.pending_images {
        if y >= bottom {
            break;
        }
        let want = state.image_renderer.cell_size(block, width).1.max(1);
        let h = want.min(bottom.saturating_sub(y));
        let cell = ratatui::layout::Rect {
            x: area.x,
            y,
            width,
            height: h,
        };
        state
            .image_renderer
            .render(frame, cell, block, &state.theme, state.show_images);
        y = y.saturating_add(h);
    }
}
