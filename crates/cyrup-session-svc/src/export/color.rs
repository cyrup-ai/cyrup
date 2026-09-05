//! Export-palette arithmetic — the Rust half of pi's `parseColor`, `getLuminance`,
//! `adjustBrightness` and `deriveExportColors`
//! (`packages/coding-agent/src/core/export-html/index.ts:42-106` @v0.84.4).
//!
//! Upstream carries colours through this arithmetic as bare `string`s and re-checks them at every
//! step: `adjustBrightness` opens `const parsed = parseColor(color); if (!parsed) return color;`
//! (`index.ts:74-75`), handing an unparseable value straight back into the emitted stylesheet.
//! [`CssColor`] is the parse boundary instead — a value that exists has already matched pi's
//! `parseColor` grammar, so the derivation functions below take and return parsed colours and
//! upstream's per-step re-check has nowhere to fail. The one place the miss is a real outcome —
//! `deriveExportColors`' `if (!parsed) return {…}` constant triple (`index.ts:82-89`) — is
//! preserved verbatim in [`derive_export_colors`], which is the only entry point that accepts the
//! absence.

use std::fmt;
use std::str::FromStr;

/// A colour that is legal in a CSS declaration, stored as the sRGB triple pi's `parseColor`
/// (`export-html/index.ts:43-61` @v0.84.4) extracts.
///
/// Constructible only by parsing pi's two accepted spellings (`#rrggbb`, `rgb(r, g, b)`) or from an
/// explicit triple, so no unvalidated string can reach [`Self::adjust_brightness`],
/// [`derive_export_colors`] or the `--role: …;` declarations the export stylesheet is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CssColor {
    r: u8,
    g: u8,
    b: u8,
}

/// `parseColor` returned `undefined` (`export-html/index.ts:60` @v0.84.4): the text matched neither
/// `#rrggbb` nor `rgb(r, g, b)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseColorError;

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a CSS colour: expected `#rrggbb` or `rgb(r, g, b)`")
    }
}

impl std::error::Error for ParseColorError {}

impl CssColor {
    /// An already-decoded triple — the bridge from `cyrup_resources::ColorSpec::Rgb`, which has
    /// done this crate's parsing for the theme document (hex, `#rgb`, var references and
    /// 256-colour indices all land there as `r`/`g`/`b`).
    #[must_use]
    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[must_use]
    pub const fn r(self) -> u8 {
        self.r
    }

    #[must_use]
    pub const fn g(self) -> u8 {
        self.g
    }

    #[must_use]
    pub const fn b(self) -> u8 {
        self.b
    }

    /// Relative luminance, 0..1 (pi `getLuminance`, `export-html/index.ts:64-70` @v0.84.4).
    #[must_use]
    pub fn luminance(self) -> f64 {
        let to_linear = |c: u8| {
            let s = f64::from(c) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * to_linear(self.r) + 0.7152 * to_linear(self.g) + 0.0722 * to_linear(self.b)
    }

    /// pi's `isLight` test inside `deriveExportColors` (`export-html/index.ts:92` @v0.84.4).
    #[must_use]
    pub fn is_light(self) -> bool {
        self.luminance() > 0.5
    }

    /// pi `adjustBrightness` (`export-html/index.ts:73-78` @v0.84.4): `factor > 1` lightens,
    /// `< 1` darkens, each channel `min(255, max(0, round(c * factor)))`.
    ///
    /// Upstream's `if (!parsed) return color` guard has no analogue here by construction — see the
    /// module docs.
    #[must_use]
    pub fn adjust_brightness(self, factor: f64) -> Self {
        let adjust = |c: u8| -> u8 {
            let v = (f64::from(c) * factor).round();
            // `Math.min(255, Math.max(0, …))`. `clamp` first so the `as` cast is always in range.
            v.clamp(0.0, 255.0) as u8
        };
        Self {
            r: adjust(self.r),
            g: adjust(self.g),
            b: adjust(self.b),
        }
    }

    /// Per-channel saturating offsets, the shape both `deriveExportColors` `infoBg` arms use
    /// (`export-html/index.ts:98`, `:104` @v0.84.4).
    #[must_use]
    fn offset(self, dr: i16, dg: i16, db: i16) -> Self {
        let shift = |c: u8, d: i16| -> u8 { (i16::from(c) + d).clamp(0, 255) as u8 };
        Self {
            r: shift(self.r, dr),
            g: shift(self.g, dg),
            b: shift(self.b, db),
        }
    }
}

impl FromStr for CssColor {
    type Err = ParseColorError;

