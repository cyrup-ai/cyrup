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
        rx.borrow().vars.get("bg").map(String::as_str),
        Some("#ffffff")
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
fn theme_resolve_var_indirection_and_bad_hex() {
    let theme = Theme::parse(
        &full_theme_json(
            "t",
            &[("bg", "#112233")],
            &[("background", "$bg"), ("bad", "nothex"), ("blank", "")],
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
    assert_eq!(resolved.roles.get("bad"), Some(&crate::ColorSpec::Inherit));
    assert_eq!(
        resolved.roles.get("blank"),
        Some(&crate::ColorSpec::Inherit)
    );
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

    // Circular reference degrades to Inherit (Pi throws; cyrup is total).
    let cyc = Theme::parse(
        &full_theme_json("c", &[("a", "$b"), ("b", "$a")], &[("accent", "$a")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(cyc.resolve().roles.get("accent"), Some(&ColorSpec::Inherit));

    // Integer 256-color index 196 → bright red via the xterm palette (theme.ts:23-28).
    let idx = Theme::parse(
        &full_theme_json("i", &[], &[("accent", "196")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        idx.resolve().roles.get("accent"),
        Some(&ColorSpec::Rgb { r: 255, g: 0, b: 0 })
    );

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
    // A var-indirected token and a literal-hex token resolve to the Pi values.
    assert_eq!(
        resolved.roles.get("syntaxKeyword"),
        Some(&ColorSpec::Rgb {
            r: 0x56,
            g: 0x9c,
            b: 0xd6
        }),
        "syntaxKeyword = #569CD6 (literal hex from dark.json)"
    );
    assert_eq!(
        resolved.roles.get("success"),
        Some(&ColorSpec::Rgb {
            r: 0xb5,
            g: 0xbd,
            b: 0x68
        }),
        "success -> $green -> #b5bd68 (var indirection)"
    );

    // Typed export section resolves for HTML export (theme.ts:94-100; G10).
    let export = dark.resolve_export();
    assert_eq!(
        export.page_bg,
        ColorSpec::Rgb {
            r: 0x18,
            g: 0x18,
            b: 0x1e
        }
    );
    assert_eq!(
        export.card_bg,
        ColorSpec::Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x24
        }
    );
    assert_eq!(
        export.info_bg,
        ColorSpec::Rgb {
            r: 0x3c,
            g: 0x37,
            b: 0x28
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
