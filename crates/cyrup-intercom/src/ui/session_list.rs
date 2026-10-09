//! [`SessionListOverlay`] — a port of `pi-intercom/ui/session-list.ts` `SessionListOverlay` (the
//! width-88, max-8-visible session picker the `/intercom` overlay opens), `v0.16.1`.
//!
//! WIRING: bare `/intercom` and the `alt+m` shortcut open it LIVE through
//! `HostServices::open_overlay`, wrapped in [`SessionListOverlayHost`]; Enter answers
//! [`SessionListSelection::Message`] and `h` answers [`SessionListSelection::Handover`]
//! (`session-list.ts:52-55,:103-109`), which the extension turns into the compose overlay or the
//! handover picker (`index.ts:3191-3194`). With no interactive surface the same
//! [`SessionListOverlay::render`] is the `/intercom` command's text output.

use std::sync::{Arc, Mutex};

use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayLine, OverlayOptions, OverlayOutcome};

use crate::identity::short_session_id;
use crate::transport::protocol::SessionInfo;
use crate::ui::overlay::{OverlayTheme, key_to_data, to_overlay_line};
use crate::ui::{
    DefaultKeybindings, Keybindings, Theme, middle_truncate, truncate_to_width, visible_width,
};

/// The maximum inner width of the session-list overlay (pi `Math.min(width, 88)`).
pub const SESSION_LIST_MAX_WIDTH: usize = 88;
/// The maximum number of "other sessions" rows shown at once (pi `maxVisible = 8`).
pub const MAX_VISIBLE: usize = 8;

/// The `name (id) [tags]` title for a session (pi `sessionTitle`, `session-list.ts:36-42`).
#[must_use]
pub fn session_title(session: &SessionInfo, is_self: bool, same_cwd: bool) -> String {
    let name = session
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed session");
    let mut tags: Vec<&str> = Vec::new();
    if is_self {
        tags.push("self");
    }
    if same_cwd {
        tags.push("same cwd");
    }
    let suffix = if tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tags.join(", "))
    };
    format!("{name} ({}){suffix}", short_session_id(&session.id))
}

/// `herdrLocationText(session)` (`v0.14.0 ui/session-list.ts:36-42`, ICOM-065) — the overlay's own
/// wording, which differs from the `list` row's ([`crate::herdr_location::format_herdr_location`]):
/// no `Herdr ` prefix on a current location, no `pane ` before its id, and `Herdr unavailable
/// (<reason>, pane <id>)`. `None` when the roster carries no location.
#[must_use]
pub fn herdr_location_text(session: &SessionInfo) -> Option<String> {
    use crate::transport::protocol::HerdrLocation;
    Some(match session.herdr_location.as_ref()? {
        HerdrLocation::NotHosted => "not under Herdr".to_string(),
        HerdrLocation::Unavailable {
            pane_id, reason, ..
        } => format!("Herdr unavailable ({}, pane {pane_id})", reason.as_str()),
        HerdrLocation::Current {
            workspace,
            tab,
            pane_id,
            ..
        } => format!(
            "{} [{}] / {} [{}] / {pane_id}",
            workspace.label, workspace.id, tab.label, tab.id
        ),
    })
}

/// What a keystroke did to the session-list overlay (pi `SessionListOverlay.handleInput`).
#[derive(Clone, Debug, PartialEq)]
pub enum SessionListAction {
    /// The selection moved / needs a redraw.
    Redraw,
    /// The keystroke was ignored.
    Ignore,
    /// The user cancelled (`tui.select.cancel`).
    Cancel,
    /// The user chose a session to MESSAGE (`tui.select.confirm`; pi `action: "message"`).
    ///
    /// Boxed: [`SessionInfo`] is by far the largest thing this enum carries, and every other
    /// variant is a unit, so an inline `SessionInfo` would make a `Redraw` cost the same as a
    /// selection. Same reasoning as [`crate::transport::client::InboundEvent::Message`].
    Select(Box<SessionInfo>),
    /// The user chose a session to HAND OVER to (`h`; pi `action: "handover"`,
    /// `v0.16.1 session-list.ts:108`).
    Handover(Box<SessionInfo>),
}

