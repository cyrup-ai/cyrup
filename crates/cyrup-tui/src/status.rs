//! The footer chrome (R-10-028; arch-10 §2 `chrome/footer.rs`; spec/tui/01 §4).
//!
//! A 1:1 port of Pi's `components/footer.ts` two-line footer:
//! - **Line 1** — home-relativized cwd, `(git branch)`, ` • session name` (`footer.ts:116-130`),
//!   right-truncated to width with a dim ellipsis (`footer.ts:230`).
//! - **Line 2** — the usage/cost/context **left** cluster (`↑in ↓out R{cache_read} W{cache_write}
//!   CH{hit}% ${cost}[ (sub)] {pct}%/{window}[ (auto)] • xp`, `footer.ts:132-165`) and a
//!   **right-aligned** `[(provider) ]{model}[ • {thinking}]` cluster (`footer.ts:170-221`).
//!
//! All footer text is `muted`/`dim` except the context-% segment, which colors `warning` above 70%
//! and `error` above 90% (`footer.ts:155-161`), and the experimental `xp` marker which is bold
//! `warning` (`footer.ts:164`). Token counts use Pi's `formatTokens` thresholds (`footer.ts:22-29`).
//!
//! ### Parity notes (closing gap 22 — footer divergence)
//! - The right cluster is the **model + thinking level**, never an invented `streaming`/`idle` word —
//!   Pi shows `{model} • thinking off` / `{model} • {level}` when the model supports reasoning
//!   (`footer.ts:184-189`); the working/idle indicator is the separate status band (spec/tui/01 §6).
//! - Cumulative usage is **summed across turns** with the cache-hit rate taken from the **latest**
//!   turn over `input + cacheRead + cacheWrite` (`footer.ts:86-107`), not a single per-turn count.
//! - The right cluster is right-aligned by padding (minimum **2** spaces), truncated right-side when
//!   it does not fit; the whole left cluster is `truncateToWidth`'d with a `...` ellipsis when it
//!   overflows (`footer.ts:172-221`). Visible width uses `Span::width` (unicode-width), never
//!   `str::len` (spec/tui/01 §8).

use cyrup_core::Usage;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::component::Component;
use crate::theme::UiTheme;

/// The style role a left-cluster segment carries, resolved against the live theme at render time.
#[derive(Clone, Copy, Debug)]
enum SegStyle {
    /// Default footer body — `muted`.
    Muted,
    /// Context-% segment, colored by band (`>90` error, `>70` warning, else muted).
    Context(f64),
    /// The `•` separator before the experimental marker — `dim`.
    Dim,
    /// The `xp` experimental marker — bold `warning`.
    BoldWarning,
}

impl SegStyle {
    fn resolve(self, theme: &UiTheme) -> Style {
        match self {
            SegStyle::Muted => theme.muted_style(),
            SegStyle::Context(pct) => context_style(pct, theme),
            SegStyle::Dim => theme.dim_style(),
            SegStyle::BoldWarning => theme.warning_style().add_modifier(Modifier::BOLD),
        }
    }
}

