//! Shared `~/.flux/<flattened-cwd>/` state model — a function-for-function Rust port of
//! [`flux_status.py`](../../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)'s
//! data layer (`flatten_cwd`, `derive_base`, `parse_frontmatter`, `collect_todos`,
//! `collect_done`, `format_timestamp`, `collect_reviews`). This module is the shared read model
//! for both the plain-text `/flux/status` renderer ([`crate::render_status`], FLUX_07) and the
//! themed interactive overlay (FLUX_09).
//!
//! Tolerant parsing is a requirement, not politeness (port doc §5.6): this parser serves BOTH
//! cyrup's own `/flux/*` prompt-written trees and code-puppy's — an unreadable, malformed, or
//! frontmatter-less file yields an empty map, never an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Replace every maximal run of non-ASCII-alphanumeric characters with a single `-` (Python
/// `re.sub(r"[^a-zA-Z0-9]+", "-", cwd)`; the `//flux` prompts' own
/// `tr -cs 'a-zA-Z0-9' '-'`). Case is preserved; a leading or trailing run collapses too, exactly
/// as the regex substitution does.
#[must_use]
pub fn flatten_cwd(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len());
    let mut pending_dash = false;
    for ch in cwd.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    if pending_dash {
        out.push('-'); // matches re.sub: a trailing run collapses too
    }
    out
}

/// The env var the `//flux` prompts and `flux_status.py --base`-less invocations both honour:
/// `${FLUX_ROOT:-$HOME/.flux}`.
const FLUX_ROOT_ENV_VAR: &str = "FLUX_ROOT";

/// Derive the flux base directory for the current working directory, mirroring the prompts'
/// `FLUX_ROOT="${FLUX_ROOT:-$HOME/.flux}"; FLUX_DIR=$(... | tr -cs 'a-zA-Z0-9' '-');
/// FLUX_BASE="$FLUX_ROOT/$FLUX_DIR"` exactly. The Python script's `--base` flag has no cyrup
/// analog: the native commands take section args only.
#[must_use]
pub fn derive_base() -> PathBuf {
    let root: PathBuf = match std::env::var_os(FLUX_ROOT_ENV_VAR) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(".flux"),
            None => PathBuf::from(".flux"),
        },
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let cwd_str = cwd.display().to_string();
    root.join(flatten_cwd(&cwd_str))
}

/// Read the leading `--- ... ---` block into a flat map. Tolerates a missing file, a file with no
/// frontmatter, and malformed lines — all yield an empty (or partial) map, never an error.
#[must_use]
pub fn parse_frontmatter(path: &Path) -> BTreeMap<String, String> {
    let mut data = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return data };
    if !text.starts_with("---") {
        return data;
    }
    let mut lines = text.lines();
    lines.next(); // the opening `---`
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((key, val)) = line.split_once(':') {
            data.insert(key.trim().to_string(), val.trim().to_string());
        }
    }
    data
}

/// `(file_stem, stage, status)` for every `todo/*.md`, sorted by filename. `stage`/`status`
/// default to `""` when absent from frontmatter.
#[must_use]
pub fn collect_todos(base: &Path) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    let todo_dir = base.join("todo");
    if !todo_dir.is_dir() {
        return rows;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&todo_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    for md in entries {
        let fm = parse_frontmatter(&md);
        let stem = md.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        rows.push((
            stem,
            fm.get("stage").cloned().unwrap_or_default(),
            fm.get("status").cloned().unwrap_or_default(),
        ));
    }
    rows
}

/// One `done/<ts>/` group: `(name, stage, status)` rows for every task file processed in the
/// run identified by that timestamp directory.
pub type DoneGroup = (String, Vec<(String, String, String)>);

/// `(timestamp_label, [(name, stage, status), ...])` per `done/<ts>/` group, groups
/// reverse-sorted by directory name, rows sorted by filename within a group. A group with no
/// `.md` rows is omitted. `status` defaults to `"completed"` here (not `""`) — the Python's
/// `fm.get("status", "completed")` — which is what makes a code-puppy-written done file render
/// correctly.
#[must_use]
pub fn collect_done(base: &Path) -> Vec<DoneGroup> {
    let mut groups = Vec::new();
    let done_dir = base.join("done");
    if !done_dir.is_dir() {
        return groups;
    }
    let mut ts_dirs: Vec<PathBuf> = std::fs::read_dir(&done_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    ts_dirs.sort();
    ts_dirs.reverse();
    for ts_dir in ts_dirs {
        let mut mds: Vec<PathBuf> = std::fs::read_dir(&ts_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
                    .collect()
            })
            .unwrap_or_default();
        mds.sort();
        let mut rows = Vec::new();
        for md in mds {
            let fm = parse_frontmatter(&md);
            let stem = md.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            rows.push((
                stem,
                fm.get("stage").cloned().unwrap_or_default(),
                fm.get("status").cloned().unwrap_or_else(|| "completed".to_string()),
            ));
        }
        if !rows.is_empty() {
            let name = ts_dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            groups.push((format_timestamp(&name), rows));
        }
    }
    groups
}

/// `2026-04-29-16-57` -> `2026-04-29 16:57`; anything not splitting into exactly 5 `-`-separated
/// parts passes through verbatim.
#[must_use]
pub fn format_timestamp(dirname: &str) -> String {
    let parts: Vec<&str> = dirname.split('-').collect();
    match parts.as_slice() {
        [a, b, c, d, e] => format!("{a}-{b}-{c} {d}:{e}"),
        _ => dirname.to_string(),
    }
}

/// The fixed severity scan order.
pub const SEVERITIES: [&str; 4] = ["critical", "high", "medium", "low"];

/// `(review_name, severity)` ordered critical->low; sorted by filename within a severity;
/// missing severity dirs skipped.
#[must_use]
pub fn collect_reviews(base: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let review_dir = base.join("review");
    if !review_dir.is_dir() {
        return out;
    }
    for sev in SEVERITIES {
        let sev_dir = review_dir.join(sev);
        if !sev_dir.is_dir() {
            continue;
        }
        let mut mds: Vec<PathBuf> = std::fs::read_dir(&sev_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
                    .collect()
            })
            .unwrap_or_default();
        mds.sort();
        for md in mds {
            let stem = md.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((stem, sev.to_string()));
        }
    }
    out
}
