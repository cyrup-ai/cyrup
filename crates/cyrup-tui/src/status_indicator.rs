//! The working / idle status band (spec/tui/01 §6; `components/status-indicator.ts`).
//!
//! Pi renders a fixed **2-row** band between the active turn and the editor (container index 5,
//! editor index 7) so the editor never jumps when a spinner appears. While a turn runs it shows a
//! `Loader`: a leading blank line then `{spinner} {message}` (`loader.ts:43-45`); idle is two blank
//! lines (`IdleStatus`, `status-indicator.ts:105-114`).
//!
//! `Loader extends Text` and is constructed `super("", 1, 0)` (`loader.ts:35`) — **paddingX 1** — so
//! `Text.render` emits `leftMargin + line + rightMargin` (`text.ts:70-76`). The rendered row is
//! ` ⠋ Working `, inset one column, not flush against the terminal edge.
//!
//! ## States (`status-indicator.ts:7`)
//! `working | retry | compaction | branchSummary`, plus idle — exactly one active at a time. Each
//! non-idle state is an accent/warning spinner + a muted message.
//!
//! The trailing `(<key> to cancel)` is **baked into the message by three of the four constructors**
//! — retry (`status-indicator.ts:47`), compaction (`:78-82`) and branch summary (`:100`), each from
//! the live keymap. `WorkingStatusIndicator` (`:29-40`) takes `this.workingMessage ??
//! defaultWorkingMessage` and appends NOTHING (`interactive-mode.ts:2074-2080`); the only place a
//! working message ever gains a suffix is `resetExtensionUI` (`:2188-2191`), which spells it
//! `(… to interrupt)` and fires on an extension reload, not on a turn. cyrup appended
//! ` ({hint} to cancel)` to every kind, adding 18 columns to `Working` for the whole duration of
//! every turn.
//!
//! ## Citation tags
//! The `interactive-mode.ts` and `loader.ts` lines added for TUI-030 (the working-indicator
//! family) are **@v0.84.2**, the checked-out upstream. They are NOT interchangeable with this
//! file's older @v0.83.0 cites: `setWorkingVisible` alone moved `:1877` → `:2091` between the two
//! tags, so an untagged number in this cluster would point at unrelated code.
//!
//! ## Spinner (spec/tui/01 §6.2)
//! Braille frames `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` advancing every **80 ms** (`loader.ts:11-12`). ratatui is
//! immediate-mode, so the frame is selected as `(elapsed_ms / 80) % frames.len()` — no timer thread;
//! the app loop schedules an 80 ms redraw tick only while an indicator is active (§6.2 / §10).

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::UiTheme;

/// The Braille spinner frames (`loader.ts:11`).
pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Spinner advance interval (`loader.ts:12`).
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// The active status-indicator kind (`StatusIndicatorKind`, `status-indicator.ts:7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndicatorKind {
    /// No active run — two blank lines (`IdleStatus`).
    Idle,
    /// A turn is streaming — accent spinner + `Working` (`interactive-mode.ts:1778`).
    Working,
    /// Auto-retry in progress — warning spinner (`AutoRetryStart`).
    Retry,
    /// Context compaction in progress — accent spinner (`CompactionStart`).
    Compaction,
    /// Branch summarization in progress — accent spinner.
    BranchSummary,
}

impl IndicatorKind {
    /// Whether this kind animates a spinner (everything but idle).
    pub fn is_active(self) -> bool {
        !matches!(self, IndicatorKind::Idle)
    }
}

/// A live retry backoff (`RetryStatusIndicator`, `status-indicator.ts:42-72`).
///
/// Upstream drives the `{n}s` with a `CountdownTimer` — `remainingSeconds = Math.ceil(timeoutMs /
/// 1000)`, an immediate `onTick`, then `setInterval(… 1000)` decrementing and re-`setMessage`ing
/// (`countdown-timer.ts:18-30`). ratatui is immediate-mode, so cyrup stores the same two inputs and
/// re-derives the remaining seconds from the band's own elapsed clock — the identical treatment the
/// spinner frame already gets. cyrup previously formatted the message ONCE at
/// `auto_retry_start`, so a 30-second backoff showed a frozen `in 30s...` while only the spinner
/// moved, which reads as a hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RetryCountdown {
    attempt: u32,
    max_attempts: u32,
    /// `Math.ceil(delayMs / 1000)` — the value the timer starts at (`countdown-timer.ts:18`).
    initial_seconds: u64,
}