/// The footer state (Pi `FooterData`, `footer.ts:131-228`).
#[derive(Clone, Debug, Default)]
pub struct StatusLine {
    /// Active model label (`provider/model`).
    pub model: String,
    /// Whether a run is currently streaming. Drives the separate status band (spec/tui/01 §6), **not**
    /// the footer right cluster — Pi's footer never shows a streaming/idle word.
    pub streaming: bool,
    /// Running total token count for the latest assistant turn (compat fallback; superseded by `usage`).
    pub tokens: u64,
    /// Number of queued (steering + follow-up) messages (R-10-030).
    pub queued: usize,
    /// Cumulative token + cost usage across the session (`footer.ts:86-107`). Accumulated via
    /// [`add_usage`](Self::add_usage), one call per finalized assistant turn.
    pub usage: Usage,
    /// Cache-hit rate (%) of the **latest** assistant turn (`footer.ts:102-105`), if that turn had any
    /// prompt tokens. `None` ⇒ no `CH` segment.
    pub latest_cache_hit: Option<f64>,
    /// Home-relativized current working directory (`footer.ts:31-43`).
    pub cwd: String,
    /// Git branch, if known (`(branch)` on line 1).
    pub branch: Option<String>,
    /// Session display name, if set (` • {name}` on line 1).
    pub session_name: Option<String>,
    /// Context-window utilization fraction in `0.0..=1.0` (`getContextUsage`), if known.
    pub context_percent: Option<f64>,
    /// Context-window size in tokens (the `/{window}` part), if known.
    pub context_window: Option<u64>,
    /// Auto-compaction enabled → ` (auto)` suffix on the context segment.
    pub auto_compact: bool,
    /// Current reasoning level (`off`…`xhigh`/`max`). Rendered only when [`reasoning`](Self::reasoning) is set.
    pub thinking_level: String,
    /// Whether the active model supports reasoning (`state.model?.reasoning`, `footer.ts:185`). When
    /// set, the right cluster appends ` • thinking off` / ` • {level}`.
    pub reasoning: bool,
    /// Active provider id (`anthropic`, `openai`, …), for the optional `(provider)` right-cluster prefix.
    pub provider: Option<String>,
    /// Number of configured providers; the `(provider)` prefix is shown only when this is `> 1` and it
    /// still fits (`footer.ts:193-199`).
    pub provider_count: usize,
    /// Cost paid via an OAuth subscription → ` (sub)` suffix on the cost segment (`footer.ts:142-145`).
    pub using_subscription: bool,
    /// Experimental features enabled → trailing `• xp` marker (`footer.ts:163-165`).
    pub experimental: bool,
    /// Extension-published status texts, keyed by extension id (`footer.ts:233-242`). Rendered as a
    /// **third** footer line, the values sorted by key, sanitized (control chars → space, collapsed),
    /// space-joined, width-truncated with a dim `...` ellipsis. Empty ⇒ no third line.
    pub extension_statuses: std::collections::BTreeMap<String, String>,
}

impl StatusLine {
    /// A fresh footer with the given model label.
    pub fn new(model: impl Into<String>) -> Self {
        StatusLine { model: model.into(), ..Self::default() }
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }
    pub fn set_streaming(&mut self, streaming: bool) {
        self.streaming = streaming;
    }
    pub fn set_tokens(&mut self, tokens: u64) {
        self.tokens = tokens;
    }
    pub fn set_queued(&mut self, queued: usize) {
        self.queued = queued;
    }
    /// Accumulate one finalized turn's `Usage` into the cumulative session totals and record that
    /// turn's cache-hit rate (`footer.ts:86-107`). Token sums saturate (never panic / wrap).
    pub fn add_usage(&mut self, turn: &Usage) {
        self.add_usage_totals(turn);
        // Latest-turn cache-hit rate over the turn's full prompt (`footer.ts:102-105`).
        let prompt = turn.input.saturating_add(turn.cache_read).saturating_add(turn.cache_write);
        self.latest_cache_hit =
            if prompt > 0 { Some((turn.cache_read as f64 / prompt as f64) * 100.0) } else { None };
    }

