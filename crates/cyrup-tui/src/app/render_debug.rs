//! The two render-debug instruments — the port of pi's `PI_TUI_DEBUG` and `PI_TUI_DEBUG_REDRAW`
//! (`packages/tui/src/tui-main-screen.ts:321-327`, `:569-595` @v0.87.1) — CFG-063.
//!
//! # What upstream records
//!
//! * `PI_TUI_DEBUG=1` — at the end of every differential frame, one file per frame under the
//!   literal `/tmp/tui`, named `render-<Date.now()>-<random>.log`, holding the renderer's decision
//!   state (`firstChanged`, `viewportTop`, `height`, …), then the new and previous line arrays as
//!   JSON.
//! * `PI_TUI_DEBUG_REDRAW=1` (renamed from `PI_DEBUG_REDRAW` by v0.85.1) — one appended line per
//!   FULL redraw, `[<ISO time>] fullRender: <reason> (prev=<n>, new=<n>, height=<n>)`, in
//!   `<logDirectory>/pi-tui-debug.log`. Disabled outright when the host supplied no log directory
//!   (`const redrawLogDirectory = … ? this.logDirectory : undefined`, #8699); the coding agent
//!   supplies `getAgentDir()` (`interactive-mode.ts:581`).
//!
//! # How the names map
//!
//! `CYRUP_TUI_DEBUG` / `CYRUP_TUI_DEBUG_REDRAW` (the hard rename of R-07-028; no `PI_` alias,
//! matching `crates/cyrup-config/src/env.rs`), both compared strictly to `"1"`, and the redraw log
//! is `cyrup-tui-debug.log`.
//!
//! # `[CYRUP-DELTA]` what a "full redraw" and a "line" are here
//!
//! cyrup's inline renderer is ratatui's diffing `Terminal`, not pi's hand-rolled line differ, so the
//! events and fields are the ones cyrup's frame path actually has ([`App::draw`]):
//!
//! * A full redraw is anything that repaints the whole inline region instead of diffing it: the
//!   first frame, a terminal WIDTH or HEIGHT change (ratatui's autoresize clears the viewport), and
//!   a live-region height change (`resize_viewport` erases the region and rebuilds the `Terminal`)
//!   — pi's `first render`, `terminal width changed`, `terminal height changed` and, for the last,
//!   its `clearOnShrink` / viewport-moved family. `prev` / `new` are the live region's row counts
//!   before and after, the analogue of pi's `previousLines.length` / `newLines.length`.
//! * A frame dump's lines are the inline viewport's rendered rows as plain text (cell symbols,
//!   trailing blanks trimmed) — pi's strings carry ANSI styling; ratatui's buffer holds styling per
//!   cell, not in the text. `firstChanged` / `lastChanged` are computed over those rows exactly as
//!   pi computes them over its lines; pi's cursor bookkeeping (`cursorRow`, `hardwareCursorRow`,
//!   `lineDiff`, `renderEnd`, `finalCursorRow`) belongs to a differ cyrup does not have and is
//!   replaced by the live-region inputs that decide cyrup's frame (`liveFloor`, `committedRows`).
//!   pi's `cursorPos` line and its closing `=== buffer ===` byte count are omitted with them:
//!   ratatui's `CompletedFrame` reports neither the cursor it placed nor the bytes its backend wrote.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// `CYRUP_TUI_DEBUG` — pi `PI_TUI_DEBUG`.
pub const ENV_TUI_DEBUG: &str = "CYRUP_TUI_DEBUG";
/// `CYRUP_TUI_DEBUG_REDRAW` — pi `PI_TUI_DEBUG_REDRAW`.
pub const ENV_TUI_DEBUG_REDRAW: &str = "CYRUP_TUI_DEBUG_REDRAW";
/// pi's hardcoded frame-dump directory (`const debugDir = "/tmp/tui"`, `tui-main-screen.ts:570`).
pub const FRAME_DUMP_DIR: &str = "/tmp/tui";
/// The redraw log's file name inside the host's log directory — pi's `pi-tui-debug.log`.
pub const REDRAW_LOG_FILE: &str = "cyrup-tui-debug.log";

/// Where each instrument writes, or `None` when it is off. The default is both off, which is what an
/// [`App`] the composition root never configures behaves as.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderDebug {
    /// `CYRUP_TUI_DEBUG=1` → [`FRAME_DUMP_DIR`].
    pub frame_dump_dir: Option<PathBuf>,
    /// `CYRUP_TUI_DEBUG_REDRAW=1` AND a host log directory → that directory.
    pub redraw_log_dir: Option<PathBuf>,
}

