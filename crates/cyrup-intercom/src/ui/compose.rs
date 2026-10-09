//! [`ComposeOverlay`] — a port of `pi-intercom/ui/compose.ts` `ComposeOverlay` (the width-72
//! message-compose box), plus [`compose_send`] — the send leg the `/intercom` slash command drives.
//!
//! WIRING: picking a session with Enter in the live `/intercom` list opens [`ComposeOverlayHost`]
//! through `HostServices::open_overlay` (`v0.16.1 index.ts:3207-3210`): keystrokes reach
//! [`ComposeOverlay::handle_input`], Enter sends over the broker from inside the overlay (pi's
//! private `sendMessage`, `compose.ts:76-103`) and a delivery failure stays on screen with the
//! buffer intact. [`compose_send`] runs the broker send for the text form
//! `/intercom <target> <message>` ([`crate::extension::IntercomExtension`]'s `execute_command`).

use std::sync::{Arc, Mutex};

use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayLine, OverlayOptions, OverlayOutcome};

use crate::error::{IntercomError, Result};
use crate::transport::client::{IntercomClient, SendOptions, SendResult};
use crate::transport::protocol::SessionInfo;
use crate::ui::overlay::{OverlayTheme, key_to_data, to_overlay_line};
use crate::ui::{DefaultKeybindings, Keybindings, Theme, truncate_to_width, visible_width};

/// The maximum inner width of the compose overlay (pi `Math.min(width, 72)`).
pub const COMPOSE_MAX_WIDTH: usize = 72;

/// The outcome of a compose session (pi's exported `ComposeResult`, `compose.ts:7-11`), consumed by
/// the host after `done(result)`: `sent: false` (the `Default`) on cancel, or `sent: true` with the
/// broker message id + the text that was sent on a successful [`ComposeOverlay::send_message`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposeResult {
    /// Whether the message was actually sent (pi `sent: boolean`).
    pub sent: bool,
    /// The broker-assigned message id; set only when `sent` (pi `messageId?: string`).
    pub message_id: Option<String>,
    /// The text that was sent; set only when `sent` (pi `text?: string`).
    pub text: Option<String>,
}

/// What a keystroke did to the compose overlay (pi's `ComposeOverlay.handleInput` effects).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposeAction {
    /// The buffer changed / needs a redraw.
    Redraw,
    /// The keystroke was ignored (no state change; e.g. an escape sequence while composing).
    Ignore,
    /// The user cancelled (`tui.select.cancel`).
    Cancel,
    /// The user confirmed a non-empty buffer; the caller sends this (trimmed) text.
    Submit(String),
}

/// The message-compose overlay (pi `ComposeOverlay`).
#[derive(Clone, Debug)]
pub struct ComposeOverlay {
    target: SessionInfo,
    target_label: String,
    input_buffer: String,
    sending: bool,
    error: Option<String>,
}

impl ComposeOverlay {
    /// A fresh overlay targeting `target` (displayed as `target_label`).
    #[must_use]
    pub fn new(target: SessionInfo, target_label: String) -> Self {
        Self {
            target,
            target_label,
            input_buffer: String::new(),
            sending: false,
            error: None,
        }
    }

    /// The session this overlay sends to.
    #[must_use]
    pub fn target(&self) -> &SessionInfo {
        &self.target
    }