    /// Accumulate usage that is NOT an assistant turn — today, the usage a tool reported for its own
    /// execution (`footer.ts:99-101`: `else if (… role === "toolResult" && entry.message.usage)
    /// addUsageToTotals(usageTotals, entry.message.usage)`).
    ///
    /// Deliberately does NOT touch [`latest_cache_hit`](Self::latest_cache_hit): upstream that
    /// figure is written only inside the ASSISTANT branch, so a tool result must never restate the
    /// footer's `CH` segment (and a tool's usage typically has no prompt tokens at all, which would
    /// blank it).
    pub fn add_usage_totals(&mut self, turn: &Usage) {
        self.usage.input = self.usage.input.saturating_add(turn.input);
        self.usage.output = self.usage.output.saturating_add(turn.output);
        self.usage.cache_read = self.usage.cache_read.saturating_add(turn.cache_read);
        self.usage.cache_write = self.usage.cache_write.saturating_add(turn.cache_write);
        self.usage.total_tokens = self.usage.total_tokens.saturating_add(turn.total_tokens);
        self.usage.cost.total += turn.cost.total;
    }
    pub fn set_cwd(&mut self, cwd: impl Into<String>) {
        self.cwd = cwd.into();
    }
    pub fn set_branch(&mut self, branch: Option<String>) {
        self.branch = branch;
    }
    pub fn set_session_name(&mut self, name: Option<String>) {
        self.session_name = name;
    }
    /// Set the context-window utilization (fraction `0..=1`), window size, and auto-compaction flag.
    pub fn set_context(&mut self, percent: f64, window: u64, auto: bool) {
        self.context_percent = Some(percent.clamp(0.0, 1.0));
        self.context_window = Some(window);
        self.auto_compact = auto;
    }
    /// Set the reasoning level shown in the right cluster (`footer.ts:186-188`).
    pub fn set_thinking_level(&mut self, level: impl Into<String>) {
        self.thinking_level = level.into();
    }
    /// Set whether the active model supports reasoning (gates the ` • thinking …` suffix).
    pub fn set_reasoning(&mut self, reasoning: bool) {
        self.reasoning = reasoning;
    }
    /// Set the active provider id (for the optional `(provider)` prefix).
    pub fn set_provider(&mut self, provider: Option<String>) {
        self.provider = provider;
    }
    /// Set how many providers are configured (the `(provider)` prefix needs `> 1`).
    pub fn set_provider_count(&mut self, count: usize) {
        self.provider_count = count;
    }
    /// Set whether the active cost is paid via an OAuth subscription (` (sub)` suffix).
    pub fn set_using_subscription(&mut self, sub: bool) {
        self.using_subscription = sub;
    }
    /// Set whether experimental features are enabled (the trailing `• xp` marker).
    pub fn set_experimental(&mut self, on: bool) {
        self.experimental = on;
    }
    /// Publish (or replace) one extension's footer status text (`footer.ts:233`). An empty/whitespace
    /// value removes the entry, matching Pi's clear semantics.
    pub fn set_extension_status(&mut self, id: impl Into<String>, text: impl Into<String>) {
        let id = id.into();
        let text = text.into();
        if text.trim().is_empty() {
            self.extension_statuses.remove(&id);
        } else {
            self.extension_statuses.insert(id, text);
        }
    }
    /// Remove one extension's footer status.
    pub fn clear_extension_status(&mut self, id: &str) {
        self.extension_statuses.remove(id);
    }
    /// Whether a third (extension-status) footer line is present.
    pub fn has_extension_statuses(&self) -> bool {
        !self.extension_statuses.is_empty()
    }
    /// The third footer line as plain text: extension statuses sorted by key (the `BTreeMap` is already
    /// key-ordered), each sanitized, space-joined (`footer.ts:235-240`). Not width-truncated here.
    pub fn extension_status_text(&self) -> String {
        self.extension_statuses
            .values()
            .map(|t| sanitize_status_text(t))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Footer **line 1**: `~/path (branch) • session` (`footer.ts:116-130`).
    pub fn location_text(&self) -> String {
        let mut s = if self.cwd.is_empty() { String::new() } else { self.cwd.clone() };
        if let Some(branch) = &self.branch {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push('(');
            s.push_str(branch);
            s.push(')');
        }
        if let Some(name) = &self.session_name {
            s.push_str(" • ");
            s.push_str(name);
        }
        s
    }

    /// Footer **line 2** as plain text (`{left cluster}  {right cluster}`) — useful for assertions; the
    /// rendered form colors the segments and right-aligns the model (see [`Component::render`]).
    pub fn text(&self) -> String {
        let left = self.usage_cluster();
        let right = self.right_cluster();
        if left.is_empty() {
            return right;
        }
        format!("{left}  {right}")
    }

    /// The left usage/cost/context cluster as plain text (`footer.ts:132-165`).
    pub fn usage_cluster(&self) -> String {
        self.left_segments()
            .into_iter()
            .map(|(t, _)| t)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The left-cluster segments (text + style role), space-joined at render time (`footer.ts:132-165`).
    fn left_segments(&self) -> Vec<(String, SegStyle)> {
        let mut segs: Vec<(String, SegStyle)> = Vec::new();
        if self.usage.input > 0 {
            segs.push((format!("↑{}", format_tokens(self.usage.input)), SegStyle::Muted));
        }
        if self.usage.output > 0 {
            segs.push((format!("↓{}", format_tokens(self.usage.output)), SegStyle::Muted));
        }
        if self.usage.cache_read > 0 {
            segs.push((format!("R{}", format_tokens(self.usage.cache_read)), SegStyle::Muted));
        }
        if self.usage.cache_write > 0 {
            segs.push((format!("W{}", format_tokens(self.usage.cache_write)), SegStyle::Muted));
        }
        if let Some(hit) = self.cache_hit_rate() {
            segs.push((format!("CH{hit:.1}%"), SegStyle::Muted));
        }
        if self.usage.cost.total > 0.0 || self.using_subscription {
            let sub = if self.using_subscription { " (sub)" } else { "" };
            segs.push((format!("${:.3}{sub}", self.usage.cost.total), SegStyle::Muted));
        }
        if let (Some(ctx), Some(pct)) = (self.context_text(), self.context_percent) {
            segs.push((ctx, SegStyle::Context(pct)));
        }
        // Legacy fallback: a bare `set_tokens` still shows a count when no per-field usage exists.
        if segs.is_empty() && self.tokens > 0 {
            segs.push((format!("{} tokens", self.tokens), SegStyle::Muted));
        }
        // cyrup extension (R-10-030): queued follow-up count (Pi has no footer queue segment).
        if self.queued > 0 {
            segs.push((format!("{} queued", self.queued), SegStyle::Muted));
        }
        // Experimental marker `• xp` (`footer.ts:163-165`): dim bullet + bold-warning `xp`.
        if self.experimental {
            segs.push(("•".to_string(), SegStyle::Dim));
            segs.push(("xp".to_string(), SegStyle::BoldWarning));
        }
        segs
    }

    /// The `{pct}%/{window}` context segment with optional ` (auto)` (`footer.ts:151-154`).
    fn context_text(&self) -> Option<String> {
        let pct = self.context_percent?;
        let window = self.context_window?;
        let mut s = format!("{:.1}%/{}", pct * 100.0, format_tokens(window));
        if self.auto_compact {
            s.push_str(" (auto)");
        }
        Some(s)
    }

    /// Cache-hit rate % for the `CH` segment: the latest turn's rate, shown only when the cumulative
    /// totals have any cache activity (`footer.ts:137-139`).
    fn cache_hit_rate(&self) -> Option<f64> {
        if self.usage.cache_read == 0 && self.usage.cache_write == 0 {
            return None;
        }
        self.latest_cache_hit
    }

    /// The right cluster **without** the optional `(provider)` prefix (`footer.ts:184-189`):
    /// `{model}`, plus ` • thinking off` / ` • {level}` when the model supports reasoning.
    fn right_cluster(&self) -> String {
        let model = if self.model.is_empty() { "no-model" } else { self.model.as_str() };
        if self.reasoning {
            let level = if self.thinking_level.is_empty() { "off" } else { self.thinking_level.as_str() };
            if level == "off" {
                format!("{model} • thinking off")
            } else {
                format!("{model} • {level}")
            }
        } else {
            model.to_string()
        }
    }
}

/// Pi `formatTokens` (`footer.ts:22-29`): `<1000` plain · `<10k` `X.Xk` · `<1M` `Xk` · `<10M`
/// `X.XM` · else `XM`. Total and allocation-light; never panics.
pub fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1_000)
    } else if n < 10_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{}M", n / 1_000_000)
    }
}

