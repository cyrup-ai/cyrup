//! The `/settings` theme submenu against pi's `ThemeSubmenu` (`settings-selector.ts:208-413` @v1.0.0).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::*;

fn available() -> Vec<String> {
    ["system", "dark", "light", "solarized"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn press(selector: &mut ThemeSelector, code: KeyCode) -> SelectorOutcome {
    selector.handle(&KeyEvent::from(code), &SelectKeymap::default())
}

/// The rendered slot, one string per row.
fn screen(selector: &mut ThemeSelector) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            let height = selector.desired_height(area.width).min(area.height);
            selector.render(frame, Rect { height, ..area }, &UiTheme::dark());
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    buffer
        .content()
        .chunks(usize::from(buffer.area.width))
        .map(|row| {
            row.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixed(setting: &str, terminal: TerminalTheme) -> ThemeSelector {
    ThemeSelector::new(setting, terminal, available())
}

/// The single menu: the system theme first with its description, then `automatic`, then the others
/// in the order given; the current theme carries `✓`.
///
/// FAILS without the change: the picker was a flat list of the two built-ins with no `automatic` row.
#[test]
fn the_single_menu_lists_system_then_automatic_then_the_rest() {
    let mut selector = fixed("dark", TerminalTheme::Dark);
    let text = screen(&mut selector);
    // The row each name is on: its line starts with the name once the cursor and the mark are gone.
    let row_of = |name: &str| {
        text.lines()
            .position(|line| {
                let label = line.trim_start_matches(['→', ' ']).trim_start_matches("✓ ");
                label.split_whitespace().next() == Some(name)
            })
            .unwrap_or_else(|| panic!("{name} missing:\n{text}"))
    };
    let order: Vec<usize> = ["system", "automatic", "dark", "light", "solarized"]
        .iter()
        .map(|name| row_of(name))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "order:\n{text}");
    assert!(
        text.contains("✓ dark"),
        "the current theme is marked:\n{text}"
    );
    assert!(!text.contains("✓ light"));
    assert!(text.contains("Theme"), "{text}");
    assert!(
        text.contains("Select a theme, or choose automatic to follow terminal appearance."),
        "{text}"
    );
    assert!(
        text.contains("Use separate themes for light and dark terminal appearance"),
        "{text}"
    );
}

/// The highlight starts on the current theme and a move previews the theme under it
/// (`onSelectionChange`).
#[test]
fn moving_previews_the_theme_under_the_highlight() {
    let mut selector = fixed("system", TerminalTheme::Dark);
    // system -> automatic: the pair preview, `system/system`.
    assert_eq!(
        press(&mut selector, KeyCode::Down),
        SelectorOutcome::Preview("system/system".to_string())
    );
    assert_eq!(
        press(&mut selector, KeyCode::Down),
        SelectorOutcome::Preview("dark".to_string())
    );
}

/// Picking a theme confirms its NAME; `Esc` cancels (the caller restores the opening theme).
#[test]
fn picking_a_theme_confirms_its_name_and_escape_cancels() {
    let mut selector = fixed("system", TerminalTheme::Dark);
    press(&mut selector, KeyCode::Down);
    press(&mut selector, KeyCode::Down);
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Confirm("dark".to_string())
    );
    let mut selector = fixed("system", TerminalTheme::Dark);
    assert_eq!(press(&mut selector, KeyCode::Esc), SelectorOutcome::Cancel);
}

/// Choosing `automatic` opens the automatic menu: the light and dark rows, `Apply`, `Change mode`,
/// and pi's two explanatory lines — with no search row and the shorter hint.
#[test]
fn choosing_automatic_opens_the_pair_menu() {
    let mut selector = fixed("dark", TerminalTheme::Dark);
    // The highlight starts on `dark`; `automatic` is above it.
    press(&mut selector, KeyCode::Up);
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Preview("dark/dark".to_string()),
        "the pair preview, seeded from the theme that was current"
    );
    let text = screen(&mut selector);
    for want in [
        "Automatic Theme",
        "Choose themes for terminal light and dark appearance.",
        "Light/dark detection requires terminal support.",
        "Light theme",
        "Dark theme",
        "Apply",
        "Change mode",
        "Enter/Space to change · Esc to cancel",
    ] {
        assert!(text.contains(want), "{want:?} missing:\n{text}");
    }
    assert!(
        !text.contains("Type to search"),
        "the automatic menu has no search:\n{text}"
    );
    assert!(!text.contains("> "), "and no search row:\n{text}");
}

/// A pair setting opens straight on the automatic menu, with its two halves.
#[test]
fn a_pair_setting_opens_the_automatic_menu() {
    let mut selector = fixed("light/solarized", TerminalTheme::Dark);
    let text = screen(&mut selector);
    assert!(text.contains("Automatic Theme"), "{text}");
    assert!(text.contains("Light theme"), "{text}");
    assert!(text.contains("solarized"), "{text}");
    assert_eq!(selector.original_setting(), "light/solarized");
}

/// Each half is chosen from its own picker; `Apply` confirms the pair as `light/dark`.
#[test]
fn the_pair_is_built_from_two_pickers_and_applied() {
    let mut selector = fixed("light/solarized", TerminalTheme::Dark);
    // Light theme row -> its picker.
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Redraw
    );
    let text = screen(&mut selector);
    assert!(text.contains("Light Theme"), "{text}");
    assert!(
        text.contains("Select the theme to use for light terminal appearance"),
        "{text}"
    );
    assert!(
        text.contains("✓ light"),
        "the current half is marked:\n{text}"
    );
    // `light` is under the highlight; move to `dark` (previewing it), pick it.
    assert_eq!(
        press(&mut selector, KeyCode::Up),
        SelectorOutcome::Preview("dark".to_string())
    );
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Preview("dark/solarized".to_string()),
        "the pair is previewed on return"
    );
    // Back on the automatic menu, the row shows the new value and the cursor is where it was left.
    let text = screen(&mut selector);
    assert!(text.contains("Automatic Theme"), "{text}");
    let light_row = text.lines().find(|l| l.contains("Light theme")).unwrap();
    assert!(
        light_row.contains("dark") && light_row.contains("→"),
        "{light_row}"
    );

    // Down to Dark theme, pick `system`.
    press(&mut selector, KeyCode::Down);
    press(&mut selector, KeyCode::Enter);
    assert!(screen(&mut selector).contains("Dark Theme"));
    // The Dark picker opens on its current half, `solarized`; `system` is three rows above.
    press(&mut selector, KeyCode::Up);
    press(&mut selector, KeyCode::Up);
    press(&mut selector, KeyCode::Up);
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Preview("dark/system".to_string())
    );

    // Down to Apply.
    press(&mut selector, KeyCode::Down);
    assert_eq!(
        press(&mut selector, KeyCode::Enter),
        SelectorOutcome::Confirm("dark/system".to_string())
    );
}

