//! The **pending-messages region** — Pi's `pendingMessagesContainer` and the
//! `updatePendingMessagesDisplay` that fills it
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3974-3991` @v0.83.0).
//!
//! While a turn streams, a submitted message is not sent — it is queued, as steering (delivered at
//! the next loop iteration) or as a follow-up (delivered once the agent goes idle). Pi renders that
//! queue in a dedicated container docked directly above the status band and the editor — the dock
//! order is literal at `:709-719`: `chatContainer`, `pendingMessagesContainer`, `statusContainer`,
//! `widgetContainerAbove`, `editorContainer`, `widgetContainerBelow`, `footer`:
//!
//! ```ts
//! this.pendingMessagesContainer.clear();
//! const { steering, followUp } = this.getAllQueuedMessages();          // :3976
//! if (steering.length > 0 || followUp.length > 0) {
//!     this.pendingMessagesContainer.addChild(new Spacer(1));           // :3978
//!     for (const message of steering)  … `Steering: ${message}`        // :3979-3982
//!     for (const message of followUp)  … `Follow-up: ${message}`       // :3983-3986
//!     const dequeueHint = this.getAppKeyDisplay("app.message.dequeue");// :3987
//!     … `↳ ${dequeueHint} to edit all queued messages`                 // :3988-3989
//! }
//! ```
//!
//! Every row is a `TruncatedText(text, 1, 0)` — `paddingX` 1, `paddingY` 0
//! (`tui/src/components/truncated-text.ts:12-64`), i.e. **one** row, cut at the first newline and
//! truncated to `width - 2`, with a single leading space. All of it is `theme.fg("dim", …)`.
//!
//! `getAllQueuedMessages` (`:3942-3953`) folds the session's two queues together with the
//! *compaction* queue, so a message typed during a compaction shows up in the same region; that
//! second source is `TUI-031` and is not wired here yet — the shape below takes both lists and does
//! not care where they came from.
//!
//! # Why this exists, and what it replaced
//!
//! `TUI-016`, measured live on 2026-08-13 (`docs/gap-analysis/REPRO-LOG.md`): cyrup had **no** queue
//! surface at all — the `{n} queued` footer segment was deleted by a fidelity pass and nothing took
//! its place — and, worse, `dispatch_submission` echoed every submission into the **chat
//! transcript** as an ordinary user bubble before the session had decided whether to queue it. The
//! user was shown the opposite of the truth: text that looks delivered while it sits in a queue.
//!
//! So this region lands together with the removal of that echo (`crate::app::App::dispatch_submission`
//! no longer calls `push_user`; the bubble is written when the session emits `message_start` for the
//! user message, which is exactly where Pi writes it — `interactive-mode.ts:2915-2918`,
//! `addMessageToChat(event.message)` followed by `updatePendingMessagesDisplay()`). Adding the rows
//! without removing the echo would render every queued message twice.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::text_width::truncate_to_width;
use crate::theme::UiTheme;

/// The queued-message snapshot the region draws, updated from `queue_update`.
#[derive(Debug, Default, Clone)]
pub struct PendingMessages {
    steering: Vec<String>,
    follow_up: Vec<String>,
}

impl PendingMessages {
    /// Replace the snapshot — Pi's `updatePendingMessagesDisplay` rebuilds the container from
    /// scratch on every `queue_update` (`interactive-mode.ts:3975`), so this is a replace, never a
    /// merge.
    pub fn set(&mut self, steering: Vec<String>, follow_up: Vec<String>) {
        self.steering = steering;
        self.follow_up = follow_up;
    }

