//! Chrome-tail components (spec/tui/01 §startup-help; Pi `components/{keybinding-hints,
//! visual-truncate,bordered-loader}.ts`).
//!
//! Three small, dependency-free pieces of Pi's interactive chrome that sit *around* the transcript +
//! editor + footer the crate already renders:
//!
//! - [`format_key_text`] / [`key_hint_line`] / [`compact_hints`] — the keybinding-hint formatter and
//!   the startup "interrupt · clear/exit · / commands · ! bash · …" bar (`keybinding-hints.ts`,
//!   `interactive-mode.ts:697-703`), sourced from the **live** [`Keymap`] so rebinds flow through.
//! - [`truncate_to_visual_lines`] — the shared tail-truncate used by tool/bash blocks
//!   (`visual-truncate.ts` `truncateToVisualLines`): keep the last *N* wrapped lines, report how many
//!   were hidden.
//! - [`BorderedLoader`] — a `DynamicBorder`-delimited spinner+message with an optional cancel hint
//!   (`bordered-loader.ts` `BorderedLoader`), the loader chrome extension UI and long ops draw inline.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::keymap::{Action, EditorAction, EditorKeymap, Keymap};
use crate::selector::border_rule;
use crate::status_indicator::SPINNER_FRAMES;
use crate::theme::UiTheme;

/// Format a key string for display (`formatKeyText`, `keybinding-hints.ts:18-27`): split on `/`
/// (alternatives) and `+` (chords); on macOS rewrite the `alt` modifier to `option`. With
/// `capitalize`, each part is title-cased (`keyDisplayText`).
pub fn format_key_text(key: &str, capitalize: bool) -> String {
    key.split('/')
        .map(|alt| {
            alt.split('+')
                .map(|part| format_key_part(part, capitalize))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Format one chord part: macOS shows `option` for `alt`; `capitalize` title-cases.
fn format_key_part(part: &str, capitalize: bool) -> String {
    let display = if cfg!(target_os = "macos") && part.eq_ignore_ascii_case("alt") {
        "option"
    } else {
        part
    };
    if capitalize {
        let mut chars = display.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    } else {
        display.to_string()
    }
}

/// One hint as a `[dim key] [muted description]` [`Line`] (`keyHint`, `keybinding-hints.ts:39-41`).
/// The key text is formatted via [`format_key_text`].
pub fn key_hint_line(key: &str, description: &str, theme: &UiTheme) -> Line<'static> {
    Line::from(key_hint_spans(key, description, theme))
}

/// The styled spans of a single key hint (dim key + muted ` description`), for composing a hint bar.
pub fn key_hint_spans(key: &str, description: &str, theme: &UiTheme) -> Vec<Span<'static>> {
    vec![
        Span::styled(format_key_text(key, false), theme.dim_style()),
        Span::styled(format!(" {description}"), theme.muted_style()),
    ]
}

/// The compact startup-help hint pairs (`(key, description)`), in Pi's order
/// (v0.84.1 `interactive-mode.ts:936-942` `compactInstructions`), with the interrupt/clear/exit keys
/// resolved from the **live** keymap (so a rebind is reflected). `/` and `!` are literal affordances.
pub fn compact_hints(keymap: &Keymap) -> Vec<(String, String)> {
    // `hint(kb, desc)` is `keyHint` → `keyText`, and the clear/exit pair is
    // `rawKeyHint(`${keyText("app.clear")}/${keyText("app.exit")}`, …)` — every key here resolves
    // through `keyText`, which joins ALL bound keys with `/` (`keybinding-hints.ts:29-36`). So these
    // are [`Keymap::keys_label`], not the first-key `key_label`: a two-key rebind must show both.
    let interrupt = keymap
        .keys_label(Action::Interrupt)
        .unwrap_or_else(|| "escape".into());
    let clear = keymap
        .keys_label(Action::Clear)
        .unwrap_or_else(|| "ctrl+c".into());
    let exit = keymap
        .keys_label(Action::Quit)
        .unwrap_or_else(|| "ctrl+d".into());
    let expand = keymap
        .keys_label(Action::ToolsExpand)
        .unwrap_or_else(|| "ctrl+o".into());
    vec![
        (interrupt, "interrupt".to_string()),
        (format!("{clear}/{exit}"), "clear/exit".to_string()),
        ("/".to_string(), "commands".to_string()),
        ("!".to_string(), "bash".to_string()),
        (expand, "more".to_string()),
    ]
}

/// Whether the startup details (model scope, loaded resources) are shown this session — pi's
/// `shouldShowStartupDetails()` (`interactive-mode.ts:1415-1417` @v1.0.0). The header's onboarding
/// line names them only when they are there to be revealed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartupDetails {
    /// Shown: the onboarding line promises "full startup help and loaded resources".
    #[default]
    Shown,
    /// Hidden (`quietStartup: "header"`): the line promises the startup help alone.
    Hidden,
}