/// pi `SessionListSelection` (`v0.16.1 session-list.ts:52-55`) — what the live overlay closes
/// with.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionListSelection {
    /// `{ session, action: "message" }`.
    Message(SessionInfo),
    /// `{ session, action: "handover" }`.
    Handover(SessionInfo),
}

/// pi `matchesKey(data, "h")`: the raw byte, or its unmodified Kitty CSI-u encoding
/// (`\x1b[104u`), which the upstream test presses too (`test/handover-picker.test.ts`). A Shift-
/// or Ctrl-modified `h` is a different key and does not match.
fn is_plain_h(data: &str) -> bool {
    data == "h" || data == "\x1b[104u"
}

/// The session picker overlay (pi `SessionListOverlay`).
#[derive(Clone, Debug)]
pub struct SessionListOverlay {
    current_session: SessionInfo,
    sessions: Vec<SessionInfo>,
    selected_index: usize,
    max_visible: usize,
}

impl SessionListOverlay {
    /// A picker over `sessions` (the OTHER sessions), with `current_session` shown at the top.
    #[must_use]
    pub fn new(current_session: SessionInfo, sessions: Vec<SessionInfo>) -> Self {
        Self {
            current_session,
            sessions,
            selected_index: 0,
            max_visible: MAX_VISIBLE,
        }
    }

    /// The currently-highlighted session, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected_index)
    }

    /// Handle one raw input chunk (pi `handleInput`): cancel, up/down (wrapping), or confirm-select.
    pub fn handle_input(&mut self, keybindings: &dyn Keybindings, data: &str) -> SessionListAction {
        if keybindings.matches(data, "tui.select.cancel") {
            return SessionListAction::Cancel;
        }
        if self.sessions.is_empty() {
            return SessionListAction::Ignore;
        }
        let last = self.sessions.len() - 1;
        if keybindings.matches(data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                last
            } else {
                self.selected_index - 1
            };
            return SessionListAction::Redraw;
        }
        if keybindings.matches(data, "tui.select.down") {
            self.selected_index = if self.selected_index == last {
                0
            } else {
                self.selected_index + 1
            };
            return SessionListAction::Redraw;
        }
        let Some(session) = self.sessions.get(self.selected_index) else {
            return SessionListAction::Ignore;
        };
        if keybindings.matches(data, "tui.select.confirm") {
            return SessionListAction::Select(Box::new(session.clone()));
        }
        // `else if (matchesKey(data, "h"))` (`v0.16.1 session-list.ts:107-109`), after confirm and
        // reached only with a non-empty list, exactly as upstream orders it.
        if is_plain_h(data) {
            return SessionListAction::Handover(Box::new(session.clone()));
        }
        SessionListAction::Ignore
    }

    /// The `[start, end)` window of visible "other sessions" rows (pi `startIndex`/`endIndex`,
    /// `session-list.ts:132-136`), computed in signed space to mirror pi's clamping.
    fn window(&self) -> (usize, usize) {
        let len = self.sessions.len();
        let half = (self.max_visible / 2) as isize;
        let by_selection = self.selected_index as isize - half;
        let by_tail = len as isize - self.max_visible as isize;
        let start = by_selection.min(by_tail).max(0) as usize;
        let end = (start + self.max_visible).min(len);
        (start, end)
    }

    /// Render the overlay to `width` display columns (pi `SessionListOverlay.render`). Inner width is
    /// clamped to [`SESSION_LIST_MAX_WIDTH`]; every line is exactly `min(width, 88)` columns.
    #[must_use]
    pub fn render(
        &self,
        theme: &dyn Theme,
        keybindings: &dyn Keybindings,
        width: usize,
    ) -> Vec<String> {
        let inner_width = width.clamp(1, SESSION_LIST_MAX_WIDTH);
        if inner_width == 1 {
            return vec![theme.fg("accent", "│")];
        }
        let content_width = inner_width.saturating_sub(2);
        let path_width = std::cmp::max(8, content_width.saturating_sub(4));
        // `v0.16.1 session-list.ts:119`.
        let footer = format!(
            "{}: Message • h: Hand over • {}: Close",
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
        let path_line = |session: &SessionInfo| {
            format!(
                "{} • {}",
                middle_truncate(&session.cwd, path_width),
                session.model
            )
        };

        let mut lines: Vec<String> = Vec::new();
        lines.push(rule('╭', '╮'));
        lines.push(row(&theme.bold(" Current Session")));
        lines.push(rule('├', '┤'));
        lines.push(row(""));
        lines.push(row(&format!(
            "  {}",
            theme.fg("dim", &session_title(&self.current_session, true, false))
        )));
        lines.push(row(&format!(
            "  {}",
            theme.fg("dim", &path_line(&self.current_session))
        )));
        // `session-list.ts:132-133`: one dim line under the path, only when there is a location.
        if let Some(location) = herdr_location_text(&self.current_session) {
            lines.push(row(&format!("  {}", theme.fg("dim", &location))));
        }
        lines.push(row(""));
        lines.push(rule('├', '┤'));
        lines.push(row(&theme.bold(" Other Sessions")));
        lines.push(row(""));

        if self.sessions.is_empty() {
            lines.push(row(
                &theme.fg("dim", " No other intercom-connected sessions")
            ));
        } else {
            let (start, end) = self.window();
            for index in start..end {
                let Some(session) = self.sessions.get(index) else {
                    continue;
                };
                let is_selected = index == self.selected_index;
                let same_cwd = session.cwd == self.current_session.cwd;
                let prefix = if is_selected {
                    theme.fg("accent", "→ ")
                } else {
                    "  ".to_string()
                };
                let title = session_title(session, false, same_cwd);
                let title = if is_selected {
                    theme.fg("accent", &title)
                } else {
                    title
                };
                lines.push(row(&format!("{prefix}{title}")));
                lines.push(row(&format!("  {}", theme.fg("dim", &path_line(session)))));
                // `session-list.ts:158-159`.
                if let Some(location) = herdr_location_text(session) {
                    lines.push(row(&format!("  {}", theme.fg("dim", &location))));
                }
                if index < end - 1 {
                    lines.push(row(""));
                }
            }
            if start > 0 || end < self.sessions.len() {
                lines.push(row(""));
                lines.push(row(&theme.fg(
                    "dim",
                    &format!(" {}/{}", self.selected_index + 1, self.sessions.len()),
                )));
            }
        }

        lines.push(row(""));
        lines.push(rule('├', '┤'));
        lines.push(row(&theme.fg("dim", &format!(" {footer}"))));
        lines.push(rule('╰', '╯'));
        lines
    }
}

