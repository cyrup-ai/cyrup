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
//!   it to `HostServices::inject_message` — so the model always gets the `**From …**` attribution,
//!   the sender's cwd, the `_id …_` delivery-metadata line and the reply instruction, never the
//!   bare body.
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

/// The `intercom_message` inline card (pi `InlineMessageComponent`) AND the `details` payload pi
/// passes alongside it (`InboundMessageEntry` / `deliveredEntry`, `index.ts:65-70`, passed at
/// `:1216`, read back by the renderer at `:1817`).
///
/// The serde shape IS upstream's `deliveredEntry` — `{from, message, replyCommand?, bodyText?}` —
/// plus cyrup's `collapsed`, which upstream's renderer derives per-frame from `!options.expanded`.
/// One type serves the `append_entry` surface, the `inject_message` details, and the renderer that
/// reads them back, so the three can never drift apart.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineMessage {
    /// The sender's session info.
    pub from: SessionInfo,
    /// The received message.
    pub message: Message,
    /// The reply-hint command shown in the card, when the sender expects a reply and the hint is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_command: Option<String>,
    /// The pre-formatted body (text + attachment text). `None` falls back to `message.content.text`
    /// (pi `this.bodyText || this.message.content.text`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    /// Whether the card is collapsed (preview + expand hint) vs. fully expanded.
    #[serde(default)]
    pub collapsed: bool,
}

impl InlineMessage {
    /// Build a card. `body_text`/`reply_command` default to the un-decorated message + no hint
    /// (matching pi's optional constructor args).
    #[must_use]
    pub fn new(from: SessionInfo, message: Message) -> Self {
        Self {
            from,
            message,
            reply_command: None,
            body_text: None,
            collapsed: false,
        }
    }

