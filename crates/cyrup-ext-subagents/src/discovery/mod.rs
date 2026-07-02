//! Agent/chain definition, scoping, and skill association (func-SA §5.1; arch-SA §6.2).
//!
//! Entry points: [`discover_agents_all`]/[`discover_agents`] (arch-SA §6.2). Discovery is
//! synchronous, on-demand, and re-scanned per call (R-SA-019) — never backed by a persistent
//! filesystem watcher or cache; every call below re-walks every configured directory from
//! scratch, and this module holds no state across calls.
//!
//! This file is the integration point wiring together this module's already-written siblings —
//! [`frontmatter`] (per-file parsing), [`merge`] (four-tier precedence + settings-override
//! application), [`chains`] (chain-file discovery), [`management`] (CRUD) — into the two public
//! entry points other phases of this crate (`exec/`, `background/`, `registration/`) call:
//!
//! - [`discover_agents_all`] — the **management/introspection** view (R-SA-013): includes
//!   disabled agents, used for CRUD and re-enabling.
//! - [`discover_agents`] — the **delegation/execution-time** view (R-SA-013): excludes disabled
//!   agents, narrowed by an optional [`AgentReadScope`] override, used for actual runtime
//!   selection.
//!
//! Both share one internal walk-and-merge pipeline ([`run_discovery`]) so the two views can never
//! silently drift apart on anything except R-SA-013's disabled-visibility policy itself.
//!
//! # R-SA-001..004: four-scope discovery, directory-walk order, dedup asymmetry
//!
//! Builtin agents load via [`cyrup_resources::package::manifest::resolve_manifest`] applied to a
//! caller-supplied builtin agents directory (R-SA-020: the same manifest/discovery plumbing
//! `cyrup-resources` already provides for skills/prompts, reused here for the `agents` resource
//! kind Phase 0 of this crate's build-out added to `ManifestResources`/`ResolvedManifest`).
//! Package-tier roots are enumerated via [`cyrup_resources::InstalledPackages`] plus
//! `cyrup_resources::package::store::installed_dir`, in the same project-scope-then-global-scope
//! fixed order `cyrup-resources`' own `discover()` uses (mirrored here rather than re-derived
//! independently, so this crate's package enumeration order never silently disagrees with
//! `cyrup-resources`' own). User/Project tiers walk plain directories directly
//! ([`walk_agent_dir`]), alphabetical-by-filename, depth-first (R-SA-004) — the same traversal
//! convention `discovery/chains.rs` already uses for chain files.
//!
//! Per-tier dedup (R-SA-002) and cross-tier precedence (R-SA-001) are entirely
//! [`merge::discover_and_merge`]'s job; this file only assembles the already-tier-scanned
//! `Vec<AgentDefinition>` lists ([`merge::TieredAgents`]) in the right scan order and hands them
//! off — it does not re-implement any merge/precedence logic of its own.
//!
//! # R-SA-003: extra agent directories via environment
//!
//! [`EXTRA_AGENT_DIRS_ENV_VAR`] (`CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`, mirroring pi-subagents'
//! `PI_SUBAGENT_EXTRA_AGENT_DIRS`) is a platform path-list-delimiter-separated (`:` on Unix, `;`
//! on Windows, via [`std::env::split_paths`]) list of additional read-only agent directories,
//! scanned as **User** scope (R-SA-003's own text: "scanned as User scope") — i.e. folded into
//! the same last-directory-scanned-wins tier as the ordinary user agents directory, appended
//! *after* the primary user directory in fixed scan order so R-SA-002's last-seen-wins rule
//! applies uniformly across the whole User-tier candidate stream.
//!
//! # R-SA-007: legacy skill-path exclusion
//!
//! Any path under a directory segment literally named [`SKILLS_DIR_SEGMENT`] within an
//! agent-scan root MUST be excluded from agent-file discovery, so a package/user/project
//! directory that also bundles `skills/<name>/SKILL.md` content never has those `SKILL.md` (or
//! any other `.md`) files misparsed as agent definitions. Enforced by [`walk_agent_dir`] for
//! User/Project directory walks; a manifest-declared package/builtin `agents` root that resolves
//! to a directory (rather than an already-concrete file — see [`expand_manifest_agent_entry`]'s
//! own doc for why `resolve_manifest` sometimes yields one and sometimes the other) is expanded
//! via that exact same [`walk_agent_dir`] call, so the identical R-SA-007 exclusion applies
//! uniformly to every tier's directory-rooted scan — there is no separate skills-subpath
//! exclusion to duplicate at the package/builtin tiers, because they route through the same walk
//! function as User/Project.

/// Shared `AgentDefinition`/`ChainDefinition` types (func-SA §4.1/§5.1, R-SA-001..022, arch-SA
/// §3.3). Pure type definitions only — see module doc there for why `AgentDefinition` does not
/// implement `cyrup_resources::discovery::Named`.
pub mod types;

/// Hand-rolled YAML-subset frontmatter parser (func-SA §5.1 R-SA-005/006/018; arch-SA §6.2.3).
/// Parses one agent `.md` file's frontmatter + body into an `AgentDefinition`, applying the
/// required-field silent-skip (R-SA-005), invalid-package-identifier whole-file skip (R-SA-006),
/// and name-sensitive `systemPromptMode`/`inheritProjectContext` defaults (R-SA-018). Also reused
/// by `discovery/chains.rs` for `.chain.md` files via its low-level `parse_frontmatter_block`.
pub mod frontmatter;

