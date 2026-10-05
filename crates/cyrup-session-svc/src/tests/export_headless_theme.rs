//! The HTML export with no TUI attached: pi's default theme at v1.0.0 is `system`
//! (`getResolvedThemeColors` falls back through `themeName ?? currentThemeName ?? SYSTEM_THEME_NAME`,
//! `theme.ts:903-906`), and with no terminal behind it nothing was reported, so the generator runs
//! at its third tier (ANSI palette indices and the terminal's default colours) and `Theme.colors`
//! (`theme.ts:321-338`) turns that into concrete colours.
//!
//! `export_headless_system_dark.json` and `export_headless_system_light.json` were produced by
//! RUNNING pi's own `generateSystemThemeColors({ saturation: 1, appearanceHint })` from
//! `modes/interactive/theme/system-theme.ts` @v1.0.0 (under Bun) and applying `Theme.colors`'
//! arithmetic to it with pi's `parseColor`, `mixColors` and `colorToHex` from `packages/tui/src`:
//! an index is the xterm colour it names, `""` is the guessed default foreground (`#e5e5e7` /
//! `#000000`) or background (`#000000` / `#ffffff`) of the theme's appearance, and a faint token is
//! mixed 40% toward that background in OKLCH. Nothing here is derived from this port's arithmetic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use cyrup_resources::Appearance;
use serde_json::Value;

use crate::export::{
    ExportState, ExportTheme, session_jsonl_to_html, session_jsonl_to_html_with_theme,
};

const DARK: &str = include_str!("export_headless_system_dark.json");
const LIGHT: &str = include_str!("export_headless_system_light.json");

const JSONL: &str = concat!(
    r#"{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000","timestamp":"2026-09-04T10:00:00.000Z","cwd":"/home/dev/proj"}"#,
    "\n",
    r#"{"type":"message","id":"aaaaaaaa","parentId":null,"timestamp":"2026-09-04T10:00:01.000Z","message":{"role":"user","content":"hello"}}"#,
    "\n",
);

/// The tokens pi's generator + `Theme.colors` produced, by token.
fn pi_colors(golden: &str) -> BTreeMap<String, String> {
    let value: Value = serde_json::from_str(golden).unwrap();
    value["out"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(token, hex)| (token.clone(), hex.as_str().unwrap().to_string()))
        .collect()
}

/// Every one of the 56 tokens of the headless palette is what pi computes, for a dark and a light
/// hint.
#[test]
fn a_headless_export_carries_pis_tier_three_system_theme() {
    for (golden, appearance) in [(DARK, Appearance::Dark), (LIGHT, Appearance::Light)] {
        let expected = pi_colors(golden);
        assert_eq!(expected.len(), 56);
        let theme = ExportTheme::headless(appearance);
        for (token, hex) in &expected {
            let got = theme
                .role(token)
                .unwrap_or_else(|| panic!("{appearance:?}: `{token}` is missing"))
                .to_string();
            assert_eq!(&got, hex, "{appearance:?}: `{token}`");
        }
    }
}

/// The spot checks the arithmetic is easy to see in: an index is the xterm colour, a default is the
/// guessed terminal colour, a faint token is 40% of the way toward the background.
#[test]
fn the_three_colour_kinds_resolve_the_way_theme_colors_resolves_them() {
    let dark = ExportTheme::headless(Appearance::Dark);
    let hex = |theme: &ExportTheme, token: &str| theme.role(token).unwrap().to_string();
    // ANSI slot 5 (magenta) for the violet family; slot 1 (red); slot 2 (green).
    assert_eq!(hex(&dark, "accent"), "#800080");
    assert_eq!(hex(&dark, "error"), "#800000");
    assert_eq!(hex(&dark, "success"), "#008000");
    // `""`: the guessed foreground for body text, the guessed background for a panel.
    assert_eq!(hex(&dark, "text"), "#e5e5e7");
    assert_eq!(hex(&dark, "userMessageBg"), "#000000");
    // A faint neutral: `#e5e5e7` mixed 0.4 toward `#000000` in OKLCH.
    assert_eq!(hex(&dark, "muted"), "#727273");

    let light = ExportTheme::headless(Appearance::Light);
    assert_eq!(hex(&light, "text"), "#000000");
    assert_eq!(hex(&light, "userMessageBg"), "#ffffff");
}

/// The default export theme IS the headless one — not the compiled-in `dark` document it used to
/// be — and it reaches the stylesheet.
#[test]
fn the_default_export_theme_is_the_headless_system_theme() {
    let hint = std::env::var("COLORFGBG")
        .ok()
        .as_deref()
        .and_then(Appearance::from_colorfgbg)
        .unwrap_or(Appearance::Dark);
    assert_eq!(ExportTheme::default(), ExportTheme::headless(hint));

    // Pinned to a hint so the stylesheet assertion does not depend on the environment.
    let html = session_jsonl_to_html_with_theme(
        JSONL,
        &ExportTheme::headless(Appearance::Dark),
        &ExportState::from_file(),
    );
    assert!(html.contains("--accent: #800080;"), "slot-5 magenta");
    assert!(html.contains("--text: #e5e5e7;"), "the guessed foreground");
    assert!(html.contains("--muted: #727273;"), "a faint neutral");
    // The old default: the dark built-in's accent is not in the document.
    let builtin = cyrup_resources::BUILTIN_DARK_JSON;
    let value: Value = serde_json::from_str(builtin).unwrap();
    let old_accent = value["vars"]["accent"].as_str().unwrap_or_default();
    if !old_accent.is_empty() {
        assert!(!html.contains(&format!("--accent: {old_accent};")));
    }
    // And the file/stdin entry point reaches the same default.
    assert_eq!(
        session_jsonl_to_html(JSONL),
        session_jsonl_to_html_with_theme(JSONL, &ExportTheme::default(), &ExportState::from_file())
    );
}

/// The backdrops derive from `userMessageBg`: a generated theme has no `export` block
/// (`getThemeExportColors`, `theme.ts:924-925`).
#[test]
fn the_headless_backdrops_derive_from_the_guessed_panel() {
    let theme = ExportTheme::headless(Appearance::Dark);
    let panel = theme.role("userMessageBg").unwrap();
    assert_eq!(
        theme.backdrops(),
        crate::export::derive_export_colors(Some(panel))
    );
}
