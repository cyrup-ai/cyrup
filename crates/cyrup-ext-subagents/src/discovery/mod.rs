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
//! scanned as **User** scope (R-SA-003's own text: "scanned as User scope"). They are **prepended
//! ahead of** the ordinary user agent directories in fixed scan order (pi `discoverAgents`
//! `[...userAgentsExtra, ...userAgentsOld, ...userAgentsNew]`, agents.ts:1300-1304), so under
//! R-SA-002's last-directory-scanned-wins rule the user's *own* agent dirs win over a bundled
//! extra-dir agent of the same name — extras are the **lowest-precedence** User-tier stream, a
//! read-only fallback for a same-named agent the user has not defined themselves. (A prior bug
//! appended extras *after* the user dirs, inverting this so a bundled extra shadowed the user's
//! own agent.)
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

/// Skills association (T5, C4): skill-name resolution against a cwd (reusing `cyrup-resources`
/// discovery), the lazy `<available_skills>` system-prompt injection, the `pi-subagents`
/// orchestration-skill exclusion, and proactive skill-subagent suggestions — a port of pi-subagents'
/// `agents/skills.ts:608-682` + `agents/proactive-skills.ts`. Consumed by `exec/mod.rs`'s prompt
/// assembly (the injection is composed into the child's task text at spawn).
pub mod skills;

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
use types::{
    AgentDefinition, AgentReadScope, AgentSource, ChainDefinition, ChainDiscoveryDiagnostic,
    LayeredOverrideSettings, SubagentSettings,
};

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
// Directory topology (pi findNearestProjectRoot / resolveNearestProject*Dirs, agents.ts:511-522,
// 1234-1259,1279-1280). Pure, filesystem-probing helpers the caller (`extension.rs::
// discovery_config`) uses to populate an `AgentDiscoveryConfig`'s per-scope directory lists.
// -------------------------------------------------------------------------------------------

/// Config-directory segment holding cyrup's per-scope agent/chain state (`.cyrup`, cyrup's analog
/// of pi's `getConfigDirName()` = `.pi`, shared/utils.ts:16,68). A directory containing this
/// segment marks a project root.
const PROJECT_CONFIG_DIR_SEGMENT: &str = ".cyrup";

/// Legacy top-level agents directory segment (pi's `.agents`), honored BOTH at a project root (a
/// lower-precedence project agent read dir, agents.ts:1238) AND at the user home (the "second"
/// user agent dir `os.homedir()/.agents`, agents.ts:1280).
const LEGACY_AGENTS_DIR_SEGMENT: &str = ".agents";

/// Subdirectory under [`PROJECT_CONFIG_DIR_SEGMENT`] holding agent `.md` files.
const AGENTS_SUBDIR: &str = "agents";

/// Subdirectory under [`PROJECT_CONFIG_DIR_SEGMENT`] holding chain files (`.chain.md`/`.chain.json`)
/// — a directory SEPARATE from [`AGENTS_SUBDIR`] (pi `getUserChainDir`/`resolveNearestProjectChainDirs`
/// key each User/Project chain scope on a dedicated `chains/` dir, agents.ts:180,1254, NOT the
/// shared `agents/` dir; only *package*-tier chains co-locate with a package's agents dir, handled
/// separately by [`scan_package_chain_scopes`]).
const CHAINS_SUBDIR: &str = "chains";

/// Walk `cwd` and every ancestor, returning the nearest directory that qualifies as a project root
/// — one holding either a [`PROJECT_CONFIG_DIR_SEGMENT`] (`.cyrup`) config directory OR a legacy
/// [`LEGACY_AGENTS_DIR_SEGMENT`] (`.agents`) directory (a faithful port of pi `findNearestProjectRoot`,
/// agents.ts:511-522). Returns `None` when no ancestor up to the filesystem root qualifies, so a
/// caller run outside any project scans no project-scope directories (pi's `readDirs: []`). Always
/// terminates: the walk stops at the filesystem-root fixpoint where `Path::parent` no longer yields
/// a distinct ancestor (pi's `path.dirname(dir) === dir` guard).
#[must_use]
pub fn find_nearest_project_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        if current.join(PROJECT_CONFIG_DIR_SEGMENT).is_dir()
            || current.join(LEGACY_AGENTS_DIR_SEGMENT).is_dir()
        {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => return None,
        }
    }
}

/// Project-scope agent read directories for `project_root`, **lowest-precedence first** (pi
/// `resolveNearestProjectAgentDirs`, agents.ts:1234-1248): the legacy `<root>/.agents` dir first
/// (included only when it already exists on disk), then the preferred `<root>/.cyrup/agents` dir
/// **last** — always included because it is simultaneously the highest-precedence project read dir
/// under the tier's last-directory-scanned-wins rule (R-SA-002) AND the create/write target
/// (`management::pick_scope_dir` targets the last entry). Scanning a not-yet-created preferred dir
/// yields no agents (an absent dir walks empty, [`walk_agent_dir`]), so this reproduces pi's gated
/// `readDirs` observable read result while still giving management a stable write target.
#[must_use]
pub fn resolve_project_agent_read_dirs(project_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let legacy = project_root.join(LEGACY_AGENTS_DIR_SEGMENT);
    if legacy.is_dir() {
        dirs.push(legacy);
    }
    dirs.push(project_root.join(PROJECT_CONFIG_DIR_SEGMENT).join(AGENTS_SUBDIR));
    dirs
}

/// Project-scope chain read directories for `project_root` (pi `resolveNearestProjectChainDirs`,
/// agents.ts:1250-1259): the single preferred `<root>/.cyrup/chains` dir — a directory SEPARATE
/// from the project agents dir ([`CHAINS_SUBDIR`]).
#[must_use]
pub fn resolve_project_chain_read_dirs(project_root: &Path) -> Vec<PathBuf> {
    vec![project_root.join(PROJECT_CONFIG_DIR_SEGMENT).join(CHAINS_SUBDIR)]
}