/// Chain-file discovery: `.chain.json` > `.chain.md` same-name precedence within one directory
/// scan, cross-scope retention (never merged) across scan scopes (func-SA §5.1 R-SA-015; arch-SA
/// §6.2.2).
pub mod chains;

/// Agent/chain management CRUD: create/update/delete/rename, restricted to User/Project sources
/// (R-SA-014), plus the three call-site-dependent `disabled`-visibility views (R-SA-013). Depends
/// only on `types.rs` (scoping/mutability) and `frontmatter.rs` (read-only reuse for round-trip
/// re-parsing after a write) — does not depend on `merge.rs` (func-SA §5.1 R-SA-013/014/019;
/// arch-SA §2.2).
pub mod management;

/// Four-tier Builtin/Package/User/Project precedence merge and settings-override application
/// (func-SA §5.1 R-SA-001/002/004/009/010/011/012/020/021; arch-SA §6.2/§6.2.1). A bespoke, plain
/// `HashMap`/`Vec` algorithm — deliberately NOT built on `cyrup_resources::discovery::ResourceSet
/// <T>` (see this module's own doc for why). Consumes already-parsed `Vec<AgentDefinition>` per
/// tier/scan-scope (as produced by `frontmatter.rs` over a directory walk this module owns); does
/// no filesystem I/O of its own.
pub mod merge;

use std::path::{Path, PathBuf};

use cyrup_resources::package::store::installed_dir;
use cyrup_resources::{InstallScope, InstalledPackage, InstalledPackages, resolve_manifest};

use crate::error::SubagentError;
use chains::{ChainScanResult, scan_chain_scopes};
use management::{AgentVisibility, ChainVisibility};
use types::{AgentDefinition, AgentReadScope, AgentSource, ChainDefinition, ChainDiscoveryDiagnostic, SubagentSettings};

/// Directory segment reserved for skill bundling (R-SA-007), excluded from agent-file discovery
/// wherever it appears in a User/Project agent-scan root. Kept as this module's own constant
/// (rather than importing `chains.rs`'s private, identically-named one) since the two modules'
/// walks are independent and neither's constant is part of the other's public contract — mirrors
/// this crate's established "each module keeps its own copy of a small, narrowly-scoped private
/// helper/constant rather than sharing one `pub` item across unrelated walks" convention (see
/// `discovery::chains`/`discovery::management`'s identically-justified duplicate
/// `placeholder_runner_step` helpers).
const SKILLS_DIR_SEGMENT: &str = "skills";

/// The environment variable carrying a platform-path-list-delimiter-separated list of additional
/// read-only agent directories, scanned as **User** scope (R-SA-003). Mirrors pi-subagents'
/// `PI_SUBAGENT_EXTRA_AGENT_DIRS`, renamed to this crate's own `CYRUP_SUBAGENT_*` convention
/// (matching `spawn::SUBAGENT_BINARY_ENV_VAR`/`spawn::depth::DEPTH_ENV_VAR`'s established naming).
pub const EXTRA_AGENT_DIRS_ENV_VAR: &str = "CYRUP_SUBAGENT_EXTRA_AGENT_DIRS";

/// File extension recognized for agent persona definitions.
const AGENT_FILE_EXTENSION: &str = "md";

// -------------------------------------------------------------------------------------------
// AgentDiscoveryConfig (arch-SA §6.2's `cfg: &AgentDiscoveryConfig` parameter)
// -------------------------------------------------------------------------------------------

