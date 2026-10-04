//! Theme colour VALUES: `#RGB`/`#RRGGBB`, `oklch(…)` and `okhsl(…)`.
//!
//! TUI-131. The port of two files pi added at `567469096` (#8398): `packages/tui/src/oklab.ts`
//! (the Oklab/OKHSL ↔ sRGB conversions) and the colour-value half of `packages/tui/src/colors.ts`
//! (`parseColor`, `okhslColor`, `oklchColor` and the OKLCH gamut mapping). pi 1.0's own
//! `dark.json` and `light.json` are written ENTIRELY in `okhsl(…)`, and the theme schema now
//! describes every colour as "Hex color (#RGB or #RRGGBB), OKLCH or OKHSL color, variable
//! reference, or empty string for terminal default" (`theme/theme-schema.json:29`, `:350`), so
//! without this a pi 1.0 theme — the built-ins included — resolves to nothing and every role falls
//! back to a compiled hex.
//!
//! The v1.0.0 CHANGELOG entry: *"Added `#rgb`, `oklch()`, and `okhsl()` colors and an optional
//! `appearance` field to theme files"*. Only the colour values are this row; `appearance` is
//! `TUI-132`.
//!
//! ADR-0001 scopes the TUI substrate carve-out to DRAWING only; none of this draws, so it is in
//! scope. It lives here rather than in `cyrup-tui` because both theme resolvers need it and
//! `cyrup-resources` is the lower crate — `cyrup-tui` depends on it, not the other way round.
//!
//! What is NOT ported here, deliberately: `mixColors` (needed by `TUI-144` only),
//! `foregroundAnsi`/`backgroundAnsi`/`styleTextWithAnsi` (cyrup's `with_color_mode` projection in
//! `cyrup-tui` is already the port of those, over the same `CUBE_VALUES`/`GRAY_VALUES` tables) and
//! the `Color` union's `indexed` variant (`index_to_rgb` in `theme.rs` already covers pi's numeric
//! palette-index colour values).
//!
//! The Oklab/OKHSL maths is Björn Ottosson's reference implementation
//! (<https://bottosson.github.io/posts/colorpicker/>), Copyright (c) 2021 Björn Ottosson, MIT
//! licensed — the same provenance pi's `oklab.ts` header records.
//!
//! Three of the matrix entries below carry more decimal digits than an `f64` can distinguish, which
//! is `clippy::excessive_precision`. They are upstream's literals, digit for digit, so that
//! `oklab.ts:30-48` and this file diff by eye; truncating them would change no value (the lint's
//! own suggestion is the same `f64`) and would cost that property, so the lint is allowed for this
//! module and nowhere else in the crate.
#![allow(clippy::excessive_precision)]

/// One row of a 3×3 matrix, or a vector in any of the three linear spaces.
///
/// A tuple rather than `[f64; 3]` so every access below is a field and not an index: the workspace
/// denies `clippy::indexing_slicing` outside tests, and upstream's `row[0] * x + …` shape would
/// otherwise have to be written through `get()` and a fallback that cannot happen.
type Vec3 = (f64, f64, f64);

/// `multiply(m, v)` (Pi `oklab.ts:22-23`).
fn apply(m: (Vec3, Vec3, Vec3), v: Vec3) -> Vec3 {
    let dot = |r: Vec3| r.0 * v.0 + r.1 * v.1 + r.2 * v.2;
    (dot(m.0), dot(m.1), dot(m.2))
}

// `LINEAR_SRGB_TO_LMS` … `LMS_TO_LINEAR_SRGB` (Pi `oklab.ts:30-48`), digit for digit.
const LINEAR_SRGB_TO_LMS: (Vec3, Vec3, Vec3) = (
    (0.4122214694707629, 0.5363325372617349, 0.0514459932675022),
    (0.2119034958178251, 0.6806995506452344, 0.1073969535369405),
    (0.0883024591900564, 0.2817188391361215, 0.6299787016738222),
);
const LMS_TO_LAB: (Vec3, Vec3, Vec3) = (
    (0.210454268309314, 0.793617774702305, -0.0040720430116193),
    (1.9779985324311684, -2.42859224204858, 0.450593709617411),
    (0.0259040424655478, 0.7827717124575296, -0.8086757549230774),
);
const LAB_TO_LMS: (Vec3, Vec3, Vec3) = (
    (1.0, 0.3963377773761749, 0.2158037573099136),
    (1.0, -0.1055613458156586, -0.0638541728258133),
    (1.0, -0.0894841775298119, -1.2914855480194092),
);
const LMS_TO_LINEAR_SRGB: (Vec3, Vec3, Vec3) = (
    (4.0767416360759583, -3.3077115392580629, 0.2309699031821043),
    (-1.2684379732850315, 2.6097573492876882, -0.341319376002657),
    (-0.0041960761386756, -0.7034186179359362, 1.7076146940746117),
);

