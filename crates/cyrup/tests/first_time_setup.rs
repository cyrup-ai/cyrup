//! First-run setup wizard — parity with Pi v0.83.0.
//!
//! Fixtures are upstream-derived: the four gate cases are 1:1 with
//! `pi/packages/coding-agent/test/first-time-setup.test.ts:36-55`, the fork case with
//! `first-time-setup-fork.test.ts:34-36`, the persisted analytics/tracking-id behaviour with
//! `first-time-setup.test.ts:58-88`, and every wizard string is quoted from
//! `pi/packages/coding-agent/src/modes/interactive/components/first-time-setup.ts` (v0.83.0) with
//! only the product name rebranded, exactly as Pi itself interpolates `APP_NAME` at :60.
//!
//! `run_first_time_setup` itself drives a real `CrosstermBackend` terminal (like `run_trust_prompt`)
//! and cannot be driven from here; everything it is made of — the gate, both step definitions, the
//! confirm-value mapping and the persistence — is.
//!
//! **UW-2 / ADR-0011.** This file used to be the whole story, and it could not see the hole it was
//! written around: the wizard had NO production caller — `main.rs`'s gate evaluated into an empty
//! `if` body — so every assertion below passed while the wizard was unreachable. It is now called at
//! pi's position (`main.rs`, mirroring `main.ts:615-617` @v0.83.0 / `:663-664` @v0.84.1). What a
//! unit test still cannot reach is the wizard APPEARING; that needs a pty, and a live run must show:
//! `CYRUP_EXPERIMENTAL=1` + a fresh agent dir + no `settings.json` → the theme picker, then the
//! analytics question; Light + "Don't share" → `settings.json` with `"theme":"light"`,
//! `"enableAnalytics":false` and NO `trackingId`; the opt-in answer → a non-empty `trackingId`;
//! Escape at either step → no `settings.json` at all; and a relaunch after completing it → no
//! wizard. The `--list-models` conjunct below is the one clause a test here CAN pin.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

use cyrup::startup::{
    APP_NAME, CONFIG_DIR_NAME, DistributionMetadata, FirstTimeSetupResult, PACKAGE_NAME,
    apply_first_time_setup, are_experimental_features_enabled, distribution,
    first_time_setup_analytics_step, first_time_setup_theme_step, is_official_distribution,
    is_official_distribution_of, parse_analytics_choice, parse_theme_choice,
    should_run_first_time_setup, should_run_first_time_setup_with,
};
use cyrup::{Cli, resolve_app_mode};
use cyrup_config::{ConfigDirs, SettingsManager};
use cyrup_session_svc::AppMode;
use cyrup_tui::TerminalTheme;

// ---------------------------------------------------------------------------------------------
// The gate — `shouldRunFirstTimeSetup` (startup-ui.ts:115-133).
// Upstream cases: first-time-setup.test.ts:36-55.
// ---------------------------------------------------------------------------------------------

/// `it("returns true when experimental, default agent dir, and no settings.json")` (:36-38).
#[test]
fn returns_true_when_experimental_default_agent_dir_and_no_settings_json() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");

    assert!(should_run_first_time_setup_with(&settings_path, false, true));
}

/// `it("returns false when experimental features are disabled")` (:41-43).
#[test]
fn returns_false_when_experimental_features_are_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");

    assert!(!should_run_first_time_setup_with(
        &settings_path,
        false,
        false
    ));
}

/// `it("returns false when a custom agent dir is set")` (:46-48) — Pi reads
/// `process.env[ENV_AGENT_DIR]`; cyrup's bin has already folded that into `EnvVars::agent_dir`.
#[test]
fn returns_false_when_a_custom_agent_dir_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");

    assert!(!should_run_first_time_setup_with(&settings_path, true, true));
}

/// `it("returns false when settings.json already exists")` (:52-54).
#[test]
fn returns_false_when_settings_json_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");
    std::fs::write(&settings_path, "{}").unwrap();

    assert!(!should_run_first_time_setup_with(&settings_path, false, true));
}