impl Component for StatusLine {
    /// Render the (up to) two footer lines into `area`. Line 1 is the location (muted, width-truncated);
    /// line 2 is the usage cluster (muted, context-% colored by band) plus the right-aligned model +
    /// thinking cluster, padded to `area.width` (`footer.ts:204-231`).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let width = area.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        if area.height >= 2 {
            let loc = truncate_to_width(&self.location_text(), width, "...");
            lines.push(Line::styled(loc, theme.muted_style()));
        }
        lines.push(self.usage_line(width, theme));
        // Third line: extension statuses, only when present and the area has a spare row
        // (`footer.ts:232-241`). Width-truncated with a dim `...` ellipsis to match footer style.
        if area.height >= 3 && self.has_extension_statuses() {
            let status = truncate_to_width(&self.extension_status_text(), width, "...");
            lines.push(Line::styled(status, theme.dim_style()));
        }
        let para = Paragraph::new(lines).style(theme.base_style());
        frame.render_widget(para, area);
    }
}

/// Sanitize an extension status for single-line display (`footer.ts:12-18`): CR/LF/Tab → space,
/// collapse runs of spaces, trim. Keeps the third footer line stable regardless of extension output.
fn sanitize_status_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        let c = if matches!(ch, '\r' | '\n' | '\t') { ' ' } else { ch };
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

