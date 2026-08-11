//! Startup package-update availability check — the port of Pi's
//! `DefaultPackageManager.checkForAvailableUpdates` (`package-manager.ts:1175-1238`) and the
//! `InteractiveMode.checkForPackageUpdates` wrapper that fires it (`interactive-mode.ts:850-861`,
//! `:922-937`).
//!
//! # The gap this closes
//!
//! Pi kicks off two detached checks the moment the interactive UI is up and reports whichever
//! settles first (`interactive-mode.ts:843-861`): a release-feed version check and a package-update
//! check. cyrup had NEITHER — and the `NetworkPolicy::allow_update_check()` gate that exists to
//! govern them (`cyrup-config/src/policy.rs:39-41`) had **zero** production callers, only its own
//! unit tests. This module is that missing caller and the package half of the missing feature.
//!
//! # Scope: the package half only
//!
//! Pi's OTHER check, `checkForNewPiVersion`, fetches `https://pi.dev/api/latest-version`
//! (`utils/version-check.ts`) and offers `pi update`. That endpoint is pi's own release feed for the
//! `pi` npm distribution; cyrup is a rebranded from-scratch port with no such feed and no
//! self-update channel to point at, which puts it in the same category as the first-run wizard
//! (`crates/cyrup/src/startup.rs:17-30`) — a gate that is ported faithfully but can never fire.
//! Porting it would mean inventing a cyrup release endpoint, which is a product decision, not a
//! parity one. The package half needs no such invention: cyrup already installs packages from git
//! and already has `cyrup update --extensions` to act on the answer (`subcommands.rs`).
//!
//! # Shape of the port
//!
//! [`check_for_available_updates`] walks the installed-package registries exactly as upstream walks
//! its settings `packages` arrays: local (`Path`) and pinned sources are skipped
//! (`parsed.type === "local" || parsed.pinned`, `:1198`), a package whose install directory is gone
//! is skipped (`existsSync(installedPath)`, `:1204/:1222`), and every survivor is asked
//! [`git_has_available_update`] — upstream's `gitHasAvailableUpdate` (`:1521-1536`): compare
//! `git rev-parse HEAD` in the install tree against `git ls-remote origin <upstream-ref|HEAD>`.
//! Checks run at [`UPDATE_CHECK_CONCURRENCY`], upstream's figure, and every failure mode (no `git`,
//! no network, a private remote that wants a password) resolves to "no update" rather than an error
//! — upstream's `catch { return false }`.
//!
//! npm packages have no analog here: the npm channel is dropped in the Rust port (R-09-021,
//! `PackageSource::parse` returns `Unsupported` for `npm:`), so upstream's `npmHasAvailableUpdate`
//! branch has nothing to run against.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_config::policy::NetworkPolicy;
use cyrup_resources::{PackageManager, PackageSource, PackageStore};

/// Pi's `NETWORK_TIMEOUT_MS` (`package-manager.ts:38`) — the per-`git`-invocation budget. A remote
/// that hangs must never wedge the check, which is why every command is bounded.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);

/// Pi's `UPDATE_CHECK_CONCURRENCY` (`package-manager.ts:39`): how many packages are probed at once.
pub const UPDATE_CHECK_CONCURRENCY: usize = 4;

/// One package with a newer remote — Pi's `PackageUpdate` (`package-manager.ts`), reduced to the one
/// field the notification uses (`updates.map((u) => u.displayName)`, `interactive-mode.ts:934`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageUpdate {
    /// Upstream's `displayName` for a git source: `${parsed.host}/${parsed.path}` (`:1228`).
    pub display_name: String,
}

/// The display name upstream builds for a git package (`:1228`), derived from the SOURCE URL rather
/// than the install path so it reads the way the user typed it (`github.com/nicobailon/pi-intercom`).
fn git_display_name(url: &str) -> String {
    match cyrup_resources::parse_git_url(url) {
        Some(parsed) => format!("{}/{}", parsed.host, parsed.path),
        // Upstream cannot reach this branch (an installed git package parsed once already); keeping
        // the raw source is strictly better than dropping the package from the report.
        None => url.to_string(),
    }
}