/// `areExperimentalFeaturesEnabled` (experimental.ts) is `process.env.PI_EXPERIMENTAL === "1"` — a
/// STRICT equality, not the truthy-flag predicate `PI_TELEMETRY`/`PI_OFFLINE` use, so `true`/`yes`
/// must NOT enable it. `CYRUP_EXPERIMENTAL` is the renamed primary and `PI_EXPERIMENTAL` survives as
/// the lower-precedence fallback (`cyrup-config/src/env.rs:68-91` convention).
///
/// One test owns the process env so nothing here races; no other test in this binary reads it.
#[test]
fn experimental_flag_is_strict_one_under_either_name() {
    let _restore = EnvRestore::capture(&["CYRUP_EXPERIMENTAL", "PI_EXPERIMENTAL"]);

    set_env("CYRUP_EXPERIMENTAL", None);
    set_env("PI_EXPERIMENTAL", None);
    assert!(!are_experimental_features_enabled());

    set_env("PI_EXPERIMENTAL", Some("1"));
    assert!(are_experimental_features_enabled());

    set_env("PI_EXPERIMENTAL", None);
    set_env("CYRUP_EXPERIMENTAL", Some("1"));
    assert!(are_experimental_features_enabled());

    for not_one in ["true", "yes", "0", "on", ""] {
        set_env("CYRUP_EXPERIMENTAL", Some(not_one));
        assert!(
            !are_experimental_features_enabled(),
            "{not_one:?} must not enable experimental features"
        );
    }

    // The env-reading wrapper composes the same way as the pure gate.
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.json");
    set_env("CYRUP_EXPERIMENTAL", Some("1"));
    assert!(should_run_first_time_setup(&settings_path, false));
    set_env("CYRUP_EXPERIMENTAL", Some("0"));
    assert!(!should_run_first_time_setup(&settings_path, false));
}

// ---------------------------------------------------------------------------------------------
// Distribution identity — `isOfficialDistribution` (startup-ui.ts:36-42).
// ---------------------------------------------------------------------------------------------

/// MIRROR: the running build IS the official distribution, so the gate above is not vacuous — this
/// is the assertion that keeps `returns_true_when_...` honest.
#[test]
fn the_running_build_is_the_official_distribution() {
    assert!(is_official_distribution());
    assert_eq!(
        distribution(),
        DistributionMetadata {
            package_name: PACKAGE_NAME,
            app_name: APP_NAME,
            config_dir_name: CONFIG_DIR_NAME,
        }
    );
}

/// `it("returns false for a forked package")` (first-time-setup-fork.test.ts:34-36): upstream mocks
/// `PACKAGE_NAME` to `@example/pi-coding-agent` and the gate goes false. Any one of the three
/// identity fields is enough to disqualify a fork.
#[test]
fn a_forked_distribution_is_not_official_and_never_runs_setup() {
    let official = distribution();

    let forked_package = DistributionMetadata {
        package_name: "@example/cyrup-coding-agent",
        ..official
    };
    let forked_app = DistributionMetadata {
        app_name: "notcyrup",
        ..official
    };
    let forked_config_dir = DistributionMetadata {
        config_dir_name: ".notcyrup",
        ..official
    };

    assert!(!is_official_distribution_of(&forked_package));
    assert!(!is_official_distribution_of(&forked_app));
    assert!(!is_official_distribution_of(&forked_config_dir));
    // MIRROR: the unforked triple still passes.
    assert!(is_official_distribution_of(&official));
}

/// The identity is not free-floating: `CONFIG_DIR_NAME` must be the directory the resolved layout
/// actually uses (Pi's constant is the same `pkg.piConfig.configDir` `getAgentDir` joins,
/// config.ts:516-520).
#[test]
fn config_dir_name_is_the_directory_the_layout_uses() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs_under(root.path());

    assert_eq!(
        dirs.project_config_dir().file_name().and_then(|s| s.to_str()),
        Some(CONFIG_DIR_NAME)
    );
    assert_eq!(dirs.project_config_dir(), dirs.cwd.join(".cyrup"));
}

// ---------------------------------------------------------------------------------------------
// The wizard steps — `FirstTimeSetupComponent` (first-time-setup.ts:19-140).
// ---------------------------------------------------------------------------------------------

