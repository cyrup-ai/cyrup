//! Startup detection: distribution-identity + experimental-features gating, the first-run setup
//! wizard, and the file-backed settings store (Pi `cli/startup-ui.ts` + `core/experimental.ts` +
//! `modes/interactive/components/first-time-setup.ts`).
//!
//! `shouldRunFirstTimeSetup` (startup-ui.ts:115-133) fires the first-run wizard only when ALL hold:
//! official distribution, `*_EXPERIMENTAL=1`, the default agent dir, and no existing `settings.json`.
//! `isOfficialDistribution` (startup-ui.ts:36-42) compares the RUNNING build's
//! `(packageName, appName, configDirName)` triple — read at runtime from `package.json` /
//! `pkg.piConfig` (config.ts:488-491) — against the constants naming the distribution this source
//! tree *is*. Under the rebrand those constants name the official **cyrup** build, so the predicate
//! answers for cyrup's own identity: it is true for this distribution and false for a fork that
//! reships it under another package/app/config-dir name (Pi's `first-time-setup-fork.test.ts`, which
//! mocks `PACKAGE_NAME` to `@example/pi-coding-agent`).
//!
//! Mechanism note: Rust has no runtime `package.json`, so the triple is read from Cargo's
//! compile-time `CARGO_PKG_NAME` plus the app/config-dir names this build actually uses
//! (`.cyrup`, asserted against `ConfigDirs` in the tests below). A fork changes them in `Cargo.toml`
//! exactly as a fork changes `package.json`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_config::{
    ConfigDirs, ConfigError, FileSettingsStore, SettingsManager, SettingsScope, SettingsStore,
};
use cyrup_tui::{
    ListSelector, SelectKeymap, SelectorOutcome, TerminalTheme, UiTheme, run_startup_selector,
};

/// The official **cyrup** distribution identity — the rebrand's counterpart to Pi's `OFFICIAL_*`
/// triple (startup-ui.ts:26-28, `@earendil-works/pi-coding-agent` / `pi` / `.pi`).
const OFFICIAL_PACKAGE_NAME: &str = "cyrup";
const OFFICIAL_APP_NAME: &str = "cyrup";
const OFFICIAL_CONFIG_DIR_NAME: &str = ".cyrup";

/// This build's package name (Pi `PACKAGE_NAME`, config.ts:488 — `pkg.name`). Cargo's crate name is
/// the direct analog: a fork that reships cyrup renames it here.
pub const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");
/// This build's app name (Pi `APP_NAME`, config.ts:489 — `pkg.piConfig.name || "pi"`).
pub const APP_NAME: &str = "cyrup";
/// This build's config-dir name (Pi `CONFIG_DIR_NAME`, config.ts:491 — `pkg.piConfig.configDir`).
/// Must stay in step with `ConfigDirs` (`~/<CONFIG_DIR_NAME>/agent`, `<cwd>/<CONFIG_DIR_NAME>`).
pub const CONFIG_DIR_NAME: &str = ".cyrup";

/// Port of `DistributionMetadata` (startup-ui.ts:30-34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistributionMetadata<'a> {
    pub package_name: &'a str,
    pub app_name: &'a str,
    pub config_dir_name: &'a str,
}

/// The metadata of the build that is running (Pi passes `{PACKAGE_NAME, APP_NAME, CONFIG_DIR_NAME}`
/// from `config.ts` at startup-ui.ts:117-121).
pub fn distribution() -> DistributionMetadata<'static> {
    DistributionMetadata {
        package_name: PACKAGE_NAME,
        app_name: APP_NAME,
        config_dir_name: CONFIG_DIR_NAME,
    }
}

/// Port of `isOfficialDistribution` (startup-ui.ts:36-42): all three identity fields must match the
/// official triple. A fork that changes any one of them is not the official distribution.
pub fn is_official_distribution_of(meta: &DistributionMetadata<'_>) -> bool {
    meta.package_name == OFFICIAL_PACKAGE_NAME
        && meta.app_name == OFFICIAL_APP_NAME
        && meta.config_dir_name == OFFICIAL_CONFIG_DIR_NAME
}

