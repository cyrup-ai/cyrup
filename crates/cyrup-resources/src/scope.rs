//! Scope, origin, and install-scope (arch-09 §3.1).
//!
//! Same-name precedence is governed by [`ResourceScope::precedence_rank`], a 1:1 port of Pi's
//! `resourcePrecedenceRank` (package-manager.ts:172-188): a **lower** rank wins under first-wins
//! same-name dedup. The declaration order of [`ResourceScope`] mirrors that ranking (highest
//! precedence first) for readability, but `precedence_rank` — not the derived `Ord` — is the
//! authoritative precedence (it assigns equal ranks where Pi does, e.g. both package tiers share
//! one rank). The explicit-CLI tier ranks **below** every package: Pi never feeds the `--skill` /
//! `--prompt-template` / `--theme` paths (`additionalSkillPaths`) through `resourcePrecedenceRank`
//! — it appends them after the entire sorted accumulator via
//! `mergePaths([...cliEnabledSkills, ...enabledSkills], additionalSkillPaths)`
//! (resource-loader.ts:421/436/455), so under first-wins a same-name package (already in the sorted
//! `enabledSkills`, rank `4`) wins and the CLI path loses.

use std::path::{Path, PathBuf};

use cyrup_core::{ExtensionId, PackageId};

/// Where a resource came from. Precedence is via [`ResourceScope::precedence_rank`] (Pi
/// `resourcePrecedenceRank`, package-manager.ts:172-188), **not** the derived `Ord`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceScope {
    /// Entry explicitly listed in **project** settings (`.cyrup/settings.json`
    /// `skills`/`prompts`/`themes` plain path) — Pi `source:"local", scope:"project"`, rank `0`
    /// (package-manager.ts:906-916, 184-188). Highest precedence.
    ProjectSettings,
    /// Auto-discovered **project** loose file (`.cyrup/*`, `.agents/skills`; trust-gated) — Pi
    /// `source:"auto", scope:"project"`, rank `1` (package-manager.ts:2254-2259).
    Project,
    /// Entry explicitly listed in **global** settings — Pi `source:"local", scope:"user"`, rank `2`
    /// (package-manager.ts:917-927, 184-188).
    GlobalSettings,
    /// Auto-discovered **global** loose file — Pi `source:"auto", scope:"user"`, rank `3`
    /// (package-manager.ts:2248-2253).
    Global,
    /// Package installed project-local (trust-gated) — Pi `origin:"package"`, rank `4`
    /// (package-manager.ts:185). All packages share one rank regardless of scope; the
    /// project-local install is inserted before the global one so it wins the same-rank tie.
    ProjectPackage,
    /// Package installed at global scope — Pi `origin:"package"`, rank `4`.
    GlobalPackage,
    /// Explicit `--skill` / `--prompt-template` / `--theme` — Pi `source:"cli", scope:"temporary"`.
    /// Pi never runs the explicit `additionalSkillPaths` through `resourcePrecedenceRank`; it
    /// concatenates them **after** the entire sorted accumulator —
    /// `mergePaths([...cliEnabledSkills, ...enabledSkills], additionalSkillPaths)`
    /// (resource-loader.ts:421/436/455) — so under first-wins a same-name resource of *any* rank,
    /// **including a rank-`4` package**, wins and the CLI path loses. Modeled here as rank `5`
    /// (below every package, above the cyrup-only `Discovered`/`Builtin` deltas).
    Cli,
    /// Contributed via `resources_discover` (R-09-022). [CYRUP-DELTA] — no Pi equivalent; ranked
    /// `6` (below every package, user/project, and CLI resource, above the compiled fallback) so any
    /// user-, package-, or CLI-supplied resource overrides an extension contribution.
    Discovered,
    /// Compiled-in fallback (themes `dark`/`light`, R-09-011). [CYRUP-DELTA] — ranked `7`, the
    /// lowest precedence, so anything else of the same name overrides it.
    Builtin,
}

impl ResourceScope {
    /// Pi's `resourcePrecedenceRank` (package-manager.ts:172-188): **lower rank wins** under the
    /// first-wins same-name dedup applied in [`crate::discovery::ResourceSet::build`]. Pi's five
    /// ranks are `0` project+settings, `1` project+auto, `2` user+settings, `3` user+auto, and `4`
    /// for *any* package (`if (m.origin === "package") return 4`, regardless of scope). The explicit
    /// CLI/`temporary` source is **not** in `resourcePrecedenceRank`: Pi appends
    /// `additionalSkillPaths` after the entire sorted accumulator (resource-loader.ts:421/436/455),
    /// so it loses first-wins to a same-name resource of any rank — including a rank-`4` package.
    /// Modeled here as rank `5`. The `Discovered` (`6`) and `Builtin` (`7`) tiers are cyrup-only and
    /// rank below everything Pi knows.
    pub fn precedence_rank(self) -> u8 {
        match self {
            ResourceScope::ProjectSettings => 0,
            ResourceScope::Project => 1,
            ResourceScope::GlobalSettings => 2,
            ResourceScope::Global => 3,
            ResourceScope::ProjectPackage => 4,
            ResourceScope::GlobalPackage => 4,
            ResourceScope::Cli => 5,
            ResourceScope::Discovered => 6,
            ResourceScope::Builtin => 7,
        }
    }

