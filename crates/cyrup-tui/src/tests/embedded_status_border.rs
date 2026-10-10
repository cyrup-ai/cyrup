//! TUI-103 — the working / compaction / branch-summary / retry status lives in the editor's TOP
//! BORDER, not in a band of its own above the editor.
//!
//! Upstream is `CustomEditor.renderTopBorder` (`custom-editor.ts:36-79`), reached because the
//! default chat editor is constructed `embedWorkingStatus: true` (`interactive-mode.ts:647-652`),
//! and the two border forms on the `StatusIndicator` BASE class (`status-indicator.ts:24-31`), so
//! all four kinds embed. The row cyrup used to spend is gone for the same reason pi spends none:
//! `showStatusIndicator` adds to `statusContainer` only when the editor cannot embed
//! (`interactive-mode.ts:2312-2323` → `:222-229`), and cyrup has exactly one editor.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;
use ratatui::style::Style;
use ratatui::text::Span;

use super::harness::*;
use crate::editor::{EmbeddedStatus, top_border_with_status};
use crate::status_indicator::{IndicatorKind, StatusIndicator};
use crate::{App, SPINNER_FRAMES, UiTheme};

/// Every indicator kind that embeds — `working | retry | compaction | branchSummary`
/// (`status-indicator.ts:7`); `Idle` is the `statusWidth === 0` fall-through, tested separately.
const ACTIVE_KINDS: [IndicatorKind; 4] = [
    IndicatorKind::Working,
    IndicatorKind::Retry,
    IndicatorKind::Compaction,
    IndicatorKind::BranchSummary,
];

fn indicator(kind: IndicatorKind) -> StatusIndicator {
    let mut ind = StatusIndicator::new();
    ind.set(kind, None);
    ind
}

/// The composed row as text and as a column count, at a fixed spinner phase so the assertions are
/// not wall-clock dependent.
fn compose(
    ind: &StatusIndicator,
    hidden: usize,
    width: u16,
    rule: Style,
) -> Option<Vec<Span<'static>>> {
    // The composer reads the indicator's live clock. `set` anchored it microseconds ago, so the
    // frame is `SPINNER_FRAMES[0]` — but nothing here depends on that: every Braille frame is one
    // column, so the width assertions hold at any phase, and the text assertions check membership
    // in `SPINNER_FRAMES` rather than a particular glyph.
    top_border_with_status(
        &EmbeddedStatus {
            indicator: ind,
            cancel_hint: Some("Esc"),
        },
        hidden,
        width,
        rule,
        &UiTheme::dark(),
    )
}

fn text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

