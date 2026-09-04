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

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::status_indicator::StatusIndicator;
use crate::theme::UiTheme;

/// Preview-line limit when not expanded (`bash-execution.ts:19`, matches tool-execution behavior).
pub const PREVIEW_LINES: usize = 20;

/// The live block retains at most the LAST `MAX_OUTPUT_LINES` output lines — the same bound the
/// executor applies to a recorded result ([`cyrup_tools::truncate::DEFAULT_MAX_LINES`]); earlier
/// lines are counted, not kept (TUI-092 F6). Deliberately equal to
/// [`BashExecution::context_truncated`]'s own former `MAX_LINES`, so hitting the ring cap and
/// hitting the context-truncation threshold are the same event.
const MAX_OUTPUT_LINES: usize = 2000;

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
    /// Accumulated output, one logical line per element (`outputLines`), bounded to the last
    /// [`MAX_OUTPUT_LINES`] — earlier lines are evicted from the front in [`Self::append_output`]
    /// and counted in `omitted_lines`, never rendered again (TUI-092 F6). Pure render state:
    /// nothing persists from here (the session record is built independently by
    /// `cyrup-session-svc`'s `BashOutputBuffer`), so the bound is display-only.
    output_lines: VecDeque<String>,
    /// Count of lines evicted from the front of `output_lines` so far. Rendered as a one-line dim
    /// omission notice ahead of the retained window whenever non-zero (TUI-092 F6).
    omitted_lines: usize,
    status: BashStatus,
    /// The exit code once finished (`None` while running or if the process was signalled).
    exit_code: Option<i32>,
    /// Whether the full output is shown (vs. the [`PREVIEW_LINES`] tail). Toggled by `Ctrl+O`.
    expanded: bool,
    /// When the block was created — the phase anchor for the `Running...` spinner. Upstream's
    /// `Loader` owns a `setInterval` (`loader.ts:77-80`); ratatui is immediate-mode, so the frame is
    /// derived from elapsed time exactly as the status band does
    /// ([`crate::status_indicator::StatusIndicator::spinner_at`]).
    started: Instant,
    /// X13 — Pi `BashExecutionComponent.truncationResult?.truncated` (`bash-execution.ts:27`), set
    /// by `setComplete`'s third argument from `result.truncated` (`interactive-mode.ts:6310`,
    /// `:6351`, and the replay at `:3460`). The executor decided the output it handed back was
    /// clipped; the component itself does not recompute that.
    truncated: bool,
    /// X13 — Pi `BashExecutionComponent.fullOutputPath` (`:28`): where the executor spooled the
    /// UNclipped output. `bash-execution.ts:197` renders the warning only when a path is present,
    /// because the whole point of the row is telling the user where to find the rest.
    full_output_path: Option<String>,
}