    /// Total queued count — the number the deleted footer segment used to show, kept because
    /// `AppState` still feeds `crate::status::StatusBar::set_queued`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steering.len().saturating_add(self.follow_up.len())
    }

    /// `steering.length > 0 || followUp.length > 0` (`interactive-mode.ts:3977`), negated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }

    /// Rows this region occupies: nothing when the queue is empty, otherwise the `Spacer(1)`, one
    /// row per message, and the dequeue hint.
    #[must_use]
    pub fn height(&self) -> u16 {
        if self.is_empty() {
            return 0;
        }
        u16::try_from(self.len().saturating_add(2)).unwrap_or(u16::MAX)
    }

    /// The rendered rows. `dequeue_key` is the `app.message.dequeue` label from the live keymap —
    /// Pi's `getAppKeyDisplay` (`:5743-5745` → `keyDisplayText`), i.e. **all** bound keys joined
    /// with `/` and title-cased, so a rebind is reflected.
    #[must_use]
    pub fn lines(
        &self,
        width: u16,
        theme: &UiTheme,
        dequeue_key: Option<&str>,
    ) -> Vec<Line<'static>> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Line<'static>> = Vec::with_capacity(self.len().saturating_add(2));
        // `new Spacer(1)` (`interactive-mode.ts:3978`).
        out.push(Line::from(""));
        for message in &self.steering {
            out.push(row(&format!("Steering: {message}"), width, theme));
        }
        for message in &self.follow_up {
            out.push(row(&format!("Follow-up: {message}"), width, theme));
        }
        let key = dequeue_key.unwrap_or_default();
        out.push(row(&format!("↳ {key} to edit all queued messages"), width, theme));
        out
    }

    /// Draw the region into `area`.
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &UiTheme,
        dequeue_key: Option<&str>,
    ) {
        if area.height == 0 || self.is_empty() {
            return;
        }
        let mut lines = self.lines(area.width, theme, dequeue_key);
        lines.truncate(usize::from(area.height));
        frame.render_widget(Paragraph::new(lines).style(theme.base_style()), area);
    }
}

/// One `TruncatedText(text, 1, 0)` row (`tui/src/components/truncated-text.ts:22-64`): the text up
/// to its first newline, truncated to `width - 2 * paddingX`, behind one space of left padding, all
/// in the `dim` colour `updatePendingMessagesDisplay` wraps every child in.
fn row(text: &str, width: u16, theme: &UiTheme) -> Line<'static> {
    let single = text.split('\n').next().unwrap_or(text);
    let available = usize::from(width).saturating_sub(2).max(1);
    // `truncateToWidth(singleLineText, availableWidth)` (`truncated-text.ts:44`) — the shared port,
    // with upstream's default `"..."` ellipsis (`utils.ts:939`).
    let clipped = truncate_to_width(single, available, "...");
    Line::from(vec![Span::raw(" "), Span::styled(clipped, theme.dim_style())])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn rendered(p: &PendingMessages, width: u16) -> Vec<String> {
        let theme = UiTheme::default();
        p.lines(width, &theme, Some("Alt+Up")).iter().map(text_of).collect()
    }

    #[test]
    fn an_empty_queue_draws_nothing() {
        let p = PendingMessages::default();
        assert_eq!(p.height(), 0);
        assert!(rendered(&p, 80).is_empty());
    }

    #[test]
    fn steering_comes_first_then_follow_up_then_the_hint() {
        let mut p = PendingMessages::default();
        p.set(vec!["one".into(), "two".into()], vec!["three".into()]);
        assert_eq!(
            rendered(&p, 80),
            vec![
                "".to_string(),
                " Steering: one".to_string(),
                " Steering: two".to_string(),
                " Follow-up: three".to_string(),
                " ↳ Alt+Up to edit all queued messages".to_string(),
            ]
        );
        assert_eq!(p.height(), 5, "spacer + three rows + hint");
    }

    #[test]
    fn a_row_is_one_line_cut_at_the_first_newline_and_truncated_to_width_minus_two() {
        let mut p = PendingMessages::default();
        p.set(vec!["first line\nsecond line".into()], Vec::new());
        assert_eq!(rendered(&p, 80)[1], " Steering: first line");

        p.set(vec!["x".repeat(200)], Vec::new());
        let row = rendered(&p, 30).remove(1);
        assert_eq!(row.chars().count(), 29, "one space of padding + 28 columns of text");
        assert!(row.ends_with("..."), "truncateToWidth's default ellipsis: {row:?}");
    }

    #[test]
    fn the_hint_carries_the_live_keymap_label() {
        let mut p = PendingMessages::default();
        p.set(vec!["q".into()], Vec::new());
        let theme = UiTheme::default();
        let lines = p.lines(80, &theme, Some("Ctrl+K"));
        assert_eq!(text_of(lines.last().unwrap()), " ↳ Ctrl+K to edit all queued messages");
    }

    #[test]
    fn every_row_is_dim() {
        let mut p = PendingMessages::default();
        p.set(vec!["a".into()], vec!["b".into()]);
        let theme = UiTheme::default();
        for line in p.lines(80, &theme, Some("Alt+Up")).iter().skip(1) {
            assert_eq!(
                line.spans[1].style,
                theme.dim_style(),
                "`theme.fg(\"dim\", …)` wraps every child (interactive-mode.ts:3980,3984,3988)"
            );
        }
    }
}