    /// pi `parseColor` (`export-html/index.ts:43-61` @v0.84.4): `#RRGGBB` or `rgb(r, g, b)` with
    /// arbitrary whitespace around the components. Nothing else parses — upstream's two regexes are
    /// anchored and it returns `undefined` for every other spelling, three-digit hex included.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hex) = s.strip_prefix('#') {
            let bytes = hex.as_bytes();
            if bytes.len() != 6 {
                return Err(ParseColorError);
            }
            let pair = |i: usize| -> Option<u8> {
                let slice = hex.get(i..i + 2)?;
                u8::from_str_radix(slice, 16).ok()
            };
            return match (pair(0), pair(2), pair(4)) {
                (Some(r), Some(g), Some(b)) => Ok(Self { r, g, b }),
                _ => Err(ParseColorError),
            };
        }
        // `^rgb\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)$`
        let rest = s.strip_prefix("rgb").ok_or(ParseColorError)?.trim_start();
        let inner = rest
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
            .ok_or(ParseColorError)?;
        let mut parts = inner.split(',');
        let mut next = || -> Result<u8, ParseColorError> {
            let raw = parts.next().ok_or(ParseColorError)?.trim();
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseColorError);
            }
            // `Number.parseInt` saturates nothing, but the CSS consumer clamps; a component wider
            // than a byte is out of gamut either way, so clamp rather than reject.
            Ok(raw.parse::<u32>().unwrap_or(255).min(255) as u8)
        };
        let (r, g, b) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(ParseColorError);
        }
        Ok(Self { r, g, b })
    }
}

impl fmt::Display for CssColor {
    /// `#rrggbb`.
    ///
    /// [CYRUP-DELTA] pi round-trips whichever spelling the theme author used and emits
    /// `rgb(r, g, b)` only for values `adjustBrightness` produced (`index.ts:77`); cyrup normalizes
    /// every emitted colour to hex because `cyrup_resources` has already decoded the theme document
    /// to a triple and the original literal no longer exists. The two spellings are the same colour
    /// to every CSS engine; only the bytes of the emitted stylesheet differ.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// The three backdrop colours the export template needs (`{{BODY_BG}}`, `{{CONTAINER_BG}}`,
/// `{{INFO_BG}}` — pi `export-html/index.ts:155-157`, `template.css:3-5` @v0.84.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportBackdrops {
    pub page_bg: CssColor,
    pub card_bg: CssColor,
    pub info_bg: CssColor,
}

/// pi `deriveExportColors(baseColor)` (`export-html/index.ts:81-106` @v0.84.4), driven from the
/// theme's `userMessageBg`.
///
/// `None` is upstream's `if (!parsed)` arm (`:82-89`) — the constant triple, verbatim.
#[must_use]
pub fn derive_export_colors(base: Option<CssColor>) -> ExportBackdrops {
    let Some(base) = base else {
        return ExportBackdrops {
            page_bg: CssColor::from_rgb(24, 24, 30),
            card_bg: CssColor::from_rgb(30, 30, 36),
            info_bg: CssColor::from_rgb(60, 55, 40),
        };
    };
    if base.is_light() {
        // `:94-100`
        ExportBackdrops {
            page_bg: base.adjust_brightness(0.96),
            card_bg: base,
            info_bg: base.offset(10, 5, -20),
        }
    } else {
        // `:101-105`
        ExportBackdrops {
            page_bg: base.adjust_brightness(0.7),
            card_bg: base.adjust_brightness(0.85),
            info_bg: base.offset(20, 15, 0),
        }
    }
}
