#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::transcript::*;

fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// A user entry whose `interactive-mode.ts:3500` gate is already decided — this module is about
/// the pad, not about where the leading `Spacer(1)` comes from.
fn user(text: &str, lead_spacer: bool) -> Entry {
    Entry::User { text: text.to_string(), lead_spacer }
}

/// F12: a fresh transcript defaults to Pi's `outputPad = 1` and `set_output_pad` drives it.
#[test]
fn output_pad_defaults_to_one_and_is_settable() {
    let mut view = TranscriptView::new();
    assert_eq!(view.output_pad(), 1, "Pi's default outputPad is 1");
    view.set_output_pad(0);
    assert_eq!(view.output_pad(), 0);
}

/// `outputPad` left-indents the message BODY; `0` renders flush-left, `1` prepends a single
/// leading column.
///
/// X1 — there is no `you: ` / `assistant: ` label to indent past. `user-message.ts:38-58` adds
/// exactly one child (a `Box` wrapping a `Markdown`) and `assistant-message.ts:104-114` adds one
/// `Markdown` per text block; neither component contains a role prefix.
///
/// L1/L3 — the user block's first two rows are the leading `Spacer(1)`
/// (`interactive-mode.ts:3501`) and the `Box`'s top `paddingY` row (`box.ts:107-109`); the
/// assistant block's first row is `assistant-message.ts:100-102`'s `Spacer(1)`.
#[test]
fn output_pad_left_indents_committed_messages() {
    let theme = UiTheme::dark();
    // pad = 1 → the body starts one column in.
    let u1 = entry_lines(&user("hello", true), &theme, 80, 1, ImageOpts::default());
    assert_eq!(line_text(&u1[0]), "", "user leading Spacer(1): {:?}", line_text(&u1[0]));
    assert_eq!(line_text(&u1[1]).trim(), "", "user top paddingY row: {:?}", line_text(&u1[1]));
    assert!(line_text(&u1[2]).starts_with(" hello"), "pad=1 user: {:?}", line_text(&u1[2]));
    let a1 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 1, ImageOpts::default());
    assert_eq!(line_text(&a1[0]), "", "assistant leading Spacer(1)");
    assert_eq!(line_text(&a1[1]), " hi", "pad=1 assistant: {:?}", line_text(&a1[1]));
    // pad = 0 → flush-left (no leading space).
    let u0 = entry_lines(&user("hello", true), &theme, 80, 0, ImageOpts::default());
    assert!(line_text(&u0[2]).starts_with("hello"), "pad=0 user: {:?}", line_text(&u0[2]));
    let a0 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 0, ImageOpts::default());
    assert_eq!(line_text(&a0[1]), "hi", "pad=0 assistant: {:?}", line_text(&a0[1]));

    // MIRROR (X1): no role label at any pad, in either arm.
    for pad in [0usize, 1] {
        for e in [user("hello", true), Entry::Assistant("hi".into())] {
            let joined: String = entry_lines(&e, &theme, 80, pad, ImageOpts::default())
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!joined.contains("you:"), "pad={pad}: {joined:?}");
            assert!(!joined.contains("assistant:"), "pad={pad}: {joined:?}");
        }
    }
}

/// The live streaming partial honors the pad too (Pi keeps the outputPad on the in-flight
/// `AssistantMessageComponent`). Rendering the active region with pad=1 vs pad=0 shifts the line.
///
/// Row 0 is L3's `Spacer(1)` (`assistant-message.ts:100-102`), which the live view emits for the
/// same reason the committed arm does — it is one component either side of the commit.
#[test]
fn output_pad_indents_the_live_streaming_partial() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_assistant_delta("streaming answer");
    let padded = view.lines(80, &theme);
    assert_eq!(line_text(&padded[0]), "", "leading Spacer(1) missing");
    assert_eq!(line_text(&padded[1]), " streaming answer", "pad=1 live");
    view.set_output_pad(0);
    let flush = view.lines(80, &theme);
    assert_eq!(line_text(&flush[1]), "streaming answer", "pad=0 live");

    // MIRROR (X1): no `assistant: ` label and no `▌` caret in the live region either.
    // `git grep "▌" v0.84.1 -- packages/` finds only `examples/extensions/custom-header.ts:22`.
    let joined: String = flush.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(!joined.contains("assistant:"), "live label: {joined:?}");
    assert!(!joined.contains('\u{258c}'), "live caret: {joined:?}");
}

