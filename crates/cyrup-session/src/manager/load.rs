//! The tolerant streaming JSONL reader behind [`SessionManager::open`] and
//! [`SessionManager::fork_from`] (R-04-034/032) — header recovery and "last good line wins".
//!
//! [`SessionManager::open`]: super::SessionManager::open
//! [`SessionManager::fork_from`]: super::SessionManager::fork_from

use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::entry::Entry;
use crate::error::SessionError;
use crate::header::SessionHeader;

/// Tolerant streaming load (R-04-034/032): the header is the FIRST PARSED entry, not the first
/// physical line; entries after; a malformed or truncated trailing line is dropped (valid prefix
/// kept), returning `recovered = true`.
///
/// Pi's `parseSessionEntryLine` returns `null` for a blank line **and** for an unparseable one
/// (`session-manager.ts:503-511`), and `loadEntriesFromFile` pushes only the parsed ones before
/// validating `entries[0].type === "session"` (`:548-553`). So the header candidate is the first
/// line that PARSES — a stray leading newline (a truncated write, an editor, a merge) or a garbage
/// first line does not make the file unopenable. Testing `lineno == 0` off
/// `reader.lines().enumerate()` did: the real header landed at `lineno == 1`, was parsed as an
/// ordinary `Entry`, and `header` stayed `None` → `NotASession`.
///
/// `NotASession` for a non-empty file whose first parsed entry is NOT a session header is
/// deliberately kept: Pi's `loadEntriesFromFile` returns `[]`, and `_setSessionFile` then throws
/// `Session file is not a valid pi session: <path>` because `statSync(path).size > 0`
/// (`session-manager.ts:900-906`). Only a MISSING or ZERO-LENGTH file is a soft new session, which
/// [`crate::SessionManager::open_with_cwd`] handles before reaching here.
pub(super) fn load(path: &Path) -> Result<(SessionHeader, Vec<Entry>, bool), SessionError> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut header: Option<SessionHeader> = None;
    let mut entries: Vec<Entry> = Vec::new();
    let mut recovered = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => {
                recovered = true;
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if header.is_none() {
            // Pi skips an unparseable line and KEEPS SCANNING for the first parsed entry
            // (`parseSessionEntryLine`'s `catch { return null }`, `session-manager.ts:507-510`).
            if serde_json::from_str::<serde_json::Value>(&line).is_err() {
                recovered = true;
                continue;
            }
            match serde_json::from_str::<SessionHeader>(&line) {
                Ok(h) if h.kind == "session" => header = Some(h),
                _ => return Err(SessionError::NotASession { path: path.to_path_buf() }),
            }
            continue;
        }
        match serde_json::from_str::<Entry>(&line) {
            Ok(e) => entries.push(e),
            Err(_) => {
                // Skip malformed line, keep the valid prefix (R-04-034). A half-written final
                // line lands here and is dropped — "last good line wins".
                recovered = true;
            }
        }
    }

    let header = header.ok_or_else(|| SessionError::NotASession { path: path.to_path_buf() })?;
    Ok((header, entries, recovered))
}
