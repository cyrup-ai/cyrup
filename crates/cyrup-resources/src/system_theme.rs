//! The `system` theme: cyrup's colours derived from the terminal's own theme.
//!
//! TUI-130. The port of pi's `modes/interactive/theme/system-theme.ts` @v1.0.0 (new at
//! `bf8e4b953`, #10067; `409e808f5` added the chroma cap in [`anchored`]) — the recipe AS DATA with
//! its constants digit for digit, and the generator that evaluates it.
//!
//! Every token belongs to a colour [`Family`] (its hue) and has contrast [`RULES`]: it must reach a
//! contrast [`Level`] on the background and on the panels it is drawn on. Hue and saturation come
//! from the terminal's palette colour for the family's ANSI slot, or from the family's own hue when
//! the terminal reports no palette. Lightness comes from the rules alone. Colours are built in
//! OKHSL, whose saturation is relative to the sRGB gamut, and fade toward gray near black and
//! white. A palette colour never gains OKLCH chroma when it moves to another lightness, so pastel
//! palettes stay pastel.
//!
//! A contrast level is a target-lightness curve: the OKLab lightness a token needs, given the
//! lightness of the surface below it. The curves were fitted by pi to its reference theme design.
//! On dark backgrounds they aim for nearly fixed lightness; on light backgrounds the required
//! difference grows as the background darkens.
//!
//! Depending on what the terminal reports, the theme is generated in one of three tiers
//! ([`generate_system_theme_colors`]):
//!
//! - background and palette: hues from the palette, lightness from the background;
//! - background only: the families' own hues, lightness from the background;
//! - nothing: ANSI palette indices and the default colours, which the terminal renders itself.
//!
//! Evaluated here in `f64` exactly as pi evaluates it in JavaScript numbers; the expected outputs in
//! this module's tests were produced by running pi's own `generateSystemThemeColors` over the same
//! inputs.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::color::{
    Rgb, okhsl_to_rgb, oklab_to_okhsl_lightness, oklch_color, rgb_to_okhsl, rgb_to_oklch,
};
use crate::theme::Appearance;

/// Pi `SYSTEM_THEME_NAME` (`system-theme.ts:35`).
pub const SYSTEM_THEME_NAME: &str = "system";

/// What `/settings` says the system theme is (`themeItems`, `components/settings-selector.ts:212`).
pub const SYSTEM_THEME_DESCRIPTION: &str = "Theme created from your terminal's colors";

// ============================================================================
// Recipe: colour families and their tokens
// ============================================================================

/// A family's OKHSL hue and saturation: `max` at mid lightness, falling toward `min` at black and
/// white (`system-theme.ts:42-47`).
#[derive(Clone, Copy, Debug)]
struct Family {
    hue: f64,
    min: f64,
    max: f64,
    /// ANSI palette slot the family takes its hue and saturation from.
    slot: usize,
}

/// The 14 families of pi's `FAMILIES` table (`system-theme.ts:49-64`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FamilyName {
    Neutral,
    Blue,
    Green,
    Red,
    Yellow,
    Orange,
    Violet,
    Calamine,
    ThinkingSlate,
    ThinkingBlue,
    ThinkingPeriwinkle,
    ThinkingViolet,
    ThinkingMagenta,
    ThinkingRed,
}

impl FamilyName {
    const fn family(self) -> Family {
        const fn f(hue: f64, min: f64, max: f64, slot: usize) -> Family {
            Family {
                hue,
                min,
                max,
                slot,
            }
        }
        match self {
            FamilyName::Neutral => f(231.49, 0.02, 0.08, 8),
            FamilyName::Blue => f(231.49, 0.1, 0.68, 4),
            FamilyName::Green => f(158.68, 0.1, 0.76, 2),
            FamilyName::Red => f(20.0, 0.1, 0.92, 1),
            FamilyName::Yellow => f(82.36, 0.5, 1.0, 3),
            FamilyName::Orange => f(52.0, 0.12, 0.85, 3),
            FamilyName::Violet => f(295.0, 0.2, 0.6, 5),
            FamilyName::Calamine => f(202.43, 0.1, 0.74, 6),
            FamilyName::ThinkingSlate => f(231.49, 0.08, 0.2, 4),
            FamilyName::ThinkingBlue => f(231.49, 0.2, 0.45, 4),
            FamilyName::ThinkingPeriwinkle => f(263.25, 0.3, 0.6, 6),
            FamilyName::ThinkingViolet => f(295.0, 0.4, 0.75, 5),
            FamilyName::ThinkingMagenta => f(337.5, 0.5, 0.85, 13),
            FamilyName::ThinkingRed => f(20.0, 0.95, 1.0, 1),
        }
    }
}