impl StatusLine {
    /// Build line 2: the (context-colored) left cluster, padding, and the right-aligned model cluster,
    /// reproducing `footer.ts:172-221`'s width math (min 2-space gap, right-side then left-side
    /// truncation). All measurement is visible width (`Span::width`), never `str::len`.
    fn usage_line(&self, width: usize, theme: &UiTheme) -> Line<'static> {
        let segs = self.left_segments();
        let mut left_spans: Vec<Span<'static>> = Vec::new();
        let mut left_plain = String::new();
        for (i, (text, style)) in segs.into_iter().enumerate() {
            if i > 0 {
                left_spans.push(Span::styled(" ", theme.muted_style()));
                left_plain.push(' ');
            }
            left_plain.push_str(&text);
            left_spans.push(Span::styled(text, style.resolve(theme)));
        }
        let left_width = str_width(&left_plain);

        // Left cluster overflows: degrade to the truncated left only (spec/tui/01 §8; footer.ts:175-178).
        if left_width > width {
            let t = truncate_to_width(&left_plain, width, "...");
            return Line::from(vec![Span::styled(t, theme.muted_style())]);
        }

        // Choose the right cluster, prepending `(provider)` only if there are multiple providers and it
        // still fits (`footer.ts:191-199`).
        let base_right = self.right_cluster();
        let mut right = base_right.clone();
        if self.provider_count > 1
            && let Some(provider) = self.provider.as_deref()
        {
            let with_provider = format!("({provider}) {base_right}");
            if left_width.saturating_add(2).saturating_add(str_width(&with_provider)) <= width {
                right = with_provider;
            }
        }
        let right_width = str_width(&right);

        let muted = theme.muted_style();
        let mut spans = left_spans;
        if left_width.saturating_add(2).saturating_add(right_width) <= width {
            let pad = width.saturating_sub(left_width).saturating_sub(right_width);
            spans.push(Span::styled(" ".repeat(pad), muted));
            spans.push(Span::styled(right, muted));
        } else {
            let avail = width.saturating_sub(left_width).saturating_sub(2);
            if avail > 0 {
                let truncated = truncate_to_width(&right, avail, "");
                let trunc_width = str_width(&truncated);
                let pad = width.saturating_sub(left_width).saturating_sub(trunc_width);
                spans.push(Span::styled(" ".repeat(pad), muted));
                spans.push(Span::styled(truncated, muted));
            }
            // else: no room for the right cluster at all — left only (`footer.ts:217-220`).
        }
        Line::from(spans)
    }
}

/// Context-% color band (`footer.ts:155-161`): `>90%` error, `>70%` warning, else muted.
fn context_style(percent: f64, theme: &UiTheme) -> Style {
    let pct = percent * 100.0;
    if pct > 90.0 {
        theme.error_style()
    } else if pct > 70.0 {
        theme.warning_style()
    } else {
        theme.muted_style()
    }
}

/// Visible (terminal column) width of `s`, unicode-width correct via ratatui's `Span::width`
/// (mirrors Pi's `visibleWidth`, `footer.ts:2`). Never `str::len`.
fn str_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Visible width of a single `char` (no heap allocation).
fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    str_width(c.encode_utf8(&mut buf))
}

/// Right-truncate `s` to a visible width of `max`, appending `ellipsis` when truncation occurs
/// (mirrors Pi's `truncateToWidth`, `footer.ts:2`). Width-aware (CJK/emoji safe), never panics.
fn truncate_to_width(s: &str, max: usize, ellipsis: &str) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(str_width(ellipsis));
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = char_width(ch);
        if w.saturating_add(cw) > budget {
            break;
        }
        out.push(ch);
        w = w.saturating_add(cw);
    }
    out.push_str(ellipsis);
    out
}
