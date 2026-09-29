//! JSONL export (R-04-029) and the plain getters over the manager's own state (R-04-028).

use std::io::Write;
use std::path::Path;

use cyrup_core::{EntryId, SessionId};
use serde_json::Value;

use crate::entry::{Entry, KnownEntry};
use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::ids::now_ts;

use super::SessionManager;

impl SessionManager {
    /// Write the WHOLE session as JSONL — the stored header plus every entry of the tree, original
    /// `parentId`s intact (R-04-029). This is the document the HTML export renders, whose tree view
    /// needs every branch (pi's `exportSessionToHtml` reads the full session manager). The exported
    /// header's `cwd` is the manager's own (possibly cwd-overridden) cwd, not the persisted header's:
    /// normally `self.cwd == self.header.cwd`; they differ only for a session opened via
    /// [`Self::open_with_cwd`] with an override, which then exports under the override.
    ///
    /// `/export <file>.jsonl`, `/share` and RPC `export_jsonl` export the current BRANCH instead —
    /// [`Self::export_branch_jsonl`].
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

    /// Write the CURRENT BRANCH as a linear JSONL session — Pi `serializeSessionBranch`
    /// (`core/session-export.ts:9-29` @v0.87.1), behind `AgentSession.exportToJsonl` and the share
    /// upload (SESS-058, DRIFT-055):
    ///
    /// * a FRESH header — `{type, version, id, timestamp: now, cwd: getCwd()}`, no `parentSession`;
    /// * then `getBranch()` root→leaf, each entry re-chained onto the one before it (`{ ...entry,
    ///   parentId }` with `parentId` starting at `null`), so abandoned branches are left out and the
    ///   file resumes as one linear conversation.
    ///
    /// An entry cyrup keeps verbatim ([`Entry::Unknown`]) is re-chained the same way on its raw
    /// object.
    pub fn export_branch_jsonl(&self, w: &mut dyn Write) -> Result<(), SessionError> {
        let header = SessionHeader::new(
            self.header.id.clone(),
            self.cwd.to_string_lossy().into_owned(),
            now_ts(),
        );
        w.write_all(serde_json::to_string(&header)?.as_bytes())?;
        w.write_all(b"\n")?;
        let mut parent_id: Option<EntryId> = None;
        for entry in self.branch_path(None) {
            let mut entry = entry.clone();
            match &mut entry {
                Entry::Known(k) => k.base_mut().parent_id = parent_id.clone(),
                Entry::Unknown(Value::Object(raw)) => {
                    raw.insert(
                        "parentId".to_string(),
                        parent_id
                            .as_ref()
                            .map_or(Value::Null, |p| Value::String(p.to_string())),
                    );
                }
                Entry::Unknown(_) => {}
            }
            w.write_all(entry.to_line()?.as_bytes())?;
            w.write_all(b"\n")?;
            parent_id = Some(entry.id());
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
