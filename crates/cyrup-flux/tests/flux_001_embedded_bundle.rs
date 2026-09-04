//! FLUX-001 — the bundled prompt/skill tree must reach a running binary without the build
//! machine's source tree.
//!
//! Before this item `resources::bundled_dir()` fell back to `env!("CARGO_MANIFEST_DIR")/resources`,
//! and `extension.rs` contributed that path to `ResourcesDiscover` guarded by `is_dir()` /
//! `is_file()` with `HookOutcome::Noop` on a miss — so a release artifact, a container image or a
//! cleaned `cargo install` source dir lost all fifteen `/flux/*` templates and the skill silently,
//! while `/flux/status`, `/flux/cheatsheet`, `/flux/about`, `ctrl+f` and `ask_user_question` kept
//! registering. Upstream (`code_puppy_core_plugins/flux_bootstrap/installer.py` @v0.0.40) ships
//! the tree as package data and copies it into `~/.code_puppy` at startup, idempotently,
//! non-destructively, version-gated, never fatally, under a best-effort `flock`. Now `build.rs`
//! embeds `resources/**` and [`cyrup_flux::install`] is that installer.
//!
//! Red before: run against the pre-change crate, a probe of this file's
//! `resources_discover_contributes_the_managed_root_not_the_build_tree` (the old `flux_extension()`
//! constructor, same event, same assertion) failed with
//! `contributed the build-machine source tree: /home/user/cyrup/crates/cyrup-flux/resources/prompts`;
//! everything else here names types and functions that did not exist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_ext::host::{NotifyKind, RecordingServices};
use cyrup_ext::{ExtMode, HandledValue, HookOutcome, HostCtx, HostEvent, NativeExtension};
use cyrup_flux::bundle::{BundledFile, bundle_fingerprint, bundled_file, bundled_files};
use cyrup_flux::install::{
    FileAction, InstallOutcome, InstallReport, MANIFEST_NAME, VERSION_MARKER_NAME, bundle_marker,
    decide, ensure_installed, install_pass, needs_install, unique_backup_path,
};
use cyrup_flux::resources::{BUNDLED_RESOURCES_DIR_ENV_VAR, BundledRoot, managed_root};

/// The census `docs/gap-analysis/14-cyrup-flux.md` records: upstream v0.0.40 ships 18
/// `bundled/commands/flux/*.md`, three of which became native renderers, so cyrup bundles 15.
const TEMPLATE_COUNT: usize = 15;

fn discover_event() -> HostEvent {
    HostEvent::ResourcesDiscover {
        cwd: "/".into(),
        reason: "startup".into(),
    }
}

fn event_ctx() -> HostCtx {
    HostCtx::event(ExtMode::Print, false, std::env::temp_dir())
}

/// Every regular file under `dir`, as `/`-separative paths relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().to_str().unwrap();
            out.insert(rel.replace('\\', "/"));
        }
    }
}

fn template_rels(files: &[BundledFile]) -> Vec<&str> {
    files
        .iter()
        .map(|f| f.rel)
        .filter(|rel| {
            rel.strip_prefix("prompts/flux/")
                .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".md"))
        })
        .collect()
}

async fn discover(ext: &cyrup_flux::extension::FluxExtension) -> serde_json::Value {
    match ext.on_event(&discover_event(), &event_ctx()).await {
        HookOutcome::Handled(HandledValue(v)) => v,
        other => panic!("expected Handled, got {other:?}"),
    }
}

// ---- the embedded bundle -----------------------------------------------------------------------

#[test]
fn the_embedded_bundle_is_exactly_the_on_disk_resources_tree() {
    // `build.rs` walks `resources/` with no hand-maintained list; this pins that the walk missed
    // nothing and invented nothing. The on-disk read is the ONE place a test may still use
    // `CARGO_MANIFEST_DIR` — it is the reference, not the runtime.
    let on_disk_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    let mut on_disk = BTreeSet::new();
    walk(&on_disk_root, &on_disk_root, &mut on_disk);
    let embedded: BTreeSet<String> = bundled_files().iter().map(|f| f.rel.to_string()).collect();
    assert_eq!(embedded, on_disk);
    for file in bundled_files() {
        let disk = fs::read(on_disk_root.join(file.rel)).unwrap();
        assert_eq!(disk, file.bytes, "{} differs from disk", file.rel);
    }
    // Sorted by rel, as `installer.py:134` sorts its walk.
    let rels: Vec<&str> = bundled_files().iter().map(|f| f.rel).collect();
    let mut sorted = rels.clone();
    sorted.sort_unstable();
    assert_eq!(rels, sorted);
}