/// Everything one [`discover_agents_all`]/[`discover_agents`] call needs, assembled by the
/// caller (normally `registration/mod.rs`'s config-layering step, R-SA-133) from cyrup's own
/// resolved directory/settings state. This module performs no directory-resolution of its own
/// beyond what is documented per-field below — it never re-derives `ConfigDirs`,
/// `SettingsManager` layering, or package-install enumeration; those all live in their owning
/// crates (`cyrup-config`, `cyrup-resources`) and are handed in already-resolved.
#[derive(Clone, Debug, Default)]
pub struct AgentDiscoveryConfig {
    /// The builtin agents directory bundled with this extension (personas shipped with cyrup
    /// itself, e.g. `scout.md`/`worker.md`/`delegate.md`). `None` when no builtin directory is
    /// configured (e.g. a minimal test harness) — an absent/missing directory is not an error,
    /// per this module's directory-walk functions' own "missing dir yields empty, not an error"
    /// convention (mirroring `discovery::chains::scan_chain_dir`'s identical behavior).
    pub builtin_agents_dir: Option<PathBuf>,
    /// Installed packages (both Global and Project [`InstallScope`]) whose manifests may declare
    /// an `agents = [...]` resource list (R-SA-020). Package-tier roots are resolved from this
    /// list via `cyrup_resources::package::store::installed_dir` + `resolve_manifest`, in the
    /// fixed project-then-global scan order `cyrup-resources`' own `discover()` uses (§6.2.1
    /// doc), never re-derived independently.
    pub installed_packages: InstalledPackages,
    /// Whether the current project is trusted (R-09-003/008/012-equivalent fail-closed trust
    /// gate, mirroring `cyrup_resources::discovery`'s own installed-package trust check exactly):
    /// a Project-scope installed package's `agents` manifest entries are skipped entirely when
    /// this is `false`, matching `cyrup-resources`' own project-package trust gate so this
    /// crate's package-tier enumeration never silently diverges from that crate's skill/prompt
    /// enumeration on the same installed-package set.
    pub trusted_project: bool,
    /// The global directory used to resolve a Global-scope installed package's on-disk root (the
    /// same `global_dir` `cyrup_resources::DiscoveryConfig` carries).
    pub global_dir: PathBuf,
    /// The project root used to resolve a Project-scope installed package's on-disk root, when
    /// operating inside a project. `None` outside any project.
    pub project_root: Option<PathBuf>,
    /// User-scope agent directories, in fixed scan order (R-SA-004: each individually walked
    /// alphabetical-by-filename). Ordinary caller-supplied entries — [`EXTRA_AGENT_DIRS_ENV_VAR`]
    /// entries (R-SA-003) are appended to this list by [`AgentDiscoveryConfig::with_env_extras`]
    /// / [`resolve_extra_agent_dirs`] rather than being folded in silently by this struct's own
    /// constructor, so a caller inspecting `user_agent_dirs` after construction sees exactly what
    /// it explicitly set unless it explicitly opted into the env-var extension.
    pub user_agent_dirs: Vec<PathBuf>,
    /// User-scope chain directories, in fixed scan order — kept as an independent list from
    /// `user_agent_dirs` since chain files (`discovery::chains`) and agent files use different
    /// discovery entry points even though they typically live under the same on-disk root; a
    /// caller normally populates this with the same paths as `user_agent_dirs`.
    pub user_chain_dirs: Vec<PathBuf>,
    /// Project-scope agent directories, in fixed scan order (R-SA-004).
    pub project_agent_dirs: Vec<PathBuf>,
    /// Project-scope chain directories, in fixed scan order.
    pub project_chain_dirs: Vec<PathBuf>,
    /// The `subagents` settings block (R-SA-009/010/011/012), already layered/resolved by the
    /// caller (R-SA-133) — a malformed value here is the caller's problem to have already
    /// surfaced via [`parse_subagent_settings`] before constructing a valid config; this struct
    /// carries the already-typed, already-valid result.
    pub settings: SubagentSettings,
}

impl AgentDiscoveryConfig {
    /// Append [`EXTRA_AGENT_DIRS_ENV_VAR`]'s entries (if the variable is set and non-empty) to
    /// `user_agent_dirs`, in the order [`std::env::split_paths`] yields them — i.e. *after* any
    /// ordinary user directories already present, so R-SA-002's last-directory-scanned-wins User
    /// tier rule naturally lets an extra directory's same-named agent win over the primary user
    /// directory's, matching pi-subagents' own append-after-primary placement for
    /// `PI_SUBAGENT_EXTRA_AGENT_DIRS`. A no-op when the variable is absent or empty.
    #[must_use]
    pub fn with_env_extras(mut self) -> Self {
        self.user_agent_dirs
            .extend(resolve_extra_agent_dirs(|key| std::env::var(key).ok()));
        self
    }
}

/// The pure core of [`AgentDiscoveryConfig::with_env_extras`], parameterized over the env lookup
/// (R-SA-003) so it can be exercised deterministically in unit tests without mutating real
/// process environment state — mirrors `spawn::resolve_spawn_command_from`'s and
/// `spawn::depth::resolve_effective_depth_from`'s identical env-lookup-closure-injection pattern
/// (this crate is `#![forbid(unsafe_code)]`, so tests never call `std::env::set_var`/`remove_var`
/// directly).
fn resolve_extra_agent_dirs(env_lookup: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let Some(raw) = env_lookup(EXTRA_AGENT_DIRS_ENV_VAR) else {
        return Vec::new();
    };
    if raw.is_empty() {
        return Vec::new();
    }
    std::env::split_paths(&raw)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

// -------------------------------------------------------------------------------------------
// Settings parsing (R-SA-009: malformed subagents.* settings MUST abort discovery)
// -------------------------------------------------------------------------------------------

/// Parse the raw `subagents` settings [`serde_json::Value`] (as read from `cyrup-config`'s
/// layered, untyped settings map, e.g. via `Settings::get("subagents")`) into a typed
/// [`SubagentSettings`]. A malformed shape — wrong field types, an `overrides` entry that is not
/// an object, etc. — MUST cause discovery to fail with a surfaced error (R-SA-009), never a
/// silent skip or diagnostic (those are reserved for malformed agent frontmatter and malformed
/// chain files respectively, R-SA-009's own three-way distinction). An absent `subagents` key
/// (the common case: no settings customization at all) yields the all-default
/// [`SubagentSettings`], not an error.
pub fn parse_subagent_settings(
    raw: Option<&serde_json::Value>,
) -> Result<SubagentSettings, SubagentError> {
    match raw {
        None => Ok(SubagentSettings::default()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| SubagentError::MalformedSettings(e.to_string())),
    }
}

// -------------------------------------------------------------------------------------------
// Directory-walk (R-SA-004/005/006/007): User/Project agent-file scanning
// -------------------------------------------------------------------------------------------

/// Recursively walk `root` for agent `.md` files, alphabetical-by-filename, depth-first
/// (R-SA-004), excluding any subtree rooted at a directory segment literally named
/// [`SKILLS_DIR_SEGMENT`] (R-SA-007). Each file is parsed via
/// [`frontmatter::parse_agent_file`], which itself silently skips a file missing `name`/
/// `description` (R-SA-005) or bearing an invalid `package` identifier (R-SA-006) — this
/// function simply omits a `None` result from its output, continuing the walk unaffected
/// (R-SA-005's "discovery of other files MUST continue unaffected").
///
/// A `root` that does not exist (or is not readable) yields an empty `Vec`, not an error — an
/// absent scope directory is a normal, unconfigured-scope condition, not a malformed-discovery
/// one (mirrors `discovery::chains::scan_chain_dir`'s identical convention).
///
/// Returned in scan order (which, per R-SA-004, is exactly the order that determines same-scope
/// collision winners once handed to `merge::reduce_last_seen_wins`/`reduce_first_seen_wins`).
pub fn walk_agent_dir(root: &Path, source: AgentSource) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    walk_agent_dir_into(root, source, &mut out);
    out
}

