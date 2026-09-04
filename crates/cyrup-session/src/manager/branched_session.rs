//! Re-rooting the session onto an explicit leaf — Pi `createBranchedSession`
//! (`session-manager.ts:1292-1392`). Distinct from [`super::navigate`], which only moves the leaf
//! pointer inside the current file; this mints a NEW session (and, when persisting, a new file).

use std::path::PathBuf;

use cyrup_core::EntryId;

use crate::entry::{Entry, EntryBase, KnownEntry};
use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::ids::{gen_session_id, gen_short_id, now_ts};
use crate::layout::SessionLayout;
use crate::store::{DiskStore, MemStore, SessionStore};

use super::{SessionManager, entries_have_assistant};

impl SessionManager {
    /// Re-root this session onto the path from root through an EXPLICIT `leaf_id` and switch this
    /// manager to it IN PLACE — Pi `createBranchedSession(leafId)`
    /// (`session-manager.ts:1292-1392`). The manager is mutated in place (Pi assigns `this.fileEntries`/
    /// `this.sessionId`/`this.sessionFile` and rebuilds its index), the previous file is never
    /// touched, and `parentSession` records the previous file only when persisting
    /// (`session-manager.ts:1321`). `Label` entries are dropped and the retained path re-chained, then
    /// labels for retained targets re-attached as trailing entries (so removing labels never orphans
    /// a subtree). Returns the new session-file path when persisting, or `None` for an in-memory
    /// session (Pi returns `string | undefined`).
    pub fn create_branched_session(
        &mut self,
        leaf_id: &EntryId,
        layout: &SessionLayout,
    ) -> Result<Option<PathBuf>, SessionError> {
        let path_entries: Vec<Entry> = self
            .branch_path(Some(leaf_id))
            .into_iter()
            .cloned()
            .collect();
        if path_entries.is_empty() {
            // Pi: `throw new Error(`Entry ${leafId} not found`)` (`session-manager.ts:1295-1297`).
            return Err(SessionError::EntryNotFound(leaf_id.clone()));
        }

        // Re-chain the non-label entries linearly.
        let mut retained: Vec<Entry> = Vec::new();
        let mut prev: Option<EntryId> = None;
        for e in &path_entries {
            if matches!(e, Entry::Known(KnownEntry::Label { .. })) {
                continue;
            }
            let mut cloned = e.clone();
            if let Some(base) = cloned.base_mut() {
                base.parent_id = prev.clone();
            }
            prev = Some(cloned.id());
            retained.push(cloned);
        }

        // Re-attach labels for retained targets as trailing label entries. Pi collects from the
        // GLOBAL `labelsById`/`labelTimestampsById` maps for any target present in the retained path
        // (`session-manager.ts:1324-1331`) — NOT just the `Label` entries that happen to lie on the
        // branched path. cyrup's `self.labels` (target id → (label, original timestamp)) is the exact
        // equivalent, rebuilt over the WHOLE file in `rebuild_index`/`apply_label` with latest-wins +
        // cleared-removed semantics. Iterating it (a) preserves the latest label even when the
        // governing `Label` entry is off-path, and (b) never re-emits a set-then-cleared label.
        let retained_ids: std::collections::HashSet<EntryId> =
            retained.iter().map(Entry::id).collect();
        // Pi iterates `labelsById` in JS `Map` insertion order: a target is positioned at its FIRST
        // live `set` and removed on clear (a re-`set` of a still-live target keeps its slot). Replay
        // the full-file label entries with those semantics so the trailing label order is
        // deterministic and matches Pi (ids themselves are irreducibly random in both ports).
        let mut order: Vec<EntryId> = Vec::new();
        for e in &self.entries {
            if let Entry::Known(KnownEntry::Label {
                target_id, label, ..
            }) = e
            {
                match label {
                    Some(_) => {
                        if !order.contains(target_id) {
                            order.push(target_id.clone());
                        }
                    }
                    None => order.retain(|t| t != target_id),
                }
            }
        }
        for target_id in &order {
            if !retained_ids.contains(target_id) {
                continue;
            }
            // `self.labels` holds the latest (label, original-timestamp) for this target; absent ⇒
            // cleared (skip). Pi re-emits with the ORIGINAL `labelTimestampsById` timestamp, NOT now.
            if let Some((label, label_ts)) = self.labels.get(target_id) {
                let base = EntryBase {
                    id: gen_short_id(),
                    parent_id: prev.clone(),
                    timestamp: label_ts.clone(),
                    extra: Default::default(),
                };
                let lbl = Entry::known(KnownEntry::Label {
                    base,
                    target_id: target_id.clone(),
                    label: Some(label.clone()),
                });
                prev = Some(lbl.id());
                retained.push(lbl);
            }
        }

        let previous_session_file = self
            .session_file()
            .map(|p| p.to_string_lossy().into_owned());
        let persisted = self.store.is_persisted();
        let id = gen_session_id();
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), self.cwd.to_string_lossy(), ts.clone());
        // Pi: `parentSession: this.persist ? previousSessionFile : undefined`
        // (`session-manager.ts:1321`).
        header.parent_session = if persisted {
            previous_session_file
        } else {
            None
        };

        // Pi `createBranchedSession` in-memory branch (`session-manager.ts:1373-1391`): replace the
        // entries + id, rebuild the index, and return `undefined` — no disk write.
        if !persisted {
            self.adopt_branch(header, Box::new(MemStore), retained, false);
            return Ok(None);
        }

        let path = layout.new_file_path(&ts, id.as_str());
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path.clone()));
        // Pi `createBranchedSession` defers the file write until an assistant message exists
        // (`session-manager.ts:1362-1368`, explicitly to avoid the duplicate-header bug): write
        // eagerly only when the retained path already contains an assistant, otherwise leave the
        // file uncreated and let the first assistant append flush it (`flushed = false`).
        let flushed = if entries_have_assistant(&retained) {
            store.rewrite(&header, &retained)?;
            true
        } else {
            false
        };
        self.adopt_branch(header, store, retained, flushed);
        Ok(Some(path))
    }

    /// Replace this manager's header/store/entries in place (Pi `createBranchedSession` mutation),
    /// rebuilding the index and resetting the leaf to the new last entry. `cwd` is preserved.
    fn adopt_branch(
        &mut self,
        header: SessionHeader,
        store: Box<dyn SessionStore>,
        entries: Vec<Entry>,
        flushed: bool,
    ) {
        self.header = header;
        self.store = store;
        self.entries = entries;
        self.flushed = flushed;
        self.rebuild_index();
        self.leaf = self.entries.last().map(Entry::id);
    }
}
