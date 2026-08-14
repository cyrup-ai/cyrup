//! Integration test: `discovery::discover_agents`/`discover_agents_all` end to end against a
//! real fixture directory tree spanning all four scopes (func-SA §5.1; arch-SA §6.2; task R-SA-
//! 003/007/016/017/022 assignment's own "integration test exercising the full discover_agents()
//! path end to end against a real fixture directory tree spanning all four scopes" requirement).
//!
//! No mocking of filesystem/package state anywhere in this file (matching this codebase's
//! standing "never mock subprocess/git/filesystem behavior in tests" convention): every scope
//! (Builtin/Package/User/Project) is a real directory tree under a real `tempfile::tempdir()`,
//! the "installed package" is a real `PackageSource::Path` install (cyrup-resources' own
//! local-dev-install channel — "no copy; referenced in place") pointed at a real on-disk package
//! root with a real `cyrup.toml`, and settings-override application runs against a real,
//! populated `SubagentSettings`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use cyrup_core::PackageId;
use crate::discovery::types::{
    AgentOverrideConfig, AgentReadScope, AgentSource, LayeredOverrideSettings, OverrideField,
    SubagentSettings, ToolRef,
};
use crate::discovery::{AgentDiscoveryConfig, discover_agents, discover_agents_all};
use cyrup_resources::package::source::PackageSource;
use cyrup_resources::{InstallScope, InstalledPackage, InstalledPackages};

fn write_agent(dir: &Path, file_name: &str, frontmatter_extra: &str, name: &str, description: &str) {
    std::fs::create_dir_all(dir).expect("mkdir agent dir");
    std::fs::write(
        dir.join(file_name),
        format!(
            "---\nname: {name}\ndescription: {description}\n{frontmatter_extra}---\n\nYou are {name}.\n"
        ),
    )
    .expect("write agent fixture file");
}

fn write_package_manifest(pkg_root: &Path, package_name: &str) {
    std::fs::create_dir_all(pkg_root).expect("mkdir package root");
    std::fs::write(
        pkg_root.join("cyrup.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\n\n[resources]\nagents = [\"./agents\"]\n"
        ),
    )
    .expect("write cyrup.toml");
}