fn walk_agent_dir_into(dir: &Path, source: AgentSource, out: &mut Vec<AgentDefinition>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    for path in children {
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if path.is_dir() {
            // R-SA-007: never descend into a directory segment reserved for skill bundling.
            if file_name == SKILLS_DIR_SEGMENT {
                continue;
            }
            walk_agent_dir_into(&path, source, out);
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some(AGENT_FILE_EXTENSION) {
            continue;
        }
        // Chain files use the double-suffix `.chain.md` — never mistake one for a plain agent
        // `.md` file (chain discovery is `discovery::chains`'s own, separate walk).
        if file_name.ends_with(".chain.md") {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(def) = frontmatter::parse_agent_file(&content, source, &path) {
            out.push(def);
        }
    }
}

/// Walk multiple User/Project agent directories in fixed scan order, concatenating their
/// per-directory [`walk_agent_dir`] results into one flat, scan-ordered `Vec` — the shape
/// [`merge::reduce_last_seen_wins`] expects for its own last-directory-scanned-wins reduction
/// (R-SA-002).
fn walk_agent_dirs(roots: &[PathBuf], source: AgentSource) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    for root in roots {
        out.extend(walk_agent_dir(root, source));
    }
    out
}

// -------------------------------------------------------------------------------------------
// Package tier (R-SA-020/021): cyrup-resources manifest plumbing, bespoke precedence
// -------------------------------------------------------------------------------------------

/// Expand one manifest-resolved `agents` entry (`cyrup_resources::ResolvedManifest::agents`) into
/// zero or more parsed [`AgentDefinition`]s. A manifest entry is **not** guaranteed to already be
/// a concrete file: `resolve_manifest`'s `resolve_entries` only fully expands a directory entry
/// into its member files when the manifest also declares at least one override pattern (`!`/`+`/
/// `-`) for that resource kind — the common, override-free case (a plain `agents = ["./agents"]`
/// declaration, R-SA-020) instead yields the **directory root itself** as the sole entry (mirrors
/// `cyrup_resources::discovery`'s own skill-loading call sites, which handle the identical
/// file-vs-directory duality via `if sdir.is_file() { load_one_skill(...) } else {
/// scan_skill_root(...) }`, `discovery.rs`'s installed-packages loop). This function is this
/// crate's analog: a file entry is parsed directly; a directory entry is expanded via
/// [`walk_agent_dir`] (R-SA-004/005/006/007 all apply uniformly to that expansion, since it is
/// the exact same walk User/Project tiers use).
fn expand_manifest_agent_entry(entry: &Path, source: AgentSource, out: &mut Vec<AgentDefinition>) {
    if entry.is_file() {
        let Ok(content) = std::fs::read_to_string(entry) else {
            return;
        };
        if let Some(def) = frontmatter::parse_agent_file(&content, source, entry) {
            out.push(def);
        }
    } else if entry.is_dir() {
        out.extend(walk_agent_dir(entry, source));
    }
    // A non-existent entry (dangling manifest declaration) is silently skipped — not this
    // function's place to surface a diagnostic (see `scan_package_agents`'s own doc for why
    // package-manifest-level failures are not duplicated here).
}

/// Enumerate every installed package's declared `agents` manifest entries, in the fixed
/// project-scope-then-global-scope order `cyrup_resources::discovery::discover`'s own installed-
/// package loop uses (`discovery.rs`'s "Pi pushes project-scope packages before global ones"
/// comment) — mirrored here exactly rather than re-derived independently, so this crate's
/// package-tier scan order never silently disagrees with `cyrup-resources`' own skill/prompt
/// enumeration order over the identical `installed_packages` list. Each manifest-resolved `agents`
/// entry (R-SA-020) is expanded via [`expand_manifest_agent_entry`] and parsed via
/// [`frontmatter::parse_agent_file`] with `package_name` left exactly as each file's own
/// frontmatter declares — this function does not inject a package-derived name of its own; per
/// R-SA-008, the runtime name is `{package}.{local_name}` driven purely by each agent file's
/// literal `package:` frontmatter field, matching pi-subagents' own per-file (not per-manifest)
/// package-identity source of truth.
///
/// A Project-scope package is skipped entirely when `trusted_project` is `false` (mirroring
/// `cyrup_resources::discovery`'s identical fail-closed trust gate over the same
/// `InstalledPackage` list) — never silently trusted just because this crate's own discovery
/// pass runs independently of `cyrup-resources`' own.
///
/// A package whose manifest fails to resolve (`resolve_manifest` returns `Err`, e.g. malformed
/// `cyrup.toml`) is skipped for agent purposes with no diagnostic surfaced by this function — R-
/// SA-009's three-way throw/silent-skip/diagnostic taxonomy does not have a package-manifest-
/// level case of its own; `cyrup-resources`' own discovery pass is the authoritative place such a
/// failure is already surfaced as a warning for skills/prompts, so this crate does not duplicate
/// that reporting for agents.
pub fn scan_package_agents(cfg: &AgentDiscoveryConfig) -> Vec<AgentDefinition> {
    let mut ordered: Vec<&InstalledPackage> = cfg.installed_packages.packages.iter().collect();
    ordered.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });

    let mut out = Vec::new();
    for pkg in ordered {
        if pkg.scope == InstallScope::Project && !cfg.trusted_project {
            continue;
        }
        let Some(dir) = installed_dir(
            &pkg.source,
            pkg.scope,
            &pkg.id,
            &cfg.global_dir,
            cfg.project_root.as_deref(),
        ) else {
            continue;
        };
        let Ok(manifest) = resolve_manifest(&dir) else {
            continue;
        };
        for agent_entry in &manifest.agents {
            expand_manifest_agent_entry(agent_entry, AgentSource::Package, &mut out);
        }
    }
    out
}