/// Pi's `TOKEN_FAMILIES` (`system-theme.ts:68-128`): every theme token and the family it belongs to,
/// in pi's declaration order (the order tier three walks).
const TOKEN_FAMILIES: [(&str, FamilyName); 56] = [
    ("selectedBg", FamilyName::Blue),
    ("searchMatchBg", FamilyName::Orange),
    ("userMessageBg", FamilyName::Blue),
    ("customMessageBg", FamilyName::Violet),
    ("toolPendingBg", FamilyName::Neutral),
    ("toolSuccessBg", FamilyName::Green),
    ("toolErrorBg", FamilyName::Red),
    ("text", FamilyName::Neutral),
    ("userMessageText", FamilyName::Neutral),
    ("customMessageText", FamilyName::Neutral),
    ("toolTitle", FamilyName::Neutral),
    ("syntaxOperator", FamilyName::Neutral),
    ("syntaxPunctuation", FamilyName::Neutral),
    ("muted", FamilyName::Neutral),
    ("dim", FamilyName::Neutral),
    ("thinkingText", FamilyName::Neutral),
    ("toolOutput", FamilyName::Neutral),
    ("mdLinkUrl", FamilyName::Neutral),
    ("mdQuote", FamilyName::Neutral),
    ("mdQuoteBorder", FamilyName::Neutral),
    ("mdHr", FamilyName::Neutral),
    ("mdCodeBlockBorder", FamilyName::Neutral),
    ("toolDiffContext", FamilyName::Neutral),
    ("syntaxComment", FamilyName::Neutral),
    ("scrollbarTrack", FamilyName::Neutral),
    ("scrollbarThumb", FamilyName::Neutral),
    ("searchMatchText", FamilyName::Neutral),
    ("borderMuted", FamilyName::Neutral),
    ("accent", FamilyName::Violet),
    ("borderAccent", FamilyName::Violet),
    ("customMessageLabel", FamilyName::Violet),
    ("mdCode", FamilyName::Violet),
    ("mdListBullet", FamilyName::Violet),
    ("syntaxType", FamilyName::Violet),
    ("border", FamilyName::Blue),
    ("mdLink", FamilyName::Blue),
    ("syntaxKeyword", FamilyName::Blue),
    ("syntaxVariable", FamilyName::Calamine),
    ("success", FamilyName::Green),
    ("mdCodeBlock", FamilyName::Green),
    ("toolDiffAdded", FamilyName::Green),
    ("bashMode", FamilyName::Green),
    ("syntaxNumber", FamilyName::Green),
    ("error", FamilyName::Red),
    ("toolDiffRemoved", FamilyName::Red),
    ("warning", FamilyName::Yellow),
    ("mdHeading", FamilyName::Yellow),
    ("syntaxFunction", FamilyName::Yellow),
    ("syntaxString", FamilyName::Orange),
    ("thinkingOff", FamilyName::Neutral),
    ("thinkingMinimal", FamilyName::ThinkingSlate),
    ("thinkingLow", FamilyName::ThinkingBlue),
    ("thinkingMedium", FamilyName::ThinkingPeriwinkle),
    ("thinkingHigh", FamilyName::ThinkingViolet),
    ("thinkingXhigh", FamilyName::ThinkingMagenta),
    ("thinkingMax", FamilyName::ThinkingRed),
];

/// Every token the system theme defines, in [`TOKEN_FAMILIES`] order.
pub fn system_tokens() -> impl Iterator<Item = &'static str> {
    TOKEN_FAMILIES.iter().map(|(token, _)| *token)
}

fn family_of(token: &str) -> Option<FamilyName> {
    TOKEN_FAMILIES
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, family)| *family)
}

/// Pi's `TOKEN_SLOTS` (`system-theme.ts:131`): palette slots for tokens that would otherwise share
/// a hue with a similar token.
fn token_slot(token: &str) -> Option<usize> {
    match token {
        "syntaxString" => Some(2),
        "syntaxNumber" => Some(5),
        "searchMatchBg" => Some(3),
        _ => None,
    }
}

// ============================================================================
// Contrast levels and rules
// ============================================================================

/// A target-lightness curve: a polynomial in the surface's OKLab lightness giving the OKLab
/// lightness a token needs on it. `reachable` is the range of surface lightness where the level can
/// be reached; beyond it the level is relaxed (`system-theme.ts:142-145`).
#[derive(Clone, Copy, Debug)]
struct Curve {
    coefficients: [f64; 6],
    reachable: (f64, f64),
}

/// Pi's `LEVELS` (`system-theme.ts:147-247`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Panel,
    Track,
    Thinking0,
    Thinking1,
    Thinking2,
    Thinking3,
    Thinking4,
    Thinking5,
    Thinking6,
    Subtle,
    Thumb,
    Readable,
    Emphasis,
    TextOnPanel,
    Text,
}

