//! TUI-130 — the `system` theme as the renderer's [`crate::UiTheme`].
//!
//! The generator itself is pinned to the output of pi's own `generateSystemThemeColors` by the
//! golden tests in `cyrup-resources` (`system_theme_tests.rs`, `tests/system_theme_*golden.json`),
//! which own that code. What belongs here is what this crate does with the colours it generates.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_resources::color::Rgb;

use super::*;

// ------------------------------------------------------------------ as a UiTheme ----

/// `UiTheme::builtin_named("system")` is `Some` — the row's standing requirement — and the roles
/// carry the three colour kinds: hex as RGB, indices as `Color::Indexed`, `""` as `Color::Reset`.
#[test]
fn the_system_theme_is_a_builtin_and_projects_all_three_colour_kinds() {
    use ratatui::style::{Color, Modifier};
    let theme = crate::UiTheme::builtin_named("system").expect("`system` is a built-in theme");
    assert_eq!(theme.name, "system");
    assert_eq!(theme.roles.len(), 56);
    // No terminal colours ⇒ tier three.
    assert_eq!(theme.roles.get("accent"), Some(&Color::Indexed(5)));
    assert_eq!(theme.roles.get("text"), Some(&Color::Reset));
    assert_eq!(theme.accent, Some(Color::Indexed(5)));
    assert_eq!(theme.foreground, Some(Color::Reset));
    // …with the neutral tokens faint (SGR 2) on top of the terminal default.
    assert!(theme.muted_style().add_modifier.contains(Modifier::DIM));
    assert!(theme.dim_style().add_modifier.contains(Modifier::DIM));
    assert!(!theme.base_style().add_modifier.contains(Modifier::DIM));

    // A terminal that reported its background (Dracula's `#282a36`): tier two, every token a hex
    // colour, and nothing left to render faint.
    let input = SystemThemeInput {
        background: Some(Rgb::new(0x28, 0x2a, 0x36)),
        foreground: Some(Rgb::new(0xf8, 0xf8, 0xf2)),
        ..SystemThemeInput::default()
    };
    let generated_theme = crate::UiTheme::system(&input);
    assert!(matches!(
        generated_theme.roles.get("accent"),
        Some(Color::Rgb(..))
    ));
    assert!(
        !generated_theme
            .muted_style()
            .add_modifier
            .contains(Modifier::DIM),
        "a terminal that reported colours needs no faint fallback"
    );
}

/// The renderer reaches the generator through this crate's re-export, and it is the one in
/// `cyrup-resources` — there is no second copy to drift.
#[test]
fn the_reexport_is_the_resources_generator() {
    assert_eq!(
        SYSTEM_THEME_NAME,
        cyrup_resources::system_theme::SYSTEM_THEME_NAME
    );
    let input = SystemThemeInput::default();
    let here = generate_system_theme_colors(&input);
    let there = cyrup_resources::system_theme::generate_system_theme_colors(&input);
    assert_eq!(here.colors, there.colors);
    assert_eq!(here.dim, there.dim);
}