/// A `│ … │` overlay row: truncate `text` to `content_width`, right-pad, frame with accent borders.
pub(crate) fn box_row(theme: &dyn Theme, content_width: usize, text: &str) -> String {
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

/// The live `/intercom` session list: [`SessionListOverlay`] behind the host's overlay seam —
/// pi `ctx.ui.custom((_tui, theme, keybindings, done) => new SessionListOverlay(…), { overlay:
/// true, overlayOptions: { width: 88 } })` (`v0.16.1 index.ts:3185-3188`).
///
/// `done(selection)` is the `result` cell: `open_overlay` consumes the box and blocks until it
/// closes, so the caller reads the cell afterwards (the MCP/permission-system result-cell pattern).
pub struct SessionListOverlayHost {
    overlay: SessionListOverlay,
    result: Arc<Mutex<Option<SessionListSelection>>>,
}

impl SessionListOverlayHost {
    /// Wrap `overlay`, publishing its selection into `result`.
    #[must_use]
    pub fn new(
        overlay: SessionListOverlay,
        result: Arc<Mutex<Option<SessionListSelection>>>,
    ) -> Self {
        Self { overlay, result }
    }
}

impl InteractiveOverlay for SessionListOverlayHost {
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
        let selection = match self.overlay.handle_input(&DefaultKeybindings, &data) {
            SessionListAction::Redraw => return OverlayOutcome::Redraw,
            SessionListAction::Ignore => return OverlayOutcome::Ignored,
            SessionListAction::Cancel => None,
            SessionListAction::Select(session) => Some(SessionListSelection::Message(*session)),
            SessionListAction::Handover(session) => Some(SessionListSelection::Handover(*session)),
        };
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = selection;
        OverlayOutcome::Close
    }

    fn options(&self) -> OverlayOptions {
        OverlayOptions {
            width: Some(SESSION_LIST_MAX_WIDTH as u16),
            ..OverlayOptions::default()
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::ui::{DefaultKeybindings, PlainTheme};

    fn session(id: &str, name: &str) -> SessionInfo {
        SessionInfo {
            endpoint_epoch: None,
            id: id.to_string(),
            name: Some(name.to_string()),
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

    /// ICOM-065 — the overlay prints a location line under the path of each session that carries
    /// one, in `herdrLocationText`'s wording (`v0.14.0 ui/session-list.ts:36-42`), and nothing for a
    /// roster Herdr was never asked about.
    #[test]
    fn a_herdr_location_line_follows_the_path_line() {
        use crate::transport::protocol::{HerdrLabelRef, HerdrLocation, HerdrUnavailableReason};
        let mut me = session("session-12345678", "me");
        me.herdr_location = Some(HerdrLocation::Current {
            workspace: HerdrLabelRef {
                id: "w5".into(),
                label: "Platform".into(),
            },
            tab: HerdrLabelRef {
                id: "w5:t2".into(),
                label: "API".into(),
            },
            pane_id: "w5:p4".into(),
            refreshed_at: 1u64.into(),
        });
        let mut gone = session("session-87654321", "gone");
        gone.herdr_location = Some(HerdrLocation::Unavailable {
            pane_id: "w1:p9".into(),
            reason: HerdrUnavailableReason::PaneMissing,
            detail: None,
        });
        let mut plain = session("session-11111111", "plain");
        plain.herdr_location = Some(HerdrLocation::NotHosted);
        let overlay = SessionListOverlay::new(me, vec![gone, plain]);
        let text = overlay
            .render(&PlainTheme, &MockKeybindings, 88)
            .into_iter()
            .map(|l| l.trim_matches('│').trim_end().to_string())
            .collect::<Vec<_>>();
        for expected in [
            "  Platform [w5] / API [w5:t2] / w5:p4",
            "  Herdr unavailable (pane_missing, pane w1:p9)",
            "  not under Herdr",
        ] {
            let at = text.iter().position(|l| l == expected);
            assert!(at.is_some(), "{expected:?} missing from {text:#?}");
            assert!(
                text[at.unwrap() - 1].contains("bsy-deepseek-v4-pro"),
                "{expected:?} follows its path line: {text:#?}"
            );
        }
        let bare = SessionListOverlay::new(session("a", "a"), vec![session("b", "b")]);
        assert!(
            !bare
                .render(&PlainTheme, &MockKeybindings, 88)
                .join("\n")
                .contains("Herdr"),
            "no location, no line"
        );
    }

    // Port of test/overlay-width.test.ts:60-66.
    #[test]
    fn renders_lines_at_the_declared_overlay_width() {
        let s = session("session-12345678", "subagent-chat-019ecaf6");
        let overlay = SessionListOverlay::new(s.clone(), vec![s]);
        for width in [1usize, 2, 20, 50, 88] {
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
    fn navigation_wraps_and_selects() {
        let kb = DefaultKeybindings;
        let current = session("self-0000", "me");
        let a = session("aaaa1111", "alice");
        let b = session("bbbb2222", "bob");
        let mut overlay = SessionListOverlay::new(current, vec![a.clone(), b.clone()]);
        // Down moves to bob.
        assert_eq!(
            overlay.handle_input(&kb, "\x1b[B"),
            SessionListAction::Redraw
        );
        assert_eq!(overlay.selected().map(|s| s.id.as_str()), Some("bbbb2222"));
        // Down again wraps to alice.
        overlay.handle_input(&kb, "\x1b[B");
        assert_eq!(overlay.selected().map(|s| s.id.as_str()), Some("aaaa1111"));
        // Up wraps back to bob.
        overlay.handle_input(&kb, "\x1b[A");
        assert_eq!(overlay.selected().map(|s| s.id.as_str()), Some("bbbb2222"));
        // Enter selects the highlighted session.
        assert_eq!(
            overlay.handle_input(&kb, "\r"),
            SessionListAction::Select(Box::new(b))
        );
        // Esc cancels.
        assert_eq!(overlay.handle_input(&kb, "\x1b"), SessionListAction::Cancel);
    }

    #[test]
    fn empty_other_sessions_renders_a_placeholder() {
        let current = session("self-0000", "me");
        let overlay = SessionListOverlay::new(current, Vec::new());
        let rendered = overlay.render(&PlainTheme, &MockKeybindings, 60).join("\n");
        assert!(rendered.contains("No other intercom-connected sessions"));
    }

    #[test]
    fn session_title_tags_self_and_same_cwd() {
        let s = session("abcdef1234", "worker");
        assert_eq!(session_title(&s, true, false), "worker (abcdef12) [self]");
        assert_eq!(
            session_title(&s, false, true),
            "worker (abcdef12) [same cwd]"
        );
        assert_eq!(session_title(&s, false, false), "worker (abcdef12)");
    }

    /// Port of `test/handover-picker.test.ts` "session list starts a handover on plain or
    /// Kitty-encoded h and messages on Enter" (`v0.16.1`).
    #[test]
    fn session_list_starts_a_handover_on_plain_or_kitty_encoded_h_and_messages_on_enter() {
        let kb = DefaultKeybindings;
        let peer = session("adapter-00", "adapter");
        for (key, expected) in [
            ("h", SessionListAction::Handover(Box::new(peer.clone()))),
            (
                "\x1b[104u",
                SessionListAction::Handover(Box::new(peer.clone())),
            ),
            ("\r", SessionListAction::Select(Box::new(peer.clone()))),
        ] {
            let mut overlay =
                SessionListOverlay::new(session("self-0000", "me"), vec![peer.clone()]);
            assert_eq!(overlay.handle_input(&kb, key), expected, "{key:?}");
        }
        // Shift+h is a different key.
        let mut overlay = SessionListOverlay::new(session("self-0000", "me"), vec![peer]);
        assert_eq!(overlay.handle_input(&kb, "H"), SessionListAction::Ignore);
    }

    /// `if (this.sessions.length === 0) return;` comes before the `h` arm (`session-list.ts:91-93`).
    #[test]
    fn h_on_an_empty_list_is_ignored_and_escape_still_closes() {
        let kb = DefaultKeybindings;
        let mut overlay = SessionListOverlay::new(session("self-0000", "me"), Vec::new());
        assert_eq!(overlay.handle_input(&kb, "h"), SessionListAction::Ignore);
        assert_eq!(overlay.handle_input(&kb, "\r"), SessionListAction::Ignore);
        assert_eq!(overlay.handle_input(&kb, "\x1b"), SessionListAction::Cancel);
    }

    /// The footer names the `h` key (`v0.16.1 session-list.ts:119`).
    #[test]
    fn the_footer_names_message_hand_over_and_close() {
        let overlay = SessionListOverlay::new(session("self-0000", "me"), Vec::new());
        let text = overlay
            .render(&PlainTheme, &DefaultKeybindings, 88)
            .join("\n");
        assert!(
            text.contains(" enter: Message • h: Hand over • escape/ctrl+c: Close"),
            "{text}"
        );
    }

    /// The live adapter: keys arrive as `OverlayKey`s, the selection lands in the shared cell, and
    /// every painted row is the declared width.
    #[test]
    fn the_overlay_adapter_publishes_message_and_handover_selections() {
        use cyrup_ext::OverlayKeyCode;
        let peer = session("adapter-00", "adapter");
        for (code, expected) in [
            (
                OverlayKeyCode::Char('h'),
                Some(SessionListSelection::Handover(peer.clone())),
            ),
            (
                OverlayKeyCode::Enter,
                Some(SessionListSelection::Message(peer.clone())),
            ),
            (OverlayKeyCode::Escape, None),
        ] {
            let cell = Arc::new(Mutex::new(Some(SessionListSelection::Message(session(
                "stale", "stale",
            )))));
            let mut host = SessionListOverlayHost::new(
                SessionListOverlay::new(session("self-0000", "me"), vec![peer.clone()]),
                cell.clone(),
            );
            assert_eq!(host.options().width, Some(88));
            for line in host.render(88, 40) {
                assert_eq!(line.plain_text().chars().count(), 88);
            }
            assert_eq!(
                host.handle_key(OverlayKey::plain(OverlayKeyCode::Down)),
                OverlayOutcome::Redraw
            );
            assert_eq!(
                host.handle_key(OverlayKey::plain(code)),
                OverlayOutcome::Close
            );
            assert_eq!(*cell.lock().unwrap(), expected, "{code:?}");
        }
    }
}