fn width_of(spans: &[Span<'static>]) -> usize {
    crate::text_width::spans_width(spans)
}

// ----------------------------------------------------------------- TUI-103's own Verify line ----

/// TUI-103's Verify, first half: "`TestBackend` render during a turn: row 0 of the editor block
/// reads `── ⠋ Working ───…`, and no blank band precedes it."
///
/// (The row's Verify spells the message `Working...`; the live default has been `Working` since pi
/// v0.85.0 / #8799 — `interactive-mode.ts:451`, already recorded on TUI-102 and pinned by
/// `tests::status_indicator::working_band_shows_spinner_message_and_cancel_hint`.)
#[test]
fn the_top_rule_carries_the_spinner_and_message_during_a_turn() {
    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app.state_mut().indicator.working();
    app.draw().unwrap();

    let slot = app.state().regions.slot;
    assert!(slot.height >= 3, "editor slot too short: {slot:?}");
    let rule = row_text(&app, slot.y);

    assert!(
        rule.starts_with("── "),
        "top rule does not open with pi's `── ` prefix (custom-editor.ts:59): [{rule}]"
    );
    let frame = SPINNER_FRAMES
        .iter()
        .find(|f| rule.contains(**f))
        .unwrap_or_else(|| panic!("no spinner glyph in the top rule: [{rule}]"));
    assert!(
        rule.contains(&format!("── {frame} Working ─")),
        "top rule is not `── <spinner> Working ───…`: [{rule}]"
    );
    // Everything after the status is rule glyphs, to the last column.
    let tail = rule.rsplit_once(" Working ").unwrap().1;
    assert!(
        !tail.is_empty() && tail.chars().all(|c| c == '─'),
        "the rule does not run out to the edge after the message: [{rule}]"
    );
    assert_eq!(
        crate::text_width::str_width(&rule),
        80,
        "the composed rule must be exactly the slot width — it is painted OVER the Block's drawn \
         rule, so a short row leaks the `─`s underneath: [{rule}]"
    );

    // "…and no blank band precedes it": `statusContainer` is empty upstream, so the band has no
    // rows at all, not merely blank ones.
    assert_eq!(
        app.state().regions.band.height,
        0,
        "the 2-row status band is still being allocated"
    );
}

/// TUI-103's Verify, second half: "narrow width shows the spinner alone." Pi's narrow fallback is
/// `custom-editor.ts:71-78`: the spinner alone, `prefixWidth = min(3, max(0, width - statusWidth))`
/// leading `─`, and whatever is left trailing.
#[test]
fn a_narrow_width_degrades_to_the_spinner_alone() {
    let ind = indicator(IndicatorKind::Working);
    // `Working` renders `⠋ Working` = 9 columns, so `width < statusWidth + 5` from 13 down.
    for width in [12u16, 10, 6, 4] {
        let spans = compose(&ind, 0, width, Style::default())
            .unwrap_or_else(|| panic!("no status composed at width {width}"));
        let row = text(&spans);
        assert!(
            !row.contains("Working"),
            "the message survived at width {width}: [{row}]"
        );
        assert!(
            SPINNER_FRAMES.iter().any(|f| row.contains(*f)),
            "the spinner is missing at width {width}: [{row}]"
        );
        let leading = row.chars().take_while(|c| *c == '─').count();
        assert!(
            leading <= 3,
            "pi caps the prefix at three `─` (custom-editor.ts:73); got {leading} at width {width}"
        );
        assert_eq!(
            width_of(&spans),
            usize::from(width),
            "narrow rule is not exactly {width} columns: [{row}]"
        );
    }
}

/// Pi's `canFitOverflow` ladder (`custom-editor.ts:48-65`), both rungs.
///
/// The label keeps the column a plain `createScrollBorder` would have put it at — upstream's
/// `overflowStart` is literally `createScrollBorder`'s `leftWidth`
/// (`packages/tui/src/components/editor.ts:279-294`) — so only the LEFT run of `─` is overwritten
/// by the status. When the status will not fit beside the label, the MESSAGE yields and the
/// spinner alone keeps it company (`:51-54`).
#[test]
fn the_overflow_label_keeps_its_centred_column_and_the_message_yields_to_it() {
    let ind = indicator(IndicatorKind::Working);
    let label = " ↑ 7 more ";
    let label_w = crate::text_width::str_width(label);

    // Narrow enough that `── ⠋ Working ` (13 columns) cannot sit left of the centred label.
    let narrow = 28u16;
    let spans = compose(&ind, 7, narrow, Style::default()).unwrap();
    let row = text(&spans);
    assert!(row.contains(label), "overflow label dropped: [{row}]");
    assert!(
        !row.contains("Working"),
        "the message must yield to the overflow label: [{row}]"
    );
    assert!(
        SPINNER_FRAMES.iter().any(|f| row.contains(*f)),
        "the spinner must survive beside the label: [{row}]"
    );
    assert_eq!(width_of(&spans), usize::from(narrow));
    assert_eq!(
        col_of(&row, label),
        u16::try_from((usize::from(narrow) - label_w) / 2).unwrap(),
        "the label left its centred column: [{row}]"
    );

    // Wide enough for both. The label is still centred at the same formula.
    let wide = 80u16;
    let spans = compose(&ind, 7, wide, Style::default()).unwrap();
    let row = text(&spans);
    assert!(row.contains("Working"), "message lost at width 80: [{row}]");
    assert!(
        row.contains(label),
        "overflow label lost at width 80: [{row}]"
    );
    assert_eq!(width_of(&spans), usize::from(wide));
    assert_eq!(
        col_of(&row, label),
        u16::try_from((usize::from(wide) - label_w) / 2).unwrap(),
        "the label left its centred column at width 80: [{row}]"
    );
    assert!(
        row.starts_with("── "),
        "the `── ` prefix is missing in the overflow branch: [{row}]"
    );
}

/// The regression fence, not the red-prover: [`crate::editor::top_border_with_status`] must return
/// exactly `width` display columns for every width, both scroll states and all four kinds. This is
/// the same invariant `editor/render.rs`'s `scroll_border` doc records, and it exists because cyrup
/// paints the composed row OVER the `Block`'s already-drawn rule — a short row leaks the `─`s
/// underneath, which pi (composing each row from scratch) cannot suffer.
#[test]
fn the_rule_is_exactly_as_wide_as_it_is_asked_for_with_a_status() {
    for kind in ACTIVE_KINDS {
        let ind = indicator(kind);
        for width in 1u16..=120 {
            for hidden in [0usize, 7] {
                let spans = compose(&ind, hidden, width, Style::default()).unwrap_or_else(|| {
                    panic!("{kind:?} composed nothing at width {width} hidden {hidden}")
                });
                assert_eq!(
                    width_of(&spans),
                    usize::from(width),
                    "{kind:?} at width {width} hidden {hidden}: [{}]",
                    text(&spans)
                );
            }
        }
    }
}

/// An embedded `working` status takes the EDITOR'S BORDER COLOUR for spinner and message alike:
/// `showWorkingStatusIndicator` passes ONE `colorFn` — `this.editor.borderColor ??
/// theme.getThinkingBorderColor(...)` — and `WorkingStatusIndicator` hands it to both
/// `spinnerColorFn` and `messageColorFn` (`interactive-mode.ts:2346-2359`,
/// `status-indicator.ts:38-47`). The other three kinds build their own colours in their own
/// constructors (`status-indicator.ts:52-77`, `:84-99`, `:102-111`) and keep them when embedded.
#[test]
fn an_embedded_working_status_takes_the_editor_border_colour() {
    let theme = UiTheme::dark();
    let rule = theme.bash_mode_style();

    let working = compose(&indicator(IndicatorKind::Working), 0, 80, rule).unwrap();
    for span in &working {
        assert_eq!(
            span.style, rule,
            "an embedded working status must be entirely the rule colour: [{}] {:?}",
            span.content, span.style
        );
    }

    // Compaction keeps accent spinner + muted message regardless of the rule colour.
    let compaction = compose(&indicator(IndicatorKind::Compaction), 0, 80, rule).unwrap();
    let spinner = compaction
        .iter()
        .find(|s| SPINNER_FRAMES.iter().any(|f| s.content.contains(*f)))
        .expect("no spinner span");
    assert_eq!(
        spinner.style,
        theme.accent_style(),
        "compaction's spinner must stay accent when embedded"
    );
    let message = compaction
        .iter()
        .find(|s| s.content.contains("Compacting"))
        .expect("no message span");
    assert_eq!(
        message.style,
        theme.muted_style(),
        "compaction's message must stay muted when embedded"
    );
    // Retry keeps its warning spinner.
    let retry = compose(&indicator(IndicatorKind::Retry), 0, 80, rule).unwrap();
    let spinner = retry
        .iter()
        .find(|s| SPINNER_FRAMES.iter().any(|f| s.content.contains(*f)))
        .expect("no spinner span");
    assert_eq!(
        spinner.style,
        theme.warning_style(),
        "retry's spinner must stay warning when embedded"
    );
}

/// `clearStatusIndicator` adds the 2-blank-row `IdleStatus` only when
/// `!clearedIndicatorWasEmbedded` (`interactive-mode.ts:2336-2342`). cyrup's editor always embeds,
/// so there is no band to reserve and an idle indicator composes nothing (pi's `statusWidth === 0`
/// arm, `custom-editor.ts:43`).
///
/// `TUI-182` removed this test's `reserve_status_rows` half. It used to flip the flag on and assert
/// the editor did not move — i.e. it asserted the flag had no effect, which is a dead flag's
/// epitaph, not a rule. The flag is gone; what is live is that the idle band costs zero rows.
#[test]
fn the_idle_band_rows_are_gone() {
    assert!(
        compose(&indicator(IndicatorKind::Idle), 0, 80, Style::default()).is_none(),
        "an idle indicator must fall through to the plain border"
    );

    let mut idle = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    idle.draw().unwrap();
    assert_eq!(
        idle.state().regions.band.height,
        0,
        "the idle band must cost no rows"
    );
}
