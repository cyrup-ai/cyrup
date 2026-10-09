//! The bridge between this crate's string-based UI ports and the host's live overlay seam
//! ([`cyrup_ext::InteractiveOverlay`], `HostServices::open_overlay`) — the cyrup side of pi's
//! `ctx.ui.custom(factory, { overlay: true, overlayOptions })`.
//!
//! pi's components take raw terminal data in `handleInput(data: string)` and return ANSI-styled
//! `string[]` from `render(width)`. The ports in this module tree keep exactly that shape (a
//! [`super::Keybindings`] over raw data, a [`super::Theme`] that styles strings), because their
//! state machines and their width math are what the upstream tests pin. The host speaks a
//! different, backend-free vocabulary: an [`OverlayKey`] in, [`OverlayLine`]s out. Three small
//! pieces translate:
//!
//! * [`key_to_data`] turns a routed [`OverlayKey`] back into the raw bytes a terminal would have
//!   sent, so `handle_input` sees what pi's `handleInput` sees;
//! * [`OverlayTheme`] styles runs with ZERO-WIDTH role markers — APC sequences (`ESC _ … BEL`),
//!   which [`super::visible_width`], [`super::truncate_to_width`] and [`super::wrap_text`] already
//!   skip exactly as pi's `visibleWidth` skips them — so every width the ports compute stays exact;
//! * [`to_overlay_line`] reads those markers back into [`OverlaySpan`]s carrying
//!   [`OverlayColor::Theme`] roles, which the host resolves against the user's live theme at paint
//!   time (pi `theme.fg(role, …)`).

use cyrup_ext::{OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine, OverlaySpan, ThemeRole};

use super::{Theme, extract_ansi_code_len};

/// The APC introducer every marker starts with. `ci` namespaces the payload so a stray APC from
/// elsewhere (pi's `CURSOR_MARKER` is `ESC _ pi:c BEL`) is never mistaken for one of ours.
const MARKER_PREFIX: &str = "\x1b_ci:";
/// The marker terminator (BEL), which [`extract_ansi_code_len`] accepts for an APC.
const MARKER_END: char = '\x07';
/// Closes the innermost open marker.
const CLOSE: &str = "\x1b_ci:/\x07";
/// Closes every open marker — what pi's `truncateToWidth(…, pad = true)` gets from the `\x1b[0m` it
/// appends, needed here because a truncated row can lose the close markers after the cut.
const RESET: &str = "\x1b_ci:0\x07";

fn open(tag: &str) -> String {
    format!("{MARKER_PREFIX}{tag}{MARKER_END}")
}

/// A key as the raw terminal data pi's components match against (`\r`, `\x1b`, `\x1b[A`, …).
///
/// `None` for a key no ported component can act on (an Alt chord, a function key), which the host
/// adapter treats as "ignored". Ctrl+letter becomes its C0 control byte, so `Ctrl+C` is `\x03` —
/// the second `tui.select.cancel` binding pi ships.
#[must_use]
pub fn key_to_data(key: OverlayKey) -> Option<String> {
    if key.alt {
        return None;
    }
    let data = match key.code {
        OverlayKeyCode::Char(c) if key.ctrl => {
            let lower = c.to_ascii_lowercase();
            if !lower.is_ascii_lowercase() {
                return None;
            }
            // `a` = 0x01 … `z` = 0x1a.
            char::from((lower as u8) - b'a' + 1).to_string()
        }
        OverlayKeyCode::Char(c) => c.to_string(),
        OverlayKeyCode::Enter => "\r".to_string(),
        OverlayKeyCode::Escape => "\x1b".to_string(),
        OverlayKeyCode::Backspace => "\x7f".to_string(),
        OverlayKeyCode::Delete => "\x1b[3~".to_string(),
        OverlayKeyCode::Tab => "\t".to_string(),
        OverlayKeyCode::BackTab => "\x1b[Z".to_string(),
        OverlayKeyCode::Up => "\x1b[A".to_string(),
        OverlayKeyCode::Down => "\x1b[B".to_string(),
        OverlayKeyCode::Right => "\x1b[C".to_string(),
        OverlayKeyCode::Left => "\x1b[D".to_string(),
        OverlayKeyCode::Home => "\x1b[H".to_string(),
        OverlayKeyCode::End => "\x1b[F".to_string(),
        OverlayKeyCode::PageUp => "\x1b[5~".to_string(),
        OverlayKeyCode::PageDown => "\x1b[6~".to_string(),
        OverlayKeyCode::Insert | OverlayKeyCode::F(_) => return None,
    };
    Some(data)
}

