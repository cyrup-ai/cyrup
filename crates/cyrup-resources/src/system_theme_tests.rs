//! TUI-130 — the `system` theme, against the output of pi's own generator. This crate owns the
//! generator, so it owns these tests and the two golden files beside them.
//!
//! `tests/system_theme_golden.json` and `tests/system_theme_random_golden.json` were produced by
//! RUNNING `generateSystemThemeColors` and `terminalAppearance` from pi's
//! `modes/interactive/theme/system-theme.ts` @v1.0.0 (with `409e808f5`'s chroma cap), together with
//! its `colors.ts`/`oklab.ts`, under Node 22 (V8) and again under Bun (JavaScriptCore) — the two
//! engines agree byte for byte, so the numbers do not depend on a `Math.pow` implementation. Nothing
//! here is re-derived from this port's own arithmetic.
//!
//! The first file is nineteen hand-picked terminals: real palettes (xterm, Dracula, One Light,
//! Solarized Light, Catppuccin Frappe), a mid-gray background that forces the relaxation search, a
//! weak foreground, a palette without a background, and the three tiers' degenerate inputs. The
//! second is forty-eight seeded random terminals.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value;

use super::*;

const GOLDEN: &str = include_str!("tests/system_theme_golden.json");
const RANDOM_GOLDEN: &str = include_str!("tests/system_theme_random_golden.json");

fn rgb_of(value: &Value) -> Rgb {
    let channel = |key: &str| u8::try_from(value[key].as_u64().unwrap()).unwrap();
    Rgb::new(channel("r"), channel("g"), channel("b"))
}

fn optional_rgb(value: &Value) -> Option<Rgb> {
    (!value.is_null()).then(|| rgb_of(value))
}

fn palette_of(value: &Value) -> Option<[Rgb; 16]> {
    if value.is_null() {
        return None;
    }
    let colors: Vec<Rgb> = value.as_array().unwrap().iter().map(rgb_of).collect();
    Some(<[Rgb; 16]>::try_from(colors).expect("pi's fixtures carry sixteen palette colours"))
}

fn appearance_of(value: &Value) -> Option<Appearance> {
    match value.as_str() {
        Some("dark") => Some(Appearance::Dark),
        Some("light") => Some(Appearance::Light),
        _ => None,
    }
}

/// pi's `string | number`: `"#rrggbb"`, an ANSI index, or `""` for the terminal default.
fn expected_color(value: &str) -> SystemColor {
    if value.is_empty() {
        SystemColor::Default
    } else if let Some(hex) = value.strip_prefix('#') {
        let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap(), 16).unwrap();
        SystemColor::Rgb(Rgb::new(channel(0), channel(2), channel(4)))
    } else {
        SystemColor::Indexed(value.parse().unwrap())
    }
}

fn input_of(fixture: &Value) -> SystemThemeInput {
    let input = &fixture["input"];
    SystemThemeInput {
        foreground: optional_rgb(&input["foreground"]),
        background: optional_rgb(&input["background"]),
        palette: palette_of(&input["palette"]),
        saturation: Saturation::new(input["saturation"].as_f64().unwrap_or(1.0)),
        appearance_hint: appearance_of(&input["appearanceHint"]),
    }
}

fn generated(name: &str) -> (SystemThemeInput, SystemThemeColors) {
    let fixtures: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    let fixture = fixtures
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("no fixture named {name}"));
    let input = input_of(fixture);
    let colors = generate_system_theme_colors(&input);
    (input, colors)
}

/// Compare one generated theme token by token, naming the first token that differs.
fn assert_matches_pi(
    label: &str,
    got: &SystemThemeColors,
    expected: &serde_json::Map<String, Value>,
) {
    let mut wrong = Vec::new();
    for (token, want) in expected {
        let want = match want {
            Value::String(s) => expected_color(s),
            Value::Number(n) => SystemColor::Indexed(u8::try_from(n.as_u64().unwrap()).unwrap()),
            other => panic!("unexpected colour {other}"),
        };
        match got.colors.get(token.as_str()) {
            Some(have) if *have == want => {}
            have => wrong.push(format!("{token}: pi {want:?}, here {have:?}")),
        }
    }
    assert!(
        wrong.is_empty(),
        "{label}: {} tokens differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert_eq!(
        got.colors.len(),
        expected.len(),
        "{label}: token sets differ"
    );
}

// ------------------------------------------------------------------ pi's own output ----