impl RenderDebug {
    /// Resolve both instruments from an env lookup and the host's log directory, with pi's exact
    /// gates: each variable must be the string `"1"`, and the redraw log needs a log directory
    /// (`tui-main-screen.ts:321`).
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>, log_directory: Option<&Path>) -> Self {
        let on = |k: &str| lookup(k).as_deref() == Some("1");
        Self {
            frame_dump_dir: on(ENV_TUI_DEBUG).then(|| PathBuf::from(FRAME_DUMP_DIR)),
            redraw_log_dir: on(ENV_TUI_DEBUG_REDRAW)
                .then(|| log_directory.map(Path::to_path_buf))
                .flatten(),
        }
    }

    /// pi `logRedraw(reason)` (`tui-main-screen.ts:322-328`): `mkdir -p` the directory and append
    /// one line. Write failures are dropped — a debug instrument must never take the frame down.
    pub(crate) fn log_redraw(&self, reason: &str, prev: u16, new: u16, height: u16) {
        let Some(dir) = self.redraw_log_dir.as_deref() else {
            return;
        };
        let line = format!(
            "[{}] fullRender: {reason} (prev={prev}, new={new}, height={height})\n",
            iso_now()
        );
        let _ = std::fs::create_dir_all(dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(REDRAW_LOG_FILE))
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// pi's `PI_TUI_DEBUG` block (`tui-main-screen.ts:569-595`): one file per frame, decision state
    /// first, then both line arrays as pretty JSON.
    ///
    /// Only a frame that actually rewrites a row is dumped. pi's dump sits at the end of the
    /// differential write, and every frame that writes nothing returns before it: `firstChanged ===
    /// -1` (`:386-392`, the idle spinner-less frame) and the deleted-lines-only branch
    /// (`firstChanged >= newLines.length`, `:394-446`).
    pub(crate) fn dump_frame(&self, frame: &FrameDump<'_>) {
        let Some(dir) = self.frame_dump_dir.as_deref() else {
            return;
        };
        let (first, last) = changed_range(frame.new_lines, frame.previous_lines);
        if first == -1 || first >= frame.new_lines.len() as i64 {
            return;
        }
        let _ = std::fs::create_dir_all(dir);
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let path = dir.join(format!(
            "render-{millis}-{}.log",
            uuid::Uuid::now_v7().simple()
        ));
        let json = |lines: &[String]| {
            serde_json::to_string_pretty(lines).unwrap_or_else(|_| "[]".to_string())
        };
        let body = [
            format!("firstChanged: {first}"),
            format!("lastChanged: {last}"),
            format!("viewportTop: {}", frame.viewport_top),
            format!("height: {}", frame.height),
            format!("width: {}", frame.width),
            format!("liveRegionHeight: {}", frame.live_region_height),
            format!("liveFloor: {}", frame.live_floor),
            format!("committedRows: {}", frame.committed_rows),
            format!("newLines.length: {}", frame.new_lines.len()),
            format!("previousLines.length: {}", frame.previous_lines.len()),
            String::new(),
            "=== newLines ===".to_string(),
            json(frame.new_lines),
            String::new(),
            "=== previousLines ===".to_string(),
            json(frame.previous_lines),
        ]
        .join("\n");
        let _ = std::fs::write(path, body);
    }
}

/// pi's `previousWidth` / `previousHeight` before any frame has been drawn (`private previousWidth
/// = 0`, `tui-main-screen.ts:128` @v0.87.1) — the state that makes the next frame `first render`.
pub(crate) const NO_FRAME: (i32, i32) = (0, 0);

/// The same pair after pi's `resetRenderState()` (`previousWidth = -1; previousHeight = -1`,
/// `:158-166`), the from-scratch repaint `requestRender(true)` asks for. The next frame then logs
/// `terminal width changed (-1 -> <width>)`, exactly as upstream's does.
pub(crate) const RESET_FRAME: (i32, i32) = (-1, -1);

/// Why this inline frame repaints the whole region, or `None` for an ordinary diffed frame — pi's
/// `logRedraw` reasons in pi's precedence (`tui-main-screen.ts:251-252`, `:330-358` @v0.87.1): the
/// first frame, then a terminal width change, then a terminal height change, then (cyrup's own) a
/// live-region height change that forces `resize_viewport`'s erase-and-rebuild. `previous_size`
/// is pi's `(previousWidth, previousHeight)`: [`NO_FRAME`], [`RESET_FRAME`] or the last frame's
/// size, and a change is `previous !== 0 && previous !== current`, as upstream computes it.
///
/// pi skips its height reason under Termux, where the soft keyboard toggles the height and a full
/// redraw would replay the history (`:345-349`); cyrup's inline region is bottom-anchored and
/// content-sized, so a height change is a full repaint on every terminal and is logged as one.
pub(crate) fn full_redraw_reason(
    previous_size: (i32, i32),
    (width, height): (u16, u16),
    live_region_height: u16,
    desired_height: u16,
) -> Option<String> {
    let (prev_w, prev_h) = previous_size;
    let width_changed = prev_w != 0 && prev_w != i32::from(width);
    let height_changed = prev_h != 0 && prev_h != i32::from(height);
    if previous_size == NO_FRAME {
        Some("first render".to_string())
    } else if width_changed {
        Some(format!("terminal width changed ({prev_w} -> {width})"))
    } else if height_changed {
        Some(format!("terminal height changed ({prev_h} -> {height})"))
    } else {
        (desired_height != live_region_height).then(|| {
            format!("live region height changed ({live_region_height} -> {desired_height})")
        })
    }
}

