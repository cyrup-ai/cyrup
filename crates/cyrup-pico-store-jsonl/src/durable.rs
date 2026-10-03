//! `durable_rename` and `fsync_dir` — the fsync sequences that make a directory entry survive power
//! loss (ADR-0030 F6 §D's second half).
//!
//! # Why this is here and not imported
//!
//! PICO5-PLAN S11 landed exactly this helper in `cyrup-session` (`crates/cyrup-session/src/durable.rs`)
//! and it is `pub(crate)` there. ADR-0030 §8 forbids this crate an edge to `cyrup-session`
//! (see `Cargo.toml`), so the sequence is written again rather than imported. The duplication is
//! deliberate and bounded — three syscalls and one `Path::parent` fallback — and the natural end state
//! is a leaf crate holding it once; that is a refactor across two crate boundaries and does not belong
//! inside the slice that first needs the second copy. S11's module documentation carries the full
//! statement of the defect and of ADR-0030 §14 open question 1 (the Windows arm); it is not repeated
//! here, only the part this backend's own correctness rests on.
//!
//! # What this backend rests on
//!
//! `spec.md:4443-4446` makes reclamation write a temporary replacement and rename it, and
//! `spec.md:4439-4442` makes a flushed `main.jsonl` the authorisation for that destructive step. The
//! two together are only sound if the rename is durable: if the authorising marker reaches the device
//! and the directory entry naming the replacement does not, power loss restores the pre-reclamation
//! sidecar under a surviving authorisation — which is the *marker without its data* corruption
//! `spec.md:4425` exists to prevent, arriving by the back door.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// Atomically replace `dst` with `tmp`, and make the replacement durable.
///
/// Three steps, in this order: `fdatasync` the payload through a **write** handle (a read-only handle
/// fails `FlushFileBuffers` on Windows with `ERROR_ACCESS_DENIED`, and `fsync` is per inode rather
/// than per descriptor, so a second handle flushes the writer's dirty pages); rename; `fsync` the
/// parent directory.
///
/// # Errors
///
/// [`io::Error`] from any step. Steps 1 and 2 leave `dst` naming exactly what it named before; step 3
/// leaves the rename visible to every process and only its survival across power loss unknown. This
/// backend treats the whole call as one unit and defers reclamation on any failure
/// (`spec.md:4441-4442`), so it does not need to distinguish them — `cyrup-session`'s copy documents
/// why the distinction nonetheless exists.
pub(crate) fn durable_rename(tmp: &Path, dst: &Path) -> io::Result<()> {
    {
        let f = OpenOptions::new().write(true).open(tmp)?;
        f.sync_data()?;
    }
    std::fs::rename(tmp, dst)?;
    fsync_dir(parent_dir_of(dst))
}

/// The directory whose entry a create-or-replace of `dst` rewrites.
///
/// `Path::parent` answers `Some("")` for a bare relative name and `None` for a root, and neither is
/// openable; both mean the process's current directory for the purposes of the rename that just
/// happened, so both map to `.`.
#[cfg(any(unix, test))]
fn parent_dir_of(dst: &Path) -> &Path {
    match dst.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Non-unix: [`fsync_dir`] ignores its argument, so computing one would be dead code.
#[cfg(not(any(unix, test)))]
fn parent_dir_of(dst: &Path) -> &Path {
    dst
}

/// Flush a directory's entries.
///
/// # Unix
///
/// `fsync(2)` on a descriptor for the directory is the only portable way to ask for a directory entry
/// back. This is the line whose absence ADR-0030 F6 §D records as *"already latent in the tree"*.
///
/// # Every other target, Windows included
///
/// A no-op, and an **interim envelope** rather than a decision: ADR-0030 §14 open question 1 is open,
/// and `crates/cyrup-session/src/durable.rs` states the two Win32 facts and the three candidate
/// answers in full. Nothing here is weaker than it would be without this function.
///
/// # Errors
///
/// [`io::Error`] if the directory cannot be opened or flushed.
#[cfg(unix)]
pub(crate) fn fsync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(test)]
    observe::record_dir_fsync(dir);
    Ok(())
}

/// Non-unix arm: a deliberate no-op. See the unix arm's documentation.
///
/// # Errors
///
/// Never.
#[cfg(not(unix))]
pub(crate) fn fsync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// The observation seam for the directory `fsync`.
///
/// Power loss cannot be simulated in-process, so the only assertable form of *"the directory entry was
/// made durable"* is *"the `fsync` on the directory happened"*. This records that it did, **after** the
/// real `sync_all` returned `Ok`, so it is instrumentation on the production call rather than a stub
/// standing in for it: a test passing here cannot pass with the syscall removed.
#[cfg(all(test, unix))]
mod observe {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    static SYNCED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

    fn slot() -> &'static Mutex<Vec<PathBuf>> {
        SYNCED.get_or_init(|| Mutex::new(Vec::new()))
    }

    pub(super) fn record_dir_fsync(dir: &Path) {
        let mut g = slot().lock().unwrap_or_else(|p| p.into_inner());
        g.push(dir.to_path_buf());
    }

    /// True if some call in this process has `fsync`ed a directory that resolves to `want`.
    /// Canonicalised on both sides so a symlinked temp root (`/var` -> `/private/var`) does not make
    /// the comparison lie.
    pub(crate) fn dir_was_fsynced(want: &Path) -> bool {
        let Ok(want) = want.canonicalize() else {
            return false;
        };
        let g = slot().lock().unwrap_or_else(|p| p.into_inner());
        g.iter()
            .any(|p| p.canonicalize().ok() == Some(want.clone()))
    }
}

#[cfg(all(test, unix))]
pub(crate) use observe::dir_was_fsynced;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The one part of the sequence that is a pure function, and the two answers `Path::parent` gives
    /// that are not openable.
    #[test]
    fn parent_dir_of_maps_unopenable_answers_to_cwd() {
        assert_eq!(parent_dir_of(Path::new("main.jsonl")), Path::new("."));
        assert_eq!(parent_dir_of(Path::new("/")), Path::new("."));
        assert_eq!(
            parent_dir_of(Path::new("/a/b/doc-1-g0.jsonl")),
            Path::new("/a/b")
        );
    }

    /// The whole sequence, on a real directory: the destination is replaced and the temp file is gone.
    #[test]
    fn replaces_an_existing_destination_and_consumes_the_temp() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("doc-1-g1.jsonl");
        let tmp = dir.path().join("doc-1-g1.jsonl.tmp");
        std::fs::write(&dst, b"old\n").unwrap();
        std::fs::write(&tmp, b"new\n").unwrap();

        durable_rename(&tmp, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"new\n");
        assert!(!tmp.exists(), "the temp file must not survive the rename");
    }

    /// A rejected rename leaves the destination exactly as it was.
    #[test]
    fn a_missing_temp_is_rejected_without_touching_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("doc-1-g1.jsonl");
        std::fs::write(&dst, b"old\n").unwrap();

        let err = durable_rename(&dir.path().join("absent.tmp"), &dst).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::read(&dst).unwrap(), b"old\n");
    }
}