    /// The current input text (for the caller's send on [`ComposeAction::Submit`]).
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input_buffer
    }

    /// Mark the overlay as sending (pi `sending = true` in `sendMessage`); the caller sets this before
    /// running the async broker send so the next render shows "Sending…".
    pub fn set_sending(&mut self, sending: bool) {
        self.sending = sending;
    }

    /// Whether a broker send is in flight (pi's `private sending`). The read half of
    /// [`Self::set_sending`]: the host that drives the overlay owns this flag across an `await`, so
    /// it has to be able to read back what [`Self::send_message`] left it as — pi's overlay reads
    /// `this.sending` in both `handleInput` and `render` for exactly that reason
    /// (`compose.ts:34,76,112`).
    #[must_use]
    pub fn is_sending(&self) -> bool {
        self.sending
    }

    /// The recorded delivery error, if any (pi's `private error`). The read half of
    /// [`Self::set_error`], same rationale as [`Self::is_sending`].
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Record a delivery error (pi `error = …; sending = false`).
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.sending = false;
    }

    /// Handle one raw input chunk (pi `handleInput`): cancel, confirm (send non-empty), backspace, or
    /// append printable text. Escape sequences are ignored while composing.
    pub fn handle_input(&mut self, keybindings: &dyn Keybindings, data: &str) -> ComposeAction {
        if self.sending {
            return ComposeAction::Ignore;
        }
        if keybindings.matches(data, "tui.select.cancel") {
            return ComposeAction::Cancel;
        }
        if data.starts_with('\x1b') {
            return ComposeAction::Ignore;
        }
        if keybindings.matches(data, "tui.select.confirm") {
            let trimmed = self.input_buffer.trim();
            if trimmed.is_empty() {
                return ComposeAction::Ignore;
            }
            return ComposeAction::Submit(trimmed.to_string());
        }
        if keybindings.matches(data, "tui.editor.deleteCharBackward") {
            self.input_buffer.pop();
            return ComposeAction::Redraw;
        }
        // Append the printable chars (drop control chars, pi's `c >= " "` filter).
        let printable: String = data.chars().filter(|c| *c >= ' ').collect();
        if printable.is_empty() {
            return ComposeAction::Ignore;
        }
        self.input_buffer.push_str(&printable);
        ComposeAction::Redraw
    }

    /// Send the current (trimmed) input buffer to the target (pi's private `ComposeOverlay.sendMessage`,
    /// `compose.ts:76-103`): marks the overlay `sending`, clears any prior error, then awaits the broker
    /// send. A non-delivered result or a transport error records the failure and clears `sending` (so a
    /// re-render shows the retry prompt with the buffer preserved); a delivered send returns the
    /// [`ComposeResult`] for the caller's `done` callback and — exactly like pi, which never resets
    /// `sending` on the success path because the overlay is torn down instead of re-rendered — leaves
    /// `sending` set.
    pub async fn send_message(&mut self, client: &Arc<IntercomClient>) -> Option<ComposeResult> {
        self.sending = true;
        self.error = None;
        let text = self.input_buffer.trim().to_string();
        match client
            .send(
                &self.target.id,
                SendOptions {
                    text: text.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(result) if result.delivered => Some(ComposeResult {
                sent: true,
                message_id: Some(result.id),
                text: Some(text),
            }),
            Ok(result) => {
                self.error = Some(result.reason.unwrap_or_else(|| {
                    "Message not delivered. Session may not exist or has disconnected.".to_string()
                }));
                self.sending = false;
                None
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.sending = false;
                None
            }
        }
    }

    /// Render the overlay to `width` display columns (pi `ComposeOverlay.render`). Inner width is
    /// clamped to [`COMPOSE_MAX_WIDTH`]; every line is exactly `min(width, 72)` columns.
    #[must_use]
    pub fn render(
        &self,
        theme: &dyn Theme,
        keybindings: &dyn Keybindings,
        width: usize,
    ) -> Vec<String> {
        let inner_width = width.clamp(1, COMPOSE_MAX_WIDTH);
        if inner_width == 1 {
            return vec![theme.fg("accent", "│")];
        }
        let content_width = inner_width.saturating_sub(2);
        let footer = format!(
            "{}: Send • {}: Close",
            keybindings.get_keys("tui.select.confirm").join("/"),
            keybindings.get_keys("tui.select.cancel").join("/"),
        );
        let row = |text: &str| box_row(theme, content_width, text);
        let rule = |left: char, right: char| {
            theme.fg(
                "accent",
                &format!("{left}{}{right}", "─".repeat(content_width)),
            )
        };

        let mut lines: Vec<String> = Vec::new();
        lines.push(rule('╭', '╮'));
        lines.push(row(&theme.bold(&format!(" Send to: {}", self.target_label))));
        lines.push(row(&theme.fg(
            "dim",
            &format!(" {} • {}", self.target.cwd, self.target.model),
        )));
        lines.push(rule('├', '┤'));
        lines.push(row(""));

        if self.sending {
            lines.push(row(&theme.fg("dim", " Sending...")));
        } else if let Some(err) = &self.error {
            lines.push(row(&theme.fg("error", &format!(" Error: {err}"))));
            lines.push(row(""));
            lines.push(row(&format!(" > {}\u{2588}", self.input_buffer)));
        } else {
            lines.push(row(&format!(" > {}\u{2588}", self.input_buffer)));
        }

        lines.push(row(""));
        lines.push(rule('├', '┤'));
        lines.push(row(&theme.fg("dim", &format!(" {footer}"))));
        lines.push(rule('╰', '╯'));
        lines
    }
}

