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
//! [`discover`] runs one pass over all roots and returns a [`ResourceRegistry`] snapshot;
//! [`ResourceHandle`] is the swap primitive offered to embedders that want lock-free reads with an
//! atomic `/reload` swap, though in-tree consumers currently hold an `Arc<ResourceRegistry>`
//! directly.
//!
//! Same-name precedence is a 1:1 port of Pi's `resourcePrecedenceRank`
//! (package-manager.ts:172-188): the lower-ranked candidate wins under first-wins dedup —
//! project-settings < project-auto < user-settings < user-auto < any package < CLI (the explicit
//! `--skill`/`--prompt-template`/`--theme` paths Pi appends after the sorted accumulator,
//! resource-loader.ts:421) — see [`scope::ResourceScope::precedence_rank`].
//!
//! Lints: `[lints] workspace = true` in `Cargo.toml` is the single source of this crate's denies
//! (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`), test code included.
#![forbid(unsafe_code)]

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
    ConfiguredPackage, DisabledSet, InstalledPackage, InstalledPackages, ManifestKind,
    ManifestResources, PackageFilter, PackageStore, ParsedGitUrl, ResolvedManifest,
    ResourceSelector, SECURITY_CAVEAT, SecurityNotice, UpdateReport, UpdateTarget,
    has_unsafe_git_install_part, migrate_legacy_doubled_packages_root, package_identity,
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

/// A lock-free, atomically-swappable holder of a [`ResourceRegistry`], offered to embedders.
///
/// This is the R-09-023 swap primitive: a reader calls [`ResourceHandle::load`] on the hot path
/// while a `/reload` builds a fresh registry off the loop and calls [`ResourceHandle::store`] — a
/// single pointer swap, no torn reads. In-tree consumers currently hold an `Arc<ResourceRegistry>`
/// directly rather than going through this type.
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
