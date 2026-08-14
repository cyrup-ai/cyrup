//! F1 / G133 — `ExtensionConfig::save` is REACHABLE, through the real host command path.
//!
//! G133 landed `ExtensionConfig::save` plus the `OrderedJson` machinery it needs (v0.8.0
//! `extension-config.ts:240-293`) with **zero non-test callers**: every `.save(` call site in the
//! crate sat inside `#[cfg(test)] mod tests`. The three behaviours that rewrite exists to provide —
//! non-extension keys preserved, a corrupt file refused, a symlinked config written through — were
//! therefore unobservable in cyrup, because cyrup never saved this config at all.
//!
//! These tests drive the two upstream WRITERS through the same route a human does:
//! `ExtensionHost::execute_native_command("permission-system", …)` (the call
//! `cyrup-session-svc/src/session.rs:958` makes for a `/`-prefixed submission) →
//! `NativeExtension::execute_command` → `save_extension_config` (pi `index.ts:1402-1420`) /
//! `set_yolo_mode` (pi `index.ts:1422-1469`) → `ExtensionConfig::save`. Nothing here calls `save`
//! itself; every assertion is against the bytes that landed on disk.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cyrup_core::CancelToken;
use cyrup_ext::{ExtensionHost, HostConfig, HostServices, NativeExtension, NotifyKind};
use cyrup_permission_system::{PermissionSystemExtension, PERMISSION_SYSTEM_COMMAND};

/// A `HostServices` backend that records every notification the extension raises, so a test can
/// assert on the channel the HUMAN actually sees rather than on a returned string.
///
/// This is what a handler needing a non-Info level talks to: per the convention on
/// `NativeExtension::execute_command`, such a handler notifies itself and returns `Ok(None)`, so the
/// notification IS the whole observable output and asserting on it is strictly stronger than
/// asserting on a return value the session would only have re-emitted at Info.
#[derive(Default)]
struct RecordingHost {
    notifications: Mutex<Vec<(String, NotifyKind)>>,
}

impl RecordingHost {
    fn taken(&self) -> Vec<(String, NotifyKind)> {
        self.notifications.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl HostServices for RecordingHost {
    fn notify(&self, message: &str, kind: NotifyKind) {
        if let Ok(mut g) = self.notifications.lock() {
            g.push((message.to_string(), kind));
        }
    }
}

/// `<agent_dir>/cyrup-permission-system/config.json` — `PermissionSystemExtension::config_path_for`.
fn config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("cyrup-permission-system").join("config.json")
}

fn write_config(agent_dir: &Path, body: &str) -> PathBuf {
    let path = config_path(agent_dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

fn read_config(agent_dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(config_path(agent_dir)).unwrap();
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("config.json is not JSON ({e}): {raw}"))
}

/// A config document carrying keys this extension does NOT own, alongside the three it does.
/// `mergeExtensionFields` (v0.8.0 `extension-config.ts:186-216`) must carry all of them through.
const OPERATOR_CONFIG: &str = r#"{
  "$schema": "https://example.invalid/permission-system.json",
  "enabled": true,
  "debug": false,
  "yoloMode": false,
  "forwardedPromptTimeoutSeconds": 30,
  "operatorNotes": { "owner": "platform-team", "ticket": "OPS-1234" }
}
"#;

/// Load the extension into a REAL `ExtensionHost` and run `/permission-system <args>` through
/// `execute_native_command` — the same routing `cyrup-session-svc` performs for a slash command.
///
/// Uses `load_native_with_services`, the route the session builder itself takes (P-1), so the
/// extension holds a live `HostServices` backend and its notifications are observable. Returns the
/// handler's output (`None` when it notified at its own level instead), the live extension handle,
/// and the recorder.
async fn run_command_observed(
    agent_dir: &Path,
    args: &str,
    has_ui: bool,
) -> (Option<String>, Arc<PermissionSystemExtension>, Arc<RecordingHost>) {
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.to_path_buf(),
        agent_dir.to_path_buf(),
    ));
    let host = ExtensionHost::new(HostConfig {
        has_ui,
        cwd: agent_dir.to_path_buf(),
        ..HostConfig::default()
    });
    let recorder = Arc::new(RecordingHost::default());
    let as_native: Arc<dyn NativeExtension> = ext.clone();
    host.load_native_with_services(as_native, recorder.clone() as Arc<dyn HostServices>)
        .await
        .expect("load native");

    let out = host
        .execute_native_command(PERMISSION_SYSTEM_COMMAND, args, &CancelToken::new())
        .await
        .expect("routing must not fail")
        .expect("the `permission-system` command must be OWNED by a native extension")
        .expect("the handler must not error");
    (out, ext, recorder)
}