/// `Esc` in a picker returns to the automatic menu without changing the half; `Esc` in the menu
/// leaves for `/settings`.
#[test]
fn escape_walks_back_one_step_at_a_time() {
    let mut selector = fixed("light/dark", TerminalTheme::Dark);
    press(&mut selector, KeyCode::Enter);
    assert!(screen(&mut selector).contains("Light Theme"));
    assert_eq!(
        press(&mut selector, KeyCode::Esc),
        SelectorOutcome::Preview("light/dark".to_string()),
        "pi previews the unchanged pair (`preview(getThemeSetting()); done()`)"
    );
    assert!(screen(&mut selector).contains("Automatic Theme"));
    assert_eq!(press(&mut selector, KeyCode::Esc), SelectorOutcome::Cancel);
}

/// `Change mode` returns to the single menu on the theme the terminal's appearance selects.
#[test]
fn change_mode_returns_to_the_single_menu_on_the_active_half() {
    for (terminal, expected) in [
        (TerminalTheme::Light, "solarized"),
        (TerminalTheme::Dark, "dark"),
    ] {
        let mut selector = fixed("solarized/dark", terminal);
        // Down to `Change mode` (the fourth row).
        press(&mut selector, KeyCode::Down);
        press(&mut selector, KeyCode::Down);
        press(&mut selector, KeyCode::Down);
        assert_eq!(
            press(&mut selector, KeyCode::Enter),
            SelectorOutcome::Preview(expected.to_string()),
            "{terminal:?}"
        );
        let text = screen(&mut selector);
        assert!(
            text.contains(&format!("✓ {expected}")),
            "{terminal:?}:\n{text}"
        );
        assert!(text.contains("automatic"), "{text}");
    }
}

/// `preferredTheme`: a current theme that is not on offer (a removed custom theme) falls back to the
/// system theme.
#[test]
fn an_unavailable_current_theme_falls_back_to_system() {
    let mut selector = fixed("gone", TerminalTheme::Dark);
    let text = screen(&mut selector);
    assert!(text.contains("✓ system"), "{text}");
}

/// Space is a key too: on the automatic menu (no search `Input`) it activates the row.
#[test]
fn space_activates_a_row_on_the_automatic_menu() {
    let mut selector = fixed("light/dark", TerminalTheme::Dark);
    press(&mut selector, KeyCode::Char(' '));
    assert!(screen(&mut selector).contains("Light Theme"));
}