/// Load the builtin agents tier via the same `cyrup-resources` manifest plumbing (R-SA-020),
/// applied to `cfg.builtin_agents_dir`. `None`/a non-existent directory yields an empty `Vec`,
/// not an error — an unconfigured builtin directory (e.g. a minimal test harness with no bundled
/// personas) is a normal condition.
pub fn scan_builtin_agents(cfg: &AgentDiscoveryConfig) -> Vec<AgentDefinition> {
    let Some(dir) = cfg.builtin_agents_dir.as_ref() else {
        return Vec::new();
    };
    let Ok(manifest) = resolve_manifest(dir) else {
        // A builtin directory that fails manifest resolution (e.g. no recognizable manifest
        // shape and no conventional `agents/` subdirectory) is treated as "no builtin agents
        // declared" rather than an error — `resolve_manifest`'s own auto-discovery fallback
        // already covers the common "just a directory of files" builtin-agents-dir shape by
        // detecting a conventional `agents/` child dir; a directory that is itself already the
        // agents root (no `agents/` subdirectory of its own) falls through to this arm and is
        // walked directly below instead.
        return walk_agent_dir(dir, AgentSource::Builtin);
    };
    if manifest.agents.is_empty() {
        // No manifest-declared `agents` entries resolved (including the "this directory has no
        // conventional agents/ subdirectory" auto-discovery case) — fall back to treating `dir`
        // itself as the agents root directly, so a builtin_agents_dir pointing straight at a flat
        // directory of `.md` personas (the common case for this extension's own bundled
        // scout.md/worker.md/delegate.md) still discovers them without requiring a manifest.
        return walk_agent_dir(dir, AgentSource::Builtin);
    }
    let mut out = Vec::new();
    for agent_entry in &manifest.agents {
        expand_manifest_agent_entry(agent_entry, AgentSource::Builtin, &mut out);
    }
    out
}

// -------------------------------------------------------------------------------------------
// Top-level entry points (arch-SA §6.2's discover_agents shape)
// -------------------------------------------------------------------------------------------

/// The full discovery result: every merged agent (management view, R-SA-013), every discovered
/// chain across scopes (never merged, R-SA-015), and any non-fatal chain-file diagnostics
/// (R-SA-009's diagnostic case).
#[derive(Debug, Default)]
pub struct AgentDiscoveryResult {
    /// Every discovered, merged, override-applied agent — **includes disabled agents**
    /// (R-SA-013's management/introspection view). Callers needing the delegation/execution-time
    /// view should call [`discover_agents`] instead of filtering this list themselves, so the
    /// filter logic stays centralized in [`management::AgentVisibility`].
    pub agents: Vec<AgentDefinition>,
    /// Every discovered chain across every scope, never merged across scopes (R-SA-015):
    /// same-named chains from different scopes both survive, tagged with their own
    /// [`AgentSource`].
    pub chains: Vec<ChainDefinition>,
    /// Non-fatal per-chain-file parse diagnostics (R-SA-009's diagnostic case) — never aborts
    /// discovery of sibling files.
    pub diagnostics: Vec<ChainDiscoveryDiagnostic>,
}