/// `SATURATION_FIT[0..3][1]` (Pi `oklab.ts:55,59,63`): the polynomial coefficients `k0..k4`.
type Fit = (f64, f64, f64, f64, f64);
const FIT_RED: Fit = (1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245);
const FIT_GREEN: Fit = (0.73956515, -0.45954404, 0.08285427, 0.12541073, -0.14503204);
const FIT_BLUE: Fit = (1.35733652, -0.00915799, -1.1513021, -0.50559606, 0.00692167);

/// `SATURATION_FIT[0..3][0]` (Pi `oklab.ts:54,58,62`): the `(a, b)` half-plane where each channel
/// clips first. Only red's and green's are tested — blue's arm is unconditional, because upstream's
/// `findIndex` predicate is `index === 2 || x * a + y * b > 1`.
const HALF_RED: (f64, f64) = (-1.8817031, -0.80936501);
const HALF_GREEN: (f64, f64) = (1.8144408, -1.19445267);

const K1: f64 = 0.206;
const K2: f64 = 0.03;
const K3: f64 = (1.0 + K1) / (1.0 + K2);

/// Oklab lightness to OKHSL lightness (Pi `oklabToOkhslLightness`, `oklab.ts:71-72`).
pub fn oklab_to_okhsl_lightness(x: f64) -> f64 {
    0.5 * (K3 * x - K1 + ((K3 * x - K1).powi(2) + 4.0 * K2 * K3 * x).sqrt())
}

/// OKHSL lightness to Oklab lightness (Pi `okhslToOklabLightness`, `oklab.ts:74`).
fn okhsl_to_oklab_lightness(x: f64) -> f64 {
    (x * x + K1 * x) / (K3 * (x + K2))
}

/// sRGB transfer function, linear to encoded channel, both 0-1 (Pi `linearToSrgb`, `oklab.ts:77-78`).
fn linear_to_srgb(value: f64) -> f64 {
    if value > 0.0031308 {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * value
    }
}

/// Inverse sRGB transfer function (Pi `srgbToLinear`, `oklab.ts:80`).
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Oklab `[L, a, b]` to linear sRGB (0-1, may leave the gamut) — Pi `oklabToLinearSrgb`
/// (`oklab.ts:83-85`).
pub fn oklab_to_linear_srgb(lab: Vec3) -> Vec3 {
    let lms = apply(LAB_TO_LMS, lab);
    apply(
        LMS_TO_LINEAR_SRGB,
        (lms.0.powi(3), lms.1.powi(3), lms.2.powi(3)),
    )
}

/// Linear sRGB to Oklab (Pi `linearSrgbToOklab`, `oklab.ts:88-89`).
fn linear_srgb_to_oklab(rgb: Vec3) -> Vec3 {
    let lms = apply(LINEAR_SRGB_TO_LMS, rgb);
    apply(LMS_TO_LAB, (lms.0.cbrt(), lms.1.cbrt(), lms.2.cbrt()))
}

/// sRGB channels (0-255) to Oklab (Pi `rgbToOklab`, `oklab.ts:92-93`).
pub fn rgb_to_oklab((r, g, b): (u8, u8, u8)) -> Vec3 {
    linear_srgb_to_oklab((
        srgb_to_linear(f64::from(r) / 255.0),
        srgb_to_linear(f64::from(g) / 255.0),
        srgb_to_linear(f64::from(b) / 255.0),
    ))
}

/// Linear sRGB to sRGB channels (0-255, rounded), clipping out-of-gamut channels — Pi
/// `linearSrgbToRgb` (`oklab.ts:96-99`).
pub fn linear_srgb_to_rgb(linear: Vec3) -> (u8, u8, u8) {
    let channel = |value: f64| -> u8 {
        let encoded = linear_to_srgb(value).clamp(0.0, 1.0) * 255.0;
        // Already clamped into 0..=255, so the cast cannot saturate surprisingly; `round` matches
        // `Math.round` for non-negative values (both go to the larger magnitude at .5).
        encoded.round() as u8
    };
    (channel(linear.0), channel(linear.1), channel(linear.2))
}

