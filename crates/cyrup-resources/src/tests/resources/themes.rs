//! Themes — built-in dark/light, hot-reload + runtime switch, recursive vars and cycles,
//! required-token schema, malformed color rejection (A-09-3, G1/G2/G3/G10).

use std::time::Duration;

use super::fixtures::{cfg, full_theme_json, run_discover, write};
use crate::{ResourceScope, Theme, ThemeWatcher, builtin_themes};
use cyrup_core::CancelToken;

// ===========================================================================
// A-09-3 — themes: built-in dark/light, hot-reload, runtime switch
// ===========================================================================

#[tokio::test]
async fn a09_3_builtin_dark_and_light_present() {
    let builtins = builtin_themes();
    assert!(
        builtins.iter().any(|t| t.data.name == "dark"),
        "built-in dark exists (R-09-011)"
    );
    assert!(
        builtins.iter().any(|t| t.data.name == "light"),
        "built-in light exists (R-09-011)"
    );

    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path());
    let report = run_discover(&c).await;
    assert!(report.registry.themes.contains("dark"));
    assert!(report.registry.themes.contains("light"));
}

#[tokio::test]
async fn a09_3_theme_disable_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = cfg(tmp.path());
    c.enable_themes = false;
    let report = run_discover(&c).await;
    assert!(
        !report.registry.themes.contains("dark"),
        "--no-themes drops built-ins too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a09_3_theme_hot_reload_and_runtime_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let active = tmp.path().join("active.json");
    write(
        &active,
        &full_theme_json("mine", &[("bg", "#000000")], &[("background", "$bg")]),
    );

    let theme = Theme::load(&active, ResourceScope::Cli, crate::ResourceOrigin::Builtin).unwrap();
    let watcher = ThemeWatcher::spawn(
        std::sync::Arc::new(theme.data.clone()),
        active.clone(),
        CancelToken::new(),
    )
    .expect("theme watcher spawns");
    let mut rx = watcher.subscribe();
    assert_eq!(rx.borrow_and_update().name, "mine");

    // Mutate the active theme file; the watcher must publish the new theme (R-09-013).
    tokio::time::sleep(Duration::from_millis(120)).await;
    write(
        &active,
        &full_theme_json("mine", &[("bg", "#ffffff")], &[("background", "$bg")]),
    );

    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("hot-reload fired before timeout")
        .expect("watch channel open");
    assert_eq!(
        rx.borrow().vars.get("bg"),
        Some(&crate::ColorValue::from("#ffffff"))
    );

    // Runtime switch to a different theme file (R-09-014).
    let other = tmp.path().join("other.json");
    write(
        &other,
        &full_theme_json("other", &[], &[("foreground", "#abcdef")]),
    );
    watcher
        .retarget(other)
        .expect("retarget to a new active theme");
    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("retarget published before timeout")
        .expect("watch channel open");
    assert_eq!(rx.borrow().name, "other");
}

#[test]
fn theme_resolve_var_indirection_and_empty_value() {
    let theme = Theme::parse(
        &full_theme_json(
            "t",
            &[("bg", "#112233")],
            &[("background", "$bg"), ("blank", "")],
        ),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    let resolved = theme.resolve();
    assert_eq!(
        resolved.roles.get("background"),
        Some(&crate::ColorSpec::Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33
        })
    );
    // `""` is the terminal default, and the only value that resolves to `Inherit`.
    assert_eq!(
        resolved.roles.get("blank"),
        Some(&crate::ColorSpec::Inherit)
    );
}

