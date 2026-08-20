//! The adapter's **interactive surface**: the `/mcp` browser panel, the `/mcp-auth` picker it
//! doubles as, and the `/mcp setup` onboarding panel — ports of `mcp-panel.ts` (1,015 lines),
//! `panel-keys.ts` (53) and `mcp-setup-panel.ts` (667), plus the footer-status text `init.ts`'s
//! `updateStatusBar` writes and the shared-config notice `commands.ts` builds (gap-analysis 13h,
//! MCP-351..MCP-380, MCP-391..MCP-394).
//!
//! # Where this lands
//!
//! Upstream both panels are pi `Component`s — `render(width): string[]` plus
//! `handleInput(data: string)` — handed to
//! `ctx.ui.custom(factory, { overlay: true, overlayOptions: { anchor: "center", width: 82 | 92 } })`.
//! cyrup's counterpart is [`cyrup_ext::InteractiveOverlay`] opened with
//! `HostServices::open_overlay`, painted by `cyrup-tui`'s `ExtensionOverlay`. Two shipped
//! precedents are followed rather than re-derived:
//!
//! * `FleetOverlay` (`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`) — the async-job shape:
//!   a captured `tokio::runtime::Handle`, one `oneshot::Receiver` per in-flight job, `try_recv` in
//!   `tick`, and the `TryRecvError::Closed` arm that **must still clear the busy latch**.
//! * `PermissionSystemSettingsOverlay` (`crates/cyrup-permission-system/src/config_modal.rs`) —
//!   the result-escape shape: `open_overlay` consumes the box and returns `bool`, so the outcome is
//!   read off an `Arc`-shared cell the overlay writes through (MCP-369).
//!
//! # Two mechanism differences, recorded rather than absorbed (MCP-350)
//!
//! 1. **Poll-repaint replaces push-repaint.** Upstream calls `this.tui.requestRender()` from inside
//!    a settled promise. There is no push channel from an extension to the host, so every unit that
//!    would have called `requestRender` mutates state and lets the next
//!    [`InteractiveOverlay::tick`] pick it up. [`REFRESH_MS`] is 250 ms — the smallest cadence that
//!    does not busy-render — so an async settle can be up to one tick stale.
//! 2. **The 60 s inactivity `setTimeout` becomes a deadline compared in `tick`** (MCP-362), so the
//!    panel lives up to one cadence longer than upstream's.
//!
//! # What is *not* in scope here, and why the carve-out is narrow
//!
//! The host paints a `Clear`ed rect and a `Paragraph` from the [`OverlayLine`]s returned here, so
//! what genuinely belongs to the host is the cell painting. Upstream's `fg()` helper and its raw
//! SGR literals are transcribed into [`OverlaySpan`] fields — *which* slot is dim and which is cyan
//! is what the user sees — and the box glyphs are emitted here too, because `row()`'s framing is
//! what makes the width arithmetic visible. Every layout decision (`innerW`, `contentW`,
//! `previewW`, the pad flag, the 12-row window, the hint-bar wrap) computes *what fits*, and a
//! panel that drops them renders different content, not merely different pixels.
//!
//! # `ratatui` is used for measurement only
//!
//! [`InteractiveOverlay::render`] returns `Vec<OverlayLine>`, so a `ratatui::text::Line` cannot
//! cross this seam. `ratatui` is still the right dependency for the two things pi's `visibleWidth`
//! and `truncateToWidth` need and `std` does not have: grapheme-cluster segmentation
//! (`Span::styled_graphemes`) and East-Asian display width (`Span::width`), both of which `ratatui`
//! already vendors. That keeps `unicode-width`/`unicode-segmentation` out of this crate's manifest
//! and keeps `cyrup-tui` out of its dependency graph, which is the crate-boundary rule
//! `cyrup-ext-subagents` also follows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use ratatui::style::Style as RtStyle;
use ratatui::text::Span as RtSpan;

use cyrup_ext::{
    InteractiveOverlay, OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome,
    OverlaySpan,
};

use crate::config::{
    ConfigWritePreview, ImportKind, KnownServerPreset, McpConfig, McpDiscoverySummary, ServerEntry,
    ServerProvenance, SourceId, SourceKind, ToolPrefix,
};
use crate::dirs::{
    compute_server_hash, CachedTool, MetadataCache, ResolvedIdentity, ServerCacheEntry,
    CACHE_MAX_AGE_MS,
};
use crate::onboarding::OnboardingState;
use crate::registration::{
    matches_tool_pattern, resolve_tool_prefix, resource_base_tool_name, tool_name_candidates,
};

// =================================================================================================
// 0 · Constants
// =================================================================================================

/// `McpPanel.MAX_VISIBLE` (`mcp-panel.ts`) — the body window, in rows.
pub const MAX_VISIBLE: usize = 12;

/// `INACTIVITY_MS` — both panels, 60 000 ms, re-armed on every keystroke (MCP-362).
pub const INACTIVITY_MS: u64 = 60_000;

/// The poll cadence both overlays declare. Upstream arms no repaint timer at all; cyrup has only
/// `tick`, and `refresh_ms() == 0` means "never tick me", which would strand every async settle and
/// disable the inactivity auto-cancel outright. 250 ms is the smallest cadence that does not
/// busy-render (`FleetOverlay` carries 750 ms only because pi's own component declares one).
pub const REFRESH_MS: u64 = 250;

/// `mcp-setup-panel.ts` `MIN_PANEL_WIDTH`.
pub const MIN_PANEL_WIDTH: usize = 24;
/// `mcp-setup-panel.ts` `COMPACT_WIDTH` — an `inner_w` below this takes the compact branch.
pub const COMPACT_WIDTH: usize = 60;
/// `mcp-setup-panel.ts` `COMPACT_ACTION_ROWS`.
pub const COMPACT_ACTION_ROWS: usize = 7;
/// `mcp-setup-panel.ts` `DESKTOP_PREVIEW_WIDTH`.
pub const DESKTOP_PREVIEW_WIDTH: usize = 74;

/// `formatWritePreview`'s diff cap.
const PREVIEW_MAX_DIFF_LINES: usize = 18;

// =================================================================================================
// 1 · Text primitives — `visibleWidth`, `truncateToWidth`, the two `wrapText`s, the sanitizers
// =================================================================================================

/// Grapheme clusters of `text`, in order.
///
/// `Span::styled_graphemes` is `ratatui`'s vendored `unicode-segmentation` walk. It filters out any
/// cluster containing a control character, which is harmless here — every string that reaches a row
/// has been through [`sanitize_terminal_text`] or [`sanitize_row_content`] first, and a control byte
/// surviving into a measured string is precisely what those two exist to prevent.
fn graphemes(text: &str) -> Vec<String> {
    let span = RtSpan::raw(text);
    span.styled_graphemes(RtStyle::default()).map(|g| g.symbol.to_string()).collect()
}

/// pi `visibleWidth(text)` (`pi/packages/tui/src/utils.ts` @v0.84.1) for escape-free input.
///
/// Styling crosses this seam out-of-band in [`OverlaySpan`] fields, so there are no escapes to
/// exclude; what remains is East-Asian width over grapheme clusters, which is exactly
/// `Span::width`. One measured divergence from pi is carried deliberately: pi normalises a tab to
/// **three** spaces before measuring. Unreachable from these call sites — every string reaching a
/// row has had its C0 controls collapsed to single spaces — and reproducing it here would make the
/// helper disagree with itself for callers that have not sanitized.
#[must_use]
pub fn visible_width(text: &str) -> usize {
    RtSpan::raw(text).width()
}

/// pi `truncateToWidth(text, maxWidth, ellipsis = "...", pad = false)`.
///
/// The `pad` flag is not decoration: it right-pads to **exactly** `max_width`, including the
/// empty-text arm returning `max_width` spaces, and it is the only thing keeping the MCP panel's
/// right border aligned (MCP-366).
#[must_use]
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.is_empty() {
        return if pad { " ".repeat(max_width) } else { String::new() };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        // pi's degenerate arm: no room for both, so either the text fits whole or a clipped
        // ellipsis stands in for the whole row.
        let text_width = visible_width(text);
        if text_width <= max_width {
            return pad_to(text, text_width, max_width, pad);
        }
        let (clipped, clipped_width) = take_prefix(ellipsis, max_width);
        if clipped_width == 0 {
            return if pad { " ".repeat(max_width) } else { String::new() };
        }
        return pad_to(&clipped, clipped_width, max_width, pad);
    }

    let target = max_width - ellipsis_width;
    let mut result = String::new();
    let mut kept_width = 0usize;
    let mut visible_so_far = 0usize;
    let mut keep_prefix = true;
    let mut overflowed = false;
    for cluster in graphemes(text) {
        let width = visible_width(&cluster);
        if keep_prefix && kept_width + width <= target {
            result.push_str(&cluster);
            kept_width += width;
        } else {
            keep_prefix = false;
        }
        visible_so_far += width;
        if visible_so_far > max_width {
            overflowed = true;
            break;
        }
    }
    if !overflowed {
        return pad_to(text, visible_so_far, max_width, pad);
    }
    result.push_str(ellipsis);
    pad_to(&result, kept_width + ellipsis_width, max_width, pad)
}

/// The longest grapheme prefix of `text` fitting in `max_width`, with its width.
fn take_prefix(text: &str, max_width: usize) -> (String, usize) {
    let mut out = String::new();
    let mut width = 0usize;
    for cluster in graphemes(text) {
        let w = visible_width(&cluster);
        if width + w > max_width {
            break;
        }
        out.push_str(&cluster);
        width += w;
    }
    (out, width)
}

fn pad_to(text: &str, text_width: usize, max_width: usize, pad: bool) -> String {
    if !pad {
        return text.to_string();
    }
    let mut out = text.to_string();
    out.push_str(&" ".repeat(max_width.saturating_sub(text_width)));
    out
}

/// `mcp-panel.ts`'s `wrapText(text, width)` — greedy wrap with `max = max(8, width)` that **hard
/// splits** a word wider than `max`, and falls back to `[text]` when it produced nothing.
///
/// This is a *different function* from [`wrap_text_soft`] and both must exist: the setup panel's
/// variant deliberately does not hard-split, which is why a long JSON diff line there shows as a
/// single ellipsis rather than wrapping (MCP-367, MCP-376).
#[must_use]
pub fn wrap_text_hard(text: &str, width: usize) -> Vec<String> {
    let max = width.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    // `splitLongWord`: emit whole chunks and carry the remainder, with a one-cluster forced take so
    // a zero-width cluster cannot spin forever.
    fn split_long_word(word: &str, max: usize, lines: &mut Vec<String>) -> String {
        let mut rest = word.to_string();
        while visible_width(&rest) > max {
            let (mut take, _) = take_prefix(&rest, max);
            if take.is_empty() {
                take = graphemes(&rest).first().cloned().unwrap_or_default();
                if take.is_empty() {
                    break;
                }
            }
            let consumed = take.len();
            lines.push(take);
            rest = rest.get(consumed..).unwrap_or("").to_string();
        }
        rest
    }

    for word in text.split_whitespace() {
        let candidate =
            if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
        if visible_width(&candidate) <= max {
            current = candidate;
        } else {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            current = split_long_word(word, max, &mut lines);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

/// `mcp-setup-panel.ts`'s `wrapText(text, width)` — returns `[text]` unchanged when `width <= 8`,
/// greedy-wraps otherwise, **never hard-splits**, and returns one empty line when it produced
/// nothing.
#[must_use]
pub fn wrap_text_soft(text: &str, width: usize) -> Vec<String> {
    if width <= 8 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate =
            if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
        if visible_width(&candidate) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        current = word.to_string();
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// `utils.ts` `stripOscSequences` (MCP-364).
///
/// Removes `ESC ]` and C1 OSC (U+009D) introducers **and their payloads even when unterminated**:
/// the scanner looks for BEL (U+0007), ST (U+009C) or `ESC` + backslash and, finding none, consumes
/// to the end of the string. `__tests__/mcp-panel-rendering.test.ts` asserts exactly that — an
/// input ending in an unterminated OSC 8 hyperlink must not leak its URL. A regex cannot express
/// the "or to end of input" arm cleanly, which is why this is a hand-written scanner.
#[must_use]
pub fn strip_osc_sequences(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars.get(index).copied().unwrap_or('\0');
        let is_esc_osc = ch == '\u{1b}' && chars.get(index + 1) == Some(&']');
        let is_c1_osc = ch == '\u{9d}';
        if !is_esc_osc && !is_c1_osc {
            out.push(ch);
            index += 1;
            continue;
        }
        index += if is_esc_osc { 2 } else { 1 };
        while index < chars.len() {
            let code = chars.get(index).copied().unwrap_or('\0');
            index += 1;
            if code == '\u{7}' || code == '\u{9c}' {
                break;
            }
            if code == '\u{1b}' && chars.get(index) == Some(&'\\') {
                index += 1;
                break;
            }
        }
    }
    out
}

/// Is `ch` a C0 control, DEL, or a C1 control? — upstream's U+0000..U+001F / U+007F..U+009F class.
fn is_terminal_control(ch: char) -> bool {
    ch <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&ch)
}

/// The length of the CSI or two-byte escape starting at `at`, in `char`s, or `None`.
///
/// Upstream's pattern is `ESC [` + params U+0030..U+003F + intermediates U+0020..U+002F + a final
/// byte U+0040..U+007E, or `ESC` + one byte in U+0040..U+005A / U+005C..U+005F.
fn ansi_escape_len(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at) != Some(&'\u{1b}') {
        return None;
    }
    if chars.get(at + 1) == Some(&'[') {
        let mut i = at + 2;
        while matches!(chars.get(i), Some(c) if ('\u{30}'..='\u{3f}').contains(c)) {
            i += 1;
        }
        while matches!(chars.get(i), Some(c) if ('\u{20}'..='\u{2f}').contains(c)) {
            i += 1;
        }
        return match chars.get(i) {
            Some(c) if ('\u{40}'..='\u{7e}').contains(c) => Some(i + 1 - at),
            _ => None,
        };
    }
    match chars.get(at + 1) {
        Some(c) if ('\u{40}'..='\u{5a}').contains(c) || ('\u{5c}'..='\u{5f}').contains(c) => Some(2),
        _ => None,
    }
}

/// `utils.ts` `sanitizeTerminalText` (MCP-364): strip OSC, strip every CSI and two-byte escape,
/// replace runs of C0/DEL/C1 with **one** space, collapse whitespace runs to one space, trim.
///
/// Applied to server names, import kinds, tool names, descriptions, notice lines, failure messages
/// and every interpolated error string — everything a hostile MCP server can name.
#[must_use]
pub fn sanitize_terminal_text(text: &str) -> String {
    let stripped = strip_osc_sequences(text);
    let chars: Vec<char> = stripped.chars().collect();

    // Pass 1: drop escapes, collapse control runs to a single space.
    let mut no_escapes = String::with_capacity(stripped.len());
    let mut index = 0usize;
    let mut in_control_run = false;
    while index < chars.len() {
        if let Some(len) = ansi_escape_len(&chars, index) {
            index += len;
            continue;
        }
        let ch = chars.get(index).copied().unwrap_or('\0');
        index += 1;
        if is_terminal_control(ch) {
            if !in_control_run {
                no_escapes.push(' ');
                in_control_run = true;
            }
            continue;
        }
        in_control_run = false;
        no_escapes.push(ch);
    }

    // Pass 2: a whitespace run collapses to one space, then trim. JS's whitespace class is Unicode
    // whitespace plus U+FEFF; the difference is not reachable after pass 1 folded the control range.
    let mut collapsed = String::with_capacity(no_escapes.len());
    let mut in_space_run = false;
    for ch in no_escapes.chars() {
        if ch.is_whitespace() {
            if !in_space_run {
                collapsed.push(' ');
                in_space_run = true;
            }
            continue;
        }
        in_space_run = false;
        collapsed.push(ch);
    }
    collapsed.trim().to_string()
}

/// `mcp-panel.ts` `sanitizeDisplayText(text)` = `sanitizeTerminalText(text ?? "")`.
#[must_use]
pub fn sanitize_display_text(text: Option<&str>) -> String {
    sanitize_terminal_text(text.unwrap_or(""))
}

/// `mcp-panel.ts` `sanitizeRowContent(content)`, applied to the **assembled** row (MCP-364).
///
/// A different function from [`sanitize_terminal_text`]: it runs after the panel's own styling has
/// been added, so upstream copies a matched escape through verbatim while dropping every control
/// character — the next non-control emits one space first *if* the result so far is non-empty and
/// does not already end in a space. Here styling is out-of-band in the spans, so there is no escape
/// to copy through; what survives is the control-dropping walk **and its `pendingSpace` flag
/// threaded across the whole row**, which is what guarantees every emitted row is a single line
/// with no control bytes — the whole-frame property the rendering test asserts.
#[must_use]
pub fn sanitize_row_content(spans: &[OverlaySpan]) -> Vec<OverlaySpan> {
    let mut out: Vec<OverlaySpan> = Vec::with_capacity(spans.len());
    let mut pending_space = false;
    let mut any_emitted = false;
    let mut last_was_space = false;
    for span in spans {
        let stripped = strip_osc_sequences(&span.text);
        let chars: Vec<char> = stripped.chars().collect();
        let mut text = String::with_capacity(stripped.len());
        let mut index = 0usize;
        while index < chars.len() {
            if let Some(len) = ansi_escape_len(&chars, index) {
                // Upstream copies the escape through; a span carries no escapes to copy, so the
                // only faithful thing left is to not let it count as content.
                index += len;
                continue;
            }
            let ch = chars.get(index).copied().unwrap_or('\0');
            index += 1;
            if is_terminal_control(ch) {
                pending_space = true;
                continue;
            }
            if pending_space && any_emitted && !last_was_space {
                text.push(' ');
            }
            pending_space = false;
            text.push(ch);
            any_emitted = true;
            last_was_space = ch == ' ';
        }
        if !text.is_empty() {
            out.push(OverlaySpan { text, ..span.clone() });
        }
    }
    out
}

/// `mcp-panel.ts` `fuzzyScore(query, text)` (MCP-354).
///
/// A substring hit scores 100–150; a subsequence scores by run length; a non-subsequence scores 0.
/// Only `> 0` / `== 0` and the one `* 0.6` relative comparison are consumed, but the weighting is
/// meaningless against a different curve — `cyrup-tui`'s autocomplete scorer is a **different**
/// algorithm and must not be substituted. Indexing is by `char`, which agrees with JS's UTF-16 code
/// units for every character below U+10000; the `to_lowercase()`/`toLowerCase()` divergence for
/// locale-sensitive characters (Turkish dotless i) is accepted and recorded.
#[must_use]
pub fn fuzzy_score(query: &str, text: &str) -> f64 {
    let lq_s = query.to_lowercase();
    let lt_s = text.to_lowercase();
    let lq: Vec<char> = lq_s.chars().collect();
    let lt: Vec<char> = lt_s.chars().collect();
    if lt_s.contains(&lq_s) {
        let ratio = if lt.is_empty() { 0.0 } else { lq.len() as f64 / lt.len() as f64 };
        return 100.0 + ratio * 50.0;
    }
    let mut score = 0f64;
    let mut qi = 0usize;
    let mut consecutive = 0f64;
    for ch in &lt {
        if qi >= lq.len() {
            break;
        }
        if lq.get(qi) == Some(ch) {
            score += 10.0 + consecutive;
            consecutive += 5.0;
            qi += 1;
        } else {
            consecutive = 0.0;
        }
    }
    if qi == lq.len() {
        score
    } else {
        0.0
    }
}

/// JS `String.prototype.length` — UTF-16 code units, not bytes and not `char`s.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `mcp-panel.ts` `estimateTokens(tool)` (MCP-365):
/// `ceil((name.length + description.length + JSON.stringify(inputSchema ?? {}).length) / 4) + 10`.
///
/// The empty-schema case stringifies to two characters, so the floor for a bare name is
/// `ceil((n + 2) / 4) + 10`. A resource tool is deliberately fed a `CachedTool` carrying only
/// `{name, description?}` — **no** `inputSchema` — so its estimate is systematically smaller than a
/// real tool's.
///
/// Key order does not reach the answer: only the *length* of the stringified schema is consumed and
/// a permutation of the same keys has the same length, so this workspace's `BTreeMap`-backed
/// `serde_json::Map` costs nothing here. What does differ is number rendering — a JSON round trip
/// through JS collapses `1.0` to `1` where `serde_json` keeps `1.0` — which shifts an estimate by
/// one character per such literal. Recorded; JSON Schema's numeric keywords are integers in
/// practice.
#[must_use]
pub fn estimate_tokens(
    name: &str,
    description: Option<&str>,
    input_schema: Option<&serde_json::Value>,
) -> usize {
    let schema = match input_schema {
        Some(value) => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),
        None => "{}".to_string(),
    };
    let total = utf16_len(name) + description.map_or(0, utf16_len) + utf16_len(&schema);
    total.div_ceil(4) + 10
}

/// `Number.prototype.toLocaleString()` under Node's default locale: thousands-grouped with a comma.
///
/// Grouped explicitly rather than through a locale crate — this reproduces a Node default, not a
/// user's locale, and reaching for ICU here would make the panel's numbers depend on the ambient
/// locale environment.
#[must_use]
pub fn to_locale_string(value: usize) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(*byte));
    }
    out
}

// =================================================================================================
// 2 · Styling — `DEFAULT_THEME`'s SGR bodies as `OverlaySpan` fields, and the row builder
// =================================================================================================

/// One slot of a panel theme, as the [`OverlaySpan`] fields upstream's SGR body maps onto.
///
/// The colours are **not** substrate-excluded: which slot is dim and which is cyan is what the user
/// sees, so `mcp-panel.ts`'s `DEFAULT_THEME` is transcribed rather than reinterpreted —
/// `border`/`title`/`description`/`hint` are all SGR `2` (so the title is *not* visually distinct
/// from the border), `selected` is `36`, `direct` `32`, `needsAuth` `33`, `placeholder` `2;3`,
/// `confirm` `32`, `cancel` `31`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// Foreground colour, or the terminal default.
    pub fg: Option<OverlayColor>,
    /// SGR 1.
    pub bold: bool,
    /// SGR 2.
    pub dim: bool,
    /// SGR 3.
    pub italic: bool,
    /// SGR 7.
    pub reversed: bool,
}

impl Style {
    /// No styling at all.
    #[must_use]
    pub const fn plain() -> Self {
        Self { fg: None, bold: false, dim: false, italic: false, reversed: false }
    }
    /// A foreground colour alone.
    #[must_use]
    pub const fn fg(color: OverlayColor) -> Self {
        Self { fg: Some(color), bold: false, dim: false, italic: false, reversed: false }
    }
    /// SGR 2.
    #[must_use]
    pub const fn dim() -> Self {
        Self { fg: None, bold: false, dim: true, italic: false, reversed: false }
    }
    /// `bold(s)` — SGR 1/22.
    #[must_use]
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    /// `italic(s)` — SGR 3/23.
    #[must_use]
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    /// `inverse(s)` — SGR 7/27.
    #[must_use]
    pub const fn inverse(mut self) -> Self {
        self.reversed = true;
        self
    }

    fn span(self, text: impl Into<String>) -> OverlaySpan {
        OverlaySpan {
            text: text.into(),
            fg: self.fg,
            bg: None,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underlined: false,
            reversed: self.reversed,
        }
    }
}

/// `mcp-panel.ts` `DEFAULT_THEME`, transcribed slot for slot.
#[derive(Clone, Copy, Debug)]
pub struct PanelTheme {
    /// SGR `2`.
    pub border: Style,
    /// SGR `2` — deliberately the same as [`Self::border`].
    pub title: Style,
    /// SGR `36`.
    pub selected: Style,
    /// SGR `32`.
    pub direct: Style,
    /// SGR `33`.
    pub needs_auth: Style,
    /// SGR `2;3`.
    pub placeholder: Style,
    /// SGR `2`.
    pub description: Style,
    /// SGR `2`.
    pub hint: Style,
    /// SGR `32`.
    pub confirm: Style,
    /// SGR `31`.
    pub cancel: Style,
}

impl Default for PanelTheme {
    fn default() -> Self {
        Self {
            border: Style::dim(),
            title: Style::dim(),
            selected: Style::fg(OverlayColor::Cyan),
            direct: Style::fg(OverlayColor::Green),
            needs_auth: Style::fg(OverlayColor::Yellow),
            placeholder: Style::dim().italic(),
            description: Style::dim(),
            hint: Style::dim(),
            confirm: Style::fg(OverlayColor::Green),
            cancel: Style::fg(OverlayColor::Red),
        }
    }
}

/// `mcp-setup-panel.ts` `DEFAULT_THEME` — a different table from [`PanelTheme`]: its `title` is
/// cyan (`36`) and its `selected` is green (`32`), the reverse of the browser panel's pairing.
#[derive(Clone, Copy, Debug)]
pub struct SetupTheme {
    /// SGR `2`.
    pub border: Style,
    /// SGR `36`.
    pub title: Style,
    /// SGR `32`.
    pub selected: Style,
    /// SGR `2`.
    pub hint: Style,
    /// SGR `32`.
    pub success: Style,
    /// SGR `33`.
    pub warning: Style,
    /// SGR `2;3`.
    pub muted: Style,
}

impl Default for SetupTheme {
    fn default() -> Self {
        Self {
            border: Style::dim(),
            title: Style::fg(OverlayColor::Cyan),
            selected: Style::fg(OverlayColor::Green),
            hint: Style::dim(),
            success: Style::fg(OverlayColor::Green),
            warning: Style::fg(OverlayColor::Yellow),
            muted: Style::dim().italic(),
        }
    }
}

/// `RAINBOW_COLORS` — seven truecolor SGR bodies, as [`OverlayColor::Rgb`] triples.
pub const RAINBOW_COLORS: [OverlayColor; 7] = [
    OverlayColor::Rgb(178, 129, 214),
    OverlayColor::Rgb(215, 135, 175),
    OverlayColor::Rgb(254, 188, 56),
    OverlayColor::Rgb(228, 192, 15),
    OverlayColor::Rgb(137, 210, 129),
    OverlayColor::Rgb(0, 175, 175),
    OverlayColor::Rgb(23, 143, 185),
];

/// A row under construction: the styled runs upstream builds by concatenating `fg()` results.
///
/// Keeping the runs apart rather than flattening to a string is what lets [`truncate_spans`]
/// reproduce pi's `truncateToWidth` (which measures with escapes excluded) without ever parsing an
/// escape back out of the text.
#[derive(Clone, Debug, Default)]
pub struct StyledText {
    /// The runs, left to right.
    pub spans: Vec<OverlaySpan>,
}

impl StyledText {
    /// An empty row.
    #[must_use]
    pub fn new() -> Self {
        Self { spans: Vec::new() }
    }

    /// Append one styled run. Empty text is dropped, which keeps the span list the same shape a
    /// string concatenation would have produced.
    pub fn push(&mut self, style: Style, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        if !text.is_empty() {
            self.spans.push(style.span(text));
        }
        self
    }

    /// Append an unstyled run.
    pub fn raw(&mut self, text: impl Into<String>) -> &mut Self {
        self.push(Style::plain(), text)
    }

    /// Append every run of `other`.
    pub fn extend(&mut self, other: StyledText) -> &mut Self {
        self.spans.extend(other.spans);
        self
    }

