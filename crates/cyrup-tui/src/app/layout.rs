use super::*;

/// The six live-region row heights `[msg, band, images, slot, popup, footer]` for a viewport of
/// `avail` rows (audit #1). Filled by priority **from the bottom** — footer, then the editor/selector
/// slot (+completion popup), then the status band, then the inline image strip — and the message
/// region takes the remainder, **capped to the active turn's content height** so an empty turn never
/// balloons into a void (the old `Constraint::Min(1)` flex). The function is idempotent: feeding back
/// its own sum reproduces the same split, so [`render`] (called with the viewport height) and
/// [`live_region_height`] (called with the terminal height) never disagree on row counts.
/// The editor's visible text-row budget on a `term_rows`-row terminal — Pi `editor.ts:499-501`:
///
/// ```text
/// // Calculate max visible lines: 30% of terminal height, minimum 5 lines
/// const terminalRows = this.tui.terminal.rows;
/// const maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3));
/// ```
///
/// `floor(rows * 0.3)` is computed in integers as `rows * 3 / 10` (identical for every `u16`, and
/// free of the float rounding that would make e.g. `rows = 10` ambiguous). Rules are NOT counted:
/// the editor draws `1 + min(layoutLines, maxVisibleLines) + 1` rows.
///
/// Shared by [`region_constraints`] (which reserves the slot) and
/// [`crate::extension_editor::ExtensionEditorSelector`] (E12 — the `ui.editor` dialog embeds the
/// same `Editor`, `extension-editor.ts:70`, so the same cap applies to its body).
pub(crate) fn max_visible_editor_lines(term_rows: u16) -> u16 {
    ((u32::from(term_rows) * 3 / 10).min(u32::from(u16::MAX)) as u16).max(5)
}

/// The rows an extension widget list occupies, `above` or `below` the editor (TUI-014).
pub(crate) fn widget_rows(state: &AppState, below: bool) -> u16 {
    state
        .extension_widgets
        .iter()
        .filter(|w| w.below == below)
        .map(|w| w.lines.len().min(u16::MAX as usize) as u16)
        .fold(0u16, u16::saturating_add)
}

/// The rows the extension header occupies (TUI-033).
pub(crate) fn header_rows(state: &AppState) -> u16 {
    state
        .extension_header
        .as_deref()
        .map(|h| h.lines().count().min(u16::MAX as usize) as u16)
        .unwrap_or(0)
}