/// Build a real `InstalledPackages` list with a single `PackageSource::Path`-installed package
/// (cyrup-resources' local-dev-install channel: "no copy; referenced in place", so no package
/// store/registry file machinery is needed for this test — the path IS the package root).
fn installed_packages_pointing_at(pkg_root: &Path) -> InstalledPackages {
    InstalledPackages {
        packages: vec![InstalledPackage {
            id: PackageId::from("fixture-pkg"),
            source: PackageSource::Path {
                path: pkg_root.to_path_buf(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    }
}

/// A full four-scope fixture tree:
///
/// ```text
/// <root>/
///   builtin/
///     delegate.md         (Builtin; local name "delegate" -> name-sensitive defaults, R-SA-018)
///     shared-name.md       (Builtin; will be shadowed by every higher tier)
///   package/
///     cyrup.toml
///     agents/
///       packaged.md        (Package; package: fixture-pkg -> qualified name)
///       shared-name.md      (Package; will be shadowed by User/Project)
///   user/
///     dir-a/
///       shared-name.md      (User; scanned first)
///     dir-b/
///       shared-name.md      (User; scanned second -> last-seen-wins, R-SA-002)
///     skills/
///       some-skill/
///         SKILL.md          (R-SA-007: must NEVER be discovered as an agent)
///     scoped-user-only.md
///   project/
///     shared-name.md        (Project; wins the whole four-tier merge, R-SA-001)
///     disabled-agent.md     (Project; disabled via a project SETTINGS override — visibility split,
///                            R-SA-013. A file-level `disabled:` is an unknown extra field, not the
///                            honored flag — only settings disable an agent.)
///     nested-fanout.md      (Project; tools includes literal "subagent" -> R-SA-016 eligible)
///     mcp-only.md            (Project; tools is only "mcp:subagent" -> R-SA-016 NOT eligible)
///     depth-limited.md       (Project; maxSubagentDepth: 1 -> feeds R-SA-022's tightening input)
///     skill-associated.md    (Project; skills: research, writing -> R-SA-017 pointer metadata)
///     scoped-project-only.md
/// ```
struct FourScopeFixture {
    _root: tempfile::TempDir,
    cfg: AgentDiscoveryConfig,
}

fn build_four_scope_fixture() -> FourScopeFixture {
    let root = tempfile::tempdir().expect("tempdir");
    let base = root.path();

    let builtin_dir = base.join("builtin");
    let package_dir = base.join("package");
    let user_dir_a = base.join("user").join("dir-a");
    let user_dir_b = base.join("user").join("dir-b");
    let user_skills_dir = base.join("user").join("skills").join("some-skill");
    let user_root = base.join("user");
    let project_dir = base.join("project");

    // --- Builtin tier ---
    write_agent(&builtin_dir, "delegate.md", "", "delegate", "builtin delegate persona");
    write_agent(
        &builtin_dir,
        "shared-name.md",
        "",
        "shared-name",
        "builtin shared-name (lowest precedence)",
    );

    // --- Package tier ---
    write_package_manifest(&package_dir, "fixture-pkg");
    write_agent(
        &package_dir.join("agents"),
        "packaged.md",
        "package: fixture-pkg\n",
        "packaged",
        "a packaged agent",
    );
    write_agent(
        &package_dir.join("agents"),
        "shared-name.md",
        "",
        "shared-name",
        "package shared-name (beats builtin, loses to user/project)",
    );

    // --- User tier (two dirs: last-seen-wins for the collision, R-SA-002) ---
    write_agent(
        &user_dir_a,
        "shared-name.md",
        "",
        "shared-name",
        "user dir-a shared-name (scanned first)",
    );
    write_agent(
        &user_dir_b,
        "shared-name.md",
        "",
        "shared-name",
        "user dir-b shared-name (scanned second, must win the User tier)",
    );
    write_agent(&user_root, "scoped-user-only.md", "", "scoped-user-only", "visible only under User scope");
    // R-SA-007: a skill bundle living under the SAME root being scanned for agents. Its SKILL.md
    // declares a `name`/`description` that WOULD parse as a valid agent if not excluded.
    std::fs::create_dir_all(&user_skills_dir).expect("mkdir skills bundle");
    std::fs::write(
        user_skills_dir.join("SKILL.md"),
        "---\nname: some-skill\ndescription: a bundled skill, never an agent\n---\n\nSkill body.\n",
    )
    .expect("write SKILL.md");

    // --- Project tier (wins every collision, R-SA-001) ---
    write_agent(
        &project_dir,
        "shared-name.md",
        "",
        "shared-name",
        "project shared-name (must win the whole four-tier merge)",
    );
    // Plain project agent — it is disabled below via a SETTINGS override, not a file-level
    // `disabled:` line (which pi treats as an unknown extra field, never the honored flag).
    write_agent(
        &project_dir,
        "disabled-agent.md",
        "",
        "disabled-agent",
        "a disabled project agent",
    );
    write_agent(
        &project_dir,
        "nested-fanout.md",
        "tools: read, subagent\n",
        "nested-fanout",
        "eligible for nested fanout (R-SA-016)",
    );
    write_agent(
        &project_dir,
        "mcp-only.md",
        "tools: mcp:subagent\n",
        "mcp-only",
        "NOT eligible for nested fanout despite same bare name (R-SA-016)",
    );
    write_agent(
        &project_dir,
        "depth-limited.md",
        "maxSubagentDepth: 1\n",
        "depth-limited",
        "feeds the tightening-only depth ceiling (R-SA-022 notes; enforced at spawn boundary)",
    );
    write_agent(
        &project_dir,
        "skill-associated.md",
        "skills: research, writing\ninheritSkills: true\n",
        "skill-associated",
        "carries proactive-injection skill pointers (R-SA-017)",
    );
    write_agent(
        &project_dir,
        "scoped-project-only.md",
        "",
        "scoped-project-only",
        "visible only under Project scope",
    );

    let installed = installed_packages_pointing_at(&package_dir);

    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert(
        "delegate".to_string(),
        AgentOverrideConfig {
            thinking: OverrideField::Value("high".to_string()),
            ..Default::default()
        },
    );
    // Tier 7: settings are carried unflattened, per scope. The `delegate` (builtin) override lives
    // in the user scope. The project scope disables `disabled-agent` via a settings override — pi
    // disables an agent ONLY through settings (`agentOverrides.<name>.disabled` / `disableBuiltins`),
    // never a frontmatter `disabled:` line — so the R-SA-013 visibility split is driven from here.
    let mut project_overrides = std::collections::BTreeMap::new();
    project_overrides.insert(
        "disabled-agent".to_string(),
        AgentOverrideConfig {
            disabled: OverrideField::Value(true),
            ..Default::default()
        },
    );
    let override_settings = LayeredOverrideSettings {
        user: SubagentSettings {
            overrides,
            ..Default::default()
        },
        project: SubagentSettings {
            overrides: project_overrides,
            ..Default::default()
        },
        user_settings_path: PathBuf::from("/user/settings.json"),
        project_settings_path: Some(PathBuf::from("/proj/settings.json")),
    };

    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(builtin_dir),
        installed_packages: installed,
        trusted_project: true,
        global_dir: base.join("global"),
        project_root: Some(base.join("project-root-unused-by-path-source")),
        user_agent_dirs: vec![user_dir_a, user_dir_b, user_root],
        user_chain_dirs: Vec::new(),
        project_agent_dirs: vec![project_dir],
        project_chain_dirs: Vec::new(),
        override_settings,
    };

    FourScopeFixture { _root: root, cfg }
}

// -------------------------------------------------------------------------------------------
// R-SA-001/002: four-tier precedence + asymmetric dedup, exercised end to end
// -------------------------------------------------------------------------------------------

#[test]
fn project_tier_wins_the_full_four_tier_collision_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");

    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::Project);
    assert_eq!(
        winner.description,
        "project shared-name (must win the whole four-tier merge)"
    );
}

#[test]
fn user_tier_second_directory_wins_over_first_last_seen_wins() {
    let mut fixture = build_four_scope_fixture();
    // Remove the project-tier collision so the User tier's own internal winner is directly
    // observable at the top level (otherwise Project would mask it, as proven by the previous
    // test) — this isolates R-SA-002's User-tier last-seen-wins rule end to end.
    fixture.cfg.project_agent_dirs.clear();

    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::User);
    assert_eq!(
        winner.description,
        "user dir-b shared-name (scanned second, must win the User tier)"
    );
}