/// Every fixture, token for token: colours, the faint set, the appearance, and
/// `terminalAppearance` for the ones that reported a background.
#[test]
fn the_nineteen_fixtures_match_pis_generator_token_for_token() {
    let fixtures: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(fixtures.len(), 19);
    for fixture in &fixtures {
        let name = fixture["name"].as_str().unwrap();
        let input = input_of(fixture);
        let got = generate_system_theme_colors(&input);
        let output = &fixture["output"];
        assert_matches_pi(name, &got, output["colors"].as_object().unwrap());

        let dim: Vec<&str> = output["dim"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(got.dim, dim, "{name}: faint tokens");
        assert_eq!(
            got.appearance,
            appearance_of(&output["appearance"]),
            "{name}: appearance"
        );

        if let Some(background) = input.background {
            assert_eq!(
                Some(terminal_appearance(background, input.foreground)),
                appearance_of(&fixture["terminalAppearance"]),
                "{name}: terminalAppearance"
            );
        }
    }
}

/// Forty-eight seeded random terminals — random backgrounds biased to the dark and light ends,
/// random foregrounds, random palettes, some with a saturation below one.
#[test]
fn the_random_corpus_matches_pis_generator_token_for_token() {
    let cases: Vec<Value> = serde_json::from_str(RANDOM_GOLDEN).unwrap();
    assert_eq!(cases.len(), 48);
    let tokens: Vec<&str> = system_tokens().collect();
    for (index, case) in cases.iter().enumerate() {
        let i = &case["i"];
        let input = SystemThemeInput {
            background: optional_rgb(&i[0]),
            foreground: optional_rgb(&i[1]),
            palette: palette_of(&i[2]),
            saturation: Saturation::new(i[3].as_f64().unwrap_or(1.0)),
            appearance_hint: None,
        };
        let got = generate_system_theme_colors(&input);
        let expected: Vec<&str> = case["c"].as_str().unwrap().split(',').collect();
        assert_eq!(expected.len(), tokens.len(), "case {index}: token count");
        let wrong: Vec<String> = tokens
            .iter()
            .zip(&expected)
            .filter(|(token, want)| got.colors.get(*token) != Some(&expected_color(want)))
            .map(|(token, want)| format!("{token}: pi {want:?}, here {:?}", got.colors.get(token)))
            .collect();
        assert!(
            wrong.is_empty(),
            "case {index} {:?}:\n{}",
            i,
            wrong.join("\n")
        );
        assert_eq!(
            got.appearance,
            appearance_of(&case["a"]),
            "case {index}: appearance"
        );
    }
}

// ------------------------------------------------------------------ the recipe itself ----

/// The recipe is data: fourteen families, and every token the theme schema knows has a family.
#[test]
fn every_schema_token_belongs_to_a_family() {
    let tokens: Vec<&str> = system_tokens().collect();
    assert_eq!(tokens.len(), 56);
    let unique: std::collections::BTreeSet<&str> = tokens.iter().copied().collect();
    assert_eq!(unique.len(), tokens.len(), "no token is listed twice");
    for required in crate::REQUIRED_COLOR_TOKENS {
        assert!(unique.contains(required), "`{required}` has no family");
    }
    for optional in [
        "thinkingMax",
        "scrollbarTrack",
        "scrollbarThumb",
        "searchMatchBg",
        "searchMatchText",
    ] {
        assert!(unique.contains(optional), "`{optional}` has no family");
    }
    let families: std::collections::BTreeSet<String> = TOKEN_FAMILIES
        .iter()
        .map(|(_, family)| format!("{family:?}"))
        .collect();
    assert_eq!(
        families.len(),
        14,
        "all fourteen families are used: {families:?}"
    );
}

/// Pi's `SOLVE_ORDER`: every surface is solved before a token drawn on it.
#[test]
fn surfaces_are_solved_before_the_tokens_drawn_on_them() {
    let position = |token: &str| SOLVE_ORDER.iter().position(|t| *t == token).unwrap();
    for rule in RULES.iter() {
        for surface in &rule.on {
            if let Surface::Token(surface) = surface {
                assert!(
                    position(surface) < position(rule.token),
                    "{} is drawn on {surface} but solved before it",
                    rule.token
                );
            }
        }
    }
}

// ------------------------------------------------------------------ the three tiers ----

fn lightness_and_hue(color: &SystemColor) -> (f64, f64, f64) {
    let SystemColor::Rgb(rgb) = color else {
        panic!("expected a hex colour, got {color:?}");
    };
    let (h, s, _) = rgb_to_okhsl(rgb.channels());
    (h, s, rgb_to_oklch(rgb.channels()).1)
}

fn hue_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// Tier one: the hue of a palette-derived token follows the terminal's palette slot, not the
/// family's own hue.
#[test]
fn with_a_palette_the_hues_track_the_reported_slots() {
    let (input, theme) = generated("dark-dracula");
    let palette = input.palette.unwrap();
    // `accent` is the violet family, slot 5 (Dracula pink `#ff79c6`, hue ~350), not violet's 295.
    let (hue, _, _) = lightness_and_hue(theme.colors.get("accent").unwrap());
    let slot = rgb_to_okhsl(palette[5].channels()).0;
    assert!(
        hue_distance(hue, slot) < 4.0,
        "accent hue {hue} should follow palette slot 5 ({slot}), not violet's 295"
    );
    // …and `error`, the red family on slot 1.
    let (hue, _, _) = lightness_and_hue(theme.colors.get("error").unwrap());
    assert!(hue_distance(hue, rgb_to_okhsl(palette[1].channels()).0) < 4.0);
}

/// Tier two: no palette, so each family's own OKHSL hue.
#[test]
fn with_only_a_background_the_families_use_their_own_hues() {
    let (_, theme) = generated("background-only-black");
    for (token, hue) in [
        ("accent", 295.0),
        ("error", 20.0),
        ("success", 158.68),
        ("border", 231.49),
    ] {
        let (got, _, _) = lightness_and_hue(theme.colors.get(token).unwrap());
        assert!(
            hue_distance(got, hue) < 5.0,
            "{token}: hue {got} should be the family's {hue}"
        );
    }
}

/// `409e808f5`: a palette colour never gains OKLCH chroma when it moves to another lightness. The
/// Catppuccin Frappe pink `#f4b8e4` has chroma 0.089; the accent (violet family, slot 5) sits at
/// a very different lightness, where the same OKHSL saturation would be `#eb76d1` (chroma 0.180).
#[test]
fn a_pastel_palette_never_gains_chroma() {
    let (input, theme) = generated("pastel-catppuccin-frappe");
    let source = input.palette.unwrap()[5];
    assert_eq!(source, Rgb::new(0xf4, 0xb8, 0xe4));
    let source_chroma = rgb_to_oklch(source.channels()).1;
    assert!((source_chroma - 0.089).abs() < 0.001, "{source_chroma}");

    for token in ["accent", "borderAccent", "mdCode", "syntaxType"] {
        let (_, _, chroma) = lightness_and_hue(theme.colors.get(token).unwrap());
        assert!(
            chroma <= source_chroma + 0.006,
            "{token}: chroma {chroma:.3} exceeds the palette colour's {source_chroma:.3}"
        );
    }
}

/// Tier three: nothing reported ⇒ ANSI indices and the terminal's own defaults.
#[test]
fn a_silent_terminal_gets_ansi_indices_and_terminal_defaults() {
    let (_, theme) = generated("nothing-hint-dark");
    let idx = |token: &str| *theme.colors.get(token).unwrap();
    // Hues come from the palette SLOT the family names, which the terminal renders itself.
    assert_eq!(idx("accent"), SystemColor::Indexed(5));
    assert_eq!(idx("error"), SystemColor::Indexed(1));
    assert_eq!(idx("warning"), SystemColor::Indexed(3));
    assert_eq!(idx("border"), SystemColor::Indexed(4));
    assert_eq!(idx("success"), SystemColor::Indexed(2));
    // Two tokens override their family's slot to stay apart from a sibling.
    assert_eq!(idx("syntaxString"), SystemColor::Indexed(2));
    assert_eq!(idx("syntaxNumber"), SystemColor::Indexed(5));
    // Neutral tokens and every panel are the terminal default; neutral tokens below body text are
    // faint instead of bright black.
    assert_eq!(idx("text"), SystemColor::Default);
    assert_eq!(idx("muted"), SystemColor::Default);
    assert_eq!(idx("userMessageBg"), SystemColor::Default);
    assert_eq!(idx("toolSuccessBg"), SystemColor::Default);
    assert!(theme.dim.contains(&"muted") && theme.dim.contains(&"dim"));
    assert!(!theme.dim.contains(&"text"), "body text is not faint");
    assert!(
        !theme.dim.contains(&"toolPendingBg"),
        "a panel is not faint text"
    );
    assert_eq!(theme.appearance, Some(Appearance::Dark));
}

/// While the terminal's colours are still pending the theme is grayscale: no token has a colour.
#[test]
fn grayscale_has_no_colour_at_all() {
    let (_, theme) = generated("nothing-grayscale");
    assert!(theme.colors.values().all(|c| *c == SystemColor::Default));
}

#[test]
fn saturation_is_clamped_into_zero_to_one() {
    assert_eq!(Saturation::new(7.0).get(), 1.0);
    assert_eq!(Saturation::new(-2.0).get(), 0.0);
    assert_eq!(Saturation::new(f64::NAN).get(), 0.0);
    assert_eq!(Saturation::new(0.25).get(), 0.25);
}

// ------------------------------------------------------------------ strict theme schema ----

/// TUI-178 regression pin: the `system` theme's generated document, written out as theme JSON the
/// way `UiTheme`'s publication builds it, carries only keys pi 1.1's closed schema declares, so it
/// still loads through [`crate::Theme::parse`] for every one of pi's nineteen terminals.
#[test]
fn every_generated_system_theme_document_passes_the_strict_theme_check() {
    let fixtures: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    for fixture in &fixtures {
        let name = fixture["name"].as_str().unwrap();
        let generated = generate_system_theme_colors(&input_of(fixture));
        let colors: serde_json::Map<String, Value> = generated
            .colors
            .iter()
            .map(|(token, color)| {
                let value = match color {
                    SystemColor::Rgb(rgb) => Value::from(rgb.hex()),
                    SystemColor::Indexed(index) => Value::from(*index),
                    SystemColor::Default => Value::from(""),
                };
                ((*token).to_string(), value)
            })
            .collect();
        let document = serde_json::json!({ "name": SYSTEM_THEME_NAME, "colors": colors });
        crate::Theme::parse(
            &document.to_string(),
            None,
            crate::ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .unwrap_or_else(|e| panic!("{name}: generated system theme rejected: {e}"));
    }
}
