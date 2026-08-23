//! The `ctrl+f` interactive status overlay — the cyrup-native restoration of Wibey's
//! `ui-mode: flux-status` panel (port doc §3.4.3). `/flux/status` ([`crate::render_status`],
//! FLUX_07) owns the plain-text channel; this module owns real COLOUR, which can only exist
//! inside an overlay because the TUI strips ANSI from externally supplied text
//! (`crates/cyrup-tui/src/ansi.rs`). The overlay draws styled [`OverlaySpan`]s the host paints
//! natively, so nothing is stripped.
//!
//! Reuses [`crate::state`]'s data model and [`crate::render_status`]'s layout arithmetic
//! (`name_w`, `stage_w`, the section pad/floor, the fixed severity column widths, the section
//! order TODO -> COMPLETED -> REVIEW) verbatim; only the OUTPUT shape differs — [`OverlayLine`]s
//! of styled [`OverlaySpan`]s instead of one padded `String`.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use cyrup_ext::host::{HostServices, InteractiveOverlay, NotifyKind, OverlayColor, OverlayKey,
                      OverlayKeyCode, OverlayLine, OverlayOutcome, OverlaySpan};

use crate::state::{self, DoneGroup};

const STAGE_W: usize = 8;
const SECTION_PAD: usize = 18;
const MIN_PANEL_W: usize = 48;

fn sev_col_width(sev: &str) -> usize {
    match sev {
        "critical" => 10,
        "high" => 6,
        "medium" => 8,
        "low" => 5,
        _ => 0,
    }
}

/// The status colour palette (port doc table, FLUX_09 SUBTASK 1), mapped from
/// `flux_status.py`'s ANSI codes onto the 16-colour [`OverlayColor`] enum: the two 256-colour
/// values collapse (`ORANGE` -> `Yellow`, `TEAL` -> `Cyan`).
fn status_style(status: &str) -> (Option<&'static str>, Option<OverlayColor>) {
    match status {
        "in-progress" => (Some("\u{1F504}"), Some(OverlayColor::Yellow)),
        "needs-rework" => (Some("\u{1F501}"), Some(OverlayColor::Red)),
        "done" | "completed" => (Some("\u{2705}"), Some(OverlayColor::Green)),
        _ => (None, None),
    }
}

fn severity_color(sev: &str) -> OverlayColor {
    match sev {
        "critical" => OverlayColor::Red,
        "high" => OverlayColor::Yellow,
        "medium" => OverlayColor::Cyan,
        _ => OverlayColor::Green, // "low" and any other value
    }
}

/// A snapshot of everything the overlay draws, re-collected on open and on every `tick` that
/// finds a change. Plain `PartialEq`-able data (no styling) so `tick` can compare frames cheaply.
#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    todos: Vec<(String, String, String)>,
    done_groups: Vec<DoneGroup>,
    reviews: Vec<(String, String)>,
}

impl Snapshot {
    fn collect(base: &std::path::Path) -> Self {
        Self {
            todos: state::collect_todos(base),
            done_groups: state::collect_done(base),
            reviews: state::collect_reviews(base),
        }
    }
}

/// The `ctrl+f` overlay: [`InteractiveOverlay`] over the same `~/.flux/<flattened-cwd>/` model
/// [`crate::render_status`] renders as plain text.
pub struct FluxStatusOverlay {
    base: PathBuf,
    snapshot: Snapshot,
}

impl FluxStatusOverlay {
    /// Collect the initial snapshot for the current working directory's flux base
    /// ([`state::derive_base`]) and construct the overlay.
    #[must_use]
    pub fn new() -> Self {
        let base = state::derive_base();
        let snapshot = Snapshot::collect(&base);
        Self { base, snapshot }
    }

    fn name_w(&self) -> usize {
        let mut longest = "TODO-FILE".chars().count();
        for (n, _, _) in &self.snapshot.todos {
            longest = longest.max(n.chars().count());
        }
        for (_, rows) in &self.snapshot.done_groups {
            for (n, _, _) in rows {
                longest = longest.max(n.chars().count());
            }
        }
        for (n, _) in &self.snapshot.reviews {
            longest = longest.max(n.chars().count());
        }
        (longest + 2).min(50)
    }

    fn total_w(&self) -> usize {
        (self.name_w() + STAGE_W + SECTION_PAD).max(MIN_PANEL_W)
    }

