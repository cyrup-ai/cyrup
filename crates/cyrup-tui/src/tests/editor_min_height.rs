//! **L7 — the editor slot's minimum height**, against pi v0.84.1.
//!
//! The upstream fact, read in full rather than inferred
//! (`git -C pi show v0.84.1:packages/coding-agent/src/modes/interactive/interactive-mode.ts`):
//!
//! ```ts
//! // :876-883
//! const dock = new TuiLayouts.VStack([
//!     { component: this.pendingMessagesContainer, shrink: 1, minSize: 0 },
//!     { component: this.statusContainer,          shrink: 1, minSize: 0 },
//!     { component: this.widgetContainerAbove,     shrink: 1, minSize: 0 },
//!     { component: this.editorContainer,          shrink: 1, minSize: 3 },
//!     { component: this.widgetContainerBelow,     shrink: 1, minSize: 0 },
//!     { component: this.footerContainer,          shrink: 1, minSize: 1 },
//! ]);
//! ```
//!
//! and the allocator those floors feed, `allocateStackSizes` →
//! `distribute(sizes, entries, total - contentSize, "shrink")`
//! (`packages/tui/src/components/stack.ts:135-153`), whose shrink pass filters to
//!
//! ```ts
//! // :109
//! return (entry.shrink ?? 1) > 0 && sizes[index]! > (entry.minSize ?? 0);
//! // :124
//! const capacity = mode === "grow" ? … : sizes[index]! - (entry.minSize ?? 0);
//! ```
//!
//! so a row is never taken from an entry already at its floor, and when every entry is at its floor
//! `candidates` is empty and the pass returns (`:111`) leaving the stack OVERFLOWING its box. The
//! children past the box's clip rect are the ones that disappear, and the editor — laid out before
//! the footer (`layout.ts:181-190`) — is not one of them.
//!
//! The audit recorded L7 "without a rendered-character comparison — verify before fixing". The
//! comparison is here: these tests read the rendered `TestBackend` grid, count the editor's rules,
//! and are the reason the fix is not a guess.
//!
//! One mechanism note. That dock is `fullscreenLayoutRoot`'s (`:884-887`), i.e. it governs pi's
//! **fullscreen** (alt-screen) layout; pi's regular main-screen mount is a flat
//! `mountInteractiveTui(this.renderer, [...])` (`:888-896`) that appends into scrollback with no
//! vertical budget at all, so nothing there can squeeze anything. cyrup has one inline layout with
//! a fixed row budget, which is the situation the dock's floors exist to answer, so the dock is the
//! thing to port — and "no budget" is not an alternative cyrup's renderer can express.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{App, UiTheme};
use ratatui::backend::TestBackend;

/// Every row of the frame, trailing blanks trimmed.
fn rows(app: &App<TestBackend>) -> Vec<String> {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut s = String::new();
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            s.trim_end().to_string()
        })
        .collect()
}

/// The editor's rules — a full-width `─` run (`editor.ts:530`, `:587`) or a `createScrollBorder`
/// indicator, which still opens with `─── ` (`:261`).
fn rule_rows(app: &App<TestBackend>) -> Vec<usize> {
    rows(app)
        .iter()
        .enumerate()
        .filter(|(_, r)| r.starts_with('─'))
        .map(|(i, _)| i)
        .collect()
}

fn app_of_height(h: u16) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(60, h), UiTheme::dark()).unwrap();
    app.draw().unwrap();
    app
}

/// **L7.** At four rows the dock's floors sum to exactly four (`0+0+0+3+0+1`), so pi gives the
/// editor its 3 and the footer its 1. cyrup allocated the footer FIRST and then took
/// `want_slot.min(remaining)` with no floor at all, leaving the editor 2 rows — not even room for
/// its own two rules, let alone the line of text between them.
///
/// FAILS before the fix: the frame carries fewer than two rules, because a 2-row slot cannot hold
/// the `─` top rule, a text row and the `─` bottom rule.
#[test]
fn the_editor_keeps_pis_three_row_floor_on_a_short_terminal() {
    for h in [4u16, 5, 6, 8] {
        let app = app_of_height(h);
        let r = rows(&app);
        let rules = rule_rows(&app);
        assert!(
            rules.len() >= 2,
            "at {h} rows the editor must still render BOTH rules — pi's dock entry is \
             `{{ shrink: 1, minSize: 3 }}` (`interactive-mode.ts:880`):\n{}",
            r.join("\n")
        );
        assert!(
            rules[1] - rules[0] >= 2,
            "at {h} rows the editor must keep a text row between its rules (slot >= 3):\n{}",
            r.join("\n")
        );
    }
}

/// **L7, the floor is exact.** pi's editor floor is 3, not "as much as is left". At 4 rows the
/// editor takes 3 and the footer takes its own `minSize: 1` — the split is not 2/2 and not 4/0.
///
/// FAILS before the fix: the editor got `4 - footer_max(2) = 2`.
#[test]
fn a_four_row_viewport_splits_three_editor_one_footer_like_pis_dock() {
    let app = app_of_height(4);
    let r = rows(&app);
    let rules = rule_rows(&app);
    assert_eq!(
        rules,
        vec![0, 2],
        "the editor owns rows 0..=2 (rule, text, rule) and the footer row 3:\n{}",
        r.join("\n")
    );
}

/// **MIRROR — the floor must not become a ceiling, and must not perturb a normal terminal.** The
/// editor is `{ shrink: 1, minSize: 3 }`: `shrink` is what makes the surplus flow to the regions
/// above it, and `minSize` binds only when the budget runs out. On any terminal with room, the
/// split has to be byte-for-byte what it was before L7 — the fix reserves the two floors up front
/// and then hands the surplus out in the same order it always did, so this is an equality, not an
/// inequality.
///
/// (Ran, not assumed: this is the test that would have caught a "reserve 3 for the editor" fix that
/// stole a row from the transcript on a 40-row terminal.)
#[test]
fn the_floor_does_not_change_the_layout_on_a_terminal_with_room() {
    for h in [10u16, 24, 40, 80] {
        let app = app_of_height(h);
        let r = rows(&app);
        let rules = rule_rows(&app);
        assert_eq!(
            rules.len(),
            2,
            "exactly the editor's own pair of rules at {h} rows:\n{}",
            r.join("\n")
        );
        // An empty editor is one text row: `rule / "" / rule` = a 3-row slot, sitting directly
        // above the 2-row footer, at the bottom of the frame.
        assert_eq!(
            rules[1] - rules[0],
            2,
            "an empty editor is a 3-row slot at {h} rows:\n{}",
            r.join("\n")
        );
        assert_eq!(
            usize::from(h) - rules[1],
            3,
            "the 2-row footer still sits below the editor's bottom rule at {h} rows:\n{}",
            r.join("\n")
        );
    }
}

/// **MIRROR — degenerate heights stay total.** `region_constraints` computes every budget by
/// subtraction; reserving two floors up front adds two more subtractions that must saturate. A
/// 1-row terminal has no room for even the editor's floor.
#[test]
fn a_viewport_shorter_than_the_floor_still_renders() {
    for h in [1u16, 2, 3] {
        let app = app_of_height(h);
        let buf = app.terminal().backend().buffer();
        assert_eq!(buf.area.height, h, "the frame is still {h} rows");
    }
}