/// User-scope agent read directories rooted at `home`, **lowest-precedence first** (pi
/// `discoverAgents` `userDirOld`/`userDirNew`, agents.ts:1279-1280,1301-1302): the primary
/// `<home>/.cyrup/agents` dir first (always included — the create/write fallback target), then the
/// legacy `<home>/.agents` "second" user dir **last** (included only when it already exists, so it
/// wins the last-directory-scanned reduce only once the user actually populates it, matching pi's
/// `fs.existsSync(userDirNew) ? userDirNew : userDirOld` write-target selection over the same
/// last-entry rule). [`EXTRA_AGENT_DIRS_ENV_VAR`] entries are prepended ahead of BOTH by
/// [`AgentDiscoveryConfig::with_env_extras`] (R-SA-003).
#[must_use]
pub fn resolve_user_agent_read_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(PROJECT_CONFIG_DIR_SEGMENT).join(AGENTS_SUBDIR)];
    let legacy = home.join(LEGACY_AGENTS_DIR_SEGMENT);
    if legacy.is_dir() {
        dirs.push(legacy);
    }
    dirs
}

/// User-scope chain read directories rooted at `home` (pi `getUserChainDir`, agents.ts:180): the
/// single `<home>/.cyrup/chains` dir — again SEPARATE from the user agents dir ([`CHAINS_SUBDIR`]).
#[must_use]
pub fn resolve_user_chain_read_dirs(home: &Path) -> Vec<PathBuf> {
    vec![home.join(PROJECT_CONFIG_DIR_SEGMENT).join(CHAINS_SUBDIR)]
}

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
    /// alphabetical-by-filename), ordered lowest-precedence-first so the LAST entry wins a
    /// same-name collision under the User tier's last-directory-scanned-wins reduce (R-SA-002).
    /// Ordinary caller-supplied entries — [`EXTRA_AGENT_DIRS_ENV_VAR`] entries (R-SA-003) are
    /// **prepended ahead of** this list by [`AgentDiscoveryConfig::with_env_extras`] /
    /// [`resolve_extra_agent_dirs`] (extras are the lowest-precedence stream) rather than being
    /// folded in silently by this struct's own constructor, so a caller inspecting
    /// `user_agent_dirs` after construction sees exactly what it explicitly set unless it
    /// explicitly opted into the env-var extension.
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
    /// The user- and project-scope `subagents` settings blocks (R-SA-009/010/011/012), carried
    /// **UNFLATTENED** with each scope's own `settings.json` path (Tier 7). `merge.rs` resolves
    /// project-beats-user precedence at application time and records the true winning scope + path
    /// in each overridden agent's provenance — a malformed value at either scope is the caller's
    /// problem to have already surfaced via [`read_subagent_settings_file`] /
    /// [`load_layered_override_settings`] before constructing a valid config.
    pub override_settings: LayeredOverrideSettings,
}

impl AgentDiscoveryConfig {
    /// **Prepend** [`EXTRA_AGENT_DIRS_ENV_VAR`]'s entries (if the variable is set and non-empty)
    /// *ahead of* `user_agent_dirs`, in the order [`std::env::split_paths`] yields them — i.e.
    /// *before* any ordinary user directories already present, so R-SA-002's
    /// last-directory-scanned-wins User tier rule lets an ordinary user directory's same-named
    /// agent win over an extra directory's, matching pi-subagents'
    /// `[...userAgentsExtra, ...userAgentsOld, ...userAgentsNew]` placement for
    /// `PI_SUBAGENT_EXTRA_AGENT_DIRS` (agents.ts:1300-1304). Extras are therefore the
    /// **lowest-precedence** User-tier stream (a read-only fallback), never an override of the
    /// user's own agents. A no-op when the variable is absent or empty.
    #[must_use]
    pub fn with_env_extras(self) -> Self {
        let extras = resolve_extra_agent_dirs(|key| std::env::var(key).ok());
        self.with_prepended_user_extras(extras)
    }

