//! [`InlineMessage`] — a port of `pi-intercom/ui/inline-message.ts` `InlineMessageComponent`.
//!
//! Two roles, both WIRED on every real inbound broker message:
//! - [`InlineMessage::content_markdown`] builds the attributed message pi's `sendIncomingMessage`
//!   passes to `pi.sendMessage({customType:"intercom_message", content, …})` (`index.ts:654-666`).
//!   Upstream this ONE string is both what the human sees and what the model receives; cyrup splits
//!   the two surfaces (the port doc §4.2/§7.2 divergence) but sends the SAME string down both:
//!   [`crate::inbound::surface_incoming_message`] puts it in the `content` field of
//!   `append_entry("intercom_message", …)` (the card degrades to a custom entry, §4.3), and
//!   [`crate::inbound::trigger_turn_over_inbound`] / [`crate::inbound::send_incoming_message`] pass
//!   it to `HostServices::inject_message` — so the model always gets the `📨 From …` attribution,
//!   the sender's cwd and the reply instruction, never the bare body.
//!
//!   NOTE (EXT-006): the reason for that degradation is gone. `InitApi::register_message_renderer`
//!   now exists for native built-ins and `ExtensionHost::render_message_call` routes a custom type
//!   back to its owner, so this crate COULD register `"intercom_message"` and have
//!   [`InlineMessage::render`] draw it the way `index.ts:1142-1146` does. Adopting it is deliberate
//!   follow-up work in this crate, not a missing capability in the extension seam.
//! - [`InlineMessage::render`] draws the bordered card (`registerMessageRenderer("intercom_message",
//!   …)`, `index.ts:1142-1146`); cyrup embeds the rendered lines in the same `append_entry` payload
//!   (`card`) so the human still sees the framed message even without a live renderer.

use crate::transport::protocol::{Message, SessionInfo};
use crate::ui::{Theme, truncate_to_width, visible_width, wrap_text};

/// The `intercom_message` inline card (pi `InlineMessageComponent`).
#[derive(Clone, Debug)]
pub struct InlineMessage {
    /// The sender's session info.
    pub from: SessionInfo,
    /// The received message.
    pub message: Message,
    /// The reply-hint command shown in the card, when the sender expects a reply and the hint is on.
    pub reply_command: Option<String>,
    /// The pre-formatted body (text + attachment text). `None` falls back to `message.content.text`
    /// (pi `this.bodyText || this.message.content.text`).
    pub body_text: Option<String>,
    /// Whether the card is collapsed (preview + expand hint) vs. fully expanded.
    pub collapsed: bool,
}

impl InlineMessage {
    /// Build a card. `body_text`/`reply_command` default to the un-decorated message + no hint
    /// (matching pi's optional constructor args).
    #[must_use]
    pub fn new(from: SessionInfo, message: Message) -> Self {
        Self { from, message, reply_command: None, body_text: None, collapsed: false }
    }

    /// The pre-formatted body, or `message.content.text` when unset (pi `bodyText ||
    /// message.content.text`).
    #[must_use]
    pub fn body(&self) -> &str {
        self.body_text.as_deref().unwrap_or(&self.message.content.text)
    }