/// Theme step copy + rows (first-time-setup.ts:19-22, :48-70). Labels and order are Pi's; the
/// welcome line is Pi's `Welcome to ${APP_NAME}, the minimal coding agent.` (:60).
#[test]
fn theme_step_matches_upstream_copy_and_options() {
    let step = first_time_setup_theme_step(TerminalTheme::Dark);

    assert!(
        step.title
            .contains("Welcome to cyrup, the minimal coding agent."),
        "title was: {}",
        step.title
    );
    assert!(step.title.contains("Pick a theme."));
    assert!(step.title.contains("Detected system appearance: dark"));
    // SETUP_LOGO_LINES (:29).
    assert!(step.title.contains("██████\n██  ██\n████  ██\n██    ██"));

    let labels: Vec<&str> = step.rows.iter().map(|(_, label, _)| label.as_str()).collect();
    assert_eq!(labels, vec!["Dark", "Light"]);
    let values: Vec<&str> = step.rows.iter().map(|(value, _, _)| value.as_str()).collect();
    assert_eq!(values, vec!["dark", "light"]);
}

/// `themeIndex = Math.max(0, THEME_OPTIONS.findIndex(o => o.value === detectedTheme))` (:40-43):
/// the detected appearance is preselected, and an unmatched detection falls back to row 0 (Dark).
#[test]
fn theme_step_preselects_the_detected_appearance() {
    assert_eq!(first_time_setup_theme_step(TerminalTheme::Dark).selected, 0);
    assert_eq!(first_time_setup_theme_step(TerminalTheme::Light).selected, 1);
    assert!(
        first_time_setup_theme_step(TerminalTheme::Light)
            .title
            .contains("Detected system appearance: light")
    );
}

/// Analytics step copy + rows (first-time-setup.ts:24-27, :71-83). The blurb is upstream's verbatim,
/// with `Pi` → `cyrup` per the rebrand; `analyticsIndex = 0` (:35) preselects the opt-in row.
#[test]
fn analytics_step_matches_upstream_copy_and_options() {
    let step = first_time_setup_analytics_step();

    assert!(step.title.contains("Opt-in to anonymous usage data sharing?"));
    assert!(
        step.title.contains(
            "Opting in stores a tracking identifier in settings.json and enables anonymous\nusage analytics. This helps us to better debug, reproduce, and resolve issues\nand bugs within cyrup. You can observe what is shared using /privacy and make\nchanges anytime in settings.json."
        ),
        "title was: {}",
        step.title
    );

    let labels: Vec<&str> = step.rows.iter().map(|(_, label, _)| label.as_str()).collect();
    assert_eq!(labels, vec!["Share anonymous usage data", "Don't share"]);
    assert_eq!(step.selected, 0);
}

/// Confirming a row yields that row's option value (first-time-setup.ts:133-137).
#[test]
fn confirm_values_map_back_to_the_upstream_options() {
    let theme_step = first_time_setup_theme_step(TerminalTheme::Dark);
    assert_eq!(
        parse_theme_choice(&theme_step.rows[0].0),
        Some(TerminalTheme::Dark)
    );
    assert_eq!(
        parse_theme_choice(&theme_step.rows[1].0),
        Some(TerminalTheme::Light)
    );
    assert_eq!(parse_theme_choice("mauve"), None);

    let analytics_step = first_time_setup_analytics_step();
    assert_eq!(parse_analytics_choice(&analytics_step.rows[0].0), Some(true));
    assert_eq!(parse_analytics_choice(&analytics_step.rows[1].0), Some(false));
    assert_eq!(parse_analytics_choice("maybe"), None);
}

// ---------------------------------------------------------------------------------------------
// Persistence — `finish(result)` (startup-ui.ts:176-181) over `setTheme` (settings-manager.ts:734)
// and `setEnableAnalytics` (:959-967).
// ---------------------------------------------------------------------------------------------

/// Opting in writes the theme, `enableAnalytics: true` and a freshly minted `trackingId`
/// (`expect(manager.getTrackingId()).toMatch(/^[0-9a-f-]{36}$/)`, first-time-setup.test.ts:63-68).
#[test]
fn submitting_the_wizard_persists_theme_and_analytics_opt_in() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs_under(root.path());
    std::fs::create_dir_all(&dirs.agent_dir).unwrap();
    let mut settings = manager_for(&dirs);

    apply_first_time_setup(
        &mut settings,
        &FirstTimeSetupResult {
            theme: TerminalTheme::Light,
            share_analytics: true,
        },
    )
    .unwrap();

    let written = read_settings(&dirs);
    assert_eq!(written["theme"], serde_json::json!("light"));
    assert_eq!(written["enableAnalytics"], serde_json::json!(true));
    let tracking_id = written["trackingId"].as_str().unwrap();
    assert_eq!(tracking_id.len(), 36);
    assert!(
        tracking_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase() || c == '-'),
        "tracking id was: {tracking_id}"
    );

    // The gate closes for the next run: `settings.json` now exists (startup-ui.ts:132).
    assert!(!should_run_first_time_setup_with(
        &dirs.settings_path(),
        false,
        true
    ));
}

