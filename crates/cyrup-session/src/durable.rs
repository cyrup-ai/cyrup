//! `durable_rename` — the fsync sequence that makes replace-by-rename survive power loss.
//!
//! # The defect this exists to close
//!
//! A replace-by-rename has **three** things to make durable, not one: the temp file's bytes, the
//! rename itself, and — on unix — the *directory entry* the rename rewrote. Before this module,
//! `DiskStore::rewrite` (`store.rs`) did only the first:
//!
//! ```text
//! f.sync_data()?;                              // the bytes reach the device
//! std::fs::rename(&tmp, &self.path)?;          // the swap reaches the PAGE CACHE
//! ```
//!
//! `rename(2)` is atomic with respect to other processes, which is what makes the pattern safe
//! against a *process* crash. It is not atomic with respect to the *device*: the modified directory
//! block is dirty page cache like any other write, and nothing above asks for it back. On power
//! loss after that `rename` returns `Ok(())`, a unix filesystem may legally come back with the old
//! directory entry — so the caller was told the rewrite succeeded, the new inode is fully durable,
//! and the path still names the old one. The temp file is also still linked, under its own name, so
//! the loss is silent rather than detectable.
//!
//! `cyrup-session` reaches `rewrite` only on a format migration or an eager clone seed
//! (`manager/lifecycle.rs:102,123`, `manager/branched_session.rs:174`) — i.e. at exactly the two
//! moments the only copy of the user's history is being rebuilt from memory, which is why a latent
//! defect on a rare path still matters.
//!
//! ADR-0030 §3 (F6 §D) records this as the second latent failure mode in the tree and names the fix
//! `durable_rename`; `docs/PICO5-PLAN.md` S11 scopes it. ADR-0030 §11.3's reclamation path renames a
//! sidecar replacement over a live sidecar and then trusts an authorising marker, so it inherits this
//! helper's guarantee directly — the marker-without-its-data corruption `spec.md:4425` exists to
//! prevent is precisely what a non-durable rename reintroduces.
//!
//! # Why there is no unit test for the power-loss property itself
//!
//! There is no in-process way to drop the page cache on the floor, so the *outcome* (old entry comes
//! back) cannot be asserted from a test; that is exactly why the defect survived in-tree. What the
//! tests below assert instead is the **mechanism**: that the parent directory of `dst` is opened and
//! `fsync`ed, observed at the real syscall through the `observe` module below, which documents why it
//! is instrumentation on the production call rather than a stub standing in for it.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// Atomically replace `dst` with `tmp`, and make the replacement durable.
///
/// The sequence is fixed and the order is the whole point:
///
/// 1. **`fsync` the payload.** `fdatasync` rather than `fsync`: POSIX requires it to flush enough
///    metadata to *retrieve the data*, which covers the one metadata field a fresh temp file needs
///    (its size). Mode and timestamps are not durability-relevant here, so the extra inode write a
///    full `fsync` costs buys nothing.
///    `tmp` is opened for **write** — not read — because that is what `FlushFileBuffers` requires on
///    Windows, where a read-only handle fails with `ERROR_ACCESS_DENIED`. `fsync` is per *inode*,
///    not per descriptor, so flushing through a second handle flushes the writer's dirty pages.
/// 2. **Rename.** Atomic against other processes; `dst` is either wholly old or wholly new.
/// 3. **`fsync` the parent directory** — see `fsync_parent_dir`. Without this, step 2 is durable
///    only as far as the page cache.
///
/// # Errors
///
/// Steps 1 and 2 fail in the *rejected* class: nothing durable has happened and `dst` still names
/// what it named before, so the caller may retry or report cleanly. Step 3 fails in the
/// **uncertain** class — the rename has taken effect for every reader in this and every other
/// process, and only its survival across power loss is unknown. ADR-0030 §9's obligation (6) asks
/// those two classes to be distinguishable; `SessionError` has one `Io` variant and cannot express
/// the distinction, which is noted here rather than silently dropped. The error is still propagated:
/// swallowing it is the defect this module closes.
///
/// `tmp` is **not** removed if step 2 fails; the caller owns the temp file's lifetime.
pub(crate) fn durable_rename(tmp: &Path, dst: &Path) -> io::Result<()> {
    {
        let f = OpenOptions::new().write(true).open(tmp)?;
        f.sync_data()?;
    }
    std::fs::rename(tmp, dst)?;
    fsync_parent_dir(dst)
}