/// A sentence long enough to wrap several times at any of the widths this module tests.
const LONG: &str = "The quick brown fox jumps over the lazy dog and then keeps running for \
                    quite a long while indeed before it finally stops.";

/// **L2 + M10** — EVERY row of a multi-row message carries the `outputPad` margin, and no row
/// reaches the last column.
///
/// `markdown.ts:316-326` wraps at `contentWidth = width - paddingX * 2` (`:284`) and only then
/// does `:334-340` emit `leftMargin + line + rightMargin` for **each** produced row. cyrup used
/// to insert the margin into the single unwrapped logical line and let the outer
/// `Paragraph::wrap` reflow it at full frame width, so row 0 started at column 1 and rows 1..N
/// at column 0 — a ragged left edge on nearly every turn — with nothing holding a right gutter.
#[test]
fn l2_every_wrapped_row_of_a_message_carries_the_margin_and_a_right_gutter() {
    let theme = UiTheme::dark();
    for width in [20usize, 40, 80] {
        let rows = entry_lines(&Entry::Assistant(LONG.into()), &theme, width, 1, ImageOpts::default());
        // Row 0 is `assistant-message.ts:100-102`'s `Spacer(1)`; the body follows.
        let body = &rows[1..];
        assert!(body.len() > 1, "width={width}: expected a wrapped body, got {body:?}");
        for row in body {
            let t = line_text(row);
            assert!(t.starts_with(' '), "width={width}: row lost its leftMargin: {t:?}");
            assert!(!t.starts_with("  "), "width={width}: over-indented row: {t:?}");
            // `contentWidth = width - paddingX*2` plus one column of `leftMargin` — the last
            // column stays empty, which is the `rightMargin` (`markdown.ts:330`/`:340`).
            assert!(row.width() < width, "width={width}: no right gutter: {t:?} ({})", row.width());
        }
        // MIRROR: at `outputPad = 0` there is no margin, and the wrap uses the full width.
        let flush = entry_lines(&Entry::Assistant(LONG.into()), &theme, width, 0, ImageOpts::default());
        for row in &flush[1..] {
            assert!(row.width() <= width, "pad=0 width={width}: {:?}", line_text(row));
        }
        assert!(!line_text(&flush[1]).starts_with(' '), "pad=0 must be flush-left");
    }

    // MIRROR: a short message still occupies exactly one body row, and an empty turn none.
    let short = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 1, ImageOpts::default());
    assert_eq!(short.len(), 2, "spacer + one row: {short:?}");
    assert!(entry_lines(&Entry::Assistant("   ".into()), &theme, 80, 1, ImageOpts::default())
        .is_empty());
}

/// The same for the LIVE streaming partial (`transcript.rs:1000`'s call site) — the row a user
/// watches for the whole turn.
#[test]
fn l2_live_streaming_partial_wraps_inside_its_own_padding() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_assistant_delta(LONG);
    let rows = view.lines(40, &theme);
    assert!(rows.len() > 2, "expected a wrapped live body: {rows:?}");
    for row in &rows[1..] {
        let t = line_text(row);
        assert!(t.starts_with(' '), "live row lost its leftMargin: {t:?}");
        assert!(row.width() <= 39, "live row has no right gutter: {t:?}");
    }
}

/// **Edit 6** — a long `Entry::Error` / `Entry::Warning` is a `Text`, and a `Text` WRAPS at
/// `contentWidth = width - paddingX * 2` (`text.ts:64`) before prefixing `leftMargin` to each
/// produced row (`:70-76`).
///
/// `assistant-message.ts:180`/`:189`/`:193` construct them as `new Text(theme.fg("error", …),
/// this.outputPad, 0)`; `interactive-mode.ts:3956-3960` does the same in the warning colour.
/// cyrup pushed ONE unwrapped logical line and `pad_lines`'d it, i.e. the L2 defect again.
#[test]
fn error_and_warning_rows_wrap_inside_the_output_pad() {
    let theme = UiTheme::dark();
    for entry in [Entry::Error(LONG.into()), Entry::Warning(LONG.into())] {
        let rows = entry_lines(&entry, &theme, 40, 1, ImageOpts::default());
        assert_eq!(line_text(&rows[0]), "", "leading Spacer(1)");
        assert!(rows.len() > 2, "expected a wrapped body: {rows:?}");
        for row in &rows[1..] {
            let t = line_text(row);
            assert!(t.starts_with(' '), "row lost its leftMargin: {t:?}");
            assert!(row.width() <= 39, "row has no right gutter: {t:?}");
        }
        // The colour rides on the span, inside the margins (`theme.fg("error", text)`).
        assert!(rows[1].spans.iter().any(|s| s.style.fg.is_some()), "colour lost: {rows:?}");
    }
}

