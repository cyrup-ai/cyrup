//! Integration test: closing gap R-SA-130 for the 5 registration-surface slash commands that need
//! no real child-OS-subprocess spawn — `/subagents-models`, `/subagents-refresh-provider-models`,
//! `/subagents-generate-profiles`, `/subagents-check-profile`, `/subagents-companions` — proving
//! each now routes through REAL execution (the static seed catalog in `cyrup_provider::catalog`,
//! and `registration::profiles`' real on-disk profile read/write primitives) rather than the
//! "recognized, not yet executing" stub arm `extension.rs::dispatch_slash` used to fall into.
//!
//! Lives in `tests/` (a separate compilation unit from this crate's own `lib.rs`) for the same
//! reason every other fixture-based integration test in this crate does: these tests need to
//! mutate `CYRUP_HOME` (process-global state) via `std::env::set_var`/`remove_var`, which Rust
//! 2024 requires `unsafe` for — a `#[cfg(test)]` module inside `src/` cannot do this because this
//! crate's own `#![forbid(unsafe_code)]` (`src/lib.rs`) applies even to its own test code (a HARD
//! forbid, unlike `deny`, cannot be locally `#[allow(...)]`-ed away).
//!
//! No mocking: every dispatch below drives the REAL `SubagentsExtension::execute_command` (the
//! `cyrup_ext::native::NativeExtension` trait method), which reads the REAL, already-built
//! `cyrup_provider::catalog::seed_catalog()` static catalog and writes/reads REAL files under a
//! temporary `CYRUP_HOME`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use tokio::sync::Mutex;

use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::profiles::{load_profile, NamedProfile};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes every test in this file that mutates `CYRUP_HOME` (process-global state) — mirrors
/// every fixture-based integration test in this crate's identical `ENV_MUTATION_LOCK` convention.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const CYRUP_HOME_ENV_VAR: &str = "CYRUP_HOME";

/// RAII guard installing `CYRUP_HOME` at a temp dir for the life of one test.
struct CyrupHomeGuard {
    _dir: tempfile::TempDir,
}

impl CyrupHomeGuard {
    fn install() -> (Self, PathBuf) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().to_path_buf();
        // SAFETY: scoped, mutex-serialized env mutation for the duration of one test — this file
        // is a separate compilation unit from this crate's own `#![forbid(unsafe_code)]` `lib.rs`.
        unsafe {
            std::env::set_var(CYRUP_HOME_ENV_VAR, &path);
        }
        (Self { _dir: dir }, path)
    }
}