impl Level {
    const fn curve(self, appearance: Appearance) -> Curve {
        const fn c(coefficients: [f64; 6], low: f64, high: f64) -> Curve {
            Curve {
                coefficients,
                reachable: (low, high),
            }
        }
        match (self, appearance) {
            (Level::Panel, Appearance::Dark) => c(
                [0.29131, -0.39746, 2.33185, -0.85524, -1.2076, 0.86276],
                0.0,
                0.979,
            ),
            (Level::Panel, Appearance::Light) => c(
                [-3.74073, 27.94549, -78.44258, 112.6798, -79.60015, 22.11277],
                0.348,
                1.0,
            ),
            (Level::Track, Appearance::Dark) => c(
                [0.39028, -0.23015, 0.83573, 2.43829, -4.38292, 2.01582],
                0.0,
                0.946,
            ),
            (Level::Track, Appearance::Light) => c(
                [
                    -5.24921, 38.37322, -107.28833, 152.10005, -106.17127, 29.18061,
                ],
                0.368,
                1.0,
            ),
            (Level::Thinking0, Appearance::Dark) => c(
                [0.52988, -0.05809, -0.30924, 4.63567, -6.52933, 2.89108],
                0.0,
                0.873,
            ),
            (Level::Thinking0, Appearance::Light) => c(
                [
                    -28.27749, 182.85284, -469.62416, 603.15916, -384.59976, 97.35147,
                ],
                0.51,
                1.0,
            ),
            (Level::Thinking1, Appearance::Dark) => c(
                [0.55278, -0.03667, -0.45659, 4.95347, -6.90265, 3.0706],
                0.0,
                0.858,
            ),
            (Level::Thinking1, Appearance::Light) => c(
                [
                    -37.10484, 235.86282, -596.62344, 754.3633, -474.00763, 118.3551,
                ],
                0.535,
                1.0,
            ),
            (Level::Thinking2, Appearance::Dark) => c(
                [0.57486, -0.01765, -0.58987, 5.25227, -7.27175, 3.25532],
                0.0,
                0.842,
            ),
            (Level::Thinking2, Appearance::Light) => c(
                [
                    -59.89653, 377.05024, -945.07843, 1182.03145, -734.96375, 181.68658,
                ],
                0.556,
                1.0,
            ),
            (Level::Thinking3, Appearance::Dark) => c(
                [0.59621, -0.00062, -0.71148, 5.53588, -7.6392, 3.44606],
                0.0,
                0.827,
            ),
            (Level::Thinking3, Appearance::Light) => c(
                [
                    -72.07122,
                    445.84082,
                    -1099.57352,
                    1353.88793,
                    -829.53392,
                    202.26164,
                ],
                0.58,
                1.0,
            ),
            (Level::Thinking4, Appearance::Dark) => c(
                [0.61691, 0.01462, -0.82288, 5.80651, -8.00641, 3.64333],
                0.0,
                0.811,
            ),
            (Level::Thinking4, Appearance::Light) => c(
                [
                    -110.14338,
                    674.21488,
                    -1645.75941,
                    2004.32367,
                    -1215.15899,
                    293.3183,
                ],
                0.6,
                1.0,
            ),
            (Level::Thinking5, Appearance::Dark) => c(
                [0.63702, 0.02826, -0.92498, 6.06465, -8.37246, 3.84651],
                0.0,
                0.795,
            ),
            (Level::Thinking5, Appearance::Light) => c(
                [
                    -175.47701,
                    1063.54495,
                    -2570.70594,
                    3098.80776,
                    -1860.15527,
                    444.76392,
                ],
                0.62,
                1.0,
            ),
            (Level::Thinking6, Appearance::Dark) => c(
                [0.65658, 0.04044, -1.01835, 6.30989, -8.73529, 4.05439],
                0.0,
                0.779,
            ),
            (Level::Thinking6, Appearance::Light) => c(
                [
                    -183.81712,
                    1094.70055,
                    -2602.68539,
                    3088.71276,
                    -1826.91131,
                    430.75931,
                ],
                0.643,
                1.0,
            ),
            (Level::Subtle, Appearance::Dark) => c(
                [0.56762, -0.02475, -0.5383, 5.12628, -7.10931, 3.17324],
                0.0,
                0.848,
            ),
            (Level::Subtle, Appearance::Light) => c(
                [
                    -232.85459,
                    1376.54473,
                    -3249.11801,
                    3827.91186,
                    -2248.29472,
                    526.55751,
                ],
                0.657,
                1.0,
            ),
            (Level::Thumb, Appearance::Dark) => c(
                [0.60323, 0.00278, -0.73328, 5.57157, -7.68067, 3.46933],
                0.0,
                0.823,
            ),
            (Level::Thumb, Appearance::Light) => c(
                [
                    -82.89897,
                    511.01355,
                    -1255.98095,
                    1540.76821,
                    -940.68087,
                    228.58523,
                ],
                0.586,
                1.0,
            ),
            (Level::Readable, Appearance::Dark) => c(
                [0.66937, 0.04704, -1.06871, 6.43941, -8.9332, 4.17229],
                0.0,
                0.77,
            ),
            (Level::Readable, Appearance::Light) => c(
                [
                    -1554.52576,
                    8733.56817,
                    -19604.93507,
                    21977.72696,
                    -12300.99599,
                    2749.81288,
                ],
                0.751,
                1.0,
            ),
            (Level::Emphasis, Appearance::Dark) => c(
                [0.7303, 0.07695, -1.31626, 7.1681, -10.14436, 4.92846],
                0.0,
                0.712,
            ),
            (Level::Emphasis, Appearance::Light) => c(
                [
                    -4948.31942,
                    26870.91986,
                    -58334.48399,
                    63280.17197,
                    -34298.01053,
                    7430.30146,
                ],
                0.811,
                1.0,
            ),
            (Level::TextOnPanel, Appearance::Dark) => c(
                [0.86713, 0.05232, -0.89428, 4.79014, -5.5432, 1.75023],
                0.0,
                0.542,
            ),
            (Level::TextOnPanel, Appearance::Light) => c(
                [
                    -8570.89457,
                    43954.60805,
                    -90084.00702,
                    92220.6791,
                    -47152.15802,
                    9632.27113,
                ],
                0.867,
                1.0,
            ),
            (Level::Text, Appearance::Dark) => c(
                [0.89242, 0.02311, -0.44862, 2.34417, -0.06084, -2.63844],
                0.0,
                0.5,
            ),
            (Level::Text, Appearance::Light) => c(
                [
                    -2004.67048,
                    6664.47299,
                    -6060.70202,
                    -1792.61209,
                    5133.82359,
                    -1939.85583,
                ],
                0.894,
                1.0,
            ),
        }
    }
}

/// What a rule measures a token against: the terminal's background, or another token already drawn
/// under it (`Surface`, `system-theme.ts:251`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Background,
    Token(&'static str),
}

impl Surface {
    /// The key a solved colour is stored under.
    const fn key(self) -> &'static str {
        match self {
            Surface::Background => "background",
            Surface::Token(token) => token,
        }
    }
}

/// One contrast rule (`Rule`, `system-theme.ts:253-257`): `token` must reach `level` on each of
/// `on`.
#[derive(Clone, Debug)]
struct Rule {
    token: &'static str,
    on: Vec<Surface>,
    level: Level,
}