/// Pi `checkForAvailableUpdates` (`package-manager.ts:1175-1238`): every installed, non-pinned,
/// non-local package whose remote is ahead of the installed tree.
///
/// Returns an EMPTY list — never an error — when the [`NetworkPolicy`] declines
/// (upstream's `if (isOfflineModeEnabled()) return []`, `:1176`). That is this crate's only
/// production consumer of [`NetworkPolicy::allow_update_check`].
pub async fn check_for_available_updates(
    store: PackageStore,
    policy: NetworkPolicy,
) -> Vec<PackageUpdate> {
    if !policy.allow_update_check() {
        return Vec::new();
    }
    let installed = PackageManager::new(store.clone()).list();

    // The (install_dir, display_name) pairs worth probing — upstream's filtered `checks` array.
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    for pkg in installed {
        // `parsed.type === "local" || parsed.pinned` (`:1198`). Oci is unimplemented (`Unsupported`
        // at install time), so it can never appear here; skipping it keeps that true.
        let PackageSource::Git { url, .. } = &pkg.source else {
            continue;
        };
        if pkg.source.pin().is_pinned() {
            continue;
        }
        let Some(dir) = store.package_dir(pkg.scope, &pkg.id) else {
            continue;
        };
        // `if (!existsSync(installedPath)) return undefined` (`:1222`).
        if !dir.exists() {
            continue;
        }
        candidates.push((dir, git_display_name(url)));
    }

    // Upstream's `runWithConcurrency(checks, UPDATE_CHECK_CONCURRENCY)`: a fixed worker pool over an
    // index cursor, results kept in the INPUT order (`results[index] = …`), which is what makes the
    // notification's package list stable rather than a completion race.
    let mut out: Vec<Option<PackageUpdate>> = vec![None; candidates.len()];
    for chunk_start in (0..candidates.len()).step_by(UPDATE_CHECK_CONCURRENCY) {
        let mut joins = Vec::new();
        for (offset, (dir, name)) in candidates
            .iter()
            .enumerate()
            .skip(chunk_start)
            .take(UPDATE_CHECK_CONCURRENCY)
        {
            let dir = dir.clone();
            let name = name.clone();
            joins.push(tokio::spawn(async move {
                (offset, git_has_available_update(&dir).await, name)
            }));
        }
        for join in joins {
            if let Ok((idx, true, name)) = join.await
                && let Some(slot) = out.get_mut(idx)
            {
                *slot = Some(PackageUpdate { display_name: name });
            }
        }
    }
    out.into_iter().flatten().collect()
}

/// Pi `gitHasAvailableUpdate` (`package-manager.ts:1521-1536`): `git rev-parse HEAD` in the install
/// tree versus the remote head. Any failure ⇒ `false` (upstream's `catch { return false }`), so a
/// private remote or a missing `git` is silent rather than a startup error.
pub async fn git_has_available_update(installed_path: &Path) -> bool {
    let Some(local) = git_capture(installed_path, &["rev-parse", "HEAD"], false).await else {
        return false;
    };
    let Some(remote) = remote_git_head(installed_path).await else {
        return false;
    };
    local.trim() != remote.trim()
}

/// Pi `getRemoteGitHead` (`:1538-1554`): prefer the tracked upstream branch's ref, fall back to the
/// remote's `HEAD`.
async fn remote_git_head(installed_path: &Path) -> Option<String> {
    if let Some(upstream_ref) = git_upstream_ref(installed_path).await
        && let Some(out) =
            git_capture(installed_path, &["ls-remote", "origin", &upstream_ref], true).await
        && let Some(sha) = first_sha(&out, None)
    {
        return Some(sha);
    }
    let out = git_capture(installed_path, &["ls-remote", "origin", "HEAD"], true).await?;
    // Upstream anchors this one on the ref name: `/^([0-9a-f]{40})\s+HEAD$/m`.
    first_sha(&out, Some("HEAD"))
}

/// Pi `getGitUpstreamRef` (`:1618-1634`): the tracked branch as `refs/heads/<branch>`, and only when
/// it is on `origin` — upstream ignores any other remote.
async fn git_upstream_ref(installed_path: &Path) -> Option<String> {
    let out = git_capture(installed_path, &["rev-parse", "--abbrev-ref", "@{upstream}"], false).await?;
    let trimmed = out.trim();
    let branch = trimmed.strip_prefix("origin/")?;
    if branch.is_empty() {
        return None;
    }
    Some(format!("refs/heads/{branch}"))
}

/// The first `<40-hex-sha>\t<ref>` line of an `ls-remote` answer, optionally requiring an exact ref
/// name — upstream's two regexes (`:1543`, `:1549`) in one helper.
fn first_sha(output: &str, require_ref: Option<&str>) -> Option<String> {
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        // A blank or malformed line is SKIPPED, not fatal — upstream's `/…/m` regex keeps scanning
        // the remaining lines, and `git ls-remote` output routinely carries a trailing newline.
        let Some(sha) = parts.next() else { continue };
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        match require_ref {
            Some(want) if parts.next() != Some(want) => continue,
            _ => return Some(sha.to_string()),
        }
    }
    None
}

