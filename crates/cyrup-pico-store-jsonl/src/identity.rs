//! `store.json` — the durable store identity ADR-0030 F6 §D asks for *"so a stale or ignored lock is
//! detectable"*.
//!
//! Three fields, each earning its place:
//!
//! * **`format`** — the on-disk format version. The log is append-only and never compacted
//!   (`spec.md:4447-4448`), so a future format cannot be migrated by rewriting; the version is what
//!   lets a build refuse a store it does not understand instead of reading it wrongly.
//! * **`store`** — the [`StoreId`] every cursor this store issues is stamped with (ADR-0030 F6 §B).
//!   It has to be **durable**, not per-process: a cursor from a reopened store resumes the same scan,
//!   and `MemoryStore`'s process counter cannot offer that.
//! * **`holder_pid`** — the process that most recently opened the store **for writing**. This is the
//!   detectability half of F6 §D's *"an advisory lock is advisory"*: a holder pid that names a live
//!   process while this one holds the `flock` means something wrote without taking the lock, and a
//!   holder pid naming a dead process is the ordinary trace of a crash. It is deliberately **not**
//!   cleared on close, because a crash would not clear it either — a field that is only accurate on
//!   the clean path tells you nothing on the path you are debugging.

use std::io;
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};

use cyrup_pico_store::{Corruption, StorageFailure, StoreId};
use serde::{Deserialize, Serialize};

use crate::durable::{durable_rename, fsync_dir};

/// The identity file's name.
pub(crate) const IDENTITY_FILE: &str = "store.json";

/// This build's on-disk format version.
///
/// Bumped only by a change that an older build would read **wrongly** — a new line kind an old build
/// would skip is not one, because [`crate::recover`] treats an unknown line as corruption rather than
/// skipping it.
pub const FORMAT_VERSION: u32 = 1;

/// One store's durable identity.
///
/// `Deserialize` is derived, and that is within ADR-0030 §10's rule rather than an exception to it: the
/// invariant-bearing field is [`StoreId`], whose own `Deserialize` is hand-written (it requires a
/// 32-digit nonzero hex string and rejects the number form), and `format` carries no invariant a
/// constructor could establish — a value this build does not know is a *compatibility* answer, decided
/// by [`StoreIdentity::read`], not a decode error.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StoreIdentity {
    /// The on-disk format version.
    pub format: u32,
    /// This store's durable identity, stamped into every cursor it issues.
    pub store: StoreId,
    /// The process that most recently opened this store for writing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
}

impl StoreIdentity {
    /// Read `dir`'s identity, or `Ok(None)` when the store does not exist yet.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Corrupt`] when the file exists and cannot be decoded — absence is `Ok(None)`
    /// and damage is an error, which is `spec.md:4352-4360`'s split applied to the identity record
    /// itself. [`StorageFailure::Io`] with [`io::ErrorKind::InvalidData`] for a format this build does
    /// not know, and for every other filesystem failure.
    pub(crate) fn read(dir: &Path) -> Result<Option<Self>, StorageFailure> {
        let path = path_in(dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StorageFailure::Io(e)),
        };
        let identity: Self = serde_json::from_str(&text).map_err(|e| {
            StorageFailure::Corrupt(Corruption::RecordMalformed {
                what: "store identity",
                detail: e.to_string(),
            })
        })?;
        if identity.format != FORMAT_VERSION {
            return Err(StorageFailure::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the store at {} is format {}, and this build reads format {FORMAT_VERSION}",
                    dir.display(),
                    identity.format
                ),
            )));
        }
        Ok(Some(identity))
    }

    /// Create `dir`'s identity with a fresh [`StoreId`], durably.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Io`]. `create_new` rather than a write, so two processes racing to initialise
    /// one directory cannot both believe they minted its identity — one gets `EEXIST`. That race is
    /// already excluded by the lock; this is the cheaper guard that does not depend on it.
    pub(crate) fn create(dir: &Path) -> Result<Self, StorageFailure> {
        let identity = Self {
            format: FORMAT_VERSION,
            store: fresh_store_id(),
            holder_pid: Some(std::process::id()),
        };
        let path = path_in(dir);
        let text = serde_json::to_string_pretty(&identity).map_err(json_io)?;
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(StorageFailure::Io)?;
            f.write_all(text.as_bytes()).map_err(StorageFailure::Io)?;
            f.flush().map_err(StorageFailure::Io)?;
            f.sync_data().map_err(StorageFailure::Io)?;
        }
        // The file's bytes are durable; its *directory entry* is not until the directory is flushed.
        // Same defect as a non-durable rename, same fix (`crate::durable`).
        fsync_dir(dir).map_err(StorageFailure::Io)?;
        Ok(identity)
    }

    /// Record this process as the writing holder, durably replacing the identity file.
    ///
    /// # Errors
    ///
    /// [`StorageFailure::Io`]. This runs at `open_for_write` and nowhere else, so it costs one
    /// temp-file write and one `durable_rename` per open — the one place this backend uses the helper
    /// outside reclamation, which is deliberate: it means the rename path is exercised by every
    /// writable open rather than only by the rarer destructive step.
    pub(crate) fn claim(mut self, dir: &Path) -> Result<Self, StorageFailure> {
        self.holder_pid = Some(std::process::id());
        let text = serde_json::to_string_pretty(&self).map_err(json_io)?;
        let tmp = dir.join("store.json.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(StorageFailure::Io)?;
        durable_rename(&tmp, &path_in(dir)).map_err(StorageFailure::Io)?;
        Ok(self)
    }
}

fn path_in(dir: &Path) -> PathBuf {
    dir.join(IDENTITY_FILE)
}

/// A `serde_json` failure writing a struct this module owns is an I/O-class failure, not a domain one:
/// the value is always encodable, so the only reachable cause is the writer.
fn json_io(e: serde_json::Error) -> StorageFailure {
    StorageFailure::Io(io::Error::other(e))
}

/// Mint a store identity that no other store shares.
///
/// There is no `rand` in this crate's dependency list and ADR-0030 F6 §D asks for one small dependency
/// only, so the value is composed rather than drawn: the wall clock in nanoseconds, the process id, and
/// a per-process counter. Each one alone collides — two stores created in the same nanosecond, two
/// processes with one clock, a pid reused after a reboot — and the three together do not, because the
/// pid distinguishes same-nanosecond creations across processes and the counter distinguishes them
/// within one. The value is **not** a secret and nothing authenticates with it: its whole job is to make
/// `cursor.store() == self.store_id()` false when a cursor came from somewhere else (ADR-0030 F6 §B).
fn fresh_store_id() -> StoreId {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = u128::from(std::process::id());
    let seq = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let raw = (nanos << 32) ^ (pid << 96) ^ seq;
    // `NonZeroU128::MIN` only if the composition landed exactly on zero, which is as good an identity
    // as any other number and is written rather than asserted so no panic sits on this path.
    StoreId::new(NonZeroU128::new(raw).unwrap_or(NonZeroU128::MIN))
}