    /// The row's plain text — what upstream's `visibleWidth` would have measured.
    #[must_use]
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The row's display width.
    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| visible_width(&s.text)).sum()
    }

    /// A one-run row.
    #[must_use]
    pub fn styled(style: Style, text: impl Into<String>) -> Self {
        let mut out = Self::new();
        out.push(style, text);
        out
    }
}

/// pi `truncateToWidth(text, maxWidth, ellipsis, pad)` over styled runs.
///
/// Identical arithmetic to [`truncate_to_width`], but the kept prefix keeps each cluster's own
/// style; the ellipsis inherits the style of the run that overflowed, which is what upstream's
/// escape buffering produces (the last flushed SGR is still in force when the ellipsis is written).
#[must_use]
pub fn truncate_spans(
    spans: &[OverlaySpan],
    max_width: usize,
    ellipsis: &str,
    pad: bool,
) -> Vec<OverlaySpan> {
    if max_width == 0 {
        return Vec::new();
    }
    let total_width: usize = spans.iter().map(|s| visible_width(&s.text)).sum();
    let is_empty = spans.iter().all(|s| s.text.is_empty());
    if is_empty {
        return if pad { vec![OverlaySpan::raw(" ".repeat(max_width))] } else { Vec::new() };
    }

    let ellipsis_width = visible_width(ellipsis);
    if total_width <= max_width {
        let mut out = spans.to_vec();
        if pad && total_width < max_width {
            out.push(OverlaySpan::raw(" ".repeat(max_width - total_width)));
        }
        return out;
    }
    let target = max_width.saturating_sub(ellipsis_width);

    let mut out: Vec<OverlaySpan> = Vec::with_capacity(spans.len() + 2);
    let mut kept_width = 0usize;
    let mut last_style: Option<OverlaySpan> = None;
    'outer: for span in spans {
        let mut kept = String::new();
        for cluster in graphemes(&span.text) {
            let w = visible_width(&cluster);
            if kept_width + w > target {
                if !kept.is_empty() {
                    out.push(OverlaySpan { text: kept, ..span.clone() });
                }
                last_style = Some(span.clone());
                break 'outer;
            }
            kept.push_str(&cluster);
            kept_width += w;
        }
        if !kept.is_empty() {
            out.push(OverlaySpan { text: kept, ..span.clone() });
        }
        last_style = Some(span.clone());
    }

    if ellipsis_width > 0 && ellipsis_width <= max_width {
        let style = last_style.unwrap_or_default();
        out.push(OverlaySpan { text: ellipsis.to_string(), ..style });
        kept_width += ellipsis_width;
    }
    if pad && kept_width < max_width {
        out.push(OverlaySpan::raw(" ".repeat(max_width - kept_width)));
    }
    out
}

// =================================================================================================
// 3 · `panel-keys.ts` — the three canonical ids plus the adapter-defined `mcp.panel.save`
// =================================================================================================

/// One `KeyId` spec, parsed from the same `"ctrl+p"` / `"up"` / `"return"` grammar pi's `matchesKey`
/// accepts.
///
/// Reimplemented here rather than imported: `cyrup-mcp` must not depend on `cyrup-tui`, whose
/// `Key::parse` this mirrors token for token (`crates/cyrup-tui/src/keymap.rs`). Every token pi
/// spells is accepted, including the `enter`/`return` and `esc`/`escape` aliases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    /// The key itself.
    pub code: OverlayKeyCode,
    /// Control held.
    pub ctrl: bool,
    /// Alt / Meta held.
    pub alt: bool,
    /// Shift explicitly requested by the spec.
    pub shift: bool,
}

impl KeySpec {
    /// Parse one `KeyId`. Returns `None` for a spec naming no key, which upstream's `matchesKey`
    /// treats as never matching.
    #[must_use]
    pub fn parse(spec: &str) -> Option<KeySpec> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut code: Option<OverlayKeyCode> = None;
        for part in spec.split('+') {
            let token = part.trim();
            if token.is_empty() {
                continue;
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" | "meta" => alt = true,
                // `super`/`cmd` has no `OverlayKey` bit; a spec naming it can never match, which is
                // the same outcome as pi's on a terminal that does not report it.
                "super" | "cmd" | "command" => return None,
                "enter" | "return" => code = Some(OverlayKeyCode::Enter),
                "tab" => code = Some(OverlayKeyCode::Tab),
                "backtab" => code = Some(OverlayKeyCode::BackTab),
                "esc" | "escape" => code = Some(OverlayKeyCode::Escape),
                "space" => code = Some(OverlayKeyCode::Char(' ')),
                "up" => code = Some(OverlayKeyCode::Up),
                "down" => code = Some(OverlayKeyCode::Down),
                "left" => code = Some(OverlayKeyCode::Left),
                "right" => code = Some(OverlayKeyCode::Right),
                "home" => code = Some(OverlayKeyCode::Home),
                "end" => code = Some(OverlayKeyCode::End),
                "backspace" => code = Some(OverlayKeyCode::Backspace),
                "delete" | "del" => code = Some(OverlayKeyCode::Delete),
                "pageup" | "pgup" => code = Some(OverlayKeyCode::PageUp),
                "pagedown" | "pgdn" => code = Some(OverlayKeyCode::PageDown),
                "insert" | "ins" => code = Some(OverlayKeyCode::Insert),
                other
                    if other.len() >= 2
                        && other.starts_with('f')
                        && other.get(1..).is_some_and(|d| {
                            !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
                        }) =>
                {
                    match other.get(1..).and_then(|d| d.parse::<u8>().ok()) {
                        Some(n @ 1..=12) => code = Some(OverlayKeyCode::F(n)),
                        _ => return None,
                    }
                }
                other => {
                    let mut chars = other.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => code = Some(OverlayKeyCode::Char(c)),
                        _ => return None,
                    }
                }
            }
        }
        code.map(|code| KeySpec { code, ctrl, alt, shift })
    }

    /// Does `key` satisfy this spec?
    ///
    /// `shift` is compared only when the spec asks for it, because the host delivers a printable
    /// character already shift-resolved (`Shift+k` arrives as `Char('K')`) — pi distinguishes `K`
    /// from `k` off the raw byte for exactly the same reason.
    #[must_use]
    pub fn matches(&self, key: &OverlayKey) -> bool {
        if self.ctrl != key.ctrl || self.alt != key.alt {
            return false;
        }
        match (self.code, key.code) {
            (OverlayKeyCode::Char(want), OverlayKeyCode::Char(got)) => {
                if self.shift {
                    got.eq_ignore_ascii_case(&want) && (key.shift || got.is_uppercase())
                } else {
                    want == got || (!key.shift && want.eq_ignore_ascii_case(&got))
                }
            }
            (a, b) => a == b && (!self.shift || key.shift),
        }
    }
}

/// `panel-keys.ts` `PanelKeys` — the five members both panels resolve every keystroke through.
///
/// # The three-way `mcp.panel.save`
///
/// `mcp.panel.save` is an **adapter-defined** id: it exists in no host keybinding table and is read
/// straight out of the user's raw bindings map, which is also why it cannot be answered by asking a
/// keybinding manager. Its three outcomes are the point (`__tests__/mcp-panel-keybindings.test.ts`
/// exercises the middle one explicitly):
///
/// | document state | `save()` | `save_label()` |
/// |---|---|---|
/// | absent | `ctrl+s` | `Some("ctrl+s")` |
/// | `["ctrl+p"]` | `ctrl+p` | `Some("ctrl+p")` |
/// | `[]` (present, empty) | never | `None` — the `save` hint disappears and saving is disabled |
///
/// # The residue (MCP-363a)
///
/// The **defaults** for `tui.select.up`/`down`/`confirm` live in `cyrup-tui`'s `SelectKeymap`, which
/// this crate must not depend on, so option (a) of MCP-363a is taken: `cyrup-mcp` carries its own
/// copy (`up` / `down` / `return`, which are also upstream's own no-manager fallbacks) and a
/// cross-crate test is what keeps the two from drifting.
#[derive(Clone, Debug)]
pub struct PanelKeys {
    up: Vec<KeySpec>,
    down: Vec<KeySpec>,
    confirm: Vec<KeySpec>,
    save_specs: Vec<KeySpec>,
    save_label: Option<String>,
}

/// `cyrup-tui`'s `SelectKeymap` defaults for the three ids this panel reads, and upstream's own
/// no-manager fallbacks. MCP-363a's cross-crate test asserts these stay equal.
pub const DEFAULT_SELECT_UP: &str = "up";
/// See [`DEFAULT_SELECT_UP`].
pub const DEFAULT_SELECT_DOWN: &str = "down";
/// See [`DEFAULT_SELECT_UP`].
pub const DEFAULT_SELECT_CONFIRM: &str = "return";
/// The `mcp.panel.save` default, used only when the id is **absent** from the user's document.
pub const DEFAULT_SAVE_KEY: &str = "ctrl+s";

impl Default for PanelKeys {
    /// `createPanelKeys(undefined)` — the hardcoded no-manager arm.
    fn default() -> Self {
        Self {
            up: KeySpec::parse(DEFAULT_SELECT_UP).into_iter().collect(),
            down: KeySpec::parse(DEFAULT_SELECT_DOWN).into_iter().collect(),
            confirm: KeySpec::parse(DEFAULT_SELECT_CONFIRM).into_iter().collect(),
            save_specs: KeySpec::parse(DEFAULT_SAVE_KEY).into_iter().collect(),
            save_label: Some(DEFAULT_SAVE_KEY.to_string()),
        }
    }
}

/// A `KeyId | KeyId[]` value from the user's document, as the list upstream's
/// `Array.isArray(explicit) ? explicit : [explicit]` produces.
fn key_ids(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(one) => Some(vec![one.clone()]),
        serde_json::Value::Array(many) => {
            Some(many.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        }
        _ => None,
    }
}

impl PanelKeys {
    /// `createPanelKeys(keybindings)` over an already-read, already-migrated bindings document.
    ///
    /// `bindings` is `keybindings.getUserBindings()` — the user's raw map, **not** the resolved
    /// `id -> keys` table, which is the only place `mcp.panel.save` can be seen at all.
    #[must_use]
    pub fn from_user_bindings(bindings: &[(String, serde_json::Value)]) -> Self {
        let lookup = |id: &str| -> Option<Vec<String>> {
            bindings.iter().find(|(key, _)| key == id).and_then(|(_, value)| key_ids(value))
        };
        let specs = |ids: Option<Vec<String>>, fallback: &str| -> Vec<KeySpec> {
            match ids {
                // Present, including present-and-empty: the user's list wins outright.
                Some(list) => list.iter().filter_map(|id| KeySpec::parse(id)).collect(),
                None => KeySpec::parse(fallback).into_iter().collect(),
            }
        };

        let save = lookup("mcp.panel.save");
        let configured = save.is_some();
        let save_ids = save.unwrap_or_default();
        // `save(data) = keys.length > 0 ? keys.some(matches) : (!configured && matchesKey("ctrl+s"))`
        let save_specs = if save_ids.is_empty() {
            if configured {
                Vec::new()
            } else {
                KeySpec::parse(DEFAULT_SAVE_KEY).into_iter().collect()
            }
        } else {
            save_ids.iter().filter_map(|id| KeySpec::parse(id)).collect()
        };
        // `saveLabel() = keys[0] ?? (configured ? null : "ctrl+s")`
        let save_label = save_ids.first().cloned().or(if configured {
            None
        } else {
            Some(DEFAULT_SAVE_KEY.to_string())
        });

        Self {
            up: specs(lookup("tui.select.up"), DEFAULT_SELECT_UP),
            down: specs(lookup("tui.select.down"), DEFAULT_SELECT_DOWN),
            confirm: specs(lookup("tui.select.confirm"), DEFAULT_SELECT_CONFIRM),
            save_specs,
            save_label,
        }
    }

    /// Read `<agent_dir>/keybindings.json` and resolve from it.
    ///
    /// The document is run through `cyrup_config::migrate_keybindings_config` first, so a user who
    /// still has a legacy id spelled the old way gets the same answer the TUI gives — the two
    /// readers cannot disagree about what `tui.select.up` means. Every failure mode (absent,
    /// unreadable, malformed, non-object) falls back to [`PanelKeys::default`], which is pi's
    /// "no manager" arm.
    #[must_use]
    pub fn from_agent_dir(agent_dir: &Path) -> Self {
        let path = agent_dir.join("keybindings.json");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw)
        else {
            return Self::default();
        };
        let (migrated, _) = cyrup_config::migrate_keybindings_config(&map);
        Self::from_user_bindings(&migrated)
    }

    /// `keys.selectUp(data)`.
    #[must_use]
    pub fn select_up(&self, key: &OverlayKey) -> bool {
        self.up.iter().any(|spec| spec.matches(key))
    }
    /// `keys.selectDown(data)`.
    #[must_use]
    pub fn select_down(&self, key: &OverlayKey) -> bool {
        self.down.iter().any(|spec| spec.matches(key))
    }
    /// `keys.selectConfirm(data)`.
    #[must_use]
    pub fn select_confirm(&self, key: &OverlayKey) -> bool {
        self.confirm.iter().any(|spec| spec.matches(key))
    }
    /// `keys.save(data)`.
    #[must_use]
    pub fn save(&self, key: &OverlayKey) -> bool {
        self.save_specs.iter().any(|spec| spec.matches(key))
    }
    /// `keys.saveLabel()` — `None` disables saving *and* removes the hint from the hint bar.
    #[must_use]
    pub fn save_label(&self) -> Option<&str> {
        self.save_label.as_deref()
    }
}

/// `matchesKey(data, "ctrl+c")` — the one binding neither panel routes through [`PanelKeys`].
fn is_ctrl_c(key: &OverlayKey) -> bool {
    key.ctrl && !key.alt && matches!(key.code, OverlayKeyCode::Char('c' | 'C'))
}

/// `matchesKey(data, "ctrl+<letter>")` for the panel's own three non-configurable shortcuts.
fn is_ctrl(key: &OverlayKey, letter: char) -> bool {
    key.ctrl
        && !key.alt
        && matches!(key.code, OverlayKeyCode::Char(c) if c.eq_ignore_ascii_case(&letter))
}

/// `matchesKey(data, "escape")`.
fn is_escape(key: &OverlayKey) -> bool {
    !key.ctrl && !key.alt && key.code == OverlayKeyCode::Escape
}

/// `matchesKey(data, "backspace")`.
fn is_backspace(key: &OverlayKey) -> bool {
    !key.ctrl && !key.alt && key.code == OverlayKeyCode::Backspace
}

/// `matchesKey(data, "space")`.
fn is_space(key: &OverlayKey) -> bool {
    !key.ctrl && !key.alt && key.code == OverlayKeyCode::Char(' ')
}

/// pi's printable catch-all, `data.length === 1 && data.charCodeAt(0) >= 32`.
///
/// A `Char` with no `ctrl`/`alt` and no control codepoint is exactly that test: the host never
/// delivers a multi-byte paste as one `Char`, and a control character cannot reach
/// [`OverlayKeyCode::Char`] because the host maps it to a named code first.
fn printable(key: &OverlayKey) -> Option<char> {
    match key.code {
        OverlayKeyCode::Char(c) if !key.ctrl && !key.alt && !c.is_control() => Some(c),
        _ => None,
    }
}

// =================================================================================================
// 4 · `McpPanel` — the data model (13h §1)
// =================================================================================================

/// `mcp-panel.ts`'s `ConnectionStatus`.
///
/// `Connecting` is in the enum but is **never** returned by [`McpPanelCallbacks::connection_status`]
/// (`types.ts` omits it from that callback's declared return type); only the panel sets it, for the
/// duration of a reconnect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Connected and usable.
    Connected,
    /// Configured, not currently connected, no failure recorded.
    Idle,
    /// The last connect attempt failed.
    Failed,
    /// OAuth credentials are missing or unreadable.
    NeedsAuth,
    /// A reconnect this panel started is in flight.
    Connecting,
    /// `disabled: true` in config.
    Disabled,
}

/// `mcp-panel.ts` `ToolState`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolState {
    /// The **raw** MCP tool name, never prefixed — this is what a saved `directTools` array holds.
    pub name: String,
    /// The tool description, already sanitized for display.
    pub description: String,
    /// Live, user-editable.
    pub is_direct: bool,
    /// The baseline the dirty check compares against.
    pub was_direct: bool,
    /// [`estimate_tokens`] over the cached descriptor.
    pub estimated_tokens: usize,
}

/// `mcp-panel.ts` `ServerState`.
#[derive(Clone, Debug)]
pub struct ServerState {
    /// The configured server name.
    pub name: String,
    /// `false` for every server at construction.
    pub expanded: bool,
    /// `prov?.kind ?? "user"`.
    pub source: SourceKind,
    /// Only when `prov.importKind` is present.
    pub import_kind: Option<String>,
    /// Captured at construction and used by `rebuildServerTools`, which is why it lives here rather
    /// than being re-read off the config.
    pub include_tools: Option<Vec<String>>,
    /// See [`Self::include_tools`].
    pub exclude_tools: Option<Vec<String>>,
    /// `definition.exposeResources !== false`, captured.
    pub expose_resources: bool,
    /// The current status.
    pub connection_status: ConnectionStatus,
    /// `callbacks.getFailureMessage?.(name) ?? null`.
    pub failure_message: Option<String>,
    /// The tools, in cache order: real tools first, then the synthetic resource tools.
    pub tools: Vec<ToolState>,
    /// A **valid** cache entry, not merely a present one — this is what drives the `(not cached)`
    /// row.
    pub has_cached_data: bool,
}

/// `mcp-panel.ts` `VisibleItem` — the flattened list the cursor indexes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibleItem {
    /// A server header row.
    Server {
        /// Index into [`McpPanelModel::servers`].
        server_index: usize,
    },
    /// A tool row beneath its server.
    Tool {
        /// Index into [`McpPanelModel::servers`].
        server_index: usize,
        /// Index into that server's `tools`.
        tool_index: usize,
    },
}

impl VisibleItem {
    /// The owning server, for either shape.
    #[must_use]
    pub fn server_index(self) -> usize {
        match self {
            VisibleItem::Server { server_index } | VisibleItem::Tool { server_index, .. } => {
                server_index
            }
        }
    }
}

/// `buildResult`'s tri-state, serialising to `true` / `false` / an array.
///
/// Emitting [`Self::Named`] where upstream emits [`Self::All`] silently rewrites a user's
/// `directTools: true` into a brittle name list that stops matching on the next server update — that
/// is the failure this shape defends against, and why MCP-358 is `critical`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectToolsChange {
    /// Every tool is direct: write the literal `true`.
    All,
    /// No tool is direct: write the literal `false`.
    None,
    /// A partial selection: write the raw names, in list order.
    Named(Vec<String>),
}

/// `types.ts` `McpPanelResult`.
///
/// Servers with no net change are **absent** from [`Self::changes`] entirely, which is what stops
/// `writeDirectToolsConfig` from touching their config files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpPanelResult {
    /// Server name to the tri-state, in `servers` order.
    pub changes: IndexMap<String, DirectToolsChange>,
    /// `true` for Esc / `ctrl+c` / the inactivity timer; `false` only for a save.
    pub cancelled: bool,
}

impl DirectToolsChange {
    /// The `directTools` value this change serialises to, in the shape
    /// [`crate::config::write_direct_tools_config`] consumes.
    #[must_use]
    pub fn to_setting(&self) -> crate::config::BoolOrList {
        match self {
            DirectToolsChange::All => crate::config::BoolOrList::All(true),
            DirectToolsChange::None => crate::config::BoolOrList::All(false),
            DirectToolsChange::Named(names) => {
                crate::config::BoolOrList::Named(names.clone())
            }
        }
    }
}

impl McpPanelResult {
    /// `done({ cancelled: true, changes: new Map() })`.
    #[must_use]
    pub fn cancelled() -> Self {
        Self { changes: IndexMap::new(), cancelled: true }
    }

    /// The write-back argument, in `servers` order.
    ///
    /// A server absent from [`Self::changes`] stays absent here, which is what stops
    /// `writeDirectToolsConfig` from touching its config file at all.
    #[must_use]
    pub fn to_config_changes(&self) -> IndexMap<String, crate::config::BoolOrList> {
        self.changes
            .iter()
            .map(|(name, change)| (name.clone(), change.to_setting()))
            .collect()
    }
}

/// `types.ts` `McpAuthResult`, as much of it as the panel reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpAuthResult {
    /// Whether the flow completed.
    pub ok: bool,
    /// The failure reason, when there is one to show.
    pub message: Option<String>,
}

/// One async job a synchronous keystroke started (MCP-359, MCP-360, MCP-361).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelJob {
    /// `callbacks.authenticate(name)`.
    Authenticate(String),
    /// `callbacks.reconnect(name)`; `after_auth` carries `reconnectServer(server, {afterAuth:true})`.
    Reconnect {
        /// Server name.
        server: String,
        /// Set when this reconnect is the automatic chain after a successful OAuth.
        after_auth: bool,
    },
    /// `copyToClipboard(sanitizeDisplayText(failureMessage))`.
    CopyError {
        /// Server name, already sanitized for the notice.
        server: String,
        /// The sanitized failure text to place on the clipboard.
        text: String,
    },
}

/// A settled [`PanelJob`], fed back through [`McpPanelModel::finish_job`].
#[derive(Clone, Debug)]
pub enum PanelJobResult {
    /// `callbacks.authenticate(name)` settled; `Err` is the promise's rejection.
    Authenticated {
        /// Server name.
        server: String,
        /// The resolved result, or the rejection message.
        outcome: Result<McpAuthResult, String>,
    },
    /// `callbacks.reconnect(name)` settled.
    Reconnected {
        /// Server name.
        server: String,
        /// Whether this was the post-OAuth chain.
        after_auth: bool,
        /// `Ok(connected)` or the rejection message.
        outcome: Result<bool, String>,
    },
    /// The clipboard write settled.
    Copied {
        /// Server name, sanitized.
        server: String,
        /// `Err` carries the message upstream interpolates into its failure notice.
        outcome: Result<(), String>,
    },
}

/// What one keystroke asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelInputOutcome {
    /// Nothing changed.
    Ignored,
    /// State changed; repaint.
    Redraw,
    /// `done(...)` — the result is already on [`McpPanelModel::result`].
    Close,
    /// Start this job and keep the panel open.
    Run(PanelJob),
}

// TODO(MCP-392): `buildMcpPanelCallbacks`' eight-rung connection-status derivation — the per-open
// `authStatusFailures` map, the `resolveServerUrl`-throws-gives-`failed` arm and the four-condition
// OAuth guard — is the `/mcp` dispatcher's, not the panel's: it reads `McpState`'s connection map and
// the credential store. This trait is the seam it plugs into, and nothing in this module derives a
// status of its own.

/// The synchronous half of `McpPanelCallbacks` (`types.ts`) plus its two promise-returning members.
///
/// Upstream's `getConnectionStatus`, `canAuthenticate` and `refreshCacheAfterReconnect` are all
/// **synchronous** and are called from inside a settled promise, so they stay synchronous here and
/// run inside [`McpPanelModel::finish_job`]. `refreshCacheAfterReconnect` re-reads the whole cache
/// file every time; keep that — it is how the panel observes what `updateMetadataCache` just
/// flushed.
pub trait McpPanelCallbacks: Send + Sync + 'static {
    /// `getConnectionStatus(serverName)`. Never returns [`ConnectionStatus::Connecting`].
    fn connection_status(&self, server: &str) -> ConnectionStatus;

    /// `getFailureMessage?.(serverName) ?? null` — optional upstream, defaulted here.
    fn failure_message(&self, _server: &str) -> Option<String> {
        None
    }

    /// `canAuthenticate(serverName)`.
    fn can_authenticate(&self, server: &str) -> bool;

    /// `refreshCacheAfterReconnect(serverName)`.
    fn refresh_cache_after_reconnect(&self, _server: &str) -> Option<ServerCacheEntry> {
        None
    }

    /// `authenticate(serverName): Promise<McpAuthResult>`.
    fn authenticate(
        &self,
        server: String,
    ) -> futures::future::BoxFuture<'static, Result<McpAuthResult, String>>;

    /// `reconnect(serverName): Promise<boolean>`.
    fn reconnect(&self, server: String)
        -> futures::future::BoxFuture<'static, Result<bool, String>>;
}

/// `computeServerHash(definition)` as an injectable seam.
///
/// The digest's five identity fields go through resolvers (`resolveServerUrl` can **throw**, which
/// upstream maps to "this cache entry is invalid") that this module does not own, so the panel is
/// handed the function rather than calling it. `None` falls back to
/// [`ResolvedIdentity::verbatim`], which is correct only for a definition with no interpolation
/// token, no tilde and no secret marker.
pub type ServerHashFn = Arc<dyn Fn(&ServerEntry) -> Option<String> + Send + Sync>;

/// Construction inputs that are not the config, the cache or the provenance map.
///
/// The derived `Default` is upstream's `options = {}` arm: no notice lines, not `authOnly`, the
/// no-manager key defaults, and the verbatim hasher.
#[derive(Default)]
pub struct PanelOptions {
    /// `options.noticeLines` — the shared-config notice, rendered under the search row.
    pub notice_lines: Vec<String>,
    /// `options.authOnly` — the `/mcp-auth` picker. One type, not two (MCP-391).
    pub auth_only: bool,
    /// Resolved once at open; see [`PanelKeys`].
    pub keys: PanelKeys,
    /// `computeServerHash(definition)`, which `isServerCacheValid` compares `configHash` against.
    ///
    /// Injected rather than called directly because the hash's five identity fields go through
    /// resolvers (`resolveServerUrl` can **throw**, which upstream maps to "invalid cache") that
    /// this module does not own. `None` falls back to
    /// [`ResolvedIdentity::verbatim`], which is correct only for definitions with no interpolation
    /// token, no tilde and no secret marker.
    pub server_hash: Option<ServerHashFn>,
}

impl std::fmt::Debug for PanelOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelOptions")
            .field("notice_lines", &self.notice_lines)
            .field("auth_only", &self.auth_only)
            .field("keys", &self.keys)
            .field("server_hash", &self.server_hash.is_some())
            .finish()
    }
}

/// `isUiToolVisibleToModel(tool.uiVisibility)` over the on-disk shape (13h §1 step 5).
///
/// Cut 2 removes the *producer* — nothing in the ported tree writes a non-absent `uiVisibility` —
/// but the predicate's total form is kept verbatim, because a cache written by an
/// upstream-compatible producer must still filter correctly: absent passes, a list containing
/// `"model"` passes, anything else is hidden.
fn ui_visible_to_model(tool: &CachedTool) -> bool {
    match tool.ui_visibility.as_ref() {
        None => true,
        Some(list) => list.iter().any(|entry| entry == "model"),
    }
}

/// `types.ts` `matchesToolSelector`'s **`Set` arm** (13h §1.2).
///
/// This is not the same code path as [`crate::registration::is_tool_allowed`]'s
/// `ToolSelectorCandidateIndex`: the panel supplies a pre-subtracted `Set`, and the two arms differ
/// once a server carries its own `toolPrefix`, because `getOtherCurrentCandidates` deletes using
/// `resolveToolPrefix(definition, this.prefix)` while `matchesToolSelector` computes its current
/// candidates from the panel's **global** prefix. That asymmetry is upstream's and is reproduced
/// rather than "fixed".
fn matches_tool_selector_set(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    patterns: &[String],
    other_current: &HashSet<String>,
) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let current = tool_name_candidates(tool_name, server_name, prefix, false);
    if matches_tool_pattern(&current, patterns) {
        return true;
    }
    let mut legacy = tool_name_candidates(tool_name, server_name, prefix, true);
    for candidate in &current {
        legacy.remove(candidate);
    }
    patterns.iter().any(|pattern| {
        let one = std::slice::from_ref(pattern);
        matches_tool_pattern(&legacy, one) && !matches_tool_pattern(other_current, one)
    })
}

