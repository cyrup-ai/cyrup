//! ASSEMBLED proof that the bin's `Cli::to_session_config` threads a NON-DEFAULT package dir
//! (`--package-dir` / `CYRUP_PACKAGE_DIR` / `PI_PACKAGE_DIR`) into `SessionConfig.package_dir`, so a
//! package installed into a CUSTOM package dir loads into the assembled session.
//!
//! This closes the documented residual of the install no-op fix (f5eee19): the session-svc/resources
//! half wired `SessionConfig.package_dir` into discovery, but the bin's `to_session_config` left it at
//! the `SessionConfig::new` default (`<agent_dir>/packages`), so `install --package-dir <custom>`
//! wrote to `<custom>` while the session read `<agent_dir>/packages` — a custom-dir install loaded into
//! ZERO sessions.
//!
//! Pi ground truth: Pi resolves ONE `agentDir` (`getAgentDir()`, main.ts:481, honoring
//! `PI_CODING_AGENT_DIR`) and threads that SAME value into both the package manager and the resource
//! loader (`DefaultResourceLoader` constructs `new DefaultPackageManager({ cwd, agentDir, … })`,
//! resource-loader.ts:222-224; the package manager roots user-scope installs at `this.agentDir`,
//! package-manager.ts:899,1650,2002,2029), so an install into a configured dir is ALWAYS visible to
//! the assembled session. cyrup splits the package root into its own `dirs.package_dir` knob
//! (`ConfigDirs::resolve`, env.rs:156-160: CLI > `CYRUP_PACKAGE_DIR`/`PI_PACKAGE_DIR` > default), which
//! `install` honors (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`, subcommands.rs:396); this
//! test proves the session path now honors the SAME resolved value.
//!
//! Drives the REAL assembled product end-to-end: real `ConfigDirs::resolve` (the exact call the bin
//! makes at main.rs:157) → the bin's `Cli::to_session_config` → `SessionBuilder` → `AgentSession` over
//! a real `FauxProvider` and real cyrup-resources discovery. On PRE-FIX code both the config assertion
//! and the skill-load assertion fail; POST-FIX both pass.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use crate::{AppMode, Cli};
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_resources::{InstallScope, PackageManager, PackageSource, PackageStore};
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::SessionBuilder;
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

/// A package tree with one skill (`alpha`) declared under `cyrup.toml` — the minimal faithful package.
fn make_package_tree(dir: &Path) {
    write(&dir.join("skills/alpha/SKILL.md"), &skill_md("alpha", "alpha skill"));
    write(
        &dir.join("cyrup.toml"),
        "[package]\nname = \"pack\"\nversion = \"0.1.0\"\n\n\
         [resources]\nskills = [\"./skills\"]\n",
    );
}

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    /// A CUSTOM package dir DELIBERATELY distinct from the `<agent_dir>/packages` default, standing in
    /// for `--package-dir <custom>` / `CYRUP_PACKAGE_DIR=<custom>`.
    custom_package_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let custom_package_dir = tmp.path().join("custom-packages");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&custom_package_dir).unwrap();
    Fx { _tmp: tmp, cwd, agent_dir, custom_package_dir }
}

/// THE headline proof: a GLOBAL-scope package recorded by the real `install` path into a CUSTOM
/// package dir surfaces its skill in an assembled `AgentSession` when the session is started with that
/// same custom `--package-dir`. On PRE-FIX code `to_session_config` left `SessionConfig.package_dir` at
/// the `<agent_dir>/packages` default, so the builder read the wrong (empty) registry and the skill was
/// absent.
#[tokio::test]
async fn custom_package_dir_install_loads_into_assembled_session() {
    let fx = fixture();

    // A package source tree living OUTSIDE every config dir (a local Path install — the simplest
    // faithful `install <path>`; `installed_dir` returns the path directly for Path).
    let pkg_src = fx._tmp.path().join("pkgsrc");
    make_package_tree(&pkg_src);

    // Seed the installed registry into the CUSTOM package dir EXACTLY the way the `install` subcommand
    // does (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`, subcommands.rs:396; `.install(…)`
    // :410) — only here `dirs.package_dir` is the custom dir, not the default.
    let store = PackageStore::new(fx.custom_package_dir.clone(), Some(fx.cwd.clone()));
    let mgr = PackageManager::new(store);
    mgr.install(
        PackageSource::Path { path: pkg_src.clone() },
        InstallScope::Global,
        true,
        CancelToken::new(),
    )
    .await
    .expect("install records the package to <custom>/packages.json");

    // Resolve dirs through the EXACT path the bin uses (main.rs:157 `ConfigDirs::resolve(&overrides,
    // &env)`). `CliConfigOverrides.package_dir` is the `--package-dir` slot; `EnvVars.package_dir` is
    // `CYRUP_PACKAGE_DIR`/`PI_PACKAGE_DIR` — both funnel into `dirs.package_dir`.
    let overrides = CliConfigOverrides {
        agent_dir: Some(fx.agent_dir.clone()),
        cwd: Some(fx.cwd.clone()),
        package_dir: Some(fx.custom_package_dir.clone()),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, &EnvVars::default()).unwrap();
    // Sanity: the resolved package dir is the CUSTOM dir and is genuinely distinct from the default the
    // pre-fix code fell back to — otherwise the test could pass for the wrong reason.
    assert_eq!(dirs.package_dir, fx.custom_package_dir);
    assert_ne!(
        dirs.package_dir,
        dirs.agent_dir.join("packages"),
        "the custom package dir must differ from the <agent_dir>/packages default"
    );

    // `--approve` so global-scope discovery loads without an interactive trust prompt (drives the real
    // `trust_override()` → `to_session_config` path).
    let cli = Cli::try_parse_from(["cyrup", "--approve"]).expect("parse cli");
    let config = cli.to_session_config(&dirs, AppMode::Print);

    // The property under fix: `to_session_config` threads the resolved package dir. PRE-FIX this is
    // `<agent_dir>/packages` (the `SessionConfig::new` default); POST-FIX it is the custom dir.
    assert_eq!(
        config.package_dir, fx.custom_package_dir,
        "to_session_config must thread the resolved --package-dir/CYRUP_PACKAGE_DIR into \
         SessionConfig.package_dir (was left at the <agent_dir>/packages default)"
    );

    // Assemble the REAL session and prove the custom-dir install loads.
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, config).build().await.expect("build");

    let res = session.resources();
    assert!(
        res.skills.contains("alpha"),
        "a package installed into the CUSTOM package dir must load into the live session when the \
         session is started with that --package-dir (found {} skills)",
        res.skills.len()
    );

    // …and the skill is USABLE as a `/skill:alpha` slash command in the assembled session.
    let catalog = session.slash_command_catalog();
    assert!(
        catalog
            .iter()
            .any(|c| c.get("name").and_then(serde_json::Value::as_str) == Some("skill:alpha")),
        "the custom-dir package skill must be a usable /skill:alpha command: {catalog:?}"
    );
}