    /// The pure core of [`AgentDiscoveryConfig::with_env_extras`], parameterized over the already-
    /// resolved `extras` list so the extras-first ordering can be exercised deterministically in
    /// unit tests without touching real process environment state (this crate is
    /// `#![forbid(unsafe_code)]`, so tests never call `std::env::set_var`). Prepends `extras`
    /// ahead of any existing `user_agent_dirs`; a no-op when `extras` is empty.
    #[must_use]
    fn with_prepended_user_extras(mut self, extras: Vec<PathBuf>) -> Self {
        if !extras.is_empty() {
            let mut combined = extras;
            combined.append(&mut self.user_agent_dirs);
            self.user_agent_dirs = combined;
        }
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
    let Some(value) = raw else {
        return Ok(SubagentSettings::default());
    };
    let mut settings: SubagentSettings = serde_json::from_value(value.clone())
        .map_err(|e| SubagentError::MalformedSettings(e.to_string()))?;
    // pi `readSubagentSettings` (`agents.ts:695-702`): `defaultModel` must be a NON-EMPTY string;
    // an empty/whitespace-only value is malformed and MUST abort (R-SA-009). Stored trimmed so a
    // stray-whitespace value resolves to the same model everywhere it is consulted.
    if let Some(dm) = settings.default_model.as_ref() {
        let trimmed = dm.trim();
        if trimmed.is_empty() {
            return Err(SubagentError::MalformedSettings(
                "invalid 'defaultModel'; expected a non-empty string".to_string(),
            ));
        }
        if trimmed.len() != dm.len() {
            settings.default_model = Some(trimmed.to_string());
        }
    }
    Ok(settings)
}

/// Read one on-disk `settings.json` file and extract its typed `subagents` block (pi
/// `readSubagentSettings`, `agents.ts:672-714`). Mirrors pi's outcome taxonomy:
/// - an **absent** file yields the all-default [`SubagentSettings`] (the common "no customization"
///   case — NOT an error);
/// - a file that cannot be read, does not parse as JSON, or is not a JSON object aborts with
///   [`SubagentError::MalformedSettings`] (R-SA-009), the offending path named in the message;
/// - a present-but-non-object `subagents` value yields the all-default settings (pi returns its
///   `EMPTY_SUBAGENT_SETTINGS` for that shape rather than throwing);
/// - a present **object** `subagents` value is parsed (and its `defaultModel` validated) via
///   [`parse_subagent_settings`].
pub fn read_subagent_settings_file(path: &Path) -> Result<SubagentSettings, SubagentError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SubagentSettings::default());
        }
        Err(e) => {
            return Err(SubagentError::MalformedSettings(format!(
                "Failed to read settings file '{}': {e}",
                path.display()
            )));
        }
    };
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        SubagentError::MalformedSettings(format!(
            "Failed to parse settings file '{}': {e}",
            path.display()
        ))
    })?;
    if !value.is_object() {
        return Err(SubagentError::MalformedSettings(format!(
            "Settings file '{}' must contain a JSON object.",
            path.display()
        )));
    }
    // A `subagents` value that is present but not an object is treated as "no customization" (pi
    // returns EMPTY_SUBAGENT_SETTINGS), so only forward an object value to the strict parser.
    let subagents = value.get("subagents").filter(|v| v.is_object());
    parse_subagent_settings(subagents).map_err(|e| match e {
        // Prefix the originating file path so R-SA-009's surfaced error names the offending file
        // (pi includes the settings path in every malformed-settings message).
        SubagentError::MalformedSettings(msg) => SubagentError::MalformedSettings(format!(
            "settings file '{}': {msg}",
            path.display()
        )),
        other => other,
    })
}

/// Read and layer the user-scope and project-scope `settings.json` `subagents` blocks into one
/// resolved [`SubagentSettings`] (pi's two `readSubagentSettings(...)` calls plus the
/// project-over-user resolution `discoverAgents` performs inline, `agents.ts:1282-1286` +
/// `716-728`). Resolution (R-SA-012/R-SA-133):
/// - per-agent `agentOverrides.<name>`: the **project** entry wins over a same-named **user** entry
///   (only one is ever applied to a given agent name);
/// - `defaultModel` / `disableBuiltins` / `disableThinking`: the **project** value wins when
///   present, else the **user** value — so a project `disableBuiltins: false` re-enables what a user
///   `disableBuiltins: true` disabled, and a project `defaultModel` overrides a user one.
///
/// A malformed file at EITHER scope aborts discovery (R-SA-009).
pub fn load_layered_subagent_settings(
    user_settings_path: &Path,
    project_settings_path: Option<&Path>,
) -> Result<SubagentSettings, SubagentError> {
    let user = read_subagent_settings_file(user_settings_path)?;
    let project = match project_settings_path {
        Some(p) => read_subagent_settings_file(p)?,
        None => SubagentSettings::default(),
    };
    Ok(resolve_layered_subagent_settings(user, project))
}

/// Read the user- and project-scope `settings.json` `subagents` blocks into a [`LayeredOverrideSettings`],
/// carrying BOTH scopes UNFLATTENED plus each scope's own path (Tier 7). This is what `merge.rs`
/// consumes so it can resolve project-beats-user precedence at APPLICATION time and record the real
/// winning scope + settings-file path in each overridden agent's provenance — unlike
/// [`load_layered_subagent_settings`], which pre-flattens the two scopes (losing which scope an
/// override came from) and is retained only as a scalar-resolution utility.
///
/// A malformed file at EITHER scope aborts discovery (R-SA-009), the offending path named in the
/// message (via [`read_subagent_settings_file`]). An absent file at either scope is the common
/// no-customization case and yields that scope's all-default [`SubagentSettings`] (not an error) —
/// but its path is still recorded, mirroring pi's non-null `userSettingsPath`/`projectSettingsPath`.
pub fn load_layered_override_settings(
    user_settings_path: &Path,
    project_settings_path: Option<&Path>,
) -> Result<LayeredOverrideSettings, SubagentError> {
    let user = read_subagent_settings_file(user_settings_path)?;
    let project = match project_settings_path {
        Some(p) => read_subagent_settings_file(p)?,
        None => SubagentSettings::default(),
    };
    Ok(LayeredOverrideSettings {
        user,
        project,
        user_settings_path: user_settings_path.to_path_buf(),
        project_settings_path: project_settings_path.map(Path::to_path_buf),
    })
}

