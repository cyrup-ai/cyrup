//! The `gix` layer: changed-tracked-files listing + per-path fingerprinting.
//!
//! Upstream shells out to `git diff --name-only -z HEAD --` and
//! `git diff --no-ext-diff --binary HEAD -- <path>` (`mutation-evidence.ts:48-60`), with
//! `-c core.fsmonitor=false` on every invocation (`:12-15`) so a stale fsmonitor daemon cannot
//! block child startup. cyrup uses `gix` in-process instead: the evidence path runs at a deadline
//! kill, where a subprocess is the one thing that must not be spawned, and shelling out imports
//! the user's `diff.external`/`core.pager`/fsmonitor configuration into a diagnostic — the exact
//! class of interference upstream's `gitArguments` exists to fend off, structurally absent here.
//!
//! Every function returns `Result<_, String>`: the caller ([`super::snapshot`]) degrades any error
//! to an `unavailable` record, never a failed run.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::Path;

use gix::bstr::ByteSlice as _;
use sha2::{Digest as _, Sha256};

use super::types::{FingerprintKind, MAX_HASH_BYTES, MAX_TRACKED_PATHS, TrackedMutationFingerprint};

/// Discover the repository `cwd` sits in. A non-repository `cwd` is the ordinary case, not an
/// error — the caller records the message as `unavailable`.
pub(super) fn open_repository(cwd: &Path) -> Result<gix::Repository, String> {
    gix::discover(cwd).map_err(|err| format!("git repository discovery failed: {err}"))
}

/// The repository's worktree root — also the [CYRUP-DELTA] `git_root` value the snapshot records.
/// A bare repository has no worktree and therefore nothing this module can fingerprint.
pub(super) fn required_workdir(repo: &gix::Repository) -> Result<&Path, String> {
    repo.workdir()
        .ok_or_else(|| "repository has no worktree (bare)".to_string())
}

/// The `HEAD` tree every fingerprint is measured against. Resolved ONCE per snapshot/collect pass
/// — and resolved at all, rather than falling back to an empty tree, because upstream's
/// `git diff HEAD` fails outright on an unborn `HEAD` and the whole snapshot degrades to
/// `unavailable`; silently measuring against an empty tree would instead report every tracked
/// file as changed.
pub(super) fn head_tree(repo: &gix::Repository) -> Result<gix::Tree<'_>, String> {
    repo.head_tree()
        .map_err(|err| format!("failed to resolve HEAD tree: {err}"))
}

/// The repo-relative tracked paths that differ from `HEAD`, capped at [`MAX_TRACKED_PATHS`].
pub(super) struct ChangedTrackedFiles {
    /// Sorted (collected through a `BTreeSet`) and capped.
    pub paths: Vec<String>,
    /// The listing produced more than [`MAX_TRACKED_PATHS`] paths and was cut (pi `:50`).
    pub truncated: bool,
}

/// pi `listChangedTrackedFiles` (`:48-51`): the union of *(HEAD-tree vs index)* and *(index vs
/// worktree)* — exactly what `git diff --name-only HEAD --` reports.
/// `gix::Repository::status(…)` → `Platform` → `into_iter` computes exactly that pair, one
/// [`gix::status::Item`] variant per half.
///
/// **The dirwalk is disabled**, which is `trackedOnly: true` enforced structurally rather than by
/// filtering afterwards: `dirwalk_options = None` is documented as "only modification checks are
/// performed" (`gix-0.85.0/src/status/index_worktree.rs:48-53`), so an untracked file cannot
/// appear in the output at all — and the walk itself is skipped. Submodules are ignored because
/// upstream's `git diff --name-only` against a plain path list reports a submodule as one path and
/// never recurses.
///
/// Paths are collected into a `BTreeSet` first: both halves can report the same path (a file that
/// is both staged and further modified), and iteration order is explicitly undefined when `gix` is
/// compiled with `parallel` (`src/status/iter/mod.rs:26-37`) — which is also why upstream's `:94`
/// sort downstream is not optional here.
pub(super) fn list_changed_tracked_files(
    repo: &gix::Repository,
) -> Result<ChangedTrackedFiles, String> {
    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|err| format!("failed to prepare repository status: {err}"))?
        .index_worktree_options_mut(|opts| {
            opts.dirwalk_options = None;
        })
        .index_worktree_submodules(gix::status::Submodule::Given {
            ignore: gix::submodule::config::Ignore::All,
            check_dirty: false,
        });
    let iter = platform
        .into_iter(Vec::new())
        .map_err(|err| format!("failed to start repository status: {err}"))?;

    let mut paths = BTreeSet::new();
    for item in iter {
        let item = item.map_err(|err| format!("repository status failed: {err}"))?;
        paths.insert(item.location().to_str_lossy().into_owned());
    }

    let mut paths: Vec<String> = paths.into_iter().collect();
    let truncated = paths.len() > MAX_TRACKED_PATHS;
    paths.truncate(MAX_TRACKED_PATHS); // pi `:50`
    Ok(ChangedTrackedFiles { paths, truncated })
}