/// `types.ts` `isToolAllowed` = `isToolIncluded && !isToolExcluded`, over the `Set` arm.
fn is_tool_allowed_set(
    tool_name: &str,
    server_name: &str,
    prefix: ToolPrefix,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
    other_current: &HashSet<String>,
) -> bool {
    let included = match include_tools.filter(|p| !p.is_empty()) {
        None => true,
        Some(patterns) => {
            matches_tool_selector_set(tool_name, server_name, prefix, patterns, other_current)
        }
    };
    if !included {
        return false;
    }
    match exclude_tools.filter(|p| !p.is_empty()) {
        None => true,
        Some(patterns) => {
            !matches_tool_selector_set(tool_name, server_name, prefix, patterns, other_current)
        }
    }
}

/// The `/mcp` browser panel's whole state machine — pure, testable, no tokio and no host.
///
/// A **snapshot**: the panel never re-reads config. The only post-construction mutation source is
/// [`Self::finish_job`]'s reconnect arm, which re-reads the metadata cache through
/// [`McpPanelCallbacks::refresh_cache_after_reconnect`].
pub struct McpPanelModel {
    theme: PanelTheme,
    notice_lines: Vec<String>,
    auth_only: bool,
    keys: PanelKeys,
    callbacks: Arc<dyn McpPanelCallbacks>,
    /// `config.settings?.toolPrefix ?? "server"`. Deliberately **not** per-server-resolved when
    /// filtering that server's own tools (13h §1.2's recorded asymmetry).
    prefix: ToolPrefix,
    /// `config.mcpServers`, snapshotted in file order — an `IndexMap`, never a `BTreeMap`: this
    /// order is the row order, the connect order and the collision tie-break.
    config_servers: IndexMap<String, ServerEntry>,
    server_hash: Option<ServerHashFn>,
    cache: Option<MetadataCache>,

    servers: Vec<ServerState>,
    visible_items: Vec<VisibleItem>,
    cursor_index: usize,
    name_query: String,
    desc_search_active: bool,
    desc_query: String,
    dirty: bool,
    confirming_discard: bool,
    discard_selected: u8,
    import_notice: Option<String>,
    auth_notice: Option<String>,
    auth_in_flight: Option<String>,
    /// Where `done(result)` lands. Read by the caller **after** `open_overlay` returns, which the
    /// block-until-closed contract makes safe (MCP-369).
    result: Option<McpPanelResult>,
}

impl McpPanelModel {
    /// `new McpPanel(config, cache, provenance, callbacks, tui, done, options)` (13h §1).
    ///
    /// Construction order, per server, in `Object.entries(config.mcpServers)` order:
    ///
    /// 1. under `auth_only`, a server that cannot authenticate is **skipped entirely**;
    /// 2. the cache entry is used only when it is **valid**; an invalid or stale entry is treated as
    ///    absent, which is what drives the `(not cached)` row;
    /// 3. `directTools` is tri-state: a per-server value that is merely *present* (including
    ///    `false`) beats the global, but a global `false` falls through to the same `false`;
    /// 4. tools are built **only when** there is a valid cache entry, `!auth_only`, and the server
    ///    is not disabled — so `auth_only` shows zero tools and a disabled server shows none even
    ///    with a valid cache;
    /// 5. each resource becomes a synthetic `read_<name>` tool whose token estimate is fed a
    ///    descriptor carrying **no** input schema, so it is systematically smaller than a real
    ///    tool's.
    #[must_use]
    pub fn new(
        config: &McpConfig,
        cache: Option<MetadataCache>,
        provenance: &IndexMap<String, ServerProvenance>,
        callbacks: Arc<dyn McpPanelCallbacks>,
        options: PanelOptions,
    ) -> Self {
        let prefix = config.tool_prefix();
        let mut model = Self {
            theme: PanelTheme::default(),
            notice_lines: options.notice_lines,
            auth_only: options.auth_only,
            keys: options.keys,
            callbacks,
            prefix,
            config_servers: config.mcp_servers.clone(),
            server_hash: options.server_hash,
            cache,
            servers: Vec::new(),
            visible_items: Vec::new(),
            cursor_index: 0,
            name_query: String::new(),
            desc_search_active: false,
            desc_query: String::new(),
            dirty: false,
            confirming_discard: false,
            discard_selected: 1,
            import_notice: None,
            auth_notice: None,
            auth_in_flight: None,
            result: None,
        };

        // `settings.directTools` is a plain boolean upstream (`types.ts:508`), unlike the
        // per-server field, so a global value can only ever contribute `true`.
        let global_direct = config.settings_or_default().direct_tools;
        let config_servers = model.config_servers.clone();
        for (server_name, definition) in &config_servers {
            if model.auth_only && !model.callbacks.can_authenticate(server_name) {
                continue;
            }
            let prov = provenance.get(server_name);
            let server_cache = model.valid_entry(server_name, definition).cloned();

            // `definition.directTools !== undefined` wins outright — **including** an explicit
            // `false` — else a *truthy* global. The asymmetry is deliberate: a per-server `false`
            // wins, and a global `false` falls through to the same `false`.
            let tool_filter = match definition.direct_tools.clone() {
                Some(value) => Some(value),
                None if global_direct == Some(true) => Some(crate::config::BoolOrList::All(true)),
                None => None,
            };
            let is_direct_for = |name: &str| match &tool_filter {
                Some(crate::config::BoolOrList::All(all)) => *all,
                Some(crate::config::BoolOrList::Named(list)) => {
                    list.iter().any(|n| n == name)
                }
                None => false,
            };

            let mut tools: Vec<ToolState> = Vec::new();
            if let Some(entry) = server_cache.as_ref()
                && !model.auth_only
                && !definition.is_disabled()
            {
                let definition_prefix = resolve_tool_prefix(definition, model.prefix);
                for tool in &entry.tools {
                    if !ui_visible_to_model(tool) {
                        continue;
                    }
                    let others = model.other_current_candidates(
                        server_name,
                        definition_prefix,
                        entry,
                        &tool.name,
                    );
                    if !is_tool_allowed_set(
                        &tool.name,
                        server_name,
                        model.prefix,
                        definition.include_tools.as_deref(),
                        definition.exclude_tools.as_deref(),
                        &others,
                    ) {
                        continue;
                    }
                    tools.push(ToolState {
                        name: tool.name.clone(),
                        description: tool.description.clone().unwrap_or_default(),
                        is_direct: is_direct_for(&tool.name),
                        was_direct: is_direct_for(&tool.name),
                        estimated_tokens: estimate_tokens(
                            &tool.name,
                            tool.description.as_deref(),
                            tool.input_schema.as_ref(),
                        ),
                    });
                }
                if definition.expose_resources() {
                    for resource in &entry.resources {
                        let base_name = resource_base_tool_name(&resource.name);
                        let others = model.other_current_candidates(
                            server_name,
                            definition_prefix,
                            entry,
                            &base_name,
                        );
                        if !is_tool_allowed_set(
                            &base_name,
                            server_name,
                            model.prefix,
                            definition.include_tools.as_deref(),
                            definition.exclude_tools.as_deref(),
                            &others,
                        ) {
                            continue;
                        }
                        tools.push(ToolState {
                            name: base_name.clone(),
                            description: resource.description.clone().unwrap_or_else(|| {
                                format!("Read resource: {}", resource.uri)
                            }),
                            is_direct: is_direct_for(&base_name),
                            was_direct: is_direct_for(&base_name),
                            // Deliberately no input schema: a resource tool's estimate is meant to
                            // be smaller than a real tool's.
                            estimated_tokens: estimate_tokens(
                                &base_name,
                                resource.description.as_deref(),
                                None,
                            ),
                        });
                    }
                }
            }

            let status = model.callbacks.connection_status(server_name);
            let failure_message = model.callbacks.failure_message(server_name);
            model.servers.push(ServerState {
                name: server_name.clone(),
                expanded: false,
                source: prov.map_or(SourceKind::User, |p| p.kind),
                import_kind: prov.and_then(|p| p.import_kind.clone()),
                include_tools: definition.include_tools.clone(),
                exclude_tools: definition.exclude_tools.clone(),
                expose_resources: definition.expose_resources(),
                connection_status: status,
                failure_message,
                tools,
                has_cached_data: server_cache.is_some(),
            });
        }

        model.rebuild_visible_items();
        model
    }

    /// `cache?.servers?.[name]` **only if** `isServerCacheValid(entry, definition)`.
    fn valid_entry(&self, server_name: &str, definition: &ServerEntry) -> Option<&ServerCacheEntry> {
        let entry = self.cache.as_ref()?.servers.get(server_name)?;
        let hash = match self.server_hash.as_ref() {
            Some(hasher) => hasher(definition)?,
            None => compute_server_hash(definition, &ResolvedIdentity::verbatim(definition)),
        };
        crate::dirs::is_server_cache_valid(entry, &hash, CACHE_MAX_AGE_MS).then_some(entry)
    }

    /// `getOtherCurrentCandidates(serverName, definition, currentEntry, toolName)` (13h §1.2).
    ///
    /// Iterates **every enabled server including `server_name` itself** — for the current server it
    /// uses `current_entry` rather than re-validating the cache, so a mid-construction entry stays
    /// consistent — collects each one's *current-form* candidates under **its own** effective
    /// prefix, and then deletes the candidates belonging to `(server_name, tool_name)` itself.
    ///
    /// The set therefore contains this server's *sibling* tools' current names as well as every
    /// other server's. Porting this as "every *other* server" is wrong and silently widens what a
    /// legacy pattern may match.
    fn other_current_candidates(
        &self,
        server_name: &str,
        definition_prefix: ToolPrefix,
        current_entry: &ServerCacheEntry,
        tool_name: &str,
    ) -> HashSet<String> {
        let mut candidates: HashSet<String> = HashSet::new();
        for (other_name, other_definition) in &self.config_servers {
            if other_definition.is_disabled() {
                continue;
            }
            let entry = if other_name == server_name {
                current_entry
            } else {
                match self.valid_entry(other_name, other_definition) {
                    Some(found) => found,
                    None => continue,
                }
            };
            let other_prefix = resolve_tool_prefix(other_definition, self.prefix);
            for tool in &entry.tools {
                if !ui_visible_to_model(tool) {
                    continue;
                }
                candidates
                    .extend(tool_name_candidates(&tool.name, other_name, other_prefix, false));
            }
            if other_definition.expose_resources() {
                for resource in &entry.resources {
                    let base_name = resource_base_tool_name(&resource.name);
                    candidates
                        .extend(tool_name_candidates(&base_name, other_name, other_prefix, false));
                }
            }
        }
        for candidate in tool_name_candidates(tool_name, server_name, definition_prefix, false) {
            candidates.remove(&candidate);
        }
        candidates
    }

    /// `rebuildVisibleItems()` (13h §1.4).
    ///
    /// Three behaviours that must not be lost: a non-empty query **force-expands every server**;
    /// a server-name match propagates to its tools at 0.6 weight in name mode, so typing a server
    /// name lists all of its tools even when no tool name matches; and the final filter drops
    /// server headers with no surviving tools, evaluated against the *pre-filter* list, which is
    /// what makes it one pass rather than a fixpoint.
    fn rebuild_visible_items(&mut self) {
        let (query, desc_mode) = if self.desc_search_active {
            (self.desc_query.clone(), true)
        } else {
            (self.name_query.clone(), false)
        };
        let mut items: Vec<VisibleItem> = Vec::new();
        for (si, server) in self.servers.iter().enumerate() {
            if !query.is_empty() && self.auth_only {
                // `mode === "name" ? fuzzyScore(query, server.name) : 0` — desc search is refused in
                // authOnly, so the `0` arm is unreachable and kept only for shape.
                let score = if desc_mode { 0.0 } else { fuzzy_score(&query, &server.name) };
                if score > 0.0 {
                    items.push(VisibleItem::Server { server_index: si });
                }
                continue;
            }
            items.push(VisibleItem::Server { server_index: si });
            if server.expanded || !query.is_empty() {
                for (ti, tool) in server.tools.iter().enumerate() {
                    if !query.is_empty() {
                        let score = if desc_mode {
                            fuzzy_score(&query, &tool.description)
                        } else {
                            fuzzy_score(&query, &tool.name)
                                .max(fuzzy_score(&query, &server.name) * 0.6)
                        };
                        if score == 0.0 {
                            continue;
                        }
                    }
                    items.push(VisibleItem::Tool { server_index: si, tool_index: ti });
                }
            }
        }
        if !query.is_empty() && !self.auth_only {
            let keep: HashSet<usize> = items
                .iter()
                .filter_map(|item| match item {
                    VisibleItem::Tool { server_index, .. } => Some(*server_index),
                    VisibleItem::Server { .. } => None,
                })
                .collect();
            items.retain(|item| match item {
                VisibleItem::Tool { .. } => true,
                VisibleItem::Server { server_index } => keep.contains(server_index),
            });
        }
        self.visible_items = items;
    }

    /// The clamp that follows **every** `rebuildVisibleItems` call: the cursor is clamped, never
    /// reset to 0.
    fn clamp_cursor(&mut self) {
        self.cursor_index =
            self.cursor_index.min(self.visible_items.len().saturating_sub(1));
    }

    fn rebuild_and_clamp(&mut self) {
        self.rebuild_visible_items();
        self.clamp_cursor();
    }

    /// `moveCursor(delta)` — clamps and does **not** wrap; a no-op on an empty list.
    fn move_cursor(&mut self, delta: i64) {
        if self.visible_items.is_empty() {
            return;
        }
        let last = self.visible_items.len().saturating_sub(1) as i64;
        let next = (self.cursor_index as i64 + delta).clamp(0, last);
        self.cursor_index = usize::try_from(next).unwrap_or(0);
    }

    /// `updateDirty()`.
    fn update_dirty(&mut self) {
        self.dirty = self
            .servers
            .iter()
            .any(|s| s.tools.iter().any(|t| t.is_direct != t.was_direct));
    }

    /// The import notice, at all three of upstream's identical sites. Note the fallback here is
    /// `"external"`, where the server row's import *label* falls back to `"import"`.
    fn import_notice_text(import_kind: Option<&str>) -> String {
        format!(
            "Imported from {} \u{2014} will copy to user config on save",
            sanitize_display_text(Some(import_kind.unwrap_or("external")))
        )
    }

    /// `buildResult()` (13h §1.8) — per server, **only when that server has at least one changed
    /// tool**.
    #[must_use]
    pub fn build_result(&self) -> McpPanelResult {
        let mut changes: IndexMap<String, DirectToolsChange> = IndexMap::new();
        for server in &self.servers {
            if !server.tools.iter().any(|t| t.is_direct != t.was_direct) {
                continue;
            }
            let direct: Vec<&ToolState> = server.tools.iter().filter(|t| t.is_direct).collect();
            let change = if direct.len() == server.tools.len() && !server.tools.is_empty() {
                DirectToolsChange::All
            } else if direct.is_empty() {
                DirectToolsChange::None
            } else {
                DirectToolsChange::Named(direct.iter().map(|t| t.name.clone()).collect())
            };
            changes.insert(server.name.clone(), change);
        }
        McpPanelResult { changes, cancelled: false }
    }

    /// The result `done(...)` published, or `None` while the panel is still open (MCP-369).
    #[must_use]
    pub fn result(&self) -> Option<&McpPanelResult> {
        self.result.as_ref()
    }

    /// Read-only access for tests and for the overlay's hint logic.
    #[must_use]
    pub fn servers(&self) -> &[ServerState] {
        &self.servers
    }

    /// The flattened list the cursor indexes into.
    #[must_use]
    pub fn visible_items(&self) -> &[VisibleItem] {
        &self.visible_items
    }

    /// The cursor's position in [`Self::visible_items`].
    #[must_use]
    pub fn cursor_index(&self) -> usize {
        self.cursor_index
    }

    /// Whether any toggle differs from its baseline.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The OAuth / reconnect / clipboard notice, un-truncated. Rendered italic in `needsAuth`
    /// beneath the body; exposed because the frame truncates it to the panel width.
    #[must_use]
    pub fn auth_notice(&self) -> Option<&str> {
        self.auth_notice.as_deref()
    }

    /// The "will copy to user config on save" notice, un-truncated.
    #[must_use]
    pub fn import_notice(&self) -> Option<&str> {
        self.import_notice.as_deref()
    }

    /// `toggleItem(item)` — a no-op when `authOnly` (13h §1.8).
    ///
    /// On a **server** row the new state is `!server.tools.every(isDirect)` — "all on unless already
    /// all on". `every()` on an *empty* list is `true`, so a server with no tools toggles to `false`
    /// and nothing happens.
    fn toggle_item(&mut self, item: VisibleItem) {
        if self.auth_only {
            return;
        }
        let Some(server) = self.servers.get_mut(item.server_index()) else {
            return;
        };
        let source = server.source;
        let import_kind = server.import_kind.clone();
        let mut notice = None;
        match item {
            VisibleItem::Server { .. } => {
                let new_state = !server.tools.iter().all(|t| t.is_direct);
                if source == SourceKind::Import && new_state {
                    notice = Some(Self::import_notice_text(import_kind.as_deref()));
                }
                for tool in &mut server.tools {
                    tool.is_direct = new_state;
                }
            }
            VisibleItem::Tool { tool_index, .. } => {
                let Some(tool) = server.tools.get_mut(tool_index) else {
                    return;
                };
                tool.is_direct = !tool.is_direct;
                if tool.is_direct && source == SourceKind::Import {
                    notice = Some(Self::import_notice_text(import_kind.as_deref()));
                }
            }
        }
        if let Some(text) = notice {
            self.import_notice = Some(text);
        }
        self.update_dirty();
    }
}

impl McpPanelModel {
    /// `handleInput(data)` — the 14-step ordered dispatch of 13h §1.5 (MCP-355, `critical`).
    ///
    /// The order decides the edge cases. `ctrl+s` saves from inside description search (steps 2–3
    /// are tested *above* the modal branch) but **not** from inside the discard modal (step 1
    /// returns first). Mis-order it and the panel either persists changes the user was being asked
    /// about or silently drops them.
    ///
    /// The preamble runs on **every** keystroke: the import notice is always cleared, and the auth
    /// notice is cleared **only when no OAuth is in flight** — an in-flight notice survives
    /// keystrokes, every other notice does not.
    pub fn handle_key(&mut self, key: OverlayKey) -> PanelInputOutcome {
        self.import_notice = None;
        if self.auth_in_flight.is_none() {
            self.auth_notice = None;
        }

        // 1 — the discard modal swallows everything else, `ctrl+s` included.
        if self.confirming_discard {
            return self.handle_discard_key(key);
        }

        // 2 — `ctrl+c`.
        if is_ctrl_c(&key) {
            self.result = Some(McpPanelResult::cancelled());
            return PanelInputOutcome::Close;
        }

        // 3 — save. Works during desc search, because it is tested above the modal branch.
        if self.keys.save(&key) {
            self.result = Some(self.build_result());
            return PanelInputOutcome::Close;
        }

        // 4 — description-search modal.
        if self.desc_search_active {
            return self.handle_desc_search_key(key);
        }

        // 5 — escape: clear the query, else confirm a discard, else cancel.
        if is_escape(&key) {
            if !self.name_query.is_empty() {
                self.name_query.clear();
                self.rebuild_and_clamp();
                return PanelInputOutcome::Redraw;
            }
            if self.dirty {
                self.confirming_discard = true;
                self.discard_selected = 1;
                return PanelInputOutcome::Redraw;
            }
            self.result = Some(McpPanelResult::cancelled());
            return PanelInputOutcome::Close;
        }

        // 6 — navigation.
        if self.keys.select_up(&key) {
            self.move_cursor(-1);
            return PanelInputOutcome::Redraw;
        }
        if self.keys.select_down(&key) {
            self.move_cursor(1);
            return PanelInputOutcome::Redraw;
        }

        // 7 — `space` toggles. Claimed here, which is why a literal space is unenterable in either
        // query.
        if is_space(&key) {
            let Some(item) = self.visible_items.get(self.cursor_index).copied() else {
                return PanelInputOutcome::Ignored;
            };
            if self.auth_only {
                return PanelInputOutcome::Ignored;
            }
            self.toggle_item(item);
            return PanelInputOutcome::Redraw;
        }

        // 8 — confirm: expand/auth on a server row, toggle on a tool row.
        if self.keys.select_confirm(&key) {
            let Some(item) = self.visible_items.get(self.cursor_index).copied() else {
                return PanelInputOutcome::Ignored;
            };
            let index = item.server_index();
            let Some(server) = self.servers.get(index) else {
                return PanelInputOutcome::Ignored;
            };
            match item {
                VisibleItem::Server { .. } => {
                    if server.connection_status == ConnectionStatus::Disabled {
                        return PanelInputOutcome::Ignored;
                    }
                    if self.auth_only || server.connection_status == ConnectionStatus::NeedsAuth {
                        return self.authenticate_server(index);
                    }
                    if let Some(server) = self.servers.get_mut(index) {
                        server.expanded = !server.expanded;
                    }
                    self.rebuild_and_clamp();
                    PanelInputOutcome::Redraw
                }
                VisibleItem::Tool { tool_index, .. } => {
                    let source = server.source;
                    let import_kind = server.import_kind.clone();
                    let Some(server) = self.servers.get_mut(index) else {
                        return PanelInputOutcome::Ignored;
                    };
                    let Some(tool) = server.tools.get_mut(tool_index) else {
                        return PanelInputOutcome::Ignored;
                    };
                    tool.is_direct = !tool.is_direct;
                    let now_direct = tool.is_direct;
                    if now_direct && source == SourceKind::Import {
                        self.import_notice =
                            Some(Self::import_notice_text(import_kind.as_deref()));
                    }
                    self.update_dirty();
                    PanelInputOutcome::Redraw
                }
            }
        } else if is_ctrl(&key, 'a') {
            // 9 — authenticate the cursor's server.
            match self.visible_items.get(self.cursor_index).copied() {
                Some(item) => self.authenticate_server(item.server_index()),
                None => PanelInputOutcome::Ignored,
            }
        } else if is_ctrl(&key, 'r') {
            // 10 — reconnect the cursor's server.
            let Some(item) = self.visible_items.get(self.cursor_index).copied() else {
                return PanelInputOutcome::Ignored;
            };
            match self.start_reconnect(item.server_index(), false) {
                Some(job) => PanelInputOutcome::Run(job),
                None => PanelInputOutcome::Ignored,
            }
        } else if is_ctrl(&key, 'y') {
            // 11 — copy the failure message, only when there is one.
            self.copy_failure_message()
        } else if !key.ctrl && !key.alt && key.code == OverlayKeyCode::Char('?') {
            // 12 — desc search. Intercepted *before* the printable catch-all, so `?` can never be
            // typed into the name query.
            if self.auth_only {
                return PanelInputOutcome::Ignored;
            }
            self.desc_search_active = true;
            self.desc_query.clear();
            self.rebuild_and_clamp();
            PanelInputOutcome::Redraw
        } else if is_backspace(&key) {
            // 13 — backspace pops from the name query.
            if self.name_query.pop().is_some() {
                self.rebuild_and_clamp();
                return PanelInputOutcome::Redraw;
            }
            PanelInputOutcome::Ignored
        } else if let Some(ch) = printable(&key) {
            // 14 — every other printable feeds the always-on name search.
            self.name_query.push(ch);
            self.rebuild_and_clamp();
            PanelInputOutcome::Redraw
        } else {
            PanelInputOutcome::Ignored
        }
    }

    /// The description-search modal (13h §1.5.1). Everything not listed here is **swallowed**;
    /// `ctrl+c` and save still work because they are tested at steps 2–3, above this branch.
    fn handle_desc_search_key(&mut self, key: OverlayKey) -> PanelInputOutcome {
        if is_escape(&key) || self.keys.select_confirm(&key) {
            self.desc_search_active = false;
            // Exiting always clears the query, so the description filter is never sticky.
            self.desc_query.clear();
            self.rebuild_and_clamp();
            return PanelInputOutcome::Redraw;
        }
        if is_backspace(&key) {
            if self.desc_query.pop().is_some() {
                self.rebuild_and_clamp();
                return PanelInputOutcome::Redraw;
            }
            return PanelInputOutcome::Ignored;
        }
        if self.keys.select_up(&key) {
            self.move_cursor(-1);
            return PanelInputOutcome::Redraw;
        }
        if self.keys.select_down(&key) {
            self.move_cursor(1);
            return PanelInputOutcome::Redraw;
        }
        if is_space(&key) {
            // Upstream's comment is "Toggle even while in desc search".
            if let Some(item) = self.visible_items.get(self.cursor_index).copied() {
                self.toggle_item(item);
                return PanelInputOutcome::Redraw;
            }
            return PanelInputOutcome::Ignored;
        }
        if let Some(ch) = printable(&key) {
            self.desc_query.push(ch);
            self.rebuild_and_clamp();
            return PanelInputOutcome::Redraw;
        }
        PanelInputOutcome::Ignored
    }

    /// `handleDiscardInput(data)` (13h §1.6). Armed only by `escape` with `dirty == true` and an
    /// empty name query; `discardSelected` starts at **1** (Keep & Close), the safe option.
    /// Everything not listed, `ctrl+s` included, is ignored.
    fn handle_discard_key(&mut self, key: OverlayKey) -> PanelInputOutcome {
        if is_ctrl_c(&key) {
            self.result = Some(McpPanelResult::cancelled());
            return PanelInputOutcome::Close;
        }
        // `data === "n"` upstream, i.e. the raw byte: a modified `n` is a different string.
        if is_escape(&key)
            || (!key.ctrl && !key.alt && matches!(key.code, OverlayKeyCode::Char('n' | 'N')))
        {
            self.confirming_discard = false;
            return PanelInputOutcome::Redraw;
        }
        if self.keys.select_confirm(&key) {
            self.result = Some(if self.discard_selected == 0 {
                McpPanelResult::cancelled()
            } else {
                self.build_result()
            });
            return PanelInputOutcome::Close;
        }
        if !key.ctrl && !key.alt && matches!(key.code, OverlayKeyCode::Char('y' | 'Y')) {
            self.result = Some(McpPanelResult::cancelled());
            return PanelInputOutcome::Close;
        }
        if !key.ctrl
            && !key.alt
            && matches!(
                key.code,
                OverlayKeyCode::Left | OverlayKeyCode::Right | OverlayKeyCode::Tab
            )
        {
            self.discard_selected = u8::from(self.discard_selected == 0);
            return PanelInputOutcome::Redraw;
        }
        PanelInputOutcome::Ignored
    }