/// Pure user+project [`SubagentSettings`] resolution (see [`load_layered_subagent_settings`]),
/// factored out so it can be exercised deterministically without touching the filesystem. Project
/// wins over user on every scalar (via [`Option::or`], project first) and per-agent override name
/// (project entries overwrite user entries in the merged map).
fn resolve_layered_subagent_settings(
    user: SubagentSettings,
    project: SubagentSettings,
) -> SubagentSettings {
    let mut overrides = user.overrides;
    for (name, delta) in project.overrides {
        overrides.insert(name, delta);
    }
    SubagentSettings {
        overrides,
        default_model: project.default_model.or(user.default_model),
        disable_builtins: project.disable_builtins.or(user.disable_builtins),
        disable_thinking: project.disable_thinking.or(user.disable_thinking),
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

/// Enumerate the on-disk directories every installed package contributes as **chain** scopes
/// (R-SA-015/020), in the same fixed project-then-global order (and behind the identical fail-closed
/// `trusted_project` gate) [`scan_package_agents`] uses. Chains share the agents directory (a
/// package's `.chain.md`/`.chain.json` files live alongside its `.md` agent files, exactly as
/// pi-subagents' `extractSubagentPathsFromPackageRoot` returns both an `agents` and a `chains` list
/// rooted at the same package subagent directory, `agents.ts:387-436`), so each manifest-declared
/// `agents` directory entry is also a chain scope. Only **directory** manifest entries are returned
/// (a chain scope is scanned via [`crate::discovery::chains::scan_chain_dir`], which walks a
/// directory root); an individual-file `agents` entry contributes no chain scope of its own here.
///
/// The returned `(dir, AgentSource::Package)` pairs are appended to the User/Project chain scopes
/// [`run_discovery`] scans, so a package-provided chain is discovered at `AgentSource::Package`
/// scope and — per R-SA-015 — survives alongside any same-named User/Project chain rather than being
/// merged away.
pub fn scan_package_chain_scopes(cfg: &AgentDiscoveryConfig) -> Vec<(PathBuf, AgentSource)> {
    let mut ordered: Vec<&InstalledPackage> = cfg.installed_packages.packages.iter().collect();
    ordered.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });

    let mut scopes = Vec::new();
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
            if agent_entry.is_dir() {
                scopes.push((agent_entry.clone(), AgentSource::Package));
            }
        }
    }
    scopes
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
/// (R-SA-001/002/004/009/010/011/012/020/021), plus cross-scope chain discovery (R-SA-015). The
/// `scope` parameter narrows the User-vs-Project axis **within the scan, before the merge** (see
/// the tier-zeroing comment in the body): [`discover_agents_all`] always passes
/// [`AgentReadScope::Both`] (the full management/introspection view), while [`discover_agents`]
/// forwards its caller's `scope_override`. Both entry points share this one pipeline and otherwise
/// differ only in which [`management::AgentVisibility`] filter they apply to the result afterward —
/// so they can never diverge on anything except R-SA-013's disabled-visibility policy and the
/// scope narrowing itself.
fn run_discovery(
    cfg: &AgentDiscoveryConfig,
    scope: AgentReadScope,
) -> Result<AgentDiscoveryResult, SubagentError> {
    let builtin = scan_builtin_agents(cfg);
    let package = scan_package_agents(cfg);
    // Scope-filtered discovery (R-SA-013; pi `discoverAgents` + `mergeAgentsForScope`,
    // agents.ts:1300-1320, agent-selection.ts): narrow the User-vs-Project axis **within each
    // tier, BEFORE the merge** — never merge-all-then-filter. Zeroing the excluded tier's
    // candidates up front means a Project agent can never dedup-shadow a same-named User agent out
    // of the User-scope view (the bug a post-merge scope filter has: the merge would keep only the
    // Project agent, then the filter would drop it for being Project-sourced, erasing the User
    // agent that still exists on disk). Builtin/Package tiers are orthogonal to this axis and are
    // always scanned (an `AgentReadScope` narrows only User-vs-Project, mirroring
    // `mergeAgentsForScope`, which always seeds the map with builtin+package then adds only the
    // in-scope User/Project agents).
    let user = if scope == AgentReadScope::Project {
        Vec::new()
    } else {
        walk_agent_dirs(&cfg.user_agent_dirs, AgentSource::User)
    };
    let project = if scope == AgentReadScope::User {
        Vec::new()
    } else {
        walk_agent_dirs(&cfg.project_agent_dirs, AgentSource::Project)
    };

    let tiers = merge::TieredAgents {
        builtin,
        package,
        user,
        project,
    };
    let merged = merge::discover_and_merge(tiers, &cfg.override_settings)?;

    let mut agents: Vec<AgentDefinition> = merged.into_values().collect();
    // Deterministic output order (by runtime name) independent of the underlying `HashMap`'s
    // iteration order, so repeated calls over the same on-disk state are stable for callers/tests
    // — mirrors `discovery::chains::scan_chain_dir`'s identical "sort by name before returning"
    // convention.
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let mut chain_scopes: Vec<(PathBuf, AgentSource)> = Vec::new();
    // Package-scope chain scopes first (R-SA-020, chains-share-agents-dir), so a package-provided
    // chain is discovered at `AgentSource::Package` scope; per R-SA-015 chains are never merged
    // across scopes, so this ordering only fixes the deterministic scan order, never which chain
    // "wins" a same-name collision (all survive, tagged with their own source).
    chain_scopes.extend(scan_package_chain_scopes(cfg));
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
    // Management/introspection is always the full Both-scope view (pi `discoverAgentsAll` loads
    // every tier unconditionally, agents.ts:1325-1422); scope narrowing is a delegation-only
    // concern applied by `discover_agents`.
    let mut result = run_discovery(cfg, AgentReadScope::Both)?;
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
/// `scope_override` narrows the **User-vs-Project axis at scan time, within each tier, before the
/// merge** (forwarded to [`run_discovery`]'s `scope` parameter; `None` uses the default
/// [`AgentReadScope::Both`], i.e. no narrowing): [`AgentReadScope::User`] scans no Project agent
/// dirs (so a Project agent can never dedup-shadow a same-named User agent out of the result — the
/// bug a post-merge filter would have), [`AgentReadScope::Project`] scans no User agent dirs, and
/// Builtin/Package tiers are scanned under either named scope (they are orthogonal to the
/// User-vs-Project axis — R-SA data model's own `AgentReadScope` doc: "a read filter... distinct
/// from `AgentSource`", mirroring pi `mergeAgentsForScope` seeding builtin+package unconditionally).
/// This is the correction of the earlier merge-all-then-filter approach; the narrowing is now a
/// per-tier scan decision, not a filter over an already-merged result.
pub fn discover_agents(
    cfg: &AgentDiscoveryConfig,
    scope_override: Option<AgentReadScope>,
) -> Result<AgentDiscoveryResult, SubagentError> {
    let mut result = run_discovery(cfg, scope_override.unwrap_or_default())?;
    result.agents = AgentVisibility::delegation(&result.agents)
        .into_iter()
        .cloned()
        .collect();
    result.chains = ChainVisibility::delegation(&result.chains)
        .into_iter()
        .cloned()
        .collect();
    Ok(result)
}

/// Resolve one saved chain by exact name (R-SA-008-style equality) across every discovered scope,
/// applying pi's run-time cross-scope precedence: on a same-name collision the highest-precedence
/// scope wins, in the order Project > User > Package > Builtin. This mirrors pi's
/// `discoverSavedChains` (slash-commands.ts:171-177,1040), which feeds `discoverAgentsAll`'s
/// `[...package, ...user, ...project]` chain array (agents.ts:1409-1413) into a name-keyed `Map`
/// whose last write wins — so the project-scope chain (emitted last) is the one actually run.
/// cyrup deliberately RETAINS every scope's same-named chain in [`AgentDiscoveryResult::chains`]
/// (R-SA-015, never merged across scopes) for management/doctor visibility, so this run-time
/// precedence MUST be applied here at resolution time rather than by the discovery walk. Fixes the
/// prior first-match bug, which returned the User chain (emitted before the Project one in scan
/// order) and so let a User chain shadow a same-named Project chain.
#[must_use]
pub fn resolve_chain_by_name<'a>(
    chains: &'a [ChainDefinition],
    name: &str,
) -> Option<&'a ChainDefinition> {
    chains
        .iter()
        .filter(|c| c.name == name)
        .max_by_key(|c| chain_run_precedence(c.source))
}

