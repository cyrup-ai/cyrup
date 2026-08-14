//! ASSEMBLED proof for the settings-declared resource RESOLVE pipeline (gap-analysis 05:
//! CFG-002 / CFG-003 / CFG-004).
//!
//! Pi's `PackageManager.resolve()` (package-manager.ts:885-937) runs THREE steps on every call:
//!   1. `resolvePackageSources` over `projectSettings.packages` + `globalSettings.packages` (:891-901)
//!   2. `resolveLocalEntries` per resource type, per scope — which LOADS the plain (non-pattern)
//!      entries of the `extensions`/`skills`/`prompts`/`themes` arrays (:906-931, :2255-2276)
//!   3. `addAutoDiscoveredResources` — the pattern/filter half (:2278+)
//!
//! and `ModelRuntime` separately loads `<agentDir>/models.json` (model-runtime.ts:137-139,
//! model-config.ts:241-279) so a user-declared provider/model is part of the live registry.
//!
//! cyrup shipped step 3 plus step 2 for skills/prompts/themes only; `packages`, `extensions` and
//! `models.json` had ZERO production readers, so every one of those declarations was inert.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_config::FileSettingsStore;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use crate::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

fn full_theme_json(name: &str) -> String {
    let parts: Vec<String> = cyrup_resources::REQUIRED_COLOR_TOKENS
        .iter()
        .map(|tok| format!("\"{tok}\":\"#000000\""))
        .collect();
    format!(
        "{{\"name\":\"{name}\",\"vars\":{{}},\"colors\":{{{}}}}}",
        parts.join(",")
    )
}

/// A package tree with one skill (`alpha`), one prompt (`greet`), one theme (`midnight`) and one
/// declared extension dir (`extensions/deploy`).
fn make_package_tree(dir: &Path) {
    write(
        &dir.join("skills/alpha/SKILL.md"),
        &skill_md("alpha", "alpha skill"),
    );
    write(&dir.join("prompts/greet.md"), "Hello {{who}}");
    write(
        &dir.join("themes/midnight.json"),
        &full_theme_json("midnight"),
    );
    std::fs::create_dir_all(dir.join("extensions/deploy")).unwrap();
    write(
        &dir.join("cyrup.toml"),
        "[package]\nname = \"pack\"\nversion = \"0.1.0\"\n\n\
         [resources]\nextensions = [\"./extensions/deploy\"]\nskills = [\"./skills\"]\n\
         prompts = [\"./prompts\"]\nthemes = [\"./themes\"]\n",
    );
}

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

impl Fx {
    fn store(&self) -> Arc<dyn cyrup_config::SettingsStore> {
        Arc::new(FileSettingsStore::new(
            self.agent_dir.join("settings.json"),
            self.cwd.join(".cyrup/settings.json"),
        ))
    }

    async fn session(&self) -> crate::AgentSession {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let mut cfg = SessionConfig::new(self.cwd.clone(), self.agent_dir.clone());
        cfg.trust_override = Some(true);
        SessionBuilder::new(faux, cfg)
            .settings_store(self.store())
            .build()
            .await
            .expect("build")
    }
}

/// CFG-003: a package DECLARED in `settings.json` (never installed through `cyrup install`) must
/// contribute its skills/prompts/themes/extensions to the live session — Pi re-resolves
/// `globalSettings.packages` on every `resolve()` call (package-manager.ts:891-901).
#[tokio::test]
async fn settings_declared_package_resources_load_in_assembled_session() {
    let fx = fixture();
    let pkg_src = fx._tmp.path().join("pkgsrc");
    make_package_tree(&pkg_src);

    write(
        &fx.agent_dir.join("settings.json"),
        &format!(
            "{{\"packages\": [{}]}}",
            serde_json::json!(pkg_src.to_string_lossy())
        ),
    );

    let session = fx.session().await;
    let res = session.resources();
    assert!(
        res.skills.contains("alpha"),
        "a settings-declared package's skill must load (found {} skills)",
        res.skills.len()
    );
    assert!(
        res.prompts.contains("greet"),
        "settings-declared package prompt must load"
    );
    assert!(
        res.themes.contains("midnight"),
        "settings-declared package theme must load"
    );
    assert!(
        res.ext_crate_paths.iter().any(|p| p.ends_with("deploy")),
        "settings-declared package's extension dir must be collected: {:?}",
        res.ext_crate_paths
    );
}

