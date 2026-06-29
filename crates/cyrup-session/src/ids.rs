//! Entry-id minting and timestamps (arch-04 §4.1). Entry ids are short 8-hex tokens (Pi
//! `generateId`); session header ids are uuid v7; timestamps are RFC3339.

use cyrup_core::{EntryId, SessionId};

/// A short 8-hex entry id drawn from the random tail of a uuid v7 (collision-checked by the
/// caller against existing ids).
pub fn gen_short_id() -> EntryId {
    let s = uuid::Uuid::now_v7().as_simple().to_string();
    let start = s.len().saturating_sub(8);
    let tail = s.get(start..).unwrap_or(s.as_str());
    EntryId::from(tail)
}

/// A fresh session id (uuid v7, time-sortable so newest-session selection needs no timestamp
/// parsing).
pub fn gen_session_id() -> SessionId {
    SessionId::from(uuid::Uuid::now_v7().to_string())
}

/// Validate a caller-supplied session id (Pi `assertValidSessionId`, `session-manager.ts:207-213`):
/// non-empty, only `[A-Za-z0-9._-]`, and first/last char alphanumeric. `Err` carries Pi's message.
pub fn validate_session_id(id: &str) -> Result<(), String> {
    fn alnum(b: u8) -> bool {
        b.is_ascii_alphanumeric()
    }
    let bytes = id.as_bytes();
    let ok = match bytes {
        [] => false,
        [only] => alnum(*only),
        [first, mid @ .., last] => {
            alnum(*first)
                && alnum(*last)
                && mid.iter().all(|&b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
        }
    };
    if ok {
        Ok(())
    } else {
        Err("Session id must be non-empty, contain only alphanumeric characters, '-', '_', and \
'.', and start and end with an alphanumeric character"
            .to_string())
    }
}

/// Current time as an RFC3339 string.
pub fn now_ts() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
