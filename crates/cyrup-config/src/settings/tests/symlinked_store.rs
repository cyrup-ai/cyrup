//! CFG-112 — `FileSettingsStore` resolves the PHYSICAL settings file before it locks and writes,
//! so a symlinked `settings.json` is locked on `<target>.lock` (the sidecar every
//! `cyrup-ext-subagents` settings writer locks) and saved through the link onto its target.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::lock::FileLock;
use crate::settings::*;

/// A tempdir holding `dotfiles/settings.json` (the real file) and `settings.json`, a relative
/// symlink to it — a dotfile manager's layout.
#[cfg(unix)]
fn linked_layout(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let dotfiles = root.join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    let real = dotfiles.join("settings.json");
    std::fs::write(&real, r#"{"theme":"dark"}"#).unwrap();
    let link = root.join("settings.json");
    std::os::unix::fs::symlink("dotfiles/settings.json", &link).unwrap();
    (real, link)
}

/// THE exclusion: a holder of the TARGET's sidecar — exactly what the subagents writers take
/// (`lock_settings_file(&settings_write_target(path))`) — blocks a `FileSettingsStore` write made
/// through the link until it releases, and the write then lands on the target with the link kept.
///
/// Red before CFG-112: `with_lock` locked `settings.json.lock` beside the LINK, so it did not
/// wait for the target's holder at all (and its rename then replaced the link with a plain file).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_symlinked_settings_file_is_locked_on_its_targets_sidecar() {
    let tmp = crate::test_util::temp_dir();
    let (real, link) = linked_layout(&tmp);
    let real_physical = std::fs::canonicalize(&real).unwrap();

    let held = FileLock::acquire(&real_physical, None).await.unwrap();
    let store = Arc::new(FileSettingsStore::new(
        link.clone(),
        tmp.join("project.json"),
    ));
    let writer = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .with_lock(SettingsScope::Global, &mut |current| {
                    let mut doc: serde_json::Value =
                        serde_json::from_str(current.unwrap_or("{}")).unwrap();
                    doc["quietStartup"] = serde_json::Value::Bool(true);
                    Some(doc.to_string())
                })
                .await
        })
    };
    let mut writer = std::pin::pin!(writer);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut writer)
            .await
            .is_err(),
        "a FileSettingsStore write through the link must wait for the holder of the target's \
         sidecar"
    );
    drop(held);
    writer
        .await
        .expect("writer task")
        .expect("the write succeeds once the holder releases");

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&real).unwrap()).unwrap();
    assert_eq!(after["theme"], serde_json::json!("dark"));
    assert_eq!(after["quietStartup"], serde_json::json!(true));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link survives the write"
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("dotfiles/settings.json")
    );
    assert!(
        !tmp.join("settings.json.lock").exists(),
        "no sidecar is taken beside the link"
    );
}

/// The save through the link keeps the link, its text and the TARGET's mode, for both scopes,
/// through the real `SettingsManager` writer.
#[cfg(unix)]
#[tokio::test]
async fn a_settings_save_through_a_symlink_keeps_the_link_and_the_targets_mode() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = crate::test_util::temp_dir();
    let global_root = tmp.join("global");
    let project_root = tmp.join("project");
    std::fs::create_dir_all(&global_root).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let (global_real, global_link) = linked_layout(&global_root);
    let (project_real, project_link) = linked_layout(&project_root);
    std::fs::set_permissions(&global_real, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&project_real, std::fs::Permissions::from_mode(0o640)).unwrap();

    let store = Arc::new(FileSettingsStore::new(
        global_link.clone(),
        project_link.clone(),
    ));
    let mut mgr = SettingsManager::load(store, true);
    mgr.set(SettingsScope::Global, "quietStartup", true)
        .await
        .unwrap();
    mgr.set(SettingsScope::Project, "quietStartup", true)
        .await
        .unwrap();

    for (real, link, mode) in [
        (&global_real, &global_link, 0o600),
        (&project_real, &project_link, 0o640),
    ] {
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{} must stay a link",
            link.display()
        );
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(real).unwrap()).unwrap();
        assert_eq!(after["theme"], serde_json::json!("dark"));
        assert_eq!(after["quietStartup"], serde_json::json!(true));
        assert_eq!(
            std::fs::metadata(real).unwrap().permissions().mode() & 0o7777,
            mode,
            "the target's mode survives the save"
        );
    }
}

/// A plain (unlinked) settings file keeps its mode across a save too — pi's `writeFileSync`
/// leaves an existing file's mode alone. Before CFG-112 the temp was created at the umask
/// default, so a `0600` settings file came back `0644`.
#[cfg(unix)]
#[tokio::test]
async fn a_plain_settings_save_keeps_the_existing_mode() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = crate::test_util::temp_dir();
    let path = tmp.join("settings.json");
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = FileSettingsStore::new(path.clone(), tmp.join("project.json"));
    store
        .with_lock(SettingsScope::Global, &mut |_| {
            Some("{\"a\":1}".to_string())
        })
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
}

/// A dangling link names the file to create (pi `resolveSettingsWriteTarget`'s readlink arm).
#[cfg(unix)]
#[tokio::test]
async fn a_settings_save_through_a_dangling_symlink_creates_the_link_target() {
    let tmp = crate::test_util::temp_dir();
    let dotfiles = tmp.join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    let link = tmp.join("settings.json");
    std::os::unix::fs::symlink(dotfiles.join("settings.json"), &link).unwrap();
    let store = FileSettingsStore::new(link.clone(), tmp.join("project.json"));
    store
        .with_lock(SettingsScope::Global, &mut |current| {
            assert_eq!(current, None);
            Some("{}".to_string())
        })
        .await
        .unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(dotfiles.join("settings.json")).unwrap(),
        "{}"
    );
}

/// A settings file in a directory that does not exist yet (a project with no `.cyrup/`) is still
/// created on first write, as before CFG-112: the parent is made before the target is resolved.
#[tokio::test]
async fn a_first_write_still_creates_the_settings_directory() {
    let tmp = crate::test_util::temp_dir();
    let path = tmp.join(".cyrup").join("settings.json");
    let store = FileSettingsStore::new(tmp.join("global.json"), path.clone());
    store
        .with_lock(SettingsScope::Project, &mut |_| Some("{}".to_string()))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
}

/// pi `resolveSettingsWriteTarget`: a missing path ending in a separator cannot name a new file
/// (the `ENOENT` is rethrown), and a missing target needs an existing physical parent.
#[test]
fn write_target_resolution_refuses_a_trailing_separator_and_a_missing_parent() {
    let tmp = crate::test_util::temp_dir();
    let mut trailing = tmp.join("missing").into_os_string();
    trailing.push("/");
    let err = resolve_settings_write_target(Path::new(&trailing)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    let err = resolve_settings_write_target(&tmp.join("no-dir").join("settings.json")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    let fresh = tmp.join("settings.json");
    assert_eq!(
        resolve_settings_write_target(&fresh).unwrap(),
        std::fs::canonicalize(&*tmp).unwrap().join("settings.json")
    );
}

/// A directory link in the middle of the path is followed too.
#[cfg(unix)]
#[test]
fn write_target_resolution_follows_a_directory_link() {
    let tmp = crate::test_util::temp_dir();
    let real_dir = tmp.join("real");
    std::fs::create_dir(&real_dir).unwrap();
    std::os::unix::fs::symlink(&real_dir, tmp.join("alias")).unwrap();
    assert_eq!(
        resolve_settings_write_target(&tmp.join("alias").join("settings.json")).unwrap(),
        std::fs::canonicalize(&real_dir)
            .unwrap()
            .join("settings.json")
    );
}
