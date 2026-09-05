//! [`SessionListOverlay`] — a port of `pi-intercom/ui/session-list.ts` `SessionListOverlay` (the
//! width-88, max-8-visible session picker the `/intercom` overlay opens).
//!
//! WIRING: [`SessionListOverlay::render`] draws the session picker the `/intercom` slash command
//! returns as its text output ([`crate::extension::IntercomExtension`]'s `execute_command`). The
//! interactive selection state machine ([`SessionListOverlay::handle_input`]) is the faithful port of
//! the live overlay; a live keystroke source is the Phase-6 `register_shortcut`/overlay-host gap (the
//! port doc §4.3/§5 Phase 6), so it is unit-tested here and wired to real input only once that host
//! hook lands.

use crate::identity::short_session_id;
use crate::transport::protocol::SessionInfo;
use crate::ui::{Keybindings, Theme, middle_truncate, truncate_to_width, visible_width};

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

/// What a keystroke did to the session-list overlay (pi `SessionListOverlay.handleInput`).
#[derive(Clone, Debug, PartialEq)]
pub enum SessionListAction {
    /// The selection moved / needs a redraw.
    Redraw,
    /// The keystroke was ignored.
    Ignore,
    /// The user cancelled (`tui.select.cancel`).
    Cancel,
    /// The user chose a session (`tui.select.confirm`).
    ///
    /// Boxed: [`SessionInfo`] is by far the largest thing this enum carries, and every other
    /// variant is a unit, so an inline `SessionInfo` would make a `Redraw` cost the same as a
    /// selection. Same reasoning as [`crate::transport::client::InboundEvent::Message`].
    Select(Box<SessionInfo>),
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
        if keybindings.matches(data, "tui.select.confirm")
            && let Some(session) = self.sessions.get(self.selected_index)
        {
            return SessionListAction::Select(Box::new(session.clone()));
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
        let footer = format!(
            "{}: Message • {}: Close",
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
fn box_row(theme: &dyn Theme, content_width: usize, text: &str) -> String {
    let clipped = truncate_to_width(text, content_width);
    let pad = content_width.saturating_sub(visible_width(&clipped));
    format!(
        "{}{clipped}{}{}",
        theme.fg("accent", "│"),
        " ".repeat(pad),
        theme.fg("accent", "│")
    )
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
}