impl Drop for CyrupHomeGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`'s own safety comment.
        unsafe {
            std::env::remove_var(CYRUP_HOME_ENV_VAR);
        }
    }
}

fn command_ctx(cwd: &std::path::Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

// =====================================================================================================
// /subagents-models
// =====================================================================================================

#[tokio::test]
async fn subagents_models_command_reports_the_real_static_seed_catalog() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-models", "", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("subagents-models produces textual output");

    assert!(
        !output.contains("recognized by the subagents extension"),
        "the stub placeholder text must be gone: {output}"
    );
    let catalog = cyrup_provider::catalog::seed_catalog();
    assert!(!catalog.is_empty(), "sanity: the seed catalog is genuinely non-empty");
    let sample = catalog.first().expect("checked non-empty above");
    assert!(
        output.contains(sample.id.as_str()),
        "the rendered report must contain a REAL model id from the static seed catalog: {output}"
    );
}

#[tokio::test]
async fn subagents_models_command_rejects_more_than_one_positional_token() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-models", "one two", &ctx)
        .await
        .expect("execute_command wraps SubagentError as rendered text, never ExtError")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed") || output.contains("Usage:"),
        "a malformed call must surface a real error, not a silent success: {output}"
    );
}

// =====================================================================================================
// /subagents-refresh-provider-models
// =====================================================================================================

#[tokio::test]
async fn subagents_refresh_provider_models_writes_a_real_catalog_cache_file() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let (_home_guard, home) = CyrupHomeGuard::install();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let catalog = cyrup_provider::catalog::seed_catalog();
    let provider = catalog.first().expect("static seed catalog is non-empty").provider.as_str().to_string();

    let output = ext
        .execute_command("subagents-refresh-provider-models", &provider, &ctx)
        .await
        .expect("execute_command does not error")
        .expect("refresh produces textual output");

    assert!(!output.contains("recognized by the subagents extension"), "got: {output}");
    assert!(output.contains("refreshed catalog cache"), "got: {output}");

    let cache_path = home.join(".cyrup").join("subagents").join("provider-catalog-cache.json");
    let contents = tokio::fs::read_to_string(&cache_path)
        .await
        .expect("the refresh command must genuinely write the cache file");
    let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
    assert_eq!(parsed["provider"], serde_json::json!(provider));
    assert!(parsed["modelCount"].as_u64().unwrap_or(0) > 0, "got: {parsed}");
}

#[tokio::test]
async fn subagents_refresh_provider_models_rejects_unsafe_provider_names() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-refresh-provider-models", "../../etc/passwd", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed"),
        "a traversal-shaped provider name must be rejected, not silently accepted: {output}"
    );
}

// =====================================================================================================
// /subagents-generate-profiles
// =====================================================================================================

#[tokio::test]
async fn subagents_generate_profiles_writes_two_real_loadable_profiles() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let (_home_guard, home) = CyrupHomeGuard::install();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let catalog = cyrup_provider::catalog::seed_catalog();
    let provider = catalog.first().expect("static seed catalog is non-empty").provider.as_str().to_string();

    let output = ext
        .execute_command("subagents-generate-profiles", &provider, &ctx)
        .await
        .expect("execute_command does not error")
        .expect("generation produces textual output");

    assert!(output.contains(&format!("{provider}.quota")), "got: {output}");
    assert!(output.contains(&format!("{provider}.quality")), "got: {output}");

    let profiles_dir = home.join(".cyrup").join("subagents").join("profiles");
    let quota: NamedProfile = load_profile(&profiles_dir, &format!("{provider}.quota"))
        .expect("the quota profile must be genuinely loadable from disk");
    let quality: NamedProfile = load_profile(&profiles_dir, &format!("{provider}.quality"))
        .expect("the quality profile must be genuinely loadable from disk");
    assert!(quota.subagents.default_model.is_some(), "got: {quota:?}");
    assert!(quality.subagents.default_model.is_some(), "got: {quality:?}");
}

// =====================================================================================================
// /subagents-check-profile
// =====================================================================================================

#[tokio::test]
async fn subagents_check_profile_cross_references_the_real_static_seed_catalog() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let (_home_guard, home) = CyrupHomeGuard::install();

    let profiles_dir = home.join(".cyrup").join("subagents").join("profiles");
    tokio::fs::create_dir_all(&profiles_dir).await.expect("mkdir profiles dir");
    let catalog = cyrup_provider::catalog::seed_catalog();
    let known_model = catalog.first().expect("non-empty catalog").id.as_str().to_string();

    let profile = NamedProfile {
        subagents: cyrup_ext_subagents::discovery::types::SubagentSettings {
            overrides: std::collections::BTreeMap::new(),
            default_model: Some(known_model.clone()),
            disable_builtins: None,
            disable_thinking: None,
        },
    };
    tokio::fs::write(profiles_dir.join("mixed.json"), serde_json::to_vec_pretty(&profile).expect("serialize"))
        .await
        .expect("write profile");

    let bogus_profile = NamedProfile {
        subagents: cyrup_ext_subagents::discovery::types::SubagentSettings {
            overrides: std::collections::BTreeMap::new(),
            default_model: Some("definitely-not-a-real-model-id".to_string()),
            disable_builtins: None,
            disable_thinking: None,
        },
    };
    tokio::fs::write(
        profiles_dir.join("bogus.json"),
        serde_json::to_vec_pretty(&bogus_profile).expect("serialize"),
    )
    .await
    .expect("write profile");

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let ok_output = ext
        .execute_command("subagents-check-profile", "mixed", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("check produces textual output");
    assert!(ok_output.contains(&known_model), "got: {ok_output}");
    assert!(ok_output.contains("OK"), "got: {ok_output}");

    let bogus_output = ext
        .execute_command("subagents-check-profile", "bogus", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("check produces textual output even for an unresolvable model reference");
    assert!(bogus_output.contains("UNKNOWN"), "got: {bogus_output}");
}

#[tokio::test]
async fn subagents_check_profile_errors_for_a_nonexistent_profile() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let (_home_guard, _home) = CyrupHomeGuard::install();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-check-profile", "does-not-exist", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed"),
        "a nonexistent profile must surface as a real error, never a fabricated OK: {output}"
    );
}

// =====================================================================================================
// /subagents-companions
// =====================================================================================================

#[tokio::test]
async fn subagents_companions_status_reports_no_companions_honestly() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-companions", "", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("status produces textual output");
    assert!(!output.contains("recognized by the subagents extension"), "got: {output}");
    assert!(output.contains("no companion extensions"), "got: {output}");
}

#[tokio::test]
async fn subagents_companions_hide_then_show_round_trips_through_real_disk_state() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let (_home_guard, home) = CyrupHomeGuard::install();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let hide_output = ext
        .execute_command("subagents-companions", "hide pi-intercom workspace", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("hide produces textual output");
    assert!(hide_output.contains("recorded"), "got: {hide_output}");

    let marker = home
        .join(".cyrup")
        .join("subagents")
        .join("companions")
        .join("pi-intercom.workspace.hidden.json");
    assert!(
        tokio::fs::try_exists(&marker).await.unwrap_or(false),
        "the hide command must genuinely write a dismissal marker file to disk"
    );

    let show_output = ext
        .execute_command("subagents-companions", "show pi-intercom", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("show produces textual output");
    assert!(show_output.contains("cleared"), "got: {show_output}");
    assert!(
        !tokio::fs::try_exists(&marker).await.unwrap_or(false),
        "the show command must genuinely remove the dismissal marker file from disk"
    );
}

#[tokio::test]
async fn subagents_companions_rejects_an_unrecognized_package_name() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-companions", "hide not-a-real-package workspace", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed"),
        "an unrecognized companion package must be rejected, not silently accepted: {output}"
    );
}