/// Run-time precedence rank for a chain's source scope (**higher wins**), consulted only by
/// [`resolve_chain_by_name`]. `Project` outranks `User` outranks `Package` outranks `Builtin` —
/// the last-write-wins ordering pi's `discoverSavedChains` map encodes via its
/// package→user→project append order. `max_by_key` returns the LAST element among equal ranks, so
/// two same-source same-name chains resolve to the one emitted later in scan order, matching pi's
/// within-scope last-write-wins `Map.set`.
fn chain_run_precedence(source: AgentSource) -> u8 {
    match source {
        AgentSource::Builtin => 0,
        AgentSource::Package => 1,
        AgentSource::User => 2,
        AgentSource::Project => 3,
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
    fn with_prepended_user_extras_puts_extras_first_so_primary_user_dirs_win_last() {
        // pi loads PI_SUBAGENT_EXTRA_AGENT_DIRS agents FIRST (lowest precedence), then the user's
        // own agent dirs (agents.ts:1300-1304), so under the User tier's last-directory-scanned-
        // wins reduce (R-SA-002) the user's own agent wins over a bundled extra-dir agent of the
        // same name. A prior bug appended extras AFTER the user dirs, inverting this.
        let primary = PathBuf::from("/home/user/.cyrup/agents");
        let extra_a = PathBuf::from("/nix/store/pkg-a/agents");
        let extra_b = PathBuf::from("/nix/store/pkg-b/agents");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![primary.clone()],
            ..AgentDiscoveryConfig::default()
        }
        .with_prepended_user_extras(vec![extra_a.clone(), extra_b.clone()]);

        // Extras precede the primary user dir, so the primary is scanned LAST and wins collisions.
        assert_eq!(cfg.user_agent_dirs, vec![extra_a, extra_b, primary]);
    }

    #[test]
    fn with_prepended_user_extras_is_a_no_op_when_no_extras() {
        let primary = PathBuf::from("/home/user/.cyrup/agents");
        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![primary.clone()],
            ..AgentDiscoveryConfig::default()
        }
        .with_prepended_user_extras(Vec::new());
        assert_eq!(cfg.user_agent_dirs, vec![primary]);
    }

    #[test]
    fn extra_dir_agent_does_not_override_a_same_named_user_agent() {
        // The required end-to-end behavior: a bundled extra-dir agent must NOT shadow the user's
        // own same-named agent. `with_prepended_user_extras` orders [extra, primary]; the User
        // tier's last-seen-wins reduce then lets `primary/scout` win over `extra/scout`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let extra = tmp.path().join("extra");
        let primary = tmp.path().join("primary");
        write_agent(&extra, "scout.md", "scout", "extra scout");
        write_agent(&primary, "scout.md", "scout", "primary scout");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![primary.clone()],
            ..AgentDiscoveryConfig::default()
        }
        .with_prepended_user_extras(vec![extra]);

        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let scouts: Vec<&AgentDefinition> =
            result.agents.iter().filter(|a| a.name == "scout").collect();
        assert_eq!(scouts.len(), 1, "the same-named agent must be deduped, not duplicated");
        assert_eq!(
            scouts[0].description, "primary scout",
            "the user's own dir must win over an extra dir"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Directory topology: upward project-root search, legacy `.agents`, second user dir,
    // separate chain dirs (pi findNearestProjectRoot / resolveNearestProject*Dirs).
    // -----------------------------------------------------------------------------------------

    #[test]
    fn find_nearest_project_root_walks_up_to_the_dir_holding_the_config_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".cyrup").join("agents")).expect("mkdir .cyrup/agents");
        let nested = root.join("crates").join("thing").join("src");
        std::fs::create_dir_all(&nested).expect("mkdir nested");
        assert_eq!(find_nearest_project_root(&nested).as_deref(), Some(root));
    }

    #[test]
    fn find_nearest_project_root_honors_legacy_dot_agents_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agents")).expect("mkdir legacy .agents");
        let nested = root.join("sub");
        std::fs::create_dir_all(&nested).expect("mkdir sub");
        assert_eq!(find_nearest_project_root(&nested).as_deref(), Some(root));
    }

    #[test]
    fn find_nearest_project_root_ignores_dirs_without_project_markers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("x").join("y");
        std::fs::create_dir_all(&nested).expect("mkdir");
        // With no `.cyrup`/`.agents` marker anywhere inside the temp tree, the search must not
        // resolve to any directory within it (it either finds nothing, or a marker far outside
        // tmp — never a false-positive on an unmarked temp dir).
        match find_nearest_project_root(&nested) {
            None => {}
            Some(found) => assert!(
                !found.starts_with(tmp.path()),
                "must not treat an unmarked temp dir as a project root, got {found:?}"
            ),
        }
    }

    #[test]
    fn project_agent_read_dirs_put_preferred_last_and_include_existing_legacy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".agents")).expect("mkdir legacy");
        // Preferred `.cyrup/agents` need not exist yet — it is still included (write target), last.
        let dirs = resolve_project_agent_read_dirs(root);
        assert_eq!(
            dirs,
            vec![root.join(".agents"), root.join(".cyrup").join("agents")],
            "legacy first (lower precedence), preferred last (wins last-seen + is the write target)"
        );
    }

    #[test]
    fn project_agent_read_dirs_omit_absent_legacy_but_always_keep_preferred() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let dirs = resolve_project_agent_read_dirs(root);
        assert_eq!(dirs, vec![root.join(".cyrup").join("agents")]);
    }

    #[test]
    fn user_agent_read_dirs_put_second_dot_agents_dir_last_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".agents")).expect("mkdir second user dir");
        let dirs = resolve_user_agent_read_dirs(home);
        assert_eq!(
            dirs,
            vec![home.join(".cyrup").join("agents"), home.join(".agents")],
            "primary first (write fallback), second `.agents` last (wins once it exists)"
        );
    }

    #[test]
    fn user_agent_read_dirs_omit_absent_second_dir_but_always_keep_primary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let dirs = resolve_user_agent_read_dirs(home);
        assert_eq!(dirs, vec![home.join(".cyrup").join("agents")]);
    }

    #[test]
    fn chain_read_dirs_are_separate_from_agent_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            resolve_project_chain_read_dirs(tmp.path()),
            vec![tmp.path().join(".cyrup").join("chains")]
        );
        assert_eq!(
            resolve_user_chain_read_dirs(tmp.path()),
            vec![tmp.path().join(".cyrup").join("chains")]
        );
    }

    #[test]
    fn a_cwd_nested_below_the_project_root_finds_project_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write_agent(
            &root.join(".cyrup").join("agents"),
            "builder.md",
            "builder",
            "the project builder",
        );
        let nested = root.join("crates").join("thing").join("src");
        std::fs::create_dir_all(&nested).expect("mkdir nested cwd");

        // Replicate `discovery_config`'s topology resolution for a cwd deep inside the project.
        let project_root =
            find_nearest_project_root(&nested).expect("nested cwd resolves to the project root");
        assert_eq!(project_root.as_path(), root);
        let cfg = AgentDiscoveryConfig {
            project_agent_dirs: resolve_project_agent_read_dirs(&project_root),
            ..AgentDiscoveryConfig::default()
        };
        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        assert!(
            result.agents.iter().any(|a| a.name == "builder"),
            "a cwd nested below the project root must still discover project agents"
        );
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-015 chain run-resolution: a project chain wins a same-name collision (bug fix — was
    // resolving to the User chain).
    // -----------------------------------------------------------------------------------------

    fn write_json_chain(dir: &Path, file_name: &str, name: &str, description: &str) {
        std::fs::create_dir_all(dir).expect("mkdir chain dir");
        std::fs::write(
            dir.join(file_name),
            format!("{{\"name\":\"{name}\",\"description\":\"{description}\",\"chain\":[]}}"),
        )
        .expect("write chain");
    }

    #[test]
    fn a_project_chain_wins_a_name_collision_with_a_user_chain() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_chains = tmp.path().join("user-chains");
        let project_chains = tmp.path().join("project-chains");
        write_json_chain(&user_chains, "release.chain.json", "release", "user release");
        write_json_chain(&project_chains, "release.chain.json", "release", "project release");

        let cfg = AgentDiscoveryConfig {
            user_chain_dirs: vec![user_chains],
            project_chain_dirs: vec![project_chains],
            ..AgentDiscoveryConfig::default()
        };
        let result = discover_agents(&cfg, None).expect("discovery succeeds");

        // R-SA-015: both scopes' same-named chains are RETAINED in the result...
        assert_eq!(
            result.chains.iter().filter(|c| c.name == "release").count(),
            2,
            "both the user and project chains must be retained (never merged across scopes)"
        );
        // ...but run-resolution picks the PROJECT chain (pi discoverSavedChains last-wins map).
        let picked = resolve_chain_by_name(&result.chains, "release").expect("release resolves");
        assert_eq!(
            picked.source,
            AgentSource::Project,
            "the project chain must win the name collision, not the user chain"
        );
        assert_eq!(picked.description, "project release");
    }

    #[test]
    fn resolve_chain_by_name_returns_none_for_an_unknown_name() {
        assert!(resolve_chain_by_name(&[], "nope").is_none());
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
        // `agentOverrides` must be an object keyed by agent name; a plain string is malformed.
        let raw = serde_json::json!({ "agentOverrides": "not-an-object" });
        let result = parse_subagent_settings(Some(&raw));
        assert!(matches!(result, Err(SubagentError::MalformedSettings(_))));
    }

    #[test]
    fn parse_subagent_settings_rejects_empty_default_model() {
        // pi `agent-overrides.test.ts:215-229`: an empty `defaultModel` is malformed.
        let raw = serde_json::json!({ "defaultModel": "  " });
        let result = parse_subagent_settings(Some(&raw));
        assert!(matches!(result, Err(SubagentError::MalformedSettings(msg)) if msg.contains("defaultModel")));
    }

    #[test]
    fn parse_subagent_settings_reads_pi_agent_overrides_key() {
        let raw = serde_json::json!({
            "agentOverrides": { "reviewer": { "model": "openai/gpt-5.4" } }
        });
        let settings = parse_subagent_settings(Some(&raw)).expect("valid");
        let reviewer = settings.overrides.get("reviewer").expect("reviewer override present");
        assert_eq!(
            reviewer.model,
            crate::discovery::types::OverrideField::Value("openai/gpt-5.4".to_string())
        );
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
    // C2 wiring: read real pi-shaped settings.json from disk + layer user/project (R-SA-012/133)
    // -----------------------------------------------------------------------------------------

    fn write_settings(path: &Path, value: serde_json::Value) {
        std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir settings");
        std::fs::write(path, serde_json::to_vec_pretty(&value).expect("serialize")).expect("write");
    }

    #[test]
    fn read_subagent_settings_file_absent_is_default_not_error() {
        let settings = read_subagent_settings_file(Path::new("/no/such/settings.json"))
            .expect("absent file is not an error");
        assert!(settings.overrides.is_empty());
        assert_eq!(settings.default_model, None);
    }

    #[test]
    fn read_subagent_settings_file_reads_the_subagents_block_of_a_real_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        write_settings(
            &path,
            serde_json::json!({
                "theme": "dark",
                "subagents": { "defaultModel": "acme/model-x", "disableBuiltins": true }
            }),
        );
        let settings = read_subagent_settings_file(&path).expect("read");
        assert_eq!(settings.default_model.as_deref(), Some("acme/model-x"));
        assert_eq!(settings.disable_builtins, Some(true));
    }

    #[test]
    fn read_subagent_settings_file_surfaces_malformed_json_with_the_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, b"{\"subagents\":").expect("write malformed");
        let result = read_subagent_settings_file(&path);
        assert!(
            matches!(&result, Err(SubagentError::MalformedSettings(msg)) if msg.contains("settings.json")),
            "expected a malformed-settings error naming the file, got {result:?}"
        );
    }

    #[test]
    fn layered_settings_resolve_project_over_user_scalars_and_overrides() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user/settings.json");
        let project = tmp.path().join("proj/settings.json");
        write_settings(
            &user,
            serde_json::json!({
                "subagents": {
                    "defaultModel": "user/default",
                    "agentOverrides": { "reviewer": { "model": "user/reviewer-model" } }
                }
            }),
        );
        write_settings(
            &project,
            serde_json::json!({
                "subagents": {
                    "defaultModel": "project/default",
                    "agentOverrides": { "reviewer": { "model": "project/reviewer-model" } }
                }
            }),
        );

        let resolved = load_layered_subagent_settings(&user, Some(&project)).expect("layer");
        assert_eq!(resolved.default_model.as_deref(), Some("project/default"));
        assert_eq!(
            resolved.overrides.get("reviewer").expect("reviewer").model,
            crate::discovery::types::OverrideField::Value("project/reviewer-model".to_string()),
            "project override wins over the same-named user override"
        );
    }

    #[test]
    fn layered_settings_project_disable_builtins_false_re_enables_user_true() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user/settings.json");
        let project = tmp.path().join("proj/settings.json");
        write_settings(&user, serde_json::json!({ "subagents": { "disableBuiltins": true } }));
        write_settings(&project, serde_json::json!({ "subagents": { "disableBuiltins": false } }));

        let resolved = load_layered_subagent_settings(&user, Some(&project)).expect("layer");
        assert_eq!(
            resolved.disable_builtins,
            Some(false),
            "a project `disableBuiltins: false` must re-enable what a user `true` disabled"
        );
    }

    /// Build a real four-tier config: a temp BUILTIN dir with `reviewer`/`worker` personas, plus a
    /// real user+project `settings.json` on disk, exercising the full read -> layer -> merge ->
    /// override -> discover pipeline the way pi's `agent-overrides.test.ts` does end-to-end.
    fn e2e_config_with_builtins_and_settings(
        builtin_dir: &Path,
        user_settings: &Path,
        project_settings: &Path,
    ) -> AgentDiscoveryConfig {
        write_agent(builtin_dir, "reviewer.md", "reviewer", "reviews things");
        write_agent(builtin_dir, "worker.md", "worker", "does work");
        let override_settings = load_layered_override_settings(user_settings, Some(project_settings))
            .expect("layered settings load");
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(builtin_dir.to_path_buf()),
            override_settings,
            ..AgentDiscoveryConfig::default()
        }
    }

    #[test]
    fn e2e_agent_overrides_reviewer_model_changes_the_reviewer() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user/settings.json");
        let project = tmp.path().join("proj/settings.json");
        write_settings(&user, serde_json::json!({}));
        write_settings(
            &project,
            serde_json::json!({
                "subagents": { "agentOverrides": { "reviewer": { "model": "openai/gpt-5.4" } } }
            }),
        );
        let cfg = e2e_config_with_builtins_and_settings(&tmp.path().join("builtins"), &user, &project);

        let result = discover_agents(&cfg, None).expect("discovery");
        let reviewer = result.agents.iter().find(|a| a.name == "reviewer").expect("reviewer present");
        assert_eq!(reviewer.source, AgentSource::Builtin);
        assert_eq!(reviewer.model, Some("openai/gpt-5.4".into()));
        // A sibling builtin the override never named keeps its (absent) model.
        let worker = result.agents.iter().find(|a| a.name == "worker").expect("worker present");
        assert_eq!(worker.model, None);
    }

    #[test]
    fn e2e_default_model_fills_all_agents_missing_a_model() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user/settings.json");
        let project = tmp.path().join("proj/settings.json");
        write_settings(&user, serde_json::json!({ "subagents": { "defaultModel": "deepseek-v4-flash" } }));
        write_settings(&project, serde_json::json!({}));
        let cfg = e2e_config_with_builtins_and_settings(&tmp.path().join("builtins"), &user, &project);

        let result = discover_agents(&cfg, None).expect("discovery");
        for name in ["reviewer", "worker"] {
            let a = result.agents.iter().find(|a| a.name == name).expect("present");
            assert_eq!(a.model, Some("deepseek-v4-flash".into()), "{name} filled from defaultModel");
            assert_eq!(a.model_source, Some(types::AgentModelSourceInfo::SettingsDefault));
        }
    }

    #[test]
    fn e2e_disable_builtins_hides_builtins_from_delegation_but_not_management() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user/settings.json");
        let project = tmp.path().join("proj/settings.json");
        write_settings(&user, serde_json::json!({ "subagents": { "disableBuiltins": true } }));
        write_settings(&project, serde_json::json!({}));
        let cfg = e2e_config_with_builtins_and_settings(&tmp.path().join("builtins"), &user, &project);

        // Delegation view excludes disabled builtins entirely.
        let delegation = discover_agents(&cfg, None).expect("discovery");
        assert!(
            !delegation.agents.iter().any(|a| a.name == "reviewer"),
            "disableBuiltins must hide reviewer from the delegation view"
        );
        assert!(!delegation.agents.iter().any(|a| a.name == "worker"));

        // Management view still lists them (so a user can re-enable), marked disabled.
        let mgmt = discover_agents_all(&cfg).expect("discovery-all");
        let reviewer = mgmt.agents.iter().find(|a| a.name == "reviewer").expect("mgmt lists disabled");
        assert_eq!(reviewer.disabled, Some(true));
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
        write_agent(tmp.path(), "on.md", "on-agent", "d");
        write_agent(tmp.path(), "off.md", "off-agent", "d");

        // pi disables a custom agent ONLY via a settings override (`agentOverrides.<name>.disabled`);
        // a frontmatter `disabled:` line is an ignored extra field, never the honored flag. Disable
        // "off-agent" through a project-scope override (Tier 7 two-scope path), then assert the
        // delegation view excludes it while the management view still would not.
        let project_settings = tmp.path().join("proj-settings.json");
        write_settings(
            &project_settings,
            serde_json::json!({
                "subagents": { "agentOverrides": { "off-agent": { "disabled": true } } }
            }),
        );
        let mut cfg = base_config(tmp.path());
        cfg.override_settings = load_layered_override_settings(
            &tmp.path().join("user-settings.json"),
            Some(&project_settings),
        )
        .expect("layered settings");

        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"on-agent"));
        assert!(
            !names.contains(&"off-agent"),
            "delegation view must exclude a settings-disabled agent"
        );
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

    #[test]
    fn scope_filtered_within_tier_does_not_let_project_erase_a_same_named_user_agent() {
        // The scope-filtered-discovery bug: a same-named agent exists in BOTH the user and project
        // tiers. A merge-all-then-filter implementation would keep only the Project agent (project
        // wins the merge), then a post-merge User-scope filter would drop it for being Project-
        // sourced — erasing the User agent from the User-scope view even though it exists on disk.
        // Filtering WITHIN each tier (zeroing the Project tier before the merge) keeps the User one.
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        write_agent(&user_dir, "reviewer.md", "reviewer", "the user reviewer");
        write_agent(&project_dir, "reviewer.md", "reviewer", "the project reviewer");

        let cfg = AgentDiscoveryConfig {
            user_agent_dirs: vec![user_dir],
            project_agent_dirs: vec![project_dir],
            ..AgentDiscoveryConfig::default()
        };

        // User scope: the user reviewer must survive (not be erased by the project one).
        let user_view = discover_agents(&cfg, Some(AgentReadScope::User)).expect("discovery");
        let reviewer = user_view
            .agents
            .iter()
            .find(|a| a.name == "reviewer")
            .expect("the user reviewer must remain visible under User scope");
        assert_eq!(reviewer.source, AgentSource::User);
        assert_eq!(reviewer.description, "the user reviewer");

        // Project scope: symmetrically, the project reviewer is the one seen.
        let project_view = discover_agents(&cfg, Some(AgentReadScope::Project)).expect("discovery");
        let reviewer = project_view
            .agents
            .iter()
            .find(|a| a.name == "reviewer")
            .expect("the project reviewer must be visible under Project scope");
        assert_eq!(reviewer.source, AgentSource::Project);

        // Both scope: project wins the merge (R-SA-001), a single deduped entry.
        let both_view = discover_agents(&cfg, Some(AgentReadScope::Both)).expect("discovery");
        let reviewers: Vec<&AgentDefinition> =
            both_view.agents.iter().filter(|a| a.name == "reviewer").collect();
        assert_eq!(reviewers.len(), 1);
        assert_eq!(reviewers[0].source, AgentSource::Project);
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
            "{\"name\":\"release\",\"description\":\"d\",\"chain\":[]}",
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
