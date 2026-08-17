//! Bundled-resource path resolution (port doc §3.4.1 Fact 7).
//!
//! `env!("CARGO_MANIFEST_DIR")` is a build-machine source path; a `cargo install`ed binary may
//! have no source tree at that path. `CYRUP_FLUX_RESOURCES_DIR` lets a packaged build point at
//! its vendored location, falling back to `CARGO_MANIFEST_DIR/resources` (correct for every
//! from-source build) — the same shape as
//! `cyrup-ext-subagents`'s `CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR`.

use std::path::PathBuf;

/// Environment override for the bundled-resources root: a packaged/installed binary that does
/// not ship an intact `CARGO_MANIFEST_DIR`-relative source tree points this at the fixed
/// install-time location `prompts/` and `skills/` were vendored into.
const BUNDLED_RESOURCES_DIR_ENV_VAR: &str = "CYRUP_FLUX_RESOURCES_DIR";

/// The bundled-resources root: `resources/` under this crate's manifest dir, or the
/// [`BUNDLED_RESOURCES_DIR_ENV_VAR`] override.
#[must_use]
pub fn bundled_dir() -> PathBuf {
    std::env::var_os(BUNDLED_RESOURCES_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// The prompt ROOT contributed to `promptPaths` — a directory, never a file (FLUX_01 Fact 4).
#[must_use]
pub fn bundled_prompts_dir() -> PathBuf {
    bundled_dir().join("prompts")
}

/// The bundled skill entry point (FLUX_06 adds the file and the `skillPaths` contribution).
#[must_use]
pub fn bundled_skill_md() -> PathBuf {
    bundled_dir().join("skills").join("flux").join("SKILL.md")
}
