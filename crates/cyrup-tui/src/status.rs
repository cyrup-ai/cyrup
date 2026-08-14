//! The footer chrome (R-10-028; arch-10 §2 `chrome/footer.rs`; spec/tui/01 §4).
//!
//! A 1:1 port of Pi's `components/footer.ts` two-line footer:
//! - **Line 1** — home-relativized cwd, `(git branch)`, ` • session name` (`footer.ts:116-130`),
//!   right-truncated to width with a dim ellipsis (`footer.ts:230`).
//! - **Line 2** — the usage/cost/context **left** cluster (`↑in ↓out R{cache_read} W{cache_write}
//!   CH{hit}% ${cost}[ (sub)] {pct}%/{window}[ (auto)] • xp`, `footer.ts:132-165`) and a
//!   **right-aligned** `[(provider) ]{model}[ • {thinking}]` cluster (`footer.ts:170-221`).
//!
//! All footer text is the **`dim`** token — upstream wraps line 1 in `theme.fg("dim", pwd)` and
//! line 2 in `theme.fg("dim", statsLeft) + theme.fg("dim", remainder)` (v0.84.1 `footer.ts:225-229`)
//! — except the context-% segment, which colors `warning` above 70% and `error` above 90%
//! (`footer.ts:154-160`), and the experimental `xp` marker which is bold `warning` (`footer.ts:163`).
//! Line 3 is the one line upstream leaves UNstyled: only its ellipsis is `dim` (`footer.ts:240`).
//! Token counts use Pi's `formatTokens` thresholds (`footer.ts:24-30`).
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
    /// Default footer body — the **`dim`** token. Pi wraps the whole left cluster in
    /// `theme.fg("dim", statsLeft)` (v0.84.1 `footer.ts:225`), so every segment that does not paint
    /// itself inherits `dim` — including the `•` before the experimental marker, which upstream
    /// spells out as `theme.fg("dim", "•")` (`footer.ts:163`).
    Dim,
    /// Context-% segment, colored by band (`>90` error, `>70` warning, else the inherited `dim`).
    Context(f64),
    /// The `xp` experimental marker — bold `warning` (`footer.ts:163`).
    BoldWarning,
}

