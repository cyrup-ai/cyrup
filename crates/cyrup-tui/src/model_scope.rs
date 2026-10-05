//! The `Model scope:` startup line — pi's `init()` announcement of the `--models` /
//! `enabledModels` cycle set (`interactive-mode.ts:940-953` @v1.0.0):
//!
//! ```text
//! Model scope: <id[:thinking], …> (<CycleKey> to cycle)
//! ```
//!
//! # Where it goes
//! Pi prints it with `console.log` from `init()`, before its TUI mounts. That puts it in the
//! terminal's own scrollback, above everything the interface draws: above the inline region, and —
//! because the alternate screen is entered later — on the *main* screen in fullscreen, where it
//! is not visible while the alternate screen is up and is there again once it is left. This module
//! reproduces that and nothing more: [`ModelScopeBanner::write_to`] is for a plain stdout write made
//! BEFORE `App::into_stdout` (raw mode, the cursor-position probe that anchors the inline region,
//! and the alternate screen all come after), and the banner is deliberately not an [`crate::Entry`],
//! not a fullscreen document row and not part of any frame.
//!
//! # Gate
//! `session.scopedModels.length > 0 && shouldShowStartupDetails()`: a non-empty scope, and
//! `--verbose` or `quietStartup: false` ([`QuietStartup::shows_details`]) — `true` and `"header"`
//! both hide it. [`ModelScopeBanner::for_startup`] returns `None` otherwise, so an empty or gated
//! banner has no representation to print.

use std::io::Write;

use cyrup_config::settings::QuietStartup;
use cyrup_session_svc::ScopedModel;
use ratatui::backend::IntoCrossterm;
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{ContentStyle, Print, PrintStyledContent};
use ratatui::text::{Line, Span};

use crate::chrome::format_key_text;
use crate::keymap::{Action, Keymap};
use crate::theme::UiTheme;

/// The announcement line, built only when pi would print it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelScopeBanner {
    line: Line<'static>,
}

impl ModelScopeBanner {
    /// The banner for `scoped`, or `None` when pi prints nothing: an empty scope, or a startup
    /// whose details are hidden (`quietStartup` `true` / `"header"` without `--verbose`).
    ///
    /// `cycle_keys` is the user's resolved `app.model.cycleForward` keys, `/`-joined as pi's
    /// `cycleKeys.join("/")` (see [`cycle_forward_keys`]); `None` is an unbound action, which pi
    /// renders without the hint.
    pub fn for_startup(
        scoped: &[ScopedModel],
        quiet_startup: QuietStartup,
        verbose: bool,
        cycle_keys: Option<&str>,
        theme: &UiTheme,
    ) -> Option<Self> {
        if scoped.is_empty() || !quiet_startup.shows_details(verbose) {
            return None;
        }
        // `${sm.model.id}${sm.thinkingLevel ? `:${sm.thinkingLevel}` : ""}`, joined with ", ". A
        // set level is always printed, `off` included — it is a non-empty string in pi.
        let list = scoped
            .iter()
            .map(|sm| match sm.thinking_level {
                Some(level) => format!(
                    "{}:{}",
                    sm.model.id.as_str(),
                    crate::app::thinking_level_str(level)
                ),
                None => sm.model.id.as_str().to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        // `theme.fg("dim", `Model scope: ${list}${theme.fg("muted", hint)}`)`: the hint is the
        // muted island at the end of the dim line.
        let mut spans = vec![Span::styled(
            format!("Model scope: {list}"),
            theme.dim_style(),
        )];
        if let Some(keys) = cycle_keys.filter(|k| !k.is_empty()) {
            spans.push(Span::styled(
                format!(" ({} to cycle)", format_key_text(keys, true)),
                theme.muted_style(),
            ));
        }
        Some(Self {
            line: Line::from(spans),
        })
    }

    /// The line's text without styling.
    pub fn text(&self) -> String {
        self.line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The styled spans, for a test of the dim/muted split.
    pub fn line(&self) -> &Line<'static> {
        &self.line
    }

    /// Write the line and its newline, as `console.log` does. Each span goes through crossterm's
    /// `PrintStyledContent`, which restores what it set, so no styling leaks into the shell or the
    /// interface that follows. Meant for a cooked-mode terminal, i.e. before `App::into_stdout`.
    pub fn write_to<W: Write>(&self, out: &mut W) -> std::io::Result<()> {
        for span in &self.line.spans {
            let style: ContentStyle = self.line.style.patch(span.style).into_crossterm();
            queue!(out, PrintStyledContent(style.apply(&*span.content)))?;
        }
        queue!(out, Print("\n"))?;
        out.flush()
    }
}

/// The user's `app.model.cycleForward` keys as pi's `keybindings.getKeys(…).join("/")` yields them:
/// the default table with `keybindings_json` (the contents of `keybindings.json`) merged over it,
/// every bound key joined with `/`, or `None` when the user unbound the action.
///
/// The announcement is made before the interface — and with it the live [`Keymap`] — exists, so the
/// table is rebuilt here the way the app builds its own: defaults, then the user's document. A
/// document the keymap rejects leaves the defaults, which is also what the app falls back to.
pub fn cycle_forward_keys(keybindings_json: Option<&str>) -> Option<String> {
    let mut keymap = Keymap::default();
    if let Some(json) = keybindings_json {
        let _ = keymap.merge_json(json);
    }
    keymap.keys_label(Action::ModelCycleForward)
}
