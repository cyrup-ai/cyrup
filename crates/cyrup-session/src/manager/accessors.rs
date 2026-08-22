//! JSONL export (R-04-029) and the plain getters over the manager's own state (R-04-028).

use std::io::Write;
use std::path::Path;

use cyrup_core::SessionId;

use crate::entry::{Entry, KnownEntry};
use crate::error::SessionError;
use crate::header::SessionHeader;

use super::SessionManager;

impl SessionManager {
    /// Write the session as JSONL (header + entries) for export (R-04-029). The exported header's
    /// `cwd` is taken from the manager's own (possibly cwd-overridden) cwd, not the persisted header —
    /// 1:1 with Pi `exportToJsonl`, which builds `cwd: this.sessionManager.getCwd()`
    /// (agent-session.ts:3061). Normally `self.cwd == self.header.cwd`; they differ only for a session
    /// opened via [`Self::open_with_cwd`] with an override, which then exports under the override.
    pub fn export_jsonl(&self, w: &mut dyn Write) -> Result<(), SessionError> {
        let mut header = self.header.clone();
        header.cwd = self.cwd.to_string_lossy().into_owned();
        w.write_all(serde_json::to_string(&header)?.as_bytes())?;
        w.write_all(b"\n")?;
        for e in &self.entries {
            w.write_all(e.to_line()?.as_bytes())?;
            w.write_all(b"\n")?;
        }
        Ok(())
    }

    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn session_id(&self) -> &SessionId {
        &self.header.id
    }

    pub fn session_file(&self) -> Option<&Path> {
        self.store.path()
    }

    pub fn is_persisted(&self) -> bool {
        self.store.is_persisted()
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The most recent session display name (latest `SessionInfo`, R-04-028). Pi `getSessionName`
    /// (`session-manager.ts:1045-1056`) returns the latest `session_info` name trimmed, mapping an
    /// empty/whitespace-only name to `None` (an empty name explicitly clears the title).
    pub fn session_name(&self) -> Option<String> {
        // Stop at the latest `session_info`; its (possibly empty) name decides the result.
        let latest = self.entries.iter().rev().find_map(|e| match e {
            Entry::Known(KnownEntry::SessionInfo { name, .. }) => {
                Some(name.as_deref().unwrap_or(""))
            }
            _ => None,
        })?;
        let trimmed = latest.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}