/// The directory whose entry a create-or-replace of `dst` rewrites.
///
/// `Path::parent` answers `Some("")` for a bare relative name like `session.jsonl` and `None` for a
/// root, and neither is openable. Both mean "the process's current directory" for the purposes of
/// the rename that just happened, so both map to `.`. Split out from `durable_rename` because it
/// is the one part of the sequence that is a pure function and can be tested without a filesystem.
///
/// Only `fsync_parent_dir`'s unix arm needs it, so it is `cfg`-gated to the configurations that have
/// a caller — otherwise a Windows build of this crate carries a `dead_code` warning, and
/// `ADR-0007-windows-scope.md` records `cyrup-session` as one of the crates that cross-compile clean.
#[cfg(any(unix, test))]
fn parent_dir_of(dst: &Path) -> &Path {
    match dst.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Flush the directory entry written by the preceding `rename`.
///
/// # Unix
///
/// `fsync(2)` on a descriptor for the directory is the only portable way to ask for a directory
/// entry back, and POSIX permits the read-only descriptor `File::open` gives us for it. This is the
/// line whose absence was the defect.
///
/// # Every other target, Windows included
///
/// This is a **no-op**, and that is an *interim* envelope rather than a decision.
/// **ADR-0030 §14 open question 1 is open**, and S11 is not the place to close it, for two reasons
/// that are facts rather than preferences:
///
/// - `std::fs::rename` lowers to `MoveFileExW(.., MOVEFILE_REPLACE_EXISTING)` and deliberately does
///   **not** pass `MOVEFILE_WRITE_THROUGH`, which is the flag whose documented effect ("does not
///   return until the file has actually been moved on the disk") is what step 3 wants. Reaching it
///   means calling `MoveFileExW` directly, i.e. a **new `windows-sys` dependency** in this crate.
/// - There is no directory-fsync equivalent to substitute for it. `File::open` cannot open a
///   directory on Windows at all (it needs `FILE_FLAG_BACKUP_SEMANTICS` via `CreateFileW`), and
///   `FlushFileBuffers` on a directory handle is not a documented metadata flush — on a *volume*
///   handle it flushes the volume, which is a different and far heavier operation.
///
/// So the three candidate answers ADR-0030 §14 lists — take the dependency, accept NTFS metadata
/// journalling as sufficient, or state the Windows envelope as weaker — differ by a dependency and a
/// durability claim, not by an implementation detail. `docs/adr/ADR-0007-windows-scope.md` puts
/// Windows in scope and says nothing about either, so the choice belongs to whoever owns ADR-0030.
///
/// What this arm does guarantee in the meantime: step 1 and step 2 run identically on every target,
/// so Windows is **no weaker than it was** before this module existed, and the whole of the
/// unanswered question is localised to this one function body.
#[cfg(unix)]
fn fsync_parent_dir(dst: &Path) -> io::Result<()> {
    let dir = parent_dir_of(dst);
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(test)]
    observe::record_dir_fsync(dir);
    Ok(())
}

/// Non-unix arm: a deliberate no-op. The reasoning, the two Win32 facts behind it and the three
/// candidate answers belong to the unix arm's documentation above — read it there, because this body
/// is the single place ADR-0030 §14 open question 1 has to change when it is answered.
#[cfg(not(unix))]
fn fsync_parent_dir(_dst: &Path) -> io::Result<()> {
    Ok(())
}

/// The observation seam for the directory `fsync`.
///
/// Power loss cannot be simulated in-process, so the only assertable form of "the directory entry
/// was made durable" is "the `fsync` on the directory happened". This records that it did, **after**
/// the real `sync_all` returned `Ok` — it is instrumentation on the production call, not a stub
/// standing in for it, so a test passing here cannot pass with the syscall removed.
///
/// A `Vec` of every directory synced, rather than a counter, so the assertion is `contains(my
/// tempdir)` and stays correct whether the suite runs process-per-test (`cargo nextest`) or
/// thread-per-test (`cargo test`).
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

    /// True if some `durable_rename` in this process has `fsync`ed a directory that resolves to
    /// `want`. Canonicalised on both sides so a symlinked temp root (`/var` -> `/private/var`)
    /// does not make the comparison lie.
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    #[test]
    fn parent_dir_of_maps_unopenable_answers_to_cwd() {
        // A bare relative name: `Path::parent` says `Some("")`, which `File::open` rejects.
        assert_eq!(parent_dir_of(Path::new("session.jsonl")), Path::new("."));
        // A root: `Path::parent` says `None`.
        assert_eq!(parent_dir_of(Path::new("/")), Path::new("."));
        // The ordinary cases are untouched.
        assert_eq!(parent_dir_of(Path::new("/a/b/c.jsonl")), Path::new("/a/b"));
        assert_eq!(parent_dir_of(Path::new("a/b.jsonl")), Path::new("a"));
    }

    #[test]
    fn replaces_an_existing_destination_and_consumes_the_temp() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("s.jsonl");
        let tmp = dir.path().join("s.jsonl.tmp");
        std::fs::write(&dst, b"old\n").unwrap();
        std::fs::write(&tmp, b"new\n").unwrap();

        durable_rename(&tmp, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"new\n");
        assert!(!tmp.exists(), "the temp file must not survive the rename");
    }

    #[test]
    fn creates_a_destination_that_did_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("s.jsonl");
        let tmp = dir.path().join("s.jsonl.tmp");
        std::fs::write(&tmp, b"fresh\n").unwrap();

        durable_rename(&tmp, &dst).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"fresh\n");
    }

    #[test]
    fn a_missing_temp_is_rejected_without_touching_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("s.jsonl");
        std::fs::write(&dst, b"old\n").unwrap();

        let err = durable_rename(&dir.path().join("absent.tmp"), &dst).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            b"old\n",
            "a rejected durable_rename must leave the destination exactly as it was"
        );
    }

    /// The mechanism assertion: step 3 of the sequence actually runs.
    #[cfg(unix)]
    #[test]
    fn fsyncs_the_parent_directory_of_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("s.jsonl");
        let tmp = dir.path().join("s.jsonl.tmp");
        std::fs::write(&tmp, b"x\n").unwrap();

        assert!(
            !dir_was_fsynced(dir.path()),
            "fresh tempdir cannot have been synced yet"
        );
        durable_rename(&tmp, &dst).unwrap();
        assert!(
            dir_was_fsynced(dir.path()),
            "durable_rename must fsync the directory whose entry it rewrote"
        );
    }
}
