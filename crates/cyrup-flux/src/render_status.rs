//! `/flux/status` plain-text panel — a function-for-function Rust port of
//! [`flux_status.py`](../../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)'s
//! `render()` (`:180-267`) with `--no-color` semantics: the TUI strips ANSI from externally
//! supplied text (`crates/cyrup-tui/src/ansi.rs`), so this always returns the Python's
//! `--no-color` output — aligned columns plus the Unicode glyphs that carry the semantics colour
//! carried upstream. Colour lives only in FLUX_09's overlay, which draws ratatui lines natively.
//!
//! Padding is CHAR-COUNT based (`chars().count()`, matching Python's `len()` on `str`, i.e. code
//! points) — not byte length, not display width.

use std::path::Path;

use crate::state;

/// Section name width: fixed per `flux_status.py:192`.
const STAGE_W: usize = 8;
/// Extra width added to `name_w + stage_w` for the main section rule (`flux_status.py:70`).
const SECTION_PAD: usize = 18;
/// Floor so short content still frames nicely (`flux_status.py:71`).
const MIN_PANEL_W: usize = 48;

/// Fixed review-grid column widths, keyed by severity (`flux_status.py:66`).
fn sev_col_width(sev: &str) -> usize {
    match sev {
        "critical" => 10,
        "high" => 6,
        "medium" => 8,
        "low" => 5,
        _ => 0,
    }
}

/// Char-count based left-justify, matching Python's `str.ljust` over `len()` (code points, not
/// display columns or bytes).
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

/// The icon for a known status, or `None` for anything else (`STATUS_STYLE`, `flux_status.py:47-52`).
fn status_icon(status: &str) -> Option<&'static str> {
    match status {
        "in-progress" => Some("\u{1F504}"),
        "needs-rework" => Some("\u{1F501}"),
        "done" | "completed" => Some("\u{2705}"),
        _ => None,
    }
}

/// `status_cell` (`flux_status.py:115-118`): empty status -> `(unknown)`; known status ->
/// `"{icon}  {status}"`; unknown non-empty status -> the status text alone.
fn status_cell(status: &str) -> String {
    if status.is_empty() {
        return "(unknown)".to_string();
    }
    match status_icon(status) {
        Some(icon) => format!("{icon}  {status}"),
        None => status.to_string(),
    }
}

/// Parse the positional section filter, matching the Python's `--sections` validation
/// (`flux_status.py:main`). Empty args -> all three sections. A non-empty, all-valid token list
/// enables exactly the named sections. Any invalid token(s) -> `Err` of the (sorted, deduped)
/// bad names, so the caller can self-issue an Error notification without inventing wording.
pub fn parse_sections(args: &str) -> Result<(bool, bool, bool), Vec<String>> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok((true, true, true));
    }
    let mut bad = Vec::new();
    let (mut todo, mut done, mut review) = (false, false, false);
    for t in &tokens {
        match *t {
            "todo" => todo = true,
            "done" => done = true,
            "review" => review = true,
            other => bad.push(other.to_string()),
        }
    }
    if !bad.is_empty() {
        bad.sort();
        bad.dedup();
        return Err(bad);
    }
    Ok((todo, done, review))
}

/// Render the `/flux/status` panel for `base`, restricted to the enabled sections. Mirrors
/// `flux_status.py`'s `main()` empty-state check plus its `render()` layout exactly.
#[must_use]
pub fn render(base: &Path, todo: bool, done: bool, review: bool) -> String {
    if !base.exists() {
        return format!("(no flux state at {})", base.display());
    }

    let todos = if todo {
        state::collect_todos(base)
    } else {
        Vec::new()
    };
    let done_groups = if done {
        state::collect_done(base)
    } else {
        Vec::new()
    };
    let reviews = if review {
        state::collect_reviews(base)
    } else {
        Vec::new()
    };

    let mut names: Vec<&str> = Vec::new();
    for (n, _, _) in &todos {
        names.push(n);
    }
    for (_, rows) in &done_groups {
        for (n, _, _) in rows {
            names.push(n);
        }
    }
    for (n, _) in &reviews {
        names.push(n);
    }

    let longest = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    let name_w = (longest.max("TODO-FILE".chars().count()) + 2).min(50);
    let total_w = (name_w + STAGE_W + SECTION_PAD).max(MIN_PANEL_W);

    let mut lines: Vec<String> = Vec::new();
    lines.push("\u{1D571} FLUX STATUS".to_string());
    lines.push("\u{2550}".repeat(total_w));
    let mut rendered_any = false;

    // --- TODO section --------------------------------------------------
    if todo {
        rendered_any = true;
        lines.push(String::new());
        lines.push(format!(
            "{}{}{}",
            ljust("TODO-FILE", name_w),
            ljust("STAGE", STAGE_W),
            "STATUS"
        ));
        lines.push("\u{2500}".repeat(total_w));
        if todos.is_empty() {
            lines.push("(no todos)".to_string());
        }
        for (name, stage, status) in &todos {
            lines.push(format!(
                "{}{}{}",
                ljust(name, name_w),
                ljust(stage, STAGE_W),
                status_cell(status)
            ));
        }
    }

    // --- COMPLETED section -----------------------------------------------
    if done && !done_groups.is_empty() {
        lines.push(String::new());
        if rendered_any {
            lines.push("\u{2550}".repeat(total_w));
        }
        rendered_any = true;
        lines.push("COMPLETED TASKS".to_string());
        lines.push(String::new());
        lines.push(format!(
            "{}{}{}",
            ljust("TASK-FILE", name_w),
            ljust("STAGE", STAGE_W),
            "STATUS"
        ));
        for (ts_label, rows) in &done_groups {
            lines.push(format!("\u{2500}\u{2500} {ts_label} \u{2500}\u{2500}"));
            for (name, stage, status) in rows {
                lines.push(format!(
                    "{}{}{}",
                    ljust(name, name_w),
                    ljust(stage, STAGE_W),
                    status_cell(status)
                ));
            }
        }
    }

    // --- REVIEW section --------------------------------------------------
    if review && !reviews.is_empty() {
        lines.push(String::new());
        if rendered_any {
            lines.push("\u{2550}".repeat(total_w));
        }
        lines.push("REVIEW TASKS".to_string());
        lines.push(String::new());
        let mut head = ljust("REVIEW-FILE", name_w);
        for sev in state::SEVERITIES {
            head.push_str(&ljust(&sev.to_uppercase(), sev_col_width(sev)));
        }
        lines.push(head);
        let review_w = (name_w
            + state::SEVERITIES
                .iter()
                .map(|s| sev_col_width(s))
                .sum::<usize>())
        .max(MIN_PANEL_W);
        lines.push("\u{2500}".repeat(review_w));
        for (name, sev) in &reviews {
            let mut row = ljust(name, name_w);
            for col in state::SEVERITIES {
                let w = sev_col_width(col);
                if col == sev {
                    row.push('\u{25CF}');
                    for _ in 0..w.saturating_sub(1) {
                        row.push(' ');
                    }
                } else {
                    for _ in 0..w {
                        row.push(' ');
                    }
                }
            }
            lines.push(row.trim_end().to_string());
        }
    }

    lines.push("\u{2550}".repeat(total_w));
    lines.join("\n")
}