    /// `authenticateServer(server)` (13h §1.7) — the panel's own method, not the `commands.ts` free
    /// function of the same name it eventually calls.
    ///
    /// Single-flight **globally**: `authInFlight` is one name, not a set, so a second `ctrl+a`
    /// while one flow is running is refused outright.
    fn authenticate_server(&mut self, index: usize) -> PanelInputOutcome {
        if self.auth_in_flight.is_some() {
            return PanelInputOutcome::Ignored;
        }
        let Some(server) = self.servers.get(index) else {
            return PanelInputOutcome::Ignored;
        };
        if matches!(
            server.connection_status,
            ConnectionStatus::Connecting | ConnectionStatus::Disabled
        ) {
            return PanelInputOutcome::Ignored;
        }
        let raw_name = server.name.clone();
        let display = sanitize_display_text(Some(&raw_name));
        if !self.callbacks.can_authenticate(&raw_name) {
            self.auth_notice = Some(format!("{display} does not use OAuth authentication."));
            return PanelInputOutcome::Redraw;
        }
        self.auth_in_flight = Some(raw_name.clone());
        self.auth_notice = Some(format!("Authenticating {display}..."));
        PanelInputOutcome::Run(PanelJob::Authenticate(raw_name))
    }

    /// `reconnectServer(server, {afterAuth})`'s synchronous half: refuse while connecting or
    /// disabled, otherwise set `connecting` and hand the job out.
    fn start_reconnect(&mut self, index: usize, after_auth: bool) -> Option<PanelJob> {
        let server = self.servers.get_mut(index)?;
        if matches!(
            server.connection_status,
            ConnectionStatus::Connecting | ConnectionStatus::Disabled
        ) {
            return None;
        }
        server.connection_status = ConnectionStatus::Connecting;
        Some(PanelJob::Reconnect { server: server.name.clone(), after_auth })
    }

    /// `ctrl+y` (13h §1.9) — requires `status == "failed"` **and** a failure message, and copies the
    /// **sanitized** form, not the raw one.
    fn copy_failure_message(&mut self) -> PanelInputOutcome {
        let Some(item) = self.visible_items.get(self.cursor_index).copied() else {
            return PanelInputOutcome::Ignored;
        };
        let Some(server) = self.servers.get(item.server_index()) else {
            return PanelInputOutcome::Ignored;
        };
        if server.connection_status != ConnectionStatus::Failed {
            return PanelInputOutcome::Ignored;
        }
        let Some(failure) = server.failure_message.as_deref().filter(|m| !m.is_empty()) else {
            return PanelInputOutcome::Ignored;
        };
        PanelInputOutcome::Run(PanelJob::CopyError {
            server: sanitize_display_text(Some(&server.name)),
            text: sanitize_display_text(Some(failure)),
        })
    }

    /// `selectedServerHasFailureMessage()` — gates the `ctrl+y copy error` hint.
    #[must_use]
    pub fn selected_server_has_failure_message(&self) -> bool {
        self.visible_items
            .get(self.cursor_index)
            .and_then(|item| self.servers.get(item.server_index()))
            .is_some_and(|server| {
                server.connection_status == ConnectionStatus::Failed
                    && server.failure_message.as_deref().is_some_and(|m| !m.is_empty())
            })
    }

    fn server_index(&self, name: &str) -> Option<usize> {
        self.servers.iter().position(|s| s.name == name)
    }

    /// Settle one [`PanelJob`] into panel state — upstream's `.then(...)` / `.catch(...)` bodies.
    ///
    /// Returns the follow-on job when there is one: a successful OAuth chains straight into
    /// `reconnectServer(server, { afterAuth: true })`, which is the one place upstream starts a
    /// second promise from inside the first.
    ///
    /// The status is refreshed from [`McpPanelCallbacks::connection_status`] in **all three** auth
    /// arms, *before* the notice is set.
    pub fn finish_job(&mut self, result: PanelJobResult) -> Option<PanelJob> {
        match result {
            PanelJobResult::Authenticated { server, outcome } => {
                let display = sanitize_display_text(Some(&server));
                let index = self.server_index(&server)?;
                let status = self.callbacks.connection_status(&server);
                if let Some(state) = self.servers.get_mut(index) {
                    state.connection_status = status;
                }
                match outcome {
                    Ok(auth) if auth.ok => {
                        self.auth_notice =
                            Some(format!("OAuth finished for {display}. Reconnecting..."));
                        self.auth_in_flight = None;
                        return self.start_reconnect(index, true);
                    }
                    Ok(auth) => {
                        let message = sanitize_display_text(auth.message.as_deref());
                        self.auth_notice = Some(if message.is_empty() {
                            format!(
                                "OAuth failed for {display}. Check the notification for details."
                            )
                        } else {
                            format!("OAuth failed for {display}: {message}")
                        });
                        self.auth_in_flight = None;
                    }
                    Err(error) => {
                        let message = sanitize_display_text(Some(&error));
                        self.auth_notice = Some(format!("OAuth failed for {display}: {message}"));
                        self.auth_in_flight = None;
                    }
                }
                None
            }
            PanelJobResult::Reconnected { server, after_auth, outcome } => {
                let display = sanitize_display_text(Some(&server));
                let index = self.server_index(&server)?;
                match outcome {
                    Ok(connected) => {
                        let status = self.callbacks.connection_status(&server);
                        let failure = self.callbacks.failure_message(&server);
                        if let Some(state) = self.servers.get_mut(index) {
                            state.connection_status = status;
                            state.failure_message = failure;
                        }
                        if status == ConnectionStatus::Connected {
                            let entry = self.callbacks.refresh_cache_after_reconnect(&server);
                            if let Some(entry) = entry {
                                let cache =
                                    self.cache.get_or_insert_with(MetadataCache::default);
                                cache.servers.insert(server.clone(), entry.clone());
                                self.rebuild_server_tools(index, &entry);
                            }
                            // Unconditional inside the connected branch — even when the cache
                            // re-read returned nothing.
                            if let Some(state) = self.servers.get_mut(index) {
                                state.has_cached_data = true;
                            }
                        }
                        if after_auth {
                            self.auth_notice = Some(
                                if connected && status == ConnectionStatus::Connected {
                                    format!("OAuth finished for {display}. Reconnected.")
                                } else {
                                    format!(
                                        "OAuth finished for {display}, but reconnect did not complete. Press ctrl+r to retry."
                                    )
                                },
                            );
                        }
                    }
                    Err(error) => {
                        // Set **directly**, not re-derived from the callback.
                        if let Some(state) = self.servers.get_mut(index) {
                            state.connection_status = ConnectionStatus::Failed;
                        }
                        let message = sanitize_display_text(Some(&error));
                        self.auth_notice =
                            Some(format!("Reconnect failed for {display}: {message}"));
                    }
                }
                None
            }
            PanelJobResult::Copied { server, outcome } => {
                self.auth_notice = Some(match outcome {
                    Ok(()) => format!("Copied error for {server} to clipboard"),
                    Err(error) => {
                        let message = sanitize_display_text(Some(&error));
                        format!("Failed to copy error for {server}: {message}")
                    }
                });
                None
            }
        }
    }

    /// `rebuildServerTools(server, entry)` (13h §1.7) — preserves user edits across a reconnect.
    ///
    /// A tool that vanished and came back is treated as **never having been direct**
    /// (`was_direct = false`), while a surviving tool keeps its dirty-comparison baseline. The
    /// filters use `server`'s *captured* include/exclude and `expose_resources`, not the config's,
    /// and the prefix is the panel's global one — upstream passes a `ServerState` where
    /// construction passed a definition, and a `ServerState` has no `toolPrefix` field, so
    /// `resolveToolPrefix` falls through to the global.
    ///
    /// **`cursor_index` is deliberately not clamped here**: upstream does not, so a shrinking list
    /// can leave the cursor past the end until the next keystroke clamps it, and `render` tolerates
    /// it by skipping the row.
    fn rebuild_server_tools(&mut self, index: usize, entry: &ServerCacheEntry) {
        let Some(server) = self.servers.get(index) else {
            return;
        };
        let name = server.name.clone();
        let include = server.include_tools.clone();
        let exclude = server.exclude_tools.clone();
        let expose_resources = server.expose_resources;
        let existing: HashMap<String, (bool, bool)> = server
            .tools
            .iter()
            .map(|t| (t.name.clone(), (t.is_direct, t.was_direct)))
            .collect();

        let mut new_tools: Vec<ToolState> = Vec::new();
        for tool in &entry.tools {
            if !ui_visible_to_model(tool) {
                continue;
            }
            let others = self.other_current_candidates(&name, self.prefix, entry, &tool.name);
            if !is_tool_allowed_set(
                &tool.name,
                &name,
                self.prefix,
                include.as_deref(),
                exclude.as_deref(),
                &others,
            ) {
                continue;
            }
            let prev = existing.get(&tool.name).copied();
            new_tools.push(ToolState {
                name: tool.name.clone(),
                description: tool.description.clone().unwrap_or_default(),
                is_direct: prev.is_some_and(|(is_direct, _)| is_direct),
                was_direct: prev.is_some_and(|(_, was_direct)| was_direct),
                estimated_tokens: estimate_tokens(
                    &tool.name,
                    tool.description.as_deref(),
                    tool.input_schema.as_ref(),
                ),
            });
        }
        if expose_resources {
            for resource in &entry.resources {
                let base_name = resource_base_tool_name(&resource.name);
                let others = self.other_current_candidates(&name, self.prefix, entry, &base_name);
                if !is_tool_allowed_set(
                    &base_name,
                    &name,
                    self.prefix,
                    include.as_deref(),
                    exclude.as_deref(),
                    &others,
                ) {
                    continue;
                }
                let prev = existing.get(&base_name).copied();
                new_tools.push(ToolState {
                    name: base_name.clone(),
                    description: resource
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Read resource: {}", resource.uri)),
                    is_direct: prev.is_some_and(|(is_direct, _)| is_direct),
                    was_direct: prev.is_some_and(|(_, was_direct)| was_direct),
                    estimated_tokens: estimate_tokens(
                        &base_name,
                        resource.description.as_deref(),
                        None,
                    ),
                });
            }
        }

        if let Some(server) = self.servers.get_mut(index) {
            server.tools = new_tools;
        }
        self.rebuild_visible_items();
        self.update_dirty();
    }

    /// The inactivity auto-cancel (MCP-362): a **silent** cancel that discards unsaved changes with
    /// no confirmation prompt, exactly as upstream's `setTimeout` body does.
    pub fn expire(&mut self) {
        self.result = Some(McpPanelResult::cancelled());
    }
}

// =================================================================================================
// 5 · The panel frame (13h §1.11) and the row renderers (§1.12)
// =================================================================================================

/// `rainbowProgress(filled, total)` — `total` dots joined by a single space, dot `i` coloured
/// `RAINBOW_COLORS[i % 7]` and rendered filled when `i < filled`.
#[must_use]
pub fn rainbow_progress(filled: usize, total: usize) -> StyledText {
    let mut out = StyledText::new();
    for i in 0..total {
        if i > 0 {
            out.raw(" ");
        }
        let color = RAINBOW_COLORS.get(i % RAINBOW_COLORS.len()).copied();
        let Some(color) = color else { continue };
        out.push(Style::fg(color), if i < filled { "\u{25cf}" } else { "\u{25cb}" });
    }
    out
}

impl McpPanelModel {
    /// `row(content)` — the bordered, sanitized, truncated-and-**padded** body row. The pad is what
    /// keeps the right border aligned.
    fn row(&self, inner_w: usize, content: &StyledText) -> OverlayLine {
        let mut spans = vec![self.theme.border.span("\u{2502}")];
        let mut body = vec![OverlaySpan::raw(" ")];
        body.extend(sanitize_row_content(&content.spans));
        spans.extend(truncate_spans(&body, inner_w, "\u{2026}", true));
        spans.push(self.theme.border.span("\u{2502}"));
        OverlayLine::new(spans)
    }

    /// `emptyRow()`.
    fn empty_row(&self, inner_w: usize) -> OverlayLine {
        OverlayLine::new(vec![
            self.theme.border.span("\u{2502}"),
            OverlaySpan::raw(" ".repeat(inner_w)),
            self.theme.border.span("\u{2502}"),
        ])
    }

    /// `divider()`.
    fn divider(&self, inner_w: usize) -> OverlayLine {
        OverlayLine::new(vec![self.theme.border.span(format!(
            "\u{251c}{}\u{2524}",
            "\u{2500}".repeat(inner_w)
        ))])
    }

    /// `render(width)` — the whole frame, top to bottom (13h §1.11, MCP-366).
    ///
    /// The 12-row body window is the panel's own answer to the host's height clip: the seam's
    /// contract calls returning more rows than fit "lossless-by-design", so a panel that does not
    /// window its body simply loses the rows (MCP-368's height half).
    #[must_use]
    pub fn render(&self, width: usize) -> Vec<OverlayLine> {
        let inner_w = width.saturating_sub(2);
        let t = self.theme;
        let mut lines: Vec<OverlayLine> = Vec::new();

        // 1 — title bar, centred by width, not by character count.
        let title_text = if self.auth_only { " MCP OAuth " } else { " MCP Servers " };
        let border_len = inner_w.saturating_sub(visible_width(title_text));
        let left_b = border_len / 2;
        let right_b = border_len - left_b;
        lines.push(OverlayLine::new(vec![
            t.border.span(format!("\u{256d}{}", "\u{2500}".repeat(left_b))),
            t.title.span(title_text),
            t.border.span(format!("{}\u{256e}", "\u{2500}".repeat(right_b))),
        ]));

        // 2 — a blank row.
        lines.push(self.empty_row(inner_w));

        // 3 — the search row, in one of three forms.
        let mut search = StyledText::new();
        search.push(t.border, "\u{25ce}").raw("  ");
        if self.desc_search_active {
            search.push(t.needs_auth, "desc:").raw(" ").raw(self.desc_query.clone());
            search.push(t.selected, "\u{2502}");
        } else if !self.name_query.is_empty() {
            search.raw(self.name_query.clone());
            search.push(t.selected, "\u{2502}");
        } else {
            search.push(t.placeholder.italic(), "search...");
        }
        lines.push(self.row(inner_w, &search));

        // 4 — a blank row, then the notice block when there is one.
        lines.push(self.empty_row(inner_w));
        if !self.notice_lines.is_empty() {
            for notice in &self.notice_lines {
                lines.push(self.row(
                    inner_w,
                    &StyledText::styled(
                        t.hint.italic(),
                        sanitize_display_text(Some(notice)),
                    ),
                ));
            }
            lines.push(self.empty_row(inner_w));
        }

        // 5 — the divider above the body.
        lines.push(self.divider(inner_w));

        // 6 — the body.
        if self.servers.is_empty() {
            lines.push(self.empty_row(inner_w));
            lines.push(self.row(
                inner_w,
                &StyledText::styled(
                    t.hint.italic(),
                    if self.auth_only {
                        "No OAuth-capable MCP servers configured."
                    } else {
                        "No MCP servers configured."
                    },
                ),
            ));
            lines.push(self.empty_row(inner_w));
        } else {
            let total = self.visible_items.len();
            let max_vis = MAX_VISIBLE;
            // `total - maxVis` is computed as a **signed** value, so `total < maxVis` yields 0.
            let signed_total = i64::try_from(total).unwrap_or(i64::MAX);
            let signed_cursor = i64::try_from(self.cursor_index).unwrap_or(0);
            let signed_max = i64::try_from(max_vis).unwrap_or(0);
            let start = (signed_cursor - signed_max / 2).min(signed_total - signed_max).max(0);
            let start_idx = usize::try_from(start).unwrap_or(0);
            let end_idx = start_idx.saturating_add(max_vis).min(total);

            lines.push(self.empty_row(inner_w));
            for i in start_idx..end_idx {
                let Some(item) = self.visible_items.get(i) else { continue };
                let is_cursor = i == self.cursor_index;
                let Some(server) = self.servers.get(item.server_index()) else { continue };
                match item {
                    VisibleItem::Server { .. } => {
                        lines.push(self.row(inner_w, &self.render_server_row(server, is_cursor)));
                        if is_cursor
                            && server.connection_status == ConnectionStatus::Failed
                            && let Some(failure) = server.failure_message.as_deref()
                            && !failure.is_empty()
                        {
                            let sanitized = sanitize_display_text(Some(failure));
                            for line in wrap_text_hard(&sanitized, inner_w.saturating_sub(6)) {
                                let mut row = StyledText::new();
                                row.raw("    ").push(t.cancel, line);
                                lines.push(self.row(inner_w, &row));
                            }
                        }
                    }
                    VisibleItem::Tool { tool_index, .. } => {
                        if let Some(tool) = server.tools.get(*tool_index) {
                            lines.push(
                                self.row(inner_w, &self.render_tool_row(tool, is_cursor, inner_w)),
                            );
                        }
                    }
                }
            }
            lines.push(self.empty_row(inner_w));

            if total > max_vis {
                let ratio = (self.cursor_index as f64 + 1.0) / total as f64;
                let prog = usize::try_from((ratio * 10.0).round() as i64).unwrap_or(0);
                let mut row = rainbow_progress(prog, 10);
                row.raw("  ");
                row.push(t.hint, format!("{}/{total}", self.cursor_index + 1));
                lines.push(self.row(inner_w, &row));
                lines.push(self.empty_row(inner_w));
            }

            if let Some(notice) = self.import_notice.as_deref() {
                lines.push(self.row(
                    inner_w,
                    &StyledText::styled(t.needs_auth.italic(), sanitize_display_text(Some(notice))),
                ));
                lines.push(self.empty_row(inner_w));
            }
            if let Some(notice) = self.auth_notice.as_deref() {
                lines.push(self.row(
                    inner_w,
                    &StyledText::styled(t.needs_auth.italic(), sanitize_display_text(Some(notice))),
                ));
                lines.push(self.empty_row(inner_w));
            }
        }

        // 7 — the divider above the status line.
        lines.push(self.divider(inner_w));
        lines.push(self.empty_row(inner_w));

        // 8 — the status line.
        if self.confirming_discard {
            let mut row = StyledText::new();
            row.raw("Discard unsaved changes?  ");
            // The two-space padding inside each label is part of the string.
            if self.discard_selected == 0 {
                row.push(t.cancel.bold().inverse(), "  Discard  ");
            } else {
                row.push(t.hint, "  Discard  ");
            }
            row.raw("   ");
            if self.discard_selected == 1 {
                row.push(t.confirm.bold().inverse(), "  Keep & Close  ");
            } else {
                row.push(t.hint, "  Keep & Close  ");
            }
            lines.push(self.row(inner_w, &row));
        } else if self.auth_only {
            lines.push(self.row(
                inner_w,
                &StyledText::styled(t.description, "select a server to authenticate"),
            ));
        } else {
            let direct_count: usize =
                self.servers.iter().map(|s| s.tools.iter().filter(|t| t.is_direct).count()).sum();
            let total_tokens: usize = self
                .servers
                .iter()
                .map(|s| {
                    s.tools.iter().filter(|t| t.is_direct).map(|t| t.estimated_tokens).sum::<usize>()
                })
                .sum();
            let stats = if direct_count > 0 {
                format!("{direct_count} direct  ~{} tokens", to_locale_string(total_tokens))
            } else {
                "no direct tools".to_string()
            };
            let mut row = StyledText::new();
            row.push(t.description, stats);
            if self.dirty {
                row.push(t.needs_auth, "  (unsaved)");
            }
            lines.push(self.row(inner_w, &row));
        }

        // 9 — a blank row.
        lines.push(self.empty_row(inner_w));

        // 10 — the hint bar, greedily wrapped at `innerW - 2`.
        for line in self.hint_rows(inner_w) {
            lines.push(self.row(inner_w, &line));
        }

        // 11 — the bottom border.
        lines.push(OverlayLine::new(vec![t.border.span(format!(
            "\u{2570}{}\u{256f}",
            "\u{2500}".repeat(inner_w)
        ))]));

        lines
    }

    /// The hint list and its greedy wrap (13h §1.11 step 10).
    fn hint_rows(&self, inner_w: usize) -> Vec<StyledText> {
        let t = self.theme;
        let save_label = self.keys.save_label().map(str::to_string);
        let mut hints: Vec<(String, String)> = Vec::new();
        if self.auth_only {
            hints.push(("\u{2191}\u{2193}".into(), " navigate".into()));
            hints.push(("\u{23ce}".into(), " auth".into()));
            hints.push(("ctrl+a".into(), " auth".into()));
            hints.push(("esc".into(), " clear/close".into()));
            hints.push(("ctrl+c".into(), " quit".into()));
        } else {
            hints.push(("\u{2191}\u{2193}".into(), " navigate".into()));
            hints.push(("space".into(), " toggle".into()));
            hints.push(("\u{23ce}".into(), " expand/auth".into()));
            hints.push(("ctrl+a".into(), " auth".into()));
            hints.push(("ctrl+r".into(), " reconnect".into()));
            if self.selected_server_has_failure_message() {
                hints.push(("ctrl+y".into(), " copy error".into()));
            }
            hints.push(("?".into(), " desc search".into()));
            if let Some(label) = save_label {
                hints.push((label, " save".into()));
            }
            hints.push(("esc".into(), " clear/close".into()));
            hints.push(("ctrl+c".into(), " quit".into()));
        }

        let gap_w = 2usize;
        let max_w = inner_w.saturating_sub(2);
        let mut rows: Vec<StyledText> = Vec::new();
        let mut current = StyledText::new();
        let mut cur_w = 0usize;
        for (key, label) in hints {
            let hw = visible_width(&key) + visible_width(&label);
            let needed = if cur_w == 0 { hw } else { gap_w + hw };
            if cur_w > 0 && cur_w + needed > max_w {
                rows.push(std::mem::take(&mut current));
                current.push(t.hint.italic(), key).push(t.hint, label);
                cur_w = hw;
            } else {
                if cur_w > 0 {
                    current.push(t.hint, "  ");
                }
                current.push(t.hint.italic(), key).push(t.hint, label);
                cur_w += needed;
            }
        }
        if !current.spans.is_empty() {
            rows.push(current);
        }
        rows
    }

    /// `renderServerRow(server, isCursor)` (13h §1.12).
    ///
    /// Note the two different fallbacks for a missing import kind: the row label falls back to
    /// `"import"`, while [`Self::import_notice_text`]'s notice falls back to `"external"`.
    fn render_server_row(&self, server: &ServerState, is_cursor: bool) -> StyledText {
        let t = self.theme;
        let expand_icon = if server.expanded { "\u{25be}" } else { "\u{25b8}" };
        let mut row = StyledText::new();
        if is_cursor {
            row.push(t.selected, expand_icon);
        } else {
            row.push(t.border, if server.expanded { expand_icon } else { "\u{b7}" });
        }

        let server_name = sanitize_display_text(Some(&server.name));
        let import_kind =
            sanitize_display_text(Some(server.import_kind.as_deref().unwrap_or("import")));
        let name_style = if is_cursor { t.selected.bold() } else { Style::plain() };
        let status_label = self.render_connection_status(server);

        if !server.has_cached_data && !self.auth_only {
            row.raw("   ");
            row.push(name_style, server_name);
            if server.source == SourceKind::Import {
                row.push(t.description, format!(" ({import_kind})"));
            }
            row.raw("  ");
            row.push(t.description, "(not cached)");
            row.extend(status_label);
            return row;
        }

        let direct_count = server.tools.iter().filter(|t| t.is_direct).count();
        let total_count = server.tools.len();
        let (toggle_style, toggle_icon) = if direct_count == total_count && total_count > 0 {
            (t.direct, "\u{25cf}")
        } else if direct_count > 0 {
            (t.needs_auth, "\u{25d0}")
        } else {
            (t.description, "\u{25cb}")
        };

        row.raw(" ");
        row.push(toggle_style, toggle_icon);
        row.raw(" ");
        row.push(name_style, server_name);
        if server.source == SourceKind::Import {
            row.push(t.description, format!(" ({import_kind})"));
        }
        row.raw("  ");
        if total_count > 0 {
            let mut tool_info = format!("{direct_count}/{total_count}");
            if direct_count > 0 {
                let tokens: usize = server
                    .tools
                    .iter()
                    .filter(|t| t.is_direct)
                    .map(|t| t.estimated_tokens)
                    .sum();
                tool_info.push_str(&format!("  ~{}", to_locale_string(tokens)));
            }
            row.push(t.description, tool_info);
        }
        row.extend(status_label);
        row
    }

    /// `renderConnectionStatus(server)` — first match wins, each with a **two-space** prefix. In
    /// normal (non-`authOnly`) mode a connected or idle server shows **no** status label at all.
    fn render_connection_status(&self, server: &ServerState) -> StyledText {
        let t = self.theme;
        let mut out = StyledText::new();
        let (style, text) = if self.auth_in_flight.as_deref() == Some(server.name.as_str()) {
            (t.needs_auth, "authenticating")
        } else if server.connection_status == ConnectionStatus::Disabled {
            (t.description, "disabled")
        } else if server.connection_status == ConnectionStatus::NeedsAuth {
            (t.needs_auth, "needs auth")
        } else if server.connection_status == ConnectionStatus::Connecting {
            (t.needs_auth, "connecting")
        } else if server.connection_status == ConnectionStatus::Failed {
            (t.cancel, "failed")
        } else if self.auth_only && server.connection_status == ConnectionStatus::Connected {
            (t.direct, "connected")
        } else if self.auth_only {
            (t.description, "idle")
        } else {
            return out;
        };
        out.raw("  ");
        out.push(style, text);
        out
    }

    /// `renderToolRow(tool, isCursor, innerW)` — `prefixLen = 7 + width(name)`,
    /// `maxDescLen = max(0, innerW - prefixLen - 8)`, and the description is rendered **only when
    /// `maxDescLen > 5`** and non-empty.
    fn render_tool_row(&self, tool: &ToolState, is_cursor: bool, inner_w: usize) -> StyledText {
        let t = self.theme;
        let tool_name = sanitize_display_text(Some(&tool.name));
        let description = sanitize_display_text(Some(&tool.description));
        let prefix_len = 7 + visible_width(&tool_name);
        let max_desc_len = inner_w.saturating_sub(prefix_len).saturating_sub(8);

        let mut row = StyledText::new();
        row.raw("  ");
        if is_cursor {
            row.push(t.selected, "\u{25b8}");
        } else {
            row.raw(" ");
        }
        row.raw(" ");
        if tool.is_direct {
            row.push(t.direct, "\u{25cf}");
        } else {
            row.push(t.description, "\u{25cb}");
        }
        row.raw(" ");
        row.push(if is_cursor { t.selected.bold() } else { Style::plain() }, tool_name);
        row.raw(" ");
        if max_desc_len > 5 && !description.is_empty() {
            row.push(
                t.description,
                format!(
                    "\u{2014} {}",
                    truncate_to_width(&description, max_desc_len, "\u{2026}", false)
                ),
            );
        }
        row
    }
}

// =================================================================================================
// 6 · The clipboard (MCP-361) and the path opener (MCP-372) — both extension-owned
// =================================================================================================