/// [`is_official_distribution_of`] applied to the running build (startup-ui.ts:116-122).
pub fn is_official_distribution() -> bool {
    is_official_distribution_of(&distribution())
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

/// Port of `shouldRunFirstTimeSetup` (startup-ui.ts:115-133): official distribution AND experimental
/// features AND default agent dir AND no existing `settings.json`. `agent_dir_overridden` reflects
/// whether `$CYRUP_AGENT_DIR` / `$PI_CODING_AGENT_DIR` was set (Pi's `process.env[ENV_AGENT_DIR]`,
/// startup-ui.ts:128); it is a parameter because the bin has already resolved the env into
/// [`cyrup_config::EnvVars`] by this point.
pub fn should_run_first_time_setup(settings_path: &Path, agent_dir_overridden: bool) -> bool {
    should_run_first_time_setup_with(
        settings_path,
        agent_dir_overridden,
        are_experimental_features_enabled(),
    )
}

/// [`should_run_first_time_setup`] with the experimental-flag read supplied by the caller — the same
/// conditions in the same order (startup-ui.ts:116-132), with no process-env access so the gate can
/// be exercised deterministically.
pub fn should_run_first_time_setup_with(
    settings_path: &Path,
    agent_dir_overridden: bool,
    experimental: bool,
) -> bool {
    if !is_official_distribution() {
        return false;
    }
    if !experimental {
        return false;
    }
    if agent_dir_overridden {
        return false;
    }
    !settings_path.exists()
}

// ---------------------------------------------------------------------------------------------
// The first-run wizard — Pi `showFirstTimeSetup` (startup-ui.ts:166-207) over
// `FirstTimeSetupComponent` (modes/interactive/components/first-time-setup.ts).
// ---------------------------------------------------------------------------------------------

/// Port of `FirstTimeSetupResult` (first-time-setup.ts:7-10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstTimeSetupResult {
    pub theme: TerminalTheme,
    pub share_analytics: bool,
}

/// `SETUP_LOGO_LINES` (first-time-setup.ts:29), re-drawn on every step because Pi's `update()`
/// rebuilds the whole dialog (first-time-setup.ts:48-55).
const SETUP_LOGO_LINES: [&str; 4] = ["██████", "██  ██", "████  ██", "██    ██"];

/// `THEME_OPTIONS` (first-time-setup.ts:19-22), in Pi's order — Dark first.
const THEME_OPTIONS: [(TerminalTheme, &str); 2] = [
    (TerminalTheme::Dark, "Dark"),
    (TerminalTheme::Light, "Light"),
];

/// `ANALYTICS_OPTIONS` (first-time-setup.ts:24-27), in Pi's order — opt-in first.
const ANALYTICS_OPTIONS: [(bool, &str); 2] =
    [(true, "Share anonymous usage data"), (false, "Don't share")];

/// The analytics blurb (first-time-setup.ts:72-77), verbatim apart from the product name, which the
/// rebrand carries the same way Pi interpolates `APP_NAME` into the welcome line (:60).
const ANALYTICS_BLURB: &str = "Opting in stores a tracking identifier in settings.json and enables anonymous\nusage analytics. This helps us to better debug, reproduce, and resolve issues\nand bugs within cyrup. You can observe what is shared using /privacy and make\nchanges anytime in settings.json.";

/// One step of the wizard, as the data a selector is mounted from: the header text Pi's `update()`
/// stacks above the option list, the `(value, label, description)` rows, and the preselected row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstTimeSetupStep {
    pub title: String,
    pub rows: Vec<(String, String, Option<String>)>,
    pub selected: usize,
}

/// The dialog header Pi rebuilds for every step (first-time-setup.ts:48-60): logo, then
/// `Welcome to ${APP_NAME}, the minimal coding agent.`
fn setup_header() -> String {
    format!(
        "{}\n\nWelcome to {APP_NAME}, the minimal coding agent.\n",
        SETUP_LOGO_LINES.join("\n")
    )
}

/// Step 1 — theme choice (first-time-setup.ts:62-70). The preselected row is the detected terminal
/// appearance (`Math.max(0, findIndex(...))`, :40-43), i.e. Dark when detection finds nothing.
pub fn first_time_setup_theme_step(detected: TerminalTheme) -> FirstTimeSetupStep {
    let selected = THEME_OPTIONS
        .iter()
        .position(|(value, _)| *value == detected)
        .unwrap_or(0);
    FirstTimeSetupStep {
        title: format!(
            "{}\nPick a theme.\nDetected system appearance: {}",
            setup_header(),
            detected.theme_name()
        ),
        rows: THEME_OPTIONS
            .iter()
            .map(|(value, label)| ((*value).theme_name().to_string(), (*label).to_string(), None))
            .collect(),
        selected,
    }
}

/// Step 2 — analytics opt-in (first-time-setup.ts:71-83). Pi starts this step on
/// `analyticsIndex = 0` (:35), i.e. highlighting "Share anonymous usage data".
pub fn first_time_setup_analytics_step() -> FirstTimeSetupStep {
    FirstTimeSetupStep {
        title: format!(
            "{}\nOpt-in to anonymous usage data sharing?\n\n{ANALYTICS_BLURB}",
            setup_header()
        ),
        rows: ANALYTICS_OPTIONS
            .iter()
            .map(|(value, label)| {
                (
                    if *value { "yes" } else { "no" }.to_string(),
                    (*label).to_string(),
                    None,
                )
            })
            .collect(),
        selected: 0,
    }
}

/// Map a confirmed theme row back to its option value (first-time-setup.ts:134).
pub fn parse_theme_choice(value: &str) -> Option<TerminalTheme> {
    THEME_OPTIONS
        .iter()
        .find(|(option, _)| option.theme_name() == value)
        .map(|(option, _)| *option)
}

