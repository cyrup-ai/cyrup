//! [`Appender`] — one `O_APPEND` handle, with the fd-invalidation discipline ADR-0030 §9 lifts from
//! `cyrup-session`.
//!
//! # The three properties, and why each is written the way it is
//!
//! **One `write(2)` per line.** The buffer is assembled first *precisely* so the line reaches the kernel
//! in a single call: `O_APPEND` atomicity is per-call, and that is what bounds a crash — or a second
//! appender that ignored the lock — to at most one partial final line, which [`crate::recover`] removes.
//!
//! **The write is synchronous; only the flush is deferred.** `write` hands the bytes to the kernel, so
//! once it returns they are visible to every reader and safe against a `SIGKILL`, an `abort` or a
//! `std::process::exit`. That is the whole of [`Durability::ProcessCrash`]
//! (`spec.md:4419-4421`: *"Without `fsync`, it guarantees ordinary process-crash consistency"*), and it
//! is why the append must not move onto [`crate::syncer`]'s worker.
//!
//! **The descriptor is invalidated on every write error and before every replacement.** ADR-0030 §9
//! calls this *"literally §11.3's 'invalidates cached file descriptors so future appends cannot target
//! an unlinked inode' — already written and already reasoned about"*. `cyrup-session`'s comment states
//! the failure in full: an append to an unlinked inode **succeeds**, returns `Ok(())`, and is destroyed
//! when the last handle closes. Silent data loss.
//!
//! # The length is tracked, not measured
//!
//! The offset [`Appender::append`] returns is what the marker-carried offsets of ADR-0030 F6 §D are
//! made of, and it is maintained in memory rather than read back with `metadata()`. That is sound **only** because the
//! store lock makes this process the only appender — which is exactly the dependency ADR-0030 §14 open
//! question 3 asks about, and the reason [`crate::recover`] validates every offset against the file's
//! actual length at open instead of trusting it.
//!
//! [`Durability::ProcessCrash`]: crate::Durability::ProcessCrash

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::syncer::{self, SyncErr};

/// An append-only handle to one file, with its tracked length.
#[derive(Debug)]
pub(crate) struct Appender {
    path: PathBuf,
    /// The held `O_APPEND` handle, opened lazily and shared with the flush worker. `None` means *"not
    /// opened yet, or invalidated"*.
    file: Option<Arc<File>>,
    /// The file's length as of the last successful append.
    len: u64,
    /// Sticky failure from a deferred flush, reported by the next caller that asks.
    sync_err: SyncErr,
}

impl Appender {
    /// A handle to `path`, whose current length is `len`.
    ///
    /// `len` comes from recovery, which has just truncated the file to its last confirmed byte, so the
    /// two cannot disagree at construction.
    pub(crate) fn new(path: PathBuf, len: u64) -> Self {
        Self {
            path,
            file: None,
            len,
            sync_err: Arc::new(Mutex::new(None)),
        }
    }

    /// This handle's path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Append one line, returning the file's new length.
    ///
    /// The newline is added here, so no caller can forget it and glue two records into one.
    ///
    /// # Errors
    ///
    /// [`io::Error`] from the open or the write. The descriptor is invalidated on a write failure, so a
    /// retry reopens rather than writing into a handle whose state is unknown. A *partial* write leaves
    /// a torn final line, which is the one damage shape recovery is built to remove — the caller's job
    /// is to classify the failure, and in this backend every post-admission write failure is
    /// `Uncertain` ([`crate::store`]).
    pub(crate) fn append(&mut self, line: &str) -> io::Result<u64> {
        let file = self.handle()?;
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        let mut w: &File = &file;
        if let Err(e) = w.write_all(&buf) {
            self.file = None;
            return Err(e);
        }
        self.len = self.len.saturating_add(buf.len() as u64);
        // The bytes are the kernel's now. Only the device flush is deferred.
        syncer::request(&file, &self.sync_err);
        Ok(self.len)
    }

    /// Flush this file to the device and wait for it.
    ///
    /// One coalesced round, shared with every other flush queued at the same moment. This is what
    /// `spec.md:4421-4424` means by *"flushes each affected sidecar, and only then appends the main
    /// marker"*: the wait is the ordering.
    ///
    /// # Errors
    ///
    /// [`io::Error`] from this flush or from any earlier deferred one, taken once.
    pub(crate) fn flush_now(&mut self) -> io::Result<()> {
        if let Some(file) = &self.file {
            syncer::request(file, &self.sync_err);
        }
        syncer::barrier();
        self.take_error()
    }

    /// Report, once, any failure a deferred flush discovered.
    ///
    /// # Errors
    ///
    /// [`io::Error`] from a deferred flush. Take-once: a failure is reported exactly one time and then
    /// cleared, because a sticky error that is reported forever turns one degraded power-loss guarantee
    /// into a permanently unusable store.
    pub(crate) fn take_error(&mut self) -> io::Result<()> {
        match self
            .sync_err
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Drop the cached descriptor.
    ///
    /// **Mandatory before anything replaces the file this handle names.** See the module documentation:
    /// an append into an unlinked inode succeeds and is then destroyed.
    pub(crate) fn invalidate(&mut self) {
        self.file = None;
    }

    /// Point this handle at a new file of a known length, invalidating the old descriptor first.
    ///
    /// Used by reclamation, which replaces a sidecar generation with a shorter one.
    pub(crate) fn rebind(&mut self, path: PathBuf, len: u64) {
        self.invalidate();
        self.path = path;
        self.len = len;
    }

    fn handle(&mut self) -> io::Result<Arc<File>> {
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
}

impl Drop for Appender {
    /// One last flush of whatever this handle wrote.
    ///
    /// Non-blocking: the worker holds its own `Arc<File>` clone, so the descriptor outlives this value
    /// exactly long enough to be flushed. A store dropped without [`close`](crate::JsonlStore) still
    /// gets its bytes to the device.
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            syncer::request(f, &self.sync_err);
        }
    }
}
