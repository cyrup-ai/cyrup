//! SEAM-136 — the startup `--resume` picker driven through a REAL pseudo-terminal.
//!
//! `startup_session_selector.rs` steps [`crate::startup_loop::StartupLoop`] over a `TestBackend` and a
//! scripted key source, and `input/reader_tests.rs` covers the key source over an in-process pty.
//! Neither runs the glue between them: `run_startup_session_selector` turning raw mode and the
//! alternate screen on and off, painting through a `CrosstermBackend`, and reading keys with
//! `Input::open` / `TtyReader`. These do, the way `input_pty.rs` does for the app's input stream:
//! each test re-runs this test binary as a child whose stdin AND stdout are the slave side of a
//! fresh pty and whose controlling terminal it is (`setsid -c`). The parent plays the terminal:
//! it reads the escape stream the child paints, and types keys on the master.
//!
//! The listing the child shows holds one ordinary session and, in the first test, a FIFO that
//! nobody ever writes to, so the scan is stuck for the whole test: whatever the picker does, it does
//! while a load is in flight.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_session_svc::SessionListing;

use super::input_pty::{PtyChild, child_mode, report};
use super::resume_streamed_listing::session_lines;
use crate::keymap::SelectKeymap;
use crate::selector::SelectorOutcome;
use crate::session_selector::SessionSelector;
use crate::startup_loop::StartupSessionLoads;

/// Names the directory the child lists, in its environment.
const DIR_ENV: &str = "CYRUP_TUI_PTY_STARTUP_DIR";

const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

/// The terminal's text with every escape sequence removed, so a row can be looked for whatever
/// styling the painter wrapped around it.
fn visible(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in `@`..=`~`.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ST.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next() == Some('\\')) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The listing the child shows: `current/` holds the one session, `all/` does not exist.
fn listing_dir(stuck_scan: bool) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let current = tmp.path().join("current");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::write(
        current.join("2026-08-09T10-00-00-000Z_aaaa.jsonl"),
        session_lines(
            "01890000-0000-7000-8000-00000000aaaa",
            Path::new("/work"),
            "the newest session",
        ),
    )
    .unwrap();
    if stuck_scan {
        // Read after the file above (names descend); opening it for reading blocks until a writer
        // appears, and none ever does.
        let fifo = current.join("2025-01-01T00-00-00-000Z_bbbb.jsonl");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed");
    }
    tmp
}

/// The child: the production startup picker over `$DIR_ENV`, on the pty it was given.
fn child_picker() {
    let root = PathBuf::from(std::env::var(DIR_ENV).unwrap());
    let loads = StartupSessionLoads {
        current: SessionListing::Dir {
            dir: root.join("current"),
            cwd_filter: None,
        },
        all: SessionListing::Dir {
            dir: root.join("no-such-sessions"),
            cwd_filter: None,
        },
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut picker = SessionSelector::new(vec![]).with_async_loaders();
    report("ready");
    let result = rt.block_on(crate::run_startup_session_selector(
        &crate::StartupTheme::from_controller(crate::ThemeController::boot(
            Some("dark"),
            crate::ColorMode::TrueColor,
            crate::TerminalTheme::Dark,
        )),
        &SelectKeymap::default(),
        &mut picker,
        loads,
        async |_: &str| {},
    ));
    // After the selector returns the terminal must be the shell's again.
    let raw = ratatui::crossterm::terminal::is_raw_mode_enabled().unwrap_or(true);
    match result {
        Ok((SelectorOutcome::Confirm(path), _)) => report(format!(
            "confirm {}",
            Path::new(&path).file_name().unwrap().to_string_lossy()
        )),
        Ok((SelectorOutcome::Cancel, _)) => report("cancel"),
        Ok((other, _)) => report(format!("other {other:?}")),
        Err(e) => report(format!("error {e}")),
    }
    report(format!("raw {raw}"));
    // A listing still parked on the FIFO must not hold the process open.
    rt.shutdown_background();
}

/// The child of the colour test: the production startup runner over a bare prompt, in the theme
/// `createStartupTui` would resolve from a fresh `settings.json`.
fn child_prompt() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut selector = crate::ListSelector::prompt(
        "Pick one".to_string(),
        vec![
            ("a".to_string(), "Alpha".to_string(), None),
            ("b".to_string(), "Beta".to_string(), None),
        ],
        0,
    );
    report("ready");
    let result = rt.block_on(crate::run_startup_selector(
        &crate::StartupTheme::resolve(None),
        &SelectKeymap::default(),
        &mut selector,
        async |_: &str| {},
    ));
    let raw = ratatui::crossterm::terminal::is_raw_mode_enabled().unwrap_or(true);
    match result {
        Ok(SelectorOutcome::Confirm(value)) => report(format!("confirm {value}")),
        Ok(SelectorOutcome::Cancel) => report("cancel"),
        Ok(other) => report(format!("other {other:?}")),
        Err(e) => report(format!("error {e}")),
    }
    report(format!("raw {raw}"));
}

