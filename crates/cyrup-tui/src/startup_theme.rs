//! The theme a pre-launch selector paints in — pi's `createStartupTui` theme handling
//! (`cli/startup-ui.ts:77-100`, `:117-127` @v1.0.0).
//!
//! A startup selector is a short-lived TUI that mounts before the agent runtime exists, so there is
//! no controller, no resource registry and no settings manager beyond the file itself. What pi gives
//! it is:
//!
//! 1. `markTerminalColorsPending()` + `initTheme(resolveThemeSetting(theme, terminalTheme) ??
//!    "system")` — the theme the setting names, and the `system` theme in grayscale until the
//!    terminal's colours are known;
//! 2. `ui.start()` and, at once, `queryStartupTerminalColors` — the terminal is asked for its colours
//!    WITHOUT waiting: the selector is on screen already, and when the answer arrives (or the
//!    100 ms query gives up) the colours are recorded, the theme is re-applied and the screen is
//!    rebuilt;
//! 3. for the first-run wizard, `onThemePreview` → `setTheme(name)`: moving the highlight recolours
//!    the dialog, and a colour answer re-applies the PREVIEWED theme, not the setting's.
//!
//! [`StartupTheme`] is that state as a value; [`crate::startup_loop`] feeds it the terminal's colours
//! and the selector's previews.

use crate::terminal_query::TerminalColors;
use crate::theme::{ThemeController, UiTheme};

/// The theme state of one pre-launch selector. Cheap to clone: each selector re-runs pi's
/// `createStartupTui`, so each starts from the same pending state.
#[derive(Clone, Debug)]
pub struct StartupTheme {
    controller: ThemeController,
    /// The theme the wizard's highlight names (`previewTheme`, `startup-ui.ts:202`), if any.
    preview: Option<String>,
}

impl StartupTheme {
    /// pi `createStartupTui`'s theme preamble: `settings.theme` (or the `--use-theme` override,
    /// already folded in by the caller) resolved against the environment's guess of the terminal's
    /// appearance, `system` when it names nothing, and the system theme in grayscale until
    /// [`Self::apply_colors`] is called.
    #[must_use]
    pub fn resolve(setting: Option<&str>) -> Self {
        Self {
            controller: ThemeController::boot_from_env(setting),
            preview: None,
        }
    }

    /// A startup theme over a controller that has already settled — a test seam and the path for a
    /// caller that holds the terminal's colours.
    #[must_use]
    pub fn from_controller(controller: ThemeController) -> Self {
        Self {
            controller,
            preview: None,
        }
    }

    /// The theme to paint now.
    #[must_use]
    pub fn theme(&self) -> UiTheme {
        match &self.preview {
            Some(name) => self.controller.theme_named(name),
            None => self.controller.theme(),
        }
    }

    /// Whether the terminal's colours are still awaited, i.e. the system theme is grayscale.
    #[must_use]
    pub fn colors_pending(&self) -> bool {
        self.controller.terminal_colors_pending()
    }

    /// pi `queryStartupTerminalColors`'s callback (`startup-ui.ts:117-127`): record the colours —
    /// `setTerminalColors(colors)` — and re-apply the theme, which regenerates the system theme and
    /// fills the `""` tokens. `true` when the painted theme may differ.
    pub fn apply_colors(&mut self, colors: TerminalColors) -> bool {
        self.controller.apply_terminal_colors(colors).is_some()
    }

    /// pi `onThemePreview` (`startup-ui.ts:204-209`): paint the named theme from now on.
    pub fn preview(&mut self, name: &str) {
        self.preview = Some(name.to_string());
    }
}

#[cfg(test)]
#[path = "startup_theme_tests.rs"]
mod tests;
