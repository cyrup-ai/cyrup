//! Unified-diff renderer tests (spec/tui/06 §6; port of `components/diff.ts`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{render_diff, UiTheme};
use ratatui::style::Modifier;

fn plain(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn context_added_removed_lines_are_classified() {
    let theme = UiTheme::dark();
    let diff = " 1 unchanged\n-2 old line\n+2 new line\n+3 added";
    let lines = render_diff(diff, &theme);
    let text: Vec<String> = lines.iter().map(plain).collect();
    assert!(text.iter().any(|l| l.contains("unchanged")), "context kept: {text:?}");
    assert!(text.iter().any(|l| l.starts_with("-2 ")), "removed line: {text:?}");
    assert!(text.iter().any(|l| l.starts_with("+2 ")), "added line: {text:?}");
    assert!(text.iter().any(|l| l.starts_with("+3 ")), "standalone added: {text:?}");
}

#[test]
fn single_removed_then_added_gets_intra_line_inverse() {
    // One removed + one added line → word-level diff highlights only the changed token, reversed.
    let theme = UiTheme::dark();
    let diff = "-1 the quick brown fox\n+1 the slow brown fox";
    let lines = render_diff(diff, &theme);
    // Find the added line; the changed word ("slow") must carry the REVERSED modifier, the unchanged
    // words ("the ", "brown fox") must not.
    let added = lines.iter().find(|l| plain(l).starts_with("+1 ")).expect("added line");
    let reversed: String = added
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(reversed.contains("slow"), "changed token reversed, got `{reversed}`");
    assert!(!reversed.contains("brown"), "unchanged token must not be reversed: `{reversed}`");
}

#[test]
fn tabs_are_expanded_to_three_spaces() {
    let theme = UiTheme::dark();
    let lines = render_diff(" 1 \tindented", &theme);
    let joined: String = lines.iter().map(plain).collect();
    assert!(!joined.contains('\t'), "tabs expanded: {joined:?}");
    assert!(joined.contains("   indented"), "three-space indent: {joined:?}");
}
