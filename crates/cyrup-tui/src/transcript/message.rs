use super::*;

/// Pi's `hiddenThinkingLabel` default (`assistant-message.ts:29`) — the single static line shown in
/// place of the reasoning body when `hideThinkingBlock` is on.
pub const HIDDEN_THINKING_LABEL: &str = "Thinking...";

/// Render one run of assistant reasoning (`assistant-message.ts:139-165`): the static
/// [`HIDDEN_THINKING_LABEL`] when `hidden`, otherwise the coalesced thinking body.
///
/// X5. The hidden form is a plain `new Text(theme.italic(theme.fg("thinkingText", label)),
/// outputPad, 0)` (`:141-143`) — one styled line. The **body** is a real
/// `new Markdown(thinkingBlocks.join("\n\n"), this.outputPad, 0, this.markdownTheme,
/// { color: (text) => theme.fg("thinkingText", text), italic: true }, …)` (`:146-164`).
///
/// The `{ color, italic }` pair reaches only `applyDefaultStyle` (`markdown.ts:377-404`), which
/// `renderToken` hands to the `paragraph`/`text` arms alone: a `heading` builds its own style
/// context (`:470-480`) and a fenced `code` block never consults one (`:520-539`). So `## Plan`
/// keeps `mdHeading` and a fence keeps its border + syntax colours — a thinking block is markdown,
/// not a flat grey wall. The doc comment that used to sit here claimed upstream "forces every span
/// to the one colour regardless of markdown structure"; that claim was false, and it was the
/// justification for splitting the body on `\n` and never calling the markdown renderer.
pub(super) fn thinking_lines(
    text: &str,
    hidden: bool,
    width: usize,
    theme: &UiTheme,
    label: &str,
) -> Vec<Line<'static>> {
    let style = theme.thinking_text_style();
    if hidden {
        return vec![Line::styled(label.to_string(), style)];
    }
    let body = text.trim();
    if body.is_empty() {
        return Vec::new();
    }
    crate::markdown::render_with_default_style(body, width.max(1), theme, style.fg, true)
}