/// One frame's decision state, as [`RenderDebug::dump_frame`] writes it.
pub(crate) struct FrameDump<'a> {
    pub(crate) viewport_top: u16,
    pub(crate) height: u16,
    pub(crate) width: u16,
    pub(crate) live_region_height: u16,
    pub(crate) live_floor: u16,
    pub(crate) committed_rows: u16,
    pub(crate) new_lines: &'a [String],
    pub(crate) previous_lines: &'a [String],
}

/// pi's first/last changed line scan (`tui-main-screen.ts:363-383`): compare index by index over the
/// longer of the two arrays (a missing line reads as `""`), then extend to the end when lines were
/// appended. `-1` for both when nothing changed.
fn changed_range(new: &[String], previous: &[String]) -> (i64, i64) {
    let (mut first, mut last) = (-1_i64, -1_i64);
    for i in 0..new.len().max(previous.len()) {
        let old = previous.get(i).map_or("", String::as_str);
        let now = new.get(i).map_or("", String::as_str);
        if old != now {
            if first == -1 {
                first = i as i64;
            }
            last = i as i64;
        }
    }
    if new.len() > previous.len() {
        if first == -1 {
            first = previous.len() as i64;
        }
        last = new.len() as i64 - 1;
    }
    (first, last)
}

/// A ratatui buffer's rows as plain text — each row's cell symbols, trailing blanks trimmed.
pub(crate) fn buffer_lines(buf: &ratatui::buffer::Buffer) -> Vec<String> {
    let area = buf.area;
    (area.y..area.y.saturating_add(area.height))
        .map(|y| {
            let row: String = (area.x..area.x.saturating_add(area.width))
                .filter_map(|x| buf.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                .collect();
            row.trim_end().to_string()
        })
        .collect()
}

/// `new Date().toISOString()` — UTC, millisecond precision, `Z` suffix.
fn iso_now() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// pi's gates: exactly `"1"` for each variable, and no redraw log without a log directory
    /// (#8699, `tui-main-screen.ts:321-323`).
    #[test]
    fn both_instruments_use_pis_gates() {
        let dir = Path::new("/agent");
        assert_eq!(
            RenderDebug::from_env(lookup(&[]), Some(dir)),
            RenderDebug::default()
        );
        let on = RenderDebug::from_env(
            lookup(&[(ENV_TUI_DEBUG, "1"), (ENV_TUI_DEBUG_REDRAW, "1")]),
            Some(dir),
        );
        assert_eq!(on.frame_dump_dir.as_deref(), Some(Path::new("/tmp/tui")));
        assert_eq!(on.redraw_log_dir.as_deref(), Some(dir));
        let truthy_but_not_one = RenderDebug::from_env(
            lookup(&[(ENV_TUI_DEBUG, "true"), (ENV_TUI_DEBUG_REDRAW, "yes")]),
            Some(dir),
        );
        assert_eq!(truthy_but_not_one, RenderDebug::default());
        let no_log_dir = RenderDebug::from_env(lookup(&[(ENV_TUI_DEBUG_REDRAW, "1")]), None);
        assert_eq!(no_log_dir.redraw_log_dir, None);
    }

    /// pi's reason precedence — first render, width, height — then cyrup's live-region rebuild.
    #[test]
    fn full_redraw_reasons_follow_pis_precedence() {
        assert_eq!(
            full_redraw_reason(NO_FRAME, (80, 24), 0, 5).as_deref(),
            Some("first render")
        );
        assert_eq!(
            full_redraw_reason(RESET_FRAME, (80, 24), 5, 5).as_deref(),
            Some("terminal width changed (-1 -> 80)"),
            "pi's resetRenderState leaves -1, which the width check sees first"
        );
        assert_eq!(
            full_redraw_reason((100, 30), (80, 24), 5, 7).as_deref(),
            Some("terminal width changed (100 -> 80)"),
            "width outranks height and the live region"
        );
        assert_eq!(
            full_redraw_reason((80, 30), (80, 24), 5, 7).as_deref(),
            Some("terminal height changed (30 -> 24)")
        );
        assert_eq!(
            full_redraw_reason((80, 24), (80, 24), 5, 7).as_deref(),
            Some("live region height changed (5 -> 7)")
        );
        assert_eq!(full_redraw_reason((80, 24), (80, 24), 5, 5), None);
    }

    /// pi's changed-range scan, including the appended-lines extension.
    #[test]
    fn changed_range_matches_pis_scan() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        assert_eq!(changed_range(&s(&["a", "b"]), &s(&["a", "b"])), (-1, -1));
        assert_eq!(
            changed_range(&s(&["a", "x", "c"]), &s(&["a", "b", "c"])),
            (1, 1)
        );
        assert_eq!(changed_range(&s(&["a", "b", "c"]), &s(&["a", "b"])), (2, 2));
        assert_eq!(changed_range(&s(&["a"]), &s(&["a", "b"])), (1, 1));
        assert_eq!(changed_range(&s(&["a"]), &s(&[])), (0, 0));
    }
}