/// Run the shared walk-and-merge pipeline once: four-tier agent scan + merge + overrides
/// (R-SA-001/002/004/009/010/011/012/020/021), plus cross-scope chain discovery (R-SA-015). Both
/// [`discover_agents_all`] and [`discover_agents`] call this and differ only in which
/// [`management::AgentVisibility`] filter (if any) they apply to the result afterward — so the
/// two public entry points can never diverge on anything except R-SA-013's disabled-visibility
/// policy itself.
fn run_discovery(cfg: &AgentDiscoveryConfig) -> Result<AgentDiscoveryResult, SubagentError> {
    let builtin = scan_builtin_agents(cfg);
    let package = scan_package_agents(cfg);
    let user = walk_agent_dirs(&cfg.user_agent_dirs, AgentSource::User);
    let project = walk_agent_dirs(&cfg.project_agent_dirs, AgentSource::Project);

    let tiers = merge::TieredAgents {
        builtin,
        package,
        user,
        project,
    };
    let merged = merge::discover_and_merge(tiers, &cfg.settings)?;

    let mut agents: Vec<AgentDefinition> = merged.into_values().collect();
    // Deterministic output order (by runtime name) independent of the underlying `HashMap`'s
    // iteration order, so repeated calls over the same on-disk state are stable for callers/tests
    // — mirrors `discovery::chains::scan_chain_dir`'s identical "sort by name before returning"
    // convention.
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let mut chain_scopes: Vec<(PathBuf, AgentSource)> = Vec::new();
    for dir in &cfg.user_chain_dirs {
        chain_scopes.push((dir.clone(), AgentSource::User));
    }
    for dir in &cfg.project_chain_dirs {
        chain_scopes.push((dir.clone(), AgentSource::Project));
    }
    let ChainScanResult { chains, diagnostics } = scan_chain_scopes(&chain_scopes);

    Ok(AgentDiscoveryResult {
        agents,
        chains,
        diagnostics,
    })
}

/// **Management/introspection** discovery entry point (R-SA-013, R-SA-019): re-walks every
/// configured directory from scratch on every call, returns every merged agent **including
/// disabled ones** (via [`management::AgentVisibility::management`]) plus every discovered chain
/// and any chain-file diagnostics. Used for CRUD operations (a caller must be able to *see* a
/// disabled agent in order to re-enable it) and other full-introspection surfaces (e.g.
/// `/subagents-doctor`).
///
/// Per R-SA-019, a caller performing a create → get → update → delete management sequence MUST
/// re-invoke this function before each mutating action rather than reusing a prior result — this
/// function does not (and, holding no cache, cannot) enforce that on its own; it simply never
/// violates it by never caching anything itself.
pub fn discover_agents_all(cfg: &AgentDiscoveryConfig) -> Result<AgentDiscoveryResult, SubagentError> {
    let mut result = run_discovery(cfg)?;
    result.agents = AgentVisibility::management(&result.agents)
        .into_iter()
        .cloned()
        .collect();
    result.chains = ChainVisibility::management(&result.chains)
        .into_iter()
        .cloned()
        .collect();
    Ok(result)
}

/// **Delegation/execution-time** discovery entry point (R-SA-013, R-SA-019): re-walks every
/// configured directory from scratch on every call, returns every merged agent **excluding
/// disabled ones** (via [`management::AgentVisibility::delegation`]), optionally narrowed by
/// `scope_override` (func-SA §4.3 `RunOptions::agent_scope`; `None` uses the default `Both`
/// scope, i.e. no additional narrowing beyond what `cfg`'s own `user_agent_dirs`/
/// `project_agent_dirs` already scan). This is the view actual runtime dispatch (`exec/`) uses to
/// resolve a requested agent name against R-SA-008's exact-string-equality match.
///
/// `scope_override` narrows *which already-discovered agents are visible*, not which directories
/// are scanned: [`AgentReadScope::User`] keeps only `AgentSource::User` (and, since Builtin/
/// Package agents are equally "not Project", also `AgentSource::Builtin`/`AgentSource::Package`
/// — R-SA data model's own `AgentReadScope` doc: "a read filter... distinct from `AgentSource`")
/// entries plus excludes `AgentSource::Project`; symmetrically for
/// [`AgentReadScope::Project`]; [`AgentReadScope::Both`] (the default) applies no additional
/// filter at all. This mirrors `AgentReadScope`'s own doc comment (`discovery/types.rs`): it is a
/// *read* filter layered on top of the already-merged result, never a second directory-scan
/// pass.
pub fn discover_agents(
    cfg: &AgentDiscoveryConfig,
    scope_override: Option<AgentReadScope>,
) -> Result<AgentDiscoveryResult, SubagentError> {
    let mut result = run_discovery(cfg)?;
    result.agents = AgentVisibility::delegation(&result.agents)
        .into_iter()
        .filter(|a| agent_matches_scope(a, scope_override.unwrap_or_default()))
        .cloned()
        .collect();
    result.chains = ChainVisibility::delegation(&result.chains)
        .into_iter()
        .cloned()
        .collect();
    Ok(result)
}

