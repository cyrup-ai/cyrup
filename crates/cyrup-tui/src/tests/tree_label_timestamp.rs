//! `/tree`'s label timestamp — Pi `tree-selector.ts` @v0.87.1: `showLabelTimestamps = false`
//! (`:116`), the inline render `prefix + "[label] " + "<label time> " + content` (`:745-754`), the
//! `[+label time]` status marker (`:663-665`) and `formatLabelTimestamp` (`:862-885`).
//!
//! Pi's timestamp is `labelTimestamp`: the time an entry's **label** was set. It is off until the
//! `shift+t` toggle (`app.tree.toggleLabelTimestamp`) turns it on, and even then it decorates only
//! rows that carry a label — `showLabelTimestamps && node.label && node.labelTimestamp`.
//!
//! SESS-S05: cyrup had the toggle but no producer (`SessionDagNode` carried no label timestamp, so
//! the projection hard-coded `None`), and rendered what it would have shown as a right-aligned column
//! after a `☆labeled` star rather than inline after pi's `[label] ` prefix.
//!
//! The clock is pi's LOCAL time. The render tests below build "today at 12:04" in the local zone
//! with chrono, so they hold under any `TZ`; run them under a non-UTC `TZ` to see the zone matter.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::harness::*;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    App, InputEvent, Selector as _, SelectorKind, TreeNode, TreeSelector, UiTheme,
    format_label_timestamp, tree_node_from_dag,
};
use cyrup_session_svc::{SessionDagKind, SessionDagNode};
use ratatui::backend::TestBackend;
use time::{Date, Month, OffsetDateTime, UtcOffset};

/// `app.tree.toggleLabelTimestamp` is bound to **`shift+t`** upstream (`keybindings.ts:131-134`),
/// which a terminal delivers as the shifted letter.
fn shift_t() -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT))
}

/// Today at `hh:mm` LOCAL time (what pi's `Date` getters read), as the RFC3339 a label entry
/// carries.
pub(super) fn today_at(hour: u32, minute: u32) -> String {
    chrono::Local::now()
        .date_naive()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .unwrap()
        .to_rfc3339()
}

/// `y-m-d hh:mm` at a whole-hour UTC offset.
fn at(y: i32, m: Month, d: u8, hh: u8, mm: u8, offset_hours: i8) -> OffsetDateTime {
    Date::from_calendar_date(y, m, d)
        .unwrap()
        .with_hms(hh, mm, 0)
        .unwrap()
        .assume_offset(UtcOffset::from_hms(offset_hours, 0, 0).unwrap())
}

/// Open `/tree` over `nodes` in an assembled app, exactly as the run loop's `/tree` arm does.
fn tree_app(nodes: Vec<TreeNode>) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    app
}

/// Three rows. Row 0 is the highlighted anchor; row 1 is labeled and carries a label timestamp;
/// row 2 carries the same kind of timestamp but no label. Pi decorates row 1 only, and only once
/// the toggle is on.
fn labeled_and_unlabeled() -> Vec<TreeNode> {
    let anchor = TreeNode::message("e0", 0, "user: port the editor");

    let mut labeled = TreeNode::message("e1", 0, "user: wire up streaming");
    labeled.user_label = Some("checkpoint".to_string());
    labeled.label_timestamp = Some(today_at(12, 4));

    let mut unlabeled = TreeNode::message("e2", 0, "user: fix the footer");
    unlabeled.label_timestamp = Some(today_at(9, 41));

    vec![anchor, labeled, unlabeled]
}

/// The projection carries pi's `label` and `labelTimestamp` through unformatted, and the row text
/// no longer embeds the label (the renderer composes it).
#[test]
fn the_dag_projection_carries_the_label_and_its_timestamp() {
    let node = SessionDagNode {
        entry_id: "e9".into(),
        parent_id: None,
        depth: 0,
        label: "user: fix the footer".to_string(),
        kind: SessionDagKind::Message,
        foldable: false,
        is_leaf: true,
        user_label: Some("checkpoint".to_string()),
        label_timestamp: Some("2026-08-07T12:04:00Z".to_string()),
        timestamp: "2026-08-07T11:00:00Z".to_string(),
    };
    let row = tree_node_from_dag(&node);
    assert_eq!(row.user_label.as_deref(), Some("checkpoint"));
    assert_eq!(row.label_timestamp.as_deref(), Some("2026-08-07T12:04:00Z"));
    assert_eq!(row.label, "user: fix the footer");
}

fn hours(h: i8) -> UtcOffset {
    UtcOffset::from_hms(h, 0, 0).unwrap()
}

/// Pi's three formats: `HH:MM` today, `M/D HH:MM` this year, `YY/M/D HH:MM` before that —
/// month and day unpadded, the clock zero-padded.
#[test]
fn format_label_timestamp_matches_pis_three_branches() {
    let utc = |_: OffsetDateTime| UtcOffset::UTC;
    let now = at(2026, Month::September, 28, 15, 30, 0);
    assert_eq!(
        format_label_timestamp("2026-09-28T07:05:00Z", now, utc).as_deref(),
        Some("07:05")
    );
    assert_eq!(
        format_label_timestamp("2026-03-04T23:09:59.123Z", now, utc).as_deref(),
        Some("3/4 23:09")
    );
    assert_eq!(
        format_label_timestamp("2024-11-30T00:00:00Z", now, utc).as_deref(),
        Some("24/11/30 00:00")
    );
    // Read in the zone: 23:30 UTC yesterday is 01:30 today at +02:00.
    assert_eq!(
        format_label_timestamp("2026-09-27T23:30:00Z", now, |_| hours(2)).as_deref(),
        Some("01:30")
    );
    assert_eq!(format_label_timestamp("not a time", now, utc), None);
}