impl SegStyle {
    fn resolve(self, theme: &UiTheme) -> Style {
        match self {
            SegStyle::Dim => theme.dim_style(),
            SegStyle::Context(pct) => context_style(pct, theme),
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
    /// Running total token count for the latest assistant turn (superseded by `usage`).
    ///
    /// **Not rendered.** cyrup used to fall back to a `{n} tokens` segment when no per-field usage
    /// existed; pi's `statsParts` (`footer.ts:129-164`) has no such segment, and the slot it stood in
    /// for — the context segment — is now unconditional, so the fallback could never fire again.
    pub tokens: u64,
    /// Number of queued (steering + follow-up) messages.
    ///
    /// **Not rendered.** `footer.ts:129-164` builds exactly `↑ ↓ R W CH% $cost`, the context segment
    /// and `xp` — there is no queue segment upstream under any name, and the extra segment pushed the
    /// right-aligned model name over (or off) the line. Pi surfaces queued messages in the transcript.
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
    /// Context-window utilization fraction in `0.0..=1.0` — Pi's `contextUsage.percent / 100`
    /// (`footer.ts:110`). `None` is upstream's `percent: null`, which the segment renders as `?`
    /// (`footer.ts:151-152`); it is what a compaction leaves behind until the next LLM response.
    ///
    /// Only consulted when [`context_window`](Self::context_window) is `Some`.
    pub context_percent: Option<f64>,
    /// Context-window size in tokens (the `/{window}` part). `None` is upstream's
    /// `getContextUsage() === undefined` (no model, or a model with no declared window), where the
    /// `?? 0` fallbacks at `footer.ts:109-110` make the segment read `0.0%/0`.
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
    /// Cost paid via a provider **subscription** → ` (sub)` suffix on the cost segment
    /// (`footer.ts:142-145`).
    ///
    /// Not "the credential is an OAuth credential": pi narrowed this at v0.84.0 from `isUsingOAuth`
    /// to `isUsingSubscription` (`footer.ts:140` vs `v0.83.0:footer.ts:140`) precisely so a metered
    /// OAuth sign-in stops being labelled a subscription. The predicate lives in
    /// [`crate::App`]`::provider_uses_subscription`, which is the only production writer of this
    /// field; it is pushed on `model_changed` and on every credential change (`/login`, `/logout`,
    /// session bind/swap).
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
    /// Set the context-window utilization (fraction, `1.0` == 100%), window size, and
    /// auto-compaction flag.
    ///
    /// **Not clamped.** Upstream is `const percent = (estimate.tokens / contextWindow) * 100`
    /// (`agent-session.ts:3211`) and the footer prints `contextPercentValue.toFixed(1)`
    /// (`footer.ts:151`) — nothing anywhere caps it at 100. An over-budget context legitimately
    /// reads `112.3%`, in `error` red (`footer.ts:154-155`), which is exactly the moment a user most
    /// needs the number. Clamping pinned it at a calm-looking `100.0%`.
    pub fn set_context(&mut self, percent: f64, window: u64, auto: bool) {
        self.context_percent = Some(percent);
        self.context_window = Some(window);
        self.auto_compact = auto;
    }

    /// Fold one `getContextUsage()` answer into the footer, in Pi's own three shapes
    /// (`footer.ts:106-111`):
    ///
    /// | upstream | `window` | `percent` | segment |
    /// |---|---|---|---|
    /// | `{tokens, contextWindow, percent}` | `Some(w)` | `Some(f)` | `41.2%/200k` |
    /// | `{tokens: null, contextWindow, percent: null}` | `Some(w)` | `None` | `?/200k` |
    /// | `undefined` | `None` | ignored | `0.0%/0` |
    ///
    /// `percent` is a **fraction** (`1.0` == 100%), matching [`set_context`](Self::set_context); the
    /// session-side figure is a 0-100 percentage and is divided by the caller. Like `set_context` it
    /// is **not** clamped — see that method for why (`agent-session.ts:3211` has no cap).
    pub fn set_context_usage(&mut self, percent: Option<f64>, window: Option<u64>, auto: bool) {
        self.context_percent = percent;
        self.context_window = window;
        self.auto_compact = auto;
    }

    /// `footer.setAutoCompactEnabled(enabled)` (`footer.ts` setter; called from
    /// `interactive-mode.ts:572`, `:1902` and `:4418`) — the ` (auto)` suffix on the context
    /// segment, on its own, without disturbing the occupancy figures.
    ///
    /// It needs its own setter because upstream's third call site is a **settings toggle**
    /// (`onAutoCompactChange`, `:4417-4419`), not a turn event: the flag can change while nothing
    /// else about the context has.
    pub fn set_auto_compact(&mut self, auto: bool) {
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
    /// Set whether the active cost is paid via a provider subscription (` (sub)` suffix). Production
    /// callers go through [`crate::App`]`::refresh_subscription_marker`, which owns the predicate.
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
            segs.push((format!("↑{}", format_tokens(self.usage.input)), SegStyle::Dim));
        }
        if self.usage.output > 0 {
            segs.push((format!("↓{}", format_tokens(self.usage.output)), SegStyle::Dim));
        }
        if self.usage.cache_read > 0 {
            segs.push((format!("R{}", format_tokens(self.usage.cache_read)), SegStyle::Dim));
        }
        if self.usage.cache_write > 0 {
            segs.push((format!("W{}", format_tokens(self.usage.cache_write)), SegStyle::Dim));
        }
        if let Some(hit) = self.cache_hit_rate() {
            segs.push((format!("CH{hit:.1}%"), SegStyle::Dim));
        }
        if self.usage.cost.total > 0.0 || self.using_subscription {
            let sub = if self.using_subscription { " (sub)" } else { "" };
            segs.push((format!("${:.3}{sub}", self.usage.cost.total), SegStyle::Dim));
        }
        // The context segment is UNCONDITIONAL upstream — `statsParts.push(contextPercentStr)` at
        // `footer.ts:161` sits outside every `if`. cyrup used to gate it on both `context_percent`
        // and `context_window` being `Some`, and nothing in production ever set either, so pi's
        // most-watched footer segment (the only one that turns yellow/red) never rendered at all.
        segs.push((
            self.context_text(),
            // `contextPercentValue = contextUsage?.percent ?? 0` (`footer.ts:110`): the unknown (`?`)
            // case takes the band for 0%, i.e. no colour of its own.
            SegStyle::Context(self.context_percent.unwrap_or(0.0)),
        ));
        // Experimental marker `• xp` (`footer.ts:162-164`): dim bullet + bold-warning `xp`.
        if self.experimental {
            segs.push(("•".to_string(), SegStyle::Dim));
            segs.push(("xp".to_string(), SegStyle::BoldWarning));
        }
        segs
    }

    /// The `{pct}%/{window}` context segment with optional ` (auto)` (`footer.ts:148-153`).
    ///
    /// Three shapes, matching upstream's two `??` fallbacks and its `!== null` test:
    /// `41.2%/200k (auto)` when the occupancy is known, `?/200k (auto)` when a compaction has left
    /// it unknown, and `0.0%/0 (auto)` when `getContextUsage()` itself returned `undefined`.
    fn context_text(&self) -> String {
        let auto = if self.auto_compact { " (auto)" } else { "" };
        // `contextWindow = contextUsage?.contextWindow ?? state.model?.contextWindow ?? 0` and
        // `contextPercentValue = contextUsage?.percent ?? 0` (`footer.ts:109-110`). `undefined` is
        // returned exactly when the model has no positive window, so the second fallback is 0 too.
        let Some(window) = self.context_window else {
            return format!("0.0%/0{auto}");
        };
        let window = format_tokens(window);
        match self.context_percent {
            Some(pct) => format!("{:.1}%/{window}{auto}", pct * 100.0),
            // `contextPercent === "?"` branch (`footer.ts:151-152`).
            None => format!("?/{window}{auto}"),
        }
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

/// Pi `formatTokens` (v0.84.1 `footer.ts:24-30`): `<1000` plain · `<10k` `X.Xk` · `<1M` `Xk` ·
/// `<10M` `X.XM` · else `XM`. Total and allocation-light; never panics.
///
/// The two integer branches are `Math.round(count / 1000)` (`:27`) and
/// `Math.round(count / 1000000)` (`:29`) — **rounding**, not truncation. cyrup used `n / 1_000`,
/// so 45 600 read `45k` where pi reads `46k` and 10 900 read `10k` where pi reads `11k`.
///
/// Both branches round half-up as `quotient + (remainder >= half)` rather than the obvious
/// `(n + half) / divisor`. The `+ half` form is **not** total: `n` here is a `u64` fed from
/// [`StatusLine::usage`], an accumulator, and this function is `pub` (re-exported at `lib.rs`), so
/// any `n > u64::MAX - 500_000` overflowed the `M` branch — a debug panic (`attempt to add with
/// overflow`) and a silently wrapped, absurd `M` count in release. The remainder form cannot
/// overflow for any `u64`; `format_tokens(u64::MAX)` is a covered case.
pub fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n < 1_000_000 {
        format!("{}k", (n / 1_000) + u64::from(n % 1_000 >= 500))
    } else if n < 10_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{}M", (n / 1_000_000) + u64::from(n % 1_000_000 >= 500_000))
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
            // `truncateToWidth(theme.fg("dim", pwd), width, theme.fg("dim", "..."))`
            // (`footer.ts:229`) — the whole location line, ellipsis included, is the `dim` token.
            // This was `muted_style()`, one shade too bright on every frame.
            let loc = truncate_to_width(&self.location_text(), width, "...");
            lines.push(Line::styled(loc, theme.dim_style()));
        }
        lines.push(self.usage_line(width, theme));
        // Third line: extension statuses, only when present and the area has a spare row
        // (`footer.ts:233-241`). Upstream styles ONLY the ellipsis —
        // `truncateToWidth(statusLine, width, theme.fg("dim", "..."))` (`:240`) — leaving the
        // statuses themselves at the terminal's default so an extension's own colours survive.
        // cyrup dimmed the whole row, which both darkened it and discarded that intent.
        if area.height >= 3 && self.has_extension_statuses() {
            let (body, truncated) = truncate_parts(&self.extension_status_text(), width, "...");
            let mut spans = vec![Span::styled(body, theme.base_style())];
            if truncated {
                spans.push(Span::styled("...", theme.dim_style()));
            }
            lines.push(Line::from(spans));
        }
        let para = Paragraph::new(lines).style(theme.base_style());
        frame.render_widget(para, area);
    }
}

