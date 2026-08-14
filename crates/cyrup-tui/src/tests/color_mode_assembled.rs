//! Feature #3 — `ColorMode` + `rgb_to_256`, proven by an **assembled** `App` render on a 256-color
//! backend. Without a color mode, a 256-color terminal receives truecolor (`Color::Rgb`) escapes and
//! mangles them. With `UiTheme::with_color_mode(ColorMode::Ansi256)` applied at the style-projection
//! boundary, every role color is quantized to an indexed palette entry (`Color::Indexed`) via
//! `rgb_to_256` (Pi `hexTo256`/`fgAnsi`, `theme.ts:222-283`). Here we render the WHOLE app — footer,
//! editor, and an open `/model` selector — through a `TestBackend` and assert **no cell carries a
//! truecolor `Color::Rgb`** while indexed colors ARE used.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{rgb_to_256, App, ColorMode, ModelEntry, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

fn model() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "claude-opus-4-6".into(),
            name: "Claude Opus 4.6".into(),
            provider: "anthropic".into(),
            current: true,
            scoped: false,
        },
        ModelEntry {
            id: "gpt-5.1".into(),
            name: "GPT 5.1".into(),
            provider: "openai".into(),
            current: false,
            scoped: false,
        },
    ]
}

#[test]
fn assembled_256color_render_emits_indexed_never_truecolor() {
    // Boot the app with the dark theme projected into 256-color mode (the ThemeController does this at
    // boot from `ColorMode::detect`). `App::new` adopts the theme's color mode, so every subsequent
    // theme touch re-projects too.
    let theme = UiTheme::dark().with_color_mode(ColorMode::Ansi256);
    assert_eq!(theme.color_mode, ColorMode::Ansi256);
    let mut app = App::new(TestBackend::new(100, 30), theme).unwrap();
    assert_eq!(app.color_mode(), ColorMode::Ansi256, "app did not adopt the theme color mode");

    // A colored, assembled surface: seeded footer + a committed exchange + an OPEN model selector
    // (accent/muted/success roles all in play).
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("high");
    app.transcript_mut().push_assistant_delta("indexing the workspace");
    app.open_model_selector(model(), None);
    app.draw().unwrap();

    let buf = app.terminal().backend().buffer();
    let mut indexed = 0usize;
    for cell in buf.content() {
        assert!(
            !matches!(cell.fg, Color::Rgb(_, _, _)),
            "a truecolor fg escaped the 256-color projection: {:?}",
            cell.fg
        );
        assert!(
            !matches!(cell.bg, Color::Rgb(_, _, _)),
            "a truecolor bg escaped the 256-color projection: {:?}",
            cell.bg
        );
        if matches!(cell.fg, Color::Indexed(_)) || matches!(cell.bg, Color::Indexed(_)) {
            indexed += 1;
        }
    }
    assert!(indexed > 0, "no indexed colors were used — the 256-color projection did nothing");
}

#[test]
fn assembled_truecolor_render_keeps_rgb_for_contrast() {
    // The default (truecolor) app DOES emit `Color::Rgb`, confirming the 256-color assertion above is a
    // real behavioral difference, not a theme that happens to have no colors.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    assert_eq!(app.color_mode(), ColorMode::TrueColor);
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    app.open_model_selector(model(), None);
    app.draw().unwrap();
    let has_rgb = app
        .terminal()
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| matches!(c.fg, Color::Rgb(_, _, _)) || matches!(c.bg, Color::Rgb(_, _, _)));
    assert!(has_rgb, "truecolor mode should keep RGB roles (the 256-color test proves the contrast)");
}

#[test]
fn rgb_to_256_matches_pi_reference_points() {
    // Reference quantizations from Pi `rgbTo256` (`theme.ts:222-253`).
    // Pure white → last grayscale index (255) is out of the 8..238 ramp, so it lands on the cube
    // white 231 (spread 0, but grayscale value 238 is closer? white=255: cube white dist 0 → 231).
    assert_eq!(rgb_to_256(0xff, 0xff, 0xff), 231, "white → cube white 231");
    assert_eq!(rgb_to_256(0x00, 0x00, 0x00), 16, "black → cube black 16");
    // A mid-gray `#808080` sits exactly on the grayscale ramp (value 128 = index 244) and is closer to
    // the ramp than to any cube cell → grayscale (Pi's spread<10 && grayDist<cubeDist branch).
    assert_eq!(rgb_to_256(0x80, 0x80, 0x80), 244, "#808080 → grayscale ramp index 244");
    // A saturated teal `#8abeb7` keeps its hue → the cube (16..=231), not grayscale.
    let teal = rgb_to_256(0x8a, 0xbe, 0xb7);
    assert!((16..=231).contains(&teal), "saturated teal → cube, got {teal}");
}