#[test]
fn package_tier_beats_builtin_but_loses_to_user_and_project() {
    let mut fixture = build_four_scope_fixture();
    fixture.cfg.project_agent_dirs.clear();
    fixture.cfg.user_agent_dirs.clear();

    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::Package);
}

#[test]
fn packaged_agent_gets_the_qualified_runtime_name_from_its_own_frontmatter() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let packaged = result
        .agents
        .iter()
        .find(|a| a.local_name == "packaged")
        .expect("packaged agent present");
    assert_eq!(packaged.name, "fixture-pkg.packaged");
    assert_eq!(packaged.package_name, Some("fixture-pkg".to_string()));
    assert_eq!(packaged.source, AgentSource::Package);
}

// -------------------------------------------------------------------------------------------
// R-SA-007: legacy skill-path exclusion, exercised end to end (not just at the walk-fn level)
// -------------------------------------------------------------------------------------------

#[test]
fn skill_bundle_under_a_scanned_user_root_never_becomes_an_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(
        !result.agents.iter().any(|a| a.name == "some-skill"),
        "SKILL.md content must never surface as a discovered agent"
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-013: management view includes disabled; delegation view excludes them
// -------------------------------------------------------------------------------------------

#[test]
fn management_view_includes_disabled_project_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(result.agents.iter().any(|a| a.name == "disabled-agent"));
}

#[test]
fn delegation_view_excludes_disabled_project_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents(&fixture.cfg, None).expect("discovery succeeds");
    assert!(!result.agents.iter().any(|a| a.name == "disabled-agent"));
}

// -------------------------------------------------------------------------------------------
// R-SA-016: nested-fanout eligibility is exact-name, surfaced end to end via a real discovered
// AgentDefinition (canonical enforcement lives at the spawn boundary; this only proves the
// discovery-time data the spawn boundary will consult is itself correct).
// -------------------------------------------------------------------------------------------

#[test]
fn nested_fanout_eligibility_is_exact_name_not_fuzzy_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");

    let eligible = result
        .agents
        .iter()
        .find(|a| a.name == "nested-fanout")
        .expect("nested-fanout agent present");
    assert!(eligible.is_nested_fanout_eligible());

    let not_eligible = result
        .agents
        .iter()
        .find(|a| a.name == "mcp-only")
        .expect("mcp-only agent present");
    assert!(
        !not_eligible.is_nested_fanout_eligible(),
        "an mcp:-prefixed entry sharing the bare name \"subagent\" must not count (R-SA-016/DI-SA-7)"
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-017: skill association round-trips as proactive-injection metadata (name + inheritSkills),
// not any form of on-demand content loading.
// -------------------------------------------------------------------------------------------

#[test]
fn skill_association_frontmatter_round_trips_as_pointer_metadata() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let agent = result
        .agents
        .iter()
        .find(|a| a.name == "skill-associated")
        .expect("skill-associated agent present");
    assert_eq!(agent.skills, vec!["research".to_string(), "writing".to_string()]);
    assert!(agent.inherit_skills);
}

