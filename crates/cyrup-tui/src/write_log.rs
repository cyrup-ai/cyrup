//! The escape-sequence write log — cyrup's port of pi's `PI_TUI_WRITE_LOG` (TUI-040).
//!
//! # What upstream records
//!
//! `ProcessTerminal` resolves one log path per process from `PI_TUI_WRITE_LOG` (`writeLogPath`,
//! `packages/tui/src/terminal.ts:149-162` @v0.87.1): unset or empty is off; an existing directory
//! gets a per-instance file `tui-<YYYY-MM-DD_HH-MM-SS>-<pid>.log` inside it; anything else — a
//! stat failure included — is used as the file path as-is. Its `write(data)` (`:470-479`) writes
//! to stdout and then `fs.appendFileSync`s the same bytes, swallowing any logging error.
//!
//! `write` is the TUI's own channel, and the log is exactly what flows through it: every rendered
//! frame (`tui-main-screen.ts` / `tui-alt-screen.ts` `this.terminal.write(buffer)`), the alternate
//! screen's enter/exit and mouse-mode writes, and the TUI's terminal queries — OSC 11 background
//! (`tui.ts:1426`), `CSI ? 996 n` colour scheme (`:1453`) and `CSI 16 t` cell size (`:929`). The
//! terminal-lifecycle writes `ProcessTerminal` makes with `process.stdout.write` directly are NOT
//! logged upstream: bracketed paste, the Kitty push/query/pop, modifyOtherKeys, cursor show/hide,
//! the window title and OSC 9;4 progress (`terminal.ts:185-537`).
//!
//! # How cyrup maps it
//!
//! [`TuiStdout`] is that `write`: stdout, teed. It is the writer under the inline renderer's
//! [`crate::InlineBackend`] (every frame), under the alternate screen's escape sink
//! (`altscreen/out.rs`), around each frame's synchronized-output markers, under the startup
//! selector's frames, and under the three TUI-level queries in [`crate::terminal_query`]. The
//! writes pi makes around it stay on plain stdout here too, so a log reads the same on both sides.
//!
//! `[CYRUP-DELTA]` the variable is `CYRUP_TUI_WRITE_LOG` with no `PI_TUI_WRITE_LOG` alias — the
//! hard rename every `PI_*` variable took (`crates/cyrup-config/src/env.rs`). The directory form's
//! timestamp is local time, as pi's `new Date().getHours()` … is, read through the same offset
//! source `/tree`'s label timestamps use ([`crate::tree_selector::local_offset_at`]).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::dead_terminal::{TerminalWriter, terminal_stdout};

/// `CYRUP_TUI_WRITE_LOG` — pi `PI_TUI_WRITE_LOG`.
pub const ENV_TUI_WRITE_LOG: &str = "CYRUP_TUI_WRITE_LOG";

/// pi's `writeLogPath` initializer (`terminal.ts:149-162` @v0.87.1) over an env value, the clock,
/// the local-offset source and the process id. `None` when the variable is unset or empty
/// (`if (!env) return ""`).
pub(crate) fn resolve_write_log_path(
    value: Option<OsString>,
    now_utc: time::OffsetDateTime,
    local_offset_at: impl Fn(time::OffsetDateTime) -> time::UtcOffset,
    pid: u32,
) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|v| !v.is_empty())?);
    // `fs.statSync(env).isDirectory()` in a `try`: a path that cannot be stat'ed is a file path.
    if path.is_dir() {
        // `new Date()` read through `getFullYear()` … `getSeconds()`: the local wall clock.
        let now = now_utc.to_offset(local_offset_at(now_utc));
        let ts = format!(
            "{}-{:02}-{:02}_{:02}-{:02}-{:02}",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        return Some(path.join(format!("tui-{ts}-{pid}.log")));
    }
    Some(path)
}

/// This process's log path, resolved once — pi resolves it once per `ProcessTerminal`, and a
/// process has one.
fn process_write_log() -> Option<&'static Path> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        resolve_write_log_path(
            std::env::var_os(ENV_TUI_WRITE_LOG),
            time::OffsetDateTime::now_utc(),
            crate::tree_selector::local_offset_at,
            std::process::id(),
        )
    })
    .as_deref()
}

/// A writer that forwards to `inner` and appends every byte `inner` accepted to the write log —
/// pi's `ProcessTerminal.write` (`terminal.ts:470-479`).
#[derive(Debug)]
pub struct TeeWriter<W> {
    inner: W,
    log: Option<PathBuf>,
}

impl<W> TeeWriter<W> {
    /// Tee `inner` into `log`; `None` is a plain pass-through.
    pub fn new(inner: W, log: Option<PathBuf>) -> Self {
        Self { inner, log }
    }
}

