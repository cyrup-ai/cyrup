//! The `/settings` theme submenu — a port of pi's `ThemeSubmenu`
//! (`modes/interactive/components/settings-selector.ts:208-357` @v1.0.0).
//!
//! One theme, or two. The **single** menu lists the system theme first ("Theme created from your
//! terminal's colors"), then `automatic`, then every other theme; picking a theme applies it,
//! picking `automatic` switches to the **automatic** menu, where a light theme and a dark theme are
//! chosen — each from its own picker — and `Apply` saves the pair as `light/dark`. `Change mode`
//! goes back to the single menu with the theme the terminal's appearance currently selects.
//!
//! Every move previews the theme under it (`onThemePreview`): a name previews as itself and the
//! pair as the `light/dark` setting, which resolves against the terminal's appearance. `Esc` leaves
//! the current step — back out of a picker to the automatic menu, out of either menu to `/settings`
//! — and the caller restores the theme the submenu opened on.
//!
//! The value this selector confirms is the theme SETTING: `system`, a theme name, or `light/dark`.
//!
//! [CYRUP-DELTA] pi's submenu is one `Container` that swaps its children, and its nested pickers are
//! submenus of the automatic menu's `SettingsList`. Here the three screens are an enum and the
//! nested picker holds the automatic menu aside until it returns, so the highlighted row survives
//! the trip exactly as `closeSubmenu` restores it (`settings-list.ts:233-251`).

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::keymap::{EditorKeymap, SelectKeymap};
use crate::select_list::ColumnLayout;
use crate::selector::{Selector, SelectorOutcome};
use crate::settings_selector::{FIELD_SEP, HeaderLine, SettingRow, SettingsSelector};
use crate::submenu_selector::SubmenuSelector;
use crate::system_theme::{SYSTEM_THEME_DESCRIPTION, SYSTEM_THEME_NAME};
use crate::theme::{TerminalTheme, UiTheme, parse_auto_theme_setting};

/// `AUTOMATIC_THEME_VALUE` (`settings-selector.ts:216`): the single menu's `automatic` row.
const AUTOMATIC_THEME_VALUE: &str = "/";

/// The `SettingItem` ids of the automatic menu (`:343`, `:359`, `:372`, `:380`).
const LIGHT_THEME_ID: &str = "light-theme";
const DARK_THEME_ID: &str = "dark-theme";
const APPLY_ID: &str = "apply";
const SINGLE_MODE_ID: &str = "single-mode";

/// `SUBMENU_SELECT_LIST_LAYOUT` (`settings-submenu.ts:15-18`).
const LAYOUT: ColumnLayout = ColumnLayout {
    primary_min: 12,
    primary_max: 32,
};

/// Which half of an automatic pair a nested picker sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Light,
    Dark,
}

impl Slot {
    const fn id(self) -> &'static str {
        match self {
            Slot::Light => LIGHT_THEME_ID,
            Slot::Dark => DARK_THEME_ID,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Slot::Light => "Light Theme",
            Slot::Dark => "Dark Theme",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Slot::Light => "Select the theme to use for light terminal appearance",
            Slot::Dark => "Select the theme to use for dark terminal appearance",
        }
    }
}

/// The step on screen.
enum Screen {
    /// `showSingleMenu`.
    Single(SubmenuSelector),
    /// `showAutomaticMenu`.
    Automatic(Box<SettingsSelector>),
    /// `createThemeSelect`, a submenu of the automatic menu, which waits in `back`.
    Pick {
        slot: Slot,
        menu: SubmenuSelector,
        back: Box<SettingsSelector>,
    },
}

/// The theme submenu. See the module docs.
pub struct ThemeSelector {
    screen: Option<Screen>,
    available: Vec<String>,
    terminal: TerminalTheme,
    /// The theme setting the submenu opened on (`originalThemeSetting`); restoring it is the
    /// caller's, on [`SelectorOutcome::Cancel`].
    original: String,
    single: String,
    light: String,
    dark: String,
}