// -------------------------------------------------------------------------------------------
// R-SA-022 (discovery-layer note): max_subagent_depth is faithfully parsed and carried through
// discovery as the frontmatter field the spawn boundary's tightening-only algorithm consumes —
// this test does not re-implement or assert that algorithm itself (owned by spawn/depth.rs).
// -------------------------------------------------------------------------------------------

#[test]
fn max_subagent_depth_frontmatter_field_is_carried_through_discovery() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let agent = result
        .agents
        .iter()
        .find(|a| a.name == "depth-limited")
        .expect("depth-limited agent present");
    assert_eq!(agent.max_subagent_depth, Some(1));
}

// -------------------------------------------------------------------------------------------
// R-SA-018: name-sensitive defaults for a Builtin-sourced "delegate" persona, surfaced through
// the real Builtin scan path (not a synthetic AgentDefinition).
// -------------------------------------------------------------------------------------------

#[test]
fn builtin_delegate_persona_gets_name_sensitive_defaults_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("builtin delegate present");
    assert_eq!(delegate.source, AgentSource::Builtin);
    assert!(delegate.inherit_project_context);
}

// -------------------------------------------------------------------------------------------
// R-SA-010/011/012: settings-based override application flows through discover_agents_all
// end to end (builtin full-replace branch: overriding "delegate"'s thinking level).
// -------------------------------------------------------------------------------------------

#[test]
fn builtin_override_applies_end_to_end_through_full_discovery_pipeline() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("builtin delegate present");
    assert_eq!(delegate.thinking, Some("high".to_string()));
    assert!(delegate.override_info.is_some());
}

// -------------------------------------------------------------------------------------------
// AgentReadScope narrowing over the full four-scope fixture.
// -------------------------------------------------------------------------------------------

#[test]
fn read_scope_project_excludes_user_only_agent_but_keeps_builtin_and_package() {
    let fixture = build_four_scope_fixture();
    let result =
        discover_agents(&fixture.cfg, Some(AgentReadScope::Project)).expect("discovery succeeds");
    let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();

    assert!(!names.contains(&"scoped-user-only"));
    assert!(names.contains(&"scoped-project-only"));
    // Builtin/Package agents are orthogonal to the User/Project scope axis and remain visible.
    assert!(names.contains(&"delegate"));
    assert!(names.contains(&"fixture-pkg.packaged"));
}

#[test]
fn read_scope_user_excludes_project_only_agent_but_keeps_builtin_and_package() {
    let fixture = build_four_scope_fixture();
    let result =
        discover_agents(&fixture.cfg, Some(AgentReadScope::User)).expect("discovery succeeds");
    let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();

    assert!(names.contains(&"scoped-user-only"));
    assert!(!names.contains(&"scoped-project-only"));
    assert!(names.contains(&"delegate"));
    assert!(names.contains(&"fixture-pkg.packaged"));
}

// -------------------------------------------------------------------------------------------
// R-SA-019: re-scanned per call — a filesystem change between two discover_agents_all calls is
// observed by the second call without any explicit cache invalidation.
// -------------------------------------------------------------------------------------------

#[test]
fn discovery_observes_filesystem_changes_between_calls_over_the_full_fixture() {
    let fixture = build_four_scope_fixture();
    let before = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(!before.agents.iter().any(|a| a.name == "brand-new"));

    let project_dir = fixture
        .cfg
        .project_agent_dirs
        .first()
        .expect("project dir configured");
    write_agent(project_dir, "brand-new.md", "", "brand-new", "added after first call");

    let after = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(after.agents.iter().any(|a| a.name == "brand-new"));
}

// -------------------------------------------------------------------------------------------
// R-SA-003: extra agent directories via environment, exercised against the real config type
// (the pure closure-injected core is unit-tested in discovery/mod.rs itself; this integration
// test additionally proves `with_env_extras` composes correctly with a full discovery pass).
// -------------------------------------------------------------------------------------------

