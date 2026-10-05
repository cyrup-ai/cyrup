//! A pre-launch selector paints in the theme the terminal's colours generate — pi's
//! `createStartupTui` / `startStartupTui` / `queryStartupTerminalColors` (`cli/startup-ui.ts:77-127`
//! @v1.0.0) — and a first-run preview recolours it (`:204-209`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cyrup_resources::color::Rgb;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::style::Color;
use tokio::sync::mpsc::unbounded_channel;

use super::*;
use crate::ListSelector;
use crate::error::TuiError;
use crate::keymap::SelectKeymap;
use crate::startup_loop::{Retheme, StartupEvents, StartupLoop};

fn mocha() -> TerminalColors {
    let mut palette = [Rgb::new(0x45, 0x47, 0x5a); 16];
    for (i, slot) in palette.iter_mut().enumerate() {
        let i = u8::try_from(i).unwrap();
        *slot = Rgb::new(0x40 + i * 8, 0x90 - i * 4, 0xa0 + i * 3);
    }
    TerminalColors {
        foreground: Some(Rgb::new(0xcd, 0xd6, 0xf4)),
        background: Some(Rgb::new(0x1e, 0x1e, 0x2e)),
        palette: Some(palette),
    }
}

/// A key source that answers from a script and records how long each turn was willing to wait.
struct Script {
    keys: VecDeque<Event>,
    waits: Vec<Duration>,
}

impl Script {
    fn new(keys: Vec<Event>) -> Self {
        Self {
            keys: keys.into(),
            waits: Vec::new(),
        }
    }
}

impl StartupEvents for Script {
    fn next(&mut self, wait: Duration) -> Result<Option<Event>, TuiError> {
        self.waits.push(wait);
        Ok(self.keys.pop_front())
    }
}

/// `StartupTheme::resolve` over a fixed colour depth: the environment this runs in would otherwise
/// pick the projection.
fn fresh(setting: Option<&str>) -> StartupTheme {
    StartupTheme::from_controller(crate::ThemeController::boot(
        setting,
        crate::ColorMode::TrueColor,
        crate::TerminalTheme::Dark,
    ))
}

fn selector() -> ListSelector {
    ListSelector::prompt(
        "Pick".to_string(),
        vec![
            ("system".to_string(), "System".to_string(), None),
            ("dark".to_string(), "Dark".to_string(), None),
            ("light".to_string(), "Light".to_string(), None),
        ],
        0,
    )
}

fn painted(terminal: &Terminal<TestBackend>, color: Color) -> bool {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|cell| cell.fg == color)
}

// ---------------------------------------------------------------------------------------------

/// `markTerminalColorsPending(); initTheme(…)` then, when the colours land, `setTerminalColors` and
/// `setTheme(resolveThemeSetting(…))`: grayscale (here: terminal defaults) first, then the palette.
#[test]
fn the_startup_theme_is_pending_until_the_terminal_reports_and_then_generated() {
    let mut theme = fresh(None);
    assert!(theme.colors_pending());
    assert_eq!(theme.theme().name, "system");
    assert_eq!(
        theme.theme().accent,
        Some(Color::Reset),
        "grayscale until the colours are known"
    );

    assert!(theme.apply_colors(mocha()));
    assert!(!theme.colors_pending());
    assert!(
        matches!(theme.theme().accent, Some(Color::Rgb(..))),
        "generated from the reported palette: {:?}",
        theme.theme().accent
    );
}

/// A setting that names a fixed theme is painted as itself from the first frame.
#[test]
fn a_fixed_setting_is_the_theme_before_and_after_the_colours() {
    let mut theme = fresh(Some("light"));
    assert_eq!(theme.theme().name, "light");
    theme.apply_colors(mocha());
    assert_eq!(theme.theme().name, "light");
}

/// The wizard's `previewTheme` outlives a colour answer (`startup-ui.ts:215-217` re-applies
/// `previewTheme`, not the setting's theme).
#[test]
fn a_preview_survives_the_colour_answer() {
    let mut theme = fresh(None);
    theme.preview("light");
    assert_eq!(theme.theme().name, "light");
    theme.apply_colors(mocha());
    assert_eq!(theme.theme().name, "light");
}

