//! `SessionManager` — the facade over the JSONL tree (arch-04 §3.2/§6). Single-writer, `&mut self`
//! mutation; append-only in memory and on disk; the leaf pointer is the only thing branching moves.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use cyrup_core::{EntryId, Message, ModelId, ModelRef, ProviderId, SessionId};
use serde_json::Value;

use crate::context::{compaction_summary_message, push_as_message, SessionContext};
use crate::entry::{Entry, EntryBase, KnownEntry};
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
        let id = opts.id.unwrap_or_else(gen_session_id);
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
        let meta = std::fs::metadata(path);
        if matches!(&meta, Ok(m) if m.len() == 0) {
            // Empty file → fresh session anchored here.
            let id = gen_session_id();
            let header = SessionHeader::new(id, String::new(), now_ts());
            let store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
            return Ok(Self::assemble(header, PathBuf::new(), store, Vec::new(), false));
        }
        let (mut header, mut entries, recovered) = load(path)?;
        let migrated = crate::migrate::to_current(&mut header, &mut entries);
        let cwd = PathBuf::from(&header.cwd);
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
        match newest_session_file(layout) {
            Some(path) => Self::open(&path),
            None => Self::create(cwd, layout, NewSessionOpts::default()),
        }
    }

    /// An ephemeral session with no file persistence (R-04-027).
    pub fn in_memory(cwd: &Path, opts: NewSessionOpts) -> Self {
        let id = opts.id.unwrap_or_else(gen_session_id);
        let mut header = SessionHeader::new(id, cwd.to_string_lossy(), now_ts());
        header.parent_session = opts.parent_session;
        Self::assemble(header, cwd.to_path_buf(), Box::new(MemStore), Vec::new(), false)
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
        let id = opts.id.unwrap_or_else(gen_session_id);
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), target_cwd.to_string_lossy(), ts.clone());
        header.parent_session =
            Some(opts.parent_session.unwrap_or_else(|| src.to_string_lossy().into_owned()));
        let path = layout.new_file_path(&ts, id.as_str());
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        store.rewrite(&header, &entries)?;
        Ok(Self::assemble(header, target_cwd.to_path_buf(), store, entries, true))
    }

    /// Clone the current active path through the current leaf into a new file (R-04-021).
    /// `Label` entries are dropped and the retained path re-chained, then labels for retained
    /// targets re-attached as trailing entries (so removing labels never orphans a subtree).
    pub fn clone_session(&self, layout: &SessionLayout) -> Result<Self, SessionError> {
        let path_entries: Vec<Entry> = self.branch_path(None).into_iter().cloned().collect();
        if path_entries.is_empty() {
            return Err(SessionError::EmptyFork(
                self.session_file().map(Path::to_path_buf).unwrap_or_default(),
            ));
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

        // Re-attach labels for retained targets as trailing label entries.
        let retained_ids: std::collections::HashSet<EntryId> =
            retained.iter().map(Entry::id).collect();
        for e in &path_entries {
            if let Entry::Known(KnownEntry::Label { target_id, label, .. }) = e {
                if retained_ids.contains(target_id) {
                    let base = EntryBase {
                        id: gen_short_id(),
                        parent_id: prev.clone(),
                        timestamp: now_ts(),
                    };
                    let lbl = Entry::known(KnownEntry::Label {
                        base,
                        target_id: target_id.clone(),
                        label: label.clone(),
                    });
                    prev = Some(lbl.id());
                    retained.push(lbl);
                }
            }
        }

        let id = gen_session_id();
        let ts = now_ts();
        let mut header = SessionHeader::new(id.clone(), self.cwd.to_string_lossy(), ts.clone());
        header.parent_session =
            self.session_file().map(|p| p.to_string_lossy().into_owned());
        let path = layout.new_file_path(&ts, id.as_str());
        let mut store: Box<dyn SessionStore> = Box::new(DiskStore::new(path));
        store.rewrite(&header, &retained)?;
        Ok(Self::assemble(header, self.cwd.clone(), store, retained, true))
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
        for (idx, e) in self.entries.iter().enumerate() {
            let id = e.id();
            self.by_id.insert(id.clone(), idx);
            match e.parent_id() {
                Some(p) => self.children.entry(p).or_default().push(id.clone()),
                None => self.roots.push(id.clone()),
            }
            if let Entry::Known(KnownEntry::Label { target_id, label, base }) = e {
                apply_label(&mut self.labels, target_id, label, &base.timestamp);
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
            // First assistant message → create the file and write everything buffered so far.
            self.store.rewrite(&self.header, &self.entries)?;
            self.flushed = true;
        }
        Ok(())
    }

    fn has_assistant_message(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(e, Entry::Known(KnownEntry::Message { message: Message::Assistant(_), .. }))
        })
    }

    pub fn append_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
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

    pub fn append_compaction(
        &mut self,
        summary: String,
        first_kept: EntryId,
        tokens_before: u64,
        details: Option<Value>,
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::Compaction {
            base: self.make_base(),
            summary,
            first_kept_entry_id: first_kept,
            tokens_before,
            details,
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
        self.push_entry(Entry::known(KnownEntry::SessionInfo {
            base: self.make_base(),
            name: Some(name.to_string()),
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

    /// Defensive tree copy for UIs (children sorted by timestamp).
    pub fn tree(&self) -> Vec<TreeNode> {
        let mut roots = self.roots.clone();
        roots.sort_by_key(|id| self.entry(id).and_then(|e| e.base()).map(|b| b.timestamp.clone()));
        roots.iter().filter_map(|id| self.build_node(id)).collect()
    }

    fn build_node(&self, id: &EntryId) -> Option<TreeNode> {
        let entry = self.entry(id)?.clone();
        let mut kids = self.children.get(id).cloned().unwrap_or_default();
        kids.sort_by_key(|k| self.entry(k).and_then(|e| e.base()).map(|b| b.timestamp.clone()));
        let children = kids.iter().filter_map(|k| self.build_node(k)).collect();
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
        from_hook: bool,
    ) -> Result<EntryId, SessionError> {
        self.push_entry(Entry::known(KnownEntry::BranchSummary {
            base: self.make_base(),
            from_id,
            summary,
            details,
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
        let mut compaction: Option<&Entry> = None;
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
                    KnownEntry::Message { message: Message::Assistant(a), .. } => {
                        model = Some(a.model_ref());
                    }
                    KnownEntry::Compaction { .. } => compaction = Some(e),
                    _ => {}
                }
            }
        }

        let mut messages = Vec::new();
        match compaction {
            Some(ce) => {
                if let Entry::Known(KnownEntry::Compaction {
                    summary,
                    first_kept_entry_id,
                    tokens_before,
                    ..
                }) = ce
                {
                    messages.push(compaction_summary_message(summary, *tokens_before));
                    if let Some(cpos) = path.iter().position(|e| std::ptr::eq(*e, ce)) {
                        if let Some(before) = path.get(..cpos) {
                            let mut keeping = false;
                            for e in before {
                                if &e.id() == first_kept_entry_id {
                                    keeping = true;
                                }
                                if keeping {
                                    push_as_message(&mut messages, e);
                                }
                            }
                        }
                        if let Some(after) = path.get(cpos + 1..) {
                            for e in after {
                                push_as_message(&mut messages, e);
                            }
                        }
                    }
                }
            }
            None => {
                for e in &path {
                    push_as_message(&mut messages, e);
                }
            }
        }

        SessionContext { messages, thinking_level: thinking, model }
    }

    // ------------------------------------------------------------- export / accessors ---------

    /// Write the session as JSONL (header + entries) for export (R-04-029).
    pub fn export_jsonl(&self, w: &mut dyn Write) -> Result<(), SessionError> {
        w.write_all(serde_json::to_string(&self.header)?.as_bytes())?;
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

    /// The most recent session display name (latest `SessionInfo`, R-04-028).
    pub fn session_name(&self) -> Option<String> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Known(KnownEntry::SessionInfo { name, .. }) => name.clone(),
            _ => None,
        })
    }
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

/// Newest `*.jsonl` file (by mtime) in the layout's cwd directory, if any (R-04-017).
fn newest_session_file(layout: &SessionLayout) -> Option<PathBuf> {
    let dir = layout.dir();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if best.as_ref().is_none_or(|(t, _)| modified >= *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}