/// The onboarding line printed directly under the compact hint bar — verbatim
/// `interactive-mode.ts:1044-1048` @v1.0.0:
/// `theme.fg("dim", \`Press ${keyText("app.tools.expand")} to show full startup help${showDetails ?
/// " and loaded resources" : ""}.\`)`. It is the only place a new user is told the expanded help
/// exists, and cyrup had no counterpart at all (`grep -rn "to show full startup help" crates/`
/// found nothing).
pub fn compact_onboarding(keymap: &Keymap, details: StartupDetails) -> String {
    // `keyText("app.tools.expand")` — all bound keys joined with `/` (`keybinding-hints.ts:29-36`).
    let expand = keymap
        .keys_label(Action::ToolsExpand)
        .unwrap_or_else(|| "ctrl+o".into());
    let resources = match details {
        StartupDetails::Shown => " and loaded resources",
        StartupDetails::Hidden => "",
    };
    format!("Press {expand} to show full startup help{resources}.")
}

/// The product wordmark that opens the startup header — pi's `piWordmark()`
/// (`components/pi-logo.ts:37-39`), rebranded pi→cyrup.
///
/// CAPITALIZED deliberately, and not to be "fixed" to lowercase. `piWordmark` is coral `"P"` plus
/// yellow `"i"` — the capitalized `"Pi"` (`pi-logo.ts:38`), NOT the lowercase `APP_NAME` (`"pi"`)
/// that the resume hint and the terminal title use. So the literal port is `"Cyrup"`, matching
/// [`STARTUP_ONBOARDING`]'s prose rather than `resume_hint::APP_NAME` (`resume_hint.rs:46`, which is
/// lowercase `"cyrup"`).
pub const APP_WORDMARK: &str = "Cyrup";

