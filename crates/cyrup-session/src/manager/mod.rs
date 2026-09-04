//! `SessionManager` — the facade over the JSONL tree (arch-04 §3.2/§6). Single-writer, `&mut self`
//! mutation; append-only in memory and on disk; the leaf pointer is the only thing branching moves.
//!
//! ## Layout
//! The facade spans the whole arch-04 surface, so its `impl` blocks are split by concern. The
//! submodules are private — an internal layout, not new public paths:
//! `lifecycle` (create/open/continue/fork/ephemeral + [`NewSessionOpts`]), `load` (the tolerant
//! JSONL reader behind `open`/`fork_from`), `branched_session` (re-root onto an explicit leaf, Pi
//! `createBranchedSession`), `append` (the typed `append_*` entry constructors), `tree`
//! (read-only tree/leaf/label queries + [`TreeNode`]), `navigate` (leaf movement and branch
//! summaries), `context` (active-path context projection) and `accessors` (export + plain
//! getters).
//!
//! The struct, its fields, construction (`assemble`/`rebuild_index`) and the single write path every
//! concern shares (`make_base`, `push_entry`, `persist_last`) stay here. Those are private to this
//! module, which Rust already makes visible to every child module above.

mod accessors;
mod append;
mod branched_session;
mod context;
mod lifecycle;
mod load;
mod navigate;
mod tree;

// The surface `lib.rs` re-exports (`pub use manager::{…}`) — same names, same paths.
pub use lifecycle::NewSessionOpts;
pub use tree::TreeNode;

use std::collections::HashMap;
use std::path::PathBuf;

use cyrup_core::EntryId;

use crate::entry::{Entry, EntryBase, KnownEntry};
use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::ids::{gen_short_id, now_ts};
use crate::store::SessionStore;

pub struct SessionManager {
    header: SessionHeader,
    cwd: PathBuf,
    store: Box<dyn SessionStore>,
    entries: Vec<Entry>,
    by_id: HashMap<EntryId, usize>,
    children: HashMap<EntryId, Vec<EntryId>>,
    roots: Vec<EntryId>,
    /// target id → (label, label-change timestamp); empty/cleared labels are removed.
    labels: HashMap<EntryId, (String, String)>,
    leaf: Option<EntryId>,
    /// Whether the file has been created on disk (deferred-flush; R-04 §6.4).
    flushed: bool,
}

impl SessionManager {
    // ---------------------------------------------------------- internal construction --------

    fn assemble(
        header: SessionHeader,
        cwd: PathBuf,
        store: Box<dyn SessionStore>,
        entries: Vec<Entry>,
        flushed: bool,
    ) -> Self {
        let mut m = Self {
            header,
            cwd,
            store,
            entries,
            by_id: HashMap::new(),
            children: HashMap::new(),
            roots: Vec::new(),
            labels: HashMap::new(),
            leaf: None,
            flushed,
        };
        m.rebuild_index();
        m.leaf = m.entries.last().map(Entry::id);
        m
    }

    fn rebuild_index(&mut self) {
        self.by_id.clear();
        self.children.clear();
        self.roots.clear();
        self.labels.clear();
        // Pass 1: id index + labels (so parent existence can be checked in pass 2).
        for (idx, e) in self.entries.iter().enumerate() {
            self.by_id.insert(e.id(), idx);
            if let Entry::Known(KnownEntry::Label {
                target_id,
                label,
                base,
            }) = e
            {
                apply_label(&mut self.labels, target_id, label, &base.timestamp);
            }
        }
        // Pass 2: parent→children, promoting roots per Pi `getTree` (`session-manager.ts:1210-1223`):
        // a `null` parent, a self-parent (`parentId === id`), and an orphan (parent not present)
        // are ALL treated as roots — so an orphaned subtree is never dropped and a self-parent
        // never recurses into itself.
        for e in &self.entries {
            let id = e.id();
            match e.parent_id() {
                Some(p) if p != id && self.by_id.contains_key(&p) => {
                    self.children.entry(p).or_default().push(id);
                }
                _ => self.roots.push(id),
            }
        }
    }

    // -------------------------------------------------------- write path (R-04-016/032/036) ---

    fn make_base(&self) -> EntryBase {
        EntryBase {
            id: self.mint_id(),
            parent_id: self.leaf.clone(),
            timestamp: now_ts(),
            extra: Default::default(),
        }
    }

    fn mint_id(&self) -> EntryId {
        loop {
            let id = gen_short_id();
            if !self.by_id.contains_key(&id) {
                return id;
            }
        }
    }

    /// Push a fully-formed entry: index it, advance the leaf, persist (R-04-016/032/036).
    fn push_entry(&mut self, entry: Entry) -> Result<EntryId, SessionError> {
        let id = entry.id();
        let parent = entry.parent_id();
        let idx = self.entries.len();
        if let Entry::Known(KnownEntry::Label {
            target_id,
            label,
            base,
        }) = &entry
        {
            let ts = base.timestamp.clone();
            let (t, l) = (target_id.clone(), label.clone());
            apply_label(&mut self.labels, &t, &l, &ts);
        }
        self.entries.push(entry);
        self.by_id.insert(id.clone(), idx);
        match parent {
            Some(p) => self.children.entry(p).or_default().push(id.clone()),
            None => self.roots.push(id.clone()),
        }
        self.leaf = Some(id.clone());
        self.persist_last()?;
        Ok(id)
    }

    fn persist_last(&mut self) -> Result<(), SessionError> {
        if !self.store.is_persisted() {
            return Ok(());
        }
        if self.flushed {
            if let Some(e) = self.entries.last() {
                let line = e.to_line()?;
                self.store.append_line(&line)?;
            }
        } else if self.has_assistant_message() {
            // First assistant message → exclusive-create the file and write everything buffered so
            // far (Pi `_persist` first flush via `openSync(file,"wx")`, `session-manager.ts:926-935`).
            self.store.create_exclusive(&self.header, &self.entries)?;
            self.flushed = true;
        }
        Ok(())
    }

    fn has_assistant_message(&self) -> bool {
        entries_have_assistant(&self.entries)
    }
}

/// Whether any entry is a core `assistant` message (Pi's `hasAssistant` guard,
/// `session-manager.ts:915,1362`). Drives the deferred first-flush.
fn entries_have_assistant(entries: &[Entry]) -> bool {
    entries.iter().any(|e| {
        matches!(e, Entry::Known(KnownEntry::Message { message, .. }) if message.is_core_assistant())
    })
}

fn apply_label(
    labels: &mut HashMap<EntryId, (String, String)>,
    target: &EntryId,
    label: &Option<String>,
    timestamp: &str,
) {
    match label {
        Some(l) if !l.is_empty() => {
            labels.insert(target.clone(), (l.clone(), timestamp.to_string()));
        }
        _ => {
            labels.remove(target);
        }
    }
}
