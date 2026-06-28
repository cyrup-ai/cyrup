//! The status / footer line (R-10-028; arch-10 §2 `chrome/footer.rs`).
//!
//! A single line showing the active model, the run state (streaming vs. idle), the running token
//! count, and any queued-message count. Themed via [`UiTheme`].

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::component::Component;
use crate::theme::UiTheme;

/// The footer status line state.
#[derive(Clone, Debug, Default)]
pub struct StatusLine {
    /// Active model label (`provider/model`).
    pub model: String,
    /// Whether a run is currently streaming.
    pub streaming: bool,
    /// Running total token count for the latest assistant turn.
    pub tokens: u64,
    /// Number of queued (steering + follow-up) messages (R-10-030).
    pub queued: usize,
}

impl StatusLine {
    /// A fresh status line with the given model label.
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

    /// The rendered status text (also useful for assertions).
    pub fn text(&self) -> String {
        let state = if self.streaming { "streaming" } else { "idle" };
        let model = if self.model.is_empty() { "(no model)" } else { self.model.as_str() };
        let mut s = format!(" {model} · {state} · {} tokens", self.tokens);
        if self.queued > 0 {
            s.push_str(&format!(" · {} queued", self.queued));
        }
        s
    }
}

impl Component for StatusLine {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let state_style = if self.streaming { theme.accent_style() } else { theme.dim_style() };
        let line = Line::from(vec![Span::styled(self.text(), state_style)]);
        frame.render_widget(Paragraph::new(line).style(theme.base_style()), area);
    }
}
