//! cyrup-resources — skills, prompt templates, themes, packages (arch-09; conformance: func-09).
//!
//! The four non-executable resource kinds that shape agent behavior without being code
//! extensions, plus the package model that bundles them:
//!
//! - **Skills** ([`Skill`]) — Agent Skills standard `SKILL.md` directories, lazy-bodied.
//! - **Prompt templates** ([`PromptTemplate`]) — markdown expanded by `/name args` with shell-style
//!   positional substitution (`$1 $@ $ARGUMENTS ${N:-default} ${@:N:L}`).
//! - **Themes** ([`Theme`]) — JSON TUI color schemes, hot-reloadable ([`ThemeWatcher`]).
//! - **Packages** ([`PackageManager`]) — manifest-declared bundles, git/local-path installs.
//!
//! [`discover`] runs one pass over all roots and returns a [`ResourceRegistry`] snapshot held
//! behind [`ResourceHandle`] (lock-free reads, atomic `/reload` swap). Same-name precedence is a
//! 1:1 port of Pi's `resourcePrecedenceRank` (package-manager.ts:172-188): the lower-ranked
//! candidate wins under first-wins dedup — project-settings < project-auto < user-settings <
//! user-auto < any package < CLI (the explicit `--skill`/`--prompt-template`/`--theme` paths Pi
//! appends after the sorted accumulator, resource-loader.ts:421) — see
//! [`scope::ResourceScope::precedence_rank`].
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod discovery;
pub mod error;
pub mod key;
pub mod package;
pub mod prompt;
pub mod scope;
pub mod skill;
pub mod theme;

use std::sync::Arc;

pub use cyrup_core::PackageId;

pub use discovery::{
    CliResourcePaths, DiscoveredPaths, DiscoveryConfig, DiscoveryReport, Named, ResourceOverrides,
    ResourceRegistry, ResourceSet, discover, discover_append_system_prompt_file,
    discover_system_prompt_file, scope_base_dir,
};
pub use error::{
    Collision, DiagnosticType, ResourceDiagnostic, ResourceError, ResourceKind, ResourceWarning,
};
pub use key::ResourceKey;
pub use package::install::{PackageManager, security_notice_for};
pub use package::source::{PackageSource, PinRef};
pub use package::{
    ConfiguredPackage, DisabledSet, InstalledPackage, InstalledPackages, ManifestResources,
    PackageFilter, PackageStore, ParsedGitUrl, ResolvedManifest, ResourceSelector, SECURITY_CAVEAT,
    SecurityNotice, UpdateReport, UpdateTarget, has_unsafe_git_install_part, package_identity,
    parse_git_url, resolve_manifest,
};
pub use prompt::{PromptTemplate, expand_prompt_template, parse_command_args, substitute_args};
pub use scope::{InstallScope, ResourceOrigin, ResourceScope};
pub use skill::{
    MAX_DESCRIPTION_LENGTH, MAX_NAME_LENGTH, Skill, SkillFrontMatter, SkillPointer,
    validate_description, validate_name,
};
pub use theme::{
    BUILTIN_DARK_JSON, BUILTIN_LIGHT_JSON, ColorSpec, ExportColors, REQUIRED_COLOR_TOKENS,
    ResolvedTheme, Theme, ThemeData, ThemeWatcher, builtin_themes,
};

/// The lock-free, atomically-swappable holder of the active [`ResourceRegistry`].
///
/// Readers call [`ResourceHandle::load`] on the hot path; `/reload` builds a fresh registry off
/// the loop and calls [`ResourceHandle::store`] (R-09-023) — a single pointer swap, no torn reads.
pub struct ResourceHandle(arc_swap::ArcSwap<ResourceRegistry>);

impl ResourceHandle {
    pub fn new(registry: ResourceRegistry) -> Self {
        Self(arc_swap::ArcSwap::from_pointee(registry))
    }

    /// Lock-free snapshot read.
    pub fn load(&self) -> arc_swap::Guard<Arc<ResourceRegistry>> {
        self.0.load()
    }

    /// Atomically install a freshly discovered registry (`/reload`).
    pub fn store(&self, next: Arc<ResourceRegistry>) {
        self.0.store(next);
    }
}

impl Default for ResourceHandle {
    fn default() -> Self {
        Self::new(ResourceRegistry::default())
    }
}

#[cfg(test)]
mod tests;