/// The clipboard writers, in the order `cyrup-tui`'s own `copy_to_clipboard` tries them: first that
/// **spawns** wins.
#[cfg(unix)]
const CLIPBOARD_COMMANDS: [(&str, &[&str]); 3] =
    [("pbcopy", &[]), ("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];
/// Windows' built-in `clip`. `cyrup-tui`'s helper is `#[cfg(unix)]` and silently no-ops here, which
/// would tell a Windows user "Copied error to clipboard" over an empty clipboard.
#[cfg(windows)]
const CLIPBOARD_COMMANDS: [(&str, &[&str]); 1] = [("clip", &[])];
/// Nothing to try.
#[cfg(not(any(unix, windows)))]
const CLIPBOARD_COMMANDS: [(&str, &[&str]); 0] = [];

/// `ctrl+y`'s clipboard write (MCP-361).
///
/// Reimplemented here rather than reused from `cyrup-tui` for two reasons the in-tree helper cannot
/// satisfy: it must **return** the spawn/write error, because upstream's
/// `Failed to copy error for {name}: {msg}` notice has no other source, and it must implement the
/// non-unix arm, which in `cyrup-tui` is a silent no-op. `arboard` is deliberately not used: in this
/// tree it exists only for clipboard *image read*, and a second text-write path would behave
/// differently from `/copy`'s.
pub async fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let mut spawn_errors: Vec<String> = Vec::new();
    for (program, args) in CLIPBOARD_COMMANDS {
        let mut child = match tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                spawn_errors.push(format!("{program}: {error}"));
                continue;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|error| format!("{program}: {error}"))?;
            stdin.shutdown().await.map_err(|error| format!("{program}: {error}"))?;
            drop(stdin);
        }
        let status = child.wait().await.map_err(|error| format!("{program}: {error}"))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("{program} exited with {status}"))
        };
    }
    Err(if spawn_errors.is_empty() {
        "no clipboard command is available on this platform".to_string()
    } else {
        format!("no clipboard command could be started ({})", spawn_errors.join("; "))
    })
}

/// `utils.ts` `openPath(pi, targetPath)` over `execOpen` (MCP-372).
///
/// The platform dispatch is ported literally rather than delegated to the `opener` crate:
/// `execOpen` also carries a `$BROWSER` override and abort support and is shared with `openUrl`, so
/// collapsing it here would lose a mechanism the other call site needs. The error text is upstream's
/// `stderr || "Failed to open path (exit code {code})"`.
pub async fn open_path(target: &Path) -> Result<(), String> {
    let display = target.display().to_string();
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<String>) = ("open", vec![display.clone()]);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<String>) =
        ("cmd", vec!["/c".to_string(), "start".to_string(), String::new(), display.clone()]);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (program, args): (&str, Vec<String>) = ("xdg-open", vec![display.clone()]);

    let output = tokio::process::Command::new(program)
        .args(&args)
        .output()
        .await
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!(
            "Failed to open path (exit code {})",
            output.status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
        )
    } else {
        stderr
    })
}

// =================================================================================================
// 7 · `McpPanelOverlay` — the host adapter (MCP-359, MCP-360, MCP-362, MCP-369)
// =================================================================================================

/// An in-flight async job started from a synchronous keystroke handler, polled on the next tick.
type Pending = Option<(PanelJob, tokio::sync::oneshot::Receiver<PanelJobResult>)>;

/// The `/mcp` panel wired to a live terminal through [`InteractiveOverlay`].
///
/// Mirrors `FleetOverlay` (`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`) exactly: a
/// captured `tokio::runtime::Handle`, one `oneshot::Receiver` per job, `try_recv` in [`Self::tick`],
/// and a `TryRecvError::Closed` arm that **still settles the job** — a dropped sender that left
/// `auth_in_flight` raised would make every later authentication silently refuse.
pub struct McpPanelOverlay {
    model: McpPanelModel,
    callbacks: Arc<dyn McpPanelCallbacks>,
    handle: tokio::runtime::Handle,
    job: Pending,
    deadline: Instant,
    expired: bool,
    out: Arc<Mutex<Option<McpPanelResult>>>,
}

impl McpPanelOverlay {
    /// Wrap a constructed model.
    ///
    /// `out` is how [`McpPanelResult`] escapes an `open_overlay` that returns only `bool`
    /// (MCP-369): the caller keeps a clone, the overlay writes through it on the close path, and the
    /// block-until-closed contract of `LiveHostServices::open_overlay` supplies the happens-before.
    /// Do **not** add a result type to the seam — a typed return would have to be `Value`-erased.
    #[must_use]
    pub fn new(
        model: McpPanelModel,
        callbacks: Arc<dyn McpPanelCallbacks>,
        handle: tokio::runtime::Handle,
        out: Arc<Mutex<Option<McpPanelResult>>>,
    ) -> Self {
        Self {
            model,
            callbacks,
            handle,
            job: None,
            deadline: Instant::now() + Duration::from_millis(INACTIVITY_MS),
            expired: false,
            out,
        }
    }

    /// The driven model, for tests and for a caller that wants to inspect live state.
    #[must_use]
    pub fn model(&self) -> &McpPanelModel {
        &self.model
    }

    /// `resetInactivityTimeout()` — re-armed on **every** keystroke.
    fn touch(&mut self) {
        self.deadline = Instant::now() + Duration::from_millis(INACTIVITY_MS);
    }

    fn publish(&self) {
        if let Some(result) = self.model.result()
            && let Ok(mut slot) = self.out.lock()
        {
            *slot = Some(result.clone());
        }
    }

    /// Start one job on the captured runtime (`FleetOverlay::spawn_action`).
    ///
    /// The spawned future owns everything it touches — an `Arc` to the callbacks, never a borrow of
    /// the panel — so a teardown that drops the overlay mid-flight cannot leave a dangling task.
    fn spawn_job(&mut self, job: PanelJob) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let callbacks = Arc::clone(&self.callbacks);
        let spawned = job.clone();
        self.handle.spawn(async move {
            let result = match spawned {
                PanelJob::Authenticate(server) => {
                    let outcome = callbacks.authenticate(server.clone()).await;
                    PanelJobResult::Authenticated { server, outcome }
                }
                PanelJob::Reconnect { server, after_auth } => {
                    let outcome = callbacks.reconnect(server.clone()).await;
                    PanelJobResult::Reconnected { server, after_auth, outcome }
                }
                PanelJob::CopyError { server, text } => {
                    let outcome = copy_to_clipboard(&text).await;
                    PanelJobResult::Copied { server, outcome }
                }
            };
            let _ = tx.send(result);
        });
        self.job = Some((job, rx));
    }

    /// The rejection a dropped sender stands in for — upstream's `.catch(...)` arm.
    fn cancelled_result(job: &PanelJob) -> PanelJobResult {
        let message = "The MCP panel action was cancelled.".to_string();
        match job {
            PanelJob::Authenticate(server) => {
                PanelJobResult::Authenticated { server: server.clone(), outcome: Err(message) }
            }
            PanelJob::Reconnect { server, after_auth } => PanelJobResult::Reconnected {
                server: server.clone(),
                after_auth: *after_auth,
                outcome: Err(message),
            },
            PanelJob::CopyError { server, .. } => {
                PanelJobResult::Copied { server: server.clone(), outcome: Err(message) }
            }
        }
    }

    /// Poll the in-flight job; `true` when the frame would differ.
    fn drain_job(&mut self) -> bool {
        let Some((job, rx)) = self.job.as_mut() else { return false };
        let settled = match rx.try_recv() {
            Ok(result) => result,
            // The task vanished without answering. Settling it anyway is what clears the busy latch
            // — the exact hazard `FleetOverlay`'s drain documents.
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => Self::cancelled_result(job),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
        };
        self.job = None;
        if let Some(next) = self.model.finish_job(settled) {
            self.spawn_job(next);
        }
        true
    }
}

impl InteractiveOverlay for McpPanelOverlay {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        self.model.render(width)
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        // The inactivity deadline has passed and `tick` had no way to say so; honour it on the
        // first key rather than resurrecting a panel the user was told would close itself.
        if self.expired {
            self.publish();
            return OverlayOutcome::Close;
        }
        self.touch();
        match self.model.handle_key(key) {
            PanelInputOutcome::Ignored => OverlayOutcome::Ignored,
            PanelInputOutcome::Redraw => OverlayOutcome::Redraw,
            PanelInputOutcome::Close => {
                self.publish();
                OverlayOutcome::Close
            }
            PanelInputOutcome::Run(job) => {
                self.spawn_job(job);
                OverlayOutcome::Redraw
            }
        }
    }

    fn refresh_ms(&self) -> u64 {
        REFRESH_MS
    }

    fn tick(&mut self) -> bool {
        let changed = self.drain_job();
        // TODO(MCP-362): the one place the seam is genuinely short: `tick` returns `bool`, so an
        // overlay cannot ask the host to close itself. Upstream's `setTimeout` fires at exactly
        // 60 s and calls `done({cancelled: true, changes: {}})` with the panel still on screen. The
        // best available approximation is to publish the cancelled result at the deadline and close
        // on the next keystroke; the residue is that an untouched panel stays painted. Closing it
        // needs `tick` to be able to return an `OverlayOutcome` (or a `should_close` companion) —
        // recorded as a host addition, not absorbed.
        if !self.expired && Instant::now() >= self.deadline {
            self.expired = true;
            self.model.expire();
            self.publish();
            return true;
        }
        changed
    }
}

// =================================================================================================
// 8 · `McpSetupPanel` (13h §3) — the onboarding/config-writing flow
// =================================================================================================

/// `mcp-setup-panel.ts`'s `Screen`. The action list's *length* changes with this, so the cursor
/// index means different things on different screens — which is why [`McpSetupPanelModel::actions`]
/// is rebuilt on every call and never cached into a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupScreen {
    /// Nothing is configured yet.
    Empty,
    /// The main action menu.
    Setup,
    /// The compatibility-import chooser.
    Imports,
    /// The detected-path browser.
    Paths,
}

/// `mcp-setup-panel.ts`'s `ActionId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupActionId {
    /// Move from `empty` to `setup`.
    RunSetup,
    /// Open the imports sub-screen.
    AdoptImports,
    /// A read-only preview of a working shared config.
    ViewExample,
    /// Explain the read order.
    ShowPrecedence,
    /// Open the paths sub-screen.
    OpenPaths,
    /// Quick-add RepoPrompt.
    AddRepoPrompt,
    /// Quick-add one of the five curated servers.
    AddKnownServer,
    /// Write a minimal project config.
    ScaffoldProject,
    /// Leave.
    Close,
}

/// One row of the action list, plus the preset an `add-known-server` row carries.
#[derive(Clone, Debug)]
pub struct SetupAction {
    /// Which action.
    pub id: SetupActionId,
    /// The row text.
    pub label: String,
    /// Read from the action but **never rendered as a row** — the preview block is what the user
    /// reads. Kept because upstream keeps it, and because it is what a future compact layout would
    /// show.
    pub description: String,
    /// Present only on [`SetupActionId::AddKnownServer`].
    pub preset: Option<KnownServerPreset>,
}

/// The tone of the panel's one-line notice. `Muted` renders with the **hint** colour, not the muted
/// one — upstream's own mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeTone {
    /// A completed write.
    Success,
    /// A refusal or an error.
    Warning,
    /// Progress and read-only actions.
    Muted,
}

/// `ensureCompatibilityImports`' result, as the panel reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptImportsOutcome {
    /// What was actually added; **empty means nothing was written**.
    pub added: Vec<ImportKind>,
    /// The adapter-owned file the write targeted.
    pub path: PathBuf,
}

/// `writeSharedServerEntry`'s result. Note `server_name` is the preset's **display name**, while the
/// key written into the file is its `id` — adding "Chrome DevTools" writes `"chrome-devtools"` and
/// notices `Added Chrome DevTools to ...` (MCP-379).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddServerOutcome {
    /// The file written.
    pub path: PathBuf,
    /// What the success notice reports.
    pub server_name: String,
}

/// `SetupPanelCallbacks` (`mcp-setup-panel.ts`).
///
/// The four `preview_*` members are **synchronous and are invoked from inside `render`**, on every
/// frame, exactly as upstream's are: they re-read config files from disk each time. Ported
/// literally; coupled with a non-zero [`REFRESH_MS`] (which upstream does not have) that means an
/// *idle* panel re-reads the config four times a second where upstream re-reads it only on a
/// keystroke. That amplification is the poll-repaint residue, filed rather than cached away
/// (MCP-375).
pub trait SetupPanelCallbacks: Send + Sync + 'static {
    /// `previewImports(imports)`.
    fn preview_imports(&self, imports: &[ImportKind]) -> ConfigWritePreview;
    /// `previewStarterProject()`.
    fn preview_starter_project(&self) -> ConfigWritePreview;
    /// `previewRepoPrompt()` — `None` when RepoPrompt cannot be offered.
    fn preview_repo_prompt(&self) -> Option<ConfigWritePreview>;
    /// `previewKnownServer(preset)`.
    fn preview_known_server(&self, preset: &KnownServerPreset) -> ConfigWritePreview;
    /// `adoptImports(imports)`.
    fn adopt_imports(
        &self,
        imports: Vec<ImportKind>,
    ) -> futures::future::BoxFuture<'static, Result<AdoptImportsOutcome, String>>;
    /// `scaffoldProjectConfig()`.
    fn scaffold_project_config(
        &self,
    ) -> futures::future::BoxFuture<'static, Result<PathBuf, String>>;
    /// `addRepoPrompt()`.
    fn add_repo_prompt(
        &self,
    ) -> futures::future::BoxFuture<'static, Result<AddServerOutcome, String>>;
    /// `addKnownServer(preset)`.
    fn add_known_server(
        &self,
        preset: KnownServerPreset,
    ) -> futures::future::BoxFuture<'static, Result<AddServerOutcome, String>>;
    /// `openPath(targetPath)`.
    fn open_path(&self, path: PathBuf) -> futures::future::BoxFuture<'static, Result<(), String>>;
    /// `markSetupCompleted()` — called after **every** successful write.
    fn mark_setup_completed(&self);
}

/// One async action the busy latch is raised for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupJob {
    /// `applySelectedImports()`.
    AdoptImports(Vec<ImportKind>),
    /// `scaffoldProjectConfig()`.
    ScaffoldProject,
    /// `addRepoPrompt()`.
    AddRepoPrompt,
    /// `addKnownServer(preset)`, by index into [`crate::config::known_server_presets`].
    AddKnownServer(usize),
    /// `openPath(path)`.
    OpenPath(PathBuf),
}

/// A settled [`SetupJob`].
#[derive(Clone, Debug)]
pub enum SetupJobResult {
    /// `adoptImports` settled.
    AdoptImports(Result<AdoptImportsOutcome, String>),
    /// `scaffoldProjectConfig` settled, carrying the written path.
    ScaffoldProject(Result<PathBuf, String>),
    /// `addRepoPrompt` settled.
    AddRepoPrompt(Result<AddServerOutcome, String>),
    /// `addKnownServer` settled.
    AddKnownServer(Result<AddServerOutcome, String>),
    /// `openPath` settled, carrying the path for the notice.
    OpenPath(PathBuf, Result<(), String>),
}

/// What one keystroke asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupInputOutcome {
    /// Nothing changed.
    Ignored,
    /// Repaint.
    Redraw,
    /// `done()`.
    Close,
    /// Start this job under the busy latch.
    Run(SetupJob),
}

/// `/mcp setup`'s state machine — pure, no tokio, no host.
pub struct McpSetupPanelModel {
    theme: SetupTheme,
    discovery: McpDiscoverySummary,
    onboarding: OnboardingState,
    callbacks: Arc<dyn SetupPanelCallbacks>,
    keys: PanelKeys,
    presets: Vec<KnownServerPreset>,

    screen: SetupScreen,
    action_cursor: usize,
    import_cursor: usize,
    path_cursor: usize,
    selected_imports: HashSet<ImportKind>,
    busy: bool,
    notice: Option<(String, NoticeTone)>,
    closed: bool,
}

/// `hostConfigDiscovery` as the word the two summary lines interpolate.
fn host_config_discovery_word(value: crate::config::HostConfigDiscovery) -> &'static str {
    match value {
        crate::config::HostConfigDiscovery::Off => "off",
        crate::config::HostConfigDiscovery::Prompt => "prompt",
        crate::config::HostConfigDiscovery::On => "on",
    }
}

impl McpSetupPanelModel {
    /// `new McpSetupPanel(discovery, callbacks, options, tui, done)`.
    ///
    /// `selectedImports` is pre-seeded with **every** detected import, so the default action on the
    /// imports screen is "adopt everything found".
    #[must_use]
    pub fn new(
        discovery: McpDiscoverySummary,
        onboarding: OnboardingState,
        callbacks: Arc<dyn SetupPanelCallbacks>,
        mode: SetupScreen,
        keys: PanelKeys,
    ) -> Self {
        let selected_imports = discovery.imports.iter().map(|entry| entry.kind).collect();
        Self {
            theme: SetupTheme::default(),
            discovery,
            onboarding,
            callbacks,
            keys,
            presets: crate::config::known_server_presets(),
            screen: mode,
            action_cursor: 0,
            import_cursor: 0,
            path_cursor: 0,
            selected_imports,
            busy: false,
            notice: None,
            closed: false,
        }
    }

    /// `getActions()` (13h §3.1) — rebuilt on **every** call, never cached.
    #[must_use]
    pub fn actions(&self) -> Vec<SetupAction> {
        let mut actions: Vec<SetupAction> = Vec::new();
        if self.screen == SetupScreen::Empty {
            actions.push(SetupAction {
                id: SetupActionId::RunSetup,
                label: "Run setup".into(),
                description:
                    "Inspect detected configs, adopt imports, and scaffold a minimal `.mcp.json`."
                        .into(),
                preset: None,
            });
        }
        let import_count = self.discovery.imports.len();
        if import_count > 0 {
            actions.push(SetupAction {
                id: SetupActionId::AdoptImports,
                label: "Adopt detected compatibility imports".into(),
                description: format!(
                    "Choose which host-specific MCP configs Pi should import into its own override file. {import_count} source{} found.",
                    if import_count == 1 { "" } else { "s" }
                ),
                preset: None,
            });
        }
        actions.push(SetupAction {
            id: SetupActionId::ViewExample,
            label: "View example `.mcp.json`".into(),
            description: "Preview a working shared MCP config you can paste or adapt.".into(),
            preset: None,
        });
        if !self
            .discovery
            .sources
            .iter()
            .any(|source| source.id == SourceId::SharedProject && source.exists)
        {
            actions.push(SetupAction {
                id: SetupActionId::ScaffoldProject,
                label: "Scaffold project `.mcp.json`".into(),
                description:
                    "Write a minimal project config using the standard shared MCP file path, then reload Pi."
                        .into(),
                preset: None,
            });
        }
        actions.push(SetupAction {
            id: SetupActionId::ShowPrecedence,
            label: "Explain config precedence".into(),
            description: "Show the read order and where Pi writes compatibility settings.".into(),
            preset: None,
        });
        if !self.detected_paths().is_empty() {
            actions.push(SetupAction {
                id: SetupActionId::OpenPaths,
                label: "Open detected config paths".into(),
                description: "Browse the actual config files that Pi discovered on this machine."
                    .into(),
                preset: None,
            });
        }
        for preset in &self.presets {
            actions.push(SetupAction {
                id: SetupActionId::AddKnownServer,
                label: preset.name.to_string(),
                description: preset.summary.to_string(),
                preset: Some(preset.clone()),
            });
        }
        let repo = &self.discovery.repo_prompt;
        if !repo.configured
            && repo.executable_path.is_some()
            && repo.target_path.is_some()
            && repo.entry.is_some()
            && repo.server_name.is_some()
        {
            actions.push(SetupAction {
                id: SetupActionId::AddRepoPrompt,
                label: "Add RepoPrompt to shared MCP config".into(),
                description:
                    "Write a standard MCP entry for RepoPrompt to the recommended shared target, then reload MCP in-session."
                        .into(),
                preset: None,
            });
        }
        actions.push(SetupAction {
            id: SetupActionId::Close,
            label: "Close".into(),
            description: "Exit the onboarding flow.".into(),
            preset: None,
        });
        actions
    }