/// The startup header's FIRST logical line — pi's `withLogo` on its text-wordmark branch:
/// `` `${piWordmark()} ${theme.fg("dim", `v${this.version}`)}\n${hints}` ``
/// (`interactive-mode.ts:1016`).
///
/// The structure is reproduced exactly: the mark, then a PLAIN (unstyled) space, then a dim
/// `v{version}` with the `v` INSIDE the dim span. Upstream puts that space OUTSIDE the dim span on
/// BOTH `withLogo` branches (`:1016` and `:1018`), so it carries no style here either.
///
/// The version is this crate's `CARGO_PKG_VERSION`, which is `version.workspace = true`
/// (`crates/cyrup-tui/Cargo.toml:4`) and therefore the same string the `cyrup` binary reports —
/// upstream's `this.version` is likewise the app package's `VERSION` (field `:477`, assigned
/// `:621`).
///
/// [CYRUP-DELTA] **Only the wordmark branch is ported, and it is ported unconditionally.** Upstream
/// has TWO `withLogo` branches, chosen by `showLogo = supportsPiLogo()` (`:1014`;
/// `pi-logo.ts:32-33` = `!isAppleTerminalSession()`). The DEFAULT branch (`:1018-1019`) draws
/// `piLogoLines()` (`pi-logo.ts:19-25`) — a 4-column × 2-row half-block GRAPHIC in pi's FIXED brand
/// RGB (coral `228,138,122`, blue `79,142,179`, yellow `234,182,93`, `pi-logo.ts:4-6`) — and the
/// wordmark is merely its Apple-Terminal fallback. cyrup takes the fallback for every terminal
/// because the graphic is pi's own brand mark and cyrup has no mark of its own to substitute. Both
/// branches emit the SAME two logical lines, so the block's row arithmetic
/// ([`COMPACT_HINT_ROWS`]) is unaffected by which one is taken.
///
/// [CYRUP-DELTA] The colours are the theme's `accent` plus bold, not pi's fixed brand RGB — for the
/// same reason, and because a hard-coded triplet would fight every theme. This is an upstream rule
/// rather than an invention: pi v0.85.1's `logo` was
/// `theme.bold(theme.fg("accent", APP_NAME)) + theme.fg("dim", " v{version}")`, which is also the
/// form `TUI-018`'s **Fix** asks for.
pub fn logo_line(theme: &UiTheme) -> Line<'static> {
    let mut spans = vec![Span::styled(
        APP_WORDMARK,
        theme.accent_style().add_modifier(Modifier::BOLD),
    )];
    // [CYRUP-DELTA] — pi always renders its version beside the wordmark, because pi HAS a version.
    // cyrup's workspace version is still the cargo placeholder `0.0.0` (root `Cargo.toml`), and
    // `Cyrup v0.0.0` as the first thing a user sees reads as a broken build rather than as an
    // unreleased one. The version span is therefore omitted while the version is that placeholder,
    // and appears by itself the moment the workspace carries a real one — no further change needed.
    //
    // This is the same `0.0.0` that `TUI-011` is held OPEN on (its "What's New" notice has no
    // version to compare against), so the two rows now agree about it instead of one shipping what
    // the other calls a blocker.
    if !is_placeholder_version(APP_VERSION) {
        // Unstyled, per `:1016` — the space sits between the two spans, not inside the dim one.
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("v{APP_VERSION}"), theme.dim_style()));
    }
    Line::from(spans)
}

/// The crate version the header would show, as a value rather than a macro, so a test can assert
/// BOTH branches of [`logo_line`] instead of deriving its expectation from the same `env!` the code
/// reads — which is how the version half of an assertion becomes unfailable.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `true` for cargo's unset-version placeholder. Kept a named predicate so the reason is greppable
/// from both the header and the ledger row that cites it.
#[must_use]
pub fn is_placeholder_version(version: &str) -> bool {
    version == "0.0.0"
}

