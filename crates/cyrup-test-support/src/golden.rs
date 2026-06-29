//! Golden / snapshot recorder (the crate's self-disclosed-deferred promise, lib.rs:4; func-00
//! R-00-012 mechanism).
//!
//! Serializes a captured event sequence to a normalized JSONL snapshot (volatile fields — timestamps,
//! response ids — zeroed so reruns are stable) and compares it against a stored golden file. A
//! missing golden, or `UPDATE_GOLDEN=1` / `CYRUP_UPDATE_GOLDEN=1`, (re)writes the file; otherwise a
//! mismatch returns a unified diff.

use std::path::Path;

use serde::Serialize;
use similar::TextDiff;

/// Zero volatile fields in a JSON value so snapshots are stable across runs: `timestamp`/`expires`
/// → 0, `responseId`/`response_id` → null. Recurses into objects and arrays.
pub fn normalize_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match key.as_str() {
                    "timestamp" | "expires" => *v = serde_json::Value::from(0),
                    "responseId" | "response_id" => *v = serde_json::Value::Null,
                    _ => normalize_value(v),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() {
                normalize_value(v);
            }
        }
        _ => {}
    }
}

/// Serialize a sequence to a normalized JSONL snapshot: one normalized JSON object per line (Pi's
/// golden-event recorder shape). Non-serializable items are skipped defensively (never panics).
pub fn snapshot<T: Serialize>(items: &[T]) -> String {
    let mut out = String::new();
    for item in items {
        if let Ok(mut v) = serde_json::to_value(item) {
            normalize_value(&mut v);
            if let Ok(line) = serde_json::to_string(&v) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

fn update_requested() -> bool {
    std::env::var("UPDATE_GOLDEN").map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
        || std::env::var("CYRUP_UPDATE_GOLDEN").map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
}

/// Compare `actual` against the golden file at `path` (Pi golden compare). A missing file or an
/// update request (re)writes the golden and returns `Ok`. A mismatch returns a unified diff string.
pub fn verify(path: impl AsRef<Path>, actual: &str) -> Result<(), String> {
    let path = path.as_ref();
    let existing = std::fs::read_to_string(path).ok();

    match existing {
        Some(expected) if !update_requested() => {
            if expected == actual {
                Ok(())
            } else {
                let diff = TextDiff::from_lines(&expected, actual);
                Err(diff.unified_diff().header("golden", "actual").to_string())
            }
        }
        _ => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create golden dir: {e}"))?;
            }
            std::fs::write(path, actual).map_err(|e| format!("write golden: {e}"))?;
            Ok(())
        }
    }
}

/// Snapshot `items` and [`verify`] against the golden at `path` (convenience).
pub fn verify_snapshot<T: Serialize>(path: impl AsRef<Path>, items: &[T]) -> Result<(), String> {
    verify(path, &snapshot(items))
}