/// A per-file fingerprint of "has this file's content diverged from HEAD, and how".
///
/// [CYRUP-DELTA, strictly stronger] Upstream fingerprints by running
/// `git diff --no-ext-diff --binary HEAD -- <path>` and SHA-256'ing the patch TEXT (`:53-59`),
/// with a spill-to-tempfile path for diffs over 1 MiB (`hashLargeDiff`, `:25-46`). cyrup
/// fingerprints the tuple *(HEAD blob oid, worktree presence, SHA-256 of the worktree bytes capped
/// at [`MAX_HASH_BYTES`], untruncated length)* through `gix`.
///
/// The two answer the same question — "did this file change relative to HEAD since the snapshot" —
/// and the tuple form is better on every axis that matters here:
///
/// * it never shells out, so it cannot be perturbed by the user's `diff.external`, `core.pager`,
///   `diff.algorithm`, or a stale fsmonitor daemon — upstream needs an explicit
///   `-c core.fsmonitor=false` (`:12-15`) precisely because of the last one;
/// * it has no `maxBuffer` cliff and needs no temp file, so `hashLargeDiff`'s `mkdtemp`/`rmSync`
///   pair does not exist here — which also removes one more producer of the leaked-temp-directory
///   class of bug this workspace has already fixed twice;
/// * a rename is a change under both, and a deletion is a change under both (the worktree-presence
///   marker keeps a deleted file distinguishable from an emptied one).
///
/// Including the untruncated length keeps two files that share a first megabyte distinguishable.
///
/// The digest is NOT comparable across the two implementations, and does not need to be: it is
/// only ever compared against another fingerprint taken by this same build within one run's
/// lifetime ([`same_fingerprint`]). The `kind: "diff"` discriminant is preserved for wire
/// compatibility (pi `shared/types.ts:595`).
pub(super) fn fingerprint_path(
    workdir: &Path,
    head_tree: &gix::Tree<'_>,
    rel_path: &str,
) -> Result<TrackedMutationFingerprint, String> {
    let head_state = match head_tree.lookup_entry_by_path(rel_path) {
        Ok(Some(entry)) => entry.oid().to_string(),
        // Not in HEAD: a newly added tracked file (upstream's diff-against-HEAD covers it too).
        Ok(None) => "absent".to_string(),
        Err(err) => return Err(format!("failed to look up {rel_path} in HEAD: {err}")),
    };

    let mut hasher = Sha256::new();
    hasher.update(head_state.as_bytes());
    hasher.update(b"\0");
    match read_worktree_capped(&workdir.join(rel_path)) {
        Ok(Some((bytes, total_len))) => {
            hasher.update(b"present\0");
            hasher.update(&bytes);
            hasher.update(b"\0");
            hasher.update(total_len.to_string().as_bytes());
        }
        // Deleted from the worktree — a change state of its own, distinct from an emptied file.
        Ok(None) => hasher.update(b"absent\0"),
        Err(err) => return Err(format!("failed to read {rel_path}: {err}")),
    }

    Ok(TrackedMutationFingerprint {
        kind: FingerprintKind::Diff,
        digest: hex_digest(hasher),
    })
}

/// Read at most [`MAX_HASH_BYTES`] of `path` plus its true total length; `None` when the path does
/// not exist in the worktree (a legitimate state, not an error).
fn read_worktree_capped(path: &Path) -> std::io::Result<Option<(Vec<u8>, u64)>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let total_len = file.metadata()?.len();
    let mut bytes = Vec::with_capacity(usize::min(
        MAX_HASH_BYTES,
        usize::try_from(total_len).unwrap_or(MAX_HASH_BYTES),
    ));
    file.take(MAX_HASH_BYTES as u64).read_to_end(&mut bytes)?;
    Ok(Some((bytes, total_len)))
}

/// pi `sameFingerprint` (`:62-65`). `None` on the left ⇒ **`false`** — a file that was dirty at
/// snapshot time but has no recorded fingerprint counts as changed. The asymmetry is upstream's,
/// ported literally.
pub(super) fn same_fingerprint(
    left: Option<&TrackedMutationFingerprint>,
    right: &TrackedMutationFingerprint,
) -> bool {
    match left {
        None => false,
        Some(left) => left.kind == right.kind && left.digest == right.digest,
    }
}

/// The crate's `sha2` 0.11 hex idiom (`watchdog/change_signature.rs:167-177`): the 0.11 digest
/// output does not implement `LowerHex`, so the hex encoding is an explicit fold rather than a
/// `{:x}` format.
fn hex_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn same_fingerprint_rejects_a_missing_left_hand_side() {
        let right = TrackedMutationFingerprint {
            kind: FingerprintKind::Diff,
            digest: "d".to_string(),
        };
        // pi `:62-65` — a dirty file with no recorded fingerprint counts as changed.
        assert!(!same_fingerprint(None, &right));
        assert!(same_fingerprint(Some(&right.clone()), &right));
        let other = TrackedMutationFingerprint {
            kind: FingerprintKind::Diff,
            digest: "e".to_string(),
        };
        assert!(!same_fingerprint(Some(&other), &right));
    }

    #[test]
    fn read_worktree_capped_reports_absent_as_none_and_caps_bytes() {
        let dir = tempfile::tempdir().expect("real tempdir");
        assert_eq!(
            read_worktree_capped(&dir.path().join("missing.txt")).unwrap(),
            None
        );
        let file = dir.path().join("present.txt");
        std::fs::write(&file, b"hello").unwrap();
        let (bytes, len) = read_worktree_capped(&file).unwrap().unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(len, 5);
    }
}