/// **CFG-051** — the migrated-credential notice must RENDER, verbatim, and BEFORE the
/// model-fallback warning.
///
/// pi shows the line inside the running UI — `if (migratedProviders && migratedProviders.length
/// > 0) { this.showWarning(\`Migrated credentials to auth.json: ${migratedProviders.join(", ")}\`); }`
/// (`interactive-mode.ts:874-876` @v0.83.0) — ahead of the `modelFallbackMessage` warning
/// (`:883-885`). cyrup pushes both from `run_interactive` in that order
/// (`crates/cyrup/src/main.rs:1940` then `:1946`), and the STRING is pinned on that side by
/// `the_migrated_credential_notice_is_pis_line_and_is_absent_when_nothing_moved`.
///
/// What no test pinned — the residual REPRO-LOG carried for this row — is the RENDER: a string
/// pushed into `pending` is only a notice if `entry_lines` (the production path, `app.rs:1851`)
/// actually emits it. `Entry::Warning` renders its text VERBATIM, which is why `Warning: ` is a
/// per-caller obligation here (TUI-062) — so a renderer that re-prefixed, truncated or dropped
/// the line would leave the string test green and the user with nothing on screen.
#[test]
fn the_migrated_credential_notice_renders_first_and_verbatim_in_the_transcript() {
    // The two production lines, in `run_interactive` order. Deliberately DISTINCT values (two
    // providers, a comma join) so a renderer that emitted the wrong entry cannot pass.
    const MIGRATED: &str = "Warning: Migrated credentials to auth.json: anthropic, openai";
    const FALLBACK: &str = "Warning: No models available.";
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_warning(MIGRATED);
    view.push_warning(FALLBACK);
    // PRESENCE before absence: an empty queue would make every row assertion below vacuous.
    assert_eq!(view.pending().len(), 2, "both warnings queued: {:?}", view.pending());

    // The production render path: `app.rs:1851` maps every entry through `entry_lines` at the
    // transcript's own `output_pad`. Width 100 is wider than either line, so a row that does
    // not match exactly is a render defect, not a wrap.
    let rows: Vec<Line<'static>> = view
        .pending()
        .iter()
        .flat_map(|e| entry_lines(e, &theme, 100, view.output_pad(), ImageOpts::default()))
        .collect();
    let text: Vec<String> = rows.iter().map(line_text).collect();

    let migrated_at = text
        .iter()
        .position(|r| r.trim() == MIGRATED)
        .unwrap_or_else(|| panic!("the migrated-credential notice never rendered: {text:?}"));
    let fallback_at = text
        .iter()
        .position(|r| r.trim() == FALLBACK)
        .unwrap_or_else(|| panic!("the model-fallback warning never rendered: {text:?}"));
    assert!(
        migrated_at < fallback_at,
        "pi renders the migrated-credential notice (`:874-876`) BEFORE the modelFallbackMessage \
         warning (`:883-885`); got {text:?}"
    );
    // Verbatim: exactly one `Warning: `, no second prefix from the renderer.
    assert_eq!(
        text[migrated_at].matches("Warning: ").count(),
        1,
        "the renderer must not re-prefix a verbatim `Entry::Warning`: {:?}",
        text[migrated_at]
    );
    // …and in the warning colour, not the default foreground.
    assert_eq!(
        rows[migrated_at].spans.iter().find_map(|s| s.style.fg),
        theme.warning_style().fg,
        "the notice must render in the warning colour (`theme.fg(\"warning\", …)`)"
    );
}