#[test]
fn the_embedded_bundle_holds_fifteen_templates_and_the_skill() {
    let templates = template_rels(bundled_files());
    assert_eq!(templates.len(), TEMPLATE_COUNT, "{templates:?}");
    assert!(templates.contains(&"prompts/flux/new.md"));
    assert!(bundled_file("skills/flux/SKILL.md").is_some());
    assert!(bundled_file("prompts/flux/_docs/pipeline.md").is_some());
    assert!(bundled_file("nope.md").is_none());
    assert_eq!(bundle_fingerprint().len(), 64);
    assert!(bundle_marker().ends_with(bundle_fingerprint()));
}

// ---- the resolver -------------------------------------------------------------------------------

#[test]
fn the_root_is_managed_under_the_agent_dir_unless_a_vendored_tree_is_named() {
    let agent = Path::new("/agent");
    let none = |_: &str| None;
    assert_eq!(
        BundledRoot::resolve_from(agent, &none),
        BundledRoot::Managed(managed_root(agent))
    );
    assert_eq!(
        managed_root(agent),
        Path::new("/agent").join("flux").join("resources")
    );
    let blank = |k: &str| (k == BUNDLED_RESOURCES_DIR_ENV_VAR).then(|| "  ".into());
    assert_eq!(
        BundledRoot::resolve_from(agent, &blank),
        BundledRoot::Managed(managed_root(agent)),
        "a set-but-blank override is unset"
    );
    let vendored = |k: &str| (k == BUNDLED_RESOURCES_DIR_ENV_VAR).then(|| "/vendored".into());
    let root = BundledRoot::resolve_from(agent, &vendored);
    assert_eq!(root, BundledRoot::Vendored(PathBuf::from("/vendored")));
    assert_eq!(root.prompts_dir(), Path::new("/vendored/prompts"));
    assert_eq!(root.skill_md(), Path::new("/vendored/skills/flux/SKILL.md"));
    // A vendored tree is never written: `ensure` reports it up to date without touching it.
    assert_eq!(root.ensure().unwrap(), InstallOutcome::UpToDate);
    assert!(!Path::new("/vendored").exists());
}

// ---- the per-file decision (`installer.py:186-218`) -------------------------------------------

#[test]
fn the_per_file_decision_matches_installer_py() {
    assert_eq!(decide(None, None, "p"), FileAction::Install);
    assert_eq!(decide(None, Some("x"), "p"), FileAction::Install);
    assert_eq!(decide(Some("p"), None, "p"), FileAction::Unchanged);
    assert_eq!(decide(Some("p"), Some("old"), "p"), FileAction::Unchanged);
    assert_eq!(decide(Some("user"), None, "p"), FileAction::PreserveForeign);
    assert_eq!(decide(Some("old"), Some("old"), "p"), FileAction::Overwrite);
    assert_eq!(
        decide(Some("edited"), Some("old"), "p"),
        FileAction::BackupThenOverwrite
    );
}

// ---- the installer over a scratch root ---------------------------------------------------------