/// A selector that mounts BEFORE the interface launches asks the terminal for its colours and
/// repaints in the theme they generate — pi's `startStartupTui` → `queryStartupTerminalColors`
/// (`cli/startup-ui.ts:92-127` @v1.0.0). The parent plays the terminal: it sees the colour query
/// (OSC 10, OSC 11, OSC 4 ×16, DA1) leave the child, answers it on the master, and looks for the
/// generated colour in what the child paints next.
///
/// FAILS without the change: the startup selector painted a theme resolved before any query, with
/// no OSC query on the wire at all.
#[test]
fn a_prelaunch_selector_queries_the_terminal_and_repaints_in_the_generated_theme() {
    if child_mode().as_deref() == Some("startup_theme") {
        return child_prompt();
    }
    let dir = listing_dir(false);
    let mut pty = PtyChild::spawn_capturing(
        "tests::startup_selector_pty::a_prelaunch_selector_queries_the_terminal_and_repaints_in_the_generated_theme",
        "startup_theme",
        &[
            (DIR_ENV, dir.path().to_str().unwrap()),
            ("COLORTERM", "truecolor"),
            ("CYRUP_TUI_ESC_TIMEOUT", "150"),
        ],
    );
    let mut seen = Vec::new();
    assert!(
        pty.screen_until(&mut seen, ENTER_ALT_SCREEN, Duration::from_secs(20)),
        "the selector never entered the alternate screen; stderr {:#?}",
        pty.noise
    );
    assert!(
        pty.screen_until(&mut seen, "\x1b]11;?", Duration::from_secs(20)),
        "the selector never asked the terminal for its colours; saw {:?}",
        String::from_utf8_lossy(&seen)
    );
    assert!(
        String::from_utf8_lossy(&seen).contains("\x1b]4;15;?"),
        "the whole palette is asked for"
    );
    let asked = seen.len();

    // The terminal answers.
    let colors = super::system_theme_surfaces::mocha();
    let mut controller = crate::ThemeController::boot(
        None,
        crate::ColorMode::TrueColor,
        crate::TerminalTheme::Dark,
    );
    controller.apply_terminal_colors(colors);
    let Some(ratatui::style::Color::Rgb(r, g, b)) = controller.theme().accent else {
        panic!("the generated accent is concrete");
    };
    let generated = format!("38;2;{r};{g};{b}");
    assert!(
        !String::from_utf8_lossy(&seen).contains(&generated),
        "the generated colour was painted before the terminal answered"
    );
    pty.write(&super::system_theme_surfaces::terminal_reply(&colors));
    assert!(
        pty.screen_until(&mut seen, &generated, Duration::from_secs(20)),
        "the selector never repainted in the generated theme ({generated}); saw {:?}",
        String::from_utf8_lossy(seen.get(asked..).unwrap_or_default())
    );

    // The reply was the terminal's, not typing: the selector is still open, and Esc leaves it.
    pty.write(b"\x1b");
    pty.expect("cancel");
    pty.expect("raw false");
}

