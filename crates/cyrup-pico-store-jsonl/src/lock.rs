//! [`StoreLock`] — the exclusive store lock ADR-0030 F6 §D adds to a specification that does not ask
//! for one.
//!
//! # The failure this exists to prevent, which is cyrup's and not pi's
//!
//! ADR-0030 F6 §D states it exactly: **cyrup is a CLI.** A user runs `cyrup` in a repository, leaves it
//! open, and runs a second `cyrup` in the same repository in another terminal. `cyrup-session` tolerates
//! that today by design — its reader skips malformed lines and last-good-line wins — *because it has no
//! cross-record invariants to break*. Under Pico5 both processes mint ids from their own durable
//! high-water mark and both allocate commit sequences. Within seconds two records share id 41, a fork's
//! `parent.at` points at the wrong entry, and two incarnations' half-open lifetimes overlap. Nothing
//! errors; the store is silently incoherent and the next reopen publishes a plausible, wrong history.
//!
//! So the lock is a **required argument** to [`JsonlStore::open_for_write`]: not holding it is a compile
//! error (`tests/compile-fail/a_writable_store_needs_the_lock.rs`), which is ADR-0030 §2.2's
//! *"**guarded** cross-process by `StoreLock`"* for the sole-committer row.
//!
//! # And the second mechanism the lock makes sound
//!
//! Because this process is the only appender, it knows each sidecar's byte length before it writes a
//! marker — so the marker can carry those offsets and the open pass never reads a document payload at
//! all (ADR-0030 F6 §D; [`crate::recover`] validates every offset, and [`crate::wire`] is the shape).
//! That mechanism rests on this file.
//!
//! # What an advisory lock does not buy, stated rather than assumed
//!
//! ADR-0030 F6 §D: *"an advisory lock is advisory. A process that does not take it still writes;
//! `flock` is unreliable or absent on NFS and some network filesystems; delete-and-recreate of the
//! lockfile under a held lock is possible on unix."* Hence the holder record below and the durable
//! identity record ([`crate::identity`]): a stale or ignored lock is **detectable** even though it
//! cannot be prevented. It says nothing about a second process *reading* while this one writes, which
//! is legal, desirable, and which the marker protocol already makes safe
//! ([`JsonlStore::open_read_only`]).
//!
//! [`JsonlStore::open_for_write`]: crate::JsonlStore::open_for_write
//! [`JsonlStore::open_read_only`]: crate::JsonlStore::open_read_only

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

/// The lock file's name. A sidecar of its own, never the store's data: it is never renamed over, so a
/// held lock survives every atomic replacement this backend performs.
pub(crate) const LOCK_FILE: &str = "store.lock";

/// What the lock file holds while it is held: who holds it.
///
/// Diagnostic only, and it is the only reason the file has contents at all. `flock` identifies no
/// holder, so without this a refusal could say *"busy"* and nothing else — and the user's question is
/// always *"busy with what?"*.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
struct Holder {
    /// The process that acquired the lock.
    pid: u32,
}

/// RAII proof that this process holds the store exclusively.
///
/// A writable backend cannot be constructed without one, and this type cannot be cloned, copied,
/// defaulted or built from parts — [`StoreLock::acquire`] is the only constructor, so the proof cannot
/// be manufactured by a caller that did not take the lock.
#[derive(Debug)]
pub struct StoreLock {
    /// The held descriptor. Declared second so it closes *after* the explicit unlock in [`Drop`] —
    /// field drops run in declaration order, after the `drop` body.
    dir: PathBuf,
    file: File,
}

/// Why the store could not be locked.
///
/// ADR-0030 F6 §D sketches `acquire` as `Result<Self, StoreBusy>` with one case. There are two in
/// practice and they are not the same answer to a user: the store is in use by a process this one can
/// name, or the lock file itself could not be opened. The name is the ADR's; the second arm is the
/// residue the sketch leaves out.
#[derive(Debug, thiserror::Error)]
pub enum StoreBusy {
    /// Another process holds the store.
    ///
    /// `holder` is the pid from the lock file, absent only when the file could not be read or does not
    /// carry one — which is itself the *"stale or ignored lock"* case F6 §D asks to be detectable.
    #[error("the store at {dir} is open for writing{}", match holder {
        Some(pid) => format!(" by process {pid}"),
        None => " by another process".to_owned(),
    })]
    Held {
        /// The store directory.
        dir: PathBuf,
        /// The holder's pid, if the lock file names one.
        holder: Option<u32>,
    },
    /// The lock file could not be created, opened or locked.
    #[error("the store lock at {dir} could not be taken: {source}")]
    Unavailable {
        /// The store directory.
        dir: PathBuf,
        /// What the filesystem said.
        #[source]
        source: io::Error,
    },
}

impl StoreLock {
    /// Take the store at `dir` exclusively, creating the directory if it does not exist.
    ///
    /// `flock(LOCK_EX | LOCK_NB)` on unix, `LockFileEx` on Windows, through `fs4`. **Non-blocking on
    /// purpose:** a blocking acquire would park a thread inside a syscall no cancellation can
    /// interrupt, and a CLI's answer to *"another cyrup already has this directory"* is to say so, not
    /// to wait. `cyrup-config`'s own lock documents the same choice at length
    /// (`crates/cyrup-config/src/lock.rs`).
    ///
    /// # Errors
    ///
    /// [`StoreBusy::Held`] when another process holds it, naming the holder when the lock file says
    /// who. [`StoreBusy::Unavailable`] when the lock file cannot be created, opened or locked.
    pub fn acquire(dir: &Path) -> Result<Self, StoreBusy> {
        let unavailable = |source: io::Error| StoreBusy::Unavailable {
            dir: dir.to_path_buf(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(unavailable)?;
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(unavailable)?;
        match FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(StoreBusy::Held {
                    dir: dir.to_path_buf(),
                    holder: read_holder(&path),
                });
            }
            Err(TryLockError::Error(e)) => return Err(unavailable(e)),
        }
        let lock = Self {
            dir: dir.to_path_buf(),
            file,
        };
        // Record who holds it, now that we do. A failure here is not fatal: the lock is held, and the
        // holder record is a diagnostic — refusing to open a lockable store because a comment could not
        // be written would be the worse answer.
        lock.write_holder();
        Ok(lock)
    }

    /// The directory this lock covers.
    ///
    /// [`JsonlStore::open_for_write`](crate::JsonlStore::open_for_write) compares it against the
    /// directory it was asked to open, because ADR-0030 F6 §D's sketch takes both and nothing in the
    /// signature makes them agree.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Overwrite the holder record with this process's pid.
    fn write_holder(&self) {
        let holder = Holder {
            pid: std::process::id(),
        };
        let Ok(line) = serde_json::to_string(&holder) else {
            return;
        };
        let mut f: &File = &self.file;
        let _ = f.set_len(0);
        let _ = f.write_all(format!("{line}\n").as_bytes());
        let _ = f.flush();
    }
}

impl Drop for StoreLock {
    /// Release the lock **before** the descriptor closes.
    ///
    /// Closing the fd would release it too, but the explicit unlock makes the order a fact rather than
    /// an accident, and leaves no window in which a successor in this process wakes into a lock this
    /// process still holds.
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Read the holder pid from a lock file this process does not hold.
///
/// Best effort by construction: the file may be mid-rewrite by the holder, empty because the holder
/// crashed between acquiring and recording, or unreadable. Every one of those answers `None`, which the
/// error renders as *"another process"*.
fn read_holder(path: &Path) -> Option<u32> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    let line = text.lines().next()?;
    serde_json::from_str::<Holder>(line).ok().map(|h| h.pid)
}