    /// The sender display label (pi `from.name || from.id.slice(0, 8)`).
    #[must_use]
    pub fn sender_display(&self) -> String {
        self.from
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.from.id.chars().take(8).collect())
    }

    /// The human-readable body pi's `sendIncomingMessage` injects into the session
    /// (`index.ts:654-666`): `**📨 From {sender}** ({cwd}){replyInstruction}\n\n{bodyText}`.
    #[must_use]
    pub fn content_markdown(&self) -> String {
        let reply_instruction = self
            .reply_command
            .as_deref()
            .map(|rc| format!("\n\nTo reply, use the intercom tool: {rc}"))
            .unwrap_or_default();
        format!(
            "**📨 From {}** ({}){}\n\n{}",
            self.sender_display(),
            self.from.cwd,
            reply_instruction,
            self.body()
        )
    }

    /// Render the bordered card to `width` display columns (pi `InlineMessageComponent.render`).
    /// Every returned line is exactly `width` columns wide (for `width >= 3`).
    #[must_use]
    pub fn render(&self, theme: &dyn Theme, width: usize) -> Vec<String> {
        let sender_name = self.sender_display();
        if width < 3 {
            return vec![truncate_to_width(&format!("From {sender_name}"), width)];
        }
        let body_width = std::cmp::max(1, width - 2);
        let mut lines: Vec<String> = Vec::new();

        // Header: ╭ 📨 From: sender (cwd) ───╮
        let header = format!(" 📨 From: {sender_name} ({}) ", self.from.cwd);
        let header_src = if self.collapsed { format!("{header} Ctrl+O expands ") } else { header };
        let header_text = truncate_to_width(&header_src, body_width);
        let header_pad = body_width.saturating_sub(visible_width(&header_text));
        lines.push(theme.fg("accent", &format!("╭{header_text}{}╮", "─".repeat(header_pad))));

        if self.collapsed {
            self.render_collapsed(theme, body_width, &mut lines);
            return lines;
        }

        // Body content lines.
        for line in wrap_text(self.body(), body_width) {
            lines.push(card_row(theme, body_width, &line));
        }

        // Reply hint block.
        if let Some(rc) = &self.reply_command {
            lines.push(card_row(theme, body_width, ""));
            let hint = theme.fg("dim", &format!(" ↩ To reply: {rc}"));
            for line in wrap_text(&hint, body_width) {
                lines.push(card_row(theme, body_width, &line));
            }
        }

        // Attachments.
        if let Some(atts) = &self.message.content.attachments
            && !atts.is_empty()
        {
            lines.push(card_row(theme, body_width, ""));
            for att in atts {
                let label = theme.fg("dim", &format!(" 📎 {}", att.name));
                lines.push(card_row(theme, body_width, &label));
            }
        }

        // Reply-to breadcrumb (only when this is NOT itself an ask).
        if self.message.reply_to.is_some() && self.message.expects_reply != Some(true) {
            lines.push(card_row(theme, body_width, ""));
            let short: String =
                self.message.reply_to.as_deref().unwrap_or_default().chars().take(8).collect();
            let reply = theme.fg("dim", &format!(" ↳ Reply to {short}"));
            lines.push(card_row(theme, body_width, &reply));
        }

        lines.push(theme.fg("accent", &format!("╰{}╯", "─".repeat(body_width))));
        lines
    }

    /// The collapsed card body: preview line + meta line + bottom border (pi lines 46-65). Always
    /// yields exactly 3 lines here (header pushed by the caller → 4 total).
    fn render_collapsed(&self, theme: &dyn Theme, body_width: usize, lines: &mut Vec<String>) {
        // Preview: the body with runs of whitespace collapsed to single spaces, trimmed.
        let preview: String = self.body().split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(card_row(theme, body_width, &preview));

        let mut meta: Vec<String> = Vec::new();
        if let Some(rc) = &self.reply_command {
            meta.push(format!("↩ To reply: {rc}"));
        }
        if let Some(atts) = &self.message.content.attachments {
            let count = atts.len();
            if count > 0 {
                let plural = if count == 1 { "" } else { "s" };
                meta.push(format!("📎 {count} attachment{plural}"));
            }
        }
        if self.message.reply_to.is_some() && self.message.expects_reply != Some(true) {
            let short: String =
                self.message.reply_to.as_deref().unwrap_or_default().chars().take(8).collect();
            meta.push(format!("↳ Reply to {short}"));
        }
        meta.push("Ctrl+O to expand".to_string());

        let meta_line = theme.fg("dim", &format!(" {}", meta.join(" · ")));
        lines.push(card_row(theme, body_width, &meta_line));
        lines.push(theme.fg("accent", &format!("╰{}╯", "─".repeat(body_width))));
    }
}