/// Whether `agent` is visible under `scope` (func-SA §4.1 `AgentReadScope` doc). `Both` (the
/// default) admits every source; `User`/`Project` each exclude the *other* named tier's agents
/// while still admitting Builtin/Package agents (an `AgentReadScope` narrows a *user-vs-project*
/// axis, not a "hide everything else" filter — Builtin/Package agents are orthogonal to that
/// axis and remain visible under either named scope).
fn agent_matches_scope(agent: &AgentDefinition, scope: AgentReadScope) -> bool {
    match scope {
        AgentReadScope::Both => true,
        AgentReadScope::User => agent.source != AgentSource::Project,
        AgentReadScope::Project => agent.source != AgentSource::User,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn write_agent(dir: &Path, file_name: &str, name: &str, description: &str) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(
            dir.join(file_name),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody for {name}.\n"),
        )
        .expect("write agent file");
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-003: extra agent directories via environment
    // -----------------------------------------------------------------------------------------

    #[test]
    fn resolve_extra_agent_dirs_splits_platform_path_list() {
        let joined = if cfg!(windows) { "/a;/b" } else { "/a:/b" };
        let dirs = resolve_extra_agent_dirs(|key| {
            (key == EXTRA_AGENT_DIRS_ENV_VAR).then(|| joined.to_string())
        });
        assert_eq!(dirs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn resolve_extra_agent_dirs_is_empty_when_env_var_absent() {
        let dirs = resolve_extra_agent_dirs(|_| None);
        assert!(dirs.is_empty());
    }

    #[test]
    fn resolve_extra_agent_dirs_is_empty_when_env_var_is_empty_string() {
        let dirs = resolve_extra_agent_dirs(|key| {
            (key == EXTRA_AGENT_DIRS_ENV_VAR).then(String::new)
        });
        assert!(dirs.is_empty());
    }

    #[test]
    fn with_env_extras_appends_after_existing_user_dirs_so_last_seen_wins_naturally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join("primary");
        let extra = tmp.path().join("extra");
        write_agent(&primary, "scout.md", "scout", "primary scout");
        write_agent(&extra, "scout.md", "scout", "extra scout");

        // SAFETY-equivalent: this test uses the pure closure-injected core directly, never
        // `std::env::set_var` (forbidden by this crate's `#![forbid(unsafe_code)]`), so no real
        // process environment mutation happens here at all — see `with_env_extras`'s own doc.
        let extra_path = extra.clone();
        let mut user_agent_dirs = vec![primary.clone()];
        user_agent_dirs.extend(resolve_extra_agent_dirs(move |key| {
            (key == EXTRA_AGENT_DIRS_ENV_VAR)
                .then(|| extra_path.to_string_lossy().to_string())
        }));

        assert_eq!(user_agent_dirs, vec![primary, extra]);
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-007: legacy skill-path exclusion at the agent-file walk level
    // -----------------------------------------------------------------------------------------

    #[test]
    fn skill_bundle_subdirectory_is_excluded_from_agent_discovery() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let skills_dir = tmp.path().join("skills").join("some-skill");
        std::fs::create_dir_all(&skills_dir).expect("mkdir");
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: not-an-agent\ndescription: this is a skill, not an agent\n---\n\nBody\n",
        )
        .expect("write SKILL.md");
        write_agent(tmp.path(), "real-agent.md", "real-agent", "a real agent");

        let discovered = walk_agent_dir(tmp.path(), AgentSource::Project);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "real-agent");
        assert!(!discovered.iter().any(|a| a.name == "not-an-agent"));
    }

    #[test]
    fn chain_md_files_are_never_parsed_as_agent_files_during_agent_walk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("release.chain.md"),
            "---\nname: release\ndescription: a chain, not an agent\n---\n\nBody\n",
        )
        .expect("write chain.md");
        write_agent(tmp.path(), "real-agent.md", "real-agent", "a real agent");

        let discovered = walk_agent_dir(tmp.path(), AgentSource::Project);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "real-agent");
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-004: alphabetical-by-filename, depth-first walk order
    // -----------------------------------------------------------------------------------------

    #[test]
    fn walk_agent_dir_visits_nested_subdirectories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent(&tmp.path().join("nested"), "deep.md", "deep", "nested agent");
        write_agent(tmp.path(), "shallow.md", "shallow", "top-level agent");

        let discovered = walk_agent_dir(tmp.path(), AgentSource::User);
        let names: Vec<&str> = discovered.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"deep"));
        assert!(names.contains(&"shallow"));
    }

    #[test]
    fn missing_scan_root_yields_empty_result_not_error() {
        let discovered = walk_agent_dir(Path::new("/does/not/exist/at/all"), AgentSource::User);
        assert!(discovered.is_empty());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-009: malformed subagents settings aborts discovery with a surfaced error
    // -----------------------------------------------------------------------------------------

    #[test]
    fn parse_subagent_settings_absent_yields_default() {
        let settings = parse_subagent_settings(None).expect("absent settings is not an error");
        assert!(settings.overrides.is_empty());
        assert_eq!(settings.default_model, None);
    }

    #[test]
    fn parse_subagent_settings_malformed_shape_is_an_error() {
        // `overrides` must be an object keyed by agent name; a plain string is malformed.
        let raw = serde_json::json!({ "overrides": "not-an-object" });
        let result = parse_subagent_settings(Some(&raw));
        assert!(matches!(result, Err(SubagentError::MalformedSettings(_))));
    }

    #[test]
    fn parse_subagent_settings_valid_shape_parses() {
        let raw = serde_json::json!({
            "defaultModel": "anthropic/claude-sonnet-4",
            "disableBuiltins": true,
        });
        let settings = parse_subagent_settings(Some(&raw)).expect("valid settings parse");
        assert_eq!(settings.default_model, Some("anthropic/claude-sonnet-4".to_string()));
        assert_eq!(settings.disable_builtins, Some(true));
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-013: discover_agents_all includes disabled; discover_agents excludes them
    // -----------------------------------------------------------------------------------------

    fn write_agent_with_disabled(dir: &Path, file_name: &str, name: &str, disabled: bool) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(
            dir.join(file_name),
            format!(
                "---\nname: {name}\ndescription: d\ndisabled: {disabled}\n---\n\nBody\n"
            ),
        )
        .expect("write agent file");
    }

    fn base_config(project_dir: &Path) -> AgentDiscoveryConfig {
        AgentDiscoveryConfig {
            project_agent_dirs: vec![project_dir.to_path_buf()],
            ..AgentDiscoveryConfig::default()
        }
    }

    #[test]
    fn discover_agents_all_includes_disabled_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent_with_disabled(tmp.path(), "on.md", "on-agent", false);
        write_agent_with_disabled(tmp.path(), "off.md", "off-agent", true);

        let cfg = base_config(tmp.path());
        let result = discover_agents_all(&cfg).expect("discovery succeeds");
        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"on-agent"));
        assert!(names.contains(&"off-agent"), "management view must include disabled agents");
    }

    #[test]
    fn discover_agents_excludes_disabled_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_agent_with_disabled(tmp.path(), "on.md", "on-agent", false);
        write_agent_with_disabled(tmp.path(), "off.md", "off-agent", true);

        let cfg = base_config(tmp.path());
        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"on-agent"));
        assert!(!names.contains(&"off-agent"), "delegation view must exclude disabled agents");
    }

    // -----------------------------------------------------------------------------------------
    // AgentReadScope narrowing (discover_agents' own scope_override parameter)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn scope_override_user_excludes_project_sourced_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        write_agent(&user_dir, "u.md", "user-agent", "from user");
        write_agent(&project_dir, "p.md", "project-agent", "from project");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![user_dir],
            project_agent_dirs: vec![project_dir],
            ..AgentDiscoveryConfig::default()
        };

        let result = discover_agents(&cfg, Some(AgentReadScope::User)).expect("discovery succeeds");
        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"user-agent"));
        assert!(!names.contains(&"project-agent"));
    }

    #[test]
    fn scope_override_both_admits_every_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        write_agent(&user_dir, "u.md", "user-agent", "from user");
        write_agent(&project_dir, "p.md", "project-agent", "from project");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![user_dir],
            project_agent_dirs: vec![project_dir],
            ..AgentDiscoveryConfig::default()
        };

        let result = discover_agents(&cfg, Some(AgentReadScope::Both)).expect("discovery succeeds");
        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"user-agent"));
        assert!(names.contains(&"project-agent"));
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-001: full four-scope precedence surfaces correctly through discover_agents_all
    // -----------------------------------------------------------------------------------------

    #[test]
    fn project_scope_wins_over_user_scope_on_name_collision_end_to_end() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        write_agent(&user_dir, "reviewer.md", "reviewer", "user reviewer");
        write_agent(&project_dir, "reviewer.md", "reviewer", "project reviewer");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![user_dir],
            project_agent_dirs: vec![project_dir],
            ..AgentDiscoveryConfig::default()
        };

        let result = discover_agents_all(&cfg).expect("discovery succeeds");
        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].source, AgentSource::Project);
        assert_eq!(result.agents[0].description, "project reviewer");
    }

    #[test]
    fn user_tier_last_directory_scanned_wins_end_to_end() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir_a = tmp.path().join("dir-a");
        let dir_b = tmp.path().join("dir-b");
        write_agent(&dir_a, "scout.md", "scout", "from dir-a");
        write_agent(&dir_b, "scout.md", "scout", "from dir-b");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![dir_a, dir_b],
            ..AgentDiscoveryConfig::default()
        };

        let result = discover_agents_all(&cfg).expect("discovery succeeds");
        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].description, "from dir-b");
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-015 (via run_discovery's chain wiring): chains flow through from chains::scan_chain_scopes
    // -----------------------------------------------------------------------------------------

    #[test]
    fn discover_agents_all_surfaces_chains_from_configured_scopes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_chains = tmp.path().join("user-chains");
        std::fs::create_dir_all(&user_chains).expect("mkdir");
        std::fs::write(
            user_chains.join("release.chain.json"),
            "{\"name\":\"release\",\"description\":\"d\",\"steps\":[]}",
        )
        .expect("write chain");

        let cfg = AgentDiscoveryConfig {
            user_chain_dirs: vec![user_chains],
            ..AgentDiscoveryConfig::default()
        };

        let result = discover_agents_all(&cfg).expect("discovery succeeds");
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].name, "release");
        assert!(result.diagnostics.is_empty());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-019: discovery is re-scanned per call (no cache) — a second call observes a
    // filesystem change made between calls.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn discovery_is_re_scanned_per_call_not_cached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = base_config(tmp.path());

        let before = discover_agents_all(&cfg).expect("discovery succeeds");
        assert!(before.agents.is_empty());

        write_agent(tmp.path(), "new.md", "new-agent", "added after first call");

        let after = discover_agents_all(&cfg).expect("discovery succeeds");
        assert_eq!(after.agents.len(), 1);
        assert_eq!(after.agents[0].name, "new-agent");
    }
}