/// A `│ … │` overlay row: truncate `text` to `content_width`, right-pad, frame with accent borders.
fn box_row(theme: &dyn Theme, content_width: usize, text: &str) -> String {
    let clipped = truncate_to_width(text, content_width);
    let pad = content_width.saturating_sub(visible_width(&clipped));
    format!(
        "{}{clipped}{}{}{}",
        theme.fg("accent", "│"),
        theme.reset(),
        " ".repeat(pad),
        theme.fg("accent", "│")
    )
}

/// The outcome of one in-flight send, handed from the spawned broker call back to the overlay.
type SendOutcome = std::result::Result<SendResult, String>;

/// The live compose box: [`ComposeOverlay`] behind the host's overlay seam — pi `ctx.ui.custom((tui,
/// theme, keybindings, done) => new ComposeOverlay(…, overlayClient, done), { overlay: true,
/// overlayOptions: { width: 72 } })` (`v0.16.1 index.ts:3207-3210`).
///
/// [`InteractiveOverlay::handle_key`] is synchronous, so pi's `async sendMessage` is split at its
/// `await`: Enter marks the overlay sending and spawns `client.send` on the captured runtime, and
/// [`InteractiveOverlay::tick`] collects the answer — `done({ sent: true, … })` closes the overlay,
/// a refusal or a transport error is shown inline with the buffer kept (`compose.ts:76-103`).
pub struct ComposeOverlayHost {
    overlay: ComposeOverlay,
    client: Arc<IntercomClient>,
    runtime: tokio::runtime::Handle,
    in_flight: Option<(String, tokio::sync::oneshot::Receiver<SendOutcome>)>,
    result: Arc<Mutex<Option<ComposeResult>>>,
    closed: bool,
}

impl ComposeOverlayHost {
    /// Wrap `overlay`, sending through `client` on `runtime` and publishing the outcome into
    /// `result`.
    #[must_use]
    pub fn new(
        overlay: ComposeOverlay,
        client: Arc<IntercomClient>,
        runtime: tokio::runtime::Handle,
        result: Arc<Mutex<Option<ComposeResult>>>,
    ) -> Self {
        Self {
            overlay,
            client,
            runtime,
            in_flight: None,
            result,
            closed: false,
        }
    }

    fn done(&mut self, result: ComposeResult) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.closed = true;
    }
}

impl InteractiveOverlay for ComposeOverlayHost {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        self.overlay
            .render(&OverlayTheme, &DefaultKeybindings, width)
            .iter()
            .map(|line| to_overlay_line(line))
            .collect()
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let Some(data) = key_to_data(key) else {
            return OverlayOutcome::Ignored;
        };
        match self.overlay.handle_input(&DefaultKeybindings, &data) {
            ComposeAction::Redraw => OverlayOutcome::Redraw,
            ComposeAction::Ignore => OverlayOutcome::Ignored,
            ComposeAction::Cancel => {
                self.done(ComposeResult::default());
                OverlayOutcome::Close
            }
            ComposeAction::Submit(text) => {
                // `sendMessage()`'s synchronous prelude: `sending = true; error = null`.
                self.overlay.set_sending(true);
                self.overlay.error = None;
                let (tx, rx) = tokio::sync::oneshot::channel();
                let client = self.client.clone();
                let target = self.overlay.target.id.clone();
                let body = text.clone();
                self.runtime.spawn(async move {
                    let outcome = client
                        .send(
                            &target,
                            SendOptions {
                                text: body,
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(outcome);
                });
                self.in_flight = Some((text, rx));
                OverlayOutcome::Redraw
            }
        }
    }

    fn refresh_ms(&self) -> u64 {
        50
    }

    fn tick(&mut self) -> bool {
        let Some((text, rx)) = self.in_flight.as_mut() else {
            return false;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                Err("the send task ended without an answer".to_string())
            }
        };
        let text = std::mem::take(text);
        self.in_flight = None;
        match outcome {
            Ok(result) if result.delivered => self.done(ComposeResult {
                sent: true,
                message_id: Some(result.id),
                text: Some(text),
            }),
            Ok(result) => self.overlay.set_error(result.reason.unwrap_or_else(|| {
                "Message not delivered. Session may not exist or has disconnected.".to_string()
            })),
            Err(error) => self.overlay.set_error(error),
        }
        true
    }

    fn should_close(&self) -> bool {
        self.closed
    }

    fn options(&self) -> OverlayOptions {
        OverlayOptions {
            width: Some(COMPOSE_MAX_WIDTH as u16),
            ..OverlayOptions::default()
        }
    }
}