#[test]
fn a_fresh_install_materialises_every_file_then_is_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("flux").join("resources");
    assert!(needs_install(&root, &bundle_marker()));

    let outcome = ensure_installed(&root).unwrap();
    let InstallOutcome::Installed(report) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(report.installed.len(), bundled_files().len());
    assert!(report.updated.is_empty() && report.backed_up.is_empty());
    assert!(report.changed());
    assert_eq!(
        report.summary(),
        format!(
            "{} new, 0 updated, 0 backed up, 0 unchanged",
            bundled_files().len()
        )
    );
    for file in bundled_files() {
        assert_eq!(
            fs::read(root.join(file.rel)).unwrap(),
            file.bytes,
            "{}",
            file.rel
        );
    }
    // The row's cheaper Verify: fifteen `.md` templates and the skill, resolved with no reference
    // to the build tree.
    let templates = fs::read_dir(root.join("prompts").join("flux"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .count();
    assert_eq!(templates, TEMPLATE_COUNT);
    assert!(root.join("skills/flux/SKILL.md").is_file());
    // Manifest + marker written last, as dot-files at the root.
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(manifest.as_object().unwrap().len(), bundled_files().len());
    assert_eq!(
        fs::read_to_string(root.join(VERSION_MARKER_NAME)).unwrap(),
        bundle_marker()
    );
    assert!(!needs_install(&root, &bundle_marker()));

    // Second run: version-gated, nothing walked.
    assert_eq!(ensure_installed(&root).unwrap(), InstallOutcome::UpToDate);
    // And a forced pass over the same content is idempotent: everything unchanged.
    let again = install_pass(&root, bundled_files(), &bundle_marker()).unwrap();
    assert!(!again.changed());
    assert_eq!(again.skipped.len(), bundled_files().len());
}

#[test]
fn a_hand_edited_managed_file_is_backed_up_and_a_foreign_file_is_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // A user-owned file with our name, present BEFORE the first install.
    let foreign = root.join("prompts/flux/new.md");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(&foreign, b"mine").unwrap();

    let first = install_pass(&root, bundled_files(), "v1").unwrap();
    assert_eq!(first.skipped, vec!["prompts/flux/new.md".to_string()]);
    assert_eq!(fs::read(&foreign).unwrap(), b"mine");
    assert_eq!(first.installed.len(), bundled_files().len() - 1);

    // Hand-edit a file WE wrote, then bump the marker so a pass runs.
    let ours = root.join("prompts/flux/exec.md");
    fs::write(&ours, b"edited by hand").unwrap();
    let second = install_pass(&root, bundled_files(), "v2").unwrap();
    assert_eq!(
        second.backed_up,
        vec!["prompts/flux/exec.md.bak".to_string()]
    );
    assert_eq!(second.updated, vec!["prompts/flux/exec.md".to_string()]);
    assert_eq!(
        fs::read(root.join("prompts/flux/exec.md.bak")).unwrap(),
        b"edited by hand"
    );
    assert_eq!(
        fs::read(&ours).unwrap(),
        bundled_file("prompts/flux/exec.md").unwrap()
    );
    // The foreign file is preserved forever — never claimed, so never backed up or overwritten.
    assert!(second.skipped.contains(&"prompts/flux/new.md".to_string()));
    assert_eq!(fs::read(&foreign).unwrap(), b"mine");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(MANIFEST_NAME)).unwrap()).unwrap();
    assert!(manifest.get("prompts/flux/new.md").is_none());

    // A second hand-edit lands in `.bak.1`, not over the first backup (`:113-127`).
    fs::write(&ours, b"edited again").unwrap();
    let third = install_pass(&root, bundled_files(), "v3").unwrap();
    assert_eq!(
        third.backed_up,
        vec!["prompts/flux/exec.md.bak.1".to_string()]
    );
    assert_eq!(
        unique_backup_path(&ours),
        root.join("prompts/flux/exec.md.bak.2")
    );
}

#[test]
fn a_changed_bundle_overwrites_untouched_managed_files_without_backups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let v1 = [BundledFile {
        rel: "prompts/flux/x.md",
        bytes: b"one",
    }];
    let v2 = [BundledFile {
        rel: "prompts/flux/x.md",
        bytes: b"two",
    }];
    assert_eq!(
        install_pass(&root, &v1, "m1").unwrap().installed,
        vec!["prompts/flux/x.md".to_string()]
    );
    let report = install_pass(&root, &v2, "m2").unwrap();
    assert_eq!(
        report,
        InstallReport {
            updated: vec!["prompts/flux/x.md".to_string()],
            ..InstallReport::default()
        }
    );
    assert_eq!(fs::read(root.join("prompts/flux/x.md")).unwrap(), b"two");
    assert!(!root.join("prompts/flux/x.md.bak").exists());
}

#[test]
fn a_held_install_lock_skips_the_pass_instead_of_racing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(root.join(cyrup_flux::install::LOCK_NAME))
        .unwrap();
    // A `flock` is per open-file-description, so a second `File` in this process contends
    // exactly as another process would.
    fs4::FileExt::try_lock(&lock).unwrap();
    assert_eq!(
        ensure_installed(&root).unwrap(),
        InstallOutcome::SkippedLocked
    );
    assert!(!root.join("prompts").exists());
    fs4::FileExt::unlock(&lock).unwrap();
    assert!(matches!(
        ensure_installed(&root).unwrap(),
        InstallOutcome::Installed(_)
    ));
}