    fn push_rule(lines: &mut Vec<OverlayLine>, width: usize, ch: char) {
        lines.push(OverlayLine::new(vec![OverlaySpan {
            text: ch.to_string().repeat(width),
            fg: Some(OverlayColor::Magenta),
            bold: true,
            ..OverlaySpan::default()
        }]));
    }

    fn push_dim_rule(lines: &mut Vec<OverlayLine>, width: usize, ch: char) {
        lines.push(OverlayLine::new(vec![OverlaySpan {
            text: ch.to_string().repeat(width),
            dim: true,
            ..OverlaySpan::default()
        }]));
    }

    fn status_span(status: &str) -> OverlaySpan {
        if status.is_empty() {
            return OverlaySpan { text: "(unknown)".to_string(), dim: true, ..OverlaySpan::default() };
        }
        let (icon, color) = status_style(status);
        let text = match icon {
            Some(icon) => format!("{icon}  {status}"),
            None => status.to_string(),
        };
        OverlaySpan { text, fg: color, ..OverlaySpan::default() }
    }

    fn row_line(name: &str, stage: &str, status: &str, name_w: usize) -> OverlayLine {
        let mut spans = vec![
            OverlaySpan { text: ljust(name, name_w), fg: Some(OverlayColor::Cyan), ..OverlaySpan::default() },
            OverlaySpan::raw(ljust(stage, STAGE_W)),
        ];
        spans.push(Self::status_span(status));
        OverlayLine::new(spans)
    }
}

impl Default for FluxStatusOverlay {
    fn default() -> Self {
        Self::new()
    }
}

fn ljust(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = String::with_capacity(width);
        out.push_str(s);
        for _ in 0..(width - len) {
            out.push(' ');
        }
        out
    }
}

impl InteractiveOverlay for FluxStatusOverlay {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        let name_w = self.name_w();
        let total_w = self.total_w();
        let mut lines: Vec<OverlayLine> = Vec::new();

        // Header: the plain-text panel's title plus an ESC hint — the overlay has no host frame
        // title of its own (`overlay.rs:286-288`).
        lines.push(OverlayLine::new(vec![
            OverlaySpan {
                text: "\u{1D571} FLUX STATUS".to_string(),
                fg: Some(OverlayColor::Magenta),
                bold: true,
                ..OverlaySpan::default()
            },
            OverlaySpan { text: "   (ESC to close)".to_string(), dim: true, ..OverlaySpan::default() },
        ]));
        Self::push_rule(&mut lines, total_w, '\u{2550}');