    /// Pi's `SourceScope` (`"user" | "project" | "temporary"`, source-info.ts:3) for the RPC
    /// `get_commands` catalog. Pi's package-manager assigns `scope:"project"` to project settings
    /// and project packages, `scope:"user"` to global settings and global packages
    /// (package-manager.ts:911-924/1225-1229), and `scope:"temporary"` to explicit CLI/`--skill`
    /// sources (package-manager.ts:940). The cyrup-only `Discovered`/`Builtin` deltas have no Pi
    /// tier; they map to `"temporary"` (the transient, non-persisted scope).
    pub fn pi_source_scope(self) -> &'static str {
        match self {
            ResourceScope::ProjectSettings
            | ResourceScope::Project
            | ResourceScope::ProjectPackage => "project",
            ResourceScope::GlobalSettings
            | ResourceScope::Global
            | ResourceScope::GlobalPackage => "user",
            ResourceScope::Cli | ResourceScope::Discovered | ResourceScope::Builtin => "temporary",
        }
    }

    /// Pi's `PathMetadata.source` string (package-manager.ts): `"local"` for settings-listed
    /// entries, `"auto"` for auto-discovered loose files. Package/CLI/delta tiers carry a more
    /// specific source rendered by [`ResourceOrigin::source_info_json`].
    fn pi_source_label(self) -> &'static str {
        match self {
            ResourceScope::ProjectSettings | ResourceScope::GlobalSettings => "local",
            _ => "auto",
        }
    }
}

impl ResourceOrigin {
    /// Render this provenance as Pi's `SourceInfo` (`{path, source, scope, origin, baseDir?}`,
    /// source-info.ts:6-12) for the RPC `get_commands` catalog (rpc-mode.ts:653-683). `path` is the
    /// resource file; `source`/`scope`/`origin`/`baseDir` are the `PathMetadata` fields Pi records at
    /// discovery time (package-manager.ts:56-61) — mapped 1:1 from the `scope`/`origin` cyrup's
    /// `Skill`/`PromptTemplate` already carry. `origin:"package"` for installed packages, else
    /// `"top-level"`; `baseDir` is present only for the settings/auto loose-file tiers (the config
    /// dir root), omitted for packages (Pi never sets it there).
    pub fn source_info_json(&self, path: &Path) -> serde_json::Value {
        let path_str = path.display().to_string();
        match self {
            ResourceOrigin::LooseFile { scope, root } => serde_json::json!({
                "path": path_str,
                "source": scope.pi_source_label(),
                "scope": scope.pi_source_scope(),
                "origin": "top-level",
                "baseDir": root.display().to_string(),
            }),
            ResourceOrigin::Package { id, scope } => serde_json::json!({
                "path": path_str,
                "source": id.as_str(),
                "scope": scope.pi_source_scope(),
                "origin": "package",
            }),
            ResourceOrigin::Cli { path: _ } => serde_json::json!({
                "path": path_str,
                "source": "cli",
                "scope": "temporary",
                "origin": "top-level",
            }),
            ResourceOrigin::Extension { ext } => serde_json::json!({
                "path": path_str,
                "source": ext.as_str(),
                "scope": "temporary",
                "origin": "top-level",
            }),
            ResourceOrigin::Builtin => serde_json::json!({
                "path": path_str,
                "source": "builtin",
                "scope": "temporary",
                "origin": "top-level",
            }),
        }
    }
}

/// Provenance detail kept for diagnostics / `list`.
#[derive(Clone, Debug)]
pub enum ResourceOrigin {
    Builtin,
    LooseFile { scope: ResourceScope, root: PathBuf },
    Package { id: PackageId, scope: ResourceScope },
    Cli { path: PathBuf },
    Extension { ext: ExtensionId },
}

/// Install destination for a package (R-09-017). Project-local installs are trust-gated.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallScope {
    Global,
    Project,
}

impl InstallScope {
    /// The `ResourceScope` tier a package's resources enter at.
    pub fn package_resource_scope(self) -> ResourceScope {
        match self {
            InstallScope::Global => ResourceScope::GlobalPackage,
            InstallScope::Project => ResourceScope::ProjectPackage,
        }
    }
}
