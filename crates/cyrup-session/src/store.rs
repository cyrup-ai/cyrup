//! Persistence seam (arch-04 §3.3). `DiskStore` = atomic append-only JSONL; `MemStore` = no-op
//! ephemeral backing (R-04-027). Keeps `SessionManager` logic identical for both.
//!
//! # Durability, and why the write is synchronous but the flush is not
//!
//! Pi's `_persist` (`session-manager.ts:1016-1043`) appends with `appendFileSync` (`:1041` in the
//! steady state, `:1022` before the first assistant message) and writes the first flush with
//! `writeFileSync` into an `openSync(file,"wx")` fd (`:1031-1034`). It **never** calls
//! `fsync`/`fdatasync` — `grep -n 'fsync\|fdatasync' session-manager.ts` returns nothing — so when
//! it returns, the bytes are in the page cache. cyrup deliberately goes further and flushes them to
//! the device, so a session survives power loss where pi's would not.
//!
//! That guarantee is kept — it is just no longer paid for on the caller's thread:
//! [`DiskStore::append_line`] hands the `fdatasync` to [`SESSION_SYNCER`], which costs the turn
//! ~1.5 µs instead of ~214 µs (PERF-004 §7).
//!
//! **The `write(2)` itself stays synchronous on purpose. Do not "simplify" it onto the worker.**
//! `write` hands ownership of the bytes to the kernel, so once it returns they are visible to every
//! reader and safe against a `SIGKILL`, an `abort`, or a `std::process::exit` — the kernel is what
//! writes them back. Moving the write to a background thread would leave them in a *userspace*
//! queue that dies with the process, which is **weaker than pi**, not stronger. Only the device
//! flush is deferred, and only power-loss durability is briefly outstanding.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, LazyLock, Mutex};

use crate::entry::Entry;
use crate::error::SessionError;
use crate::header::SessionHeader;

/// Abstraction over where a session's bytes live.
pub trait SessionStore: Send {
    /// `None` for in-memory (ephemeral) sessions.
    fn path(&self) -> Option<&Path>;
    fn is_persisted(&self) -> bool;
    /// Append ONE serialized entry line atomically (R-04-032). No-op in memory.
    fn append_line(&mut self, line: &str) -> Result<(), SessionError>;
    /// Rewrite the whole file (migration / clone eager-seed). No-op in memory.
    fn rewrite(&mut self, header: &SessionHeader, entries: &[Entry]) -> Result<(), SessionError>;
    /// Exclusive-create the file (header + entries), failing rather than clobbering a pre-existing
    /// file — Pi's `openSync(file,"wx")` first-flush and `writeFileSync … {flag:"wx"}` fork header
    /// (`session-manager.ts:927,1489`). This is the duplicate-header guard. No-op in memory.
    fn create_exclusive(
        &mut self,
        header: &SessionHeader,
        entries: &[Entry],
    ) -> Result<(), SessionError>;
}

/// A background `fdatasync` request: the handle to flush, plus the slot its failure is
/// reported through (see `DiskStore::sync_err`).
enum SyncReq {
    Sync {
        file: Arc<File>,
        err: Arc<Mutex<Option<std::io::Error>>>,
    },
    /// Answered once every request queued ahead of it has been flushed.
    Barrier(Sender<()>),
}

/// The one session-fsync worker for the whole process.
///
/// [`DiskStore::append_line`] keeps its `write(2)` synchronous — so the bytes are in the page
/// cache, and therefore visible to every reader and safe against a process crash, before it
/// returns — and hands only the `fdatasync` here. Global rather than per-store because
/// `SessionManager` swaps its `Box<dyn SessionStore>` on a branch
/// (`manager/branched_session.rs:145`) and several call sites open a second manager over a live
/// file, so a per-store worker would neither bound the thread count nor order those writes
/// against each other.
static SESSION_SYNCER: LazyLock<Syncer> = LazyLock::new(Syncer::start);

struct Syncer {
    /// `None` when the worker thread could not be spawned — `request` then syncs inline, i.e.
    /// degrades to exactly the pre-PERF-004 behaviour rather than silently dropping durability.
    tx: Option<Sender<SyncReq>>,
}

impl Syncer {
    fn start() -> Self {
        let (tx, rx) = channel::<SyncReq>();
        match std::thread::Builder::new()
            .name("cyrup-session-fsync".into())
            .spawn(move || run(rx))
        {
            Ok(_handle) => Self { tx: Some(tx) },
            // Spawn failed (EAGAIN / thread rlimit). Fall back to syncing inline — i.e. the old
            // cost and the same guarantee — rather than dropping the flush silently.
            Err(_) => Self { tx: None },
        }
    }

    fn request(&self, file: &Arc<File>, err: &Arc<Mutex<Option<std::io::Error>>>) {
        let Some(tx) = &self.tx else {
            return Self::sync_inline(file, err);
        };
        let req = SyncReq::Sync {
            file: Arc::clone(file),
            err: Arc::clone(err),
        };
        if tx.send(req).is_err() {
            // The worker can only end if the static's sender is dropped, which never happens for
            // a `LazyLock` static — but if it somehow does, do not lose the flush.
            Self::sync_inline(file, err);
        }
    }

