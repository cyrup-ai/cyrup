//! The floating overlay layer (spec/tui/05 §2; arch-10 §3.5 `OverlayManager`).
//!
//! Unlike the editor-swap selectors (which replace the input slot in place, spec/tui/05 §1.1), an
//! overlay is a true z-ordered floating modal drawn **on top of** the live region at an anchor/size,
//! capturing focus until it dismisses (`tui.ts:showOverlay`). cyrup keeps this layer for the
//! first-party **hotkeys/help popup** ([`HotkeysOverlay`]) and, later, the extension-custom-UI
//! sub-protocol (the only genuinely-gated consumer, residual ledger).
//!
//! Rendering follows arch-10 §6.4: stack bottom→top, each overlay computes its `Rect` from the full
//! frame, erases the cells under it (`ratatui::widgets::Clear`), then draws into the box. Focus
//! routing (spec/tui/05 §2) delivers a key to the **topmost** overlay first; an unconsumed key
//! bubbles. Everything is pure ratatui layout over existing state — no new dependency.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Margin, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::UiTheme;

/// The result of routing one key to an overlay (spec/tui/05 §2 step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayOutcome {
    /// The overlay handled the key and stays open (scroll, etc.) → redraw.
    Redraw,
    /// The overlay requested dismissal (`Esc`/`q`/`Enter`).
    Close,
    /// The key was not an overlay binding — the chrome may let it bubble.
    Ignored,
}

/// A floating, focus-capturing modal drawn over the live region. Object-safe so the chrome holds a
/// z-stack of `Box<dyn Overlay>`.
pub trait Overlay: Send {
    /// Render into the full-frame `area` (the overlay computes its own centered/anchored sub-`Rect`,
    /// clears it, and draws the box).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme);
    /// Route one key, returning the outcome.
    fn handle(&mut self, key: &KeyEvent) -> OverlayOutcome;
}

/// One row of the hotkeys/help popup: a bold section header or a `keys — description` entry
/// (`keybinding-hints.ts` formatting, rebind-aware — the app builds these from the live keymap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyRow {
    /// A bold section title (`**Navigation**`, `**Editing**`, …).
    Section(String),
    /// A binding row: the key label(s) + the action description.
    Entry { keys: String, desc: String },
}

/// The first-party keyboard-shortcuts popup (`/hotkeys` → `handleHotkeysCommand`,
/// interactive-mode.ts:5396-5470), shown as a floating, dismissable overlay rather than a scrollback
/// block. Scrolls when the content is taller than the box.
pub struct HotkeysOverlay {
    title: String,
    rows: Vec<HotkeyRow>,
    scroll: u16,
}

impl HotkeysOverlay {
    /// Build the popup from its title + rows (rows derived from the live keymap by the caller).
    pub fn new(title: impl Into<String>, rows: Vec<HotkeyRow>) -> Self {
        HotkeysOverlay { title: title.into(), rows, scroll: 0 }
    }

    /// The styled body lines (key column padded so descriptions align).
    fn body_lines(&self, theme: &UiTheme) -> Vec<Line<'static>> {
        let key_w = self
            .rows
            .iter()
            .filter_map(|r| match r {
                HotkeyRow::Entry { keys, .. } => Some(keys.chars().count()),
                HotkeyRow::Section(_) => None,
            })
            .max()
            .unwrap_or(0);
        let mut out: Vec<Line<'static>> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            match row {
                HotkeyRow::Section(title) => {
                    if !out.is_empty() {
                        out.push(Line::default());
                    }
                    out.push(Line::styled(
                        title.clone(),
                        theme.accent_style().add_modifier(Modifier::BOLD),
                    ));
                }
                HotkeyRow::Entry { keys, desc } => {
                    let pad = key_w.saturating_sub(keys.chars().count());
                    out.push(Line::from(vec![
                        Span::styled(format!("{keys}{}  ", " ".repeat(pad)), theme.accent_style()),
                        Span::styled(desc.clone(), theme.muted_style()),
                    ]));
                }
            }
        }
        out
    }

    /// Total body-line count (for scroll clamping / tests).
    #[cfg(test)]
    fn content_height(&self, theme: &UiTheme) -> usize {
        self.body_lines(theme).len()
    }
}

