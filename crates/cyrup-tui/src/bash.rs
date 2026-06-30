//! The `!`/`!!` bash-execution component (spec/tui/03 §7; port of
//! `modes/interactive/components/bash-execution.ts`).
//!
//! A `!command` (or `!!command`, excluded-from-context) runs a shell command and streams its
//! stdout/stderr into a live, border-delimited block in the message region: a `$ command` header, a
//! preview of the trailing [`PREVIEW_LINES`] lines (full output when expanded with `Ctrl+O`,
//! `app.tools.expand`), and a running/complete/cancelled/error status with the exit code
//! (`bash-execution.ts:171-204`). The component is **pure render state**: the app shell spawns the
//! process (`tokio::process`) and feeds it via [`append_output`](BashExecution::append_output) /
//! [`set_complete`](BashExecution::set_complete), so the same logic is exercised headlessly in tests.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::theme::UiTheme;

/// Preview-line limit when not expanded (`bash-execution.ts:19`, matches tool-execution behavior).
pub const PREVIEW_LINES: usize = 20;

/// The lifecycle of a bash run (`bash-execution.ts:24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BashStatus {
    /// The process is still running (spinner + cancel hint).
    Running,
    /// Exited 0.
    Complete,
    /// Cancelled by the user (`Esc`/`Ctrl+C`, `tui.select.cancel`).
    Cancelled,
    /// Exited non-zero (`exit N` badge).
    Error,
}

/// One `!`/`!!` bash execution rendered live in the message region and committed to scrollback when
/// it finishes (`BashExecutionComponent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashExecution {
    /// The command line as typed (without the leading `!`/`!!`).
    command: String,
    /// `true` for `!!` (excluded from agent context → dim border instead of bash-green).
    excluded: bool,
    /// Accumulated output, one logical line per element (`outputLines`).
    output_lines: Vec<String>,
    status: BashStatus,
    /// The exit code once finished (`None` while running or if the process was signalled).
    exit_code: Option<i32>,
    /// Whether the full output is shown (vs. the [`PREVIEW_LINES`] tail). Toggled by `Ctrl+O`.
    expanded: bool,
}

impl BashExecution {
    /// Start a fresh running execution of `command` (`!!` when `excluded`).
    pub fn new(command: impl Into<String>, excluded: bool) -> Self {
        BashExecution {
            command: command.into(),
            excluded,
            output_lines: Vec::new(),
            status: BashStatus::Running,
            exit_code: None,
            expanded: false,
        }
    }

    /// The command that was run (`getCommand`).
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Whether this was a `!!` (excluded-from-context) invocation.
    pub fn excluded(&self) -> bool {
        self.excluded
    }

    /// The current lifecycle status.
    pub fn status(&self) -> BashStatus {
        self.status
    }

    /// `true` while the process is still running.
    pub fn is_running(&self) -> bool {
        self.status == BashStatus::Running
    }

    /// The raw accumulated output (`getOutput`), `\n`-joined.
    pub fn output(&self) -> String {
        self.output_lines.join("\n")
    }

    /// Set the expansion state (`Ctrl+O`, `setExpanded`).
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Whether the full output is currently shown.
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Append a streamed chunk (`appendOutput`, `bash-execution.ts:80-96`): strip ANSI escapes,
    /// normalize CRLF/CR to LF, then merge the first new line onto the last existing line (an
    /// incomplete-line continuation) and push the rest as new lines.
    pub fn append_output(&mut self, chunk: &str) {
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");
        let new_lines: Vec<&str> = clean.split('\n').collect();
        if let (Some(last), Some(first)) = (self.output_lines.last_mut(), new_lines.first()) {
            last.push_str(first);
            self.output_lines.extend(new_lines.iter().skip(1).map(|s| (*s).to_string()));
        } else {
            self.output_lines.extend(new_lines.iter().map(|s| (*s).to_string()));
        }
    }

    /// Mark the run finished (`setComplete`, `bash-execution.ts:98-117`): a `cancelled` run becomes
    /// [`BashStatus::Cancelled`]; a non-zero/`None` exit code is [`BashStatus::Error`]; else
    /// [`BashStatus::Complete`].
    pub fn set_complete(&mut self, exit_code: Option<i32>, cancelled: bool) {
        self.exit_code = exit_code;
        self.status = if cancelled {
            BashStatus::Cancelled
        } else if exit_code != Some(0) {
            BashStatus::Error
        } else {
            BashStatus::Complete
        };
    }

    /// The styled lines for the current frame (`updateDisplay` render, `bash-execution.ts:119-204`):
    /// a blank spacer, the top `DynamicBorder`, the bold `$ command` header, the output (full when
    /// expanded, else the trailing [`PREVIEW_LINES`] with a `… N more lines` hint), the running
    /// spinner-or-status line, and the bottom `DynamicBorder`. `cancel_hint`/`expand_hint` are the
    /// live key labels (`Esc`, `Ctrl+O`) so rebinds reflect.
    pub fn render_lines(
        &self,
        width: usize,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
        expand_hint: Option<&str>,
    ) -> Vec<Line<'static>> {
        let border_style =
            if self.excluded { theme.dim_style() } else { theme.bash_mode_style() };
        let header_style = border_style.add_modifier(Modifier::BOLD);
        let rule = "─".repeat(width.max(1));
        let mut out: Vec<Line<'static>> = Vec::new();
        out.push(Line::default());
        out.push(Line::styled(rule.clone(), border_style));
        out.push(Line::styled(format!("$ {}", self.command), header_style));

