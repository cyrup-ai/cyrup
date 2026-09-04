//! The **automatic** window/tab title — the port of Pi's `updateTerminalTitle`
//! (`coding-agent/src/modes/interactive/interactive-mode.ts:818-826`).
//!
//! Pi composes the title from two live facts and hands it to `ui.terminal.setTitle`
//! (`tui/src/terminal.ts:504-507`, the OSC 0 write that [`crate::app::write_terminal_title`] ports):
//!
//! ```text
//! sessionName ? `${APP_TITLE} - ${sessionName} - ${cwdBasename}`
//!             : `${APP_TITLE} - ${cwdBasename}`
//! ```
//!
//! and re-runs it whenever either input can have moved: once the interactive mode is up (`:860`),
//! after a session (re-)bind (`:1761`), when the extension set is unbound (`:1995`), and on the
//! `session_info_changed` event (`:2901`). That is the
//! half cyrup was missing: the OSC 0 primitive existed but only an extension's `ui.setTitle` ever
//! reached it, so several cyrup sessions in adjacent tabs/panes were indistinguishable.
//!
//! `APP_TITLE` is Pi's `config.ts:490` (`piConfig.name`, falling back to the `π` glyph). Cyrup's
//! rebrand fixes the distribution name to `cyrup` (`crates/cyrup/src/startup.rs:22`), so the
//! constant is that name rather than a package-derived value.

use std::path::Path;

/// Pi `APP_TITLE` (`config.ts:490`) under cyrup's rebrand — `crates/cyrup/src/startup.rs:22`'s
/// `APP_NAME`. Pi's value is `piConfig.name` when a fork sets one and the `π` glyph otherwise; a
/// rebranded distribution therefore uses its own name, which is what this is.
pub const APP_TITLE: &str = "cyrup";

/// Compose the automatic terminal title from the session's display name and its working directory —
/// Pi `updateTerminalTitle` (`interactive-mode.ts:818-826`).
///
/// * `session_name` — Pi's `sessionManager.getSessionName()`. JavaScript's truthiness test
///   (`if (sessionName)`) drops an EMPTY name to the un-named form, so `Some("")` is treated
///   exactly like `None` here.
/// * `cwd` — Pi's `path.basename(sessionManager.getCwd())`. Node's `basename` yields `""` for a
///   path with no final component (`"/"`, `""`), which is what [`Path::file_name`]'s `None` maps
///   to; a trailing separator is ignored by both (`"/a/b/"` ⇒ `b`).
///
/// Lossy UTF-8 for a non-UTF-8 component: the title is a display string, and the OSC writer strips
/// control characters anyway ([`crate::app::write_terminal_title`]).
pub fn session_terminal_title(session_name: Option<&str>, cwd: &Path) -> String {
    let base = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match session_name.filter(|n| !n.is_empty()) {
        Some(name) => format!("{APP_TITLE} - {name} - {base}"),
        None => format!("{APP_TITLE} - {base}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn named_and_unnamed_forms_match_pi() {
        let cwd = PathBuf::from("/home/u/src/cyrup");
        assert_eq!(
            session_terminal_title(Some("my session"), &cwd),
            "cyrup - my session - cyrup"
        );
        assert_eq!(session_terminal_title(None, &cwd), "cyrup - cyrup");
    }

    #[test]
    fn an_empty_name_is_falsy_like_pis_if_check() {
        let cwd = PathBuf::from("/home/u/proj");
        assert_eq!(session_terminal_title(Some(""), &cwd), "cyrup - proj");
    }

    #[test]
    fn basename_follows_node_semantics() {
        // Trailing separator is ignored, and a root path has no final component (Node: `""`).
        assert_eq!(
            session_terminal_title(None, &PathBuf::from("/a/b/")),
            "cyrup - b"
        );
        assert_eq!(
            session_terminal_title(None, &PathBuf::from("/")),
            "cyrup - "
        );
        assert_eq!(session_terminal_title(None, &PathBuf::from("")), "cyrup - ");
    }
}