#[test]
fn with_env_extras_is_a_no_op_when_the_env_var_is_unset_in_the_real_process_environment() {
    // This test intentionally does NOT set the env var (this crate forbids unsafe code and never
    // calls std::env::set_var) — it only proves that calling `with_env_extras()` against whatever
    // the real test-process environment happens to be does not corrupt an otherwise-valid
    // discovery config, by re-running discovery afterward and getting the same agent set back.
    let fixture = build_four_scope_fixture();
    let user_dirs_before = fixture.cfg.user_agent_dirs.clone();

    let cfg_with_extras = AgentDiscoveryConfig {
        user_agent_dirs: user_dirs_before.clone(),
        ..fixture.cfg.clone()
    }
    .with_env_extras();

    // Whatever CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is (or is not) set to in this test process, the
    // ordinary configured dirs must still be present and in their original relative order as a
    // prefix — with_env_extras only ever appends.
    assert_eq!(
        &cfg_with_extras.user_agent_dirs[..user_dirs_before.len()],
        user_dirs_before.as_slice()
    );

    let result = discover_agents_all(&cfg_with_extras).expect("discovery still succeeds");
    assert!(result.agents.iter().any(|a| a.name == "scoped-user-only"));
}

// -------------------------------------------------------------------------------------------
// Sanity: default config with nothing configured discovers nothing and does not error.
// -------------------------------------------------------------------------------------------

#[test]
fn empty_config_discovers_nothing_without_erroring() {
    let cfg = AgentDiscoveryConfig::default();
    let all = discover_agents_all(&cfg).expect("empty discovery succeeds");
    assert!(all.agents.is_empty());
    assert!(all.chains.is_empty());
    assert!(all.diagnostics.is_empty());

    let delegation = discover_agents(&cfg, None).expect("empty discovery succeeds");
    assert!(delegation.agents.is_empty());
}

// Sanity: PathBuf import is exercised by the fixture builder; referenced here only to keep this
// test module's imports honest against unused-import lint drift.
#[test]
fn path_buf_import_is_reachable() {
    let _ = PathBuf::from("/tmp");
}

// -------------------------------------------------------------------------------------------
// R-SA-132/134: the 6 bundled builtin personas (scout, delegate, researcher, reviewer, worker,
// oracle) are real, discoverable resources — not hardcoded Rust
// strings — under this crate's own `resources/agents/` directory (the exact path
// `extension.rs::builtin_agents_dir()` resolves at runtime via `CARGO_MANIFEST_DIR`, mirrored
// here so this test exercises the SAME on-disk resource root the real extension uses, not a
// synthetic fixture standing in for it). Resolved through the real `AgentDiscoveryConfig::
// builtin_agents_dir` -> `scan_builtin_agents` -> `cyrup_resources::resolve_manifest` auto-
// discovery pipeline (R-SA-020), end to end via `discover_agents_all`.
// -------------------------------------------------------------------------------------------

/// The 6 bundled builtin persona runtime names this crate ships, mirroring `pi-subagents/agents/
/// {delegate,oracle,researcher,reviewer,scout,worker}.md` @ v0.43.0 (`git ls-tree v0.43.0 agents/`
/// lists exactly those six). Each `.md` file's frontmatter `name:` is unqualified (no `package:`
/// field), so per R-SA-008 the runtime name is exactly the local name.
///
/// `planner`/`context-builder` were deleted upstream in `83b9872` ("fix: remove stale bundled
/// roles"). `advisor` is in `BUILTIN_AGENT_NAMES` but ships NO file of its own — upstream `34a018f`
/// made it an `oracle` ALIAS (`agents/oracle.md:3`), so it is resolved by
/// `discovery::resolve_agent_name`, not discovered as a seventh persona.
const BUILTIN_PERSONA_NAMES: &[&str] = &[
    "delegate",
    "oracle",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

/// Resolve this crate's bundled `resources/` directory the exact same way
/// `extension.rs::builtin_agents_dir()` does in production: `CARGO_MANIFEST_DIR`-relative,
/// baked in at compile time. Kept as its own tiny helper (rather than importing a private
/// `extension.rs` function into this black-box integration test) so this test proves the
/// resource files are genuinely present and parseable at the conventional path independent of
/// `extension.rs`'s own internal wiring.
fn bundled_resources_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
}

