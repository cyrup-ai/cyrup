//! Startup detection: official-distribution + experimental-features gating and the file-backed
//! settings store (Pi `cli/startup-ui.ts` + `core/experimental.ts`).
//!
//! `shouldRunFirstTimeSetup` (startup-ui.ts:115) fires the first-run wizard only when ALL hold:
//! official distribution, `*_EXPERIMENTAL=1`, the default agent dir, and no existing `settings.json`.
//! For cyrup the FIRST predicate is always false — cyrup is a fork/rebrand, not the official `pi`
//! distribution (startup-ui.ts:36-42 compares the package/app/config-dir names to the `pi`
//! constants), so the wizard is faithfully never invoked here. The predicate is implemented exactly
//! so the gate is real (the wizard UI itself is the ext-UI dialog host, a separate outer layer).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_config::{ConfigDirs, FileSettingsStore, SettingsManager, SettingsStore};

/// The official Pi distribution identity (startup-ui.ts:26-28).
const OFFICIAL_APP_NAME: &str = "pi";
const OFFICIAL_CONFIG_DIR_NAME: &str = ".pi";

/// This build's distribution identity. cyrup is a rebrand, so these never equal the official `pi`
/// constants — [`is_official_distribution`] is consequently always `false`.
const APP_NAME: &str = "cyrup";
const CONFIG_DIR_NAME: &str = ".cyrup";

/// Port of `isOfficialDistribution` (startup-ui.ts:36): true only for the official `pi` build. Always
/// false for cyrup (the rebrand), which is exactly why the first-time wizard does not apply.
pub fn is_official_distribution() -> bool {
    APP_NAME == OFFICIAL_APP_NAME && CONFIG_DIR_NAME == OFFICIAL_CONFIG_DIR_NAME
}

/// Port of `areExperimentalFeaturesEnabled` (experimental.ts): `*_EXPERIMENTAL=1`.
pub fn are_experimental_features_enabled() -> bool {
    std::env::var("CYRUP_EXPERIMENTAL")
        .map(|v| v == "1")
        .unwrap_or(false)
        || std::env::var("PI_EXPERIMENTAL")
            .map(|v| v == "1")
            .unwrap_or(false)
}

/// Port of `shouldRunFirstTimeSetup` (startup-ui.ts:115): official distribution AND experimental
/// features AND default agent dir AND no existing `settings.json`. `agent_dir_overridden` reflects
/// whether `*_CODING_AGENT_DIR` was set (startup-ui.ts:128). For cyrup this returns `false` because
/// [`is_official_distribution`] is false.
pub fn should_run_first_time_setup(settings_path: &Path, agent_dir_overridden: bool) -> bool {
    if !is_official_distribution() {
        return false;
    }
    if !are_experimental_features_enabled() {
        return false;
    }
    if agent_dir_overridden {
        return false;
    }
    !settings_path.exists()
}

/// The persistent, file-backed settings store rooted at the resolved config dirs (replaces the
/// default `InMemorySettingsStore` so `settings.json` is read AND written — the wizard predicate's
/// `existsSync(settingsPath)` and `/settings` persistence both rely on this). Global writes go to
/// `<agent_dir>/settings.json`; project writes to `<cwd>/.cyrup/settings.json`.
pub fn file_settings_store(dirs: &ConfigDirs) -> Arc<dyn SettingsStore> {
    Arc::new(FileSettingsStore::new(
        dirs.settings_path(),
        dirs.project_settings_path(),
    ))
}

/// The global `settings.json` path for the resolved dirs (the wizard predicate reads this).
pub fn settings_path(dirs: &ConfigDirs) -> PathBuf {
    dirs.settings_path()
}