/// pi's `expandedInstructions` — the nineteen hints the header carries when it is EXPANDED
/// (`interactive-mode.ts:1024-1048`), in upstream order, one per line (`:1048` `join("\n")`).
///
/// Four details of the upstream list are load-bearing and are reproduced rather than smoothed over:
///
/// - SIX entries are `rawKeyHint` — a literal affordance, not a binding: `` `{clear} twice` ``
///   (`:1028`), the `cycleForward/cycleBackward` pair (`:1033-1036`), `/` (`:1041`), `!` (`:1042`),
///   `!!` (`:1043`) and `drop files` (`:1047`). Both `keyHint` and `rawKeyHint` emit
///   `fg("dim", key) + fg("muted", " " + description)` (`keybinding-hints.ts:42-48`), which is
///   exactly [`key_hint_spans`], so the two kinds differ only in how the key string is sourced.
/// - `keyHint("tui.editor.deleteToLineEnd", "to delete to end")` (`:1031`) resolves through the
///   EDITOR keymap, not the app keymap — hence the second map parameter.
/// - `clear` appears TWICE, as `app.clear` "to clear" (`:1027`) and as `{clear} twice` "to exit"
///   (`:1028`); `app.exit` is separately "to exit (empty)" (`:1029`).
/// - An unbound action yields an EMPTY key string, because upstream's `formatKeys` returns `""`
///   for no keys (`keybinding-hints.ts:29-31`) — so these use `unwrap_or_default` rather than
///   inventing a fallback binding.
pub fn expanded_hints(
    theme: &UiTheme,
    keymap: &Keymap,
    editor_keymap: &EditorKeymap,
) -> Vec<Line<'static>> {
    // `keyText(kb)` — ALL bound keys joined with `/` (`keybinding-hints.ts:29-36`), `""` if none.
    let k = |action: Action| keymap.keys_label(action).unwrap_or_default();
    let clear = k(Action::Clear);
    let fwd = k(Action::ModelCycleForward);
    let back = k(Action::ModelCycleBackward);
    let del_to_end = editor_keymap
        .keys_label(EditorAction::DeleteToLineEnd)
        .unwrap_or_default();
    [
        (k(Action::Interrupt), "to interrupt"),
        (clear.clone(), "to clear"),
        (format!("{clear} twice"), "to exit"),
        (k(Action::Quit), "to exit (empty)"),
        (k(Action::Suspend), "to suspend"),
        (del_to_end, "to delete to end"),
        (k(Action::ThinkingCycle), "to cycle thinking level"),
        (format!("{fwd}/{back}"), "to cycle models"),
        (k(Action::ModelSelect), "to select model"),
        (k(Action::ToolsExpand), "to expand tools"),
        (k(Action::ThinkingToggle), "to expand thinking"),
        (k(Action::ExternalEditor), "for external editor"),
        ("/".to_string(), "for commands"),
        ("!".to_string(), "to run bash"),
        ("!!".to_string(), "to run bash (no context)"),
        (k(Action::FollowUp), "to queue follow-up"),
        (k(Action::Dequeue), "to edit all queued messages"),
        (
            k(Action::ClipboardPasteImage),
            "to paste files on macOS, images, or text",
        ),
        ("drop files".to_string(), "to attach"),
    ]
    .into_iter()
    .map(|(key, desc)| key_hint_line(&key, desc, theme))
    .collect()
}

/// The block's closing sentence — `onboarding`, `interactive-mode.ts:947-950`:
/// `theme.fg("dim", \`Pi can explain its own features and look up its docs. Ask it how to use or
/// extend Pi.\`)`.
///
/// It is the FIFTH part of the same `ExpandableText` body the hint bar comes from
/// (`${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}`, `:952`) — one
/// statement, one theme call — and it is also the one part upstream keeps in BOTH the collapsed and
/// the expanded body (`:953`), so dropping it removed the line unconditionally. The product name is
/// rebranded pi→cyrup exactly as `terminal_title.rs` and `resume_hint.rs` rebrand `APP_NAME`.
pub const STARTUP_ONBOARDING: &str =
    "Cyrup can explain its own features and look up its docs. Ask it how to use or extend Cyrup.";

/// The `paddingX` of the startup `ExpandableText` — `new ExpandableText(…, 1, 0)`
/// (`interactive-mode.ts:951-957`). `Text.render` emits `leftMargin + line + rightMargin` around
/// EVERY wrapped row and wraps at `contentWidth = max(1, width - paddingX * 2)` (`text.ts:64-76`).
const HINT_PADDING_X: u16 = 1;

/// Rows the COLLAPSED block occupies when nothing wraps: a framing blank, the logo/version line,
/// the hint bar, the compact onboarding line, the body's own blank, the closing onboarding line,
/// and a second framing blank.
///
/// The block is upstream's startup `BuiltInHeader`, whose collapsed body is
/// `${withLogo(compactInstructions())}\n${compactOnboarding()}\n\n${onboarding()}`
/// (`interactive-mode.ts:1065`), framed by a `Spacer(1)` on each side (`:1075-1077`). `withLogo`
/// (`:1015-1020`) prepends the logo line and puts the hints on the line BELOW it — on BOTH of its
/// branches — so the body is 5 logical lines and 1 + 5 + 1 = **7**.
///
/// This replaces an earlier `6`, which encoded cyrup not drawing the `logo` part at all; it now
/// draws it ([`logo_line`]).
///
/// This is the UNWRAPPED count, and it is the COLLAPSED one: the expanded body swaps the one-line
/// hint bar for [`expanded_hints`]' nineteen lines and drops `compactOnboarding`, so it is taller.
/// On a narrow terminal the text rows wrap and the block grows either way, which is why the layout
/// must ask [`compact_hint_height`] rather than use this constant directly.
pub const COMPACT_HINT_ROWS: u16 = 7;