impl RetryCountdown {
    /// The message at `elapsed` (`status-indicator.ts:46-47`), before the cancel suffix.
    fn message_at(self, elapsed: Duration) -> String {
        let remaining = self.initial_seconds.saturating_sub(elapsed.as_secs());
        format!(
            "Retrying ({}/{}) in {}s...",
            self.attempt, self.max_attempts, remaining
        )
    }
}

/// An extension's `setWorkingIndicator(options?)` payload — Pi `WorkingIndicatorOptions`
/// (`core/extensions/types.ts:116-121` @v0.83.0, unchanged at v0.84.2), resolved against
/// `Loader`'s own defaults (`pi/packages/tui/src/components/loader.ts:64-69` @v0.84.2 — every
/// `loader.ts` line below is that tag).
///
/// Resolution is upstream's, verbatim:
/// * `frames` absent ⇒ the default Braille set; `frames: []` ⇒ NO glyph at all (`updateDisplay`
///   reads `frames[currentFrame] ?? ""` at `:84` and emits the `"{frame} "` prefix only when the
///   frame is non-empty, `:86`), leaving a bare message;
/// * `intervalMs` absent, zero or negative ⇒ 80 ms (`indicator?.intervalMs && > 0`, `:67`;
///   `DEFAULT_INTERVAL_MS` is `:12`);
/// * a frame list of length ≤ 1 never animates (`restartAnimation`'s early return, `:74-76`).
///
/// Upstream's `renderIndicatorVerbatim = indicator !== undefined` (`:65`) needs no field here: the
/// MERE EXISTENCE of this struct is that condition. The moment an extension supplies ANY options
/// object — even `{}` — the frame stops going through the spinner colour function and is drawn
/// exactly as given (`:85`; upstream documents it as "Custom frames are rendered verbatim",
/// `core/extensions/types.ts:117`, and again at `:162`), so an extension can colour its own glyphs.
/// [`StatusIndicator::lines_at`] reads `Option::is_some` for exactly that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingIndicator {
    frames: Vec<String>,
    interval: Duration,
}

impl WorkingIndicator {
    /// Read the options bag off the guest's JSON (`{frames?: string[], intervalMs?: number}`).
    /// Unreadable fields fall back to upstream's defaults rather than erroring — the WIT verb is
    /// fire-and-forget with no error channel.
    #[must_use]
    pub fn from_json(opts: &serde_json::Value) -> Self {
        let frames = opts.get("frames").and_then(|f| f.as_array()).map_or_else(
            || SPINNER_FRAMES.iter().map(|f| (*f).to_string()).collect(),
            |a| {
                a.iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect()
            },
        );
        let interval = opts
            .get("intervalMs")
            .and_then(serde_json::Value::as_u64)
            .filter(|ms| *ms > 0)
            .map_or(SPINNER_INTERVAL, Duration::from_millis);
        WorkingIndicator { frames, interval }
    }

    /// The frame to draw at `elapsed`, or `""` when the extension asked for no glyph
    /// (`frames: []`). `frames.len() <= 1` is static — upstream arms no interval for it.
    fn frame_at(&self, elapsed: Duration) -> &str {
        if self.frames.len() <= 1 {
            return self.frames.first().map_or("", String::as_str);
        }
        let idx =
            (elapsed.as_millis() / self.interval.as_millis().max(1)) as usize % self.frames.len();
        self.frames.get(idx).map_or("", String::as_str)
    }
}

