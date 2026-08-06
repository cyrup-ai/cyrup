//! `SessionManager` — the facade over the JSONL tree (arch-04 §3.2/§6). Single-writer, `&mut self`
//! mutation; append-only in memory and on disk; the leaf pointer is the only thing branching moves.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use cyrup_core::{EntryId, Message, ModelId, ModelRef, ProviderId, SessionId, Usage};
use serde_json::Value;

use crate::agent_message::AgentMessage;
use crate::context::{build_context_agent_messages, build_context_messages, SessionContext};
use crate::entry::{Entry, EntryBase, KnownEntry};
use crate::ids::validate_session_id;
use crate::error::SessionError;
use crate::header::SessionHeader;
use crate::ids::{gen_session_id, gen_short_id, now_ts};
use crate::layout::SessionLayout;
use crate::store::{DiskStore, MemStore, SessionStore};

/// Options for a new session.
#[derive(Clone, Debug, Default)]
pub struct NewSessionOpts {
    pub id: Option<SessionId>,
    pub parent_session: Option<String>,
}

/// A node of the defensive tree copy returned to UIs (R-04-025).
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub entry: Entry,
    pub children: Vec<TreeNode>,
    pub label: Option<String>,
}

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
    // ---------------------------------------------------------------- lifecycle (R-04-015) ----

    /// Create a new persisted session for `cwd` (file deferred until the first assistant message).
    pub fn create(
        cwd: &Path,
        layout: &SessionLayout,
        opts: NewSessionOpts,
    ) -> Result<Self, SessionError> {
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), cwd.to_string_lossy(), ts.clone());
        header.parent_session = opts.parent_session;
        let path = layout.new_file_path(&ts, id.as_str());
        let store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        Ok(Self::assemble(header, cwd.to_path_buf(), store, Vec::new(), false))
    }

    /// Open an existing session by path, migrating to the current version on load (R-04-004).
    /// An empty/zero-length file initializes a fresh session at that path (Pi parity).
    pub fn open(path: &Path) -> Result<Self, SessionError> {
        Self::open_with_cwd(path, None)
    }

    /// Open an existing session by path, optionally overriding the manager's working directory
    /// (Pi `SessionManager.open(path, sessionDir?, cwdOverride?)`, session-manager.ts:1410): the
    /// effective cwd is `cwd_override ?? header.cwd` (Pi `cwdOverride ?? header?.cwd ?? process.cwd()`).
    /// The override rebinds only the manager's own cwd (what [`Self::cwd`] reports, used for cwd
    /// assertions and rebound services); the on-disk header retains its original cwd, exactly as Pi
    /// leaves `fileEntries`' header untouched (the override lives on the `this.cwd` field only).
    pub fn open_with_cwd(path: &Path, cwd_override: Option<&Path>) -> Result<Self, SessionError> {
        let meta = std::fs::metadata(path);
        // Pi treats a NONEXISTENT `--session <path>` and an EXISTING zero-length file identically:
        // both anchor a brand-new session at that exact path. `loadEntriesFromFile` returns `[]` for a
        // missing file (session-manager.ts:489-491), so `static open` reaches `setSessionFile`'s
        // `!existsSync` branch, which runs `newSession()` and preserves the explicit path
        // (session-manager.ts:843-847); an existing empty file takes the sibling `size === 0` branch
        // (session-manager.ts:822-831). Only a non-empty, unparseable file is an error. cyrup
        // previously special-cased only the zero-length case, so a missing path fell through to
        // `load()` → `File::open` → `NotFound` → a CLI hard-error (gap-analysis 05, Finding 2).
        let missing = matches!(&meta, Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        if missing || matches!(&meta, Ok(m) if m.len() == 0) {
            // Missing/empty file → fresh session anchored here. The override (when given) seeds both
            // the header and the manager cwd, since there is no persisted header to preserve.
            let id = gen_session_id();
            let cwd = cwd_override.map(Path::to_path_buf).unwrap_or_default();
            let header = SessionHeader::new(id, cwd.to_string_lossy(), now_ts());
            let store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
            return Ok(Self::assemble(header, cwd, store, Vec::new(), false));
        }
        let (mut header, mut entries, recovered) = load(path)?;
        let migrated = crate::migrate::to_current(&mut header, &mut entries);
        // Pi `cwdOverride ?? header?.cwd` — the override wins; the persisted header is left intact.
        let cwd = cwd_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&header.cwd));
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        if migrated && !recovered {
            // Rewrite once at the current version (never silently discard a recovered prefix).
            store.rewrite(&header, &entries)?;
        }
        Ok(Self::assemble(header, cwd, store, entries, true))
    }

    /// Import an exported JSONL file and resume at its leaf (R-04-029). Equivalent to `open`.
    pub fn import_jsonl(path: &Path) -> Result<Self, SessionError> {
        Self::open(path)
    }

    /// Continue the most recent session for `cwd`, or create a new one if none exist (R-04-017).
    pub fn continue_recent(
        cwd: &Path,
        layout: &SessionLayout,
    ) -> Result<Self, SessionError> {
        Self::continue_recent_filtered(cwd, layout, false)
    }

    /// Continue the most recent session, optionally restricting the search to sessions whose header
    /// cwd matches `cwd` — Pi's `continueRecent` applies this filter when a custom `sessionDir` that
    /// is not the cwd-default is supplied (`filterCwd`, `session-manager.ts:1426-1434`), so a shared
    /// directory holding sessions from several projects only resumes the current one. With
    /// `filter_cwd = false` (the default layout case, where the dir already encodes the cwd) the
    /// behavior is identical to [`Self::continue_recent`].
    pub fn continue_recent_filtered(
        cwd: &Path,
        layout: &SessionLayout,
        filter_cwd: bool,
    ) -> Result<Self, SessionError> {
        let filter = if filter_cwd { Some(cwd) } else { None };
        match crate::listing::newest_session(&layout.dir(), filter) {
            Some(path) => Self::open(&path),
            None => Self::create(cwd, layout, NewSessionOpts::default()),
        }
    }

    /// An ephemeral session with no file persistence (R-04-027), validating a caller-supplied id
    /// 1:1 with Pi: `inMemory` constructs through the same constructor as a persisted session, whose
    /// `newSession` runs `assertValidSessionId` whenever an id is supplied
    /// (`session-manager.ts:830-831,1437-1439`) — so a malformed id is rejected for an ephemeral
    /// session exactly as for a persisted one. A `None` id is generated and never fails.
    pub fn in_memory(cwd: &Path, opts: NewSessionOpts) -> Result<Self, SessionError> {
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let mut header = SessionHeader::new(id, cwd.to_string_lossy(), now_ts());
        header.parent_session = opts.parent_session;
        Ok(Self::assemble(header, cwd.to_path_buf(), Box::new(MemStore), Vec::new(), false))
    }

    /// Fork a source session into a new file under `target_cwd`, copying all source history
    /// verbatim and recording `parentSession` (R-04-020/022). The source is unchanged.
    pub fn fork_from(
        src: &Path,
        target_cwd: &Path,
        layout: &SessionLayout,
        opts: NewSessionOpts,
    ) -> Result<Self, SessionError> {
        let (_src_header, entries, _recovered) = load(src)?;
        if entries.is_empty() {
            return Err(SessionError::EmptyFork(src.to_path_buf()));
        }
        let id = match opts.id {
            Some(id) => {
                validate_session_id(id.as_str()).map_err(SessionError::InvalidSessionId)?;
                id
            }
            None => gen_session_id(),
        };
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), target_cwd.to_string_lossy(), ts.clone());
        header.parent_session =
            Some(opts.parent_session.unwrap_or_else(|| src.to_string_lossy().into_owned()));
        let path = layout.new_file_path(&ts, id.as_str());
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        // Pi `forkFrom` writes the header with `{flag:"wx"}` (`session-manager.ts:1489`) — exclusive
        // create that refuses to clobber an existing file.
        store.create_exclusive(&header, &entries)?;
        Ok(Self::assemble(header, target_cwd.to_path_buf(), store, entries, true))
    }

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
        let path_entries: Vec<Entry> =
            self.branch_path(Some(leaf_id)).into_iter().cloned().collect();
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
            if let Entry::Known(KnownEntry::Label { target_id, label, .. }) = e {
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

        let previous_session_file = self.session_file().map(|p| p.to_string_lossy().into_owned());
        let persisted = self.store.is_persisted();
        let id = gen_session_id();
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), self.cwd.to_string_lossy(), ts.clone());
        // Pi: `parentSession: this.persist ? previousSessionFile : undefined`
        // (`session-manager.ts:1321`).
        header.parent_session = if persisted { previous_session_file } else { None };

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
            if let Entry::Known(KnownEntry::Label { target_id, label, base }) = e {
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

    // ----------------------------------------------------------------- append (R-04-016) ------

    fn make_base(&self) -> EntryBase {
        EntryBase { id: self.mint_id(), parent_id: self.leaf.clone(), timestamp: now_ts() }
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
        if let Entry::Known(KnownEntry::Label { target_id, label, base }) = &entry {
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

    /// Append a core `user`/`assistant`/`toolResult` message (Pi `appendMessage`,
    /// `session-manager.ts:954`). Backward-compatible: callers still pass a [`cyrup_core::Message`].
    pub fn append_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
        self.append_agent_message(AgentMessage::Core(message))
    }

    /// Append any Pi `AgentMessage` (including the `bashExecution`/`custom` roles) inside a
    /// `type:"message"` entry (Pi `appendMessage(Message | CustomMessage | BashExecutionMessage)`,
    /// `session-manager.ts:954`).
    pub fn append_agent_message(
        &mut self,
        message: AgentMessage,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Message { base: self.make_base(), message }))
    }

    pub fn append_model_change(
        &mut self,
        provider: ProviderId,
        model_id: ModelId,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::ModelChange {
            base: self.make_base(),
            provider,
            model_id,
        }))
    }

    pub fn append_thinking_level_change(
        &mut self,
        level: &str,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::ThinkingLevelChange {
            base: self.make_base(),
            thinking_level: level.to_string(),
        }))
    }

    /// Append a compaction entry (Pi `appendCompaction(summary, firstKeptEntryId, tokensBefore,
    /// details, fromHook, usage)`, `session-manager.ts:1096-1116`). `usage` is the token spend of
    /// the summarization call(s); `None` when the summary came from a hook that reported none.
    pub fn append_compaction(
        &mut self,
        summary: String,
        first_kept: EntryId,
        tokens_before: u64,
        details: Option<Value>,
        usage: Option<Usage>,
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Compaction {
            base: self.make_base(),
            summary,
            first_kept_entry_id: Some(first_kept),
            tokens_before,
            details,
            usage,
            from_hook: Some(from_hook),
        }))
    }

    pub fn append_custom_entry(
        &mut self,
        ty: &str,
        data: Option<Value>,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Custom {
            base: self.make_base(),
            custom_type: ty.to_string(),
            data,
        }))
    }

    pub fn append_custom_message(
        &mut self,
        ty: &str,
        content: Value,
        display: bool,
        details: Option<Value>,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::CustomMessage {
            base: self.make_base(),
            custom_type: ty.to_string(),
            content,
            display,
            details,
        }))
    }

    pub fn append_session_info(&mut self, name: &str) -> Result<EntryId, SessionError> {
        // Pi sanitizes on write: `name.replace(/[\r\n]+/g, " ").trim()` (`session-manager.ts:1031`),
        // so newlines never corrupt the JSONL line and the persisted bytes match Pi.
        self.push_entry(Entry::known(KnownEntry::SessionInfo {
            base: self.make_base(),
            name: Some(sanitize_session_name(name)),
        }))
    }

    pub fn append_label(
        &mut self,
        target: &EntryId,
        label: Option<&str>,
    ) -> Result<EntryId, SessionError> {
        if !self.by_id.contains_key(target) {
            return Err(SessionError::EntryNotFound(target.clone()));
        }
        self.push_entry(Entry::known(KnownEntry::Label {
            base: self.make_base(),
            target_id: target.clone(),
            label: label.map(str::to_string),
        }))
    }

    // ------------------------------------------------------------------ tree read (R-04-010) --

    pub fn leaf_id(&self) -> Option<&EntryId> {
        self.leaf.as_ref()
    }

    pub fn leaf_entry(&self) -> Option<&Entry> {
        self.leaf.as_ref().and_then(|id| self.entry(id))
    }

    pub fn entry(&self, id: &EntryId) -> Option<&Entry> {
        self.by_id.get(id).and_then(|&i| self.entries.get(i))
    }

    pub fn children(&self, id: &EntryId) -> Vec<&Entry> {
        self.children
            .get(id)
            .map(|kids| kids.iter().filter_map(|k| self.entry(k)).collect())
            .unwrap_or_default()
    }

    pub fn label(&self, id: &EntryId) -> Option<&str> {
        self.labels.get(id).map(|(l, _)| l.as_str())
    }

    /// Walk to root from `from` (default: the current leaf), returned root→leaf (R-04-010).
    pub fn branch_path(&self, from: Option<&EntryId>) -> Vec<&Entry> {
        let mut out = Vec::new();
        let mut cur = from.cloned().or_else(|| self.leaf.clone());
        while let Some(id) = cur {
            let e = match self.entry(&id) {
                Some(e) => e,
                None => break,
            };
            out.push(e);
            cur = e.parent_id();
        }
        out.reverse();
        out
    }

    /// Defensive tree copy for UIs. Children are sorted by timestamp, but roots are left in
    /// insertion order: Pi `getTree` sorts only each node's `children` and pushes roots in entry
    /// order (`session-manager.ts:1210-1234`). Observable only with multiple roots (orphan /
    /// self-parent entries); a well-formed session has exactly one root.
    pub fn tree(&self) -> Vec<TreeNode> {
        let mut visited = std::collections::HashSet::new();
        self.roots.iter().filter_map(|id| self.build_node(id, &mut visited)).collect()
    }

    fn build_node(
        &self,
        id: &EntryId,
        visited: &mut std::collections::HashSet<EntryId>,
    ) -> Option<TreeNode> {
        // Cycle guard: a malformed file could still form a non-self loop; never revisit a node.
        if !visited.insert(id.clone()) {
            return None;
        }
        let entry = self.entry(id)?.clone();
        let mut kids = self.children.get(id).cloned().unwrap_or_default();
        kids.sort_by_key(|k| self.entry(k).and_then(|e| e.base()).map(|b| b.timestamp.clone()));
        let children = kids.iter().filter_map(|k| self.build_node(k, visited)).collect();
        Some(TreeNode { entry, children, label: self.label(id).map(str::to_string) })
    }

    // ---------------------------------------------------------------- tree mutate (R-04-009) --

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

    // ------------------------------------------------------------- context (R-04-011/012/013) -

    pub fn build_context(&self) -> SessionContext {
        let path = self.branch_path(None);
        if path.is_empty() {
            return SessionContext::empty();
        }

        let mut thinking = "off".to_string();
        let mut model: Option<ModelRef> = None;
        for e in &path {
            if let Entry::Known(k) = e {
                match k {
                    KnownEntry::ThinkingLevelChange { thinking_level, .. } => {
                        thinking = thinking_level.clone();
                    }
                    KnownEntry::ModelChange { provider, model_id, .. } => {
                        model = Some(ModelRef {
                            provider: provider.clone(),
                            api: None,
                            model: model_id.clone(),
                        });
                    }
                    KnownEntry::Message { message: AgentMessage::Core(Message::Assistant(a)), .. } => {
                        model = Some(a.model_ref());
                    }
                    _ => {}
                }
            }
        }

        let messages = build_context_messages(&path);
        SessionContext { messages, thinking_level: thinking, model }
    }

    /// The active-path context with its **roles intact** — Pi's
    /// `buildContextEntries().flatMap(sessionEntryToContextMessages)` (`session-manager.ts:441-453`
    /// composed with `:383-408`), i.e. [`build_context`](Self::build_context) *without* the
    /// `convertToLlm` flattening.
    ///
    /// [`build_context`](Self::build_context) is the LLM boundary: it renders a `compaction`,
    /// `branch_summary`, `custom_message` or `bashExecution` entry down to a `user` message carrying
    /// the wrapper prose the model conditions on. A UI that replays a resumed session must NOT see
    /// that flattening — Pi's `renderSessionEntries` feeds the raw projection so each role still
    /// reaches its own component (`CompactionSummaryMessageComponent`, `BranchSummaryMessageComponent`,
    /// `CustomMessageComponent`, `BashExecutionComponent`; interactive-mode.ts:3506-3516, :3308-3350).
    /// This is that projection.
    ///
    /// Note a `!!`-prefixed (`excludeFromContext`) bash message is PRESENT here and absent from
    /// [`build_context`](Self::build_context) — Pi's raw context keeps it too (`messages.ts:153-155`
    /// drops it only in `convertToLlm`), which is why the user still sees their own `!!` command
    /// after a resume.
    pub fn build_context_raw(&self) -> Vec<AgentMessage> {
        let path = self.branch_path(None);
        if path.is_empty() {
            return Vec::new();
        }
        build_context_agent_messages(&path)
    }

    // ------------------------------------------------------------- export / accessors ---------

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

/// Pi name sanitization for `appendSessionInfo`: collapse any run of `\r`/`\n` to a single space,
/// then trim (`session-manager.ts:1031`).
fn sanitize_session_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_newline_run = false;
    for ch in name.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_newline_run {
                out.push(' ');
                in_newline_run = true;
            }
        } else {
            out.push(ch);
            in_newline_run = false;
        }
    }
    out.trim().to_string()
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

/// Tolerant streaming load (R-04-034/032): header on line 1, entries after; a malformed or
/// truncated trailing line is dropped (valid prefix kept), returning `recovered = true`.
fn load(path: &Path) -> Result<(SessionHeader, Vec<Entry>, bool), SessionError> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut header: Option<SessionHeader> = None;
    let mut entries: Vec<Entry> = Vec::new();
    let mut recovered = false;

    for (lineno, line) in reader.lines().enumerate() {
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
        if lineno == 0 {
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
