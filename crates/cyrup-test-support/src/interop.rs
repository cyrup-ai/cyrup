//! Session JSONL interop fixtures/runner (func-00 R-00-013): a Pi-shaped session JSONL loads in
//! cyrup and re-exports to an equivalent JSONL (round-trip), asserting interop for the common entry
//! types. Built on cyrup's `SessionManager::{import_jsonl, export_jsonl}` (arch-04 R-04-029).

use std::io::Write;

use cyrup_session::error::SessionError;
use cyrup_session::manager::SessionManager;

use crate::golden::normalize_value;

/// Failure round-tripping a session JSONL fixture.
#[derive(Debug, thiserror::Error)]
pub enum InteropError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Import a Pi-shaped session JSONL (header line + entry lines) and re-export it, returning the
/// exported JSONL. Round-trips through cyrup's [`SessionManager`].
pub fn import_export(input: &str) -> Result<String, InteropError> {
    let mut file = tempfile::Builder::new()
        .prefix("cyrup-interop-")
        .suffix(".jsonl")
        .tempfile()?;
    file.write_all(input.as_bytes())?;
    file.flush()?;
    let manager = SessionManager::import_jsonl(file.path())?;
    let mut buf: Vec<u8> = Vec::new();
    manager.export_jsonl(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Parse the entry lines (everything after the header line) of a session JSONL into normalized JSON
/// values (volatile fields folded). Malformed lines are skipped defensively.
fn entry_values(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .skip(1) // header line
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|mut v| {
            normalize_value(&mut v);
            v
        })
        .collect()
}

/// Assert a Pi-shaped session JSONL round-trips through cyrup with entry-for-entry equality
/// (normalized). Returns the exported JSONL on success, or a description of the first mismatch.
pub fn assert_jsonl_roundtrip(input: &str) -> Result<String, String> {
    let exported = import_export(input).map_err(|e| e.to_string())?;
    let before = entry_values(input);
    let after = entry_values(&exported);
    if before.len() != after.len() {
        return Err(format!(
            "entry count changed on round-trip: {} → {}",
            before.len(),
            after.len()
        ));
    }
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if b != a {
            return Err(format!(
                "entry {i} differs after round-trip:\n  before: {b}\n  after:  {a}"
            ));
        }
    }
    Ok(exported)
}