/// The status-band state (`StatusState`, spec/tui/01 §9). `started` anchors the spinner phase.
#[derive(Clone, Debug)]
pub struct StatusIndicator {
    kind: IndicatorKind,
    message: Option<String>,
    started: Option<Instant>,
    /// Set only while a retry backoff is counting down; re-derives `message` per frame.
    retry: Option<RetryCountdown>,
    /// Pi `this.workingMessage` (`interactive-mode.ts:431`) — an extension's `setWorkingMessage`
    /// override, `None` for `defaultWorkingMessage` (`"Working"`, `:451` @v0.87.1). Held ACROSS turns:
    /// upstream seeds every new `WorkingStatusIndicator` from it (`:2102`, `:3118`), so an extension
    /// sets it once and every subsequent turn shows it, until it clears it or a session swap resets
    /// it.
    working_message: Option<String>,
    /// Pi `this.workingVisible` (`:432`), initialised `true`. `false` suppresses the working band
    /// for the whole session — `turn_start` takes `clearStatusIndicator()` instead of showing the
    /// loader (`:3307-3313` @v0.87.1).
    working_visible: bool,
    /// Pi `this.workingIndicatorOptions` (`:433`) — `None` is the built-in animated spinner.
    working_indicator: Option<WorkingIndicator>,
}

impl Default for StatusIndicator {
    fn default() -> Self {
        StatusIndicator {
            kind: IndicatorKind::Idle,
            message: None,
            started: None,
            retry: None,
            working_message: None,
            // Pi `private workingVisible = true` (`interactive-mode.ts:432`).
            working_visible: true,
            working_indicator: None,
        }
    }
}

/// The per-frame derivation shared by the band and border forms — see [`StatusIndicator::derive`].
struct Derived {
    /// The spinner glyph for this frame, or `""` when an extension passed `frames: []`.
    spinner: String,
    /// Whether the glyph came from an extension, and so is drawn **unstyled**
    /// (`renderIndicatorVerbatim`, `loader.ts:88-91`).
    custom: bool,
    /// The message, with the `(<key> to cancel)` suffix already appended where it belongs.
    msg: String,
}

impl StatusIndicator {
    /// A fresh idle band.
    pub fn new() -> Self {
        StatusIndicator::default()
    }

    /// The active kind.
    pub fn kind(&self) -> IndicatorKind {
        self.kind
    }

    /// Whether the band is currently animating (non-idle).
    pub fn is_active(&self) -> bool {
        self.kind.is_active()
    }

    /// Transition to a new kind with an optional custom message, (re)starting the spinner phase when
    /// the kind becomes active. Idle clears the message + phase.
    pub fn set(&mut self, kind: IndicatorKind, message: Option<String>) {
        if kind == IndicatorKind::Idle {
            self.started = None;
        } else if self.kind != kind || self.started.is_none() {
            self.started = Some(Instant::now());
        }
        self.kind = kind;
        self.message = message;
        // A new indicator replaces the old one outright upstream (`showStatusIndicator` disposes the
        // previous, `interactive-mode.ts`), and `RetryStatusIndicator.dispose` kills its timer
        // (`status-indicator.ts:67-71`).
        self.retry = None;
    }

    /// Enter the retry state with a **live** countdown (`RetryStatusIndicator`,
    /// `status-indicator.ts:42-65`). The `{n}s` is re-derived every frame from the band's elapsed
    /// clock, so the message ticks 30→29→…→0 exactly as `CountdownTimer` does.
    ///
    /// `started` is re-anchored **unconditionally**. [`Self::set`] only restarts the clock when the
    /// *kind* changes, but a second `AutoRetryStart` arrives while the band is already `Retry` —
    /// attempt 2 of 3 — and would otherwise inherit attempt 1's anchor, so its countdown would open
    /// part-way down (or, past the first delay, pinned at `0s`). Upstream cannot have that bug: each
    /// `RetryStatusIndicator` is a fresh object with a fresh `CountdownTimer`, and the previous one
    /// is disposed (`status-indicator.ts:50-64`, `:67-71`).
    pub fn set_retry(&mut self, attempt: u32, max_attempts: u32, delay_ms: u64) {
        self.set(IndicatorKind::Retry, None);
        self.started = Some(Instant::now());
        self.retry = Some(RetryCountdown {
            attempt,
            max_attempts,
            initial_seconds: delay_ms.div_ceil(1_000),
        });
    }

    /// The retry message at the band's current phase, or `None` when no backoff is counting down —
    /// the same string [`Self::lines`] renders, for the transcript status line that mirrors it.
    pub fn retry_message(&self) -> Option<String> {
        let elapsed = self.started.map(|s| s.elapsed()).unwrap_or_default();
        self.retry.map(|r| r.message_at(elapsed))
    }