        // --- TODO section ------------------------------------------------
        let rendered_any = true; // TODO always renders (even as "(no todos)")
        lines.push(OverlayLine::default());
        lines.push(OverlayLine::new(vec![
            OverlaySpan { text: ljust("TODO-FILE", name_w), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
            OverlaySpan { text: ljust("STAGE", STAGE_W), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
            OverlaySpan { text: "STATUS".to_string(), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
        ]));
        Self::push_dim_rule(&mut lines, total_w, '\u{2500}');
        if self.snapshot.todos.is_empty() {
            lines.push(OverlayLine::new(vec![OverlaySpan {
                text: "(no todos)".to_string(),
                dim: true,
                ..OverlaySpan::default()
            }]));
        }
        for (name, stage, status) in self.snapshot.todos.clone() {
            lines.push(Self::row_line(&name, &stage, &status, name_w));
        }

        // --- COMPLETED section --------------------------------------------
        if !self.snapshot.done_groups.is_empty() {
            lines.push(OverlayLine::default());
            if rendered_any {
                Self::push_rule(&mut lines, total_w, '\u{2550}');
            }
            lines.push(OverlayLine::new(vec![OverlaySpan {
                text: "COMPLETED TASKS".to_string(),
                fg: Some(OverlayColor::Magenta),
                bold: true,
                ..OverlaySpan::default()
            }]));
            lines.push(OverlayLine::default());
            lines.push(OverlayLine::new(vec![
                OverlaySpan { text: ljust("TASK-FILE", name_w), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
                OverlaySpan { text: ljust("STAGE", STAGE_W), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
                OverlaySpan { text: "STATUS".to_string(), fg: Some(OverlayColor::White), bold: true, ..OverlaySpan::default() },
            ]));
            for (ts_label, rows) in self.snapshot.done_groups.clone() {
                lines.push(OverlayLine::new(vec![OverlaySpan {
                    text: format!("\u{2500}\u{2500} {ts_label} \u{2500}\u{2500}"),
                    dim: true,
                    ..OverlaySpan::default()
                }]));
                for (name, stage, status) in rows {
                    lines.push(Self::row_line(&name, &stage, &status, name_w));
                }
            }
        }

        // --- REVIEW section ------------------------------------------------
        if !self.snapshot.reviews.is_empty() {
            lines.push(OverlayLine::default());
            if rendered_any {
                Self::push_rule(&mut lines, total_w, '\u{2550}');
            }
            lines.push(OverlayLine::new(vec![OverlaySpan {
                text: "REVIEW TASKS".to_string(),
                fg: Some(OverlayColor::Yellow),
                bold: true,
                ..OverlaySpan::default()
            }]));
            lines.push(OverlayLine::default());
            let mut head = vec![OverlaySpan {
                text: ljust("REVIEW-FILE", name_w),
                fg: Some(OverlayColor::White),
                bold: true,
                ..OverlaySpan::default()
            }];
            for sev in state::SEVERITIES {
                head.push(OverlaySpan {
                    text: ljust(&sev.to_uppercase(), sev_col_width(sev)),
                    fg: Some(severity_color(sev)),
                    bold: true,
                    ..OverlaySpan::default()
                });
            }
            lines.push(OverlayLine::new(head));
            let review_w = (name_w
                + state::SEVERITIES.iter().map(|s| sev_col_width(s)).sum::<usize>())
                .max(MIN_PANEL_W);
            Self::push_dim_rule(&mut lines, review_w, '\u{2500}');
            for (name, sev) in self.snapshot.reviews.clone() {
                let mut spans = vec![OverlaySpan {
                    text: ljust(&name, name_w),
                    fg: Some(OverlayColor::Cyan),
                    ..OverlaySpan::default()
                }];
                for col in state::SEVERITIES {
                    let w = sev_col_width(col);
                    if col == sev {
                        let mut text = String::from("\u{25CF}");
                        for _ in 0..w.saturating_sub(1) {
                            text.push(' ');
                        }
                        spans.push(OverlaySpan {
                            text,
                            fg: Some(severity_color(&sev)),
                            ..OverlaySpan::default()
                        });
                    } else {
                        spans.push(OverlaySpan::raw(" ".repeat(w)));
                    }
                }
                lines.push(OverlayLine::new(spans));
            }
        }

        lines.push(OverlayLine::default());
        Self::push_rule(&mut lines, total_w, '\u{2550}');
        lines
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        match key.code {
            OverlayKeyCode::Escape => OverlayOutcome::Close,
            _ => OverlayOutcome::Ignored,
        }
    }

    /// Track exec/qa frontmatter transitions live (port doc SUBTASK 1).
    fn refresh_ms(&self) -> u64 {
        2000
    }

    /// Re-collect and repaint only when the model actually changed — returning `true`
    /// unconditionally would make the host repaint twice a second forever.
    fn tick(&mut self) -> bool {
        let fresh = Snapshot::collect(&self.base);
        if fresh == self.snapshot {
            false
        } else {
            self.snapshot = fresh;
            true
        }
    }
}

/// Open the `ctrl+f` overlay, or fall back to the plain-text table when no interactive surface is
/// attached. `host_services` is the `OnceLock` slot [`crate::extension::FluxExtension`] binds
/// through `set_host_services` (`native.rs:683`).
pub fn open_status_overlay(host_services: &Arc<OnceLock<Arc<dyn HostServices>>>) {
    let Some(host) = host_services.get() else {
        // No host bound at all (headless print/json, default host, or `set_host_services` never
        // called): `notify` is unavailable too, so there is nowhere to hand the plain table. This
        // is the documented "no interactive surface" case with no notification channel either —
        // degrade quietly rather than panic; there is no caller to report to.
        return;
    };
    if !host.open_overlay(Box::new(FluxStatusOverlay::new())) {
        // A `false` return is NOT an error (`services.rs:248-252`) — the caller's cue to fall
        // back to non-interactive rendering. Fall back to the plain table via the same channel
        // `/flux/status`'s own invalid-input path uses.
        let base = state::derive_base();
        let table = crate::render_status::render(&base, true, true, true);
        host.notify(&table, NotifyKind::Info);
    }
}