/// One bounded `git` invocation with stdout captured. `remote` adds upstream's
/// `GIT_TERMINAL_PROMPT=0` (`runGitRemoteCommand`, `:1636-1644`) so a private remote fails fast
/// instead of blocking on a credential prompt no TTY will ever answer.
async fn git_capture(cwd: &Path, args: &[&str], remote: bool) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // Upstream's timeout arm kills the child; dropping the future on timeout drops the child,
        // and `kill_on_drop` is what turns that drop into the same kill.
        .kill_on_drop(true);
    if remote {
        cmd.env("GIT_TERMINAL_PROMPT", "0");
    }
    let child = cmd.spawn().ok()?;
    let out = tokio::time::timeout(NETWORK_TIMEOUT, child.wait_with_output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Fire the check DETACHED and deliver its answer over a channel — Pi's
/// `this.checkForPackageUpdates().then((u) => u.length > 0 && this.showPackageUpdateNotification(u))`
/// (`interactive-mode.ts:850-856`), which is deliberately not awaited before the first frame.
///
/// `None` when the [`NetworkPolicy`] declines, so the caller wires no channel at all and the run
/// loop never grows an arm it cannot use. The task sends AT MOST one message and only when something
/// is actually out of date (upstream's `if (updates.length > 0)`), then drops the sender.
pub fn spawn_package_update_check(
    package_dir: PathBuf,
    project_root: Option<PathBuf>,
    policy: NetworkPolicy,
) -> Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>> {
    if !policy.allow_update_check() {
        return None;
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let store = PackageStore::new(package_dir, project_root);
    tokio::spawn(async move {
        let updates = check_for_available_updates(store, policy).await;
        if !updates.is_empty() {
            let _ = tx.send(updates.into_iter().map(|u| u.display_name).collect());
        }
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[test]
    fn ls_remote_sha_extraction_matches_pis_two_regexes() {
        let out = "5b1c0d3f9a2e4b6c8d0f1a2b3c4d5e6f70819202\tHEAD\n\
                   aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/heads/main\n";
        // Anchored on the ref name (upstream's `\s+HEAD$`).
        assert_eq!(
            first_sha(out, Some("HEAD")).as_deref(),
            Some("5b1c0d3f9a2e4b6c8d0f1a2b3c4d5e6f70819202")
        );
        assert_eq!(
            first_sha(out, Some("refs/heads/main")).as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        // Unanchored (upstream's `^([0-9a-f]{40})\s+`) takes the first line.
        assert_eq!(
            first_sha(out, None).as_deref(),
            Some("5b1c0d3f9a2e4b6c8d0f1a2b3c4d5e6f70819202")
        );
        // Junk / short shas / an empty answer never produce a head.
        assert_eq!(first_sha("fatal: could not read\n", None), None);
        assert_eq!(first_sha("abc123\tHEAD\n", Some("HEAD")), None);
        assert_eq!(first_sha("", None), None);
        // A blank leading line must not abort the scan.
        assert_eq!(
            first_sha("\n\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tHEAD\n", Some("HEAD")).as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn the_display_name_is_host_slash_path() {
        assert_eq!(
            git_display_name("https://github.com/nicobailon/pi-intercom"),
            "github.com/nicobailon/pi-intercom"
        );
        assert_eq!(
            git_display_name("https://github.com/nicobailon/pi-intercom.git"),
            "github.com/nicobailon/pi-intercom"
        );
    }

    /// The offline gate short-circuits before any process is spawned — upstream `:1176`.
    #[tokio::test]
    async fn an_offline_policy_reports_nothing_and_spawns_no_task() {
        let policy = NetworkPolicy {
            offline: true,
            update_check: true,
            install_telemetry: false,
            analytics: false,
        };
        let store = PackageStore::new(std::env::temp_dir().join("cyrup-upd-none"), None);
        assert!(check_for_available_updates(store, policy).await.is_empty());
        assert!(spawn_package_update_check(
            std::env::temp_dir().join("cyrup-upd-none"),
            None,
            policy
        )
        .is_none());
    }

    /// …and so does the dedicated skip toggle, independently of `offline` (R-07-024: the two knobs
    /// are separate).
    #[tokio::test]
    async fn a_skipped_version_check_reports_nothing() {
        let policy = NetworkPolicy {
            offline: false,
            update_check: false,
            install_telemetry: true,
            analytics: false,
        };
        assert!(spawn_package_update_check(
            std::env::temp_dir().join("cyrup-upd-skip"),
            None,
            policy
        )
        .is_none());
    }

    /// A directory that is not a git work tree can never claim an update — `git rev-parse HEAD`
    /// fails and upstream's `catch` returns `false`. This is what keeps a half-installed or
    /// hand-deleted package out of the notification.
    #[tokio::test]
    async fn a_non_repo_install_dir_reports_no_update() {
        // `TempDir` (not a hand-rolled `temp_dir().join(..)`) so the directory is removed when
        // the guard drops instead of accumulating one per run under `/tmp`.
        let dir = tempfile::Builder::new()
            .prefix("cyrup-upd-notrepo-")
            .tempdir()
            .unwrap();
        assert!(!git_has_available_update(dir.path()).await);
    }
}