    /// The pre-formatted body, or `message.content.text` when unset (pi `bodyText ||
    /// message.content.text`).
    #[must_use]
    pub fn body(&self) -> &str {
        self.body_text
            .as_deref()
            .unwrap_or(&self.message.content.text)
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
    /// (`v0.10.1 index.ts:890-895`):
    ///
    /// ```text
    /// `**From ${senderDisplay}** (${entry.from.cwd})${replyInstruction}\n\n_${deliveryMetadata}_\n\n${entry.bodyText}`
    /// ```
    ///
    /// Two corrections live here, and they are separate:
    /// - the `📨` is gone from v0.10.0 (`633e782`, "refactor: deslop intercom protocol cleanup"), so
    ///   the string the MODEL sees starts `**From …**`;
    /// - the `_{deliveryMetadata}_` segment has been present since v0.9.2 and was never ported. It
    ///   always carries at least `id {message.id}`, which is the only way the model learns the id
    ///   `intercom({action:"reply", replyTo})` needs without a separate `pending` round trip.
    #[must_use]
    pub fn content_markdown(&self) -> String {
        let reply_instruction = self
            .reply_command
            .as_deref()
            .map(|rc| format!("\n\nTo reply, use the intercom tool: {rc}"))
            .unwrap_or_default();
        format!(
            "**From {}** ({}){}\n\n_{}_\n\n{}",
            self.sender_display(),
            self.from.cwd,
            reply_instruction,
            format_inbound_delivery_metadata(&self.message),
            self.body()
        )
    }

    /// Render the bordered card to `width` display columns (pi `InlineMessageComponent.render`).
    /// Every returned line is exactly `width` columns wide (for `width >= 3`).
    #[must_use]
    pub fn render(&self, theme: &dyn Theme, width: usize) -> Vec<String> {
        self.render_with(theme, width, self.collapsed, None)
    }

    /// [`Self::render`] with the collapsed flag and the wrap cache supplied by the caller.
    ///
    /// The live component resolves `collapsed` from `RenderCtx::expanded` on EVERY frame rather
    /// than from the struct, which is what lets one pushed card open and close in place.
    #[must_use]
    pub fn render_with(
        &self,
        theme: &dyn Theme,
        width: usize,
        collapsed: bool,
        caches: Option<&InlineMessageComponent>,
    ) -> Vec<String> {
        let sender_name = self.sender_display();
        if width < 3 {
            return vec![truncate_to_width(&format!("From {sender_name}"), width)];
        }
        let body_width = std::cmp::max(1, width - 2);
        let mut lines: Vec<String> = Vec::new();

        // Header: ╭ From: sender (cwd) ───╮ (`v0.10.1 ui/inline-message.ts:45`; the `📨` was removed
        // by the v0.10.0 deslop, `633e782`).
        let header = format!(" From: {sender_name} ({}) ", self.from.cwd);
        let header_src = if collapsed {
            format!("{header} Ctrl+O expands ")
        } else {
            header
        };
        let header_text = truncate_to_width(&header_src, body_width);
        let header_pad = body_width.saturating_sub(visible_width(&header_text));
        // Two-tone, as upstream: `muted` borders around a `toolTitle` header. Colouring the whole
        // line `accent` is invisible under `PlainTheme` and wrong under a real one.
        lines.push(format!(
            "{}{}{}",
            theme.fg("muted", "╭"),
            theme.fg("toolTitle", &header_text),
            theme.fg("muted", &format!("{}╮", "─".repeat(header_pad))),
        ));

        if collapsed {
            self.render_collapsed(theme, body_width, &mut lines, caches);
            return lines;
        }

        // Body content lines, in the custom-message text role (upstream `theme.fg("text", line)`).
        let wrapped = match caches {
            Some(c) => c.wrapped(body_width),
            None => wrap_text(self.body(), body_width),
        };
        for line in wrapped {
            lines.push(card_row(theme, body_width, &theme.fg("text", &line)));
        }

        // Reply hint block.
        if let Some(rc) = &self.reply_command {
            lines.push(card_row(theme, body_width, ""));
            let hint = theme.fg("dim", &format!(" To reply: {rc}"));
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
                let label = theme.fg("dim", &format!(" Attachment: {}", att.name));
                lines.push(card_row(theme, body_width, &label));
            }
        }

        // Reply-to breadcrumb (only when this is NOT itself an ask).
        if self.message.reply_to.is_some() && self.message.expects_reply != Some(true) {
            lines.push(card_row(theme, body_width, ""));
            let short: String = self
                .message
                .reply_to
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(8)
                .collect();
            let reply = theme.fg("dim", &format!(" Reply to {short}"));
            lines.push(card_row(theme, body_width, &reply));
        }

        // ICOM-056 / `v0.12.0 ui/inline-message.ts:109-112`: who actually originated this message,
        // when it was not the peer agent itself. The LAST block, so it reads as a footnote to the
        // body — note this is NOT the same position it takes in the collapsed meta line.
        if let Some(provenance) = &self.message.provenance {
            lines.push(card_row(theme, body_width, ""));
            let via = theme.fg(
                "dim",
                &format!(" Via extension: {}", provenance.extension_name),
            );
            lines.push(card_row(theme, body_width, &via));
        }

        lines.push(theme.fg("muted", &format!("╰{}╯", "─".repeat(body_width))));
        lines
    }