const TOOL_PANELS: [&str; 3] = ["toolPendingBg", "toolSuccessBg", "toolErrorBg"];
const MESSAGE_PANELS: [&str; 2] = ["userMessageBg", "customMessageBg"];
/// Pi's `PANELS` (`system-theme.ts:261-269`): the background tokens.
const PANELS: [&str; 7] = [
    "userMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "selectedBg",
    "searchMatchBg",
    "customMessageBg",
];
const THINKING: [&str; 7] = [
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    "thinkingMax",
];
const THINKING_LEVELS: [Level; 7] = [
    Level::Thinking0,
    Level::Thinking1,
    Level::Thinking2,
    Level::Thinking3,
    Level::Thinking4,
    Level::Thinking5,
    Level::Thinking6,
];

/// A surface list: optionally the terminal background first, then each group of tokens in turn —
/// pi's `["background", "selectedBg", ...TOOL_PANELS]` spelling, one group per spread.
fn surfaces(with_background: bool, groups: &[&[&'static str]]) -> Vec<Surface> {
    let mut on = Vec::new();
    if with_background {
        on.push(Surface::Background);
    }
    for group in groups {
        on.extend(group.iter().map(|token| Surface::Token(token)));
    }
    on
}

/// Pi's `each(tokens, on, level)` (`system-theme.ts:289`).
fn each(tokens: &[&'static str], on: &[Surface], level: Level, out: &mut Vec<Rule>) {
    out.extend(tokens.iter().map(|token| Rule {
        token,
        on: on.to_vec(),
        level,
    }));
}

/// Pi's `RULES` (`system-theme.ts:292-338`), in declaration order.
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let mut rules = Vec::new();
    let background = [Surface::Background];

    each(&PANELS, &background, Level::Panel, &mut rules);
    rules.push(Rule {
        token: "text",
        on: vec![Surface::Background],
        level: Level::Text,
    });
    rules.push(Rule {
        token: "text",
        on: vec![Surface::Token("selectedBg")],
        level: Level::TextOnPanel,
    });
    rules.push(Rule {
        token: "userMessageText",
        on: vec![Surface::Token("userMessageBg")],
        level: Level::TextOnPanel,
    });
    rules.push(Rule {
        token: "toolTitle",
        on: surfaces(false, &[&TOOL_PANELS]),
        level: Level::TextOnPanel,
    });
    each(
        &["accent", "success", "error", "warning"],
        &surfaces(true, &[&["selectedBg"], &TOOL_PANELS]),
        Level::Readable,
        &mut rules,
    );
    rules.push(Rule {
        token: "muted",
        on: surfaces(true, &[&["selectedBg", "customMessageBg"], &TOOL_PANELS]),
        level: Level::Readable,
    });
    rules.push(Rule {
        token: "dim",
        on: surfaces(true, &[&["selectedBg", "customMessageBg"], &TOOL_PANELS]),
        level: Level::Subtle,
    });
    rules.push(Rule {
        token: "thinkingText",
        on: vec![Surface::Background],
        level: Level::Readable,
    });
    rules.push(Rule {
        token: "customMessageText",
        on: surfaces(false, &[&["customMessageBg"], &TOOL_PANELS]),
        level: Level::Readable,
    });
    rules.push(Rule {
        token: "customMessageLabel",
        on: surfaces(true, &[&["customMessageBg", "selectedBg"], &TOOL_PANELS]),
        level: Level::Readable,
    });
    rules.push(Rule {
        token: "toolOutput",
        on: surfaces(true, &[&TOOL_PANELS]),
        level: Level::Readable,
    });
    each(
        &[
            "mdHeading",
            "mdLink",
            "mdLinkUrl",
            "mdCode",
            "mdQuote",
            "mdCodeBlockBorder",
            "mdListBullet",
        ],
        &surfaces(true, &[&MESSAGE_PANELS]),
        Level::Readable,
        &mut rules,
    );
    rules.push(Rule {
        token: "mdCodeBlock",
        on: surfaces(true, &[&MESSAGE_PANELS, &TOOL_PANELS]),
        level: Level::Readable,
    });
    each(
        &["toolDiffAdded", "toolDiffRemoved", "toolDiffContext"],
        &surfaces(true, &[&TOOL_PANELS]),
        Level::Readable,
        &mut rules,
    );
    each(
        &[
            "syntaxComment",
            "syntaxKeyword",
            "syntaxFunction",
            "syntaxVariable",
            "syntaxString",
            "syntaxNumber",
            "syntaxType",
            "syntaxOperator",
            "syntaxPunctuation",
        ],
        &surfaces(true, &[&MESSAGE_PANELS, &TOOL_PANELS]),
        Level::Readable,
        &mut rules,
    );
    rules.push(Rule {
        token: "searchMatchText",
        on: vec![Surface::Token("searchMatchBg")],
        level: Level::Readable,
    });
    each(
        &["bashMode", "border", "borderAccent"],
        &background,
        Level::Readable,
        &mut rules,
    );
    rules.push(Rule {
        token: "borderMuted",
        on: vec![Surface::Background],
        level: Level::Subtle,
    });
    each(
        &["mdQuoteBorder", "mdHr"],
        &surfaces(true, &[&MESSAGE_PANELS, &TOOL_PANELS]),
        Level::Readable,
        &mut rules,
    );
    rules.push(Rule {
        token: "scrollbarTrack",
        on: vec![Surface::Background],
        level: Level::Track,
    });
    rules.push(Rule {
        token: "scrollbarThumb",
        on: vec![Surface::Token("scrollbarTrack")],
        level: Level::Thumb,
    });
    for (index, token) in THINKING.iter().enumerate() {
        if let Some(level) = THINKING_LEVELS.get(index) {
            rules.push(Rule {
                token,
                on: vec![Surface::Background],
                level: *level,
            });
        }
    }
    rules
});

/// Relaxation compresses levels stronger than this one toward it before weakening all levels
/// (`READABLE_FLOOR`, `system-theme.ts:341`).
const fn readable_floor(appearance: Appearance) -> Level {
    match appearance {
        Appearance::Dark => Level::Readable,
        Appearance::Light => Level::Subtle,
    }
}

/// Body text uses the terminal's foreground when it reaches this level, which is clearly stronger
/// than muted (`FOREGROUND_LEVEL`, `system-theme.ts:344`).
const FOREGROUND_LEVEL: Level = Level::Emphasis;

/// Text-level tokens that take the terminal's foreground (`FOREGROUND_TOKENS`, `:347`).
const FOREGROUND_TOKENS: [&str; 3] = ["text", "userMessageText", "toolTitle"];

/// WCAG 2 contrast ratio that body text must reach on the surfaces it is drawn on (`:350`).
const TEXT_MINIMUM_WCAG_CONTRAST: f64 = 4.5;

/// Tokens in dependency order: every surface before the tokens drawn on it (`SOLVE_ORDER`,
/// `system-theme.ts:353-365`).
static SOLVE_ORDER: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    fn visit(token: &'static str, order: &mut Vec<&'static str>) {
        if order.contains(&token) {
            return;
        }
        for rule in RULES.iter().filter(|rule| rule.token == token) {
            for surface in &rule.on {
                if let Surface::Token(surface) = surface {
                    visit(surface, order);
                }
            }
        }
        order.push(token);
    }
    let mut order = Vec::new();
    for rule in RULES.iter() {
        visit(rule.token, &mut order);
    }
    order
});