/// SUBA-044 — pi made the reviewer lane READ-ONLY in `0b1976b` ("fix: make reviewer lanes
/// read-only by default", #1008), released v0.47.1: `agents/reviewer.md:4` @v0.47.1 is
/// `tools: read, grep, find, ls, intercom` and `defaultReads` is gone.
///
/// THE USER ACTION: a user delegates to the SHIPPED `reviewer` expecting pi's behaviour and gets a
/// lane that can run arbitrary shell and write arbitrary files during what upstream defines as an
/// inspection-only pass. cyrup's copy still granted `bash`, `edit` and `write`, with a prose-only
/// restraint ("Use `bash` only for read-only inspection…") over a grant that permits neither
/// restriction.
///
/// Asserted through the REAL discovery pipeline rather than by reading the file, so a `tools:` line
/// that parses to something other than what it looks like still fails.
#[test]
fn the_bundled_reviewer_lane_is_read_only() {
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let result = discover_agents_all(&cfg).expect("builtin-only discovery succeeds");
    let reviewer = result
        .agents
        .iter()
        .find(|a| a.name == "reviewer")
        .expect("the bundled reviewer must be discoverable");

    let tools: Vec<String> = reviewer
        .tools
        .as_ref()
        .expect("the reviewer pins an explicit allowlist")
        .iter()
        .map(|t| match t {
            crate::discovery::types::ToolRef::Builtin(name)
            | crate::discovery::types::ToolRef::Mcp(name)
            | crate::discovery::types::ToolRef::ExtensionPath(name) => name.clone(),
        })
        .collect();
    for denied in ["bash", "edit", "write"] {
        assert!(
            !tools.iter().any(|t| t == denied),
            "upstream's reviewer lane grants no `{denied}`; got {tools:?}"
        );
    }
    assert_eq!(
        tools,
        vec![
            "read".to_string(),
            "grep".to_string(),
            "find".to_string(),
            "ls".to_string(),
            "intercom".to_string()
        ],
        "the allowlist must be upstream's, in upstream's order"
    );
    assert!(
        reviewer.default_reads.is_none(),
        "upstream removed `defaultReads` from the reviewer in the same commit; got {:?}",
        reviewer.default_reads
    );
}