/// The live theme: every run is wrapped in a zero-width role marker that [`to_overlay_line`] turns
/// into an [`OverlaySpan`] the host colours from the user's theme.
#[derive(Clone, Copy, Debug, Default)]
pub struct OverlayTheme;

impl Theme for OverlayTheme {
    fn fg(&self, color: &str, text: &str) -> String {
        format!("{}{text}{CLOSE}", open(color))
    }
    fn bold(&self, text: &str) -> String {
        format!("{}{text}{CLOSE}", open("bold"))
    }
    fn inverse(&self, text: &str) -> String {
        format!("{}{text}{CLOSE}", open("inverse"))
    }
    fn reset(&self) -> String {
        RESET.to_string()
    }
}

/// pi's theme slot name → the host's closed role set. A slot this crate never emits falls back to
/// the default foreground rather than inventing a colour.
fn role(tag: &str) -> Option<ThemeRole> {
    Some(match tag {
        "accent" => ThemeRole::Accent,
        "dim" => ThemeRole::Dim,
        "muted" => ThemeRole::Muted,
        "warning" => ThemeRole::Warning,
        "error" => ThemeRole::Error,
        "success" => ThemeRole::Success,
        "border" => ThemeRole::Border,
        "borderMuted" => ThemeRole::BorderMuted,
        "text" => ThemeRole::Text,
        _ => return None,
    })
}

/// The style a stack of open markers resolves to: the innermost colour wins, bold and inverse are
/// additive (pi nests `theme.fg` inside `theme.bold` and the terminal composes them the same way).
fn style_of(stack: &[String]) -> (Option<OverlayColor>, bool, bool) {
    let fg = stack
        .iter()
        .rev()
        .find_map(|tag| role(tag))
        .map(OverlayColor::Theme);
    let bold = stack.iter().any(|tag| tag == "bold");
    let reversed = stack.iter().any(|tag| tag == "inverse");
    (fg, bold, reversed)
}