    /// The collapsed card body: preview line + meta line + bottom border (pi lines 46-65). Always
    /// yields exactly 3 lines here (header pushed by the caller → 4 total).
    fn render_collapsed(
        &self,
        theme: &dyn Theme,
        body_width: usize,
        lines: &mut Vec<String>,
        caches: Option<&InlineMessageComponent>,
    ) {
        // Preview: the body with runs of whitespace collapsed to single spaces, trimmed.
        let preview: String = match caches {
            Some(c) => c.collapsed_preview(self.body()),
            None => self.body().split_whitespace().collect::<Vec<_>>().join(" "),
        };
        lines.push(card_row(theme, body_width, &theme.fg("text", &preview)));

        let mut meta: Vec<String> = Vec::new();
        if let Some(rc) = &self.reply_command {
            meta.push(format!("To reply: {rc}"));
        }
        if let Some(atts) = &self.message.content.attachments {
            let count = atts.len();
            if count > 0 {
                let plural = if count == 1 { "" } else { "s" };
                meta.push(format!("{count} attachment{plural}"));
            }
        }
        // ICOM-056 / `v0.12.0 ui/inline-message.ts:70`: after the attachment count and BEFORE the
        // reply-to breadcrumb. Upstream's collapsed `meta` order is deliberately not the expanded
        // block order, where provenance comes last.
        if let Some(provenance) = &self.message.provenance {
            meta.push(format!("Via {}", provenance.extension_name));
        }
        if self.message.reply_to.is_some() && self.message.expects_reply != Some(true) {
            let short: String = self
                .message
                .reply_to
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(8)
                .collect();
            meta.push(format!("Reply to {short}"));
        }
        meta.push("Ctrl+O to expand".to_string());

        let meta_line = theme.fg("dim", &format!(" {}", meta.join(" · ")));
        lines.push(card_row(theme, body_width, &meta_line));
        lines.push(theme.fg("muted", &format!("╰{}╯", "─".repeat(body_width))));
    }
}

/// `formatInboundDeliveryMetadata` (`v0.10.1 index.ts:471-485`, identical at v0.9.2 `:446-460`):
///
/// ```text
/// const parts = [`id ${message.id}`];
/// if (typeof message.senderSequence === "number") parts.push(`seq ${message.senderSequence}`);
/// if (message.supersedes) parts.push(`supersedes ${message.supersedes}`);
/// if (message.retryOf) parts.push(`retry of ${message.retryOf}`);
/// … sent / broker delivered / receiver received / injected, each an ISO-8601 timestamp …
/// return parts.join(" · ");
/// ```
///
/// It ALWAYS emits at least `id {message.id}` — that unconditional first part is the whole reason
/// the segment exists, because it is the only place the model is shown the id that
/// `intercom({action:"reply", replyTo})` takes.
#[must_use]
pub fn format_inbound_delivery_metadata(message: &Message) -> String {
    let mut parts = vec![format!("id {}", message.id)];
    if let Some(seq) = &message.sender_sequence {
        // pi's guard is `typeof … === "number"`, i.e. any JSON number including a float — the
        // rendering is `String(n)`, which `serde_json::Number`'s `Display` matches.
        parts.push(format!("seq {seq}"));
    }
    if let Some(supersedes) = &message.supersedes {
        parts.push(format!("supersedes {supersedes}"));
    }
    if let Some(retry_of) = &message.retry_of {
        parts.push(format!("retry of {retry_of}"));
    }
    if let Some(sent) = format_message_timestamp(Some(&message.timestamp)) {
        parts.push(format!("sent {sent}"));
    }
    if let Some(at) = format_message_timestamp(message.broker_delivered_at.as_ref()) {
        parts.push(format!("broker delivered {at}"));
    }
    if let Some(at) = format_message_timestamp(message.receiver_received_at.as_ref()) {
        parts.push(format!("receiver received {at}"));
    }
    if let Some(at) = format_message_timestamp(message.injected_at.as_ref()) {
        parts.push(format!("injected {at}"));
    }
    parts.join(" · ")
}

/// `formatMessageTimestamp` (`v0.10.1 index.ts:468-470`):
/// `typeof t === "number" && Number.isFinite(t) ? new Date(t).toISOString() : undefined`.
///
/// CYRUP-DELTA (`v0.10.1 index.ts:469`): JS throws `RangeError` for `|ms| > 8.64e15`; this port
/// returns `None` for anything outside the representable range instead, because the crate is
/// `#![forbid(unsafe_code)]` and no-panic by policy and a hostile peer must not be able to abort a
/// delivery by sending an absurd timestamp.
fn format_message_timestamp(value: Option<&serde_json::Number>) -> Option<String> {
    let ms = value?.as_f64()?;
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    // `new Date(ms)` truncates the fractional part toward zero for positive values; JS `Date`
    // stores an integer millisecond count.
    Some(format_iso8601_millis(ms.trunc() as i64))
}