/// Port of `areExperimentalFeaturesEnabled` (v0.84.1
/// `coding-agent/src/core/experimental.ts:1-3`: `return process.env.PI_EXPERIMENTAL === "1"`) —
/// the predicate gating the footer's `• xp` marker (`footer.ts:162`).
///
/// `CYRUP_EXPERIMENTAL` is the renamed primary and `PI_EXPERIMENTAL` survives as the
/// lower-precedence fallback, exactly as `crates/cyrup/src/startup.rs:76-84` reads it for the
/// first-run wizard. Both must be the literal `"1"`; any other value is off.
pub fn experimental_features_enabled() -> bool {
    experimental_features_enabled_from(|k| std::env::var(k).ok())
}

/// [`experimental_features_enabled`] against an injected environment, so the precedence can be
/// tested without mutating the process env (same seam as `ColorMode::detect_from`).
pub fn experimental_features_enabled_from(get: impl Fn(&str) -> Option<String>) -> bool {
    get("CYRUP_EXPERIMENTAL").as_deref() == Some("1")
        || get("PI_EXPERIMENTAL").as_deref() == Some("1")
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
                // `statsParts.join(" ")` inside `theme.fg("dim", statsLeft)` (`footer.ts:166,225`).
                left_spans.push(Span::styled(" ", theme.dim_style()));
                left_plain.push(' ');
            }
            left_plain.push_str(&text);
            left_spans.push(Span::styled(text, style.resolve(theme)));
        }
        let left_width = str_width(&left_plain);

        // Left cluster overflows: degrade to the truncated left only (spec/tui/01 §8;
        // `footer.ts:174-177`). Upstream truncates the STYLED string — `truncateToWidth` carries
        // `pendingAnsi` across the cut (`tui/src/utils.ts:1119-1122`, `:1129-1131`) and the footer's
        // own comment at `:222-224` spells out why it dims the parts around the coloured section
        // rather than the whole line: "statsLeft may contain color codes (for context %)". Flattening
        // it into one `dim` span discarded exactly those codes — the >90% error red and >70% warning
        // yellow on the context segment — at precisely the width where the user has the least text
        // to read and most needs the colour.
        if left_width > width {
            return Line::from(truncate_spans(left_spans, width, "...", theme.dim_style()));
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

        // `const dimRemainder = theme.fg("dim", remainder)` where `remainder` is the padding plus the
        // right cluster (`footer.ts:226-227`) — the model name and thinking level are `dim`, not
        // `muted`.
        let dim = theme.dim_style();
        let mut spans = left_spans;
        if left_width.saturating_add(2).saturating_add(right_width) <= width {
            let pad = width.saturating_sub(left_width).saturating_sub(right_width);
            spans.push(Span::styled(" ".repeat(pad), dim));
            spans.push(Span::styled(right, dim));
        } else {
            let avail = width.saturating_sub(left_width).saturating_sub(2);
            if avail > 0 {
                let truncated = truncate_to_width(&right, avail, "");
                let trunc_width = str_width(&truncated);
                let pad = width.saturating_sub(left_width).saturating_sub(trunc_width);
                spans.push(Span::styled(" ".repeat(pad), dim));
                spans.push(Span::styled(truncated, dim));
            }
            // else: no room for the right cluster at all — left only (`footer.ts:217-220`).
        }
        Line::from(spans)
    }
}