/// Apply the **third** tier of Pi's `sessionDir` chain to an already-resolved layout (main.ts:625-630):
///
/// ```text
/// const sessionDir =
///     (parsed.sessionDir ? normalizePath(parsed.sessionDir) : undefined) ??
///     (envSessionDir ? expandTildePath(envSessionDir) : undefined) ??
///     startupSettingsManager.getSessionDir();
/// ```
///
/// Tiers 1 (`--session-dir`) and 2 (`$CYRUP_SESSION_DIR`) are already folded into `dirs` by
/// `ConfigDirs::resolve`; this reads the merged `settings.json` key off the caller's startup
/// settings manager (Pi `getSessionDir()`, settings-manager.ts:670-673, which tilde-normalizes —
/// `EffectiveSettings::session_dir` does the same). The lookup lives in the BIN, not in
/// `cyrup-config`, because the settings file sits under the `agent_dir` that `ConfigDirs::resolve`
/// is what computes — Pi has the identical ordering and constructs its `startupSettingsManager`
/// only after the dirs (main.ts:610).
///
/// A settings-derived dir is treated as EXPLICIT (used literally, not cwd-encoded): Pi passes it
/// into `createSessionManager(parsed, cwd, sessionDir, …)` (main.ts:630) through the same argument
/// slot as `--session-dir`.
#[must_use]
pub fn apply_settings_session_dir(dirs: ConfigDirs, settings: &SettingsManager) -> ConfigDirs {
    dirs.with_settings_session_dir(settings.effective().session_dir().map(PathBuf::from))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn cyrup_is_not_the_official_distribution() {
        // The rebrand guard: cyrup must never present as the official `pi` build, so the
        // first-time-setup wizard is faithfully gated off regardless of env/state.
        assert!(!is_official_distribution());
    }

    #[test]
    fn first_time_setup_never_runs_for_the_rebrand() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("settings.json");
        // Even with no settings file + (hypothetically) experimental on, the distribution check
        // short-circuits to false.
        assert!(!should_run_first_time_setup(&missing, false));
    }

    #[test]
    fn file_store_round_trips_via_settings_path() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = ConfigDirs {
            agent_dir: dir.path().join("agent"),
            session_dir: dir.path().join("agent/sessions"),
            session_dir_explicit: false,
            package_dir: dir.path().join("agent/packages"),
            cwd: dir.path().join("work"),
            home: dir.path().to_path_buf(),
        };
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        let store = file_settings_store(&dirs);
        // The store reads from the resolved settings path; absent file ⇒ None.
        use cyrup_config::SettingsScope;
        assert!(store.read(SettingsScope::Global).unwrap().is_none());
        std::fs::write(settings_path(&dirs), "{\"theme\":\"dark\"}").unwrap();
        assert!(store.read(SettingsScope::Global).unwrap().is_some());
    }

    fn dirs_under(root: &Path) -> ConfigDirs {
        ConfigDirs {
            agent_dir: root.join("agent"),
            session_dir: root.join("agent/sessions"),
            session_dir_explicit: false,
            package_dir: root.join("agent/packages"),
            cwd: root.join("work"),
            home: root.to_path_buf(),
        }
    }

    /// The full startup path Pi runs at main.ts:610-630: build the settings manager over the
    /// resolved dirs, then let `getSessionDir()` (settings-manager.ts:670-673) supply the third
    /// `sessionDir` tier. A `"sessionDir"` written into the global `settings.json` must relocate the
    /// session dir AND mark it explicit, or `session_list_layout`/`Cli::to_session_config` cwd-encode
    /// a dir the user asked to be used literally.
    #[test]
    fn settings_json_session_dir_is_wired_into_config_dirs() {
        let root = tempfile::tempdir().unwrap();
        let dirs = dirs_under(root.path());
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        let configured = root.path().join("elsewhere/sessions");
        std::fs::write(
            settings_path(&dirs),
            format!(
                "{{\"sessionDir\": {}}}",
                serde_json::Value::String(configured.to_string_lossy().into_owned())
            ),
        )
        .unwrap();

        let settings = SettingsManager::load(
            file_settings_store(&dirs),
            cyrup_config::Settings::new(),
            false,
        );
        let dirs = apply_settings_session_dir(dirs, &settings);

        assert_eq!(dirs.session_dir, configured);
        assert!(dirs.session_dir_explicit);
    }

    /// No `sessionDir` key ⇒ the `<agent_dir>/sessions` default stands and the layout stays
    /// cwd-encoded (Pi's `getSessionDir()` returns `undefined`, so `createSessionManager` falls
    /// through to `getDefaultSessionDir(cwd)`).
    #[test]
    fn absent_settings_session_dir_leaves_the_default_layout() {
        let root = tempfile::tempdir().unwrap();
        let dirs = dirs_under(root.path());
        std::fs::create_dir_all(&dirs.agent_dir).unwrap();
        std::fs::write(settings_path(&dirs), "{\"theme\":\"dark\"}").unwrap();

        let settings = SettingsManager::load(
            file_settings_store(&dirs),
            cyrup_config::Settings::new(),
            false,
        );
        let default_dir = dirs.session_dir.clone();
        let dirs = apply_settings_session_dir(dirs, &settings);

        assert_eq!(dirs.session_dir, default_dir);
        assert!(!dirs.session_dir_explicit);
    }
}