/// The SPEAKING-command form: the handler is expected to return text (which the session surfaces as
/// an Info notification). Also asserts the handler raised no notification of its own — a speaking
/// command must not ALSO notify, or the human sees the same thing twice.
async fn run_command(agent_dir: &Path, args: &str) -> (String, Arc<PermissionSystemExtension>) {
    let (out, ext, recorder) = run_command_observed(agent_dir, args, true).await;
    let raised = recorder.taken();
    assert!(
        raised.is_empty(),
        "a command that returns text must not ALSO notify — the session already surfaces the \
         return value, so doing both double-prints: {raised:?}"
    );
    (out.expect("the handler returns text output"), ext)
}

/// THE POINT OF F1: a real `/permission-system yoloMode on` reaches `ExtensionConfig::save`, the
/// file on disk changes, and every key this extension does not own survives the write
/// (v0.8.0 `extension-config.ts:186-216`, reached via `index.ts:1422-1469`).
#[tokio::test]
async fn the_yolo_setting_command_persists_to_disk_and_preserves_foreign_keys() {
    let agent_dir = tempfile::tempdir().unwrap();
    write_config(agent_dir.path(), OPERATOR_CONFIG);

    let (out, ext) = run_command(agent_dir.path(), "yoloMode on").await;

    assert!(out.contains("YOLO mode on"), "handler output: {out}");
    assert!(ext.yolo_mode(), "the live in-memory config must report yolo on");

    let saved = read_config(agent_dir.path());
    assert_eq!(saved["yoloMode"], serde_json::json!(true), "on disk: {saved}");
    assert_eq!(
        saved["operatorNotes"],
        serde_json::json!({ "owner": "platform-team", "ticket": "OPS-1234" }),
        "a non-extension key must survive the save: {saved}"
    );
    assert_eq!(
        saved["$schema"],
        serde_json::json!("https://example.invalid/permission-system.json"),
        "`$schema` must survive the save: {saved}"
    );
}

/// MIRROR: the sibling row writes only ITS own field. `debug on` must not drag `yoloMode` with it,
/// and must not disturb the foreign keys either (pi `applySetting` `case "debug"`,
/// `config-modal.ts:49-50` → `setConfig` → `saveExtensionConfig`, `index.ts:1402-1420`).
#[tokio::test]
async fn the_debug_setting_command_writes_only_its_own_field() {
    let agent_dir = tempfile::tempdir().unwrap();
    write_config(agent_dir.path(), OPERATOR_CONFIG);

    let (out, ext) = run_command(agent_dir.path(), "debug on").await;

    assert!(out.contains("Debug logging on"), "handler output: {out}");
    assert!(!ext.yolo_mode(), "toggling debug must not arm yolo mode");

    let saved = read_config(agent_dir.path());
    assert_eq!(saved["debug"], serde_json::json!(true), "on disk: {saved}");
    assert_eq!(saved["yoloMode"], serde_json::json!(false), "yolo must be untouched: {saved}");
    assert_eq!(
        saved["operatorNotes"]["ticket"],
        serde_json::json!("OPS-1234"),
        "a non-extension key must survive: {saved}"
    );
}

/// MIRROR: a command that names no setting is READ-ONLY. The modal's initial view (pi
/// `buildSettingItems`, `config-modal.ts:24-41`) writes nothing, so the file must be byte-identical
/// afterwards — this is what proves the wiring is not "any invocation saves".
#[tokio::test]
async fn a_bare_invocation_renders_settings_without_writing() {
    let agent_dir = tempfile::tempdir().unwrap();
    write_config(agent_dir.path(), OPERATOR_CONFIG);
    let before = std::fs::read_to_string(config_path(agent_dir.path())).unwrap();

    let (out, _ext) = run_command(agent_dir.path(), "").await;

    assert!(out.contains("debug"), "handler output: {out}");
    assert!(out.contains("yoloMode"), "handler output: {out}");
    assert!(out.contains("Config file:"), "pi's modal helpText, config-modal.ts:85: {out}");

    let after = std::fs::read_to_string(config_path(agent_dir.path())).unwrap();
    assert_eq!(before, after, "rendering the settings must not rewrite the file");
}

/// MIRROR: an unknown setting id is rejected and writes nothing (pi `applySetting`'s
/// `default: return config`, `config-modal.ts:53-54`).
#[tokio::test]
async fn an_unknown_setting_writes_nothing() {
    let agent_dir = tempfile::tempdir().unwrap();
    write_config(agent_dir.path(), OPERATOR_CONFIG);
    let before = std::fs::read_to_string(config_path(agent_dir.path())).unwrap();

    let (out, ext) = run_command(agent_dir.path(), "enabled off").await;

    assert!(out.contains("Unknown setting"), "handler output: {out}");
    assert!(!ext.yolo_mode());
    assert_eq!(
        before,
        std::fs::read_to_string(config_path(agent_dir.path())).unwrap(),
        "an unknown setting must not rewrite the file"
    );
}