/// `new Date(ms).toISOString()` — `YYYY-MM-DDTHH:MM:SS.mmmZ`. Pure arithmetic (Howard Hinnant's
/// proleptic-Gregorian `civil_from_days`), so it needs no date-time dependency and cannot panic.
/// Mirrors `cyrup-ext-subagents`' `background::run_status::format_iso8601_millis`, which is
/// `pub(crate)` there and therefore not reusable across the crate boundary.
fn format_iso8601_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → proleptic-Gregorian `(y, m, d)`.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// A `│ … │` card row: truncate `inner` to `body_width`, right-pad with spaces, frame with borders
/// (the whole row accent-colored, matching pi's `theme.fg("accent", …)` wrapping).
impl InlineMessage {
    /// Rebuild the card from the `details` payload `inject_message` carried (upstream
    /// `InboundMessageEntry`, `index.ts:65-70`) — the exact object
    /// [`crate::inbound::surface_incoming_message`] already writes.
    ///
    /// `None` when `from` or `message` will not deserialize, which is upstream's
    /// `if (!details) return undefined`: a v0.9.2 peer, or a payload written before the seam
    /// carried `details`, falls through to the pre-rendered card rather than drawing an empty box.
    #[must_use]
    pub fn from_details(details: &serde_json::Value) -> Option<Self> {
        Some(Self {
            from: serde_json::from_value(details.get("from")?.clone()).ok()?,
            message: serde_json::from_value(details.get("message")?.clone()).ok()?,
            reply_command: details
                .get("replyCommand")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            body_text: details
                .get("bodyText")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Resolved per render from `RenderCtx::expanded`, never stored.
            collapsed: false,
        })
    }
}

/// The live component (pi `InlineMessageComponent`). Holds the immutable card plus the two caches
/// upstream memoizes.
///
/// Theme styling is deliberately OUTSIDE both caches, so a live theme change repaints without
/// invalidating either — upstream states this in its own comment.
#[derive(Debug)]
pub struct InlineMessageComponent {
    card: InlineMessage,
    /// pi `collapsedPreview` — the whitespace-collapsed body. Independent of width and theme.
    collapsed_preview: std::sync::OnceLock<String>,
    /// pi `wrappedBody: {width, lines}` — invalidated only when the body width changes, so a resize
    /// re-wraps and a mere repaint does not.
    wrapped_body: std::sync::Mutex<Option<(usize, Vec<String>)>>,
}

impl InlineMessageComponent {
    /// Wrap a card as a live component.
    #[must_use]
    pub fn new(card: InlineMessage) -> Self {
        Self {
            card,
            collapsed_preview: std::sync::OnceLock::new(),
            wrapped_body: std::sync::Mutex::new(None),
        }
    }

    /// pi `this.wrappedBody?.width !== bodyWidth`.
    fn wrapped(&self, body_width: usize) -> Vec<String> {
        // The crate is no-panic by policy; a poisoned cache is recovered, never unwrapped.
        let mut guard = self.wrapped_body.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_none_or(|(w, _)| *w != body_width) {
            *guard = Some((body_width, wrap_text(self.card.body(), body_width)));
        }
        guard
            .as_ref()
            .map(|(_, lines)| lines.clone())
            .unwrap_or_default()
    }

    /// pi `collapsedPreview` — computed once, independent of width and theme.
    fn collapsed_preview(&self, body: &str) -> String {
        self.collapsed_preview
            .get_or_init(|| body.split_whitespace().collect::<Vec<_>>().join(" "))
            .clone()
    }
}

impl cyrup_ext::RenderedComponent for InlineMessageComponent {
    /// pi `render(width)`. `collapsed = !options.expanded` — the flag comes from the LIVE context on
    /// every frame, never from the struct.
    fn render(&self, ctx: &cyrup_ext::RenderCtx<'_>) -> Vec<String> {
        self.card.render_with(
            &RenderThemeAdapter(ctx.theme),
            ctx.width,
            !ctx.expanded,
            Some(self),
        )
    }
}

