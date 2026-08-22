//! Leaf movement and branch summaries (R-04-009/023/024, R-05-016/017). Moving the leaf mutates
//! nothing on disk and deletes nothing (DI-9); the abandoned branch is never touched. Contrast
//! [`super::branched_session`], which re-roots into a whole new session.

use serde_json::Value;

use cyrup_core::{EntryId, Usage};

use crate::entry::{Entry, KnownEntry};
use crate::error::SessionError;

use super::SessionManager;

impl SessionManager {
    /// Move the leaf to `to` in place — no file mutation, nothing deleted (R-04-023).
    pub fn branch(&mut self, to: &EntryId) -> Result<(), SessionError> {
        if !self.by_id.contains_key(to) {
            return Err(SessionError::EntryNotFound(to.clone()));
        }
        self.leaf = Some(to.clone());
        Ok(())
    }

    /// Reset the leaf to before the first entry (the next append starts a new root, R-04-023).
    pub fn reset_leaf(&mut self) {
        self.leaf = None;
    }

    /// Move the leaf to `to`, then append a `BranchSummary` capturing the abandoned branch
    /// (R-04-024). The abandoned branch is never touched.
    pub fn branch_with_summary(
        &mut self,
        to: Option<&EntryId>,
        summary: String,
        details: Option<Value>,
        usage: Option<Usage>,
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        match to {
            Some(id) => self.branch(id)?,
            None => self.reset_leaf(),
        }
        let from_id = to.cloned().unwrap_or_else(|| EntryId::from("root"));
        self.push_entry(Entry::known(KnownEntry::BranchSummary {
            base: self.make_base(),
            from_id,
            summary,
            details,
            usage,
            from_hook: Some(from_hook),
        }))
    }

    /// Append a `BranchSummary` at the current leaf with an explicit `from_id` (the entry navigated
    /// *from*), per the corrected R-05-016. Unlike [`Self::branch_with_summary`], this does not move
    /// the leaf — the caller navigates first so the summary is recorded at the navigation point. The
    /// abandoned branch is never touched (R-05-017).
    pub fn append_branch_summary(
        &mut self,
        from_id: EntryId,
        summary: String,
        details: Option<Value>,
        usage: Option<Usage>,
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::BranchSummary {
            base: self.make_base(),
            from_id,
            summary,
            details,
            usage,
            from_hook: Some(from_hook),
        }))
    }
}