/// Compute a centered overlay `Rect` (spec/tui/05 §2 `Anchor::Center`): clamp the desired
/// `content` width/height to the frame minus a 1-cell margin, then center.
pub fn centered_rect(area: Rect, want_w: u16, want_h: u16) -> Rect {
    let max_w = area.width.saturating_sub(2).max(1);
    let max_h = area.height.saturating_sub(2).max(1);
    let w = want_w.min(max_w).max(1);
    let h = want_h.min(max_h).max(1);
    let x = area.x.saturating_add((area.width.saturating_sub(w)) / 2);
    let y = area.y.saturating_add((area.height.saturating_sub(h)) / 2);
    Rect { x, y, width: w, height: h }
}

impl Overlay for HotkeysOverlay {
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let body = self.body_lines(theme);
        let content_w = body.iter().map(Line::width).max().unwrap_or(20) as u16;
        // Desired box: content + borders (2) + 1-cell horizontal padding each side.
        let want_w = content_w.saturating_add(4).min(80);
        let want_h = (body.len() as u16).saturating_add(2);
        let rect = centered_rect(area, want_w, want_h);
        // Clamp scroll so the tail can't scroll past the content.
        let inner_h = rect.height.saturating_sub(2) as usize;
        let max_scroll = body.len().saturating_sub(inner_h).min(u16::MAX as usize) as u16;
        self.scroll = self.scroll.min(max_scroll);

        frame.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border_style())
            .title(Span::styled(
                format!(" {} ", self.title),
                theme.accent_style().add_modifier(Modifier::BOLD),
            ));
        let para = Paragraph::new(body)
            .style(theme.base_style())
            .block(block)
            .scroll((self.scroll, 0));
        frame.render_widget(para, rect);
        // A dismiss hint pinned to the bottom border, if there is room.
        if rect.height >= 3 {
            let hint = " ↑/↓ scroll · Esc close ";
            let hint_w = hint.chars().count() as u16;
            if hint_w < rect.width {
                let hint_area = Rect {
                    x: rect.x + rect.width.saturating_sub(hint_w + 1),
                    y: rect.y + rect.height - 1,
                    width: hint_w,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::styled(hint.to_string(), theme.dim_style())),
                    hint_area.inner(Margin::new(0, 0)),
                );
            }
        }
    }

    fn handle(&mut self, key: &KeyEvent) -> OverlayOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => OverlayOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                OverlayOutcome::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                OverlayOutcome::Redraw
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                OverlayOutcome::Redraw
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                OverlayOutcome::Redraw
            }
            _ => OverlayOutcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn rows() -> Vec<HotkeyRow> {
        vec![
            HotkeyRow::Section("Navigation".to_string()),
            HotkeyRow::Entry { keys: "Up/Down".to_string(), desc: "Move cursor".to_string() },
            HotkeyRow::Entry { keys: "Ctrl+A".to_string(), desc: "Start of line".to_string() },
        ]
    }

    #[test]
    fn esc_and_enter_close() {
        let mut o = HotkeysOverlay::new("Keyboard Shortcuts", rows());
        assert_eq!(o.handle(&KeyEvent::from(KeyCode::Esc)), OverlayOutcome::Close);
        assert_eq!(o.handle(&KeyEvent::from(KeyCode::Enter)), OverlayOutcome::Close);
        assert_eq!(o.handle(&KeyEvent::from(KeyCode::Char('q'))), OverlayOutcome::Close);
    }

    #[test]
    fn arrows_scroll_and_clamp() {
        let mut o = HotkeysOverlay::new("Keyboard Shortcuts", rows());
        assert_eq!(o.handle(&KeyEvent::from(KeyCode::Up)), OverlayOutcome::Redraw);
        assert_eq!(o.scroll, 0); // saturating at top
        assert_eq!(o.handle(&KeyEvent::from(KeyCode::Down)), OverlayOutcome::Redraw);
        assert_eq!(o.scroll, 1);
    }

    #[test]
    fn body_has_a_section_and_aligned_entries() {
        let theme = UiTheme::dark();
        let o = HotkeysOverlay::new("Keyboard Shortcuts", rows());
        assert_eq!(o.content_height(&theme), 3); // section + 2 entries (no leading blank)
    }
}