/// THE SECURITY PROPERTY shared by `saveExtensionConfig` (v0.8.0 `index.ts:1405-1409`) and
/// `setYoloModeFromRuntimeApi` (`:1438-1451`): when the persist FAILS, in-memory yolo mode is left
/// exactly as it was and the reported value is the one still in effect. The command routes through
/// `saveExtensionConfig`, matching upstream's modal (`config-modal.ts:75`), so this exercises the
/// `setConfig` path — but the invariant is the same on both, and it is the half that matters: a gate
/// that believes it is in YOLO mode while disk says otherwise auto-approves on the operator's
/// behalf. The failure is induced by the v0.8.0 refuse-to-clobber rule
/// (`extension-config.ts:249-257`): a config file that exists but cannot be parsed is never
/// overwritten with extension defaults.
#[tokio::test]
async fn a_refused_save_leaves_yolo_mode_off_in_memory_and_on_disk() {
    let agent_dir = tempfile::tempdir().unwrap();
    // Valid-looking permission data the extension must not destroy, inside a document that does not
    // parse (an unterminated object).
    let corrupt = "{\n  \"yoloMode\": false,\n  \"operatorNotes\": \"do not clobber\"\n";
    write_config(agent_dir.path(), corrupt);

    let (out, ext, recorder) = run_command_observed(agent_dir.path(), "yoloMode on", true).await;

    // The refusal is reported through the ERROR notification, not the return value: a save failure
    // needs `NotifyKind::Error`, which the `Ok(Some(String))` channel cannot express (it is always
    // surfaced as Info), so the handler notifies itself and returns `Ok(None)`. Asserting here is
    // strictly stronger than the old assertion on the returned string — this is the channel the
    // human actually sees, and it now also pins the LEVEL.
    let raised = recorder.taken();
    assert_eq!(raised.len(), 1, "a refused save raises exactly ONE notification: {raised:?}");
    assert_eq!(
        raised[0].1,
        NotifyKind::Error,
        "a refused save is an ERROR, not an Info toast: {raised:?}"
    );
    assert!(
        raised[0].0.contains("YOLO mode is unchanged (off)"),
        "a refused save must report the value STILL in effect: {raised:?}"
    );
    // ...and the same one notification carries the WHY (the raw save error) alongside the what, so
    // the human is not told "it failed" in one toast and "here is why" in another.
    assert!(
        raised[0].0.contains("Config file:"),
        "the error names the config file it could not write: {raised:?}"
    );
    assert!(
        raised[0].0.lines().count() >= 3,
        "the single error carries the summary, the path, AND the raw cause: {raised:?}"
    );
    assert!(
        out.is_none(),
        "the handler must return Ok(None) after notifying at Error — returning the sentence too \
         would re-surface it as a second, Info-level toast: {out:?}"
    );
    assert!(
        !ext.yolo_mode(),
        "a failed persist must NOT leave in-memory state claiming yolo mode changed"
    );
    assert_eq!(
        std::fs::read_to_string(config_path(agent_dir.path())).unwrap(),
        corrupt,
        "the corrupt file must be left exactly as found"
    );
}

/// The `has_ui` guard (pi `createPermissionSystemCommandHandler`, v0.8.0 `common.ts:192-195`):
/// with no interactive UI the handler returns without touching the config.
#[tokio::test]
async fn without_a_ui_the_command_declines_and_writes_nothing() {
    let agent_dir = tempfile::tempdir().unwrap();
    write_config(agent_dir.path(), OPERATOR_CONFIG);
    let before = std::fs::read_to_string(config_path(agent_dir.path())).unwrap();

    let (out, ext, recorder) =
        run_command_observed(agent_dir.path(), "yoloMode on", /* has_ui */ false).await;

    // Upstream chose `warning` for this refusal (`common.ts:192-195`), a level the return channel
    // cannot express, so the handler notifies and returns `Ok(None)`. Asserting on the notification
    // is strictly stronger than the old assertion on the returned string: it checks the channel the
    // human sees AND that the level really is `warning`.
    let raised = recorder.taken();
    assert_eq!(raised.len(), 1, "the UI-less refusal raises exactly ONE notification: {raised:?}");
    assert_eq!(
        raised[0].1,
        NotifyKind::Warning,
        "upstream's level for this refusal is `warning` (common.ts:192-195): {raised:?}"
    );
    assert!(
        raised[0].0.contains("requires interactive TUI mode"),
        "notification: {raised:?}"
    );
    assert!(
        out.is_none(),
        "the handler must return Ok(None) after notifying at Warning — returning the same sentence \
         would re-surface it as a second, Info-level toast: {out:?}"
    );
    assert!(!ext.yolo_mode());
    assert_eq!(
        before,
        std::fs::read_to_string(config_path(agent_dir.path())).unwrap(),
        "a UI-less invocation must not rewrite the file"
    );
}