/// One row group of the startup hint block: its logical lines, its WRAPPED height at the block's
/// content width, and the order in which it is given up when the area is too short.
struct HintEntry {
    lines: Vec<Line<'static>>,
    rows: u16,
    /// Higher is dropped first; the hint bar is `0` and is never dropped.
    drop_rank: u8,
}

/// The block's content width — `contentWidth = Math.max(1, width - paddingX * 2)` (`text.ts:64`).
fn hint_content_width(width: u16) -> u16 {
    width
        .saturating_sub(HINT_PADDING_X.saturating_mul(2))
        .max(1)
}

/// The block's row groups, each already measured against `width`'s wrapping.
///
/// `expanded` selects which of pi's two `BuiltInHeader` bodies is built (`:1065` collapsed,
/// `:1066` expanded). Both open with [`logo_line`] via `withLogo` (`:1015-1020`) and both END with
/// `onboarding()` — upstream keeps that line in BOTH bodies — but the expanded body swaps the
/// one-line hint bar for [`expanded_hints`]' nineteen lines and carries NO `compactOnboarding`,
/// whose whole job is to advertise the expansion that has already happened.
///
/// The drop ranks degrade the block from its EDGES INWARD, so the hints — the only rows carrying
/// information the user cannot get anywhere else — are the last thing standing: trailing blank,
/// leading blank, closing onboarding, the body's inner blank, the compact onboarding line, the
/// logo/version line, and only then the hints themselves. A previous revision put the framing blank
/// FIRST in a fixed-height, top-aligned `Paragraph`, so a one-row budget drew the blank and the bar
/// vanished entirely.
fn compact_hint_entries(
    theme: &UiTheme,
    keymap: &Keymap,
    editor_keymap: &EditorKeymap,
    width: u16,
    details: StartupDetails,
    expanded: bool,
) -> Vec<HintEntry> {
    let content = hint_content_width(width);
    let blank = |rank: u8| HintEntry {
        lines: vec![Line::default()],
        rows: 1,
        drop_rank: rank,
    };
    let text = |lines: Vec<Line<'static>>, rank: u8| HintEntry {
        rows: crate::transcript::wrapped_height(&lines, content as usize).min(u16::MAX as usize)
            as u16,
        lines,
        drop_rank: rank,
    };

    // `compactInstructions.join(theme.fg("muted", " · "))` (`interactive-mode.ts:942`).
    let mut bar: Vec<Span<'static>> = Vec::new();
    for (i, (key, desc)) in compact_hints(keymap).into_iter().enumerate() {
        if i > 0 {
            bar.push(Span::styled(" · ", theme.muted_style()));
        }
        bar.extend(key_hint_spans(&key, &desc, theme));
    }

    // The closing `onboarding()` line, kept in BOTH bodies (`:1065` and `:1066`).
    let closing = |rank: u8| {
        text(
            vec![Line::styled(
                STARTUP_ONBOARDING.to_string(),
                theme.dim_style(),
            )],
            rank,
        )
    };

    if expanded {
        // `${withLogo(expandedInstructions())}\n\n${onboarding()}` (`:1066`): the logo line, the
        // nineteen hints, the body's `\n\n` blank, the closing line — and NO `compactOnboarding`.
        return vec![
            blank(5),
            text(vec![logo_line(theme)], 1),
            text(expanded_hints(theme, keymap, editor_keymap), 0),
            blank(3),
            closing(4),
            blank(6),
        ];
    }

    // `${withLogo(compactInstructions())}\n${compactOnboarding()}\n\n${onboarding()}` (`:1065`).
    vec![
        blank(5),
        text(vec![logo_line(theme)], 1),
        text(vec![Line::from(bar)], 0),
        text(
            vec![Line::styled(
                compact_onboarding(keymap, details),
                theme.dim_style(),
            )],
            2,
        ),
        blank(3),
        closing(4),
        blank(6),
    ]
}

