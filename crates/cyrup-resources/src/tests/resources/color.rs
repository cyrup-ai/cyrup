//! TUI-131 — `okhsl(…)` / `oklch(…)` theme colour values.
//!
//! Every expected channel triple below was produced by RUNNING pi's own code at the tag:
//! `packages/tui/src/colors.ts` + `oklab.ts` @v1.0.0 through `parseColor` → `colorToRgb`. They are
//! not re-derived from this port's own arithmetic.

use crate::color::{is_color_function, parse_color};

/// The values pi 1.0's own `theme/dark.json` is written in, plus the OKLCH forms the schema
/// advertises ("Hex color (#RGB or #RRGGBB), OKLCH or OKHSL color, variable reference, or empty
/// string for terminal default" — `theme/theme-schema.json:29`).
///
/// RED before this row: `parse_hex` took `#RGB`/`#RRGGBB` only, so every one of these was `None`
/// and the role silently fell back to a compiled hex.
#[test]
fn okhsl_and_oklch_values_resolve_to_pis_own_channels() {
    // `okhsl(h s l)` — pi `okhslColor` → `okhslToRgb`.
    assert_eq!(parse_color("okhsl(234 3% 89%)"), Some((222, 224, 225)));
    assert_eq!(parse_color("okhsl(229 6% 67%)"), Some((157, 165, 169)));
    // dark.json's new `accent` (TUI-132's violet).
    assert_eq!(parse_color("okhsl(295 50% 67%)"), Some((167, 152, 215)));
    // The two achromatic ends, which take `okhslToRgb`'s `l <= 0 || l >= 1 || s <= 0` shortcut.
    assert_eq!(parse_color("okhsl(0 0% 0%)"), Some((0, 0, 0)));
    assert_eq!(parse_color("okhsl(0 0% 100%)"), Some((255, 255, 255)));
    // Unitless saturation/lightness are 0-1, so `1` is full saturation and not 1%.
    assert_eq!(parse_color("okhsl(120 1 0.5)"), Some((109, 130, 0)));
    // A negative hue wraps forward: `((h % 360) + 360) % 360`, and `deg` is accepted on the hue.
    assert_eq!(parse_color("okhsl(-30deg 0.5 0.5)"), Some((157, 92, 151)));
    // The function name and its units are case-insensitive, and `\s+` is any run of whitespace.
    assert_eq!(
        parse_color("OKHSL( 234   3%   89% )"),
        Some((222, 224, 225))
    );

    // `oklch(l c h)` — pi `oklchColor` → `oklchToRgb`, including the `%` on lightness only.
    assert_eq!(parse_color("oklch(0.7 0.1 230)"), Some((84, 170, 209)));
    assert_eq!(parse_color("oklch(70% 0.1 230)"), Some((84, 170, 209)));
    // Upstream's own gamut-mapping example: no bisection step fits, so it must map to white.
    assert_eq!(parse_color("oklch(100% 0.3 150)"), Some((255, 255, 255)));
    // `deg` on the hue, and a chroma far outside the gamut that bisects back in.
    assert_eq!(parse_color("oklch(0.5 0.4 30deg)"), Some((187, 12, 0)));
    // The exponent leg of `NUMBER_PATTERN`: `2.9e2` is 290 degrees.
    assert_eq!(parse_color("oklch(0.62 0.17 2.9e2)"), Some((135, 111, 228)));

    // `docs/themes.md:86-87`'s own two examples, so the user-facing documentation is covered.
    assert_eq!(parse_color("oklch(62% 0.1 200)"), Some((28, 152, 158)));
    assert_eq!(parse_color("okhsl(250 60% 55%)"), Some((78, 136, 194)));

    // The hex legs still work, unchanged.
    assert_eq!(parse_color("#abc"), Some((170, 187, 204)));
    assert_eq!(parse_color("#ABCDEF"), Some((171, 205, 239)));
}