    /// Shortcut: enter the `Working` state (`TurnStart`).
    ///
    /// Pi's `turn_start` arm is a BRANCH on `workingVisible`, not an unconditional show:
    /// `if (this.workingVisible) { … showWorkingStatusIndicator() } else {
    /// this.clearStatusIndicator() }` (`interactive-mode.ts:3307-3313` @v0.87.1; `agent_start`
    /// until v0.84.4). Both halves are ported here because this is the ONE funnel every cyrup
    /// caller uses — `TurnStart`, and the four "back to working if still streaming" restores after
    /// a retry/compaction/branch-summary band.
    pub fn working(&mut self) {
        if !self.working_visible {
            // `clearStatusIndicator()` with NO kind argument — unfiltered, unlike the `"working"`
            // clear in [`Self::set_working_visible`].
            self.set(IndicatorKind::Idle, None);
            return;
        }
        let message = self.working_message.clone();
        self.set(IndicatorKind::Working, message);
    }

    /// Pi `setWorkingMessage(message?)` (`extensions/types.ts:151` @v0.83.0; the interactive body is
    /// `interactive-mode.ts:2377-2382`): record the override, and push it onto the LIVE band when
    /// one is already showing — `if (this.activeStatusIndicator?.kind === "working")
    /// this.activeStatusIndicator.setMessage(message ?? this.defaultWorkingMessage)`.
    ///
    /// `None` is upstream's no-argument call: [`Self::default_message`] then supplies
    /// `"Working"`, which is what `?? this.defaultWorkingMessage` does there.
    pub fn set_working_message(&mut self, message: Option<String>) {
        self.working_message = message.clone();
        if self.kind == IndicatorKind::Working {
            self.message = message;
        }
    }

    /// Pi `setWorkingVisible(visible)` (`extensions/types.ts:154`; body at
    /// `interactive-mode.ts:2091-2106`). `streaming` is upstream's `this.session.isStreaming`.
    ///
    /// Two asymmetries are load-bearing and both are ported:
    /// * hiding calls `clearStatusIndicator("working")` (`:2094`), which is KIND-FILTERED (`:2079`
    ///   returns early when the live band is not the working one) — so an extension hiding the
    ///   working row mid-retry does not blank the retry countdown;
    /// * showing only mounts a band when a turn is actually streaming AND the live band is not
    ///   already the working one (`:2098`), so it cannot resurrect a spinner over an idle session
    ///   or restart the phase of one already running.
    pub fn set_working_visible(&mut self, visible: bool, streaming: bool) {
        self.working_visible = visible;
        if !visible {
            if self.kind == IndicatorKind::Working {
                self.set(IndicatorKind::Idle, None);
            }
            return;
        }
        if streaming && self.kind != IndicatorKind::Working {
            let message = self.working_message.clone();
            self.set(IndicatorKind::Working, message);
        }
    }

    /// Pi `setWorkingIndicator(options?)` (`extensions/types.ts:164`; body at
    /// `interactive-mode.ts:2110-2116`): record the frames/interval, and re-apply them to the LIVE
    /// band when it is the working one (`activeStatusIndicator.setIndicator(options)`).
    ///
    /// `Loader.setIndicator` also resets `currentFrame = 0` and restarts the animation
    /// (`loader.ts:68-69`), which in an immediate-mode renderer is re-anchoring the phase clock —
    /// hence the `started` reset, gated on the working kind so a retry countdown's anchor (which
    /// also drives its `{n}s`) is never disturbed.
    pub fn set_working_indicator(&mut self, indicator: Option<WorkingIndicator>) {
        self.working_indicator = indicator;
        if self.kind == IndicatorKind::Working {
            self.started = Some(Instant::now());
        }
    }