    fn sync_inline(file: &Arc<File>, err: &Arc<Mutex<Option<std::io::Error>>>) {
        if let Err(e) = file.sync_data() {
            let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(e);
        }
    }

    /// Block until every request enqueued before this call has been flushed.
    fn barrier(&self) {
        // In inline mode every sync already completed synchronously; nothing can be pending.
        let Some(tx) = &self.tx else { return };
        let (ack_tx, ack_rx) = channel::<()>();
        if tx.send(SyncReq::Barrier(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}

/// One queued flush: the handle, plus the slot its failure is reported through. Named because
/// `clippy::type_complexity` fires on the bare tuple below and the gate expects zero warnings.
type PendingSync = (Arc<File>, Arc<Mutex<Option<std::io::Error>>>);

fn run(rx: Receiver<SyncReq>) {
    // Reused across rounds to keep the loop allocation-free in steady state.
    let mut round: Vec<PendingSync> = Vec::new();
    let mut acks: Vec<Sender<()>> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();

    while let Ok(first) = rx.recv() {
        round.clear();
        acks.clear();
        seen.clear();

        // Drain everything already queued behind the message that woke us: an entire burst
        // collapses into one flush per file. This is the debounce — no timer needed.
        for req in std::iter::once(first).chain(rx.try_iter()) {
            match req {
                SyncReq::Sync { file, err } => round.push((file, err)),
                SyncReq::Barrier(ack) => acks.push(ack),
            }
        }

        // Dedup by fd identity, not by path: two stores can name one path, and one path can name
        // two inodes across a `rewrite`. Every `Arc` stays alive in `round` for the whole loop,
        // so an address cannot be freed and reused mid-dedup (which would silently skip a real
        // flush). That is why the requests are collected first rather than synced as they drain.
        for (file, err) in &round {
            if !seen.insert(Arc::as_ptr(file) as usize) {
                continue;
            }
            if let Err(e) = file.sync_data() {
                // Sticky, take-once — surfaced by the next `append_line`.
                let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
                *slot = Some(e);
            }
        }

        // Acked only after the whole round flushed, so a barrier strictly follows every request
        // enqueued before it (mpsc is FIFO, so "enqueued before" == "drained before").
        for ack in acks.drain(..) {
            let _ = ack.send(());
        }
        round.clear();
    }
}

/// Block until every `sync_data` requested before this call has completed. Costs one flush
/// round (~200 µs); a no-op when nothing is pending. This is a *power-loss* guarantee only — the
/// bytes are already in the page cache, so no process-exit path can lose them — which is why it is
/// a courtesy at teardown rather than a correctness requirement.
pub fn flush_session_writes() {
    SESSION_SYNCER.barrier();
}

/// Append-only JSONL file backing.
pub struct DiskStore {
    path: PathBuf,
    /// The held `O_APPEND` handle, opened lazily on the first append and shared with
    /// [`SESSION_SYNCER`]. `None` means "not opened yet, or invalidated" — a write error and
    /// `rewrite`'s inode swap both clear it, so a broken or stale fd is never cached.
    ///
    /// One consequence is deliberate: where the pre-PERF-004 reopen-per-append silently
    /// *recreated* a session file deleted underneath a live manager — leaving a headerless stub —
    /// the held fd lets the file stay deleted, which is what the deleting caller asked for. Do not
    /// add recreate-on-delete back.
    file: Option<Arc<File>>,
    /// Sticky failure from a background `sync_data`, reported by the next [`Self::append_line`].
    /// Typed, not stringified, so `ErrorKind` (e.g. ENOSPC) survives the deferral.
    sync_err: Arc<Mutex<Option<std::io::Error>>>,
}

impl DiskStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
            sync_err: Arc::new(Mutex::new(None)),
        }
    }

    fn handle(&mut self) -> Result<Arc<File>, SessionError> {
        if let Some(f) = &self.file {
            return Ok(Arc::clone(f));
        }
        let f = Arc::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.file = Some(Arc::clone(&f));
        Ok(f)
    }

    fn take_sync_error(&self) -> Option<std::io::Error> {
        self.sync_err
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }
}

impl Drop for DiskStore {
    /// One last flush of whatever this store wrote. Non-blocking: the syncer holds its own
    /// `Arc<File>` clone, so the fd outlives this `DiskStore` exactly long enough to be flushed.
    /// This is the branch path — `adopt_branch` replaces `self.store`
    /// (`manager/branched_session.rs:145`), dropping the outgoing store here.
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            SESSION_SYNCER.request(f, &self.sync_err);
        }
    }
}

impl SessionStore for DiskStore {
    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn is_persisted(&self) -> bool {
        true
    }