/// Rate of change of each cube-root LMS component along a chroma direction — Pi `lmsSlopes`
/// (`oklab.ts:102-104`).
fn lms_slopes(a: f64, b: f64) -> Vec3 {
    let slope = |r: Vec3| r.1 * a + r.2 * b;
    (
        slope(LAB_TO_LMS.0),
        slope(LAB_TO_LMS.1),
        slope(LAB_TO_LMS.2),
    )
}

/// Largest saturation (C/L) inside sRGB for hue `(a, b)` — Pi `maxSaturation`
/// (`oklab.ts:107-120`): the polynomial fit plus one Halley step.
fn max_saturation(a: f64, b: f64) -> f64 {
    let (poly, weights) = if HALF_RED.0 * a + HALF_RED.1 * b > 1.0 {
        (FIT_RED, LMS_TO_LINEAR_SRGB.0)
    } else if HALF_GREEN.0 * a + HALF_GREEN.1 * b > 1.0 {
        (FIT_GREEN, LMS_TO_LINEAR_SRGB.1)
    } else {
        (FIT_BLUE, LMS_TO_LINEAR_SRGB.2)
    };
    let (k0, k1, k2, k3, k4) = poly;
    let saturation = k0 + k1 * a + k2 * b + k3 * a * a + k4 * a * b;

    let slopes = lms_slopes(a, b);
    let base = (
        1.0 + saturation * slopes.0,
        1.0 + saturation * slopes.1,
        1.0 + saturation * slopes.2,
    );
    let dot = |v: Vec3| weights.0 * v.0 + weights.1 * v.1 + weights.2 * v.2;
    let f = dot((base.0.powi(3), base.1.powi(3), base.2.powi(3)));
    let f1 = dot((
        3.0 * slopes.0 * base.0.powi(2),
        3.0 * slopes.1 * base.1.powi(2),
        3.0 * slopes.2 * base.2.powi(2),
    ));
    let f2 = dot((
        6.0 * slopes.0.powi(2) * base.0,
        6.0 * slopes.1.powi(2) * base.1,
        6.0 * slopes.2.powi(2) * base.2,
    ));
    saturation - (f * f1) / (f1 * f1 - 0.5 * f * f2)
}

/// Oklab lightness and chroma of the most saturated sRGB colour of hue `(a, b)` — Pi `cusp`
/// (`oklab.ts:123-127`).
fn cusp(a: f64, b: f64) -> (f64, f64) {
    let saturation = max_saturation(a, b);
    let linear = oklab_to_linear_srgb((1.0, saturation * a, saturation * b));
    let peak = linear.0.max(linear.1).max(linear.2);
    let lightness = (1.0 / peak).cbrt();
    (lightness, lightness * saturation)
}

/// Chroma where the constant-lightness line leaves the sRGB gamut — Pi `maxChroma`
/// (`oklab.ts:130-151`).
fn max_chroma(a: f64, b: f64, lightness: f64, (cusp_l, cusp_c): (f64, f64)) -> f64 {
    if lightness <= cusp_l {
        return (cusp_c * lightness) / cusp_l;
    }
    // Upper half: triangle edge, then one Halley step against each channel reaching 1.
    let t = (cusp_c * (lightness - 1.0)) / (cusp_l - 1.0);
    let slopes = lms_slopes(a, b);
    let lms = (
        lightness + t * slopes.0,
        lightness + t * slopes.1,
        lightness + t * slopes.2,
    );
    let cubes = (lms.0.powi(3), lms.1.powi(3), lms.2.powi(3));
    let first = (
        3.0 * slopes.0 * lms.0.powi(2),
        3.0 * slopes.1 * lms.1.powi(2),
        3.0 * slopes.2 * lms.2.powi(2),
    );
    let second = (
        6.0 * slopes.0.powi(2) * lms.0,
        6.0 * slopes.1.powi(2) * lms.1,
        6.0 * slopes.2.powi(2) * lms.2,
    );
    let dot = |row: Vec3, v: Vec3| row.0 * v.0 + row.1 * v.1 + row.2 * v.2;
    let step = |row: Vec3| -> f64 {
        let f = dot(row, cubes) - 1.0;
        let f1 = dot(row, first);
        let f2 = dot(row, second);
        let u = f1 / (f1 * f1 - 0.5 * f * f2);
        if u >= 0.0 { -f * u } else { f64::MAX }
    };
    let min = step(LMS_TO_LINEAR_SRGB.0)
        .min(step(LMS_TO_LINEAR_SRGB.1))
        .min(step(LMS_TO_LINEAR_SRGB.2));
    t + min
}