    /// Pi `resetExtensionUI`'s working-indicator block (`interactive-mode.ts:2210-2218`), run on an
    /// extension reload / session swap: drop the message override, restore visibility to `true`,
    /// restore the default spinner — and, when the working band is LIVE, re-message it to
    /// `"Working ({interrupt} to interrupt)"` (`:2382` @v0.87.1).
    ///
    /// That suffix is upstream's, verbatim, and it is deliberately the odd one out: it says
    /// "to interrupt" where the retry / compaction / branch-summary constructors bake in
    /// "to cancel", and it is the ONLY path on which a working message carries a key hint at all
    /// (see this module's header). `interrupt` is the live `app.interrupt` label; `None` (no
    /// binding) drops the suffix rather than printing an empty parenthesis.
    pub fn reset_extension_working_state(&mut self, interrupt: Option<&str>) {
        self.working_message = None;
        self.working_visible = true;
        self.set_working_indicator(None);
        if self.kind == IndicatorKind::Working {
            self.message = Some(match interrupt {
                Some(key) => format!("{} ({key} to interrupt)", self.default_message()),
                None => self.default_message().to_string(),
            });
        }
    }

    /// The redraw period the run loop should tick at: the extension's `intervalMs` while a custom
    /// indicator is in force, else the built-in 80 ms (`loader.ts:12`). Upstream re-arms its
    /// `setInterval` with the new period inside `setIndicator` (`:69` → `:77-80`); cyrup's run loop
    /// re-creates its `tokio::time::interval` from this on the same event.
    pub fn spinner_period(&self) -> Duration {
        self.working_indicator
            .as_ref()
            .map_or(SPINNER_INTERVAL, |i| i.interval)
    }

    /// Shortcut: return to idle (`AgentEnd` / interrupt).
    pub fn idle(&mut self) {
        self.set(IndicatorKind::Idle, None);
    }

