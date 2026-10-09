#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::transcript::*;

fn line_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// A user entry whose `interactive-mode.ts:3500` gate is already decided — this module is about
/// the pad, not about where the leading `Spacer(1)` comes from.
fn user(text: &str, lead_spacer: bool) -> Entry {
    Entry::User {
        text: text.to_string(),
        lead_spacer,
    }
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
    assert_eq!(
        line_text(&u1[0]),
        "",
        "user leading Spacer(1): {:?}",
        line_text(&u1[0])
    );
    assert_eq!(
        line_text(&u1[1]).trim(),
        "",
        "user top paddingY row: {:?}",
        line_text(&u1[1])
    );
    assert!(
        line_text(&u1[2]).starts_with(" hello"),
        "pad=1 user: {:?}",
        line_text(&u1[2])
    );
    let a1 = entry_lines(
        &Entry::Assistant("hi".into()),
        &theme,
        80,
        1,
        ImageOpts::default(),
    );
    assert_eq!(line_text(&a1[0]), "", "assistant leading Spacer(1)");
    assert_eq!(
        line_text(&a1[1]),
        " hi",
        "pad=1 assistant: {:?}",
        line_text(&a1[1])
    );
    // pad = 0 → flush-left (no leading space).
    let u0 = entry_lines(&user("hello", true), &theme, 80, 0, ImageOpts::default());
    assert!(
        line_text(&u0[2]).starts_with("hello"),
        "pad=0 user: {:?}",
        line_text(&u0[2])
    );
    let a0 = entry_lines(
        &Entry::Assistant("hi".into()),
        &theme,
        80,
        0,
        ImageOpts::default(),
    );
    assert_eq!(
        line_text(&a0[1]),
        "hi",
        "pad=0 assistant: {:?}",
        line_text(&a0[1])
    );

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
        let rows = entry_lines(
            &Entry::Assistant(LONG.into()),
            &theme,
            width,
            1,
            ImageOpts::default(),
        );
        // Row 0 is `assistant-message.ts:100-102`'s `Spacer(1)`; the body follows.
        let body = &rows[1..];
        assert!(
            body.len() > 1,
            "width={width}: expected a wrapped body, got {body:?}"
        );
        for row in body {
            let t = line_text(row);
            assert!(
                t.starts_with(' '),
                "width={width}: row lost its leftMargin: {t:?}"
            );
            assert!(
                !t.starts_with("  "),
                "width={width}: over-indented row: {t:?}"
            );
            // `contentWidth = width - paddingX*2` plus one column of `leftMargin` — the last
            // column stays empty, which is the `rightMargin` (`markdown.ts:330`/`:340`).
            assert!(
                row.width() < width,
                "width={width}: no right gutter: {t:?} ({})",
                row.width()
            );
        }
        // MIRROR: at `outputPad = 0` there is no margin, and the wrap uses the full width.
        let flush = entry_lines(
            &Entry::Assistant(LONG.into()),
            &theme,
            width,
            0,
            ImageOpts::default(),
        );
        for row in &flush[1..] {
            assert!(
                row.width() <= width,
                "pad=0 width={width}: {:?}",
                line_text(row)
            );
        }
        assert!(
            !line_text(&flush[1]).starts_with(' '),
            "pad=0 must be flush-left"
        );
    }

    // MIRROR: a short message still occupies exactly one body row, and an empty turn none.
    let short = entry_lines(
        &Entry::Assistant("hi".into()),
        &theme,
        80,
        1,
        ImageOpts::default(),
    );
    assert_eq!(short.len(), 2, "spacer + one row: {short:?}");
    assert!(
        entry_lines(
            &Entry::Assistant("   ".into()),
            &theme,
            80,
            1,
            ImageOpts::default()
        )
        .is_empty()
    );
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
/// this.outputPad, 0)`; `showWarning` (`interactive-mode.ts:4469-4473` @v0.87.1) does the same in
/// the warning colour with a paddingX of `1` — equal to `outputPad` at the `1` used here.
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
        assert!(
            rows[1].spans.iter().any(|s| s.style.fg.is_some()),
            "colour lost: {rows:?}"
        );
    }
}

