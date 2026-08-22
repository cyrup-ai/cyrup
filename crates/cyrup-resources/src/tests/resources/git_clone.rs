//! Install from a git URL through gix's real clone machinery over `file://` (hermetic), pinned-ref
//! checkout, pull-on-update, plus the `#[ignore]`d true-network https clone (G1).

use std::fs;

use super::fixtures::{git_in, make_local_git_repo, make_local_git_repo_two_commits};
use crate::{InstallScope, PackageManager, PackageSource, PackageStore, PinRef, UpdateTarget};
use cyrup_core::CancelToken;

// ===========================================================================
// G1 — install from a git URL via gix's real clone (file:// transport, hermetic)
// ===========================================================================

#[tokio::test]
async fn git_clone_from_file_url_uses_real_gix_clone() {
    // A `file://` URL is NOT the bare-directory copy fast path — it goes through gix's real clone
    // machinery (prepare_clone + fetch_then_checkout + main_worktree), the same code path remote
    // https/ssh/git URLs use. This exercises the transport end-to-end without a network.
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping file:// gix-clone test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install via real gix clone over file:// transport");
    assert!(
        rec.resolved_commit.is_some(),
        "HEAD commit resolved by gix clone"
    );

    // The working tree was actually checked out (the fixture's SKILL.md is present).
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir.join("skills/alpha/SKILL.md").exists(),
        "gix checked out the worktree at {}",
        store_dir.display()
    );
}

#[tokio::test]
async fn git_clone_url_with_ref_checks_out_pinned_tag() {
    // A `file://` URL pinned to a tag exercises gix's clone-with-ref path
    // (prepare_clone.with_ref_name + fetch_then_checkout), the SAME machinery a pinned remote
    // https/ssh install uses — distinct from the bare-directory copy path. The materialized
    // worktree must hold the *tagged* commit's content, not default HEAD (utils/git.ts:6-19,
    // R-09-018/020).
    let Some((_tmp, repo_dir)) = make_local_git_repo_two_commits() else {
        eprintln!("skipping file:// ref-checkout test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Tag("v1".into()),
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("gix clone of file:// pinned to tag v1");

    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    let marker = store_dir.join("marker.txt");
    assert!(
        marker.exists(),
        "worktree checked out at {}",
        store_dir.display()
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "v1",
        "gix checked out tag v1 (not HEAD's v2) over file:// transport — pin applied via with_ref_name"
    );
}

#[tokio::test]
async fn git_update_pulls_new_commits_from_file_url() {
    // Install from a `file://` remote, advance that remote by one commit (an upstream push), then
    // `update`. The recorded commit must advance to the new remote HEAD and the new file must be
    // present in the working tree — Pi `updateGit` fetch + reset-to-remote semantics
    // (package-manager.ts:1805-1818). Without a real network fetch this would be impossible.
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping pull-on-update test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("initial gix clone over file://");
    let first = rec
        .resolved_commit
        .clone()
        .expect("HEAD resolved on install");

    // Advance the remote (source repo) by one commit.
    fs::write(repo_dir.join("NEW.txt"), "new\n").unwrap();
    assert!(
        git_in(&repo_dir, &["add", "-A"]),
        "stage new file in source repo"
    );
    assert!(
        git_in(&repo_dir, &["commit", "-q", "-m", "c2"]),
        "commit in source repo"
    );

    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(report.updated.contains(&rec.id), "unpinned package updated");

    let after = mgr.list();
    let updated = after
        .iter()
        .find(|p| p.id == rec.id)
        .expect("package still installed");
    let second = updated
        .resolved_commit
        .clone()
        .expect("HEAD resolved on update");
    assert_ne!(
        first, second,
        "update pulled the new upstream commit (resolved_commit advanced)"
    );

    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir.join("NEW.txt").exists(),
        "pulled worktree reflects the new commit"
    );
}

/// True-network smoke test for the remote https transport. Ignored by default (and additionally
/// gated on `CYRUP_GIT_NETWORK_TESTS=1`) so CI stays hermetic; run with
/// `cargo test -p cyrup-resources -- --ignored` plus the env var to exercise a real GitHub clone.
#[tokio::test]
#[ignore = "true-network clone; set CYRUP_GIT_NETWORK_TESTS=1 and run with --ignored"]
async fn git_clone_real_network_https() {
    if std::env::var_os("CYRUP_GIT_NETWORK_TESTS").is_none() {
        eprintln!("skipping true-network clone: set CYRUP_GIT_NETWORK_TESTS=1");
        return;
    }
    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url: "https://github.com/octocat/Hello-World.git".into(),
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("real https clone over the network");
    assert!(
        rec.resolved_commit.is_some(),
        "remote HEAD resolved over https"
    );
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(
        store_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "remote worktree materialized at {}",
        store_dir.display()
    );
}