// ============================================================================
// Public API
// ============================================================================

/// A saturation multiplier from 0 (grayscale) to 1 — the invariant `clamp(input.saturation ?? 1,
/// 0, 1)` (`system-theme.ts:466`) establishes, held by construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saturation(f64);

impl Saturation {
    /// Full colour.
    pub const FULL: Saturation = Saturation(1.0);
    /// No colour: the first frame renders in grayscale until the terminal's colours arrive.
    pub const GRAYSCALE: Saturation = Saturation(0.0);

    /// Clamp `value` into `[0, 1]`; NaN becomes grayscale.
    #[must_use]
    pub fn new(value: f64) -> Self {
        if value.is_nan() {
            return Saturation(0.0);
        }
        Saturation(value.clamp(0.0, 1.0))
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// What the terminal reported — pi's `SystemThemeInput` (`system-theme.ts:371-380`).
#[derive(Clone, Copy, Debug)]
pub struct SystemThemeInput {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    /// ANSI colours 0-15. Whole or absent: pi sets it only when the terminal reported all sixteen.
    pub palette: Option<[Rgb; 16]>,
    pub saturation: Saturation,
    /// Appearance when the terminal did not report its background, e.g. from its light/dark report
    /// or `COLORFGBG`.
    pub appearance_hint: Option<Appearance>,
}

impl Default for SystemThemeInput {
    /// A terminal that reported nothing, rendered in full colour.
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            palette: None,
            saturation: Saturation::FULL,
            appearance_hint: None,
        }
    }
}

/// One generated token colour — pi's `string | number` (`SystemThemeColors.colors`,
/// `system-theme.ts:384`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemColor {
    /// A hex colour.
    Rgb(Rgb),
    /// An ANSI palette index the terminal renders with its own theme.
    Indexed(u8),
    /// `""` — the terminal's default foreground or background.
    Default,
}

/// pi's `SystemThemeColors` (`system-theme.ts:382-388`).
#[derive(Clone, Debug)]
pub struct SystemThemeColors {
    /// One entry per [`system_tokens`] token.
    pub colors: BTreeMap<&'static str, SystemColor>,
    /// Foreground tokens rendered faint (SGR 2), for terminals that did not report colours.
    pub dim: Vec<&'static str>,
    pub appearance: Option<Appearance>,
}

/// OKLab lightness of an sRGB colour, 0-1 (`oklabLightness`, `system-theme.ts:391`).
fn oklab_lightness(color: Rgb) -> f64 {
    rgb_to_oklch(color.channels()).0
}