impl<W: Write> Write for TeeWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if let (Some(log), Some(written)) = (self.log.as_deref(), buf.get(..n)) {
            // `fs.appendFileSync(this.writeLogPath, data)` inside `try { } catch { }`: opened per
            // write, created if missing, and a failure is dropped — the log must never cost a frame.
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .and_then(|mut f| f.write_all(written));
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Stdout as the TUI writes it: dead-terminal guarded ([`crate::dead_terminal`]) and teed into
/// `CYRUP_TUI_WRITE_LOG` when that is set.
pub type TuiStdout = TeeWriter<TerminalWriter<io::Stdout>>;

/// A fresh [`TuiStdout`] handle (handles share stdout's one lock, as `io::stdout()` handles do).
pub(crate) fn tui_stdout() -> TuiStdout {
    TeeWriter::new(
        terminal_stdout(),
        process_write_log().map(Path::to_path_buf),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 2026-09-08 07:05:04 UTC.
    fn at() -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(1_788_851_104).unwrap()
    }

    /// A local zone that is UTC.
    fn utc(_: time::OffsetDateTime) -> time::UtcOffset {
        time::UtcOffset::UTC
    }

    /// A `Write` over a shared buffer, so the bytes the terminal received can be read back.
    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// **TUI-040.** A frame drawn through the production [`crate::InlineBackend`] over a teed
    /// writer lands in the log byte-for-byte as the terminal received it — pi's `write(data)`
    /// (`terminal.ts:470-479`): the terminal first, then `appendFileSync` of the same data, with an
    /// existing log appended to rather than replaced.
    #[test]
    fn a_drawn_frame_is_teed_into_the_log_verbatim() {
        use ratatui::layout::Rect;
        use ratatui::widgets::Paragraph;
        let log = std::env::temp_dir().join(format!(
            "cyrup-write-log-test-{}-{:?}.log",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&log, b"earlier\n").unwrap();
        let screen = Shared::default();
        let backend = crate::InlineBackend::with_anchor(
            TeeWriter::new(screen.clone(), Some(log.clone())),
            ratatui::layout::Position::ORIGIN,
        );
        let mut terminal = ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(Rect::new(0, 0, 20, 2)),
            },
        )
        .unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new("hello frame"), f.area()))
            .unwrap();
        let sent = screen.0.lock().unwrap().clone();
        let logged = std::fs::read(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        assert!(String::from_utf8_lossy(&sent).contains("hello"), "{sent:?}");
        assert_eq!(logged, [b"earlier\n".as_slice(), &sent].concat());
    }

    /// With no log the writer is a plain pass-through.
    #[test]
    fn no_log_is_a_pass_through() {
        let screen = Shared::default();
        let mut tee = TeeWriter::new(screen.clone(), None);
        tee.write_all(b"\x1b[?2026h").unwrap();
        assert_eq!(screen.0.lock().unwrap().as_slice(), b"\x1b[?2026h");
    }

    /// pi `if (!env) return ""` — unset and empty are both off.
    #[test]
    fn unset_or_empty_is_off() {
        assert_eq!(resolve_write_log_path(None, at(), utc, 7), None);
        assert_eq!(
            resolve_write_log_path(Some(OsString::new()), at(), utc, 7),
            None
        );
    }

    /// An existing directory gets `tui-<YYYY-MM-DD_HH-MM-SS>-<pid>.log`, zero-padded as pi's
    /// `padStart(2, "0")` pads; any other value is the file path itself.
    #[test]
    fn a_directory_gets_a_per_process_file_and_anything_else_is_the_path() {
        let dir = std::env::temp_dir();
        assert_eq!(
            resolve_write_log_path(Some(dir.clone().into_os_string()), at(), utc, 4242),
            Some(dir.join("tui-2026-09-08_07-05-04-4242.log"))
        );
        let file = dir.join("cyrup-write-log-that-does-not-exist.log");
        assert_eq!(
            resolve_write_log_path(Some(file.clone().into_os_string()), at(), utc, 4242),
            Some(file)
        );
    }

    /// pi stamps the directory form with `new Date()`'s LOCAL fields (`getHours()` …), so a clock
    /// read at 07:05:04 UTC names the file after the local wall clock — here 21:05:04 the previous
    /// day, ten hours west — date included.
    #[test]
    fn the_directory_form_is_stamped_in_local_time() {
        let dir = std::env::temp_dir();
        let hawaii = |_| time::UtcOffset::from_hms(-10, 0, 0).unwrap();
        assert_eq!(
            resolve_write_log_path(Some(dir.clone().into_os_string()), at(), hawaii, 4242),
            Some(dir.join("tui-2026-09-07_21-05-04-4242.log"))
        );
    }
}