/// Opting out writes the flag but mints NO tracking identifier
/// (`it("does not generate a tracking identifier on opt-out")`, first-time-setup.test.ts:73-79).
#[test]
fn opting_out_persists_the_flag_without_a_tracking_id() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs_under(root.path());
    std::fs::create_dir_all(&dirs.agent_dir).unwrap();
    let mut settings = manager_for(&dirs);

    apply_first_time_setup(
        &mut settings,
        &FirstTimeSetupResult {
            theme: TerminalTheme::Dark,
            share_analytics: false,
        },
    )
    .unwrap();

    let written = read_settings(&dirs);
    assert_eq!(written["theme"], serde_json::json!("dark"));
    assert_eq!(written["enableAnalytics"], serde_json::json!(false));
    assert!(written.get("trackingId").is_none());
}

/// Skipping setup (`onCancel` → `finish(undefined)`, startup-ui.ts:189/197) persists NOTHING, so no
/// `settings.json` is created and the wizard is offered again on the next launch.
#[test]
fn cancelling_the_wizard_writes_nothing_and_leaves_the_gate_open() {
    let root = tempfile::tempdir().unwrap();
    let dirs = dirs_under(root.path());
    std::fs::create_dir_all(&dirs.agent_dir).unwrap();
    // Building the store/manager is the whole of what happens before the user cancels.
    let _settings = manager_for(&dirs);

    assert!(!dirs.settings_path().exists());
    assert!(should_run_first_time_setup_with(
        &dirs.settings_path(),
        false,
        true
    ));
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

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

fn manager_for(dirs: &ConfigDirs) -> SettingsManager {
    SettingsManager::load(cyrup::startup::file_settings_store(dirs), false)
}

fn read_settings(dirs: &ConfigDirs) -> serde_json::Value {
    let raw = std::fs::read_to_string(dirs.settings_path()).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// `std::env::set_var` is `unsafe` in Rust 2024 (it is not thread-safe); this binary mutates the env
/// from exactly one test, and restores it afterwards.
fn set_env(key: &str, value: Option<&str>) {
    match value {
        // SAFETY: single-threaded use — only `experimental_flag_is_strict_one_under_either_name`
        // touches these variables, and no other test in this binary reads them.
        Some(v) => unsafe { std::env::set_var(key, v) },
        None => unsafe { std::env::remove_var(key) },
    }
}

struct EnvRestore(Vec<(String, Option<String>)>);

impl EnvRestore {
    fn capture(keys: &[&str]) -> Self {
        EnvRestore(
            keys.iter()
                .map(|k| ((*k).to_string(), std::env::var(k).ok()))
                .collect(),
        )
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            set_env(key, value.as_deref());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// UW-2 — the call site's `listModels` conjunct (pi main.ts:615, `parsed.listModels === undefined`).
// ---------------------------------------------------------------------------------------------

/// `cyrup --list-models gpt` on a TTY resolves to `AppMode::Interactive` and its listing exit is
/// DOWNSTREAM of the first-run gate, so the gate MUST carry pi's `listModels` conjunct: without it
/// the wizard mounts full-screen on a command pi answers with a model list. This pins the fact that
/// makes the conjunct necessary — the conjunct itself lives at the `main.rs` call site.
#[test]
fn a_list_models_run_is_still_interactive_so_the_gate_needs_the_list_models_conjunct() {
    let cli = Cli {
        list_models: Some("gpt".to_string()),
        ..Default::default()
    };
    assert_eq!(resolve_app_mode(&cli, true, true), AppMode::Interactive);
    assert!(cli.list_models.is_some(), "…and the conjunct sees it");

    // The control: an ordinary interactive run has no `--list-models`, so the conjunct never blocks
    // the wizard on the path it is meant for.
    let plain = Cli::default();
    assert_eq!(resolve_app_mode(&plain, true, true), AppMode::Interactive);
    assert!(plain.list_models.is_none());
}