/// The block's height at `width` with pi's wrapping applied (`wrapTextWithAnsi(normalizedText,
/// contentWidth)`, `text.ts:67`), so the layout reserves the rows the block will actually need.
///
/// cyrup previously reserved a fixed row count and rendered a fixed-height `Paragraph` with no
/// `.wrap()`, so on a narrow terminal the overflowing half of each line was simply lost.
pub fn compact_hint_height(
    theme: &UiTheme,
    keymap: &Keymap,
    editor_keymap: &EditorKeymap,
    width: u16,
    details: StartupDetails,
    expanded: bool,
) -> u16 {
    compact_hint_entries(theme, keymap, editor_keymap, width, details, expanded)
        .iter()
        .map(|e| e.rows)
        .fold(0u16, u16::saturating_add)
}

/// The compact hint block as document rows — the built-in header pi puts at the top of its
/// `documentContainer` (`headerContainer = [Spacer(1), builtInHeader, Spacer(1)]`,
/// `interactive-mode.ts:1061-1065` @v1.0.0), for the renderer that scrolls it with the
/// conversation.
///
/// The same row groups [`render_compact_hints`] paints, each wrapped at the block's content
/// width and inset by its `paddingX`, one [`Line`] per display row at `width`. Nothing is dropped
/// from the edges inward here: that degradation exists to keep the bar visible in a fixed-height
/// slot, and a scrolled document has no slot to overflow.
pub fn compact_hint_lines(
    theme: &UiTheme,
    keymap: &Keymap,
    editor_keymap: &EditorKeymap,
    width: u16,
    details: StartupDetails,
    expanded: bool,
) -> Vec<Line<'static>> {
    let content = usize::from(hint_content_width(width));
    let pad = width >= 3;
    let base = theme.base_style();
    compact_hint_entries(theme, keymap, editor_keymap, width, details, expanded)
        .into_iter()
        .flat_map(|entry| entry.lines)
        .flat_map(|line| crate::transcript::wrap_line(&line, content))
        .map(|mut row| {
            if pad {
                row.spans
                    .insert(0, Span::raw(" ".repeat(usize::from(HINT_PADDING_X))));
            }
            // The block's own background, as `render_compact_hints` paints it under the groups.
            row.style = base.patch(row.style);
            row
        })
        .collect()
}

/// Render the compact hint block into `area`, wrapping each row group at
/// [`content width`](hint_content_width) and insetting it by the `paddingX 1` margin.
///
/// If `area` is shorter than [`compact_hint_height`] the block degrades from its edges inward (see
/// [`compact_hint_entries`]) until it fits, so the hint bar survives down to a one-row budget.
pub fn render_compact_hints(
    frame: &mut Frame,
    area: Rect,
    theme: &UiTheme,
    keymap: &Keymap,
    editor_keymap: &EditorKeymap,
    details: StartupDetails,
    expanded: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut entries =
        compact_hint_entries(theme, keymap, editor_keymap, area.width, details, expanded);
    let total = |es: &[HintEntry]| es.iter().map(|e| e.rows).fold(0u16, u16::saturating_add);
    while total(&entries) > area.height {
        // Give up the outermost droppable group; `drop_rank == 0` (the bar) is never a candidate.
        let Some(idx) = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.drop_rank > 0)
            .max_by_key(|(_, e)| e.drop_rank)
            .map(|(i, _)| i)
        else {
            break;
        };
        entries.remove(idx);
    }

    // Paint the block's own background first: each group renders into an INSET rect, so the padding
    // columns would otherwise keep whatever was in the buffer.
    frame.render_widget(
        Paragraph::new(Vec::<Line<'static>>::new()).style(theme.base_style()),
        area,
    );

    // `paddingX` only fits once the row is at least 3 columns wide; below that upstream's
    // `Math.max(1, width - 2)` collapses the content anyway.
    let pad = if area.width >= 3 { HINT_PADDING_X } else { 0 };
    let mut y = area.y;
    let end = area.y.saturating_add(area.height);
    for entry in entries {
        if y >= end {
            break;
        }
        let height = entry.rows.min(end.saturating_sub(y));
        let rect = Rect {
            x: area.x.saturating_add(pad),
            y,
            width: hint_content_width(area.width),
            height,
        };
        frame.render_widget(
            Paragraph::new(entry.lines)
                .wrap(Wrap { trim: false })
                .style(theme.base_style()),
            rect,
        );
        y = y.saturating_add(height);
    }
}