pub(crate) fn region_constraints(state: &mut AppState, width: u16, avail: u16) -> [u16; 10] {
    let avail = avail.max(1);
    let max_editor = avail.saturating_sub(2).max(3);
    // A selector — else a mounted loader — owns the slot at its desired height; otherwise the editor
    // sizes to its line count + the two rule rows (spec/tui/05 §1.1, spec/tui/03 §3.1).
    let want_slot = match (state.selector.as_ref(), state.loader.as_ref()) {
        (Some(active), _) => active.inner.desired_height(width).clamp(3, max_editor),
        // A mounted `BorderedLoader` owns the slot at ITS height, for the same reason a selector
        // does: pi clears `editorContainer` and puts the loader there (`session-share.ts:152-156`),
        // so the rows the editor would have asked for are irrelevant. `BorderedLoader::height()` is
        // 7 cancellable / 5 plain (`chrome.rs:334-340`) against the editor's usual 3 — sized from
        // the editor the loader was clipped to top-rule + blank + spinner, with neither the
        // `escape/ctrl+c cancel` hint row nor the bottom rule ever reaching a frame.
        (None, Some(loader)) => loader.height().clamp(3, max_editor),
        // Size from the VISUAL (wrapped) line count, windowed and measured exactly as Pi's
        // `Editor.render` does:
        //
        // * **E15 — measure at the width it renders at.** Pi derives ONE `layoutWidth` and feeds it
        //   to both `this.lastWidth` and `layoutText()` (`editor.ts:489-497`). cyrup measured at a
        //   hardcoded `width - 1` while [`crate::editor::InputEditor`]'s render wraps at
        //   `layout_width(width)` = `width - 2 * paddingX` when `paddingX > 0`, so any
        //   `editorPaddingX` made the render wrap NARROWER than the measurement and produced rows
        //   the slot had no space for — clipped, caret row included.
        // * **E3 — the window is capped at 30% of the terminal.** `maxVisibleLines = Math.max(5,
        //   Math.floor(terminalRows * 0.3))` (`editor.ts:499-501`), then `layoutLines.slice(...)`
        //   (`:519`). The old cap was `avail - 2`, so a long paste grew the editor until it owned the
        //   terminal minus two rows and the transcript collapsed: on a 40-row terminal pi shows 12
        //   text rows and scrolls, cyrup showed 38.
        //
        // The `+2` is the two rule rows; `clamp(3, max_editor)` stays as the viewport backstop.
        (None, None) => (state
            .editor
            .visual_line_count(usize::from(state.editor.layout_width(width)))
            .min(usize::from(max_visible_editor_lines(state.term_rows)))
            .min(u16::MAX as usize) as u16)
            .saturating_add(2)
            .clamp(3, max_editor),
    };
    // The completion popup is appended below the editor's bottom rule (spec/tui/04 §7); suppressed
    // while a selector — or a loader — owns the slot, since both replace the editor the popup
    // completes for (pi's `editorContainer.clear()`, `session-share.ts:153`).
    let want_popup = if state.selector.is_some() || state.loader.is_some() {
        0
    } else {
        state
            .editor
            .autocomplete()
            .map(|ac| ac.list.rendered_height())
            .unwrap_or(0)
    };
    let footer_max: u16 = if state.status.has_extension_statuses() {
        3
    } else {
        2
    };
    let want_status = state.indicator.is_active() || state.reserve_status_rows;
    let want_images: u16 =
        if state.selector.is_some() || state.loader.is_some() || state.pending_images.is_empty() {
            0
        } else {
            state
                .pending_images
                .iter()
                .map(|b| state.image_renderer.cell_size(b, width).1)
                .fold(0u16, |a, h| a.saturating_add(h))
        };

    // L7 — the editor's MINIMUM HEIGHT. Pi docks the two bottom regions with explicit floors
    // (`interactive-mode.ts:876-883`):
    //
    // ```ts
    // const dock = new TuiLayouts.VStack([
    //     { component: this.pendingMessagesContainer, shrink: 1, minSize: 0 },
    //     { component: this.statusContainer,          shrink: 1, minSize: 0 },
    //     { component: this.widgetContainerAbove,     shrink: 1, minSize: 0 },
    //     { component: this.editorContainer,          shrink: 1, minSize: 3 },
    //     { component: this.widgetContainerBelow,     shrink: 1, minSize: 0 },
    //     { component: this.footerContainer,          shrink: 1, minSize: 1 },
    // ]);
    // ```
    //
    // and `allocateStackSizes`' shrink pass only ever takes rows from an entry while
    // `sizes[index] > (entry.minSize ?? 0)` — `capacity = sizes[index] - minSize`
    // (`tui/src/components/stack.ts:109,124`). So pi's editor never goes below 3 rows. When even
    // the floors do not fit, `candidates` empties and the pass returns (`:111`) with the stack
    // OVERFLOWING its box; the children past the box's clip rect are the ones that vanish, and the
    // editor — laid out before the footer — is not one of them.
    //
    // cyrup allocated the footer first and then took `want_slot.min(remaining)` with no floor at
    // all, so on a viewport of 3-4 rows the editor was squeezed to 1-2 rows: its own top/bottom
    // rules do not fit, let alone a line of text. Both floors are now reserved up front, editor
    // first, and only the surplus is handed out — which reproduces pi's answer on a very short
    // terminal (editor 3, footer 1 at 4 rows; editor 3 and no footer at 3) and is bit-identical to
    // the old split at every height where the old one was not already squeezing.
    const EDITOR_MIN_ROWS: u16 = 3;
    let mut remaining = avail;
    let slot_floor = want_slot.min(EDITOR_MIN_ROWS).min(remaining);
    remaining = remaining.saturating_sub(slot_floor);
    let footer_floor = 1u16.min(remaining);
    remaining = remaining.saturating_sub(footer_floor);
    // Surplus, in the old order: the footer fills out to `footer_max`, then the slot to `want_slot`.
    let footer_extra = footer_max.saturating_sub(footer_floor).min(remaining);
    let footer = footer_floor.saturating_add(footer_extra);
    remaining = remaining.saturating_sub(footer_extra);
    let slot_extra = want_slot.saturating_sub(slot_floor).min(remaining);
    let slot = slot_floor.saturating_add(slot_extra);
    remaining = remaining.saturating_sub(slot_extra);
    let popup = want_popup.min(remaining);
    remaining = remaining.saturating_sub(popup);
    let band = if want_status { 2u16.min(remaining) } else { 0 };
    remaining = remaining.saturating_sub(band);
    let images = want_images.min(remaining);
    remaining = remaining.saturating_sub(images);
    // TUI-016 — Pi's `pendingMessagesContainer`, docked immediately after `chatContainer` and
    // immediately before `statusContainer` (`interactive-mode.ts:712-714`), i.e. the first
    // live-region row after the message area. Its `VStack` entry is `shrink: 1, minSize: 0`, so it
    // is one of the entries that gives its rows up before the editor does; taking it after the
    // editor/footer floors, the popup, the band and the images reproduces that priority.
    let pending = state.pending_messages.height().min(remaining);
    remaining = remaining.saturating_sub(pending);
    // TUI-014 — Pi's `widgetContainerAbove` / `widgetContainerBelow` are two more `VStack` entries
    // in the dock, at `shrink: 1, minSize: 0`, sitting either side of `editorContainer`
    // (`interactive-mode.ts:876-883`, and the mount order at `:709-719`). Taken after the editor and
    // footer floors for the same reason the pending region is: they yield their rows first.
    let widgets_above = widget_rows(state, false).min(remaining);
    remaining = remaining.saturating_sub(widgets_above);
    let widgets_below = widget_rows(state, true).min(remaining);
    remaining = remaining.saturating_sub(widgets_below);
    // TUI-033 — the custom header replaces `builtInHeader` inside `headerContainer`
    // (`interactive-mode.ts:2273-2290`), which is docked ABOVE the chat container.
    let header = header_rows(state).min(remaining);
    remaining = remaining.saturating_sub(header);
    // The message region = the active turn's content, plus the startup-hint block at idle, capped
    // to whatever rows remain (so the inline viewport stays content-sized, not full-screen).
    let active = state
        .transcript
        .content_height(width as usize, &state.theme)
        .min(u16::MAX as usize) as u16;
    // …at the block's WRAPPED height (`Text.render` wraps at `contentWidth = width - paddingX * 2`,
    // `tui/src/components/text.ts:64-67`), so a narrow terminal reserves the extra rows the block
    // grows into instead of clipping them off.
    let hint =
        if state.show_startup_hints && state.selector.is_none() && !state.transcript.has_active() {
            crate::chrome::compact_hint_height(&state.theme, &state.keymap, width)
        } else {
            0
        };
    let msg = active.max(hint).min(remaining);
    [
        header,
        msg,
        pending,
        band,
        images,
        widgets_above,
        slot,
        popup,
        widgets_below,
        footer,
    ]
}

/// The inline-viewport height = the sum of the live-region rows (audit #1). Driven by
/// [`region_constraints`] against the **terminal** height so the content-sized viewport never
/// exceeds the screen.
pub(crate) fn live_region_height(state: &mut AppState, width: u16, term_height: u16) -> u16 {
    // A floating overlay (hotkeys/help; spec/tui/05 §2) is a modal that draws *over* the whole live
    // region — it needs the full screen to center its box, so the inline viewport expands to the
    // terminal height while one is open (the editor/footer still render behind it).
    if !state.overlays.is_empty() {
        return term_height.max(1);
    }
    region_constraints(state, width, term_height)
        .iter()
        .copied()
        .fold(0u16, u16::saturating_add)
}