/// `preferredTheme` (`:234-238`).
fn preferred_theme(available: &[String], preferred: Option<&str>, fallback: &str) -> String {
    if let Some(preferred) = preferred
        && available.iter().any(|name| name == preferred)
    {
        return preferred.to_string();
    }
    if available.iter().any(|name| name == fallback) {
        return fallback.to_string();
    }
    available
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// `themeItems` (`:208-214`): a `✓` on the current theme, the system theme described.
fn theme_rows(available: &[String], current: &str) -> Vec<(String, String, Option<String>)> {
    available
        .iter()
        .map(|name| {
            (
                name.clone(),
                format!("{}{name}", if name == current { "✓ " } else { "  " }),
                (name == SYSTEM_THEME_NAME).then(|| SYSTEM_THEME_DESCRIPTION.to_string()),
            )
        })
        .collect()
}

/// `singleModeThemeItems` (`:219-232`): the system theme first, then `automatic`, then the rest.
fn single_rows(available: &[String], current: &str) -> Vec<(String, String, Option<String>)> {
    let mut rows = theme_rows(available, current);
    let system = rows
        .iter()
        .position(|(value, _, _)| value == SYSTEM_THEME_NAME)
        .map(|index| rows.remove(index));
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.extend(system);
    out.push((
        AUTOMATIC_THEME_VALUE.to_string(),
        "  automatic".to_string(),
        Some("Use separate themes for light and dark terminal appearance".to_string()),
    ));
    out.extend(rows);
    out
}

fn preselect(rows: &[(String, String, Option<String>)], value: &str) -> usize {
    rows.iter()
        .position(|(candidate, _, _)| candidate == value)
        .unwrap_or(0)
}

impl ThemeSelector {
    /// Open on `current_setting` — the theme setting now in force (`getThemeSelection()`), a name or
    /// a `light/dark` pair — with `available` the names to offer and `terminal` the appearance that
    /// decides which half of a pair is active.
    #[must_use]
    pub fn new(current_setting: &str, terminal: TerminalTheme, available: Vec<String>) -> Self {
        let auto = parse_auto_theme_setting(Some(current_setting));
        // `defaultAutomaticThemes` (`:240-250`).
        let fixed = (!current_setting.contains('/')).then_some(current_setting);
        let (light, dark) = match &auto {
            Some((light, dark)) => (light.clone(), dark.clone()),
            None => {
                let name = preferred_theme(&available, fixed, SYSTEM_THEME_NAME);
                (name.clone(), name)
            }
        };
        let mut selector = ThemeSelector {
            screen: None,
            available,
            terminal,
            original: current_setting.to_string(),
            single: String::new(),
            light,
            dark,
        };
        let active = selector.active_automatic_theme().to_string();
        selector.single = preferred_theme(
            &selector.available,
            if auto.is_some() {
                Some(active.as_str())
            } else {
                fixed
            },
            SYSTEM_THEME_NAME,
        );
        selector.screen = Some(if auto.is_some() {
            Screen::Automatic(Box::new(selector.automatic_menu()))
        } else {
            Screen::Single(selector.single_menu())
        });
        selector
    }

    /// `getActiveAutomaticTheme` (`:352`).
    fn active_automatic_theme(&self) -> &str {
        match self.terminal {
            TerminalTheme::Light => &self.light,
            TerminalTheme::Dark => &self.dark,
        }
    }

    /// `getAutomaticThemeSetting` (`:356`).
    fn automatic_setting(&self) -> String {
        format!("{}/{}", self.light, self.dark)
    }

    /// `showSingleMenu` (`:303-330`).
    fn single_menu(&self) -> SubmenuSelector {
        let rows = single_rows(&self.available, &self.single);
        let selected = preselect(&rows, &self.single);
        SubmenuSelector::new(
            "Theme".to_string(),
            "Select a theme, or choose automatic to follow terminal appearance.".to_string(),
            rows,
            selected,
            false,
            LAYOUT,
        )
        .with_preview()
    }

    /// `showAutomaticMenu` (`:332-391`).
    fn automatic_menu(&self) -> SettingsSelector {
        let rows = vec![
            SettingRow::submenu(
                LIGHT_THEME_ID,
                "Light theme",
                self.light.clone(),
                LIGHT_THEME_ID,
            )
            .with_description("Theme to use in automatic mode when the terminal is light"),
            SettingRow::submenu(
                DARK_THEME_ID,
                "Dark theme",
                self.dark.clone(),
                DARK_THEME_ID,
            )
            .with_description("Theme to use in automatic mode when the terminal is dark"),
            SettingRow::choice(
                APPLY_ID,
                "Apply",
                "save and go back",
                vec!["save and go back".to_string()],
            )
            .with_description("Save and go back"),
            SettingRow::choice(
                SINGLE_MODE_ID,
                "Change mode",
                "switch to single theme",
                vec!["switch to single theme".to_string()],
            )
            .with_description("Switch to one theme for light and dark"),
        ];
        SettingsSelector::new("Automatic Theme", rows)
            .without_search()
            .with_header(vec![
                HeaderLine::Title("Automatic Theme".to_string()),
                HeaderLine::Blank,
                HeaderLine::Muted(
                    "Choose themes for terminal light and dark appearance.".to_string(),
                ),
                HeaderLine::Muted("Light/dark detection requires terminal support.".to_string()),
                HeaderLine::Blank,
            ])
    }

    /// `createThemeSelect` (`:393-413`).
    fn pick_menu(&self, slot: Slot) -> SubmenuSelector {
        let current = match slot {
            Slot::Light => &self.light,
            Slot::Dark => &self.dark,
        };
        let rows = theme_rows(&self.available, current);
        let selected = preselect(&rows, current);
        SubmenuSelector::new(
            slot.title().to_string(),
            slot.description().to_string(),
            rows,
            selected,
            false,
            LAYOUT,
        )
        .with_preview()
    }

    /// The single menu's answer: choosing `automatic` is a mode change, a theme is the result.
    fn single_step(&mut self, outcome: SelectorOutcome) -> SelectorOutcome {
        match outcome {
            // `if (value === AUTOMATIC_THEME_VALUE) { mode = automatic; preview(getThemeSetting());
            // showAutomaticMenu() }` (`:314-318`).
            SelectorOutcome::Confirm(value) if value == AUTOMATIC_THEME_VALUE => {
                self.screen = Some(Screen::Automatic(Box::new(self.automatic_menu())));
                SelectorOutcome::Preview(self.automatic_setting())
            }
            SelectorOutcome::Confirm(value) => {
                self.single.clone_from(&value);
                SelectorOutcome::Confirm(value)
            }
            // `value === AUTOMATIC_THEME_VALUE ? getAutomaticThemeSetting() : value` (`:326`).
            SelectorOutcome::Preview(value) if value == AUTOMATIC_THEME_VALUE => {
                SelectorOutcome::Preview(self.automatic_setting())
            }
            other => other,
        }
    }

    /// The automatic menu's answer.
    fn automatic_step(
        &mut self,
        back: Box<SettingsSelector>,
        outcome: SelectorOutcome,
    ) -> SelectorOutcome {
        match outcome {
            // A row with a submenu: the light or dark picker.
            SelectorOutcome::OpenSubmenu(id) => {
                let slot = if id == LIGHT_THEME_ID {
                    Slot::Light
                } else {
                    Slot::Dark
                };
                let menu = self.pick_menu(slot);
                self.screen = Some(Screen::Pick { slot, menu, back });
                SelectorOutcome::Redraw
            }
            // `onChange` of the list's own rows (`:393-412`).
            SelectorOutcome::Apply(payload) => {
                let id = payload.split(FIELD_SEP).next().unwrap_or_default();
                match id {
                    // `apply(getAutomaticThemeSetting())`.
                    APPLY_ID => {
                        self.screen = Some(Screen::Automatic(back));
                        SelectorOutcome::Confirm(self.automatic_setting())
                    }
                    // `mode = single; singleTheme = getActiveAutomaticTheme(); preview(singleTheme);
                    // showSingleMenu()`.
                    SINGLE_MODE_ID => {
                        self.single = self.active_automatic_theme().to_string();
                        self.screen = Some(Screen::Single(self.single_menu()));
                        SelectorOutcome::Preview(self.single.clone())
                    }
                    _ => {
                        self.screen = Some(Screen::Automatic(back));
                        SelectorOutcome::Redraw
                    }
                }
            }
            other => {
                self.screen = Some(Screen::Automatic(back));
                other
            }
        }
    }

    /// A nested picker's answer: a pick sets that half of the pair and returns to the automatic
    /// menu; `Esc` returns without setting it.
    fn pick_step(
        &mut self,
        slot: Slot,
        menu: SubmenuSelector,
        mut back: Box<SettingsSelector>,
        outcome: SelectorOutcome,
    ) -> SelectorOutcome {
        match outcome {
            SelectorOutcome::Confirm(value) => {
                match slot {
                    Slot::Light => self.light.clone_from(&value),
                    Slot::Dark => self.dark.clone_from(&value),
                }
                // `item.currentValue = selectedValue` before `closeSubmenu()` (`settings-list.ts:222`).
                back.update_value(slot.id(), &value);
                self.screen = Some(Screen::Automatic(back));
                SelectorOutcome::Preview(self.automatic_setting())
            }
            // `preview(getThemeSetting()); done()` (`:405-408`).
            SelectorOutcome::Cancel => {
                self.screen = Some(Screen::Automatic(back));
                SelectorOutcome::Preview(self.automatic_setting())
            }
            other => {
                self.screen = Some(Screen::Pick { slot, menu, back });
                other
            }
        }
    }

    /// Route `outcome` from whichever step produced it.
    fn advance(
        &mut self,
        mut screen: Screen,
        ask: impl FnOnce(&mut Screen) -> SelectorOutcome,
    ) -> SelectorOutcome {
        let answer = ask(&mut screen);
        match screen {
            Screen::Single(menu) => {
                self.screen = Some(Screen::Single(menu));
                self.single_step(answer)
            }
            Screen::Automatic(list) => self.automatic_step(list, answer),
            Screen::Pick { slot, menu, back } => self.pick_step(slot, menu, back, answer),
        }
    }

    /// The theme setting the submenu opened on (test/inspection).
    #[must_use]
    pub fn original_setting(&self) -> &str {
        &self.original
    }
}

impl Selector for ThemeSelector {
    fn desired_height(&self, width: u16) -> u16 {
        match &self.screen {
            Some(Screen::Single(menu)) | Some(Screen::Pick { menu, .. }) => {
                menu.desired_height(width)
            }
            Some(Screen::Automatic(list)) => list.desired_height(width),
            None => 0,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        match &mut self.screen {
            Some(Screen::Single(menu)) | Some(Screen::Pick { menu, .. }) => {
                menu.render(frame, area, theme);
            }
            Some(Screen::Automatic(list)) => list.render(frame, area, theme),
            None => {}
        }
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        let Some(screen) = self.screen.take() else {
            return SelectorOutcome::Ignored;
        };
        self.advance(screen, |screen| match screen {
            Screen::Single(menu) | Screen::Pick { menu, .. } => menu.handle(key, keymap),
            Screen::Automatic(list) => list.handle(key, keymap),
        })
    }

    fn pointer(&mut self, area: Rect, event: crate::app::Pointer) -> SelectorOutcome {
        let Some(screen) = self.screen.take() else {
            return SelectorOutcome::Ignored;
        };
        self.advance(screen, |screen| match screen {
            Screen::Single(menu) | Screen::Pick { menu, .. } => menu.pointer(area, event),
            Screen::Automatic(list) => list.pointer(area, event),
        })
    }

    fn set_editor_keymap(&mut self, keymap: &EditorKeymap) {
        if let Some(Screen::Single(menu) | Screen::Pick { menu, .. }) = &mut self.screen {
            menu.set_editor_keymap(keymap);
        }
    }
}

#[cfg(test)]
#[path = "theme_selector_tests.rs"]
mod tests;