/// Context-% color band (`footer.ts:154-160`): `>90%` error, `>70%` warning, else no colour of its
/// own — which means the outer `theme.fg("dim", statsLeft)` wrapper (`:225`) shows through.
fn context_style(percent: f64, theme: &UiTheme) -> Style {
    let pct = percent * 100.0;
    if pct > 90.0 {
        theme.error_style()
    } else if pct > 70.0 {
        theme.warning_style()
    } else {
        theme.dim_style()
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
    let (mut body, truncated) = truncate_parts(s, max, ellipsis);
    if truncated {
        body.push_str(ellipsis);
    }
    body
}

/// [`truncate_to_width`] split into `(kept text, was it truncated)`, so a caller can style the
/// ellipsis differently from the body — which is what the third footer line needs
/// (`truncateToWidth(statusLine, width, theme.fg("dim", "..."))`, `footer.ts:240`: the ellipsis
/// carries the colour, the statuses do not). `ellipsis` is measured but never appended here.
fn truncate_parts(s: &str, max: usize, ellipsis: &str) -> (String, bool) {
    if str_width(s) <= max {
        return (s.to_string(), false);
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
    (out, true)
}

/// [`truncate_to_width`] over a **styled span vector**, preserving each span's own style across the
/// cut and appending `ellipsis` as a span of its own in `ellipsis_style`.
///
/// This is what pi's `truncateToWidth` does to `statsLeft` at `footer.ts:175`: the string it walks
/// already carries the context segment's `theme.fg("error"/"warning", …)` escape, and the ANSI-aware
/// branch of `truncateToWidth` accumulates each code in `pendingAnsi` and re-emits it with the next
/// kept character (`tui/src/utils.ts:1119-1122`), so the colour survives truncation. Reducing the
/// line to a single `dim` span instead silently deleted the only colour in the footer.
///
/// Width-aware (CJK/emoji safe) and total: no indexing, no panics.
fn truncate_spans(
    spans: Vec<Span<'static>>,
    max: usize,
    ellipsis: &str,
    ellipsis_style: Style,
) -> Vec<Span<'static>> {
    let total = spans.iter().map(|s| str_width(&s.content)).fold(0usize, usize::saturating_add);
    if total <= max {
        return spans;
    }
    let budget = max.saturating_sub(str_width(ellipsis));
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len().saturating_add(1));
    let mut w = 0usize;
    for span in spans {
        if w >= budget {
            break;
        }
        let span_width = str_width(&span.content);
        if w.saturating_add(span_width) <= budget {
            w = w.saturating_add(span_width);
            out.push(span);
            continue;
        }
        // The span straddling the cut: keep its prefix, in its own style.
        let (kept, _) = truncate_parts(&span.content, budget.saturating_sub(w), "");
        if !kept.is_empty() {
            out.push(Span::styled(kept, span.style));
        }
        break;
    }
    out.push(Span::styled(ellipsis.to_string(), ellipsis_style));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::StatusLine;

    /// Salvaged from the retired `tests/footer_extensions.rs`: the only two assertions there that
    /// were not already covered by `tests/footer_chrome_fidelity.rs` (rendered/resolved cell colour)
    /// and `tests/extension_ui_effects.rs` (the real `UiEffect` set/clear path). Both are pure
    /// statements about `extension_status_text` — `BTreeMap` key ordering and control-char collapse
    /// (`footer.ts:235-240`).
    #[test]
    fn extension_status_text_is_key_sorted_and_sanitized() {
        let mut status = StatusLine::new("anthropic/opus");
        // Inserted out of order; the BTreeMap keys order the output (alpha before zeta).
        status.set_extension_status("zeta", "z-status");
        status.set_extension_status("alpha", "a\n\tline");
        assert!(status.has_extension_statuses());
        assert_eq!(status.extension_status_text(), "a line z-status");
    }

    #[test]
    fn blank_extension_status_value_clears_the_entry() {
        let mut status = StatusLine::new("m");
        status.set_extension_status("ext", "busy");
        assert!(status.has_extension_statuses());
        status.set_extension_status("ext", "   ");
        assert!(!status.has_extension_statuses(), "blank value removes the entry");
    }
}