/// The result of [`truncate_to_visual_lines`]: the visible (last-N) wrapped lines + how many were
/// hidden above them (`VisualTruncateResult`, `visual-truncate.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualTruncate {
    /// The visual lines to display (≤ `max`).
    pub lines: Vec<String>,
    /// How many wrapped lines were skipped off the top.
    pub skipped: usize,
}

/// Truncate `text` to at most `max` visual lines counted **from the end**, wrapping each logical line
/// to `width` (`truncateToVisualLines`, `visual-truncate.ts:30-53`). Returns the visible tail and the
/// number of hidden lines. `width == 0` is treated as `1`.
///
/// Upstream this function owns no wrapping of its own — it is literally
/// ```text
/// const tempText = new Text(text, paddingX, 0);
/// const allVisualLines = tempText.render(width);
/// ```
/// (`visual-truncate.ts:37-38`), i.e. `wrapTextWithAnsi(text, width - paddingX * 2)`
/// (`text.ts:64`, `:67`). cyrup's callers pass the already-reduced content width and add the margin
/// themselves, so `width` here is upstream's `width - paddingX * 2`; the WRAP is the shared
/// [`crate::transcript::wrap_line`] — the same `wrapSingleLine` port `Text`, `Box` and
/// [`crate::markdown`] use.
///
/// It used to be a `chars()`-indexed hard chunker: every logical line was sliced into fixed
/// `width`-*char* pieces, so it broke mid-word (`… output tha` / `t certainly …`), miscounted every
/// CJK ideograph, emoji and box-drawing glyph as one column, and could split a ZWJ sequence or
/// detach a combining mark. That is the same char-vs-grapheme defect already fixed in `wrap_line`,
/// `wrap_cell` and `word_wrap_line`, and — because the hidden-line count is derived from the row
/// count — it also reported the wrong `... N more lines`.
pub fn truncate_to_visual_lines(text: &str, max: usize, width: usize) -> VisualTruncate {
    if text.is_empty() {
        return VisualTruncate {
            lines: Vec::new(),
            skipped: 0,
        };
    }
    let width = width.max(1);
    let mut visual: Vec<String> = Vec::new();
    // `wrapTextWithAnsi` splits on `/\r\n|\r|\n/` first (`utils.ts:839`) and wraps each piece,
    // returning `[""]` for an empty one (`:858-860`) so a blank output line keeps its row.
    for logical in text.split('\n') {
        for row in crate::transcript::wrap_line(&Line::from(Span::raw(logical.to_string())), width)
        {
            visual.push(row.spans.iter().map(|s| s.content.as_ref()).collect());
        }
    }
    if visual.len() <= max {
        return VisualTruncate {
            lines: visual,
            skipped: 0,
        };
    }
    let skipped = visual.len() - max;
    let lines = visual.split_off(skipped);
    VisualTruncate { lines, skipped }
}