    /// The default message for the active kind when none was supplied.
    ///
    /// Verbatim from upstream: `"Working"` (`interactive-mode.ts:451 defaultWorkingMessage`
    /// @v0.87.1 — `"Working..."` until v0.85.0, #8799), and, ASCII `...` and all,
    /// `"Retrying …s..."` (`status-indicator.ts:47`), `"Compacting context..."` (`:81`),
    /// `"Summarizing branch..."` (`:100`). cyrup spelled every one with U+2026, which is one column
    /// where pi draws three.
    fn default_message(&self) -> &'static str {
        match self.kind {
            IndicatorKind::Idle => "",
            IndicatorKind::Working => "Working",
            IndicatorKind::Retry => "Retrying...",
            IndicatorKind::Compaction => "Compacting context...",
            IndicatorKind::BranchSummary => "Summarizing branch...",
        }
    }

    /// The current spinner glyph for `elapsed` (`(elapsed_ms / 80) % len`, spec/tui/01 §6.2).
    pub fn spinner_at(elapsed: Duration) -> &'static str {
        let idx = (elapsed.as_millis() / SPINNER_INTERVAL.as_millis().max(1)) as usize
            % SPINNER_FRAMES.len();
        SPINNER_FRAMES.get(idx).copied().unwrap_or("⠋")
    }

    /// Build the two band lines for a given `elapsed` (testable form). Idle ⇒ two blanks. Active ⇒ a
    /// leading blank then ` {spinner} {message} `, spinner colored by kind, message muted
    /// (`loader.ts:43-45`, `text.ts:70-76`). `cancel_hint` is the live-keymap label for
    /// `app.interrupt` (§6.1); it is appended for retry / compaction / branch-summary, whose upstream
    /// constructors bake `(${keyText("app.interrupt")} to cancel)` into the message
    /// (`status-indicator.ts:47,78,100`), and **never** for `Working`, whose constructor appends
    /// nothing (`:29-40`, `interactive-mode.ts:2074-2080`).
    pub fn lines_at(
        &self,
        elapsed: Duration,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
    ) -> Vec<Line<'static>> {
        let Some(Derived {
            spinner,
            custom,
            msg,
        }) = self.derive(elapsed, cancel_hint)
        else {
            return vec![Line::default(), Line::default()];
        };
        let spinner_style = self.spinner_style(theme, None);
        // `Loader extends Text` with `paddingX = 1` (`loader.ts:35`), so `Text.render` emits
        // `leftMargin + line + rightMargin` (`text.ts:70,76`). The band was starting at column 0,
        // one column out of alignment with every other component.
        //
        // The glyph itself is `updateDisplay`'s `const indicator = frame.length > 0 ? "${frame} " :
        // ""` (`loader.ts:86` @v0.84.2): an extension that passed `frames: []` gets NO glyph and NO
        // trailing space, i.e. ` Working ` flush against the one-column margin — not a blank
        // column where a spinner used to be. A custom indicator is drawn UNSTYLED
        // (`renderIndicatorVerbatim`, `:85`) so the extension's own colouring survives; the built-in
        // keeps its accent/warning.
        let spans = vec![
            if spinner.is_empty() {
                Span::raw(" ")
            } else if custom {
                Span::raw(format!(" {spinner} "))
            } else {
                Span::styled(format!(" {spinner} "), spinner_style)
            },
            Span::styled(msg, theme.muted_style()),
            Span::styled(" ", theme.muted_style()),
        ];
        vec![Line::default(), Line::from(spans)]
    }

    /// The spinner colour. `override_style` is the EMBEDDED working case: pi's
    /// `showWorkingStatusIndicator` passes ONE `colorFn` — `this.editor.borderColor ??
    /// theme.getThinkingBorderColor(...)` — to `new WorkingStatusIndicator`, which hands the same
    /// fn to both `spinnerColorFn` and `messageColorFn` (`interactive-mode.ts:2346-2359`,
    /// `status-indicator.ts:38-47`), so an embedded `working` status takes the editor's rule colour
    /// for glyph and message alike. Retry / compaction / branch-summary build their own colours in
    /// their own constructors (`status-indicator.ts:52-77`, `:84-99`, `:102-111`) and keep them
    /// when embedded.
    fn spinner_style(&self, theme: &UiTheme, override_style: Option<Style>) -> Style {
        if let Some(st) = override_style.filter(|_| self.kind == IndicatorKind::Working) {
            return st;
        }
        match self.kind {
            IndicatorKind::Retry => theme.warning_style(),
            _ => theme.accent_style(),
        }
    }

    /// The message colour, under the same rule as [`Self::spinner_style`].
    fn message_style(&self, theme: &UiTheme, override_style: Option<Style>) -> Style {
        override_style
            .filter(|_| self.kind == IndicatorKind::Working)
            .unwrap_or_else(|| theme.muted_style())
    }

    /// The **un-margined** content of the active row — pi's `Loader.updateDisplay` text, which is
    /// `${frame.length > 0 ? `${frame} ` : ""}${messageColorFn(message)}` (`loader.ts:93-96`)
    /// *before* `Text.render` adds its `paddingX = 1` margins. [`Self::lines_at`] adds the margins;
    /// [`Self::border_spans`] does not, because `renderInBorder` strips them again
    /// (`status-indicator.ts:24-27`).
    fn content_spans(
        &self,
        elapsed: Duration,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
        override_style: Option<Style>,
    ) -> Vec<Span<'static>> {
        let Some(Derived {
            spinner,
            custom,
            msg,
        }) = self.derive(elapsed, cancel_hint)
        else {
            return Vec::new();
        };
        let mut spans = Vec::with_capacity(2);
        if !spinner.is_empty() {
            spans.push(if custom {
                Span::raw(format!("{spinner} "))
            } else {
                Span::styled(
                    format!("{spinner} "),
                    self.spinner_style(theme, override_style),
                )
            });
        }
        spans.push(Span::styled(msg, self.message_style(theme, override_style)));
        spans
    }

    /// `StatusIndicator.renderInBorder(width)` — `status-indicator.ts:24-27`:
    ///
    /// ```ts
    /// const line = super.render(width + 2)[1] ?? "";
    /// return truncateToWidth(line.startsWith(" ") ? line.slice(1).trimEnd() : line.trimEnd(), width, "");
    /// ```
    ///
    /// `super` is `Loader`, whose `render` is `["", ...Text.render(width)]` (`loader.ts:43-45`), so
    /// index 1 is `Text.render(width + 2)[0]`: the **first wrapped row** of the content, wrapped at
    /// `contentWidth = (width + 2) - 2 * paddingX = width` (`text.ts:67-73`, `paddingX = 1`), with
    /// the two margins and the right-pad around it. Stripping the one leading space and trimming the
    /// end removes exactly those, leaving the first wrapped row — which is then hard-cut to `width`
    /// with an **empty** ellipsis, so no `…` ever appears in the rule.
    ///
    /// Wrapping first and truncating second is not interchangeable with truncating alone: a message
    /// too long for the border is cut at pi's word boundary, not mid-word.
    ///
    /// Returns an empty vector when the indicator is idle, which is pi's `statusWidth === 0` arm
    /// (`custom-editor.ts:43`).
    pub fn border_spans(
        &self,
        elapsed: Duration,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
        width: usize,
        override_style: Option<Style>,
    ) -> Vec<Span<'static>> {
        let content = self.content_spans(elapsed, theme, cancel_hint, override_style);
        if content.is_empty() {
            return Vec::new();
        }
        let wrapped = crate::transcript::layout::wrap_line(&Line::from(content), width);
        let first = wrapped.into_iter().next().unwrap_or_default();
        crate::text_width::truncate_spans_to_width(first.spans, width, "")
    }

    /// `StatusIndicator.renderSpinnerInBorder(width)` — `status-indicator.ts:29-31`:
    /// `truncateToWidth(this.getRenderedIndicator(), width, "")`. `getRenderedIndicator` is the
    /// coloured frame **alone** (`loader.ts:88-91`) — no trailing space, unlike the `${frame} ` the
    /// full content carries.
    pub fn border_spinner_spans(
        &self,
        elapsed: Duration,
        theme: &UiTheme,
        width: usize,
        override_style: Option<Style>,
    ) -> Vec<Span<'static>> {
        let Some(Derived {
            spinner, custom, ..
        }) = self.derive(elapsed, None)
        else {
            return Vec::new();
        };
        if spinner.is_empty() {
            return Vec::new();
        }
        let span = if custom {
            Span::raw(spinner)
        } else {
            Span::styled(spinner, self.spinner_style(theme, override_style))
        };
        crate::text_width::truncate_spans_to_width(vec![span], width, "")
    }

    /// The spinner glyph, the extension-verbatim flag and the message for `elapsed` — the one
    /// derivation the band form ([`Self::lines_at`]) and the border forms ([`Self::border_spans`])
    /// share, so they cannot drift.
    fn derive(&self, elapsed: Duration, cancel_hint: Option<&str>) -> Option<Derived> {
        if !self.kind.is_active() {
            return None;
        }
        // An extension's `setWorkingIndicator` frames apply to the WORKING band only: upstream
        // passes `workingIndicatorOptions` to `new WorkingStatusIndicator` alone
        // (`interactive-mode.ts:3116-3120`) and re-applies it only when the live band's kind is
        // `"working"` (`:2112`). Retry / compaction / branch-summary keep the built-in spinner.
        let custom = self
            .working_indicator
            .as_ref()
            .filter(|_| self.kind == IndicatorKind::Working);
        let spinner = custom.map_or_else(|| Self::spinner_at(elapsed), |i| i.frame_at(elapsed));
        let mut msg = match (self.retry, &self.message) {
            (Some(retry), _) => retry.message_at(elapsed),
            (None, Some(m)) => m.clone(),
            (None, None) => self.default_message().to_string(),
        };
        if let Some(hint) = cancel_hint
            && self.kind != IndicatorKind::Working
        {
            msg.push_str(&format!(" ({hint} to cancel)"));
        }
        Some(Derived {
            spinner: spinner.to_string(),
            custom: custom.is_some(),
            msg,
        })
    }

    /// How long the current indicator has been running — the spinner's phase. Zero when nothing
    /// started it, which keeps the render-time forms pure functions of this one value.
    pub fn elapsed(&self) -> Duration {
        self.started.map(|s| s.elapsed()).unwrap_or_default()
    }

    /// The two band lines at the current wall-clock phase (the render-time form).
    pub fn lines(&self, theme: &UiTheme, cancel_hint: Option<&str>) -> Vec<Line<'static>> {
        self.lines_at(self.elapsed(), theme, cancel_hint)
    }

    /// Render the band into `area` (the live region's 2-row status slot).
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
    ) {
        let lines = self.lines(theme, cancel_hint);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }
}