    fn append_line(&mut self, line: &str) -> Result<(), SessionError> {
        // The `fdatasync` is the only part of this write that is not synchronous, so this is where
        // its failure re-enters the caller's `Result`. Take-once: reported exactly one time, then
        // cleared. Taken up-front but returned at the BOTTOM — a stale flush error must not cost
        // us this entry, which would turn a degraded power-loss guarantee on an EARLIER entry into
        // immediate, permanent loss of this one.
        let deferred = self.take_sync_error();
        let file = self.handle()?;
        // ONE `write` of `<json>\n` to an append-mode fd: a crash mid-write leaves a partial
        // final line that the tolerant reader drops ("last good line wins" — R-04-032). The buffer
        // is assembled first *precisely* so this is a single `write(2)`: `O_APPEND` atomicity is
        // per-call, and that is what bounds a crash — or a concurrent appender on the same file —
        // to at most one partial final line.
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        let mut w: &File = &file;
        if let Err(e) = w.write_all(&buf) {
            self.file = None; // never cache a broken fd; the next append reopens
            return Err(e.into());
        }
        // The bytes are the kernel's now — visible to every reader, and safe against a process
        // crash. Only the device flush is deferred.
        SESSION_SYNCER.request(&file, &self.sync_err);
        match deferred {
            Some(e) => Err(SessionError::Io(e)),
            None => Ok(()),
        }
    }

    fn rewrite(&mut self, header: &SessionHeader, entries: &[Entry]) -> Result<(), SessionError> {
        // MANDATORY: the rename below swaps the inode out from under any held handle. Keeping one
        // would send every later append into the old, unlinked inode — where the write SUCCEEDS,
        // returns `Ok(())`, and is destroyed when the last handle closes. Silent data loss.
        //
        // No current call site can reach that state: all three construct the store a few lines
        // earlier and never append to it first (`manager/lifecycle.rs:85`->`:96`, `:116`->`:119`,
        // `manager/branched_session.rs:120`->`:126`). That is an accident of three call sites, not
        // an enforced property, so do not remove this line on the strength of it.
        self.file = None;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // [CYRUP-DELTA] Pi's `_rewriteFile` truncates the live file in place —
        // `const fd = openSync(this.sessionFile, "w"); … writeFileSync(fd, …)`
        // (`session-manager.ts:979-988` @v0.83.0) — so a crash between the truncate and the last
        // `writeFileSync` leaves a half-rewritten session with no recovery path. cyrup writes a
        // temp sibling and renames over the target instead. The reason this is not spelled pi's
        // way: `rewrite` is only ever called to persist a MIGRATION (`manager.rs`'s
        // `migrated && !recovered` gate) or an eager clone seed, i.e. exactly the two moments the
        // file's only copy of the user's history is being rebuilt from memory, and cyrup has no
        // equivalent of pi's `preloadedFileEntries` re-read to fall back on. The visible
        // differences are confined to the failure path plus a new inode on success; the resulting
        // bytes are identical.
        let tmp = self.path.with_extension("jsonl.tmp");
        let mut buf = String::new();
        buf.push_str(&serde_json::to_string(header)?);
        buf.push('\n');
        for e in entries {
            buf.push_str(&e.to_line()?);
            buf.push('\n');
        }
        {
            let mut f = File::create(&tmp)?;
            f.write_all(buf.as_bytes())?;
            f.flush()?;
            f.sync_data()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn create_exclusive(
        &mut self,
        header: &SessionHeader,
        entries: &[Entry],
    ) -> Result<(), SessionError> {
        // Same reason as `rewrite`: this creates a fresh inode at the path, so no previously held
        // handle can still describe it.
        self.file = None;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `create_new` is the atomic exclusive-create equivalent of Pi's `"wx"` flag: if the file
        // already exists it errors (`AlreadyExists`/EEXIST) instead of overwriting, guarding the
        // duplicate-header bug (`session-manager.ts:927,1489`). Written directly (no temp+rename)
        // to mirror Pi, which writes header+entries straight to the freshly-created fd.
        let mut f = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(SessionError::AlreadyExists(self.path.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        let mut buf = String::new();
        buf.push_str(&serde_json::to_string(header)?);
        buf.push('\n');
        for e in entries {
            buf.push_str(&e.to_line()?);
            buf.push('\n');
        }
        f.write_all(buf.as_bytes())?;
        f.flush()?;
        f.sync_data()?;
        Ok(())
    }
}

/// No-op backing for ephemeral sessions (R-04-027): never touches the filesystem.
pub struct MemStore;

impl SessionStore for MemStore {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn is_persisted(&self) -> bool {
        false
    }

    fn append_line(&mut self, _line: &str) -> Result<(), SessionError> {
        Ok(())
    }

    fn rewrite(&mut self, _header: &SessionHeader, _entries: &[Entry]) -> Result<(), SessionError> {
        Ok(())
    }

    fn create_exclusive(
        &mut self,
        _header: &SessionHeader,
        _entries: &[Entry],
    ) -> Result<(), SessionError> {
        Ok(())
    }
}