/// A `│ … │` card row: truncate `inner` to `body_width`, right-pad with spaces, frame with borders
/// (the whole row accent-colored, matching pi's `theme.fg("accent", …)` wrapping).
fn card_row(theme: &dyn Theme, body_width: usize, inner: &str) -> String {
    let text = truncate_to_width(inner, body_width);
    let pad = body_width.saturating_sub(visible_width(&text));
    theme.fg("accent", &format!("│{text}{}│", " ".repeat(pad)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::transport::protocol::{Attachment, AttachmentKind, MessageContent};
    use crate::ui::PlainTheme;

    fn from() -> SessionInfo {
        SessionInfo {
            id: "session-12345678".to_string(),
            name: Some("sender".to_string()),
            cwd: "/tmp/project".to_string(),
            model: "model".to_string(),
            pid: 1,
            started_at: 0,
            last_activity: 0,
            status: None,
            peer_uid: None,
            trusted_local: None,
        }
    }

    fn message(text: &str) -> Message {
        Message {
            id: "message-1".to_string(),
            timestamp: 0,
            reply_to: None,
            expects_reply: None,
            content: MessageContent { text: text.to_string(), attachments: None },
        }
    }

    // Port of test/inline-message.test.ts:32-39.
    #[test]
    fn renders_at_the_available_terminal_width() {
        let card = InlineMessage::new(
            from(),
            message(
                "This is a long message that should use the available terminal width instead of a narrow fixed card.",
            ),
        );
        let lines = card.render(&PlainTheme, 120);
        assert!(!lines.is_empty());
        for line in &lines {
            assert_eq!(visible_width(line), 120, "line not full width: {line:?}");
        }
    }

    // Port of test/inline-message.test.ts:41-51.
    #[test]
    fn expanded_shows_full_body_without_collapse_controls() {
        let mut card = InlineMessage::new(
            from(),
            message(
                "This is a long message that should use the available terminal width instead of a narrow fixed card.",
            ),
        );
        card.reply_command = Some("intercom({ action: \"reply\", message: \"...\" })".to_string());
        let rendered = card.render(&PlainTheme, 100).join("\n");
        assert!(rendered.contains("available terminal width"));
        assert!(rendered.contains("narrow fixed"));
        assert!(rendered.contains("card"));
        assert!(rendered.contains("To reply: intercom"));
        assert!(!rendered.contains("Ctrl+O"));
    }

    // Port of test/inline-message.test.ts:53-79.
    #[test]
    fn collapsed_keeps_preview_reply_hint_and_expand_key() {
        let mut msg = message(
            "Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu. This tail should only appear when expanded because the collapsed preview is intentionally brief.",
        );
        msg.content.attachments = Some(vec![Attachment {
            kind: AttachmentKind::Snippet,
            name: "note.txt".to_string(),
            content: "important details".to_string(),
            language: None,
        }]);
        let card = InlineMessage {
            from: from(),
            message: msg,
            reply_command: Some("intercom({ action: \"reply\", message: \"...\" })".to_string()),
            body_text: None,
            collapsed: true,
        };
        let lines = card.render(&PlainTheme, 120);
        let rendered = lines.join("\n");
        assert_eq!(lines.len(), 4, "collapsed card is exactly 4 lines");
        for line in &lines {
            assert_eq!(visible_width(line), 120);
        }
        assert!(rendered.contains("Alpha beta gamma"));
        assert!(!rendered.contains("intentionally brief"));
        assert!(rendered.contains("To reply: intercom"));
        assert!(rendered.contains("Ctrl+O"));
        assert!(rendered.contains("1 attachment"));
    }

    #[test]
    fn content_markdown_matches_pi_send_message_body() {
        let mut card = InlineMessage::new(from(), message("body here"));
        card.reply_command = Some("intercom({ action: \"reply\" })".to_string());
        let md = card.content_markdown();
        assert!(md.starts_with("**📨 From sender** (/tmp/project)"));
        assert!(md.contains("To reply, use the intercom tool: intercom({ action: \"reply\" })"));
        assert!(md.ends_with("body here"));
    }

    #[test]
    fn narrow_width_degrades_to_a_from_line() {
        let card = InlineMessage::new(from(), message("hi"));
        let lines = card.render(&PlainTheme, 2);
        assert_eq!(lines.len(), 1);
        assert!(visible_width(&lines[0]) <= 2);
    }
}