/// OKHSL's chroma reference points at lightness `L` and hue `(a, b)` — Pi `chromaStops`
/// (`oklab.ts:154-179`): `[c0, cMid, cMax]`.
fn chroma_stops(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let peak = cusp(a, b);
    let c_max = max_chroma(a, b, l, peak);
    let k = c_max / (l * (peak.1 / peak.0)).min((1.0 - l) * (peak.1 / (1.0 - peak.0)));
    let mid_s = 0.11516993
        + 1.0
            / (7.4477897
                + 4.1590124 * b
                + a * (-2.19557347
                    + 1.75198401 * b
                    + a * (-2.13704948 - 10.02301043 * b
                        + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let mid_t = 0.11239642
        + 1.0
            / (1.6132032 - 0.68124379 * b
                + a * (0.40370612
                    + 0.90148123 * b
                    + a * (-0.27087943
                        + 0.6122399 * b
                        + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    let c_mid = 0.9
        * k
        * (1.0 / (1.0 / (l * mid_s).powi(4) + 1.0 / ((1.0 - l) * mid_t).powi(4)))
            .sqrt()
            .sqrt();
    let c0 = (1.0 / (1.0 / (l * 0.4).powi(2) + 1.0 / ((1.0 - l) * 0.8).powi(2))).sqrt();
    (c0, c_mid, c_max)
}

/// OKHSL to sRGB channels (0-255, rounded) — Pi `okhslToRgb` (`oklab.ts:187-212`).
///
/// `hue` is in degrees (wrapped), `saturation` and `lightness` are 0-1. Saturation is relative to
/// the sRGB gamut at that hue and lightness, so every value is in gamut.
pub fn okhsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let l = okhsl_to_oklab_lightness(lightness);
    let mut lab: Vec3 = (l, 0.0, 0.0);
    if l > 0.0 && l < 1.0 && saturation > 0.0 {
        // `(2 * Math.PI * (((hue % 360) + 360) % 360)) / 360` — the double modulo is what makes
        // a negative hue wrap forward rather than backward.
        let wrapped = ((hue % 360.0) + 360.0) % 360.0;
        let angle = (2.0 * std::f64::consts::PI * wrapped) / 360.0;
        let a = angle.cos();
        let b = angle.sin();
        let (c0, c_mid, c_max) = chroma_stops(l, a, b);
        // Chroma rises from 0 through cMid at s = 0.8 to cMax at s = 1.
        let chroma = if saturation < 0.8 {
            let t = 1.25 * saturation;
            let k1 = 0.8 * c0;
            (t * k1) / (1.0 - (1.0 - k1 / c_mid) * t)
        } else {
            let t = 5.0 * (saturation - 0.8);
            let k1 = (0.2 * c_mid.powi(2) * 1.25_f64.powi(2)) / c0;
            c_mid + (t * k1) / (1.0 - (1.0 - k1 / (c_max - c_mid)) * t)
        };
        lab = (l, chroma * a, chroma * b);
    }
    linear_srgb_to_rgb(oklab_to_linear_srgb(lab))
}

/// sRGB channels (0-255) to OKHSL — Pi `rgbToOkhsl` (`oklab.ts:219-238`). Hue in degrees (0 for
/// grays), saturation and lightness 0-1.
pub fn rgb_to_okhsl(rgb: (u8, u8, u8)) -> (f64, f64, f64) {
    let (l_lab, lab_a, lab_b) = rgb_to_oklab(rgb);
    let chroma = lab_a.hypot(lab_b);
    let lightness = oklab_to_okhsl_lightness(l_lab);
    if chroma < 1e-9 || lightness <= 0.0 || lightness >= 1.0 {
        return (0.0, 0.0, lightness);
    }
    let hue = (lab_b.atan2(lab_a).to_degrees() + 360.0) % 360.0;
    let (c0, c_mid, c_max) = chroma_stops(l_lab, lab_a / chroma, lab_b / chroma);
    let saturation = if chroma < c_mid {
        let k1 = 0.8 * c0;
        0.8 * (chroma / (k1 + (1.0 - k1 / c_mid) * chroma))
    } else {
        let k1 = (0.2 * c_mid.powi(2) * 1.25_f64.powi(2)) / c0;
        let offset = chroma - c_mid;
        0.8 + 0.2 * (offset / (k1 + (1.0 - k1 / (c_max - c_mid)) * offset))
    };
    (hue, saturation.clamp(0.0, 1.0), lightness)
}

/// `isInSrgbGamut` (Pi `colors.ts:185-188`).
fn is_in_srgb_gamut(linear: Vec3) -> bool {
    const EPSILON: f64 = 1e-7;
    let ok = |c: f64| (-EPSILON..=1.0 + EPSILON).contains(&c);
    ok(linear.0) && ok(linear.1) && ok(linear.2)
}

/// OKLCH to sRGB channels — Pi `oklchToRgb` (`colors.ts:190-217`).
///
/// Gamut mapping keeps the hue fixed and bisects the chroma down 20 times. The achromatic colour is
/// always in gamut, so it is the fallback when no bisection step fits — upstream's own example is
/// `oklch(100% 0.3 150)`, which must map to white.
pub fn oklch_to_rgb(l: f64, c: f64, h: f64) -> (u8, u8, u8) {
    let radians = h.to_radians();
    let cos = radians.cos();
    let sin = radians.sin();
    let at_chroma = |chroma: f64| oklab_to_linear_srgb((l, chroma * cos, chroma * sin));

    let direct = at_chroma(c);
    if is_in_srgb_gamut(direct) {
        return linear_srgb_to_rgb(direct);
    }

    let mut linear = at_chroma(0.0);
    let mut low = 0.0;
    let mut high = c;
    for _ in 0..20 {
        let chroma = (low + high) / 2.0;
        let candidate = at_chroma(chroma);
        if is_in_srgb_gamut(candidate) {
            low = chroma;
            linear = candidate;
        } else {
            high = chroma;
        }
    }
    linear_srgb_to_rgb(linear)
}

/// `okhslColor(h, s, l)` (Pi `colors.ts:107-115`): validates the ranges, then converts.
///
/// `None` where upstream throws — the two range checks (`s`, `l` ∈ [0, 1]) and the finiteness
/// check `requireFinite` applies to all three channels.
pub fn okhsl_color(h: f64, s: f64, l: f64) -> Option<(u8, u8, u8)> {
    if !h.is_finite() || !s.is_finite() || !l.is_finite() {
        return None;
    }
    if !(0.0..=1.0).contains(&s) || !(0.0..=1.0).contains(&l) {
        return None;
    }
    Some(okhsl_to_rgb(h, s, l))
}

/// `oklchColor(l, c, h)` (Pi `colors.ts:79-86`): validates, wraps the hue into `[0, 360)`, then
/// converts.
///
/// `None` where upstream throws: a non-finite channel, `l` outside `[0, 1]`, or a negative `c`.
pub fn oklch_color(l: f64, c: f64, h: f64) -> Option<(u8, u8, u8)> {
    if !l.is_finite() || !c.is_finite() || !h.is_finite() {
        return None;
    }
    if !(0.0..=1.0).contains(&l) || c < 0.0 {
        return None;
    }
    Some(oklch_to_rgb(l, c, (h % 360.0 + 360.0) % 360.0))
}

/// `NUMBER_PATTERN` (Pi `colors.ts:87`): `[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?`, matched
/// whole.
///
/// Hand-matched rather than through a regex crate, and deliberately STRICTER than Rust's
/// `str::parse::<f64>`, which would otherwise accept `inf`, `NaN`, `infinity` and `1_0` — values
/// pi's pattern rejects before `requireFinite` ever runs.
fn parse_number(token: &str) -> Option<f64> {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    // `\d+(?:\.\d*)?|\.\d+`
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => {
            !(int.is_empty() && frac.is_empty())
                && int.bytes().all(|b| b.is_ascii_digit())
                && frac.bytes().all(|b| b.is_ascii_digit())
        }
        None => !mantissa.is_empty() && mantissa.bytes().all(|b| b.is_ascii_digit()),
    };
    if !mantissa_ok {
        return None;
    }
    // `(?:e[+-]?\d+)?`
    if let Some(exp) = exponent {
        let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    token.parse::<f64>().ok()
}

/// A number with an optional trailing unit, as the two patterns spell it: the unit may appear only
/// where upstream's capture group allows it. Returns the value and whether the unit was present.
fn parse_number_with_unit(token: &str, unit: &str) -> Option<(f64, bool)> {
    if unit.len() < token.len()
        && token
            .get(token.len() - unit.len()..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(unit))
        && let Some(head) = token.get(..token.len() - unit.len())
        && let Some(value) = parse_number(head)
    {
        return Some((value, true));
    }
    parse_number(token).map(|value| (value, false))
}

/// Split `name( a b c )` into its three whitespace-separated components, case-insensitively on the
/// function name — upstream's `^name\(\s*…\s+…\s+…\s*\)$` anchored, with `\s+` separators and no
/// commas.
fn split_function<'a>(value: &'a str, name: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let open = value.get(..name.len() + 1)?;
    if !open.eq_ignore_ascii_case(&format!("{name}(")) {
        return None;
    }
    let inner = value.get(name.len() + 1..)?.strip_suffix(')')?;
    let mut parts = inner.split_whitespace();
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// `parseColor(value)` for the string half (Pi `colors.ts:121-148`), as RGB channels.
///
/// Accepts `#RGB`, `#RRGGBB`, `oklch(L[%] C H[deg])` and `okhsl(H[deg] S[%] L[%])`,
/// case-insensitively for the two function names and their units. `None` is upstream's
/// `throw new Error(\`Invalid color value: ${value}\`)`.
///
/// TUI-131 — the sole reason this exists. `parse_hex` took `#RGB`/`#RRGGBB` and nothing else, so
/// pi 1.0's built-in themes, which are written entirely in `okhsl(…)`, resolved to no colour at
/// all and every role silently fell back to a compiled hex.
pub fn parse_color(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim();

    if let Some(rgb) = parse_hex(v) {
        return Some(rgb);
    }

    // `oklch(L[%] C H[deg])` — only `L` may carry `%`, only `H` may carry `deg`.
    if let Some((l, c, h)) = split_function(v, "oklch") {
        let (lightness, percent) = parse_number_with_unit(l, "%")?;
        let chroma = parse_number(c)?;
        let (hue, _) = parse_number_with_unit(h, "deg")?;
        return oklch_color(lightness / if percent { 100.0 } else { 1.0 }, chroma, hue);
    }

    // `okhsl(H[deg] S[%] L[%])`.
    if let Some((h, s, l)) = split_function(v, "okhsl") {
        let (hue, _) = parse_number_with_unit(h, "deg")?;
        let (saturation, s_percent) = parse_number_with_unit(s, "%")?;
        let (lightness, l_percent) = parse_number_with_unit(l, "%")?;
        return okhsl_color(
            hue,
            saturation / if s_percent { 100.0 } else { 1.0 },
            lightness / if l_percent { 100.0 } else { 1.0 },
        );
    }

    None
}

/// `/^ok(lch|hsl)\(/i` (Pi `resolveVarRefs`, `theme/theme.ts:140`): a value that is a colour
/// FUNCTION rather than a variable reference, so var resolution must not chase it.
pub fn is_color_function(value: &str) -> bool {
    let v = value.trim_start();
    v.get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("oklch(") || p.eq_ignore_ascii_case("okhsl("))
}

/// `#RGB` / `#RRGGBB`, and (a cyrup tolerance that predates this module) the same without the `#`.
///
/// Pi's hex leg is `/^#([\da-f]{3}|[\da-f]{6})$/i` (`colors.ts:124`), which requires the `#`;
/// cyrup's two theme resolvers have always also accepted a bare `rrggbb` as a last resort for a
/// value that is not a known var, and that tolerance is preserved here rather than narrowed in a
/// row about OKLCH.
fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#').unwrap_or(s);
    let bytes = h.as_bytes();
    match bytes.len() {
        6 => Some((
            hex_pair(bytes.first(), bytes.get(1))?,
            hex_pair(bytes.get(2), bytes.get(3))?,
            hex_pair(bytes.get(4), bytes.get(5))?,
        )),
        3 => Some((
            hex_digit(bytes.first())? * 17,
            hex_digit(bytes.get(1))? * 17,
            hex_digit(bytes.get(2))? * 17,
        )),
        _ => None,
    }
}

fn hex_digit(b: Option<&u8>) -> Option<u8> {
    match *b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_pair(hi: Option<&u8>, lo: Option<&u8>) -> Option<u8> {
    hex_digit(hi)?.checked_mul(16)?.checked_add(hex_digit(lo)?)
}