/// A `DynamicBorder`-delimited spinner+message block (`bordered-loader.ts` `BorderedLoader`): a top
/// rule, the spinner glyph + accent message, an optional cancel hint, and a bottom rule. Used by the
/// extension-UI loader and any long inline op. Immediate-mode: the spinner frame is chosen from the
/// elapsed-tick index just like [`crate::status_indicator`].
pub struct BorderedLoader {
    message: String,
    cancellable: bool,
    /// The cancel-key label for the hint (live keymap), shown only when `cancellable`.
    ///
    /// Upstream this is `keyHint("tui.select.cancel", "cancel")` (`bordered-loader.ts:36`), so it
    /// must come from [`crate::keymap::SelectKeymap::keys_label`]`(SelectAction::Cancel)` — **all**
    /// bound keys joined with `/` (`keybinding-hints.ts:29-36`), stock `escape/ctrl+c` — never the
    /// first key of a different action.
    cancel_key: Option<String>,
}

impl BorderedLoader {
    /// A cancellable loader with `message` and the `tui.select.cancel` key label for its hint.
    pub fn cancellable(message: impl Into<String>, cancel_key: impl Into<String>) -> Self {
        BorderedLoader {
            message: message.into(),
            cancellable: true,
            cancel_key: Some(cancel_key.into()),
        }
    }

    /// A non-cancellable loader (no hint row).
    pub fn plain(message: impl Into<String>) -> Self {
        BorderedLoader {
            message: message.into(),
            cancellable: false,
            cancel_key: None,
        }
    }

    /// The number of rows this loader occupies: **7** cancellable, **5** plain.
    ///
    /// `BorderedLoader`'s children (`bordered-loader.ts:16-39`) are `DynamicBorder` (1 row),
    /// `Loader` (**2** rows — `render` returns `["", ...super.render(width)]`, `loader.ts:43-45`),
    /// then when cancellable `Spacer(1)` + `Text(keyHint, 1, 0)` (`:35-36`), then `Spacer(1)` (`:38`)
    /// and the closing `DynamicBorder` (`:39`). cyrup drew 4/3 — missing the loader's own leading
    /// blank and both `Spacer(1)` rows, so the spinner sat against the top rule and the hint against
    /// the bottom one.
    pub fn height(&self) -> u16 {
        if self.cancellable { 7 } else { 5 }
    }

    /// Render the loader into `area`, selecting the spinner frame from `tick` (the 80 ms phase index).
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &UiTheme, tick: usize) {
        let hint_h = u16::from(self.cancellable);
        // top rule / loader blank / loader body / spacer / hint / spacer / bottom rule.
        let [top, lead, body, gap, hint, tail, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(hint_h),
            Constraint::Length(hint_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let _ = (lead, gap, tail); // `Spacer(1)` rows: deliberately blank (`spacer.ts:21-27`).
        frame.render_widget(border_rule(top.width, theme), top);
        let spin = SPINNER_FRAMES
            .get(tick % SPINNER_FRAMES.len())
            .copied()
            .unwrap_or("⠋");
        let body_line = Line::from(vec![
            // `spinnerColorFn = (s) => theme.fg("accent", s)` but
            // `messageColorFn = (s) => theme.fg("muted", s)` (`bordered-loader.ts:20-21`, `:28-29`)
            // — in BOTH the cancellable and the plain branch. cyrup painted the message accent too,
            // so `Creating gist...` came out bright teal, and disagreed with the status band
            // (`status_indicator.rs`), which already had it right.
            Span::styled(format!(" {spin} "), theme.accent_style()),
            Span::styled(self.message.clone(), theme.muted_style()),
            Span::styled(" ", theme.muted_style()),
        ]);
        frame.render_widget(Paragraph::new(body_line).style(theme.base_style()), body);
        if self.cancellable {
            let key = self
                .cancel_key
                .clone()
                .unwrap_or_else(|| "escape/ctrl+c".into());
            let hint_line = Line::from({
                let mut spans = vec![Span::raw(" ")];
                spans.extend(key_hint_spans(&key, "cancel", theme));
                spans
            });
            frame.render_widget(Paragraph::new(hint_line).style(theme.base_style()), hint);
        }
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }
}