/// Render a labeled extension/system message (`skill`/`custom`/`branch`/`compaction` variants),
/// then the optional bold `header` + the `body` rendered as markdown. The committed scrollback form
/// is the *expanded* render (the complete record), like committed tools.
///
/// T9 (TUI-FIDELITY §2): the `[label]` bracket is Pi's `customMessageLabel` token, not `accent`.
/// All four upstream components build it identically — `theme.fg("customMessageLabel",
/// "\x1b[1m[<name>]\x1b[22m")` — at v0.84.1
/// `coding-agent/src/modes/interactive/components/skill-invocation-message.ts:38`,
/// `custom-message.ts:92`, `branch-summary-message.ts:35` and `compaction-summary-message.ts:36`.
/// The `\x1b[1m…\x1b[22m` pair is SGR bold, so the bold stays; only the colour role changes
/// (`dark.json:41` `#9575cd`, `light.json:40` `#7e57c2` — purple, where cyrup was painting the teal
/// accent).
///
/// T9 continued — `customMessageBg` + `customMessageText`. All four upstream components are (or
/// wrap) a `Box` whose fill is `theme.bg("customMessageBg", …)` and hand their body to
/// `new Markdown(…, { color: (text) => theme.fg("customMessageText", text) })`:
/// `custom-message.ts:36,107-111`, `skill-invocation-message.ts:17,42-44`,
/// `branch-summary-message.ts:16,42-44`, `compaction-summary-message.ts:16,43-45`. Both tokens were
/// dead on screen — [`UiTheme::custom_message_bg_style`] had zero callers — so the block drew no
/// fill and the body took the plain `text` role. The fill goes on `Line::style` (the same mechanism
/// the `userMessageBg` block uses) and the body colour goes through
/// [`crate::markdown::render_with_text_color`], because a span-level `fg` set by the markdown
/// renderer would otherwise mask a line-level one.
///
/// X2 — the block shell. All four components are (or extend) `new Box(1, 1, (t) =>
/// theme.bg("customMessageBg", t))`, so the body sits in a **1-column inset** and the box emits a
/// tinted blank row above and below it (`box.ts:79-88`, `:106-119`). Both were missing: the label
/// started at column 0 and the purple band was exactly as tall as its content.
///
/// X2 — the `Spacer(1)` after the label. `custom-message.ts:94`, `branch-summary-message.ts:37` and
/// `compaction-summary-message.ts:38` each `addChild(new Spacer(1))` immediately after the label
/// `Text`; `skill-invocation-message.ts` does **not** (`:36-45` is label then `Markdown`, `:47-53`
/// is one collapsed line). Hence `spacer_after_label` rather than an unconditional blank — the row
/// is a property of three of the four components, not of the shared shell.
///
/// `lead_spacer` is the `chatContainer.addChild(new Spacer(1))` that precedes the component. It is
/// **not** uniform across the call sites, so each one passes its own answer:
/// `interactive-mode.ts:3484` (compaction) and `:3491` (branch) are unconditional, whereas `:3500`
/// — which covers the skill component, since `:3506` sits inside it — is gated on
/// `this.chatContainer.children.length > 0`. A custom message supplies its own in the constructor
/// (`custom-message.ts:33`), also unconditional.
/// X14 — the COLLAPSED branch/compaction summary: the same `Box(1, 1, customMessageBg)` +
/// `[label]` + `Spacer(1)` shell [`labeled_message_lines`] builds, but with one `Text` row in place
/// of the markdown body (`branch-summary-message.ts:46-56`, `compaction-summary-message.ts:47-56`).
///
/// ```ts
/// this.addChild(new Text(
///     theme.fg("customMessageText", "Branch summary (") +
///         theme.fg("dim", keyText("app.tools.expand")) +
///         theme.fg("customMessageText", " to expand)"),
///     0, 0));
/// ```
///
/// Three runs, and the outer two are `customMessageText` — NOT `muted`. This is not `keyHint`; the
/// two components spell the pair out by hand and only the key label shares `dim` with it. `lead`
/// carries the trailing `(` so the compaction variant can interpolate its token count
/// (`Compacted from 12,345 tokens (`).
pub(super) fn collapsed_summary_lines(
    label: &str,
    lead: &str,
    expand_key: &str,
    theme: &UiTheme,
    width: usize,
) -> Vec<Line<'static>> {
    let block = theme.custom_message_bg_style();
    let text = theme.custom_message_text_style();
    let content_width = width.saturating_sub(2).max(1);
    let row = Line::from(vec![
        Span::styled(lead.to_string(), text),
        Span::styled(expand_key.to_string(), theme.dim_style()),
        Span::styled(" to expand)".to_string(), text),
    ]);
    let mut children = vec![Line::styled(
        format!("[{label}]"),
        theme.custom_message_label_style(),
    )];
    children.push(Line::default());
    // `new Text(…, 0, 0)` — paddingX 0 inside the `Box`, so the row wraps at the box's own content
    // width with no extra margin.
    children.extend(text_lines_of(&row, content_width, 0));
    let fill = match block.bg {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    };
    let mut out = box_lines(children, width, 1, 1, fill);
    // `interactive-mode.ts:3484`/`:3491` — the leading `Spacer(1)` is unconditional for both.
    if !out.is_empty() {
        out.insert(0, Line::default());
    }
    out
}

pub(super) fn labeled_message_lines(
    label: &str,
    header: &str,
    body: &str,
    spacer_after_label: bool,
    lead_spacer: bool,
    theme: &UiTheme,
    width: usize,
) -> Vec<Line<'static>> {
    let block = theme.custom_message_bg_style();
    // `Box(1, 1)` renders its children at `contentWidth = width - 2` (`box.ts:79`).
    let content_width = width.saturating_sub(2).max(1);
    let mut children = vec![Line::styled(
        format!("[{label}]"),
        theme.custom_message_label_style(),
    )];
    if spacer_after_label {
        children.push(Line::default());
    }
    let md_src = if header.is_empty() {
        body.to_string()
    } else if body.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{body}")
    };
    if !md_src.is_empty() {
        children.extend(crate::markdown::render_with_text_color(
            &md_src,
            content_width,
            theme,
            block.fg,
        ));
    }
    // The `customMessageBg` fill covers the whole box — padding rows and label row included.
    // A theme that omits the token leaves `bg` `None` and the terminal default shows through.
    // `applyBackgroundToLine` paints the BACKGROUND only (`box.ts:132-134`); the foreground comes
    // from the content, which already carries `customMessageText` via `render_with_text_color`.
    let fill = match block.bg {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    };
    let mut out = box_lines(children, width, 1, 1, fill);
    // The leading `Spacer(1)` — see `lead_spacer` above. Skipped when the `Box` produced no rows at
    // all (`box.ts:75-77`/`:91-93`), so a contentless block cannot leave an orphan blank behind.
    if lead_spacer && !out.is_empty() {
        out.insert(0, Line::default());
    }
    out
}

/// Group an integer with `,` thousands separators (Pi `Number.toLocaleString()` for the compaction
/// token count). Pure ASCII; never allocates beyond the result.
pub(super) fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