/// **TUI-062** — pi builds every warning row this entry ports as `new Text(theme.fg("warning", …),
/// 1, 0)` — `showWarning` (`interactive-mode.ts:4471` @v0.87.1), the trust banner, the cost notices
/// — a literal paddingX of `1`, while `showError` passes `this.outputPad` (`:4465`). So with
/// `outputPad` at `0` or `3` a warning still sits one column in and an error moves with the setting.
#[test]
fn a_warning_keeps_padding_one_while_an_error_follows_output_pad() {
    let theme = UiTheme::dark();
    let indent = |row: &Line<'static>| line_text(row).chars().take_while(|c| *c == ' ').count();
    for pad in [0usize, 3] {
        let warning = entry_lines(
            &Entry::Warning("Warning: careful".into()),
            &theme,
            80,
            pad,
            ImageOpts::default(),
        );
        assert_eq!(line_text(&warning[0]), "", "leading Spacer(1)");
        assert_eq!(
            indent(&warning[1]),
            1,
            "warning at outputPad={pad}: {warning:?}"
        );
        let error = entry_lines(
            &Entry::Error("Error: broken".into()),
            &theme,
            80,
            pad,
            ImageOpts::default(),
        );
        assert_eq!(
            indent(&error[1]),
            pad,
            "error at outputPad={pad}: {error:?}"
        );
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
    assert_eq!(
        view.pending().len(),
        2,
        "both warnings queued: {:?}",
        view.pending()
    );

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

// --- TUI-175: `outputPad` reaches every transcript block -----------------------------------------
//
// pi `18336987a` (#10557) threads `outputPad` into every chat component at f1b2e77f5: the tool
// shell (`components/tool-execution.ts:283`, `:335`), the built-in `edit` component
// (`core/tools/renderers/edit.ts:139`), `BashExecutionComponent` (`bash-execution.ts:141`, `:149`,
// `:159`, `:205`), the compaction, branch and skill boxes (`super(outputPad, 1, …)`), the default
// custom-message box (`custom-message.ts:90`) and the renderer-failed custom-entry box
// (`custom-entry.ts:56`). Ported from `packages/coding-agent/test/output-pad.test.ts`, whose cases
// are the bash block, a defined tool, a tool with no definition, the self-rendered edit result and
// the compaction summary; this extends the table to every block the row names.

/// The render width `output-pad.test.ts` uses (`component.render(60)`).
const PAD_WIDTH: usize = 60;

/// A settled tool run, as the transcript holds it.
fn settled_tool(
    name: &str,
    args: Value,
    is_error: bool,
    text: &str,
    definition: Option<ToolRenderKind>,
) -> Entry {
    let mut view = TranscriptView::new();
    view.push_tool_start_defined(name, Some("id".into()), args, None, definition);
    view.push_tool_end_rendered(
        name,
        Some("id"),
        is_error,
        Some(serde_json::json!({ "content": [{ "type": "text", "text": text }], "details": {} })),
        None,
        None,
    );
    Entry::Tool(view.active_tools()[0].clone())
}

type MakeEntry = Box<dyn Fn(&str) -> Entry>;

/// One block per case the Verify line names; the closure's argument is the block's own body text,
/// so the same table drives the short-text comparison and the long-text width check. The `bool` is
/// the live `toolOutputExpanded` the block is painted under.
fn pad_cases() -> Vec<(&'static str, bool, MakeEntry)> {
    vec![
        (
            "built-in bash tool row",
            false,
            Box::new(|t: &str| {
                settled_tool(
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                    false,
                    t,
                    None,
                )
            }),
        ),
        (
            "defined tool with no renderers",
            false,
            Box::new(|t: &str| {
                settled_tool(
                    "custom_tool",
                    serde_json::json!({}),
                    false,
                    t,
                    Some(ToolRenderKind::Default),
                )
            }),
        ),
        (
            "tool with no definition",
            false,
            Box::new(|t: &str| settled_tool("custom_tool", serde_json::json!({}), false, t, None)),
        ),
        (
            "self-rendered edit result",
            false,
            Box::new(|t: &str| {
                settled_tool(
                    "edit",
                    serde_json::json!({
                        "path": "file.txt",
                        "edits": [{ "oldText": "old", "newText": "new" }],
                    }),
                    true,
                    t,
                    None,
                )
            }),
        ),
        (
            "`!` bash block",
            false,
            Box::new(|t: &str| {
                let mut b = BashExecution::new("pwd", false);
                b.append_output(t);
                b.set_complete(Some(1), false, false, None);
                Entry::Bash(b)
            }),
        ),
        (
            "collapsed compaction summary",
            false,
            Box::new(|t: &str| Entry::CompactionSummary {
                tokens_before: 10,
                summary: t.to_string(),
            }),
        ),
        (
            "expanded compaction summary",
            true,
            Box::new(|t: &str| Entry::CompactionSummary {
                tokens_before: 10,
                summary: t.to_string(),
            }),
        ),
        (
            "collapsed branch summary",
            false,
            Box::new(|t: &str| Entry::BranchSummary {
                summary: t.to_string(),
            }),
        ),
        (
            "expanded branch summary",
            true,
            Box::new(|t: &str| Entry::BranchSummary {
                summary: t.to_string(),
            }),
        ),
        (
            "collapsed skill block",
            false,
            Box::new(|t: &str| Entry::SkillInvocation {
                name: "demo".into(),
                content: t.to_string(),
                lead_spacer: true,
            }),
        ),
        (
            "expanded skill block",
            true,
            Box::new(|t: &str| Entry::SkillInvocation {
                name: "demo".into(),
                content: t.to_string(),
                lead_spacer: true,
            }),
        ),
        (
            "default custom message",
            false,
            Box::new(|t: &str| Entry::Custom {
                label: "note".into(),
                body: t.to_string(),
                rendered: Rendered::None,
            }),
        ),
        (
            "renderer-failed custom entry",
            false,
            Box::new(|t: &str| Entry::Custom {
                label: "note".into(),
                body: String::new(),
                rendered: Rendered::Failed(t.to_string()),
            }),
        ),
    ]
}

/// `output-pad.test.ts`'s `renderLines`: text without trailing fill, keeping only rows that carry a
/// word character, `$` or `(` (so blank rows, tinted padding rows and full-width rules drop out).
fn pad_rows(entry: &Entry, pad: usize, expanded: bool) -> Vec<String> {
    let opts = ImageOpts {
        tools_expanded: expanded,
        ..ImageOpts::default()
    };
    entry_lines(entry, &UiTheme::dark(), PAD_WIDTH, pad, opts)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .filter(|l| l.chars().any(|c| c.is_alphanumeric() || "_$(".contains(c)))
        .collect()
}

/// The port of `output-pad.test.ts`'s one assertion, per block: at `outputPad = 0` no content row
/// starts with a space, and at `outputPad = 1` every row is exactly the `0` row shifted one column.
#[test]
fn every_transcript_block_renders_flush_at_output_pad_zero_and_shifted_at_one() {
    for (name, expanded, make) in pad_cases() {
        let entry = make("ok");
        let flush = pad_rows(&entry, 0, expanded);
        assert!(!flush.is_empty(), "{name}: rendered nothing");
        let indented: Vec<&String> = flush.iter().filter(|l| l.starts_with(' ')).collect();
        assert!(
            indented.is_empty(),
            "{name}: rows still inset at outputPad 0: {indented:?} in {flush:?}"
        );
        let padded = pad_rows(&entry, 1, expanded);
        let shifted: Vec<String> = flush.iter().map(|l| format!(" {l}")).collect();
        assert_eq!(
            padded, shifted,
            "{name}: outputPad 1 is not outputPad 0 shifted by one"
        );
    }
}

/// "…and fill the width": a body token longer than the pane hard-wraps at the block's content
/// width, `width - 2 * outputPad`. At `0` a row of it starts at column 0 and runs to the last
/// column; at `1` the same row is inset one column on each side, which is the pre-TUI-175 output.
#[test]
fn every_transcript_block_uses_the_full_width_at_output_pad_zero() {
    let token = "x".repeat(PAD_WIDTH * 2);
    let full = "x".repeat(PAD_WIDTH);
    let inset = format!(" {}", "x".repeat(PAD_WIDTH - 2));
    for (name, expanded, make) in pad_cases() {
        let entry = make(&token);
        let flush = pad_rows(&entry, 0, expanded);
        let padded = pad_rows(&entry, 1, expanded);
        // The collapsed summaries and the collapsed skill block draw no body, so the flush/shifted
        // comparison above is all there is to check for them.
        if !flush.iter().any(|l| l.contains("xxxx")) {
            assert!(
                name.starts_with("collapsed"),
                "{name}: body missing: {flush:?}"
            );
            continue;
        }
        assert!(
            flush.contains(&full),
            "{name}: no row fills all {PAD_WIDTH} columns at outputPad 0: {flush:?}"
        );
        assert!(
            padded.contains(&inset),
            "{name}: outputPad 1 no longer insets the body one column each side: {padded:?}"
        );
        assert!(
            padded.iter().all(|l| l.starts_with(' ')),
            "{name}: a row lost its outputPad 1 margin: {padded:?}"
        );
    }
}

/// The live region paints its tool rows and `!` block at the view's `outputPad` too, not only the
/// committed entries (`onOutputPadChange` calls `setOutputPad` on every child of the chat AND the
/// pending containers, `interactive-mode.ts:5079-5088` @f1b2e77f5).
#[test]
fn the_live_tool_row_and_bash_block_follow_the_views_output_pad() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_tool_start("bash", serde_json::json!({ "command": "pwd" }));
    view.start_bash("live", false, None, None);
    let rows = |view: &mut TranscriptView| -> Vec<String> {
        view.lines(PAD_WIDTH, &theme)
            .iter()
            .map(|l| line_text(l).trim_end().to_string())
            .filter(|l| l.contains("$ pwd") || l.contains("$ live"))
            .collect()
    };
    assert_eq!(rows(&mut view), vec![" $ pwd", " $ live"]);
    view.set_output_pad(0);
    assert_eq!(rows(&mut view), vec!["$ pwd", "$ live"]);
}