        let available: Vec<&String> = self.output_lines.iter().collect();
        let total = available.len();
        let (visible, hidden): (Vec<&String>, usize) = if self.expanded || total <= PREVIEW_LINES {
            (available.clone(), 0)
        } else {
            let start = total - PREVIEW_LINES;
            (available.get(start..).map(<[&String]>::to_vec).unwrap_or_default(), start)
        };
        for line in &visible {
            out.push(Line::styled(format!("  {line}"), theme.muted_style()));
        }

        match self.status {
            BashStatus::Running => {
                let hint = cancel_hint.unwrap_or("Esc");
                out.push(Line::styled(format!("  Running... ({hint} to cancel)"), theme.dim_style()));
            }
            _ => {
                let mut status_spans: Vec<Span<'static>> = Vec::new();
                if hidden > 0 {
                    let key = expand_hint.unwrap_or("Ctrl+O");
                    let (key_label, what) = if self.expanded {
                        (key.to_string(), "to collapse".to_string())
                    } else {
                        (key.to_string(), format!("({hidden} more lines, to expand)"))
                    };
                    status_spans.push(Span::styled(format!("  {key_label} "), theme.dim_style()));
                    status_spans.push(Span::styled(what, theme.muted_style()));
                }
                match self.status {
                    BashStatus::Cancelled => {
                        if !status_spans.is_empty() {
                            out.push(Line::from(std::mem::take(&mut status_spans)));
                        }
                        out.push(Line::styled("  (cancelled)".to_string(), theme.warning_style()));
                    }
                    BashStatus::Error => {
                        if !status_spans.is_empty() {
                            out.push(Line::from(std::mem::take(&mut status_spans)));
                        }
                        let code = self
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".to_string());
                        out.push(Line::styled(format!("  (exit {code})"), theme.error_style()));
                    }
                    _ => {
                        if !status_spans.is_empty() {
                            out.push(Line::from(status_spans));
                        }
                    }
                }
            }
        }
        out.push(Line::styled(rule, border_style));
        out
    }
}


/// Strip ANSI/VT escape sequences (CSI/OSC and the common single-char escapes) from `s`
/// (`utils/ansi.ts` `stripAnsi`). A small hand-rolled scanner — no new dependency, no `unsafe`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            // CSI: ESC [ ... final-byte in @-~
            Some('[') => {
                chars.next();
                for n in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... terminated by BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '\u{07}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // Other escapes (e.g. ESC ( B): drop ESC + the next byte.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m text"), "red text");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{07}body"), "body");
    }

    #[test]
    fn append_merges_incomplete_lines() {
        let mut b = BashExecution::new("echo hi", false);
        b.append_output("foo");
        b.append_output("bar\nbaz");
        // "foo" + "bar" merge onto one line, "baz" is a new line.
        assert_eq!(b.output(), "foobar\nbaz");
    }

    #[test]
    fn complete_classifies_exit_codes() {
        let mut ok = BashExecution::new("true", false);
        ok.set_complete(Some(0), false);
        assert_eq!(ok.status(), BashStatus::Complete);

        let mut err = BashExecution::new("false", false);
        err.set_complete(Some(1), false);
        assert_eq!(err.status(), BashStatus::Error);

        let mut cancelled = BashExecution::new("sleep 10", false);
        cancelled.set_complete(None, true);
        assert_eq!(cancelled.status(), BashStatus::Cancelled);
    }

    #[test]
    fn render_shows_header_and_running_hint() {
        let theme = UiTheme::dark();
        let b = BashExecution::new("ls -la", false);
        let lines = b.render_lines(40, &theme, Some("Esc"), Some("Ctrl+O"));
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text.iter().any(|l| l.contains("$ ls -la")), "header: {text:?}");
        assert!(text.iter().any(|l| l.contains("Running...") && l.contains("Esc")), "{text:?}");
    }

    #[test]
    fn collapsed_preview_truncates_and_counts_hidden() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("seq 30", false);
        for i in 1..=30 {
            b.append_output(&format!("line{i}\n"));
        }
        b.set_complete(Some(0), false);
        let lines = b.render_lines(40, &theme, None, Some("Ctrl+O"));
        let text: Vec<String> = lines.iter().map(plain).collect();
        // 30 output lines + a trailing empty (from the final "\n") → preview keeps the last 20.
        assert!(text.iter().any(|l| l.contains("line30")), "tail shown: {text:?}");
        assert!(!text.iter().any(|l| l.trim() == "line1"), "first line hidden: {text:?}");
        assert!(
            text.iter().any(|l| l.contains("11 more lines") && l.contains("Ctrl+O")),
            "hidden count + expand hint: {text:?}"
        );

        // Expanded shows everything.
        let mut e = b.clone();
        e.set_expanded(true);
        let etext: Vec<String> =
            e.render_lines(40, &theme, None, None).iter().map(plain).collect();
        assert!(etext.iter().any(|l| l.contains("line1")), "expanded shows first line: {etext:?}");
    }

    #[test]
    fn excluded_uses_dim_border_style() {
        let theme = UiTheme::dark();
        let b = BashExecution::new("secret", true);
        assert!(b.excluded());
        let lines = b.render_lines(20, &theme, None, None);
        // Top border (line index 1, after the spacer) carries the dim style for `!!`.
        assert_eq!(lines[1].style, theme.dim_style());
    }
}