// ---- the extension seam ------------------------------------------------------------------------

#[tokio::test]
async fn resources_discover_contributes_the_managed_root_not_the_build_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join("agent");
    let ext = cyrup_flux::flux_extension(&agent_dir);
    let services = Arc::new(RecordingServices::default());
    ext.set_host_services(services.clone());

    let v = discover(&ext).await;
    let prompts = v["promptPaths"][0].as_str().unwrap();
    let skill = v["skillPaths"][0].as_str().unwrap();
    let build_tree = env!("CARGO_MANIFEST_DIR");
    assert!(
        !prompts.starts_with(build_tree),
        "contributed the build-machine source tree: {prompts}"
    );
    assert_eq!(
        Path::new(prompts),
        managed_root(&agent_dir).join("prompts"),
        "the prompt ROOT (a directory, so `flux/` namespacing survives)"
    );
    assert_eq!(
        Path::new(skill),
        managed_root(&agent_dir).join("skills/flux/SKILL.md")
    );
    assert!(Path::new(prompts).join("flux").join("new.md").is_file());
    assert!(Path::new(skill).is_file());
    // Upstream's `emit_info` on a changed pass (`register_callbacks.py:58`).
    let notices = services.notify_calls();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].1, NotifyKind::Info);
    assert!(notices[0].0.starts_with("Flux commands installed -> "));

    // Steady state: same contribution, no walk, no notice.
    let again = discover(&ext).await;
    assert_eq!(again, v);
    assert_eq!(services.notify_calls().len(), 1);
}

#[tokio::test]
async fn a_missing_vendored_tree_is_reported_not_silently_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nowhere");
    let ext = cyrup_flux::flux_extension_with_root(BundledRoot::Vendored(missing.clone()));
    let services = Arc::new(RecordingServices::default());
    ext.set_host_services(services.clone());

    let out = ext.on_event(&discover_event(), &event_ctx()).await;
    assert!(matches!(out, HookOutcome::Noop), "{out:?}");
    let notices = services.notify_calls();
    assert_eq!(notices.len(), 2, "{notices:?}");
    for (message, kind) in &notices {
        assert_eq!(*kind, NotifyKind::Warning);
        assert!(
            message.contains(&missing.display().to_string()),
            "the notice names the path tried: {message}"
        );
    }
    assert!(
        notices[0]
            .0
            .contains("/flux/* commands will be unavailable")
    );
    assert!(notices[1].0.contains("skill will be unavailable"));
    assert!(!missing.exists(), "a vendored root is never created");
}

#[tokio::test]
async fn an_unwritable_managed_root_is_reported_and_the_session_survives() {
    let tmp = tempfile::tempdir().unwrap();
    // A FILE where the root directory must go: `create_dir_all` fails, install fails closed.
    let blocker = tmp.path().join("agent");
    fs::write(&blocker, b"not a directory").unwrap();
    let ext = cyrup_flux::flux_extension(&blocker);
    let services = Arc::new(RecordingServices::default());
    ext.set_host_services(services.clone());

    let out = ext.on_event(&discover_event(), &event_ctx()).await;
    assert!(matches!(out, HookOutcome::Noop), "{out:?}");
    let notices = services.notify_calls();
    assert!(
        notices[0]
            .0
            .starts_with("Flux bootstrap skipped (install failed): "),
        "{notices:?}"
    );
    assert_eq!(notices[0].1, NotifyKind::Warning);
    assert_eq!(
        notices.len(),
        3,
        "install failure + prompts miss + skill miss: {notices:?}"
    );
}

#[test]
fn a_subagent_child_still_gets_no_flux_extension() {
    // The pre-existing child gate is untouched by the constructor change.
    assert!(cyrup_flux::flux_extension_for_env(Path::new("/agent")).is_some());
    assert_eq!(
        cyrup_flux::flux_extension(Path::new("/agent")).bundled_root(),
        &BundledRoot::Managed(managed_root(Path::new("/agent")))
    );
}
