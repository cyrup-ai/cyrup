#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::editor::render::scroll_border;
use crate::editor::*;

// ------------------------------------------------------------- createScrollBorder ------------

/// The wide path (`editor.ts:279-281` @v0.87.1, v0.85.0 on): the label centred, `floor` of the
/// spare columns as rule on the left and the rest on the right.
#[test]
fn the_scroll_rule_centres_its_label_when_it_fits() {
    assert_eq!(
        scroll_border('↑', 3, 40),
        format!("{} ↑ 3 more {}", "─".repeat(15), "─".repeat(15))
    );
    assert_eq!(
        scroll_border('↓', 3, 41),
        format!("{} ↓ 3 more {}", "─".repeat(15), "─".repeat(16)),
        "an odd spare column goes to the right"
    );
    assert_eq!(
        scroll_border('↑', 6, 12),
        "─ ↑ 6 more ─",
        "one rule cell each side is still the centred form"
    );
}

/// The narrow path (`editor.ts:288-291`): below `label + 2` columns, a strict slice of the
/// left-anchored `─── ↑ N more ` plus `...`, itself clipped on a terminal too narrow even for
/// that.
#[test]
fn a_terminal_too_narrow_for_the_indicator_gets_an_ellipsis() {
    assert_eq!(
        scroll_border('↓', 5, 10),
        "─── ↓ 5...",
        "{:?}",
        scroll_border('↓', 5, 10)
    );
    assert_eq!(scroll_border('↑', 6, 11), "─── ↑ 6 ...");
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