    /// `getDetectedPaths()` — existing sources first, then every import, **deduplicated preserving
    /// first-seen order**.
    #[must_use]
    pub fn detected_paths(&self) -> Vec<PathBuf> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out: Vec<PathBuf> = Vec::new();
        for source in self.discovery.sources.iter().filter(|s| s.exists) {
            if seen.insert(source.path.clone()) {
                out.push(source.path.clone());
            }
        }
        for entry in &self.discovery.imports {
            if seen.insert(entry.path.clone()) {
                out.push(entry.path.clone());
            }
        }
        out
    }

    fn selected_action(&self) -> Option<SetupAction> {
        self.actions().get(self.action_cursor).cloned()
    }

    /// The current screen.
    #[must_use]
    pub fn screen(&self) -> SetupScreen {
        self.screen
    }

    /// Whether an async write is in flight.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The current notice, if any.
    #[must_use]
    pub fn notice(&self) -> Option<(&str, NoticeTone)> {
        self.notice.as_ref().map(|(text, tone)| (text.as_str(), *tone))
    }

    /// Whether `done()` has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// `handleInput(data)` (13h §3.2).
    ///
    /// The order is load-bearing in one specific way: `busy` is checked **after** `ctrl+c` and
    /// `escape`, so both close the panel while an async write is still running. In JS the settled
    /// promise then writes to a dead object harmlessly; here the spawned task owns everything it
    /// touches and its result is discarded when the receiver is gone.
    pub fn handle_key(&mut self, key: OverlayKey) -> SetupInputOutcome {
        if !self.busy {
            self.notice = None;
        }

        if is_ctrl_c(&key) {
            self.closed = true;
            return SetupInputOutcome::Close;
        }

        if is_escape(&key) {
            if matches!(self.screen, SetupScreen::Imports | SetupScreen::Paths) {
                // Back to the *root* screen, not to the one you came from.
                self.screen = if self.discovery.has_any_config {
                    SetupScreen::Setup
                } else {
                    SetupScreen::Empty
                };
                return SetupInputOutcome::Redraw;
            }
            self.closed = true;
            return SetupInputOutcome::Close;
        }

        if self.busy {
            return SetupInputOutcome::Ignored;
        }

        match self.screen {
            SetupScreen::Imports => return self.handle_imports_key(key),
            SetupScreen::Paths => return self.handle_paths_key(key),
            SetupScreen::Empty | SetupScreen::Setup => {}
        }

        let actions = self.actions();
        if self.keys.select_up(&key) {
            self.action_cursor = self.action_cursor.saturating_sub(1);
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_down(&key) {
            self.action_cursor =
                (self.action_cursor + 1).min(actions.len().saturating_sub(1));
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_confirm(&key) {
            if let Some(action) = self.selected_action() {
                return self.run_action(&action);
            }
            return SetupInputOutcome::Ignored;
        }
        SetupInputOutcome::Ignored
    }

    /// `handleImportsInput(data)` (13h §3.3).
    fn handle_imports_key(&mut self, key: OverlayKey) -> SetupInputOutcome {
        let count = self.discovery.imports.len();
        if self.keys.select_up(&key) {
            self.import_cursor = self.import_cursor.saturating_sub(1);
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_down(&key) {
            self.import_cursor = (self.import_cursor + 1).min(count.saturating_sub(1));
            return SetupInputOutcome::Redraw;
        }
        if is_space(&key) {
            let Some(current) = self.discovery.imports.get(self.import_cursor) else {
                return SetupInputOutcome::Ignored;
            };
            let kind = current.kind;
            if !self.selected_imports.remove(&kind) {
                self.selected_imports.insert(kind);
            }
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_confirm(&key) {
            return self.apply_selected_imports();
        }
        SetupInputOutcome::Ignored
    }

    /// `handlePathsInput(data)` (13h §3.4).
    fn handle_paths_key(&mut self, key: OverlayKey) -> SetupInputOutcome {
        let paths = self.detected_paths();
        if self.keys.select_up(&key) {
            self.path_cursor = self.path_cursor.saturating_sub(1);
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_down(&key) {
            self.path_cursor = (self.path_cursor + 1).min(paths.len().saturating_sub(1));
            return SetupInputOutcome::Redraw;
        }
        if self.keys.select_confirm(&key) {
            let Some(selected) = paths.get(self.path_cursor).cloned() else {
                return SetupInputOutcome::Ignored;
            };
            return self.run_busy(SetupJob::OpenPath(selected));
        }
        SetupInputOutcome::Ignored
    }

    /// `applySelectedImports()` — the selected kinds are taken in `discovery.imports` order, **not**
    /// selection order: the set is a membership test only.
    fn apply_selected_imports(&mut self) -> SetupInputOutcome {
        let selected: Vec<ImportKind> = self
            .discovery
            .imports
            .iter()
            .filter(|entry| self.selected_imports.contains(&entry.kind))
            .map(|entry| entry.kind)
            .collect();
        if selected.is_empty() {
            self.notice = Some((
                "Select at least one compatibility import first.".to_string(),
                NoticeTone::Warning,
            ));
            return SetupInputOutcome::Redraw;
        }
        self.run_busy(SetupJob::AdoptImports(selected))
    }

    /// `runBusy(fn)`'s synchronous half: raise the latch, set the `Working...` notice, hand the job
    /// out. The settle happens in [`Self::finish_job`].
    fn run_busy(&mut self, job: SetupJob) -> SetupInputOutcome {
        self.busy = true;
        self.notice = Some(("Working...".to_string(), NoticeTone::Muted));
        SetupInputOutcome::Run(job)
    }

    /// `runAction(action)` (13h §3.5).
    fn run_action(&mut self, action: &SetupAction) -> SetupInputOutcome {
        match action.id {
            SetupActionId::RunSetup => {
                self.screen = SetupScreen::Setup;
                self.action_cursor = 0;
                SetupInputOutcome::Redraw
            }
            SetupActionId::AdoptImports => {
                self.screen = SetupScreen::Imports;
                self.import_cursor = 0;
                SetupInputOutcome::Redraw
            }
            SetupActionId::OpenPaths => {
                self.screen = SetupScreen::Paths;
                self.path_cursor = 0;
                SetupInputOutcome::Redraw
            }
            SetupActionId::ScaffoldProject => self.run_busy(SetupJob::ScaffoldProject),
            SetupActionId::AddRepoPrompt => self.run_busy(SetupJob::AddRepoPrompt),
            SetupActionId::AddKnownServer => match action.preset.as_ref() {
                Some(preset) => {
                    match self.presets.iter().position(|p| p.id == preset.id) {
                        Some(index) => self.run_busy(SetupJob::AddKnownServer(index)),
                        // A preset the list no longer carries falls through to the read-only arm,
                        // exactly as a preset-less `add-known-server` does upstream.
                        None => self.review_notice(),
                    }
                }
                None => self.review_notice(),
            },
            SetupActionId::Close => {
                self.closed = true;
                SetupInputOutcome::Close
            }
            // `view-example`, `show-precedence` and a preset-less `add-known-server`.
            SetupActionId::ViewExample | SetupActionId::ShowPrecedence => self.review_notice(),
        }
    }

    fn review_notice(&mut self) -> SetupInputOutcome {
        self.notice = Some((
            "Review the details below. Press Enter on an action with a side effect to apply it."
                .to_string(),
            NoticeTone::Muted,
        ));
        SetupInputOutcome::Redraw
    }

    /// `runBusy`'s settle half, including `markSetupCompleted()` after **every** successful write
    /// and the three identical success-notice texts (MCP-374).
    ///
    /// The `finally` clause is unconditional: `busy` is cleared on every arm, which is what a
    /// dropped sender must also reach or the panel refuses every later action until it is closed.
    pub fn finish_job(&mut self, result: SetupJobResult) {
        match result {
            SetupJobResult::AdoptImports(Ok(outcome)) => {
                self.callbacks.mark_setup_completed();
                self.notice = Some(if outcome.added.is_empty() {
                    (
                        format!("No changes needed in {}.", outcome.path.display()),
                        NoticeTone::Muted,
                    )
                } else {
                    let added: Vec<&str> =
                        outcome.added.iter().map(|kind| kind.as_str()).collect();
                    (
                        format!(
                            "Added {} to {}. Pi will reload after this panel closes.",
                            added.join(", "),
                            outcome.path.display()
                        ),
                        NoticeTone::Success,
                    )
                });
                self.screen = if self.discovery.has_any_config {
                    SetupScreen::Setup
                } else {
                    SetupScreen::Empty
                };
                self.action_cursor = 0;
            }
            SetupJobResult::ScaffoldProject(Ok(path)) => {
                self.callbacks.mark_setup_completed();
                self.notice = Some((
                    format!(
                        "Wrote starter config to {}. Pi will reload after this panel closes.",
                        path.display()
                    ),
                    NoticeTone::Success,
                ));
            }
            SetupJobResult::AddRepoPrompt(Ok(outcome))
            | SetupJobResult::AddKnownServer(Ok(outcome)) => {
                self.callbacks.mark_setup_completed();
                self.notice = Some((
                    format!(
                        "Added {} to {}. Pi will reload after this panel closes.",
                        outcome.server_name,
                        outcome.path.display()
                    ),
                    NoticeTone::Success,
                ));
            }
            SetupJobResult::OpenPath(path, Ok(())) => {
                self.notice =
                    Some((format!("Opened {}", path.display()), NoticeTone::Success));
            }
            SetupJobResult::AdoptImports(Err(error))
            | SetupJobResult::ScaffoldProject(Err(error))
            | SetupJobResult::AddRepoPrompt(Err(error))
            | SetupJobResult::AddKnownServer(Err(error))
            | SetupJobResult::OpenPath(_, Err(error)) => {
                self.notice = Some((error, NoticeTone::Warning));
            }
        }
        self.busy = false;
    }

    /// The inactivity auto-cancel: the setup panel calls a bare `done()`, so there is nothing to
    /// publish.
    pub fn expire(&mut self) {
        self.closed = true;
    }
}

// =================================================================================================
// 9 · The setup panel's frame (13h §3.6) and previews (§3.7)
// =================================================================================================

impl McpSetupPanelModel {
    /// `contentWidth(innerW)` = `max(8, innerW - 4)`.
    fn content_width(inner_w: usize) -> usize {
        inner_w.saturating_sub(4).max(8)
    }

    /// `previewWidth(innerW)` = `max(12, min(74, contentWidth(innerW)))`.
    fn preview_width(inner_w: usize) -> usize {
        Self::content_width(inner_w).clamp(12, DESKTOP_PREVIEW_WIDTH)
    }

    /// `padLine(text, innerW)`.
    ///
    /// The extra `max(0, contentW - visibleWidth(fitted))` spaces are redundant given the pad flag,
    /// and are kept because upstream has them: a helper that drops them is a helper that behaves
    /// differently the day `truncateToWidth`'s padding changes.
    fn pad_line(&self, inner_w: usize, content: &StyledText) -> OverlayLine {
        let content_w = inner_w.saturating_sub(4);
        let fitted = truncate_spans(&content.spans, content_w, "\u{2026}", true);
        let fitted_width: usize = fitted.iter().map(|s| visible_width(&s.text)).sum();
        let mut spans = vec![OverlaySpan::raw("\u{2502}"), OverlaySpan::raw("  ")];
        spans.extend(fitted);
        spans.push(OverlaySpan::raw(" ".repeat(content_w.saturating_sub(fitted_width))));
        spans.push(OverlaySpan::raw("  "));
        spans.push(OverlaySpan::raw("\u{2502}"));
        OverlayLine::new(spans)
    }

    fn pad_text(&self, inner_w: usize, style: Style, text: impl Into<String>) -> OverlayLine {
        self.pad_line(inner_w, &StyledText::styled(style, text))
    }

    /// `render(width)` (13h §3.6). The corners are **uncoloured** here, unlike `mcp-panel.ts`,
    /// where the whole border string is wrapped in the border colour.
    #[must_use]
    pub fn render(&self, width: usize) -> Vec<OverlayLine> {
        let panel_w = width.max(MIN_PANEL_WIDTH);
        let inner_w = panel_w.saturating_sub(2);
        let content_w = Self::content_width(inner_w);
        let t = self.theme;
        let mut lines: Vec<OverlayLine> = Vec::new();

        let border = |left: &str, right: &str| {
            OverlayLine::new(vec![
                OverlaySpan::raw(left),
                t.border.span("\u{2500}".repeat(inner_w)),
                OverlaySpan::raw(right),
            ])
        };

        lines.push(border("\u{250c}", "\u{2510}"));
        lines.push(self.pad_text(inner_w, t.title, "MCP setup"));

        let (summary_style, summary) = self.discovery_summary_line();
        for line in wrap_text_soft(&summary, content_w) {
            lines.push(self.pad_text(inner_w, summary_style, line));
        }
        for line in wrap_text_soft(&self.secondary_summary_line(), content_w) {
            lines.push(self.pad_text(inner_w, t.muted, line));
        }
        lines.push(self.pad_text(inner_w, Style::plain(), ""));

        if let Some((text, tone)) = self.notice() {
            // The `muted` tone renders with the **hint** colour, not the muted one.
            let style = match tone {
                NoticeTone::Success => t.success,
                NoticeTone::Warning => t.warning,
                NoticeTone::Muted => t.hint,
            };
            for line in wrap_text_soft(text, content_w) {
                lines.push(self.pad_text(inner_w, style, line));
            }
            lines.push(self.pad_text(inner_w, Style::plain(), ""));
        }

        lines.push(border("\u{251c}", "\u{2524}"));

        match self.screen {
            SetupScreen::Imports => lines.extend(self.render_imports(inner_w)),
            SetupScreen::Paths => lines.extend(self.render_paths(inner_w)),
            SetupScreen::Empty | SetupScreen::Setup => lines.extend(self.render_actions(inner_w)),
        }

        lines.push(border("\u{2514}", "\u{2518}"));
        lines
    }

    /// `visibleActionRange(total)` — the compact 7-row window (MCP-377).
    ///
    /// The compact branch **is** reachable: `ExtensionOverlay`'s `OVERLAY_MIN_WIDTH = 60` means
    /// `inner_w` bottoms out at 58, so any terminal narrower than about 63 columns takes it.
    fn visible_action_range(&self, total: usize) -> (usize, usize) {
        if total <= COMPACT_ACTION_ROWS {
            return (0, total);
        }
        let half = COMPACT_ACTION_ROWS / 2;
        let start = self
            .action_cursor
            .saturating_sub(half)
            .min(total.saturating_sub(COMPACT_ACTION_ROWS));
        (start, total.min(start + COMPACT_ACTION_ROWS))
    }

    /// `renderActions(innerW)`.
    ///
    /// The action list is **not** scroll-windowed at all when `inner_w >= 60`, and the preview block
    /// is never windowed — which, combined with the host's height clip, is where a long list is
    /// silently cut. Ported as upstream has it.
    ///
    // TODO(MCP-368, MCP-377): the height half of the geometry problem is the panel's, not the
    // host's — the seam's own contract calls an over-tall frame "lossless-by-design". Closing it
    // means windowing the action list at every width and bounding the preview against the frame
    // height `render` is handed, which changes what the user sees and therefore wants the live
    // terminal pass first. The width half (upstream's fixed 82/92 columns) is HA-3 and is the
    // host's: `open_overlay` takes no options bag.
    fn render_actions(&self, inner_w: usize) -> Vec<OverlayLine> {
        let t = self.theme;
        let mut lines: Vec<OverlayLine> = Vec::new();
        let actions = self.actions();
        let compact = inner_w < COMPACT_WIDTH;
        let (start, end) =
            if compact { self.visible_action_range(actions.len()) } else { (0, actions.len()) };

        if start > 0 {
            lines.push(self.pad_text(inner_w, t.muted, format!("\u{2026} {start} more above")));
        }
        for index in start..end {
            let Some(action) = actions.get(index) else { continue };
            // The heading is emitted when the preset is the first *visible* row, not only the first
            // row overall — which is what makes it interact correctly with the compact window.
            if action.id == SetupActionId::AddKnownServer
                && (index == start
                    || actions
                        .get(index.wrapping_sub(1))
                        .is_none_or(|prev| prev.id != SetupActionId::AddKnownServer))
            {
                lines.push(self.pad_text(inner_w, t.title, "Add a known server"));
            }
            let mut row = StyledText::new();
            if index == self.action_cursor {
                row.push(t.selected, "\u{203a}");
            } else {
                row.raw(" ");
            }
            row.raw(" ");
            row.raw(truncate_to_width(
                &action.label,
                Self::content_width(inner_w).saturating_sub(2),
                "...",
                false,
            ));
            lines.push(self.pad_line(inner_w, &row));
        }
        if end < actions.len() {
            lines.push(self.pad_text(
                inner_w,
                t.muted,
                format!("\u{2026} {} more below", actions.len() - end),
            ));
        }
        lines.push(self.pad_text(inner_w, Style::plain(), ""));

        for line in
            self.action_preview(self.selected_action().as_ref(), Self::preview_width(inner_w))
        {
            lines.push(self.pad_text(inner_w, Style::plain(), line));
        }
        lines.push(self.pad_text(inner_w, Style::plain(), ""));
        lines.push(self.pad_text(
            inner_w,
            t.muted,
            if compact {
                "Enter select \u{b7} Esc back"
            } else {
                "Enter selects, Esc goes back, Ctrl+C closes."
            },
        ));
        lines
    }

    /// `renderImports(innerW)` — the write preview is recomputed from the *currently selected* set
    /// on every frame.
    fn render_imports(&self, inner_w: usize) -> Vec<OverlayLine> {
        let t = self.theme;
        let mut lines = vec![
            self.pad_text(
                inner_w,
                Style::plain(),
                "Select compatibility imports. Space toggles, Enter saves, Esc goes back.",
            ),
            self.pad_text(inner_w, Style::plain(), ""),
        ];
        for (index, entry) in self.discovery.imports.iter().enumerate() {
            let mut row = StyledText::new();
            if index == self.import_cursor {
                row.push(t.selected, "\u{203a}");
            } else {
                row.raw(" ");
            }
            row.raw(format!(
                " {} {}  {}",
                if self.selected_imports.contains(&entry.kind) { "[x]" } else { "[ ]" },
                entry.kind.as_str(),
                entry.path.display()
            ));
            lines.push(self.pad_line(inner_w, &row));
        }
        lines.push(self.pad_text(inner_w, Style::plain(), ""));
        let selected: Vec<ImportKind> = self
            .discovery
            .imports
            .iter()
            .filter(|entry| self.selected_imports.contains(&entry.kind))
            .map(|entry| entry.kind)
            .collect();
        let preview = self.callbacks.preview_imports(&selected);
        for line in Self::format_write_preview(
            "Compatibility import write preview",
            &preview,
            &[],
            Self::preview_width(inner_w),
        ) {
            lines.push(self.pad_text(inner_w, Style::plain(), line));
        }
        lines
    }

    /// `renderPaths(innerW)` — **no preview block on this screen**.
    fn render_paths(&self, inner_w: usize) -> Vec<OverlayLine> {
        let t = self.theme;
        let mut lines = vec![
            self.pad_text(
                inner_w,
                Style::plain(),
                "Select a detected config path to open. Enter opens it, Esc goes back.",
            ),
            self.pad_text(inner_w, Style::plain(), ""),
        ];
        for (index, path) in self.detected_paths().iter().enumerate() {
            let mut row = StyledText::new();
            if index == self.path_cursor {
                row.push(t.selected, "\u{203a}");
            } else {
                row.raw(" ");
            }
            row.raw(format!(" {}", path.display()));
            lines.push(self.pad_line(inner_w, &row));
        }
        lines
    }

    /// `discoverySummaryLine()` — first match wins (MCP-378).
    #[must_use]
    pub fn discovery_summary_line(&self) -> (Style, String) {
        let t = self.theme;
        if !self.discovery.has_any_config {
            return (
                t.warning,
                if self.onboarding.setup_completed {
                    "No MCP servers are active right now.".to_string()
                } else {
                    "No MCP config is active yet.".to_string()
                },
            );
        }
        if self.discovery.total_server_count == 0
            && (!self.discovery.imports.is_empty()
                || self.discovery.repo_prompt.executable_path.is_some())
        {
            return (
                t.warning,
                "Pi found MCP-related setup options, but none are active in Pi yet.".to_string(),
            );
        }
        let shared = self
            .discovery
            .sources
            .iter()
            .filter(|s| s.kind == crate::config::DiscoveryKind::Shared && s.server_count > 0)
            .count();
        let pi_owned = self
            .discovery
            .sources
            .iter()
            .filter(|s| s.kind == crate::config::DiscoveryKind::Pi && s.server_count > 0)
            .count();
        (
            t.hint,
            format!(
                "Detected {} configured servers across {shared} shared and {pi_owned} Pi-owned source{}.",
                self.discovery.total_server_count,
                // The plural is suppressed only when the two counts sum to one; `configured
                // servers` is never singularised.
                if shared + pi_owned == 1 { "" } else { "s" }
            ),
        )
    }

    /// `secondarySummaryLine()` — three branches plus the two note suffixes (MCP-378).
    ///
    /// Three different pluralisation rules are in play: `source{s}` keyed on `shared + piOwned`,
    /// `host source{s}` on the host-config count, and `conflict{s}` on the conflict count.
    #[must_use]
    pub fn secondary_summary_line(&self) -> String {
        let host_count = self.discovery.host_configs.len();
        let host_note = if host_count > 0 {
            format!(
                " Host discovery is {}; {host_count} host source{} detected.",
                host_config_discovery_word(self.discovery.host_config_discovery),
                if host_count == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        let conflict_count = self.discovery.conflicts.len();
        let conflict_note = if conflict_count > 0 {
            format!(
                " {conflict_count} same-name conflict{} reported.",
                if conflict_count == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };

        if !self.discovery.has_any_config {
            return format!(
                "Create a shared .mcp.json, adopt host imports, or quick-add RepoPrompt from this screen.{host_note}{conflict_note}"
            );
        }
        let import_count = self.discovery.imports.len();
        if self.discovery.total_server_count == 0 && import_count > 0 {
            return format!(
                "Detected {import_count} compatibility import source{}. Adopt them into Pi or inspect the underlying files.{host_note}{conflict_note}",
                if import_count == 1 { "" } else { "s" }
            );
        }
        format!(
            "Shared MCP files are preferred. Pi-owned files are only for compatibility imports and adapter-specific overrides.{host_note}{conflict_note}"
        )
    }

    /// `formatPreview(lines, width)`.
    fn format_preview(lines: &[String], width: usize) -> Vec<String> {
        lines.iter().flat_map(|line| wrap_text_soft(line, width)).collect()
    }

    /// `formatWritePreview(title, preview, intro, width)` (MCP-376).
    ///
    /// The diff is capped at 18 lines and the overflow line is singular only at exactly one. The
    /// **soft** wrap is used deliberately: a diff line longer than the width is emitted whole and
    /// then truncated by `padLine`, which is why a long JSON line shows as an ellipsis rather than
    /// wrapping.
    #[must_use]
    pub fn format_write_preview(
        title: &str,
        preview: &ConfigWritePreview,
        intro: &[String],
        width: usize,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for line in intro {
            lines.extend(wrap_text_soft(line, width));
        }
        if !intro.is_empty() {
            lines.push(String::new());
        }
        lines.extend(wrap_text_soft(&format!("{title}: {}", preview.path.display()), width));
        lines.extend(wrap_text_soft(
            if preview.existed {
                "Existing file detected. Showing exact before/after diff."
            } else {
                "New file will be created. Showing exact content diff."
            },
            width,
        ));
        lines.push(String::new());
        let diff_lines: Vec<&str> = preview.diff_text.split('\n').collect();
        for line in diff_lines.iter().take(PREVIEW_MAX_DIFF_LINES) {
            lines.extend(wrap_text_soft(line, width));
        }
        if diff_lines.len() > PREVIEW_MAX_DIFF_LINES {
            let extra = diff_lines.len() - PREVIEW_MAX_DIFF_LINES;
            lines.extend(wrap_text_soft(
                &format!(
                    "\u{2026} {extra} more diff line{}",
                    if extra == 1 { "" } else { "s" }
                ),
                width,
            ));
        }
        lines
    }

    /// `getActionPreview(action, previewW)` (13h §3.7).
    ///
    /// Every `preview_*` callback is invoked from here, i.e. inside `render`, on every frame — see
    /// the note on [`SetupPanelCallbacks`].
    #[must_use]
    pub fn action_preview(&self, action: Option<&SetupAction>, width: usize) -> Vec<String> {
        let Some(action) = action else {
            return Self::format_preview(&["Close the setup flow.".to_string()], width);
        };
        match action.id {
            SetupActionId::RunSetup => Self::format_preview(
                &["Run setup to adopt host-specific imports, inspect detected paths, and scaffold a minimal `.mcp.json` if needed.".to_string()],
                width,
            ),
            SetupActionId::AdoptImports => {
                let selected: Vec<ImportKind> = self
                    .discovery
                    .imports
                    .iter()
                    .filter(|entry| self.selected_imports.contains(&entry.kind))
                    .map(|entry| entry.kind)
                    .collect();
                let detected: Vec<String> = self
                    .discovery
                    .imports
                    .iter()
                    .map(|entry| format!("{} ({} servers)", entry.kind.as_str(), entry.server_count))
                    .collect();
                Self::format_write_preview(
                    "Compatibility import write preview",
                    &self.callbacks.preview_imports(&selected),
                    &[
                        format!("Detected imports: {}", detected.join(", ")),
                        "Selected imports are written into the Pi agent dir config as Pi-owned compatibility state.".to_string(),
                    ],
                    width,
                )
            }
            SetupActionId::ViewExample => Self::format_preview(
                &[
                    "Example shared `.mcp.json`:".to_string(),
                    "{".to_string(),
                    "  \"mcpServers\": {".to_string(),
                    "    \"chrome-devtools\": {".to_string(),
                    "      \"command\": \"npx\",".to_string(),
                    "      \"args\": [\"-y\", \"chrome-devtools-mcp@1.6.0\"]".to_string(),
                    "    }".to_string(),
                    "  }".to_string(),
                    "}".to_string(),
                    String::new(),
                    "Use Scaffold project `.mcp.json` when you want a safe empty shell instead of a live example server.".to_string(),
                ],
                width,
            ),
            SetupActionId::ShowPrecedence => {
                let mut lines = vec![
                    "Read order (later entries win):".to_string(),
                    "0. detected host configs (opt-in lowest-precedence fallback)".to_string(),
                    "1. ~/.config/mcp/mcp.json".to_string(),
                    "2. ~/.agents/mcp.json".to_string(),
                    "3. ~/.agents/mcp/mcp.json".to_string(),
                    "4. <Pi agent dir>/mcp.json".to_string(),
                    "5. .mcp.json".to_string(),
                    // The one literal cyrup renames: upstream's `getConfigDirName()` is `.pi`.
                    format!("6. {}/mcp.json", crate::config::PROJECT_OVERRIDE_DIR),
                    format!(
                        "Host discovery: {}. Conflicts reported: {}.",
                        host_config_discovery_word(self.discovery.host_config_discovery),
                        self.discovery.conflicts.len()
                    ),
                ];
                for conflict in self.discovery.conflicts.iter().take(8) {
                    let sources: Vec<String> = conflict
                        .sources
                        .iter()
                        .map(|source| source.path.display().to_string())
                        .collect();
                    lines.push(format!(
                        "{}: {} (winner: {})",
                        conflict.server_name,
                        sources.join(" -> "),
                        conflict.winner.path.display()
                    ));
                }
                lines.push(
                    "Pi writes compatibility imports and adapter-only overrides to Pi-owned files."
                        .to_string(),
                );
                Self::format_preview(&lines, width)
            }
            SetupActionId::OpenPaths => {
                let paths = self.detected_paths();
                let lines = if paths.is_empty() {
                    vec!["No config paths were detected.".to_string()]
                } else {
                    let mut out = vec!["Detected paths:".to_string()];
                    out.extend(paths.iter().map(|p| p.display().to_string()));
                    out
                };
                Self::format_preview(&lines, width)
            }
            SetupActionId::AddRepoPrompt => {
                let repo = &self.discovery.repo_prompt;
                let Some(preview) = self.callbacks.preview_repo_prompt() else {
                    return Self::format_preview(
                        &["RepoPrompt is not available to add from this setup screen.".to_string()],
                        width,
                    );
                };
                Self::format_write_preview(
                    "RepoPrompt write preview",
                    &preview,
                    &[
                        format!(
                            "Executable: {}",
                            repo.executable_path
                                .as_ref()
                                .map_or_else(|| "not found".to_string(), |p| p.display().to_string())
                        ),
                        format!(
                            "Target: {}",
                            repo.target_path
                                .as_ref()
                                .map_or_else(|| "n/a".to_string(), |p| p.display().to_string())
                        ),
                        format!(
                            "Server name: {}",
                            repo.server_name.as_deref().unwrap_or("repoprompt")
                        ),
                    ],
                    width,
                )
            }
            SetupActionId::AddKnownServer => {
                let Some(preset) = action.preset.as_ref() else {
                    return Self::format_preview(
                        &["Known server preset is unavailable.".to_string()],
                        width,
                    );
                };
                Self::format_write_preview(
                    &format!("{} write preview", preset.name),
                    &self.callbacks.preview_known_server(preset),
                    &[preset.summary.to_string()],
                    width,
                )
            }
            SetupActionId::ScaffoldProject => Self::format_write_preview(
                "Starter project `.mcp.json` write preview",
                &self.callbacks.preview_starter_project(),
                &[
                    "This writes a minimal `.mcp.json` in the current project using the shared MCP layout.".to_string(),
                    "It intentionally avoids adding a fake placeholder server that would fail on first reload.".to_string(),
                ],
                width,
            ),
            SetupActionId::Close => {
                Self::format_preview(&["Close the setup flow.".to_string()], width)
            }
        }
    }
}

/// The `/mcp setup` panel wired to a live terminal, mirroring [`McpPanelOverlay`].
pub struct McpSetupOverlay {
    model: McpSetupPanelModel,
    callbacks: Arc<dyn SetupPanelCallbacks>,
    presets: Vec<KnownServerPreset>,
    handle: tokio::runtime::Handle,
    job: Option<(SetupJob, tokio::sync::oneshot::Receiver<SetupJobResult>)>,
    deadline: Instant,
    expired: bool,
}

impl McpSetupOverlay {
    /// Wrap a constructed model.
    #[must_use]
    pub fn new(
        model: McpSetupPanelModel,
        callbacks: Arc<dyn SetupPanelCallbacks>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            model,
            callbacks,
            presets: crate::config::known_server_presets(),
            handle,
            job: None,
            deadline: Instant::now() + Duration::from_millis(INACTIVITY_MS),
            expired: false,
        }
    }

    /// The driven model.
    #[must_use]
    pub fn model(&self) -> &McpSetupPanelModel {
        &self.model
    }

    fn spawn_job(&mut self, job: SetupJob) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let callbacks = Arc::clone(&self.callbacks);
        let preset = match &job {
            SetupJob::AddKnownServer(index) => self.presets.get(*index).cloned(),
            _ => None,
        };
        let spawned = job.clone();
        self.handle.spawn(async move {
            let result = match spawned {
                SetupJob::AdoptImports(imports) => {
                    SetupJobResult::AdoptImports(callbacks.adopt_imports(imports).await)
                }
                SetupJob::ScaffoldProject => {
                    SetupJobResult::ScaffoldProject(callbacks.scaffold_project_config().await)
                }
                SetupJob::AddRepoPrompt => {
                    SetupJobResult::AddRepoPrompt(callbacks.add_repo_prompt().await)
                }
                SetupJob::AddKnownServer(_) => SetupJobResult::AddKnownServer(match preset {
                    Some(preset) => callbacks.add_known_server(preset).await,
                    None => Err("Known server preset is unavailable.".to_string()),
                }),
                SetupJob::OpenPath(path) => {
                    let outcome = callbacks.open_path(path.clone()).await;
                    SetupJobResult::OpenPath(path, outcome)
                }
            };
            let _ = tx.send(result);
        });
        self.job = Some((job, rx));
    }

    /// The rejection a dropped sender stands in for. It must still clear the busy latch, or the
    /// panel refuses every later action until it is closed.
    fn cancelled_result(job: &SetupJob) -> SetupJobResult {
        let message = "The MCP setup action was cancelled.".to_string();
        match job {
            SetupJob::AdoptImports(_) => SetupJobResult::AdoptImports(Err(message)),
            SetupJob::ScaffoldProject => SetupJobResult::ScaffoldProject(Err(message)),
            SetupJob::AddRepoPrompt => SetupJobResult::AddRepoPrompt(Err(message)),
            SetupJob::AddKnownServer(_) => SetupJobResult::AddKnownServer(Err(message)),
            SetupJob::OpenPath(path) => SetupJobResult::OpenPath(path.clone(), Err(message)),
        }
    }

    fn drain_job(&mut self) -> bool {
        let Some((job, rx)) = self.job.as_mut() else { return false };
        let settled = match rx.try_recv() {
            Ok(result) => result,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => Self::cancelled_result(job),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
        };
        self.job = None;
        self.model.finish_job(settled);
        true
    }
}

impl InteractiveOverlay for McpSetupOverlay {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        self.model.render(width)
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        if self.expired {
            return OverlayOutcome::Close;
        }
        self.deadline = Instant::now() + Duration::from_millis(INACTIVITY_MS);
        match self.model.handle_key(key) {
            SetupInputOutcome::Ignored => OverlayOutcome::Ignored,
            SetupInputOutcome::Redraw => OverlayOutcome::Redraw,
            SetupInputOutcome::Close => OverlayOutcome::Close,
            SetupInputOutcome::Run(job) => {
                self.spawn_job(job);
                OverlayOutcome::Redraw
            }
        }
    }

    fn refresh_ms(&self) -> u64 {
        REFRESH_MS
    }

    fn tick(&mut self) -> bool {
        let changed = self.drain_job();
        // TODO(MCP-362): see [`McpPanelOverlay::tick`] — the deadline cannot close the overlay by
        // itself, so it is honoured on the next keystroke instead.
        if !self.expired && Instant::now() >= self.deadline {
            self.expired = true;
            self.model.expire();
            return true;
        }
        changed
    }
}

// =================================================================================================
// 10 · The footer segment (`updateStatusBar`/`formatMcpStatus`) and the shared-config notice
// =================================================================================================

/// The three counts `updateStatusBar` computes before it decides what to write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FooterCounts {
    /// `Object.entries(config.mcpServers).length`.
    pub configured: usize,
    /// Connections whose status is `connected` **and** whose definition exists and is not disabled.
    pub connected: usize,
    /// `entries.length - disabledCount`.
    pub enabled: usize,
    /// `entries.filter(isServerDisabled).length`.
    pub disabled: usize,
}

impl FooterCounts {
    /// Derive the disabled/enabled split from a config; the caller supplies `connected`, which only
    /// the live connection map knows.
    #[must_use]
    pub fn from_config(config: &McpConfig, connected: usize) -> Self {
        let configured = config.mcp_servers.len();
        let disabled = config.mcp_servers.values().filter(|entry| entry.is_disabled()).count();
        Self { configured, connected, enabled: configured - disabled, disabled }
    }
}

/// `utils.ts` `formatMcpStatus(config, message)`.
///
/// `None` is upstream's `undefined`, which the caller turns into `ui.setStatus("mcp", undefined)` —
/// i.e. *clear the segment*, not "write an empty one".
#[must_use]
pub fn format_mcp_status(config: &McpConfig, message: &str) -> Option<String> {
    let settings = config.settings_or_default();
    if settings.mcp_footer_status() == crate::config::FooterStatus::Off {
        return None;
    }
    Some(if settings.show_status_icon() {
        format!("\u{1f50c} MCP: {message}")
    } else {
        format!("MCP: {message}")
    })
}

/// `init.ts` `updateStatusBar`'s text, steps 4 and 6-10 — the pure half.
///
/// `None` means "clear the segment": an empty server table, or `mcpFooterStatus: "off"`. The
/// `compact` form deliberately **skips** [`format_mcp_status`], so the icon prefix appears in `full`
/// mode only.
///
/// Step 11's `ui.theme.fg("accent", …)` has no counterpart — `HostServices` exposes a theme *name*
/// and no `fg(role, text)` — so the text goes out uncoloured, which is upstream's own no-theme
/// branch. Recorded as an accepted delta.
#[must_use]
pub fn footer_status_text(config: &McpConfig, counts: FooterCounts) -> Option<String> {
    if counts.configured == 0 {
        return None;
    }
    let footer = config.settings_or_default().mcp_footer_status();
    match footer {
        crate::config::FooterStatus::Off => None,
        crate::config::FooterStatus::Compact => {
            Some(format!("MCP {}/{}", counts.connected, counts.enabled))
        }
        crate::config::FooterStatus::Full => {
            let mut status = format!(
                "{} {} enabled",
                counts.enabled,
                if counts.enabled == 1 { "server" } else { "servers" }
            );
            if counts.connected > 0 {
                status.push_str(&format!(" ({} connected)", counts.connected));
            }
            if counts.disabled > 0 {
                status.push_str(&format!(" ({} disabled)", counts.disabled));
            }
            format_mcp_status(config, &status)
        }
    }
}

/// `commands.ts` `buildSharedConfigNoticeLines(configOverridePath, cwd)` (MCP-393).
///
/// Returns the panel's `noticeLines` plus the fingerprint to stamp into the onboarding file when the
/// panel actually opens. A panel that never opens does not consume the one-shot, which is why the
/// fingerprint is handed back rather than written here.
///
/// The flag is a plain boolean and the fingerprint is stored but **never compared**, so a changed
/// fingerprint does not re-arm the hint.
#[must_use]
pub fn shared_config_notice_lines(
    summary: &crate::config::McpStandardConfigSummary,
    onboarding: &OnboardingState,
) -> (Vec<String>, Option<String>) {
    if !summary.has_shared_servers || onboarding.shared_config_hint_shown {
        return (Vec::new(), None);
    }
    let source_list: Vec<String> = summary
        .sources
        .iter()
        .filter(|source| {
            source.kind == crate::config::DiscoveryKind::Shared && source.server_count > 0
        })
        .map(|source| source.path.display().to_string())
        .collect();
    (
        vec![
            format!("Using standard MCP config from {}.", source_list.join(", ")),
            "Pi only writes compatibility imports and adapter-specific overrides into Pi-owned files when needed."
                .to_string(),
        ],
        Some(summary.fingerprint.clone()),
    )
}

// =================================================================================================
// 11 · Entry points — how a panel actually reaches the terminal (MCP-369, MCP-391, MCP-394)
// =================================================================================================

// WHY NEITHER ENTRY POINT BELOW CARRIES UPSTREAM'S `canRenderPanel` GUARD
// (`commands.ts:31-42` @v2.26.1, upstream `5787ecd` "don't hang MCP panels outside TUI mode").
//
// `openMcpPanel`/`openMcpSetup`/`openMcpAuthPanel` used to gate on `ctx.hasUI` alone, which pi
// documents as true in TUI **and RPC** modes, and then awaited the panel through
// `new Promise(resolve => ctx.ui.custom(factory, …))`. Outside the terminal `ctx.ui.custom()` is a
// headless stub that returns *without ever invoking the factory*, so `done`/`resolve` never fire,
// the promise never settles, and `/mcp` hangs forever for a host embedding the adapter over
// `pi --mode rpc` — no output, no response, calling command blocked. v2.26.1 added
// `canRenderPanel(ctx) = ctx.hasUI && ctx.mode === "tui"` in front of every overlay.
//
// cyrup cannot reach that state, and not by accident: `HostServices::open_overlay` is a
// `bool`-returning call rather than a promise, and the live implementation answers `false`
// **without blocking** whenever no renderer is attached — headless print/json, rpc, or a bare
// embedder (`crates/cyrup-session-svc/src/host_services.rs:1043-1048`; the trait default at
// `crates/cyrup-ext/src/host/services.rs:254-256` is likewise `false`). The mode test is therefore
// already made, by the host, as a capability probe instead of a string comparison, and the two
// entry points below hand the caller the same "no overlay, fall back to text" signal upstream now
// returns early: `None` from [`open_mcp_panel`], `false` from [`open_mcp_setup_panel`]. The port's
// own spelling of upstream's predicate — `hasUI && mode === "tui"`, which `commands.ts` duplicated
// from `init.ts`'s `isTuiMode` — is [`crate::runtime::ContextSnapshot::is_tui_mode`].
//
// What the `/mcp` dispatcher still owes (MCP-394) is the *fallback output* v2.26.1 chose per
// command, one arm per guard: `/mcp` and `/mcp status` re-render `showStatus()` as text
// (`commands.ts:553-557`), while `/mcp setup` (`:415-418`) and `/mcp-auth`'s picker (`:612-615`)
// notify at `info` with [`panel_unavailable_message`] / [`auth_panel_unavailable_message`].

/// `openMcpAuthPanel`'s `noticeLines` (MCP-391), rendered under the search row.
pub const AUTH_PANEL_NOTICE: &str =
    "Select an OAuth MCP server and press Enter or ctrl+a to authenticate.";

/// `openMcpSetup`'s refusal when the session has a UI but not a *terminal* one — `{mode}` is
/// `HostCtx::mode`. Exposed here because the text belongs to the panel, not to the dispatcher.
///
/// `commands.ts:415-418` @v2.26.1 (upstream `5787ecd`); see the section note above for why the
/// guard that emits it is a host capability probe here rather than a mode string comparison.
#[must_use]
pub fn panel_unavailable_message(mode: &str) -> String {
    format!(
        "The interactive MCP setup panel is only available in the terminal UI (current mode: {mode}). Edit .mcp.json directly, or run /mcp status to review servers."
    )
}

/// `openMcpAuthPanel`'s refusal — the `/mcp-auth` twin of [`panel_unavailable_message`]
/// (`commands.ts:612-615` @v2.26.1, upstream `5787ecd`).
///
/// The two texts are **not** interchangeable, which is why this is a second function and not a
/// parameter: the setup refusal sends the user to `.mcp.json` and `/mcp status` because the thing
/// they wanted (editing the server table) has no non-overlay form, whereas the auth refusal names
/// `/mcp-auth <server>` — the picker's whole job is choosing a server, and once the server is named
/// on the command line the OAuth flow itself needs no overlay and runs in every mode.
#[must_use]
pub fn auth_panel_unavailable_message(mode: &str) -> String {
    format!(
        "The interactive MCP auth panel is only available in the terminal UI (current mode: {mode}). Use /mcp-auth <server> to authenticate a specific server."
    )
}

// TODO(MCP-394): the rest of `openMcpPanel`'s orchestration belongs to the `/mcp` dispatcher, not
// here — the flag/override config-path resolution, the zero-servers-delegates-to-setup path, and the
// `writeDirectToolsConfig` -> `onDirectToolsConfigChanged` -> notify chain whose **error** arm sets
// `configChanged`. This function owns only the half that is genuinely the panel's: getting the
// result back out of a `bool`-returning `open_overlay`. Use [`McpPanelResult::to_config_changes`]
// for the write-back argument.

/// Open the `/mcp` browser panel (or, with `auth_only`, the `/mcp-auth` picker) and block the
/// calling extension task until it closes.
///
/// **How the result escapes a `bool`** (MCP-369): `HostServices::open_overlay` consumes the box and
/// reports only whether a host took the overlay, so the outcome is read off an `Arc`-shared cell the
/// overlay writes through on its close path. `LiveHostServices::open_overlay` blocks the extension's
/// task (never the run loop's) until teardown, which supplies the happens-before; the shipped
/// worked example is `PermissionSystemSettingsOverlay` + `ConfigController::take_last_error`.
///
/// `None` is pi's `if (!ctx.hasUI)` branch — no renderer is attached, so the caller falls back to
/// `showStatus`. It is **not** an error.
#[must_use]
pub fn open_mcp_panel(
    services: &dyn cyrup_ext::host::HostServices,
    model: McpPanelModel,
    callbacks: Arc<dyn McpPanelCallbacks>,
    handle: tokio::runtime::Handle,
) -> Option<McpPanelResult> {
    let out: Arc<Mutex<Option<McpPanelResult>>> = Arc::new(Mutex::new(None));
    let overlay = McpPanelOverlay::new(model, callbacks, handle, Arc::clone(&out));
    if !services.open_overlay(Box::new(overlay)) {
        return None;
    }
    // Read AFTER `open_overlay` returns: the block-until-closed contract is the synchronisation.
    out.lock().ok().and_then(|mut slot| slot.take())
}

/// Open the `/mcp setup` panel and block until it closes.
///
/// Returns `false` for the no-renderer branch, exactly as [`open_mcp_panel`] returns `None`. The
/// setup panel's own `done()` carries no value — whether anything was written is tracked by the
/// caller's [`SetupPanelCallbacks`] implementation, which is where `configChanged` lives upstream.
#[must_use]
pub fn open_mcp_setup_panel(
    services: &dyn cyrup_ext::host::HostServices,
    model: McpSetupPanelModel,
    callbacks: Arc<dyn SetupPanelCallbacks>,
    handle: tokio::runtime::Handle,
) -> bool {
    services.open_overlay(Box::new(McpSetupOverlay::new(model, callbacks, handle)))
}

// =================================================================================================
// 11 · Tests
// =================================================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::config::{BoolOrList, McpSettings};
    use crate::dirs::CachedResource;

    // ---------------------------------------------------------------------------------------
    // MCP-364 — the terminal-injection sanitizers
    // ---------------------------------------------------------------------------------------

    #[test]
    fn a_bel_inside_a_tool_name_becomes_a_single_space() {
        // `__tests__/mcp-panel-rendering.test.ts`: a tool named `search\u{7}issues`.
        assert_eq!(sanitize_terminal_text("search\u{7}issues"), "search issues");
    }

    #[test]
    fn sgr_osc_tab_and_nul_are_all_removed_from_a_description() {
        let hostile = "\u{1b}[31mred\u{1b}[0m\u{1b}]8;;https://evil.invalid\u{7}link\tafter\0end";
        let clean = sanitize_terminal_text(hostile);
        assert!(!clean.contains('\u{1b}'), "no escape survives: {clean:?}");
        assert!(!clean.contains("evil.invalid"), "the OSC payload is gone: {clean:?}");
        assert!(clean.contains("red"));
        assert!(clean.contains("after"));
    }

    #[test]
    fn an_unterminated_osc_never_leaks_its_url() {
        // The exact case the rendering test pins: no BEL, no ST, no ESC-backslash.
        let hostile = "visible\u{1b}]8;; https://secret.invalid/truncated";
        let clean = sanitize_terminal_text(hostile);
        assert_eq!(clean, "visible");
        assert!(!clean.contains("secret.invalid"));
    }

    #[test]
    fn row_sanitation_drops_controls_and_inserts_at_most_one_space() {
        let spans = vec![OverlaySpan::raw("ab\u{1}\u{2}cd"), OverlaySpan::raw("\u{7}ef")];
        let out = sanitize_row_content(&spans);
        let text: String = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "ab cd ef");
        assert!(!text.chars().any(char::is_control));
    }

    #[test]
    fn every_emitted_frame_row_is_free_of_control_characters() {
        let model = fixture_panel(false);
        for line in model.render(82) {
            let text = line.plain_text();
            assert!(!text.chars().any(char::is_control), "control byte in {text:?}");
        }
    }

    // ---------------------------------------------------------------------------------------
    // MCP-354 / MCP-365 — scoring and estimation
    // ---------------------------------------------------------------------------------------

    #[test]
    fn fuzzy_scores_substring_above_subsequence_and_refuses_non_subsequences() {
        // A substring hit is 100 + (len(q)/len(t)) * 50.
        let substring = fuzzy_score("las", "atlassian");
        assert!((100.0..=150.0).contains(&substring), "{substring}");
        // A subsequence with no run: 10 per matched character, consecutive resets each miss.
        assert_eq!(fuzzy_score("ac", "abc"), 10.0 + 10.0);
        // A run of two: 10, then 10 + 5.
        assert_eq!(fuzzy_score("ab", "abx"), 100.0 + (2.0 / 3.0) * 50.0);
        assert_eq!(fuzzy_score("zzz", "abc"), 0.0);
    }

    #[test]
    fn token_estimation_matches_the_literal_formula() {
        // ceil((name + desc + "{}") / 4) + 10, with the empty schema stringifying to two chars.
        assert_eq!(estimate_tokens("ab", None, None), (2 + 2usize).div_ceil(4) + 10);
        let schema = serde_json::json!({"type": "object"});
        let schema_len = serde_json::to_string(&schema).unwrap().chars().count();
        assert_eq!(
            estimate_tokens("tool", Some("desc"), Some(&schema)),
            (4 + 4 + schema_len).div_ceil(4) + 10
        );
    }

    #[test]
    fn thousands_are_grouped_the_way_node_groups_them() {
        assert_eq!(to_locale_string(0), "0");
        assert_eq!(to_locale_string(999), "999");
        assert_eq!(to_locale_string(12_345), "12,345");
        assert_eq!(to_locale_string(1_234_567), "1,234,567");
    }

    // ---------------------------------------------------------------------------------------
    // Text primitives
    // ---------------------------------------------------------------------------------------

    #[test]
    fn truncation_pads_to_exactly_the_requested_width() {
        assert_eq!(truncate_to_width("", 5, "\u{2026}", true), "     ");
        assert_eq!(truncate_to_width("abc", 5, "\u{2026}", true), "abc  ");
        assert_eq!(truncate_to_width("abcdefgh", 5, "\u{2026}", true), "abcd\u{2026}");
        assert_eq!(truncate_to_width("abcdefgh", 5, "\u{2026}", false), "abcd\u{2026}");
    }

    #[test]
    fn the_two_wraps_differ_on_an_overlong_word() {
        // The panel's wrap hard-splits.
        assert_eq!(wrap_text_hard("aaaaaaaaaaaa", 8), vec!["aaaaaaaa", "aaaa"]);
        // The setup panel's does not.
        assert_eq!(wrap_text_soft("aaaaaaaaaaaa", 12), vec!["aaaaaaaaaaaa"]);
        assert_eq!(wrap_text_soft("anything", 8), vec!["anything"], "width <= 8 is verbatim");
        assert_eq!(wrap_text_soft("", 40), vec![String::new()]);
    }

    // ---------------------------------------------------------------------------------------
    // MCP-363 — `panel-keys.ts`
    // ---------------------------------------------------------------------------------------

    fn key(code: OverlayKeyCode) -> OverlayKey {
        OverlayKey::plain(code)
    }

    #[test]
    fn the_three_save_binding_states_each_behave_differently() {
        let ctrl_s = OverlayKey::ctrl(OverlayKeyCode::Char('s'));
        let ctrl_p = OverlayKey::ctrl(OverlayKeyCode::Char('p'));

        // Absent: the ctrl+s default, with a label.
        let absent = PanelKeys::from_user_bindings(&[]);
        assert!(absent.save(&ctrl_s));
        assert_eq!(absent.save_label(), Some("ctrl+s"));

        // Bound: those keys, first as the label; the default no longer fires.
        let bound = PanelKeys::from_user_bindings(&[(
            "mcp.panel.save".into(),
            serde_json::json!(["ctrl+p"]),
        )]);
        assert!(bound.save(&ctrl_p));
        assert!(!bound.save(&ctrl_s));
        assert_eq!(bound.save_label(), Some("ctrl+p"));

        // Present but EMPTY: saving is disabled entirely and the hint disappears.
        let unbound =
            PanelKeys::from_user_bindings(&[("mcp.panel.save".into(), serde_json::json!([]))]);
        assert!(!unbound.save(&ctrl_s));
        assert!(!unbound.save(&ctrl_p));
        assert_eq!(unbound.save_label(), None);
    }

    #[test]
    fn rebinding_the_canonical_select_ids_moves_the_cursor_on_the_new_key() {
        let keys = PanelKeys::from_user_bindings(&[(
            "tui.select.up".into(),
            serde_json::json!("ctrl+p"),
        )]);
        assert!(keys.select_up(&OverlayKey::ctrl(OverlayKeyCode::Char('p'))));
        assert!(!keys.select_up(&key(OverlayKeyCode::Up)));
        // The other two keep their defaults.
        assert!(keys.select_down(&key(OverlayKeyCode::Down)));
        assert!(keys.select_confirm(&key(OverlayKeyCode::Enter)));
    }

    #[test]
    fn the_default_select_keys_are_cyrup_tuis_select_keymap_defaults() {
        // MCP-363a option (a): this crate carries its own copy, and this is the test that fails the
        // day either side moves. `cyrup-tui`'s `SelectKeymap::default` binds Up / Down / Enter.
        assert_eq!(DEFAULT_SELECT_UP, "up");
        assert_eq!(DEFAULT_SELECT_DOWN, "down");
        assert_eq!(DEFAULT_SELECT_CONFIRM, "return");
        let keys = PanelKeys::default();
        assert!(keys.select_up(&key(OverlayKeyCode::Up)));
        assert!(keys.select_down(&key(OverlayKeyCode::Down)));
        assert!(keys.select_confirm(&key(OverlayKeyCode::Enter)));
    }

    // ---------------------------------------------------------------------------------------
    // Fixtures
    // ---------------------------------------------------------------------------------------

    struct StubCallbacks {
        status: ConnectionStatus,
        failure: Option<String>,
        can_auth: bool,
    }

    impl Default for StubCallbacks {
        fn default() -> Self {
            Self { status: ConnectionStatus::Idle, failure: None, can_auth: false }
        }
    }

    impl McpPanelCallbacks for StubCallbacks {
        fn connection_status(&self, _server: &str) -> ConnectionStatus {
            self.status
        }
        fn failure_message(&self, _server: &str) -> Option<String> {
            self.failure.clone()
        }
        fn can_authenticate(&self, _server: &str) -> bool {
            self.can_auth
        }
        fn authenticate(
            &self,
            _server: String,
        ) -> futures::future::BoxFuture<'static, Result<McpAuthResult, String>> {
            Box::pin(async { Ok(McpAuthResult { ok: true, message: None }) })
        }
        fn reconnect(
            &self,
            _server: String,
        ) -> futures::future::BoxFuture<'static, Result<bool, String>> {
            Box::pin(async { Ok(true) })
        }
    }

    /// A cache entry whose `configHash` matches `definition`, so `isServerCacheValid` accepts it.
    fn valid_cache_entry(definition: &ServerEntry, tools: &[&str]) -> ServerCacheEntry {
        ServerCacheEntry {
            config_hash: compute_server_hash(definition, &ResolvedIdentity::verbatim(definition)),
            tools: tools
                .iter()
                .map(|name| CachedTool {
                    name: (*name).to_string(),
                    description: Some(format!("{name} description")),
                    ..CachedTool::default()
                })
                .collect(),
            resources: Vec::new(),
            prompts: None,
            instructions: None,
            cached_at: crate::dirs::now_ms(),
        }
    }

    fn fixture_panel(auth_only: bool) -> McpPanelModel {
        let mut config = McpConfig::default();
        let stdio = ServerEntry { command: Some("echo".into()), ..ServerEntry::default() };
        let disabled =
            ServerEntry { command: Some("echo".into()), disabled: Some(true), ..ServerEntry::default() };
        config.mcp_servers.insert("atlassian".into(), stdio.clone());
        config.mcp_servers.insert("stale".into(), stdio.clone());
        config.mcp_servers.insert("off".into(), disabled.clone());

        let mut cache = MetadataCache::default();
        cache
            .servers
            .insert("atlassian".into(), valid_cache_entry(&stdio, &["search_issues", "create"]));
        // A stale entry: the hash does not match, so the panel treats it as absent.
        let mut bad = valid_cache_entry(&stdio, &["ignored"]);
        bad.config_hash = "0".repeat(64);
        cache.servers.insert("stale".into(), bad);
        cache.servers.insert("off".into(), valid_cache_entry(&disabled, &["never"]));

        McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(StubCallbacks::default()),
            PanelOptions { auth_only, ..PanelOptions::default() },
        )
    }

    // ---------------------------------------------------------------------------------------
    // MCP-351 — construction
    // ---------------------------------------------------------------------------------------

    #[test]
    fn construction_honours_config_order_cache_validity_and_the_disabled_gate() {
        let model = fixture_panel(false);
        let names: Vec<&str> = model.servers().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["atlassian", "stale", "off"], "file order is row order");

        assert!(model.servers()[0].has_cached_data);
        assert_eq!(model.servers()[0].tools.len(), 2);

        assert!(!model.servers()[1].has_cached_data, "a hash mismatch reads as not cached");
        assert!(model.servers()[1].tools.is_empty());

        assert!(model.servers()[2].has_cached_data, "the entry is valid ...");
        assert!(model.servers()[2].tools.is_empty(), "... but a disabled server lists no tools");
    }

    #[test]
    fn a_global_direct_tools_true_pre_ticks_every_tool() {
        let mut config = McpConfig::default();
        let stdio = ServerEntry { command: Some("echo".into()), ..ServerEntry::default() };
        config.mcp_servers.insert("a".into(), stdio.clone());
        config.settings = Some(McpSettings { direct_tools: Some(true), ..McpSettings::default() });
        let mut cache = MetadataCache::default();
        cache.servers.insert("a".into(), valid_cache_entry(&stdio, &["one", "two"]));
        let model = McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(StubCallbacks::default()),
            PanelOptions::default(),
        );
        assert!(model.servers()[0].tools.iter().all(|t| t.is_direct && t.was_direct));
        assert!(!model.is_dirty(), "the baseline equals the live value at construction");
    }

    #[test]
    fn a_per_server_direct_tools_false_beats_a_global_true() {
        let mut config = McpConfig::default();
        let stdio = ServerEntry {
            command: Some("echo".into()),
            direct_tools: Some(BoolOrList::All(false)),
            ..ServerEntry::default()
        };
        config.mcp_servers.insert("a".into(), stdio.clone());
        config.settings = Some(McpSettings { direct_tools: Some(true), ..McpSettings::default() });
        let mut cache = MetadataCache::default();
        cache.servers.insert("a".into(), valid_cache_entry(&stdio, &["one"]));
        let model = McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(StubCallbacks::default()),
            PanelOptions::default(),
        );
        assert!(!model.servers()[0].tools[0].is_direct);
    }

    #[test]
    fn a_resource_becomes_a_read_tool_with_a_smaller_estimate_than_a_real_tool() {
        let mut config = McpConfig::default();
        let stdio = ServerEntry { command: Some("echo".into()), ..ServerEntry::default() };
        config.mcp_servers.insert("a".into(), stdio.clone());
        let mut entry = valid_cache_entry(&stdio, &[]);
        entry.tools.push(CachedTool {
            name: "read_docs".into(),
            description: Some("x".into()),
            input_schema: Some(serde_json::json!({"type": "object", "properties": {}})),
            ..CachedTool::default()
        });
        entry.resources.push(CachedResource {
            uri: "file:///docs".into(),
            name: "Docs".into(),
            description: None,
        });
        // Re-stamp the hash: `valid_cache_entry` computed it before the pushes, but the hash is over
        // the *definition*, not the entry, so it is still correct.
        let mut cache = MetadataCache::default();
        cache.servers.insert("a".into(), entry);
        let model = McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(StubCallbacks::default()),
            PanelOptions::default(),
        );
        let names: Vec<&str> =
            model.servers()[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_docs", "read_docs"], "the resource's base name collides");
        assert_eq!(
            model.servers()[0].tools[1].description,
            "Read resource: file:///docs",
            "the description falls back to the URI"
        );
        assert!(
            model.servers()[0].tools[1].estimated_tokens
                < model.servers()[0].tools[0].estimated_tokens,
            "a resource tool is fed no input schema"
        );
    }

    // ---------------------------------------------------------------------------------------
    // MCP-353 — the flattened list
    // ---------------------------------------------------------------------------------------

    #[test]
    fn a_query_force_expands_and_then_drops_headers_with_no_surviving_tools() {
        let mut model = fixture_panel(false);
        assert_eq!(model.visible_items().len(), 3, "three collapsed headers");

        for ch in "search".chars() {
            model.handle_key(key(OverlayKeyCode::Char(ch)));
        }
        let items = model.visible_items().to_vec();
        assert!(
            items.iter().any(|i| matches!(i, VisibleItem::Tool { .. })),
            "the query force-expands"
        );
        // `stale` and `off` have no tools, so their headers are dropped.
        assert!(items.iter().all(|i| i.server_index() == 0));
    }

    #[test]
    fn a_server_name_match_lists_all_of_its_tools_at_the_reduced_weight() {
        let mut model = fixture_panel(false);
        for ch in "atlas".chars() {
            model.handle_key(key(OverlayKeyCode::Char(ch)));
        }
        let tools =
            model.visible_items().iter().filter(|i| matches!(i, VisibleItem::Tool { .. })).count();
        assert_eq!(tools, 2, "both tools survive on the server-name match alone");
    }

    #[test]
    fn auth_only_lists_no_tools_and_skips_servers_that_cannot_authenticate() {
        let model = fixture_panel(true);
        assert!(model.servers().is_empty(), "the stub refuses every server");
        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(frame.iter().any(|l| l.contains("MCP OAuth")));
        assert!(frame.iter().any(|l| l.contains("No OAuth-capable MCP servers configured.")));
        assert!(frame.iter().any(|l| l.contains("select a server to authenticate")));
    }

    // ---------------------------------------------------------------------------------------
    // MCP-355 / MCP-357 / MCP-358 — dispatch, the discard modal, the tri-state
    // ---------------------------------------------------------------------------------------

    #[test]
    fn space_toggles_a_whole_server_and_build_result_is_tri_state() {
        let mut model = fixture_panel(false);
        // Cursor starts on `atlassian`'s header.
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert_eq!(
            model.build_result().changes.get("atlassian"),
            Some(&DirectToolsChange::All),
            "all on"
        );
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert!(model.build_result().changes.is_empty(), "back to the baseline: no change at all");

        // A partial selection: expand, move to the first tool, toggle it.
        model.handle_key(key(OverlayKeyCode::Enter));
        model.handle_key(key(OverlayKeyCode::Down));
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert_eq!(
            model.build_result().changes.get("atlassian"),
            Some(&DirectToolsChange::Named(vec!["search_issues".into()]))
        );
    }

    #[test]
    fn a_server_with_no_tools_toggles_to_false_and_changes_nothing() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Down)); // `stale`, which has no tools
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert!(model.build_result().changes.is_empty());
        assert!(!model.is_dirty());
    }

    #[test]
    fn escape_on_a_dirty_panel_arms_the_discard_modal_with_keep_preselected() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert!(model.is_dirty());
        assert_eq!(model.handle_key(key(OverlayKeyCode::Escape)), PanelInputOutcome::Redraw);

        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(frame.iter().any(|l| l.contains("Discard unsaved changes?")));

        // `ctrl+s` is ignored while the modal is up — clause 1 and clause 2 in one bug otherwise.
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('s'))),
            PanelInputOutcome::Ignored
        );
        assert!(model.result().is_none(), "nothing was saved from inside the modal");

        // Enter on the preselected `Keep & Close` keeps the changes.
        assert_eq!(model.handle_key(key(OverlayKeyCode::Enter)), PanelInputOutcome::Close);
        let result = model.result().unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.changes.get("atlassian"), Some(&DirectToolsChange::All));
    }

    #[test]
    fn tab_then_enter_in_the_discard_modal_discards() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        model.handle_key(key(OverlayKeyCode::Escape));
        model.handle_key(key(OverlayKeyCode::Tab));
        model.handle_key(key(OverlayKeyCode::Enter));
        let result = model.result().unwrap();
        assert!(result.cancelled);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn ctrl_s_saves_from_inside_description_search_but_question_mark_never_reaches_the_query() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Char('?')));
        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(frame.iter().any(|l| l.contains("desc:")));
        // `?` is claimed at step 12, above the printable catch-all, so it never lands in a query.
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert!(model.is_dirty(), "space toggles even inside desc search");
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('s'))),
            PanelInputOutcome::Close
        );
        assert!(!model.result().unwrap().cancelled);
    }

    #[test]
    fn leaving_description_search_always_clears_the_query() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Char('?')));
        for ch in "des".chars() {
            model.handle_key(key(OverlayKeyCode::Char(ch)));
        }
        model.handle_key(key(OverlayKeyCode::Escape));
        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(!frame.iter().any(|l| l.contains("desc:")), "the modal is gone");
        assert!(frame.iter().any(|l| l.contains("search...")), "and the query is empty");
    }

    #[test]
    fn ctrl_c_cancels_with_no_changes_even_when_dirty() {
        let mut model = fixture_panel(false);
        model.handle_key(key(OverlayKeyCode::Char(' ')));
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('c'))),
            PanelInputOutcome::Close
        );
        assert!(model.result().unwrap().cancelled);
        assert!(model.result().unwrap().changes.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // MCP-359 / MCP-360 / MCP-361 — the async settles
    // ---------------------------------------------------------------------------------------

    #[test]
    fn a_server_that_cannot_authenticate_says_so_and_starts_no_job() {
        let mut model = fixture_panel(false);
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('a'))),
            PanelInputOutcome::Redraw
        );
        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(
            frame.iter().any(|l| l.contains("atlassian does not use OAuth authentication.")),
            "{frame:#?}"
        );
    }

    #[test]
    fn a_dropped_authenticate_sender_still_clears_the_single_flight_latch() {
        struct AuthStub;
        impl McpPanelCallbacks for AuthStub {
            fn connection_status(&self, _server: &str) -> ConnectionStatus {
                ConnectionStatus::NeedsAuth
            }
            fn can_authenticate(&self, _server: &str) -> bool {
                true
            }
            fn authenticate(
                &self,
                _server: String,
            ) -> futures::future::BoxFuture<'static, Result<McpAuthResult, String>> {
                Box::pin(async { Ok(McpAuthResult { ok: false, message: None }) })
            }
            fn reconnect(
                &self,
                _server: String,
            ) -> futures::future::BoxFuture<'static, Result<bool, String>> {
                Box::pin(async { Ok(false) })
            }
        }
        let mut config = McpConfig::default();
        config.mcp_servers.insert(
            "notion".into(),
            ServerEntry { url: Some("https://x.invalid/mcp".into()), ..ServerEntry::default() },
        );
        let mut model = McpPanelModel::new(
            &config,
            None,
            &IndexMap::new(),
            Arc::new(AuthStub),
            PanelOptions::default(),
        );

        let outcome = model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('a')));
        assert_eq!(outcome, PanelInputOutcome::Run(PanelJob::Authenticate("notion".into())));
        // A second attempt is refused while the first is in flight.
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('a'))),
            PanelInputOutcome::Ignored
        );

        // The task vanished: the overlay settles it as a rejection, which must clear the latch.
        let cancelled =
            McpPanelOverlay::cancelled_result(&PanelJob::Authenticate("notion".into()));
        assert!(model.finish_job(cancelled).is_none());
        assert_eq!(
            model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('a'))),
            PanelInputOutcome::Run(PanelJob::Authenticate("notion".into())),
            "the latch cleared, so a later authentication is not silently refused"
        );
    }

    #[test]
    fn the_four_oauth_notice_strings_are_byte_exact() {
        let mut model = fixture_panel(false);

        model.finish_job(PanelJobResult::Authenticated {
            server: "atlassian".into(),
            outcome: Ok(McpAuthResult { ok: false, message: Some("bad token".into()) }),
        });
        assert_eq!(model.auth_notice(), Some("OAuth failed for atlassian: bad token"));

        model.finish_job(PanelJobResult::Authenticated {
            server: "atlassian".into(),
            outcome: Ok(McpAuthResult { ok: false, message: None }),
        });
        assert_eq!(
            model.auth_notice(),
            Some("OAuth failed for atlassian. Check the notification for details.")
        );

        model.finish_job(PanelJobResult::Reconnected {
            server: "atlassian".into(),
            after_auth: true,
            outcome: Ok(false),
        });
        assert_eq!(
            model.auth_notice(),
            Some(
                "OAuth finished for atlassian, but reconnect did not complete. Press ctrl+r to retry."
            )
        );

        model.finish_job(PanelJobResult::Reconnected {
            server: "atlassian".into(),
            after_auth: false,
            outcome: Err("socket closed".into()),
        });
        assert_eq!(model.auth_notice(), Some("Reconnect failed for atlassian: socket closed"));
        assert_eq!(model.servers()[0].connection_status, ConnectionStatus::Failed);
        // The notice is rendered, truncated to the panel width, in the needsAuth slot.
        let frame: String =
            model.render(82).iter().map(OverlayLine::plain_text).collect::<Vec<_>>().join("\n");
        assert!(frame.contains("Reconnect failed for atlassian"));
    }

    #[test]
    fn ctrl_y_copies_the_sanitized_failure_text_and_gates_its_own_hint() {
        let mut config = McpConfig::default();
        config
            .mcp_servers
            .insert("dock\u{7}er".into(), ServerEntry { command: Some("x".into()), ..ServerEntry::default() });
        let mut model = McpPanelModel::new(
            &config,
            None,
            &IndexMap::new(),
            Arc::new(StubCallbacks {
                status: ConnectionStatus::Failed,
                failure: Some("spawn \u{1b}[31mENOENT\u{1b}[0m".into()),
                can_auth: false,
            }),
            PanelOptions::default(),
        );
        assert!(model.selected_server_has_failure_message());
        let frame: String =
            model.render(82).iter().map(OverlayLine::plain_text).collect::<Vec<_>>().join("\n");
        assert!(frame.contains("ctrl+y copy error"), "the hint is shown");

        let outcome = model.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('y')));
        match outcome {
            PanelInputOutcome::Run(PanelJob::CopyError { server, text }) => {
                assert_eq!(server, "dock er", "the name is sanitized");
                assert_eq!(text, "spawn ENOENT", "the copied text is the sanitized form");
                model.finish_job(PanelJobResult::Copied {
                    server,
                    outcome: Err("pbcopy exited with 1".into()),
                });
            }
            other => panic!("expected a copy job, got {other:?}"),
        }
        let frame: String =
            model.render(82).iter().map(OverlayLine::plain_text).collect::<Vec<_>>().join("\n");
        assert!(frame.contains("Failed to copy error for dock er: pbcopy exited with 1"));
    }

    #[test]
    fn a_reconnect_preserves_pending_toggles_and_resets_a_vanished_tools_baseline() {
        struct Refresher {
            entry: ServerCacheEntry,
        }
        impl McpPanelCallbacks for Refresher {
            fn connection_status(&self, _server: &str) -> ConnectionStatus {
                ConnectionStatus::Connected
            }
            fn can_authenticate(&self, _server: &str) -> bool {
                false
            }
            fn refresh_cache_after_reconnect(&self, _server: &str) -> Option<ServerCacheEntry> {
                Some(self.entry.clone())
            }
            fn authenticate(
                &self,
                _server: String,
            ) -> futures::future::BoxFuture<'static, Result<McpAuthResult, String>> {
                Box::pin(async { Err("no".to_string()) })
            }
            fn reconnect(
                &self,
                _server: String,
            ) -> futures::future::BoxFuture<'static, Result<bool, String>> {
                Box::pin(async { Ok(true) })
            }
        }

        let mut config = McpConfig::default();
        let stdio = ServerEntry { command: Some("echo".into()), ..ServerEntry::default() };
        config.mcp_servers.insert("a".into(), stdio.clone());
        config.settings = Some(McpSettings { direct_tools: Some(true), ..McpSettings::default() });
        let mut cache = MetadataCache::default();
        cache.servers.insert("a".into(), valid_cache_entry(&stdio, &["keep", "gone"]));
        // After the reconnect `gone` is absent and `fresh` is new.
        let rebuilt = valid_cache_entry(&stdio, &["keep", "fresh"]);

        let mut model = McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(Refresher { entry: rebuilt }),
            PanelOptions::default(),
        );
        assert!(model.servers()[0].tools.iter().all(|t| t.was_direct));

        model.finish_job(PanelJobResult::Reconnected {
            server: "a".into(),
            after_auth: false,
            outcome: Ok(true),
        });
        let tools = &model.servers()[0].tools;
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "keep");
        assert!(tools[0].is_direct && tools[0].was_direct, "a survivor keeps both");
        assert_eq!(tools[1].name, "fresh");
        assert!(
            !tools[1].is_direct && !tools[1].was_direct,
            "a tool that was not there before starts from nothing"
        );
        assert!(model.servers()[0].has_cached_data);
    }

    // ---------------------------------------------------------------------------------------
    // MCP-366 / MCP-367 — the frame
    // ---------------------------------------------------------------------------------------

    #[test]
    fn every_body_row_is_exactly_the_requested_width() {
        for width in [60usize, 82, 120] {
            let model = fixture_panel(false);
            for line in model.render(width) {
                assert_eq!(
                    visible_width(&line.plain_text()),
                    width,
                    "a row at width {width}: {:?}",
                    line.plain_text()
                );
            }
        }
    }

    #[test]
    fn a_long_failure_message_wraps_under_its_server_rather_than_truncating() {
        let long = "spawn docker ENOENT: could not start the container runtime for the atlassian mcp server";
        let mut config = McpConfig::default();
        config
            .mcp_servers
            .insert("atlassian".into(), ServerEntry { command: Some("x".into()), ..ServerEntry::default() });
        let model = McpPanelModel::new(
            &config,
            None,
            &IndexMap::new(),
            Arc::new(StubCallbacks {
                status: ConnectionStatus::Failed,
                failure: Some(long.to_string()),
                can_auth: false,
            }),
            PanelOptions::default(),
        );
        let frame: String =
            model.render(82).iter().map(OverlayLine::plain_text).collect::<Vec<_>>().join("\n");
        for word in long.split(' ') {
            assert!(frame.contains(word), "{word:?} is missing from the frame");
        }
        assert!(frame.contains("failed"), "the status ladder shows `failed`");
    }

    #[test]
    fn a_long_list_shows_the_progress_counter_and_windows_to_twelve_rows() {
        let mut config = McpConfig::default();
        let stdio = ServerEntry { command: Some("echo".into()), ..ServerEntry::default() };
        let mut cache = MetadataCache::default();
        for i in 0..20 {
            let name = format!("s{i}");
            config.mcp_servers.insert(name.clone(), stdio.clone());
            cache.servers.insert(name, valid_cache_entry(&stdio, &["t"]));
        }
        let model = McpPanelModel::new(
            &config,
            Some(cache),
            &IndexMap::new(),
            Arc::new(StubCallbacks::default()),
            PanelOptions::default(),
        );
        let frame: Vec<String> = model.render(82).iter().map(OverlayLine::plain_text).collect();
        assert!(frame.iter().any(|l| l.contains("1/20")), "the counter is shown");
        let server_rows = frame.iter().filter(|l| l.contains("s1") || l.contains("s0")).count();
        assert!(server_rows <= MAX_VISIBLE, "the body is windowed");
    }

    #[test]
    fn the_hint_bar_wraps_instead_of_overflowing_the_frame() {
        let model = fixture_panel(false);
        let rows = model.hint_rows(58);
        assert!(rows.len() > 1, "at 60 columns the hints do not fit on one line");
        for row in rows {
            assert!(row.width() <= 56, "each line fits in innerW - 2");
        }
    }

    // ---------------------------------------------------------------------------------------
    // MCP-371 .. MCP-379 — the setup panel
    // ---------------------------------------------------------------------------------------

    struct StubSetup;

    fn empty_preview() -> ConfigWritePreview {
        ConfigWritePreview {
            path: PathBuf::from("/tmp/.mcp.json"),
            existed: false,
            changed: true,
            before_text: String::new(),
            after_text: "{}".into(),
            diff_text: "+{}".into(),
        }
    }

    impl SetupPanelCallbacks for StubSetup {
        fn preview_imports(&self, _imports: &[ImportKind]) -> ConfigWritePreview {
            empty_preview()
        }
        fn preview_starter_project(&self) -> ConfigWritePreview {
            empty_preview()
        }
        fn preview_repo_prompt(&self) -> Option<ConfigWritePreview> {
            None
        }
        fn preview_known_server(&self, _preset: &KnownServerPreset) -> ConfigWritePreview {
            empty_preview()
        }
        fn adopt_imports(
            &self,
            _imports: Vec<ImportKind>,
        ) -> futures::future::BoxFuture<'static, Result<AdoptImportsOutcome, String>> {
            Box::pin(async { Err("no".to_string()) })
        }
        fn scaffold_project_config(
            &self,
        ) -> futures::future::BoxFuture<'static, Result<PathBuf, String>> {
            Box::pin(async { Err("no".to_string()) })
        }
        fn add_repo_prompt(
            &self,
        ) -> futures::future::BoxFuture<'static, Result<AddServerOutcome, String>> {
            Box::pin(async { Err("no".to_string()) })
        }
        fn add_known_server(
            &self,
            _preset: KnownServerPreset,
        ) -> futures::future::BoxFuture<'static, Result<AddServerOutcome, String>> {
            Box::pin(async { Err("no".to_string()) })
        }
        fn open_path(
            &self,
            _path: PathBuf,
        ) -> futures::future::BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn mark_setup_completed(&self) {}
    }

    fn empty_discovery() -> McpDiscoverySummary {
        McpDiscoverySummary {
            sources: Vec::new(),
            imports: Vec::new(),
            host_configs: Vec::new(),
            host_config_discovery: crate::config::HostConfigDiscovery::Off,
            agent_plugins: Vec::new(),
            conflicts: Vec::new(),
            has_any_config: false,
            has_any_detected_paths: false,
            has_shared_servers: false,
            has_pi_owned_servers: false,
            total_server_count: 0,
            fingerprint: "fp".into(),
            repo_prompt: crate::config::RepoPromptDiscovery::default(),
        }
    }

    fn setup_model(screen: SetupScreen) -> McpSetupPanelModel {
        McpSetupPanelModel::new(
            empty_discovery(),
            OnboardingState::default(),
            Arc::new(StubSetup),
            screen,
            PanelKeys::default(),
        )
    }

    #[test]
    fn the_empty_screen_offers_run_setup_first_and_close_last() {
        let model = setup_model(SetupScreen::Empty);
        let ids: Vec<SetupActionId> = model.actions().iter().map(|a| a.id).collect();
        assert_eq!(ids.first(), Some(&SetupActionId::RunSetup));
        assert_eq!(ids.last(), Some(&SetupActionId::Close));
        assert_eq!(
            ids.iter().filter(|id| **id == SetupActionId::AddKnownServer).count(),
            5,
            "five presets"
        );
        assert!(!ids.contains(&SetupActionId::AdoptImports), "no imports were detected");
        assert!(!ids.contains(&SetupActionId::OpenPaths), "no paths were detected");
        assert!(ids.contains(&SetupActionId::ScaffoldProject), "no shared-project source exists");
    }

    #[test]
    fn run_setup_changes_the_screen_and_therefore_the_action_list() {
        let mut model = setup_model(SetupScreen::Empty);
        let before = model.actions().len();
        assert_eq!(model.handle_key(key(OverlayKeyCode::Enter)), SetupInputOutcome::Redraw);
        assert_eq!(model.screen(), SetupScreen::Setup);
        assert_eq!(model.actions().len(), before - 1, "`run-setup` is gone from the setup screen");
    }

    #[test]
    fn escape_and_ctrl_c_close_the_panel_even_while_busy() {
        let mut model = setup_model(SetupScreen::Setup);
        // Reach `scaffold-project` and run it.
        let index = model
            .actions()
            .iter()
            .position(|a| a.id == SetupActionId::ScaffoldProject)
            .unwrap();
        for _ in 0..index {
            model.handle_key(key(OverlayKeyCode::Down));
        }
        assert!(matches!(
            model.handle_key(key(OverlayKeyCode::Enter)),
            SetupInputOutcome::Run(SetupJob::ScaffoldProject)
        ));
        assert!(model.is_busy());
        assert_eq!(model.handle_key(key(OverlayKeyCode::Down)), SetupInputOutcome::Ignored);
        assert_eq!(model.handle_key(key(OverlayKeyCode::Escape)), SetupInputOutcome::Close);
        assert!(model.is_closed());
    }

    #[test]
    fn a_read_only_action_only_sets_the_review_notice() {
        let mut model = setup_model(SetupScreen::Setup);
        let index =
            model.actions().iter().position(|a| a.id == SetupActionId::ViewExample).unwrap();
        for _ in 0..index {
            model.handle_key(key(OverlayKeyCode::Down));
        }
        assert_eq!(model.handle_key(key(OverlayKeyCode::Enter)), SetupInputOutcome::Redraw);
        assert_eq!(
            model.notice().map(|(text, _)| text.to_string()),
            Some(
                "Review the details below. Press Enter on an action with a side effect to apply it."
                    .to_string()
            )
        );
    }

    #[test]
    fn a_dropped_setup_sender_clears_the_busy_latch() {
        let mut model = setup_model(SetupScreen::Setup);
        let index = model
            .actions()
            .iter()
            .position(|a| a.id == SetupActionId::ScaffoldProject)
            .unwrap();
        for _ in 0..index {
            model.handle_key(key(OverlayKeyCode::Down));
        }
        model.handle_key(key(OverlayKeyCode::Enter));
        assert!(model.is_busy());
        model.finish_job(McpSetupOverlay::cancelled_result(&SetupJob::ScaffoldProject));
        assert!(!model.is_busy(), "the finally clause must run on the rejection arm too");
    }

    #[test]
    fn adding_a_known_server_notices_the_display_name() {
        let mut model = setup_model(SetupScreen::Setup);
        model.finish_job(SetupJobResult::AddKnownServer(Ok(AddServerOutcome {
            path: PathBuf::from("/repo/.mcp.json"),
            server_name: "Chrome DevTools".into(),
        })));
        assert_eq!(
            model.notice().map(|(text, _)| text.to_string()),
            Some(
                "Added Chrome DevTools to /repo/.mcp.json. Pi will reload after this panel closes."
                    .to_string()
            )
        );
        // ... while the key written into the file is the preset's id.
        let presets = crate::config::known_server_presets();
        assert_eq!(presets[4].id, "chrome-devtools");
        assert_eq!(presets[4].name, "Chrome DevTools");
    }

    #[test]
    fn the_empty_import_selection_warns_without_starting_a_job() {
        let mut model = setup_model(SetupScreen::Imports);
        assert_eq!(model.handle_key(key(OverlayKeyCode::Enter)), SetupInputOutcome::Redraw);
        assert_eq!(
            model.notice().map(|(text, tone)| (text.to_string(), tone)),
            Some((
                "Select at least one compatibility import first.".to_string(),
                NoticeTone::Warning
            ))
        );
        assert!(!model.is_busy());
    }

    #[test]
    fn the_diff_cap_is_eighteen_lines_with_a_singular_overflow_at_one() {
        let mut preview = empty_preview();
        preview.diff_text = (0..25).map(|i| format!("+line{i}")).collect::<Vec<_>>().join("\n");
        let lines = McpSetupPanelModel::format_write_preview("T", &preview, &[], 74);
        assert!(lines.contains(&"\u{2026} 7 more diff lines".to_string()), "{lines:#?}");

        preview.diff_text = (0..19).map(|i| format!("+line{i}")).collect::<Vec<_>>().join("\n");
        let lines = McpSetupPanelModel::format_write_preview("T", &preview, &[], 74);
        assert!(lines.contains(&"\u{2026} 1 more diff line".to_string()), "{lines:#?}");
    }

    #[test]
    fn the_two_summary_lines_follow_their_own_pluralisation_rules() {
        let model = setup_model(SetupScreen::Empty);
        assert_eq!(model.discovery_summary_line().1, "No MCP config is active yet.");
        assert_eq!(
            model.secondary_summary_line(),
            "Create a shared .mcp.json, adopt host imports, or quick-add RepoPrompt from this screen."
        );

        let mut completed = setup_model(SetupScreen::Empty);
        completed.onboarding.setup_completed = true;
        assert_eq!(completed.discovery_summary_line().1, "No MCP servers are active right now.");
    }

    #[test]
    fn the_precedence_preview_names_cyrups_project_override_dir() {
        let model = setup_model(SetupScreen::Setup);
        let action = SetupAction {
            id: SetupActionId::ShowPrecedence,
            label: String::new(),
            description: String::new(),
            preset: None,
        };
        let preview = model.action_preview(Some(&action), 74);
        assert!(preview.contains(&"5. .mcp.json".to_string()));
        assert!(
            preview.contains(&format!("6. {}/mcp.json", crate::config::PROJECT_OVERRIDE_DIR)),
            "{preview:#?}"
        );
        assert!(!preview.iter().any(|l| l.contains(".pi/mcp.json")));
    }

    #[test]
    fn the_compact_window_shows_seven_rows_with_an_above_marker() {
        let mut model = setup_model(SetupScreen::Setup);
        let total = model.actions().len();
        assert!(total >= 9, "the fixture has enough actions to window");
        for _ in 0..(total - 1) {
            model.handle_key(key(OverlayKeyCode::Down));
        }
        let (start, end) = model.visible_action_range(total);
        assert_eq!(end - start, COMPACT_ACTION_ROWS);
        assert_eq!(end, total, "the cursor is on the last row");
        let frame: Vec<String> = model.render(60).iter().map(OverlayLine::plain_text).collect();
        assert!(frame.iter().any(|l| l.contains("more above")), "{frame:#?}");
        assert!(!frame.iter().any(|l| l.contains("more below")));
        assert!(frame.iter().any(|l| l.contains("Enter select")), "the compact hint");
    }

    #[test]
    fn the_setup_frame_pads_every_row_to_the_panel_width() {
        for width in [60usize, 92] {
            let model = setup_model(SetupScreen::Setup);
            for line in model.render(width) {
                assert_eq!(visible_width(&line.plain_text()), width, "{:?}", line.plain_text());
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // The footer segment and the shared-config notice
    // ---------------------------------------------------------------------------------------

    #[test]
    fn the_footer_segment_has_three_shapes_and_one_cleared_state() {
        let mut config = McpConfig::default();
        config
            .mcp_servers
            .insert("a".into(), ServerEntry { command: Some("x".into()), ..ServerEntry::default() });
        config.mcp_servers.insert(
            "b".into(),
            ServerEntry { command: Some("x".into()), disabled: Some(true), ..ServerEntry::default() },
        );
        let counts = FooterCounts::from_config(&config, 1);
        assert_eq!(counts, FooterCounts { configured: 2, connected: 1, enabled: 1, disabled: 1 });

        assert_eq!(
            footer_status_text(&config, counts).as_deref(),
            Some("\u{1f50c} MCP: 1 server enabled (1 connected) (1 disabled)")
        );

        config.settings =
            Some(McpSettings { mcp_footer_status: Some(crate::config::FooterStatus::Compact), ..McpSettings::default() });
        assert_eq!(footer_status_text(&config, counts).as_deref(), Some("MCP 1/1"));

        config.settings =
            Some(McpSettings { mcp_footer_status: Some(crate::config::FooterStatus::Off), ..McpSettings::default() });
        assert_eq!(footer_status_text(&config, counts), None);

        assert_eq!(footer_status_text(&McpConfig::default(), FooterCounts::default()), None);
    }

    #[test]
    fn the_shared_config_notice_is_a_one_shot_a_fingerprint_does_not_re_arm() {
        let summary = crate::config::McpStandardConfigSummary {
            sources: vec![crate::config::ConfigDiscoverySource {
                id: SourceId::SharedGlobal,
                label: "global MCP config",
                path: PathBuf::from("/home/u/.config/mcp/mcp.json"),
                exists: true,
                scope: crate::config::SourceScope::Global,
                kind: crate::config::DiscoveryKind::Shared,
                server_count: 2,
                contributes: true,
            }],
            has_shared_servers: true,
            fingerprint: "fp-1".into(),
        };
        let (lines, fingerprint) =
            shared_config_notice_lines(&summary, &OnboardingState::default());
        assert_eq!(
            lines[0],
            "Using standard MCP config from /home/u/.config/mcp/mcp.json."
        );
        assert_eq!(fingerprint.as_deref(), Some("fp-1"));

        let shown = OnboardingState { shared_config_hint_shown: true, ..OnboardingState::default() };
        assert!(shared_config_notice_lines(&summary, &shown).0.is_empty());
        // A changed fingerprint does NOT re-arm it: the flag is a plain boolean.
        let shown_with_old_fingerprint = OnboardingState {
            shared_config_hint_shown: true,
            last_discovery_fingerprint: Some("fp-0".into()),
            ..OnboardingState::default()
        };
        assert!(shared_config_notice_lines(&summary, &shown_with_old_fingerprint).0.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // Entry points — the two non-TUI refusals (`__tests__/commands-panel-non-tui.test.ts`
    // @v2.26.1, upstream `5787ecd`)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn the_two_non_tui_refusals_are_byte_exact_and_name_different_escape_hatches() {
        // `commands.ts:416` @v2.26.1 — `/mcp setup` outside the terminal.
        assert_eq!(
            panel_unavailable_message("rpc"),
            "The interactive MCP setup panel is only available in the terminal UI (current mode: rpc). Edit .mcp.json directly, or run /mcp status to review servers."
        );
        // `commands.ts:613` @v2.26.1 — the `/mcp-auth` picker outside the terminal.
        assert_eq!(
            auth_panel_unavailable_message("print"),
            "The interactive MCP auth panel is only available in the terminal UI (current mode: print). Use /mcp-auth <server> to authenticate a specific server."
        );

        // The upstream test asserts only `stringContaining("/mcp-auth <server>")` on the auth arm,
        // because the point of that arm is the escape hatch it offers, not the sentence: an auth
        // refusal that pointed at `.mcp.json` would leave the user editing a file that cannot hold
        // an OAuth token. Pin the distinction, not just the bytes.
        let auth = auth_panel_unavailable_message("json");
        assert!(auth.contains("/mcp-auth <server>"), "{auth}");
        assert!(!auth.contains(".mcp.json"), "{auth}");
        assert!(panel_unavailable_message("json").contains(".mcp.json"));

        // Both interpolate `HostCtx::mode` verbatim — the user needs to know which mode refused.
        assert!(panel_unavailable_message("json").contains("current mode: json"));
        assert!(auth.contains("current mode: json"));
    }
}