fn spawn_child(test: &str, dir: &tempfile::TempDir) -> PtyChild {
    PtyChild::spawn_capturing(
        test,
        "startup",
        &[
            (DIR_ENV, dir.path().to_str().unwrap()),
            // Wide enough that a lone Esc is decided inside the test's wait, short enough to keep
            // it quick.
            ("CYRUP_TUI_ESC_TIMEOUT", "150"),
        ],
    )
}

/// A pick, end to end on a real tty, with the scan STUCK.
///
/// The picker enters the alternate screen and paints its rows with no key pressed (so the key wait
/// yields to the load's progress, SEAM-134), `Enter` confirms the streamed row although the other
/// session file never finishes, and on the way out the terminal is restored: alternate screen left
/// and raw mode off.
#[test]
fn a_real_tty_pick_streams_rows_confirms_and_restores_the_terminal() {
    if child_mode().as_deref() == Some("startup") {
        return child_picker();
    }
    let dir = listing_dir(true);
    let mut pty = spawn_child(
        "tests::startup_selector_pty::a_real_tty_pick_streams_rows_confirms_and_restores_the_terminal",
        &dir,
    );
    let mut seen = Vec::new();
    assert!(
        pty.screen_until(&mut seen, ENTER_ALT_SCREEN, Duration::from_secs(20)),
        "the picker never entered the alternate screen; saw {:?}, stderr {:#?}",
        String::from_utf8_lossy(&seen),
        pty.noise
    );
    // No key has been typed: the row can only come from the load's progress being folded in.
    let mut text = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !text.contains("the newest session") {
        assert!(
            std::time::Instant::now() < deadline,
            "no row was painted without a key press; saw {text:?}, stderr {:#?}",
            pty.noise
        );
        pty.screen_until(&mut seen, "\0never", Duration::from_millis(100));
        text = visible(&seen);
    }
    assert!(
        !text.contains("the blocked session"),
        "the stuck file cannot have been listed: {text:?}"
    );
    assert!(
        !String::from_utf8_lossy(&seen).contains(LEAVE_ALT_SCREEN),
        "the terminal was restored while the picker was open"
    );

    pty.write(b"\r");
    pty.expect("confirm 2026-08-09T10-00-00-000Z_aaaa.jsonl");
    pty.expect("raw false");
    assert!(
        pty.screen_until(&mut seen, LEAVE_ALT_SCREEN, Duration::from_secs(20)),
        "the alternate screen was not left; saw {:?}",
        String::from_utf8_lossy(&seen)
    );
}

/// `Esc` cancels over a real tty, and the terminal is restored on that exit too.
#[test]
fn escape_on_a_real_tty_cancels_and_restores_the_terminal() {
    if child_mode().as_deref() == Some("startup") {
        return child_picker();
    }
    let dir = listing_dir(false);
    let mut pty = spawn_child(
        "tests::startup_selector_pty::escape_on_a_real_tty_cancels_and_restores_the_terminal",
        &dir,
    );
    let mut seen = Vec::new();
    assert!(
        pty.screen_until(&mut seen, ENTER_ALT_SCREEN, Duration::from_secs(20)),
        "the picker never entered the alternate screen; stderr {:#?}",
        pty.noise
    );
    // A lone ESC is told from the start of a sequence by waiting out the escape timeout.
    pty.write(b"\x1b");
    pty.expect("cancel");
    pty.expect("raw false");
    assert!(
        pty.screen_until(&mut seen, LEAVE_ALT_SCREEN, Duration::from_secs(20)),
        "the alternate screen was not left; saw {:?}",
        String::from_utf8_lossy(&seen)
    );
    assert_eq!(pty.exit_code(Duration::from_secs(10)), Some(0));
}