#[test]
fn all_six_bundled_builtin_personas_are_discovered_with_builtin_source() {
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };

    let result = discover_agents_all(&cfg).expect("builtin-only discovery succeeds");

    for expected_name in BUILTIN_PERSONA_NAMES {
        let found = result
            .agents
            .iter()
            .find(|a| a.name == *expected_name)
            .unwrap_or_else(|| {
                panic!(
                    "expected bundled builtin persona '{expected_name}' to be discovered; \
                     discovered names were: {:?}",
                    result.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            found.source,
            AgentSource::Builtin,
            "bundled persona '{expected_name}' must be discovered at AgentSource::Builtin precedence"
        );
        assert_eq!(
            found.local_name, *expected_name,
            "bundled persona '{expected_name}' must be unqualified (no package prefix)"
        );
    }

    assert_eq!(
        result.agents.len(),
        BUILTIN_PERSONA_NAMES.len(),
        "exactly the 6 bundled personas should be discovered from the builtin resource root, \
         found: {:?}",
        result.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn bundled_builtin_personas_are_visible_through_the_delegation_view_too() {
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };

    let result = discover_agents(&cfg, None).expect("builtin-only delegation discovery succeeds");
    for expected_name in BUILTIN_PERSONA_NAMES {
        assert!(
            result.agents.iter().any(|a| a.name == *expected_name),
            "delegation view must include bundled builtin persona '{expected_name}'"
        );
    }
}

/// The bare tool-name string a [`ToolRef`] carries, regardless of variant — a bare frontmatter
/// tool name (no `mcp:` prefix) parses to [`ToolRef::Builtin`] (`from_tool_string`), but this test
/// treats every variant uniformly so it asserts on the resolved name in all cases.
fn tool_ref_name(tool: &ToolRef) -> &str {
    match tool {
        ToolRef::Builtin(name) | ToolRef::Mcp(name) | ToolRef::ExtensionPath(name) => name.as_str(),
    }
}

/// T8 (16-cyrup-ext-subagents.md moderate row + remediation Tier 8): the bundled `researcher`
/// persona must name only tools that ACTUALLY EXIST for a subagent child in cyrup. The earlier
/// byte-for-byte port carried pi's `web_search`/`fetch_content`/`get_search_content` verbatim —
/// none of which cyrup registers (cyrup-tools registers exactly `bash/edit/find/grep/ls/read/write`)
/// — so a discovered researcher advertised three phantom tools to its child. This pins that the
/// persona's resolved `tools` allowlist references only real tool names and NONE of the three
/// phantom web-tool names.
#[test]
fn bundled_researcher_persona_names_only_real_cyrup_tools() {
    // The tool names that genuinely exist for a subagent child: the registered `cyrup-tools` file
    // tools, plus the subagent coordination tools the crate's own completion-guard read-only set
    // recognizes (`intercom`/`contact_supervisor`) and that the other bundled personas reference.
    const REAL_TOOL_NAMES: &[&str] = &[
        "read",
        "write",
        "edit",
        "grep",
        "find",
        "ls",
        "bash",
        "intercom",
        "contact_supervisor",
    ];
    // The phantom pi web tools cyrup does not register — must not appear in any bundled persona.
    const PHANTOM_TOOL_NAMES: &[&str] = &["web_search", "fetch_content", "get_search_content"];

    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let result = discover_agents_all(&cfg).expect("builtin-only discovery succeeds");
    let researcher = result
        .agents
        .iter()
        .find(|a| a.name == "researcher")
        .expect("researcher persona must be discovered");
    let tools = researcher
        .tools
        .as_ref()
        .expect("researcher must declare a tools allowlist");
    assert!(!tools.is_empty(), "researcher must declare at least one tool");

    for tool in tools {
        let name = tool_ref_name(tool);
        assert!(
            !PHANTOM_TOOL_NAMES.contains(&name),
            "researcher must not reference the phantom (unregistered) tool '{name}'"
        );
        assert!(
            REAL_TOOL_NAMES.contains(&name),
            "researcher references '{name}', which is not a real cyrup tool; \
             real tools are {REAL_TOOL_NAMES:?}"
        );
    }
}

#[test]
fn delegate_persona_keeps_its_name_sensitive_defaults_when_loaded_from_the_bundled_resource_root() {
    // R-SA-018: `delegate`'s `systemPromptMode`/`inheritProjectContext` name-sensitive defaults
    // must still apply when parsed from the real bundled file (not just a synthetic fixture) —
    // the frontmatter itself declares them explicitly (systemPromptMode: append), so this also
    // pins that the real file's frontmatter was ported byte-for-byte rather than paraphrased.
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let result = discover_agents_all(&cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("bundled delegate.md must be discovered");
    assert_eq!(
        delegate.system_prompt_mode,
        crate::discovery::types::SystemPromptMode::Append
    );
    assert!(delegate.inherit_project_context);
}

// ============================================================================================
// CROSS-CUTTING (batch 9): G97 (agent aliases) x G99 (the re-tiered builtin roster)
// ============================================================================================

/// G97 added aliases and G99 changed the roster underneath them, in the same batch. Both feed the
/// SAME resolver, and their interaction is invisible to either group's own tests: G97's alias tests
/// use hand-built `AgentDefinition`s, and G99's roster test walks names without ever resolving one.
///
/// The collision this pins is not hypothetical. G99 left `advisor` in `BUILTIN_AGENT_NAMES`
/// (`agents.ts:38-46` @v0.43.0) while deleting `agents/advisor.md`, because upstream `34a018f`
/// demoted it to an ALIAS on `oracle` (`agents/oracle.md:3`). So one entry of the roster is
/// resolvable ONLY through the machinery G97 introduced — if aliases regress, or if a future roster
/// edit reinstates an `advisor` file, the name either stops resolving or resolves ambiguously, and
/// nothing else in the suite would notice.
///
/// Asserted against the REAL bundled `resources/agents/`, not a fixture, so a stray alias added to
/// a shipped persona is caught here.
#[test]
fn no_bundled_alias_can_collide_with_a_builtin_name_or_another_alias() {
    use crate::discovery::{AgentNameResolution, resolve_agent_name};

    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let agents = discover_agents_all(&cfg)
        .expect("builtin-only discovery succeeds")
        .agents;

    // 1. No alias may equal any agent's NAME. Stage 2 of `resolveAgentName` (`agents.ts:513-524`)
    //    matches names before aliases and never falls through when it hits, so such an alias would
    //    be permanently dead — silently unreachable rather than an error.
    for agent in &agents {
        for alias in &agent.aliases {
            assert!(
                !agents.iter().any(|other| other.name == *alias),
                "alias '{alias}' on '{}' is shadowed forever by the agent actually named '{alias}'",
                agent.name
            );
        }
    }

    // 2. No two agents may claim the same alias — that is `Ambiguous agent alias` (`agents.ts:526`),
    //    which fails the run rather than picking a winner.
    let mut claimed: Vec<(&str, &str)> = Vec::new();
    for agent in &agents {
        for alias in &agent.aliases {
            if let Some((other, _)) = claimed.iter().find(|(a, _)| *a == alias.as_str()) {
                panic!("alias '{alias}' is claimed by both '{other}' and '{}'", agent.name);
            }
            claimed.push((alias.as_str(), agent.name.as_str()));
        }
    }

    // 3. EVERY name the roster advertises must resolve to exactly one agent — including `advisor`,
    //    which ships no file and is reachable only as `oracle`'s alias.
    for name in crate::discovery::management::BUILTIN_AGENT_NAMES {
        match resolve_agent_name(name, &agents) {
            AgentNameResolution::Found(agent) => {
                if name == "advisor" {
                    assert_eq!(
                        agent.name, "oracle",
                        "`advisor` ships no file of its own; it must resolve through oracle's alias"
                    );
                } else {
                    assert_eq!(agent.name, name, "roster name '{name}' must resolve to itself");
                }
            }
            AgentNameResolution::NotFound => {
                panic!("roster advertises '{name}' but it resolves to nothing")
            }
            AgentNameResolution::Ambiguous(msg) => {
                panic!("roster name '{name}' is ambiguous: {msg}")
            }
        }
    }

    // 4. The roles G99 removed must not resolve by name OR by any surviving alias — a deleted role
    //    that is still reachable is exactly the stale-roster state `83b9872` set out to end.
    for gone in ["planner", "context-builder"] {
        assert!(
            matches!(resolve_agent_name(gone, &agents), AgentNameResolution::NotFound),
            "removed role '{gone}' must not resolve at all"
        );
    }
}

/// The other half of the same seam: a real agent named `x` must beat another agent's alias `x`
/// (`agents.ts:513-524` — stage 3 never runs when stage 2 matched), and a SECOND claimant of a
/// shipped alias must produce the ambiguity refusal rather than a silent pick.
///
/// Driven against the real bundled roster plus one project-scope agent, because that is how the
/// collision actually arises: a user adds an agent whose name or alias happens to be one the
/// shipped personas already use.
#[test]
fn a_user_agent_named_after_a_bundled_alias_wins_and_a_second_claimant_is_refused() {
    use crate::discovery::{AgentNameResolution, resolve_agent_name};

    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let mut agents = discover_agents_all(&cfg)
        .expect("builtin-only discovery succeeds")
        .agents;

    // Baseline: `advisor` is oracle's alias today.
    assert_eq!(
        resolve_agent_name("advisor", &agents)
            .agent()
            .expect("advisor resolves")
            .name,
        "oracle"
    );

    // A project agent literally NAMED `advisor` takes the name back — the alias never shadows it.
    let mut named_advisor = agents
        .iter()
        .find(|a| a.name == "scout")
        .expect("scout is bundled")
        .clone();
    named_advisor.name = "advisor".to_string();
    named_advisor.local_name = "advisor".to_string();
    named_advisor.aliases = Vec::new();
    agents.push(named_advisor);

    assert_eq!(
        resolve_agent_name("advisor", &agents)
            .agent()
            .expect("the real `advisor` agent resolves")
            .name,
        "advisor",
        "an agent NAMED advisor must beat oracle's ALIAS advisor"
    );

    // And a second agent claiming the same alias is refused, not silently won.
    let mut second_claimant = agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("delegate is bundled")
        .clone();
    second_claimant.name = "seer".to_string();
    second_claimant.local_name = "seer".to_string();
    second_claimant.aliases = vec!["sage".to_string()];
    let mut oracle_also_sage = agents
        .iter()
        .find(|a| a.name == "oracle")
        .expect("oracle is bundled")
        .clone();
    oracle_also_sage.aliases = vec!["sage".to_string()];
    agents.retain(|a| a.name != "oracle");
    agents.push(oracle_also_sage);
    agents.push(second_claimant);

    match resolve_agent_name("sage", &agents) {
        AgentNameResolution::Ambiguous(msg) => {
            assert!(
                msg.starts_with("Ambiguous agent alias 'sage'"),
                "must be the ALIAS ambiguity message, got: {msg}"
            );
        }
        other => panic!("two claimants of alias 'sage' must be refused, got {other:?}"),
    }
}