/// Bridges the host's [`cyrup_ext::RenderTheme`] onto this module's [`Theme`]. The two have the same
/// shape; the split exists because `cyrup-ext` must not depend on this crate's UI module.
struct RenderThemeAdapter<'a>(&'a dyn cyrup_ext::RenderTheme);

impl Theme for RenderThemeAdapter<'_> {
    fn fg(&self, color: &str, text: &str) -> String {
        self.0.fg(color, text)
    }
    fn bold(&self, text: &str) -> String {
        self.0.bold(text)
    }
}

fn card_row(theme: &dyn Theme, body_width: usize, inner: &str) -> String {
    let text = truncate_to_width(inner, body_width);
    let pad = body_width.saturating_sub(visible_width(&text));
    // Only the borders are coloured; the content passes through with whatever styling the caller
    // already applied. Wrapping the whole row would overpaint it.
    format!(
        "{}{text}{}",
        theme.fg("muted", "│"),
        theme.fg("muted", &format!("{}│", " ".repeat(pad))),
    )
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
            runtime_fallback_alias: None,
            cwd: "/tmp/project".to_string(),
            model: "model".to_string(),
            pid: 1u32.into(),
            started_at: 0u64.into(),
            last_activity: 0u64.into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            tmux_pane: None,
            extra: Default::default(),
        }
    }

    fn message(text: &str) -> Message {
        Message {
            id: "message-1".to_string(),
            timestamp: 0u64.into(),
            reply_to: None,
            expects_reply: None,
            content: MessageContent {
                text: text.to_string(),
                attachments: None,
                ..Default::default()
            },
            ..Default::default()
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
            extra: Default::default(),
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

    /// `v0.10.1 index.ts:890-895`, byte for byte. Two independent regressions are pinned here:
    /// the v0.10.0 deslop (no `📨`) and the v0.9.2 `_deliveryMetadata_` line that was never ported.
    #[test]
    fn content_markdown_matches_pi_send_message_body() {
        let mut card = InlineMessage::new(from(), message("body here"));
        card.reply_command = Some("intercom({ action: \"reply\" })".to_string());
        let md = card.content_markdown();
        assert_eq!(
            md,
            "**From sender** (/tmp/project)\n\nTo reply, use the intercom tool: intercom({ action: \"reply\" })\n\n_id message-1 · sent 1970-01-01T00:00:00.000Z_\n\nbody here"
        );
    }

    /// ICOM-029 — the `details` the seam carries and the `content` the model reads are built from
    /// ONE card (`inbound.rs:238-239`: `card.content_markdown()` then `to_value(&card)`), so the two
    /// must describe the same message. This pins that correspondence rather than the card's shape
    /// alone: a future refactor that rebuilt `details` from a second source would still render a
    /// plausible card while the renderer and the model disagreed about what arrived.
    #[test]
    fn the_details_and_the_content_markdown_describe_the_same_message() {
        let mut msg = message("the body as sent");
        msg.injected_at = Some(1_609_459_200_003u64.into());
        let card = InlineMessage {
            body_text: Some("the body as sent".to_string()),
            ..InlineMessage::new(from(), msg)
        };

        let content = card.content_markdown();
        let details = serde_json::to_value(&card).expect("the card serializes");
        let round_tripped =
            InlineMessage::from_details(&details).expect("details deserialize as the inline card");

        // `bodyText` is the string the markdown's body was rendered from — not a re-derived twin.
        let body = round_tripped
            .body_text
            .as_deref()
            .expect("the card carries bodyText");
        assert!(
            content.ends_with(body),
            "the content's body IS the card's bodyText: content={content:?} bodyText={body:?}"
        );

        // The stamped `injectedAt` survives into `details` and is the one the metadata line rendered.
        assert_eq!(
            round_tripped.message.injected_at, card.message.injected_at,
            "the injectedAt stamp survives serialization"
        );
        assert!(
            content.contains(&round_tripped.message.id),
            "the id in details is the id the metadata line published: {content:?}"
        );
    }

    /// `v0.10.1 index.ts:471-472`: the `id` part is UNCONDITIONAL. A message carrying no optional
    /// envelope field at all must still hand the model its own id, because that id is what
    /// `intercom({action:"reply", replyTo})` takes and `pending` is otherwise the only place it
    /// appears.
    #[test]
    fn delivery_metadata_always_carries_the_message_id() {
        let mut msg = message("hi");
        msg.timestamp = 0u64.into();
        assert_eq!(
            format_inbound_delivery_metadata(&msg),
            "id message-1 · sent 1970-01-01T00:00:00.000Z"
        );
    }

    /// `v0.10.1 index.ts:471-485` in declaration order, joined by ` · `.
    #[test]
    fn delivery_metadata_renders_every_envelope_field_in_upstream_order() {
        let mut msg = message("hi");
        msg.timestamp = 1_609_459_200_000u64.into();
        msg.sender_sequence = Some(3u64.into());
        msg.supersedes = Some("older".to_string());
        msg.retry_of = Some("attempt-1".to_string());
        msg.broker_delivered_at = Some(1_609_459_200_001u64.into());
        msg.receiver_received_at = Some(1_609_459_200_002u64.into());
        msg.injected_at = Some(1_609_459_200_003u64.into());
        assert_eq!(
            format_inbound_delivery_metadata(&msg),
            "id message-1 · seq 3 · supersedes older · retry of attempt-1 \
             · sent 2021-01-01T00:00:00.000Z · broker delivered 2021-01-01T00:00:00.001Z \
             · receiver received 2021-01-01T00:00:00.002Z · injected 2021-01-01T00:00:00.003Z"
        );
    }

    /// `new Date(ms).toISOString()`.
    #[test]
    fn iso8601_matches_javascript_date_to_iso_string() {
        assert_eq!(format_iso8601_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_iso8601_millis(1_609_459_200_000),
            "2021-01-01T00:00:00.000Z"
        );
        assert_eq!(
            format_iso8601_millis(1_609_459_245_296),
            "2021-01-01T00:00:45.296Z"
        );
        // Pre-epoch: JS renders a negative offset as a real civil date, not a wrapped one.
        assert_eq!(format_iso8601_millis(-1), "1969-12-31T23:59:59.999Z");
    }

    /// The v0.10.0 deslop applied to the card itself (`v0.10.1 ui/inline-message.ts:45,65,68,70,90,99,105`).
    #[test]
    fn card_carries_no_deslopped_emoji() {
        let mut msg = message("body");
        msg.reply_to = Some("prev-message".to_string());
        msg.content.attachments = Some(vec![Attachment {
            kind: AttachmentKind::Snippet,
            name: "note.txt".to_string(),
            content: "x".to_string(),
            language: None,
            extra: Default::default(),
        }]);
        for collapsed in [false, true] {
            let card = InlineMessage {
                from: from(),
                message: msg.clone(),
                reply_command: Some("intercom({ action: \"reply\" })".to_string()),
                body_text: None,
                collapsed,
            };
            let rendered = card.render(&PlainTheme, 120).join("\n");
            for slop in ["📨", "📎", "↩", "↳"] {
                assert!(
                    !rendered.contains(slop),
                    "v0.10.0 removed {slop:?}:\n{rendered}"
                );
            }
            assert!(rendered.contains("From: sender"));
            assert!(rendered.contains("To reply: intercom"));
        }
        // Expanded lists attachments as `Attachment: <name>` (`ui/inline-message.ts:99`).
        let card = InlineMessage {
            from: from(),
            message: msg,
            reply_command: None,
            body_text: None,
            collapsed: false,
        };
        assert!(
            card.render(&PlainTheme, 120)
                .join("\n")
                .contains("Attachment: note.txt")
        );
    }

    #[test]
    fn narrow_width_degrades_to_a_from_line() {
        let card = InlineMessage::new(from(), message("hi"));
        let lines = card.render(&PlainTheme, 2);
        assert_eq!(lines.len(), 1);
        assert!(visible_width(&lines[0]) <= 2);
    }
}