/// `parseColor`'s three patterns are anchored `^…$` and nothing is trimmed or defaulted, so each of
/// these THROWS in pi (`Invalid color value`, verified by running `colors.ts` @v1.0.0). cyrup used
/// to accept the first three — a bare `rrggbb`, and hex or a function with surrounding whitespace —
/// as a tolerance that predates the module; a theme that loaded here then failed in pi.
#[test]
fn colour_values_are_matched_exactly_as_pi_matches_them() {
    for value in [
        "abcdef",
        "abc",
        " #abc",
        "#abc ",
        "\t#aabbcc",
        " okhsl(295 50% 67%)",
        "okhsl(295 50% 67%) ",
        // Four hex digits, five, seven: not `{3}` or `{6}`.
        "#abcd",
        "#12345",
        "#1234567",
        // Units belong inside the parentheses, attached to their number.
        "oklch(0.7 0.1 230 deg)",
        "oklch(70%0.1 230)",
        "oklch(0.7,0.1,230)",
    ] {
        assert_eq!(parse_color(value), None, "{value:?} must not parse");
    }
    // The units are case-insensitive, and a leading dot / exponent are `NUMBER_PATTERN` forms.
    assert_eq!(parse_color("oklch(0.5 0.1 230DEG)"), Some((0, 109, 145)));
    assert_eq!(parse_color("oklch(.5 .1 1e2)"), Some((113, 100, 11)));
}

/// Every one of these THROWS in pi (`Invalid color value`, or a range/finiteness error from
/// `okhslColor`/`oklchColor`/`requireFinite`), verified by running `parseColor` at the tag. cyrup's
/// resolvers are total by R-00-009, so the port answers `None` and the caller degrades.
#[test]
fn malformed_color_functions_do_not_parse() {
    for value in [
        // Lightness 89 with no `%` is 89, outside 0-1 — the unit is load-bearing.
        "okhsl(234 3% 89)",
        // Four components; the pattern takes exactly three.
        "okhsl(1 2 3 4)",
        // Two components.
        "oklch(0.5 0.1)",
        // Commas; the separators are whitespace only.
        "okhsl(0,0.5,0.5)",
        // Out of range: l > 1, negative chroma, s > 1.
        "oklch(1.5 0.1 0)",
        "oklch(0.5 -0.1 0)",
        "okhsl(0 1.5 0.5)",
        // No closing paren.
        "okhsl(0 0.5 0.5",
        // `NUMBER_PATTERN` admits neither, and Rust's own `parse::<f64>` would accept both.
        "okhsl(nan 0 0)",
        "oklch(inf 0 0)",
        // Not a colour at all — in a theme this is a var reference, resolved before parsing.
        "nope",
        "",
    ] {
        assert_eq!(parse_color(value), None, "{value:?} must not parse");
    }
}

/// `/^ok(lch|hsl)\(/i` (pi `resolveVarRefs`, `theme/theme.ts:140`): a colour function is a VALUE,
/// so var resolution must not chase it as a variable name.
#[test]
fn colour_functions_are_recognised_before_var_lookup() {
    assert!(is_color_function("okhsl(234 3% 89%)"));
    assert!(is_color_function("OKLCH(0.7 0.1 230)"));
    // `/^ok(lch|hsl)\(/i` is anchored at the first character: leading whitespace makes it a
    // variable name, which then fails to resolve.
    assert!(!is_color_function("  okhsl(0 0 0)"));
    assert!(!is_color_function("okhsl"));
    assert!(!is_color_function("text"));
    assert!(!is_color_function("#abc"));
    assert!(!is_color_function(""));
}

