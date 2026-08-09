//! The working / idle status band (spec/tui/01 §6; `components/status-indicator.ts`).
//!
//! Pi renders a fixed **2-row** band between the active turn and the editor (container index 5,
//! editor index 7) so the editor never jumps when a spinner appears. While a turn runs it shows a
//! `Loader`: a leading blank line then `{spinner} {message}` (`loader.ts:43-45`); idle is two blank
//! lines (`IdleStatus`, `status-indicator.ts:105-114`).
//!
//! `Loader extends Text` and is constructed `super("", 1, 0)` (`loader.ts:35`) — **paddingX 1** — so
//! `Text.render` emits `leftMargin + line + rightMargin` (`text.ts:70-76`). The rendered row is
//! ` ⠋ Working... `, inset one column, not flush against the terminal edge.
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
//! ` ({hint} to cancel)` to every kind, adding 18 columns to `Working...` for the whole duration of
//! every turn.
//!
//! ## Spinner (spec/tui/01 §6.2)
//! Braille frames `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` advancing every **80 ms** (`loader.ts:11-12`). ratatui is
//! immediate-mode, so the frame is selected as `(elapsed_ms / 80) % frames.len()` — no timer thread;
//! the app loop schedules an 80 ms redraw tick only while an indicator is active (§6.2 / §10).

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

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
    /// A turn is streaming — accent spinner + `Working…` (`interactive-mode.ts:1778`).
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
        format!("Retrying ({}/{}) in {}s...", self.attempt, self.max_attempts, remaining)
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
}

impl Default for StatusIndicator {
    fn default() -> Self {
        StatusIndicator { kind: IndicatorKind::Idle, message: None, started: None, retry: None }
    }
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

    /// Shortcut: enter the `Working…` state (`AgentStart`).
    pub fn working(&mut self) {
        self.set(IndicatorKind::Working, None);
    }

    /// Shortcut: return to idle (`AgentEnd` / interrupt).
    pub fn idle(&mut self) {
        self.set(IndicatorKind::Idle, None);
    }

    /// The default message for the active kind when none was supplied.
    ///
    /// Verbatim from upstream, ASCII `...` and all: `"Working..."`
    /// (`interactive-mode.ts:420 defaultWorkingMessage`), `"Retrying …s..."`
    /// (`status-indicator.ts:47`), `"Compacting context..."` (`:81`), `"Summarizing branch..."`
    /// (`:100`). cyrup spelled every one with U+2026, which is one column where pi draws three.
    fn default_message(&self) -> &'static str {
        match self.kind {
            IndicatorKind::Idle => "",
            IndicatorKind::Working => "Working...",
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
        if !self.kind.is_active() {
            return vec![Line::default(), Line::default()];
        }
        let spinner = Self::spinner_at(elapsed);
        let spinner_style = match self.kind {
            IndicatorKind::Retry => theme.warning_style(),
            _ => theme.accent_style(),
        };
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
        let line = Line::from(vec![
            // `Loader extends Text` with `paddingX = 1` (`loader.ts:35`), so `Text.render` emits
            // `leftMargin + line + rightMargin` (`text.ts:70,76`). The band was starting at column 0,
            // one column out of alignment with every other component.
            Span::styled(format!(" {spinner} "), spinner_style),
            Span::styled(msg, theme.muted_style()),
            Span::styled(" ", theme.muted_style()),
        ]);
        vec![Line::default(), line]
    }

    /// The two band lines at the current wall-clock phase (the render-time form).
    pub fn lines(&self, theme: &UiTheme, cancel_hint: Option<&str>) -> Vec<Line<'static>> {
        let elapsed = self.started.map(|s| s.elapsed()).unwrap_or_default();
        self.lines_at(elapsed, theme, cancel_hint)
    }

    /// Render the band into `area` (the live region's 2-row status slot).
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &UiTheme, cancel_hint: Option<&str>) {
        let lines = self.lines(theme, cancel_hint);
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }
}
