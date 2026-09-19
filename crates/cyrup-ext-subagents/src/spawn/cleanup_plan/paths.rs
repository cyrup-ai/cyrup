//! Path identity and containment — pi `worktree-cleanup-plan.ts:158-232` and `:443-454` @`v0.68.0`.
//!
//! Everything here is a PREDICATE over paths that may or may not exist. Nothing in this file
//! touches a path for anything but `lstat`/`realpath`; the plan builder mutates nothing but the
//! plan file.

use std::path::{Path, PathBuf};

/// pi `comparablePath` (`:178-195`): canonicalize the longest EXISTING ancestor, re-join the
/// missing tail, then normalize lexically.
///
/// The missing-tail walk is the part that matters. A plain `canonicalize` fails outright on a path
/// whose leaf has been removed, and the caller would then be comparing a canonical path against a
/// non-canonical one — which is how a `/var` vs `/private/var` symlink silently defeats a
/// containment check.
///
/// The two halves are [`crate::spawn::worktree::normalize_comparable_cwd`] (absolutize +
/// canonicalize, with a fallback) and [`crate::spawn::worktree::lexical_normalize`] (collapse
/// `.`/`..` without touching the filesystem). They are REUSED rather than reimplemented: a second
/// path-escape predicate in this crate is a security bug waiting to happen.
#[must_use]
pub(crate) fn comparable_path(candidate: &Path) -> PathBuf {
    let absolute = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());

    let mut current = absolute.as_path();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(current) {
            let mut joined = canonical;
            for segment in missing.iter().rev() {
                joined.push(segment);
            }
            return crate::spawn::worktree::lexical_normalize(Path::new(""), &joined);
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        let Some(leaf) = current.file_name() else {
            break;
        };
        missing.push(leaf.to_os_string());
        current = parent;
    }
    crate::spawn::worktree::lexical_normalize(Path::new(""), &absolute)
}

/// pi `samePath` (`:197-212`).
///
/// [CYRUP-DELTA] pi's Windows `dev`/`ino` fallback (`:203-212`) is NOT ported. Premise verified
/// against this repository: the README's clippy/test gates name `wasm32-wasip2` and the host
/// triple only, and no `*-pc-windows-*` target is built anywhere, so a `cfg(windows)` branch
/// would be code nothing compiles — which is code that rots into a false sense of coverage. The
/// fallback also exists for a Windows-specific short-name/drive-alias problem that does not arise
/// on the targets this crate ships.
#[must_use]
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    comparable_path(left) == comparable_path(right)
}

/// pi `resolveExistingPath` (`:214-220`): realpath when possible, absolute path otherwise.
#[must_use]
pub(crate) fn resolve_existing_path(candidate: &Path) -> PathBuf {
    std::fs::canonicalize(candidate)
        .unwrap_or_else(|_| std::path::absolute(candidate).unwrap_or_else(|_| candidate.into()))
}

/// pi `pathInside` (`:222-232`).
///
/// `strict` forbids `root == candidate`, which is what makes "inside the base directory" mean
/// "a child of it" rather than "it, or a child of it".
#[must_use]
pub(crate) fn path_inside(root: &Path, candidate: &Path, strict: bool) -> bool {
    let root = comparable_path(root);
    let candidate = comparable_path(candidate);
    if root == candidate {
        return !strict;
    }
    candidate.starts_with(&root)
}

/// pi `PathInspection` (`:113-120`) — one `lstat`, decomposed.
#[derive(Clone, Debug)]
pub(crate) struct PathInspection {
    /// The absolutized candidate.
    pub(crate) resolved: PathBuf,
    /// The realpath, when the entry exists and is not a symlink.
    pub(crate) realpath: Option<PathBuf>,
    /// `ENOENT`.
    pub(crate) missing: bool,
    /// A symlink. **Rejected rather than followed** everywhere it is tested: a removal acting
    /// through an attacker-chosen link would act outside the sandbox.
    pub(crate) symlink: bool,
    /// A real directory.
    pub(crate) directory: bool,
    /// Any other `lstat` failure — a permission problem, a broken mount.
    pub(crate) error: Option<String>,
}

/// pi `inspectPath` (`:443-454`).
#[must_use]
pub(crate) fn inspect_path(candidate: &Path) -> PathInspection {
    let resolved = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    match std::fs::symlink_metadata(&resolved) {
        Ok(meta) if meta.file_type().is_symlink() => PathInspection {
            resolved,
            realpath: None,
            missing: false,
            symlink: true,
            directory: false,
            error: None,
        },
        Ok(meta) => {
            let realpath = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
            PathInspection {
                resolved,
                realpath: Some(realpath),
                missing: false,
                symlink: false,
                directory: meta.is_dir(),
                error: None,
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PathInspection {
            resolved,
            realpath: None,
            missing: true,
            symlink: false,
            directory: false,
            error: None,
        },
        Err(err) => PathInspection {
            resolved,
            realpath: None,
            missing: false,
            symlink: false,
            directory: false,
            error: Some(err.to_string()),
        },
    }
}

/// pi `metadataPath` (`:456-458`): a manifest-relative path is resolved against the manifest's own
/// directory, never against the process cwd.
#[must_use]
pub(crate) fn metadata_relative_path(manifest_path: &Path, candidate: &Path) -> PathBuf {
    if candidate.is_absolute() {
        std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf())
    } else {
        let dir = manifest_path.parent().unwrap_or(Path::new("."));
        crate::spawn::worktree::lexical_normalize(dir, candidate)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;

    #[test]
    fn path_inside_is_strict_about_the_root_itself() {
        let root = Path::new("/tmp/base");
        assert!(path_inside(root, root, false));
        assert!(!path_inside(root, root, true));
    }

    #[test]
    fn path_inside_rejects_a_sibling_with_a_shared_prefix() {
        // The bug a naive `starts_with` on STRINGS has: `/tmp/base-evil` does not start with
        // `/tmp/base/`. Component-wise `Path::starts_with` is what makes this correct.
        assert!(!path_inside(
            Path::new("/tmp/base"),
            Path::new("/tmp/base-evil"),
            true
        ));
    }

    #[test]
    fn path_inside_rejects_an_escape_through_parent_segments() {
        assert!(!path_inside(
            Path::new("/tmp/base"),
            Path::new("/tmp/base/../../etc"),
            true
        ));
    }

    #[test]
    fn comparable_path_collapses_a_missing_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist").join("..").join("leaf");
        let expected = comparable_path(dir.path()).join("leaf");
        assert_eq!(comparable_path(&missing), expected);
    }
}