/// Map a confirmed analytics row back to its option value (first-time-setup.ts:135).
pub fn parse_analytics_choice(value: &str) -> Option<bool> {
    match value {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// Persist a completed wizard (Pi `finish(result)`, startup-ui.ts:176-181):
/// `setTheme` (settings-manager.ts:734-738) then `setEnableAnalytics` (:959-967, which mints a
/// `trackingId` on first opt-in) into the GLOBAL scope. Pi's trailing `await settingsManager.flush()`
/// drains its async write queue; cyrup's setters write through the locked store synchronously, so
/// both keys are already on disk when this returns.
///
/// Cancelling the wizard persists nothing (Pi's `finish(undefined)` skips the whole block), so this
/// is only ever called with a submitted result.
pub async fn apply_first_time_setup(
    settings: &mut SettingsManager,
    result: &FirstTimeSetupResult,
) -> Result<(), ConfigError> {
    settings
        .set(SettingsScope::Global, "theme", result.theme.theme_name())
        .await?;
    settings.set_enable_analytics(result.share_analytics).await?;
    Ok(())
}

/// Run the first-run wizard and persist the outcome (Pi `showFirstTimeSetup`, startup-ui.ts:166-207).
/// Returns the submitted result, or `None` when the user skipped setup (`onCancel` → `finish(undefined)`,
/// :189/:197) at either step.
///
/// Mechanism note: Pi mounts ONE `FirstTimeSetupComponent` that switches an internal `step` field
/// (first-time-setup.ts:126-140); cyrup's pre-launch surface is [`run_startup_selector`], which mounts
/// exactly one [`cyrup_tui::Selector`] per call, so the two steps are two sequential mounts. The
/// observable flow is the same — theme, then analytics, confirm advances, cancel at either step
/// abandons setup without writing settings. The one behaviour this layer cannot carry is Pi's
/// `onThemePreview` live recolour (:184-187): `run_startup_selector` treats
/// [`SelectorOutcome::Preview`] as a no-op, so the chosen theme applies on the next render rather
/// than while navigating.
pub async fn run_first_time_setup(
    ui: &UiTheme,
    settings: &mut SettingsManager,
    detected: TerminalTheme,
) -> anyhow::Result<Option<FirstTimeSetupResult>> {
    let keymap = SelectKeymap::default();

    let step = first_time_setup_theme_step(detected);
    let mut selector = ListSelector::prompt(step.title, step.rows, step.selected);
    let theme = match run_startup_selector(ui, &keymap, &mut selector, async |_| {}).await? {
        SelectorOutcome::Confirm(value) => match parse_theme_choice(&value) {
            Some(theme) => theme,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };

    let step = first_time_setup_analytics_step();
    let mut selector = ListSelector::prompt(step.title, step.rows, step.selected);
    let share_analytics =
        match run_startup_selector(ui, &keymap, &mut selector, async |_| {}).await? {
            SelectorOutcome::Confirm(value) => match parse_analytics_choice(&value) {
                Some(share) => share,
                None => return Ok(None),
            },
            _ => return Ok(None),
        };

    let result = FirstTimeSetupResult {
        theme,
        share_analytics,
    };
    apply_first_time_setup(settings, &result)
        .await
        .map_err(|e| anyhow::anyhow!("saving first-time setup: {e}"))?;
    Ok(Some(result))
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

    /// This build IS the official cyrup distribution, so the identity check passes and the wizard
    /// can fire (Pi's `isOfficialDistribution` is true for the official `pi` build,
    /// startup-ui.ts:36-42).
    #[test]
    fn this_build_is_the_official_distribution() {
        assert!(is_official_distribution());
        assert_eq!(distribution().package_name, "cyrup");
        assert_eq!(distribution().app_name, "cyrup");
        assert_eq!(distribution().config_dir_name, ".cyrup");
    }

    /// `CONFIG_DIR_NAME` must name the directory `ConfigDirs` actually uses, the way Pi's constant
    /// is the same `pkg.piConfig.configDir` that `getAgentDir` joins (config.ts:516-520).
    #[test]
    fn config_dir_name_matches_the_resolved_layout() {
        let root = tempfile::tempdir().unwrap();
        let dirs = dirs_under(root.path());
        assert_eq!(
            dirs.project_config_dir().file_name().and_then(|s| s.to_str()),
            Some(CONFIG_DIR_NAME)
        );
    }

    #[test]
    fn first_time_setup_runs_on_a_genuine_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("settings.json");
        // Experimental on, default agent dir, no settings file (Pi first-time-setup.test.ts:36-38).
        assert!(should_run_first_time_setup_with(&missing, false, true));
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

        let settings = SettingsManager::load(file_settings_store(&dirs), false);
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

        let settings = SettingsManager::load(file_settings_store(&dirs), false);
        let default_dir = dirs.session_dir.clone();
        let dirs = apply_settings_session_dir(dirs, &settings);

        assert_eq!(dirs.session_dir, default_dir);
        assert!(!dirs.session_dir_explicit);
    }
}
