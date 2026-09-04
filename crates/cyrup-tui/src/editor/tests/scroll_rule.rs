#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::editor::render::scroll_border;
use crate::editor::*;

// ------------------------------------------------------------- createScrollBorder ------------

/// The wide path (`editor.ts:262-263`): the indicator, then `─` to the requested width.
#[test]
fn the_scroll_rule_reads_as_an_indicator_padded_with_rule() {
    let s = scroll_border('↑', 6, 20);
    assert!(s.starts_with("─── ↑ 6 more "), "{s:?}");
    assert_eq!(display_width(&s), 20, "{s:?}");
    assert!(s.ends_with('─'), "the remainder is rule, not blank: {s:?}");
}

/// The narrow path (`editor.ts:265-267`): a strict slice of the indicator plus `...`, itself
/// clipped on a terminal too narrow even for that.
#[test]
fn a_terminal_too_narrow_for_the_indicator_gets_an_ellipsis() {
    assert_eq!(
        scroll_border('↓', 5, 10),
        "─── ↓ 5...",
        "{:?}",
        scroll_border('↓', 5, 10)
    );
    assert_eq!(scroll_border('↓', 5, 2), "..");
    assert_eq!(scroll_border('↓', 5, 0), "");
}

/// The invariant the render depends on: the string is EXACTLY `width` columns for every width
/// and every hidden count, so it overwrites the `Block`'s pre-painted rule with no `─` leaking
/// out from underneath. (Upstream can be one column short here — `strict` may reject a wide
/// grapheme at the boundary and nothing pads afterwards — but the indicator's alphabet is
/// entirely single-column, so the case does not arise. See [`scroll_border`].)
#[test]
fn the_scroll_rule_is_exactly_as_wide_as_it_is_asked_for() {
    for direction in ['↑', '↓'] {
        for hidden in [0usize, 1, 9, 10, 99, 1234, 1_000_000] {
            for width in 0..=120u16 {
                let s = scroll_border(direction, hidden, width);
                assert_eq!(
                    display_width(&s),
                    usize::from(width),
                    "scroll_border({direction:?}, {hidden}, {width}) = {s:?}"
                );
            }
        }
    }
}
