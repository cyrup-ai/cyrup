//! Error classification for index I/O.
//!
//! Ports pi's `isUnaddressableResultCandidate` (`result-files.ts:44-46`) and the errno sets its
//! readers branch on (`:268`, `:322-326`, `:388-391`).
//!
//! # Why this is one module
//!
//! pi spells the same errno set out inline at five call sites, and they are **not** all the same
//! set — `listIndexFiles` swallows `EPERM`/`EACCES` while `readResultIndexForSessionRun` rethrows
//! them. Writing each set once, named for what it means, is what keeps that distinction visible
//! instead of leaving it as a difference between two long `||` chains.

use std::io;

/// `ENAMETOOLONG` — a legacy alias can be valid metadata while still being unaddressable on the
/// current filesystem.
///
/// pi `isUnaddressableResultCandidate` (`result-files.ts:44-46`). This is a **supported outcome**,
/// not an error: the alias fan-out deliberately produces paths that may be too long for this
/// filesystem, and the correct response is to move on to the next candidate.
#[must_use]
pub(crate) fn is_unaddressable(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENAMETOOLONG)
}

/// The run is simply not there: `ENOENT` or `ENOTDIR`, plus [`is_unaddressable`].
///
/// A missing index directory is the overwhelmingly common case — most sessions have no results —
/// so this must be silent.
#[must_use]
pub(crate) fn is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    ) || error.raw_os_error() == Some(libc::ENOTDIR)
        || is_unaddressable(error)
}

/// `EPERM` / `EACCES`.
///
/// Kept separate from [`is_absent`] because the two upstream readers disagree about it on purpose:
/// enumeration treats a permission fault as "nothing here" and continues
/// (`listIndexFiles`, `:388-391`), while a targeted read **rethrows** it
/// (`readResultIndexForSessionRun`, `:322-326`). Quiet when scanning, loud when asked a direct
/// question — a permission fault on a specific lookup is a real fault the caller must see.
#[must_use]
pub(crate) fn is_access_denied(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::PermissionDenied)
        || matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES))
}

/// The set enumeration swallows: absent **or** access-denied.
///
/// pi `listIndexFiles` (`:388-391`) and `pendingResultFilesForSession` (`:430`) — both return
/// empty for `ENOENT`/`ENOTDIR`/`EPERM`/`EACCES`/`ENAMETOOLONG` and rethrow everything else.
#[must_use]
pub(crate) fn is_ignorable_listing_error(error: &io::Error) -> bool {
    is_absent(error) || is_access_denied(error)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn errno(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    #[test]
    fn enametoolong_is_unaddressable_and_absent_but_not_denied() {
        let e = errno(libc::ENAMETOOLONG);
        assert!(is_unaddressable(&e));
        assert!(
            is_absent(&e),
            "an unaddressable candidate is skipped like a missing one"
        );
        assert!(!is_access_denied(&e));
    }

    #[test]
    fn enoent_and_enotdir_are_absent() {
        assert!(is_absent(&errno(libc::ENOENT)));
        assert!(is_absent(&errno(libc::ENOTDIR)));
        assert!(is_absent(&io::Error::new(io::ErrorKind::NotFound, "x")));
    }

    #[test]
    fn eperm_and_eacces_are_access_denied_and_not_absent() {
        for code in [libc::EPERM, libc::EACCES] {
            let e = errno(code);
            assert!(is_access_denied(&e), "errno {code}");
            assert!(
                !is_absent(&e),
                "errno {code} must NOT be absent — a targeted read has to rethrow it"
            );
        }
    }

    #[test]
    fn enumeration_swallows_both_families_but_not_a_real_fault() {
        for code in [
            libc::ENOENT,
            libc::ENOTDIR,
            libc::EPERM,
            libc::EACCES,
            libc::ENAMETOOLONG,
        ] {
            assert!(is_ignorable_listing_error(&errno(code)), "errno {code}");
        }
        // EIO is a genuine fault and must propagate out of enumeration.
        assert!(!is_ignorable_listing_error(&errno(libc::EIO)));
    }

    #[test]
    fn the_two_reader_policies_actually_differ() {
        // The distinction this module exists to preserve: enumeration ignores a permission fault,
        // a targeted read does not.
        let denied = errno(libc::EACCES);
        assert!(is_ignorable_listing_error(&denied));
        assert!(!is_absent(&denied));
    }
}