/// One line rendered under [`OverlayTheme`] as the host's styled runs.
///
/// Markers are applied as a stack; an unclosed marker at the end of the line simply ends with it
/// (pi's rows end in a reset too). Any other escape sequence is dropped: the host owns styling,
/// and a raw SGR code painted as text would be visible garbage.
#[must_use]
pub fn to_overlay_line(line: &str) -> OverlayLine {
    let chars: Vec<char> = line.chars().collect();
    let mut stack: Vec<String> = Vec::new();
    let mut spans: Vec<OverlaySpan> = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, stack: &[String], spans: &mut Vec<OverlaySpan>| {
        if text.is_empty() {
            return;
        }
        let (fg, bold, reversed) = style_of(stack);
        spans.push(OverlaySpan {
            text: std::mem::take(text),
            fg,
            bold,
            reversed,
            ..OverlaySpan::default()
        });
    };
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        if let Some(len) = extract_ansi_code_len(&chars, i) {
            let code: String = chars
                .get(i..i + len)
                .map_or_else(String::new, |s| s.iter().collect());
            if let Some(tag) = code
                .strip_prefix(MARKER_PREFIX)
                .and_then(|rest| rest.strip_suffix(MARKER_END))
            {
                flush(&mut text, &stack, &mut spans);
                match tag {
                    "/" => {
                        stack.pop();
                    }
                    "0" => stack.clear(),
                    other => stack.push(other.to_string()),
                }
            }
            i += len;
            continue;
        }
        text.push(c);
        i += 1;
    }
    flush(&mut text, &stack, &mut spans);
    OverlayLine::new(spans)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::ui::{truncate_to_width, visible_width};

    #[test]
    fn markers_are_zero_width_and_round_trip_into_theme_spans() {
        let theme = OverlayTheme;
        let line = format!(
            "{}{} plain {}{}",
            theme.fg("accent", "│"),
            theme.bold(&theme.fg("dim", "title")),
            theme.fg("warning", "93% ctx"),
            theme.reset()
        );
        assert_eq!(visible_width(&line), 20);
        let parsed = to_overlay_line(&line);
        assert_eq!(parsed.plain_text(), "│title plain 93% ctx");
        let spans: Vec<(&str, Option<OverlayColor>, bool)> = parsed
            .spans
            .iter()
            .map(|s| (s.text.as_str(), s.fg, s.bold))
            .collect();
        assert_eq!(
            spans,
            vec![
                ("│", Some(OverlayColor::Theme(ThemeRole::Accent)), false),
                ("title", Some(OverlayColor::Theme(ThemeRole::Dim)), true),
                (" plain ", None, false),
                (
                    "93% ctx",
                    Some(OverlayColor::Theme(ThemeRole::Warning)),
                    false
                ),
            ]
        );
    }

    #[test]
    fn a_truncated_row_does_not_leak_its_open_style_past_the_reset() {
        let theme = OverlayTheme;
        let clipped = truncate_to_width(&theme.fg("dim", "abcdefgh"), 3);
        let line = format!("{clipped}{}{}", theme.reset(), theme.fg("accent", "│"));
        let parsed = to_overlay_line(&line);
        assert_eq!(parsed.plain_text(), "abc│");
        assert_eq!(
            parsed.spans[0].fg,
            Some(OverlayColor::Theme(ThemeRole::Dim))
        );
        assert_eq!(
            parsed.spans[1].fg,
            Some(OverlayColor::Theme(ThemeRole::Accent))
        );
    }

    #[test]
    fn inverse_marks_the_fake_cursor() {
        let parsed = to_overlay_line(&format!("> ab{}", OverlayTheme.inverse(" ")));
        assert_eq!(parsed.plain_text(), "> ab ");
        assert!(parsed.spans.last().unwrap().reversed);
    }

    #[test]
    fn keys_become_the_raw_data_pi_components_match() {
        let plain = |code| key_to_data(OverlayKey::plain(code));
        assert_eq!(plain(OverlayKeyCode::Escape).as_deref(), Some("\x1b"));
        assert_eq!(plain(OverlayKeyCode::Enter).as_deref(), Some("\r"));
        assert_eq!(plain(OverlayKeyCode::Up).as_deref(), Some("\x1b[A"));
        assert_eq!(plain(OverlayKeyCode::Down).as_deref(), Some("\x1b[B"));
        assert_eq!(plain(OverlayKeyCode::Tab).as_deref(), Some("\t"));
        assert_eq!(plain(OverlayKeyCode::Backspace).as_deref(), Some("\x7f"));
        assert_eq!(plain(OverlayKeyCode::Char('h')).as_deref(), Some("h"));
        assert_eq!(
            key_to_data(OverlayKey::ctrl(OverlayKeyCode::Char('c'))).as_deref(),
            Some("\x03")
        );
        assert_eq!(plain(OverlayKeyCode::F(1)), None);
        let alt_m = OverlayKey {
            code: OverlayKeyCode::Char('m'),
            ctrl: false,
            alt: true,
            shift: false,
        };
        assert_eq!(key_to_data(alt_m), None);
    }
}
