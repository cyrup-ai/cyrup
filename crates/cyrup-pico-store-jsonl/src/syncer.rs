//! The deferred-`fdatasync` worker: ADR-0030 §9's mechanism for [`Durability::PowerLoss`].
//!
//! # Provenance, stated because this is a lift and not an invention
//!
//! ADR-0030 §9 names `crates/cyrup-session/src/store.rs:65-182` as the mechanism for the strong tier
//! and says three things *"transfer directly"*: this worker, the fd-invalidation discipline
//! ([`crate::appender`]) and the single-`write(2)` append. This module is that worker, lifted with its
//! reasoning. `Cargo.toml` states why it is lifted rather than imported — the static is private to that
//! crate and ADR-0030 §8 gives this one no edge to it.
//!
//! # What it buys, and what it does not
//!
//! The `write(2)` stays on the caller's thread, always. `write` hands the bytes to the kernel, so once
//! it returns they are visible to every reader and safe against a `SIGKILL`, an `abort` or a
//! `std::process::exit` — the kernel is what writes them back. **Moving an append onto this worker
//! would leave it in a userspace queue that dies with the process**, which is weaker than
//! [`Durability::ProcessCrash`] claims rather than stronger. Only the device flush is deferred, and the
//! worker coalesces a burst into one `fdatasync` per file.
//!
//! The one place this backend *waits* is [`Syncer::barrier`], and §11.3 forces it: *"With `fsync:
//! true`, the backend appends all affected sidecar records, flushes each affected sidecar, and only
//! then appends the main marker"* (`spec.md:4421-4424`). The flush has to have **happened** before the
//! marker is written, so the strong tier pays one coalesced flush round for a commit that writes
//! document content. It pays nothing for a main-only commit, which `spec.md:4426` singles out: *"A
//! main-only commit has no sidecars to flush."*
//!
//! [`Durability::PowerLoss`]: crate::Durability::PowerLoss
//! [`Durability::ProcessCrash`]: crate::Durability::ProcessCrash

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, LazyLock, Mutex};

/// A sticky, take-once slot for a failure a deferred flush discovered.
///
/// Typed rather than stringified, so an `ErrorKind` — `ENOSPC` above all — survives the deferral and
/// the caller that eventually sees it can still classify it.
pub(crate) type SyncErr = Arc<Mutex<Option<io::Error>>>;

/// A background flush request, or a barrier answered once everything queued ahead of it has flushed.
enum SyncReq {
    Sync { file: Arc<File>, err: SyncErr },
    Barrier(Sender<()>),
}

/// The one flush worker for the whole process.
///
/// Global rather than per-store for the reason `cyrup-session`'s copy gives: several stores can be open
/// at once (one writable plus any number of read-only), a per-store worker would neither bound the
/// thread count nor order those flushes against each other, and the dedup below is by fd identity, so
/// one worker is strictly better informed than several.
static SYNCER: LazyLock<Syncer> = LazyLock::new(Syncer::start);

/// Request a deferred `fdatasync` of `file`, reporting a failure through `err`.
pub(crate) fn request(file: &Arc<File>, err: &SyncErr) {
    SYNCER.request(file, err);
}

/// Block until every flush requested before this call has completed.
///
/// One flush round, ~200 µs, and a no-op when nothing is pending.
pub(crate) fn barrier() {
    SYNCER.barrier();
}

struct Syncer {
    /// `None` when the worker thread could not be spawned (`EAGAIN`, a thread rlimit). [`request`]
    /// then flushes inline — the old cost and the same guarantee, rather than a silently dropped
    /// flush.
    tx: Option<Sender<SyncReq>>,
}

impl Syncer {
    fn start() -> Self {
        let (tx, rx) = channel::<SyncReq>();
        match std::thread::Builder::new()
            .name("cyrup-pico-jsonl-fsync".into())
            .spawn(move || run(rx))
        {
            Ok(_handle) => Self { tx: Some(tx) },
            Err(_) => Self { tx: None },
        }
    }

    fn request(&self, file: &Arc<File>, err: &SyncErr) {
        let Some(tx) = &self.tx else {
            return Self::sync_inline(file, err);
        };
        let req = SyncReq::Sync {
            file: Arc::clone(file),
            err: Arc::clone(err),
        };
        if tx.send(req).is_err() {
            // The worker can only end if the static's sender is dropped, which never happens for a
            // `LazyLock` static — but if it somehow does, do not lose the flush.
            Self::sync_inline(file, err);
        }
    }

    fn sync_inline(file: &Arc<File>, err: &SyncErr) {
        if let Err(e) = file.sync_data() {
            let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(e);
        }
    }

    fn barrier(&self) {
        // In inline mode every flush already completed synchronously; nothing can be pending.
        let Some(tx) = &self.tx else { return };
        let (ack_tx, ack_rx) = channel::<()>();
        if tx.send(SyncReq::Barrier(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}

/// One queued flush. Named because `clippy::type_complexity` fires on the bare tuple.
type PendingSync = (Arc<File>, SyncErr);

fn run(rx: Receiver<SyncReq>) {
    // Reused across rounds to keep the loop allocation-free in steady state.
    let mut round: Vec<PendingSync> = Vec::new();
    let mut acks: Vec<Sender<()>> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();

    while let Ok(first) = rx.recv() {
        round.clear();
        acks.clear();
        seen.clear();

        // Drain everything already queued behind the message that woke us: a whole burst collapses
        // into one flush per file. This is the debounce — no timer needed.
        for req in std::iter::once(first).chain(rx.try_iter()) {
            match req {
                SyncReq::Sync { file, err } => round.push((file, err)),
                SyncReq::Barrier(ack) => acks.push(ack),
            }
        }

        // Dedup by fd identity, not by path: one path can name two inodes across a reclamation's
        // rename. Every `Arc` stays alive in `round` for the whole loop, so an address cannot be freed
        // and reused mid-dedup (which would silently skip a real flush) — which is why the requests
        // are collected first rather than flushed as they drain.
        for (file, err) in &round {
            if !seen.insert(Arc::as_ptr(file) as usize) {
                continue;
            }
            if let Err(e) = file.sync_data() {
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
