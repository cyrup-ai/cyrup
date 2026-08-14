//! ASSEMBLED proof for the `install`→live-session no-op fix (gap-analysis 07 #1/#2, 13-cyrup C1).
//!
//! `cyrup install <pkg>` durably records a package to `packages.json` via `PackageStore`, but the
//! session builder never read that registry back into `DiscoveryConfig.installed`, so an installed
//! package's skills/prompts/themes/extensions loaded into ZERO live sessions (a fully-implemented,
//! fully-tested, fully-advertised feature that never fired in a real run). These tests seed the
//! installed registry EXACTLY the way the real `install` subcommand does
//! (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))` → `PackageManager::install`,
//! subcommands.rs:396,410), assemble a real `AgentSession` via `SessionBuilder`, and assert the
//! package's resources are present + usable.
//!
//! Pi ground truth: `PackageManager.resolve()` re-reads `projectSettings.packages`/
//! `globalSettings.packages` on every call (package-manager.ts:880-897), so an installed package's
//! resources are live in every session; `resolvedPaths.extensions` (package tier) is merged into the
//! loaded extension set (resource-loader.ts:379,403-407).
//!
//! MIGRATED from `crates/cyrup-session-svc/tests/install_noop.rs`, which split at its module
//! boundary (docs/TEST-ARCHITECTURE.md §3.1, `:346`). This half asserts in-process over a real
//! `SessionBuilder` and touches no wasm guest and no child process, so §9.1 puts it here. The
//! `mod wasm_ext` half DOES drive a real `wasm32-wasip2` component and moved to
//! `crates/cyrup-it/tests/session_svc/install_noop.rs`; the small helpers the two halves shared
//! (`write`, `Fx`, `fixture`) are duplicated across the split rather than exported.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_resources::{InstallScope, PackageManager, PackageSource, PackageStore};
use tempfile::TempDir;

use crate::{SessionBuilder, SessionConfig};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

/// A schema-complete theme JSON (all 51 required color tokens) so it passes theme validation.
fn full_theme_json(name: &str) -> String {
    let parts: Vec<String> = cyrup_resources::REQUIRED_COLOR_TOKENS
        .iter()
        .map(|tok| format!("\"{tok}\":\"#000000\""))
        .collect();
    format!("{{\"name\":\"{name}\",\"vars\":{{}},\"colors\":{{{}}}}}", parts.join(","))
}

/// A package tree with one skill (`alpha`), one prompt (`greet`), one theme (`midnight`), and one
/// declared extension dir (`extensions/deploy`) — mirrors cyrup-resources' own `make_package_tree`.
fn make_package_tree(dir: &Path) {
    write(&dir.join("skills/alpha/SKILL.md"), &skill_md("alpha", "alpha skill"));
    write(&dir.join("prompts/greet.md"), "Hello {{who}}");
    write(&dir.join("themes/midnight.json"), &full_theme_json("midnight"));
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
    /// The bin's default `dirs.package_dir` = `<agent_dir>/packages` (env.rs:156-160).
    package_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let package_dir = agent_dir.join("packages");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx { _tmp: tmp, cwd, agent_dir, package_dir }
}

/// THE headline proof: a GLOBAL-scope package recorded by the real `install` path surfaces its
/// skill/prompt/theme in an assembled `AgentSession`, and its declared extension dir is collected
/// for the extension loader. On the pre-fix builder `DiscoveryConfig.installed` was always empty, so
/// EVERY assertion below failed (0% functional).
#[tokio::test]
async fn installed_global_package_resources_load_in_assembled_session() {
    let fx = fixture();
    // A package source tree living OUTSIDE the agent/package dirs (a local Path install, the
    // simplest faithful `install <path>`; `installed_dir` returns the path directly for Path).
    let pkg_src = fx._tmp.path().join("pkgsrc");
    make_package_tree(&pkg_src);

    // Seed the installed registry EXACTLY the way the `install` subcommand does
    // (subcommands.rs:396 `PackageStore::new(dirs.package_dir, Some(dirs.cwd))`; :410 install).
    let store = PackageStore::new(fx.package_dir.clone(), Some(fx.cwd.clone()));
    let mgr = PackageManager::new(store);
    mgr.install(
        PackageSource::Path { path: pkg_src.clone() },
        InstallScope::Global,
        true,
        CancelToken::new(),
    )
    .await
    .expect("install records the package to packages.json");

    // Assemble a REAL session over the same cwd/agent_dir (default `package_dir` = agent_dir/packages
    // matches the store above — the very wiring under test).
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let res = session.resources();
    assert!(
        res.skills.contains("alpha"),
        "installed package skill must load into the live session (found {} skills)",
        res.skills.len()
    );
    assert!(res.prompts.contains("greet"), "installed package prompt must load into the live session");
    assert!(res.themes.contains("midnight"), "installed package theme must load into the live session");
    assert!(
        res.ext_crate_paths.iter().any(|p| p.ends_with("deploy")),
        "installed package's declared extension dir must be collected for the loader: {:?}",
        res.ext_crate_paths
    );

    // …and the skill is USABLE as a `/skill:alpha` slash command in the assembled session.
    let catalog = session.slash_command_catalog();
    assert!(
        catalog
            .iter()
            .any(|c| c.get("name").and_then(serde_json::Value::as_str) == Some("skill:alpha")),
        "the installed package skill must be a usable /skill:alpha command: {catalog:?}"
    );
}