/// Each instant is read at ITS OWN offset, as `Date`'s local getters do: a label set in winter
/// (+01:00) shows its winter wall clock even when "now" is in summer time (+02:00).
#[test]
fn format_label_timestamp_reads_each_instant_at_its_own_offset() {
    let dst = |t: OffsetDateTime| {
        if (4..=9).contains(&u8::from(t.month())) {
            hours(2)
        } else {
            hours(1)
        }
    };
    let now = at(2026, Month::July, 1, 10, 0, 0);
    assert_eq!(
        format_label_timestamp("2026-01-15T11:00:00Z", now, dst).as_deref(),
        Some("1/15 12:00")
    );
    // "Today" is today in the zone: 22:30 UTC on June 30 is 00:30 on July 1 at +02:00.
    assert_eq!(
        format_label_timestamp("2026-06-30T22:30:00Z", now, dst).as_deref(),
        Some("00:30")
    );
}

/// The render site reads the process's local zone: its offset at an instant is the one chrono's
/// `Local` (the system zone / `TZ`) reports there.
#[test]
fn the_render_zone_is_the_local_zone() {
    use chrono::{Offset as _, TimeZone as _};
    for t in [
        at(2026, Month::January, 15, 12, 0, 0),
        at(2026, Month::July, 15, 12, 0, 0),
    ] {
        let expected = chrono::Local
            .timestamp_opt(t.unix_timestamp(), 0)
            .unwrap()
            .offset()
            .fix()
            .local_minus_utc();
        assert_eq!(
            crate::tree_selector::local_offset_at(t).whole_seconds(),
            expected
        );
    }
}

/// Pi's default is OFF (`private showLabelTimestamps = false`); `shift+t` turns it on, announces
/// itself in the header, and draws the time INLINE: `[label] HH:MM ` ahead of the entry text.
#[test]
fn the_label_timestamp_is_off_until_the_toggle_then_inline_after_the_label() {
    let mut app = tree_app(labeled_and_unlabeled());
    app.draw().unwrap();
    let closed = buf_text(&app);
    assert!(
        closed.contains("[checkpoint] user: wire up streaming"),
        "the label prefix is always drawn, the time is not:\n{closed}"
    );
    assert!(!closed.contains("12:04"), "OFF by default:\n{closed}");
    assert!(!closed.contains("[+label time]"), "{closed}");

    app.handle_input(&shift_t());
    app.draw().unwrap();
    let open = buf_text(&app);
    assert!(
        open.contains("[checkpoint] 12:04 user: wire up streaming"),
        "the time sits between the label and the entry text (tree-selector.ts:754):\n{open}"
    );
    assert!(open.contains("[+label time]"), "{open}");
}

/// The time is a *label* timestamp: an entry with no label never shows one, even with the toggle on
/// and a timestamp attached (Pi's `flatNode.node.label &&` conjunct, `:747`).
#[test]
fn an_unlabeled_row_never_shows_a_label_timestamp() {
    let mut app = tree_app(labeled_and_unlabeled());
    app.handle_input(&shift_t());
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains("fix the footer"), "{screen}");
    assert!(
        screen.contains("12:04"),
        "precondition: the labeled row is painted:\n{screen}"
    );
    assert!(!screen.contains("09:41"), "{screen}");
}

/// Saving a label from the inline editor stamps it NOW, as pi's `updateNodeLabel` does
/// (`labelTimestamp = label ? new Date().toISOString() : undefined`, `:641`).
#[test]
fn a_label_saved_in_the_tree_is_stamped_now() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(vec![TreeNode::message("e0", 0, "user: hello")]);
    let keymap = crate::keymap::SelectKeymap::default();
    let clock =
        |t: chrono::DateTime<chrono::Local>| format!("[mark] {} user: hello", t.format("%H:%M"));
    sel.handle(
        &KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT),
        &keymap,
    );
    for c in "mark".chars() {
        sel.handle(
            &KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            &keymap,
        );
    }
    let before = clock(chrono::Local::now());
    sel.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &keymap);
    let after = clock(chrono::Local::now());
    sel.handle(
        &KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT),
        &keymap,
    );
    let row: String = sel.rows(&theme)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        row.ends_with(&before) || row.ends_with(&after),
        "the fresh label renders today's clock: {row:?} (expected {before:?})"
    );
}

/// Editing an entry that already carries a label starts from that label, caret at 0 — pi's
/// `LabelInput` runs `this.input.setValue(currentLabel)` on a fresh `Input` (`:1295-1300`), and
/// `setValue` clamps the caret to `min(cursor, length)`.
#[test]
fn the_label_editor_starts_from_the_current_label() {
    let theme = UiTheme::dark();
    let mut labeled = TreeNode::message("e0", 0, "user: hello");
    labeled.user_label = Some("mark".to_string());
    let mut sel = TreeSelector::new(vec![labeled]);
    let keymap = crate::keymap::SelectKeymap::default();
    sel.handle(
        &KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT),
        &keymap,
    );
    // Caret at 0: a typed character lands BEFORE the existing text.
    sel.handle(
        &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        &keymap,
    );
    let outcome = sel.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &keymap);
    assert!(
        matches!(&outcome, crate::SelectorOutcome::Apply(payload) if payload.ends_with("xmark")),
        "{outcome:?}"
    );
    let row: String = sel.rows(&theme)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(row.contains("[xmark] user: hello"), "{row:?}");
}