/// The loop is the observable one: the first frame is painted before the terminal has answered, and
/// the frame after the answer carries the generated colours in rendered cells.
///
/// FAILS without the change: the selector painted the theme it was handed and never took another.
#[tokio::test]
async fn the_loop_repaints_in_the_generated_theme_when_the_colours_arrive() {
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let mut events = Script::new(vec![]);
    let keymap = SelectKeymap::default();
    let mut inner = selector();
    let (tx, rx) = unbounded_channel();
    let start = fresh(None);
    let mut lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: start.theme(),
        retheme: Some(Retheme::new(start, rx, Instant::now())),
        keymap: &keymap,
        inner: &mut inner,
        loads: None,
    };
    let mut noop = async |_: &str| {};

    assert!(lp.step(&mut noop).await.unwrap().is_none());
    let before = lp.theme.accent;
    assert_eq!(before, Some(Color::Reset));

    tx.send(mocha()).unwrap();
    assert!(lp.step(&mut noop).await.unwrap().is_none());
    let Some(Color::Rgb(r, g, b)) = lp.theme.accent else {
        panic!("generated accent expected, got {:?}", lp.theme.accent);
    };
    drop(lp);
    assert!(
        painted(&terminal, Color::Rgb(r, g, b)),
        "the generated accent never reached a rendered cell"
    );
}

/// A terminal that answers nothing is not waited on past the query's own timeout, and the grayscale
/// ends with nothing reported (`requestTerminalColors`'s `.then(apply, () => apply({}))`); a reply
/// that comes after the timeout still applies.
#[tokio::test]
async fn a_silent_terminal_ends_the_grayscale_at_the_timeout_and_a_late_reply_still_applies() {
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let mut events = Script::new(vec![]);
    let keymap = SelectKeymap::default();
    let mut inner = selector();
    let (tx, rx) = unbounded_channel();
    let start = fresh(None);
    let long_ago = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .expect("the clock is past five seconds");
    let mut lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: start.theme(),
        retheme: Some(Retheme::new(start, rx, long_ago)),
        keymap: &keymap,
        inner: &mut inner,
        loads: None,
    };
    let mut noop = async |_: &str| {};

    assert!(lp.step(&mut noop).await.unwrap().is_none());
    assert_eq!(
        lp.theme.accent,
        Some(Color::Indexed(5)),
        "nothing reported: the ANSI-index tier, violet is slot 5"
    );

    tx.send(mocha()).unwrap();
    assert!(lp.step(&mut noop).await.unwrap().is_none());
    assert!(matches!(lp.theme.accent, Some(Color::Rgb(..))));
}

/// While the answer is awaited the key wait is cut short, so the repaint is not held for the
/// 3600 s idle poll.
#[tokio::test]
async fn the_key_wait_does_not_outlast_the_colour_query() {
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let mut events = Script::new(vec![]);
    let keymap = SelectKeymap::default();
    let mut inner = selector();
    let (_tx, rx) = unbounded_channel();
    let start = fresh(None);
    let mut lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: start.theme(),
        retheme: Some(Retheme::new(start, rx, Instant::now())),
        keymap: &keymap,
        inner: &mut inner,
        loads: None,
    };
    let mut noop = async |_: &str| {};
    lp.step(&mut noop).await.unwrap();
    drop(lp);
    let wait = events.waits[0];
    assert!(
        wait <= crate::terminal_query::COLOR_QUERY_TIMEOUT,
        "first turn waited {wait:?}"
    );
}

/// The first-run wizard: moving the highlight previews the theme under it
/// (`onThemePreview` → `setTheme`, `startup-ui.ts:204-209`), and the preview is what the colour
/// answer re-applies.
#[tokio::test]
async fn a_preview_recolours_the_dialog_and_survives_the_colour_answer() {
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let mut events = Script::new(vec![Event::Key(KeyEvent::from(KeyCode::Down))]);
    let keymap = SelectKeymap::default();
    let mut inner = selector().with_preview();
    let (tx, rx) = unbounded_channel();
    let start = fresh(None);
    let mut lp = StartupLoop {
        terminal: &mut terminal,
        events: &mut events,
        theme: start.theme(),
        retheme: Some(Retheme::new(start, rx, Instant::now())),
        keymap: &keymap,
        inner: &mut inner,
        loads: None,
    };
    let mut noop = async |_: &str| {};
    assert_eq!(lp.theme.name, "system");

    // Down moves the highlight onto Dark.
    lp.step(&mut noop).await.unwrap();
    assert_eq!(lp.theme.name, "dark", "the highlighted theme is painted");

    tx.send(mocha()).unwrap();
    lp.step(&mut noop).await.unwrap();
    assert_eq!(lp.theme.name, "dark", "the answer re-applies the preview");
}