/// CFG-004 (extensions half): a plain path in the global `extensions` settings array must be LOADED
/// as an extension root (Pi `resolveLocalEntries` runs over `RESOURCE_TYPES`, which INCLUDES
/// `extensions`, package-manager.ts:194,:905-931).
#[tokio::test]
async fn settings_declared_extension_path_reaches_extension_roots() {
    let fx = fixture();
    std::fs::create_dir_all(fx.agent_dir.join("myexts/deployer")).unwrap();
    write(
        &fx.agent_dir.join("settings.json"),
        "{\"extensions\": [\"myexts/deployer\"]}",
    );

    let session = fx.session().await;
    let res = session.resources();
    assert!(
        res.ext_crate_paths.iter().any(|p| p.ends_with("deployer")),
        "a settings-declared extension path must be collected for the loader: {:?}",
        res.ext_crate_paths
    );
}

/// CFG-004 (skills half) — regression guard: a plain path in the global `skills` settings array is
/// loaded, not merely used as a filter.
#[tokio::test]
async fn settings_declared_skill_path_is_loaded() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("extra/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );
    write(
        &fx.agent_dir.join("settings.json"),
        "{\"skills\": [\"extra\"]}",
    );

    let session = fx.session().await;
    assert!(
        session.resources().skills.contains("beta"),
        "a settings-declared plain skill path must be LOADED"
    );
}

/// CFG-002: a provider + model declared in `<agentDir>/models.json` must be part of the live model
/// registry (Pi `ModelRuntime.create` → `ModelConfig.load(join(getAgentDir(),"models.json"))`,
/// model-runtime.ts:137-139; `composeModelProvider`/`applyModelsJson`, provider-composer.ts:161-199).
#[tokio::test]
async fn models_json_custom_provider_model_is_selectable() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("models.json"),
        r#"{
  // a JSONC comment — Pi strips these before JSON.parse (model-config.ts:257)
  "providers": {
    "mycorp": {
      "baseUrl": "https://llm.mycorp.test/v1",
      "api": "openai-completions",
      "apiKey": "sk-mycorp-test",
      "models": [{ "id": "mycorp-large", "name": "MyCorp Large", "contextWindow": 200000 }]
    }
  }
}"#,
    );

    let session = fx.session().await;
    let catalog = session.available_model_catalog();
    assert!(
        catalog
            .iter()
            .any(|m| m.provider.as_str() == "mycorp" && m.id.as_str() == "mycorp-large"),
        "a models.json-declared custom model must be selectable: {:?}",
        catalog
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .take(8)
            .collect::<Vec<_>>()
    );
}

/// CFG-002 (built-in override half): `baseUrl` declared for an EXISTING built-in provider must
/// rewrite that provider's catalog entries (Pi `applyModelsJson` maps every base model through
/// `config.baseUrl ?? model.baseUrl`, provider-composer.ts:186-190).
#[tokio::test]
async fn models_json_base_url_overrides_a_builtin_provider() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("models.json"),
        r#"{"providers": {"anthropic": {"baseUrl": "https://proxy.internal/v1"}}}"#,
    );

    let session = fx.session().await;
    let catalog = session.full_model_catalog();
    let anthropic: Vec<_> = catalog
        .iter()
        .filter(|m| m.provider.as_str() == "anthropic")
        .collect();
    assert!(
        !anthropic.is_empty(),
        "anthropic models must be in the registry"
    );
    assert!(
        anthropic
            .iter()
            .all(|m| m.base_url == "https://proxy.internal/v1"),
        "models.json baseUrl must rewrite the built-in provider's models: {:?}",
        anthropic
            .iter()
            .map(|m| m.base_url.clone())
            .take(3)
            .collect::<Vec<_>>()
    );
}