/// The row as a user meets it: a theme JSON written the way pi 1.0 writes its own built-ins — a
/// `vars` block of `okhsl(…)` values, referenced by role — resolves to real colours.
///
/// RED before this row: `accent` and `text` both came back `ColorSpec::Inherit`, so every role of a
/// pi 1.0 theme fell through to cyrup's compiled fallback with no error and no unresolved-role
/// report. `vars` indirection and the direct-value form are both covered because upstream resolves
/// them through different legs — the var leg recurses, the direct leg short-circuits on
/// `/^ok(lch|hsl)\(/i`.
#[test]
fn a_pi_1_0_theme_written_in_okhsl_resolves_every_role() {
    use super::fixtures::full_theme_json;
    use crate::{ColorSpec, ResourceScope, Theme};

    let theme = Theme::parse(
        &full_theme_json(
            "pi-one-point-oh",
            &[
                ("text", "okhsl(234 3% 89%)"),
                ("violet", "okhsl(295 50% 67%)"),
                ("sky", "oklch(0.7 0.1 230)"),
            ],
            &[
                ("text", "text"),
                ("accent", "$violet"),
                ("border", "oklch(0.7 0.1 230)"),
                ("borderAccent", "sky"),
            ],
        ),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    let resolved = theme.resolve();

    // Through `vars`, pi's bare-name form.
    assert_eq!(
        resolved.roles.get("text"),
        Some(&ColorSpec::Rgb {
            r: 222,
            g: 224,
            b: 225
        })
    );
    // Through `vars`, cyrup's `$name` tolerance.
    assert_eq!(
        resolved.roles.get("accent"),
        Some(&ColorSpec::Rgb {
            r: 167,
            g: 152,
            b: 215
        })
    );
    // Directly, which must NOT be looked up as a variable name first.
    assert_eq!(
        resolved.roles.get("border"),
        Some(&ColorSpec::Rgb {
            r: 84,
            g: 170,
            b: 209
        })
    );
    // The same OKLCH value reached through `vars` instead, so both function names are covered on
    // both legs — the row's verify line names an `okhsl(…)` var and an `oklch(…)` var.
    assert_eq!(
        resolved.roles.get("borderAccent"),
        Some(&ColorSpec::Rgb {
            r: 84,
            g: 170,
            b: 209
        })
    );
}

/// `resolveVarRefs` short-circuits a colour FUNCTION before the `vars` lookup
/// (`/^ok(lch|hsl)\(/i`, `theme/theme.ts:140` @v1.0.0), so a variable whose NAME happens to be a
/// colour function cannot shadow the value.
///
/// Contrived on purpose: it is the only input that separates "short-circuit, then parse" from
/// "look the name up, then parse as a last resort", and it is the line upstream wrote.
#[test]
fn a_var_named_like_a_colour_function_does_not_shadow_the_value() {
    use super::fixtures::full_theme_json;
    use crate::{ColorSpec, ResourceScope, Theme};

    let theme = Theme::parse(
        &full_theme_json(
            "shadow",
            &[("okhsl(234 3% 89%)", "#ff0000")],
            &[("accent", "okhsl(234 3% 89%)")],
        ),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .unwrap();
    assert_eq!(
        theme.resolve().roles.get("accent"),
        Some(&ColorSpec::Rgb {
            r: 222,
            g: 224,
            b: 225
        }),
        "the OKHSL value wins over a var of the same name"
    );
}

/// TUI-131's third verify item: an unparseable colour makes the theme load report an error instead
/// of repainting the role from a compiled fallback.
///
/// Upstream reaches this through `parseColor`'s `throw new Error(\`Invalid color value: ${value}\`)`
/// during `setTheme`, which the user sees as `Failed to load theme …`; the message here is pi's
/// own text. (An unknown NAME is the sibling `Variable reference not found`, pinned in
/// `themes.rs`.)
#[test]
fn an_unparseable_colour_function_fails_the_theme_load() {
    use super::fixtures::full_theme_json;
    use crate::{ResourceScope, Theme};

    let err = Theme::parse(
        // Lightness 89 with no `%`: out of OKHSL's 0-1 range, which is what pi throws on.
        &full_theme_json("bad", &[], &[("accent", "okhsl(234 3% 89)")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("an unparseable okhsl() value must be rejected, not silently inherited");
    assert!(
        err.to_string()
            .contains("Invalid color value: okhsl(234 3% 89)"),
        "pi's own message text: {err}"
    );

    // A malformed value reached through a variable is reported the same way.
    let err = Theme::parse(
        &full_theme_json("badvar", &[("v", "oklch(1.5 0.1 0)")], &[("accent", "$v")]),
        None,
        ResourceScope::Builtin,
        crate::ResourceOrigin::Builtin,
    )
    .expect_err("an unparseable oklch() var must be rejected");
    assert!(
        err.to_string()
            .contains("Invalid color value: oklch(1.5 0.1 0)"),
        "{err}"
    );
}
