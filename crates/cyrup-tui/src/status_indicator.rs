//! The working / idle status band (spec/tui/01 §6; `components/status-indicator.ts`).
//!
//! Pi renders a fixed **2-row** band between the active turn and the editor (container index 5,
//! editor index 7) so the editor never jumps when a spinner appears. While a turn runs it shows a
//! `Loader`: a leading blank line then `{spinner} {message}` (`loader.ts:43-45`); idle is two blank
//! lines (`IdleStatus`, `status-indicator.ts:105-114`).
//!
//! ## States (`status-indicator.ts:7`)
//! `working | retry | compaction | branchSummary`, plus idle — exactly one active at a time. Each
//! non-idle state is an accent/warning spinner + a muted message; the trailing `(<key> to cancel)`
//! text is built from the **live keymap** (`status-indicator.ts:47,78,100`), never hardcoded.
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

/// The status-band state (`StatusState`, spec/tui/01 §9). `started` anchors the spinner phase.
#[derive(Clone, Debug)]
pub struct StatusIndicator {
    kind: IndicatorKind,
    message: Option<String>,
    started: Option<Instant>,
}

impl Default for StatusIndicator {
    fn default() -> Self {
        StatusIndicator { kind: IndicatorKind::Idle, message: None, started: None }
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
    }

    /// Shortcut: enter the `Working…` state (`AgentStart`).
    pub fn working(&mut self) {
        self.set(IndicatorKind::Working, None);
    }

    /// Shortcut: return to idle (`AgentEnd` / interrupt).
    pub fn idle(&mut self) {
        self.set(IndicatorKind::Idle, None);
    }

    /// The default message for the active kind when none was supplied (`status-indicator.ts:38-100`).
    fn default_message(&self) -> &'static str {
        match self.kind {
            IndicatorKind::Idle => "",
            IndicatorKind::Working => "Working…",
            IndicatorKind::Retry => "Retrying…",
            IndicatorKind::Compaction => "Compacting context…",
            IndicatorKind::BranchSummary => "Summarizing branch…",
        }
    }

    /// The current spinner glyph for `elapsed` (`(elapsed_ms / 80) % len`, spec/tui/01 §6.2).
    pub fn spinner_at(elapsed: Duration) -> &'static str {
        let idx = (elapsed.as_millis() / SPINNER_INTERVAL.as_millis().max(1)) as usize
            % SPINNER_FRAMES.len();
        SPINNER_FRAMES.get(idx).copied().unwrap_or("⠋")
    }

    /// Build the two band lines for a given `elapsed` (testable form). Idle ⇒ two blanks. Active ⇒ a
    /// leading blank then `{spinner} {message} (<cancel> to cancel)`, spinner colored by kind, message
    /// muted (`loader.ts:43-45`). `cancel_hint` is the live-keymap label for `app.interrupt` (§6.1).
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
        let mut msg = self
            .message
            .clone()
            .unwrap_or_else(|| self.default_message().to_string());
        if let Some(hint) = cancel_hint {
            msg.push_str(&format!(" ({hint} to cancel)"));
        }
        let line = Line::from(vec![
            Span::styled(format!("{spinner} "), spinner_style),
            Span::styled(msg, theme.muted_style()),
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