/// WCAG 2 relative luminance (`system-theme.ts:396-402`).
#[must_use]
pub fn relative_luminance(color: Rgb) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG 2 contrast ratio, 1-21 (`system-theme.ts:405-409`).
#[must_use]
pub fn wcag_contrast(first: Rgb, second: Rgb) -> f64 {
    let a = relative_luminance(first);
    let b = relative_luminance(second);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Whether a terminal is dark or light, from its reported colours: the direction of its own
/// foreground when text can be readable that way, otherwise dark when white text has more contrast
/// on the background than black text (`terminalAppearance`, `system-theme.ts:416-431`).
#[must_use]
pub fn terminal_appearance(background: Rgb, foreground: Option<Rgb>) -> Appearance {
    let white = Rgb::new(255, 255, 255);
    let black = Rgb::new(0, 0, 0);
    let white_contrast = wcag_contrast(white, background);
    let black_contrast = wcag_contrast(black, background);
    if let Some(foreground) = foreground {
        let foreground_l = oklab_lightness(foreground);
        let background_l = oklab_lightness(background);
        if (foreground_l - background_l).abs() > 0.05 {
            let appearance = if foreground_l > background_l {
                Appearance::Dark
            } else {
                Appearance::Light
            };
            let best = if appearance == Appearance::Dark {
                white_contrast
            } else {
                black_contrast
            };
            if best >= TEXT_MINIMUM_WCAG_CONTRAST {
                return appearance;
            }
        }
    }
    if white_contrast >= black_contrast {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

// ============================================================================
// Generation
// ============================================================================

/// Saturation weight at a lightness: a Gaussian (centre 0.5, sigma 0.25), 0 at black and white, 1
/// in the middle (`bellWeight`, `system-theme.ts:444-447`).
fn bell_weight(lightness: f64) -> f64 {
    let gaussian = |x: f64| (-((x - 0.5).powi(2)) / (2.0 * 0.25_f64.powi(2))).exp();
    (gaussian(lightness) - gaussian(0.0)) / (1.0 - gaussian(0.0))
}

/// A family's saturation curve relative to its maximum: 1 at mid lightness, `min / max` at black
/// and white (`saturationCurve`, `system-theme.ts:450-453`).
fn saturation_curve(family: Family, lightness: f64) -> f64 {
    let floor = if family.max > 0.0 {
        family.min / family.max
    } else {
        1.0
    };
    floor + (1.0 - floor) * bell_weight(lightness)
}

/// The target lightness for a level on a surface, or `None` where the level cannot be reached
/// (`levelTarget`, `system-theme.ts:456-460`).
fn level_target(level: Level, appearance: Appearance, surface_l: f64) -> Option<f64> {
    let curve = level.curve(appearance);
    if surface_l < curve.reachable.0 || surface_l > curve.reachable.1 {
        return None;
    }
    Some(
        curve
            .coefficients
            .iter()
            .enumerate()
            .fold(0.0, |sum, (power, coefficient)| {
                sum + coefficient * surface_l.powf(power as f64)
            }),
    )
}

/// `okhslColor(h, s, l)` over arguments that are in range by construction. Pi's version throws
/// outside `[0, 1]`; here an out-of-range value is clamped, which is unreachable from the
/// generator's own arithmetic and keeps this total.
fn okhsl_rgb(hue: f64, saturation: f64, lightness: f64) -> Rgb {
    okhsl_to_rgb(hue, saturation.clamp(0.0, 1.0), lightness.clamp(0.0, 1.0)).into()
}

/// A terminal colour's OKHSL channels and its OKLCH chroma (`SourceColor`, `system-theme.ts:594`).
#[derive(Clone, Copy, Debug)]
struct SourceColor {
    h: f64,
    s: f64,
    l: f64,
    chroma: f64,
}

fn source_of(color: Rgb) -> SourceColor {
    let (h, s, l) = rgb_to_okhsl(color.channels());
    SourceColor {
        h,
        s,
        l,
        chroma: rgb_to_oklch(color.channels()).1,
    }
}

/// A source colour's hue at another OKHSL lightness. Its saturation applies at its own lightness
/// and falls off toward black and white along the family's saturation curve, never rising above it.
///
/// OKHSL saturation is relative to the most chroma sRGB allows at a lightness, so the same
/// saturation can mean more chroma elsewhere: Catppuccin Frappe's pink `#f4b8e4` (chroma 0.089)
/// would become `#eb76d1` (0.180) at the lightness the accent needs. Chroma is therefore also capped
/// at the source's, with the same falloff (`anchored`, `system-theme.ts:610-617`).
fn anchored(source: SourceColor, family: Family, lightness: f64, saturation: Saturation) -> Rgb {
    let anchor = saturation_curve(family, source.l);
    let falloff = if anchor > 0.0 {
        (saturation_curve(family, lightness) / anchor).min(1.0)
    } else {
        1.0
    };
    let color = okhsl_rgb(source.h, source.s * falloff * saturation.get(), lightness);
    let cap = source.chroma * falloff * saturation.get();
    let (l, c, _) = rgb_to_oklch(color.channels());
    if c <= cap {
        color
    } else {
        oklch_color(l, cap, source.h).map_or(color, Rgb::from)
    }
}

/// Move a text colour toward white or black until it reaches the WCAG minimum on every surface
/// (`withTextContrast`, `system-theme.ts:620-635`).
fn with_text_contrast(color: Rgb, surfaces: &[Rgb], lighter: bool) -> Rgb {
    let meets = |candidate: Rgb| {
        surfaces
            .iter()
            .all(|surface| wcag_contrast(candidate, *surface) >= TEXT_MINIMUM_WCAG_CONTRAST)
    };
    if meets(color) {
        return color;
    }
    let (h, s, l) = rgb_to_okhsl(color.channels());
    let at = |lightness: f64| okhsl_rgb(h, s, lightness);
    let extreme = if lighter { 1.0 } else { 0.0 };
    if !meets(at(extreme)) {
        return at(extreme);
    }
    let (mut low, mut high) = (l, extreme);
    for _ in 0..20 {
        let middle = (low + high) / 2.0;
        if meets(at(middle)) {
            high = middle;
        } else {
            low = middle;
        }
    }
    at(high)
}

/// Colours for terminals that reported nothing: the terminal renders ANSI indices 0-15 and the
/// default colours with its own theme, so they fit any background. Neutral tokens below body text
/// are faint (SGR 2) instead of bright black, which some themes make nearly invisible. Panels have
/// no background (`indexedColors`, `system-theme.ts:642-655`).
fn indexed_colors(saturation: Saturation, appearance: Option<Appearance>) -> SystemThemeColors {
    let mut colors = BTreeMap::new();
    let mut dim = Vec::new();
    for (token, family_name) in TOKEN_FAMILIES {
        if PANELS.contains(&token) {
            colors.insert(token, SystemColor::Default);
            continue;
        }
        let neutral = family_name == FamilyName::Neutral;
        let color = if !neutral && saturation.get() > 0.0 {
            let slot = token_slot(token).unwrap_or(family_name.family().slot);
            u8::try_from(slot).map_or(SystemColor::Default, SystemColor::Indexed)
        } else {
            SystemColor::Default
        };
        colors.insert(token, color);
        if neutral && !FOREGROUND_TOKENS.contains(&token) {
            dim.push(token);
        }
    }
    SystemThemeColors {
        colors,
        dim,
        appearance,
    }
}

/// Generate the system theme's colours from the terminal's reported colours (pi
/// `generateSystemThemeColors`, `system-theme.ts:465-587`).
#[must_use]
pub fn generate_system_theme_colors(input: &SystemThemeInput) -> SystemThemeColors {
    let saturation = input.saturation;
    let Some(background) = input.background else {
        return indexed_colors(saturation, input.appearance_hint);
    };
    let foreground = input.foreground;
    let palette: Option<[SourceColor; 16]> = input.palette.map(|palette| palette.map(source_of));

    let appearance = terminal_appearance(background, foreground);
    let lighter = appearance == Appearance::Dark;
    let extreme = if lighter { 1.0 } else { 0.0 };
    let background_l = oklab_lightness(background);

    // A token's colour at an OKLab lightness. With a palette, the palette colour's saturation
    // applies at its own lightness and falls off toward black and white along the family's curve,
    // never rising above it.
    let paint = |token: &str, oklab_l: f64| -> Rgb {
        let lightness = oklab_to_okhsl_lightness(oklab_l);
        let family = family_of(token).unwrap_or(FamilyName::Neutral).family();
        match &palette {
            None => okhsl_rgb(
                family.hue,
                (family.min + (family.max - family.min) * bell_weight(lightness))
                    * saturation.get(),
                lightness,
            ),
            Some(palette) => {
                let slot = token_slot(token).unwrap_or(family.slot);
                match palette.get(slot) {
                    Some(source) => anchored(*source, family, lightness, saturation),
                    None => okhsl_rgb(family.hue, family.min * saturation.get(), lightness),
                }
            }
        }
    };

    // The lightness a rule needs on a surface, relaxed by `t`: from 0 to 1, levels stronger than the
    // readable floor move toward it; from 1 to 2, all levels move toward the surface itself.
    let target = |level: Level, surface_l: f64, t: f64| -> Option<f64> {
        let reached = level_target(level, appearance, surface_l);
        if reached.is_none() && t == 0.0 {
            return None;
        }
        let distance = reached.unwrap_or(extreme) - surface_l;
        let floor = level_target(readable_floor(appearance), appearance, surface_l)
            .unwrap_or(extreme)
            - surface_l;
        let compressed = if distance.abs() > floor.abs() {
            distance - (distance - floor) * t.min(1.0)
        } else {
            distance
        };
        Some(surface_l + compressed * (1.0 - (t - 1.0).max(0.0)))
    };

    // Keep a panel light enough (or dark enough) that white (or black) text still reaches the body
    // text minimum on it. This only matters for backgrounds near mid-gray, where it barely does on
    // the background.
    let extreme_text = if lighter {
        Rgb::new(255, 255, 255)
    } else {
        Rgb::new(0, 0, 0)
    };
    let readable = |color: Rgb| wcag_contrast(extreme_text, color) >= TEXT_MINIMUM_WCAG_CONTRAST;
    let limit_panel = |token: &str, l: f64| -> Rgb {
        let color = paint(token, l);
        if readable(color) {
            return color;
        }
        let (mut low, mut high) = (background_l, l);
        for _ in 0..20 {
            let middle = (low + high) / 2.0;
            if readable(paint(token, middle)) {
                low = middle;
            } else {
                high = middle;
            }
        }
        paint(token, low)
    };

    let solve = |t: f64| -> Option<BTreeMap<&'static str, Rgb>> {
        let mut colors: BTreeMap<&'static str, Rgb> = BTreeMap::new();
        colors.insert(Surface::Background.key(), background);
        for token in SOLVE_ORDER.iter() {
            let mut targets: Vec<f64> = Vec::new();
            for rule in RULES.iter().filter(|rule| rule.token == *token) {
                for surface in &rule.on {
                    let surface_color = colors.get(surface.key()).copied().unwrap_or(background);
                    let value = target(rule.level, oklab_lightness(surface_color), t)?;
                    if !(0.0..=1.0).contains(&value) {
                        return None;
                    }
                    targets.push(value);
                }
            }
            let l = if lighter {
                targets.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            } else {
                targets.iter().copied().fold(f64::INFINITY, f64::min)
            };
            let color = if PANELS.contains(token) {
                limit_panel(token, l)
            } else {
                paint(token, l)
            };
            colors.insert(token, color);
        }
        Some(colors)
    };

    let mut relaxation = 0.0;
    let mut colors = solve(0.0);
    if colors.is_none() {
        // Mid-gray backgrounds cannot fit every level: relax as little as possible. Full relaxation
        // always fits.
        let (mut low, mut high) = (0.0_f64, 2.0_f64);
        colors = solve(high);
        for _ in 0..20 {
            let middle = (low + high) / 2.0;
            match solve(middle) {
                Some(attempt) => {
                    high = middle;
                    colors = Some(attempt);
                }
                None => low = middle,
            }
        }
        relaxation = high;
    }
    let solved = colors.unwrap_or_default();
    let surfaces_of = |token: &str| -> Vec<Rgb> {
        RULES
            .iter()
            .filter(|rule| rule.token == token)
            .flat_map(|rule| rule.on.iter())
            .map(|surface| solved.get(surface.key()).copied().unwrap_or(background))
            .collect()
    };

    let mut result: BTreeMap<&'static str, SystemColor> = BTreeMap::new();
    for token in system_tokens() {
        let color = solved
            .get(token)
            .copied()
            .map_or(SystemColor::Default, SystemColor::Rgb);
        result.insert(token, color);
    }

    for token in FOREGROUND_TOKENS {
        let surfaces = surfaces_of(token);
        // Body text uses the terminal's own foreground where it is clearly stronger than muted text;
        // otherwise the foreground's hue at just enough lightness.
        let mut text = solved.get(token).copied();
        if let Some(foreground) = foreground {
            let targets: Vec<Option<f64>> = surfaces
                .iter()
                .map(|surface| target(FOREGROUND_LEVEL, oklab_lightness(*surface), relaxation))
                .collect();
            let all_reachable = targets
                .iter()
                .all(|value| value.is_some_and(|v| (0.0..=1.0).contains(&v)));
            if all_reachable {
                let values = targets.iter().flatten().copied();
                let needed = if lighter {
                    values.fold(f64::NEG_INFINITY, f64::max)
                } else {
                    values.fold(f64::INFINITY, f64::min)
                };
                let foreground_l = oklab_lightness(foreground);
                let sufficient = if lighter {
                    foreground_l >= needed
                } else {
                    foreground_l <= needed
                };
                if sufficient {
                    result.insert(token, SystemColor::Default);
                    continue;
                }
                text = Some(anchored(
                    source_of(foreground),
                    FamilyName::Neutral.family(),
                    oklab_to_okhsl_lightness(needed),
                    saturation,
                ));
            }
        }
        // Body text keeps at least 4.5:1 on the surfaces it is drawn on, even on relaxed mid-gray
        // backgrounds.
        if let Some(text) = text {
            result.insert(
                token,
                SystemColor::Rgb(with_text_contrast(text, &surfaces, lighter)),
            );
        }
    }
    SystemThemeColors {
        colors: result,
        dim: Vec::new(),
        appearance: Some(appearance),
    }
}

// ============================================================================
// Concrete colours (pi `Theme.colors`)
// ============================================================================

/// How far a faint token is mixed toward the background when it must be a concrete colour
/// (`mixColors(color, background, 0.4)`, `theme.ts:334`).
const DIM_MIX: f64 = 0.4;

/// Pi's `GUESSED_DEFAULT_COLORS` (`theme.ts:216-219`): the terminal's default `(foreground,
/// background)` when it reported none, by the appearance the theme is designed for.
#[must_use]
pub fn guessed_default_colors(appearance: Appearance) -> (Rgb, Rgb) {
    match appearance {
        Appearance::Dark => (Rgb::new(0xe5, 0xe5, 0xe7), Rgb::new(0, 0, 0)),
        Appearance::Light => (Rgb::new(0, 0, 0), Rgb::new(0xff, 0xff, 0xff)),
    }
}

/// Pi's `BACKGROUND_TOKENS` (`theme.ts:573-581`): the tokens a theme paints as backgrounds.
#[must_use]
pub fn is_background_token(token: &str) -> bool {
    PANELS.contains(&token)
}

/// Pi `mixColors(first, second, amount)` in OKLCH (`colors.ts:241-257`), the default space: lightness
/// and chroma interpolate linearly, hue along the shorter arc, and a colourless end takes the other
/// end's hue.
fn mix_oklch(first: Rgb, second: Rgb, amount: f64) -> Rgb {
    let (al, ac, ah) = rgb_to_oklch(first.channels());
    let (bl, bc, bh) = rgb_to_oklch(second.channels());
    let first_hue = if ac < 1e-7 { bh } else { ah };
    let second_hue = if bc < 1e-7 { first_hue } else { bh };
    let hue_delta = ((second_hue - first_hue + 540.0) % 360.0) - 180.0;
    let (r, g, b) = crate::color::oklch_to_rgb(
        al + (bl - al) * amount,
        ac + (bc - ac) * amount,
        first_hue + hue_delta * amount,
    );
    Rgb::new(r, g, b)
}

/// The generated theme's tokens as concrete colours — pi's `Theme.colors` getter
/// (`theme.ts:321-338`). A palette index is the standard xterm colour it is named after; a token
/// set to the terminal default takes the terminal's reported foreground (or background, for a
/// background token), else the guess for the theme's appearance; and a faint token is mixed
/// [`DIM_MIX`] of the way toward the background.
///
/// `terminal_appearance` is what a theme with no appearance of its own takes (`Theme.appearance`,
/// `theme.ts:313-315`).
#[must_use]
pub fn concrete_colors(
    generated: &SystemThemeColors,
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    terminal_appearance: Appearance,
) -> BTreeMap<&'static str, Rgb> {
    let (guess_fg, guess_bg) =
        guessed_default_colors(generated.appearance.unwrap_or(terminal_appearance));
    let default_fg = foreground.unwrap_or(guess_fg);
    let default_bg = background.unwrap_or(guess_bg);
    let mut colors: BTreeMap<&'static str, Rgb> = generated
        .colors
        .iter()
        .map(|(token, color)| {
            let rgb = match color {
                SystemColor::Rgb(rgb) => *rgb,
                SystemColor::Indexed(index) => crate::ColorSpec::Indexed(*index)
                    .to_rgb()
                    .map_or(default_fg, |(r, g, b)| Rgb::new(r, g, b)),
                SystemColor::Default if is_background_token(token) => default_bg,
                SystemColor::Default => default_fg,
            };
            (*token, rgb)
        })
        .collect();
    for token in &generated.dim {
        if let Some(color) = colors.get_mut(token) {
            *color = mix_oklch(*color, default_bg, DIM_MIX);
        }
    }
    colors
}

#[cfg(test)]
#[path = "system_theme_tests.rs"]
mod tests;