/// TUI-131 — a `colors` value that is neither a known variable nor a colour is a LOAD error, as it
/// is in pi (`resolveVarRefs` throws `Variable reference not found`, `parseColor` throws
/// `Invalid color value`; `setTheme` reports either as `Failed to load theme …`). cyrup used to
/// resolve each of these to `Inherit` and load the theme anyway, so the role silently repainted
/// from a compiled fallback and nothing said so.
#[test]
fn an_unresolvable_color_value_fails_the_theme_load_with_pis_message() {
    for (colors, expected) in [
        // Not a variable and not a colour.
        (("bad", "nothex"), "Variable reference not found: nothex"),
        // A `#` value is parsed as written; this is neither `#RGB` nor `#RRGGBB`.
        (("bad", "#12345"), "Invalid color value: #12345"),
        (("bad", "#zzz"), "Invalid color value: #zzz"),
        // `parseColor` trims nothing.
        (("bad", "#abc "), "Invalid color value: #abc "),
        // A bare `rrggbb` is a variable name, not a colour.
        (("bad", "abcdef"), "Variable reference not found: abcdef"),
    ] {
        let err = Theme::parse(
            &full_theme_json("t", &[], &[colors]),
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .expect_err("an unresolvable value must be a load error, not a silent Inherit");
        assert!(
            err.to_string().contains(expected),
            "{colors:?}: wanted {expected:?}, got {err}"
        );
    }

    // A variable that resolves to garbage is reported at the value it ends on.
    let err = Theme::parse(
        &full_theme_json("t", &[("a", "#12")], &[("accent", "a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("a var that resolves to garbage fails the load");
    assert!(
        err.to_string().contains("Invalid color value: #12"),
        "{err}"
    );

    // An unreferenced variable is never resolved upstream, so it cannot fail the load either.
    Theme::parse(
        &full_theme_json("t", &[("unused", "#12")], &[]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect("pi resolves only what `colors` references");
}

// ===========================================================================
// Theme: recursive vars, cycle detection, 256-color index, name '/'
// ===========================================================================

#[test]
fn theme_recursive_vars_cycle_index_and_name_slash() {
    use crate::ColorSpec;

    // Multi-level var indirection: accent -> $a -> $b -> #0a141e (theme.ts:290-306).
    let t = Theme::parse(
        &full_theme_json("t", &[("a", "$b"), ("b", "#0a141e")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        t.resolve().roles.get("accent"),
        Some(&ColorSpec::Rgb {
            r: 0x0a,
            g: 0x14,
            b: 0x1e
        })
    );

    // A circular reference is a load error, as it is in pi (`resolveVarRefs` throws
    // `Circular variable reference detected`, `theme.ts:143-145`).
    let cyc = Theme::parse(
        &full_theme_json("c", &[("a", "$b"), ("b", "$a")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("a reference cycle cannot load");
    assert!(
        cyc.to_string()
            .contains("Circular variable reference detected"),
        "{cyc}"
    );

    // Integer 256-color index 196 stays a palette INDEX (theme.ts:23-28, `parseColor` →
    // `indexedColor`): it is emitted as `38;5;196`, not as the xterm RGB for it, because indices
    // 0-15 are the user's own terminal palette.
    let idx = Theme::parse(
        &full_theme_json("i", &[], &[("accent", "196")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        idx.resolve().roles.get("accent"),
        Some(&ColorSpec::Indexed(196))
    );
    assert_eq!(ColorSpec::Indexed(196).to_rgb(), Some((255, 0, 0)));

    // A '/' in the theme name is rejected even when the schema is otherwise complete
    // (theme.ts:506-512). Tokens are all present so validation reaches the name check.
    assert!(
        Theme::parse(
            &full_theme_json("a/b", &[], &[]),
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .is_err(),
        "theme name with '/' rejected"
    );
}

// ===========================================================================
// Theme required-token schema validation + full built-in token sets (G1/G2/G10)
// ===========================================================================

#[test]
fn theme_missing_required_tokens_is_rejected_with_pi_error() {
    // A theme that omits required color tokens fails validation with Pi's exact, sorted
    // "Missing required color tokens" message (theme.ts:514-548).
    let err = Theme::parse(
        r##"{"name":"sparse","colors":{"accent":"#ffffff"}}"##,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("incomplete theme must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Missing required color tokens"),
        "Pi error text: {msg}"
    );
    // A representative token from each schema section is reported as missing.
    for token in ["syntaxKeyword", "mdHeading", "thinkingHigh", "bashMode"] {
        assert!(msg.contains(token), "missing token `{token}` listed: {msg}");
    }
    // The present token is NOT reported as missing.
    assert!(
        !msg.contains("- accent\n"),
        "provided token must not be flagged: {msg}"
    );

    // A complete theme parses cleanly.
    assert!(
        Theme::parse(
            &full_theme_json("complete", &[], &[]),
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin,
        )
        .is_ok(),
        "schema-complete theme accepted"
    );
}

#[test]
fn builtin_themes_carry_full_token_set_and_export() {
    use crate::ColorSpec;
    let builtins = builtin_themes();
    let dark = builtins
        .iter()
        .find(|t| t.data.name == "dark")
        .expect("dark builtin");

    // Every required token resolves (no incomplete role map) — the gap that left cyrup-tui unable
    // to render (theme.rs:276-307 stub had only 4 non-Pi tokens).
    let resolved = dark.resolve();
    for token in crate::REQUIRED_COLOR_TOKENS {
        assert!(
            resolved.roles.contains_key(token),
            "dark resolves `{token}`"
        );
    }
    // Pi v1.0.0's `dark.json` is written entirely in `okhsl(…)`. Every expected triple below is
    // `colorToRgb(parseColor(value))` run through pi's own `colors.ts` @v1.0.0.
    //
    // A literal `okhsl()` token…
    assert_eq!(
        resolved.roles.get("syntaxVariable"),
        Some(&ColorSpec::Rgb {
            r: 93,
            g: 179,
            b: 186
        }),
        "syntaxVariable = okhsl(202 58% 67%)"
    );
    // …and a var-indirected one.
    assert_eq!(
        resolved.roles.get("success"),
        Some(&ColorSpec::Rgb {
            r: 104,
            g: 183,
            b: 141
        }),
        "success -> green -> okhsl(159 59% 67%)"
    );

    // Typed export section resolves for HTML export (theme.ts:94-100; G10).
    let export = dark.resolve_export();
    assert_eq!(
        export.page_bg,
        ColorSpec::Rgb {
            r: 33,
            g: 37,
            b: 44
        }
    );
    assert_eq!(
        export.card_bg,
        ColorSpec::Rgb {
            r: 40,
            g: 44,
            b: 52
        }
    );
    assert_eq!(
        export.info_bg,
        ColorSpec::Rgb {
            r: 78,
            g: 47,
            b: 27
        }
    );

    let light = builtins
        .iter()
        .find(|t| t.data.name == "light")
        .expect("light builtin");
    assert!(
        crate::REQUIRED_COLOR_TOKENS
            .iter()
            .all(|t| light.resolve().roles.contains_key(*t)),
        "light builtin also carries the full token set"
    );
}

// ===========================================================================
// Theme malformed color value rejection + "Other errors" section (G3)
// ===========================================================================

#[test]
fn theme_out_of_range_int_color_rejected_with_other_errors() {
    // Pi's ColorValueSchema is `String | Integer(0..255)`; an integer > 255 fails the union
    // (theme.ts:23-26) and is reported in the "Other errors" section (theme.ts:528-545). cyrup
    // must reject it rather than silently coerce it to inherit.
    let json = full_theme_json("oor", &[], &[("accent", "300")]);
    let err = Theme::parse(
        &json,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("out-of-range color index must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Other errors:"),
        "other-errors section present: {msg}"
    );
    assert!(
        msg.contains("/colors/accent"),
        "offending path reported: {msg}"
    );
    assert!(
        !msg.contains("Missing required color tokens"),
        "no tokens missing — only the malformed value is reported: {msg}"
    );
}

#[test]
fn theme_non_scalar_color_value_rejected_with_combined_message() {
    // A boolean color value is neither string nor integer → rejected. Because only `accent` is
    // present, the message carries BOTH the missing-token section and the "Other errors" section,
    // mirroring Pi's combined error assembly (theme.ts:533-545).
    let json = r#"{"name":"bad","colors":{"accent":true}}"#;
    let err = Theme::parse(
        json,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("non-scalar color value must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Missing required color tokens"),
        "missing section present: {msg}"
    );
    assert!(
        msg.contains("Other errors:"),
        "other section present: {msg}"
    );
    assert!(
        msg.contains("/colors/accent: Expected union value"),
        "bad value path + message: {msg}"
    );
}

#[test]
fn theme_valid_int_and_string_colors_still_accepted() {
    // Regression guard: in-range integer indices and hex/var strings remain valid (theme.ts:23-26).
    let json = full_theme_json("ok", &[("v", "12")], &[("accent", "196"), ("text", "$v")]);
    assert!(
        Theme::parse(
            &json,
            None,
            ResourceScope::Builtin,
            crate::ResourceOrigin::Builtin
        )
        .is_ok(),
        "valid int + string color values accepted"
    );
}

// ===========================================================================
// TUI-132 — `appearance`: declared, or detected from the theme's own colours
// ===========================================================================

/// A schema-complete theme whose foreground tokens all carry `fg` and whose background tokens all
/// carry `bg` — the uniform fixture the expected `appearance` values below were generated from by
/// running pi's `detectAppearance` + the `Theme` constructor's token split (`theme.ts:221-306`
/// @v1.0.0) over the same colours.
fn uniform_theme(fg: &str, bg: &str, appearance: Option<&str>) -> String {
    const BACKGROUNDS: [&str; 6] = [
        "selectedBg",
        "userMessageBg",
        "customMessageBg",
        "toolPendingBg",
        "toolSuccessBg",
        "toolErrorBg",
    ];
    let colors: Vec<(&str, &str)> = crate::REQUIRED_COLOR_TOKENS
        .iter()
        .map(|token| (*token, if BACKGROUNDS.contains(token) { bg } else { fg }))
        .collect();
    let json = full_theme_json("uniform", &[], &colors);
    match appearance {
        Some(a) => json.replacen('{', &format!("{{\"appearance\":\"{a}\","), 1),
        None => json,
    }
}

fn appearance_of(json: &str) -> Option<crate::Appearance> {
    Theme::parse(
        json,
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap()
    .appearance()
}

#[test]
fn the_builtins_declare_their_appearance() {
    use crate::Appearance;
    let builtins = builtin_themes();
    let appearance = |name: &str| {
        builtins
            .iter()
            .find(|t| t.data.name == name)
            .and_then(|t| t.data.appearance)
    };
    assert_eq!(appearance("dark"), Some(Appearance::Dark));
    assert_eq!(appearance("light"), Some(Appearance::Light));
}

/// With the field removed, the built-ins' own colours still classify them (`dark.json` and
/// `light.json` run through pi's detector give `dark` and `light`).
#[test]
fn appearance_is_detected_from_the_colours_when_the_field_is_omitted() {
    use crate::Appearance;
    for (json, expected) in [
        (crate::BUILTIN_DARK_JSON, Appearance::Dark),
        (crate::BUILTIN_LIGHT_JSON, Appearance::Light),
    ] {
        let without = json
            .lines()
            .filter(|line| !line.trim_start().starts_with("\"appearance\""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!without.contains("\"appearance\""), "field really removed");
        assert_eq!(appearance_of(&without), Some(expected));
    }
}

/// Pi's detector, case by case. Each row is `(foreground, background, expected)`; the expectations
/// come from running the pi code, not from this implementation.
#[test]
fn detected_appearance_follows_pis_lightness_comparison() {
    use crate::Appearance::{Dark, Light};
    for (fg, bg, expected) in [
        // Backgrounds darker than foregrounds ⇒ dark, and the converse.
        ("#e0e0e0", "#101010", Some(Dark)),
        ("#202020", "#f0f0f0", Some(Light)),
        ("#808080", "#101010", Some(Dark)),
        // Pi's comparison is relative, so two light sets with the lighter FOREGROUND read as dark.
        ("#f8f8f8", "#f0f0f0", Some(Dark)),
        // Palette indices 0-15 follow the user's terminal, so they say nothing about the theme:
        // the foregrounds are skipped and the backgrounds alone decide, at 0.5.
        ("7", "#101010", Some(Dark)),
        ("7", "#f0f0f0", Some(Light)),
        // The rows that tell "skipped" from "counted": index 0 is black and 15 is white, so counting
        // them would flip the verdict (black text on a dark ground reads as light, and vice versa).
        ("0", "#101010", Some(Dark)),
        ("15", "#f0f0f0", Some(Light)),
        // Indices from 16 up are fixed colours and DO count (231 is white, 16 is black).
        ("231", "#101010", Some(Dark)),
        ("16", "#f0f0f0", Some(Light)),
    ] {
        assert_eq!(
            appearance_of(&uniform_theme(fg, bg, None)),
            expected,
            "fg {fg} on bg {bg}"
        );
    }
}

#[test]
fn a_theme_with_no_concrete_colour_has_no_appearance_of_its_own() {
    assert_eq!(appearance_of(&uniform_theme("", "", None)), None);
}

/// TUI-132's second verify item: a declared `appearance` beats the lightness average. This theme's
/// colours average DARK; declaring `light` must win.
#[test]
fn a_declared_appearance_beats_the_detected_one() {
    use crate::Appearance::{Dark, Light};
    assert_eq!(
        appearance_of(&uniform_theme("#e0e0e0", "#101010", None)),
        Some(Dark)
    );
    assert_eq!(
        appearance_of(&uniform_theme("#e0e0e0", "#101010", Some("light"))),
        Some(Light)
    );
    assert_eq!(
        appearance_of(&uniform_theme("#202020", "#f0f0f0", Some("dark"))),
        Some(Dark)
    );
}

#[test]
fn an_appearance_outside_the_enum_is_a_schema_error() {
    let err = Theme::parse(
        &uniform_theme("#e0e0e0", "#101010", Some("sepia")),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("only dark and light are valid appearances");
    let msg = err.to_string();
    assert!(msg.contains("/appearance"), "{msg}");
    assert!(msg.contains("Expected union value"), "{msg}");
}