impl BashExecution {
    /// Start a fresh running execution of `command` (`!!` when `excluded`).
    pub fn new(command: impl Into<String>, excluded: bool) -> Self {
        BashExecution {
            command: command.into(),
            excluded,
            output_lines: VecDeque::new(),
            omitted_lines: 0,
            status: BashStatus::Running,
            exit_code: None,
            expanded: false,
            started: Instant::now(),
            truncated: false,
            full_output_path: None,
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
        self.output_lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
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
        let clean = crate::ansi::strip_ansi(chunk)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let new_lines: Vec<&str> = clean.split('\n').collect();
        if let (Some(last), Some(first)) = (self.output_lines.back_mut(), new_lines.first()) {
            last.push_str(first);
            self.output_lines
                .extend(new_lines.iter().skip(1).map(|s| (*s).to_string()));
        } else {
            self.output_lines
                .extend(new_lines.iter().map(|s| (*s).to_string()));
        }
        // TUI-092 F6 — evict after EVERY chunk, so memory is bounded continuously through a
        // long-running command rather than only at completion. `drain` in one batch rather than a
        // `pop_front` loop: the replay feeder ([`crate::transcript::Transcript::push_bash_execution`])
        // hands the entire recorded output to a SINGLE call, where the excess can be thousands of
        // lines. Front-eviction never touches the back element, so the continuation-merge invariant
        // above survives it.
        let excess = self.output_lines.len().saturating_sub(MAX_OUTPUT_LINES);
        if excess > 0 {
            self.output_lines.drain(..excess);
            self.omitted_lines += excess;
        }
    }

    /// Mark the run finished (`setComplete`, `bash-execution.ts:98-117`).
    ///
    /// X17 — the classification is verbatim `bash-execution.ts:105-109`:
    /// ```text
    /// cancelled ? "cancelled"
    ///           : exitCode !== 0 && exitCode !== undefined && exitCode !== null ? "error"
    ///                                                                          : "complete"
    /// ```
    /// so an **absent** exit code — a signalled command, where `Option<i32>` is `None` — is
    /// `complete`, not `error`. cyrup tested `exit_code != Some(0)`, which swept `None` into the
    /// error arm and then rendered a bold red `  (exit ?)` row upstream never draws (there is no
    /// `?` fallback anywhere in `bash-execution.ts`; `:192` interpolates the number directly).
    ///
    /// X13 — upstream's signature has FOUR parameters, not two:
    /// `setComplete(exitCode, cancelled, truncationResult?, fullOutputPath?)` (`:98-103`), and both
    /// tail arguments are supplied at every call site (`interactive-mode.ts:6307-6312`,
    /// `:6348-6353`, `:3460-3465`). cyrup dropped them, so `"Output truncated"` appeared nowhere in
    /// the crate and a spooled `!` output was silently unreachable. See
    /// [`Self::render_lines_at`]'s status block for the row they drive.
    pub fn set_complete(
        &mut self,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    ) {
        self.exit_code = exit_code;
        self.status = if cancelled {
            BashStatus::Cancelled
        } else if exit_code.is_some_and(|c| c != 0) {
            BashStatus::Error
        } else {
            BashStatus::Complete
        };
        self.truncated = truncated;
        self.full_output_path = full_output_path;
    }

    /// X13 — `contextTruncation.truncated` (`bash-execution.ts:122-126`): whether the accumulated
    /// output on its own exceeds the LLM-context limits the component re-applies every frame,
    /// independent of whatever the executor reported.
    ///
    /// `truncateTail`'s own early-out is the whole predicate — `truncate.ts:177`
    /// `if (totalLines <= maxLines && totalBytes <= maxBytes)` returns `truncated: false` — so the
    /// boolean needs no tail walk, only the two totals. Byte length is UTF-8 (`Buffer.byteLength`),
    /// which is `str::len()` here.
    fn context_truncated(&self) -> bool {
        /// `truncate.ts:12` `DEFAULT_MAX_BYTES` (50 KB).
        const MAX_BYTES: usize = 50 * 1024;
        // TUI-092 F6 — ring eviction already means more logical lines existed than were kept: the
        // direct replacement for the old `self.output_lines.len() > MAX_LINES` check, which became
        // unreachable once `output_lines` is itself bounded to `MAX_OUTPUT_LINES` (= that
        // `MAX_LINES`, `truncate.ts:11` `DEFAULT_MAX_LINES`).
        if self.omitted_lines > 0 {
            return true;
        }
        // `\n`-joined, matching `getOutput`/`updateDisplay`'s `this.outputLines.join("\n")`.
        let bytes: usize = self.output_lines.iter().map(String::len).sum::<usize>()
            + self.output_lines.len().saturating_sub(1);
        bytes > MAX_BYTES
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
        self.render_lines_at(
            self.started.elapsed(),
            width,
            theme,
            cancel_hint,
            expand_hint,
        )
    }

    /// [`render_lines`](Self::render_lines) with the spinner phase supplied, so the animated frame
    /// is deterministic in tests (the same split [`crate::status_indicator::StatusIndicator`] uses).
    pub fn render_lines_at(
        &self,
        elapsed: Duration,
        width: usize,
        theme: &UiTheme,
        cancel_hint: Option<&str>,
        expand_hint: Option<&str>,
    ) -> Vec<Line<'static>> {
        // The BORDER color is dim for a `!!` (excluded-from-context) run, bash-green otherwise — set
        // once at construction and sticky (Pi bash-execution.ts:37-44,64). The `$ command` HEADER,
        // however, is **always** bash-green (Pi's `updateDisplay` header, bash-execution.ts:138, uses
        // `theme.fg("bashMode", …)` regardless of `excludeFromContext`) — item #5 "!! header green".
        let border_style = if self.excluded {
            theme.dim_style()
        } else {
            theme.bash_mode_style()
        };
        let header_style = theme.bash_mode_style().add_modifier(Modifier::BOLD);
        let rule = "─".repeat(width.max(1));
        let mut out: Vec<Line<'static>> = Vec::new();
        out.push(Line::default());
        out.push(Line::styled(rule.clone(), border_style));
        // X3 — every child of `contentContainer` is a `new Text(…, 1, 0)`: the header
        // (`bash-execution.ts:138`), the output (`:146`/`:150`) and the status block (`:202`), all
        // at **paddingX 1**. `Text.render` lays each row out as `leftMargin + line + rightMargin`
        // (`text.ts:70-76`), so the whole block sits one column in. cyrup had the header at column 0
        // and everything below it at column 2 — a two-column stagger upstream does not have.
        //
        // Routed through [`crate::transcript::text_lines_of`] (the `Text.render` port) rather than a
        // hand-written leading space: `text.ts:64` wraps at `contentWidth = width - paddingX * 2`
        // BEFORE `:70-76` prefixes the margin, so a command longer than the pane breaks at
        // `width - 2` and every produced row carries the inset. A literal `" $ …"` left row 2 of a
        // long command flush at column 0 with no right gutter — the same L2/M10 defect the message
        // body had.
        out.extend(crate::transcript::text_lines_of(
            &Line::from(Span::styled(format!("$ {}", self.command), header_style)),
            width,
            1,
        ));

        // Collapse to the trailing [`PREVIEW_LINES`] **visual** (wrap-aware) lines, exactly like Pi's
        // `truncateToVisualLines` (`visual-truncate.ts`): wrapping each logical line to the indented
        // body width so a single long line that wraps counts as multiple preview lines (`hidden` is
        // the number of hidden VISUAL lines). Expanded shows everything.
        //
        // X16 — the hidden count is computed OUTSIDE the expanded branch upstream
        // (`bash-execution.ts:131-132` runs before `:143`'s `if (this.expanded)`), which is what
        // makes the `(… to collapse)` hint reachable at all: an expanded block still knows how many
        // lines the collapsed form would have hidden. cyrup zeroed it when expanded, so the collapse
        // hint was dead code that could never render.
        let body_width = width.saturating_sub(2).max(1);
        let joined = self
            .output_lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        let vt = crate::chrome::truncate_to_visual_lines(&joined, PREVIEW_LINES, body_width);
        let hidden = vt.skipped;
        // `visible` must stay `Vec<String>`: the other arm is `vt.lines: Vec<String>`, so a
        // `VecDeque::clone()` would not unify (TUI-092 F6).
        let visible: Vec<String> = if self.expanded {
            self.output_lines.iter().cloned().collect()
        } else {
            vt.lines
        };
        if !visible.is_empty() || self.omitted_lines > 0 {
            // X3 — the output `Text` is constructed with a **leading newline**:
            // `new Text(`\n${displayText}`, 1, 0)` (`bash-execution.ts:146`), and the collapsed arm
            // feeds `truncateToVisualLines` the same `` `\n${styledOutput}` `` (`:150`, `:156`).
            // `wrapTextWithAnsi` splits on `\n` (`utils.ts:839`), so row 0 is empty — a blank row
            // between the command and its output that cyrup never emitted. ONE shared leading blank
            // covers the TUI-092 F6 omission notice and the retained window together: they are one
            // logical `Text` group.
            out.push(Line::default());
            if self.omitted_lines > 0 {
                // TUI-092 F6 — one dim row ahead of the retained window, in BOTH the collapsed and
                // expanded views (this branch runs regardless of `self.expanded`). Span-styled, not
                // `Line`-styled: [`crate::transcript::wrap_line`] rebuilds wrapped rows from their
                // cells' styles, so a `Line`-level style would survive only the un-wrapped case —
                // the same rule as the X13 warning row below.
                out.extend(crate::transcript::text_lines_of(
                    &Line::from(vec![Span::styled(
                        format!("… ({} earlier lines omitted) …", self.omitted_lines),
                        theme.dim_style(),
                    )]),
                    width,
                    1,
                ));
            }
            for line in &visible {
                // Same `new Text(…, 1, 0)` (`bash-execution.ts:146`/`:156`). The collapsed arm's rows
                // already fit `body_width` (`truncate_to_visual_lines` wrapped them there), so this
                // only adds the margin; the EXPANDED arm feeds raw logical lines and genuinely needs
                // the wrap — upstream's expanded branch is `new Text(`\n${displayText}`, 1, 0)`
                // (`:146`), which wraps at `width - 2` exactly like the collapsed one.
                out.extend(crate::transcript::text_lines_of(
                    &Line::from(Span::styled(line.clone(), theme.muted_style())),
                    width,
                    1,
                ));
            }
        }

        match self.status {
            BashStatus::Running => {
                // X4 — the running row is a `Loader` (`bash-execution.ts:55-60`), not a static
                // string. `Loader.render` is `["", ...super.render(width)]` (`loader.ts:44`), so it
                // carries its own leading blank row; `updateDisplay` (`:83-91`) builds
                // `` `${spinnerColorFn(frame)} ${messageColorFn(message)}` `` from `DEFAULT_FRAMES`
                // (`:11`), and the `Text` base is `super("", 1, 0)` (`:35`) — paddingX 1.
                //
                // The spinner colour is the block's own `colorKey` — `dim` for a `!!`
                // excluded-from-context run, `bashMode` otherwise (`bash-execution.ts:37`, `:57`) —
                // and the message is `theme.fg("muted", …)` (`:58`). cyrup drew no glyph at all, so
                // the block looked frozen for the entire command.
                out.push(Line::default());
                let hint = cancel_hint.unwrap_or("Esc");
                let spinner = StatusIndicator::spinner_at(elapsed);
                // `Loader`'s `Text` base is `super("", 1, 0)` (`loader.ts:35`) — paddingX 1 — so the
                // inset is `Text.render`'s `leftMargin`, a SEPARATE span concatenated ahead of the
                // row (`text.ts:70`, `:76`), not part of the styled spinner content.
                out.extend(crate::transcript::text_lines_of(
                    &Line::from(vec![
                        Span::styled(format!("{spinner} "), border_style),
                        Span::styled(
                            format!("Running... ({hint} to cancel)"),
                            theme.muted_style(),
                        ),
                    ]),
                    width,
                    1,
                ));
            }
            _ => {
                // The finished block's status rows are one `new Text(`\n${statusParts.join("\n")}`,
                // 1, 0)` (`bash-execution.ts:202`) — so ONE leading blank for the whole group, then
                // one row per part, each inset by 1.
                let mut parts: Vec<Line<'static>> = Vec::new();
                if hidden > 0 {
                    // X16 — verbatim `bash-execution.ts:178-186`. Collapsed:
                    // `fg("muted", `... ${n} more lines (`) + keyHint("app.tools.expand",
                    // "to expand") + fg("muted", ")")`. Expanded: `fg("muted", "(") + keyHint(…,
                    // "to collapse") + fg("muted", ")")`. `keyHint` is
                    // `fg("dim", keyText) + fg("muted", ` ${description}`)`
                    // (`keybinding-hints.ts:42-44`), which is where the two-tone split comes from.
                    // cyrup rendered `  Ctrl+O (12 more lines, to expand)` — the key first, the
                    // count inside the parens with the wrong word order, and the collapse form with
                    // no parens at all.
                    let key = expand_hint.unwrap_or("Ctrl+O");
                    let (lead, what) = if self.expanded {
                        ("(".to_string(), " to collapse")
                    } else {
                        (format!("... {hidden} more lines ("), " to expand")
                    };
                    parts.push(Line::from(vec![
                        Span::styled(lead, theme.muted_style()),
                        Span::styled(key.to_string(), theme.dim_style()),
                        Span::styled(what.to_string(), theme.muted_style()),
                        Span::styled(")".to_string(), theme.muted_style()),
                    ]));
                }
                match self.status {
                    BashStatus::Cancelled => {
                        parts.push(Line::styled(
                            "(cancelled)".to_string(),
                            theme.warning_style(),
                        ));
                    }
                    BashStatus::Error => {
                        // X17 — only reachable with a real non-zero code now (see `set_complete`);
                        // upstream interpolates `this.exitCode` directly (`:192`) and has no `?`
                        // fallback, because a `null`/`undefined` code never classifies as `error`.
                        if let Some(code) = self.exit_code {
                            parts.push(Line::styled(format!("(exit {code})"), theme.error_style()));
                        }
                    }
                    _ => {}
                }
                // X13 — the truncation warning, the LAST status part (`bash-execution.ts:195-199`):
                //
                // ```ts
                // const wasTruncated = this.truncationResult?.truncated || contextTruncation.truncated;
                // if (wasTruncated && this.fullOutputPath) {
                //     statusParts.push(theme.fg("warning", `Output truncated. Full output: ${this.fullOutputPath}`));
                // }
                // ```
                //
                // Both conditions matter: a truncated run with no spool path renders NOTHING (there
                // would be nowhere to send the user), which is why the row is gated on the path and
                // not just the flag. `contextTruncation` is the component's own
                // `truncateTail(fullOutput, {maxLines: DEFAULT_MAX_LINES, maxBytes: DEFAULT_MAX_BYTES})`
                // (`:122-126`) — the same 2000-line / 50 KB pair `truncate.ts:11-12` defines, applied
                // to what is on screen — so an executor that reported no truncation still trips the
                // warning once the block itself overflows those limits.
                if (self.truncated || self.context_truncated())
                    && let Some(path) = &self.full_output_path
                {
                    // The colour rides on the SPAN, not the `Line`: a spool path is long enough to
                    // wrap at `width - 2`, and `wrap_line` rebuilds each produced row from its
                    // cells' styles — a `Line`-level style would survive only the un-wrapped case.
                    parts.push(Line::from(vec![Span::styled(
                        format!("Output truncated. Full output: {path}"),
                        theme.warning_style(),
                    )]));
                }
                if !parts.is_empty() {
                    out.push(Line::default());
                    // One `new Text(`\n${statusParts.join("\n")}`, 1, 0)` for the whole group
                    // (`bash-execution.ts:202`): `wrapTextWithAnsi` splits it back on `\n`
                    // (`utils.ts:839`) and `text.ts:64`/`:70-76` wrap each piece at `width - 2` and
                    // margin every produced row. X13's truncation-warning part carries a full
                    // filesystem path and is the one that actually needs the wrap.
                    for part in parts {
                        out.extend(crate::transcript::text_lines_of(&part, width, 1));
                    }
                }
            }
        }
        out.push(Line::styled(rule, border_style));
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
    use ratatui::style::Style;

    use super::*;

    fn plain(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
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
        ok.set_complete(Some(0), false, false, None);
        assert_eq!(ok.status(), BashStatus::Complete);

        let mut err = BashExecution::new("false", false);
        err.set_complete(Some(1), false, false, None);
        assert_eq!(err.status(), BashStatus::Error);

        let mut cancelled = BashExecution::new("sleep 10", false);
        cancelled.set_complete(None, true, false, None);
        assert_eq!(cancelled.status(), BashStatus::Cancelled);
    }

    #[test]
    fn render_shows_header_and_running_hint() {
        let theme = UiTheme::dark();
        let b = BashExecution::new("ls -la", false);
        let lines = b.render_lines(40, &theme, Some("Esc"), Some("Ctrl+O"));
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(
            text.iter().any(|l| l.contains("$ ls -la")),
            "header: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains("Running...") && l.contains("Esc")),
            "{text:?}"
        );
    }

    /// X17 — `bash-execution.ts:105-109` classifies as `"error"` only when
    /// `exitCode !== 0 && exitCode !== undefined && exitCode !== null`, so a **signalled** command
    /// (no exit code) is `"complete"` and draws no `(exit …)` row at all. There is no `?` fallback
    /// anywhere in the component: `:192` interpolates `this.exitCode` directly.
    #[test]
    fn signalled_command_is_complete_not_error_and_draws_no_exit_badge() {
        let theme = UiTheme::dark();
        let mut sig = BashExecution::new("sleep 10", false);
        sig.append_output("partial");
        sig.set_complete(None, false, false, None);
        assert_eq!(
            sig.status(),
            BashStatus::Complete,
            "a missing exit code is not an error"
        );
        let text: Vec<String> = sig
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert!(
            !text.iter().any(|l| l.contains("exit")),
            "invented exit badge: {text:?}"
        );

        // MIRROR: a real non-zero code still classifies as an error and still renders `(exit N)`
        // with the number, inset one column like every other row of the block.
        let mut err = BashExecution::new("false", false);
        err.set_complete(Some(3), false, false, None);
        assert_eq!(err.status(), BashStatus::Error);
        let etext: Vec<String> = err
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert!(etext.iter().any(|l| l == " (exit 3)"), "{etext:?}");
    }

    /// X3 — every child of `contentContainer` is a `new Text(…, 1, 0)`: the header
    /// (`bash-execution.ts:138`), the output (`:146`) and the status block (`:202`). `Text.render`
    /// emits `leftMargin + line + rightMargin` (`text.ts:70-76`), so the whole block is inset by
    /// exactly one column — cyrup had the header at 0 and everything under it at 2.
    ///
    /// X3 — the output and the status block are both built with a **leading newline**, which
    /// `wrapTextWithAnsi` turns into an empty first row (`utils.ts:839`).
    #[test]
    fn block_body_is_inset_one_column_with_blank_rows_before_output_and_status() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("ls", false);
        b.append_output("alpha\nbeta");
        b.set_complete(Some(7), false, false, None);
        let text: Vec<String> = b
            .render_lines(40, &theme, None, Some("Ctrl+O"))
            .iter()
            .map(plain)
            .collect();
        // [spacer, rule, header, blank, out, out, blank, (exit 7), rule]
        assert_eq!(
            text,
            vec![
                "".to_string(),
                "─".repeat(40),
                " $ ls".to_string(),
                String::new(),
                " alpha".to_string(),
                " beta".to_string(),
                String::new(),
                " (exit 7)".to_string(),
                "─".repeat(40),
            ]
        );

        // MIRROR: with NO output there is no output blank — `bash-execution.ts:142` gates the whole
        // output `Text` on `availableLines.length > 0`.
        let mut empty = BashExecution::new("true", false);
        empty.set_complete(Some(0), false, false, None);
        let etext: Vec<String> = empty
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            etext,
            vec![
                "".to_string(),
                "─".repeat(40),
                " $ true".to_string(),
                "─".repeat(40)
            ]
        );
    }

    /// X4 — the running row is a `Loader` (`bash-execution.ts:55-61`), not a static string.
    /// `Loader.render` is `["", ...super.render(width)]` (`loader.ts:44`) so it carries its own
    /// leading blank; `updateDisplay` (`:83-91`) prefixes `${spinnerColorFn(frame)} ` from
    /// `DEFAULT_FRAMES` (`:11`); the message is `theme.fg("muted", …)` (`bash-execution.ts:58`) and
    /// the spinner takes the block's `colorKey` (`:37`, `:57`).
    #[test]
    fn running_row_draws_a_spinner_glyph_inset_one_column() {
        use crate::status_indicator::SPINNER_FRAMES;
        let theme = UiTheme::dark();
        let b = BashExecution::new("sleep 1", false);
        let lines = b.render_lines_at(Duration::from_millis(0), 40, &theme, Some("escape"), None);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(
            text[3], "",
            "the Loader's own leading blank row (loader.ts:44)"
        );
        assert_eq!(text[4], " ⠋ Running... (escape to cancel)");
        let row = &lines[4];
        // `Text.render` builds the row as `leftMargin + line + rightMargin` (`text.ts:70`, `:76`) —
        // the inset is a SEPARATE unstyled string concatenated ahead of the styled content, not part
        // of the spinner's own span. So span 0 is the margin and the `Loader`'s two-tone
        // `${spinnerColorFn(frame)} ${messageColorFn(message)}` (`loader.ts:86`) starts at span 1.
        assert_eq!(
            row.spans[0].content.as_ref(),
            " ",
            "text.ts:70 leftMargin, unstyled"
        );
        assert_eq!(row.spans[0].style, Style::default());
        assert_eq!(row.spans[1].content.as_ref(), "⠋ ");
        assert_eq!(
            row.spans[1].style,
            theme.bash_mode_style(),
            "spinner takes `colorKey`"
        );
        assert_eq!(
            row.spans[2].style,
            theme.muted_style(),
            "message is `muted`"
        );

        // The glyph advances with elapsed time (`loader.ts:77-80`'s setInterval, re-derived here).
        let later = b.render_lines_at(Duration::from_millis(240), 40, &theme, Some("escape"), None);
        assert_eq!(plain(&later[4]), " ⠸ Running... (escape to cancel)");
        assert!(SPINNER_FRAMES.contains(&"⠸"));

        // MIRROR: a `!!` (excluded-from-context) run colours its spinner `dim`, the same `colorKey`
        // its border uses (`bash-execution.ts:37`).
        let excluded = BashExecution::new("secret", true);
        let el = excluded.render_lines_at(Duration::from_millis(0), 40, &theme, None, None);
        assert_eq!(el[4].spans[1].style, theme.dim_style());
    }

    /// X16 — `bash-execution.ts:178-186`, verbatim. Collapsed is
    /// `fg("muted", `... ${n} more lines (`) + keyHint("app.tools.expand", "to expand") +
    /// fg("muted", ")")`; expanded is `fg("muted", "(") + keyHint(…, "to collapse") +
    /// fg("muted", ")")`. `keyHint` splits into `fg("dim", keyText)` + `fg("muted", " desc")`
    /// (`keybinding-hints.ts:42-44`).
    #[test]
    fn expand_and_collapse_hints_match_upstream_wording() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("seq 30", false);
        for i in 1..=30 {
            b.append_output(&format!("line{i}\n"));
        }
        b.set_complete(Some(0), false, false, None);

        let lines = b.render_lines(40, &theme, None, Some("ctrl+o"));
        let hint = lines
            .iter()
            .find(|l| plain(l).contains("more lines"))
            .unwrap();
        assert_eq!(plain(hint), " ... 11 more lines (ctrl+o to expand)");
        // Span 0 is `Text.render`'s `leftMargin` (`text.ts:70`), unstyled; the three `keyHint` /
        // `fg("muted", …)` pieces of `bash-execution.ts:178-186` follow it at 1/2/3.
        assert_eq!(hint.spans[0].content.as_ref(), " ");
        assert_eq!(hint.spans[0].style, Style::default());
        assert_eq!(hint.spans[1].style, theme.muted_style());
        assert_eq!(hint.spans[2].content.as_ref(), "ctrl+o");
        assert_eq!(
            hint.spans[2].style,
            theme.dim_style(),
            "keyHint's key half is `dim`"
        );
        assert_eq!(
            hint.spans[3].style,
            theme.muted_style(),
            "keyHint's description half is `muted`"
        );

        // MIRROR: the expanded form keeps its parentheses and drops the count.
        let mut e = b.clone();
        e.set_expanded(true);
        let el = e.render_lines(40, &theme, None, Some("ctrl+o"));
        let ehint = el.iter().find(|l| plain(l).contains("collapse")).unwrap();
        assert_eq!(plain(ehint), " (ctrl+o to collapse)");
    }

    #[test]
    fn collapsed_preview_truncates_and_counts_hidden() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("seq 30", false);
        for i in 1..=30 {
            b.append_output(&format!("line{i}\n"));
        }
        b.set_complete(Some(0), false, false, None);
        let lines = b.render_lines(40, &theme, None, Some("Ctrl+O"));
        let text: Vec<String> = lines.iter().map(plain).collect();
        // 30 output lines + a trailing empty (from the final "\n") → preview keeps the last 20.
        assert!(
            text.iter().any(|l| l.contains("line30")),
            "tail shown: {text:?}"
        );
        assert!(
            !text.iter().any(|l| l.trim() == "line1"),
            "first line hidden: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains("11 more lines") && l.contains("Ctrl+O")),
            "hidden count + expand hint: {text:?}"
        );

        // Expanded shows everything.
        let mut e = b.clone();
        e.set_expanded(true);
        let etext: Vec<String> = e
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert!(
            etext.iter().any(|l| l.contains("line1")),
            "expanded shows first line: {etext:?}"
        );
    }

    /// X3's right-margin half — every child of `contentContainer` is a `new Text(…, 1, 0)`
    /// (`bash-execution.ts:138` header, `:146`/`:156` output, `:202` status), and `Text.render`
    /// WRAPS at `contentWidth = width - paddingX * 2` (`text.ts:64`) BEFORE prefixing `leftMargin`
    /// to each produced row (`:70-76`).
    ///
    /// cyrup hand-wrote a single leading space onto an unwrapped logical line, so a command or an
    /// output line longer than the pane put row 0 at column 1, every continuation row at column 0
    /// (once the outer `Paragraph::wrap` got to it) and nothing in the last column.
    #[test]
    fn long_bash_rows_wrap_inside_the_one_column_inset() {
        let theme = UiTheme::dark();
        let long_cmd = "grep --recursive --line-number --binary-files=without-match needle ./src";
        let mut b = BashExecution::new(long_cmd, false);
        b.append_output(
            "a very long line of program output that certainly does not fit in thirty\n",
        );
        b.set_complete(Some(0), false, false, None);
        let lines = b.render_lines(30, &theme, None, None);
        let text: Vec<String> = lines.iter().map(plain).collect();

        // The header wrapped: more than one row mentions the command.
        let header_rows: Vec<&String> = text
            .iter()
            .filter(|r| r.contains("grep") || r.contains("without-match"))
            .collect();
        assert!(header_rows.len() > 1, "header did not wrap: {text:?}");
        // The output wrapped too.
        assert!(
            text.iter().any(|r| r.contains("thirty")),
            "output missing: {text:?}"
        );
        assert!(
            text.iter()
                .filter(|r| r.contains("long line") || r.contains("certainly"))
                .count()
                >= 1
        );

        for (i, row) in lines.iter().enumerate() {
            let t = &text[i];
            // The two `DynamicBorder` rules are full-width by design (`dynamic-border.ts`), and the
            // blank spacer rows are empty; every CONTENT row is inset and gutter-ed.
            if t.trim().is_empty() || t.starts_with('\u{2500}') {
                continue;
            }
            assert!(t.starts_with(' '), "row {i} lost its leftMargin: {t:?}");
            assert!(!t.starts_with("  "), "row {i} over-indented: {t:?}");
            assert!(
                row.width() <= 29,
                "row {i} has no right gutter: {t:?} ({})",
                row.width()
            );
        }

        // MIRROR: a short command still renders as exactly one inset header row.
        let mut short = BashExecution::new("true", false);
        short.set_complete(Some(0), false, false, None);
        let stext: Vec<String> = short
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(stext[2], " $ true", "{stext:?}");
    }

    /// **The bash status-part WRAP.** `out.extend(parts)` → `for part in parts {
    /// out.extend(text_lines_of(&part, width, 1)) }` bundles TWO behaviours — the `leftMargin` and
    /// the wrap — and `long_bash_rows_wrap_inside_the_one_column_inset` only covers the margin,
    /// because at the widths it uses no status part is long enough to break. A version that adds
    /// the inset and never wraps leaves the whole suite green; this is the assertion that fails.
    ///
    /// The status rows are ONE `new Text(`\n${statusParts.join("\n")}`, 1, 0)`
    /// (`bash-execution.ts:202`). `Text.render` wraps that string at
    /// `contentWidth = Math.max(1, width - paddingX * 2)` (`text.ts:64`) and only then prefixes
    /// `leftMargin` to each produced row (`:70-76`), so a part longer than `width - 2` becomes
    /// several rows and every one of them is inset.
    #[test]
    fn a_long_bash_status_part_wraps_inside_its_inset() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("find /", false);
        for i in 0..40 {
            b.append_output(&format!("line{i}\n"));
        }
        b.set_complete(Some(0), false, false, None);

        // `... 21 more lines (ctrl+shift+o to expand)` is 42 cells; a 30-column block gives the
        // status `Text` a `contentWidth` of 28, so upstream breaks it in two — after `lines`, since
        // `(ctrl+shift+o` is one 13-cell token and 18 + 13 > 28. The first piece is `trimEnd`ed
        // (`utils.ts:934`) and the second never starts with whitespace (`:912-915`).
        let lines = b.render_lines(30, &theme, None, Some("ctrl+shift+o"));
        let text: Vec<String> = lines.iter().map(plain).collect();
        let hint: Vec<(usize, &String)> = text
            .iter()
            .enumerate()
            .filter(|(_, r)| r.contains("more lines") || r.contains("to expand"))
            .collect();
        assert_eq!(
            hint.len(),
            2,
            "the status part did not WRAP, only got a margin: {text:?}"
        );
        assert_eq!(
            hint[0].0 + 1,
            hint[1].0,
            "the two halves are not adjacent rows: {text:?}"
        );
        assert_eq!(
            hint[0].1.as_str(),
            " ... 21 more lines",
            "first half: {text:?}"
        );
        assert_eq!(
            hint[1].1.as_str(),
            " (ctrl+shift+o to expand)",
            "second half: {text:?}"
        );
        for (i, row) in lines.iter().enumerate() {
            let t = &text[i];
            if t.trim().is_empty() || t.starts_with('\u{2500}') {
                continue;
            }
            assert!(t.starts_with(' '), "row {i} lost its leftMargin: {t:?}");
            assert!(!t.starts_with("  "), "row {i} over-indented: {t:?}");
            assert!(
                row.width() <= 29,
                "row {i} has no right gutter: {t:?} ({})",
                row.width()
            );
        }
        // The two-tone `keyHint` split survives the wrap: the key half stays `dim`, the rest `muted`
        // (`keybinding-hints.ts:42-44`), which a re-styling wrap would flatten.
        let key_row = &lines[hint[1].0];
        assert!(
            key_row
                .spans
                .iter()
                .any(|s| s.content.contains("ctrl+shift+o") && s.style.fg == theme.dim_style().fg),
            "the key half lost `dim` across the wrap: {:?}",
            text
        );

        // MIRROR — a status part that FITS is still exactly one inset row, unwrapped.
        let mut short = BashExecution::new("false", false);
        short.set_complete(Some(1), false, false, None);
        let stext: Vec<String> = short
            .render_lines(40, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            stext.iter().filter(|r| r.contains("exit 1")).count(),
            1,
            "a fitting part must not split: {stext:?}"
        );
        assert!(stext.iter().any(|r| r == " (exit 1)"), "{stext:?}");
    }

    #[test]
    fn excluded_uses_dim_border_but_green_header() {
        let theme = UiTheme::dark();
        let b = BashExecution::new("secret", true);
        assert!(b.excluded());
        let lines = b.render_lines(20, &theme, None, None);
        // Top border (line index 1, after the spacer) carries the dim style for `!!`.
        assert_eq!(lines[1].style, theme.dim_style());
        // The `$ command` header (line index 2), however, is ALWAYS bash-green + bold even for a `!!`
        // excluded run (Pi `updateDisplay` header, bash-execution.ts:138) — item #5 "!! header green".
        // The colour now rides on the SPAN, not the `Line`: the header row is `leftMargin + line`
        // (`text.ts:70`, `:76`), i.e. an unstyled 1-column margin followed by the styled `$ command`.
        let header_style = theme.bash_mode_style().add_modifier(Modifier::BOLD);
        assert_eq!(lines[2].spans[0].content.as_ref(), " ");
        assert_eq!(lines[2].spans[0].style, Style::default());
        assert_eq!(
            lines[2].spans[1].style, header_style,
            "!! header must stay bash-green, not dim"
        );
        assert!(plain(&lines[2]).contains("$ secret"));
        // And a `!` (included) run's border matches the same green header.
        let inc = BashExecution::new("secret", false);
        let inc_lines = inc.render_lines(20, &theme, None, None);
        assert_eq!(inc_lines[2].spans[1].style, header_style);
    }

    /// **X13 — the truncation warning row (`bash-execution.ts:195-199`).**
    ///
    /// ```ts
    /// const wasTruncated = this.truncationResult?.truncated || contextTruncation.truncated;
    /// if (wasTruncated && this.fullOutputPath) {
    ///     statusParts.push(theme.fg("warning", `Output truncated. Full output: ${this.fullOutputPath}`));
    /// }
    /// ```
    #[test]
    fn x13_truncated_output_names_the_spool_file() {
        let theme = UiTheme::dark();
        let mut b = BashExecution::new("gen", false);
        b.append_output("a\nb\n");
        b.set_complete(Some(0), false, true, Some("/tmp/pi-bash-1.log".to_string()));
        let lines = b.render_lines(80, &theme, None, None);
        let row = lines
            .iter()
            .find(|l| plain(l).contains("Output truncated"))
            .unwrap_or_else(|| {
                panic!(
                    "no warning row in {:?}",
                    lines.iter().map(plain).collect::<Vec<_>>()
                )
            });
        assert!(plain(row).contains("Output truncated. Full output: /tmp/pi-bash-1.log"));
        assert_eq!(
            row.spans[1].style,
            theme.warning_style(),
            "`theme.fg(\"warning\", …)`"
        );

        // MIRROR 1: truncated but with NO spool path renders nothing — upstream's `&& this.fullOutputPath`
        // guard, because there would be nowhere to point the user.
        let mut no_path = BashExecution::new("gen", false);
        no_path.append_output("a\nb\n");
        no_path.set_complete(Some(0), false, true, None);
        assert!(
            !no_path
                .render_lines(80, &theme, None, None)
                .iter()
                .any(|l| plain(l).contains("truncated")),
            "no path ⇒ no row"
        );

        // MIRROR 2: a path with NO truncation renders nothing either.
        let mut untruncated = BashExecution::new("gen", false);
        untruncated.append_output("a\nb\n");
        untruncated.set_complete(Some(0), false, false, Some("/tmp/x.log".to_string()));
        assert!(
            !untruncated
                .render_lines(80, &theme, None, None)
                .iter()
                .any(|l| plain(l).contains("truncated")),
            "not truncated ⇒ no row"
        );

        // MIRROR 3: the component's OWN `contextTruncation` (`:122-126`) trips it even when the
        // executor reported `truncated: false` — 2001 lines is past `DEFAULT_MAX_LINES`.
        let mut ctx = BashExecution::new("gen", false);
        ctx.append_output(&"x\n".repeat(2001));
        ctx.set_complete(Some(0), false, false, Some("/tmp/x.log".to_string()));
        assert!(
            ctx.render_lines(80, &theme, None, None)
                .iter()
                .any(|l| plain(l).contains("Output truncated")),
            "contextTruncation.truncated is the second leg of `wasTruncated`"
        );

        // MIRROR 4: the warning is the LAST status part, after `(exit N)` (`:189-199` order).
        let mut failed = BashExecution::new("gen", false);
        failed.append_output("a\n");
        failed.set_complete(Some(2), false, true, Some("/tmp/x.log".to_string()));
        let rows: Vec<String> = failed
            .render_lines(80, &theme, None, None)
            .iter()
            .map(plain)
            .collect();
        let exit = rows.iter().position(|r| r.contains("(exit 2)")).unwrap();
        let warn = rows
            .iter()
            .position(|r| r.contains("Output truncated"))
            .unwrap();
        assert!(
            exit < warn,
            "exit code first, truncation warning last: {rows:?}"
        );
    }
}