/// Send a composed message to `target_id` over the broker (pi `ComposeOverlay.sendMessage`,
/// `compose.ts:76-103`): reject an empty body, else `client.send(target, { text })` (a plain message,
/// NOT an ask). Returns the broker's [`SendResult`]. Used by the `/intercom` slash command.
///
/// # Errors
/// [`IntercomError::Client`] on an empty message, a non-delivered send, or a transport failure.
pub async fn compose_send(
    client: &Arc<IntercomClient>,
    target_id: &str,
    text: &str,
) -> Result<SendResult> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(IntercomError::Client(
            "message must not be empty".to_string(),
        ));
    }
    let result = client
        .send(
            target_id,
            SendOptions {
                text: trimmed.to_string(),
                ..Default::default()
            },
        )
        .await?;
    if !result.delivered {
        return Err(IntercomError::Client(result.reason.clone().unwrap_or_else(
            || "Message not delivered. Session may not exist or has disconnected.".to_string(),
        )));
    }
    Ok(result)
}

/// Unit tests only. The two `send_message` proofs that used to live here spawned the real
/// `cyrup-intercom-broker` binary as a subprocess, which makes them seam tests; they now live in
/// `crates/cyrup-it/tests/intercom/compose_send_leg.rs`. They reach the overlay's `sending`/`error`
/// state through [`ComposeOverlay::is_sending`]/[`ComposeOverlay::error`], which is why those two
/// accessors exist. See docs/TEST-ARCHITECTURE.md §9.1.
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::ui::{DefaultKeybindings, PlainTheme};

    fn session() -> SessionInfo {
        SessionInfo {
            endpoint_epoch: None,
            id: "session-12345678".to_string(),
            name: Some("subagent-chat-019ecaf6".to_string()),
            runtime_fallback_alias: None,
            cwd: "/Users/envvar/.config/ghostty".to_string(),
            model: "bsy-deepseek-v4-pro".to_string(),
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
            herdr_pane_id: None,
            herdr_location: None,
            extra: Default::default(),
        }
    }

    /// The overlay-width mock (test/overlay-width.test.ts:9-25): `getKeys` → `["enter"]` for confirm,
    /// else `["escape","ctrl+c"]`; `matches` always false.
    struct MockKeybindings;
    impl Keybindings for MockKeybindings {
        fn matches(&self, _data: &str, _action: &str) -> bool {
            false
        }
        fn get_keys(&self, action: &str) -> Vec<String> {
            if action.contains("confirm") {
                vec!["enter".to_string()]
            } else {
                vec!["escape".to_string(), "ctrl+c".to_string()]
            }
        }
    }

    // Port of test/overlay-width.test.ts:44-58.
    #[test]
    fn renders_lines_at_the_declared_overlay_width() {
        let overlay = ComposeOverlay::new(session(), "subagent-chat-019ecaf6".to_string());
        for width in [1usize, 2, 20, 40, 72] {
            let lines = overlay.render(&PlainTheme, &MockKeybindings, width);
            assert!(!lines.is_empty());
            for (i, line) in lines.iter().enumerate() {
                assert_eq!(
                    visible_width(line),
                    width,
                    "width {width} line {i}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn input_editing_and_submit() {
        let kb = DefaultKeybindings;
        let mut overlay = ComposeOverlay::new(session(), "label".to_string());
        assert_eq!(overlay.handle_input(&kb, "hi"), ComposeAction::Redraw);
        assert_eq!(overlay.input(), "hi");
        // Backspace deletes the last char.
        assert_eq!(overlay.handle_input(&kb, "\x7f"), ComposeAction::Redraw);
        assert_eq!(overlay.input(), "h");
        // Arrow keys (escape sequences) are ignored while composing.
        assert_eq!(overlay.handle_input(&kb, "\x1b[A"), ComposeAction::Ignore);
        overlay.handle_input(&kb, "ello");
        // Enter submits the trimmed buffer.
        assert_eq!(
            overlay.handle_input(&kb, "\r"),
            ComposeAction::Submit("hello".to_string())
        );
        // Esc cancels.
        assert_eq!(overlay.handle_input(&kb, "\x1b"), ComposeAction::Cancel);
    }

    #[test]
    fn empty_buffer_does_not_submit() {
        let kb = DefaultKeybindings;
        let mut overlay = ComposeOverlay::new(session(), "label".to_string());
        overlay.handle_input(&kb, "   ");
        assert_eq!(overlay.handle_input(&kb, "\r"), ComposeAction::Ignore);
    }
}