/// CFG-002 blocker: the overlay must reach the provider the session is CURRENTLY RUNNING ON — the
/// primary use case ("point my provider at a proxy", "raise contextWindow on the model I'm using").
///
/// Pi's `recomposeProvider` calls `this.models.setProvider(composeModelProvider(...))`
/// (model-runtime.ts:215), REPLACING the provider in the one collection every consumer reads. There
/// is no bypass for the in-use provider. cyrup used to seed its registry from
/// `provider.current().models()` FIRST and then union the composed list with a first-wins dedup, so
/// the current provider's UNCOMPOSED entries shadowed their composed counterparts and `baseUrl` /
/// `compat` / `modelOverrides` were silently no-ops for exactly the provider they were written for.
#[tokio::test]
async fn models_json_overrides_reach_the_provider_the_session_is_running_on() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("models.json"),
        r#"{"providers": {"anthropic": {
             "baseUrl": "https://proxy.internal/v1",
             "modelOverrides": {"claude-opus-4-1": {"contextWindow": 999000}}
           }}}"#,
    );

    // The session runs ON anthropic — not on faux — so the shadowing bug is live.
    let anthropic: Arc<dyn Provider> = Arc::new(cyrup_provider::anthropic_provider());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(anthropic, cfg)
        .settings_store(fx.store())
        .build()
        .await
        .expect("build");

    let catalog = session.full_model_catalog();
    let entries: Vec<_> = catalog
        .iter()
        .filter(|m| m.provider.as_str() == "anthropic")
        .collect();
    assert!(!entries.is_empty(), "anthropic models must be in the registry");
    assert!(
        entries.iter().all(|m| m.base_url == "https://proxy.internal/v1"),
        "the CURRENT provider's own entries must be composed, not shadowed by their uncomposed \
         originals: {:?}",
        entries
            .iter()
            .map(|m| format!("{}={}", m.id, m.base_url))
            .take(4)
            .collect::<Vec<_>>()
    );
    // Exactly one entry per id — composition REPLACES, it does not append a second copy.
    let opus: Vec<_> = entries
        .iter()
        .filter(|m| m.id.as_str() == "claude-opus-4-1")
        .collect();
    assert_eq!(
        opus.len(),
        1,
        "composition replaces the provider's entry rather than appending a shadowed duplicate"
    );
    assert_eq!(
        opus[0].context_window, 999_000,
        "`modelOverrides` must patch the in-use provider's model too"
    );
}

/// Constraint 6 (loud + safe): a MALFORMED `models.json` must not panic, must not take the session
/// down, and must be surfaced as a startup diagnostic — Pi keeps an empty snapshot plus a distinct
/// error string per failure mode (model-config.ts:251/:261/:271).
#[tokio::test]
async fn malformed_models_json_is_reported_not_fatal() {
    let fx = fixture();
    write(&fx.agent_dir.join("models.json"), "{ this is not json ");

    let session = fx.session().await;
    let diags = &session.services().startup_diagnostics.models;
    assert!(
        diags.iter().any(|d| d.contains("models.json")),
        "a malformed models.json must be surfaced as a startup diagnostic: {diags:?}"
    );
    // …and the session still has a working registry.
    assert!(!session.full_model_catalog().is_empty());
}

/// Constraint 6 (loud + safe), packages half: a package DECLARED in settings that is not installed
/// must produce a startup diagnostic — never a silent drop and never a failed build. Pi would
/// install it on demand (`resolvePackageSources`, package-manager.ts:1244-1283); cyrup does no
/// network install during session assembly ([CYRUP-DELTA]) and says so instead.
#[tokio::test]
async fn missing_settings_declared_package_is_reported_not_fatal() {
    let fx = fixture();
    write(
        &fx.agent_dir.join("settings.json"),
        "{\"packages\": [\"./nope-not-here\"]}",
    );

    let session = fx.session().await;
    let diags = &session.services().startup_diagnostics.resources;
    assert!(
        diags.iter().any(|d| d.message.contains("nope-not-here")),
        "a missing settings-declared package must be reported: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
}

/// Constraint 6 (loud + safe), malformed-entry half: ONE unparseable `packages` entry is reported
/// and skipped — it must not discard the sibling entries and must not lose the settings document
/// (1328b0d's load-error latch protects the FILE; this protects the ARRAY).
#[tokio::test]
async fn malformed_package_entry_is_reported_and_the_rest_still_resolve() {
    let fx = fixture();
    let pkg_src = fx._tmp.path().join("pkgsrc");
    make_package_tree(&pkg_src);
    write(
        &fx.agent_dir.join("settings.json"),
        &format!(
            "{{\"defaultModel\": \"faux/faux-1\", \"packages\": [17, {}]}}",
            serde_json::json!(pkg_src.to_string_lossy())
        ),
    );

    let session = fx.session().await;
    let diags = &session.services().startup_diagnostics.resources;
    assert!(
        diags.iter().any(|d| d.message.contains("packages[0]")),
        "the malformed entry must be named: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    // The sibling entry still resolved…
    assert!(
        session.resources().skills.contains("alpha"),
        "a malformed sibling must not disable the whole `packages` array"
    );
    // …and the rest of the settings document survived.
    assert_eq!(
        session
            .services()
            .settings
            .effective()
            .default_model()
            .as_deref(),
        Some("faux/faux-1"),
        "a malformed `packages` entry must not cost the user the settings file"
    );
}
